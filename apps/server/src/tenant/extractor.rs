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

        let cooperative_name =
            verify_business_membership(pool, auth.user_id, cooperative_id).await?;

        Ok(TenantCtx {
            auth,
            cooperative_id,
            cooperative_name,
        })
    }
}

/// Shared membership/cooperative verification for tenant-bearing
/// contexts (TenantCtx and BusinessAuth). Returns the cooperative name
/// on success. Unknown cooperative, non-member and non-active
/// membership/cooperative are denied identically — the response never
/// distinguishes them (docs/21:86 spirit).
async fn verify_business_membership(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    cooperative_id: Uuid,
) -> Result<String, ApiError> {
    let resolved = crate::tenant::repo::resolve_membership(pool, user_id, cooperative_id)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "tenant membership resolution failed");
            ApiError::Internal
        })?;

    let Some(row) = resolved else {
        tracing::info!(
            outcome = "tenant_context_denied",
            user_id = %user_id,
            cooperative_id = %cooperative_id
        );
        return Err(ApiError::CooperativeAccessDenied);
    };

    if row.membership_status != "active" || row.cooperative_status != "active" {
        tracing::info!(
            outcome = "tenant_context_denied",
            user_id = %user_id,
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

    Ok(row.cooperative_name)
}

/// What a legacy business request is verified to operate inside
/// (M1-P1-c1). Two shapes only — anything else is denied at extraction:
///
/// - `Bootstrap`: the unique `is_bootstrap` cooperative after full
///   membership/status/`business_enabled` verification. Until P2 owns
///   business rows, legacy endpoints only ever run in this scope.
/// - `NeverBootstrapped`: the database provably predates tenancy —
///   `installation_state` singleton exists with a NULL marker AND zero
///   cooperatives AND zero cooperative-attributed audit events
///   (M1-P1-005). The only state where the legacy global authorization
///   lane may open.
#[derive(Debug, Clone)]
pub enum BusinessScope {
    Bootstrap {
        cooperative_id: Uuid,
        cooperative_name: String,
    },
    NeverBootstrapped,
}

/// Verified business authorization context for legacy business handlers
/// (M1-P1-c1, P1-K3). Bootstrap-pinned by construction:
///
/// - Explicit `x-cooperative-id`: malformed → 400; any UUID that is not
///   the bootstrap cooperative → 403 (even for members — no silent
///   fallback to bootstrap).
/// - Headerless: pinned to the bootstrap cooperative; the session's
///   `active_cooperative_id` UX default is deliberately IGNORED — it is
///   not a security decision and must never pick a different scope for
///   a legacy endpoint.
/// - Bootstrap resolution is marker-authoritative (M1-P1-005): the
///   `installation_state` FK makes the bootstrap row undeletable while
///   referenced, and a marker/flag inconsistency resolves to no
///   bootstrap. The legacy lane then opens only when
///   `never_bootstrapped` proves the installation predates tenancy;
///   any marker, cooperative residue, or cooperative-attributed audit
///   event means the tenant state is broken and the request fails
///   closed.
#[derive(Debug, Clone)]
pub struct BusinessAuth {
    pub auth: CurrentAuth,
    pub scope: BusinessScope,
}

impl BusinessAuth {
    /// The verified cooperative scope, if any. `None` only in the
    /// `NeverBootstrapped` lane.
    pub fn cooperative_id(&self) -> Option<Uuid> {
        match &self.scope {
            BusinessScope::Bootstrap { cooperative_id, .. } => Some(*cooperative_id),
            BusinessScope::NeverBootstrapped => None,
        }
    }
}

impl FromRequestParts<AppState> for BusinessAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth = CurrentAuth::from_request_parts(parts, state).await?;
        let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;

        let bootstrap = crate::tenant::repo::resolve_bootstrap(pool)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "business context bootstrap lookup failed");
                ApiError::Internal
            })?;

        let explicit = match parts.headers.get(COOPERATIVE_HEADER) {
            Some(value) => {
                let raw = value.to_str().map_err(|_| {
                    tracing::warn!("business context: non-UTF8 cooperative header");
                    ApiError::CooperativeContextRequired
                })?;
                Some(Uuid::parse_str(raw.trim()).map_err(|_| {
                    tracing::warn!("business context: malformed cooperative id");
                    ApiError::CooperativeContextRequired
                })?)
            }
            None => None,
        };

        let scope = match (explicit, bootstrap) {
            // Explicit header must name the bootstrap cooperative —
            // exactly. Non-bootstrap requests are rejected outright;
            // membership is then verified identically to TenantCtx.
            (Some(requested), Some((bootstrap_id, _))) => {
                if requested != bootstrap_id {
                    tracing::info!(
                        outcome = "business_context_denied",
                        user_id = %auth.user_id,
                        requested = %requested,
                        "explicit non-bootstrap cooperative rejected"
                    );
                    return Err(ApiError::CooperativeAccessDenied);
                }
                let cooperative_name =
                    verify_business_membership(pool, auth.user_id, bootstrap_id).await?;
                BusinessScope::Bootstrap {
                    cooperative_id: bootstrap_id,
                    cooperative_name,
                }
            }
            // Explicit header while no bootstrap exists: never a match.
            (Some(_), None) => return Err(ApiError::CooperativeAccessDenied),
            // Headerless: pinned to bootstrap — the session default is
            // intentionally ignored for legacy business endpoints.
            (None, Some((bootstrap_id, _))) => {
                let cooperative_name =
                    verify_business_membership(pool, auth.user_id, bootstrap_id).await?;
                BusinessScope::Bootstrap {
                    cooperative_id: bootstrap_id,
                    cooperative_name,
                }
            }
            (None, None) => {
                match crate::tenant::repo::never_bootstrapped(pool).await {
                    Ok(true) => BusinessScope::NeverBootstrapped,
                    // No bootstrap row but cooperative/audit residue
                    // exists → broken tenant state, fail closed.
                    Ok(false) => {
                        tracing::warn!(
                            outcome = "business_context_denied",
                            user_id = %auth.user_id,
                            "bootstrap missing but tenant residue exists — refusing legacy lane"
                        );
                        return Err(ApiError::CooperativeAccessDenied);
                    }
                    Err(error) => {
                        tracing::error!(error = %error, "never-bootstrapped check failed");
                        return Err(ApiError::Internal);
                    }
                }
            }
        };

        Ok(BusinessAuth { auth, scope })
    }
}
