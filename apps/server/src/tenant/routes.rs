//! Tenant context endpoints (M1-P0).
//!
//! - `POST /api/auth/cooperative`         select session default coop
//! - `GET  /api/auth/cooperative-context` resolved ctx (proves TenantCtx)
//! - `GET  /api/cooperatives`             my cooperative memberships
//!
//! The switch endpoint sets the *session UX default* only; every
//! tenant-aware operation still re-validates membership per request
//! (M1-K1). Selecting a non-`business_enabled` cooperative is refused —
//! the P0 gate that keeps not-yet-migrated cooperatives inert.

use axum::extract::State;
use axum::http::{header::HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::csrf;
use crate::auth::extractor::CurrentAuth;
use crate::auth::routes as auth_routes;
use crate::http::error::ApiError;
use crate::http::AppState;
use crate::tenant::repo::{self, CooperativeMembershipSummary};
use crate::tenant::TenantCtx;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct SelectCooperativeRequest {
    pub cooperative_id: Uuid,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CooperativeContextResponse {
    pub cooperative_id: Uuid,
    pub cooperative_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MyCooperativesResponse {
    pub cooperatives: Vec<CooperativeMembershipSummary>,
    pub active_cooperative_id: Option<Uuid>,
}

pub fn tenant_router() -> Router<AppState> {
    Router::new()
        .route("/api/auth/cooperative", post(select_cooperative))
        .route("/api/auth/cooperative-context", get(cooperative_context))
        .route("/api/cooperatives", get(my_cooperatives))
        .layer(axum::middleware::from_fn(
            auth_routes::no_store_cache_control,
        ))
}

/// Select the session's default cooperative. Membership is validated
/// server-side; a client-supplied id is never trusted (M1-K1).
pub async fn select_cooperative(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    payload: Result<Json<SelectCooperativeRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    auth_routes::require_allowed_origin(&headers, &Method::POST, &state)?;
    csrf::validate_csrf_token(&headers, &auth.csrf_token)
        .map_err(|rejection| auth_routes::csrf_rejection_to_api_error(&rejection))?;

    let Json(body) = payload.map_err(|rejection| {
        tracing::warn!(error = %rejection, "cooperative selection body rejected");
        ApiError::ValidationFailed
    })?;

    let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;
    let resolved = repo::resolve_membership(
        pool,
        &auth.session_token_hash,
        body.cooperative_id,
        auth.authed_at,
    )
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "cooperative selection lookup failed");
        ApiError::Internal
    })?;

    let Some(row) = resolved else {
        tracing::info!(
            outcome = "cooperative_select_denied",
            user_id = %auth.user_id,
            cooperative_id = %body.cooperative_id
        );
        return Err(ApiError::CooperativeAccessDenied);
    };
    if row.membership_status != "active" || row.cooperative_status != "active" {
        return Err(ApiError::CooperativeAccessDenied);
    }
    if !row.business_enabled {
        return Err(ApiError::CooperativeNotReady);
    }

    // The definer writes `cooperative_context_switched` atomically with
    // the session update (S2B) — attribution derives from the session,
    // never from request parameters.
    repo::set_session_cooperative(pool, auth.session_token_hash, body.cooperative_id)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "cooperative selection persist failed");
            ApiError::Internal
        })?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Resolved tenant context — the endpoint that exercises `TenantCtx`
/// end-to-end: it reports exactly the session scope business
/// authorization uses (M1-P6 B4 — session-authoritative, header may only
/// confirm, fail-closed).
pub async fn cooperative_context(
    ctx: TenantCtx,
) -> Result<Json<CooperativeContextResponse>, ApiError> {
    Ok(Json(CooperativeContextResponse {
        cooperative_id: ctx.cooperative_id,
        cooperative_name: ctx.cooperative_name,
    }))
}

/// The user's cooperative memberships — drives the future switcher UI
/// (P7). Includes non-enabled cooperatives so the UI can show them
/// greyed out; selection is still refused server-side.
pub async fn my_cooperatives(
    State(state): State<AppState>,
    auth: CurrentAuth,
) -> Result<Json<MyCooperativesResponse>, ApiError> {
    let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;
    let cooperatives = repo::list_user_cooperatives(pool, &auth.session_token_hash, auth.authed_at)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "cooperative list failed");
            ApiError::Internal
        })?;
    Ok(Json(MyCooperativesResponse {
        cooperatives,
        active_cooperative_id: auth.active_cooperative_id,
    }))
}
