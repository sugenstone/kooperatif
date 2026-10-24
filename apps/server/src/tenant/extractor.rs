//! Per-request cooperative context (M1-P0, M1-K1, M1-P6 B4).
//!
//! B4 — the authenticated SESSION is the only source of cooperative
//! scope (`resolve_session_scope`):
//!   1. `user_sessions.active_cooperative_id`, written only by the
//!      audited switch workflow (`POST /api/auth/cooperative`);
//!   2. a session that never switched keeps the pre-B4 bootstrap scope
//!      (compatibility lane — the former headerless behaviour);
//!   3. otherwise fail closed.
//!
//! `x-cooperative-id` is an ASSERTION, never a selector: when present
//! it must equal the session scope or the request is denied — it can
//! no longer pick, override or widen scope. The database enforces the
//! same rule independently (0071 `app_stamp_cooperative`).
//!
//! The resolved id is NEVER trusted: the user's membership must exist
//! and be `active`, the cooperative must be `active`, and the
//! `business_enabled` gate must be open. Nothing in this context grants
//! permissions; `authz` stays the authorization authority.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::HeaderMap;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::extractor::CurrentAuth;
use crate::http::error::ApiError;
use crate::http::AppState;

pub const COOPERATIVE_HEADER: &str = "x-cooperative-id";

/// B4 session-scope resolution outcome.
#[derive(Debug, Clone)]
pub(crate) enum SessionScope {
    Cooperative {
        cooperative_id: Uuid,
        cooperative_name: String,
    },
    /// Provably pre-tenancy installation (M1-P1-005 legacy lane).
    NeverBootstrapped,
}

/// Parse the optional `x-cooperative-id` assertion. Malformed → 400.
///
/// Exactly ONE header value is permitted: any duplicate field — even
/// an identical repeat — is a malformed assertion and fails closed
/// (contract hardening; previously `HeaderMap::get` silently read the
/// first value only). Comma-coalesced forms still fail the UUID parse.
fn asserted_cooperative(headers: &HeaderMap) -> Result<Option<Uuid>, ApiError> {
    let mut values = headers.get_all(COOPERATIVE_HEADER).iter();
    let value = values.next();
    if values.next().is_some() {
        tracing::warn!("cooperative context: duplicate cooperative header");
        return Err(ApiError::CooperativeContextRequired);
    }
    match value {
        None => Ok(None),
        Some(value) => {
            let raw = value.to_str().map_err(|_| {
                tracing::warn!("cooperative context: non-UTF8 cooperative header");
                ApiError::CooperativeContextRequired
            })?;
            Uuid::parse_str(raw.trim()).map(Some).map_err(|_| {
                tracing::warn!("cooperative context: malformed cooperative id");
                ApiError::CooperativeContextRequired
            })
        }
    }
}

/// The single cooperative-scope authority for business and tenant
/// requests (M1-P6 B4). The session's `active_cooperative_id` — or the
/// bootstrap cooperative while the session never switched — is the
/// scope; a header may only confirm it. Membership, cooperative status
/// and `business_enabled` are then verified; any failure denies — there
/// is never a silent fallback to another cooperative.
pub(crate) async fn resolve_session_scope(
    pool: &sqlx::PgPool,
    auth: &CurrentAuth,
    headers: &HeaderMap,
) -> Result<SessionScope, ApiError> {
    let asserted = asserted_cooperative(headers)?;
    let bootstrap = crate::tenant::repo::resolve_bootstrap(pool)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "cooperative context bootstrap lookup failed");
            ApiError::Internal
        })?;
    let authoritative = auth
        .active_cooperative_id
        .or(bootstrap.as_ref().map(|(id, _)| *id));

    match (authoritative, asserted) {
        (Some(scope), Some(header)) if header != scope => {
            tracing::info!(
                outcome = "cooperative_context_denied",
                user_id = %auth.user_id,
                session_scope = %scope,
                asserted = %header,
                "header cannot override the session cooperative scope"
            );
            Err(ApiError::CooperativeAccessDenied)
        }
        (Some(scope), _) => {
            let cooperative_name = verify_business_membership(
                pool,
                &auth.session_token_hash,
                auth.user_id,
                scope,
                auth.authed_at,
            )
            .await?;
            Ok(SessionScope::Cooperative {
                cooperative_id: scope,
                cooperative_name,
            })
        }
        (None, Some(_)) => Err(ApiError::CooperativeAccessDenied),
        (None, None) => match crate::tenant::repo::never_bootstrapped(pool).await {
            Ok(true) => Ok(SessionScope::NeverBootstrapped),
            // No bootstrap row but cooperative/audit residue exists →
            // broken tenant state, fail closed.
            Ok(false) => {
                tracing::warn!(
                    outcome = "business_context_denied",
                    user_id = %auth.user_id,
                    "bootstrap missing but tenant residue exists — refusing legacy lane"
                );
                Err(ApiError::CooperativeAccessDenied)
            }
            Err(error) => {
                tracing::error!(error = %error, "never-bootstrapped check failed");
                Err(ApiError::Internal)
            }
        },
    }
}

/// Verified per-request tenant context (plus the owning auth context).
#[derive(Debug, Clone)]
pub struct TenantCtx {
    pub auth: CurrentAuth,
    pub cooperative_id: Uuid,
    pub cooperative_name: String,
}

/// Map the database-verified context outcome onto the API surface —
/// identical denial semantics to `verify_business_membership` (the
/// extractor's checks already ran; a refusal here means the state moved
/// between extraction and stamping).
fn map_ctx_error(error: crate::db::RequestContextError) -> ApiError {
    use crate::db::RequestContextError::*;
    match error {
        Unauthenticated => ApiError::AuthenticationRequired,
        NotMember => ApiError::CooperativeAccessDenied,
        NotReady => ApiError::CooperativeNotReady,
        Db(e) => {
            tracing::error!(error = %e, "tenant tx context failed");
            ApiError::Internal
        }
    }
}

impl TenantCtx {
    /// Open the request-scoped transaction: session-derived `app.user_id`
    /// + membership-proven `app.cooperative_id`, both tx-local (M1-P4-P2B).
    pub async fn open_tx(
        &self,
        pool: &sqlx::PgPool,
    ) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, ApiError> {
        let (tx, _ctx) = crate::db::request_tx(
            pool,
            &self.auth.session_token_hash,
            self.cooperative_id,
            self.auth.authed_at,
        )
        .await
        .map_err(map_ctx_error)?;
        Ok(tx)
    }
}

impl FromRequestParts<AppState> for TenantCtx {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth = CurrentAuth::from_request_parts(parts, state).await?;
        let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;

        let (cooperative_id, cooperative_name) =
            match resolve_session_scope(pool, &auth, &parts.headers).await? {
                SessionScope::Cooperative {
                    cooperative_id,
                    cooperative_name,
                } => (cooperative_id, cooperative_name),
                SessionScope::NeverBootstrapped => {
                    return Err(ApiError::CooperativeContextRequired)
                }
            };

        Ok(TenantCtx {
            auth,
            cooperative_id,
            cooperative_name,
        })
    }
}

/// Shared membership/cooperative verification for tenant-bearing
/// contexts (TenantCtx and BusinessAuth) and realtime revalidation
/// (M1-P1-c3a). Returns the cooperative name on success. Unknown
/// cooperative, non-member and non-active membership/cooperative are
/// denied identically — the response never distinguishes them
/// (docs/21:86 spirit).
pub(crate) async fn verify_business_membership(
    pool: &sqlx::PgPool,
    token_hash: &[u8; 32],
    user_id: Uuid,
    cooperative_id: Uuid,
    now: OffsetDateTime,
) -> Result<String, ApiError> {
    // M1-P4-S2A: `user_id` is trace-only — the definer resolves the
    // principal from the session token hash inside the database.
    let resolved = crate::tenant::repo::resolve_membership(pool, token_hash, cooperative_id, now)
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

/// What a business request is verified to operate inside (M1-P1-c1,
/// M1-P6 B4). Two shapes only — anything else is denied at extraction:
///
/// - `Cooperative`: the session's authoritative cooperative (its
///   `active_cooperative_id`, or the bootstrap cooperative while the
///   session never switched) after full membership/status/
///   `business_enabled` verification.
/// - `NeverBootstrapped`: the database provably predates tenancy —
///   `installation_state` singleton exists with a NULL marker AND zero
///   cooperatives AND zero cooperative-attributed audit events
///   (M1-P1-005). The only state where the legacy global authorization
///   lane may open.
#[derive(Debug, Clone)]
pub enum BusinessScope {
    Cooperative {
        cooperative_id: Uuid,
        cooperative_name: String,
    },
    NeverBootstrapped,
}

/// Verified business authorization context (M1-P1-c1, P1-K3, M1-P6 B4).
/// Session-scoped by construction (`resolve_session_scope`):
///
/// - Scope = the session's `active_cooperative_id` (switch workflow
///   only); a session that never switched keeps the bootstrap scope.
/// - Explicit `x-cooperative-id`: malformed → 400; any UUID other than
///   the session scope → 403 (even for members of that cooperative —
///   the header confirms, it never selects).
/// - A session scope whose membership/cooperative no longer qualifies
///   is denied — never silently replaced by another cooperative.
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
            BusinessScope::Cooperative { cooperative_id, .. } => Some(*cooperative_id),
            BusinessScope::NeverBootstrapped => None,
        }
    }

    /// Open the request-scoped transaction (M1-P4-P2B): session-derived
    /// `app.user_id` always; `app.cooperative_id` stamped after
    /// membership proof in the Bootstrap lane. The NeverBootstrapped
    /// lane deliberately leaves the cooperative GUC unset — legacy
    /// NULL-scope rows are handled by the P0/P1 lanes, not by a GUC.
    pub async fn open_tx(
        &self,
        pool: &sqlx::PgPool,
    ) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, ApiError> {
        match self.cooperative_id() {
            Some(coop) => {
                let (tx, _ctx) = crate::db::request_tx(
                    pool,
                    &self.auth.session_token_hash,
                    coop,
                    self.auth.authed_at,
                )
                .await
                .map_err(map_ctx_error)?;
                Ok(tx)
            }
            None => {
                let (tx, _user) = crate::db::request_user_tx(
                    pool,
                    &self.auth.session_token_hash,
                    self.auth.authed_at,
                )
                .await
                .map_err(map_ctx_error)?;
                Ok(tx)
            }
        }
    }

    /// Repeatable-read variant (M1-P4-P2B): report snapshots must
    /// observe a single committed instant across multiple statements,
    /// so the isolation level is raised as the first statement of the
    /// request transaction — before the context-stamping queries.
    pub async fn open_tx_repeatable(
        &self,
        pool: &sqlx::PgPool,
    ) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, ApiError> {
        match self.cooperative_id() {
            Some(coop) => {
                let (tx, _ctx) = crate::db::request_tx_repeatable(
                    pool,
                    &self.auth.session_token_hash,
                    coop,
                    self.auth.authed_at,
                )
                .await
                .map_err(map_ctx_error)?;
                Ok(tx)
            }
            None => {
                let (tx, _user) = crate::db::request_user_tx_repeatable(
                    pool,
                    &self.auth.session_token_hash,
                    self.auth.authed_at,
                )
                .await
                .map_err(map_ctx_error)?;
                Ok(tx)
            }
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

        let scope = match resolve_session_scope(pool, &auth, &parts.headers).await? {
            SessionScope::Cooperative {
                cooperative_id,
                cooperative_name,
            } => BusinessScope::Cooperative {
                cooperative_id,
                cooperative_name,
            },
            SessionScope::NeverBootstrapped => BusinessScope::NeverBootstrapped,
        };

        Ok(BusinessAuth { auth, scope })
    }
}
