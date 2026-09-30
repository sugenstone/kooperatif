//! STEP-009 repository — Shareholder Credit (Excess Payment) commands.
//!
//! Domain semantics (docs/00 §"Excess payments", docs/03, docs/05
//! §"Automatic use of existing excess", docs/06 §"Existing/New
//! excess"):
//!
//!   * A Shareholder Credit is an ENTITLEMENT over already-received
//!     money — created only from a posted Payment's unassigned
//!     remainder, owned by an explicitly chosen Shareholder. It is
//!     never implied by payer/guardian/Family identity.
//!   * A Credit Application settles an Assessment from held credit.
//!     It is NOT a Payment and produces NO Account Movement — the
//!     reconciliation invariant `payment = allocations + credits +
//!     remainder` keeps every lira attributable exactly once.
//!   * Everything derives from history: no mutable balance column
//!     exists; availability and settled amounts are recomputed under
//!     row locks.
//!
//! Deterministic lock order shared with the rest of the codebase
//! (payments -> financial_accounts -> shareholder_credits ->
//! assessments, each ascending by id):
//!   * assign_credit:        payment FOR UPDATE -> credit insert ->
//!     sweep locks credits ASC then assessments ASC
//!   * apply_credit:         credits ASC -> assessment FOR UPDATE
//!   * reverse_credit:       credit FOR UPDATE (applications checked)
//!   * reverse_application:  credit FOR UPDATE -> assessment FOR UPDATE
//!   * auto-offset (period): new assessment rows -> credits ASC
//!   * reverse_payment:      payment -> account -> credits (rejects if
//!     any active application exists)

use std::collections::HashMap;

use rust_decimal::Decimal;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use super::model::{ApplicationMode, ApplicationRow, CreditRow, STATUS_ACTIVE};

/// PostgreSQL unique-violation detection (ADR-006 single-flight).
fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(dbe) if dbe.code().as_deref() == Some("23505"))
}

#[derive(Debug)]
pub enum CreditCommandError {
    /// Resource missing or indistinguishable (IDOR rule).
    NotFound,
    /// Source Payment is not `posted` — 409.
    PaymentNotPosted,
    /// Beneficiary shareholder does not exist or is voided — 422.
    BeneficiaryInvalid,
    /// Requested credit exceeds the Payment's unassigned remainder — 409.
    InsufficientRemainder,
    /// Target Assessment is missing or not `active` — 422.
    InactiveAssessment,
    /// Target Assessment is already fully settled — 409.
    AssessmentSettled,
    /// Manual apply: beneficiary has less available credit — 409.
    InsufficientCredit,
    /// Idempotency key reused with a different payload — 409.
    IdempotencyConflict,
    /// Concurrent same-key insert hit the UNIQUE index — internal
    /// signal; the caller resolves it into a replay or a conflict.
    IdempotencyRace,
    /// Entity has active dependent applications — 409.
    Consumed,
    Database(sqlx::Error),
}

// ------------------------------------------------------------------
// Shared transaction helpers
// ------------------------------------------------------------------

/// Lock a Shareholder's ACTIVE credit rows `FOR UPDATE` in ascending
/// `credit_number` order — the single deterministic consumption order
/// (FIFO by creation). Returns (id -> amount).
async fn lock_credits(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    shareholder_id: Uuid,
) -> Result<Vec<(Uuid, Decimal)>, CreditCommandError> {
    sqlx::query_as(
        "SELECT id, amount FROM shareholder_credits \
         WHERE shareholder_id = $1 AND status = 'active' \
         ORDER BY credit_number FOR UPDATE",
    )
    .bind(shareholder_id)
    .fetch_all(tx.as_mut())
    .await
    .map_err(CreditCommandError::Database)
}

/// Active applications per credit — recomputed under the credit locks.
async fn applied_map(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    credit_ids: &[Uuid],
) -> Result<HashMap<Uuid, Decimal>, CreditCommandError> {
    let rows: Vec<(Uuid, Decimal)> = sqlx::query_as(
        "SELECT credit_id, sum(amount) FROM credit_applications \
         WHERE credit_id = ANY($1) AND status = 'active' \
         GROUP BY credit_id",
    )
    .bind(credit_ids)
    .fetch_all(tx.as_mut())
    .await
    .map_err(CreditCommandError::Database)?;
    Ok(rows.into_iter().collect())
}

/// Settled amount per assessment = active Payment allocations + active
/// Credit applications. This is THE settlement formula of STEP-009 —
/// a Credit Application satisfies debt exactly like an Allocation
/// without being one (docs/06: "not new cash").
pub async fn settled_map(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    assessment_ids: &[Uuid],
) -> Result<HashMap<Uuid, Decimal>, sqlx::Error> {
    let rows: Vec<(Uuid, Decimal)> = sqlx::query_as(
        "SELECT assessment_id, sum(amount) FROM ( \
             SELECT assessment_id, amount FROM payment_allocations \
             WHERE assessment_id = ANY($1) AND status = 'active' \
             UNION ALL \
             SELECT assessment_id, amount FROM credit_applications \
             WHERE assessment_id = ANY($1) AND status = 'active' \
         ) s GROUP BY assessment_id",
    )
    .bind(assessment_ids)
    .fetch_all(tx.as_mut())
    .await?;
    Ok(rows.into_iter().collect())
}

/// Insert one credit application inside the caller's transaction.
#[allow(clippy::too_many_arguments)]
async fn insert_application(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    credit_id: Uuid,
    assessment_id: Uuid,
    amount: Decimal,
    mode: ApplicationMode,
    idempotency_key: Option<&str>,
    idempotency_fingerprint: Option<&str>,
    actor: Uuid,
) -> Result<(), CreditCommandError> {
    sqlx::query(
        "INSERT INTO credit_applications \
            (credit_id, assessment_id, amount, currency, mode, status, \
             idempotency_key, idempotency_fingerprint, created_by) \
         VALUES ($1, $2, $3, 'TRY', $4, 'active', $5, $6, $7)",
    )
    .bind(credit_id)
    .bind(assessment_id)
    .bind(amount)
    .bind(mode.as_str())
    .bind(idempotency_key)
    .bind(idempotency_fingerprint)
    .bind(actor)
    .execute(tx.as_mut())
    .await
    .map_err(|error| {
        if is_unique_violation(&error) {
            CreditCommandError::IdempotencyRace
        } else {
            CreditCommandError::Database(error)
        }
    })?;
    Ok(())
}

/// THE consumption engine (docs/05: deterministic and explainable).
///
/// Consumes the shareholder's available credit FIFO (`credit_number`
/// ASC) against the given assessments in the order supplied by the
/// caller (callers pass a deterministic ordering — due date then id —
/// never incidental row order). Assumes the caller already locked or
/// created the assessments inside this transaction; each assessment is
/// re-locked here so a concurrent manual application serializes
/// instead of racing the sweep.
///
/// `idempotency_key`/`fingerprint` are stamped on the FIRST inserted
/// row only — the UNIQUE index on `idempotency_key` then provides
/// single-flight protection while the rest of the command's rows are
/// covered by the same transaction (a manual command may split one
/// amount across several credits).
///
/// Returns (assessment_id -> total applied) for outcome/audit data.
#[allow(clippy::too_many_arguments)]
async fn consume_credit_fifo(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    shareholder_id: Uuid,
    assessment_ids: &[Uuid],
    mode: ApplicationMode,
    idempotency_key: Option<&str>,
    idempotency_fingerprint: Option<&str>,
    actor: Uuid,
) -> Result<HashMap<Uuid, Decimal>, CreditCommandError> {
    let mut applied: HashMap<Uuid, Decimal> = HashMap::new();
    if assessment_ids.is_empty() {
        return Ok(applied);
    }

    let credits = lock_credits(tx, shareholder_id).await?;
    let credit_ids: Vec<Uuid> = credits.iter().map(|(id, _)| *id).collect();
    let used = applied_map(tx, &credit_ids).await?;
    let mut available: Vec<(Uuid, Decimal)> = credits
        .iter()
        .map(|(id, amount)| (*id, *amount - used.get(id).copied().unwrap_or_default()))
        .filter(|(_, avail)| *avail > Decimal::ZERO)
        .collect();
    if available.is_empty() {
        return Ok(applied);
    }
    let mut cursor = 0usize;
    let mut key_stamped = false;

    for assessment_id in assessment_ids {
        // Lock the assessment row: serializes vs. concurrent manual
        // applications and re-derives remaining under the lock.
        let row: Option<(Decimal, String)> =
            sqlx::query_as("SELECT amount, status FROM assessments WHERE id = $1 FOR UPDATE")
                .bind(assessment_id)
                .fetch_optional(tx.as_mut())
                .await
                .map_err(CreditCommandError::Database)?;
        let Some((amount, status)) = row else {
            continue; // assessment vanished — skip, never invent targets
        };
        if status != "active" {
            continue;
        }
        let mut settled = settled_map(tx, &[*assessment_id])
            .await
            .map_err(CreditCommandError::Database)?
            .get(assessment_id)
            .copied()
            .unwrap_or_default();
        let mut remaining = amount - settled;
        if remaining <= Decimal::ZERO {
            continue;
        }

        while remaining > Decimal::ZERO && cursor < available.len() {
            let (credit_id, avail) = &mut available[cursor];
            if *avail <= Decimal::ZERO {
                cursor += 1;
                continue;
            }
            let applied_amount = remaining.min(*avail);
            let (key, fingerprint) = if key_stamped {
                (None, None)
            } else {
                key_stamped = true;
                (idempotency_key, idempotency_fingerprint)
            };
            insert_application(
                tx,
                *credit_id,
                *assessment_id,
                applied_amount,
                mode,
                key,
                fingerprint,
                actor,
            )
            .await?;
            *avail -= applied_amount;
            remaining -= applied_amount;
            settled += applied_amount;
            *applied.entry(*assessment_id).or_default() += applied_amount;
        }
    }
    Ok(applied)
}

/// Auto-offset for NEWLY GENERATED assessments (docs/05 §"Automatic
/// use of existing excess"). Called inside the generation transaction
/// after assessment rows exist. `targets` must already be in the
/// deterministic consumption order chosen by the caller.
pub async fn auto_apply_to_assessments(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: Uuid,
    targets: &[(Uuid, Uuid)], // (assessment_id, shareholder_id)
) -> Result<HashMap<Uuid, Decimal>, CreditCommandError> {
    // Group by shareholder so each beneficiary's credit rows are
    // locked once, in ascending order.
    let mut by_shareholder: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    let mut order: Vec<Uuid> = Vec::new();
    for (assessment_id, shareholder_id) in targets {
        let entry = by_shareholder.entry(*shareholder_id).or_insert_with(|| {
            order.push(*shareholder_id);
            Vec::new()
        });
        entry.push(*assessment_id);
    }
    order.sort_unstable(); // deterministic cross-shareholder order
    let mut result = HashMap::new();
    for shareholder_id in order {
        let ids = by_shareholder
            .get(&shareholder_id)
            .cloned()
            .unwrap_or_default();
        let applied = consume_credit_fifo(
            tx,
            shareholder_id,
            &ids,
            ApplicationMode::Automatic,
            None,
            None,
            actor,
        )
        .await?;
        result.extend(applied);
    }
    Ok(result)
}

// ------------------------------------------------------------------
// Read model
// ------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
struct CreditRowSql {
    id: Uuid,
    credit_number: i64,
    source_payment_id: Uuid,
    payment_number: i64,
    shareholder_id: Uuid,
    shareholder_first_name: String,
    shareholder_last_name: String,
    amount: Decimal,
    applied: Decimal,
    status: String,
    note: Option<String>,
    reversed_at: Option<OffsetDateTime>,
    reversal_reason: Option<String>,
    created_at: OffsetDateTime,
    created_by_name: Option<String>,
}

const CREDIT_SELECT: &str = "SELECT c.id, c.credit_number, c.source_payment_id, \
    pay.payment_number, c.shareholder_id, \
    sp.first_name AS shareholder_first_name, sp.last_name AS shareholder_last_name, \
    c.amount, \
    COALESCE(ap.applied, 0) AS applied, \
    c.status, c.note, c.reversed_at, c.reversal_reason, \
    c.created_at, cb.display_name AS created_by_name \
FROM shareholder_credits c \
JOIN payments pay ON pay.id = c.source_payment_id \
JOIN shareholders sh ON sh.id = c.shareholder_id \
JOIN persons sp ON sp.id = sh.person_id \
LEFT JOIN ( \
    SELECT credit_id, sum(amount) AS applied FROM credit_applications \
    WHERE status = 'active' GROUP BY credit_id \
) ap ON ap.credit_id = c.id \
LEFT JOIN users cb ON cb.id = c.created_by";

fn credit_row(r: CreditRowSql) -> CreditRow {
    let available = if r.status == STATUS_ACTIVE {
        r.amount - r.applied
    } else {
        Decimal::ZERO
    };
    CreditRow {
        id: r.id,
        credit_number: r.credit_number,
        source_payment_id: r.source_payment_id,
        payment_number: r.payment_number,
        shareholder_id: r.shareholder_id,
        shareholder_first_name: r.shareholder_first_name,
        shareholder_last_name: r.shareholder_last_name,
        amount: r.amount,
        applied_amount: r.applied,
        available_amount: available,
        status: r.status,
        note: r.note,
        reversed_at: r.reversed_at,
        reversal_reason: r.reversal_reason,
        created_at: r.created_at,
        created_by_name: r.created_by_name,
    }
}

/// Credit ledger of one Shareholder (docs/13 "Fazla Ödemeler" tab):
/// origins + derived per-credit availability, newest first.
pub async fn list_shareholder_credits(
    pool: &PgPool,
    shareholder_id: Uuid,
) -> Result<Vec<CreditRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, CreditRowSql>(&format!(
        "{CREDIT_SELECT} WHERE c.shareholder_id = $1 ORDER BY c.credit_number DESC"
    ))
    .bind(shareholder_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(credit_row).collect())
}

/// Credits sourced from one Payment (payment detail surface).
pub async fn list_payment_credits(
    pool: &PgPool,
    payment_id: Uuid,
) -> Result<Vec<CreditRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, CreditRowSql>(&format!(
        "{CREDIT_SELECT} WHERE c.source_payment_id = $1 ORDER BY c.credit_number"
    ))
    .bind(payment_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(credit_row).collect())
}

const APPLICATION_SELECT: &str = "SELECT ap.id, ap.credit_id, c.credit_number, \
    ap.assessment_id, p.name AS period_name, ap.amount, ap.mode, ap.status, \
    ap.reversed_at, ap.reversal_reason, ap.created_at, \
    cb.display_name AS created_by_name \
FROM credit_applications ap \
JOIN shareholder_credits c ON c.id = ap.credit_id \
JOIN assessments a ON a.id = ap.assessment_id \
JOIN periods p ON p.id = a.period_id \
LEFT JOIN users cb ON cb.id = ap.created_by";

fn application_row(r: ApplicationRowSql) -> ApplicationRow {
    ApplicationRow {
        id: r.id,
        credit_id: r.credit_id,
        credit_number: r.credit_number,
        assessment_id: r.assessment_id,
        period_name: r.period_name,
        amount: r.amount,
        mode: r.mode,
        status: r.status,
        reversed_at: r.reversed_at,
        reversal_reason: r.reversal_reason,
        created_at: r.created_at,
        created_by_name: r.created_by_name,
    }
}

#[derive(Debug, sqlx::FromRow)]
struct ApplicationRowSql {
    id: Uuid,
    credit_id: Uuid,
    credit_number: i64,
    assessment_id: Uuid,
    period_name: String,
    amount: Decimal,
    mode: String,
    status: String,
    reversed_at: Option<OffsetDateTime>,
    reversal_reason: Option<String>,
    created_at: OffsetDateTime,
    created_by_name: Option<String>,
}

/// Application history of one Shareholder's credits (credit-side view).
pub async fn list_shareholder_applications(
    pool: &PgPool,
    shareholder_id: Uuid,
) -> Result<Vec<ApplicationRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, ApplicationRowSql>(&format!(
        "{APPLICATION_SELECT} WHERE c.shareholder_id = $1 ORDER BY ap.created_at DESC, ap.id"
    ))
    .bind(shareholder_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(application_row).collect())
}

/// Application history against one Assessment (assessment-side view).
pub async fn list_assessment_applications(
    pool: &PgPool,
    assessment_id: Uuid,
) -> Result<Vec<ApplicationRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, ApplicationRowSql>(&format!(
        "{APPLICATION_SELECT} WHERE ap.assessment_id = $1 ORDER BY ap.created_at, ap.id"
    ))
    .bind(assessment_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(application_row).collect())
}

/// Derived summary — never a stored value.
#[derive(Debug, sqlx::FromRow)]
pub struct CreditSummaryRow {
    pub credit_count: i64,
    pub total_originated: Decimal,
    pub total_applied: Decimal,
    pub available: Decimal,
}

pub async fn shareholder_credit_summary(
    pool: &PgPool,
    shareholder_id: Uuid,
) -> Result<CreditSummaryRow, sqlx::Error> {
    sqlx::query_as::<_, CreditSummaryRow>(
        "SELECT count(*) FILTER (WHERE c.status = 'active') AS credit_count, \
            COALESCE(sum(c.amount) FILTER (WHERE c.status = 'active'), 0) AS total_originated, \
            COALESCE(sum(COALESCE(ap.applied, 0)) FILTER (WHERE c.status = 'active'), 0) \
                AS total_applied, \
            COALESCE(sum(c.amount - COALESCE(ap.applied, 0)) FILTER (WHERE c.status = 'active'), 0) \
                AS available \
         FROM shareholder_credits c \
         LEFT JOIN ( \
             SELECT credit_id, sum(amount) AS applied FROM credit_applications \
             WHERE status = 'active' GROUP BY credit_id \
         ) ap ON ap.credit_id = c.id \
         WHERE c.shareholder_id = $1",
    )
    .bind(shareholder_id)
    .fetch_one(pool)
    .await
}

/// Available remainder of a Payment for credit assignment:
/// amount − active allocations − active credits (the reconciliation
/// invariant of docs/03, now with the excess disposition term).
pub async fn payment_unassigned_remainder(
    pool: &PgPool,
    payment_id: Uuid,
) -> Result<Option<Decimal>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT p.amount - COALESCE(al.allocated, 0) - COALESCE(cr.credited, 0) \
         FROM payments p \
         LEFT JOIN (SELECT payment_id, sum(amount) AS allocated FROM payment_allocations \
                    WHERE status = 'active' GROUP BY payment_id) al \
            ON al.payment_id = p.id \
         LEFT JOIN (SELECT source_payment_id, sum(amount) AS credited FROM shareholder_credits \
                    WHERE status = 'active' GROUP BY source_payment_id) cr \
            ON cr.source_payment_id = p.id \
         WHERE p.id = $1",
    )
    .bind(payment_id)
    .fetch_optional(pool)
    .await
}

// ------------------------------------------------------------------
// Commands
// ------------------------------------------------------------------

pub struct AssignCredit {
    pub payment_id: Uuid,
    pub shareholder_id: Uuid,
    pub amount: Decimal,
    pub note: Option<String>,
    pub idempotency_key: String,
    pub fingerprint: String,
}

pub struct AssignOutcome {
    pub credit_id: Uuid,
    pub credit_number: i64,
    pub replayed: bool,
    /// Total credit auto-applied to the beneficiary's EXISTING open
    /// assessments inside the same transaction (STEP-009 policy:
    /// newly assigned credit sweeps open debt oldest-due-first).
    pub applied_amount: Decimal,
    pub application_count: i64,
}

/// Assign part of a posted Payment's unassigned remainder as a
/// Shareholder Credit (docs/06: explicit attribution, never inferred).
/// The command then auto-offsets the beneficiary's existing open
/// Assessments — oldest due date first — inside the same transaction.
pub async fn assign_credit(
    pool: &PgPool,
    actor: Uuid,
    command: AssignCredit,
    now: OffsetDateTime,
) -> Result<AssignOutcome, CreditCommandError> {
    let mut tx = pool.begin().await.map_err(CreditCommandError::Database)?;

    // 1. Idempotent replay short-circuit — before ANY side effect.
    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, credit_number, idempotency_fingerprint FROM shareholder_credits \
         WHERE idempotency_key = $1",
    )
    .bind(&command.idempotency_key)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(CreditCommandError::Database)?;
    if let Some((id, number, fingerprint)) = existing {
        tx.rollback().await.map_err(CreditCommandError::Database)?;
        if fingerprint != command.fingerprint {
            return Err(CreditCommandError::IdempotencyConflict);
        }
        return Ok(AssignOutcome {
            credit_id: id,
            credit_number: number,
            replayed: true,
            applied_amount: Decimal::ZERO,
            application_count: 0,
        });
    }

    // 2. Payment lock FIRST — the shared lock prefix of every
    //    payment-scoped mutation. Under this lock the remainder is
    //    recomputed, so two concurrent assignments can never create
    //    more credit than the remainder funds.
    let payment: Option<(String, Decimal)> =
        sqlx::query_as("SELECT status, amount FROM payments WHERE id = $1 FOR UPDATE")
            .bind(command.payment_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(CreditCommandError::Database)?;
    let Some((payment_status, payment_amount)) = payment else {
        return Err(CreditCommandError::NotFound);
    };
    if payment_status != "posted" {
        return Err(CreditCommandError::PaymentNotPosted);
    }

    // 3. Beneficiary: an explicit Shareholder, never voided.
    let beneficiary: Option<String> =
        sqlx::query_scalar("SELECT status FROM shareholders WHERE id = $1")
            .bind(command.shareholder_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(CreditCommandError::Database)?;
    match beneficiary.as_deref() {
        Some("active") | Some("inactive") => {}
        _ => return Err(CreditCommandError::BeneficiaryInvalid),
    }

    // 4. Remainder under the payment lock: amount − allocations −
    //    already-assigned credits.
    let remainder: Decimal = sqlx::query_scalar(
        "SELECT $2 - COALESCE(al.allocated, 0) - COALESCE(cr.credited, 0) \
         FROM payments p \
         LEFT JOIN (SELECT payment_id, sum(amount) AS allocated FROM payment_allocations \
                    WHERE status = 'active' GROUP BY payment_id) al \
            ON al.payment_id = p.id \
         LEFT JOIN (SELECT source_payment_id, sum(amount) AS credited FROM shareholder_credits \
                    WHERE status = 'active' GROUP BY source_payment_id) cr \
            ON cr.source_payment_id = p.id \
         WHERE p.id = $1",
    )
    .bind(command.payment_id)
    .bind(payment_amount)
    .fetch_one(tx.as_mut())
    .await
    .map_err(CreditCommandError::Database)?;
    if command.amount > remainder {
        return Err(CreditCommandError::InsufficientRemainder);
    }

    // 5. The credit origin — an entitlement row only; NO account
    //    movement (the money already entered via the Payment).
    let insert_result: Result<(Uuid, i64), CreditCommandError> = sqlx::query_as(
        "INSERT INTO shareholder_credits \
            (source_payment_id, shareholder_id, amount, currency, status, note, \
             idempotency_key, idempotency_fingerprint, created_by) \
         VALUES ($1, $2, $3, 'TRY', 'active', $4, $5, $6, $7) \
         RETURNING id, credit_number",
    )
    .bind(command.payment_id)
    .bind(command.shareholder_id)
    .bind(command.amount)
    .bind(&command.note)
    .bind(&command.idempotency_key)
    .bind(&command.fingerprint)
    .bind(actor)
    .fetch_one(tx.as_mut())
    .await
    .map_err(|error| {
        if is_unique_violation(&error) {
            CreditCommandError::IdempotencyRace
        } else {
            CreditCommandError::Database(error)
        }
    });
    let (credit_id, credit_number) = match insert_result {
        Ok(row) => row,
        Err(CreditCommandError::IdempotencyRace) => {
            // First-writer won the race — the winner's row is now
            // committed; replay its outcome when payloads match.
            tx.rollback().await.map_err(CreditCommandError::Database)?;
            let winner: Option<(Uuid, i64, String)> = sqlx::query_as(
                "SELECT id, credit_number, idempotency_fingerprint \
                 FROM shareholder_credits WHERE idempotency_key = $1",
            )
            .bind(&command.idempotency_key)
            .fetch_optional(pool)
            .await
            .map_err(CreditCommandError::Database)?;
            return match winner {
                Some((id, number, fp)) if fp == command.fingerprint => Ok(AssignOutcome {
                    credit_id: id,
                    credit_number: number,
                    replayed: true,
                    applied_amount: Decimal::ZERO,
                    application_count: 0,
                }),
                _ => Err(CreditCommandError::IdempotencyConflict),
            };
        }
        Err(other) => return Err(other),
    };

    // 6. Auto-offset sweep: the beneficiary's EXISTING open
    //    assessments, oldest due date first then assessment id —
    //    deterministic and explainable (docs/05). Consumes the
    //    beneficiary's whole available credit FIFO, not only this
    //    new row: entitlement is a single derived pool.
    let open: Vec<Uuid> = sqlx::query_scalar(
        "SELECT a.id FROM assessments a \
         JOIN periods p ON p.id = a.period_id \
         WHERE a.shareholder_id = $1 AND a.status = 'active' \
         ORDER BY p.due_date ASC, a.id ASC",
    )
    .bind(command.shareholder_id)
    .fetch_all(tx.as_mut())
    .await
    .map_err(CreditCommandError::Database)?;
    let applied = consume_credit_fifo(
        &mut tx,
        command.shareholder_id,
        &open,
        ApplicationMode::Automatic,
        None,
        None,
        actor,
    )
    .await?;

    let applied_amount: Decimal = applied.values().sum();
    let application_count = applied.len() as i64;
    let _ = now;
    tx.commit().await.map_err(CreditCommandError::Database)?;
    Ok(AssignOutcome {
        credit_id,
        credit_number,
        replayed: false,
        applied_amount,
        application_count,
    })
}

pub struct ApplyCredit {
    pub assessment_id: Uuid,
    pub amount: Decimal,
    pub idempotency_key: String,
    pub fingerprint: String,
}

pub struct ApplyOutcome {
    pub applied_amount: Decimal,
    pub assessment_remaining: Decimal,
    pub replayed: bool,
}

/// Manual apply-credit command (docs/05: same engine as the automatic
/// path — never a second arithmetic): the Assessment's own Shareholder
/// is the only legal credit source; FIFO across their active credits.
pub async fn apply_credit(
    pool: &PgPool,
    actor: Uuid,
    command: ApplyCredit,
) -> Result<ApplyOutcome, CreditCommandError> {
    let mut tx = pool.begin().await.map_err(CreditCommandError::Database)?;

    // 1. Idempotent replay — same key + identical payload replays the
    //    recorded outcome; same key + different payload is a hard 409
    //    (ADR-006). The key is stamped on exactly ONE application row
    //    per command (UNIQUE index); the applied total equals the
    //    command amount by construction.
    let keyed: Option<(Option<String>, Uuid)> = sqlx::query_as(
        "SELECT idempotency_fingerprint, assessment_id \
         FROM credit_applications WHERE idempotency_key = $1",
    )
    .bind(&command.idempotency_key)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(CreditCommandError::Database)?;
    if let Some((fingerprint, assessment_id)) = keyed {
        if fingerprint.as_deref() != Some(command.fingerprint.as_str()) {
            tx.rollback().await.map_err(CreditCommandError::Database)?;
            return Err(CreditCommandError::IdempotencyConflict);
        }
        let assessment_amount: Decimal =
            sqlx::query_scalar("SELECT amount FROM assessments WHERE id = $1")
                .bind(assessment_id)
                .fetch_one(tx.as_mut())
                .await
                .map_err(CreditCommandError::Database)?;
        let settled = settled_map(&mut tx, &[assessment_id])
            .await
            .map_err(CreditCommandError::Database)?
            .get(&assessment_id)
            .copied()
            .unwrap_or_default();
        tx.rollback().await.map_err(CreditCommandError::Database)?;
        return Ok(ApplyOutcome {
            applied_amount: command.amount,
            assessment_remaining: assessment_amount - settled,
            replayed: true,
        });
    }

    // 2. Target: an ACTIVE assessment (never voided, never
    //    over-settled). The shareholder on the row is the ONLY legal
    //    credit owner — another shareholder's credit cannot settle it.
    let target: Option<(Uuid, Decimal, String)> =
        sqlx::query_as("SELECT shareholder_id, amount, status FROM assessments WHERE id = $1")
            .bind(command.assessment_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(CreditCommandError::Database)?;
    let Some((shareholder_id, assessment_amount, status)) = target else {
        return Err(CreditCommandError::NotFound);
    };
    if status != "active" {
        return Err(CreditCommandError::InactiveAssessment);
    }

    // 3. Lock the shareholder's credits (asc), then the assessment —
    //    the canonical credits -> assessments order.
    let credits = lock_credits(&mut tx, shareholder_id).await?;
    let credit_ids: Vec<Uuid> = credits.iter().map(|(id, _)| *id).collect();
    let used = applied_map(&mut tx, &credit_ids).await?;
    let available: Decimal = credits
        .iter()
        .map(|(id, amount)| *amount - used.get(id).copied().unwrap_or_default())
        .filter(|a| *a > Decimal::ZERO)
        .sum();
    if available < command.amount {
        return Err(CreditCommandError::InsufficientCredit);
    }

    let locked: Option<(Decimal, String)> =
        sqlx::query_as("SELECT amount, status FROM assessments WHERE id = $1 FOR UPDATE")
            .bind(command.assessment_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(CreditCommandError::Database)?;
    let Some((_amount, status)) = locked else {
        return Err(CreditCommandError::NotFound);
    };
    if status != "active" {
        return Err(CreditCommandError::InactiveAssessment);
    }
    let settled = settled_map(&mut tx, &[command.assessment_id])
        .await
        .map_err(CreditCommandError::Database)?
        .get(&command.assessment_id)
        .copied()
        .unwrap_or_default();
    let remaining = assessment_amount - settled;
    if remaining <= Decimal::ZERO {
        return Err(CreditCommandError::AssessmentSettled);
    }
    if command.amount > remaining {
        return Err(CreditCommandError::AssessmentSettled);
    }

    // 4. FIFO consumption across the shareholder's active credits;
    //    the client's key is stamped on every produced row so a
    //    network retry replays instead of double-applying.
    let applied = consume_credit_fifo(
        &mut tx,
        shareholder_id,
        &[command.assessment_id],
        ApplicationMode::Manual,
        Some(&command.idempotency_key),
        Some(&command.fingerprint),
        actor,
    )
    .await;
    let applied = match applied {
        Ok(applied) => applied,
        Err(CreditCommandError::IdempotencyRace) => {
            // Concurrent same-key command committed first — replay
            // its recorded outcome when the payload is identical.
            tx.rollback().await.map_err(CreditCommandError::Database)?;
            let winner: Option<(Option<String>, Uuid)> = sqlx::query_as(
                "SELECT idempotency_fingerprint, assessment_id \
                 FROM credit_applications WHERE idempotency_key = $1",
            )
            .bind(&command.idempotency_key)
            .fetch_optional(pool)
            .await
            .map_err(CreditCommandError::Database)?;
            return match winner {
                Some((fp, assessment_id))
                    if fp.as_deref() == Some(command.fingerprint.as_str()) =>
                {
                    let remaining: Decimal = sqlx::query_scalar(
                        "SELECT a.amount \
                             - COALESCE((SELECT sum(amount) FROM payment_allocations \
                                WHERE assessment_id = a.id AND status = 'active'), 0) \
                             - COALESCE((SELECT sum(amount) FROM credit_applications \
                                WHERE assessment_id = a.id AND status = 'active'), 0) \
                         FROM assessments a WHERE a.id = $1",
                    )
                    .bind(assessment_id)
                    .fetch_one(pool)
                    .await
                    .map_err(CreditCommandError::Database)?;
                    Ok(ApplyOutcome {
                        applied_amount: command.amount,
                        assessment_remaining: remaining,
                        replayed: true,
                    })
                }
                _ => Err(CreditCommandError::IdempotencyConflict),
            };
        }
        Err(other) => return Err(other),
    };
    let applied_total = applied
        .get(&command.assessment_id)
        .copied()
        .unwrap_or_default();
    if applied_total < command.amount {
        // Should be unreachable: availability was proven under the
        // locks — refuse rather than post a partial truth.
        return Err(CreditCommandError::InsufficientCredit);
    }

    tx.commit().await.map_err(CreditCommandError::Database)?;
    Ok(ApplyOutcome {
        applied_amount: applied_total,
        assessment_remaining: remaining - applied_total,
        replayed: false,
    })
}

/// Reverse one Credit Application (docs/19): status-based, original
/// preserved, actor/time/reason recorded. Effects: the Assessment's
/// derived remaining reopens and the credit's derived availability
/// returns — no Account Movement is ever created.
pub async fn reverse_application(
    pool: &PgPool,
    actor: Uuid,
    application_id: Uuid,
    reason: &str,
    now: OffsetDateTime,
) -> Result<bool, CreditCommandError> {
    let mut tx = pool.begin().await.map_err(CreditCommandError::Database)?;

    let application: Option<(String, Uuid, Uuid)> = sqlx::query_as(
        "SELECT status, credit_id, assessment_id FROM credit_applications \
         WHERE id = $1 FOR UPDATE",
    )
    .bind(application_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(CreditCommandError::Database)?;
    let Some((status, credit_id, assessment_id)) = application else {
        return Err(CreditCommandError::NotFound);
    };
    if status != "active" {
        tx.rollback().await.map_err(CreditCommandError::Database)?;
        return Ok(true); // idempotent replay (docs/19)
    }

    // Canonical order: credit lock, then assessment lock — both must be
    // held so a concurrent consumption/reversal serializes here.
    sqlx::query("SELECT id FROM shareholder_credits WHERE id = $1 FOR UPDATE")
        .bind(credit_id)
        .fetch_one(tx.as_mut())
        .await
        .map_err(CreditCommandError::Database)?;
    sqlx::query("SELECT id FROM assessments WHERE id = $1 FOR UPDATE")
        .bind(assessment_id)
        .fetch_one(tx.as_mut())
        .await
        .map_err(CreditCommandError::Database)?;

    sqlx::query(
        "UPDATE credit_applications SET status = 'reversed', \
            reversed_at = $2, reversed_by = $3, reversal_reason = $4 \
         WHERE id = $1",
    )
    .bind(application_id)
    .bind(now)
    .bind(actor)
    .bind(reason)
    .execute(tx.as_mut())
    .await
    .map_err(CreditCommandError::Database)?;

    tx.commit().await.map_err(CreditCommandError::Database)?;
    Ok(false)
}

/// Reverse a credit ORIGIN (correction of an erroneous assignment).
/// Rejected while the credit still funds active applications — reverse
/// those first; there is no silent cascading destruction of history.
pub async fn reverse_credit(
    pool: &PgPool,
    actor: Uuid,
    credit_id: Uuid,
    reason: &str,
    now: OffsetDateTime,
) -> Result<bool, CreditCommandError> {
    let mut tx = pool.begin().await.map_err(CreditCommandError::Database)?;

    let credit: Option<String> =
        sqlx::query_scalar("SELECT status FROM shareholder_credits WHERE id = $1 FOR UPDATE")
            .bind(credit_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(CreditCommandError::Database)?;
    let Some(status) = credit else {
        return Err(CreditCommandError::NotFound);
    };
    if status != "active" {
        tx.rollback().await.map_err(CreditCommandError::Database)?;
        return Ok(true);
    }

    let consumed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM credit_applications \
         WHERE credit_id = $1 AND status = 'active')",
    )
    .bind(credit_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(CreditCommandError::Database)?;
    if consumed {
        return Err(CreditCommandError::Consumed);
    }

    sqlx::query(
        "UPDATE shareholder_credits SET status = 'reversed', \
            reversed_at = $2, reversed_by = $3, reversal_reason = $4, updated_at = $2 \
         WHERE id = $1",
    )
    .bind(credit_id)
    .bind(now)
    .bind(actor)
    .bind(reason)
    .execute(tx.as_mut())
    .await
    .map_err(CreditCommandError::Database)?;

    tx.commit().await.map_err(CreditCommandError::Database)?;
    Ok(false)
}

/// Payment-reversal integration (docs/06: "excess effects are reversed
/// appropriately"). Called inside the payment transaction AFTER the
/// payment row is locked.
///
///   * Any ACTIVE application funded by this Payment's credits →
///     `Consumed` (409): the entitlement already settled debt — the
///     applications must be corrected first; no silent corruption.
///   * Otherwise the Payment's active credit origins are status-based
///     reversed with the same actor/time/reason.
pub async fn reconcile_credits_of_payment_reversal(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payment_id: Uuid,
    actor: Uuid,
    reason: &str,
    now: OffsetDateTime,
) -> Result<i64, CreditCommandError> {
    // Lock the payment's credit rows (canonical order credits ->
    // assessments is preserved: no assessment lock needed because we
    // refuse when ANY active application exists rather than touching
    // them).
    let credit_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM shareholder_credits \
         WHERE source_payment_id = $1 AND status = 'active' \
         ORDER BY credit_number FOR UPDATE",
    )
    .bind(payment_id)
    .fetch_all(tx.as_mut())
    .await
    .map_err(CreditCommandError::Database)?;
    if credit_ids.is_empty() {
        return Ok(0);
    }
    let consumed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM credit_applications \
         WHERE credit_id = ANY($1) AND status = 'active')",
    )
    .bind(&credit_ids)
    .fetch_one(tx.as_mut())
    .await
    .map_err(CreditCommandError::Database)?;
    if consumed {
        return Err(CreditCommandError::Consumed);
    }
    let reversed = sqlx::query(
        "UPDATE shareholder_credits SET status = 'reversed', \
            reversed_at = $2, reversed_by = $3, reversal_reason = $4, updated_at = $2 \
         WHERE id = ANY($1)",
    )
    .bind(&credit_ids)
    .bind(now)
    .bind(actor)
    .bind(reason)
    .execute(tx.as_mut())
    .await
    .map_err(CreditCommandError::Database)?
    .rows_affected();
    Ok(reversed as i64)
}
