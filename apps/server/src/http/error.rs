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
            Self::CsrfFailed => StatusCode::FORBIDDEN,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::DependencyUnavailable => StatusCode::SERVICE_UNAVAILABLE,
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
