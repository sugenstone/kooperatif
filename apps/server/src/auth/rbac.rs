//! RBAC management API (STEP-003 §24–§27).
//!
//! Endpoints (all server-side authorized; mutations pass the STEP-002
//! CSRF layers; responses are no-store):
//!
//! - `GET    /api/permissions`              (roles.read)  catalog, read-only
//! - `GET    /api/roles`                    (roles.read)
//! - `POST   /api/roles`                    (roles.manage)
//! - `GET    /api/roles/{id}`               (roles.read)
//! - `PATCH  /api/roles/{id}`               (roles.manage) name/description
//! - `POST   /api/roles/{id}/disable`       (roles.manage) idempotent
//! - `POST   /api/roles/{id}/enable`        (roles.manage) idempotent
//! - `PUT    /api/roles/{id}/permissions`   (roles.manage) replace-set
//! - `GET    /api/users`                    (users.read)
//! - `GET    /api/users/{id}/roles`         (users.read)
//! - `PUT    /api/users/{id}/roles`         (users.manage) replace-set
//!
//! There is intentionally NO `POST /api/permissions`: the permission
//! catalog is application-owned and changes only through migrations
//! (§25/§7). No wildcard or restore permission exists (§16/§30).

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::audit::{self, SecurityEventType};
use crate::auth::authz;
use crate::auth::csrf;
use crate::auth::extractor::CurrentAuth;
use crate::auth::routes::csrf_rejection_to_api_error;
use crate::http::error::ApiError;
use crate::http::AppState;

const ROLE_NAME_MIN_LEN: usize = 2;
const ROLE_NAME_MAX_LEN: usize = 100;

#[derive(Debug, sqlx::FromRow)]
struct RoleRow {
    id: Uuid,
    name: String,
    description: String,
    status: String,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleDto {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub status: String,
    pub permissions: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PermissionDto {
    pub key: String,
    pub name: String,
    pub description: String,
    pub category: String,
}

#[derive(Debug, sqlx::FromRow)]
struct UserListRow {
    id: Uuid,
    username: String,
    display_name: String,
    status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserListItemDto {
    pub id: Uuid,
    pub username: String,
    pub display_name: String,
    pub status: String,
    pub roles: Vec<UserRoleDto>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct UserRoleDto {
    pub id: Uuid,
    pub name: String,
    pub status: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRoleRequest {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRoleRequest {
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionSetRequest {
    pub permissions: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserRoleSetRequest {
    pub role_ids: Vec<Uuid>,
}

pub fn rbac_router() -> Router<AppState> {
    Router::new()
        .route("/api/permissions", get(list_permissions))
        .route("/api/roles", get(list_roles).post(create_role))
        .route("/api/roles/{id}", get(get_role).patch(update_role))
        .route("/api/roles/{id}/disable", post(disable_role))
        .route("/api/roles/{id}/enable", post(enable_role))
        .route(
            "/api/roles/{id}/permissions",
            get(get_role_permissions).put(set_role_permissions),
        )
        .route("/api/users", get(list_users))
        .route("/api/users/{id}/disable", post(disable_user))
        .route("/api/users/{id}/enable", post(enable_user))
        .route(
            "/api/users/{id}/roles",
            get(get_user_roles).put(set_user_roles),
        )
        // Authorization data is per-user sensitive: never cacheable
        // (STEP-003 §48).
        .layer(axum::middleware::from_fn(
            crate::auth::routes::no_store_cache_control,
        ))
}

fn pool_of(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state.db.as_ref().ok_or(ApiError::DependencyUnavailable)
}

/// All mutating RBAC handlers share this prologue: CSRF Origin check
/// (layer 1) + synchronizer token check (layer 2) + the required
/// permission (§47). $method is the literal HTTP method of the route.
macro_rules! guarded_mutation {
    ($state:expr, $headers:expr, $auth:expr, $permission:expr, $method:expr) => {{
        csrf::validate_origin($headers, $method, &$state.auth.config.allowed_origins)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        csrf::validate_csrf_token($headers, &$auth.csrf_token)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        authz::require($state, $auth, $permission).await?;
        pool_of($state)?
    }};
}

pub async fn list_permissions(
    State(state): State<AppState>,
    auth: CurrentAuth,
) -> Result<Json<Vec<PermissionDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::ROLES_READ).await?;
    let pool = pool_of(&state)?;
    let rows = sqlx::query_as::<_, PermissionDto>(
        "SELECT key, name, description, category FROM permissions ORDER BY category, key",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "permission catalog read failed");
        ApiError::Internal
    })?;
    Ok(Json(rows))
}

async fn load_role_with_permissions(
    pool: &sqlx::PgPool,
    role_id: Uuid,
) -> Result<Option<RoleDto>, sqlx::Error> {
    let Some(role) = sqlx::query_as::<_, RoleRow>(
        "SELECT id, name, description, status, created_at, updated_at FROM roles WHERE id = $1",
    )
    .bind(role_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    let permissions: Vec<String> = sqlx::query_scalar(
        "SELECT p.key FROM role_permissions rp \
         JOIN permissions p ON p.id = rp.permission_id \
         WHERE rp.role_id = $1 ORDER BY p.key",
    )
    .bind(role_id)
    .fetch_all(pool)
    .await?;
    Ok(Some(RoleDto {
        id: role.id,
        name: role.name,
        description: role.description,
        status: role.status,
        permissions,
        created_at: role.created_at,
        updated_at: role.updated_at,
    }))
}

pub async fn list_roles(
    State(state): State<AppState>,
    auth: CurrentAuth,
) -> Result<Json<Vec<RoleDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::ROLES_READ).await?;
    let pool = pool_of(&state)?;
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM roles ORDER BY name")
        .fetch_all(pool)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "role listing failed");
            ApiError::Internal
        })?;
    let mut roles = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(role) = load_role_with_permissions(pool, id)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "role load failed");
                ApiError::Internal
            })?
        {
            roles.push(role);
        }
    }
    Ok(Json(roles))
}

pub async fn get_role(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(role_id): Path<Uuid>,
) -> Result<Json<RoleDto>, ApiError> {
    authz::require(&state, &auth, authz::catalog::ROLES_READ).await?;
    let pool = pool_of(&state)?;
    let dto = load_role_with_permissions(pool, role_id)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "role load failed");
            ApiError::Internal
        })?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(dto))
}

fn validate_role_name(name: &str) -> Result<(), ApiError> {
    let trimmed = name.trim();
    let length = trimmed.chars().count();
    if !(ROLE_NAME_MIN_LEN..=ROLE_NAME_MAX_LEN).contains(&length) {
        return Err(ApiError::ValidationFailed);
    }
    Ok(())
}

pub async fn create_role(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Json(body): Json<CreateRoleRequest>,
) -> Result<Response, ApiError> {
    let pool = guarded_mutation!(
        &state,
        &headers,
        &auth,
        authz::catalog::ROLES_MANAGE,
        &axum::http::Method::POST
    );
    let name = body.name.trim().to_string();
    validate_role_name(&name)?;
    let description = body.description.unwrap_or_default().trim().to_string();

    let role = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO roles (name, description) VALUES ($1, $2) RETURNING id",
    )
    .bind(&name)
    .bind(&description)
    .fetch_one(pool)
    .await
    .map_err(|error| match error {
        sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
            tracing::info!(outcome = "role_duplicate_name", name = %name);
            ApiError::Conflict
        }
        other => {
            tracing::error!(error = %other, "role creation failed");
            ApiError::Internal
        }
    })?;

    audit::record(
        pool,
        SecurityEventType::RoleCreated,
        Some(auth.user_id),
        None,
        serde_json::json!({ "role_id": role, "name": name }),
    )
    .await;
    tracing::info!(outcome = "role_created", role_id = %role, actor = %auth.user_id);

    let dto = load_role_with_permissions(pool, role)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "role reload failed");
            ApiError::Internal
        })?
        .ok_or(ApiError::Internal)?;
    Ok((StatusCode::CREATED, Json(dto)).into_response())
}

pub async fn update_role(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(role_id): Path<Uuid>,
    Json(body): Json<UpdateRoleRequest>,
) -> Result<Json<RoleDto>, ApiError> {
    let pool = guarded_mutation!(
        &state,
        &headers,
        &auth,
        authz::catalog::ROLES_MANAGE,
        &axum::http::Method::PATCH
    );
    if let Some(name) = &body.name {
        validate_role_name(name)?;
    }
    let existing = load_role_with_permissions(pool, role_id)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "role load failed");
            ApiError::Internal
        })?
        .ok_or(ApiError::NotFound)?;

    let name = body.name.map(|n| n.trim().to_string());
    let description = body.description.map(|d| d.trim().to_string());
    sqlx::query_scalar::<_, Uuid>(
        "UPDATE roles SET \
            name = COALESCE($2, name), \
            description = COALESCE($3, description), \
            updated_at = now() \
         WHERE id = $1 RETURNING id",
    )
    .bind(role_id)
    .bind(&name)
    .bind(&description)
    .fetch_one(pool)
    .await
    .map_err(|error| match error {
        sqlx::Error::Database(db_err) if db_err.is_unique_violation() => ApiError::Conflict,
        other => {
            tracing::error!(error = %other, "role update failed");
            ApiError::Internal
        }
    })?;

    audit::record(
        pool,
        SecurityEventType::RoleUpdated,
        Some(auth.user_id),
        None,
        serde_json::json!({
            "role_id": role_id,
            "before": { "name": existing.name, "description": existing.description },
            "after": { "name": name, "description": description }
        }),
    )
    .await;
    tracing::info!(outcome = "role_updated", role_id = %role_id, actor = %auth.user_id);

    let dto = load_role_with_permissions(pool, role_id)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "role reload failed");
            ApiError::Internal
        })?
        .ok_or(ApiError::Internal)?;
    Ok(Json(dto))
}

pub async fn disable_role(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(role_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let pool = guarded_mutation!(
        &state,
        &headers,
        &auth,
        authz::catalog::ROLES_MANAGE,
        &axum::http::Method::POST
    );
    set_role_status(&state, pool, &auth, role_id, "disabled").await
}

pub async fn enable_role(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(role_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let pool = guarded_mutation!(
        &state,
        &headers,
        &auth,
        authz::catalog::ROLES_MANAGE,
        &axum::http::Method::POST
    );
    set_role_status(&state, pool, &auth, role_id, "active").await
}

async fn set_role_status(
    _state: &AppState,
    pool: &sqlx::PgPool,
    auth: &CurrentAuth,
    role_id: Uuid,
    status: &str,
) -> Result<Response, ApiError> {
    // Guarded transaction: disabling a role can remove the last
    // administration path (§28/§44).
    let mut tx = authz::begin_guarded_transaction(pool)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "guard tx failed");
            ApiError::Internal
        })?;

    let updated = sqlx::query(
        "UPDATE roles SET status = $2, updated_at = now() WHERE id = $1 AND status <> $2",
    )
    .bind(role_id)
    .bind(status)
    .execute(tx.as_mut())
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "role status change failed");
        ApiError::Internal
    })?;
    if updated.rows_affected() == 0 {
        // Unknown role, or already in the target state (idempotent).
        tx.rollback().await.map_err(|error| {
            tracing::error!(error = %error, "rollback failed");
            ApiError::Internal
        })?;
        let exists: Option<bool> = sqlx::query_scalar("SELECT TRUE FROM roles WHERE id = $1")
            .bind(role_id)
            .fetch_optional(pool)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "role existence check failed");
                ApiError::Internal
            })?;
        if exists.is_none() {
            return Err(ApiError::NotFound);
        }
        return Ok(StatusCode::NO_CONTENT.into_response());
    }

    if status == "disabled"
        && authz::administration_path_count(&mut tx)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "admin path count failed");
                ApiError::Internal
            })?
            == 0
    {
        tx.rollback().await.map_err(|error| {
            tracing::error!(error = %error, "rollback failed");
            ApiError::Internal
        })?;
        audit_lockout_prevention(pool, auth, "role_disable", role_id).await;
        return Err(ApiError::LockoutPrevented);
    }

    tx.commit().await.map_err(|error| {
        tracing::error!(error = %error, "commit failed");
        ApiError::Internal
    })?;

    audit::record(
        pool,
        if status == "disabled" {
            SecurityEventType::RoleDisabled
        } else {
            SecurityEventType::RoleEnabled
        },
        Some(auth.user_id),
        None,
        serde_json::json!({ "role_id": role_id }),
    )
    .await;
    tracing::info!(outcome = "role_status_changed", role_id = %role_id, status, actor = %auth.user_id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn disable_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(user_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let pool = guarded_mutation!(
        &state,
        &headers,
        &auth,
        authz::catalog::USERS_MANAGE,
        &axum::http::Method::POST
    );
    set_user_status(pool, &auth, user_id, "disabled").await
}

pub async fn enable_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(user_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let pool = guarded_mutation!(
        &state,
        &headers,
        &auth,
        authz::catalog::USERS_MANAGE,
        &axum::http::Method::POST
    );
    set_user_status(pool, &auth, user_id, "active").await
}

async fn set_user_status(
    pool: &sqlx::PgPool,
    auth: &CurrentAuth,
    user_id: Uuid,
    status: &str,
) -> Result<Response, ApiError> {
    // Guarded transaction: disabling a user can remove the last
    // administration path (§28/§44) — the count already excludes
    // non-active users, so the check runs inside the same tx that
    // performs the status change.
    let mut tx = authz::begin_guarded_transaction(pool)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "guard tx failed");
            ApiError::Internal
        })?;

    let updated = sqlx::query(
        "UPDATE users SET status = $2, updated_at = now() WHERE id = $1 AND status <> $2",
    )
    .bind(user_id)
    .bind(status)
    .execute(tx.as_mut())
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "user status change failed");
        ApiError::Internal
    })?;
    if updated.rows_affected() == 0 {
        // Unknown user, or already in the target state (idempotent).
        tx.rollback().await.map_err(|error| {
            tracing::error!(error = %error, "rollback failed");
            ApiError::Internal
        })?;
        let exists: Option<bool> = sqlx::query_scalar("SELECT TRUE FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "user existence check failed");
                ApiError::Internal
            })?;
        if exists.is_none() {
            return Err(ApiError::NotFound);
        }
        return Ok(StatusCode::NO_CONTENT.into_response());
    }

    if status == "disabled" {
        // Immediate explicit revocation on top of the validation-time
        // rejection: active sessions (HTTP + WS) die at once rather
        // than on next classify.
        sqlx::query(
            "UPDATE user_sessions SET revoked_at = now(), revocation_reason = 'user_disabled' \
             WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .execute(tx.as_mut())
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "session revocation failed");
            ApiError::Internal
        })?;
    }

    if status == "disabled"
        && authz::administration_path_count(&mut tx)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "admin path count failed");
                ApiError::Internal
            })?
            == 0
    {
        tx.rollback().await.map_err(|error| {
            tracing::error!(error = %error, "rollback failed");
            ApiError::Internal
        })?;
        audit_lockout_prevention(pool, auth, "user_disable", user_id).await;
        return Err(ApiError::LockoutPrevented);
    }

    tx.commit().await.map_err(|error| {
        tracing::error!(error = %error, "commit failed");
        ApiError::Internal
    })?;

    audit::record(
        pool,
        if status == "disabled" {
            SecurityEventType::UserDisabled
        } else {
            SecurityEventType::UserEnabled
        },
        Some(auth.user_id),
        None,
        serde_json::json!({ "user_id": user_id }),
    )
    .await;
    tracing::info!(outcome = "user_status_changed", user_id = %user_id, status, actor = %auth.user_id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn get_role_permissions(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(role_id): Path<Uuid>,
) -> Result<Json<Vec<String>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::ROLES_READ).await?;
    let pool = pool_of(&state)?;
    let keys: Option<Vec<String>> = sqlx::query_scalar(
        "SELECT array_agg(p.key ORDER BY p.key) FROM role_permissions rp \
         JOIN permissions p ON p.id = rp.permission_id WHERE rp.role_id = $1",
    )
    .bind(role_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "role permission read failed");
        ApiError::Internal
    })?;
    // Distinguishing "role exists with zero permissions" from "role
    // unknown": both return keys via load check.
    let exists: Option<bool> = sqlx::query_scalar("SELECT TRUE FROM roles WHERE id = $1")
        .bind(role_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "role existence check failed");
            ApiError::Internal
        })?;
    if exists.is_none() {
        return Err(ApiError::NotFound);
    }
    Ok(Json(keys.unwrap_or_default()))
}

pub async fn set_role_permissions(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(role_id): Path<Uuid>,
    Json(body): Json<PermissionSetRequest>,
) -> Result<Response, ApiError> {
    let pool = guarded_mutation!(
        &state,
        &headers,
        &auth,
        authz::catalog::ROLES_MANAGE,
        &axum::http::Method::PUT
    );

    // Only known catalog keys are accepted: application owns the
    // permission namespace (§7/§26). Duplicates in the request collapse
    // into a set.
    let requested: std::collections::BTreeSet<String> = body.permissions.into_iter().collect();
    for key in &requested {
        if !authz::catalog::is_known(key) {
            tracing::info!(outcome = "unknown_permission_rejected", key = %key);
            return Err(ApiError::ValidationFailed);
        }
    }

    let mut tx = authz::begin_guarded_transaction(pool)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "guard tx failed");
            ApiError::Internal
        })?;

    let exists: Option<bool> = sqlx::query_scalar("SELECT TRUE FROM roles WHERE id = $1")
        .bind(role_id)
        .fetch_optional(tx.as_mut())
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "role existence check failed");
            ApiError::Internal
        })?;
    if exists.is_none() {
        tx.rollback().await.map_err(|error| {
            tracing::error!(error = %error, "rollback failed");
            ApiError::Internal
        })?;
        return Err(ApiError::NotFound);
    }

    let before: Vec<String> = sqlx::query_scalar(
        "SELECT p.key FROM role_permissions rp \
         JOIN permissions p ON p.id = rp.permission_id \
         WHERE rp.role_id = $1 ORDER BY p.key",
    )
    .bind(role_id)
    .fetch_all(tx.as_mut())
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "before-state read failed");
        ApiError::Internal
    })?;

    sqlx::query("DELETE FROM role_permissions WHERE role_id = $1")
        .bind(role_id)
        .execute(tx.as_mut())
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "permission set clear failed");
            ApiError::Internal
        })?;
    for key in &requested {
        let inserted = sqlx::query(
            "INSERT INTO role_permissions (role_id, permission_id) \
             SELECT $1, id FROM permissions WHERE key = $2",
        )
        .bind(role_id)
        .bind(key)
        .execute(tx.as_mut())
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "permission insert failed");
            ApiError::Internal
        })?;
        if inserted.rows_affected() == 0 {
            // Catalog drifted from the application constant list — refuse
            // rather than silently granting/partially applying.
            tx.rollback().await.map_err(|error| {
                tracing::error!(error = %error, "rollback failed");
                ApiError::Internal
            })?;
            tracing::error!(key = %key, "catalog key missing in database");
            return Err(ApiError::Internal);
        }
    }

    // Lockout guard: stripping the administration permission from the
    // last administrative role must not be possible (§28).
    if authz::administration_path_count(&mut tx)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "admin path count failed");
            ApiError::Internal
        })?
        == 0
    {
        tx.rollback().await.map_err(|error| {
            tracing::error!(error = %error, "rollback failed");
            ApiError::Internal
        })?;
        audit_lockout_prevention(pool, &auth, "role_permissions_change", role_id).await;
        return Err(ApiError::LockoutPrevented);
    }

    tx.commit().await.map_err(|error| {
        tracing::error!(error = %error, "commit failed");
        ApiError::Internal
    })?;

    audit::record(
        pool,
        SecurityEventType::RolePermissionsChanged,
        Some(auth.user_id),
        None,
        serde_json::json!({
            "role_id": role_id,
            "before": before,
            "after": requested.iter().cloned().collect::<Vec<_>>()
        }),
    )
    .await;
    tracing::info!(outcome = "role_permissions_changed", role_id = %role_id, count = requested.len(), actor = %auth.user_id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn list_users(
    State(state): State<AppState>,
    auth: CurrentAuth,
) -> Result<Json<Vec<UserListItemDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::USERS_READ).await?;
    let pool = pool_of(&state)?;
    let rows = sqlx::query_as::<_, UserListRow>(
        "SELECT id, username, display_name, status FROM users ORDER BY display_name, username",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "user listing failed");
        ApiError::Internal
    })?;

    let mut users = Vec::with_capacity(rows.len());
    for row in rows {
        let roles = sqlx::query_as::<_, UserRoleDto>(
            "SELECT r.id, r.name, r.status \
             FROM user_role_assignments ura \
             JOIN roles r ON r.id = ura.role_id \
             WHERE ura.user_id = $1 ORDER BY r.name",
        )
        .bind(row.id)
        .fetch_all(pool)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "user roles read failed");
            ApiError::Internal
        })?;
        users.push(UserListItemDto {
            id: row.id,
            username: row.username,
            display_name: row.display_name,
            status: row.status,
            roles,
        });
    }
    Ok(Json(users))
}

pub async fn get_user_roles(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(user_id): Path<Uuid>,
) -> Result<Json<Vec<UserRoleDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::USERS_READ).await?;
    let pool = pool_of(&state)?;
    let exists: Option<bool> = sqlx::query_scalar("SELECT TRUE FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "user existence check failed");
            ApiError::Internal
        })?;
    if exists.is_none() {
        return Err(ApiError::NotFound);
    }
    let roles = sqlx::query_as::<_, UserRoleDto>(
        "SELECT r.id, r.name, r.status \
         FROM user_role_assignments ura \
         JOIN roles r ON r.id = ura.role_id \
         WHERE ura.user_id = $1 ORDER BY r.name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "user roles read failed");
        ApiError::Internal
    })?;
    Ok(Json(roles))
}

pub async fn set_user_roles(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(user_id): Path<Uuid>,
    Json(body): Json<UserRoleSetRequest>,
) -> Result<Response, ApiError> {
    let pool = guarded_mutation!(
        &state,
        &headers,
        &auth,
        authz::catalog::USERS_MANAGE,
        &axum::http::Method::PUT
    );
    let requested: std::collections::BTreeSet<Uuid> = body.role_ids.into_iter().collect();

    let mut tx = authz::begin_guarded_transaction(pool)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "guard tx failed");
            ApiError::Internal
        })?;

    let target_exists: Option<bool> = sqlx::query_scalar("SELECT TRUE FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(tx.as_mut())
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "user existence check failed");
            ApiError::Internal
        })?;
    if target_exists.is_none() {
        tx.rollback().await.map_err(|error| {
            tracing::error!(error = %error, "rollback failed");
            ApiError::Internal
        })?;
        return Err(ApiError::NotFound);
    }

    // All requested roles must exist; assigning a disabled role is a
    // validation error (policy: disabled roles grant nothing —
    // assignment would be misleading). Removal of disabled roles is
    // allowed.
    for role_id in &requested {
        let status: Option<String> = sqlx::query_scalar("SELECT status FROM roles WHERE id = $1")
            .bind(role_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "role status read failed");
                ApiError::Internal
            })?;
        match status {
            None => {
                tx.rollback().await.map_err(|error| {
                    tracing::error!(error = %error, "rollback failed");
                    ApiError::Internal
                })?;
                tracing::info!(outcome = "unknown_role_rejected", role_id = %role_id);
                return Err(ApiError::ValidationFailed);
            }
            Some(status) if status != "active" => {
                tx.rollback().await.map_err(|error| {
                    tracing::error!(error = %error, "rollback failed");
                    ApiError::Internal
                })?;
                tracing::info!(outcome = "disabled_role_assignment_rejected", role_id = %role_id);
                return Err(ApiError::ValidationFailed);
            }
            Some(_) => {}
        }
    }

    let before: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT r.id, r.name FROM user_role_assignments ura \
         JOIN roles r ON r.id = ura.role_id WHERE ura.user_id = $1 ORDER BY r.name",
    )
    .bind(user_id)
    .fetch_all(tx.as_mut())
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "before-state read failed");
        ApiError::Internal
    })?;

    sqlx::query("DELETE FROM user_role_assignments WHERE user_id = $1")
        .bind(user_id)
        .execute(tx.as_mut())
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "assignment clear failed");
            ApiError::Internal
        })?;
    for role_id in &requested {
        sqlx::query("INSERT INTO user_role_assignments (user_id, role_id) VALUES ($1, $2)")
            .bind(user_id)
            .bind(role_id)
            .execute(tx.as_mut())
            .await
            .map_err(|error| match error {
                sqlx::Error::Database(db_err) if db_err.is_unique_violation() => ApiError::Conflict,
                other => {
                    tracing::error!(error = %other, "assignment insert failed");
                    ApiError::Internal
                }
            })?;
    }

    // Lockout guard: removing the last administrative user's path.
    if authz::administration_path_count(&mut tx)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "admin path count failed");
            ApiError::Internal
        })?
        == 0
    {
        tx.rollback().await.map_err(|error| {
            tracing::error!(error = %error, "rollback failed");
            ApiError::Internal
        })?;
        audit_lockout_prevention(pool, &auth, "user_roles_change", user_id).await;
        return Err(ApiError::LockoutPrevented);
    }

    tx.commit().await.map_err(|error| {
        tracing::error!(error = %error, "commit failed");
        ApiError::Internal
    })?;

    // Audited as explicit assigned/unassigned deltas (durable history).
    let before_ids: std::collections::BTreeSet<Uuid> = before.iter().map(|(id, _)| *id).collect();
    for (role_id, role_name) in &before {
        if !requested.contains(role_id) {
            audit::record(
                pool,
                SecurityEventType::RoleUnassigned,
                Some(auth.user_id),
                None,
                serde_json::json!({
                    "target_user_id": user_id,
                    "role_id": role_id,
                    "role_name": role_name
                }),
            )
            .await;
        }
    }
    for role_id in &requested {
        if !before_ids.contains(role_id) {
            let name: String = sqlx::query_scalar("SELECT name FROM roles WHERE id = $1")
                .bind(role_id)
                .fetch_one(pool)
                .await
                .unwrap_or_else(|_| "bilinmeyen".to_string());
            audit::record(
                pool,
                SecurityEventType::RoleAssigned,
                Some(auth.user_id),
                None,
                serde_json::json!({
                    "target_user_id": user_id,
                    "role_id": role_id,
                    "role_name": name
                }),
            )
            .await;
        }
    }
    tracing::info!(outcome = "user_roles_set", target_user = %user_id, count = requested.len(), actor = %auth.user_id);

    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn audit_lockout_prevention(
    pool: &sqlx::PgPool,
    auth: &CurrentAuth,
    operation: &str,
    target_id: Uuid,
) {
    tracing::warn!(outcome = "lockout_prevented", operation, target = %target_id, actor = %auth.user_id);
    audit::record(
        pool,
        SecurityEventType::LockoutPrevented,
        Some(auth.user_id),
        None,
        serde_json::json!({ "operation": operation, "target_id": target_id }),
    )
    .await;
}
