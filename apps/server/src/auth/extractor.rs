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

        let (session_row, user_row) = match classified {
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
            &session_row,
            state.auth.config.session_idle_ttl_secs,
            now,
        )
        .await
        {
            tracing::warn!(error = %error, session_id = %session_row.id, "session touch failed");
        }

        Ok(CurrentAuth {
            user_id: user_row.id,
            username: user_row.username,
            display_name: user_row.display_name,
            session_id: session_row.id,
            session_created_at: session_row.created_at,
            session_expires_at: session_row.expires_at,
            csrf_token: session_row.csrf_token,
        })
    }
}
