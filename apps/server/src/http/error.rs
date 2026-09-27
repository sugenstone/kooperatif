//! Error handling foundation (docs/21-API-CONTRACTS.md).
//!
//! Errors cross the API boundary as stable machine codes with
//! localizable context decided by the frontend — never raw Rust or
//! database errors.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Infrastructure-level error codes of STEP-001. Domain categories
/// (validation, forbidden, conflict, ...) join in their own steps.
#[derive(Debug)]
pub enum ApiError {
    NotFound,
    Internal,
}

impl ApiError {
    fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Internal => "internal_error",
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
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
