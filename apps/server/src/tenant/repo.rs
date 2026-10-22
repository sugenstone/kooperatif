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
/// - Records the durable lifecycle marker in `installation_state`
///   inside the same transaction (M1-P1-005): the marker row is the
///   authoritative proof that bootstrap happened, and its FK makes the
///   bootstrap cooperative undeletable while referenced.
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
        "UPDATE installation_state \
         SET bootstrap_cooperative_id = $1, \
             tenant_bootstrapped_at = COALESCE(tenant_bootstrapped_at, now())",
    )
    .bind(coop_id)
    .execute(&mut *tx)
    .await?;

    // Disabled users must never receive an ACTIVE membership from
    // bootstrap: membership is deliberately not created at all, so a
    // later user reactivation cannot silently revive cooperative access
    // — access then requires an explicit `add-member` decision.
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         SELECT $1, u.id, 'active' FROM users u \
         WHERE u.status = 'active' \
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

    // Seed the known system categories BOUND to this cooperative.
    // Since 0025 closed the NULL lane (cooperative_id NOT NULL + the
    // 0010-era global seeds removed), the canonical ten are inserted
    // here rather than claimed — same seed list, `created_by IS NULL`
    // keeps them distinguishable as system rows. Idempotent at the
    // bootstrap level: the whole function is single-shot and the body
    // rolls back as one transaction.
    sqlx::query(
        "INSERT INTO financial_categories \
             (category_type, name, created_by, cooperative_id) \
         VALUES \
             ('income',  'Diğer Gelir',               NULL, $1), \
             ('income',  'Kira Geliri',               NULL, $1), \
             ('income',  'Yatırım Geliri',            NULL, $1), \
             ('expense', 'Genel Gider',               NULL, $1), \
             ('expense', 'Kira',                      NULL, $1), \
             ('expense', 'Ulaşım',                    NULL, $1), \
             ('expense', 'Bakım / Onarım',            NULL, $1), \
             ('expense', 'Elektrik / Su / İletişim',  NULL, $1), \
             ('expense', 'Organizasyon',              NULL, $1), \
             ('expense', 'Diğer Gider',               NULL, $1) \
         ON CONFLICT (cooperative_id, category_type, lower(btrim(name))) \
         DO NOTHING",
    )
    .bind(coop_id)
    .execute(&mut *tx)
    .await?;

    // Fail closed (P0): after the seed claim, ANY remaining NULL
    // business row is user-created data without an explicit owner —
    // produced by pre-bootstrap operation in the NeverBootstrapped
    // lane or by a 0021-skipped database. Silently assigning it would
    // rewrite historical ownership, so the whole bootstrap aborts and
    // rolls back. Recovery is a deliberate operator decision (the 0021
    // backfill path), never an implicit side effect of this command.
    // Table list mirrors 0021's claim list exactly; RBAC and
    // security_events are excluded (separate NULL semantics).
    // M1-P4-P3A: the scan runs through a SECURITY DEFINER function —
    // under RLS (pilot: periods, governance_bodies) per-table EXISTS
    // probes as the runtime principal would be silently filtered and
    // the check would go fail-open.
    let orphan: Option<String> = sqlx::query_scalar("SELECT public.app_bootstrap_orphan_scan()")
        .fetch_one(&mut *tx)
        .await?;
    if let Some(table) = orphan {
        return Err(sqlx::Error::Protocol(format!(
            "bootstrap fail-closed: '{table}' contains business rows without \
             a cooperative owner — refusing to silently assign historical \
             ownership; resolve with a deliberate operator backfill (0021) \
             before retrying"
        )));
    }

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
///
/// M1-P4-S2A: credential-bound — resolves through the
/// `app_session_membership` SECURITY DEFINER, which derives the user id
/// from the presented session token hash inside the database (never a
/// caller-supplied `user_id`). An invalid/expired session returns zero
/// rows: same fail-closed shape as "not a member".
pub async fn resolve_membership(
    pool: &sqlx::PgPool,
    session_token_hash: &[u8; 32],
    cooperative_id: Uuid,
    now: OffsetDateTime,
) -> Result<Option<MembershipCooperativeRow>, sqlx::Error> {
    sqlx::query_as::<_, MembershipCooperativeRow>(
        "SELECT membership_status, cooperative_name, cooperative_status, \
                business_enabled \
         FROM public.app_session_membership($1, $2, $3)",
    )
    .bind(session_token_hash.as_slice())
    .bind(cooperative_id)
    .bind(now)
    .fetch_optional(pool)
    .await
}

/// All memberships of a user with their cooperative — `/api/auth/me`
/// and the cooperative list endpoint.
///
/// M1-P4-S2A: credential-bound via `app_session_memberships` — the
/// result can never contain another user's rows.
pub async fn list_user_cooperatives(
    pool: &sqlx::PgPool,
    session_token_hash: &[u8; 32],
    now: OffsetDateTime,
) -> Result<Vec<CooperativeMembershipSummary>, sqlx::Error> {
    sqlx::query_as::<_, CooperativeMembershipSummary>(
        "SELECT cooperative_id, name, status, business_enabled, \
                membership_status \
         FROM public.app_session_memberships($1, $2)",
    )
    .bind(session_token_hash.as_slice())
    .bind(now)
    .fetch_all(pool)
    .await
}

/// Resolve the bootstrap cooperative through the durable lifecycle
/// marker (M1-P1-005) — never from the `is_bootstrap` flag alone.
///
/// `installation_state.bootstrap_cooperative_id` is authoritative; the
/// JOIN additionally requires the referenced row to still carry
/// `is_bootstrap`. Returns `None` for BOTH legitimately-absent and
/// inconsistent states (NULL marker, flag-stripped cooperative, marker
/// pointing at a non-bootstrap row) — callers disambiguate with
/// `never_bootstrapped`, which fails closed on inconsistency.
pub async fn resolve_bootstrap(pool: &sqlx::PgPool) -> Result<Option<(Uuid, String)>, sqlx::Error> {
    // M1-P4-R4: marker+flag agreement is resolved inside the definer —
    // cooperatives/installation_state are no longer SELECTable by the
    // runtime role (enumeration closure).
    sqlx::query_as(
        "SELECT cooperative_id, cooperative_name \
         FROM public.app_resolve_bootstrap()",
    )
    .fetch_optional(pool)
    .await
}

/// Whether the one-time bootstrap has been completed — the only moment
/// the tenant model is "engaged". Before bootstrap, zero cooperatives
/// exist and the pre-tenant single-cooperative access rules apply;
/// afterwards, every business request must resolve through membership.
///
/// The durable marker is the primary evidence (M1-P1-005); the
/// `is_bootstrap` flag is kept as a secondary signal so a database
/// whose marker was not yet written (pre-0020 state mid-upgrade) is
/// still treated as engaged rather than reopened.
pub async fn bootstrap_completed(pool: &sqlx::PgPool) -> Result<bool, sqlx::Error> {
    // M1-P4-R4: same marker+flag rule behind the verified definer.
    sqlx::query_scalar("SELECT public.app_bootstrap_completed()")
        .fetch_one(pool)
        .await
}

/// Whether the installation has provably NEVER run tenant bootstrap
/// (P1-c1, hardened in M1-P1-005). The legacy lane may open ONLY when
/// ALL of the following hold:
///   1. the singleton `installation_state` row exists (a missing marker
///      record is itself evidence of tampering/partial migration);
///   2. its `bootstrap_cooperative_id` is NULL — no bootstrap was ever
///      recorded transactionally;
///   3. zero cooperative rows exist;
///   4. zero cooperative-attributed audit events exist.
///
/// Any other combination is a broken or previously-tenanted state and
/// must fail closed — a missing `is_bootstrap` row alone is NEVER
/// sufficient evidence.
///
/// M1-P4-S3: evaluated inside `app_session_legacy_lane_ok()` — the
/// definer runs as the table owner, so FORCE RLS on `security_events`
/// cannot silently empty the audit-evidence arm of this predicate.
pub async fn never_bootstrapped(pool: &sqlx::PgPool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT public.app_session_legacy_lane_ok()")
        .fetch_one(pool)
        .await
}

/// Whether the session owner holds an active membership in an active,
/// business-enabled cooperative — the M1 §5 gate predicate for
/// not-yet-tenant-scoped business modules.
///
/// M1-P4-S2A: credential-bound via `app_session_business_membership_ok`
/// (the session token hash, not a caller-supplied `user_id`, selects
/// the principal).
pub async fn has_enabled_business_membership(
    pool: &sqlx::PgPool,
    session_token_hash: &[u8; 32],
    now: OffsetDateTime,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT public.app_session_business_membership_ok($1, $2)")
        .bind(session_token_hash.as_slice())
        .bind(now)
        .fetch_one(pool)
        .await
}

/// Persist the UX-level default cooperative on the session row
/// (M1-K1: convenience only — never a security decision on its own).
///
/// Credential-bound since M1-P4-P3B: the definer resolves the hash
/// owner's current session and refuses non-members.
pub async fn set_session_cooperative(
    pool: &sqlx::PgPool,
    token_hash: [u8; 32],
    cooperative_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>("SELECT public.app_session_set_cooperative($1, $2)")
        .bind(token_hash.as_slice())
        .bind(cooperative_id)
        .fetch_one(pool)
        .await
}
