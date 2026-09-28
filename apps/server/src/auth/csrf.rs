//! CSRF defense (STEP-002 §23, ADR-002).
//!
//! Two independent layers on state-changing auth requests — SameSite
//! cookies are an extra hardening attribute, never the strategy:
//!
//! 1. **Origin allowlist** (all non-GET /api/auth requests, including
//!    login): the `Origin` header must be present and explicitly
//!    allowlisted (`KOOPERATIF_ALLOWED_ORIGINS`; production must
//!    configure its public origin — fail fast at startup otherwise).
//!    A cross-site attacker cannot forge Origin in a browser.
//!
//! 2. **Per-session synchronizer token** (authenticated mutations):
//!    the server generates a random CSRF token at session creation,
//!    hands it to the frontend in the login/me JSON, and requires it
//!    back in the `x-csrf-token` header. A cross-site page can neither
//!    read the token (SOP) nor attach custom headers without a CORS
//!    preflight our allowlist rejects.
//!
//! Future WebSocket (ADR-005): the same-origin upgrade carries the
//! session cookie; the Origin check applies identically at handshake.
//!
//! No custom cryptography is used anywhere.

#[derive(Debug, PartialEq, Eq)]
pub enum CsrfRejection {
    OriginHeaderMissing,
    OriginNotAllowed,
    TokenHeaderMissing,
    TokenMismatch,
}

impl CsrfRejection {
    /// Machine code reused in API error bodies.
    pub fn code(&self) -> &'static str {
        match self {
            Self::OriginHeaderMissing
            | Self::OriginNotAllowed
            | Self::TokenHeaderMissing
            | Self::TokenMismatch => "csrf_failed",
        }
    }
}

/// Layer 1: strict Origin validation for state-changing requests.
pub fn validate_origin(
    headers: &axum::http::HeaderMap,
    method: &axum::http::Method,
    allowed_origins: &[String],
) -> Result<(), CsrfRejection> {
    if method == axum::http::Method::GET || method == axum::http::Method::HEAD {
        return Ok(());
    }
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    let Some(origin) = origin else {
        return Err(CsrfRejection::OriginHeaderMissing);
    };
    if allowed_origins.iter().any(|allowed| allowed == origin) {
        Ok(())
    } else {
        Err(CsrfRejection::OriginNotAllowed)
    }
}

/// Layer 2: synchronizer-token comparison (constant-time).
pub fn validate_csrf_token(
    headers: &axum::http::HeaderMap,
    session_csrf_token: &str,
) -> Result<(), CsrfRejection> {
    let provided = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok());
    let Some(provided) = provided else {
        return Err(CsrfRejection::TokenHeaderMissing);
    };
    if constant_time_eq(provided.as_bytes(), session_csrf_token.as_bytes()) {
        Ok(())
    } else {
        Err(CsrfRejection::TokenMismatch)
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, Method};

    #[test]
    fn safe_reads_bypass_origin_validation() {
        let headers = HeaderMap::new();
        assert_eq!(validate_origin(&headers, &Method::GET, &[]), Ok(()));
        assert_eq!(validate_origin(&headers, &Method::HEAD, &[]), Ok(()));
    }

    #[test]
    fn state_changing_requests_require_allowlisted_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::ORIGIN,
            "https://kooperatif.example.com".parse().expect("valid"),
        );
        let allowed = vec!["https://kooperatif.example.com".to_string()];
        assert_eq!(validate_origin(&headers, &Method::POST, &allowed), Ok(()));

        let mut evil = HeaderMap::new();
        evil.insert(
            axum::http::header::ORIGIN,
            "https://evil.example".parse().expect("valid"),
        );
        assert_eq!(
            validate_origin(&evil, &Method::POST, &allowed),
            Err(CsrfRejection::OriginNotAllowed)
        );

        let missing = HeaderMap::new();
        assert_eq!(
            validate_origin(&missing, &Method::POST, &allowed),
            Err(CsrfRejection::OriginHeaderMissing)
        );
    }

    #[test]
    fn csrf_token_must_match_session_token() {
        let mut headers = HeaderMap::new();
        headers.insert("x-csrf-token", "dogru-token".parse().expect("valid"));
        assert_eq!(validate_csrf_token(&headers, "dogru-token"), Ok(()));
        assert_eq!(
            validate_csrf_token(&headers, "baska-token"),
            Err(CsrfRejection::TokenMismatch)
        );
        let empty = HeaderMap::new();
        assert_eq!(
            validate_csrf_token(&empty, "dogru-token"),
            Err(CsrfRejection::TokenHeaderMissing)
        );
    }
}
