//! Social Aid repository (STEP-013, docs/11).
//!
//! Transaction discipline (same shape as STEP-008/010/011/012):
//!   BEGIN -> idempotent replay check -> lock the Fund row
//!   FOR UPDATE -> lock Financial Account row(s) `id ASC` ->
//!   validate -> insert the authoritative movement FIRST -> insert
//!   the bound domain row -> COMMIT.
//!
//! Canonical lock prefix: the FUND row first, then account rows in
//! ascending id order (`account_repo::lock_accounts`). The fund row
//! lock serializes every command that changes the fund's restricted
//! availability — concurrent disbursements cannot overspend it.
//!
//! Restricted availability is DERIVED per (fund, financial_account):
//!   sum(posted donations) - sum(posted disbursements)
//! and is re-checked under the locks in every money command. This is
//! the conservative reading of docs/11+17: money restricted to Fund F
//! deposited in Account A can only fund aid paid FROM Account A.
//!
//! Boundaries: no Social Aid command ever writes `payments`,
//! `payment_allocations`, `shareholder_credits`, `credit_applications`,
//! `account_transfers`, `incomes`, `expenses`, `share_returns`,
//! entitlements or investment rows — Social Aid finance never merges
//! into cooperative finance (docs/11 core rule).

use rust_decimal::Decimal;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::financial_accounts::model::{AccountStatus, MovementDirection, MovementSource};
use crate::financial_accounts::repo as account_repo;
use crate::social_aid::model;

#[derive(Debug)]
pub enum SocialAidError {
    NotFound,
    /// Command rejected: the Fund is not `active` (or the target is
    /// already reversed where a status transition was expected).
    InvalidState,
    /// Fund close/cancel rejected: the fund still carries restricted
    /// availability or financial events.
    HasRestrictedBalance,
    /// Posting/reversal rejected: the account is not `active`.
    InactiveAccount,
    /// Effect would take the physical account balance below zero.
    InsufficientFunds,
    /// Effect would take the fund's restricted availability in the
    /// chosen account below zero.
    InsufficientRestricted,
    /// Idempotency key reused with a different payload.
    IdempotencyConflict,
    Database(sqlx::Error),
    AccountInvariant(account_repo::AccountCommandError),
}

fn account_to_error(error: account_repo::AccountCommandError) -> SocialAidError {
    use account_repo::AccountCommandError as A;
    match error {
        A::NotFound => SocialAidError::NotFound,
        A::InactiveAccount => SocialAidError::InactiveAccount,
        A::InsufficientFunds => SocialAidError::InsufficientFunds,
        A::IdempotencyConflict => SocialAidError::IdempotencyConflict,
        A::Database(error) => SocialAidError::Database(error),
        other => SocialAidError::AccountInvariant(other),
    }
}

// ------------------------------------------------------------------
// Read models
// ------------------------------------------------------------------

pub struct FundFilter {
    pub status: Option<String>,
    pub search: Option<String>,
    pub page: i64,
    pub page_size: i64,
}

#[derive(Debug, sqlx::FromRow)]
pub struct FundListRow {
    pub id: Uuid,
    pub fund_number: i64,
    pub name: String,
    pub status: String,
    /// Derived: sum of posted donations across every account.
    pub total_donated: Decimal,
    /// Derived: sum of posted disbursements across every account.
    pub total_disbursed: Decimal,
    /// Derived restricted availability (never a stored number).
    pub available: Decimal,
    pub created_at: OffsetDateTime,
    pub total_count: i64,
}

/// Server-side list (ADR-010): status/search filters, stable
/// `fund_number DESC` ordering, derived restricted totals.
pub async fn list_funds(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    filter: &FundFilter,
) -> Result<Vec<FundListRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    sqlx::query_as::<_, FundListRow>(&format!(
        "SELECT f.id, f.fund_number, f.name, f.status, \
                COALESCE((SELECT sum(d.amount) FROM social_aid_donations d \
                          WHERE d.fund_id = f.id AND d.status = 'posted'), 0) \
                    AS total_donated, \
                COALESCE((SELECT sum(b.amount) FROM social_aid_disbursements b \
                          WHERE b.fund_id = f.id AND b.status = 'posted'), 0) \
                    AS total_disbursed, \
                COALESCE((SELECT sum(d.amount) FROM social_aid_donations d \
                          WHERE d.fund_id = f.id AND d.status = 'posted'), 0) \
              - COALESCE((SELECT sum(b.amount) FROM social_aid_disbursements b \
                          WHERE b.fund_id = f.id AND b.status = 'posted'), 0) \
                    AS available, \
                f.created_at, \
                count(*) OVER () AS total_count \
         FROM social_aid_funds f \
         WHERE f.cooperative_id IS NOT DISTINCT FROM $3 \
           AND ($1::text IS NULL OR f.status = $1) \
           AND ($2::text IS NULL OR f.name ILIKE '%' || $2 || '%' \
                OR f.fund_number::text = $2) \
         ORDER BY f.fund_number DESC \
         LIMIT {} OFFSET {}",
        filter.page_size,
        (filter.page - 1) * filter.page_size
    ))
    .bind(&filter.status)
    .bind(&filter.search)
    .bind(cooperative_id)
    .fetch_all(&mut *conn)
    .await
}

#[derive(Debug, sqlx::FromRow)]
pub struct FundRow {
    pub id: Uuid,
    pub fund_number: i64,
    pub name: String,
    pub description: Option<String>,
    pub starts_on: Option<Date>,
    pub ends_on: Option<Date>,
    pub status: String,
    pub closed_at: Option<OffsetDateTime>,
    pub cancelled_at: Option<OffsetDateTime>,
    pub cancellation_reason: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Where restricted money physically sits: one row per financial
/// account that carries this fund's restricted balance. `available`
/// is the derived (fund, account) restricted availability; joining the
/// physical account balance is the caller's choice.
#[derive(Debug, sqlx::FromRow)]
pub struct FundAccountRow {
    pub financial_account_id: Uuid,
    pub account_name: String,
    pub donated: Decimal,
    pub disbursed: Decimal,
    pub available: Decimal,
    pub physical_balance: Decimal,
}

#[derive(Debug, sqlx::FromRow)]
pub struct DonationRow {
    pub id: Uuid,
    pub donation_number: i64,
    pub fund_id: Uuid,
    pub donor_person_id: Option<Uuid>,
    pub donor_name: Option<String>,
    pub donor_display_name: Option<String>,
    pub financial_account_id: Uuid,
    pub account_name: String,
    pub amount: Decimal,
    pub occurred_at: OffsetDateTime,
    pub reference: Option<String>,
    pub note: Option<String>,
    pub account_movement_id: Uuid,
    pub movement_status: String,
    pub status: String,
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
pub struct DisbursementRow {
    pub id: Uuid,
    pub disbursement_number: i64,
    pub fund_id: Uuid,
    pub beneficiary_person_id: Option<Uuid>,
    pub beneficiary_name: Option<String>,
    pub beneficiary_display_name: Option<String>,
    pub financial_account_id: Uuid,
    pub account_name: String,
    pub amount: Decimal,
    pub occurred_at: OffsetDateTime,
    pub reason: String,
    pub reference: Option<String>,
    pub account_movement_id: Uuid,
    pub movement_status: String,
    pub status: String,
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
}

const DONATION_SELECT: &str = "SELECT d.id, d.donation_number, d.fund_id, d.donor_person_id, \
            (p.first_name || ' ' || p.last_name) AS donor_name, \
            d.donor_display_name, \
            d.financial_account_id, a.name AS account_name, \
            d.amount, d.occurred_at, d.reference, d.note, \
            d.account_movement_id, m.status AS movement_status, \
            d.status, d.reversed_at, d.reversal_reason \
     FROM social_aid_donations d \
     LEFT JOIN persons p ON p.id = d.donor_person_id \
     JOIN financial_accounts a ON a.id = d.financial_account_id \
     JOIN account_movements m ON m.id = d.account_movement_id";

const DISBURSEMENT_SELECT: &str =
    "SELECT b.id, b.disbursement_number, b.fund_id, b.beneficiary_person_id, \
            (p.first_name || ' ' || p.last_name) AS beneficiary_name, \
            b.beneficiary_display_name, \
            b.financial_account_id, a.name AS account_name, \
            b.amount, b.occurred_at, b.reason, b.reference, \
            b.account_movement_id, m.status AS movement_status, \
            b.status, b.reversed_at, b.reversal_reason \
     FROM social_aid_disbursements b \
     LEFT JOIN persons p ON p.id = b.beneficiary_person_id \
     JOIN financial_accounts a ON a.id = b.financial_account_id \
     JOIN account_movements m ON m.id = b.account_movement_id";

pub struct DonationFilter {
    pub fund_id: Option<Uuid>,
    pub page: i64,
    pub page_size: i64,
}

pub async fn list_donations(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    filter: &DonationFilter,
) -> Result<(Vec<DonationRow>, i64), sqlx::Error> {
    let mut conn = db.conn().await?;
    let rows = sqlx::query_as::<_, DonationRow>(&format!(
        "{DONATION_SELECT} \
         WHERE d.cooperative_id IS NOT DISTINCT FROM $2 \
           AND ($1::uuid IS NULL OR d.fund_id = $1) \
         ORDER BY d.donation_number DESC \
         LIMIT {} OFFSET {}",
        filter.page_size,
        (filter.page - 1) * filter.page_size
    ))
    .bind(filter.fund_id)
    .bind(cooperative_id)
    .fetch_all(&mut *conn)
    .await?;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM social_aid_donations d \
         WHERE d.cooperative_id IS NOT DISTINCT FROM $2 \
           AND ($1::uuid IS NULL OR d.fund_id = $1)",
    )
    .bind(filter.fund_id)
    .bind(cooperative_id)
    .fetch_one(&mut *conn)
    .await?;
    Ok((rows, total))
}

pub async fn get_donation(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    id: Uuid,
) -> Result<Option<DonationRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    sqlx::query_as::<_, DonationRow>(&format!(
        "{DONATION_SELECT} WHERE d.id = $1 \
         AND d.cooperative_id IS NOT DISTINCT FROM $2"
    ))
    .bind(id)
    .bind(cooperative_id)
    .fetch_optional(&mut *conn)
    .await
}

pub struct DisbursementFilter {
    pub fund_id: Option<Uuid>,
    pub page: i64,
    pub page_size: i64,
}

pub async fn list_disbursements(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    filter: &DisbursementFilter,
) -> Result<(Vec<DisbursementRow>, i64), sqlx::Error> {
    let mut conn = db.conn().await?;
    let rows = sqlx::query_as::<_, DisbursementRow>(&format!(
        "{DISBURSEMENT_SELECT} \
         WHERE b.cooperative_id IS NOT DISTINCT FROM $2 \
           AND ($1::uuid IS NULL OR b.fund_id = $1) \
         ORDER BY b.disbursement_number DESC \
         LIMIT {} OFFSET {}",
        filter.page_size,
        (filter.page - 1) * filter.page_size
    ))
    .bind(filter.fund_id)
    .bind(cooperative_id)
    .fetch_all(&mut *conn)
    .await?;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM social_aid_disbursements b \
         WHERE b.cooperative_id IS NOT DISTINCT FROM $2 \
           AND ($1::uuid IS NULL OR b.fund_id = $1)",
    )
    .bind(filter.fund_id)
    .bind(cooperative_id)
    .fetch_one(&mut *conn)
    .await?;
    Ok((rows, total))
}

pub async fn get_disbursement(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    id: Uuid,
) -> Result<Option<DisbursementRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    sqlx::query_as::<_, DisbursementRow>(&format!(
        "{DISBURSEMENT_SELECT} WHERE b.id = $1 \
         AND b.cooperative_id IS NOT DISTINCT FROM $2"
    ))
    .bind(id)
    .bind(cooperative_id)
    .fetch_optional(&mut *conn)
    .await
}

pub struct FundDetail {
    pub fund: FundRow,
    /// Per-account restricted location rows (descending availability).
    pub accounts: Vec<FundAccountRow>,
    pub donations: Vec<DonationRow>,
    pub disbursements: Vec<DisbursementRow>,
}

pub async fn get_fund_detail(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    id: Uuid,
) -> Result<Option<FundDetail>, sqlx::Error> {
    let mut conn = db.conn().await?;
    let fund: Option<FundRow> = sqlx::query_as(
        "SELECT id, fund_number, name, description, starts_on, ends_on, \
                status, closed_at, cancelled_at, cancellation_reason, \
                created_at, updated_at \
         FROM social_aid_funds WHERE id = $1 \
           AND cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(id)
    .bind(cooperative_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(fund) = fund else {
        return Ok(None);
    };

    let accounts = sqlx::query_as::<_, FundAccountRow>(
        "SELECT a.id AS financial_account_id, a.name AS account_name, \
                COALESCE(d.donated, 0) AS donated, \
                COALESCE(b.disbursed, 0) AS disbursed, \
                COALESCE(d.donated, 0) - COALESCE(b.disbursed, 0) AS available, \
                COALESCE((SELECT sum(CASE m.direction WHEN 'inflow' \
                            THEN m.amount ELSE -m.amount END) \
                    FROM account_movements m \
                    WHERE m.account_id = a.id AND m.status = 'active'), 0) \
                    AS physical_balance \
         FROM financial_accounts a \
         LEFT JOIN (SELECT financial_account_id, sum(amount) AS donated \
                    FROM social_aid_donations \
                    WHERE fund_id = $1 AND status = 'posted' \
                    GROUP BY financial_account_id) d \
            ON d.financial_account_id = a.id \
         LEFT JOIN (SELECT financial_account_id, sum(amount) AS disbursed \
                    FROM social_aid_disbursements \
                    WHERE fund_id = $1 AND status = 'posted' \
                    GROUP BY financial_account_id) b \
            ON b.financial_account_id = a.id \
         WHERE d.financial_account_id IS NOT NULL \
            OR b.financial_account_id IS NOT NULL \
         ORDER BY a.id",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;

    let donations = sqlx::query_as::<_, DonationRow>(&format!(
        "{DONATION_SELECT} WHERE d.fund_id = $1 \
         ORDER BY d.donation_number DESC"
    ))
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;

    let disbursements = sqlx::query_as::<_, DisbursementRow>(&format!(
        "{DISBURSEMENT_SELECT} WHERE b.fund_id = $1 \
         ORDER BY b.disbursement_number DESC"
    ))
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;

    Ok(Some(FundDetail {
        fund,
        accounts,
        donations,
        disbursements,
    }))
}

// ------------------------------------------------------------------
// Fund identity commands — metadata only, ZERO money (docs/11).
// ------------------------------------------------------------------

pub struct CreateFund {
    pub name: String,
    pub description: Option<String>,
    pub starts_on: Option<Date>,
    pub ends_on: Option<Date>,
    pub idempotency_key: String,
    pub fingerprint: String,
}

pub struct FundOutcome {
    pub fund_id: Uuid,
    pub fund_number: i64,
    pub replayed: bool,
}

/// Fund creation moves ZERO money — the row is purpose/restriction
/// metadata only.
pub async fn create_fund(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    command: CreateFund,
) -> Result<FundOutcome, SocialAidError> {
    let mut tx = db.begin().await.map_err(SocialAidError::Database)?;
    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, fund_number, idempotency_fingerprint \
         FROM social_aid_funds WHERE idempotency_key = $1 \
           AND cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(&command.idempotency_key)
    .bind(cooperative_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(SocialAidError::Database)?;
    if let Some((id, number, fingerprint)) = existing {
        if fingerprint != command.fingerprint {
            return Err(SocialAidError::IdempotencyConflict);
        }
        return Ok(FundOutcome {
            fund_id: id,
            fund_number: number,
            replayed: true,
        });
    }

    // M1-P3-B6: the allocator runs inside the INSERT's own transaction
    // — a concurrent idempotency loser rolls its allocation back with
    // the tx, so a replay never consumes a document number.
    let inserted: Option<(Uuid, i64)> = sqlx::query_as(
        "INSERT INTO social_aid_funds \
            (name, description, starts_on, ends_on, status, \
             idempotency_key, idempotency_fingerprint, created_by, cooperative_id, \
             fund_number) \
         VALUES ($1, $2, $3, $4, 'active', $5, $6, $7, $8, \
             allocate_document_number($8, 'social_aid_funds.fund_number')) \
         ON CONFLICT (cooperative_id, idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING \
         RETURNING id, fund_number",
    )
    .bind(&command.name)
    .bind(&command.description)
    .bind(command.starts_on)
    .bind(command.ends_on)
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
    .map_err(SocialAidError::Database)?;

    let Some((id, number)) = inserted else {
        let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
            "SELECT id, fund_number, idempotency_fingerprint \
             FROM social_aid_funds WHERE idempotency_key = $1 \
               AND cooperative_id IS NOT DISTINCT FROM $2",
        )
        .bind(&command.idempotency_key)
        .bind(cooperative_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(SocialAidError::Database)?;
        tx.rollback().await.map_err(SocialAidError::Database)?;
        let Some((id, number, fingerprint)) = existing else {
            return Err(SocialAidError::NotFound);
        };
        if fingerprint != command.fingerprint {
            return Err(SocialAidError::IdempotencyConflict);
        }
        return Ok(FundOutcome {
            fund_id: id,
            fund_number: number,
            replayed: true,
        });
    };

    tx.commit().await.map_err(SocialAidError::Database)?;
    Ok(FundOutcome {
        fund_id: id,
        fund_number: number,
        replayed: false,
    })
}

/// Lock the fund row inside the caller's transaction — the FIRST step
/// of every fund-level command (canonical lock prefix; serializes all
/// restricted-availability mutations for this fund).
async fn lock_fund(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    cooperative_id: Option<Uuid>,
    fund_id: Uuid,
) -> Result<String, SocialAidError> {
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM public.app_lock_social_aid_fund($1, $2)")
            .bind(fund_id)
            .bind(cooperative_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(SocialAidError::Database)?;
    status.ok_or(SocialAidError::NotFound)
}

/// Derived restricted availability of one fund in one account — call
/// ONLY while holding the fund row lock (and the account lock for
/// commands that also mutate physical balance).
async fn locked_restricted_available(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    fund_id: Uuid,
    account_id: Uuid,
) -> Result<Decimal, SocialAidError> {
    sqlx::query_scalar::<_, Option<Decimal>>(
        "SELECT \
            COALESCE((SELECT sum(amount) FROM social_aid_donations \
                      WHERE fund_id = $1 AND financial_account_id = $2 \
                        AND status = 'posted'), 0) \
          - COALESCE((SELECT sum(amount) FROM social_aid_disbursements \
                      WHERE fund_id = $1 AND financial_account_id = $2 \
                        AND status = 'posted'), 0)",
    )
    .bind(fund_id)
    .bind(account_id)
    .fetch_one(tx.as_mut())
    .await
    .map(|v| v.unwrap_or_default())
    .map_err(SocialAidError::Database)
}

/// Close is terminal (no reopen in STEP-013): refused while ANY
/// (fund, account) restricted availability is non-zero — restricted
/// money must never become ownerless (docs/11 §restricted funds).
pub async fn close_fund(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    fund_id: Uuid,
    now: OffsetDateTime,
) -> Result<i64, SocialAidError> {
    let mut tx = db.begin().await.map_err(SocialAidError::Database)?;

    // R3C: one atomic definer — fund lock, the zero-restricted-
    // availability guard and the flip are indivisible.
    let (_replayed, number): (bool, i64) = sqlx::query_as(
        "SELECT replayed, fund_number \
         FROM public.app_social_aid_fund_close_tx($1, $2, $3, $4)",
    )
    .bind(fund_id)
    .bind(now)
    .bind(actor)
    .bind(cooperative_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(|error| match crate::db::package_error_token(&error) {
        Some("not_found") => SocialAidError::NotFound,
        Some("has_restricted_balance") => SocialAidError::HasRestrictedBalance,
        _ => SocialAidError::Database(error),
    })?;

    tx.commit().await.map_err(SocialAidError::Database)?;
    Ok(number)
}

/// Cancellation is identity-only: possible only while the fund has
/// never carried a financial event (no donations, no disbursements).
pub async fn cancel_fund(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    fund_id: Uuid,
    reason: String,
    now: OffsetDateTime,
) -> Result<i64, SocialAidError> {
    let mut tx = db.begin().await.map_err(SocialAidError::Database)?;

    // R3C: one atomic definer — fund lock, the no-financial-events
    // guard and the flip are indivisible.
    let (_replayed, number): (bool, i64) = sqlx::query_as(
        "SELECT replayed, fund_number \
         FROM public.app_social_aid_fund_cancel_tx($1, $2, $3, $4, $5)",
    )
    .bind(fund_id)
    .bind(now)
    .bind(actor)
    .bind(&reason)
    .bind(cooperative_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(|error| match crate::db::package_error_token(&error) {
        Some("not_found") => SocialAidError::NotFound,
        Some("has_restricted_balance") => SocialAidError::HasRestrictedBalance,
        _ => SocialAidError::Database(error),
    })?;

    tx.commit().await.map_err(SocialAidError::Database)?;
    Ok(number)
}

// ------------------------------------------------------------------
// Donation command (inflow — NOT Payment, NOT Income)
// ------------------------------------------------------------------

pub struct PostDonation {
    pub fund_id: Uuid,
    pub donor_person_id: Option<Uuid>,
    pub donor_display_name: Option<String>,
    pub financial_account_id: Uuid,
    pub amount: Decimal,
    pub occurred_at: OffsetDateTime,
    pub reference: Option<String>,
    pub note: Option<String>,
    pub idempotency_key: String,
    pub fingerprint: String,
}

pub struct EventOutcome {
    pub event_id: Uuid,
    pub event_number: i64,
    pub replayed: bool,
}

/// Post a donation: exactly ONE inflow movement ('social_aid_donation')
/// credited to the (fund, account) pair — restricted availability
/// grows. The donation table id is generated up front so movement
/// provenance and the 1:1 binding are established in one transaction.
/// Inflows need no balance check; the donor identity invariant (at
/// least one of person/display name) is enforced by callers and a DB
/// CHECK.
pub async fn post_donation(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    command: PostDonation,
) -> Result<EventOutcome, SocialAidError> {
    let mut tx = db.begin().await.map_err(SocialAidError::Database)?;

    // 1. Idempotent replay short-circuit.
    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, donation_number, idempotency_fingerprint \
         FROM social_aid_donations WHERE idempotency_key = $1 \
           AND cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(&command.idempotency_key)
    .bind(cooperative_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(SocialAidError::Database)?;
    if let Some((id, number, fingerprint)) = existing {
        if fingerprint != command.fingerprint {
            return Err(SocialAidError::IdempotencyConflict);
        }
        tx.rollback().await.map_err(SocialAidError::Database)?;
        return Ok(EventOutcome {
            event_id: id,
            event_number: number,
            replayed: true,
        });
    }

    // 2. Lock the Fund: donations only to an ACTIVE fund.
    let status = lock_fund(&mut tx, cooperative_id, command.fund_id).await?;
    if status != model::FundStatus::Active.as_str() {
        return Err(SocialAidError::InvalidState);
    }

    // 3. Lock the account (canonical prefix: fund -> account).
    let locked = account_repo::lock_accounts(&mut tx, &[command.financial_account_id])
        .await
        .map_err(account_to_error)?;
    let Some((account_status, _currency, account_coop)) = locked.get(&command.financial_account_id)
    else {
        return Err(SocialAidError::NotFound);
    };
    // Same-cooperative only: a foreign account resolves as absent.
    if *account_coop != cooperative_id {
        return Err(SocialAidError::NotFound);
    }
    if *account_status != AccountStatus::Active.as_str() {
        return Err(SocialAidError::InactiveAccount);
    }

    // 4. Donor identity: a linked person must exist (defense in depth —
    //    routes validate first, the FK enforces it in the DB).
    if let Some(person_id) = command.donor_person_id {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM persons WHERE id = $1 \
             AND cooperative_id IS NOT DISTINCT FROM $2)",
        )
        .bind(person_id)
        .bind(cooperative_id)
        .fetch_one(tx.as_mut())
        .await
        .map_err(SocialAidError::Database)?;
        if !exists {
            return Err(SocialAidError::NotFound);
        }
    }

    // 5. Movement first, then the bound domain row.
    let donation_id = Uuid::new_v4();
    let movement_id = account_repo::insert_movement(
        &mut tx,
        account_repo::NewMovement {
            account_id: command.financial_account_id,
            direction: MovementDirection::Inflow,
            amount: command.amount,
            source: MovementSource::SocialAidDonation,
            source_id: donation_id,
            occurred_at: command.occurred_at,
            actor,
            // C3: the movement lives in its account's cooperative.
            cooperative_id: *account_coop,
        },
    )
    .await
    .map_err(account_to_error)?;

    let inserted: Option<(Uuid, i64)> = sqlx::query_as(
        "INSERT INTO social_aid_donations \
            (id, fund_id, donor_person_id, donor_display_name, \
             financial_account_id, amount, currency, occurred_at, \
             reference, note, account_movement_id, status, \
             idempotency_key, idempotency_fingerprint, created_by, cooperative_id, \
             donation_number) \
         VALUES ($1, $2, $3, $4, $5, $6, 'TRY', $7, $8, $9, $10, 'posted', $11, $12, $13, $14, \
             allocate_document_number($14, 'social_aid_donations.donation_number')) \
         ON CONFLICT (cooperative_id, idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING \
         RETURNING id, donation_number",
    )
    .bind(donation_id)
    .bind(command.fund_id)
    .bind(command.donor_person_id)
    .bind(&command.donor_display_name)
    .bind(command.financial_account_id)
    .bind(command.amount)
    .bind(command.occurred_at)
    .bind(&command.reference)
    .bind(&command.note)
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
    .map_err(SocialAidError::Database)?;

    let Some((id, number)) = inserted else {
        let replayed = replay_existing_donation(&mut tx, cooperative_id, &command).await;
        tx.rollback().await.map_err(SocialAidError::Database)?;
        return replayed;
    };

    tx.commit().await.map_err(SocialAidError::Database)?;
    Ok(EventOutcome {
        event_id: id,
        event_number: number,
        replayed: false,
    })
}

async fn replay_existing_donation(
    executor: &mut sqlx::PgConnection,
    cooperative_id: Option<Uuid>,
    command: &PostDonation,
) -> Result<EventOutcome, SocialAidError> {
    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, donation_number, idempotency_fingerprint \
         FROM social_aid_donations WHERE idempotency_key = $1 \
           AND cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(&command.idempotency_key)
    .bind(cooperative_id)
    .fetch_optional(&mut *executor)
    .await
    .map_err(SocialAidError::Database)?;
    let Some((id, number, fingerprint)) = existing else {
        return Err(SocialAidError::NotFound);
    };
    if fingerprint != command.fingerprint {
        return Err(SocialAidError::IdempotencyConflict);
    }
    Ok(EventOutcome {
        event_id: id,
        event_number: number,
        replayed: true,
    })
}

/// Donation reversal (docs/19): preserves the row, flips its movement.
/// Reversing an INFLOW removes money — refused when it would overdraw
/// the physical account OR the fund's restricted availability in that
/// account (docs/11: restricted money already spent cannot be
/// invalidated by a reversal).
pub async fn reverse_donation(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    donation_id: Uuid,
    reason: String,
    now: OffsetDateTime,
) -> Result<(bool, i64), SocialAidError> {
    let mut tx = db.begin().await.map_err(SocialAidError::Database)?;

    // R3C: one atomic definer — donation lock + status, canonical
    // fund -> account lock prefix, restricted-availability and
    // physical-balance checks and the compensating movement flip are
    // indivisible. Restricted money can never be invalidated after
    // it has been consumed (docs/11, docs/15).
    let (replayed, number): (bool, i64) = sqlx::query_as(
        "SELECT replayed, donation_number \
         FROM public.app_social_aid_donation_reverse_tx($1, $2, $3, $4, $5)",
    )
    .bind(donation_id)
    .bind(now)
    .bind(actor)
    .bind(&reason)
    .bind(cooperative_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(|error| match crate::db::package_error_token(&error) {
        Some("not_found") => SocialAidError::NotFound,
        Some("insufficient_restricted") => SocialAidError::InsufficientRestricted,
        Some("insufficient_funds") => SocialAidError::InsufficientFunds,
        _ => SocialAidError::Database(error),
    })?;

    tx.commit().await.map_err(SocialAidError::Database)?;
    Ok((replayed, number))
}

// ------------------------------------------------------------------
// Aid disbursement command (outflow — NOT Expense, NOT Transfer)
// ------------------------------------------------------------------

pub struct PostDisbursement {
    pub fund_id: Uuid,
    pub beneficiary_person_id: Option<Uuid>,
    pub beneficiary_display_name: Option<String>,
    pub financial_account_id: Uuid,
    pub amount: Decimal,
    pub occurred_at: OffsetDateTime,
    pub reason: String,
    pub reference: Option<String>,
    pub idempotency_key: String,
    pub fingerprint: String,
}

/// Post an aid disbursement: exactly ONE outflow movement
/// ('social_aid_disbursement'). Both availability dimensions are
/// re-checked under the locks: the fund's restricted availability in
/// THIS account AND the physical account balance — restricted money
/// can never be spent merely because unrelated cooperative cash or
/// another account's restricted balance exists (docs/11, docs/15).
pub async fn post_disbursement(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    command: PostDisbursement,
) -> Result<EventOutcome, SocialAidError> {
    let mut tx = db.begin().await.map_err(SocialAidError::Database)?;

    // 1. Idempotent replay short-circuit.
    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, disbursement_number, idempotency_fingerprint \
         FROM social_aid_disbursements WHERE idempotency_key = $1 \
           AND cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(&command.idempotency_key)
    .bind(cooperative_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(SocialAidError::Database)?;
    if let Some((id, number, fingerprint)) = existing {
        if fingerprint != command.fingerprint {
            return Err(SocialAidError::IdempotencyConflict);
        }
        tx.rollback().await.map_err(SocialAidError::Database)?;
        return Ok(EventOutcome {
            event_id: id,
            event_number: number,
            replayed: true,
        });
    }

    // 2. Lock the Fund: disbursements only from an ACTIVE fund.
    let status = lock_fund(&mut tx, cooperative_id, command.fund_id).await?;
    if status != model::FundStatus::Active.as_str() {
        return Err(SocialAidError::InvalidState);
    }

    // 3. Lock the account (canonical prefix: fund -> account).
    let locked = account_repo::lock_accounts(&mut tx, &[command.financial_account_id])
        .await
        .map_err(account_to_error)?;
    let Some((account_status, _currency, account_coop)) = locked.get(&command.financial_account_id)
    else {
        return Err(SocialAidError::NotFound);
    };
    // Same-cooperative only: a foreign account resolves as absent.
    if *account_coop != cooperative_id {
        return Err(SocialAidError::NotFound);
    }
    if *account_status != AccountStatus::Active.as_str() {
        return Err(SocialAidError::InactiveAccount);
    }

    // 4. Beneficiary identity: a linked person must exist.
    if let Some(person_id) = command.beneficiary_person_id {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM persons WHERE id = $1 \
             AND cooperative_id IS NOT DISTINCT FROM $2)",
        )
        .bind(person_id)
        .bind(cooperative_id)
        .fetch_one(tx.as_mut())
        .await
        .map_err(SocialAidError::Database)?;
        if !exists {
            return Err(SocialAidError::NotFound);
        }
    }

    // 5. Restricted availability in THIS account under the fund lock.
    let restricted =
        locked_restricted_available(&mut tx, command.fund_id, command.financial_account_id).await?;
    if restricted < command.amount {
        return Err(SocialAidError::InsufficientRestricted);
    }

    // 6. Physical account balance under the account row lock.
    let balance =
        account_repo::locked_balance(&mut tx, *account_coop, command.financial_account_id)
            .await
            .map_err(account_to_error)?;
    if command.amount > balance {
        return Err(SocialAidError::InsufficientFunds);
    }

    // 7. Movement first, then the bound domain row.
    let disbursement_id = Uuid::new_v4();
    let movement_id = account_repo::insert_movement(
        &mut tx,
        account_repo::NewMovement {
            account_id: command.financial_account_id,
            direction: MovementDirection::Outflow,
            amount: command.amount,
            source: MovementSource::SocialAidDisbursement,
            source_id: disbursement_id,
            occurred_at: command.occurred_at,
            actor,
            // C3: the movement lives in its account's cooperative.
            cooperative_id: *account_coop,
        },
    )
    .await
    .map_err(account_to_error)?;

    let inserted: Option<(Uuid, i64)> = sqlx::query_as(
        "INSERT INTO social_aid_disbursements \
            (id, fund_id, beneficiary_person_id, beneficiary_display_name, \
             financial_account_id, amount, currency, occurred_at, \
             reason, reference, account_movement_id, status, \
             idempotency_key, idempotency_fingerprint, created_by, cooperative_id, \
             disbursement_number) \
         VALUES ($1, $2, $3, $4, $5, $6, 'TRY', $7, $8, $9, $10, 'posted', $11, $12, $13, $14, \
             allocate_document_number($14, 'social_aid_disbursements.disbursement_number')) \
         ON CONFLICT (cooperative_id, idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING \
         RETURNING id, disbursement_number",
    )
    .bind(disbursement_id)
    .bind(command.fund_id)
    .bind(command.beneficiary_person_id)
    .bind(&command.beneficiary_display_name)
    .bind(command.financial_account_id)
    .bind(command.amount)
    .bind(command.occurred_at)
    .bind(&command.reason)
    .bind(&command.reference)
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
    .map_err(SocialAidError::Database)?;

    let Some((id, number)) = inserted else {
        let replayed = replay_existing_disbursement(&mut tx, cooperative_id, &command).await;
        tx.rollback().await.map_err(SocialAidError::Database)?;
        return replayed;
    };

    tx.commit().await.map_err(SocialAidError::Database)?;
    Ok(EventOutcome {
        event_id: id,
        event_number: number,
        replayed: false,
    })
}

async fn replay_existing_disbursement(
    executor: &mut sqlx::PgConnection,
    cooperative_id: Option<Uuid>,
    command: &PostDisbursement,
) -> Result<EventOutcome, SocialAidError> {
    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, disbursement_number, idempotency_fingerprint \
         FROM social_aid_disbursements WHERE idempotency_key = $1 \
           AND cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(&command.idempotency_key)
    .bind(cooperative_id)
    .fetch_optional(&mut *executor)
    .await
    .map_err(SocialAidError::Database)?;
    let Some((id, number, fingerprint)) = existing else {
        return Err(SocialAidError::NotFound);
    };
    if fingerprint != command.fingerprint {
        return Err(SocialAidError::IdempotencyConflict);
    }
    Ok(EventOutcome {
        event_id: id,
        event_number: number,
        replayed: true,
    })
}

/// Disbursement reversal (docs/19): preserves the row, flips its
/// movement. Reversing an OUTFLOW returns money — always balance-safe;
/// restricted availability is restored in the same (fund, account)
/// pair that funded the payment. Reversal stays allowed on a closed
/// fund: it is a correction of recorded history, never a new spend.
pub async fn reverse_disbursement(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    disbursement_id: Uuid,
    reason: String,
    now: OffsetDateTime,
) -> Result<(bool, i64), SocialAidError> {
    let mut tx = db.begin().await.map_err(SocialAidError::Database)?;

    // R3C: one atomic definer — disbursement lock + status,
    // canonical fund -> account lock prefix and the compensating
    // movement flip are indivisible.
    let (replayed, number): (bool, i64) = sqlx::query_as(
        "SELECT replayed, disbursement_number \
         FROM public.app_social_aid_disbursement_reverse_tx($1, $2, $3, $4, $5)",
    )
    .bind(disbursement_id)
    .bind(now)
    .bind(actor)
    .bind(&reason)
    .bind(cooperative_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(|error| match crate::db::package_error_token(&error) {
        Some("not_found") => SocialAidError::NotFound,
        _ => SocialAidError::Database(error),
    })?;

    tx.commit().await.map_err(SocialAidError::Database)?;
    Ok((replayed, number))
}
