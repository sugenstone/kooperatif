//! Session cookie handling (STEP-002 §12).
//!
//! One dedicated project-owned cookie: `kooperatif_session`. It carries
//! ONLY the raw opaque token — never authorization state. Attributes:
//! HttpOnly; Path=/; SameSite=Lax (extra CSRF layer, never the sole
//! defense); Secure in production (environment-aware, fail-safe).
//! No localStorage/sessionStorage anywhere.

pub const SESSION_COOKIE_NAME: &str = "kooperatif_session";

/// Build the Set-Cookie header value for a new/refreshed session cookie.
pub fn build_session_cookie(token: &str, secure: bool, max_age_secs: i64) -> String {
    format!(
        "{SESSION_COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_secs}{}",
        if secure { "; Secure" } else { "" }
    )
}

/// Build the Set-Cookie header value that clears the cookie on logout.
pub fn build_clearing_cookie(secure: bool) -> String {
    format!(
        "{SESSION_COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
        if secure { "; Secure" } else { "" }
    )
}

/// Extract the raw session token from a Cookie header (split-tolerant).
pub fn extract_session_token(headers: &axum::http::HeaderMap) -> Option<String> {
    for value in headers.get_all(axum::http::header::COOKIE) {
        let raw = value.to_str().ok()?;
        for pair in raw.split(';') {
            let mut parts = pair.trim().splitn(2, '=');
            if parts.next()? == SESSION_COOKIE_NAME {
                if let Some(token) = parts.next() {
                    if !token.is_empty() {
                        return Some(token.to_string());
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn session_cookie_carries_required_security_attributes() {
        let cookie = build_session_cookie("tok", true, 1800);
        assert!(cookie.contains("kooperatif_session=tok"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        assert!(cookie.contains("Path=/"));
        assert!(cookie.contains("Secure"));
        assert!(cookie.contains("Max-Age=1800"));

        let dev = build_session_cookie("tok", false, 1800);
        assert!(!dev.contains("Secure"), "dev over plain localhost");
    }

    #[test]
    fn clearing_cookie_expires_immediately() {
        let cookie = build_clearing_cookie(true);
        assert!(cookie.contains("Max-Age=0"));
        assert!(cookie.contains("HttpOnly"));
    }

    #[test]
    fn token_extraction_reads_our_cookie_only() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            "other=x; kooperatif_session=abc123; third=y"
                .parse()
                .expect("valid header"),
        );
        assert_eq!(extract_session_token(&headers).as_deref(), Some("abc123"));

        let mut empty = HeaderMap::new();
        empty.insert(
            axum::http::header::COOKIE,
            "other=x".parse().expect("valid"),
        );
        assert_eq!(extract_session_token(&empty), None);
        assert_eq!(extract_session_token(&HeaderMap::new()), None);
    }
}
