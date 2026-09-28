//! Durable security/audit events (STEP-002 §25).
//!
//! Boundary (ADR-011): these rows are AUDIT records — durable business
//! history. Tracing output is observability and may be lost. Never
//! stored here: passwords, raw session tokens, cookie values, CSRF
//! secrets, password hashes (docs/19, docs/22).

use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Copy)]
pub enum SecurityEventType {
    LoginSucceeded,
    LoginFailed,
    Logout,
    SessionRevoked,
    SessionsRevokedAllOthers,
    UserCreated,
}

impl SecurityEventType {
    fn as_str(self) -> &'static str {
        match self {
            Self::LoginSucceeded => "login_succeeded",
            Self::LoginFailed => "login_failed",
            Self::Logout => "logout",
            Self::SessionRevoked => "session_revoked",
            Self::SessionsRevokedAllOthers => "sessions_revoked_all_others",
            Self::UserCreated => "user_created",
        }
    }
}

/// Record one durable security event. Failures are logged and swallowed
/// by design: an audit-write failure must not flip the security outcome
/// of an operation that already happened (and must never 500 a login).
pub async fn record(
    pool: &PgPool,
    event_type: SecurityEventType,
    user_id: Option<Uuid>,
    session_id: Option<Uuid>,
    metadata: serde_json::Value,
) {
    let result = sqlx::query(
        "INSERT INTO security_events (event_type, user_id, session_id, metadata) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(event_type.as_str())
    .bind(user_id)
    .bind(session_id)
    .bind(metadata)
    .execute(pool)
    .await;
    if let Err(error) = result {
        tracing::error!(error = %error, event = event_type.as_str(), "security event write failed");
    }
}

/// Convenience: failed-login reason categories (safe for storage).
pub fn login_failed_metadata(normalized_username: &str, reason: &str) -> serde_json::Value {
    json!({ "username": normalized_username, "reason": reason })
}
