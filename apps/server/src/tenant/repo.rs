//! Tenant persistence (M1-P0). All membership/context checks run
//! against the database on every call — no caching of authorization
//! facts, mirroring `auth::authz` discipline.

use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow)]
pub struct CooperativeRow {
    pub id: Uuid,
    pub name: String,
    pub legal_name: Option<String>,
    pub status: String,
    pub is_bootstrap: bool,
    pub business_enabled: bool,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, sqlx::FromRow)]
pub struct MembershipRow {
    pub id: Uuid,
    pub cooperative_id: Uuid,
    pub user_id: Uuid,
    pub status: String,
    pub joined_at: OffsetDateTime,
    pub ended_at: Option<OffsetDateTime>,
}

/// Safe membership + cooperative summary for `/api/auth/me` and the
/// cooperative list endpoint (no secrets, no internal flags beyond the
/// gate state the UI needs).
#[derive(Debug, sqlx::FromRow, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CooperativeMembershipSummary {
    pub cooperative_id: Uuid,
    pub name: String,
    pub status: String,
    pub business_enabled: bool,
    pub membership_status: String,
}

#[derive(Debug)]
pub enum CreateCooperativeError {
    DuplicateName,
    Database(sqlx::Error),
}

#[derive(Debug)]
pub enum MembershipError {
    CooperativeNotFound,
    UserNotFound,
    Database(sqlx::Error),
}

/// CLI provisioning (M1-K7): create a cooperative in `provisioning`
/// status with `business_enabled = false`. Never reachable over HTTP.
pub async fn create_cooperative(
    pool: &sqlx::PgPool,
    name: &str,
    legal_name: Option<&str>,
) -> Result<CooperativeRow, CreateCooperativeError> {
    let name = name.trim();
    let result = sqlx::query_as::<_, CooperativeRow>(
        "INSERT INTO cooperatives (name, legal_name, status, business_enabled) \
         VALUES ($1, $2, 'provisioning', false) \
         RETURNING id, name, legal_name, status, is_bootstrap, business_enabled, created_at",
    )
    .bind(name)
    .bind(legal_name.map(str::trim))
    .fetch_one(pool)
    .await;

    match result {
        Ok(row) => Ok(row),
        Err(sqlx::Error::Database(db_error))
            if db_error.constraint() == Some("cooperatives_name_uq") =>
        {
            Err(CreateCooperativeError::DuplicateName)
        }
        Err(error) => Err(CreateCooperativeError::Database(error)),
    }
}

/// One-time, idempotent bootstrap of the initial cooperative for an
/// existing single-cooperative installation (M1-P0 P0-D).
///
/// - Creates the unique `is_bootstrap` cooperative (active +
///   business-enabled) if absent; otherwise reuses it.
/// - Ensures EVERY user holds an `active` membership — preserves
///   existing single-cooperative workflows: users keep working while
///   tenant context is introduced.
/// - Returns `(cooperative_id, membership_count)`; repeat calls only
///   fill gaps, never duplicate or reassign.
pub async fn bootstrap_initial_cooperative(
    pool: &sqlx::PgPool,
    name: &str,
) -> Result<(Uuid, i64), sqlx::Error> {
    let mut tx = pool.begin().await?;

    let existing: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM cooperatives WHERE is_bootstrap")
            .fetch_optional(&mut *tx)
            .await?;

    let coop_id = match existing {
        Some(id) => id,
        None => {
            sqlx::query_scalar::<_, Uuid>(
                "INSERT INTO cooperatives \
                     (name, status, is_bootstrap, business_enabled) \
                 VALUES ($1, 'active', true, true) RETURNING id",
            )
            .bind(name.trim())
            .fetch_one(&mut *tx)
            .await?
        }
    };

    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         SELECT $1, u.id, 'active' FROM users u \
         ON CONFLICT (cooperative_id, user_id) DO NOTHING",
    )
    .bind(coop_id)
    .execute(&mut *tx)
    .await?;

    let member_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cooperative_memberships WHERE cooperative_id = $1",
    )
    .bind(coop_id)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok((coop_id, member_count))
}

/// Grant a membership (CLI, M1-K7). `ON CONFLICT` revives a `suspended`
/// or `ended` membership back to `active` instead of erroring — the row
/// is never duplicated (history-safe reactivation).
pub async fn add_member(
    pool: &sqlx::PgPool,
    cooperative_name: &str,
    user_id: Uuid,
    created_by: Option<Uuid>,
) -> Result<MembershipRow, MembershipError> {
    let coop: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM cooperatives WHERE lower(name) = lower($1)")
            .bind(cooperative_name.trim())
            .fetch_optional(pool)
            .await
            .map_err(MembershipError::Database)?;
    let Some(coop_id) = coop else {
        return Err(MembershipError::CooperativeNotFound);
    };

    let user_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = $1)")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .map_err(MembershipError::Database)?;
    if !user_exists {
        return Err(MembershipError::UserNotFound);
    }

    sqlx::query_as::<_, MembershipRow>(
        "INSERT INTO cooperative_memberships \
             (cooperative_id, user_id, status, created_by_user_id) \
         VALUES ($1, $2, 'active', $3) \
         ON CONFLICT (cooperative_id, user_id) DO UPDATE \
             SET status = 'active', ended_at = NULL, updated_at = now() \
         RETURNING id, cooperative_id, user_id, status, joined_at, ended_at",
    )
    .bind(coop_id)
    .bind(user_id)
    .bind(created_by)
    .fetch_one(pool)
    .await
    .map_err(MembershipError::Database)
}

/// Flat join row for `resolve_membership` (sqlx tuple-of-FromRow is not
/// supported; aliased columns keep the two records unambiguous).
#[derive(Debug, sqlx::FromRow)]
pub struct MembershipCooperativeRow {
    pub membership_status: String,
    pub cooperative_name: String,
    pub cooperative_status: String,
    pub business_enabled: bool,
}

/// Membership + cooperative resolution used by `TenantCtx` and the
/// cooperative-switch endpoint. Returns the joined row only when a
/// membership exists; the caller applies the status/business_enabled
/// gates explicitly (fail closed).
pub async fn resolve_membership(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    cooperative_id: Uuid,
) -> Result<Option<MembershipCooperativeRow>, sqlx::Error> {
    sqlx::query_as::<_, MembershipCooperativeRow>(
        "SELECT m.status AS membership_status, c.name AS cooperative_name, \
                c.status AS cooperative_status, c.business_enabled \
         FROM cooperative_memberships m \
         JOIN cooperatives c ON c.id = m.cooperative_id \
         WHERE m.user_id = $1 AND m.cooperative_id = $2",
    )
    .bind(user_id)
    .bind(cooperative_id)
    .fetch_optional(pool)
    .await
}

/// All memberships of a user with their cooperative — `/api/auth/me`
/// and the cooperative list endpoint.
pub async fn list_user_cooperatives(
    pool: &sqlx::PgPool,
    user_id: Uuid,
) -> Result<Vec<CooperativeMembershipSummary>, sqlx::Error> {
    sqlx::query_as::<_, CooperativeMembershipSummary>(
        "SELECT m.cooperative_id, c.name, c.status, c.business_enabled, \
                m.status AS membership_status \
         FROM cooperative_memberships m \
         JOIN cooperatives c ON c.id = m.cooperative_id \
         WHERE m.user_id = $1 \
         ORDER BY c.name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

/// Persist the UX-level default cooperative on the session row
/// (M1-K1: convenience only — never a security decision on its own).
pub async fn set_session_cooperative(
    pool: &sqlx::PgPool,
    session_id: Uuid,
    cooperative_id: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE user_sessions SET active_cooperative_id = $2 WHERE id = $1")
        .bind(session_id)
        .bind(cooperative_id)
        .execute(pool)
        .await?;
    Ok(())
}
