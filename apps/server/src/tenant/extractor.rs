//! Per-request cooperative context (M1-P0, M1-K1).
//!
//! Resolution order, all server-side verified:
//!   1. `x-cooperative-id` request header (explicit context — the
//!      mechanism that lets two browser tabs operate in different
//!      cooperatives at once);
//!   2. `user_sessions.active_cooperative_id` (UX default only);
//!   3. fail closed: `cooperative_context_required`.
//!
//! The resolved id is NEVER trusted: the user's membership must exist
//! and be `active`, the cooperative must be `active`, and the
//! `business_enabled` gate must be open (P0 §5 — keeps
//! not-yet-migrated cooperatives out of business traffic). Nothing in
//! this context grants permissions; `authz::require` stays the
//! authorization authority (cooperative-scoped from P1).

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use uuid::Uuid;

use crate::auth::extractor::CurrentAuth;
use crate::http::error::ApiError;
use crate::http::AppState;

pub const COOPERATIVE_HEADER: &str = "x-cooperative-id";

/// Verified per-request tenant context (plus the owning auth context).
#[derive(Debug, Clone)]
pub struct TenantCtx {
    pub auth: CurrentAuth,
    pub cooperative_id: Uuid,
    pub cooperative_name: String,
}

impl FromRequestParts<AppState> for TenantCtx {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth = CurrentAuth::from_request_parts(parts, state).await?;
        let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;

        let requested = match parts.headers.get(COOPERATIVE_HEADER) {
            Some(value) => {
                let raw = value.to_str().map_err(|_| {
                    tracing::warn!("tenant context: non-UTF8 cooperative header");
                    ApiError::CooperativeContextRequired
                })?;
                Some(Uuid::parse_str(raw.trim()).map_err(|_| {
                    tracing::warn!("tenant context: malformed cooperative id");
                    ApiError::CooperativeContextRequired
                })?)
            }
            None => auth.active_cooperative_id,
        };

        let Some(cooperative_id) = requested else {
            return Err(ApiError::CooperativeContextRequired);
        };

        let resolved = crate::tenant::repo::resolve_membership(pool, auth.user_id, cooperative_id)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "tenant membership resolution failed");
                ApiError::Internal
            })?;

        let Some(row) = resolved else {
            // Non-member or unknown cooperative: identical denial — the
            // response must never distinguish them (docs/21:86 spirit).
            tracing::info!(
                outcome = "tenant_context_denied",
                user_id = %auth.user_id,
                cooperative_id = %cooperative_id
            );
            return Err(ApiError::CooperativeAccessDenied);
        };

        if row.membership_status != "active" || row.cooperative_status != "active" {
            tracing::info!(
                outcome = "tenant_context_denied",
                user_id = %auth.user_id,
                cooperative_id = %cooperative_id,
                membership_status = %row.membership_status,
                cooperative_status = %row.cooperative_status
            );
            return Err(ApiError::CooperativeAccessDenied);
        }

        if !row.business_enabled {
            // Member of a real but not-yet-migrated cooperative: the
            // P0 feature gate. Distinct code so the UI can explain.
            return Err(ApiError::CooperativeNotReady);
        }

        Ok(TenantCtx {
            auth,
            cooperative_id,
            cooperative_name: row.cooperative_name,
        })
    }
}
