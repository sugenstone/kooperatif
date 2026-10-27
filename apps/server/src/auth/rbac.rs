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
//! - `authz::begin_guarded_transaction(&mut conn, scope)` takes a
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

use axum::extract::{Path, Query, State};
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
    cooperative_id: Option<Uuid>,
    requires_platform_admin: bool,
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
    /// Owning cooperative for scoped roles; NULL for platform/legacy.
    /// Needed so assignability pickers can match the verified scope.
    pub cooperative_id: Option<Uuid>,
    /// Canonical system-administrator template link: assignment and
    /// revocation require platform-administrator authority (0081).
    pub requires_platform_admin: bool,
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
    membership_status: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserListItemDto {
    pub id: Uuid,
    pub username: String,
    pub display_name: String,
    /// Global identity status (platform attribute, read-only here).
    pub status: String,
    /// Membership status in the verified cooperative — the field
    /// cooperative administrators actually act on (M1).
    pub membership_status: Option<String>,
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

// ── M1-P6 B3: cooperative membership management DTOs ────────────────

/// Add an existing global user to the verified cooperative. The target
/// cooperative is NEVER a request field — it is the sealed verified
/// scope (a caller-controlled cooperative parameter would be a
/// cross-cooperative insertion vulnerability).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct CreateMembershipRequest {
    pub user_id: Uuid,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct CreateUserRequest {
    pub username: String,
    pub display_name: String,
    /// Operator-provisioned initial credential. Never echoed, never
    /// logged; hashed (Argon2id + DB bcrypt verifier) before storage.
    pub password: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct UpdateUserRequest {
    pub display_name: String,
}

/// Member-of-the-verified-cooperative read model.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MembershipDto {
    pub user_id: Uuid,
    pub username: String,
    pub display_name: String,
    pub status: String,
    pub roles: Vec<UserRoleDto>,
    #[serde(with = "time::serde::rfc3339")]
    pub joined_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub ended_at: Option<OffsetDateTime>,
}

#[derive(Debug, sqlx::FromRow)]
struct MembershipRow {
    user_id: Uuid,
    username: String,
    display_name: String,
    status: String,
    joined_at: OffsetDateTime,
    ended_at: Option<OffsetDateTime>,
}

// ── M1-P6 B2: platform cooperative lifecycle DTOs ───────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct CreateCooperativeRequest {
    pub name: String,
    pub legal_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct ActivateCooperativeRequest {
    pub initial_admin_user_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct PlatformCooperativeQuery {
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PlatformUserQuery {
    pub search: Option<String>,
    pub limit: Option<i32>,
}

/// Platform first-admin picker projection (0080
/// `app_user_list_platform`) — allowlisted identity fields only; no
/// credential, reset or session material ever crosses the boundary.
#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PlatformUserDto {
    pub id: Uuid,
    pub username: String,
    pub display_name: String,
    pub status: String,
}

/// Platform read-model projection (0068 `app_cooperative_list_platform`)
/// — allowlisted fields only; the definer hides the table entirely from
/// the restricted role.
#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PlatformCooperativeDto {
    pub id: Uuid,
    pub name: String,
    pub legal_name: Option<String>,
    pub status: String,
    pub is_bootstrap: bool,
    pub business_enabled: bool,
    /// ACTIVE memberships only.
    pub member_count: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
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
        .route("/api/users", get(list_users).post(create_user))
        .route("/api/users/{id}", axum::routing::patch(update_user))
        // M1-P6 B3: cooperative membership management. The cooperative
        // is the sealed verified scope — never a request parameter.
        .route(
            "/api/memberships",
            get(list_memberships).post(create_membership),
        )
        // M1: cooperative administrators act on the MEMBERSHIP inside
        // their verified cooperative — never on the global identity.
        .route(
            "/api/users/{id}/membership-suspend",
            post(suspend_membership),
        )
        .route(
            "/api/users/{id}/membership-reactivate",
            post(reactivate_membership),
        )
        .route("/api/users/{id}/membership-end", post(end_membership))
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

/// Platform router (M1): global identity lifecycle endpoints. Kept
/// OUTSIDE the business membership gate — platform authority is
/// membership-independent, and a caller whose only membership ended
/// must still be reachable here for platform duties.
///
/// M1-P6 B2 adds the cooperative lifecycle lane: every endpoint is
/// authorized INSIDE the database by a NULL-scope `cooperatives.manage`
/// grant (`app_has_platform_permission`) — a cooperative-scoped grant
/// can never satisfy it, so cooperative administrators cannot reach
/// platform lifecycle through this router.
pub fn platform_router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/platform/users/{id}/disable",
            post(platform_disable_user),
        )
        .route(
            "/api/platform/users/{id}/reset-password",
            post(platform_reset_password),
        )
        .route(
            "/api/platform/users/{id}/enable",
            post(platform_enable_user),
        )
        .route(
            "/api/platform/cooperatives",
            get(platform_list_cooperatives).post(platform_create_cooperative),
        )
        .route(
            "/api/platform/cooperatives/{id}",
            get(platform_get_cooperative),
        )
        .route(
            "/api/platform/cooperatives/{id}/activate",
            post(platform_activate_cooperative),
        )
        .route(
            "/api/platform/cooperatives/{id}/suspend",
            post(platform_suspend_cooperative),
        )
        .route(
            "/api/platform/cooperatives/{id}/reactivate",
            post(platform_reactivate_cooperative),
        )
        .route(
            "/api/platform/cooperatives/{id}/archive",
            post(platform_archive_cooperative),
        )
        .route("/api/platform/users", get(platform_list_users))
        // Platform authorization data is sensitive: never cacheable.
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
    // M1-P4-R4: marker/flag agreement and the never-bootstrapped
    // predicate moved behind verified definers — the runtime role no
    // longer selects from installation_state/cooperatives directly.
    format!(
        "({col} = ${i} \
         OR ({col} IS NULL AND public.app_is_bootstrap_scope(${i})) \
         OR (${i}::uuid IS NULL AND NOT public.app_cooperatives_exist()))",
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
        "((${i}::uuid IS NULL AND NOT public.app_cooperatives_exist()) \
         OR EXISTS(SELECT 1 FROM cooperative_memberships m \
                   WHERE m.user_id = {u} AND m.cooperative_id = ${i}))",
        u = user_expr,
        i = idx
    )
}

/// M1-P4-R1: boundary definers reject a lockout-inducing mutation
/// with check_violation 'lockout_prevented' — the caller maps it to
/// the public LockoutPrevented contract.
fn is_lockout_prevented(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db_err)
        if db_err.code().as_deref() == Some("23514")
           && db_err.message().contains("lockout_prevented"))
}

/// M1-P4-R1: boundary definers raise insufficient_privilege (42501)
/// when the sealed context or the required administrative permission
/// is missing/unverifiable inside the database.
fn is_insufficient_privilege(db_err: &dyn sqlx::error::DatabaseError) -> bool {
    db_err.code().as_deref() == Some("42501")
}

/// All mutating RBAC handlers share this prologue: CSRF Origin check
/// (layer 1) + synchronizer token check (layer 2) + the required
/// permission resolved through `require_business` (session-scoped
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

/// M1-P6 B3/B4 — compatibility lane: the mutation proceeds when the
/// verified session holds ANY listed permission in its session scope
/// (mirrors the definer-side app_rbac_verified_scope_in semantics).
macro_rules! guarded_business_mutation_any {
    ($state:expr, $headers:expr, $ctx:expr, $permissions:expr, $method:expr) => {{
        csrf::validate_origin($headers, $method, &$state.auth.config.allowed_origins)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        csrf::validate_csrf_token($headers, &$ctx.auth.csrf_token)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        authz::require_business_any($state, $ctx, $permissions).await?;
        pool_of($state)?
    }};
}

pub async fn list_permissions(
    State(state): State<AppState>,
    ctx: BusinessAuth,
) -> Result<Json<Vec<PermissionDto>>, ApiError> {
    authz::require_business(&state, &ctx, authz::catalog::ROLES_READ).await?;
    let pool = pool_of(&state)?;
    let mut tx = ctx.open_tx(pool).await?;
    let __result = async {
        let rows = sqlx::query_as::<_, PermissionDto>(
            // R4: the catalog is read through the verified definer —
            // the permissions table is no longer SELECTable by the
            // runtime role.
            "SELECT key, name, description, category FROM public.app_permission_catalog()",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "permission catalog read failed");
            ApiError::Internal
        })?;
        Ok(Json(rows))
    }
    .await;
    match __result {
        Ok(v) => {
            tx.commit().await.map_err(|e| {
                tracing::error!(error = %e, "tenant tx commit failed");
                ApiError::Internal
            })?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
}

/// Role load through the verified scope (`scoped_visibility`): in the
/// bootstrap scope a role is visible iff it is bootstrap-owned OR
/// NULL-attributed (legacy lane); in the never-bootstrapped lane scope
/// is NULL and every (necessarily NULL) row matches — preserving the
/// legacy global view.
async fn load_role_with_permissions(
    db: &mut sqlx::PgConnection,
    role_id: Uuid,
    scope: Option<Uuid>,
) -> Result<Option<RoleDto>, sqlx::Error> {
    let Some(role) = sqlx::query_as::<_, RoleRow>(&format!(
        "SELECT r.id, r.name, r.description, r.status, r.cooperative_id, \
                r.created_at, r.updated_at, \
                EXISTS ( \
                    SELECT 1 FROM system_role_templates t \
                     WHERE lower(t.name) = lower(r.name) \
                       AND t.key = 'system-administrator' AND t.status = 'active' \
                ) AS requires_platform_admin \
         FROM roles r \
         WHERE r.id = $1 AND {}",
        scoped_visibility("cooperative_id", 2)
    ))
    .bind(role_id)
    .bind(scope)
    .fetch_optional(&mut *db)
    .await?
    else {
        return Ok(None);
    };
    let permissions: Vec<String> =
        sqlx::query_scalar("SELECT key FROM public.app_role_permission_keys($1)")
            .bind(role_id)
            .fetch_all(&mut *db)
            .await?;
    Ok(Some(RoleDto {
        id: role.id,
        name: role.name,
        description: role.description,
        status: role.status,
        cooperative_id: role.cooperative_id,
        requires_platform_admin: role.requires_platform_admin,
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
    let mut tx = ctx.open_tx(pool).await?;
    let __result = async {
        let scope = ctx.cooperative_id();
        let ids: Vec<Uuid> = sqlx::query_scalar(&format!(
            "SELECT id FROM roles WHERE {} ORDER BY name",
            scoped_visibility("cooperative_id", 1)
        ))
        .bind(scope)
        .fetch_all(&mut *tx)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "role listing failed");
            ApiError::Internal
        })?;
        let mut roles = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(role) = load_role_with_permissions(&mut tx, id, scope)
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
    .await;
    match __result {
        Ok(v) => {
            tx.commit().await.map_err(|e| {
                tracing::error!(error = %e, "tenant tx commit failed");
                ApiError::Internal
            })?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
}

pub async fn get_role(
    State(state): State<AppState>,
    ctx: BusinessAuth,
    Path(role_id): Path<Uuid>,
) -> Result<Json<RoleDto>, ApiError> {
    authz::require_business(&state, &ctx, authz::catalog::ROLES_READ).await?;
    let pool = pool_of(&state)?;
    let mut tx = ctx.open_tx(pool).await?;
    let __result = async {
        let dto = load_role_with_permissions(&mut tx, role_id, ctx.cooperative_id())
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "role load failed");
                ApiError::Internal
            })?
            .ok_or(ApiError::NotFound)?;
        Ok(Json(dto))
    }
    .await;
    match __result {
        Ok(v) => {
            tx.commit().await.map_err(|e| {
                tracing::error!(error = %e, "tenant tx commit failed");
                ApiError::Internal
            })?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
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
    let mut tx = ctx.open_tx(pool).await?;
    let __result = async {
        let name = body.name.trim().to_string();
        validate_role_name(&name)?;
        let description = body.description.unwrap_or_default().trim().to_string();
        let scope = ctx.cooperative_id();

        // P1-c2a: new roles carry the verified cooperative — no new NULL
        // role rows from this path (NULL only in the never-bootstrapped
        // lane, where no cooperative exists yet).
        // M1-P4-R1: the write goes through the boundary definer, which
        // re-verifies the sealed context and the roles.manage permission
        // inside the database — the sealed context alone no longer
        // authorizes RBAC mutations.
        let role = sqlx::query_scalar::<_, Uuid>("SELECT public.app_role_create($1, $2)")
            .bind(&name)
            .bind(&description)
            .fetch_one(&mut *tx)
            .await
            .map_err(|error| match error {
                sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                    tracing::info!(outcome = "role_duplicate_name", name = %name);
                    ApiError::Conflict
                }
                sqlx::Error::Database(db_err) if is_insufficient_privilege(&*db_err) => {
                    ApiError::PermissionDenied
                }
                other => {
                    tracing::error!(error = %other, "role creation failed");
                    ApiError::Internal
                }
            })?;

        // R4: role_created is minted by the roles transition trigger
        // inside this transaction — the route no longer writes it.
        tracing::info!(outcome = "role_created", role_id = %role, actor = %ctx.auth.user_id);

        let dto = load_role_with_permissions(&mut tx, role, scope)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "role reload failed");
                ApiError::Internal
            })?
            .ok_or(ApiError::Internal)?;
        Ok((StatusCode::CREATED, Json(dto)).into_response())
    }
    .await;
    match __result {
        Ok(v) => {
            tx.commit().await.map_err(|e| {
                tracing::error!(error = %e, "tenant tx commit failed");
                ApiError::Internal
            })?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
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
    let mut tx = ctx.open_tx(pool).await?;
    let __result = async {
        let scope = ctx.cooperative_id();
        if let Some(name) = &body.name {
            validate_role_name(name)?;
        }
        let _existing = load_role_with_permissions(&mut tx, role_id, scope)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "role load failed");
                ApiError::Internal
            })?
            .ok_or(ApiError::NotFound)?;

        let name = body.name.map(|n| n.trim().to_string());
        let description = body.description.map(|d| d.trim().to_string());
        // M1-P4-R1: mutation goes through the boundary definer — it
        // re-verifies roles.manage and the role's scope inside the
        // database; a NULL return means outside the verified scope.
        let updated: Option<Uuid> =
            sqlx::query_scalar::<_, Option<Uuid>>("SELECT public.app_role_update($1, $2, $3)")
                .bind(role_id)
                .bind(&name)
                .bind(&description)
                .fetch_one(&mut *tx)
                .await
                .map_err(|error| match error {
                    sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                        ApiError::Conflict
                    }
                    sqlx::Error::Database(db_err) if is_insufficient_privilege(&*db_err) => {
                        ApiError::PermissionDenied
                    }
                    other => {
                        tracing::error!(error = %other, "role update failed");
                        ApiError::Internal
                    }
                })?;
        if updated.is_none() {
            return Err(ApiError::NotFound);
        }

        // R4: role_updated is minted by the roles transition trigger.
        tracing::info!(outcome = "role_updated", role_id = %role_id, actor = %ctx.auth.user_id);

        let dto = load_role_with_permissions(&mut tx, role_id, scope)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "role reload failed");
                ApiError::Internal
            })?
            .ok_or(ApiError::Internal)?;
        Ok(Json(dto))
    }
    .await;
    match __result {
        Ok(v) => {
            tx.commit().await.map_err(|e| {
                tracing::error!(error = %e, "tenant tx commit failed");
                ApiError::Internal
            })?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
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
    let mut tx = ctx.open_tx(pool).await?;
    let __result = async {
        let scope = ctx.cooperative_id();
        set_role_status(pool, &mut tx, &ctx.auth, role_id, "disabled", scope).await
    }
    .await;
    match __result {
        Ok(v) => {
            tx.commit().await.map_err(|e| {
                tracing::error!(error = %e, "tenant tx commit failed");
                ApiError::Internal
            })?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
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
    let mut tx = ctx.open_tx(pool).await?;
    let __result = async {
        let scope = ctx.cooperative_id();
        set_role_status(pool, &mut tx, &ctx.auth, role_id, "active", scope).await
    }
    .await;
    match __result {
        Ok(v) => {
            tx.commit().await.map_err(|e| {
                tracing::error!(error = %e, "tenant tx commit failed");
                ApiError::Internal
            })?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
}

/// `scope` limits which role rows the mutation may touch (the verified
/// visibility predicate), selects the cooperative-scoped advisory lock
/// and administration-path count, and attributes the audit event. It is
/// `None` only in the never-bootstrapped lane, where the legacy global
/// lock/count/view apply unchanged.
async fn set_role_status(
    pool: &sqlx::PgPool,
    conn: &mut sqlx::PgConnection,
    auth: &CurrentAuth,
    role_id: Uuid,
    status: &str,
    scope: Option<Uuid>,
) -> Result<Response, ApiError> {
    // Scoped guarded transaction: disabling a role can remove the
    // cooperative's last administration path (§28/§44, P1-K5).
    let mut tx = authz::begin_guarded_transaction(&mut *conn, scope)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "guard tx failed");
            ApiError::Internal
        })?;

    // M1-P4-R1: the status change goes through the boundary definer,
    // which re-verifies roles.manage, the role's scope and the
    // last-administrator protection inside the database — a lockout
    // surfaces as check_violation 'lockout_prevented'.
    let updated: Option<Uuid> =
        match sqlx::query_scalar::<_, Option<Uuid>>("SELECT public.app_role_set_status($1, $2)")
            .bind(role_id)
            .bind(status)
            .fetch_one(tx.as_mut())
            .await
        {
            Ok(value) => value,
            Err(error) if is_lockout_prevented(&error) => {
                tx.rollback().await.map_err(|error| {
                    tracing::error!(error = %error, "rollback failed");
                    ApiError::Internal
                })?;
                audit_lockout_prevention(pool, auth, "role_disable", role_id, scope).await;
                return Err(ApiError::LockoutPrevented);
            }
            Err(sqlx::Error::Database(db_err)) if is_insufficient_privilege(&*db_err) => {
                return Err(ApiError::PermissionDenied);
            }
            Err(error) => {
                tracing::error!(error = %error, "role status change failed");
                return Err(ApiError::Internal);
            }
        };
    if updated.is_none() {
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
        .fetch_optional(&mut *conn)
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

    // R4: role_enabled/role_disabled is minted by the roles
    // transition trigger inside this transaction — it cannot survive
    // a failed commit and cannot be skipped by a direct definer call.
    tx.commit().await.map_err(|error| {
        tracing::error!(error = %error, "commit failed");
        ApiError::Internal
    })?;

    tracing::info!(outcome = "role_status_changed", role_id = %role_id, status, actor = %auth.user_id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Suspending a membership can remove the scope's last administration
/// path — runs inside the cooperative-scoped guarded transaction
/// (P1-c2b). The target must hold a membership row in the verified
/// scope — a user who only belongs to another cooperative answers 404.
/// M1: this never touches the global `users.status`.
pub async fn suspend_membership(
    State(state): State<AppState>,
    headers: HeaderMap,
    ctx: BusinessAuth,
    Path(user_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let pool = guarded_business_mutation_any!(
        &state,
        &headers,
        &ctx,
        &[
            authz::catalog::MEMBERSHIPS_MANAGE,
            authz::catalog::USERS_MANAGE
        ],
        &axum::http::Method::POST
    );
    let mut tx = ctx.open_tx(pool).await?;
    let __result = async {
        let scope = ctx.cooperative_id();
        set_membership_status(pool, &mut tx, &ctx.auth, user_id, "suspended", scope).await
    }
    .await;
    match __result {
        Ok(v) => {
            tx.commit().await.map_err(|e| {
                tracing::error!(error = %e, "tenant tx commit failed");
                ApiError::Internal
            })?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
}

pub async fn reactivate_membership(
    State(state): State<AppState>,
    headers: HeaderMap,
    ctx: BusinessAuth,
    Path(user_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let pool = guarded_business_mutation_any!(
        &state,
        &headers,
        &ctx,
        &[
            authz::catalog::MEMBERSHIPS_MANAGE,
            authz::catalog::USERS_MANAGE
        ],
        &axum::http::Method::POST
    );
    let mut tx = ctx.open_tx(pool).await?;
    let __result = async {
        let scope = ctx.cooperative_id();
        set_membership_status(pool, &mut tx, &ctx.auth, user_id, "active", scope).await
    }
    .await;
    match __result {
        Ok(v) => {
            tx.commit().await.map_err(|e| {
                tracing::error!(error = %e, "tenant tx commit failed");
                ApiError::Internal
            })?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
}

pub async fn end_membership(
    State(state): State<AppState>,
    headers: HeaderMap,
    ctx: BusinessAuth,
    Path(user_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let pool = guarded_business_mutation_any!(
        &state,
        &headers,
        &ctx,
        &[
            authz::catalog::MEMBERSHIPS_MANAGE,
            authz::catalog::USERS_MANAGE
        ],
        &axum::http::Method::POST
    );
    let mut tx = ctx.open_tx(pool).await?;
    let __result = async {
        let scope = ctx.cooperative_id();
        set_membership_status(pool, &mut tx, &ctx.auth, user_id, "ended", scope).await
    }
    .await;
    match __result {
        Ok(v) => {
            tx.commit().await.map_err(|e| {
                tracing::error!(error = %e, "tenant tx commit failed");
                ApiError::Internal
            })?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
}

/// Membership lifecycle mutation inside the verified cooperative.
/// `scope` pins the target row to the verified cooperative, selects the
/// cooperative-scoped advisory lock and administration-path count, and
/// attributes the audit event. `None` only in the never-bootstrapped
/// lane (no cooperatives → no memberships → nothing to mutate).
async fn set_membership_status(
    pool: &sqlx::PgPool,
    conn: &mut sqlx::PgConnection,
    auth: &CurrentAuth,
    user_id: Uuid,
    status: &str,
    scope: Option<Uuid>,
) -> Result<Response, ApiError> {
    // Scoped guarded transaction: suspending/ending a membership can
    // remove the cooperative's last administration path (§28/§44,
    // P1-K5) — the count already excludes non-active memberships, so
    // the check runs inside the same tx that performs the change.
    let mut tx = authz::begin_guarded_transaction(&mut *conn, scope)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "guard tx failed");
            ApiError::Internal
        })?;

    // M1: the membership write goes through the boundary definer — it
    // re-verifies users.manage, pins the row to the verified scope and
    // re-enforces the last-administrator path inside the database. A
    // NULL return means no membership row in the verified scope or
    // already in the requested state (idempotent); 'lockout_prevented'
    // surfaces as check_violation. The cooperative_memberships
    // transition trigger mints the audit event inside this tx.
    let updated: Option<Uuid> = match sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT public.app_membership_set_status($1, $2)",
    )
    .bind(user_id)
    .bind(status)
    .fetch_one(tx.as_mut())
    .await
    {
        Ok(value) => value,
        Err(error) if is_lockout_prevented(&error) => {
            tx.rollback().await.map_err(|error| {
                tracing::error!(error = %error, "rollback failed");
                ApiError::Internal
            })?;
            audit_lockout_prevention(pool, auth, "membership_change", user_id, scope).await;
            return Err(ApiError::LockoutPrevented);
        }
        Err(sqlx::Error::Database(db_err)) if is_insufficient_privilege(&*db_err) => {
            return Err(ApiError::PermissionDenied);
        }
        Err(error) => {
            tracing::error!(error = %error, "membership status change failed");
            return Err(ApiError::Internal);
        }
    };
    if updated.is_none() {
        // No membership row in the verified scope, or already in the
        // target state (idempotent).
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
        .fetch_optional(&mut *conn)
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

    tx.commit().await.map_err(|error| {
        tracing::error!(error = %error, "commit failed");
        ApiError::Internal
    })?;

    tracing::info!(outcome = "membership_status_changed", user_id = %user_id, status, actor = %auth.user_id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// M1-P6 B3 — list the memberships and cooperative-scoped roles inside
/// the verified cooperative. Visibility is the same member-only lane
/// as `/api/users`; cross-cooperative rows are structurally absent
/// because the query is pinned to the sealed scope.
pub async fn list_memberships(
    State(state): State<AppState>,
    ctx: BusinessAuth,
) -> Result<Json<Vec<MembershipDto>>, ApiError> {
    authz::require_business(&state, &ctx, authz::catalog::MEMBERSHIPS_READ).await?;
    let pool = pool_of(&state)?;
    let mut tx = ctx.open_tx(pool).await?;
    let __result = async {
        let scope = ctx.cooperative_id();
        let rows = sqlx::query_as::<_, MembershipRow>(
            "SELECT m.user_id, u.username, u.display_name, m.status, \
                    m.joined_at, m.ended_at \
             FROM cooperative_memberships m \
             JOIN users u ON u.id = m.user_id \
             WHERE m.cooperative_id = $1 \
             ORDER BY u.display_name, u.username",
        )
        .bind(scope)
        .fetch_all(&mut *tx)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "membership listing failed");
            ApiError::Internal
        })?;

        let mut memberships = Vec::with_capacity(rows.len());
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
            .bind(row.user_id)
            .bind(scope)
            .fetch_all(&mut *tx)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "member roles read failed");
                ApiError::Internal
            })?;
            memberships.push(MembershipDto {
                user_id: row.user_id,
                username: row.username,
                display_name: row.display_name,
                status: row.status,
                roles,
                joined_at: row.joined_at,
                ended_at: row.ended_at,
            });
        }
        Ok(Json(memberships))
    }
    .await;
    match __result {
        Ok(v) => {
            tx.commit().await.map_err(|e| {
                tracing::error!(error = %e, "tenant tx commit failed");
                ApiError::Internal
            })?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
}

/// M1-P6 B3 — add an existing global user to the verified cooperative.
/// Requires `memberships.manage` in the sealed scope; the definer
/// re-verifies it and derives the target cooperative from the sealed
/// context — the cooperative is never caller-controlled.
pub async fn create_membership(
    State(state): State<AppState>,
    headers: HeaderMap,
    ctx: BusinessAuth,
    Json(body): Json<CreateMembershipRequest>,
) -> Result<Response, ApiError> {
    let pool = guarded_business_mutation_any!(
        &state,
        &headers,
        &ctx,
        &[authz::catalog::MEMBERSHIPS_MANAGE],
        &axum::http::Method::POST
    );
    let mut tx = ctx.open_tx(pool).await?;
    let __result = async {
        let scope = ctx.cooperative_id();
        // Scoped guarded transaction, same convention as the lifecycle
        // lane: membership creation mints cooperative_membership_granted
        // through the audited transition trigger inside this tx.
        let mut guard_tx = authz::begin_guarded_transaction(&mut tx, scope)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "guard tx failed");
                ApiError::Internal
            })?;

        let created: Option<Uuid> =
            match sqlx::query_scalar::<_, Option<Uuid>>("SELECT public.app_membership_create($1)")
                .bind(body.user_id)
                .fetch_one(guard_tx.as_mut())
                .await
            {
                Ok(value) => value,
                Err(sqlx::Error::Database(db_err)) if is_insufficient_privilege(&*db_err) => {
                    return Err(ApiError::PermissionDenied);
                }
                Err(error) if is_lifecycle_violation(&error, "cooperative_not_ready") => {
                    return Err(ApiError::CooperativeNotReady);
                }
                Err(error)
                    if is_lifecycle_violation(&error, "membership_exists")
                        || is_lifecycle_violation(&error, "membership_user_ineligible") =>
                {
                    return Err(ApiError::Conflict);
                }
                Err(error) => {
                    tracing::error!(error = %error, "membership creation failed");
                    return Err(ApiError::Internal);
                }
            };

        if created.is_none() {
            guard_tx.rollback().await.map_err(|error| {
                tracing::error!(error = %error, "rollback failed");
                ApiError::Internal
            })?;
            return Err(ApiError::NotFound);
        }

        let row = sqlx::query_as::<_, MembershipRow>(
            "SELECT m.user_id, u.username, u.display_name, m.status, \
                    m.joined_at, m.ended_at \
             FROM cooperative_memberships m \
             JOIN users u ON u.id = m.user_id \
             WHERE m.user_id = $1 AND m.cooperative_id = $2",
        )
        .bind(body.user_id)
        .bind(scope)
        .fetch_one(guard_tx.as_mut())
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "membership read-back failed");
            ApiError::Internal
        })?;

        guard_tx.commit().await.map_err(|error| {
            tracing::error!(error = %error, "commit failed");
            ApiError::Internal
        })?;

        tracing::info!(
            outcome = "membership_created",
            user_id = %body.user_id,
            actor = %ctx.auth.user_id
        );
        Ok((
            StatusCode::CREATED,
            Json(MembershipDto {
                user_id: row.user_id,
                username: row.username,
                display_name: row.display_name,
                status: row.status,
                roles: Vec::new(),
                joined_at: row.joined_at,
                ended_at: row.ended_at,
            }),
        )
            .into_response())
    }
    .await;
    match __result {
        Ok(v) => {
            tx.commit().await.map_err(|e| {
                tracing::error!(error = %e, "tenant tx commit failed");
                ApiError::Internal
            })?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
}

/// Platform lane (M1): global identity lifecycle. Runs OUTSIDE the
/// business gate — a platform administrator's authority does not depend
/// on any cooperative membership. `request_user_tx` stamps only the
/// user seal; `app_platform_user_set_status` re-verifies the NULL-scope
/// `users.manage` grant and the last-platform-administrator path inside
/// the database. Cooperative-scoped callers can never reach this: the
/// definer rejects a cooperative seal.
pub async fn platform_disable_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(user_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    platform_set_user_status(&state, &headers, &auth, user_id, "disabled").await
}

pub async fn platform_enable_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(user_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    platform_set_user_status(&state, &headers, &auth, user_id, "active").await
}

async fn platform_set_user_status(
    state: &AppState,
    headers: &HeaderMap,
    auth: &CurrentAuth,
    user_id: Uuid,
    status: &str,
) -> Result<Response, ApiError> {
    crate::auth::routes::require_allowed_origin(headers, &axum::http::Method::POST, state)?;
    csrf::validate_csrf_token(headers, &auth.csrf_token)
        .map_err(|rejection| crate::auth::routes::csrf_rejection_to_api_error(&rejection))?;

    let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;
    let (mut tx, _user) =
        crate::db::request_user_tx(pool, &auth.session_token_hash, auth.authed_at)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "platform tx context failed");
                match error {
                    crate::db::RequestContextError::Unauthenticated => {
                        ApiError::AuthenticationRequired
                    }
                    _ => ApiError::Internal,
                }
            })?;

    let updated: Option<Uuid> = match sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT public.app_platform_user_set_status($1, $2)",
    )
    .bind(user_id)
    .bind(status)
    .fetch_one(tx.as_mut())
    .await
    {
        Ok(value) => value,
        Err(error) if is_lockout_prevented(&error) => {
            return Err(ApiError::LockoutPrevented);
        }
        Err(sqlx::Error::Database(db_err)) if is_insufficient_privilege(&*db_err) => {
            return Err(ApiError::PermissionDenied);
        }
        Err(error) => {
            tracing::error!(error = %error, "platform user status change failed");
            return Err(ApiError::Internal);
        }
    };

    if updated.is_none() {
        // Unknown user or already in the target state — idempotent, and
        // deliberately indistinguishable (the platform lane is not an
        // existence oracle for cooperative viewers).
        tx.commit().await.map_err(|error| {
            tracing::error!(error = %error, "commit failed");
            ApiError::Internal
        })?;
        return Ok(StatusCode::NO_CONTENT.into_response());
    }

    if status == "disabled" {
        // Immediate explicit revocation on top of the validation-time
        // rejection: active sessions (HTTP + WS) die at once rather
        // than on next classify. The post-bootstrap platform arm of the
        // definer re-verifies NULL-scope users.manage (M1-P4-P3B).
        sqlx::query("SELECT public.app_admin_revoke_user_sessions($1, $2)")
            .bind(user_id)
            .bind(auth.authed_at)
            .execute(tx.as_mut())
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "session revocation failed");
                ApiError::Internal
            })?;
    }

    // R4: user_enabled/user_disabled is minted by the users transition
    // trigger (NULL cooperative attribution — platform event);
    // admin_sessions_revoked is minted inside
    // app_admin_revoke_user_sessions itself.
    tx.commit().await.map_err(|error| {
        tracing::error!(error = %error, "commit failed");
        ApiError::Internal
    })?;

    tracing::info!(outcome = "platform_user_status_changed", user_id = %user_id, status, actor = %auth.user_id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ── M1-P6 B2: platform cooperative lifecycle ────────────────────────
//
// Every mutation runs inside `request_user_tx` (sealed user context,
// NULL cooperative) and a SECURITY DEFINER that re-verifies the seal
// AND the NULL-scope `cooperatives.manage` grant. Cooperative-scoped
// seals fail `app_user_ctx_ok` (NULL-scope HMAC) and scoped grants
// fail the predicate — the platform lane cannot be reached through
// cooperative authorization. Audit events mint inside the sealed
// transaction with NULL cooperative attribution (platform event) and
// the target cooperative in metadata; the rt_authz_cooperatives
// trigger emits the signed invalidation signal on status changes.

/// check_violation (23514) raised by the 0068 lifecycle definers,
/// discriminated by the token embedded in the message — same mapping
/// discipline as `is_lockout_prevented`.
fn is_lifecycle_violation(error: &sqlx::Error, token: &str) -> bool {
    matches!(error, sqlx::Error::Database(db_err)
        if db_err.code().as_deref() == Some("23514")
           && db_err.message().contains(token))
}

/// Map a lifecycle-definer error onto the public contract. Returns
/// `None` for unrecognized errors (caller maps to Internal).
fn lifecycle_error(error: &sqlx::Error) -> Option<ApiError> {
    match error {
        sqlx::Error::Database(db_err) if is_insufficient_privilege(&**db_err) => {
            Some(ApiError::PermissionDenied)
        }
        _ if is_lifecycle_violation(error, "cooperative_name_invalid") => {
            Some(ApiError::ValidationFailed)
        }
        _ if is_lifecycle_violation(error, "invalid_cooperative_transition")
            || is_lifecycle_violation(error, "initial_admin_ineligible")
            || is_lifecycle_violation(error, "initial_admin_role_conflict")
            || is_lifecycle_violation(error, "reactivation_admin_invariant")
            || is_lifecycle_violation(error, "activation_admin_invariant") =>
        {
            Some(ApiError::Conflict)
        }
        sqlx::Error::Database(db_err) if db_err.is_unique_violation() => Some(ApiError::Conflict),
        _ => None,
    }
}

/// Shared prologue for platform mutations: origin + CSRF + sealed user
/// transaction (NULL cooperative lane).
async fn platform_mutation_tx(
    state: &AppState,
    headers: &HeaderMap,
    auth: &CurrentAuth,
) -> Result<(sqlx::Transaction<'static, sqlx::Postgres>, Uuid), ApiError> {
    crate::auth::routes::require_allowed_origin(headers, &axum::http::Method::POST, state)?;
    csrf::validate_csrf_token(headers, &auth.csrf_token)
        .map_err(|rejection| crate::auth::routes::csrf_rejection_to_api_error(&rejection))?;
    let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;
    crate::db::request_user_tx(pool, &auth.session_token_hash, auth.authed_at)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "platform tx context failed");
            match error {
                crate::db::RequestContextError::Unauthenticated => ApiError::AuthenticationRequired,
                _ => ApiError::Internal,
            }
        })
}

pub async fn platform_create_cooperative(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Json(body): Json<CreateCooperativeRequest>,
) -> Result<Response, ApiError> {
    let (mut tx, _actor) = platform_mutation_tx(&state, &headers, &auth).await?;

    let id: Uuid =
        match sqlx::query_scalar::<_, Uuid>("SELECT public.app_cooperative_create($1, $2)")
            .bind(body.name.trim())
            .bind(body.legal_name.as_deref().map(str::trim))
            .fetch_one(tx.as_mut())
            .await
        {
            Ok(id) => id,
            Err(error) => match lifecycle_error(&error) {
                Some(mapped) => return Err(mapped),
                None => {
                    tracing::error!(error = %error, "cooperative create failed");
                    return Err(ApiError::Internal);
                }
            },
        };

    tx.commit().await.map_err(|error| {
        tracing::error!(error = %error, "commit failed");
        ApiError::Internal
    })?;
    tracing::info!(outcome = "cooperative_created", cooperative_id = %id, actor = %auth.user_id);
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "id": id, "status": "provisioning" })),
    )
        .into_response())
}

pub async fn platform_activate_cooperative(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(cooperative_id): Path<Uuid>,
    Json(body): Json<ActivateCooperativeRequest>,
) -> Result<Response, ApiError> {
    let (mut tx, _actor) = platform_mutation_tx(&state, &headers, &auth).await?;

    // Atomic activation package: membership + admin role assignment +
    // transition succeed or fail together inside the definer.
    let activated: Option<Uuid> = match sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT public.app_cooperative_activate($1, $2)",
    )
    .bind(cooperative_id)
    .bind(body.initial_admin_user_id)
    .fetch_one(tx.as_mut())
    .await
    {
        Ok(value) => value,
        Err(error) => match lifecycle_error(&error) {
            Some(mapped) => return Err(mapped),
            None => {
                tracing::error!(error = %error, "cooperative activation failed");
                return Err(ApiError::Internal);
            }
        },
    };

    tx.commit().await.map_err(|error| {
        tracing::error!(error = %error, "commit failed");
        ApiError::Internal
    })?;
    if activated.is_none() {
        return Err(ApiError::NotFound);
    }
    tracing::info!(outcome = "cooperative_activated", cooperative_id = %cooperative_id, actor = %auth.user_id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn platform_set_cooperative_status(
    state: &AppState,
    headers: &HeaderMap,
    auth: &CurrentAuth,
    cooperative_id: Uuid,
    status: &str,
) -> Result<Response, ApiError> {
    let (mut tx, _actor) = platform_mutation_tx(state, headers, auth).await?;

    let updated: Option<Uuid> = match sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT public.app_cooperative_set_status($1, $2)",
    )
    .bind(cooperative_id)
    .bind(status)
    .fetch_one(tx.as_mut())
    .await
    {
        Ok(value) => value,
        Err(error) => match lifecycle_error(&error) {
            Some(mapped) => return Err(mapped),
            None => {
                tracing::error!(error = %error, "cooperative status change failed");
                return Err(ApiError::Internal);
            }
        },
    };

    tx.commit().await.map_err(|error| {
        tracing::error!(error = %error, "commit failed");
        ApiError::Internal
    })?;
    if updated.is_none() {
        return Err(ApiError::NotFound);
    }
    tracing::info!(outcome = "cooperative_status_changed", cooperative_id = %cooperative_id, status, actor = %auth.user_id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn platform_suspend_cooperative(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(cooperative_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    platform_set_cooperative_status(&state, &headers, &auth, cooperative_id, "suspended").await
}

pub async fn platform_reactivate_cooperative(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(cooperative_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    platform_set_cooperative_status(&state, &headers, &auth, cooperative_id, "active").await
}

pub async fn platform_archive_cooperative(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(cooperative_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    platform_set_cooperative_status(&state, &headers, &auth, cooperative_id, "archived").await
}

/// Platform read lane (B4 convention): session-derived principal, the
/// definer re-verifies the NULL-scope grant. No cooperative extraction,
/// no CSRF — GET is a safe method.
async fn platform_read_cooperatives(
    state: &AppState,
    auth: &CurrentAuth,
    cooperative_id: Option<Uuid>,
    status: Option<&str>,
) -> Result<Vec<PlatformCooperativeDto>, ApiError> {
    let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;
    sqlx::query_as::<_, PlatformCooperativeDto>(
        "SELECT id, name, legal_name, status, is_bootstrap, \
                business_enabled, member_count, created_at \
           FROM public.app_cooperative_list_platform($1, $2, $3, $4, $5)",
    )
    .bind(&auth.session_token_hash[..])
    .bind(auth.authed_at)
    .bind(cooperative_id)
    .bind(status)
    .bind(500_i32)
    .fetch_all(pool)
    .await
    .map_err(|error| match error {
        sqlx::Error::Database(db_err) if is_insufficient_privilege(&*db_err) => {
            ApiError::PermissionDenied
        }
        _ => {
            tracing::error!(error = %error, "platform cooperative read failed");
            ApiError::Internal
        }
    })
}

pub async fn platform_list_cooperatives(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<PlatformCooperativeQuery>,
) -> Result<Json<Vec<PlatformCooperativeDto>>, ApiError> {
    let rows = platform_read_cooperatives(&state, &auth, None, query.status.as_deref()).await?;
    Ok(Json(rows))
}

pub async fn platform_get_cooperative(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(cooperative_id): Path<Uuid>,
) -> Result<Json<PlatformCooperativeDto>, ApiError> {
    let rows = platform_read_cooperatives(&state, &auth, Some(cooperative_id), None).await?;
    rows.into_iter().next().map(Json).ok_or(ApiError::NotFound)
}

/// 0080: platform-lane user enumeration for the first-administrator
/// picker. The definer verifies the session and the NULL-scope
/// `users.read` grant inside the database — FORCE RLS stays intact and
/// cooperative-scoped `/api/users` is untouched.
pub async fn platform_list_users(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<PlatformUserQuery>,
) -> Result<Json<Vec<PlatformUserDto>>, ApiError> {
    let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;
    let rows = sqlx::query_as::<_, PlatformUserDto>(
        "SELECT id, username, display_name, status \
           FROM public.app_user_list_platform($1, $2, $3, $4)",
    )
    .bind(&auth.session_token_hash[..])
    .bind(auth.authed_at)
    .bind(query.search.as_deref())
    .bind(query.limit)
    .fetch_all(pool)
    .await
    .map_err(|error| match error {
        sqlx::Error::Database(db_err) if is_insufficient_privilege(&*db_err) => {
            ApiError::PermissionDenied
        }
        _ => {
            tracing::error!(error = %error, "platform user list failed");
            ApiError::Internal
        }
    })?;
    Ok(Json(rows))
}

pub async fn get_role_permissions(
    State(state): State<AppState>,
    ctx: BusinessAuth,
    Path(role_id): Path<Uuid>,
) -> Result<Json<Vec<String>>, ApiError> {
    authz::require_business(&state, &ctx, authz::catalog::ROLES_READ).await?;
    let pool = pool_of(&state)?;
    let mut tx = ctx.open_tx(pool).await?;
    let __result = async {
        let scope = ctx.cooperative_id();
        // Scoped existence first — a foreign role must answer 404 without
        // touching its permission rows.
        let exists: Option<bool> = sqlx::query_scalar(&format!(
            "SELECT TRUE FROM roles WHERE id = $1 AND {}",
            scoped_visibility("cooperative_id", 2)
        ))
        .bind(role_id)
        .bind(scope)
        .fetch_optional(&mut *tx)
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
            "SELECT COALESCE(array_agg(key ORDER BY key), '{}'::text[]) \
         FROM public.app_role_permission_keys($1)",
        )
        .bind(role_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "role permission read failed");
            ApiError::Internal
        })?;
        Ok(Json(keys))
    }
    .await;
    match __result {
        Ok(v) => {
            tx.commit().await.map_err(|e| {
                tracing::error!(error = %e, "tenant tx commit failed");
                ApiError::Internal
            })?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
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
    let mut tx = ctx.open_tx(pool).await?;
    let __result = async {
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

    let mut gtx = authz::begin_guarded_transaction(&mut tx, scope)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "guard gtx failed");
            ApiError::Internal
        })?;

    let exists: Option<bool> = sqlx::query_scalar(&format!(
        "SELECT TRUE FROM roles WHERE id = $1 AND {}",
        scoped_visibility("cooperative_id", 2)
    ))
    .bind(role_id)
    .bind(scope)
    .fetch_optional(gtx.as_mut())
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "role existence check failed");
        ApiError::Internal
    })?;
    if exists.is_none() {
        gtx.rollback().await.map_err(|error| {
            tracing::error!(error = %error, "rollback failed");
            ApiError::Internal
        })?;
        return Err(ApiError::NotFound);
    }

    // M1-P4-R1: the delete+insert goes through the boundary definer —
    // it re-verifies roles.manage, enforces the role's scope through
    // the parent row (role_permissions has no cooperative_id), rejects
    // unknown catalog keys and re-checks the last-administrator path.
    let keys: Vec<String> = requested.iter().cloned().collect();
    if let Err(error) =
        sqlx::query("SELECT public.app_role_permissions_set($1, $2)")
            .bind(role_id)
            .bind(&keys)
            .execute(gtx.as_mut())
            .await
    {
        gtx.rollback().await.map_err(|error| {
            tracing::error!(error = %error, "rollback failed");
            ApiError::Internal
        })?;
        return Err(match error {
            e if is_lockout_prevented(&e) => {
                audit_lockout_prevention(pool, &ctx.auth, "role_permissions_change", role_id, scope)
                    .await;
                ApiError::LockoutPrevented
            }
            sqlx::Error::Database(db_err) if is_insufficient_privilege(&*db_err) => {
                ApiError::PermissionDenied
            }
            other => {
                tracing::error!(error = %other, "role permissions set failed");
                ApiError::Internal
            }
        });
    }

    gtx.commit().await.map_err(|error| {
        tracing::error!(error = %error, "commit failed");
        ApiError::Internal
    })?;

    // R4: role_permissions_changed is minted inside
    // app_role_permissions_set (same transaction as the replace — the
    // previous post-commit write could survive a rolled-back mutation
    // and could never be enforced for direct definer calls).
    tracing::info!(outcome = "role_permissions_changed", role_id = %role_id, count = requested.len(), actor = %ctx.auth.user_id);
    Ok(StatusCode::NO_CONTENT.into_response())
    }
    .await;
    match __result {
        Ok(v) => {
            tx.commit().await.map_err(|e| {
                tracing::error!(error = %e, "tenant tx commit failed");
                ApiError::Internal
            })?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
}

/// Cooperative administrator provisions a NEW global identity plus its
/// membership in the sealed scope (M2). The cooperative is derived from
/// the verified scope — never a request field. `memberships.manage` (or
/// the broader `users.manage`) is re-verified inside the definer; the
/// admin-set credential lands `must_change_password = true` (PD-14
/// recommended: operator credentials are one-time credentials).
pub async fn create_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    ctx: BusinessAuth,
    Json(body): Json<CreateUserRequest>,
) -> Result<Response, ApiError> {
    let pool = guarded_business_mutation_any!(
        &state,
        &headers,
        &ctx,
        &[
            authz::catalog::MEMBERSHIPS_MANAGE,
            authz::catalog::USERS_MANAGE
        ],
        &axum::http::Method::POST
    );
    // Validate and hash BEFORE opening the tenant tx — cheap inputs.
    let normalized = crate::auth::identity::normalize_and_validate_username(&body.username)
        .map_err(|_| ApiError::ValidationFailed)?;
    let display_name = body.display_name.trim().to_string();
    if display_name.is_empty() || display_name.chars().count() > 120 {
        return Err(ApiError::ValidationFailed);
    }
    crate::auth::identity::validate_password_policy(&body.password)
        .map_err(|_| ApiError::ValidationFailed)?;
    let password_hash = crate::auth::identity::hash_password(&state.auth.argon2, &body.password)
        .map_err(|error| {
            tracing::error!(error = %error, "argon2 hashing failed");
            ApiError::Internal
        })?;

    let mut tx = ctx.open_tx(pool).await?;
    let __result = async {
        let scope = ctx.cooperative_id();
        let mut guard_tx = authz::begin_guarded_transaction(&mut tx, scope)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "guard tx failed");
                ApiError::Internal
            })?;

        let created: Option<Uuid> = match sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT public.app_user_create_scoped($1, $2, $3, $4)",
        )
        .bind(&normalized)
        .bind(&display_name)
        .bind(&password_hash)
        .bind(&body.password)
        .fetch_one(guard_tx.as_mut())
        .await
        {
            Ok(value) => value,
            Err(sqlx::Error::Database(db_err)) if is_insufficient_privilege(&*db_err) => {
                return Err(ApiError::PermissionDenied);
            }
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
                guard_tx.rollback().await.map_err(|error| {
                    tracing::error!(error = %error, "rollback failed");
                    ApiError::Internal
                })?;
                return Err(ApiError::Conflict);
            }
            Err(error) if is_lifecycle_violation(&error, "cooperative_not_ready") => {
                guard_tx.rollback().await.map_err(|error| {
                    tracing::error!(error = %error, "rollback failed");
                    ApiError::Internal
                })?;
                return Err(ApiError::CooperativeNotReady);
            }
            Err(error)
                if is_lifecycle_violation(&error, "password_too_short")
                    || is_lifecycle_violation(&error, "user_fields_required")
                    || is_lifecycle_violation(&error, "user_field_too_long") =>
            {
                guard_tx.rollback().await.map_err(|error| {
                    tracing::error!(error = %error, "rollback failed");
                    ApiError::Internal
                })?;
                return Err(ApiError::ValidationFailed);
            }
            Err(error) => {
                tracing::error!(error = %error, "user creation failed");
                return Err(ApiError::Internal);
            }
        };

        let user_id = match created {
            Some(id) => id,
            None => {
                guard_tx.rollback().await.map_err(|error| {
                    tracing::error!(error = %error, "rollback failed");
                    ApiError::Internal
                })?;
                return Err(ApiError::Internal);
            }
        };

        let row = sqlx::query_as::<_, MembershipRow>(
            "SELECT m.user_id, u.username, u.display_name, m.status, \
                    m.joined_at, m.ended_at \
             FROM cooperative_memberships m \
             JOIN users u ON u.id = m.user_id \
             WHERE m.user_id = $1 AND m.cooperative_id = $2",
        )
        .bind(user_id)
        .bind(scope)
        .fetch_one(guard_tx.as_mut())
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "created user read-back failed");
            ApiError::Internal
        })?;

        guard_tx.commit().await.map_err(|error| {
            tracing::error!(error = %error, "commit failed");
            ApiError::Internal
        })?;

        tracing::info!(
            outcome = "user_created",
            user_id = %user_id,
            actor = %ctx.auth.user_id
        );
        Ok((
            StatusCode::CREATED,
            Json(MembershipDto {
                user_id: row.user_id,
                username: row.username,
                display_name: row.display_name,
                status: row.status,
                roles: Vec::new(),
                joined_at: row.joined_at,
                ended_at: row.ended_at,
            }),
        )
            .into_response())
    }
    .await;
    match __result {
        Ok(v) => {
            tx.commit().await.map_err(|e| {
                tracing::error!(error = %e, "tenant tx commit failed");
                ApiError::Internal
            })?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
}

/// Cooperative-scoped profile edit (M2): display_name only. Identity
/// fields (username), lifecycle (status) and credentials are outside
/// this lane by design. `memberships.manage`/`users.manage` re-verified
/// inside the definer; a non-member target returns 404 without leaking
/// existence.
pub async fn update_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    ctx: BusinessAuth,
    Path(user_id): Path<Uuid>,
    Json(body): Json<UpdateUserRequest>,
) -> Result<Response, ApiError> {
    let pool = guarded_business_mutation_any!(
        &state,
        &headers,
        &ctx,
        &[
            authz::catalog::MEMBERSHIPS_MANAGE,
            authz::catalog::USERS_MANAGE
        ],
        &axum::http::Method::PATCH
    );
    let display_name = body.display_name.trim().to_string();
    if display_name.is_empty() || display_name.chars().count() > 120 {
        return Err(ApiError::ValidationFailed);
    }

    let mut tx = ctx.open_tx(pool).await?;
    let __result = async {
        let scope = ctx.cooperative_id();
        let mut guard_tx = authz::begin_guarded_transaction(&mut tx, scope)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "guard tx failed");
                ApiError::Internal
            })?;

        let updated: Option<Uuid> = match sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT public.app_user_update_scoped($1, $2)",
        )
        .bind(user_id)
        .bind(&display_name)
        .fetch_one(guard_tx.as_mut())
        .await
        {
            Ok(value) => value,
            Err(sqlx::Error::Database(db_err)) if is_insufficient_privilege(&*db_err) => {
                return Err(ApiError::PermissionDenied);
            }
            Err(error)
                if is_lifecycle_violation(&error, "user_fields_required")
                    || is_lifecycle_violation(&error, "user_field_too_long") =>
            {
                guard_tx.rollback().await.map_err(|error| {
                    tracing::error!(error = %error, "rollback failed");
                    ApiError::Internal
                })?;
                return Err(ApiError::ValidationFailed);
            }
            Err(error) => {
                tracing::error!(error = %error, "user update failed");
                return Err(ApiError::Internal);
            }
        };

        if updated.is_none() {
            guard_tx.rollback().await.map_err(|error| {
                tracing::error!(error = %error, "rollback failed");
                ApiError::Internal
            })?;
            return Err(ApiError::NotFound);
        }

        guard_tx.commit().await.map_err(|error| {
            tracing::error!(error = %error, "commit failed");
            ApiError::Internal
        })?;
        Ok(StatusCode::NO_CONTENT.into_response())
    }
    .await;
    match __result {
        Ok(v) => {
            tx.commit().await.map_err(|e| {
                tracing::error!(error = %e, "tenant tx commit failed");
                ApiError::Internal
            })?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
}

pub async fn list_users(
    State(state): State<AppState>,
    ctx: BusinessAuth,
) -> Result<Json<Vec<UserListItemDto>>, ApiError> {
    authz::require_business(&state, &ctx, authz::catalog::USERS_READ).await?;
    let pool = pool_of(&state)?;
    let mut tx = ctx.open_tx(pool).await?;
    let __result = async {
        let scope = ctx.cooperative_id();
        // In a cooperative scope only members are administrable; the
        // never-bootstrapped lane (scope NULL) keeps the legacy all-users
        // view — no cooperative exists to check membership against.
        let rows = sqlx::query_as::<_, UserListRow>(&format!(
            "SELECT u.id, u.username, u.display_name, u.status, \
                    (SELECT m.status FROM cooperative_memberships m \
                      WHERE m.user_id = u.id AND m.cooperative_id = $1) \
                     AS membership_status \
             FROM users u \
         WHERE {} \
         ORDER BY u.display_name, u.username",
            member_visibility("u.id", 1)
        ))
        .bind(scope)
        .fetch_all(&mut *tx)
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
            .fetch_all(&mut *tx)
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
                membership_status: row.membership_status,
                roles,
            });
        }
        Ok(Json(users))
    }
    .await;
    match __result {
        Ok(v) => {
            tx.commit().await.map_err(|e| {
                tracing::error!(error = %e, "tenant tx commit failed");
                ApiError::Internal
            })?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
}

pub async fn get_user_roles(
    State(state): State<AppState>,
    ctx: BusinessAuth,
    Path(user_id): Path<Uuid>,
) -> Result<Json<Vec<UserRoleDto>>, ApiError> {
    authz::require_business(&state, &ctx, authz::catalog::USERS_READ).await?;
    let pool = pool_of(&state)?;
    let mut tx = ctx.open_tx(pool).await?;
    let __result = async {
        let scope = ctx.cooperative_id();
        let exists: Option<bool> = sqlx::query_scalar(&format!(
            "SELECT TRUE FROM users WHERE id = $1 AND {}",
            member_visibility("$1", 2)
        ))
        .bind(user_id)
        .bind(scope)
        .fetch_optional(&mut *tx)
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
        .fetch_all(&mut *tx)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "user roles read failed");
            ApiError::Internal
        })?;
        Ok(Json(roles))
    }
    .await;
    match __result {
        Ok(v) => {
            tx.commit().await.map_err(|e| {
                tracing::error!(error = %e, "tenant tx commit failed");
                ApiError::Internal
            })?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
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
    let mut tx = ctx.open_tx(pool).await?;
    let __result = async {
    let scope = ctx.cooperative_id();
    let requested: std::collections::BTreeSet<Uuid> = body.role_ids.into_iter().collect();

    let mut gtx = authz::begin_guarded_transaction(&mut tx, scope)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "guard gtx failed");
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
    .fetch_optional(gtx.as_mut())
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "user existence check failed");
        ApiError::Internal
    })?;
    if target_exists.is_none() {
        gtx.rollback().await.map_err(|error| {
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
                .fetch_optional(gtx.as_mut())
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
            gtx.rollback().await.map_err(|error| {
                tracing::error!(error = %error, "rollback failed");
                ApiError::Internal
            })?;
            tracing::info!(outcome, role_id = %role_id);
            return Err(ApiError::ValidationFailed);
        }
    }

    // M1-P4-R1: the delete+insert goes through the boundary definer —
    // it re-verifies users.manage, the target's membership visibility,
    // role assignability (active + cooperative-owned in scope) and the
    // last-administrator protection inside the database.
    let role_ids: Vec<Uuid> = requested.iter().cloned().collect();
    if let Err(error) = sqlx::query("SELECT public.app_user_roles_set($1, $2)")
        .bind(user_id)
        .bind(&role_ids)
        .execute(gtx.as_mut())
        .await
    {
        gtx.rollback().await.map_err(|error| {
            tracing::error!(error = %error, "rollback failed");
            ApiError::Internal
        })?;
        return Err(match error {
            e if is_lockout_prevented(&e) => {
                audit_lockout_prevention(pool, &ctx.auth, "user_roles_change", user_id, scope).await;
                ApiError::LockoutPrevented
            }
            sqlx::Error::Database(db_err) if db_err.is_unique_violation() => ApiError::Conflict,
            sqlx::Error::Database(db_err)
                if db_err.code().as_deref() == Some("23514") =>
            {
                ApiError::ValidationFailed
            }
            sqlx::Error::Database(db_err) if is_insufficient_privilege(&*db_err) => {
                ApiError::PermissionDenied
            }
            other => {
                tracing::error!(error = %other, "user roles set failed");
                ApiError::Internal
            }
        });
    }

    gtx.commit().await.map_err(|error| {
        tracing::error!(error = %error, "commit failed");
        ApiError::Internal
    })?;

    // R4: role_assigned/role_unassigned deltas are minted by the
    // user_role_assignments transition trigger INSIDE the mutation
    // transaction — the previous post-commit writes could neither be
    // rolled back with the mutation nor enforced for direct definer
    // calls.
    tracing::info!(outcome = "user_roles_set", target_user = %user_id, count = requested.len(), actor = %ctx.auth.user_id);

    Ok(StatusCode::NO_CONTENT.into_response())
    }
    .await;
    match __result {
        Ok(v) => {
            tx.commit().await.map_err(|e| {
                tracing::error!(error = %e, "tenant tx commit failed");
                ApiError::Internal
            })?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
}

/// Denial audit (S2B): the refused mutation's transaction was already
/// rolled back, so the event cannot ride it — it goes through the
/// session-bound definer which re-verifies the caller's identity and
/// membership server-side. Best-effort: the denial is already final.
async fn audit_lockout_prevention(
    pool: &sqlx::PgPool,
    auth: &CurrentAuth,
    operation: &str,
    target_id: Uuid,
    scope: Option<Uuid>,
) {
    tracing::warn!(outcome = "lockout_prevented", operation, target = %target_id, actor = %auth.user_id, cooperative_id = ?scope);
    audit::record_scoped_session(
        pool,
        &auth.session_token_hash,
        SecurityEventType::LockoutPrevented,
        scope,
        serde_json::json!({ "operation": operation, "target_id": target_id }),
        auth.authed_at,
    )
    .await;
}

/// M2 PD-14: platform-lane admin password reset. Generates a one-time
/// temporary credential, forces must_change_password, and revokes every
/// session of the target inside the definer — the temporary password
/// is returned ONCE to the caller and never logged or written to audit
/// metadata.
pub async fn platform_reset_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(user_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    csrf::validate_csrf_token(&headers, &auth.csrf_token)
        .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
    let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;

    // One-time credential: 256-bit entropy, never stored in plaintext.
    let temp = format!("r-{}", Uuid::new_v4().simple());
    let hash =
        crate::auth::identity::hash_password(&state.auth.argon2, &temp).map_err(|error| {
            tracing::error!(error = %error, "argon2 hashing failed");
            ApiError::Internal
        })?;

    let (mut tx, _seal) =
        crate::db::request_user_tx(pool, &auth.session_token_hash, auth.authed_at)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "user tx failed");
                ApiError::Internal
            })?;
    let reset: Option<Uuid> = match sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT public.app_platform_password_reset($1, $2, $3)",
    )
    .bind(user_id)
    .bind(&hash)
    .bind(&temp)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(v) => v,
        Err(sqlx::Error::Database(db_err)) if is_insufficient_privilege(&*db_err) => {
            return Err(ApiError::PermissionDenied);
        }
        Err(error) => {
            tracing::error!(error = %error, "password reset failed");
            return Err(ApiError::Internal);
        }
    };
    tx.commit().await.map_err(|error| {
        tracing::error!(error = %error, "reset commit failed");
        ApiError::Internal
    })?;
    let Some(_) = reset else {
        return Err(ApiError::NotFound);
    };
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({ "temporaryPassword": temp })),
    )
        .into_response())
}
