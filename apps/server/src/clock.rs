//! Injectable clock so session expiry/rate-limit logic is deterministically
//! testable without wall-clock sleeps (STEP-002 §39/§42).

use std::sync::Mutex;
use time::OffsetDateTime;

pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> OffsetDateTime;
}

/// Production clock: real system time (server-side authority, ADR-002).
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

/// Test clock with a mutable, controllable instant.
pub struct MutableClock {
    instant: Mutex<OffsetDateTime>,
}

impl MutableClock {
    pub fn new(initial: OffsetDateTime) -> Self {
        Self {
            instant: Mutex::new(initial),
        }
    }

    pub fn set(&self, instant: OffsetDateTime) {
        *self.instant.lock().expect("clock lock poisoned") = instant;
    }

    pub fn advance_seconds(&self, seconds: i64) {
        let mut instant = self.instant.lock().expect("clock lock poisoned");
        *instant += time::Duration::seconds(seconds);
    }
}

impl Clock for MutableClock {
    fn now(&self) -> OffsetDateTime {
        *self.instant.lock().expect("clock lock poisoned")
    }
}
