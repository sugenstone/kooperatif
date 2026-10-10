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
//!
//! M1-P1-c2a — cooperative scoping: reads and the safe mutations
//! (create/update/enable) run under `BusinessAuth` +
//! `require_business`, resolving rows with verified bootstrap
//! attribution (see `scoped_visibility`), while another cooperative's
//! rows are invisible (404, never 403 — no existence leak). New roles
//! are written WITH `cooperative_id` — no new NULL role rows.
//!
//! M1-P1-c2b — the four administration-path-sensitive mutations
//! (`disable_role`, `PUT /roles/{id}/permissions`, `disable_user`,
//! `PUT /users/{id}/roles`) are now migrated to the same verified
//! context and additionally run inside a COOPERATIVE-SCOPED guarded
//! transaction (P1-K5, ADR-006):
//!
//! - `authz::begin_guarded_transaction(pool, scope)` takes a
//!   cooperative-keyed PostgreSQL advisory lock — administrator
//!   continuity mutations serialize per cooperative; A never blocks or
//!   counts B.
//! - `authz::administration_path_count(tx, scope)` counts effective
//!   `roles.manage` paths INSIDE the verified scope only (membership +
//!   permission key, never role names), evaluated on the post-mutation
//!   state in the same transaction.
//! - `set_user_roles` writes `cooperative_id = scope` — the API no
//!   longer creates NULL-cooperative assignments. NULL-attributed
//!   legacy roles stay VISIBLE and manageable in the bootstrap scope
//!   but are NOT assignable: the composite FK can only pair
//!   scope-owned roles with scoped assignments. They become assignable
//!   after the P1-c4 contract migration backfills them.
//!
//! Recovery procedure (documented, unchanged): if every administration
//! path were ever lost, the CLI-only `grant-role` path (STEP-003 §14,
//! audited as `bootstrap_role_granted`) restores one; its NULL-cooperative
//! writes remain bootstrap-attributed and therefore effective in the
//! bootstrap scope until the P1-d/--cooperative migration contracts it.

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
use crate::tenant::BusinessAuth;

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
#[serde(deny_unknown_fields)]
pub struct CreateRoleRequest {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct UpdateRoleRequest {
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct PermissionSetRequest {
    pub permissions: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
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

/// Verified-scope row visibility predicate (M1-P1-c2b mandatory audit).
///
/// Produces the canonical SQL fragment for `col` — a `cooperative_id`
/// column — against the verified scope bound at `$idx`. It encodes the
/// bootstrap restriction INSIDE the query so even an incorrectly or
/// maliciously supplied scope value can never widen visibility:
///
/// - `$idx` = bootstrap cooperative → the cooperative's own rows plus
///   NULL-attributed legacy rows — NULL attribution fires only because
///   the marker (`installation_state`) AND flag (`is_bootstrap`) agree
///   that `$idx` IS the bootstrap cooperative at evaluation time;
/// - `$idx` = any other cooperative → own rows only; a non-bootstrap
///   scope can NEVER inherit NULL records;
/// - `$idx` = NULL → unrestricted ONLY while the installation provably
///   predates tenancy (zero cooperatives ⇒ every row is necessarily
///   NULL anyway, so "unrestricted" ≡ the never-bootstrapped lane).
///   After bootstrap a NULL scope matches nothing — a NULL scope can
///   never mean unrestricted access once tenancy exists.
///
/// `col` is always a compile-time column reference from the call site;
/// `idx` is the 1-based position of the scope bind. No user input ever
/// enters the generated fragment.
fn scoped_visibility(col: &str, idx: usize) -> String {
    format!(
        "({col} = ${i} \
         OR ({col} IS NULL AND EXISTS(\
             SELECT 1 FROM installation_state s \
             JOIN cooperatives c \
               ON c.id = s.bootstrap_cooperative_id AND c.is_bootstrap \
             WHERE s.bootstrap_cooperative_id = ${i})) \
         OR (${i}::uuid IS NULL AND NOT EXISTS (SELECT 1 FROM cooperatives)))",
        col = col,
        i = idx
    )
}

/// Verified-scope USER visibility predicate (same audit rules as
/// `scoped_visibility`). A user row is administrable in a cooperative
/// scope iff it holds a membership row there — ownership evidence at
/// any status; the ACTOR's active membership is enforced separately by
/// `BusinessAuth` extraction and the resolver join. NULL scope ⇒ all
/// users, but only while zero cooperatives exist (never-bootstrapped
/// lane) — after bootstrap a NULL scope matches nothing.
fn member_visibility(user_expr: &str, idx: usize) -> String {
    format!(
        "((${i}::uuid IS NULL AND NOT EXISTS (SELECT 1 FROM cooperatives)) \
         OR EXISTS(SELECT 1 FROM cooperative_memberships m \
                   WHERE m.user_id = {u} AND m.cooperative_id = ${i}))",
        u = user_expr,
        i = idx
    )
}

/// All mutating RBAC handlers share this prologue: CSRF Origin check
/// (layer 1) + synchronizer token check (layer 2) + the required
/// permission resolved through `require_business` (bootstrap-pinned
/// `BusinessAuth`, verified legacy attribution — never a raw header and
/// never a global-permission fallback). $method is the literal HTTP
/// method of the route.
macro_rules! guarded_business_mutation {
    ($state:expr, $headers:expr, $ctx:expr, $permission:expr, $method:expr) => {{
        csrf::validate_origin($headers, $method, &$state.auth.config.allowed_origins)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        csrf::validate_csrf_token($headers, &$ctx.auth.csrf_token)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        authz::require_business($state, $ctx, $permission).await?;
        pool_of($state)?
    }};
}

pub async fn list_permissions(
    State(state): State<AppState>,
    ctx: BusinessAuth,
) -> Result<Json<Vec<PermissionDto>>, ApiError> {
    authz::require_business(&state, &ctx, authz::catalog::ROLES_READ).await?;
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

/// Role load through the verified scope (`scoped_visibility`): in the
/// bootstrap scope a role is visible iff it is bootstrap-owned OR
/// NULL-attributed (legacy lane); in the never-bootstrapped lane scope
/// is NULL and every (necessarily NULL) row matches — preserving the
/// legacy global view.
async fn load_role_with_permissions(
    pool: &sqlx::PgPool,
    role_id: Uuid,
    scope: Option<Uuid>,
) -> Result<Option<RoleDto>, sqlx::Error> {
    let Some(role) = sqlx::query_as::<_, RoleRow>(&format!(
        "SELECT id, name, description, status, created_at, updated_at \
         FROM roles \
         WHERE id = $1 AND {}",
        scoped_visibility("cooperative_id", 2)
    ))
    .bind(role_id)
    .bind(scope)
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
    ctx: BusinessAuth,
) -> Result<Json<Vec<RoleDto>>, ApiError> {
    authz::require_business(&state, &ctx, authz::catalog::ROLES_READ).await?;
    let pool = pool_of(&state)?;
    let scope = ctx.cooperative_id();
    let ids: Vec<Uuid> = sqlx::query_scalar(&format!(
        "SELECT id FROM roles WHERE {} ORDER BY name",
        scoped_visibility("cooperative_id", 1)
    ))
    .bind(scope)
    .fetch_all(pool)
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "role listing failed");
        ApiError::Internal
    })?;
    let mut roles = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(role) = load_role_with_permissions(pool, id, scope)
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
    ctx: BusinessAuth,
    Path(role_id): Path<Uuid>,
) -> Result<Json<RoleDto>, ApiError> {
    authz::require_business(&state, &ctx, authz::catalog::ROLES_READ).await?;
    let pool = pool_of(&state)?;
    let dto = load_role_with_permissions(pool, role_id, ctx.cooperative_id())
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
    ctx: BusinessAuth,
    Json(body): Json<CreateRoleRequest>,
) -> Result<Response, ApiError> {
    let pool = guarded_business_mutation!(
        &state,
        &headers,
        &ctx,
        authz::catalog::ROLES_MANAGE,
        &axum::http::Method::POST
    );
    let name = body.name.trim().to_string();
    validate_role_name(&name)?;
    let description = body.description.unwrap_or_default().trim().to_string();
    let scope = ctx.cooperative_id();

    // P1-c2a: new roles carry the verified cooperative — no new NULL
    // role rows from this path (NULL only in the never-bootstrapped
    // lane, where no cooperative exists yet).
    let role = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO roles (name, description, cooperative_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(&name)
    .bind(&description)
    .bind(scope)
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

    audit::record_scoped(
        pool,
        SecurityEventType::RoleCreated,
        scope,
        Some(ctx.auth.user_id),
        None,
        serde_json::json!({ "role_id": role, "name": name }),
    )
    .await;
    tracing::info!(outcome = "role_created", role_id = %role, actor = %ctx.auth.user_id);

    let dto = load_role_with_permissions(pool, role, scope)
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
    ctx: BusinessAuth,
    Path(role_id): Path<Uuid>,
    Json(body): Json<UpdateRoleRequest>,
) -> Result<Json<RoleDto>, ApiError> {
    let pool = guarded_business_mutation!(
        &state,
        &headers,
        &ctx,
        authz::catalog::ROLES_MANAGE,
        &axum::http::Method::PATCH
    );
    let scope = ctx.cooperative_id();
    if let Some(name) = &body.name {
        validate_role_name(name)?;
    }
    let existing = load_role_with_permissions(pool, role_id, scope)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "role load failed");
            ApiError::Internal
        })?
        .ok_or(ApiError::NotFound)?;

    let name = body.name.map(|n| n.trim().to_string());
    let description = body.description.map(|d| d.trim().to_string());
    sqlx::query_scalar::<_, Uuid>(&format!(
        "UPDATE roles SET \
            name = COALESCE($2, name), \
            description = COALESCE($3, description), \
            updated_at = now() \
         WHERE id = $1 AND {} \
         RETURNING id",
        scoped_visibility("cooperative_id", 4)
    ))
    .bind(role_id)
    .bind(&name)
    .bind(&description)
    .bind(scope)
    .fetch_one(pool)
    .await
    .map_err(|error| match error {
        sqlx::Error::RowNotFound => ApiError::NotFound,
        sqlx::Error::Database(db_err) if db_err.is_unique_violation() => ApiError::Conflict,
        other => {
            tracing::error!(error = %other, "role update failed");
            ApiError::Internal
        }
    })?;

    audit::record_scoped(
        pool,
        SecurityEventType::RoleUpdated,
        scope,
        Some(ctx.auth.user_id),
        None,
        serde_json::json!({
            "role_id": role_id,
            "before": { "name": existing.name, "description": existing.description },
            "after": { "name": name, "description": description }
        }),
    )
    .await;
    tracing::info!(outcome = "role_updated", role_id = %role_id, actor = %ctx.auth.user_id);

    let dto = load_role_with_permissions(pool, role_id, scope)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "role reload failed");
            ApiError::Internal
        })?
        .ok_or(ApiError::Internal)?;
    Ok(Json(dto))
}

/// Disabling a role can remove the scope's last administration path —
/// runs inside the cooperative-scoped guarded transaction (P1-c2b).
pub async fn disable_role(
    State(state): State<AppState>,
    headers: HeaderMap,
    ctx: BusinessAuth,
    Path(role_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let pool = guarded_business_mutation!(
        &state,
        &headers,
        &ctx,
        authz::catalog::ROLES_MANAGE,
        &axum::http::Method::POST
    );
    let scope = ctx.cooperative_id();
    set_role_status(pool, &ctx.auth, role_id, "disabled", scope).await
}

pub async fn enable_role(
    State(state): State<AppState>,
    headers: HeaderMap,
    ctx: BusinessAuth,
    Path(role_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let pool = guarded_business_mutation!(
        &state,
        &headers,
        &ctx,
        authz::catalog::ROLES_MANAGE,
        &axum::http::Method::POST
    );
    let scope = ctx.cooperative_id();
    set_role_status(pool, &ctx.auth, role_id, "active", scope).await
}

/// `scope` limits which role rows the mutation may touch (the verified
/// visibility predicate), selects the cooperative-scoped advisory lock
/// and administration-path count, and attributes the audit event. It is
/// `None` only in the never-bootstrapped lane, where the legacy global
/// lock/count/view apply unchanged.
async fn set_role_status(
    pool: &sqlx::PgPool,
    auth: &CurrentAuth,
    role_id: Uuid,
    status: &str,
    scope: Option<Uuid>,
) -> Result<Response, ApiError> {
    // Scoped guarded transaction: disabling a role can remove the
    // cooperative's last administration path (§28/§44, P1-K5).
    let mut tx = authz::begin_guarded_transaction(pool, scope)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "guard tx failed");
            ApiError::Internal
        })?;

    let updated = sqlx::query(&format!(
        "UPDATE roles SET status = $2, updated_at = now() \
         WHERE id = $1 AND status <> $2 AND {}",
        scoped_visibility("cooperative_id", 3)
    ))
    .bind(role_id)
    .bind(status)
    .bind(scope)
    .execute(tx.as_mut())
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "role status change failed");
        ApiError::Internal
    })?;
    if updated.rows_affected() == 0 {
        // Unknown/out-of-scope role, or already in the target state
        // (idempotent).
        tx.rollback().await.map_err(|error| {
            tracing::error!(error = %error, "rollback failed");
            ApiError::Internal
        })?;
        let exists: Option<bool> = sqlx::query_scalar(&format!(
            "SELECT TRUE FROM roles WHERE id = $1 AND {}",
            scoped_visibility("cooperative_id", 2)
        ))
        .bind(role_id)
        .bind(scope)
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
        && authz::administration_path_count(&mut tx, scope)
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
        audit_lockout_prevention(pool, auth, "role_disable", role_id, scope).await;
        return Err(ApiError::LockoutPrevented);
    }

    tx.commit().await.map_err(|error| {
        tracing::error!(error = %error, "commit failed");
        ApiError::Internal
    })?;

    audit::record_scoped(
        pool,
        if status == "disabled" {
            SecurityEventType::RoleDisabled
        } else {
            SecurityEventType::RoleEnabled
        },
        scope,
        Some(auth.user_id),
        None,
        serde_json::json!({ "role_id": role_id }),
    )
    .await;
    tracing::info!(outcome = "role_status_changed", role_id = %role_id, status, actor = %auth.user_id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Disabling a user can remove the scope's last administration path —
/// runs inside the cooperative-scoped guarded transaction (P1-c2b).
/// The target must hold a membership row in the verified scope — a
/// user who only belongs to another cooperative answers 404.
pub async fn disable_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    ctx: BusinessAuth,
    Path(user_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let pool = guarded_business_mutation!(
        &state,
        &headers,
        &ctx,
        authz::catalog::USERS_MANAGE,
        &axum::http::Method::POST
    );
    let scope = ctx.cooperative_id();
    set_user_status(pool, &ctx.auth, user_id, "disabled", scope).await
}

pub async fn enable_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    ctx: BusinessAuth,
    Path(user_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let pool = guarded_business_mutation!(
        &state,
        &headers,
        &ctx,
        authz::catalog::USERS_MANAGE,
        &axum::http::Method::POST
    );
    let scope = ctx.cooperative_id();
    set_user_status(pool, &ctx.auth, user_id, "active", scope).await
}

/// `scope` limits which target users the mutation may touch to
/// membership-holders of the verified cooperative, selects the
/// cooperative-scoped advisory lock and administration-path count, and
/// attributes the audit event. `None` only in the never-bootstrapped
/// lane (legacy global semantics).
async fn set_user_status(
    pool: &sqlx::PgPool,
    auth: &CurrentAuth,
    user_id: Uuid,
    status: &str,
    scope: Option<Uuid>,
) -> Result<Response, ApiError> {
    // Scoped guarded transaction: disabling a user can remove the
    // cooperative's last administration path (§28/§44, P1-K5) — the
    // count already excludes non-active users, so the check runs inside
    // the same tx that performs the status change.
    let mut tx = authz::begin_guarded_transaction(pool, scope)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "guard tx failed");
            ApiError::Internal
        })?;

    let updated = sqlx::query(&format!(
        "UPDATE users SET status = $2, updated_at = now() \
         WHERE id = $1 AND status <> $2 AND {}",
        member_visibility("$1", 3)
    ))
    .bind(user_id)
    .bind(status)
    .bind(scope)
    .execute(tx.as_mut())
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "user status change failed");
        ApiError::Internal
    })?;
    if updated.rows_affected() == 0 {
        // Unknown/out-of-scope user, or already in the target state
        // (idempotent).
        tx.rollback().await.map_err(|error| {
            tracing::error!(error = %error, "rollback failed");
            ApiError::Internal
        })?;
        let exists: Option<bool> = sqlx::query_scalar(&format!(
            "SELECT TRUE FROM users WHERE id = $1 AND {}",
            member_visibility("$1", 2)
        ))
        .bind(user_id)
        .bind(scope)
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
        && authz::administration_path_count(&mut tx, scope)
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
        audit_lockout_prevention(pool, auth, "user_disable", user_id, scope).await;
        return Err(ApiError::LockoutPrevented);
    }

    tx.commit().await.map_err(|error| {
        tracing::error!(error = %error, "commit failed");
        ApiError::Internal
    })?;

    audit::record_scoped(
        pool,
        if status == "disabled" {
            SecurityEventType::UserDisabled
        } else {
            SecurityEventType::UserEnabled
        },
        scope,
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
    ctx: BusinessAuth,
    Path(role_id): Path<Uuid>,
) -> Result<Json<Vec<String>>, ApiError> {
    authz::require_business(&state, &ctx, authz::catalog::ROLES_READ).await?;
    let pool = pool_of(&state)?;
    let scope = ctx.cooperative_id();
    // Scoped existence first — a foreign role must answer 404 without
    // touching its permission rows.
    let exists: Option<bool> = sqlx::query_scalar(&format!(
        "SELECT TRUE FROM roles WHERE id = $1 AND {}",
        scoped_visibility("cooperative_id", 2)
    ))
    .bind(role_id)
    .bind(scope)
    .fetch_optional(pool)
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "role existence check failed");
        ApiError::Internal
    })?;
    if exists.is_none() {
        return Err(ApiError::NotFound);
    }
    // COALESCE: a role with zero permissions yields a NULL array_agg,
    // not zero rows.
    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT COALESCE(array_agg(p.key ORDER BY p.key), '{}'::text[]) \
         FROM role_permissions rp \
         JOIN permissions p ON p.id = rp.permission_id WHERE rp.role_id = $1",
    )
    .bind(role_id)
    .fetch_one(pool)
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "role permission read failed");
        ApiError::Internal
    })?;
    Ok(Json(keys))
}

/// Stripping the administration permission can remove the scope's last
/// administration path — cooperative-scoped guarded transaction
/// (P1-c2b). The role must be visible in the verified scope (404 for
/// foreign rows — never a cross-cooperative mutation target).
pub async fn set_role_permissions(
    State(state): State<AppState>,
    headers: HeaderMap,
    ctx: BusinessAuth,
    Path(role_id): Path<Uuid>,
    Json(body): Json<PermissionSetRequest>,
) -> Result<Response, ApiError> {
    let pool = guarded_business_mutation!(
        &state,
        &headers,
        &ctx,
        authz::catalog::ROLES_MANAGE,
        &axum::http::Method::PUT
    );
    let scope = ctx.cooperative_id();

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

    let mut tx = authz::begin_guarded_transaction(pool, scope)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "guard tx failed");
            ApiError::Internal
        })?;

    let exists: Option<bool> = sqlx::query_scalar(&format!(
        "SELECT TRUE FROM roles WHERE id = $1 AND {}",
        scoped_visibility("cooperative_id", 2)
    ))
    .bind(role_id)
    .bind(scope)
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
    // scope's last administrative role must not be possible (§28, P1-K5).
    if authz::administration_path_count(&mut tx, scope)
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
        audit_lockout_prevention(pool, &ctx.auth, "role_permissions_change", role_id, scope).await;
        return Err(ApiError::LockoutPrevented);
    }

    tx.commit().await.map_err(|error| {
        tracing::error!(error = %error, "commit failed");
        ApiError::Internal
    })?;

    audit::record_scoped(
        pool,
        SecurityEventType::RolePermissionsChanged,
        scope,
        Some(ctx.auth.user_id),
        None,
        serde_json::json!({
            "role_id": role_id,
            "before": before,
            "after": requested.iter().cloned().collect::<Vec<_>>()
        }),
    )
    .await;
    tracing::info!(outcome = "role_permissions_changed", role_id = %role_id, count = requested.len(), actor = %ctx.auth.user_id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn list_users(
    State(state): State<AppState>,
    ctx: BusinessAuth,
) -> Result<Json<Vec<UserListItemDto>>, ApiError> {
    authz::require_business(&state, &ctx, authz::catalog::USERS_READ).await?;
    let pool = pool_of(&state)?;
    let scope = ctx.cooperative_id();
    // In a cooperative scope only members are administrable; the
    // never-bootstrapped lane (scope NULL) keeps the legacy all-users
    // view — no cooperative exists to check membership against.
    let rows = sqlx::query_as::<_, UserListRow>(&format!(
        "SELECT id, username, display_name, status FROM users u \
         WHERE {} \
         ORDER BY u.display_name, u.username",
        member_visibility("u.id", 1)
    ))
    .bind(scope)
    .fetch_all(pool)
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "user listing failed");
        ApiError::Internal
    })?;

    let mut users = Vec::with_capacity(rows.len());
    for row in rows {
        let roles = sqlx::query_as::<_, UserRoleDto>(&format!(
            "SELECT r.id, r.name, r.status \
             FROM user_role_assignments ura \
             JOIN roles r ON r.id = ura.role_id \
             WHERE ura.user_id = $1 \
               AND {} \
               AND {} \
             ORDER BY r.name",
            scoped_visibility("r.cooperative_id", 2),
            scoped_visibility("ura.cooperative_id", 2)
        ))
        .bind(row.id)
        .bind(scope)
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
    ctx: BusinessAuth,
    Path(user_id): Path<Uuid>,
) -> Result<Json<Vec<UserRoleDto>>, ApiError> {
    authz::require_business(&state, &ctx, authz::catalog::USERS_READ).await?;
    let pool = pool_of(&state)?;
    let scope = ctx.cooperative_id();
    let exists: Option<bool> = sqlx::query_scalar(&format!(
        "SELECT TRUE FROM users WHERE id = $1 AND {}",
        member_visibility("$1", 2)
    ))
    .bind(user_id)
    .bind(scope)
    .fetch_optional(pool)
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "user existence check failed");
        ApiError::Internal
    })?;
    if exists.is_none() {
        return Err(ApiError::NotFound);
    }
    let roles = sqlx::query_as::<_, UserRoleDto>(&format!(
        "SELECT r.id, r.name, r.status \
         FROM user_role_assignments ura \
         JOIN roles r ON r.id = ura.role_id \
         WHERE ura.user_id = $1 \
           AND {} \
           AND {} \
         ORDER BY r.name",
        scoped_visibility("r.cooperative_id", 2),
        scoped_visibility("ura.cooperative_id", 2)
    ))
    .bind(user_id)
    .bind(scope)
    .fetch_all(pool)
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "user roles read failed");
        ApiError::Internal
    })?;
    Ok(Json(roles))
}

/// Replacing a user's role set can remove the scope's last
/// administration path — cooperative-scoped guarded transaction
/// (P1-c2b). The target must hold a membership row in the verified
/// scope (404 otherwise — cross-cooperative users are invisible); new
/// assignments are written WITH `cooperative_id = scope`, so the API
/// never creates NULL-cooperative assignments.
pub async fn set_user_roles(
    State(state): State<AppState>,
    headers: HeaderMap,
    ctx: BusinessAuth,
    Path(user_id): Path<Uuid>,
    Json(body): Json<UserRoleSetRequest>,
) -> Result<Response, ApiError> {
    let pool = guarded_business_mutation!(
        &state,
        &headers,
        &ctx,
        authz::catalog::USERS_MANAGE,
        &axum::http::Method::PUT
    );
    let scope = ctx.cooperative_id();
    let requested: std::collections::BTreeSet<Uuid> = body.role_ids.into_iter().collect();

    let mut tx = authz::begin_guarded_transaction(pool, scope)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "guard tx failed");
            ApiError::Internal
        })?;

    // Target ownership: the user must hold a membership row in the
    // verified scope (any status — the composite FK anchors on row
    // existence; the ACTOR's active membership is already enforced by
    // BusinessAuth + the resolver join). Users who belong only to
    // another cooperative are invisible (404, never a mutation target).
    let target_exists: Option<bool> = sqlx::query_scalar(&format!(
        "SELECT TRUE FROM users WHERE id = $1 AND {}",
        member_visibility("$1", 2)
    ))
    .bind(user_id)
    .bind(scope)
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

    // All requested roles must exist and be assignable in the verified
    // scope. Assigning a disabled role is a validation error (policy:
    // disabled roles grant nothing — assignment would be misleading).
    // In a cooperative scope the role must be OWNED by that
    // cooperative: NULL-attributed legacy roles stay manageable but are
    // not assignable until the contract migration backfills them
    // (P1-c4) — the composite FK could only pair them with a NULL
    // assignment, and this API no longer writes NULL rows. Unknown,
    // foreign-owned and NULL-attributed roles all answer the same
    // validation code — the response never distinguishes which bucket
    // failed (no cross-cooperative existence leak).
    for role_id in &requested {
        let role_row: Option<(String, Option<Uuid>)> =
            sqlx::query_as("SELECT status, cooperative_id FROM roles WHERE id = $1")
                .bind(role_id)
                .fetch_optional(tx.as_mut())
                .await
                .map_err(|error| {
                    tracing::error!(error = %error, "role status read failed");
                    ApiError::Internal
                })?;
        let reject = match role_row {
            None => Some("unknown_role_rejected"),
            Some((ref status, _)) if status != "active" => {
                Some("disabled_role_assignment_rejected")
            }
            Some((_, role_coop)) => match (scope, role_coop) {
                (Some(scope_id), Some(owner)) if owner == scope_id => None,
                (None, _) => None, // never-bootstrapped lane: legacy rule
                _ => Some("out_of_scope_role_rejected"),
            },
        };
        if let Some(outcome) = reject {
            tx.rollback().await.map_err(|error| {
                tracing::error!(error = %error, "rollback failed");
                ApiError::Internal
            })?;
            tracing::info!(outcome, role_id = %role_id);
            return Err(ApiError::ValidationFailed);
        }
    }

    // Before-state restricted to the verified scope: assignments owned
    // by another cooperative are neither reported nor touched.
    let before: Vec<(Uuid, String)> = sqlx::query_as(&format!(
        "SELECT r.id, r.name FROM user_role_assignments ura \
         JOIN roles r ON r.id = ura.role_id \
         WHERE ura.user_id = $1 AND {} AND {} \
         ORDER BY r.name",
        scoped_visibility("ura.cooperative_id", 2),
        scoped_visibility("r.cooperative_id", 2)
    ))
    .bind(user_id)
    .bind(scope)
    .fetch_all(tx.as_mut())
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "before-state read failed");
        ApiError::Internal
    })?;

    sqlx::query(&format!(
        "DELETE FROM user_role_assignments WHERE user_id = $1 AND {}",
        scoped_visibility("cooperative_id", 2)
    ))
    .bind(user_id)
    .bind(scope)
    .execute(tx.as_mut())
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "assignment clear failed");
        ApiError::Internal
    })?;
    for role_id in &requested {
        sqlx::query(
            "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
             VALUES ($1, $2, $3)",
        )
        .bind(user_id)
        .bind(role_id)
        .bind(scope)
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

    // Lockout guard: removing the scope's last administrative path.
    if authz::administration_path_count(&mut tx, scope)
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
        audit_lockout_prevention(pool, &ctx.auth, "user_roles_change", user_id, scope).await;
        return Err(ApiError::LockoutPrevented);
    }

    tx.commit().await.map_err(|error| {
        tracing::error!(error = %error, "commit failed");
        ApiError::Internal
    })?;

    // Audited as explicit assigned/unassigned deltas (durable history),
    // attributed to the verified cooperative.
    let before_ids: std::collections::BTreeSet<Uuid> = before.iter().map(|(id, _)| *id).collect();
    for (role_id, role_name) in &before {
        if !requested.contains(role_id) {
            audit::record_scoped(
                pool,
                SecurityEventType::RoleUnassigned,
                scope,
                Some(ctx.auth.user_id),
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
            audit::record_scoped(
                pool,
                SecurityEventType::RoleAssigned,
                scope,
                Some(ctx.auth.user_id),
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
    tracing::info!(outcome = "user_roles_set", target_user = %user_id, count = requested.len(), actor = %ctx.auth.user_id);

    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn audit_lockout_prevention(
    pool: &sqlx::PgPool,
    auth: &CurrentAuth,
    operation: &str,
    target_id: Uuid,
    scope: Option<Uuid>,
) {
    tracing::warn!(outcome = "lockout_prevented", operation, target = %target_id, actor = %auth.user_id, cooperative_id = ?scope);
    audit::record_scoped(
        pool,
        SecurityEventType::LockoutPrevented,
        scope,
        Some(auth.user_id),
        None,
        serde_json::json!({ "operation": operation, "target_id": target_id }),
    )
    .await;
}
