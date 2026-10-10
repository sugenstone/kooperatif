//! HTTP layer: router, middleware and endpoints.

pub mod error;
pub mod health;

use std::sync::Arc;

use axum::extract::Request;
use axum::http::header;
use axum::http::HeaderValue;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
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
            axum::http::HeaderName::from_static("x-cooperative-id"),
        ])
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::PATCH,
            axum::http::Method::DELETE,
        ])
}

/// Baseline hardening headers applied to every response (STEP-017 §27):
/// the API must never be framed, sniffed as a different content type or
/// leak URLs through the Referer header. `no-store` for authenticated
/// routes is applied per-router (`auth::routes::no_store_cache_control`).
async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

/// F6 (PILOT-FIX-001) — error-body sanitization at the boundary.
///
/// Every legitimate handler answers through `ApiError` (the stable
/// `{error:{code}}` JSON contract) or `Json<Dto>`. The only remaining
/// path that can hand internal implementation text to a client is the
/// framework's DEFAULT rejection rendering: axum extractor rejections
/// (`Json`/`Query`/`Path`) and `http` layer failures respond with
/// `text/plain` bodies — or no contract body at all (405) — that can
/// echo serde internals, parse details or other implementation strings
/// the API contract never promised.
///
/// Rule: a non-JSON error body is NEVER a contract response — rewrite
/// it to the matching `ApiError` code. Handler-produced `ApiError`
/// bodies (JSON) and `Json<Dto>` payloads pass through untouched;
/// expected business-rule errors keep their typed codes.
async fn sanitize_rejection_bodies(request: Request, next: Next) -> Response {
    use axum::body::to_bytes;

    let response = next.run(request).await;
    let status = response.status();
    if status.is_success() {
        return response;
    }
    let is_json_contract = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("application/json"))
        .unwrap_or(false);
    if is_json_contract {
        return response;
    }
    // Drain the internal body for diagnostics, then replace it with
    // the contract shape — the client only ever sees a stable code.
    // The original headers (e.g. `Allow` on 405) are preserved; only
    // content-type/length are replaced by the contract JSON.
    let (mut parts, body) = response.into_parts();
    let detail = to_bytes(body, 4096).await.unwrap_or_default();
    tracing::warn!(
        status = %status,
        detail = %String::from_utf8_lossy(&detail),
        "sanitized framework rejection body"
    );
    let api_error = match status.as_u16() {
        404 | 405 => error::ApiError::NotFound,
        500..=503 => error::ApiError::Internal,
        _ => error::ApiError::ValidationFailed,
    };
    let contract = api_error.into_response().into_parts();
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.insert(
        header::CONTENT_TYPE,
        contract.0.headers[header::CONTENT_TYPE].clone(),
    );
    Response::from_parts(parts, contract.1)
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

    // M1 §5: every business router sits behind the tenant-membership
    // gate until the P2 retrofit scopes them per cooperative. Auth and
    // tenant routers stay outside — a gated member must still be able
    // to log in, inspect memberships and switch context.
    let business = Router::new()
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
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            crate::tenant::gate::business_membership_gate,
        ));

    Router::new()
        .route(HEALTH_PATH, get(health))
        .route(READY_PATH, get(ready))
        .merge(auth_router())
        .merge(business)
        .merge(crate::tenant::routes::tenant_router())
        .fallback(fallback)
        .layer(middleware::from_fn(sanitize_rejection_bodies))
        .layer(middleware::from_fn(security_headers))
        .layer(trace)
        .layer(cors)
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .with_state(state)
}
