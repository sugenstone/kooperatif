//! Income/Expense repository (STEP-010): category management and the
//! atomic post/reverse commands.
//!
//! ONE shared engine serves Income and Expense — the `EntryKind`
//! decides only the table, the category type and the movement
//! direction. Every posting is:
//!
//!   BEGIN -> idempotent replay check -> lock account FOR UPDATE ->
//!   lock category FOR UPDATE -> validate (active account, matching
//!   active category, positive amount, funds for outflow) ->
//!   insert movement -> insert entry -> COMMIT
//!
//! so an entry can never exist without its movement and vice versa.
//! Reversal is status-based and neutralizes the movement through the
//! STEP-008 `reverse_movements_of_source` — the original rows always
//! survive (docs/19).

use rust_decimal::Decimal;
use sqlx::PgPool;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::financial_accounts::model::AccountStatus;
use crate::financial_accounts::repo as account_repo;

use super::model::{CategoryStatus, CategoryType, EntryKind};

// ------------------------------------------------------------------
// Read models
// ------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
pub struct CategoryRow {
    pub id: Uuid,
    pub category_type: String,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub entry_count: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub total_count: i64,
}

/// Bounded category list — filter by type/status, substring search on
/// name; stable order (ADR-010).
pub async fn list_categories(
    pool: &PgPool,
    category_type: Option<CategoryType>,
    status: Option<CategoryStatus>,
    search: Option<&str>,
    page: i64,
    page_size: i64,
) -> Result<Vec<CategoryRow>, sqlx::Error> {
    let pattern = search.map(|s| format!("%{}%", s.trim().to_lowercase()));
    sqlx::query_as(
        "SELECT c.id, c.category_type, c.name, c.description, c.status, \
            (SELECT count(*) FROM income_entries i WHERE i.category_id = c.id) + \
            (SELECT count(*) FROM expense_entries e WHERE e.category_id = c.id) AS entry_count, \
            c.created_at, c.updated_at, \
            count(*) OVER() AS total_count \
        FROM financial_categories c \
        WHERE ($1::text IS NULL OR c.category_type = $1) \
          AND ($2::text IS NULL OR c.status = $2) \
          AND ($3::text IS NULL OR lower(c.name) LIKE $3) \
        ORDER BY c.category_type, c.name, c.id LIMIT $4 OFFSET $5",
    )
    .bind(category_type.map(|t| t.as_str()))
    .bind(status.map(|s| s.as_str()))
    .bind(pattern)
    .bind(page_size)
    .bind((page - 1) * page_size)
    .fetch_all(pool)
    .await
}

/// Active categories of one type for entry-form pickers.
pub async fn list_category_options(
    pool: &PgPool,
    category_type: CategoryType,
) -> Result<Vec<CategoryRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT c.id, c.category_type, c.name, c.description, c.status, \
            0::bigint AS entry_count, c.created_at, c.updated_at, 0::bigint AS total_count \
        FROM financial_categories c \
        WHERE c.category_type = $1 AND c.status = 'active' \
        ORDER BY c.name, c.id",
    )
    .bind(category_type.as_str())
    .fetch_all(pool)
    .await
}

pub async fn get_category(pool: &PgPool, id: Uuid) -> Result<Option<CategoryRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT c.id, c.category_type, c.name, c.description, c.status, \
            (SELECT count(*) FROM income_entries i WHERE i.category_id = c.id) + \
            (SELECT count(*) FROM expense_entries e WHERE e.category_id = c.id) AS entry_count, \
            c.created_at, c.updated_at, 0::bigint AS total_count \
        FROM financial_categories c WHERE c.id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// One entry row with resolved labels and the bound movement's live
/// status (movement is authoritative; the join is display-only).
#[derive(Debug, sqlx::FromRow)]
pub struct EntryRow {
    pub id: Uuid,
    pub entry_number: i64,
    pub financial_account_id: Uuid,
    pub account_name: String,
    pub category_id: Uuid,
    pub category_name: String,
    pub amount: Decimal,
    pub currency: String,
    pub occurred_at: OffsetDateTime,
    pub description: String,
    pub counterparty: Option<String>,
    pub reference_no: Option<String>,
    pub account_movement_id: Uuid,
    pub movement_status: String,
    pub status: String,
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub total_count: i64,
}

fn table_of(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Income => "income_entries",
        EntryKind::Expense => "expense_entries",
    }
}

fn number_column_of(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Income => "income_number",
        EntryKind::Expense => "expense_number",
    }
}

fn entry_select(kind: EntryKind) -> String {
    format!(
        "SELECT t.id, t.{number_col} AS entry_number, \
            t.financial_account_id, a.name AS account_name, \
            t.category_id, c.name AS category_name, \
            t.amount, t.currency, t.occurred_at, t.description, t.counterparty, t.reference_no, \
            t.account_movement_id, m.status AS movement_status, \
            t.status, t.reversed_at, t.reversal_reason, t.created_at, t.updated_at",
        number_col = number_column_of(kind)
    )
}

/// Filters shared by list and summary surfaces.
#[derive(Debug, Default)]
pub struct EntryFilter {
    pub date_from: Option<Date>,
    pub date_to: Option<Date>,
    pub financial_account_id: Option<Uuid>,
    pub category_id: Option<Uuid>,
    pub status: Option<String>,
    pub search: Option<String>,
}

/// Bounded entry list (ADR-010). `date_from`/`date_to` are inclusive
/// business dates on `occurred_at`; search covers description,
/// counterparty, reference and the entry number.
pub async fn list_entries(
    pool: &PgPool,
    kind: EntryKind,
    filter: &EntryFilter,
    page: i64,
    page_size: i64,
) -> Result<Vec<EntryRow>, sqlx::Error> {
    let pattern = filter
        .search
        .as_deref()
        .map(|s| format!("%{}%", s.trim().to_lowercase()));
    sqlx::query_as::<_, EntryRow>(&format!(
        "{select}, count(*) OVER() AS total_count \
        FROM {table} t \
        JOIN financial_accounts a ON a.id = t.financial_account_id \
        JOIN financial_categories c ON c.id = t.category_id \
        JOIN account_movements m ON m.id = t.account_movement_id \
        WHERE ($1::date IS NULL OR t.occurred_at >= $1::date) \
          AND ($2::date IS NULL OR t.occurred_at < ($2::date + 1)) \
          AND ($3::uuid IS NULL OR t.financial_account_id = $3) \
          AND ($4::uuid IS NULL OR t.category_id = $4) \
          AND ($5::text IS NULL OR t.status = $5) \
          AND ($6::text IS NULL OR lower(t.description) LIKE $6 \
               OR lower(t.counterparty) LIKE $6 \
               OR lower(t.reference_no) LIKE $6 \
               OR t.{number_col}::text LIKE $6) \
        ORDER BY t.occurred_at DESC, t.{number_col} DESC, t.id LIMIT $7 OFFSET $8",
        select = entry_select(kind),
        table = table_of(kind),
        number_col = number_column_of(kind),
    ))
    .bind(filter.date_from)
    .bind(filter.date_to)
    .bind(filter.financial_account_id)
    .bind(filter.category_id)
    .bind(filter.status.as_deref())
    .bind(pattern)
    .bind(page_size)
    .bind((page - 1) * page_size)
    .fetch_all(pool)
    .await
}

pub async fn get_entry(
    pool: &PgPool,
    kind: EntryKind,
    id: Uuid,
) -> Result<Option<EntryRow>, sqlx::Error> {
    sqlx::query_as::<_, EntryRow>(&format!(
        "{select}, 0::bigint AS total_count \
        FROM {table} t \
        JOIN financial_accounts a ON a.id = t.financial_account_id \
        JOIN financial_categories c ON c.id = t.category_id \
        JOIN account_movements m ON m.id = t.account_movement_id \
        WHERE t.id = $1",
        select = entry_select(kind),
        table = table_of(kind),
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// Operational totals over POSTED entries only — deliberately NOT the
/// account balance: Payments, Transfers and other movement sources do
/// not appear here (docs/15, STEP-010 reporting semantics).
#[derive(Debug)]
pub struct OperationalSummary {
    pub income_total: Decimal,
    pub income_count: i64,
    pub expense_total: Decimal,
    pub expense_count: i64,
}

pub async fn operational_summary(
    pool: &PgPool,
    filter: &EntryFilter,
) -> Result<OperationalSummary, sqlx::Error> {
    let mut summary = OperationalSummary {
        income_total: Decimal::ZERO,
        income_count: 0,
        expense_total: Decimal::ZERO,
        expense_count: 0,
    };
    for (kind, slot) in [(EntryKind::Income, 0usize), (EntryKind::Expense, 1usize)] {
        let (total, count): (Option<Decimal>, i64) = sqlx::query_as(&format!(
            "SELECT sum(t.amount), count(*) FROM {table} t \
             WHERE t.status = 'posted' \
               AND ($1::date IS NULL OR t.occurred_at >= $1::date) \
               AND ($2::date IS NULL OR t.occurred_at < ($2::date + 1)) \
               AND ($3::uuid IS NULL OR t.financial_account_id = $3) \
               AND ($4::uuid IS NULL OR t.category_id = $4)",
            table = table_of(kind),
        ))
        .bind(filter.date_from)
        .bind(filter.date_to)
        .bind(filter.financial_account_id)
        .bind(filter.category_id)
        .fetch_one(pool)
        .await?;
        let amount = total.unwrap_or_default();
        if slot == 0 {
            summary.income_total = amount;
            summary.income_count = count;
        } else {
            summary.expense_total = amount;
            summary.expense_count = count;
        }
    }
    Ok(summary)
}

// ------------------------------------------------------------------
// Category commands
// ------------------------------------------------------------------

#[derive(Debug)]
pub enum IncomeExpenseCommandError {
    NotFound,
    /// Missing, wrong-typed or inactive category; deterministic.
    CategoryInvalid,
    /// Posting rejected: the account is not `active`.
    InactiveAccount,
    /// Effect would take the account below zero — negative balances
    /// are forbidden by the STEP-008 operator decision.
    InsufficientFunds,
    /// Idempotency key reused with a different payload.
    IdempotencyConflict,
    /// Optimistic-concurrency precondition failed (updated_at moved).
    StaleState,
    /// Category name already exists within its type.
    DuplicateName,
    /// The account layer reported a command error that cannot arise
    /// from an entry command (same-account/currency/not-posted) —
    /// mapped to a plain 500, logged for investigation.
    AccountInvariant(account_repo::AccountCommandError),
    Database(sqlx::Error),
}

pub struct CreateCategory {
    pub category_type: CategoryType,
    pub name: String,
    pub description: Option<String>,
}

pub async fn create_category(
    pool: &PgPool,
    actor: Uuid,
    command: CreateCategory,
) -> Result<Uuid, IncomeExpenseCommandError> {
    let result = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO financial_categories (category_type, name, description, created_by) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(command.category_type.as_str())
    .bind(&command.name)
    .bind(&command.description)
    .bind(actor)
    .fetch_one(pool)
    .await;
    match result {
        Ok(id) => Ok(id),
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
            Err(IncomeExpenseCommandError::DuplicateName)
        }
        Err(error) => Err(IncomeExpenseCommandError::Database(error)),
    }
}

/// Rename/description update only — the row itself is the stable
/// historical reference (docs/19), so a rename keeps old entries
/// coherent; deactivation gates NEW use.
pub async fn update_category(
    pool: &PgPool,
    id: Uuid,
    name: String,
    description: Option<String>,
    expected_updated_at: OffsetDateTime,
) -> Result<(), IncomeExpenseCommandError> {
    let result = sqlx::query(
        "UPDATE financial_categories SET name = $2, description = $3, updated_at = now() \
         WHERE id = $1 AND updated_at = $4",
    )
    .bind(id)
    .bind(&name)
    .bind(&description)
    .bind(expected_updated_at)
    .execute(pool)
    .await;
    match result {
        Ok(done) if done.rows_affected() == 1 => Ok(()),
        Ok(_) => {
            let exists: Option<Uuid> =
                sqlx::query_scalar("SELECT id FROM financial_categories WHERE id = $1")
                    .bind(id)
                    .fetch_optional(pool)
                    .await
                    .map_err(IncomeExpenseCommandError::Database)?;
            match exists {
                Some(_) => Err(IncomeExpenseCommandError::StaleState),
                None => Err(IncomeExpenseCommandError::NotFound),
            }
        }
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
            Err(IncomeExpenseCommandError::DuplicateName)
        }
        Err(error) => Err(IncomeExpenseCommandError::Database(error)),
    }
}

/// `active <-> inactive` lifecycle — never deletes: a referenced
/// category must stay interpretable forever (docs/19).
pub async fn change_category_status(
    pool: &PgPool,
    id: Uuid,
    target: CategoryStatus,
) -> Result<CategoryStatus, IncomeExpenseCommandError> {
    let previous: Option<String> = sqlx::query_scalar(
        "UPDATE financial_categories SET status = $2, updated_at = now() \
         WHERE id = $1 RETURNING status",
    )
    .bind(id)
    .bind(target.as_str())
    .fetch_optional(pool)
    .await
    .map_err(IncomeExpenseCommandError::Database)?;
    match previous {
        Some(_) => Ok(target),
        None => Err(IncomeExpenseCommandError::NotFound),
    }
}

// ------------------------------------------------------------------
// Entry commands (shared engine for Income and Expense)
// ------------------------------------------------------------------

pub struct PostEntry {
    pub financial_account_id: Uuid,
    pub category_id: Uuid,
    pub amount: Decimal,
    pub occurred_at: OffsetDateTime,
    pub description: String,
    pub counterparty: Option<String>,
    pub reference_no: Option<String>,
    pub idempotency_key: String,
    pub fingerprint: String,
}

pub struct EntryOutcome {
    pub entry_id: Uuid,
    pub entry_number: i64,
    pub replayed: bool,
}

/// THE posting command: entry + exactly ONE movement, atomically.
/// For Expense the account must fund the outflow under its row lock;
/// for Income the inflow is always safe.
pub async fn post_entry(
    pool: &PgPool,
    actor: Uuid,
    kind: EntryKind,
    command: PostEntry,
) -> Result<EntryOutcome, IncomeExpenseCommandError> {
    let table = table_of(kind);
    let number_col = number_column_of(kind);
    let mut tx = pool
        .begin()
        .await
        .map_err(IncomeExpenseCommandError::Database)?;

    // 1. Idempotent replay short-circuit.
    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(&format!(
        "SELECT id, {number_col}, idempotency_fingerprint FROM {table} \
         WHERE idempotency_key = $1"
    ))
    .bind(&command.idempotency_key)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(IncomeExpenseCommandError::Database)?;
    if let Some((id, number, fingerprint)) = existing {
        if fingerprint != command.fingerprint {
            return Err(IncomeExpenseCommandError::IdempotencyConflict);
        }
        tx.rollback()
            .await
            .map_err(IncomeExpenseCommandError::Database)?;
        return Ok(EntryOutcome {
            entry_id: id,
            entry_number: number,
            replayed: true,
        });
    }

    // 2. Lock the account row (the shared deterministic lock prefix).
    let locked = account_repo::lock_accounts(&mut tx, &[command.financial_account_id])
        .await
        .map_err(account_to_entry_error)?;
    let Some((status, _currency)) = locked.get(&command.financial_account_id) else {
        return Err(IncomeExpenseCommandError::NotFound);
    };
    if *status != AccountStatus::Active.as_str() {
        return Err(IncomeExpenseCommandError::InactiveAccount);
    }

    // 3. Lock the category: must exist, match the entry kind and be
    //    active. The composite FK then makes a wrong-typed write
    //    impossible even under a later race.
    let category: Option<(String, String)> = sqlx::query_as(
        "SELECT category_type, status FROM financial_categories WHERE id = $1 FOR UPDATE",
    )
    .bind(command.category_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(IncomeExpenseCommandError::Database)?;
    let Some((category_type, category_status)) = category else {
        return Err(IncomeExpenseCommandError::CategoryInvalid);
    };
    if category_type != kind.category_type().as_str()
        || category_status != CategoryStatus::Active.as_str()
    {
        return Err(IncomeExpenseCommandError::CategoryInvalid);
    }

    // 4. Negative balances are forbidden: an Expense must be fundable
    //    under the account lock (concurrent postings serialize here).
    if kind == EntryKind::Expense {
        let balance = account_repo::locked_balance(&mut tx, command.financial_account_id)
            .await
            .map_err(account_to_entry_error)?;
        if command.amount > balance {
            return Err(IncomeExpenseCommandError::InsufficientFunds);
        }
    }

    // 5. Insert the authoritative movement first — the entry id is
    //    generated up front so provenance is established in one pass.
    let entry_id = Uuid::new_v4();
    let movement_id = account_repo::insert_movement(
        &mut tx,
        account_repo::NewMovement {
            account_id: command.financial_account_id,
            direction: kind.movement_direction(),
            amount: command.amount,
            source: kind.movement_source(),
            source_id: entry_id,
            occurred_at: command.occurred_at,
            actor,
        },
    )
    .await
    .map_err(account_to_entry_error)?;

    // 6. The entry binds to its movement 1:1 — same transaction.
    let inserted: Option<(Uuid, i64)> = sqlx::query_as(&format!(
        "INSERT INTO {table} \
            (id, financial_account_id, category_id, amount, currency, occurred_at, \
             description, counterparty, reference_no, account_movement_id, status, \
             idempotency_key, idempotency_fingerprint, created_by) \
         VALUES ($1, $2, $3, $4, 'TRY', $5, $6, $7, $8, $9, 'posted', $10, $11, $12) \
         ON CONFLICT (idempotency_key) DO NOTHING \
         RETURNING id, {number_col}"
    ))
    .bind(entry_id)
    .bind(command.financial_account_id)
    .bind(command.category_id)
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
    .map_err(IncomeExpenseCommandError::Database)?;

    let Some((id, number)) = inserted else {
        // Concurrent replay: the other transaction already posted under
        // this key — roll back our attempt and return theirs.
        tx.rollback()
            .await
            .map_err(IncomeExpenseCommandError::Database)?;
        let existing: Option<(Uuid, i64, String)> = sqlx::query_as(&format!(
            "SELECT id, {number_col}, idempotency_fingerprint FROM {table} \
             WHERE idempotency_key = $1"
        ))
        .bind(&command.idempotency_key)
        .fetch_optional(pool)
        .await
        .map_err(IncomeExpenseCommandError::Database)?;
        let Some((id, number, fingerprint)) = existing else {
            return Err(IncomeExpenseCommandError::NotFound);
        };
        if fingerprint != command.fingerprint {
            return Err(IncomeExpenseCommandError::IdempotencyConflict);
        }
        return Ok(EntryOutcome {
            entry_id: id,
            entry_number: number,
            replayed: true,
        });
    };

    tx.commit()
        .await
        .map_err(IncomeExpenseCommandError::Database)?;
    Ok(EntryOutcome {
        entry_id: id,
        entry_number: number,
        replayed: false,
    })
}

/// Status-based reversal (docs/19): the entry row is locked FOR
/// UPDATE, its movement flipped to `reversed` with actor/time/reason,
/// and the entry follows — one transaction. An Income reversal that
/// would take the account below zero is rejected (the funds were
/// spent; restore them first). Replays return `(true, number)`.
pub async fn reverse_entry(
    pool: &PgPool,
    actor: Uuid,
    kind: EntryKind,
    entry_id: Uuid,
    reason: String,
    now: OffsetDateTime,
) -> Result<(bool, i64), IncomeExpenseCommandError> {
    let table = table_of(kind);
    let number_col = number_column_of(kind);
    let mut tx = pool
        .begin()
        .await
        .map_err(IncomeExpenseCommandError::Database)?;

    let entry: Option<(String, i64, Uuid, Decimal)> = sqlx::query_as(&format!(
        "SELECT status, {number_col}, financial_account_id, amount \
         FROM {table} WHERE id = $1 FOR UPDATE"
    ))
    .bind(entry_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(IncomeExpenseCommandError::Database)?;
    let Some((status, entry_number, account_id, amount)) = entry else {
        return Err(IncomeExpenseCommandError::NotFound);
    };
    if status != "posted" {
        tx.rollback()
            .await
            .map_err(IncomeExpenseCommandError::Database)?;
        return Ok((true, entry_number));
    }

    // Same deterministic account lock prefix as every balance command.
    account_repo::lock_accounts(&mut tx, &[account_id])
        .await
        .map_err(account_to_entry_error)?;

    // Removing an Income inflow decreases the balance — refuse if the
    // money is no longer there (docs/19 reversal safety).
    if kind == EntryKind::Income {
        let balance = account_repo::locked_balance(&mut tx, account_id)
            .await
            .map_err(account_to_entry_error)?;
        if balance - amount < Decimal::ZERO {
            return Err(IncomeExpenseCommandError::InsufficientFunds);
        }
    }

    account_repo::reverse_movements_of_source(
        &mut tx,
        kind.movement_source(),
        entry_id,
        now,
        actor,
        &reason,
    )
    .await
    .map_err(account_to_entry_error)?;

    sqlx::query(&format!(
        "UPDATE {table} SET status = 'reversed', \
            reversed_at = $2, reversed_by = $3, reversal_reason = $4, updated_at = $2 \
         WHERE id = $1"
    ))
    .bind(entry_id)
    .bind(now)
    .bind(actor)
    .bind(&reason)
    .execute(tx.as_mut())
    .await
    .map_err(IncomeExpenseCommandError::Database)?;

    tx.commit()
        .await
        .map_err(IncomeExpenseCommandError::Database)?;
    Ok((false, entry_number))
}

/// The STEP-008 account layer reports its own error enum — translate
/// once at the boundary.
fn account_to_entry_error(error: account_repo::AccountCommandError) -> IncomeExpenseCommandError {
    match error {
        account_repo::AccountCommandError::NotFound => IncomeExpenseCommandError::NotFound,
        account_repo::AccountCommandError::InactiveAccount => {
            IncomeExpenseCommandError::InactiveAccount
        }
        account_repo::AccountCommandError::InsufficientFunds => {
            IncomeExpenseCommandError::InsufficientFunds
        }
        account_repo::AccountCommandError::IdempotencyConflict => {
            IncomeExpenseCommandError::IdempotencyConflict
        }
        account_repo::AccountCommandError::StaleState => IncomeExpenseCommandError::StaleState,
        account_repo::AccountCommandError::Database(error) => {
            IncomeExpenseCommandError::Database(error)
        }
        other => IncomeExpenseCommandError::AccountInvariant(other),
    }
}
