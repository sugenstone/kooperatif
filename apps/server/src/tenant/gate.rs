//! M1 §5 backend gate for not-yet-tenant-scoped business modules.
//!
//! P0 shipped cooperative + membership infrastructure, but the business
//! routers still run on the pre-tenant (global) RBAC model — the P2
//! retrofit has not reached them yet. Without a gate, a user whose only
//! memberships live in business-disabled cooperatives could still ride
//! their global role permissions into every business module — a real
//! cross-cooperative exposure proven by `tests/tenant_isolation_gate.rs`.
//!
//! Gate semantics (applied to business routes only):
//!   - Unauthenticated requests pass through: the endpoint's own
//!     `CurrentAuth` extractor produces the contract 401.
//!   - An explicit `x-cooperative-id` header is validated exactly like
//!     `TenantCtx`: active membership + active cooperative +
//!     `business_enabled`, or the request fails closed.
//!   - Without a header: before the initial cooperative has been
//!     bootstrapped, pre-tenant single-cooperative access applies
//!     (M0 compatibility). AFTER bootstrap, an active membership in an
//!     active, business-enabled cooperative is mandatory — a user with
//!     zero or only revoked/disabled memberships is denied, so global
//!     RBAC can never serve as a fallback.
//!
//! This is intentionally not tenant scoping — it is a kill switch that
//! keeps multi-cooperative business operations impossible until P1/P2
//! land real cooperative ownership on business tables.

use axum::extract::{FromRequestParts, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use crate::auth::extractor::CurrentAuth;
use crate::http::error::ApiError;
use crate::http::AppState;
use crate::tenant::repo;

const COOPERATIVE_HEADER: &str = "x-cooperative-id";

pub async fn business_membership_gate(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let Some(pool) = state.db.as_ref() else {
        return next.run(request).await;
    };

    // Reuse the canonical authentication pipeline instead of
    // duplicating cookie/session plumbing.
    let (mut parts, body) = request.into_parts();
    let auth = CurrentAuth::from_request_parts(&mut parts, &state).await;
    let request = Request::from_parts(parts, body);
    let Ok(auth) = auth else {
        return next.run(request).await;
    };

    // Explicit cooperative context: fail closed on any denial.
    if let Some(raw) = request
        .headers()
        .get(COOPERATIVE_HEADER)
        .and_then(|value| value.to_str().ok())
    {
        let Ok(cooperative_id) = Uuid::parse_str(raw) else {
            return ApiError::ValidationFailed.into_response();
        };
        let resolved = match repo::resolve_membership(pool, auth.user_id, cooperative_id).await {
            Ok(row) => row,
            Err(error) => {
                tracing::error!(error = %error, "tenant gate membership lookup failed");
                return ApiError::Internal.into_response();
            }
        };
        let enabled = resolved.is_some_and(|row| {
            row.membership_status == "active"
                && row.cooperative_status == "active"
                && row.business_enabled
        });
        if !enabled {
            return ApiError::CooperativeAccessDenied.into_response();
        }
        return next.run(request).await;
    }

    // No explicit context. The compatibility exception is exactly one:
    // before the initial cooperative has been bootstrapped, the tenant
    // model is not engaged and pre-tenant single-cooperative access
    // applies. After bootstrap, zero membership means zero business
    // access — a newly created or fully revoked user cannot fall back
    // to global RBAC (M1-P0-003 §1).
    match repo::bootstrap_completed(pool).await {
        Ok(false) => next.run(request).await,
        Ok(true) => match repo::has_enabled_business_membership(pool, auth.user_id).await {
            Ok(true) => next.run(request).await,
            Ok(false) => ApiError::CooperativeAccessDenied.into_response(),
            Err(error) => {
                tracing::error!(error = %error, "tenant gate membership lookup failed");
                ApiError::Internal.into_response()
            }
        },
        Err(error) => {
            tracing::error!(error = %error, "tenant gate bootstrap lookup failed");
            ApiError::Internal.into_response()
        }
    }
}
