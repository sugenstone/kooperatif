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
/// One fixed key serializes exactly these mutations database-wide.
const ADMIN_PATH_GUARD_LOCK_KEY: i64 = 0x4B_4F_4F_50_30_30_33; // "KOOP003"

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
pub async fn administration_path_count(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<i64, sqlx::Error> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT ura.user_id) \
         FROM user_role_assignments ura \
         JOIN roles r ON r.id = ura.role_id AND r.status = 'active' \
         JOIN role_permissions rp ON rp.role_id = r.id \
         JOIN permissions p ON p.id = rp.permission_id AND p.key = $1 \
         JOIN users u ON u.id = ura.user_id AND u.status = 'active'",
    )
    .bind(ADMINISTRATION_PERMISSION)
    .fetch_one(tx.as_mut())
    .await?;
    Ok(count)
}

/// Serialize lockout-sensitive mutations: begin a transaction that
/// holds the administration-path guard advisory lock until commit/rollback.
///
/// Caller pattern (STEP-003 §44 — database-enforced serialization):
/// ```text
/// let mut tx = begin_guarded_transaction(pool).await?;
/// ...apply mutation via tx...
/// if administration_path_count(&mut tx).await? == 0 {
///     tx.rollback().await?;
///     return Err(ApiError::LockoutPrevented);
/// }
/// tx.commit().await?;
/// ```
pub async fn begin_guarded_transaction(
    pool: &PgPool,
) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(ADMIN_PATH_GUARD_LOCK_KEY)
        .execute(tx.as_mut())
        .await?;
    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_minimal_and_has_no_wildcards() {
        assert_eq!(catalog::ALL.len(), 24, "STEP-003..STEP-011 catalog");
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
