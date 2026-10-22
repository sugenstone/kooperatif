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
//!   - M1-P6 B4: scope is the SESSION's (`resolve_session_scope`):
//!     its `active_cooperative_id`, or the bootstrap cooperative while
//!     the session never switched. Active membership + active
//!     cooperative + `business_enabled` are mandatory, or the request
//!     fails closed — global RBAC can never serve as a fallback.
//!   - An `x-cooperative-id` header may only confirm the session scope;
//!     any other value is denied.
//!   - Before the initial cooperative has been bootstrapped (provably
//!     pre-tenancy), M0 single-cooperative access applies.
//!
//! M1-P6 B4: business tables are cooperative-owned (P2-C3) and every
//! business transaction is stamped with the session scope (0071), so
//! the gate now admits any qualifying session cooperative — it remains
//! the first, coarse fail-closed check in front of `BusinessAuth`.

use axum::extract::{FromRequestParts, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::auth::extractor::CurrentAuth;
use crate::http::AppState;

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

    // M1-P6 B4: the same session-scope authority as BusinessAuth —
    // the session's active cooperative (bootstrap while never
    // switched) must qualify; a header can only confirm it. The
    // provably pre-tenancy lane keeps M0 single-cooperative access.
    match crate::tenant::extractor::resolve_session_scope(pool, &auth, request.headers()).await {
        Ok(_) => next.run(request).await,
        Err(error) => error.into_response(),
    }
}
