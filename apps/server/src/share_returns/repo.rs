//! Share Return repository (STEP-011): the controlled return workflow
//! (initiate -> finalize | cancel), crystallized entitlements and
//! their cash settlements.
//!
//! THREE layers stay separate (docs/10):
//!   * SHARE LIFECYCLE  — `shares.status` + `share_ownerships` interval
//!   * ENTITLEMENT      — `share_return_entitlements` (no money moves)
//!   * CASH SETTLEMENT  — `share_return_settlements` bound 1:1 to ONE
//!     outflow `account_movements` row each
//!
//! Lock ordering (deterministic, documented):
//!   return commands:  shares -> share_returns -> share_ownerships
//!   settlement:       financial_accounts -> share_return_entitlements
//!                     -> share_return_settlements
//! The shared account lock prefix matches STEP-008/010 so settlement
//! serializes with transfers/income/expense on the same account.
//!
//! Time convention: `effective_return_date` is a business DATE. The
//! ownership interval ends at `effective_return_date 00:00
//! Europe/Istanbul` — the cutoff day is the first non-owned day, so the
//! share still participates in assessments whose effective date is
//! strictly before it (docs/10 cutoff; STEP-006 temporal eligibility).

use rust_decimal::Decimal;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::financial_accounts::model::AccountStatus;
use crate::financial_accounts::repo as account_repo;
use crate::parties::model as party_model;

use super::model::{EntitlementStatus, EntitlementType, ReturnStatus};

// ------------------------------------------------------------------
// Read models
// ------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
pub struct ReturnListRow {
    pub id: Uuid,
    pub return_number: i64,
    pub share_id: Uuid,
    pub share_number: i64,
    pub shareholder_id: Uuid,
    pub owner_display_name: String,
    pub requested_at: OffsetDateTime,
    pub effective_return_date: Date,
    pub status: String,
    pub entitlement_count: i64,
    pub outstanding_amount: Option<Decimal>,
    pub created_at: OffsetDateTime,
    pub total_count: i64,
}

const RETURN_LIST_SELECT: &str = "SELECT r.id, r.return_number, r.share_id, r.share_number, \
    r.shareholder_id, r.owner_display_name, r.requested_at, r.effective_return_date, \
    r.status, \
    (SELECT count(*) FROM share_return_entitlements e \
        WHERE e.share_return_id = r.id AND e.status <> 'cancelled') AS entitlement_count, \
    (SELECT sum(e.amount) - COALESCE((SELECT sum(s.amount) FROM share_return_settlements s \
            JOIN share_return_entitlements e2 ON e2.id = s.entitlement_id \
            WHERE e2.share_return_id = r.id AND s.status = 'posted'), 0) \
        FROM share_return_entitlements e \
        WHERE e.share_return_id = r.id AND e.status <> 'cancelled') AS outstanding_amount, \
    r.created_at, count(*) OVER() AS total_count \
    FROM share_returns r";

/// Bounded return list (ADR-010). `search` covers owner name, share
/// number and return number.
#[allow(clippy::too_many_arguments)]
pub async fn list_returns(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    status: Option<ReturnStatus>,
    share_id: Option<Uuid>,
    shareholder_id: Option<Uuid>,
    search: Option<&str>,
    page: i64,
    page_size: i64,
) -> Result<Vec<ReturnListRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    let pattern = search
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|term| format!("%{}%", party_model::fold_search(term)));
    let number = search
        .map(str::trim)
        .and_then(|term| term.parse::<i64>().ok())
        .filter(|n| *n > 0);
    sqlx::query_as::<_, ReturnListRow>(&format!(
        "{RETURN_LIST_SELECT} \
         WHERE r.cooperative_id IS NOT DISTINCT FROM $8 \
           AND ($1::text IS NULL OR r.status = $1) \
           AND ($2::uuid IS NULL OR r.share_id = $2) \
           AND ($3::uuid IS NULL OR r.shareholder_id = $3) \
           AND ($4::text IS NULL OR lower(r.owner_display_name) LIKE $4) \
           AND ($5::bigint IS NULL OR r.share_number = $5 OR r.return_number = $5) \
         ORDER BY r.return_number DESC LIMIT $6 OFFSET $7"
    ))
    .bind(status.map(|s| s.as_str()))
    .bind(share_id)
    .bind(shareholder_id)
    .bind(pattern)
    .bind(number)
    .bind(page_size)
    .bind((page - 1) * page_size)
    .bind(cooperative_id)
    .fetch_all(&mut *conn)
    .await
}

#[derive(Debug, sqlx::FromRow)]
pub struct ReturnRow {
    pub id: Uuid,
    pub return_number: i64,
    pub share_id: Uuid,
    pub share_number: i64,
    pub share_status: String,
    pub shareholder_id: Uuid,
    pub owner_display_name: String,
    pub ownership_started_at: OffsetDateTime,
    pub requested_at: OffsetDateTime,
    pub effective_return_date: Date,
    pub reason: Option<String>,
    pub status: String,
    pub finalized_at: Option<OffsetDateTime>,
    pub cancelled_at: Option<OffsetDateTime>,
    pub cancellation_reason: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub async fn get_return(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    id: Uuid,
) -> Result<Option<ReturnRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    sqlx::query_as::<_, ReturnRow>(
        "SELECT r.id, r.return_number, r.share_id, r.share_number, sh.status AS share_status, \
            r.shareholder_id, r.owner_display_name, r.ownership_started_at, r.requested_at, \
            r.effective_return_date, r.reason, r.status, r.finalized_at, r.cancelled_at, \
            r.cancellation_reason, r.created_at, r.updated_at \
         FROM share_returns r JOIN shares sh ON sh.id = r.share_id \
         WHERE r.id = $1 AND r.cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(id)
    .bind(cooperative_id)
    .fetch_optional(&mut *conn)
    .await
}

/// Entitlement with derived settled/remaining/due-state. `due_state`
/// is computed against the cooperative-local today (Europe/Istanbul):
/// undetermined | not_due | due | overdue | settled | cancelled.
#[derive(Debug, sqlx::FromRow)]
pub struct EntitlementRow {
    pub id: Uuid,
    pub entitlement_number: i64,
    pub share_return_id: Uuid,
    pub return_number: i64,
    pub share_id: Uuid,
    pub share_number: i64,
    pub entitlement_type: String,
    pub beneficiary_shareholder_id: Uuid,
    pub beneficiary_display_name: String,
    pub amount: Option<Decimal>,
    pub currency: String,
    pub due_date: Option<Date>,
    pub policy_reference: Option<String>,
    pub description: Option<String>,
    pub recognized_at: OffsetDateTime,
    pub determined_at: Option<OffsetDateTime>,
    pub status: String,
    pub settled_amount: Decimal,
    pub due_state: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub total_count: i64,
}

const ENTITLEMENT_SELECT: &str = "SELECT e.id, e.entitlement_number, e.share_return_id, \
    r.return_number, r.share_id, r.share_number, \
    e.entitlement_type, e.beneficiary_shareholder_id, r.owner_display_name AS beneficiary_display_name, \
    e.amount, e.currency, e.due_date, e.policy_reference, e.description, e.recognized_at, \
    e.determined_at, e.status, \
    COALESCE((SELECT sum(s.amount) FROM share_return_settlements s \
        WHERE s.entitlement_id = e.id AND s.status = 'posted'), 0) AS settled_amount, \
    CASE \
        WHEN e.status = 'settled' THEN 'settled' \
        WHEN e.status = 'cancelled' THEN 'cancelled' \
        WHEN e.amount IS NULL OR e.due_date IS NULL THEN 'undetermined' \
        WHEN e.due_date < (now() AT TIME ZONE 'Europe/Istanbul')::date THEN 'overdue' \
        WHEN e.due_date = (now() AT TIME ZONE 'Europe/Istanbul')::date THEN 'due' \
        ELSE 'not_due' \
    END AS due_state, \
    e.created_at, e.updated_at, count(*) OVER() AS total_count \
    FROM share_return_entitlements e JOIN share_returns r ON r.id = e.share_return_id";

/// Entitlements of one return case (detail surface).
pub async fn list_entitlements_of_return(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    share_return_id: Uuid,
) -> Result<Vec<EntitlementRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    sqlx::query_as::<_, EntitlementRow>(&format!(
        "{ENTITLEMENT_SELECT} WHERE e.share_return_id = $1 \
         AND e.cooperative_id IS NOT DISTINCT FROM $2 ORDER BY e.entitlement_number"
    ))
    .bind(share_return_id)
    .bind(cooperative_id)
    .fetch_all(&mut *conn)
    .await
}

/// Bounded entitlement list (shareholder statements / receivable
/// views): filter by type, status, beneficiary, return.
#[allow(clippy::too_many_arguments)]
pub async fn list_entitlements(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    entitlement_type: Option<EntitlementType>,
    status: Option<EntitlementStatus>,
    beneficiary_shareholder_id: Option<Uuid>,
    share_return_id: Option<Uuid>,
    page: i64,
    page_size: i64,
) -> Result<Vec<EntitlementRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    sqlx::query_as::<_, EntitlementRow>(&format!(
        "{ENTITLEMENT_SELECT} \
         WHERE e.cooperative_id IS NOT DISTINCT FROM $7 \
           AND ($1::text IS NULL OR e.entitlement_type = $1) \
           AND ($2::text IS NULL OR e.status = $2) \
           AND ($3::uuid IS NULL OR e.beneficiary_shareholder_id = $3) \
           AND ($4::uuid IS NULL OR e.share_return_id = $4) \
         ORDER BY e.due_date NULLS LAST, e.entitlement_number LIMIT $5 OFFSET $6"
    ))
    .bind(entitlement_type.map(|t| t.as_str()))
    .bind(status.map(|s| s.as_str()))
    .bind(beneficiary_shareholder_id)
    .bind(share_return_id)
    .bind(page_size)
    .bind((page - 1) * page_size)
    .bind(cooperative_id)
    .fetch_all(&mut *conn)
    .await
}

pub async fn get_entitlement(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    id: Uuid,
) -> Result<Option<EntitlementRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    sqlx::query_as::<_, EntitlementRow>(&format!(
        "{ENTITLEMENT_SELECT} WHERE e.id = $1 \
         AND e.cooperative_id IS NOT DISTINCT FROM $2"
    ))
    .bind(id)
    .bind(cooperative_id)
    .fetch_optional(&mut *conn)
    .await
}

/// One settlement with resolved labels and the bound movement's live
/// status (movement is authoritative; the join is display-only).
#[derive(Debug, sqlx::FromRow)]
pub struct SettlementRow {
    pub id: Uuid,
    pub settlement_number: i64,
    pub entitlement_id: Uuid,
    pub entitlement_type: String,
    pub share_return_id: Uuid,
    pub return_number: i64,
    pub financial_account_id: Uuid,
    pub account_name: String,
    pub amount: Decimal,
    pub currency: String,
    pub settled_at: OffsetDateTime,
    pub account_movement_id: Uuid,
    pub movement_status: String,
    pub status: String,
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
    pub created_at: OffsetDateTime,
}

const SETTLEMENT_SELECT: &str = "SELECT s.id, s.settlement_number, s.entitlement_id, \
    e.entitlement_type, e.share_return_id, r.return_number, \
    s.financial_account_id, a.name AS account_name, \
    s.amount, s.currency, s.settled_at, s.account_movement_id, m.status AS movement_status, \
    s.status, s.reversed_at, s.reversal_reason, s.created_at \
    FROM share_return_settlements s \
    JOIN share_return_entitlements e ON e.id = s.entitlement_id \
    JOIN share_returns r ON r.id = e.share_return_id \
    JOIN financial_accounts a ON a.id = s.financial_account_id \
    JOIN account_movements m ON m.id = s.account_movement_id";

pub async fn list_settlements_of_return(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    share_return_id: Uuid,
) -> Result<Vec<SettlementRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    sqlx::query_as::<_, SettlementRow>(&format!(
        "{SETTLEMENT_SELECT} WHERE e.share_return_id = $1 \
         AND s.cooperative_id IS NOT DISTINCT FROM $2 ORDER BY s.settlement_number"
    ))
    .bind(share_return_id)
    .bind(cooperative_id)
    .fetch_all(&mut *conn)
    .await
}

pub async fn get_settlement(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    id: Uuid,
) -> Result<Option<SettlementRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    sqlx::query_as::<_, SettlementRow>(&format!(
        "{SETTLEMENT_SELECT} WHERE s.id = $1 \
         AND s.cooperative_id IS NOT DISTINCT FROM $2"
    ))
    .bind(id)
    .bind(cooperative_id)
    .fetch_optional(&mut *conn)
    .await
}

// ------------------------------------------------------------------
// Commands
// ------------------------------------------------------------------

#[derive(Debug)]
pub enum ShareReturnCommandError {
    NotFound,
    /// The command targets a Share that cannot enter the return flow
    /// (not `active`, or no live ownership interval).
    ShareNotEligible,
    /// A pending return case already exists for this Share.
    ActiveReturnExists,
    /// effective_return_date is in the future or before the current
    /// ownership interval began — both would corrupt temporal history.
    InvalidEffectiveDate,
    /// The return case is not in the required lifecycle state for this
    /// command (e.g. finalize a finalized/cancelled case).
    InvalidState,
    /// Optimistic-concurrency precondition failed (updated_at moved).
    StaleState,
    /// An entitlement of this type already exists for the return.
    EntitlementExists,
    /// The entitlement cannot be settled: undetermined amount, or not
    /// in a settleable state.
    NotSettleable,
    /// Settlement would exceed the crystallized entitlement amount.
    OverSettlement,
    /// The entitlement has determined data/posts — only open
    /// entitlements with zero posted settlements may be cancelled.
    HasSettlements,
    /// The settlement account is not `active` or cannot fund it.
    InactiveAccount,
    InsufficientFunds,
    /// Settlement would consume cash reserved for Social Aid funds
    /// (PILOT-FIX-001 hard reservation): within the physical balance
    /// but beyond its unrestricted portion.
    InsufficientUnrestrictedFunds,
    IdempotencyConflict,
    AccountInvariant(account_repo::AccountCommandError),
    Database(sqlx::Error),
}

fn account_to_return_error(error: account_repo::AccountCommandError) -> ShareReturnCommandError {
    match error {
        account_repo::AccountCommandError::NotFound => ShareReturnCommandError::NotFound,
        account_repo::AccountCommandError::InactiveAccount => {
            ShareReturnCommandError::InactiveAccount
        }
        account_repo::AccountCommandError::InsufficientFunds => {
            ShareReturnCommandError::InsufficientFunds
        }
        account_repo::AccountCommandError::InsufficientUnrestrictedFunds => {
            ShareReturnCommandError::InsufficientUnrestrictedFunds
        }
        account_repo::AccountCommandError::IdempotencyConflict => {
            ShareReturnCommandError::IdempotencyConflict
        }
        account_repo::AccountCommandError::StaleState => ShareReturnCommandError::StaleState,
        account_repo::AccountCommandError::Database(error) => {
            ShareReturnCommandError::Database(error)
        }
        other => ShareReturnCommandError::AccountInvariant(other),
    }
}

pub struct InitiateReturn {
    pub share_id: Uuid,
    pub effective_return_date: Date,
    pub reason: Option<String>,
    pub idempotency_key: String,
    pub fingerprint: String,
}

pub struct ReturnOutcome {
    pub return_id: Uuid,
    pub return_number: i64,
    pub replayed: bool,
}

/// Istanbul day-boundary helper: `date 00:00 Europe/Istanbul` as a
/// timestamptz expression over the bound DATE parameter `$1`.
const ISTANBUL_DAY_START: &str = "($1::timestamp AT TIME ZONE 'Europe/Istanbul')";

/// INITIATE: active Share -> return_pending + the durable return case.
/// Ownership is NOT touched — the owner still owns the Share until
/// finalization fixes the economic cutoff.
pub async fn initiate_return(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    command: InitiateReturn,
    now: OffsetDateTime,
) -> Result<ReturnOutcome, ShareReturnCommandError> {
    let mut tx = db
        .begin()
        .await
        .map_err(ShareReturnCommandError::Database)?;

    // 1. Idempotent replay short-circuit (scoped: a foreign row under
    //    the global key is indistinguishable — C3/C5).
    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, return_number, idempotency_fingerprint FROM share_returns \
         WHERE idempotency_key = $1 AND cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(&command.idempotency_key)
    .bind(cooperative_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(ShareReturnCommandError::Database)?;
    if let Some((id, number, fingerprint)) = existing {
        if fingerprint != command.fingerprint {
            return Err(ShareReturnCommandError::IdempotencyConflict);
        }
        tx.rollback()
            .await
            .map_err(ShareReturnCommandError::Database)?;
        return Ok(ReturnOutcome {
            return_id: id,
            return_number: number,
            replayed: true,
        });
    }

    // 2. Lock the Share: only an `active` Share IN SCOPE may enter the
    //    flow — a foreign id is indistinguishable from absence.
    let share: Option<(String, i64)> = sqlx::query_as(
        "SELECT status, share_number FROM shares WHERE id = $1 \
         AND cooperative_id IS NOT DISTINCT FROM $2 FOR UPDATE",
    )
    .bind(command.share_id)
    .bind(cooperative_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(ShareReturnCommandError::Database)?;
    let Some((share_status, share_number)) = share else {
        return Err(ShareReturnCommandError::NotFound);
    };
    if share_status != "active" {
        return Err(ShareReturnCommandError::ShareNotEligible);
    }

    // 3. Lock the live ownership interval — the snapshot source.
    let ownership: Option<(Uuid, Uuid, OffsetDateTime)> = sqlx::query_as(
        "SELECT id, shareholder_id, started_at \
         FROM public.app_lock_share_ownership($1, $2)",
    )
    .bind(command.share_id)
    .bind(cooperative_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(ShareReturnCommandError::Database)?;
    let Some((_ownership_id, owner_id, ownership_started_at)) = ownership else {
        return Err(ShareReturnCommandError::ShareNotEligible);
    };

    // 4. Cutoff validation: the ownership interval ends at
    //    effective_return_date 00:00 Istanbul — that instant must not
    //    precede the interval start and the date must not be a future
    //    cooperative-local day. "Today" is the APP clock (frozen in
    //    tests), never the database wall clock.
    let istanbul_today = now
        .to_offset(time::UtcOffset::from_hms(3, 0, 0).expect("UTC+3"))
        .date();
    if command.effective_return_date > istanbul_today {
        return Err(ShareReturnCommandError::InvalidEffectiveDate);
    }
    let not_before_start: bool =
        sqlx::query_scalar(&format!("SELECT {ISTANBUL_DAY_START} >= $2::timestamptz"))
            .bind(command.effective_return_date)
            .bind(ownership_started_at)
            .fetch_one(tx.as_mut())
            .await
            .map_err(ShareReturnCommandError::Database)?;
    if !not_before_start {
        return Err(ShareReturnCommandError::InvalidEffectiveDate);
    }

    // 5. Owner identity snapshot — the canonical display label.
    let owner_display_name: String = sqlx::query_scalar(
        "SELECT concat_ws(' · ', p.first_name || ' ' || p.last_name, \
            'Vasi: ' || COALESCE(gp.first_name || ' ' || gp.last_name, 'Belirtilmemiş'), \
            CASE WHEN f.sequence_number IS NOT NULL THEN 'Aile No ' || f.sequence_number END) \
         FROM shareholders s \
         JOIN persons p ON p.id = s.person_id \
         LEFT JOIN persons gp ON gp.id = s.guardian_person_id \
         LEFT JOIN shareholder_family_memberships m \
             ON m.shareholder_id = s.id AND m.ended_at IS NULL \
         LEFT JOIN families f ON f.id = m.family_id \
         WHERE s.id = $1 AND s.cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(owner_id)
    .bind(cooperative_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(ShareReturnCommandError::Database)?;

    // 6. Business event + case + lifecycle — one transaction, all
    //    carrying the verified cooperative scope (C3).
    sqlx::query(
        "INSERT INTO share_events \
             (share_id, event_type, occurred_at, from_shareholder_id, \
              status_from, status_to, reason, actor_user_id, cooperative_id) \
         VALUES ($1, 'return_requested', $2, $3, 'active', 'return_pending', $4, $5, $6)",
    )
    .bind(command.share_id)
    .bind(now)
    .bind(owner_id)
    .bind(&command.reason)
    .bind(actor)
    .bind(cooperative_id)
    .execute(tx.as_mut())
    .await
    .map_err(ShareReturnCommandError::Database)?;

    let inserted = sqlx::query_as::<_, (Uuid, i64)>(
        "INSERT INTO share_returns \
            (share_id, shareholder_id, owner_display_name, share_number, \
             ownership_started_at, requested_at, effective_return_date, reason, \
             status, idempotency_key, idempotency_fingerprint, created_by, cooperative_id, \
             return_number) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'pending', $9, $10, $11, $12, \
             allocate_document_number($12, 'share_returns.return_number')) \
         ON CONFLICT (cooperative_id, idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING \
         RETURNING id, return_number",
    )
    .bind(command.share_id)
    .bind(owner_id)
    .bind(&owner_display_name)
    .bind(share_number)
    .bind(ownership_started_at)
    .bind(now)
    .bind(command.effective_return_date)
    .bind(&command.reason)
    .bind(&command.idempotency_key)
    .bind(&command.fingerprint)
    .bind(actor)
    .bind(cooperative_id)
    .fetch_optional(tx.as_mut())
    .await;

    let inserted = match inserted {
        Ok(row) => row,
        Err(ref error) if crate::db::is_idempotency_unique_violation(error) => None,
        Err(sqlx::Error::Database(db))
            if db
                .constraint()
                .is_some_and(|c| c.contains("one_pending_per_share")) =>
        {
            return Err(ShareReturnCommandError::ActiveReturnExists);
        }
        Err(error) => return Err(ShareReturnCommandError::Database(error)),
    };
    let Some((return_id, return_number)) = inserted else {
        // Concurrent replay under the same idempotency key.
        let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
            "SELECT id, return_number, idempotency_fingerprint FROM share_returns \
             WHERE idempotency_key = $1 AND cooperative_id IS NOT DISTINCT FROM $2",
        )
        .bind(&command.idempotency_key)
        .bind(cooperative_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(ShareReturnCommandError::Database)?;
        tx.rollback()
            .await
            .map_err(ShareReturnCommandError::Database)?;
        let Some((id, number, fingerprint)) = existing else {
            return Err(ShareReturnCommandError::NotFound);
        };
        if fingerprint != command.fingerprint {
            return Err(ShareReturnCommandError::IdempotencyConflict);
        }
        return Ok(ReturnOutcome {
            return_id: id,
            return_number: number,
            replayed: true,
        });
    };

    // R3C: the mark definer refuses a naked flip — the pending case
    // and its 'return_requested' event written above must exist.
    sqlx::query("SELECT public.app_share_return_mark_pending_tx($1, $2, $3)")
        .bind(command.share_id)
        .bind(now)
        .bind(cooperative_id)
        .execute(tx.as_mut())
        .await
        .map_err(ShareReturnCommandError::Database)?;

    tx.commit()
        .await
        .map_err(ShareReturnCommandError::Database)?;
    Ok(ReturnOutcome {
        return_id,
        return_number,
        replayed: false,
    })
}

/// CANCEL a pending return: Share goes back to `active`. Ownership was
/// never closed so nothing is "restored" — the interval simply keeps
/// running. History keeps the request + the cancellation (docs/19).
pub async fn cancel_return(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    return_id: Uuid,
    reason: String,
    now: OffsetDateTime,
) -> Result<(bool, i64), ShareReturnCommandError> {
    let mut tx = db
        .begin()
        .await
        .map_err(ShareReturnCommandError::Database)?;

    // R3C: one atomic definer — share -> case lock order, pending
    // checks, the 'return_cancelled' event and both flips are
    // indivisible.
    let return_number: i64 =
        sqlx::query_scalar("SELECT public.app_share_return_cancel_tx($1, $2, $3, $4, $5)")
            .bind(return_id)
            .bind(now)
            .bind(actor)
            .bind(&reason)
            .bind(cooperative_id)
            .fetch_one(tx.as_mut())
            .await
            .map_err(|error| match crate::db::package_error_token(&error) {
                Some("not_found") => ShareReturnCommandError::NotFound,
                Some("invalid_state") => ShareReturnCommandError::InvalidState,
                _ => ShareReturnCommandError::Database(error),
            })?;

    tx.commit()
        .await
        .map_err(ShareReturnCommandError::Database)?;
    Ok((false, return_number))
}

/// One right to crystallize at finalization. `amount` may be NULL —
/// the right exists, quantification pending.
pub struct EntitlementSpec {
    pub entitlement_type: EntitlementType,
    pub amount: Option<Decimal>,
    pub due_date: Option<Date>,
    pub policy_reference: Option<String>,
    pub description: Option<String>,
}

pub struct FinalizeOutcome {
    pub return_number: i64,
    pub entitlement_ids: Vec<Uuid>,
}

/// FINALIZE: pending -> finalized. Closes the ownership interval at the
/// effective cutoff, closes the Share, and crystallizes every supplied
/// entitlement — all in ONE transaction. ZERO money moves.
pub async fn finalize_return(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    return_id: Uuid,
    expected_updated_at: OffsetDateTime,
    entitlements: &[EntitlementSpec],
    now: OffsetDateTime,
) -> Result<FinalizeOutcome, ShareReturnCommandError> {
    let mut tx = db
        .begin()
        .await
        .map_err(ShareReturnCommandError::Database)?;

    // R3C: the complete finalization package — share -> return lock
    // order, optimistic-concurrency + pending checks, duplicate-type
    // rejection, ownership-interval close at the economic cutoff, the
    // share event, both status flips and every entitlement
    // crystallization — runs as ONE permission-checked definer call.
    let specs = serde_json::Value::Array(
        entitlements
            .iter()
            .map(|spec| {
                serde_json::json!({
                    "type": spec.entitlement_type.as_str(),
                    "amount": spec.amount.map(|a| a.to_string()),
                    "due_date": spec.due_date.map(|d| {
                        d.format(&time::macros::format_description!(
                            "[year]-[month]-[day]"
                        ))
                        .unwrap_or_default()
                    }),
                    "policy_reference": spec.policy_reference,
                    "description": spec.description,
                })
            })
            .collect(),
    );
    let (return_number, entitlement_ids): (i64, Vec<Uuid>) = sqlx::query_as(
        "SELECT return_number, entitlement_ids \
         FROM public.app_share_return_finalize_tx($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(return_id)
    .bind(expected_updated_at)
    .bind(
        sqlx::query_scalar::<_, Date>(
            "SELECT effective_return_date FROM share_returns WHERE id = $1 \
         AND cooperative_id IS NOT DISTINCT FROM $2",
        )
        .bind(return_id)
        .bind(cooperative_id)
        .fetch_optional(tx.as_mut())
        .await
        .map_err(ShareReturnCommandError::Database)?
        .ok_or(ShareReturnCommandError::NotFound)?,
    )
    .bind(&specs)
    .bind(now)
    .bind(actor)
    .bind(cooperative_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(|error| match crate::db::package_error_token(&error) {
        Some("not_found") => ShareReturnCommandError::NotFound,
        Some("stale_state") => ShareReturnCommandError::StaleState,
        Some("invalid_state") => ShareReturnCommandError::InvalidState,
        Some("entitlement_exists") => ShareReturnCommandError::EntitlementExists,
        Some("share_not_eligible") => ShareReturnCommandError::ShareNotEligible,
        _ => match &error {
            sqlx::Error::Database(db)
                if db.constraint().is_some_and(|c| c.contains("one_per_right")) =>
            {
                ShareReturnCommandError::EntitlementExists
            }
            _ => ShareReturnCommandError::Database(error),
        },
    })?;

    tx.commit()
        .await
        .map_err(ShareReturnCommandError::Database)?;
    Ok(FinalizeOutcome {
        return_number,
        entitlement_ids,
    })
}

async fn insert_entitlement(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    return_id: Uuid,
    beneficiary_id: Uuid,
    spec: &EntitlementSpec,
    now: OffsetDateTime,
) -> Result<Uuid, ShareReturnCommandError> {
    let result = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO share_return_entitlements \
            (share_return_id, entitlement_type, beneficiary_shareholder_id, \
             amount, due_date, policy_reference, description, recognized_at, \
             determined_at, status, created_by, cooperative_id, entitlement_number) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, \
                 CASE WHEN $4::numeric IS NULL THEN NULL ELSE $8 END, 'open', $9, $10, \
             allocate_document_number($10, 'share_return_entitlements.entitlement_number')) \
         RETURNING id",
    )
    .bind(return_id)
    .bind(spec.entitlement_type.as_str())
    .bind(beneficiary_id)
    .bind(spec.amount)
    .bind(spec.due_date)
    .bind(&spec.policy_reference)
    .bind(&spec.description)
    .bind(now)
    .bind(actor)
    .bind(cooperative_id)
    .fetch_one(tx.as_mut())
    .await;
    match result {
        Ok(id) => Ok(id),
        Err(sqlx::Error::Database(db))
            if db.constraint().is_some_and(|c| c.contains("one_per_right")) =>
        {
            Err(ShareReturnCommandError::EntitlementExists)
        }
        Err(error) => Err(ShareReturnCommandError::Database(error)),
    }
}

/// Recognize a right AFTER finalization (e.g. the Profit Right is
/// quantified years later). Only on finalized cases; one live
/// entitlement per type per case (partial UNIQUE).
pub async fn recognize_entitlement(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    return_id: Uuid,
    spec: EntitlementSpec,
    now: OffsetDateTime,
) -> Result<Uuid, ShareReturnCommandError> {
    let mut tx = db
        .begin()
        .await
        .map_err(ShareReturnCommandError::Database)?;

    let case: Option<(String, Uuid)> = sqlx::query_as(
        "SELECT status, shareholder_id \
         FROM public.app_lock_share_return($1, $2)",
    )
    .bind(return_id)
    .bind(cooperative_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(ShareReturnCommandError::Database)?;
    let Some((status, beneficiary_id)) = case else {
        return Err(ShareReturnCommandError::NotFound);
    };
    if status != ReturnStatus::Finalized.as_str() {
        return Err(ShareReturnCommandError::InvalidState);
    }

    let id = insert_entitlement(
        &mut tx,
        cooperative_id,
        actor,
        return_id,
        beneficiary_id,
        &spec,
        now,
    )
    .await?;
    tx.commit()
        .await
        .map_err(ShareReturnCommandError::Database)?;
    Ok(id)
}

/// Determine an undetermined right: set its amount (and optionally a
/// due date / policy evidence) once the cooperative fixes it. Only
/// `amount IS NULL` entitlements can be determined — a crystallized
/// amount is never overwritten (base right immutability, docs/10).
#[allow(clippy::too_many_arguments)]
pub async fn determine_entitlement(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    _actor: Uuid,
    entitlement_id: Uuid,
    amount: Decimal,
    due_date: Option<Date>,
    policy_reference: Option<String>,
    description: Option<String>,
    expected_updated_at: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<i64, ShareReturnCommandError> {
    let mut tx = db
        .begin()
        .await
        .map_err(ShareReturnCommandError::Database)?;
    let row: Option<(Option<Decimal>, i64, String, OffsetDateTime)> = sqlx::query_as(
        "SELECT amount, entitlement_number, status, updated_at \
         FROM public.app_lock_share_return_entitlement($1, $2)",
    )
    .bind(entitlement_id)
    .bind(cooperative_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(ShareReturnCommandError::Database)?;
    let Some((existing_amount, number, status, updated_at)) = row else {
        return Err(ShareReturnCommandError::NotFound);
    };
    if updated_at != expected_updated_at {
        return Err(ShareReturnCommandError::StaleState);
    }
    if status != EntitlementStatus::Open.as_str() || existing_amount.is_some() {
        return Err(ShareReturnCommandError::InvalidState);
    }
    sqlx::query("SELECT public.app_share_return_entitlement_determine($1, $2, $3, $4, $5, $6, $7)")
        .bind(entitlement_id)
        .bind(amount)
        .bind(due_date)
        .bind(&policy_reference)
        .bind(&description)
        .bind(now)
        .bind(cooperative_id)
        .execute(tx.as_mut())
        .await
        .map_err(ShareReturnCommandError::Database)?;
    tx.commit()
        .await
        .map_err(ShareReturnCommandError::Database)?;
    Ok(number)
}

/// Cancel an open entitlement (wrong recognition). Requires ZERO posted
/// settlements — a settled right is corrected by reversing settlements,
/// never by erasing the obligation (docs/19).
pub async fn cancel_entitlement(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    entitlement_id: Uuid,
    reason: String,
    now: OffsetDateTime,
) -> Result<i64, ShareReturnCommandError> {
    let mut tx = db
        .begin()
        .await
        .map_err(ShareReturnCommandError::Database)?;

    // R3C: one atomic definer — entitlement lock, open-status and
    // zero-posted-settlements guards and the flip are indivisible.
    let number: i64 = sqlx::query_scalar(
        "SELECT public.app_share_return_entitlement_cancel_tx($1, $2, $3, $4, $5)",
    )
    .bind(entitlement_id)
    .bind(now)
    .bind(actor)
    .bind(&reason)
    .bind(cooperative_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(|error| match crate::db::package_error_token(&error) {
        Some("not_found") => ShareReturnCommandError::NotFound,
        Some("invalid_state") => ShareReturnCommandError::InvalidState,
        Some("has_settlements") => ShareReturnCommandError::HasSettlements,
        _ => ShareReturnCommandError::Database(error),
    })?;
    tx.commit()
        .await
        .map_err(ShareReturnCommandError::Database)?;
    Ok(number)
}

pub struct PostSettlement {
    pub entitlement_id: Uuid,
    pub financial_account_id: Uuid,
    pub amount: Decimal,
    pub settled_at: OffsetDateTime,
    pub idempotency_key: String,
    pub fingerprint: String,
}

pub struct SettlementOutcome {
    pub settlement_id: Uuid,
    pub settlement_number: i64,
    pub replayed: bool,
}

/// THE settlement command: settlement + exactly ONE outflow movement,
/// atomically. Zero money moves on entitlement recognition — money
/// moves only here. The account must fund the outflow under its row
/// lock; the entitlement must have a determined amount with remaining.
pub async fn post_settlement(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    command: PostSettlement,
) -> Result<SettlementOutcome, ShareReturnCommandError> {
    let mut tx = db
        .begin()
        .await
        .map_err(ShareReturnCommandError::Database)?;

    // 1. Idempotent replay short-circuit (scoped: a foreign row under
    //    the global key is indistinguishable — C3/C5).
    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, settlement_number, idempotency_fingerprint \
         FROM share_return_settlements WHERE idempotency_key = $1 \
           AND cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(&command.idempotency_key)
    .bind(cooperative_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(ShareReturnCommandError::Database)?;
    if let Some((id, number, fingerprint)) = existing {
        if fingerprint != command.fingerprint {
            return Err(ShareReturnCommandError::IdempotencyConflict);
        }
        tx.rollback()
            .await
            .map_err(ShareReturnCommandError::Database)?;
        return Ok(SettlementOutcome {
            settlement_id: id,
            settlement_number: number,
            replayed: true,
        });
    }

    // 2. Lock the account row (shared deterministic lock prefix).
    let locked = account_repo::lock_accounts(&mut tx, &[command.financial_account_id])
        .await
        .map_err(account_to_return_error)?;
    let Some((account_status, _currency, account_coop)) = locked.get(&command.financial_account_id)
    else {
        return Err(ShareReturnCommandError::NotFound);
    };
    // C3: the settlement account must belong to the authenticated
    // scope — a foreign account id is indistinguishable from absence.
    if *account_coop != cooperative_id {
        return Err(ShareReturnCommandError::NotFound);
    }
    if *account_status != AccountStatus::Active.as_str() {
        return Err(ShareReturnCommandError::InactiveAccount);
    }

    // 3. Lock the entitlement: must be settleable — determined amount
    //    and open/partially_settled; its return must be finalized.
    let entitlement: Option<(String, Option<Decimal>, i64)> = sqlx::query_as(
        "SELECT status, amount, entitlement_number \
         FROM public.app_lock_finalized_entitlement($1, $2)",
    )
    .bind(command.entitlement_id)
    .bind(cooperative_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(ShareReturnCommandError::Database)?;
    let Some((entitlement_status, entitlement_amount, _entitlement_number)) = entitlement else {
        return Err(ShareReturnCommandError::NotFound);
    };
    if !matches!(
        EntitlementStatus::parse(&entitlement_status),
        Some(EntitlementStatus::Open | EntitlementStatus::PartiallySettled)
    ) {
        return Err(ShareReturnCommandError::NotSettleable);
    }
    let Some(entitlement_amount) = entitlement_amount else {
        return Err(ShareReturnCommandError::NotSettleable);
    };

    // 4. Over-settlement guard under the entitlement lock: posted
    //    settlements + this one must not exceed the crystallized right.
    let settled: Decimal = sqlx::query_scalar(
        "SELECT COALESCE(sum(amount), 0) FROM share_return_settlements \
         WHERE entitlement_id = $1 AND status = 'posted' \
           AND cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(command.entitlement_id)
    .bind(cooperative_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(ShareReturnCommandError::Database)?;
    if settled + command.amount > entitlement_amount {
        return Err(ShareReturnCommandError::OverSettlement);
    }

    // 5. Funds check under the account lock (no negative balances).
    //    PILOT-FIX-001 hard reservation: a settlement is an ordinary
    //    outflow — it may only consume the UNRESTRICTED portion of
    //    the physical balance, never Social Aid reservations.
    let balance =
        account_repo::locked_balance(&mut tx, *account_coop, command.financial_account_id)
            .await
            .map_err(account_to_return_error)?;
    if command.amount > balance {
        return Err(ShareReturnCommandError::InsufficientFunds);
    }
    let reserved = account_repo::locked_reserved_social_aid(
        &mut tx,
        cooperative_id,
        command.financial_account_id,
    )
    .await
    .map_err(account_to_return_error)?;
    if command.amount > balance - reserved {
        return Err(ShareReturnCommandError::InsufficientUnrestrictedFunds);
    }

    // 6. The authoritative movement first — settlement id pre-generated
    //    so provenance is established in one pass.
    let settlement_id = Uuid::new_v4();
    let movement_id = account_repo::insert_movement(
        &mut tx,
        account_repo::NewMovement {
            account_id: command.financial_account_id,
            direction: crate::financial_accounts::model::MovementDirection::Outflow,
            amount: command.amount,
            source: crate::financial_accounts::model::MovementSource::ShareReturnSettlement,
            source_id: settlement_id,
            occurred_at: command.settled_at,
            actor,
            // C3: the movement lives in its account's cooperative.
            cooperative_id: *account_coop,
        },
    )
    .await
    .map_err(account_to_return_error)?;

    // 7. The settlement binds to its movement 1:1 — same transaction.
    let inserted: Option<(Uuid, i64)> = sqlx::query_as(
        "INSERT INTO share_return_settlements \
            (id, entitlement_id, financial_account_id, amount, currency, settled_at, \
             account_movement_id, status, idempotency_key, idempotency_fingerprint, created_by, \
             cooperative_id, settlement_number) \
         VALUES ($1, $2, $3, $4, 'TRY', $5, $6, 'posted', $7, $8, $9, $10, \
             allocate_document_number($10, 'share_return_settlements.settlement_number')) \
         ON CONFLICT (cooperative_id, idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING \
         RETURNING id, settlement_number",
    )
    .bind(settlement_id)
    .bind(command.entitlement_id)
    .bind(command.financial_account_id)
    .bind(command.amount)
    .bind(command.settled_at)
    .bind(movement_id)
    .bind(&command.idempotency_key)
    .bind(&command.fingerprint)
    .bind(actor)
    .bind(cooperative_id)
    .fetch_optional(tx.as_mut())
    .await
    .or_else(|error| {
        if crate::db::is_idempotency_unique_violation(&error) {
            Ok(None)
        } else {
            Err(error)
        }
    })
    .map_err(ShareReturnCommandError::Database)?;

    let Some((id, number)) = inserted else {
        let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
            "SELECT id, settlement_number, idempotency_fingerprint \
             FROM share_return_settlements WHERE idempotency_key = $1 \
               AND cooperative_id IS NOT DISTINCT FROM $2",
        )
        .bind(&command.idempotency_key)
        .bind(cooperative_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(ShareReturnCommandError::Database)?;
        tx.rollback()
            .await
            .map_err(ShareReturnCommandError::Database)?;
        let Some((id, number, fingerprint)) = existing else {
            return Err(ShareReturnCommandError::NotFound);
        };
        if fingerprint != command.fingerprint {
            return Err(ShareReturnCommandError::IdempotencyConflict);
        }
        return Ok(SettlementOutcome {
            settlement_id: id,
            settlement_number: number,
            replayed: true,
        });
    };

    refresh_entitlement_status(&mut tx, command.entitlement_id, cooperative_id).await?;

    tx.commit()
        .await
        .map_err(ShareReturnCommandError::Database)?;
    Ok(SettlementOutcome {
        settlement_id: id,
        settlement_number: number,
        replayed: false,
    })
}

/// Recompute `open | partially_settled | settled` from posted
/// settlements — the lifecycle is DERIVED, never a hand-edited field.
/// Cancelled rows are terminal and never recomputed.
async fn refresh_entitlement_status(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    entitlement_id: Uuid,
    cooperative_id: Option<Uuid>,
) -> Result<(), ShareReturnCommandError> {
    sqlx::query("SELECT public.app_share_return_entitlement_refresh($1, $2)")
        .bind(entitlement_id)
        .bind(cooperative_id)
        .execute(tx.as_mut())
        .await
        .map_err(ShareReturnCommandError::Database)?;
    Ok(())
}

/// Settlement reversal: status-based (docs/19). Locks account ->
/// entitlement -> settlement, flips the movement to `reversed`, marks
/// the settlement, and recomputes the entitlement. Replays are safe;
/// the original row always survives.
pub async fn reverse_settlement(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    settlement_id: Uuid,
    reason: String,
    now: OffsetDateTime,
) -> Result<(bool, i64), ShareReturnCommandError> {
    let mut tx = db
        .begin()
        .await
        .map_err(ShareReturnCommandError::Database)?;

    // R3C: one atomic definer — canonical account -> entitlement ->
    // settlement lock order, movement reversal, settlement flip and
    // the derived entitlement recompute are indivisible.
    let (replayed, settlement_number): (bool, i64) = sqlx::query_as(
        "SELECT replayed, settlement_number \
         FROM public.app_share_return_settlement_reverse_tx($1, $2, $3, $4, $5)",
    )
    .bind(settlement_id)
    .bind(now)
    .bind(actor)
    .bind(&reason)
    .bind(cooperative_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(|error| match crate::db::package_error_token(&error) {
        Some("not_found") => ShareReturnCommandError::NotFound,
        _ => ShareReturnCommandError::Database(error),
    })?;

    tx.commit()
        .await
        .map_err(ShareReturnCommandError::Database)?;
    Ok((replayed, settlement_number))
}
