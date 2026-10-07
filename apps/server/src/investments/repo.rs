//! Investments repository (STEP-012).
//!
//! Transaction discipline (same shape as STEP-008/010/011):
//!   BEGIN -> idempotent replay check -> lock the Investment row
//!   FOR UPDATE -> lock Financial Account row(s) `id ASC` ->
//!   validate -> insert the authoritative movement FIRST -> insert
//!   the bound domain row -> COMMIT.
//!
//! Lock order is the single canonical prefix: investment row first,
//! then account rows in ascending id order (the shared
//! `account_repo::lock_accounts` helper). Every money command obeys
//! the no-negative-balance invariant; inflows need no funds check,
//! outflows and inflow reversals do.
//!
//! Boundaries: no Investment command ever writes `payments`,
//! `payment_allocations`, `shareholder_credits`, `credit_applications`,
//! `account_transfers`, `incomes`, `expenses`, `share_returns` or any
//! entitlement row. Valuations write NO movement at all.

use rust_decimal::Decimal;
use sqlx::PgPool;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::financial_accounts::model::{AccountStatus, MovementDirection, MovementSource};
use crate::financial_accounts::repo as account_repo;
use crate::investments::model;

#[derive(Debug)]
pub enum InvestmentCommandError {
    NotFound,
    StaleState,
    InvalidState,
    HasFinancialEvents,
    InactiveAccount,
    InsufficientFunds,
    IdempotencyConflict,
    Database(sqlx::Error),
    AccountInvariant(account_repo::AccountCommandError),
}

fn account_to_error(error: account_repo::AccountCommandError) -> InvestmentCommandError {
    use account_repo::AccountCommandError as A;
    match error {
        A::NotFound => InvestmentCommandError::NotFound,
        A::InactiveAccount => InvestmentCommandError::InactiveAccount,
        A::InsufficientFunds => InvestmentCommandError::InsufficientFunds,
        A::IdempotencyConflict => InvestmentCommandError::IdempotencyConflict,
        A::StaleState => InvestmentCommandError::StaleState,
        A::Database(error) => InvestmentCommandError::Database(error),
        other => InvestmentCommandError::AccountInvariant(other),
    }
}

// ------------------------------------------------------------------
// Read models
// ------------------------------------------------------------------

pub struct InvestmentFilter {
    pub status: Option<String>,
    pub investment_type: Option<String>,
    pub search: Option<String>,
    pub page: i64,
    pub page_size: i64,
}

#[derive(Debug, sqlx::FromRow)]
pub struct InvestmentListRow {
    pub id: Uuid,
    pub investment_number: i64,
    pub name: String,
    pub investment_type: String,
    pub status: String,
    pub acquired_at: Option<Date>,
    pub total_funded: Decimal,
    pub latest_valuation: Option<Decimal>,
    pub total_income: Decimal,
    pub created_at: OffsetDateTime,
    pub total_count: i64,
}

/// Server-side list (ADR-010): status/type/search filters, stable
/// `investment_number DESC` ordering, derived financial columns.
pub async fn list_investments(
    pool: &PgPool,
    filter: &InvestmentFilter,
) -> Result<Vec<InvestmentListRow>, sqlx::Error> {
    sqlx::query_as::<_, InvestmentListRow>(&format!(
        "SELECT i.id, i.investment_number, i.name, i.investment_type, i.status, \
                i.acquired_at, \
                COALESCE((SELECT sum(f.amount) FROM investment_fundings f \
                          WHERE f.investment_id = i.id AND f.status = 'posted'), 0) \
                    AS total_funded, \
                (SELECT v.amount FROM investment_valuations v \
                  WHERE v.investment_id = i.id AND v.status = 'recorded' \
                  ORDER BY v.valuation_date DESC, v.valuation_number DESC \
                  LIMIT 1) AS latest_valuation, \
                COALESCE((SELECT sum(n.amount) FROM investment_incomes n \
                          WHERE n.investment_id = i.id AND n.status = 'posted'), 0) \
                    AS total_income, \
                i.created_at, \
                count(*) OVER () AS total_count \
         FROM investments i \
         WHERE ($1::text IS NULL OR i.status = $1) \
           AND ($2::text IS NULL OR i.investment_type = $2) \
           AND ($3::text IS NULL OR i.name ILIKE '%' || $3 || '%' \
                OR i.reference ILIKE '%' || $3 || '%' \
                OR i.investment_number::text = $3) \
         ORDER BY i.investment_number DESC \
         LIMIT {} OFFSET {}",
        filter.page_size,
        (filter.page - 1) * filter.page_size
    ))
    .bind(&filter.status)
    .bind(&filter.investment_type)
    .bind(&filter.search)
    .fetch_all(pool)
    .await
}

#[derive(Debug, sqlx::FromRow)]
pub struct InvestmentRow {
    pub id: Uuid,
    pub investment_number: i64,
    pub name: String,
    pub investment_type: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub reference: Option<String>,
    pub counterparty_name: Option<String>,
    pub acquired_at: Option<Date>,
    pub status: String,
    pub disposed_at: Option<OffsetDateTime>,
    pub cancelled_at: Option<OffsetDateTime>,
    pub cancellation_reason: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, sqlx::FromRow)]
pub struct FundingRow {
    pub id: Uuid,
    pub funding_number: i64,
    pub investment_id: Uuid,
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
pub struct ValuationRow {
    pub id: Uuid,
    pub valuation_number: i64,
    pub investment_id: Uuid,
    pub valuation_date: Date,
    pub amount: Decimal,
    pub method: Option<String>,
    pub source: Option<String>,
    pub note: Option<String>,
    pub status: String,
    pub cancelled_at: Option<OffsetDateTime>,
    pub cancellation_reason: Option<String>,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, sqlx::FromRow)]
pub struct IncomeRow {
    pub id: Uuid,
    pub income_number: i64,
    pub investment_id: Uuid,
    pub financial_account_id: Uuid,
    pub account_name: String,
    pub amount: Decimal,
    pub occurred_at: OffsetDateTime,
    pub description: String,
    pub counterparty: Option<String>,
    pub reference_no: Option<String>,
    pub account_movement_id: Uuid,
    pub movement_status: String,
    pub status: String,
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
pub struct ProceedsRow {
    pub id: Uuid,
    pub financial_account_id: Uuid,
    pub account_name: String,
    pub amount: Decimal,
    pub occurred_at: OffsetDateTime,
    pub reference: Option<String>,
    pub account_movement_id: Uuid,
    pub movement_status: String,
}

pub struct DisposalRow {
    pub id: Uuid,
    pub disposal_number: i64,
    pub investment_id: Uuid,
    pub disposed_at: Date,
    pub consideration_amount: Option<Decimal>,
    pub counterparty_name: Option<String>,
    pub reference: Option<String>,
    pub note: Option<String>,
    pub status: String,
    pub created_at: OffsetDateTime,
    pub proceeds: Vec<ProceedsRow>,
}

pub struct InvestmentDetail {
    pub investment: InvestmentRow,
    pub fundings: Vec<FundingRow>,
    pub valuations: Vec<ValuationRow>,
    pub incomes: Vec<IncomeRow>,
    pub disposal: Option<DisposalRow>,
    pub total_funded: Decimal,
    pub latest_valuation: Option<Decimal>,
    pub total_income: Decimal,
    pub total_proceeds: Decimal,
}

pub async fn get_investment(pool: &PgPool, id: Uuid) -> Result<Option<InvestmentRow>, sqlx::Error> {
    sqlx::query_as::<_, InvestmentRow>(
        "SELECT id, investment_number, name, investment_type, description, \
                location, reference, counterparty_name, acquired_at, status, \
                disposed_at, cancelled_at, cancellation_reason, created_at, updated_at \
         FROM investments WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn get_investment_detail(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<InvestmentDetail>, sqlx::Error> {
    let Some(investment) = get_investment(pool, id).await? else {
        return Ok(None);
    };

    let fundings = sqlx::query_as::<_, FundingRow>(
        "SELECT f.id, f.funding_number, f.investment_id, f.financial_account_id, \
                a.name AS account_name, f.amount, f.occurred_at, f.reference, f.note, \
                f.account_movement_id, m.status AS movement_status, \
                f.status, f.reversed_at, f.reversal_reason \
         FROM investment_fundings f \
         JOIN financial_accounts a ON a.id = f.financial_account_id \
         JOIN account_movements m ON m.id = f.account_movement_id \
         WHERE f.investment_id = $1 \
         ORDER BY f.funding_number",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;

    let valuations = sqlx::query_as::<_, ValuationRow>(
        "SELECT id, valuation_number, investment_id, valuation_date, amount, \
                method, source, note, status, cancelled_at, cancellation_reason, \
                created_at \
         FROM investment_valuations \
         WHERE investment_id = $1 \
         ORDER BY valuation_date DESC, valuation_number DESC",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;

    let incomes = sqlx::query_as::<_, IncomeRow>(
        "SELECT n.id, n.income_number, n.investment_id, n.financial_account_id, \
                a.name AS account_name, n.amount, n.occurred_at, n.description, \
                n.counterparty, n.reference_no, n.account_movement_id, \
                m.status AS movement_status, n.status, n.reversed_at, \
                n.reversal_reason \
         FROM investment_incomes n \
         JOIN financial_accounts a ON a.id = n.financial_account_id \
         JOIN account_movements m ON m.id = n.account_movement_id \
         WHERE n.investment_id = $1 \
         ORDER BY n.income_number",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;

    let disposal = sqlx::query_as::<_, DisposalHeadRow>(
        "SELECT id, disposal_number, investment_id, disposed_at, \
                consideration_amount, counterparty_name, reference, note, \
                status, created_at \
         FROM investment_disposals WHERE investment_id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    let disposal = match disposal {
        Some(head) => {
            let proceeds = sqlx::query_as::<_, ProceedsRow>(
                "SELECT p.id, p.financial_account_id, a.name AS account_name, \
                        p.amount, p.occurred_at, p.reference, \
                        p.account_movement_id, m.status AS movement_status \
                 FROM investment_disposal_proceeds p \
                 JOIN financial_accounts a ON a.id = p.financial_account_id \
                 JOIN account_movements m ON m.id = p.account_movement_id \
                 WHERE p.disposal_id = $1 \
                 ORDER BY p.amount DESC, p.id",
            )
            .bind(head.id)
            .fetch_all(pool)
            .await?;
            Some(DisposalRow {
                id: head.id,
                disposal_number: head.disposal_number,
                investment_id: head.investment_id,
                disposed_at: head.disposed_at,
                consideration_amount: head.consideration_amount,
                counterparty_name: head.counterparty_name,
                reference: head.reference,
                note: head.note,
                status: head.status,
                created_at: head.created_at,
                proceeds,
            })
        }
        None => None,
    };

    let total_funded = fundings
        .iter()
        .filter(|f| f.status == "posted")
        .map(|f| f.amount)
        .fold(Decimal::ZERO, |a, b| a + b);
    let latest_valuation = valuations
        .iter()
        .find(|v| v.status == "recorded")
        .map(|v| v.amount);
    let total_income = incomes
        .iter()
        .filter(|n| n.status == "posted")
        .map(|n| n.amount)
        .fold(Decimal::ZERO, |a, b| a + b);
    let total_proceeds = disposal
        .as_ref()
        .map(|d| {
            d.proceeds
                .iter()
                .map(|p| p.amount)
                .fold(Decimal::ZERO, |a, b| a + b)
        })
        .unwrap_or_default();

    Ok(Some(InvestmentDetail {
        investment,
        fundings,
        valuations,
        incomes,
        disposal,
        total_funded,
        latest_valuation,
        total_income,
        total_proceeds,
    }))
}

#[derive(sqlx::FromRow)]
struct DisposalHeadRow {
    id: Uuid,
    disposal_number: i64,
    investment_id: Uuid,
    disposed_at: Date,
    consideration_amount: Option<Decimal>,
    counterparty_name: Option<String>,
    reference: Option<String>,
    note: Option<String>,
    status: String,
    created_at: OffsetDateTime,
}

// ------------------------------------------------------------------
// Investment identity commands
// ------------------------------------------------------------------

pub struct CreateInvestment {
    pub name: String,
    pub investment_type: model::InvestmentType,
    pub description: Option<String>,
    pub location: Option<String>,
    pub reference: Option<String>,
    pub counterparty_name: Option<String>,
    pub acquired_at: Option<Date>,
    pub idempotency_key: String,
    pub fingerprint: String,
}

pub struct InvestmentOutcome {
    pub investment_id: Uuid,
    pub investment_number: i64,
    pub replayed: bool,
}

/// Investment identity creation — deliberately moves ZERO money
/// (docs/08: acquisition is a separate funding event).
pub async fn create_investment(
    pool: &PgPool,
    actor: Uuid,
    command: CreateInvestment,
) -> Result<InvestmentOutcome, InvestmentCommandError> {
    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, investment_number, idempotency_fingerprint \
         FROM investments WHERE idempotency_key = $1",
    )
    .bind(&command.idempotency_key)
    .fetch_optional(pool)
    .await
    .map_err(InvestmentCommandError::Database)?;
    if let Some((id, number, fingerprint)) = existing {
        if fingerprint != command.fingerprint {
            return Err(InvestmentCommandError::IdempotencyConflict);
        }
        return Ok(InvestmentOutcome {
            investment_id: id,
            investment_number: number,
            replayed: true,
        });
    }

    let inserted: Option<(Uuid, i64)> = sqlx::query_as(
        "INSERT INTO investments \
            (name, investment_type, description, location, reference, \
             counterparty_name, acquired_at, status, \
             idempotency_key, idempotency_fingerprint, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'active', $8, $9, $10) \
         ON CONFLICT (idempotency_key) DO NOTHING \
         RETURNING id, investment_number",
    )
    .bind(&command.name)
    .bind(command.investment_type.as_str())
    .bind(&command.description)
    .bind(&command.location)
    .bind(&command.reference)
    .bind(&command.counterparty_name)
    .bind(command.acquired_at)
    .bind(&command.idempotency_key)
    .bind(&command.fingerprint)
    .bind(actor)
    .fetch_optional(pool)
    .await
    .map_err(InvestmentCommandError::Database)?;

    let Some((id, number)) = inserted else {
        let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
            "SELECT id, investment_number, idempotency_fingerprint \
             FROM investments WHERE idempotency_key = $1",
        )
        .bind(&command.idempotency_key)
        .fetch_optional(pool)
        .await
        .map_err(InvestmentCommandError::Database)?;
        let Some((id, number, fingerprint)) = existing else {
            return Err(InvestmentCommandError::NotFound);
        };
        if fingerprint != command.fingerprint {
            return Err(InvestmentCommandError::IdempotencyConflict);
        }
        return Ok(InvestmentOutcome {
            investment_id: id,
            investment_number: number,
            replayed: true,
        });
    };

    Ok(InvestmentOutcome {
        investment_id: id,
        investment_number: number,
        replayed: false,
    })
}

/// Lock the investment row inside the caller's transaction — the first
/// step of every investment-level command (canonical lock prefix).
async fn lock_investment(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    investment_id: Uuid,
) -> Result<String, InvestmentCommandError> {
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM investments WHERE id = $1 FOR UPDATE")
            .bind(investment_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(InvestmentCommandError::Database)?;
    status.ok_or(InvestmentCommandError::NotFound)
}

/// Cancellation is identity-only: possible only while the investment
/// has never carried a financial event (no fundings, no incomes, no
/// disposal). Valuations are informational and do not block.
pub async fn cancel_investment(
    pool: &PgPool,
    actor: Uuid,
    investment_id: Uuid,
    reason: String,
    now: OffsetDateTime,
) -> Result<i64, InvestmentCommandError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(InvestmentCommandError::Database)?;

    let row: Option<(String, i64)> = sqlx::query_as(
        "SELECT status, investment_number FROM investments WHERE id = $1 FOR UPDATE",
    )
    .bind(investment_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(InvestmentCommandError::Database)?;
    let Some((status, number)) = row else {
        return Err(InvestmentCommandError::NotFound);
    };
    if status != model::InvestmentStatus::Active.as_str() {
        tx.rollback()
            .await
            .map_err(InvestmentCommandError::Database)?;
        return Ok(number);
    }

    let has_events: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM investment_fundings WHERE investment_id = $1) \
            OR EXISTS (SELECT 1 FROM investment_incomes WHERE investment_id = $1) \
            OR EXISTS (SELECT 1 FROM investment_disposals WHERE investment_id = $1)",
    )
    .bind(investment_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(InvestmentCommandError::Database)?;
    if has_events {
        return Err(InvestmentCommandError::HasFinancialEvents);
    }

    sqlx::query(
        "UPDATE investments SET status = 'cancelled', cancelled_at = $2, \
            cancelled_by = $3, cancellation_reason = $4, updated_at = $2 \
         WHERE id = $1",
    )
    .bind(investment_id)
    .bind(now)
    .bind(actor)
    .bind(&reason)
    .execute(tx.as_mut())
    .await
    .map_err(InvestmentCommandError::Database)?;

    tx.commit()
        .await
        .map_err(InvestmentCommandError::Database)?;
    Ok(number)
}

// ------------------------------------------------------------------
// Acquisition funding (outflow — NOT Expense, NOT Transfer)
// ------------------------------------------------------------------

pub struct PostFunding {
    pub investment_id: Uuid,
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

/// Post one acquisition funding leg: exactly ONE outflow movement
/// ('investment_funding'). The funding table id is generated up front
/// so movement provenance and the 1:1 binding are established in one
/// transaction.
pub async fn post_funding(
    pool: &PgPool,
    actor: Uuid,
    command: PostFunding,
) -> Result<EventOutcome, InvestmentCommandError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(InvestmentCommandError::Database)?;

    // 1. Idempotent replay short-circuit.
    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, funding_number, idempotency_fingerprint \
         FROM investment_fundings WHERE idempotency_key = $1",
    )
    .bind(&command.idempotency_key)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(InvestmentCommandError::Database)?;
    if let Some((id, number, fingerprint)) = existing {
        if fingerprint != command.fingerprint {
            return Err(InvestmentCommandError::IdempotencyConflict);
        }
        tx.rollback()
            .await
            .map_err(InvestmentCommandError::Database)?;
        return Ok(EventOutcome {
            event_id: id,
            event_number: number,
            replayed: true,
        });
    }

    // 2. Lock the Investment: funding only an ACTIVE investment.
    let status = lock_investment(&mut tx, command.investment_id).await?;
    if status != model::InvestmentStatus::Active.as_str() {
        return Err(InvestmentCommandError::InvalidState);
    }

    // 3. Lock the account (canonical prefix: investment -> account).
    let locked = account_repo::lock_accounts(&mut tx, &[command.financial_account_id])
        .await
        .map_err(account_to_error)?;
    let Some((account_status, _currency)) = locked.get(&command.financial_account_id) else {
        return Err(InvestmentCommandError::NotFound);
    };
    if *account_status != AccountStatus::Active.as_str() {
        return Err(InvestmentCommandError::InactiveAccount);
    }

    // 4. No-negative-balance under the account row lock.
    let balance = account_repo::locked_balance(&mut tx, command.financial_account_id)
        .await
        .map_err(account_to_error)?;
    if command.amount > balance {
        return Err(InvestmentCommandError::InsufficientFunds);
    }

    // 5. Movement first, then the bound domain row.
    let funding_id = Uuid::new_v4();
    let movement_id = account_repo::insert_movement(
        &mut tx,
        account_repo::NewMovement {
            account_id: command.financial_account_id,
            direction: MovementDirection::Outflow,
            amount: command.amount,
            source: MovementSource::InvestmentFunding,
            source_id: funding_id,
            occurred_at: command.occurred_at,
            actor,
        },
    )
    .await
    .map_err(account_to_error)?;

    let inserted: Option<(Uuid, i64)> = sqlx::query_as(
        "INSERT INTO investment_fundings \
            (id, investment_id, financial_account_id, amount, currency, \
             occurred_at, reference, note, account_movement_id, status, \
             idempotency_key, idempotency_fingerprint, created_by) \
         VALUES ($1, $2, $3, $4, 'TRY', $5, $6, $7, $8, 'posted', $9, $10, $11) \
         ON CONFLICT (idempotency_key) DO NOTHING \
         RETURNING id, funding_number",
    )
    .bind(funding_id)
    .bind(command.investment_id)
    .bind(command.financial_account_id)
    .bind(command.amount)
    .bind(command.occurred_at)
    .bind(&command.reference)
    .bind(&command.note)
    .bind(movement_id)
    .bind(&command.idempotency_key)
    .bind(&command.fingerprint)
    .bind(actor)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(InvestmentCommandError::Database)?;

    let Some((id, number)) = inserted else {
        tx.rollback()
            .await
            .map_err(InvestmentCommandError::Database)?;
        return replay_existing_funding(pool, &command).await;
    };

    tx.commit()
        .await
        .map_err(InvestmentCommandError::Database)?;
    Ok(EventOutcome {
        event_id: id,
        event_number: number,
        replayed: false,
    })
}

async fn replay_existing_funding(
    pool: &PgPool,
    command: &PostFunding,
) -> Result<EventOutcome, InvestmentCommandError> {
    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, funding_number, idempotency_fingerprint \
         FROM investment_fundings WHERE idempotency_key = $1",
    )
    .bind(&command.idempotency_key)
    .fetch_optional(pool)
    .await
    .map_err(InvestmentCommandError::Database)?;
    let Some((id, number, fingerprint)) = existing else {
        return Err(InvestmentCommandError::NotFound);
    };
    if fingerprint != command.fingerprint {
        return Err(InvestmentCommandError::IdempotencyConflict);
    }
    Ok(EventOutcome {
        event_id: id,
        event_number: number,
        replayed: true,
    })
}

/// Funding reversal (docs/19): preserves the row, flips its movement.
/// Reversal of an outflow returns money — always balance-safe — but is
/// refused once the Investment is disposed (the asset is gone; the
/// deferred disposal-correction path must run first).
pub async fn reverse_funding(
    pool: &PgPool,
    actor: Uuid,
    funding_id: Uuid,
    reason: String,
    now: OffsetDateTime,
) -> Result<(bool, i64), InvestmentCommandError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(InvestmentCommandError::Database)?;

    let row: Option<(String, i64, Uuid, Uuid)> = sqlx::query_as(
        "SELECT status, funding_number, investment_id, financial_account_id \
         FROM investment_fundings WHERE id = $1 FOR UPDATE",
    )
    .bind(funding_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(InvestmentCommandError::Database)?;
    let Some((status, number, investment_id, account_id)) = row else {
        return Err(InvestmentCommandError::NotFound);
    };
    if status != "posted" {
        tx.rollback()
            .await
            .map_err(InvestmentCommandError::Database)?;
        return Ok((true, number));
    }

    let investment_status = lock_investment(&mut tx, investment_id).await?;
    if investment_status == model::InvestmentStatus::Disposed.as_str() {
        return Err(InvestmentCommandError::InvalidState);
    }

    account_repo::lock_accounts(&mut tx, &[account_id])
        .await
        .map_err(account_to_error)?;

    account_repo::reverse_movements_of_source(
        &mut tx,
        MovementSource::InvestmentFunding,
        funding_id,
        now,
        actor,
        &reason,
    )
    .await
    .map_err(account_to_error)?;

    sqlx::query(
        "UPDATE investment_fundings SET status = 'reversed', \
            reversed_at = $2, reversed_by = $3, reversal_reason = $4, updated_at = $2 \
         WHERE id = $1",
    )
    .bind(funding_id)
    .bind(now)
    .bind(actor)
    .bind(&reason)
    .execute(tx.as_mut())
    .await
    .map_err(InvestmentCommandError::Database)?;

    tx.commit()
        .await
        .map_err(InvestmentCommandError::Database)?;
    Ok((false, number))
}

// ------------------------------------------------------------------
// Valuations (informational — ZERO movements by construction)
// ------------------------------------------------------------------

pub struct RecordValuation {
    pub investment_id: Uuid,
    pub valuation_date: Date,
    pub amount: Decimal,
    pub method: Option<String>,
    pub source: Option<String>,
    pub note: Option<String>,
    pub idempotency_key: String,
    pub fingerprint: String,
}

/// Record a valuation: writes ONLY the history row — no account lock,
/// no movement, no income/expense (docs/08: "Valuation gain is not
/// automatically realized profit/cash").
pub async fn record_valuation(
    pool: &PgPool,
    actor: Uuid,
    command: RecordValuation,
) -> Result<EventOutcome, InvestmentCommandError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(InvestmentCommandError::Database)?;

    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, valuation_number, idempotency_fingerprint \
         FROM investment_valuations WHERE idempotency_key = $1",
    )
    .bind(&command.idempotency_key)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(InvestmentCommandError::Database)?;
    if let Some((id, number, fingerprint)) = existing {
        if fingerprint != command.fingerprint {
            return Err(InvestmentCommandError::IdempotencyConflict);
        }
        tx.rollback()
            .await
            .map_err(InvestmentCommandError::Database)?;
        return Ok(EventOutcome {
            event_id: id,
            event_number: number,
            replayed: true,
        });
    }

    let status = lock_investment(&mut tx, command.investment_id).await?;
    if status != model::InvestmentStatus::Active.as_str() {
        return Err(InvestmentCommandError::InvalidState);
    }

    let inserted: Option<(Uuid, i64)> = sqlx::query_as(
        "INSERT INTO investment_valuations \
            (investment_id, valuation_date, amount, currency, method, source, \
             note, status, idempotency_key, idempotency_fingerprint, created_by) \
         VALUES ($1, $2, $3, 'TRY', $4, $5, $6, 'recorded', $7, $8, $9) \
         ON CONFLICT (idempotency_key) DO NOTHING \
         RETURNING id, valuation_number",
    )
    .bind(command.investment_id)
    .bind(command.valuation_date)
    .bind(command.amount)
    .bind(&command.method)
    .bind(&command.source)
    .bind(&command.note)
    .bind(&command.idempotency_key)
    .bind(&command.fingerprint)
    .bind(actor)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(InvestmentCommandError::Database)?;

    let Some((id, number)) = inserted else {
        tx.rollback()
            .await
            .map_err(InvestmentCommandError::Database)?;
        let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
            "SELECT id, valuation_number, idempotency_fingerprint \
             FROM investment_valuations WHERE idempotency_key = $1",
        )
        .bind(&command.idempotency_key)
        .fetch_optional(pool)
        .await
        .map_err(InvestmentCommandError::Database)?;
        let Some((id, number, fingerprint)) = existing else {
            return Err(InvestmentCommandError::NotFound);
        };
        if fingerprint != command.fingerprint {
            return Err(InvestmentCommandError::IdempotencyConflict);
        }
        return Ok(EventOutcome {
            event_id: id,
            event_number: number,
            replayed: true,
        });
    };

    tx.commit()
        .await
        .map_err(InvestmentCommandError::Database)?;
    Ok(EventOutcome {
        event_id: id,
        event_number: number,
        replayed: false,
    })
}

/// Cancel a wrongly-recorded valuation — the row remains as history.
pub async fn cancel_valuation(
    pool: &PgPool,
    actor: Uuid,
    valuation_id: Uuid,
    reason: String,
    now: OffsetDateTime,
) -> Result<(bool, i64), InvestmentCommandError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(InvestmentCommandError::Database)?;

    let row: Option<(String, i64)> = sqlx::query_as(
        "SELECT status, valuation_number FROM investment_valuations \
         WHERE id = $1 FOR UPDATE",
    )
    .bind(valuation_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(InvestmentCommandError::Database)?;
    let Some((status, number)) = row else {
        return Err(InvestmentCommandError::NotFound);
    };
    if status != "recorded" {
        tx.rollback()
            .await
            .map_err(InvestmentCommandError::Database)?;
        return Ok((true, number));
    }

    sqlx::query(
        "UPDATE investment_valuations SET status = 'cancelled', \
            cancelled_at = $2, cancelled_by = $3, cancellation_reason = $4, \
            updated_at = $2 \
         WHERE id = $1",
    )
    .bind(valuation_id)
    .bind(now)
    .bind(actor)
    .bind(&reason)
    .execute(tx.as_mut())
    .await
    .map_err(InvestmentCommandError::Database)?;

    tx.commit()
        .await
        .map_err(InvestmentCommandError::Database)?;
    Ok((false, number))
}

// ------------------------------------------------------------------
// Investment cash income (inflow — NOT STEP-010 operational Income)
// ------------------------------------------------------------------

pub struct PostIncome {
    pub investment_id: Uuid,
    pub financial_account_id: Uuid,
    pub amount: Decimal,
    pub occurred_at: OffsetDateTime,
    pub description: String,
    pub counterparty: Option<String>,
    pub reference_no: Option<String>,
    pub idempotency_key: String,
    pub fingerprint: String,
}

/// Post one cash-income event: exactly ONE inflow movement
/// ('investment_income'). No `incomes` row is created — operational
/// income reporting stays untouched (one receipt, one movement).
pub async fn post_income(
    pool: &PgPool,
    actor: Uuid,
    command: PostIncome,
) -> Result<EventOutcome, InvestmentCommandError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(InvestmentCommandError::Database)?;

    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, income_number, idempotency_fingerprint \
         FROM investment_incomes WHERE idempotency_key = $1",
    )
    .bind(&command.idempotency_key)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(InvestmentCommandError::Database)?;
    if let Some((id, number, fingerprint)) = existing {
        if fingerprint != command.fingerprint {
            return Err(InvestmentCommandError::IdempotencyConflict);
        }
        tx.rollback()
            .await
            .map_err(InvestmentCommandError::Database)?;
        return Ok(EventOutcome {
            event_id: id,
            event_number: number,
            replayed: true,
        });
    }

    let status = lock_investment(&mut tx, command.investment_id).await?;
    if status != model::InvestmentStatus::Active.as_str() {
        return Err(InvestmentCommandError::InvalidState);
    }

    let locked = account_repo::lock_accounts(&mut tx, &[command.financial_account_id])
        .await
        .map_err(account_to_error)?;
    let Some((account_status, _currency)) = locked.get(&command.financial_account_id) else {
        return Err(InvestmentCommandError::NotFound);
    };
    if *account_status != AccountStatus::Active.as_str() {
        return Err(InvestmentCommandError::InactiveAccount);
    }

    let income_id = Uuid::new_v4();
    let movement_id = account_repo::insert_movement(
        &mut tx,
        account_repo::NewMovement {
            account_id: command.financial_account_id,
            direction: MovementDirection::Inflow,
            amount: command.amount,
            source: MovementSource::InvestmentIncome,
            source_id: income_id,
            occurred_at: command.occurred_at,
            actor,
        },
    )
    .await
    .map_err(account_to_error)?;

    let inserted: Option<(Uuid, i64)> = sqlx::query_as(
        "INSERT INTO investment_incomes \
            (id, investment_id, financial_account_id, amount, currency, \
             occurred_at, description, counterparty, reference_no, \
             account_movement_id, status, idempotency_key, \
             idempotency_fingerprint, created_by) \
         VALUES ($1, $2, $3, $4, 'TRY', $5, $6, $7, $8, $9, 'posted', $10, $11, $12) \
         ON CONFLICT (idempotency_key) DO NOTHING \
         RETURNING id, income_number",
    )
    .bind(income_id)
    .bind(command.investment_id)
    .bind(command.financial_account_id)
    .bind(command.amount)
    .bind(command.occurred_at)
    .bind(&command.description)
    .bind(&command.counterparty)
    .bind(&command.reference_no)
    .bind(movement_id)
    .bind(&command.idempotency_key)
    .bind(&command.fingerprint)
    .bind(actor)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(InvestmentCommandError::Database)?;

    let Some((id, number)) = inserted else {
        tx.rollback()
            .await
            .map_err(InvestmentCommandError::Database)?;
        let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
            "SELECT id, income_number, idempotency_fingerprint \
             FROM investment_incomes WHERE idempotency_key = $1",
        )
        .bind(&command.idempotency_key)
        .fetch_optional(pool)
        .await
        .map_err(InvestmentCommandError::Database)?;
        let Some((id, number, fingerprint)) = existing else {
            return Err(InvestmentCommandError::NotFound);
        };
        if fingerprint != command.fingerprint {
            return Err(InvestmentCommandError::IdempotencyConflict);
        }
        return Ok(EventOutcome {
            event_id: id,
            event_number: number,
            replayed: true,
        });
    };

    tx.commit()
        .await
        .map_err(InvestmentCommandError::Database)?;
    Ok(EventOutcome {
        event_id: id,
        event_number: number,
        replayed: false,
    })
}

/// Income reversal removes an inflow — refused if the account can no
/// longer fund the removal (docs/19 reversal safety, same rule as
/// STEP-010 Income).
pub async fn reverse_income(
    pool: &PgPool,
    actor: Uuid,
    income_id: Uuid,
    reason: String,
    now: OffsetDateTime,
) -> Result<(bool, i64), InvestmentCommandError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(InvestmentCommandError::Database)?;

    let row: Option<(String, i64, Uuid, Decimal)> = sqlx::query_as(
        "SELECT status, income_number, financial_account_id, amount \
         FROM investment_incomes WHERE id = $1 FOR UPDATE",
    )
    .bind(income_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(InvestmentCommandError::Database)?;
    let Some((status, number, account_id, amount)) = row else {
        return Err(InvestmentCommandError::NotFound);
    };
    if status != "posted" {
        tx.rollback()
            .await
            .map_err(InvestmentCommandError::Database)?;
        return Ok((true, number));
    }

    account_repo::lock_accounts(&mut tx, &[account_id])
        .await
        .map_err(account_to_error)?;

    let balance = account_repo::locked_balance(&mut tx, account_id)
        .await
        .map_err(account_to_error)?;
    if balance - amount < Decimal::ZERO {
        return Err(InvestmentCommandError::InsufficientFunds);
    }

    account_repo::reverse_movements_of_source(
        &mut tx,
        MovementSource::InvestmentIncome,
        income_id,
        now,
        actor,
        &reason,
    )
    .await
    .map_err(account_to_error)?;

    sqlx::query(
        "UPDATE investment_incomes SET status = 'reversed', \
            reversed_at = $2, reversed_by = $3, reversal_reason = $4, updated_at = $2 \
         WHERE id = $1",
    )
    .bind(income_id)
    .bind(now)
    .bind(actor)
    .bind(&reason)
    .execute(tx.as_mut())
    .await
    .map_err(InvestmentCommandError::Database)?;

    tx.commit()
        .await
        .map_err(InvestmentCommandError::Database)?;
    Ok((false, number))
}

// ------------------------------------------------------------------
// Disposal (derecognition + actual proceeds legs)
// ------------------------------------------------------------------

pub struct ProceedsLeg {
    pub financial_account_id: Uuid,
    pub amount: Decimal,
    pub occurred_at: OffsetDateTime,
    pub reference: Option<String>,
}

pub struct DisposeInvestment {
    pub investment_id: Uuid,
    pub disposed_at: Date,
    pub consideration_amount: Option<Decimal>,
    pub counterparty_name: Option<String>,
    pub reference: Option<String>,
    pub note: Option<String>,
    pub proceeds: Vec<ProceedsLeg>,
    pub idempotency_key: String,
    pub fingerprint: String,
}

/// Dispose the investment: ONE derecognition event + one inflow
/// movement per proceeds leg ('investment_disposal'), then the
/// Investment flips to `disposed` in the same transaction. Terminal in
/// STEP-012 — disposal reversal semantics are undefined (docs/08 open
/// decisions) and deliberately not implemented.
pub async fn dispose_investment(
    pool: &PgPool,
    actor: Uuid,
    command: DisposeInvestment,
    now: OffsetDateTime,
) -> Result<EventOutcome, InvestmentCommandError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(InvestmentCommandError::Database)?;

    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, disposal_number, idempotency_fingerprint \
         FROM investment_disposals WHERE idempotency_key = $1",
    )
    .bind(&command.idempotency_key)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(InvestmentCommandError::Database)?;
    if let Some((id, number, fingerprint)) = existing {
        if fingerprint != command.fingerprint {
            return Err(InvestmentCommandError::IdempotencyConflict);
        }
        tx.rollback()
            .await
            .map_err(InvestmentCommandError::Database)?;
        return Ok(EventOutcome {
            event_id: id,
            event_number: number,
            replayed: true,
        });
    }

    // 1. Lock the Investment: only an ACTIVE investment can be disposed.
    let status = lock_investment(&mut tx, command.investment_id).await?;
    if status != model::InvestmentStatus::Active.as_str() {
        return Err(InvestmentCommandError::InvalidState);
    }

    // 2. Lock every proceeds account in canonical ascending-id order.
    let account_ids: Vec<Uuid> = command
        .proceeds
        .iter()
        .map(|leg| leg.financial_account_id)
        .collect();
    let locked = account_repo::lock_accounts(&mut tx, &account_ids)
        .await
        .map_err(account_to_error)?;
    if account_ids.iter().any(|id| !locked.contains_key(id)) {
        return Err(InvestmentCommandError::NotFound);
    }
    for leg in &command.proceeds {
        let (account_status, _currency) = &locked[&leg.financial_account_id];
        if *account_status != AccountStatus::Active.as_str() {
            return Err(InvestmentCommandError::InactiveAccount);
        }
    }

    // 3. Insert the derecognition event, then each proceeds leg with
    //    its authoritative inflow movement.
    let disposal: Option<(Uuid, i64)> = sqlx::query_as(
        "INSERT INTO investment_disposals \
            (investment_id, disposed_at, consideration_amount, currency, \
             counterparty_name, reference, note, status, \
             idempotency_key, idempotency_fingerprint, created_by) \
         VALUES ($1, $2, $3, 'TRY', $4, $5, $6, 'posted', $7, $8, $9) \
         ON CONFLICT (idempotency_key) DO NOTHING \
         RETURNING id, disposal_number",
    )
    .bind(command.investment_id)
    .bind(command.disposed_at)
    .bind(command.consideration_amount)
    .bind(&command.counterparty_name)
    .bind(&command.reference)
    .bind(&command.note)
    .bind(&command.idempotency_key)
    .bind(&command.fingerprint)
    .bind(actor)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(InvestmentCommandError::Database)?;

    let Some((disposal_id, disposal_number)) = disposal else {
        tx.rollback()
            .await
            .map_err(InvestmentCommandError::Database)?;
        let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
            "SELECT id, disposal_number, idempotency_fingerprint \
             FROM investment_disposals WHERE idempotency_key = $1",
        )
        .bind(&command.idempotency_key)
        .fetch_optional(pool)
        .await
        .map_err(InvestmentCommandError::Database)?;
        let Some((id, number, fingerprint)) = existing else {
            return Err(InvestmentCommandError::NotFound);
        };
        if fingerprint != command.fingerprint {
            return Err(InvestmentCommandError::IdempotencyConflict);
        }
        return Ok(EventOutcome {
            event_id: id,
            event_number: number,
            replayed: true,
        });
    };

    for leg in &command.proceeds {
        let leg_id = Uuid::new_v4();
        let movement_id = account_repo::insert_movement(
            &mut tx,
            account_repo::NewMovement {
                account_id: leg.financial_account_id,
                direction: MovementDirection::Inflow,
                amount: leg.amount,
                source: MovementSource::InvestmentDisposal,
                source_id: leg_id,
                occurred_at: leg.occurred_at,
                actor,
            },
        )
        .await
        .map_err(account_to_error)?;

        sqlx::query(
            "INSERT INTO investment_disposal_proceeds \
                (id, disposal_id, financial_account_id, amount, currency, \
                 occurred_at, reference, account_movement_id) \
             VALUES ($1, $2, $3, $4, 'TRY', $5, $6, $7)",
        )
        .bind(leg_id)
        .bind(disposal_id)
        .bind(leg.financial_account_id)
        .bind(leg.amount)
        .bind(leg.occurred_at)
        .bind(&leg.reference)
        .bind(movement_id)
        .execute(tx.as_mut())
        .await
        .map_err(InvestmentCommandError::Database)?;
    }

    // 4. Lifecycle transition in the same transaction (docs/16).
    sqlx::query(
        "UPDATE investments SET status = 'disposed', disposed_at = $2, \
            disposed_by = $3, updated_at = $2 \
         WHERE id = $1",
    )
    .bind(command.investment_id)
    .bind(now)
    .bind(actor)
    .execute(tx.as_mut())
    .await
    .map_err(InvestmentCommandError::Database)?;

    tx.commit()
        .await
        .map_err(InvestmentCommandError::Database)?;
    Ok(EventOutcome {
        event_id: disposal_id,
        event_number: disposal_number,
        replayed: false,
    })
}
