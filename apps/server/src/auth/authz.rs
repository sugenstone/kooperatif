//! Authorization service (STEP-003 §17/§18, docs/18).
//!
//! Centralized, DB-resolved permission decisions:
//!   User → active UserRoleAssignments → active Roles → RolePermissions
//!        → effective permission keys (union, default DENY, no wildcards).
//!
//! - Resolution happens server-side on EVERY protected request (fresh by
//!   construction; no caching, nothing authorization-related in the
//!   cookie — ADR-002 unchanged).
//! - Handlers enforce with a single explicit `require`/`require_any`
//!   call: no scattered SQL joins, no hidden magic, auditable at a
//!   glance (§18).
//! - The last-administration-path guard serializes lockout-sensitive
//!   mutations with a PostgreSQL transaction-scoped advisory lock and
//!   re-verifies the invariant on the POST-mutation state inside the
//!   transaction (§28/§44 — database-enforced, never a process mutex).

use std::collections::HashSet;

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::extractor::CurrentAuth;
use crate::http::error::ApiError;
use crate::http::AppState;

/// STEP-003 permission catalog keys (single source of truth mirrored by
/// migration 0003 seeds and `@kooperatif/contracts`).
pub mod catalog {
    pub const USERS_READ: &str = "users.read";
    pub const USERS_MANAGE: &str = "users.manage";
    pub const ROLES_READ: &str = "roles.read";
    pub const ROLES_MANAGE: &str = "roles.manage";
    // STEP-004 (parties domain).
    pub const SHAREHOLDERS_READ: &str = "shareholders.read";
    pub const SHAREHOLDERS_MANAGE: &str = "shareholders.manage";
    pub const FAMILIES_READ: &str = "families.read";
    pub const FAMILIES_MANAGE: &str = "families.manage";
    // STEP-005 (shares domain).
    pub const SHARES_READ: &str = "shares.read";
    pub const SHARES_MANAGE: &str = "shares.manage";
    // STEP-006 (periods & assessments domain).
    pub const PERIODS_READ: &str = "periods.read";
    pub const PERIODS_MANAGE: &str = "periods.manage";
    pub const ASSESSMENTS_READ: &str = "assessments.read";
    pub const ASSESSMENTS_MANAGE: &str = "assessments.manage";
    // STEP-007 (payments domain).
    pub const PAYMENTS_READ: &str = "payments.read";
    pub const PAYMENTS_MANAGE: &str = "payments.manage";
    // STEP-008 (financial accounts domain).
    pub const FINANCIAL_ACCOUNTS_READ: &str = "financial_accounts.read";
    pub const FINANCIAL_ACCOUNTS_MANAGE: &str = "financial_accounts.manage";

    /// STEP-009 — Shareholder Credit ("Fazla Ödeme") surfaces.
    pub const CREDITS_READ: &str = "credits.read";
    pub const CREDITS_MANAGE: &str = "credits.manage";

    /// STEP-010 — Income/Expense ("Gelir/Gider") surfaces.
    pub const INCOME_EXPENSE_READ: &str = "income_expense.read";
    pub const INCOME_EXPENSE_MANAGE: &str = "income_expense.manage";

    /// STEP-011 — Share Return / Entitlement / Settlement surfaces.
    pub const SHARE_RETURNS_READ: &str = "share_returns.read";
    pub const SHARE_RETURNS_MANAGE: &str = "share_returns.manage";
    pub const INVESTMENTS_READ: &str = "investments.read";
    pub const INVESTMENTS_MANAGE: &str = "investments.manage";

    /// STEP-013 — Social Aid surfaces. Sensitive data (docs/11
    /// §privacy, docs/22): its own read/manage pair, never folded
    /// into broader financial permissions.
    pub const SOCIAL_AID_READ: &str = "social_aid.read";
    pub const SOCIAL_AID_MANAGE: &str = "social_aid.manage";

    /// STEP-014 — Governance surfaces. `governance.manage` lets an
    /// operator RECORD governance facts; it never makes the user a
    /// body member or voter (docs/18: RBAC != governance).
    pub const GOVERNANCE_READ: &str = "governance.read";
    pub const GOVERNANCE_MANAGE: &str = "governance.manage";

    /// STEP-015 — cross-domain reporting. A single read permission
    /// guards the reporting surface: holding one domain's read grant
    /// never implies cross-domain report access (docs/14 §permissions,
    /// docs/18). There is no `reports.manage` — reports are read-only
    /// projections and never mutate state.
    pub const REPORTS_READ: &str = "reports.read";

    /// M1-P5 B4 — audit read surface. A single key with two distinct
    /// lanes: cooperative-scoped assignment → `GET /api/audit/events`
    /// for that cooperative only; NULL-scope assignment →
    /// `GET /api/platform/audit/events` (platform lane, D4). A
    /// cooperative-scoped grant can never satisfy the platform lane.
    /// Deliberately not implied by `users.manage` (D2).
    pub const AUDIT_READ: &str = "audit.read";

    /// M1-P6 B1 — cooperative lifecycle authority (create, activate,
    /// suspend, archive). PLATFORM-scope only: a cooperative-scoped
    /// grant of this key must never authorize platform-wide
    /// cooperative administration (mirrors the `audit.read` lane
    /// split). Consumed by the B2 platform endpoints; B1 grants it to
    /// no one.
    pub const COOPERATIVES_MANAGE: &str = "cooperatives.manage";

    /// M1-P6 B1 — cooperative-scoped membership administration (add
    /// existing users to the own cooperative, list members). Distinct
    /// from `users.manage`: identity vs membership separation (0060).
    /// Consumed by the B3 membership endpoints; B1 grants it to no one.
    pub const MEMBERSHIPS_MANAGE: &str = "memberships.manage";
    /// M1-P6 B4: read memberships + cooperative-scoped role assignments
    /// in the authorized cooperative (separated from `memberships.manage`).
    pub const MEMBERSHIPS_READ: &str = "memberships.read";

    /// Every permission key the application owns. Adding a permission
    /// is a controlled catalog change: migration seed + this list +
    /// contracts (see README "adding a new permission").
    pub const ALL: &[&str] = &[
        USERS_READ,
        USERS_MANAGE,
        ROLES_READ,
        ROLES_MANAGE,
        SHAREHOLDERS_READ,
        SHAREHOLDERS_MANAGE,
        FAMILIES_READ,
        FAMILIES_MANAGE,
        SHARES_READ,
        SHARES_MANAGE,
        PERIODS_READ,
        PERIODS_MANAGE,
        ASSESSMENTS_READ,
        ASSESSMENTS_MANAGE,
        PAYMENTS_READ,
        PAYMENTS_MANAGE,
        FINANCIAL_ACCOUNTS_READ,
        FINANCIAL_ACCOUNTS_MANAGE,
        CREDITS_READ,
        CREDITS_MANAGE,
        INCOME_EXPENSE_READ,
        INCOME_EXPENSE_MANAGE,
        SHARE_RETURNS_READ,
        SHARE_RETURNS_MANAGE,
        INVESTMENTS_READ,
        INVESTMENTS_MANAGE,
        SOCIAL_AID_READ,
        SOCIAL_AID_MANAGE,
        GOVERNANCE_READ,
        GOVERNANCE_MANAGE,
        REPORTS_READ,
        AUDIT_READ,
        COOPERATIVES_MANAGE,
        MEMBERSHIPS_MANAGE,
        MEMBERSHIPS_READ,
    ];

    pub fn is_known(key: &str) -> bool {
        ALL.contains(&key)
    }
}

/// The permission whose total loss makes normal-application RBAC
/// administration unrestorable: holders of `roles.manage` can add any
/// catalog permission (including `users.manage`) back to a role they
/// hold, so preserving one active `roles.manage` path preserves
/// administrative recoverability (STEP-003 §28).
pub const ADMINISTRATION_PERMISSION: &str = catalog::ROLES_MANAGE;

/// Advisory-lock key for last-administration-path sensitive mutations.
/// One fixed key serializes exactly these mutations database-wide —
/// retained for the never-bootstrapped legacy lane only.
const ADMIN_PATH_GUARD_LOCK_KEY: i64 = 0x4B_4F_4F_50_30_30_33; // "KOOP003"

/// Advisory-lock namespace for cooperative-scoped administration-path
/// guards (M1-P1-c2b, P1-K5, ADR-006 scoped locking).
///
/// The lock is taken in the two-int form
/// `pg_advisory_xact_lock(NAMESPACE, hashtext(cooperative_id::text))`:
/// each cooperative serializes its own administrator-continuity
/// mutations while mutations in OTHER cooperatives are never blocked —
/// Cooperative A's administrator changes can neither wait on nor affect
/// Cooperative B's. The two-int form also cannot collide with the
/// legacy bigint key above (PostgreSQL maps the two forms onto
/// different lock spaces). A `hashtext` collision between two
/// cooperative ids only adds serialization — it can never widen or
/// narrow a scope's administrator count.
///
/// Public: the constant IS the locking contract — tests derive the
/// same key to prove same-cooperative serialization and
/// cross-cooperative independence.
pub const ADMIN_PATH_GUARD_LOCK_NAMESPACE: i32 = 0x4B4F4F50; // "KOOP"

/// Effective permission keys of a user: union over active roles only.
pub async fn effective_permissions(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<HashSet<String>, sqlx::Error> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT p.key \
         FROM user_role_assignments ura \
         JOIN roles r ON r.id = ura.role_id AND r.status = 'active' \
         JOIN role_permissions rp ON rp.role_id = r.id \
         JOIN permissions p ON p.id = rp.permission_id \
         WHERE ura.user_id = $1",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// M1-P1-b — cooperative-scoped resolution (P1-K1/P1-K2/P1-K3).
///
/// Resolves permission keys against ONE explicit cooperative scope. The
/// cooperative_id is a scope, not a credential: the membership join
/// makes an unverified value harmless — a cooperative the user does not
/// actively belong to yields an empty set (default DENY).
///
/// Guarantees over the global `effective_permissions`:
/// - only assignments AND roles carrying THIS cooperative count;
///   `cooperative_id IS NULL` rows (the temporary pre-tenant legacy lane
///   from migration 0019) are excluded;
/// - an `active` cooperative_memberships row in the same cooperative is
///   required, so suspended/ended memberships resolve to nothing;
/// - no global fallback — a role owned by another cooperative can never
///   leak in (the composite FK already makes such a row uninsertable).
///
/// Per-request, per-scope resolution: no caching, so role or membership
/// changes apply to the very next request.
pub async fn effective_permissions_in_cooperative(
    pool: &PgPool,
    user_id: Uuid,
    cooperative_id: Uuid,
) -> Result<HashSet<String>, sqlx::Error> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT p.key \
         FROM user_role_assignments ura \
         JOIN roles r ON r.id = ura.role_id AND r.status = 'active' \
                      AND r.cooperative_id = $2 \
         JOIN cooperative_memberships m \
           ON m.cooperative_id = $2 AND m.user_id = ura.user_id \
          AND m.status = 'active' \
         JOIN role_permissions rp ON rp.role_id = r.id \
         JOIN permissions p ON p.id = rp.permission_id \
         WHERE ura.user_id = $1 AND ura.cooperative_id = $2",
    )
    .bind(user_id)
    .bind(cooperative_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Cooperative-scoped role summaries (id + display name), safe for API
/// responses — P1-b counterpart of `active_role_summaries`.
pub async fn active_role_summaries_in_cooperative(
    pool: &PgPool,
    user_id: Uuid,
    cooperative_id: Uuid,
) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT r.id, r.name \
         FROM user_role_assignments ura \
         JOIN roles r ON r.id = ura.role_id AND r.status = 'active' \
                      AND r.cooperative_id = $2 \
         JOIN cooperative_memberships m \
           ON m.cooperative_id = $2 AND m.user_id = ura.user_id \
          AND m.status = 'active' \
         WHERE ura.user_id = $1 AND ura.cooperative_id = $2 \
         ORDER BY r.name",
    )
    .bind(user_id)
    .bind(cooperative_id)
    .fetch_all(pool)
    .await
}

/// Bootstrap-scoped role summaries (id + display name) — the `me`
/// counterpart of `effective_permissions_in_bootstrap` (M1-P1-c3a):
/// bootstrap-owned plus NULL-attributed legacy rows, but ONLY through
/// an active bootstrap membership and ONLY when `$2` resolves as the
/// marker-consistent bootstrap cooperative inside the query itself. A
/// foreign-cooperative role can never appear in this list.
pub async fn active_role_summaries_in_bootstrap(
    pool: &PgPool,
    user_id: Uuid,
    bootstrap_id: Uuid,
) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT r.id, r.name \
         FROM user_role_assignments ura \
         JOIN roles r ON r.id = ura.role_id AND r.status = 'active' \
                      AND (r.cooperative_id = $2 \
                           OR (r.cooperative_id IS NULL AND EXISTS( \
                               SELECT 1 FROM installation_state s \
                               JOIN cooperatives c \
                                 ON c.id = s.bootstrap_cooperative_id \
                                AND c.is_bootstrap \
                               WHERE s.bootstrap_cooperative_id = $2))) \
         JOIN cooperative_memberships m \
           ON m.cooperative_id = $2 AND m.user_id = ura.user_id \
          AND m.status = 'active' \
         WHERE ura.user_id = $1 \
           AND (ura.cooperative_id = $2 \
                OR (ura.cooperative_id IS NULL AND EXISTS( \
                    SELECT 1 FROM installation_state s \
                    JOIN cooperatives c \
                      ON c.id = s.bootstrap_cooperative_id \
                     AND c.is_bootstrap \
                    WHERE s.bootstrap_cooperative_id = $2))) \
         ORDER BY r.name",
    )
    .bind(user_id)
    .bind(bootstrap_id)
    .fetch_all(pool)
    .await
}

/// Cooperative-scoped permission enforcement (P1-b, P1-K2).
///
/// Accepts a server-verified `TenantCtx` — never a raw request header —
/// so the cooperative scope has already passed membership/status/
/// business_enabled validation. Membership is additionally re-verified
/// by the resolver join, keeping the check correct even if the session's
/// cooperative data went stale between extraction and enforcement.
///
/// Introduced as a separate path (§4 of the P1-a task): legacy business
/// endpoints still call global `require` until their call sites migrate
/// in P1-c. No global permission fallback exists in this function.
pub async fn require_in_cooperative(
    state: &AppState,
    ctx: &crate::tenant::TenantCtx,
    permission: &str,
) -> Result<(), ApiError> {
    let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;
    // M1-P4-R4: permission resolution is credential-bound — the
    // session token hash selects the principal inside the database
    // (app_session_permissions_scoped); the rp/permissions tables are
    // no longer directly readable by the runtime role.
    let permissions = session_permissions_scoped(
        pool,
        &ctx.auth.session_token_hash,
        ctx.cooperative_id,
        state.auth.clock.now(),
    )
    .await
    .map_err(|error| {
        tracing::error!(
            error = %error,
            permission,
            "scoped permission resolution failed"
        );
        ApiError::Internal
    })?;
    if permissions.contains(permission) {
        Ok(())
    } else {
        tracing::info!(
            outcome = "permission_denied",
            user_id = %ctx.auth.user_id,
            cooperative_id = %ctx.cooperative_id,
            permission,
            "scoped authorization denied"
        );
        Err(ApiError::PermissionDenied)
    }
}

/// M1-P1-c1 — bootstrap-scope resolution WITH verified legacy
/// attribution (P1-K3, §4 of the P1-c1 task).
///
/// NULL-cooperative role/assignment rows can only have been written in
/// exactly two ways: (a) before tenancy existed, or (b) by a legacy
/// NULL-writing path (rbac.rs before P1-c2, `grant-role` before P1-d)
/// that only ever served the single bootstrap cooperative. They are
/// therefore attributable to the bootstrap cooperative — and ONLY
/// there. Deterministic rule, never a cross-cooperative fallback:
/// `cooperative_id IS NULL` counts when the verified scope is the
/// bootstrap cooperative; it never counts in any other scope (the
/// strict resolver `effective_permissions_in_cooperative` already
/// proves that lane).
///
/// An `active` bootstrap membership is still mandatory — a NULL
/// assignment cannot create access without it.
///
/// M1-P1-c2b hardening: the NULL-attribution arm is now ENCODED in the
/// query itself — a `IS NULL` row only counts when `$2` resolves as the
/// marker-consistent bootstrap cooperative at resolution time
/// (installation_state + is_bootstrap flag agreement, same rule as
/// `tenant::repo::resolve_bootstrap`). Even a bug that passed another
/// cooperative's id here could never attribute legacy NULL rows to it:
/// the database re-verifies the bootstrap claim instead of trusting the
/// caller's parameter name.
pub async fn effective_permissions_in_bootstrap(
    pool: &PgPool,
    user_id: Uuid,
    bootstrap_id: Uuid,
) -> Result<HashSet<String>, sqlx::Error> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT p.key \
         FROM user_role_assignments ura \
         JOIN roles r ON r.id = ura.role_id AND r.status = 'active' \
                      AND (r.cooperative_id = $2 \
                           OR (r.cooperative_id IS NULL AND EXISTS( \
                               SELECT 1 FROM installation_state s \
                               JOIN cooperatives c \
                                 ON c.id = s.bootstrap_cooperative_id \
                                AND c.is_bootstrap \
                               WHERE s.bootstrap_cooperative_id = $2))) \
         JOIN cooperative_memberships m \
           ON m.cooperative_id = $2 AND m.user_id = ura.user_id \
          AND m.status = 'active' \
         JOIN role_permissions rp ON rp.role_id = r.id \
         JOIN permissions p ON p.id = rp.permission_id \
         WHERE ura.user_id = $1 \
           AND (ura.cooperative_id = $2 \
                OR (ura.cooperative_id IS NULL AND EXISTS( \
                    SELECT 1 FROM installation_state s \
                    JOIN cooperatives c \
                      ON c.id = s.bootstrap_cooperative_id \
                     AND c.is_bootstrap \
                    WHERE s.bootstrap_cooperative_id = $2)))",
    )
    .bind(user_id)
    .bind(bootstrap_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// M1-P4-S2A — credential-bound scoped resolution.
///
/// The pre-context lane cannot trust a caller-supplied `user_id`: the
/// SECURITY DEFINER reader derives the principal from the session token
/// hash inside the database (same validity rule as
/// `app_session_resolve`), then applies the bootstrap-aware scoped join
/// — an `active` membership in `cooperative_id` is mandatory, and
/// NULL-attributed legacy rows count only when `cooperative_id`
/// re-verifies as the marker-consistent bootstrap cooperative.
/// An invalid session or inapplicable scope returns an empty set
/// (default DENY).
pub async fn session_permissions_scoped(
    pool: &PgPool,
    session_token_hash: &[u8; 32],
    cooperative_id: Uuid,
    now: OffsetDateTime,
) -> Result<HashSet<String>, sqlx::Error> {
    let rows: Vec<String> =
        sqlx::query_scalar("SELECT public.app_session_permissions_scoped($1, $2, $3)")
            .bind(session_token_hash.as_slice())
            .bind(cooperative_id)
            .bind(now)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().collect())
}

/// M1-P4-S2A — credential-bound legacy global resolution. The definer
/// re-verifies the `never_bootstrapped` predicate inside the database;
/// once tenancy exists (or on broken residue) the global set is
/// unreachable and the result is empty.
pub async fn session_permissions_legacy(
    pool: &PgPool,
    session_token_hash: &[u8; 32],
    now: OffsetDateTime,
) -> Result<HashSet<String>, sqlx::Error> {
    let rows: Vec<String> =
        sqlx::query_scalar("SELECT public.app_session_permissions_legacy($1, $2)")
            .bind(session_token_hash.as_slice())
            .bind(now)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().collect())
}

/// M1-P4-S2A — credential-bound scoped role summaries.
pub async fn session_role_summaries_scoped(
    pool: &PgPool,
    session_token_hash: &[u8; 32],
    cooperative_id: Uuid,
    now: OffsetDateTime,
) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT role_id, role_name \
         FROM public.app_session_role_summaries_scoped($1, $2, $3)",
    )
    .bind(session_token_hash.as_slice())
    .bind(cooperative_id)
    .bind(now)
    .fetch_all(pool)
    .await
}

/// M1-P4-S2A — credential-bound legacy global role summaries (same
/// never-bootstrapped self-check as `session_permissions_legacy`).
pub async fn session_role_summaries_legacy(
    pool: &PgPool,
    session_token_hash: &[u8; 32],
    now: OffsetDateTime,
) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT role_id, role_name \
         FROM public.app_session_role_summaries_legacy($1, $2)",
    )
    .bind(session_token_hash.as_slice())
    .bind(now)
    .fetch_all(pool)
    .await
}

/// M1-P1-c1 — verified business authorization for legacy business
/// handlers.
///
/// Takes a `BusinessAuth` built by its extractor (never a raw UUID or
/// header value):
/// - `BusinessScope::Cooperative` → permissions resolved in the verified
///   bootstrap scope, NULL-lane attribution included (see
///   `effective_permissions_in_bootstrap`);
/// - `BusinessScope::NeverBootstrapped` → the documented legacy global
///   path — reachable only when the installation provably predates
///   tenancy.
///
/// Anything else is impossible to represent: the extractor has already
/// failed closed on unverifiable, non-bootstrap, or broken contexts.
/// Not yet attached to business routers (P1-c3 migrates call sites).
pub async fn require_business(
    state: &AppState,
    ctx: &crate::tenant::extractor::BusinessAuth,
    permission: &str,
) -> Result<(), ApiError> {
    use crate::tenant::extractor::BusinessScope;
    let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;
    // M1-P4-S2A: resolution runs through the credential-bound definer
    // readers — the session token hash authenticates the principal
    // inside the database, and the verified BusinessAuth picks only the
    // resolution lane (bootstrap-scoped vs provably legacy global).
    let permissions = match &ctx.scope {
        BusinessScope::Cooperative { cooperative_id, .. } => {
            session_permissions_scoped(
                pool,
                &ctx.auth.session_token_hash,
                *cooperative_id,
                ctx.auth.authed_at,
            )
            .await
        }
        BusinessScope::NeverBootstrapped => {
            session_permissions_legacy(pool, &ctx.auth.session_token_hash, ctx.auth.authed_at).await
        }
    }
    .map_err(|error| {
        tracing::error!(error = %error, permission, "business permission resolution failed");
        ApiError::Internal
    })?;
    if permissions.contains(permission) {
        Ok(())
    } else {
        tracing::info!(
            outcome = "permission_denied",
            user_id = %ctx.auth.user_id,
            cooperative_id = ?ctx.cooperative_id(),
            permission,
            "business authorization denied"
        );
        Err(ApiError::PermissionDenied)
    }
}

/// M1-P6 B3 — like `require_business` but succeeds when the verified
/// session holds ANY listed permission. Mirrors the database-side
/// `app_rbac_verified_scope_in` semantics: a compatibility lane where a
/// new dedicated permission coexists with a legacy broader one.
pub async fn require_business_any(
    state: &AppState,
    ctx: &crate::tenant::extractor::BusinessAuth,
    permissions: &[&str],
) -> Result<(), ApiError> {
    use crate::tenant::extractor::BusinessScope;
    let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;
    let resolved = match &ctx.scope {
        BusinessScope::Cooperative { cooperative_id, .. } => {
            session_permissions_scoped(
                pool,
                &ctx.auth.session_token_hash,
                *cooperative_id,
                ctx.auth.authed_at,
            )
            .await
        }
        BusinessScope::NeverBootstrapped => {
            session_permissions_legacy(pool, &ctx.auth.session_token_hash, ctx.auth.authed_at).await
        }
    }
    .map_err(|error| {
        tracing::error!(error = %error, "business permission resolution failed");
        ApiError::Internal
    })?;
    if permissions.iter().any(|p| resolved.contains(*p)) {
        Ok(())
    } else {
        tracing::info!(
            outcome = "permission_denied",
            user_id = %ctx.auth.user_id,
            cooperative_id = ?ctx.cooperative_id(),
            permissions = ?permissions,
            "business authorization denied"
        );
        Err(ApiError::PermissionDenied)
    }
}

/// Active role summaries of a user (id + display name), safe for API
/// responses.
pub async fn active_role_summaries(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT r.id, r.name \
         FROM user_role_assignments ura \
         JOIN roles r ON r.id = ura.role_id AND r.status = 'active' \
         WHERE ura.user_id = $1 \
         ORDER BY r.name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

/// Enforce a single permission for the authenticated caller.
/// 403 `permission_denied` — never a 401 (authentication already held).
pub async fn require(
    state: &AppState,
    auth: &CurrentAuth,
    permission: &str,
) -> Result<(), ApiError> {
    let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;
    let permissions = effective_permissions(pool, auth.user_id)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, permission, "permission resolution failed");
            ApiError::Internal
        })?;
    if permissions.contains(permission) {
        Ok(())
    } else {
        tracing::info!(
            outcome = "permission_denied",
            user_id = %auth.user_id,
            permission,
            "authorization denied"
        );
        Err(ApiError::PermissionDenied)
    }
}

/// Count the remaining administration paths: distinct ACTIVE users who
/// hold an ACTIVE role containing ADMINISTRATION_PERMISSION. Used by
/// the lockout guard AFTER applying the candidate mutation inside the
/// transaction; a zero count aborts the mutation.
///
/// `cooperative_scope` selects the counting domain (M1-P1-c2b, P1-K5):
///
/// - `Some(coop)`: only paths EFFECTIVE in that cooperative count —
///   scoped assignment + scoped role (plus NULL-attributed legacy rows,
///   but ONLY when `coop` resolves as the marker-consistent bootstrap
///   cooperative at count time — the restriction is encoded in the
///   query, so a mistakenly supplied non-bootstrap scope can never
///   inherit NULL paths), joined through an `active` membership in the
///   same cooperative. Administrator continuity is therefore evaluated
///   per cooperative: removing A's last path aborts while B's paths are
///   neither counted nor disturbed. Identification is by the
///   `roles.manage` permission key, never by role name (docs/26:
///   "test scope, not only role names").
/// - `None`: the legacy global count — reachable only in the
///   never-bootstrapped lane where every row is necessarily
///   NULL-attributed and global == scoped.
pub async fn administration_path_count(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    cooperative_scope: Option<Uuid>,
) -> Result<i64, sqlx::Error> {
    // M1-P4-R4: the count moved behind the app_admin_path_count
    // definer — it encodes the same marker-consistent bootstrap
    // attribution (NULL rows count only for the verified bootstrap
    // cooperative; NULL scope = the pre-tenancy global count) and the
    // role_permissions/permissions tables are no longer directly
    // readable by the runtime role.
    let count: i64 = sqlx::query_scalar("SELECT public.app_admin_path_count($1)")
        .bind(cooperative_scope)
        .fetch_one(tx.as_mut())
        .await?;
    Ok(count)
}

/// Serialize lockout-sensitive mutations: begin a transaction that
/// holds the administration-path guard advisory lock until
/// commit/rollback.
///
/// `cooperative_scope` selects the lock key (M1-P1-c2b):
/// - `Some(coop)` → `pg_advisory_xact_lock("KOOP" namespace,
///   hashtext(coop))` — mutations serialize per cooperative
///   (ADR-006: scope locking to the affected resource);
/// - `None` → the legacy single global key (never-bootstrapped lane).
///
/// Caller pattern (STEP-003 §44 — database-enforced serialization):
/// ```text
/// let mut tx = begin_guarded_transaction(pool, scope).await?;
/// ...apply mutation via tx...
/// if administration_path_count(&mut tx, scope).await? == 0 {
///     tx.rollback().await?;
///     return Err(ApiError::LockoutPrevented);
/// }
/// tx.commit().await?;
/// ```
pub async fn begin_guarded_transaction(
    conn: &mut sqlx::PgConnection,
    cooperative_scope: Option<Uuid>,
) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, sqlx::Error> {
    // On a plain connection this is a real transaction; on the
    // request-scoped transaction's connection (depth > 0) sqlx emits a
    // savepoint — the advisory xact lock then holds until the OUTER
    // commit, which is the semantics the guard wants (M1-P4-P2B).
    let mut tx = sqlx::Acquire::begin(&mut *conn).await?;
    match cooperative_scope {
        Some(cooperative_id) => {
            sqlx::query("SELECT pg_advisory_xact_lock($1::int4, hashtext($2::text))")
                .bind(ADMIN_PATH_GUARD_LOCK_NAMESPACE)
                .bind(cooperative_id.to_string())
                .execute(tx.as_mut())
                .await?;
        }
        None => {
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(ADMIN_PATH_GUARD_LOCK_KEY)
                .execute(tx.as_mut())
                .await?;
        }
    }
    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_minimal_and_has_no_wildcards() {
        assert_eq!(
            catalog::ALL.len(),
            35,
            "STEP-003..STEP-015 + M1-P5-B4 + M1-P6-B1 + M1-P6-B4 catalog"
        );
        assert!(catalog::ALL.iter().all(|key| {
            !key.contains('*') && !key.contains("superuser") && !key.contains("restore")
        }));
        assert!(catalog::is_known("roles.read"));
        assert!(
            !catalog::is_known("payments.reverse"),
            "future keys are unknown"
        );
        assert!(!catalog::is_known("backup.restore"), "ADR-013 boundary");
        assert!(!catalog::is_known("*.manage"), "no wildcards");
    }
}
