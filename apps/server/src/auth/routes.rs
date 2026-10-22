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
    /// The user's cooperative memberships (M1-P0, PD-02). Global
    /// `permissions`/`roles` above remain until P1 scopes them per
    /// cooperative; this block is the membership truth.
    pub cooperatives: Vec<crate::tenant::repo::CooperativeMembershipSummary>,
    /// Session's default cooperative (UX convenience, M1-K1).
    pub active_cooperative_id: Option<Uuid>,
    /// Whether the session owner holds platform authority — an active
    /// NULL-scope `users.manage` grant (M1). Drives whether the
    /// frontend surfaces global-account controls; backend enforcement
    /// is the definer gate, never this hint.
    pub platform_admin: bool,
    /// Whether the session owner holds NULL-scope `cooperatives.manage`
    /// (M1-P7-B). Drives the platform cooperative-lifecycle UI only;
    /// backend enforcement is `app_has_platform_permission` inside the
    /// lifecycle definers, never this hint.
    pub platform_cooperatives_manage: bool,
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
    let now = state.auth.clock.now();
    let normalized = match crate::auth::identity::normalize_and_validate_username(&body.username) {
        Ok(normalized) => normalized,
        Err(_) => {
            state.auth.limiter.record_ip_attempt(&client_ip);
            crate::auth::identity::dummy_verify(&state.auth.argon2, &body.password);
            audit::record_login_failed(
                pool,
                login_failed_metadata(&body.username.to_lowercase(), "unknown_username"),
                now,
            )
            .await;
            tracing::info!(outcome = "login_failed", reason = "invalid_username");
            return Err(ApiError::AuthenticationFailed);
        }
    };

    if !state.auth.limiter.check(&normalized, &client_ip).allowed {
        audit::record_login_failed(
            pool,
            login_failed_metadata(&normalized, "rate_limited"),
            now,
        )
        .await;
        tracing::warn!(outcome = "login_rate_limited", "login blocked by limiter");
        return Err(ApiError::RateLimited);
    }
    state.auth.limiter.record_ip_attempt(&client_ip);

    // Credential-bound login: `app_session_open` verifies the password
    // against the DB-side verifier and creates the session atomically.
    // A uniform `None` on failure keeps the endpoint non-oracle while
    // the definer writes the reason-granular LoginFailed audit row
    // itself (M1-P4-P3B).
    let session_token = token::generate_token();
    let csrf_token = token::generate_token();
    let client_label = session::coarse_client_label(
        headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok()),
    );

    let opened = session::open_session(
        pool,
        session::LoginAttempt {
            username: &normalized,
            password: &body.password,
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

    let Some(created) = opened else {
        state.auth.limiter.record_failure(&normalized);
        tracing::info!(outcome = "login_failed", reason = "credentials_rejected");
        return Err(ApiError::AuthenticationFailed);
    };

    // Success: fresh session (fixation-safe: brand-new token, the
    // attacker-supplied cookie value is never reused), limiter recovery.
    state.auth.limiter.on_success(&normalized);

    audit::record_platform_session(
        pool,
        &session_token.hash,
        audit::SecurityEventType::LoginSucceeded,
        serde_json::json!({ "username": normalized }),
        now,
    )
    .await;
    tracing::info!(outcome = "login_succeeded", user_id = %created.user_id, session_id = %created.session_id);

    let cookie = cookies::build_session_cookie(
        &session_token.raw,
        state.auth.config.cookie_secure,
        state.auth.config.session_absolute_ttl_secs,
    );

    let (permissions, roles) = authorization_context(
        pool,
        &session_token.hash,
        created.active_cooperative_id,
        now,
    )
    .await?;
    let platform_admin = is_platform_admin(pool, &session_token.hash, now).await?;
    let platform_cooperatives_manage =
        has_platform_cooperatives_manage(pool, created.user_id).await?;
    let cooperatives = crate::tenant::repo::list_user_cooperatives(pool, &session_token.hash, now)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "cooperative list failed at login");
            ApiError::Internal
        })?;

    let body = AuthResponse {
        user: UserSummary {
            id: created.user_id,
            username: created.username.clone(),
            display_name: created.display_name.clone(),
        },
        session: SessionInfo {
            id: created.session_id,
            created_at: created.created_at,
            expires_at: created.expires_at,
        },
        csrf_token: csrf_token.raw,
        permissions,
        roles,
        cooperatives,
        active_cooperative_id: created.active_cooperative_id,
        platform_admin,
        platform_cooperatives_manage,
    };

    Ok((StatusCode::OK, [(header::SET_COOKIE, cookie)], Json(body)).into_response())
}

/// Effective permission keys + safe active-role summaries for the
/// authorization context (STEP-003 §21). Errors propagate: an auth
/// context answer must never silently report an empty permission set.
///
/// M1-P1-c3a: after bootstrap the hint set resolves in the VERIFIED
/// bootstrap scope (NULL-attribution included, active bootstrap
/// membership required) — these hints describe exactly what backend
/// business authorization would grant, so a foreign-cooperative role
/// can never imply authority here. The global set is retained only for
/// the provably never-bootstrapped lane; tenant residue without a
/// bootstrap marker is a broken state and fails loud rather than
/// overstating authority.
///
/// M1-P4-S2A: the resolvers are credential-bound — the principal is
/// derived from the session token hash inside the database
/// (`app_session_*` definers), never from a caller-supplied `user_id`.
async fn authorization_context(
    pool: &sqlx::PgPool,
    session_token_hash: &[u8; 32],
    active_cooperative_id: Option<Uuid>,
    now: OffsetDateTime,
) -> Result<(Vec<String>, Vec<RoleSummaryDto>), ApiError> {
    let internal = |context: &'static str| {
        move |error: sqlx::Error| {
            tracing::error!(error = %error, context);
            ApiError::Internal
        }
    };
    let bootstrap = crate::tenant::repo::resolve_bootstrap(pool)
        .await
        .map_err(internal("bootstrap lookup failed"))?;
    // M1-P6 B4: hints follow the SESSION scope — the active
    // cooperative, or bootstrap while the session never switched —
    // exactly the scope BusinessAuth will authorize against.
    let scope = active_cooperative_id.or(bootstrap.map(|(id, _)| id));
    let (permissions, roles) = match scope {
        Some(cooperative_id) => (
            crate::auth::authz::session_permissions_scoped(
                pool,
                session_token_hash,
                cooperative_id,
                now,
            )
            .await
            .map_err(internal("scoped permission resolution failed"))?,
            crate::auth::authz::session_role_summaries_scoped(
                pool,
                session_token_hash,
                cooperative_id,
                now,
            )
            .await
            .map_err(internal("scoped role resolution failed"))?,
        ),
        None => {
            if !crate::tenant::repo::never_bootstrapped(pool)
                .await
                .map_err(internal("never-bootstrapped check failed"))?
            {
                tracing::warn!("authorization context: tenant residue without bootstrap marker");
                return Err(ApiError::Internal);
            }
            (
                crate::auth::authz::session_permissions_legacy(pool, session_token_hash, now)
                    .await
                    .map_err(internal("permission resolution failed"))?,
                crate::auth::authz::session_role_summaries_legacy(pool, session_token_hash, now)
                    .await
                    .map_err(internal("role resolution failed"))?,
            )
        }
    };
    let mut permissions: Vec<String> = permissions.into_iter().collect();
    permissions.sort();
    let roles = roles
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
    let (permissions, roles) = authorization_context(
        pool,
        &auth.session_token_hash,
        auth.active_cooperative_id,
        auth.authed_at,
    )
    .await?;
    let platform_admin = is_platform_admin(pool, &auth.session_token_hash, auth.authed_at).await?;
    let platform_cooperatives_manage = has_platform_cooperatives_manage(pool, auth.user_id).await?;
    let cooperatives =
        crate::tenant::repo::list_user_cooperatives(pool, &auth.session_token_hash, auth.authed_at)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "cooperative list failed");
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
        cooperatives,
        active_cooperative_id: auth.active_cooperative_id,
        platform_admin,
        platform_cooperatives_manage,
    }))
}

/// M1: does the session owner hold platform authority (active
/// NULL-scope `users.manage`)? Resolved inside the database through
/// the session-bound definer — a credential-derived check, never a
/// caller-supplied flag.
async fn is_platform_admin(
    pool: &sqlx::PgPool,
    session_token_hash: &[u8],
    now: OffsetDateTime,
) -> Result<bool, ApiError> {
    sqlx::query_scalar::<_, bool>("SELECT public.app_session_is_platform_admin($1, $2)")
        .bind(session_token_hash)
        .bind(now)
        .fetch_one(pool)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "platform admin check failed");
            ApiError::Internal
        })
}

/// M1-P7-B: does the session owner hold an effective NULL-scope
/// `cooperatives.manage` grant? `p_user_id` is the credential-derived
/// session owner (extractor/creation path), never client input.
/// `app_has_platform_permission` requires an active NULL-scope role +
/// NULL-scope assignment + active user, so cooperative-scoped or forged
/// grants can never satisfy it; a disabled owner resolves to false.
async fn has_platform_cooperatives_manage(
    pool: &sqlx::PgPool,
    user_id: Uuid,
) -> Result<bool, ApiError> {
    sqlx::query_scalar::<_, bool>(
        "SELECT public.app_has_platform_permission($1, 'cooperatives.manage')",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "platform cooperatives.manage check failed");
        ApiError::Internal
    })
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

    let Ok(session_row) = classified else {
        return Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, clearing)]).into_response());
    };

    // Authenticated mutation: full CSRF validation before revoking.
    csrf::validate_csrf_token(&headers, &session_row.csrf_token)
        .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;

    // The definer writes the `logout` audit row atomically with the
    // revocation itself (S2B) — a revoked session cannot resolve an
    // owner, so no post-revocation pool write could ever record it.
    session::revoke_session(pool, token_hash, session_row.session_id, "logout", now)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "logout: revocation failed");
            ApiError::Internal
        })?;

    tracing::info!(outcome = "logout", user_id = %session_row.user_id, session_id = %session_row.session_id);

    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, clearing)]).into_response())
}

pub async fn list_sessions(
    State(state): State<AppState>,
    auth: CurrentAuth,
) -> Result<Json<SessionsResponse>, ApiError> {
    let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;
    let rows = session::list_active_sessions(pool, auth.session_token_hash, state.auth.clock.now())
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
            current: row.is_current,
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

    // Credential-bound revocation: the definer resolves the hash owner
    // and distinguishes own-revoked (idempotent 204) from
    // foreign/unknown (indistinguishable 404) server-side.
    let outcome = session::revoke_session(
        pool,
        auth.session_token_hash,
        session_id,
        "user_revoked",
        now,
    )
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "session revocation failed");
        ApiError::Internal
    })?;

    if outcome != session::RevokeOutcome::Revoked {
        if outcome == session::RevokeOutcome::NotFound {
            tracing::info!(outcome = "session_revoke_denied", target_session = %session_id);
            return Err(ApiError::NotFound);
        }
        return Ok(StatusCode::NO_CONTENT.into_response());
    }

    // `session_revoked` is written inside `app_session_revoke` — atomic
    // with the revocation (S2B).
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
        auth.session_token_hash,
        "user_revoked_others",
        now,
    )
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "revoke-others failed");
        ApiError::Internal
    })?;

    // `sessions_revoked_all_others` is written inside
    // `app_session_revoke_others` — atomic with the revocation (S2B).
    tracing::info!(outcome = "sessions_revoked_all_others", user_id = %auth.user_id, count);

    Ok(StatusCode::NO_CONTENT.into_response())
}
