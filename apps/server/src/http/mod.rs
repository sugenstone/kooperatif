//! HTTP layer: router, middleware and endpoints.

pub mod error;
pub mod health;

use std::sync::Arc;

use axum::routing::get;
use axum::Router;
use health::{health, ready};
use sqlx::PgPool;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

use crate::auth::rbac::rbac_router;
use crate::auth::routes::auth_router;
use crate::auth::AuthRuntime;
use crate::credits::routes::credits_router;
use crate::financial_accounts::routes::financial_accounts_router;
use crate::governance::routes::governance_router;
use crate::http::error::fallback;
use crate::income_expense::routes::income_expense_router;
use crate::investments::routes::investments_router;
use crate::parties::routes::parties_router;
use crate::payments::routes::payments_router;
use crate::periods::routes::periods_router;
use crate::share_returns::routes::share_returns_router;
use crate::shares::routes::shares_router;
use crate::social_aid::routes::social_aid_router;

/// Shared application state (grows with later domain modules).
#[derive(Clone)]
pub struct AppState {
    /// `None` means the server runs without a configured database;
    /// `/ready` then reports `unconfigured` (and answers 503).
    pub db: Option<PgPool>,
    /// Authentication/session runtime (ADR-002, STEP-002).
    pub auth: Arc<AuthRuntime>,
}

pub const HEALTH_PATH: &str = "/health";
pub const READY_PATH: &str = "/ready";

/// CORS policy for the given allowlist. An empty allowlist produces a
/// layer that answers no cross-origin requests: same-origin only
/// (the production topology of ADR-012).
pub fn cors_layer(origins: &[String]) -> tower_http::cors::CorsLayer {
    use tower_http::cors::CorsLayer;

    let layer = CorsLayer::new();
    if origins.is_empty() {
        return layer;
    }
    let allowed: Vec<_> = origins.iter().filter_map(|o| o.parse().ok()).collect();
    // Development (Vite on :5173 -> API on :8080) needs credentialed
    // cross-origin cookies; allowed origins are an explicit allowlist.
    layer
        .allow_origin(allowed)
        .allow_credentials(true)
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderName::from_static("x-csrf-token"),
        ])
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::PATCH,
            axum::http::Method::DELETE,
        ])
}

/// Build the application router with observability middleware.
pub fn router(state: AppState, cors: tower_http::cors::CorsLayer) -> Router {
    let trace = TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<_>| {
        let request_id = request
            .headers()
            .get(axum::http::HeaderName::from_static("x-request-id"))
            .and_then(|value| value.to_str().ok())
            .unwrap_or("-");
        tracing::info_span!(
            "http_request",
            method = %request.method(),
            path = %request.uri().path(),
            request_id = %request_id,
        )
    });

    Router::new()
        .route(HEALTH_PATH, get(health))
        .route(READY_PATH, get(ready))
        .merge(auth_router())
        .merge(rbac_router())
        .merge(parties_router())
        .merge(shares_router())
        .merge(periods_router())
        .merge(payments_router())
        .merge(financial_accounts_router())
        .merge(credits_router())
        .merge(income_expense_router())
        .merge(share_returns_router())
        .merge(investments_router())
        .merge(social_aid_router())
        .merge(governance_router())
        .merge(crate::reports::routes::reports_router())
        .merge(crate::realtime::routes::realtime_router())
        .fallback(fallback)
        .layer(trace)
        .layer(cors)
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .with_state(state)
}
