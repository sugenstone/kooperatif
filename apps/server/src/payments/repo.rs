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
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use super::model::PaymentStatus;
use crate::credits::repo as credit_repo;
use crate::financial_accounts::model::{MovementDirection, MovementSource};
use crate::financial_accounts::repo as account_repo;
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
    /// STEP-008: where the value is held. NULL only on rows created
    /// before the Financial Accounts domain existed.
    pub destination_account_id: Option<Uuid>,
    pub destination_account_name: Option<String>,
    /// Derived: SUM of ACTIVE allocations (never a stored flag).
    pub allocated_amount: Decimal,
    /// STEP-009 derived: SUM of ACTIVE Shareholder Credits sourced
    /// from this Payment's remainder.
    pub credited_amount: Decimal,
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
    pay.destination_account_id, fa.name AS destination_account_name, \
    COALESCE(al.allocated, 0) AS allocated_amount, \
    COALESCE(cr.credited, 0) AS credited_amount, \
    pay.reversed_at, pay.reversal_reason, \
    cb.display_name AS created_by_name, rb.display_name AS reversed_by_name, \
    pay.created_at \
FROM payments pay \
JOIN persons pp ON pp.id = pay.payer_person_id \
LEFT JOIN shareholders ps ON ps.person_id = pp.id \
LEFT JOIN financial_accounts fa ON fa.id = pay.destination_account_id \
LEFT JOIN ( \
    SELECT payment_id, sum(amount) AS allocated \
    FROM payment_allocations WHERE status = 'active' GROUP BY payment_id \
) al ON al.payment_id = pay.id \
LEFT JOIN ( \
    SELECT source_payment_id, sum(amount) AS credited \
    FROM shareholder_credits WHERE status = 'active' GROUP BY source_payment_id \
) cr ON cr.source_payment_id = pay.id \
LEFT JOIN users cb ON cb.id = pay.created_by \
LEFT JOIN users rb ON rb.id = pay.reversed_by ";

/// Payment list (§60): search folds over payer name, debtor name or an
/// exact business payment number. Newest first (business number order).
///
/// `cooperative_id` is the verified business context (M1-P2-C3):
/// `Some` scopes to that cooperative; `None` is reachable only in the
/// proven never-bootstrapped lane and matches only legacy NULL rows
/// (`IS NOT DISTINCT FROM`) — never a global listing. The indirect
/// debtor search EXISTS stays reachable only through in-scope payment
/// rows.
pub async fn list_payments(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    search: Option<&str>,
    page: i64,
    page_size: i64,
) -> Result<Vec<PaymentRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    let raw = search.map(str::trim).filter(|s| !s.is_empty());
    let folded: Option<String> = raw.map(|term| format!("%{}%", party_model::fold_search(term)));
    let number: Option<i64> = raw
        .and_then(|term| term.parse::<i64>().ok())
        .filter(|n| *n > 0);

    let sql = format!(
        "SELECT *, count(*) OVER () AS total_count FROM (\
            {PAYMENT_SELECT} WHERE pay.cooperative_id IS NOT DISTINCT FROM $5\
         ) base \
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
        .bind(cooperative_id)
        .fetch_all(&mut *conn)
        .await
}

/// Cooperative-scoped detail (C3): foreign payment ids return `None`.
pub async fn find_payment(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    id: Uuid,
) -> Result<Option<PaymentRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    let sql = format!(
        "SELECT *, 0::bigint AS total_count FROM (\
            {PAYMENT_SELECT} WHERE pay.id = $1 \
            AND pay.cooperative_id IS NOT DISTINCT FROM $2\
         ) base"
    );
    sqlx::query_as::<_, PaymentRow>(&sql)
        .bind(id)
        .bind(cooperative_id)
        .fetch_optional(&mut *conn)
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
/// Cooperative-scoped (C3): allocations of a foreign payment yield an
/// empty list — never its settlement history.
pub async fn payment_allocations(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    payment_id: Uuid,
) -> Result<Vec<AllocationRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    let sql = format!(
        "{ALLOCATION_SELECT} WHERE al.payment_id = $1 \
         AND al.cooperative_id IS NOT DISTINCT FROM $2 \
         ORDER BY p.period_number, al.id"
    );
    sqlx::query_as::<_, AllocationRow>(&sql)
        .bind(payment_id)
        .bind(cooperative_id)
        .fetch_all(&mut *conn)
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

/// Cooperative-scoped (C3): settlement history of a foreign
/// assessment yields an empty list.
pub async fn assessment_payments(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    assessment_id: Uuid,
) -> Result<Vec<AssessmentPaymentRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
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
           AND al.cooperative_id IS NOT DISTINCT FROM $2 \
         ORDER BY al.created_at DESC, al.id",
    )
    .bind(assessment_id)
    .bind(cooperative_id)
    .fetch_all(&mut *conn)
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
/// Cooperative-scoped (C3): a foreign shareholder yields an empty
/// list — never their open debt.
pub async fn shareholder_open_assessments(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    shareholder_id: Uuid,
) -> Result<Vec<OpenAssessmentRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    sqlx::query_as::<_, OpenAssessmentRow>(
        "SELECT a.id, a.period_id, p.period_number, p.name AS period_name, \
            p.due_date, a.amount, a.currency, a.generated_at, \
            COALESCE(al.paid, 0) AS paid_amount, \
            a.amount - COALESCE(al.paid, 0) AS remaining_amount \
         FROM assessments a \
         JOIN periods p ON p.id = a.period_id \
         LEFT JOIN ( \
             SELECT assessment_id, sum(amount) AS paid FROM ( \
                 SELECT assessment_id, amount FROM payment_allocations \
                 WHERE status = 'active' \
                 UNION ALL \
                 SELECT assessment_id, amount FROM credit_applications \
                 WHERE status = 'active' \
             ) settled GROUP BY assessment_id \
         ) al ON al.assessment_id = a.id \
         WHERE a.shareholder_id = $1 AND a.status = 'active' \
           AND a.cooperative_id IS NOT DISTINCT FROM $2 \
         ORDER BY p.period_number DESC, a.id",
    )
    .bind(shareholder_id)
    .bind(cooperative_id)
    .fetch_all(&mut *conn)
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
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    shareholder_id: Uuid,
) -> Result<FinancialSummaryRow, sqlx::Error> {
    let mut conn = db.conn().await?;
    sqlx::query_as::<_, FinancialSummaryRow>(
        "SELECT count(*) AS assessment_count, \
            count(*) FILTER (WHERE a.amount - COALESCE(al.paid, 0) > 0) AS open_assessment_count, \
            COALESCE(sum(a.amount), 0) AS total_assessed, \
            COALESCE(sum(COALESCE(al.paid, 0)), 0) AS total_paid, \
            COALESCE(sum(a.amount - COALESCE(al.paid, 0)), 0) AS total_remaining \
         FROM assessments a \
         LEFT JOIN ( \
             SELECT assessment_id, sum(amount) AS paid FROM ( \
                 SELECT assessment_id, amount FROM payment_allocations \
                 WHERE status = 'active' \
                 UNION ALL \
                 SELECT assessment_id, amount FROM credit_applications \
                 WHERE status = 'active' \
             ) settled GROUP BY assessment_id \
         ) al ON al.assessment_id = a.id \
         WHERE a.shareholder_id = $1 AND a.status = 'active' \
           AND a.cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(shareholder_id)
    .bind(cooperative_id)
    .fetch_one(&mut *conn)
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
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    period_id: Uuid,
) -> Result<PeriodCollectionSummaryRow, sqlx::Error> {
    let mut conn = db.conn().await?;
    sqlx::query_as::<_, PeriodCollectionSummaryRow>(
        "SELECT count(*) AS assessment_count, \
            COALESCE(sum(a.amount), 0) AS total_assessed, \
            COALESCE(sum(COALESCE(al.paid, 0)), 0) AS total_collected, \
            COALESCE(sum(a.amount - COALESCE(al.paid, 0)), 0) AS total_remaining, \
            (SELECT count(DISTINCT pa.payment_id) \
             FROM payment_allocations pa \
             JOIN assessments a2 ON a2.id = pa.assessment_id \
             WHERE a2.period_id = $1 AND pa.status = 'active' \
               AND a2.cooperative_id IS NOT DISTINCT FROM $2) AS contributing_payment_count \
         FROM assessments a \
         LEFT JOIN ( \
             SELECT assessment_id, sum(amount) AS paid FROM ( \
                 SELECT assessment_id, amount FROM payment_allocations \
                 WHERE status = 'active' \
                 UNION ALL \
                 SELECT assessment_id, amount FROM credit_applications \
                 WHERE status = 'active' \
             ) settled GROUP BY assessment_id \
         ) al ON al.assessment_id = a.id \
         WHERE a.period_id = $1 AND a.status = 'active' \
           AND a.cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(period_id)
    .bind(cooperative_id)
    .fetch_one(&mut *conn)
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
    /// STEP-009: this member's OWN derived available credit —
    /// attribution only; the Family NEVER owns a pooled balance.
    pub credit_available: Decimal,
    /// FUNC-FIX-002: member-level default collection account —
    /// informational only; the family payment NEVER infers a
    /// receiving account from it (explicit operator selection is
    /// mandatory for multi-shareholder payments).
    pub default_account_id: Option<Uuid>,
    pub default_account_name: Option<String>,
    pub default_account_status: Option<String>,
}

/// Cooperative-scoped (C3): a foreign family yields an empty member
/// context — membership rows carry the scope.
pub async fn family_collection_context(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    family_id: Uuid,
) -> Result<Vec<FamilyMemberCollectionRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    sqlx::query_as::<_, FamilyMemberCollectionRow>(
        "SELECT s.id AS shareholder_id, sp.first_name, sp.last_name, \
            gp.first_name AS guardian_first_name, gp.last_name AS guardian_last_name, \
            f.sequence_number AS family_sequence, s.status AS shareholder_status, \
            COALESCE(o.cnt, 0)::bigint AS open_assessment_count, \
            COALESCE(o.remaining, 0) AS remaining_amount, \
            COALESCE(cv.available, 0) AS credit_available, \
            s.default_collection_account_id AS default_account_id, \
            da.name AS default_account_name, \
            da.status AS default_account_status \
         FROM shareholder_family_memberships m \
         JOIN families f ON f.id = m.family_id \
         JOIN shareholders s ON s.id = m.shareholder_id AND s.status <> 'voided' \
         JOIN persons sp ON sp.id = s.person_id \
         LEFT JOIN persons gp ON gp.id = s.guardian_person_id \
         LEFT JOIN financial_accounts da ON da.id = s.default_collection_account_id \
         LEFT JOIN ( \
             SELECT a.shareholder_id, \
                 count(*) FILTER (WHERE a.amount - COALESCE(al.paid, 0) > 0) AS cnt, \
                 sum(a.amount - COALESCE(al.paid, 0)) AS remaining \
             FROM assessments a \
             LEFT JOIN ( \
                 SELECT assessment_id, sum(amount) AS paid FROM ( \
                     SELECT assessment_id, amount FROM payment_allocations \
                     WHERE status = 'active' \
                     UNION ALL \
                     SELECT assessment_id, amount FROM credit_applications \
                     WHERE status = 'active' \
                 ) settled GROUP BY assessment_id \
             ) al ON al.assessment_id = a.id \
             WHERE a.status = 'active' \
             GROUP BY a.shareholder_id \
         ) o ON o.shareholder_id = s.id \
         LEFT JOIN ( \
             SELECT c.shareholder_id, sum(c.amount - COALESCE(ap.applied, 0)) AS available \
             FROM shareholder_credits c \
             LEFT JOIN ( \
                 SELECT credit_id, sum(amount) AS applied FROM credit_applications \
                 WHERE status = 'active' GROUP BY credit_id \
             ) ap ON ap.credit_id = c.id \
             WHERE c.status = 'active' \
             GROUP BY c.shareholder_id \
         ) cv ON cv.shareholder_id = s.id \
         WHERE m.family_id = $1 AND m.ended_at IS NULL \
           AND m.cooperative_id IS NOT DISTINCT FROM $2 \
         ORDER BY sp.search_name, s.id",
    )
    .bind(family_id)
    .bind(cooperative_id)
    .fetch_all(&mut *conn)
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
    /// WHERE the value is kept (STEP-008). Distinct from `method`
    /// (HOW it arrived). Required for every new Payment.
    pub destination_account_id: Uuid,
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
    /// STEP-008: the destination Financial Account does not exist or
    /// is not `active` — indistinguishable 404 (IDOR rule).
    AccountNotFoundOrInactive,
    /// STEP-008: reversing the Payment's inflow would take the
    /// account's derived balance below zero (funds already moved
    /// out by a Transfer) — 409, requires operator resolution.
    AccountEffectBlocked,
    /// STEP-009: the Payment funds Shareholder Credits that are
    /// ALREADY consumed by active Credit Applications — reversing
    /// would leave debt-settlement effects backed by nothing. The
    /// applications must be corrected first — 409.
    CreditEffectBlocked,
    Database(sqlx::Error),
}

/// Translate the Financial Accounts command error vocabulary into the
/// Payment vocabulary — account lookups stay indistinguishable (IDOR)
/// and insufficient post-reversal balance surfaces as a domain 409.
fn account_err(error: account_repo::AccountCommandError) -> PaymentCommandError {
    use account_repo::AccountCommandError::*;
    match error {
        NotFound | InactiveAccount => PaymentCommandError::AccountNotFoundOrInactive,
        InsufficientFunds => PaymentCommandError::AccountEffectBlocked,
        Database(error) => PaymentCommandError::Database(error),
        _ => PaymentCommandError::AccountEffectBlocked,
    }
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
/// `cooperative_id` scopes payer resolution (C3): a foreign Person is
/// `PayerNotFound` — indistinguishable — and a newly created payer
/// Person belongs to the verified cooperative.
async fn resolve_payer(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    cooperative_id: Option<Uuid>,
    command: &CreatePayment,
) -> Result<(Uuid, bool), PaymentCommandError> {
    if let Some(id) = command.payer_person_id {
        let exists: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM persons WHERE id = $1 \
             AND cooperative_id IS NOT DISTINCT FROM $2",
        )
        .bind(id)
        .bind(cooperative_id)
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
        "INSERT INTO persons (first_name, last_name, search_name, cooperative_id) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(first)
    .bind(last)
    .bind(&search)
    .bind(cooperative_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(PaymentCommandError::Database)?;
    Ok((id, true))
}

/// Lock the target Assessments `FOR UPDATE` in ascending id order —
/// the deterministic order shared by every allocation command. Returns
/// (id, amount, status, cooperative_id) keyed by id; the caller
/// compares each row's cooperative against the verified scope (C3),
/// so a foreign target is indistinguishable from an inactive one.
async fn lock_assessments(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, (Decimal, String, Option<Uuid>)>, PaymentCommandError> {
    let rows: Vec<(Uuid, Decimal, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, amount, status, cooperative_id \
         FROM public.app_lock_assessments($1)",
    )
    .bind(ids)
    .fetch_all(tx.as_mut())
    .await
    .map_err(PaymentCommandError::Database)?;
    Ok(rows
        .into_iter()
        .map(|(id, a, s, c)| (id, (a, s, c)))
        .collect())
}

/// SUM of ACTIVE settlement effects per assessment — Payment
/// allocations PLUS Credit applications (STEP-009: both satisfy debt;
/// a Credit Application is not a Payment but settles identically).
/// Recomputed under the row locks, never trusted from a preview.
async fn paid_map(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, Decimal>, PaymentCommandError> {
    credit_repo::settled_map(tx, ids)
        .await
        .map_err(PaymentCommandError::Database)
}

/// Validate a set of NEW allocations against freshly locked
/// assessments: targets exist and are `active`, each amount fits the
/// remaining balance, and the total fits the Payment's available
/// (unallocated) amount.
fn validate_allocations(
    allocations: &[NewAllocation],
    locked: &HashMap<Uuid, (Decimal, String, Option<Uuid>)>,
    paid: &HashMap<Uuid, Decimal>,
    available: Decimal,
    cooperative_id: Option<Uuid>,
) -> Result<(), PaymentCommandError> {
    let mut total = Decimal::ZERO;
    for allocation in allocations {
        let Some((assessment_amount, status, assessment_coop)) =
            locked.get(&allocation.assessment_id)
        else {
            return Err(PaymentCommandError::InactiveAssessment);
        };
        // C3: a target outside the verified scope is reported as
        // InactiveAssessment — the existing indistinguishable error,
        // never a disclosure.
        if *assessment_coop != cooperative_id || status != "active" {
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
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    command: CreatePayment,
) -> Result<PaymentOutcome, PaymentCommandError> {
    let mut tx = db.begin().await.map_err(PaymentCommandError::Database)?;

    // 1. Idempotent replay short-circuit — before ANY side effect.
    //    Scoped (C3): a foreign cooperative's key never replays ITS
    //    payment. The key stays globally unique until C5, so a
    //    cross-coop collision still lands in the ON CONFLICT re-read
    //    as an indistinguishable NotFound.
    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, payment_number, idempotency_fingerprint FROM payments \
         WHERE idempotency_key = $1 \
           AND cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(&command.idempotency_key)
    .bind(cooperative_id)
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
        let credited: Decimal = sqlx::query_scalar(
            "SELECT COALESCE(sum(amount), 0) FROM shareholder_credits \
             WHERE source_payment_id = $1 AND status = 'active'",
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
            unallocated_amount: command.amount - allocated - credited,
            allocation_count: 0,
        });
    }

    // 2. Payer resolution (existing Person or new — never a debtor).
    let (payer_person_id, created_person) =
        resolve_payer(&mut tx, cooperative_id, &command).await?;

    // 2b. Destination Financial Account (STEP-008): lock FIRST in the
    // domain order accounts -> payments -> assessments shared with the
    // transfer command. Must exist, be `active`, hold TRY — and live in
    // the verified cooperative (C3; foreign account is the same
    // indistinguishable error).
    let accounts = account_repo::lock_accounts(&mut tx, &[command.destination_account_id])
        .await
        .map_err(account_err)?;
    let Some((status, currency, account_coop)) = accounts.get(&command.destination_account_id)
    else {
        return Err(PaymentCommandError::AccountNotFoundOrInactive);
    };
    if *account_coop != cooperative_id || status != "active" || currency != "TRY" {
        return Err(PaymentCommandError::AccountNotFoundOrInactive);
    }

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
    validate_allocations(
        &command.allocations,
        &locked,
        &paid,
        command.amount,
        cooperative_id,
    )?;

    // 4. Insert the Payment (posted at birth — one receipt = one row)
    //    carrying the verified cooperative scope (C3).
    // payment_number is intentionally NOT bound: on the restricted
    // lane `payments_number_pin` (0058) assigns it via
    // allocate_document_number and rejects any caller-supplied value,
    // so a sealed actor cannot choose document numbers.
    let inserted: Option<(Uuid, i64)> = sqlx::query_as(
        "INSERT INTO payments \
             (payer_person_id, destination_account_id, amount, currency, \
              method, received_at, note, status, idempotency_key, \
              idempotency_fingerprint, created_by, cooperative_id) \
         VALUES ($1, $2, $3, 'TRY', $4, $5, $6, 'posted', $7, $8, $9, $10) \
         ON CONFLICT (cooperative_id, idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING \
         RETURNING id, payment_number",
    )
    .bind(payer_person_id)
    .bind(command.destination_account_id)
    .bind(command.amount)
    .bind(&command.method)
    .bind(command.received_at)
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
    .map_err(PaymentCommandError::Database)?;

    // A concurrent first-writer committed between our early check and
    // the insert: re-read and apply the same replay/conflict logic.
    let (payment_id, payment_number) = match inserted {
        Some(row) => row,
        None => {
            let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
                "SELECT id, payment_number, idempotency_fingerprint FROM payments \
                 WHERE idempotency_key = $1 \
                   AND cooperative_id IS NOT DISTINCT FROM $2",
            )
            .bind(&command.idempotency_key)
            .bind(cooperative_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(PaymentCommandError::Database)?;
            // Scoped re-read (C3): a conflicting row owned by another
            // cooperative is indistinguishable from absence.
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
            .fetch_one(&mut *tx)
            .await
            .map_err(PaymentCommandError::Database)?;
            let credited: Decimal = sqlx::query_scalar(
                "SELECT COALESCE(sum(amount), 0) FROM shareholder_credits \
                 WHERE source_payment_id = $1 AND status = 'active'",
            )
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(PaymentCommandError::Database)?;
            let allocation_count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM payment_allocations \
                 WHERE payment_id = $1 AND status = 'active'",
            )
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(PaymentCommandError::Database)?;
            tx.rollback().await.map_err(PaymentCommandError::Database)?;
            return Ok(PaymentOutcome {
                payment_id: id,
                payment_number: number,
                replayed: true,
                payer_person_id,
                created_person: false,
                allocated_amount: allocated,
                unallocated_amount: command.amount - allocated - credited,
                allocation_count,
            });
        }
    };

    // 5. Allocation lines — same transaction, all-or-nothing, each
    //    stamped with the verified scope (C3).
    for allocation in &command.allocations {
        sqlx::query(
            "INSERT INTO payment_allocations \
                 (payment_id, assessment_id, amount, created_by, cooperative_id) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(payment_id)
        .bind(allocation.assessment_id)
        .bind(allocation.amount)
        .bind(actor)
        .bind(cooperative_id)
        .execute(tx.as_mut())
        .await
        .map_err(PaymentCommandError::Database)?;
    }

    // 6. The account effect (STEP-008): exactly ONE inflow movement on
    // the destination account, sourced from this Payment. Same
    // transaction — a posted Payment can never exist without its
    // movement and vice versa.
    account_repo::insert_movement(
        &mut tx,
        account_repo::NewMovement {
            account_id: command.destination_account_id,
            direction: MovementDirection::Inflow,
            amount: command.amount,
            source: MovementSource::Payment,
            source_id: payment_id,
            occurred_at: command.received_at,
            actor,
            cooperative_id,
        },
    )
    .await
    .map_err(account_err)?;

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
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    payment_id: Uuid,
    allocations: Vec<NewAllocation>,
) -> Result<PaymentOutcome, PaymentCommandError> {
    let mut tx = db.begin().await.map_err(PaymentCommandError::Database)?;

    // Payment lock FIRST — the shared lock prefix of every
    // payment-scoped mutation. Scoped (C3): a foreign payment id is
    // the indistinguishable NotFound.
    let payment: Option<(String, Decimal)> =
        sqlx::query_as("SELECT status, amount FROM public.app_lock_payment($1, $2)")
            .bind(payment_id)
            .bind(cooperative_id)
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
    // STEP-009: remainder already attributed as Shareholder Credit is
    // NOT allocatable — the reconciliation invariant is
    // `payment = allocations + credits + unassigned remainder`.
    let credited: Decimal = sqlx::query_scalar(
        "SELECT COALESCE(sum(amount), 0) FROM shareholder_credits \
         WHERE source_payment_id = $1 AND status = 'active'",
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
    validate_allocations(
        &allocations,
        &locked,
        &paid,
        amount - allocated - credited,
        cooperative_id,
    )?;

    for allocation in &allocations {
        sqlx::query(
            "INSERT INTO payment_allocations \
                 (payment_id, assessment_id, amount, created_by, cooperative_id) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(payment_id)
        .bind(allocation.assessment_id)
        .bind(allocation.amount)
        .bind(actor)
        .bind(cooperative_id)
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
        unallocated_amount: amount - allocated - credited - added,
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
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    payment_id: Uuid,
    reason: String,
    now: OffsetDateTime,
) -> Result<ReverseOutcome, PaymentCommandError> {
    let mut tx = db.begin().await.map_err(PaymentCommandError::Database)?;

    // R3C: the complete reversal package runs inside ONE
    // permission-checked definer — payment lock + status, account
    // balance/reservation checks, compensating movement reversal,
    // shareholder-credit reconcile and allocation flips are atomic in
    // the database. The runtime role cannot invoke a partial reversal.
    let (replayed, reversed_count, payment_number): (bool, i64, i64) = sqlx::query_as(
        "SELECT replayed, reversed_allocations, payment_number \
         FROM public.app_payment_reverse_tx($1, $2, $3, $4, $5)",
    )
    .bind(payment_id)
    .bind(now)
    .bind(actor)
    .bind(&reason)
    .bind(cooperative_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(|error| match crate::db::package_error_token(&error) {
        Some("not_found") => PaymentCommandError::NotFound,
        Some("account_not_found_or_inactive") => PaymentCommandError::AccountNotFoundOrInactive,
        Some("account_effect_blocked") => PaymentCommandError::AccountEffectBlocked,
        Some("credit_effect_blocked") => PaymentCommandError::CreditEffectBlocked,
        _ => PaymentCommandError::Database(error),
    })?;

    tx.commit().await.map_err(PaymentCommandError::Database)?;
    Ok(ReverseOutcome {
        replayed,
        reversed_allocation_count: reversed_count,
        payment_number,
    })
}

/// Single-allocation reversal (fine-grained correction): the rest of
/// the Payment keeps satisfying its other obligations. Same payment ->
/// allocation lock prefix as every other command.
pub async fn reverse_allocation(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    payment_id: Uuid,
    allocation_id: Uuid,
    reason: String,
    now: OffsetDateTime,
) -> Result<ReverseOutcome, PaymentCommandError> {
    let mut tx = db.begin().await.map_err(PaymentCommandError::Database)?;

    // Scoped lock (C3): a foreign payment id is the indistinguishable
    // NotFound.
    let payment: Option<(String, i64)> =
        sqlx::query_as("SELECT status, payment_number FROM public.app_lock_payment($1, $2)")
            .bind(payment_id)
            .bind(cooperative_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(PaymentCommandError::Database)?;
    let Some((_status, payment_number)) = payment else {
        return Err(PaymentCommandError::NotFound);
    };

    let allocation: Option<(String,)> =
        sqlx::query_as("SELECT status FROM public.app_lock_allocation($1, $2, $3)")
            .bind(allocation_id)
            .bind(payment_id)
            .bind(cooperative_id)
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

    sqlx::query("SELECT public.app_payment_allocation_reverse($1, $2, $3, $4, $5)")
        .bind(allocation_id)
        .bind(now)
        .bind(actor)
        .bind(&reason)
        .bind(cooperative_id)
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
