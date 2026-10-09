//! Authentication API routes (STEP-002 §16–§22).
//!
//! Endpoint semantics:
//! - `POST /api/auth/login`               200 + session cookie / 401 generic
//! - `GET  /api/auth/me`                  200 safe user+session context
//! - `POST /api/auth/logout`              204, idempotent, clears cookie
//! - `GET  /api/auth/sessions`            200 own active sessions
//! - `DELETE /api/auth/sessions/{id}`     204 revoke own session (404-safe)
//! - `POST /api/auth/sessions/revoke-others` 204 revoke all other own sessions
//!
//! All responses: `Cache-Control: no-store`. All non-GET requests pass
//! Origin validation; authenticated mutations additionally require the
//! per-session `x-csrf-token` synchronizer token.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Path, State};
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::audit::{self, login_failed_metadata};
use crate::auth::cookies;
use crate::auth::csrf::{self, CsrfRejection};
use crate::auth::extractor::CurrentAuth;
use crate::auth::limiter;
use crate::auth::session::{self};
use crate::auth::token;
use crate::auth::users;
use crate::http::error::ApiError;
use crate::http::AppState;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSummary {
    pub id: Uuid,
    pub username: String,
    pub display_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleSummaryDto {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthResponse {
    pub user: UserSummary,
    pub session: SessionInfo,
    /// Raw CSRF synchronizer token for authenticated mutations.
    pub csrf_token: String,
    /// Effective permission keys (union over active roles) — the
    /// authoritative authorization context for frontend UX (STEP-003
    /// §21). Backend enforcement never trusts this.
    pub permissions: Vec<String>,
    /// Safe summaries of the user's active roles.
    pub roles: Vec<RoleSummaryDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub last_seen_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub current: bool,
    pub client_label: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionsResponse {
    pub current_session_id: Uuid,
    pub sessions: Vec<SessionSummary>,
}

pub fn auth_router() -> Router<AppState> {
    Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/auth/me", get(me))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/sessions", get(list_sessions))
        .route("/api/auth/sessions/revoke-others", post(revoke_others))
        .route("/api/auth/sessions/{id}", delete(revoke_session))
        .layer(middleware::from_fn(no_store_cache_control))
}

/// Auth responses must never be cached by browsers or shared caches
/// (STEP-002 §36). Shared with the RBAC router (STEP-003 §48).
pub(crate) async fn no_store_cache_control(
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

pub(crate) fn require_allowed_origin(
    headers: &HeaderMap,
    method: &Method,
    state: &AppState,
) -> Result<(), ApiError> {
    csrf::validate_origin(headers, method, &state.auth.config.allowed_origins)
        .map_err(|rejection| csrf_rejection_to_api_error(&rejection))
}

pub(crate) fn csrf_rejection_to_api_error(rejection: &CsrfRejection) -> ApiError {
    tracing::warn!(reason = ?rejection, "csrf validation failed");
    ApiError::CsrfFailed
}

pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    payload: Result<Json<LoginRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    // Layer 1 of CSRF defense applies to login too (login-CSRF).
    require_allowed_origin(&headers, &Method::POST, &state)?;

    let Json(body) = payload.map_err(|rejection| {
        tracing::warn!(error = %rejection, "login body rejected");
        ApiError::ValidationFailed
    })?;

    let Some(pool) = state.db.as_ref() else {
        return Err(ApiError::DependencyUnavailable);
    };

    let client_ip = limiter::client_ip(&headers, Some(peer), state.auth.config.forwarded_ip);

    // Malformed usernames can never match a stored user; keep the same
    // external failure shape while still doing real verification work.
    let normalized = match crate::auth::identity::normalize_and_validate_username(&body.username) {
        Ok(normalized) => normalized,
        Err(_) => {
            state.auth.limiter.record_ip_attempt(&client_ip);
            crate::auth::identity::dummy_verify(&state.auth.argon2, &body.password);
            audit::record(
                pool,
                audit::SecurityEventType::LoginFailed,
                None,
                None,
                login_failed_metadata(&body.username.to_lowercase(), "unknown_username"),
            )
            .await;
            tracing::info!(outcome = "login_failed", reason = "invalid_username");
            return Err(ApiError::AuthenticationFailed);
        }
    };

    if !state.auth.limiter.check(&normalized, &client_ip).allowed {
        audit::record(
            pool,
            audit::SecurityEventType::LoginFailed,
            None,
            None,
            login_failed_metadata(&normalized, "rate_limited"),
        )
        .await;
        tracing::warn!(outcome = "login_rate_limited", "login blocked by limiter");
        return Err(ApiError::RateLimited);
    }
    state.auth.limiter.record_ip_attempt(&client_ip);

    let user = users::find_by_normalized_username(pool, &normalized)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "user lookup failed");
            ApiError::Internal
        })?;

    let argon2 = state.auth.argon2.clone();
    let password_ok = match &user {
        Some(user) => {
            crate::auth::identity::verify_password(&argon2, &body.password, &user.password_hash)
                .unwrap_or(false)
        }
        None => {
            // Equalize timing for unknown usernames (anti-enumeration).
            crate::auth::identity::dummy_verify(&argon2, &body.password);
            false
        }
    };

    let Some(user) = user else {
        state.auth.limiter.record_failure(&normalized);
        audit::record(
            pool,
            audit::SecurityEventType::LoginFailed,
            None,
            None,
            login_failed_metadata(&normalized, "unknown_username"),
        )
        .await;
        tracing::info!(outcome = "login_failed", reason = "unknown_username");
        return Err(ApiError::AuthenticationFailed);
    };

    let user_disabled = user.status != "active";
    if !password_ok || user_disabled {
        state.auth.limiter.record_failure(&normalized);
        let reason = if user_disabled {
            "user_disabled"
        } else {
            "invalid_credentials"
        };
        audit::record(
            pool,
            audit::SecurityEventType::LoginFailed,
            Some(user.id),
            None,
            login_failed_metadata(&normalized, reason),
        )
        .await;
        tracing::info!(outcome = "login_failed", reason, user_id = %user.id);
        return Err(ApiError::AuthenticationFailed);
    }

    // Success: fresh session (fixation-safe: brand-new token, the
    // attacker-supplied cookie value is never reused), limiter recovery.
    state.auth.limiter.on_success(&normalized);
    let now = state.auth.clock.now();
    let session_token = token::generate_token();
    let csrf_token = token::generate_token();
    let client_label = session::coarse_client_label(
        headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok()),
    );

    let created = session::create_session(
        pool,
        session::NewSession {
            user_id: user.id,
            token_hash: session_token.hash,
            csrf_token: &csrf_token.raw,
            absolute_ttl_secs: state.auth.config.session_absolute_ttl_secs,
            idle_ttl_secs: state.auth.config.session_idle_ttl_secs,
            client_label,
            now,
        },
    )
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "session creation failed");
        ApiError::Internal
    })?;

    if let Err(error) = users::touch_last_login(pool, user.id, now).await {
        tracing::warn!(error = %error, "last_login update failed");
    }

    audit::record(
        pool,
        audit::SecurityEventType::LoginSucceeded,
        Some(user.id),
        Some(created.id),
        serde_json::json!({ "username": normalized }),
    )
    .await;
    tracing::info!(outcome = "login_succeeded", user_id = %user.id, session_id = %created.id);

    let cookie = cookies::build_session_cookie(
        &session_token.raw,
        state.auth.config.cookie_secure,
        state.auth.config.session_absolute_ttl_secs,
    );

    let (permissions, roles) = authorization_context(pool, user.id)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "authorization context failed at login");
            ApiError::Internal
        })?;

    let body = AuthResponse {
        user: UserSummary {
            id: user.id,
            username: user.username.clone(),
            display_name: user.display_name.clone(),
        },
        session: SessionInfo {
            id: created.id,
            created_at: created.created_at,
            expires_at: created.expires_at,
        },
        csrf_token: csrf_token.raw,
        permissions,
        roles,
    };

    Ok((StatusCode::OK, [(header::SET_COOKIE, cookie)], Json(body)).into_response())
}

/// Effective permission keys + safe active-role summaries for the
/// authorization context (STEP-003 §21). Errors propagate: an auth
/// context answer must never silently report an empty permission set.
async fn authorization_context(
    pool: &sqlx::PgPool,
    user_id: Uuid,
) -> Result<(Vec<String>, Vec<RoleSummaryDto>), sqlx::Error> {
    let mut permissions: Vec<String> = crate::auth::authz::effective_permissions(pool, user_id)
        .await?
        .into_iter()
        .collect();
    permissions.sort();
    let roles: Vec<RoleSummaryDto> = crate::auth::authz::active_role_summaries(pool, user_id)
        .await?
        .into_iter()
        .map(|(id, name)| RoleSummaryDto { id, name })
        .collect();
    Ok((permissions, roles))
}

pub async fn me(
    State(state): State<AppState>,
    auth: CurrentAuth,
) -> Result<Json<AuthResponse>, ApiError> {
    let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;
    let (permissions, roles) =
        authorization_context(pool, auth.user_id)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "authorization context failed");
                ApiError::Internal
            })?;
    Ok(Json(AuthResponse {
        user: UserSummary {
            id: auth.user_id,
            username: auth.username,
            display_name: auth.display_name,
        },
        session: SessionInfo {
            id: auth.session_id,
            created_at: auth.session_created_at,
            expires_at: auth.session_expires_at,
        },
        csrf_token: auth.csrf_token,
        permissions,
        roles,
    }))
}

pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_allowed_origin(&headers, &Method::POST, &state)?;

    let clearing = cookies::build_clearing_cookie(state.auth.config.cookie_secure);

    // Idempotent: no/invalid session still clears the cookie with 204.
    let Some(raw_token) = cookies::extract_session_token(&headers) else {
        return Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, clearing)]).into_response());
    };
    let Some(pool) = state.db.as_ref() else {
        return Err(ApiError::DependencyUnavailable);
    };

    let token_hash = token::hash_token(&raw_token);
    let now = state.auth.clock.now();
    let classified = session::classify_session(pool, token_hash, now)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "logout: session lookup failed");
            ApiError::Internal
        })?;

    let Ok((session_row, user_row)) = classified else {
        return Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, clearing)]).into_response());
    };

    // Authenticated mutation: full CSRF validation before revoking.
    csrf::validate_csrf_token(&headers, &session_row.csrf_token)
        .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;

    session::revoke_session(pool, session_row.id, Some(user_row.id), "logout", now)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "logout: revocation failed");
            ApiError::Internal
        })?;

    audit::record(
        pool,
        audit::SecurityEventType::Logout,
        Some(user_row.id),
        Some(session_row.id),
        serde_json::json!({}),
    )
    .await;
    tracing::info!(outcome = "logout", user_id = %user_row.id, session_id = %session_row.id);

    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, clearing)]).into_response())
}

pub async fn list_sessions(
    State(state): State<AppState>,
    auth: CurrentAuth,
) -> Result<Json<SessionsResponse>, ApiError> {
    let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;
    let rows = session::list_active_sessions(pool, auth.user_id, state.auth.clock.now())
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "session listing failed");
            ApiError::Internal
        })?;

    let sessions = rows
        .into_iter()
        .map(|row| SessionSummary {
            id: row.id,
            created_at: row.created_at,
            last_seen_at: row.last_seen_at,
            expires_at: row.expires_at,
            current: row.id == auth.session_id,
            client_label: row.client_label,
        })
        .collect();

    Ok(Json(SessionsResponse {
        current_session_id: auth.session_id,
        sessions,
    }))
}

pub async fn revoke_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(session_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    csrf::validate_csrf_token(&headers, &auth.csrf_token)
        .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;

    let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;
    let now = state.auth.clock.now();

    let revoked =
        session::revoke_session(pool, session_id, Some(auth.user_id), "user_revoked", now)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "session revocation failed");
                ApiError::Internal
            })?;

    if !revoked {
        // Idempotency: an already-revoked OWN session answers 204;
        // anything else (foreign/unknown) answers an indistinguishable
        // 404 — never leaking whose session it was.
        let already: Option<bool> = sqlx::query_scalar(
            "SELECT revoked_at IS NOT NULL FROM user_sessions \
                                WHERE id = $1 AND user_id = $2",
        )
        .bind(session_id)
        .bind(auth.user_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "revocation check failed");
            ApiError::Internal
        })?;
        if already != Some(true) {
            tracing::info!(outcome = "session_revoke_denied", target_session = %session_id);
            return Err(ApiError::NotFound);
        }
        return Ok(StatusCode::NO_CONTENT.into_response());
    }

    audit::record(
        pool,
        audit::SecurityEventType::SessionRevoked,
        Some(auth.user_id),
        Some(session_id),
        serde_json::json!({ "source": "user" }),
    )
    .await;
    tracing::info!(outcome = "session_revoked", user_id = %auth.user_id, session_id = %session_id);

    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn revoke_others(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
) -> Result<Response, ApiError> {
    csrf::validate_csrf_token(&headers, &auth.csrf_token)
        .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;

    let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;
    let now = state.auth.clock.now();

    let count = session::revoke_all_other_sessions(
        pool,
        auth.user_id,
        auth.session_id,
        "user_revoked_others",
        now,
    )
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "revoke-others failed");
        ApiError::Internal
    })?;

    audit::record(
        pool,
        audit::SecurityEventType::SessionsRevokedAllOthers,
        Some(auth.user_id),
        Some(auth.session_id),
        serde_json::json!({ "revoked_count": count }),
    )
    .await;
    tracing::info!(outcome = "sessions_revoked_all_others", user_id = %auth.user_id, count);

    Ok(StatusCode::NO_CONTENT.into_response())
}
