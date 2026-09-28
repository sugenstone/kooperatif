//! Login rate limiting / brute-force protection (STEP-002 §18, ADR-002).
//!
//! Design decisions:
//! - In-process fixed-window limiter (documented single-node limitation
//!   of the ADR-012 initial topology; NO Redis per ADR-009/STEP rules).
//! - Two abuse dimensions checked together:
//!   * normalized username (failed attempts only),
//!   * client IP (ALL attempts, successes included — stops username
//!     spraying from one origin).
//! - Automatic recovery: windows roll; there is no permanent lockout, so
//!   an attacker cannot permanently lock someone else's account (DoS
//!   guard). A successful login clears that username's window.
//! - Clock is injected (deterministic tests, no wall-clock sleeps).
//! - Revisit trigger for scaling out: when a second API node or a future
//!   worker needs shared limiter state, move counters into PostgreSQL.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::clock::Clock;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum BucketKey {
    Username(String),
    Ip(String),
}

#[derive(Debug, Default)]
struct Window {
    window_start_unix: i64,
    failures_or_attempts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitDecision {
    pub allowed: bool,
}

pub struct LoginRateLimiter {
    window_secs: i64,
    username_max: u32,
    ip_max: u32,
    clock: std::sync::Arc<dyn Clock>,
    buckets: Mutex<HashMap<BucketKey, Window>>,
}

impl LoginRateLimiter {
    pub fn new(
        window_secs: i64,
        username_max: u32,
        ip_max: u32,
        clock: std::sync::Arc<dyn Clock>,
    ) -> Self {
        Self {
            window_secs,
            username_max,
            ip_max,
            clock,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Check before verifying credentials. When blocked, returns false
    /// WITHOUT recording (blocked attempts do not extend the window —
    /// recovery stays time-bounded).
    pub fn check(&self, normalized_username: &str, client_ip: &str) -> RateLimitDecision {
        let now = self.clock.now().unix_timestamp();
        let buckets = self.buckets.lock().expect("limiter lock poisoned");
        let username_key = BucketKey::Username(normalized_username.to_string());
        let ip_key = BucketKey::Ip(client_ip.to_string());
        let username_blocked = self
            .active_window(&buckets, &username_key, now)
            .is_some_and(|w| w.failures_or_attempts >= self.username_max);
        let ip_blocked = self
            .active_window(&buckets, &ip_key, now)
            .is_some_and(|w| w.failures_or_attempts >= self.ip_max);
        RateLimitDecision {
            allowed: !username_blocked && !ip_blocked,
        }
    }

    /// Record a FAILED login attempt on the username dimension. The IP
    /// dimension is fed by `record_ip_attempt` for EVERY attempt (it is
    /// called once per login before verification) — incrementing it here
    /// as well would double-count each failure.
    pub fn record_failure(&self, normalized_username: &str) {
        let now = self.clock.now().unix_timestamp();
        let mut buckets = self.buckets.lock().expect("limiter lock poisoned");
        self.increment(
            &mut buckets,
            BucketKey::Username(normalized_username.to_string()),
            now,
        );
    }

    /// Record an attempt from this IP (called for every login attempt,
    /// success or failure, so the IP dimension cannot be bypassed by
    /// rotating usernames).
    pub fn record_ip_attempt(&self, client_ip: &str) {
        let now = self.clock.now().unix_timestamp();
        let mut buckets = self.buckets.lock().expect("limiter lock poisoned");
        self.increment(&mut buckets, BucketKey::Ip(client_ip.to_string()), now);
    }

    /// A successful login clears the username bucket (the legitimate
    /// user recovered); the IP bucket keeps counting total attempts.
    pub fn on_success(&self, normalized_username: &str) {
        let mut buckets = self.buckets.lock().expect("limiter lock poisoned");
        buckets.remove(&BucketKey::Username(normalized_username.to_string()));
    }

    fn active_window<'a>(
        &self,
        buckets: &'a HashMap<BucketKey, Window>,
        key: &'a BucketKey,
        now_unix: i64,
    ) -> Option<&'a Window> {
        buckets
            .get(key)
            .filter(|w| now_unix.saturating_sub(w.window_start_unix) < self.window_secs)
    }

    fn increment(&self, buckets: &mut HashMap<BucketKey, Window>, key: BucketKey, now_unix: i64) {
        let window_start_unix = match buckets.get(&key) {
            Some(existing)
                if now_unix.saturating_sub(existing.window_start_unix) < self.window_secs =>
            {
                existing.window_start_unix
            }
            _ => now_unix, // new window (also performs the automatic reset)
        };
        let entry = buckets.entry(key).or_default();
        if entry.window_start_unix != window_start_unix {
            entry.failures_or_attempts = 0;
            entry.window_start_unix = window_start_unix;
        }
        entry.failures_or_attempts = entry.failures_or_attempts.saturating_add(1);
    }

    /// Drop stale windows (bounded memory); called opportunistically.
    pub fn prune(&self) {
        let now = self.clock.now().unix_timestamp();
        self.buckets
            .lock()
            .expect("limiter lock poisoned")
            .retain(|_, w| now.saturating_sub(w.window_start_unix) < self.window_secs * 2);
    }
}

/// Extract the client IP for rate limiting. Behind the ADR-012 reverse
/// proxy the rightmost `X-Forwarded-For` entry is the proxy-added client
/// address; without the proxy the socket peer address is authoritative.
/// Spoofed left entries cannot influence the key.
pub fn client_ip(
    headers: &axum::http::HeaderMap,
    peer: Option<std::net::SocketAddr>,
    forwarded: bool,
) -> String {
    if forwarded {
        if let Some(value) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            if let Some(rightmost) = value.split(',').map(str::trim).next_back() {
                if !rightmost.is_empty() {
                    return rightmost.to_string();
                }
            }
        }
    }
    peer.map(|addr| addr.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
