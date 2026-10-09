//! Error handling foundation (docs/21-API-CONTRACTS.md).
//!
//! Errors cross the API boundary as stable machine codes with
//! localizable context decided by the frontend — never raw Rust or
//! database errors. Codes align 1:1 with `@kooperatif/contracts`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Machine-readable error codes (docs/21-API-CONTRACTS.md categories).
#[derive(Debug)]
pub enum ApiError {
    NotFound,
    Internal,
    /// 400 — malformed/invalid request payloads (STEP-002: login body,
    /// path parameters).
    ValidationFailed,
    /// 401 — no valid authentication presented.
    AuthenticationRequired,
    /// 401 — was authenticated once; session no longer valid.
    SessionExpired,
    /// 401 — credentials presented and rejected (login). Generic
    /// externally; never reveals whether the username exists.
    AuthenticationFailed,
    /// 403 — CSRF origin/token validation failed.
    CsrfFailed,
    /// 429 — login rate limit engaged.
    RateLimited,
    /// 503 — a required dependency (PostgreSQL) is unavailable.
    DependencyUnavailable,
    /// 403 — authenticated but lacking the required permission
    /// (STEP-003 §19: never collapse into 401).
    PermissionDenied,
    /// 409 — unique/normalized-identity conflict (e.g. duplicate role
    /// name).
    Conflict,
    /// 409 — mutation rejected by the last-administration-path guard
    /// (STEP-003 §28).
    LockoutPrevented,
    /// 409 — ordinary outflow rejected: the account holds enough
    /// physical cash but the operation would consume the portion
    /// reserved for Social Aid funds (PILOT-FIX-001 hard
    /// reservation). Distinct code so the UI can explain WHY the
    /// spendable balance is lower than the physical balance.
    InsufficientUnrestrictedFunds,
    /// 409 — optimistic-concurrency precondition failed: the record
    /// changed after the caller loaded it (docs/21 stale-state).
    StaleState,
}

impl ApiError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Internal => "internal_error",
            Self::ValidationFailed => "validation_failed",
            Self::AuthenticationRequired => "authentication_required",
            Self::SessionExpired => "session_expired",
            Self::AuthenticationFailed => "authentication_failed",
            Self::CsrfFailed => "csrf_failed",
            Self::RateLimited => "rate_limited",
            Self::DependencyUnavailable => "dependency_unavailable",
            Self::PermissionDenied => "permission_denied",
            Self::Conflict => "conflict",
            Self::LockoutPrevented => "lockout_prevented",
            Self::InsufficientUnrestrictedFunds => "insufficient_unrestricted_funds",
            Self::StaleState => "stale_state",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            Self::ValidationFailed => StatusCode::BAD_REQUEST,
            Self::AuthenticationRequired | Self::SessionExpired | Self::AuthenticationFailed => {
                StatusCode::UNAUTHORIZED
            }
            Self::CsrfFailed | Self::PermissionDenied => StatusCode::FORBIDDEN,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::DependencyUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::Conflict
            | Self::LockoutPrevented
            | Self::InsufficientUnrestrictedFunds
            | Self::StaleState => StatusCode::CONFLICT,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status(),
            Json(json!({ "error": { "code": self.code() } })),
        )
            .into_response()
    }
}

/// Fallback handler for unmatched routes.
pub async fn fallback() -> ApiError {
    ApiError::NotFound
}
