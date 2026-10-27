//! Authentication extractor (STEP-002 §19, §24).
//!
//! One reusable `FromRequestParts` implementation performs the entire
//! validation pipeline — cookie → token hash → session row → user row →
//! revoked/expired/idle/disabled checks → bounded touch — so protected
//! handlers never repeat security plumbing.
//!
//! It yields a `CurrentAuth` projection that deliberately contains NO
//! password hash and NO raw token: only what authorization (a later
//! STEP) and handlers need — `AuthenticatedUser`-equivalent identity
//! plus session context, per STEP-002 §24's future-authorization input.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::cookies;
use crate::auth::session::{self, SessionRejection};
use crate::auth::token;
use crate::http::error::ApiError;
use crate::http::AppState;

/// Safe per-request authentication context (no secrets, no hash).
#[derive(Debug, Clone)]
pub struct CurrentAuth {
    pub user_id: Uuid,
    pub username: String,
    pub display_name: String,
    pub session_id: Uuid,
    pub session_created_at: OffsetDateTime,
    pub session_expires_at: OffsetDateTime,
    /// Raw synchronizer token: required to answer `/api/auth/me` so the
    /// frontend can send authenticated mutations after a page refresh.
    /// It is not a bearer credential (CSRF layer relies on SOP).
    pub csrf_token: String,
    /// Session's UX-level default cooperative (M1-K1). The tenant
    /// extractor treats it as a fallback hint only — membership is
    /// re-validated per request.
    pub active_cooperative_id: Option<Uuid>,
    /// SHA-256 hash of the presented session token (M1-P4-P2B). Fed to
    /// `app_open_request_context` so the *database* derives the user id
    /// from the session row when stamping `app.user_id` — the caller
    /// never supplies a user UUID as proof of identity.
    pub session_token_hash: [u8; 32],
    /// The application-clock instant this authentication was resolved
    /// (M1-P4-P2B). Session expiry inside `app_open_request_context`
    /// is evaluated against this value — the same clock domain that
    /// wrote the session — not the database wall clock.
    pub authed_at: OffsetDateTime,
    /// M2 forced rotation (PD-14 recommended): while set, only the
    /// password-change lane, logout and `/api/auth/me` are reachable —
    /// enforced here at session resolution, never by the frontend.
    pub must_change_password: bool,
}

/// Endpoints reachable while `must_change_password` holds (M2): the
/// rotation lane itself, a safe logout, and `me` so the frontend can
/// surface the forced-change state.
fn password_rotation_exempt(method: &axum::http::Method, path: &str) -> bool {
    matches!(
        (method.as_str(), path),
        ("POST", "/api/auth/password") | ("POST", "/api/auth/logout") | ("GET", "/api/auth/me")
    )
}

impl FromRequestParts<AppState> for CurrentAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Some(pool) = state.db.as_ref() else {
            return Err(ApiError::DependencyUnavailable);
        };

        let Some(raw_token) = cookies::extract_session_token(&parts.headers) else {
            return Err(ApiError::AuthenticationRequired);
        };

        let token_hash = token::hash_token(&raw_token);
        let now = state.auth.clock.now();
        let classified = session::classify_session(pool, token_hash, now)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "session lookup failed");
                ApiError::Internal
            })?;

        let session_row = match classified {
            Ok(pair) => pair,
            Err(rejection) => {
                return Err(match rejection {
                    SessionRejection::Unknown | SessionRejection::Revoked => {
                        ApiError::AuthenticationRequired
                    }
                    SessionRejection::Expired | SessionRejection::IdleExpired => {
                        ApiError::SessionExpired
                    }
                    SessionRejection::UserDisabled => ApiError::AuthenticationRequired,
                });
            }
        };

        // Bounded last-seen refresh (write at most once / 5 minutes).
        if let Err(error) = session::touch_if_needed(
            pool,
            token_hash,
            state.auth.config.session_idle_ttl_secs,
            now,
        )
        .await
        {
            tracing::warn!(error = %error, session_id = %session_row.session_id, "session touch failed");
        }

        // M2 forced rotation: a must_change_password session may only
        // reach the rotation lane — every other protected endpoint is
        // refused at session resolution.
        if session_row.must_change_password
            && !password_rotation_exempt(&parts.method, parts.uri.path())
        {
            return Err(ApiError::PermissionDenied);
        }

        Ok(CurrentAuth {
            user_id: session_row.user_id,
            username: session_row.username,
            display_name: session_row.display_name,
            session_id: session_row.session_id,
            session_created_at: session_row.created_at,
            session_expires_at: session_row.expires_at,
            csrf_token: session_row.csrf_token,
            active_cooperative_id: session_row.active_cooperative_id,
            session_token_hash: token_hash,
            authed_at: now,
            must_change_password: session_row.must_change_password,
        })
    }
}
