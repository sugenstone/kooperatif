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
///
/// `cooperative_id` is the verified business context (M1-P2-C3):
/// `Some` scopes to that cooperative; `None` is reachable only in the
/// proven never-bootstrapped lane and matches only legacy NULL rows
/// (`IS NOT DISTINCT FROM`) — never a global listing.
pub async fn list_accounts(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    search: Option<&str>,
    account_type: Option<AccountType>,
    status: Option<AccountStatus>,
    page: i64,
    page_size: i64,
) -> Result<Vec<FinancialAccountRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    let pattern = search.map(|s| format!("%{}%", s.trim().to_lowercase()));
    sqlx::query_as::<_, FinancialAccountRow>(&format!(
        "{account_select} \
        WHERE ($1::text IS NULL OR lower(a.name) LIKE $1) \
          AND ($2::text IS NULL OR a.account_type = $2) \
          AND ($3::text IS NULL OR a.status = $3) \
          AND a.cooperative_id IS NOT DISTINCT FROM $6 \
        ORDER BY a.name, a.id LIMIT $4 OFFSET $5",
        account_select = account_select()
    ))
    .bind(pattern)
    .bind(account_type.map(|t| t.as_str()))
    .bind(status.map(|s| s.as_str()))
    .bind(page_size)
    .bind((page - 1) * page_size)
    .bind(cooperative_id)
    .fetch_all(&mut *conn)
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
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
) -> Result<Vec<AccountOptionRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    sqlx::query_as(
        "SELECT a.id, a.name, a.account_type, a.currency, \
            COALESCE(( \
                SELECT sum(CASE m.direction WHEN 'inflow' THEN m.amount ELSE -m.amount END) \
                FROM account_movements m \
                WHERE m.account_id = a.id AND m.status = 'active' \
            ), 0) AS balance \
        FROM financial_accounts a \
        WHERE a.status = 'active' \
          AND a.cooperative_id IS NOT DISTINCT FROM $1 \
        ORDER BY a.name, a.id",
    )
    .bind(cooperative_id)
    .fetch_all(&mut *conn)
    .await
}

/// Cooperative-scoped detail (C3): foreign account ids return `None`.
pub async fn get_account(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    id: Uuid,
) -> Result<Option<FinancialAccountRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    sqlx::query_as::<_, FinancialAccountRow>(&format!(
        "{account_select} WHERE a.id = $1 \
         AND a.cooperative_id IS NOT DISTINCT FROM $2",
        account_select = account_select()
    ))
    .bind(id)
    .bind(cooperative_id)
    .fetch_optional(&mut *conn)
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
/// Cooperative-scoped (C3): movements of foreign accounts are
/// invisible. The polymorphic `source_type`/`source_id` joins are the
/// documented C2 exception (no FK possible): every source label join
/// additionally demands the SOURCE row live in the same scope, so a
/// malformed or foreign-cooperative source_id can never leak a
/// business number across the boundary.
pub async fn list_movements(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    account_id: Uuid,
    page: i64,
    page_size: i64,
) -> Result<Vec<AccountMovementRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
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
            AND p.cooperative_id IS NOT DISTINCT FROM $4 \
        LEFT JOIN account_transfers t ON m.source_type = 'transfer' AND m.source_id = t.id \
            AND t.cooperative_id IS NOT DISTINCT FROM $4 \
        LEFT JOIN income_entries i ON m.source_type = 'income' AND m.source_id = i.id \
            AND i.cooperative_id IS NOT DISTINCT FROM $4 \
        LEFT JOIN expense_entries e ON m.source_type = 'expense' AND m.source_id = e.id \
            AND e.cooperative_id IS NOT DISTINCT FROM $4 \
        LEFT JOIN share_return_settlements s \
            ON m.source_type = 'share_return_settlement' AND m.source_id = s.id \
            AND s.cooperative_id IS NOT DISTINCT FROM $4 \
        LEFT JOIN investment_fundings ivf \
            ON m.source_type = 'investment_funding' AND m.source_id = ivf.id \
            AND ivf.cooperative_id IS NOT DISTINCT FROM $4 \
        LEFT JOIN investment_incomes ivn \
            ON m.source_type = 'investment_income' AND m.source_id = ivn.id \
            AND ivn.cooperative_id IS NOT DISTINCT FROM $4 \
        LEFT JOIN investment_disposal_proceeds ivp \
            ON m.source_type = 'investment_disposal' AND m.source_id = ivp.id \
            AND ivp.cooperative_id IS NOT DISTINCT FROM $4 \
        LEFT JOIN investment_disposals ivd ON ivd.id = ivp.disposal_id \
            AND ivd.cooperative_id IS NOT DISTINCT FROM $4 \
        LEFT JOIN social_aid_donations sad \
            ON m.source_type = 'social_aid_donation' AND m.source_id = sad.id \
            AND sad.cooperative_id IS NOT DISTINCT FROM $4 \
        LEFT JOIN social_aid_disbursements say \
            ON m.source_type = 'social_aid_disbursement' AND m.source_id = say.id \
            AND say.cooperative_id IS NOT DISTINCT FROM $4 \
        WHERE m.account_id = $1 \
          AND m.cooperative_id IS NOT DISTINCT FROM $4 \
        ORDER BY m.occurred_at DESC, m.id LIMIT $2 OFFSET $3",
    )
    .bind(account_id)
    .bind(page_size)
    .bind((page - 1) * page_size)
    .bind(cooperative_id)
    .fetch_all(&mut *conn)
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
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    payment_id: Uuid,
) -> Result<Option<PaymentMovementRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    sqlx::query_as(
        "SELECT m.id, m.account_id, a.name AS account_name, m.direction, m.amount, m.status \
        FROM account_movements m \
        JOIN financial_accounts a ON a.id = m.account_id \
        WHERE m.source_type = 'payment' AND m.source_id = $1 \
          AND m.cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(payment_id)
    .bind(cooperative_id)
    .fetch_optional(&mut *conn)
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

/// Cooperative-scoped detail (C3): foreign transfer ids return `None`.
pub async fn get_transfer(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    id: Uuid,
) -> Result<Option<AccountTransferRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
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
        WHERE t.id = $1 \
          AND t.cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(id)
    .bind(cooperative_id)
    .fetch_optional(&mut *conn)
    .await
}

/// Bounded transfer list, newest business time first.
pub async fn list_transfers(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    page: i64,
    page_size: i64,
) -> Result<Vec<AccountTransferRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
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
        WHERE t.cooperative_id IS NOT DISTINCT FROM $3 \
        ORDER BY t.transfer_number DESC LIMIT $1 OFFSET $2",
    )
    .bind(page_size)
    .bind((page - 1) * page_size)
    .bind(cooperative_id)
    .fetch_all(&mut *conn)
    .await
}

/// Cooperative-wide TRY total — only sums like-currency accounts
/// (every account is TRY in STEP-008; the GROUP BY keeps the rule
/// explicit for future value types).
pub async fn total_balance_by_currency(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
) -> Result<Vec<(String, Decimal)>, sqlx::Error> {
    let mut conn = db.conn().await?;
    sqlx::query_as(
        "SELECT a.currency, COALESCE(sum( \
            CASE m.direction WHEN 'inflow' THEN m.amount ELSE -m.amount END \
        ), 0) AS balance \
        FROM financial_accounts a \
        LEFT JOIN account_movements m ON m.account_id = a.id AND m.status = 'active' \
        WHERE a.cooperative_id IS NOT DISTINCT FROM $1 \
        GROUP BY a.currency ORDER BY a.currency",
    )
    .bind(cooperative_id)
    .fetch_all(&mut *conn)
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
    /// Effect would consume cash reserved for Social Aid funds
    /// (PILOT-FIX-001 hard reservation): the operation stays within
    /// the physical balance but exceeds the UNRESTRICTED portion of it.
    InsufficientUnrestrictedFunds,
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
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    command: CreateAccount,
) -> Result<Uuid, AccountCommandError> {
    let mut conn = db.conn().await.map_err(AccountCommandError::Database)?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO financial_accounts \
            (name, account_type, currency, status, description, bank_name, iban, \
             created_by, cooperative_id) \
         VALUES ($1, $2, 'TRY', 'active', $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(&command.name)
    .bind(command.account_type.as_str())
    .bind(&command.description)
    .bind(&command.bank_name)
    .bind(&command.iban)
    .bind(actor)
    .bind(cooperative_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(AccountCommandError::Database)?;
    Ok(id)
}

/// Safe metadata update only — `account_type` and `currency` are
/// IMMUTABLE at creation: changing them after movements would silently
/// rewrite financial history (docs/19).
#[allow(clippy::too_many_arguments)]
pub async fn update_account(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    id: Uuid,
    name: String,
    description: Option<String>,
    bank_name: Option<String>,
    iban: Option<String>,
    expected_updated_at: OffsetDateTime,
) -> Result<(), AccountCommandError> {
    let mut conn = db.conn().await.map_err(AccountCommandError::Database)?;
    let updated = sqlx::query(
        "UPDATE financial_accounts SET \
            name = $2, description = $3, bank_name = $4, iban = $5, updated_at = now() \
         WHERE id = $1 AND updated_at = $6 \
           AND cooperative_id IS NOT DISTINCT FROM $7",
    )
    .bind(id)
    .bind(&name)
    .bind(&description)
    .bind(&bank_name)
    .bind(&iban)
    .bind(expected_updated_at)
    .bind(cooperative_id)
    .execute(&mut *conn)
    .await
    .map_err(AccountCommandError::Database)?;
    if updated.rows_affected() == 0 {
        // Scoped existence check keeps a foreign account
        // indistinguishable from a missing one (C3 / IDOR rule).
        let exists: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM financial_accounts WHERE id = $1 \
             AND cooperative_id IS NOT DISTINCT FROM $2",
        )
        .bind(id)
        .bind(cooperative_id)
        .fetch_optional(&mut *conn)
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
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    id: Uuid,
    target: AccountStatus,
) -> Result<AccountStatus, AccountCommandError> {
    let mut conn = db.conn().await.map_err(AccountCommandError::Database)?;
    // R3B: status is a lifecycle column — the write goes through the
    // permission-checked definer; the verified scope is re-derived
    // from the sealed context inside the database.
    let updated: bool =
        sqlx::query_scalar("SELECT public.app_financial_account_set_status($1, $2, $3)")
            .bind(id)
            .bind(target.as_str())
            .bind(cooperative_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(AccountCommandError::Database)?;
    match updated {
        true => Ok(target),
        false => Err(AccountCommandError::NotFound),
    }
}

// ------------------------------------------------------------------
// Shared transaction helpers (also used by the payments module)
// ------------------------------------------------------------------

/// Lock account rows `FOR UPDATE` in ascending id order — the single
/// deterministic lock prefix every balance-affecting command shares.
/// Returns (id -> (status, currency, cooperative_id)).
///
/// Cooperative ownership is returned rather than filtered (C3):
/// callers with a verified context compare it against the row's
/// `cooperative_id`, while modules whose own C3 batch has not landed
/// yet derive movement/reversal scope from the LOCKED row itself — a
/// movement always lives in its account's cooperative.
pub async fn lock_accounts(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, (String, String, Option<Uuid>)>, AccountCommandError> {
    let rows: Vec<(Uuid, String, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, status, currency, cooperative_id \
         FROM public.app_lock_accounts($1)",
    )
    .bind(ids)
    .fetch_all(tx.as_mut())
    .await
    .map_err(AccountCommandError::Database)?;
    Ok(rows
        .into_iter()
        .map(|(id, s, c, coop)| (id, (s, c, coop)))
        .collect())
}

/// Derived balance of an account — call ONLY after lock_accounts so
/// the value is stable inside the command transaction. `cooperative_id`
/// is the owning scope of the locked account row (C3): movements are
/// always written in their account's cooperative, so the predicate is
/// a consistency assertion, never a narrowing of legitimate rows.
pub async fn locked_balance(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    cooperative_id: Option<Uuid>,
    account_id: Uuid,
) -> Result<Decimal, AccountCommandError> {
    sqlx::query_scalar::<_, Option<Decimal>>(
        "SELECT sum(CASE direction WHEN 'inflow' THEN amount ELSE -amount END) \
         FROM account_movements WHERE account_id = $1 AND status = 'active' \
           AND cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(account_id)
    .bind(cooperative_id)
    .fetch_one(tx.as_mut())
    .await
    .map(|balance| balance.unwrap_or_default())
    .map_err(AccountCommandError::Database)
}

/// Derived Social Aid reservation attributed to ONE physical account
/// across ALL funds (PILOT-FIX-001 hard reservation, docs/11):
/// `sum(posted donations) - sum(posted disbursements)` bound to this
/// account. The value is derived from the authoritative Social Aid
/// ledger rows — never a stored/cached number — and the Social Aid
/// lifecycle guarantees each (fund, account) pair stays >= 0.
///
/// Call ONLY while holding the account row lock: every writer of the
/// underlying rows (`social_aid` commands) also holds that same lock
/// (canonical prefix fund -> account), so a caller holding the account
/// lock is serialized against every concurrent reservation mutation.
/// C3 note: deliberately NOT cooperative-scoped. Social Aid ledger
/// rows are written by the `social_aid` module whose own C3 batch has
/// not landed yet — legacy/pre-batch rows carry NULL cooperative_id
/// and filtering them out would under-count the reservation and allow
/// over-spending restricted cash. The `account_id` predicate already
/// confines the sum to this account's cooperative; the explicit scope
/// lands with the Social Aid batch.
pub async fn locked_reserved_social_aid(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    cooperative_id: Option<Uuid>,
    account_id: Uuid,
) -> Result<Decimal, AccountCommandError> {
    sqlx::query_scalar::<_, Option<Decimal>>(
        "SELECT \
            COALESCE((SELECT sum(amount) FROM social_aid_donations \
                      WHERE financial_account_id = $1 AND status = 'posted' \
                        AND cooperative_id IS NOT DISTINCT FROM $2), 0) \
          - COALESCE((SELECT sum(amount) FROM social_aid_disbursements \
                      WHERE financial_account_id = $1 AND status = 'posted' \
                        AND cooperative_id IS NOT DISTINCT FROM $2), 0)",
    )
    .bind(account_id)
    .bind(cooperative_id)
    .fetch_one(tx.as_mut())
    .await
    .map(|v| v.unwrap_or_default())
    .map_err(AccountCommandError::Database)
}

/// Ordinary spendable balance of an account under the row lock:
/// `physical_balance - reserved_social_aid`. Every ORDINARY outflow
/// (transfer-out, expense, settlement, investment funding) and every
/// inflow-removing reversal must satisfy `amount <= spendable` /
/// `post_state >= reserved` — restricted money is not cooperative
/// cash (PILOT-FIX-001 user-approved hard reservation).
pub async fn locked_spendable_balance(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    cooperative_id: Option<Uuid>,
    account_id: Uuid,
) -> Result<Decimal, AccountCommandError> {
    let balance = locked_balance(tx, cooperative_id, account_id).await?;
    let reserved = locked_reserved_social_aid(tx, cooperative_id, account_id).await?;
    Ok(balance - reserved)
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
    /// C3: the owning cooperative — always the cooperative of the
    /// account the movement lands on (a movement can never live in a
    /// different cooperative than its account). Callers pass the value
    /// from `lock_accounts` or their verified context.
    pub cooperative_id: Option<Uuid>,
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
            (account_id, direction, amount, source_type, source_id, occurred_at, \
             created_by, cooperative_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(movement.account_id)
    .bind(movement.direction.as_str())
    .bind(movement.amount)
    .bind(movement.source.as_str())
    .bind(movement.source_id)
    .bind(movement.occurred_at)
    .bind(movement.actor)
    .bind(movement.cooperative_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(AccountCommandError::Database)
}

/// Status-based movement reversal inside the caller's transaction
/// (docs/19: original preserved, actor/time/reason recorded). Returns
/// the number of legs reversed.
/// `cooperative_id` is the scope the caller verified for the SOURCE
/// record (C3): polymorphic `source_id` has no FK, so the predicate is
/// the ownership validation — a source_id pointing into another
/// cooperative's rows can never flip foreign movements.
pub async fn reverse_movements_of_source(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    cooperative_id: Option<Uuid>,
    source: MovementSource,
    source_id: Uuid,
    now: OffsetDateTime,
    actor: Uuid,
    reason: &str,
) -> Result<u64, AccountCommandError> {
    // R3B: the reversal write runs inside the permission-checked
    // definer — the source domain's .manage permission is re-verified
    // in the database, the scope comes from the seal, not the caller.
    // `cooperative_id` stays bound only to the callers' scoped locks
    // above; the definer re-derives it from the verified context.
    let affected: i64 =
        sqlx::query_scalar("SELECT public.app_movements_reverse_of_source($1, $2, $3, $4, $5, $6)")
            .bind(source.as_str())
            .bind(source_id)
            .bind(now)
            .bind(actor)
            .bind(reason)
            .bind(cooperative_id)
            .fetch_one(tx.as_mut())
            .await
            .map_err(AccountCommandError::Database)?;
    Ok(affected as u64)
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
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    command: PostTransfer,
) -> Result<TransferOutcome, AccountCommandError> {
    let mut tx = db.begin().await.map_err(AccountCommandError::Database)?;
    if command.source_account_id == command.destination_account_id {
        return Err(AccountCommandError::SameAccount);
    }

    // 1. Idempotent replay short-circuit — scoped so a foreign
    //    cooperative's key never replays one of ITS transfers (C3).
    //    The key itself stays globally unique until C5: a cross-coop
    //    collision lands in the ON CONFLICT re-read below as NotFound.
    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, transfer_number, idempotency_fingerprint FROM account_transfers \
         WHERE idempotency_key = $1 \
           AND cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(&command.idempotency_key)
    .bind(cooperative_id)
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

    // 2. Deterministic lock order: ascending account id. Locked rows
    //    carry their owning cooperative; anything outside the verified
    //    scope is NotFound (IDOR rule, C3).
    let mut ids = [command.source_account_id, command.destination_account_id];
    ids.sort_unstable();
    let locked = lock_accounts(&mut tx, &ids).await?;
    let Some((source_status, source_currency, source_coop)) =
        locked.get(&command.source_account_id)
    else {
        return Err(AccountCommandError::NotFound);
    };
    let Some((destination_status, destination_currency, destination_coop)) =
        locked.get(&command.destination_account_id)
    else {
        return Err(AccountCommandError::NotFound);
    };
    if *source_coop != cooperative_id || *destination_coop != cooperative_id {
        return Err(AccountCommandError::NotFound);
    }
    if *source_status != AccountStatus::Active.as_str()
        || *destination_status != AccountStatus::Active.as_str()
    {
        return Err(AccountCommandError::InactiveAccount);
    }
    if source_currency != destination_currency {
        return Err(AccountCommandError::CurrencyMismatch);
    }

    // 3. Negative balances are forbidden: recompute under the locks.
    //    PILOT-FIX-001 hard reservation: an ordinary transfer may only
    //    spend the UNRESTRICTED portion — Social Aid reservations are
    //    not transferable cooperative cash.
    let source_balance = locked_balance(&mut tx, cooperative_id, command.source_account_id).await?;
    if command.amount > source_balance {
        return Err(AccountCommandError::InsufficientFunds);
    }
    let source_reserved =
        locked_reserved_social_aid(&mut tx, cooperative_id, command.source_account_id).await?;
    if command.amount > source_balance - source_reserved {
        return Err(AccountCommandError::InsufficientUnrestrictedFunds);
    }

    // 4. Transfer + the two paired legs — one transaction, or nothing.
    //    Every new row carries the verified cooperative scope (C3).
    let inserted: Option<(Uuid, i64)> = sqlx::query_as(
        "INSERT INTO account_transfers \
            (source_account_id, destination_account_id, amount, currency, \
             occurred_at, note, status, idempotency_key, idempotency_fingerprint, \
             created_by, cooperative_id, transfer_number) \
         VALUES ($1, $2, $3, $4, $5, $6, 'posted', $7, $8, $9, $10, \
             allocate_document_number($10, 'account_transfers.transfer_number')) \
         ON CONFLICT (cooperative_id, idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING \
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
    .map_err(AccountCommandError::Database)?;

    let (transfer_id, transfer_number) = match inserted {
        Some(row) => row,
        None => {
            let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
                "SELECT id, transfer_number, idempotency_fingerprint FROM account_transfers \
                 WHERE idempotency_key = $1 \
                   AND cooperative_id IS NOT DISTINCT FROM $2",
            )
            .bind(&command.idempotency_key)
            .bind(cooperative_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(AccountCommandError::Database)?;
            tx.rollback().await.map_err(AccountCommandError::Database)?;
            // Scoped re-read: a conflicting row owned by another
            // cooperative is indistinguishable from absence (C3).
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
            cooperative_id,
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
            cooperative_id,
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
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    transfer_id: Uuid,
    reason: String,
    now: OffsetDateTime,
) -> Result<(bool, i64), AccountCommandError> {
    let mut tx = db.begin().await.map_err(AccountCommandError::Database)?;

    // R3C: the complete reversal package (transfer lock + status,
    // both account locks in canonical order, destination balance and
    // Social-Aid reservation checks, both movement legs, status flip)
    // runs as ONE permission-checked definer — the runtime role can
    // no longer invoke a partial reversal.
    let (replayed, transfer_number): (bool, i64) = sqlx::query_as(
        "SELECT replayed, transfer_number \
         FROM public.app_transfer_reverse_tx($1, $2, $3, $4, $5)",
    )
    .bind(transfer_id)
    .bind(now)
    .bind(actor)
    .bind(&reason)
    .bind(cooperative_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(|error| match crate::db::package_error_token(&error) {
        Some("not_found") => AccountCommandError::NotFound,
        Some("insufficient_funds") => AccountCommandError::InsufficientFunds,
        Some("insufficient_unrestricted_funds") => {
            AccountCommandError::InsufficientUnrestrictedFunds
        }
        _ => AccountCommandError::Database(error),
    })?;

    tx.commit().await.map_err(AccountCommandError::Database)?;
    Ok((replayed, transfer_number))
}
