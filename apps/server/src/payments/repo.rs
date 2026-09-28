//! Payments repository (STEP-007): derived-balance reads and the
//! transactional collection commands.
//!
//! Concurrency contract (ADR-006, docs/15):
//! - Every mutation runs in ONE PostgreSQL transaction.
//! - Commands that touch Assessments lock the target rows
//!   `FOR UPDATE ORDER BY id` — a deterministic ascending order shared
//!   by create/add-allocations, so two concurrent collections against
//!   overlapping obligations serialize instead of deadlocking or
//!   over-allocating.
//! - Commands that touch a Payment lock the Payment row first
//!   (payment -> allocations order everywhere).
//! - Remaining balances are recomputed from ACTIVE allocations INSIDE
//!   the lock — a stale preview can never over-allocate.
//! - Idempotent replay is checked before any side effect; the unique
//!   idempotency key is the durable guard against double receipts.
//!
//! Derived truth: there is no mutable `paid`/`settled` flag anywhere.
//! "What an Assessment still owes" is always
//! `assessments.amount - SUM(active allocations)` computed at read or
//! command time.

use std::collections::{HashMap, HashSet};

use rust_decimal::Decimal;
use sqlx::PgPool;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use super::model::PaymentStatus;
use crate::parties::model as party_model;
use crate::shares::repo::ShareholderIdentityRow;

// ------------------------------------------------------------------
// Read models
// ------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
pub struct PaymentRow {
    pub id: Uuid,
    pub payment_number: i64,
    pub status: String,
    pub amount: Decimal,
    pub currency: String,
    pub method: String,
    pub received_at: OffsetDateTime,
    pub note: Option<String>,
    pub payer_person_id: Uuid,
    pub payer_first_name: String,
    pub payer_last_name: String,
    /// Set when the payer Person happens to be a Shareholder — context
    /// only; payer identity never implies debtor identity (§12–§14).
    pub payer_shareholder_id: Option<Uuid>,
    /// Derived: SUM of ACTIVE allocations (never a stored flag).
    pub allocated_amount: Decimal,
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
    pub created_by_name: Option<String>,
    pub reversed_by_name: Option<String>,
    pub created_at: OffsetDateTime,
    pub total_count: i64,
}

const PAYMENT_SELECT: &str = "SELECT pay.id, pay.payment_number, pay.status, \
    pay.amount, pay.currency, pay.method, pay.received_at, pay.note, \
    pp.id AS payer_person_id, pp.first_name AS payer_first_name, \
    pp.last_name AS payer_last_name, \
    ps.id AS payer_shareholder_id, \
    COALESCE(al.allocated, 0) AS allocated_amount, \
    pay.reversed_at, pay.reversal_reason, \
    cb.display_name AS created_by_name, rb.display_name AS reversed_by_name, \
    pay.created_at \
FROM payments pay \
JOIN persons pp ON pp.id = pay.payer_person_id \
LEFT JOIN shareholders ps ON ps.person_id = pp.id \
LEFT JOIN ( \
    SELECT payment_id, sum(amount) AS allocated \
    FROM payment_allocations WHERE status = 'active' GROUP BY payment_id \
) al ON al.payment_id = pay.id \
LEFT JOIN users cb ON cb.id = pay.created_by \
LEFT JOIN users rb ON rb.id = pay.reversed_by ";

/// Payment list (§60): search folds over payer name, debtor name or an
/// exact business payment number. Newest first (business number order).
pub async fn list_payments(
    pool: &PgPool,
    search: Option<&str>,
    page: i64,
    page_size: i64,
) -> Result<Vec<PaymentRow>, sqlx::Error> {
    let raw = search.map(str::trim).filter(|s| !s.is_empty());
    let folded: Option<String> = raw.map(|term| format!("%{}%", party_model::fold_search(term)));
    let number: Option<i64> = raw
        .and_then(|term| term.parse::<i64>().ok())
        .filter(|n| *n > 0);

    let sql = format!(
        "SELECT *, count(*) OVER () AS total_count FROM ({PAYMENT_SELECT}) base \
         WHERE ( \
            $1::text IS NULL \
            OR ($3::bigint IS NOT NULL AND base.payment_number = $3) \
            OR EXISTS ( \
                SELECT 1 FROM persons sp WHERE sp.id = base.payer_person_id \
                AND sp.search_name LIKE $1 \
            ) \
            OR EXISTS ( \
                SELECT 1 FROM payment_allocations al \
                JOIN assessments a ON a.id = al.assessment_id \
                JOIN shareholders s ON s.id = a.shareholder_id \
                JOIN persons dp ON dp.id = s.person_id \
                WHERE al.payment_id = base.id AND dp.search_name LIKE $1 \
            ) \
         ) \
         ORDER BY base.payment_number DESC \
         OFFSET $2 ROWS FETCH NEXT $4 ROWS ONLY"
    );
    sqlx::query_as::<_, PaymentRow>(&sql)
        .bind(folded)
        .bind((page - 1) * page_size)
        .bind(number)
        .bind(page_size)
        .fetch_all(pool)
        .await
}

pub async fn find_payment(pool: &PgPool, id: Uuid) -> Result<Option<PaymentRow>, sqlx::Error> {
    let sql = format!(
        "SELECT *, 0::bigint AS total_count FROM ({PAYMENT_SELECT} WHERE pay.id = $1) base"
    );
    sqlx::query_as::<_, PaymentRow>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// One allocation line with full obligation context: which Period,
/// which debtor (canonical identity) and how much of this Payment was
/// applied. `status`/`reversed_*` carry the reversal history.
#[derive(Debug, sqlx::FromRow)]
pub struct AllocationRow {
    pub id: Uuid,
    pub payment_id: Uuid,
    pub assessment_id: Uuid,
    pub period_id: Uuid,
    pub period_number: i64,
    pub period_name: String,
    pub assessment_amount: Decimal,
    pub amount: Decimal,
    pub status: String,
    #[sqlx(flatten)]
    pub debtor: ShareholderIdentityRow,
    pub created_at: OffsetDateTime,
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
}

const ALLOCATION_SELECT: &str = "SELECT al.id, al.payment_id, al.assessment_id, \
    a.period_id, p.period_number, p.name AS period_name, \
    a.amount AS assessment_amount, al.amount, al.status, \
    s.id AS shareholder_id, sp.first_name, sp.last_name, \
    gp.first_name AS guardian_first_name, gp.last_name AS guardian_last_name, \
    f.sequence_number AS family_sequence, s.status AS shareholder_status, \
    al.created_at, al.reversed_at, al.reversal_reason \
FROM payment_allocations al \
JOIN assessments a ON a.id = al.assessment_id \
JOIN periods p ON p.id = a.period_id \
JOIN shareholders s ON s.id = a.shareholder_id \
JOIN persons sp ON sp.id = s.person_id \
LEFT JOIN persons gp ON gp.id = s.guardian_person_id \
LEFT JOIN shareholder_family_memberships m \
    ON m.shareholder_id = s.id AND m.ended_at IS NULL \
LEFT JOIN families f ON f.id = m.family_id ";

/// All allocation lines of one Payment — both active and reversed so
/// the detail surface shows complete history (docs/19).
pub async fn payment_allocations(
    pool: &PgPool,
    payment_id: Uuid,
) -> Result<Vec<AllocationRow>, sqlx::Error> {
    let sql = format!(
        "{ALLOCATION_SELECT} WHERE al.payment_id = $1 \
         ORDER BY p.period_number, al.id"
    );
    sqlx::query_as::<_, AllocationRow>(&sql)
        .bind(payment_id)
        .fetch_all(pool)
        .await
}

/// Payment history of one Assessment: every application of received
/// value (active + reversed), newest first.
#[derive(Debug, sqlx::FromRow)]
pub struct AssessmentPaymentRow {
    pub allocation_id: Uuid,
    pub payment_id: Uuid,
    pub payment_number: i64,
    pub payment_status: String,
    pub amount: Decimal,
    pub currency: String,
    pub allocation_status: String,
    pub received_at: OffsetDateTime,
    pub payer_full_name: String,
    pub payer_shareholder_id: Option<Uuid>,
    pub created_at: OffsetDateTime,
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
}

pub async fn assessment_payments(
    pool: &PgPool,
    assessment_id: Uuid,
) -> Result<Vec<AssessmentPaymentRow>, sqlx::Error> {
    sqlx::query_as::<_, AssessmentPaymentRow>(
        "SELECT al.id AS allocation_id, pay.id AS payment_id, pay.payment_number, \
            pay.status AS payment_status, al.amount, pay.currency, \
            al.status AS allocation_status, pay.received_at, \
            (pp.first_name || ' ' || pp.last_name) AS payer_full_name, \
            ps.id AS payer_shareholder_id, \
            al.created_at, al.reversed_at, al.reversal_reason \
         FROM payment_allocations al \
         JOIN payments pay ON pay.id = al.payment_id \
         JOIN persons pp ON pp.id = pay.payer_person_id \
         LEFT JOIN shareholders ps ON ps.person_id = pp.id \
         WHERE al.assessment_id = $1 \
         ORDER BY al.created_at DESC, al.id",
    )
    .bind(assessment_id)
    .fetch_all(pool)
    .await
}

/// A Shareholder's obligations with derived settlement (§62 extension):
/// `paid` = SUM(active allocations), `remaining` = amount - paid.
/// Active assessments only; ordering by period recency.
#[derive(Debug, sqlx::FromRow)]
pub struct OpenAssessmentRow {
    pub id: Uuid,
    pub period_id: Uuid,
    pub period_number: i64,
    pub period_name: String,
    pub due_date: Date,
    pub amount: Decimal,
    pub currency: String,
    pub paid_amount: Decimal,
    pub remaining_amount: Decimal,
    pub generated_at: OffsetDateTime,
}

/// Debtor-selection rows for the collection flow AND the shareholder
/// payment surface: every ACTIVE assessment with its derived settled /
/// remaining amounts.
pub async fn shareholder_open_assessments(
    pool: &PgPool,
    shareholder_id: Uuid,
) -> Result<Vec<OpenAssessmentRow>, sqlx::Error> {
    sqlx::query_as::<_, OpenAssessmentRow>(
        "SELECT a.id, a.period_id, p.period_number, p.name AS period_name, \
            p.due_date, a.amount, a.currency, a.generated_at, \
            COALESCE(al.paid, 0) AS paid_amount, \
            a.amount - COALESCE(al.paid, 0) AS remaining_amount \
         FROM assessments a \
         JOIN periods p ON p.id = a.period_id \
         LEFT JOIN ( \
             SELECT assessment_id, sum(amount) AS paid \
             FROM payment_allocations WHERE status = 'active' GROUP BY assessment_id \
         ) al ON al.assessment_id = a.id \
         WHERE a.shareholder_id = $1 AND a.status = 'active' \
         ORDER BY p.period_number DESC, a.id",
    )
    .bind(shareholder_id)
    .fetch_all(pool)
    .await
}

/// Shareholder-level financial summary (docs/06/§62): obligations vs
/// settled value across ALL periods — never a stored balance.
#[derive(Debug, sqlx::FromRow)]
pub struct FinancialSummaryRow {
    pub assessment_count: i64,
    pub open_assessment_count: i64,
    pub total_assessed: Decimal,
    pub total_paid: Decimal,
    pub total_remaining: Decimal,
}

pub async fn shareholder_financial_summary(
    pool: &PgPool,
    shareholder_id: Uuid,
) -> Result<FinancialSummaryRow, sqlx::Error> {
    sqlx::query_as::<_, FinancialSummaryRow>(
        "SELECT count(*) AS assessment_count, \
            count(*) FILTER (WHERE a.amount - COALESCE(al.paid, 0) > 0) AS open_assessment_count, \
            COALESCE(sum(a.amount), 0) AS total_assessed, \
            COALESCE(sum(COALESCE(al.paid, 0)), 0) AS total_paid, \
            COALESCE(sum(a.amount - COALESCE(al.paid, 0)), 0) AS total_remaining \
         FROM assessments a \
         LEFT JOIN ( \
             SELECT assessment_id, sum(amount) AS paid \
             FROM payment_allocations WHERE status = 'active' GROUP BY assessment_id \
         ) al ON al.assessment_id = a.id \
         WHERE a.shareholder_id = $1 AND a.status = 'active'",
    )
    .bind(shareholder_id)
    .fetch_one(pool)
    .await
}

/// Period-level collection summary (docs/06 §68): what the Period
/// assessed, what has been settled so far and what remains — plus the
/// distinct Payments that contributed.
#[derive(Debug, sqlx::FromRow)]
pub struct PeriodCollectionSummaryRow {
    pub assessment_count: i64,
    pub total_assessed: Decimal,
    pub total_collected: Decimal,
    pub total_remaining: Decimal,
    pub contributing_payment_count: i64,
}

pub async fn period_financial_summary(
    pool: &PgPool,
    period_id: Uuid,
) -> Result<PeriodCollectionSummaryRow, sqlx::Error> {
    sqlx::query_as::<_, PeriodCollectionSummaryRow>(
        "SELECT count(*) AS assessment_count, \
            COALESCE(sum(a.amount), 0) AS total_assessed, \
            COALESCE(sum(COALESCE(al.paid, 0)), 0) AS total_collected, \
            COALESCE(sum(a.amount - COALESCE(al.paid, 0)), 0) AS total_remaining, \
            (SELECT count(DISTINCT pa.payment_id) \
             FROM payment_allocations pa \
             JOIN assessments a2 ON a2.id = pa.assessment_id \
             WHERE a2.period_id = $1 AND pa.status = 'active') AS contributing_payment_count \
         FROM assessments a \
         LEFT JOIN ( \
             SELECT assessment_id, sum(amount) AS paid \
             FROM payment_allocations WHERE status = 'active' GROUP BY assessment_id \
         ) al ON al.assessment_id = a.id \
         WHERE a.period_id = $1 AND a.status = 'active'",
    )
    .bind(period_id)
    .fetch_one(pool)
    .await
}

/// One current Family member's collection context: canonical identity
/// plus derived open-obligation totals. The Family itself is NEVER the
/// debtor — every line names its own Shareholder.
#[derive(Debug, sqlx::FromRow)]
pub struct FamilyMemberCollectionRow {
    #[sqlx(flatten)]
    pub member: ShareholderIdentityRow,
    pub open_assessment_count: i64,
    pub remaining_amount: Decimal,
}

pub async fn family_collection_context(
    pool: &PgPool,
    family_id: Uuid,
) -> Result<Vec<FamilyMemberCollectionRow>, sqlx::Error> {
    sqlx::query_as::<_, FamilyMemberCollectionRow>(
        "SELECT s.id AS shareholder_id, sp.first_name, sp.last_name, \
            gp.first_name AS guardian_first_name, gp.last_name AS guardian_last_name, \
            f.sequence_number AS family_sequence, s.status AS shareholder_status, \
            COALESCE(o.cnt, 0)::bigint AS open_assessment_count, \
            COALESCE(o.remaining, 0) AS remaining_amount \
         FROM shareholder_family_memberships m \
         JOIN families f ON f.id = m.family_id \
         JOIN shareholders s ON s.id = m.shareholder_id AND s.status <> 'voided' \
         JOIN persons sp ON sp.id = s.person_id \
         LEFT JOIN persons gp ON gp.id = s.guardian_person_id \
         LEFT JOIN ( \
             SELECT a.shareholder_id, \
                 count(*) FILTER (WHERE a.amount - COALESCE(al.paid, 0) > 0) AS cnt, \
                 sum(a.amount - COALESCE(al.paid, 0)) AS remaining \
             FROM assessments a \
             LEFT JOIN ( \
                 SELECT assessment_id, sum(amount) AS paid \
                 FROM payment_allocations WHERE status = 'active' GROUP BY assessment_id \
             ) al ON al.assessment_id = a.id \
             WHERE a.status = 'active' \
             GROUP BY a.shareholder_id \
         ) o ON o.shareholder_id = s.id \
         WHERE m.family_id = $1 AND m.ended_at IS NULL \
         ORDER BY sp.search_name, s.id",
    )
    .bind(family_id)
    .fetch_all(pool)
    .await
}

// ------------------------------------------------------------------
// Transactional commands
// ------------------------------------------------------------------

pub struct NewAllocation {
    pub assessment_id: Uuid,
    pub amount: Decimal,
}

pub struct CreatePayment {
    /// Payer resolution: an existing Person id OR a new person name
    /// (first+last required together) — validated by the handler.
    pub payer_person_id: Option<Uuid>,
    pub payer_first_name: Option<String>,
    pub payer_last_name: Option<String>,
    pub amount: Decimal,
    pub method: String,
    pub received_at: OffsetDateTime,
    pub note: Option<String>,
    pub idempotency_key: String,
    /// Canonical payload fingerprint stored beside the key (ADR-006).
    pub fingerprint: String,
    pub allocations: Vec<NewAllocation>,
}

#[derive(Debug)]
pub enum PaymentCommandError {
    /// Payment/allocation row referenced by the path does not exist.
    NotFound,
    /// Payer Person id referenced by the payload does not exist.
    PayerNotFound,
    /// An allocation target is missing or not `active` (voided
    /// assessments can never receive value).
    InactiveAssessment,
    /// Two lines of the same command target the same assessment.
    DuplicateTarget,
    /// The (payment, assessment) pair already has an allocation line
    /// (add-allocations stale retry) — 409.
    TargetAlreadyAllocated,
    /// Σ allocations exceeds the Payment's amount — deterministic
    /// request error, never a race.
    PaymentOverAllocated,
    /// An allocation exceeds the target's remaining balance —
    /// race-sensitive, always a 409.
    AssessmentOverAllocated,
    /// Same idempotency key arrived with a different payload.
    IdempotencyConflict,
    /// The Payment is already `reversed` where `posted` was required
    /// (add-allocations on a reversed Payment) — 409.
    NotPosted,
    Database(sqlx::Error),
}

pub struct PaymentOutcome {
    pub payment_id: Uuid,
    pub payment_number: i64,
    /// True when the request replayed an existing idempotency key
    /// byte-identically — nothing was re-created.
    pub replayed: bool,
    pub payer_person_id: Uuid,
    pub created_person: bool,
    pub allocated_amount: Decimal,
    pub unallocated_amount: Decimal,
    pub allocation_count: i64,
}

/// Resolve the payer inside the transaction: an existing Person or a
/// newly created one. Creating a Person here records WHO paid — it
/// never creates a Shareholder or any debtor role.
async fn resolve_payer(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &CreatePayment,
) -> Result<(Uuid, bool), PaymentCommandError> {
    if let Some(id) = command.payer_person_id {
        let exists: Option<Uuid> = sqlx::query_scalar("SELECT id FROM persons WHERE id = $1")
            .bind(id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(PaymentCommandError::Database)?;
        return Ok((exists.ok_or(PaymentCommandError::PayerNotFound)?, false));
    }
    let (Some(first), Some(last)) = (&command.payer_first_name, &command.payer_last_name) else {
        return Err(PaymentCommandError::PayerNotFound);
    };
    let search = party_model::fold_search(&format!("{first} {last}"));
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO persons (first_name, last_name, search_name) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(first)
    .bind(last)
    .bind(&search)
    .fetch_one(tx.as_mut())
    .await
    .map_err(PaymentCommandError::Database)?;
    Ok((id, true))
}

/// Lock the target Assessments `FOR UPDATE` in ascending id order —
/// the deterministic order shared by every allocation command. Returns
/// (id, amount, status) keyed by id.
async fn lock_assessments(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, (Decimal, String)>, PaymentCommandError> {
    let rows: Vec<(Uuid, Decimal, String)> = sqlx::query_as(
        "SELECT id, amount, status FROM assessments \
         WHERE id = ANY($1) ORDER BY id FOR UPDATE",
    )
    .bind(ids)
    .fetch_all(tx.as_mut())
    .await
    .map_err(PaymentCommandError::Database)?;
    Ok(rows.into_iter().map(|(id, a, s)| (id, (a, s))).collect())
}

/// SUM of ACTIVE allocations per assessment — recomputed under the row
/// locks, never trusted from a preview.
async fn paid_map(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, Decimal>, PaymentCommandError> {
    let rows: Vec<(Uuid, Decimal)> = sqlx::query_as(
        "SELECT assessment_id, sum(amount) FROM payment_allocations \
         WHERE assessment_id = ANY($1) AND status = 'active' \
         GROUP BY assessment_id",
    )
    .bind(ids)
    .fetch_all(tx.as_mut())
    .await
    .map_err(PaymentCommandError::Database)?;
    Ok(rows.into_iter().collect())
}

/// Validate a set of NEW allocations against freshly locked
/// assessments: targets exist and are `active`, each amount fits the
/// remaining balance, and the total fits the Payment's available
/// (unallocated) amount.
fn validate_allocations(
    allocations: &[NewAllocation],
    locked: &HashMap<Uuid, (Decimal, String)>,
    paid: &HashMap<Uuid, Decimal>,
    available: Decimal,
) -> Result<(), PaymentCommandError> {
    let mut total = Decimal::ZERO;
    for allocation in allocations {
        let Some((assessment_amount, status)) = locked.get(&allocation.assessment_id) else {
            return Err(PaymentCommandError::InactiveAssessment);
        };
        if status != "active" {
            return Err(PaymentCommandError::InactiveAssessment);
        }
        let already_paid = paid
            .get(&allocation.assessment_id)
            .copied()
            .unwrap_or_default();
        let remaining = *assessment_amount - already_paid;
        if allocation.amount > remaining {
            return Err(PaymentCommandError::AssessmentOverAllocated);
        }
        total += allocation.amount;
    }
    if total > available {
        return Err(PaymentCommandError::PaymentOverAllocated);
    }
    Ok(())
}

/// The collection command (docs/06): resolve payer -> dedupe targets ->
/// lock assessments in order -> recompute balances under locks ->
/// insert Payment + Allocations atomically. An unallocated remainder
/// is permitted: it stays ON the Payment as `unallocated` value awaiting
/// explicit disposition, never attributed to a debtor or member.
///
/// Idempotency (ADR-006): the unique `idempotency_key` is checked
/// BEFORE side effects; a same-fingerprint replay returns the existing
/// Payment (`replayed`), a different payload conflicts (409). A
/// concurrent first-writer is handled by ON CONFLICT re-read.
pub async fn create_payment(
    pool: &PgPool,
    actor: Uuid,
    command: CreatePayment,
) -> Result<PaymentOutcome, PaymentCommandError> {
    let mut tx = pool.begin().await.map_err(PaymentCommandError::Database)?;

    // 1. Idempotent replay short-circuit — before ANY side effect.
    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, payment_number, idempotency_fingerprint FROM payments \
         WHERE idempotency_key = $1",
    )
    .bind(&command.idempotency_key)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(PaymentCommandError::Database)?;
    if let Some((id, number, fingerprint)) = existing {
        if fingerprint != command.fingerprint {
            return Err(PaymentCommandError::IdempotencyConflict);
        }
        let allocated: Decimal = sqlx::query_scalar(
            "SELECT COALESCE(sum(amount), 0) FROM payment_allocations \
             WHERE payment_id = $1 AND status = 'active'",
        )
        .bind(id)
        .fetch_one(tx.as_mut())
        .await
        .map_err(PaymentCommandError::Database)?;
        tx.rollback().await.map_err(PaymentCommandError::Database)?;
        return Ok(PaymentOutcome {
            payment_id: id,
            payment_number: number,
            replayed: true,
            payer_person_id: command.payer_person_id.unwrap_or_default(),
            created_person: false,
            allocated_amount: allocated,
            unallocated_amount: command.amount - allocated,
            allocation_count: 0,
        });
    }

    // 2. Payer resolution (existing Person or new — never a debtor).
    let (payer_person_id, created_person) = resolve_payer(&mut tx, &command).await?;

    // 3. Request-level target dedupe, then deterministic locking.
    let mut ids: Vec<Uuid> = command
        .allocations
        .iter()
        .map(|a| a.assessment_id)
        .collect();
    if ids.len() != ids.iter().copied().collect::<HashSet<_>>().len() {
        return Err(PaymentCommandError::DuplicateTarget);
    }
    ids.sort_unstable();
    let locked = lock_assessments(&mut tx, &ids).await?;
    let paid = paid_map(&mut tx, &ids).await?;
    validate_allocations(&command.allocations, &locked, &paid, command.amount)?;

    // 4. Insert the Payment (posted at birth — one receipt = one row).
    let inserted: Option<(Uuid, i64)> = sqlx::query_as(
        "INSERT INTO payments \
             (payer_person_id, amount, currency, method, received_at, note, \
              status, idempotency_key, idempotency_fingerprint, created_by) \
         VALUES ($1, $2, 'TRY', $3, $4, $5, 'posted', $6, $7, $8) \
         ON CONFLICT (idempotency_key) DO NOTHING \
         RETURNING id, payment_number",
    )
    .bind(payer_person_id)
    .bind(command.amount)
    .bind(&command.method)
    .bind(command.received_at)
    .bind(&command.note)
    .bind(&command.idempotency_key)
    .bind(&command.fingerprint)
    .bind(actor)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(PaymentCommandError::Database)?;

    // A concurrent first-writer committed between our early check and
    // the insert: re-read and apply the same replay/conflict logic.
    let (payment_id, payment_number) = match inserted {
        Some(row) => row,
        None => {
            tx.rollback().await.map_err(PaymentCommandError::Database)?;
            let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
                "SELECT id, payment_number, idempotency_fingerprint FROM payments \
                 WHERE idempotency_key = $1",
            )
            .bind(&command.idempotency_key)
            .fetch_optional(pool)
            .await
            .map_err(PaymentCommandError::Database)?;
            let Some((id, number, fingerprint)) = existing else {
                return Err(PaymentCommandError::NotFound);
            };
            if fingerprint != command.fingerprint {
                return Err(PaymentCommandError::IdempotencyConflict);
            }
            let allocated: Decimal = sqlx::query_scalar(
                "SELECT COALESCE(sum(amount), 0) FROM payment_allocations \
                 WHERE payment_id = $1 AND status = 'active'",
            )
            .bind(id)
            .fetch_one(pool)
            .await
            .map_err(PaymentCommandError::Database)?;
            let allocation_count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM payment_allocations \
                 WHERE payment_id = $1 AND status = 'active'",
            )
            .bind(id)
            .fetch_one(pool)
            .await
            .map_err(PaymentCommandError::Database)?;
            return Ok(PaymentOutcome {
                payment_id: id,
                payment_number: number,
                replayed: true,
                payer_person_id,
                created_person: false,
                allocated_amount: allocated,
                unallocated_amount: command.amount - allocated,
                allocation_count,
            });
        }
    };

    // 5. Allocation lines — same transaction, all-or-nothing.
    for allocation in &command.allocations {
        sqlx::query(
            "INSERT INTO payment_allocations \
                 (payment_id, assessment_id, amount, created_by) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(payment_id)
        .bind(allocation.assessment_id)
        .bind(allocation.amount)
        .bind(actor)
        .execute(tx.as_mut())
        .await
        .map_err(PaymentCommandError::Database)?;
    }

    let allocated: Decimal = command.allocations.iter().map(|a| a.amount).sum();
    tx.commit().await.map_err(PaymentCommandError::Database)?;
    Ok(PaymentOutcome {
        payment_id,
        payment_number,
        replayed: false,
        payer_person_id,
        created_person,
        allocated_amount: allocated,
        unallocated_amount: command.amount - allocated,
        allocation_count: command.allocations.len() as i64,
    })
}

/// Apply additional allocations from a posted Payment's unallocated
/// remainder (docs/06 disposition path). Same locking/validation
/// contract as creation; a (payment, assessment) pair can only ever
/// receive ONE line.
pub async fn add_allocations(
    pool: &PgPool,
    actor: Uuid,
    payment_id: Uuid,
    allocations: Vec<NewAllocation>,
) -> Result<PaymentOutcome, PaymentCommandError> {
    let mut tx = pool.begin().await.map_err(PaymentCommandError::Database)?;

    // Payment lock FIRST — the shared lock prefix of every
    // payment-scoped mutation.
    let payment: Option<(String, Decimal)> =
        sqlx::query_as("SELECT status, amount FROM payments WHERE id = $1 FOR UPDATE")
            .bind(payment_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(PaymentCommandError::Database)?;
    let Some((status, amount)) = payment else {
        return Err(PaymentCommandError::NotFound);
    };
    if status != PaymentStatus::Posted.as_str() {
        return Err(PaymentCommandError::NotPosted);
    }

    let allocated: Decimal = sqlx::query_scalar(
        "SELECT COALESCE(sum(amount), 0) FROM payment_allocations \
         WHERE payment_id = $1 AND status = 'active'",
    )
    .bind(payment_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(PaymentCommandError::Database)?;

    // Existing targets can never receive a second line.
    let existing: HashSet<Uuid> = sqlx::query_scalar::<_, Uuid>(
        "SELECT assessment_id FROM payment_allocations WHERE payment_id = $1",
    )
    .bind(payment_id)
    .fetch_all(tx.as_mut())
    .await
    .map_err(PaymentCommandError::Database)?
    .into_iter()
    .collect();

    let mut ids: Vec<Uuid> = allocations.iter().map(|a| a.assessment_id).collect();
    if ids.len() != ids.iter().copied().collect::<HashSet<_>>().len() {
        return Err(PaymentCommandError::DuplicateTarget);
    }
    if ids.iter().any(|id| existing.contains(id)) {
        return Err(PaymentCommandError::TargetAlreadyAllocated);
    }
    ids.sort_unstable();
    let locked = lock_assessments(&mut tx, &ids).await?;
    let paid = paid_map(&mut tx, &ids).await?;
    validate_allocations(&allocations, &locked, &paid, amount - allocated)?;

    for allocation in &allocations {
        sqlx::query(
            "INSERT INTO payment_allocations \
                 (payment_id, assessment_id, amount, created_by) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(payment_id)
        .bind(allocation.assessment_id)
        .bind(allocation.amount)
        .bind(actor)
        .execute(tx.as_mut())
        .await
        .map_err(PaymentCommandError::Database)?;
    }

    let added: Decimal = allocations.iter().map(|a| a.amount).sum();
    tx.commit().await.map_err(PaymentCommandError::Database)?;
    Ok(PaymentOutcome {
        payment_id,
        payment_number: 0,
        replayed: false,
        payer_person_id: Uuid::nil(),
        created_person: false,
        allocated_amount: allocated + added,
        unallocated_amount: amount - allocated - added,
        allocation_count: allocations.len() as i64,
    })
}

pub struct ReverseOutcome {
    /// True when the Payment/allocation was ALREADY reversed — a retried
    /// reversal replays idempotently instead of erroring (docs/19).
    pub replayed: bool,
    pub reversed_allocation_count: i64,
    pub payment_number: i64,
}

/// Whole-Payment reversal (docs/19): the original row stays, marked
/// `reversed` with actor/time/reason, and every ACTIVE allocation is
/// reversed with the same reason. Never a DELETE. Repeating the command
/// is an idempotent replay.
pub async fn reverse_payment(
    pool: &PgPool,
    actor: Uuid,
    payment_id: Uuid,
    reason: String,
    now: OffsetDateTime,
) -> Result<ReverseOutcome, PaymentCommandError> {
    let mut tx = pool.begin().await.map_err(PaymentCommandError::Database)?;

    let payment: Option<(String, i64)> =
        sqlx::query_as("SELECT status, payment_number FROM payments WHERE id = $1 FOR UPDATE")
            .bind(payment_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(PaymentCommandError::Database)?;
    let Some((status, payment_number)) = payment else {
        return Err(PaymentCommandError::NotFound);
    };
    if status != PaymentStatus::Posted.as_str() {
        tx.rollback().await.map_err(PaymentCommandError::Database)?;
        return Ok(ReverseOutcome {
            replayed: true,
            reversed_allocation_count: 0,
            payment_number,
        });
    }

    let reversed = sqlx::query(
        "UPDATE payment_allocations SET status = 'reversed', \
            reversed_at = $2, reversed_by = $3, reversal_reason = $4 \
         WHERE payment_id = $1 AND status = 'active'",
    )
    .bind(payment_id)
    .bind(now)
    .bind(actor)
    .bind(&reason)
    .execute(tx.as_mut())
    .await
    .map_err(PaymentCommandError::Database)?
    .rows_affected();

    sqlx::query(
        "UPDATE payments SET status = 'reversed', reversed_at = $2, \
            reversed_by = $3, reversal_reason = $4, updated_at = $2 \
         WHERE id = $1",
    )
    .bind(payment_id)
    .bind(now)
    .bind(actor)
    .bind(&reason)
    .execute(tx.as_mut())
    .await
    .map_err(PaymentCommandError::Database)?;

    tx.commit().await.map_err(PaymentCommandError::Database)?;
    Ok(ReverseOutcome {
        replayed: false,
        reversed_allocation_count: reversed as i64,
        payment_number,
    })
}

/// Single-allocation reversal (fine-grained correction): the rest of
/// the Payment keeps satisfying its other obligations. Same payment ->
/// allocation lock prefix as every other command.
pub async fn reverse_allocation(
    pool: &PgPool,
    actor: Uuid,
    payment_id: Uuid,
    allocation_id: Uuid,
    reason: String,
    now: OffsetDateTime,
) -> Result<ReverseOutcome, PaymentCommandError> {
    let mut tx = pool.begin().await.map_err(PaymentCommandError::Database)?;

    let payment: Option<(String, i64)> =
        sqlx::query_as("SELECT status, payment_number FROM payments WHERE id = $1 FOR UPDATE")
            .bind(payment_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(PaymentCommandError::Database)?;
    let Some((_status, payment_number)) = payment else {
        return Err(PaymentCommandError::NotFound);
    };

    let allocation: Option<(String,)> = sqlx::query_as(
        "SELECT status FROM payment_allocations \
         WHERE id = $1 AND payment_id = $2 FOR UPDATE",
    )
    .bind(allocation_id)
    .bind(payment_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(PaymentCommandError::Database)?;
    let Some((allocation_status,)) = allocation else {
        return Err(PaymentCommandError::NotFound);
    };
    if allocation_status != "active" {
        tx.rollback().await.map_err(PaymentCommandError::Database)?;
        return Ok(ReverseOutcome {
            replayed: true,
            reversed_allocation_count: 0,
            payment_number,
        });
    }

    sqlx::query(
        "UPDATE payment_allocations SET status = 'reversed', \
            reversed_at = $2, reversed_by = $3, reversal_reason = $4 \
         WHERE id = $1",
    )
    .bind(allocation_id)
    .bind(now)
    .bind(actor)
    .bind(&reason)
    .execute(tx.as_mut())
    .await
    .map_err(PaymentCommandError::Database)?;

    tx.commit().await.map_err(PaymentCommandError::Database)?;
    Ok(ReverseOutcome {
        replayed: false,
        reversed_allocation_count: 1,
        payment_number,
    })
}
