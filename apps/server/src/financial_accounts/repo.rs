//! Financial Accounts repository (STEP-008): account CRUD, derived
//! balances, movement history and the atomic transfer commands.
//!
//! Balance is NEVER a stored number — every surface derives it from
//! `account_movements` (ADR-003). Every balance-affecting command
//! locks the involved account rows `FOR UPDATE` in ascending id order
//! FIRST, then recomputes balances under the lock (ADR-006): this is
//! what serializes transfers vs payment reversals vs posting.

use std::collections::HashMap;

use rust_decimal::Decimal;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use super::model::{AccountStatus, AccountType, MovementDirection, MovementSource};

// ------------------------------------------------------------------
// Read models
// ------------------------------------------------------------------

/// Account with its DERIVED balance (exact NUMERIC aggregate, never a
/// stored column).
#[derive(Debug, sqlx::FromRow)]
pub struct FinancialAccountRow {
    pub id: Uuid,
    pub name: String,
    pub account_type: String,
    pub currency: String,
    pub status: String,
    pub description: Option<String>,
    pub bank_name: Option<String>,
    pub iban: Option<String>,
    pub balance: Decimal,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub total_count: i64,
}

const BALANCE_SQL: &str = "COALESCE(( \
    SELECT sum(CASE m.direction WHEN 'inflow' THEN m.amount ELSE -m.amount END) \
    FROM account_movements m \
    WHERE m.account_id = a.id AND m.status = 'active' \
), 0)";

const ACCOUNT_SELECT: &str = "SELECT a.id, a.name, a.account_type, a.currency, a.status, \
    a.description, a.bank_name, a.iban, \
    {BALANCE_SQL} AS balance, \
    a.created_at, a.updated_at, \
    count(*) OVER() AS total_count \
FROM financial_accounts a";

fn account_select() -> String {
    ACCOUNT_SELECT.replace("{BALANCE_SQL}", BALANCE_SQL)
}

/// Account list: substring over the name, optional type/status filters.
/// Stable ordering by name + id (ADR-010).
pub async fn list_accounts(
    pool: &PgPool,
    search: Option<&str>,
    account_type: Option<AccountType>,
    status: Option<AccountStatus>,
    page: i64,
    page_size: i64,
) -> Result<Vec<FinancialAccountRow>, sqlx::Error> {
    let pattern = search.map(|s| format!("%{}%", s.trim().to_lowercase()));
    sqlx::query_as::<_, FinancialAccountRow>(&format!(
        "{account_select} \
        WHERE ($1::text IS NULL OR lower(a.name) LIKE $1) \
          AND ($2::text IS NULL OR a.account_type = $2) \
          AND ($3::text IS NULL OR a.status = $3) \
        ORDER BY a.name, a.id LIMIT $4 OFFSET $5",
        account_select = account_select()
    ))
    .bind(pattern)
    .bind(account_type.map(|t| t.as_str()))
    .bind(status.map(|s| s.as_str()))
    .bind(page_size)
    .bind((page - 1) * page_size)
    .fetch_all(pool)
    .await
}

/// The set of accounts an operator may pick as a Payment destination
/// or Transfer endpoint: active only, stable order.
#[derive(Debug, sqlx::FromRow)]
pub struct AccountOptionRow {
    pub id: Uuid,
    pub name: String,
    pub account_type: String,
    pub currency: String,
    pub balance: Decimal,
}

pub async fn list_active_account_options(
    pool: &PgPool,
) -> Result<Vec<AccountOptionRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.id, a.name, a.account_type, a.currency, \
            COALESCE(( \
                SELECT sum(CASE m.direction WHEN 'inflow' THEN m.amount ELSE -m.amount END) \
                FROM account_movements m \
                WHERE m.account_id = a.id AND m.status = 'active' \
            ), 0) AS balance \
        FROM financial_accounts a \
        WHERE a.status = 'active' \
        ORDER BY a.name, a.id",
    )
    .fetch_all(pool)
    .await
}

pub async fn get_account(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<FinancialAccountRow>, sqlx::Error> {
    sqlx::query_as::<_, FinancialAccountRow>(&format!(
        "{account_select} WHERE a.id = $1",
        account_select = account_select()
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// One movement line with resolved human-citable source number
/// (Tahsilat No / Transfer No — provenance is relational, the number
/// is only a label).
#[derive(Debug, sqlx::FromRow)]
pub struct AccountMovementRow {
    pub id: Uuid,
    pub account_id: Uuid,
    pub direction: String,
    pub amount: Decimal,
    pub source_type: String,
    pub source_id: Uuid,
    pub source_number: Option<i64>,
    pub occurred_at: OffsetDateTime,
    pub status: String,
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
    pub created_at: OffsetDateTime,
    pub total_count: i64,
}

/// Bounded movement history for one account (ADR-010 offset
/// pagination; ordering by business time then id).
pub async fn list_movements(
    pool: &PgPool,
    account_id: Uuid,
    page: i64,
    page_size: i64,
) -> Result<Vec<AccountMovementRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT m.id, m.account_id, m.direction, m.amount, m.source_type, m.source_id, \
            COALESCE(p.payment_number, t.transfer_number, i.income_number, e.expense_number, \
                s.settlement_number, ivf.funding_number, ivn.income_number, \
                ivd.disposal_number, sad.donation_number, say.disbursement_number) \
                AS source_number, \
            m.occurred_at, m.status, m.reversed_at, m.reversal_reason, m.created_at, \
            count(*) OVER() AS total_count \
        FROM account_movements m \
        LEFT JOIN payments p ON m.source_type = 'payment' AND m.source_id = p.id \
        LEFT JOIN account_transfers t ON m.source_type = 'transfer' AND m.source_id = t.id \
        LEFT JOIN income_entries i ON m.source_type = 'income' AND m.source_id = i.id \
        LEFT JOIN expense_entries e ON m.source_type = 'expense' AND m.source_id = e.id \
        LEFT JOIN share_return_settlements s \
            ON m.source_type = 'share_return_settlement' AND m.source_id = s.id \
        LEFT JOIN investment_fundings ivf \
            ON m.source_type = 'investment_funding' AND m.source_id = ivf.id \
        LEFT JOIN investment_incomes ivn \
            ON m.source_type = 'investment_income' AND m.source_id = ivn.id \
        LEFT JOIN investment_disposal_proceeds ivp \
            ON m.source_type = 'investment_disposal' AND m.source_id = ivp.id \
        LEFT JOIN investment_disposals ivd ON ivd.id = ivp.disposal_id \
        LEFT JOIN social_aid_donations sad \
            ON m.source_type = 'social_aid_donation' AND m.source_id = sad.id \
        LEFT JOIN social_aid_disbursements say \
            ON m.source_type = 'social_aid_disbursement' AND m.source_id = say.id \
        WHERE m.account_id = $1 \
        ORDER BY m.occurred_at DESC, m.id LIMIT $2 OFFSET $3",
    )
    .bind(account_id)
    .bind(page_size)
    .bind((page - 1) * page_size)
    .fetch_all(pool)
    .await
}

/// The movement a Payment produced, for payment-detail surfaces.
#[derive(Debug, sqlx::FromRow)]
pub struct PaymentMovementRow {
    pub id: Uuid,
    pub account_id: Uuid,
    pub account_name: String,
    pub direction: String,
    pub amount: Decimal,
    pub status: String,
}

pub async fn movement_of_payment(
    pool: &PgPool,
    payment_id: Uuid,
) -> Result<Option<PaymentMovementRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT m.id, m.account_id, a.name AS account_name, m.direction, m.amount, m.status \
        FROM account_movements m \
        JOIN financial_accounts a ON a.id = m.account_id \
        WHERE m.source_type = 'payment' AND m.source_id = $1",
    )
    .bind(payment_id)
    .fetch_optional(pool)
    .await
}

/// Transfer with resolved account names.
#[derive(Debug, sqlx::FromRow)]
pub struct AccountTransferRow {
    pub id: Uuid,
    pub transfer_number: i64,
    pub source_account_id: Uuid,
    pub source_account_name: String,
    pub destination_account_id: Uuid,
    pub destination_account_name: String,
    pub amount: Decimal,
    pub currency: String,
    pub occurred_at: OffsetDateTime,
    pub note: Option<String>,
    pub status: String,
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
    pub created_at: OffsetDateTime,
    pub total_count: i64,
}

pub async fn get_transfer(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<AccountTransferRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT t.id, t.transfer_number, \
            t.source_account_id, sa.name AS source_account_name, \
            t.destination_account_id, da.name AS destination_account_name, \
            t.amount, t.currency, t.occurred_at, t.note, t.status, \
            t.reversed_at, t.reversal_reason, t.created_at, \
            0::bigint AS total_count \
        FROM account_transfers t \
        JOIN financial_accounts sa ON sa.id = t.source_account_id \
        JOIN financial_accounts da ON da.id = t.destination_account_id \
        WHERE t.id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// Bounded transfer list, newest business time first.
pub async fn list_transfers(
    pool: &PgPool,
    page: i64,
    page_size: i64,
) -> Result<Vec<AccountTransferRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT t.id, t.transfer_number, \
            t.source_account_id, sa.name AS source_account_name, \
            t.destination_account_id, da.name AS destination_account_name, \
            t.amount, t.currency, t.occurred_at, t.note, t.status, \
            t.reversed_at, t.reversal_reason, t.created_at, \
            count(*) OVER() AS total_count \
        FROM account_transfers t \
        JOIN financial_accounts sa ON sa.id = t.source_account_id \
        JOIN financial_accounts da ON da.id = t.destination_account_id \
        ORDER BY t.transfer_number DESC LIMIT $1 OFFSET $2",
    )
    .bind(page_size)
    .bind((page - 1) * page_size)
    .fetch_all(pool)
    .await
}

/// Cooperative-wide TRY total — only sums like-currency accounts
/// (every account is TRY in STEP-008; the GROUP BY keeps the rule
/// explicit for future value types).
pub async fn total_balance_by_currency(
    pool: &PgPool,
) -> Result<Vec<(String, Decimal)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.currency, COALESCE(sum( \
            CASE m.direction WHEN 'inflow' THEN m.amount ELSE -m.amount END \
        ), 0) AS balance \
        FROM financial_accounts a \
        LEFT JOIN account_movements m ON m.account_id = a.id AND m.status = 'active' \
        GROUP BY a.currency ORDER BY a.currency",
    )
    .fetch_all(pool)
    .await
}

// ------------------------------------------------------------------
// Commands
// ------------------------------------------------------------------

#[derive(Debug)]
pub enum AccountCommandError {
    NotFound,
    /// Posting/reversal rejected: the account is not `active`.
    InactiveAccount,
    /// Transfer target equals source (DB CHECK also enforces this).
    SameAccount,
    /// Account currencies differ (TRY-only today; kept explicit).
    CurrencyMismatch,
    /// Effect would take the account below zero — negative balances
    /// are forbidden by the approved operator decision.
    InsufficientFunds,
    /// Idempotency key reused with a different payload.
    IdempotencyConflict,
    /// Optimistic-concurrency precondition failed (updated_at moved).
    StaleState,
    /// Transfer is already reversed — replayed by routes as success;
    /// surfaced here only where a plain error mapping is needed.
    NotPosted,
    Database(sqlx::Error),
}

pub struct CreateAccount {
    pub name: String,
    pub account_type: AccountType,
    pub description: Option<String>,
    pub bank_name: Option<String>,
    pub iban: Option<String>,
}

pub async fn create_account(
    pool: &PgPool,
    actor: Uuid,
    command: CreateAccount,
) -> Result<Uuid, AccountCommandError> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO financial_accounts \
            (name, account_type, currency, status, description, bank_name, iban, created_by) \
         VALUES ($1, $2, 'TRY', 'active', $3, $4, $5, $6) RETURNING id",
    )
    .bind(&command.name)
    .bind(command.account_type.as_str())
    .bind(&command.description)
    .bind(&command.bank_name)
    .bind(&command.iban)
    .bind(actor)
    .fetch_one(pool)
    .await
    .map_err(AccountCommandError::Database)?;
    Ok(id)
}

/// Safe metadata update only — `account_type` and `currency` are
/// IMMUTABLE at creation: changing them after movements would silently
/// rewrite financial history (docs/19).
pub async fn update_account(
    pool: &PgPool,
    id: Uuid,
    name: String,
    description: Option<String>,
    bank_name: Option<String>,
    iban: Option<String>,
    expected_updated_at: OffsetDateTime,
) -> Result<(), AccountCommandError> {
    let updated = sqlx::query(
        "UPDATE financial_accounts SET \
            name = $2, description = $3, bank_name = $4, iban = $5, updated_at = now() \
         WHERE id = $1 AND updated_at = $6",
    )
    .bind(id)
    .bind(&name)
    .bind(&description)
    .bind(&bank_name)
    .bind(&iban)
    .bind(expected_updated_at)
    .execute(pool)
    .await
    .map_err(AccountCommandError::Database)?;
    if updated.rows_affected() == 0 {
        let exists: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM financial_accounts WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await
                .map_err(AccountCommandError::Database)?;
        return match exists {
            Some(_) => Err(AccountCommandError::StaleState),
            None => Err(AccountCommandError::NotFound),
        };
    }
    Ok(())
}

/// Controlled lifecycle command: active <-> inactive. History, balance
/// and movements are untouched — only NEW postings are refused.
pub async fn change_account_status(
    pool: &PgPool,
    id: Uuid,
    target: AccountStatus,
) -> Result<AccountStatus, AccountCommandError> {
    let previous: Option<String> = sqlx::query_scalar(
        "UPDATE financial_accounts SET status = $2, updated_at = now() \
         WHERE id = $1 RETURNING status",
    )
    .bind(id)
    .bind(target.as_str())
    .fetch_optional(pool)
    .await
    .map_err(AccountCommandError::Database)?;
    match previous {
        Some(_) => Ok(target),
        None => Err(AccountCommandError::NotFound),
    }
}

// ------------------------------------------------------------------
// Shared transaction helpers (also used by the payments module)
// ------------------------------------------------------------------

/// Lock account rows `FOR UPDATE` in ascending id order — the single
/// deterministic lock prefix every balance-affecting command shares.
/// Returns (id -> (status, currency)).
pub async fn lock_accounts(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, (String, String)>, AccountCommandError> {
    let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT id, status, currency FROM financial_accounts \
         WHERE id = ANY($1) ORDER BY id FOR UPDATE",
    )
    .bind(ids)
    .fetch_all(tx.as_mut())
    .await
    .map_err(AccountCommandError::Database)?;
    Ok(rows.into_iter().map(|(id, s, c)| (id, (s, c))).collect())
}

/// Derived balance of an account — call ONLY after lock_accounts so
/// the value is stable inside the command transaction.
pub async fn locked_balance(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account_id: Uuid,
) -> Result<Decimal, AccountCommandError> {
    sqlx::query_scalar::<_, Option<Decimal>>(
        "SELECT sum(CASE direction WHEN 'inflow' THEN amount ELSE -amount END) \
         FROM account_movements WHERE account_id = $1 AND status = 'active'",
    )
    .bind(account_id)
    .fetch_one(tx.as_mut())
    .await
    .map(|balance| balance.unwrap_or_default())
    .map_err(AccountCommandError::Database)
}

/// Payload of a new account movement — kept as a struct so every leg
/// carries the same fields (payment posting and both transfer legs).
pub struct NewMovement {
    pub account_id: Uuid,
    pub direction: MovementDirection,
    pub amount: Decimal,
    pub source: MovementSource,
    pub source_id: Uuid,
    pub occurred_at: OffsetDateTime,
    pub actor: Uuid,
}

/// Insert a movement inside the caller's transaction.
/// The caller must hold the account row lock and have validated the
/// account is active.
pub async fn insert_movement(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    movement: NewMovement,
) -> Result<Uuid, AccountCommandError> {
    sqlx::query_scalar(
        "INSERT INTO account_movements \
            (account_id, direction, amount, source_type, source_id, occurred_at, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(movement.account_id)
    .bind(movement.direction.as_str())
    .bind(movement.amount)
    .bind(movement.source.as_str())
    .bind(movement.source_id)
    .bind(movement.occurred_at)
    .bind(movement.actor)
    .fetch_one(tx.as_mut())
    .await
    .map_err(AccountCommandError::Database)
}

/// Status-based movement reversal inside the caller's transaction
/// (docs/19: original preserved, actor/time/reason recorded). Returns
/// the number of legs reversed.
pub async fn reverse_movements_of_source(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    source: MovementSource,
    source_id: Uuid,
    now: OffsetDateTime,
    actor: Uuid,
    reason: &str,
) -> Result<u64, AccountCommandError> {
    let affected = sqlx::query(
        "UPDATE account_movements SET status = 'reversed', \
            reversed_at = $3, reversed_by = $4, reversal_reason = $5 \
         WHERE source_type = $1 AND source_id = $2 AND status = 'active'",
    )
    .bind(source.as_str())
    .bind(source_id)
    .bind(now)
    .bind(actor)
    .bind(reason)
    .execute(tx.as_mut())
    .await
    .map_err(AccountCommandError::Database)?;
    Ok(affected.rows_affected())
}

// ------------------------------------------------------------------
// Transfer command (docs/07 §transfer): ONE logical move — two linked
// movement legs, or none at all.
// ------------------------------------------------------------------

pub struct PostTransfer {
    pub source_account_id: Uuid,
    pub destination_account_id: Uuid,
    pub amount: Decimal,
    pub occurred_at: OffsetDateTime,
    pub note: Option<String>,
    pub idempotency_key: String,
    pub fingerprint: String,
}

pub struct TransferOutcome {
    pub transfer_id: Uuid,
    pub transfer_number: i64,
    pub replayed: bool,
}

/// BEGIN -> idempotent check -> lock accounts sorted -> validate
/// (active / distinct / same currency / sufficient source balance)
/// -> insert transfer -> insert both legs -> COMMIT.
pub async fn post_transfer(
    pool: &PgPool,
    actor: Uuid,
    command: PostTransfer,
) -> Result<TransferOutcome, AccountCommandError> {
    if command.source_account_id == command.destination_account_id {
        return Err(AccountCommandError::SameAccount);
    }
    let mut tx = pool.begin().await.map_err(AccountCommandError::Database)?;

    // 1. Idempotent replay short-circuit.
    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, transfer_number, idempotency_fingerprint FROM account_transfers \
         WHERE idempotency_key = $1",
    )
    .bind(&command.idempotency_key)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(AccountCommandError::Database)?;
    if let Some((id, number, fingerprint)) = existing {
        if fingerprint != command.fingerprint {
            return Err(AccountCommandError::IdempotencyConflict);
        }
        tx.rollback().await.map_err(AccountCommandError::Database)?;
        return Ok(TransferOutcome {
            transfer_id: id,
            transfer_number: number,
            replayed: true,
        });
    }

    // 2. Deterministic lock order: ascending account id.
    let mut ids = [command.source_account_id, command.destination_account_id];
    ids.sort_unstable();
    let locked = lock_accounts(&mut tx, &ids).await?;
    let Some((source_status, source_currency)) = locked.get(&command.source_account_id) else {
        return Err(AccountCommandError::NotFound);
    };
    let Some((destination_status, destination_currency)) =
        locked.get(&command.destination_account_id)
    else {
        return Err(AccountCommandError::NotFound);
    };
    if *source_status != AccountStatus::Active.as_str()
        || *destination_status != AccountStatus::Active.as_str()
    {
        return Err(AccountCommandError::InactiveAccount);
    }
    if source_currency != destination_currency {
        return Err(AccountCommandError::CurrencyMismatch);
    }

    // 3. Negative balances are forbidden: recompute under the locks.
    let source_balance = locked_balance(&mut tx, command.source_account_id).await?;
    if command.amount > source_balance {
        return Err(AccountCommandError::InsufficientFunds);
    }

    // 4. Transfer + the two paired legs — one transaction, or nothing.
    let inserted: Option<(Uuid, i64)> = sqlx::query_as(
        "INSERT INTO account_transfers \
            (source_account_id, destination_account_id, amount, currency, \
             occurred_at, note, status, idempotency_key, idempotency_fingerprint, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, 'posted', $7, $8, $9) \
         ON CONFLICT (idempotency_key) DO NOTHING \
         RETURNING id, transfer_number",
    )
    .bind(command.source_account_id)
    .bind(command.destination_account_id)
    .bind(command.amount)
    .bind(source_currency)
    .bind(command.occurred_at)
    .bind(&command.note)
    .bind(&command.idempotency_key)
    .bind(&command.fingerprint)
    .bind(actor)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(AccountCommandError::Database)?;

    let (transfer_id, transfer_number) = match inserted {
        Some(row) => row,
        None => {
            tx.rollback().await.map_err(AccountCommandError::Database)?;
            let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
                "SELECT id, transfer_number, idempotency_fingerprint FROM account_transfers \
                 WHERE idempotency_key = $1",
            )
            .bind(&command.idempotency_key)
            .fetch_optional(pool)
            .await
            .map_err(AccountCommandError::Database)?;
            let Some((id, number, fingerprint)) = existing else {
                return Err(AccountCommandError::NotFound);
            };
            if fingerprint != command.fingerprint {
                return Err(AccountCommandError::IdempotencyConflict);
            }
            return Ok(TransferOutcome {
                transfer_id: id,
                transfer_number: number,
                replayed: true,
            });
        }
    };

    insert_movement(
        &mut tx,
        NewMovement {
            account_id: command.source_account_id,
            direction: MovementDirection::Outflow,
            amount: command.amount,
            source: MovementSource::Transfer,
            source_id: transfer_id,
            occurred_at: command.occurred_at,
            actor,
        },
    )
    .await?;
    insert_movement(
        &mut tx,
        NewMovement {
            account_id: command.destination_account_id,
            direction: MovementDirection::Inflow,
            amount: command.amount,
            source: MovementSource::Transfer,
            source_id: transfer_id,
            occurred_at: command.occurred_at,
            actor,
        },
    )
    .await?;

    tx.commit().await.map_err(AccountCommandError::Database)?;
    Ok(TransferOutcome {
        transfer_id,
        transfer_number,
        replayed: false,
    })
}

/// Transfer reversal (docs/19): transfer + both legs marked reversed
/// with actor/time/reason — the original survives. Idempotent replay.
/// A reversal that would take either account below zero is rejected:
/// the money physically left; restore it first, then reverse.
pub async fn reverse_transfer(
    pool: &PgPool,
    actor: Uuid,
    transfer_id: Uuid,
    reason: String,
    now: OffsetDateTime,
) -> Result<(bool, i64), AccountCommandError> {
    let mut tx = pool.begin().await.map_err(AccountCommandError::Database)?;

    let transfer: Option<(String, i64, Uuid, Uuid, Decimal)> = sqlx::query_as(
        "SELECT status, transfer_number, source_account_id, destination_account_id, amount \
         FROM account_transfers WHERE id = $1 FOR UPDATE",
    )
    .bind(transfer_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(AccountCommandError::Database)?;
    let Some((status, transfer_number, source_id, destination_id, amount)) = transfer else {
        return Err(AccountCommandError::NotFound);
    };
    if status != "posted" {
        tx.rollback().await.map_err(AccountCommandError::Database)?;
        return Ok((true, transfer_number));
    }

    // Lock both accounts (same deterministic order). Reversal removes
    // the DESTINATION inflow (source gains back — it cannot go
    // negative): if the destination already paid the value out, the
    // reversal would fabricate a negative balance — reject (operator
    // restores funds first, then reverses).
    let mut ids = [source_id, destination_id];
    ids.sort_unstable();
    lock_accounts(&mut tx, &ids).await?;
    if locked_balance(&mut tx, destination_id).await? - amount < Decimal::ZERO {
        return Err(AccountCommandError::InsufficientFunds);
    }

    reverse_movements_of_source(
        &mut tx,
        MovementSource::Transfer,
        transfer_id,
        now,
        actor,
        &reason,
    )
    .await?;

    sqlx::query(
        "UPDATE account_transfers SET status = 'reversed', \
            reversed_at = $2, reversed_by = $3, reversal_reason = $4, updated_at = $2 \
         WHERE id = $1",
    )
    .bind(transfer_id)
    .bind(now)
    .bind(actor)
    .bind(&reason)
    .execute(tx.as_mut())
    .await
    .map_err(AccountCommandError::Database)?;

    tx.commit().await.map_err(AccountCommandError::Database)?;
    Ok((false, transfer_number))
}
