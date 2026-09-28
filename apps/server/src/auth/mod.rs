//! Authentication and session subsystem (ADR-002, STEP-002).
//!
//! Infrastructure layer of the modular monolith (ADR-001). Scope:
//! internal Users, Argon2id credentials, server-side revocable sessions,
//! CSRF protection, login rate limiting and durable security events.
//!
//! NOT in scope (later STEPs): RBAC/roles/permissions, Shareholder login
//! (Shareholders are records, never Users), password reset email, 2FA,
//! passkeys, OAuth/SSO, invitation or public registration flows.

pub mod audit;
pub mod authz;
pub mod cookies;
pub mod csrf;
pub mod extractor;
pub mod identity;
pub mod limiter;
pub mod rbac;
pub mod routes;
pub mod session;
pub mod token;
pub mod users;

use std::sync::Arc;

use crate::clock::Clock;
use crate::config::AuthConfig;

/// Shared authentication runtime: configuration + stateful components,
/// assembled once at startup and referenced by the router.
pub struct AuthRuntime {
    pub config: AuthConfig,
    pub clock: Arc<dyn Clock>,
    pub limiter: Arc<limiter::LoginRateLimiter>,
    /// Pre-parsed Argon2id context (parameters validated at startup).
    pub argon2: Arc<argon2::Argon2<'static>>,
}

impl AuthRuntime {
    pub fn new(config: AuthConfig, clock: Arc<dyn Clock>) -> Self {
        let limiter = Arc::new(limiter::LoginRateLimiter::new(
            config.login_window_secs,
            config.login_username_max_attempts,
            config.login_ip_max_attempts,
            clock.clone(),
        ));
        let argon2 = Arc::new(argon2::Argon2::new(
            argon2::Algorithm::Argon2id,
            argon2::Version::V0x13,
            argon2::Params::new(
                config.argon2_m_cost,
                config.argon2_t_cost,
                config.argon2_p_cost,
                None,
            )
            .expect("argon2 parameters are validated at config load"),
        ));
        Self {
            config,
            clock,
            limiter,
            argon2,
        }
    }
}
