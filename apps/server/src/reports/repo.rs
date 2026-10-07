//! Reporting read models (STEP-015). Every query here is a PROJECTION
//! over authoritative tables — nothing mutates, nothing is stored as a
//! second truth.
//!
//! Canonical derivations reused verbatim:
//!   * account balance      = SUM(active account_movements, signed)
//!     (financial_accounts::repo::BALANCE_SQL)
//!   * assessment settled   = active payment_allocations
//!     + active credit_applications  (payments::repo settled union)
//!   * credit available     = active credits − active applications
//!     (credits::repo::shareholder_credit_summary)
//!   * restricted available = posted donations − posted disbursements
//!     per (fund, account)  (social_aid::repo::locked_restricted_available)
//!   * investment funded    = posted investment_fundings
//!   * latest valuation     = latest 'recorded' valuation (informational)
//!   * entitlement remaining= amount − posted settlements
//!     (share_returns::repo::ENTITLEMENT_SELECT); amount NULL stays NULL.
//!
//! The overview snapshot runs inside ONE `REPEATABLE READ` transaction
//! so every card on the screen describes the same instant (docs/14:
//! numbers must answer "where did this come from" consistently).

use rust_decimal::Decimal;
use sqlx::PgPool;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

// ------------------------------------------------------------------
// Overview — one consistent read snapshot across every domain.
// ------------------------------------------------------------------

#[derive(Debug)]
pub struct OverviewRow {
    pub financial_accounts_balance: Decimal,
    pub financial_accounts_count: i64,
    pub outstanding_assessment_debt: Decimal,
    pub assessments_total: Decimal,
    pub available_shareholder_credit: Decimal,
    pub operational_income_total: Decimal,
    pub operational_expense_total: Decimal,
    pub posted_payments_total: Decimal,
    pub posted_payments_count: i64,
    pub outstanding_return_entitlement_determined: Decimal,
    pub undetermined_entitlement_count: i64,
    pub return_settled_total: Decimal,
    pub investment_total_funded: Decimal,
    pub investment_latest_valuation_total: Option<Decimal>,
    pub investment_income_total: Decimal,
    pub investment_active_count: i64,
    pub social_aid_restricted_available: Decimal,
    pub social_aid_donations_total: Decimal,
    pub social_aid_disbursements_total: Decimal,
    pub active_shareholder_count: i64,
    pub active_share_count: i64,
    pub active_body_count: i64,
    pub active_membership_count: i64,
    pub decisions_draft: i64,
    pub decisions_open: i64,
    pub decisions_approved: i64,
    pub decisions_rejected: i64,
    pub decisions_cancelled: i64,
    pub votes_total: i64,
}

/// The settled-value UNION used by every receivable projection —
/// identical to payments::repo's canonical formula. A settled amount is
/// either an active payment allocation or an active credit application.
const SETTLED_UNION: &str = "(
    SELECT assessment_id, amount FROM payment_allocations WHERE status = 'active'
    UNION ALL
    SELECT assessment_id, amount FROM credit_applications WHERE status = 'active'
)";

/// One repeatable-read snapshot so all overview metrics describe the
/// same committed instant.
pub async fn overview(pool: &PgPool) -> Result<OverviewRow, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;

    let (financial_accounts_balance, financial_accounts_count): (Decimal, i64) = sqlx::query_as(
        "SELECT COALESCE(sum(bal.balance), 0), count(*) FROM ( \
                SELECT COALESCE(( \
                    SELECT sum(CASE m.direction WHEN 'inflow' THEN m.amount ELSE -m.amount END) \
                    FROM account_movements m \
                    WHERE m.account_id = a.id AND m.status = 'active'), 0) AS balance \
                FROM financial_accounts a) bal",
    )
    .fetch_one(&mut *tx)
    .await?;

    let (assessments_total, outstanding_assessment_debt): (Decimal, Decimal) =
        sqlx::query_as(&format!(
            "SELECT COALESCE(sum(a.amount), 0), \
                COALESCE(sum(a.amount - COALESCE(s.paid, 0)), 0) \
             FROM assessments a \
             LEFT JOIN (SELECT assessment_id, sum(amount) AS paid \
                 FROM {SETTLED_UNION} settled GROUP BY assessment_id) s \
                ON s.assessment_id = a.id \
             WHERE a.status = 'active'"
        ))
        .fetch_one(&mut *tx)
        .await?;

    let available_shareholder_credit: Decimal = sqlx::query_scalar(
        "SELECT COALESCE(sum(c.amount - COALESCE(ap.applied, 0)), 0) \
         FROM shareholder_credits c \
         LEFT JOIN (SELECT credit_id, sum(amount) AS applied \
             FROM credit_applications WHERE status = 'active' GROUP BY credit_id) ap \
            ON ap.credit_id = c.id \
         WHERE c.status = 'active'",
    )
    .fetch_one(&mut *tx)
    .await
    .map(|v: Option<Decimal>| v.unwrap_or_default())?;

    let (operational_income_total, operational_expense_total): (Decimal, Decimal) = sqlx::query_as(
        "SELECT \
                COALESCE((SELECT sum(amount) FROM income_entries WHERE status = 'posted'), 0), \
                COALESCE((SELECT sum(amount) FROM expense_entries WHERE status = 'posted'), 0)",
    )
    .fetch_one(&mut *tx)
    .await?;

    let (posted_payments_total, posted_payments_count): (Decimal, i64) = sqlx::query_as(
        "SELECT COALESCE(sum(amount), 0), count(*) \
         FROM payments WHERE status = 'posted'",
    )
    .fetch_one(&mut *tx)
    .await?;

    // Determined outstanding entitlement = amount − posted settlements,
    // NULL amounts never coerced to zero — counted separately.
    let (
        outstanding_return_entitlement_determined,
        undetermined_entitlement_count,
        return_settled_total,
    ): (Decimal, i64, Decimal) = sqlx::query_as(
        "SELECT \
            COALESCE(sum(e.amount - COALESCE(s.settled, 0)) \
                FILTER (WHERE e.amount IS NOT NULL \
                    AND e.status IN ('open','partially_settled')), 0), \
            count(*) FILTER (WHERE e.amount IS NULL \
                AND e.status IN ('open','partially_settled')), \
            COALESCE((SELECT sum(amount) FROM share_return_settlements \
                WHERE status = 'posted'), 0) \
         FROM share_return_entitlements e \
         LEFT JOIN (SELECT entitlement_id, sum(amount) AS settled \
             FROM share_return_settlements WHERE status = 'posted' \
             GROUP BY entitlement_id) s ON s.entitlement_id = e.id",
    )
    .fetch_one(&mut *tx)
    .await?;

    let (
        investment_total_funded,
        investment_latest_valuation_total,
        investment_income_total,
        investment_active_count,
    ): (Decimal, Option<Decimal>, Decimal, i64) = sqlx::query_as(
        "SELECT \
            COALESCE((SELECT sum(amount) FROM investment_fundings \
                WHERE status = 'posted'), 0), \
            (SELECT sum(v.amount) FROM ( \
                SELECT DISTINCT ON (v2.investment_id) v2.amount \
                FROM investment_valuations v2 \
                JOIN investments i2 ON i2.id = v2.investment_id \
                    AND i2.status = 'active' \
                WHERE v2.status = 'recorded' \
                ORDER BY v2.investment_id, v2.valuation_date DESC, \
                    v2.valuation_number DESC) v), \
            COALESCE((SELECT sum(amount) FROM investment_incomes \
                WHERE status = 'posted'), 0), \
            count(*) FILTER (WHERE i.status = 'active') \
         FROM investments i WHERE i.status <> 'cancelled'",
    )
    .fetch_one(&mut *tx)
    .await?;

    // Restricted availability is a CLASSIFICATION of physical cash the
    // accounts already hold — never additional money (docs/11).
    let (
        social_aid_restricted_available,
        social_aid_donations_total,
        social_aid_disbursements_total,
    ): (Decimal, Decimal, Decimal) = sqlx::query_as(
        "SELECT \
            COALESCE((SELECT sum(amount) FROM social_aid_donations \
                WHERE status = 'posted'), 0) \
          - COALESCE((SELECT sum(amount) FROM social_aid_disbursements \
                WHERE status = 'posted'), 0), \
            COALESCE((SELECT sum(amount) FROM social_aid_donations \
                WHERE status = 'posted'), 0), \
            COALESCE((SELECT sum(amount) FROM social_aid_disbursements \
                WHERE status = 'posted'), 0)",
    )
    .fetch_one(&mut *tx)
    .await?;

    let (active_shareholder_count, active_share_count): (i64, i64) = sqlx::query_as(
        "SELECT \
            (SELECT count(*) FROM shareholders WHERE status = 'active'), \
            (SELECT count(*) FROM shares s WHERE s.status = 'active' AND EXISTS ( \
                SELECT 1 FROM share_ownerships o \
                WHERE o.share_id = s.id AND o.ended_at IS NULL))",
    )
    .fetch_one(&mut *tx)
    .await?;

    let (
        active_body_count,
        active_membership_count,
        decisions_draft,
        decisions_open,
        decisions_approved,
        decisions_rejected,
        decisions_cancelled,
        votes_total,
    ): (i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
            (SELECT count(*) FROM governance_bodies WHERE status = 'active'), \
            (SELECT count(*) FROM governance_memberships WHERE ended_at IS NULL), \
            count(*) FILTER (WHERE d.status = 'draft'), \
            count(*) FILTER (WHERE d.status = 'open'), \
            count(*) FILTER (WHERE d.status = 'approved'), \
            count(*) FILTER (WHERE d.status = 'rejected'), \
            count(*) FILTER (WHERE d.status = 'cancelled'), \
            (SELECT count(*) FROM governance_votes) \
         FROM governance_decisions d",
    )
    .fetch_one(&mut *tx)
    .await?;

    tx.rollback().await?; // read-only: nothing ever commits
    Ok(OverviewRow {
        financial_accounts_balance,
        financial_accounts_count,
        outstanding_assessment_debt,
        assessments_total,
        available_shareholder_credit,
        operational_income_total,
        operational_expense_total,
        posted_payments_total,
        posted_payments_count,
        outstanding_return_entitlement_determined,
        undetermined_entitlement_count,
        return_settled_total,
        investment_total_funded,
        investment_latest_valuation_total,
        investment_income_total,
        investment_active_count,
        social_aid_restricted_available,
        social_aid_donations_total,
        social_aid_disbursements_total,
        active_shareholder_count,
        active_share_count,
        active_body_count,
        active_membership_count,
        decisions_draft,
        decisions_open,
        decisions_approved,
        decisions_rejected,
        decisions_cancelled,
        votes_total,
    })
}

// ------------------------------------------------------------------
// Financial accounts — derived balance per account (all statuses
// included; inactive accounts keep their history and their money).
// ------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
pub struct AccountReportRow {
    pub id: Uuid,
    pub name: String,
    pub account_type: String,
    pub status: String,
    pub currency: String,
    pub balance: Decimal,
    /// Restricted social-aid money physically sitting in this account —
    /// a CLASSIFICATION of part of `balance`, never additive.
    pub restricted_available: Decimal,
    pub total_count: i64,
}

pub async fn accounts_report(
    pool: &PgPool,
    status: Option<&str>,
    page: i64,
    page_size: i64,
) -> Result<Vec<AccountReportRow>, sqlx::Error> {
    sqlx::query_as::<_, AccountReportRow>(
        "SELECT a.id, a.name, a.account_type, a.status, a.currency, \
            COALESCE(( \
                SELECT sum(CASE m.direction WHEN 'inflow' THEN m.amount ELSE -m.amount END) \
                FROM account_movements m \
                WHERE m.account_id = a.id AND m.status = 'active'), 0) AS balance, \
            COALESCE(( \
                SELECT sum(COALESCE(dn.donated,0) - COALESCE(db.spent,0)) \
                FROM social_aid_funds f \
                LEFT JOIN (SELECT fund_id, financial_account_id, sum(amount) AS donated \
                    FROM social_aid_donations WHERE status = 'posted' \
                    GROUP BY fund_id, financial_account_id) dn \
                    ON dn.fund_id = f.id AND dn.financial_account_id = a.id \
                LEFT JOIN (SELECT fund_id, financial_account_id, sum(amount) AS spent \
                    FROM social_aid_disbursements WHERE status = 'posted' \
                    GROUP BY fund_id, financial_account_id) db \
                    ON db.fund_id = f.id AND db.financial_account_id = a.id \
                WHERE f.status <> 'cancelled'), 0) AS restricted_available, \
            count(*) OVER() AS total_count \
         FROM financial_accounts a \
         WHERE ($1::text IS NULL OR a.status = $1) \
         ORDER BY a.name, a.id LIMIT $2 OFFSET $3",
    )
    .bind(status)
    .bind(page_size)
    .bind((page - 1) * page_size)
    .fetch_all(pool)
    .await
}

// ------------------------------------------------------------------
// Movement provenance report — the traceable ledger surface. Every row
// keeps its source_type + source_id + business number so the operator
// can jump to the authoritative record.
// ------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
pub struct MovementReportRow {
    pub id: Uuid,
    pub account_id: Uuid,
    pub account_name: String,
    pub direction: String,
    pub amount: Decimal,
    pub source_type: String,
    pub source_id: Uuid,
    pub source_number: Option<i64>,
    pub occurred_at: OffsetDateTime,
    pub status: String,
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
    pub total_count: i64,
}

#[derive(Debug, Clone)]
pub struct MovementFilter {
    pub account_id: Option<Uuid>,
    pub source_type: Option<String>,
    pub direction: Option<String>,
    pub status: Option<String>,
    /// Business-date bounds on occurred_at (NOT created_at).
    pub date_from: Option<Date>,
    pub date_to: Option<Date>,
}

#[derive(Debug, sqlx::FromRow)]
pub struct MovementSummaryRow {
    /// Inflows that are NOT the receiving leg of an internal transfer.
    pub external_inflow: Decimal,
    /// Outflows that are NOT the sending leg of an internal transfer.
    pub external_outflow: Decimal,
    /// Internal transfer leg volume (location change, not income/expense).
    pub internal_transfer_volume: Decimal,
    pub movement_count: i64,
}

const MOVEMENT_SELECT: &str = "SELECT m.id, m.account_id, a.name AS account_name, \
    m.direction, m.amount, m.source_type, m.source_id, \
    COALESCE(p.payment_number, t.transfer_number, i.income_number, e.expense_number, \
        s.settlement_number, ivf.funding_number, ivn.income_number, \
        ivd.disposal_number, sad.donation_number, say.disbursement_number) \
        AS source_number, \
    m.occurred_at, m.status, m.reversed_at, m.reversal_reason, \
    count(*) OVER() AS total_count \
FROM account_movements m \
JOIN financial_accounts a ON a.id = m.account_id \
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
    ON m.source_type = 'social_aid_disbursement' AND m.source_id = say.id";

const MOVEMENT_WHERE: &str = "WHERE ($1::uuid IS NULL OR m.account_id = $1) \
  AND ($2::text IS NULL OR m.source_type = $2) \
  AND ($3::text IS NULL OR m.direction = $3) \
  AND ($4::text IS NULL OR m.status = $4) \
  AND ($5::date IS NULL OR m.occurred_at >= $5::date) \
  AND ($6::date IS NULL OR m.occurred_at < ($6::date + 1))";

pub async fn movements_report(
    pool: &PgPool,
    filter: &MovementFilter,
    page: i64,
    page_size: i64,
) -> Result<Vec<MovementReportRow>, sqlx::Error> {
    sqlx::query_as::<_, MovementReportRow>(&format!(
        "{MOVEMENT_SELECT} {MOVEMENT_WHERE} \
         ORDER BY m.occurred_at DESC, m.id LIMIT $7 OFFSET $8"
    ))
    .bind(filter.account_id)
    .bind(filter.source_type.as_deref())
    .bind(filter.direction.as_deref())
    .bind(filter.status.as_deref())
    .bind(filter.date_from)
    .bind(filter.date_to)
    .bind(page_size)
    .bind((page - 1) * page_size)
    .fetch_all(pool)
    .await
}

/// Totals for the FULL filtered set — independent of the returned page.
/// `transfer` source legs are classified internal so cooperative-wide
/// economic flow never counts a location change twice (docs/15).
pub async fn movements_summary(
    pool: &PgPool,
    filter: &MovementFilter,
) -> Result<MovementSummaryRow, sqlx::Error> {
    sqlx::query_as::<_, MovementSummaryRow>(&format!(
        "SELECT \
            COALESCE(sum(m.amount) FILTER (WHERE m.direction = 'inflow' \
                AND m.source_type <> 'transfer'), 0) AS external_inflow, \
            COALESCE(sum(m.amount) FILTER (WHERE m.direction = 'outflow' \
                AND m.source_type <> 'transfer'), 0) AS external_outflow, \
            COALESCE(sum(m.amount) FILTER (WHERE m.direction = 'inflow' \
                AND m.source_type = 'transfer'), 0) AS internal_transfer_volume, \
            count(*) AS movement_count \
         FROM account_movements m \
         JOIN financial_accounts a ON a.id = m.account_id \
         {MOVEMENT_WHERE}"
    ))
    .bind(filter.account_id)
    .bind(filter.source_type.as_deref())
    .bind(filter.direction.as_deref())
    .bind(filter.status.as_deref())
    .bind(filter.date_from)
    .bind(filter.date_to)
    .fetch_one(pool)
    .await
}

// ------------------------------------------------------------------
// Assessment receivables — canonical settled formula per row; summary
// totals span the whole filtered set regardless of page.
// ------------------------------------------------------------------

#[derive(Debug)]
pub struct AssessmentFilter {
    pub period_id: Option<Uuid>,
    pub shareholder_id: Option<Uuid>,
    /// 'outstanding' = remaining > 0, 'settled' = remaining = 0.
    pub settlement: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
pub struct AssessmentReportRow {
    pub id: Uuid,
    pub period_id: Uuid,
    pub period_number: i64,
    pub period_name: String,
    pub shareholder_id: Uuid,
    pub shareholder_name: String,
    pub family_sequence: Option<i64>,
    pub amount: Decimal,
    pub paid_amount: Decimal,
    pub payment_allocated: Decimal,
    pub credit_applied: Decimal,
    pub remaining_amount: Decimal,
    pub assessment_effective_date: Date,
    pub generated_at: OffsetDateTime,
    pub total_count: i64,
}

const ASSESSMENT_REPORT_FROM: &str = "FROM assessments a \
JOIN periods p ON p.id = a.period_id \
JOIN shareholders s ON s.id = a.shareholder_id \
JOIN persons sp ON sp.id = s.person_id \
LEFT JOIN shareholder_family_memberships fm \
    ON fm.shareholder_id = s.id AND fm.ended_at IS NULL \
LEFT JOIN families f ON f.id = fm.family_id \
LEFT JOIN (SELECT assessment_id, sum(amount) AS paid FROM payment_allocations \
    WHERE status = 'active' GROUP BY assessment_id) pa \
    ON pa.assessment_id = a.id \
LEFT JOIN (SELECT assessment_id, sum(amount) AS applied FROM credit_applications \
    WHERE status = 'active' GROUP BY assessment_id) ca \
    ON ca.assessment_id = a.id \
WHERE a.status = 'active'";

const ASSESSMENT_REPORT_WHERE: &str = "  AND ($1::uuid IS NULL OR a.period_id = $1) \
  AND ($2::uuid IS NULL OR a.shareholder_id = $2) \
  AND ($3::text IS NULL OR ($3 = 'outstanding' \
        AND a.amount - COALESCE(pa.paid,0) - COALESCE(ca.applied,0) > 0) \
    OR ($3 = 'settled' \
        AND a.amount - COALESCE(pa.paid,0) - COALESCE(ca.applied,0) <= 0))";

pub async fn assessments_report(
    pool: &PgPool,
    filter: &AssessmentFilter,
    page: i64,
    page_size: i64,
) -> Result<Vec<AssessmentReportRow>, sqlx::Error> {
    sqlx::query_as::<_, AssessmentReportRow>(&format!(
        "SELECT a.id, a.period_id, p.period_number, p.name AS period_name, \
            a.shareholder_id, sp.first_name || ' ' || sp.last_name AS shareholder_name, \
            f.sequence_number AS family_sequence, a.amount, \
            COALESCE(pa.paid,0) + COALESCE(ca.applied,0) AS paid_amount, \
            COALESCE(pa.paid,0) AS payment_allocated, \
            COALESCE(ca.applied,0) AS credit_applied, \
            a.amount - COALESCE(pa.paid,0) - COALESCE(ca.applied,0) AS remaining_amount, \
            a.assessment_effective_date, a.generated_at, \
            count(*) OVER() AS total_count \
         {ASSESSMENT_REPORT_FROM} {ASSESSMENT_REPORT_WHERE} \
         ORDER BY p.period_number DESC, a.id LIMIT $4 OFFSET $5"
    ))
    .bind(filter.period_id)
    .bind(filter.shareholder_id)
    .bind(filter.settlement.as_deref())
    .bind(page_size)
    .bind((page - 1) * page_size)
    .fetch_all(pool)
    .await
}

#[derive(Debug, sqlx::FromRow)]
pub struct AssessmentSummaryRow {
    pub assessment_count: i64,
    pub total_assessed: Decimal,
    pub total_payment_allocated: Decimal,
    pub total_credit_applied: Decimal,
    pub total_outstanding: Decimal,
    pub fully_paid_count: i64,
    pub partially_paid_count: i64,
    pub unpaid_count: i64,
}

pub async fn assessments_summary(
    pool: &PgPool,
    filter: &AssessmentFilter,
) -> Result<AssessmentSummaryRow, sqlx::Error> {
    sqlx::query_as::<_, AssessmentSummaryRow>(&format!(
        "SELECT count(*) AS assessment_count, \
            COALESCE(sum(a.amount), 0) AS total_assessed, \
            COALESCE(sum(COALESCE(pa.paid,0)), 0) AS total_payment_allocated, \
            COALESCE(sum(COALESCE(ca.applied,0)), 0) AS total_credit_applied, \
            COALESCE(sum(a.amount - COALESCE(pa.paid,0) - COALESCE(ca.applied,0)), 0) \
                AS total_outstanding, \
            count(*) FILTER (WHERE a.amount > 0 \
                AND a.amount - COALESCE(pa.paid,0) - COALESCE(ca.applied,0) <= 0) \
                AS fully_paid_count, \
            count(*) FILTER (WHERE COALESCE(pa.paid,0) + COALESCE(ca.applied,0) > 0 \
                AND a.amount - COALESCE(pa.paid,0) - COALESCE(ca.applied,0) > 0) \
                AS partially_paid_count, \
            count(*) FILTER (WHERE COALESCE(pa.paid,0) + COALESCE(ca.applied,0) = 0) \
                AS unpaid_count \
         {ASSESSMENT_REPORT_FROM} {ASSESSMENT_REPORT_WHERE}"
    ))
    .bind(filter.period_id)
    .bind(filter.shareholder_id)
    .bind(filter.settlement.as_deref())
    .fetch_one(pool)
    .await
}

// ------------------------------------------------------------------
// Period collection report (docs/14): assessed total, prior excess
// applied (credit), new cash collected (payment allocations), debt
// satisfied and outstanding — distinct metrics, never merged.
// ------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
pub struct PeriodReportRow {
    pub id: Uuid,
    pub period_number: i64,
    pub name: String,
    pub status: String,
    pub due_date: Date,
    pub assessment_count: i64,
    pub total_assessed: Decimal,
    /// New shareholder cash allocated to this period's assessments.
    pub payment_allocated: Decimal,
    /// Prior excess applied (credit applications) — NOT new cash.
    pub credit_applied: Decimal,
    pub total_satisfied: Decimal,
    pub outstanding: Decimal,
    pub fully_paid_count: i64,
    pub partially_paid_count: i64,
    pub unpaid_count: i64,
    pub total_count: i64,
}

pub async fn periods_report(
    pool: &PgPool,
    page: i64,
    page_size: i64,
) -> Result<Vec<PeriodReportRow>, sqlx::Error> {
    sqlx::query_as::<_, PeriodReportRow>(
        "SELECT p.id, p.period_number, p.name, p.status, p.due_date, \
            COALESCE(agg.cnt,0)::bigint AS assessment_count, \
            COALESCE(agg.assessed,0) AS total_assessed, \
            COALESCE(agg.payment_allocated,0) AS payment_allocated, \
            COALESCE(agg.credit_applied,0) AS credit_applied, \
            COALESCE(agg.satisfied,0) AS total_satisfied, \
            COALESCE(agg.assessed,0) - COALESCE(agg.satisfied,0) AS outstanding, \
            COALESCE(agg.fully_paid,0)::bigint AS fully_paid_count, \
            COALESCE(agg.partially_paid,0)::bigint AS partially_paid_count, \
            COALESCE(agg.unpaid,0)::bigint AS unpaid_count, \
            count(*) OVER() AS total_count \
         FROM periods p \
         LEFT JOIN ( \
            SELECT a.period_id, count(*) AS cnt, sum(a.amount) AS assessed, \
                sum(COALESCE(pa.paid,0)) AS payment_allocated, \
                sum(COALESCE(ca.applied,0)) AS credit_applied, \
                sum(COALESCE(pa.paid,0) + COALESCE(ca.applied,0)) AS satisfied, \
                count(*) FILTER (WHERE a.amount > 0 \
                    AND a.amount - COALESCE(pa.paid,0) - COALESCE(ca.applied,0) <= 0) \
                    AS fully_paid, \
                count(*) FILTER (WHERE COALESCE(pa.paid,0) + COALESCE(ca.applied,0) > 0 \
                    AND a.amount - COALESCE(pa.paid,0) - COALESCE(ca.applied,0) > 0) \
                    AS partially_paid, \
                count(*) FILTER (WHERE COALESCE(pa.paid,0) + COALESCE(ca.applied,0) = 0) \
                    AS unpaid \
            FROM assessments a \
            LEFT JOIN (SELECT assessment_id, sum(amount) AS paid \
                FROM payment_allocations WHERE status = 'active' \
                GROUP BY assessment_id) pa ON pa.assessment_id = a.id \
            LEFT JOIN (SELECT assessment_id, sum(amount) AS applied \
                FROM credit_applications WHERE status = 'active' \
                GROUP BY assessment_id) ca ON ca.assessment_id = a.id \
            WHERE a.status = 'active' GROUP BY a.period_id \
         ) agg ON agg.period_id = p.id \
         ORDER BY p.period_number DESC, p.id LIMIT $1 OFFSET $2",
    )
    .bind(page_size)
    .bind((page - 1) * page_size)
    .fetch_all(pool)
    .await
}

// ------------------------------------------------------------------
// Shareholder financial report — debtor-side view. Every column keeps
// its own meaning: assessed, cash-allocated, credit-applied, remaining
// debt, available credit and return entitlements are NEVER netted into
// a single "balance" (docs/17: netting policy is unresolved).
// ------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
pub struct ShareholderReportRow {
    pub shareholder_id: Uuid,
    pub shareholder_name: String,
    pub shareholder_status: String,
    pub family_sequence: Option<i64>,
    pub active_share_count: i64,
    pub total_assessed: Decimal,
    pub payment_allocated: Decimal,
    pub credit_applied: Decimal,
    pub remaining_debt: Decimal,
    pub credit_available: Decimal,
    pub return_entitlement_determined: Decimal,
    pub undetermined_entitlement_count: i64,
    pub total_count: i64,
}

pub async fn shareholders_report(
    pool: &PgPool,
    search: Option<&str>,
    page: i64,
    page_size: i64,
) -> Result<Vec<ShareholderReportRow>, sqlx::Error> {
    let pattern = search
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{}%", crate::parties::model::fold_search(s)));
    sqlx::query_as::<_, ShareholderReportRow>(
        "SELECT s.id AS shareholder_id, \
            sp.first_name || ' ' || sp.last_name AS shareholder_name, \
            s.status AS shareholder_status, f.sequence_number AS family_sequence, \
            COALESCE(sh.cnt,0)::bigint AS active_share_count, \
            COALESCE(ob.assessed,0) AS total_assessed, \
            COALESCE(ob.payment_allocated,0) AS payment_allocated, \
            COALESCE(ob.credit_applied,0) AS credit_applied, \
            COALESCE(ob.assessed,0) - COALESCE(ob.satisfied,0) AS remaining_debt, \
            COALESCE(cv.available,0) AS credit_available, \
            COALESCE(rt.determined,0) AS return_entitlement_determined, \
            COALESCE(rt.undetermined,0)::bigint AS undetermined_entitlement_count, \
            count(*) OVER() AS total_count \
         FROM shareholders s \
         JOIN persons sp ON sp.id = s.person_id \
         LEFT JOIN shareholder_family_memberships fm \
             ON fm.shareholder_id = s.id AND fm.ended_at IS NULL \
         LEFT JOIN families f ON f.id = fm.family_id \
         LEFT JOIN (SELECT o.shareholder_id, count(*) AS cnt \
             FROM share_ownerships o JOIN shares s2 ON s2.id = o.share_id \
             WHERE o.ended_at IS NULL AND s2.status = 'active' \
             GROUP BY o.shareholder_id) sh ON sh.shareholder_id = s.id \
         LEFT JOIN ( \
            SELECT a.shareholder_id, sum(a.amount) AS assessed, \
                sum(COALESCE(pa.paid,0)) AS payment_allocated, \
                sum(COALESCE(ca.applied,0)) AS credit_applied, \
                sum(COALESCE(pa.paid,0) + COALESCE(ca.applied,0)) AS satisfied \
            FROM assessments a \
            LEFT JOIN (SELECT assessment_id, sum(amount) AS paid \
                FROM payment_allocations WHERE status = 'active' \
                GROUP BY assessment_id) pa ON pa.assessment_id = a.id \
            LEFT JOIN (SELECT assessment_id, sum(amount) AS applied \
                FROM credit_applications WHERE status = 'active' \
                GROUP BY assessment_id) ca ON ca.assessment_id = a.id \
            WHERE a.status = 'active' GROUP BY a.shareholder_id \
         ) ob ON ob.shareholder_id = s.id \
         LEFT JOIN (SELECT c.shareholder_id, \
                sum(c.amount - COALESCE(ap.applied,0)) AS available \
             FROM shareholder_credits c \
             LEFT JOIN (SELECT credit_id, sum(amount) AS applied \
                 FROM credit_applications WHERE status = 'active' \
                 GROUP BY credit_id) ap ON ap.credit_id = c.id \
             WHERE c.status = 'active' GROUP BY c.shareholder_id \
         ) cv ON cv.shareholder_id = s.id \
         LEFT JOIN (SELECT e.beneficiary_shareholder_id, \
                sum(e.amount - COALESCE(st.settled,0)) \
                    FILTER (WHERE e.amount IS NOT NULL) AS determined, \
                count(*) FILTER (WHERE e.amount IS NULL) AS undetermined \
             FROM share_return_entitlements e \
             LEFT JOIN (SELECT entitlement_id, sum(amount) AS settled \
                 FROM share_return_settlements WHERE status = 'posted' \
                 GROUP BY entitlement_id) st ON st.entitlement_id = e.id \
             WHERE e.status IN ('open','partially_settled') \
             GROUP BY e.beneficiary_shareholder_id \
         ) rt ON rt.beneficiary_shareholder_id = s.id \
         WHERE s.status <> 'voided' \
           AND ($1::text IS NULL OR \
                lower(sp.first_name || ' ' || sp.last_name) LIKE $1 \
                OR f.sequence_number::text = trim(both '%' from $1)) \
         ORDER BY lower(sp.first_name || ' ' || sp.last_name), s.id \
         LIMIT $2 OFFSET $3",
    )
    .bind(pattern)
    .bind(page_size)
    .bind((page - 1) * page_size)
    .fetch_all(pool)
    .await
}

// ------------------------------------------------------------------
// Family report — a PRESENTATION aggregate of member-level obligations.
// The family never owns debt; every row sums only current members.
// ------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
pub struct FamilyReportRow {
    pub family_id: Uuid,
    pub family_sequence: i64,
    pub member_count: i64,
    pub member_total_assessed: Decimal,
    pub member_remaining_debt: Decimal,
    pub member_credit_available: Decimal,
    pub total_count: i64,
}

pub async fn families_report(
    pool: &PgPool,
    search: Option<&str>,
    page: i64,
    page_size: i64,
) -> Result<Vec<FamilyReportRow>, sqlx::Error> {
    let number: Option<i64> = search
        .map(str::trim)
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|n| *n > 0);
    sqlx::query_as::<_, FamilyReportRow>(
        "SELECT f.id AS family_id, f.sequence_number AS family_sequence, \
            COALESCE(mem.cnt,0)::bigint AS member_count, \
            COALESCE(mem.assessed,0) AS member_total_assessed, \
            COALESCE(mem.remaining,0) AS member_remaining_debt, \
            COALESCE(mem.credit_available,0) AS member_credit_available, \
            count(*) OVER() AS total_count \
         FROM families f \
         LEFT JOIN ( \
            SELECT fm.family_id, count(*) AS cnt, \
                sum(COALESCE(ob.assessed,0)) AS assessed, \
                sum(COALESCE(ob.assessed,0) - COALESCE(ob.satisfied,0)) AS remaining, \
                sum(COALESCE(cv.available,0)) AS credit_available \
            FROM shareholder_family_memberships fm \
            JOIN shareholders s ON s.id = fm.shareholder_id AND s.status <> 'voided' \
            LEFT JOIN ( \
                SELECT a.shareholder_id, sum(a.amount) AS assessed, \
                    sum(COALESCE(pa.paid,0) + COALESCE(ca.applied,0)) AS satisfied \
                FROM assessments a \
                LEFT JOIN (SELECT assessment_id, sum(amount) AS paid \
                    FROM payment_allocations WHERE status = 'active' \
                    GROUP BY assessment_id) pa ON pa.assessment_id = a.id \
                LEFT JOIN (SELECT assessment_id, sum(amount) AS applied \
                    FROM credit_applications WHERE status = 'active' \
                    GROUP BY assessment_id) ca ON ca.assessment_id = a.id \
                WHERE a.status = 'active' GROUP BY a.shareholder_id \
            ) ob ON ob.shareholder_id = s.id \
            LEFT JOIN (SELECT c.shareholder_id, \
                    sum(c.amount - COALESCE(ap.applied,0)) AS available \
                FROM shareholder_credits c \
                LEFT JOIN (SELECT credit_id, sum(amount) AS applied \
                    FROM credit_applications WHERE status = 'active' \
                    GROUP BY credit_id) ap ON ap.credit_id = c.id \
                WHERE c.status = 'active' GROUP BY c.shareholder_id \
            ) cv ON cv.shareholder_id = s.id \
            WHERE fm.ended_at IS NULL GROUP BY fm.family_id \
         ) mem ON mem.family_id = f.id \
         WHERE ($1::bigint IS NULL OR f.sequence_number = $1) \
         ORDER BY f.sequence_number, f.id LIMIT $2 OFFSET $3",
    )
    .bind(number)
    .bind(page_size)
    .bind((page - 1) * page_size)
    .fetch_all(pool)
    .await
}

// ------------------------------------------------------------------
// Payments report — payer ≠ debtor. Posted amount is the cash received;
// allocated/credited columns show how the money was consumed.
// ------------------------------------------------------------------

#[derive(Debug)]
pub struct PaymentFilter {
    pub status: Option<String>,
    pub method: Option<String>,
    /// Business-date bounds on received_at.
    pub date_from: Option<Date>,
    pub date_to: Option<Date>,
}

#[derive(Debug, sqlx::FromRow)]
pub struct PaymentReportRow {
    pub id: Uuid,
    pub payment_number: i64,
    pub payer_name: String,
    pub method: String,
    pub amount: Decimal,
    /// Posted money already applied to assessments.
    pub allocated_amount: Decimal,
    /// Posted money crystallized into shareholder credit.
    pub credited_amount: Decimal,
    /// Posted money still unassigned (neither allocated nor credited).
    pub unassigned_amount: Decimal,
    /// Debtor shareholders this payment's allocations settled —
    /// names the RECEIVER of the value, never assumed == payer.
    pub debtor_shareholder_names: Option<String>,
    pub received_at: OffsetDateTime,
    pub status: String,
    pub reversed_at: Option<OffsetDateTime>,
    pub total_count: i64,
}

const PAYMENT_WHERE: &str = "WHERE ($1::text IS NULL OR p.status = $1) \
  AND ($2::text IS NULL OR p.method = $2) \
  AND ($3::date IS NULL OR p.received_at >= $3::date) \
  AND ($4::date IS NULL OR p.received_at < ($4::date + 1))";

pub async fn payments_report(
    pool: &PgPool,
    filter: &PaymentFilter,
    page: i64,
    page_size: i64,
) -> Result<Vec<PaymentReportRow>, sqlx::Error> {
    sqlx::query_as::<_, PaymentReportRow>(&format!(
        "SELECT p.id, p.payment_number, \
            pp.first_name || ' ' || pp.last_name AS payer_name, \
            p.method, p.amount, \
            COALESCE(al.allocated,0) AS allocated_amount, \
            COALESCE(cr.credited,0) AS credited_amount, \
            p.amount - COALESCE(al.allocated,0) - COALESCE(cr.credited,0) \
                AS unassigned_amount, \
            db.debtors AS debtor_shareholder_names, \
            p.received_at, p.status, p.reversed_at, \
            count(*) OVER() AS total_count \
         FROM payments p \
         JOIN persons pp ON pp.id = p.payer_person_id \
         LEFT JOIN (SELECT payment_id, sum(amount) AS allocated \
             FROM payment_allocations WHERE status = 'active' \
             GROUP BY payment_id) al ON al.payment_id = p.id \
         LEFT JOIN (SELECT source_payment_id, sum(amount) AS credited \
             FROM shareholder_credits WHERE status = 'active' \
             GROUP BY source_payment_id) cr ON cr.source_payment_id = p.id \
         LEFT JOIN (SELECT pa.payment_id, \
                string_agg(DISTINCT sp.first_name || ' ' || sp.last_name, ', ') AS debtors \
             FROM payment_allocations pa \
             JOIN assessments a ON a.id = pa.assessment_id \
             JOIN shareholders s ON s.id = a.shareholder_id \
             JOIN persons sp ON sp.id = s.person_id \
             WHERE pa.status = 'active' GROUP BY pa.payment_id) db \
             ON db.payment_id = p.id \
         {PAYMENT_WHERE} \
         ORDER BY p.payment_number DESC, p.id LIMIT $5 OFFSET $6"
    ))
    .bind(filter.status.as_deref())
    .bind(filter.method.as_deref())
    .bind(filter.date_from)
    .bind(filter.date_to)
    .bind(page_size)
    .bind((page - 1) * page_size)
    .fetch_all(pool)
    .await
}

#[derive(Debug, sqlx::FromRow)]
pub struct PaymentSummaryRow {
    pub payment_count: i64,
    pub posted_amount: Decimal,
    pub allocated_amount: Decimal,
    pub credited_amount: Decimal,
    pub unassigned_amount: Decimal,
}

/// Totals over the FULL filtered payment set (posted rows only —
/// reversed payments carry zero current effect).
pub async fn payments_summary(
    pool: &PgPool,
    filter: &PaymentFilter,
) -> Result<PaymentSummaryRow, sqlx::Error> {
    sqlx::query_as::<_, PaymentSummaryRow>(&format!(
        "SELECT count(*) AS payment_count, \
            COALESCE(sum(p.amount) FILTER (WHERE p.status = 'posted'), 0) AS posted_amount, \
            COALESCE(sum(COALESCE(al.allocated,0)) FILTER (WHERE p.status = 'posted'), 0) \
                AS allocated_amount, \
            COALESCE(sum(COALESCE(cr.credited,0)) FILTER (WHERE p.status = 'posted'), 0) \
                AS credited_amount, \
            COALESCE(sum(p.amount - COALESCE(al.allocated,0) - COALESCE(cr.credited,0)) \
                FILTER (WHERE p.status = 'posted'), 0) AS unassigned_amount \
         FROM payments p \
         LEFT JOIN (SELECT payment_id, sum(amount) AS allocated \
             FROM payment_allocations WHERE status = 'active' \
             GROUP BY payment_id) al ON al.payment_id = p.id \
         LEFT JOIN (SELECT source_payment_id, sum(amount) AS credited \
             FROM shareholder_credits WHERE status = 'active' \
             GROUP BY source_payment_id) cr ON cr.source_payment_id = p.id \
         {PAYMENT_WHERE}"
    ))
    .bind(filter.status.as_deref())
    .bind(filter.method.as_deref())
    .bind(filter.date_from)
    .bind(filter.date_to)
    .fetch_one(pool)
    .await
}

// ------------------------------------------------------------------
// Shareholder credit report — entitlement to apply already-received
// money. An origin adds NO cash (its money arrived with the Payment).
// ------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
pub struct CreditReportRow {
    pub id: Uuid,
    pub credit_number: i64,
    pub shareholder_id: Uuid,
    pub shareholder_name: String,
    pub source_payment_number: i64,
    pub amount: Decimal,
    pub applied_amount: Decimal,
    pub available_amount: Decimal,
    pub status: String,
    pub created_at: OffsetDateTime,
    pub total_count: i64,
}

#[derive(Debug, sqlx::FromRow)]
pub struct CreditSummaryRow {
    pub credit_count: i64,
    pub total_originated: Decimal,
    pub total_applied: Decimal,
    pub total_available: Decimal,
}

pub async fn credits_report(
    pool: &PgPool,
    page: i64,
    page_size: i64,
) -> Result<Vec<CreditReportRow>, sqlx::Error> {
    sqlx::query_as::<_, CreditReportRow>(
        "SELECT c.id, c.credit_number, c.shareholder_id, \
            sp.first_name || ' ' || sp.last_name AS shareholder_name, \
            pay.payment_number AS source_payment_number, \
            c.amount, COALESCE(ap.applied,0) AS applied_amount, \
            c.amount - COALESCE(ap.applied,0) AS available_amount, \
            c.status, c.created_at, count(*) OVER() AS total_count \
         FROM shareholder_credits c \
         JOIN payments pay ON pay.id = c.source_payment_id \
         JOIN shareholders s ON s.id = c.shareholder_id \
         JOIN persons sp ON sp.id = s.person_id \
         LEFT JOIN (SELECT credit_id, sum(amount) AS applied \
             FROM credit_applications WHERE status = 'active' \
             GROUP BY credit_id) ap ON ap.credit_id = c.id \
         ORDER BY c.credit_number DESC, c.id LIMIT $1 OFFSET $2",
    )
    .bind(page_size)
    .bind((page - 1) * page_size)
    .fetch_all(pool)
    .await
}

/// Totals over ACTIVE credits only — reversed credits carry no current
/// availability.
pub async fn credits_summary(pool: &PgPool) -> Result<CreditSummaryRow, sqlx::Error> {
    sqlx::query_as::<_, CreditSummaryRow>(
        "SELECT count(*) FILTER (WHERE c.status = 'active') AS credit_count, \
            COALESCE(sum(c.amount) FILTER (WHERE c.status = 'active'), 0) \
                AS total_originated, \
            COALESCE(sum(COALESCE(ap.applied,0)) FILTER (WHERE c.status = 'active'), 0) \
                AS total_applied, \
            COALESCE(sum(c.amount - COALESCE(ap.applied,0)) \
                FILTER (WHERE c.status = 'active'), 0) AS total_available \
         FROM shareholder_credits c \
         LEFT JOIN (SELECT credit_id, sum(amount) AS applied \
             FROM credit_applications WHERE status = 'active' \
             GROUP BY credit_id) ap ON ap.credit_id = c.id",
    )
    .fetch_one(pool)
    .await
}

// ------------------------------------------------------------------
// Share return entitlements — amount NULL is "undetermined" and stays
// NULL; determined outstanding subtracts posted settlements only.
// ------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
pub struct EntitlementReportRow {
    pub id: Uuid,
    pub entitlement_number: i64,
    pub return_id: Uuid,
    pub return_number: i64,
    pub share_number: i64,
    pub beneficiary_name: String,
    pub entitlement_type: String,
    pub amount: Option<Decimal>,
    pub settled_amount: Decimal,
    /// NULL while the right is undetermined — never coerced to zero.
    pub remaining_amount: Option<Decimal>,
    pub due_date: Option<Date>,
    pub due_state: String,
    pub status: String,
    pub recognized_at: OffsetDateTime,
    pub total_count: i64,
}

#[derive(Debug, sqlx::FromRow)]
pub struct EntitlementSummaryRow {
    pub determined_outstanding: Decimal,
    pub undetermined_count: i64,
    pub settled_total: Decimal,
}

pub async fn entitlements_report(
    pool: &PgPool,
    page: i64,
    page_size: i64,
) -> Result<Vec<EntitlementReportRow>, sqlx::Error> {
    sqlx::query_as::<_, EntitlementReportRow>(
        "SELECT e.id, e.entitlement_number, r.id AS return_id, r.return_number, r.share_number, \
            r.owner_display_name AS beneficiary_name, \
            e.entitlement_type, e.amount, \
            COALESCE(s.settled,0) AS settled_amount, \
            CASE WHEN e.amount IS NULL THEN NULL \
                 ELSE e.amount - COALESCE(s.settled,0) END AS remaining_amount, \
            e.due_date, \
            CASE \
                WHEN e.status = 'settled' THEN 'settled' \
                WHEN e.status = 'cancelled' THEN 'cancelled' \
                WHEN e.amount IS NULL OR e.due_date IS NULL THEN 'undetermined' \
                WHEN e.due_date < (now() AT TIME ZONE 'Europe/Istanbul')::date THEN 'overdue' \
                WHEN e.due_date = (now() AT TIME ZONE 'Europe/Istanbul')::date THEN 'due' \
                ELSE 'not_due' END AS due_state, \
            e.status, e.recognized_at, count(*) OVER() AS total_count \
         FROM share_return_entitlements e \
         JOIN share_returns r ON r.id = e.share_return_id \
         LEFT JOIN (SELECT entitlement_id, sum(amount) AS settled \
             FROM share_return_settlements WHERE status = 'posted' \
             GROUP BY entitlement_id) s ON s.entitlement_id = e.id \
         WHERE e.status <> 'cancelled' \
         ORDER BY e.entitlement_number DESC, e.id LIMIT $1 OFFSET $2",
    )
    .bind(page_size)
    .bind((page - 1) * page_size)
    .fetch_all(pool)
    .await
}

pub async fn entitlements_summary(pool: &PgPool) -> Result<EntitlementSummaryRow, sqlx::Error> {
    sqlx::query_as::<_, EntitlementSummaryRow>(
        "SELECT \
            COALESCE(sum(e.amount - COALESCE(s.settled,0)) \
                FILTER (WHERE e.amount IS NOT NULL), 0) AS determined_outstanding, \
            count(*) FILTER (WHERE e.amount IS NULL AND e.status <> 'cancelled') \
                AS undetermined_count, \
            COALESCE(sum(COALESCE(s.settled,0)), 0) AS settled_total \
         FROM share_return_entitlements e \
         LEFT JOIN (SELECT entitlement_id, sum(amount) AS settled \
             FROM share_return_settlements WHERE status = 'posted' \
             GROUP BY entitlement_id) s ON s.entitlement_id = e.id \
         WHERE e.status <> 'cancelled'",
    )
    .fetch_one(pool)
    .await
}

// ------------------------------------------------------------------
// Investments — funded (cash out), latest valuation (informational),
// investment income (cash in, NOT operational income) and disposal
// consideration vs actual proceeds stay separate. No gain/loss.
// ------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
pub struct InvestmentReportRow {
    pub id: Uuid,
    pub investment_number: i64,
    pub name: String,
    pub investment_type: String,
    pub status: String,
    pub total_funded: Decimal,
    pub latest_valuation: Option<Decimal>,
    pub latest_valuation_date: Option<Date>,
    pub income_total: Decimal,
    /// Agreed sale consideration (metadata — not cash).
    pub disposal_consideration: Option<Decimal>,
    /// Cash actually received through posted disposal proceeds.
    pub disposal_proceeds_received: Option<Decimal>,
    pub total_count: i64,
}

pub async fn investments_report(
    pool: &PgPool,
    page: i64,
    page_size: i64,
) -> Result<Vec<InvestmentReportRow>, sqlx::Error> {
    sqlx::query_as::<_, InvestmentReportRow>(
        "SELECT i.id, i.investment_number, i.name, i.investment_type, i.status, \
            COALESCE((SELECT sum(f.amount) FROM investment_fundings f \
                WHERE f.investment_id = i.id AND f.status = 'posted'), 0) \
                AS total_funded, \
            (SELECT v.amount FROM investment_valuations v \
                WHERE v.investment_id = i.id AND v.status = 'recorded' \
                ORDER BY v.valuation_date DESC, v.valuation_number DESC LIMIT 1) \
                AS latest_valuation, \
            (SELECT v.valuation_date FROM investment_valuations v \
                WHERE v.investment_id = i.id AND v.status = 'recorded' \
                ORDER BY v.valuation_date DESC, v.valuation_number DESC LIMIT 1) \
                AS latest_valuation_date, \
            COALESCE((SELECT sum(n.amount) FROM investment_incomes n \
                WHERE n.investment_id = i.id AND n.status = 'posted'), 0) \
                AS income_total, \
            (SELECT d.consideration_amount FROM investment_disposals d \
                WHERE d.investment_id = i.id AND d.status = 'posted') \
                AS disposal_consideration, \
            (SELECT sum(pr.amount) FROM investment_disposal_proceeds pr \
                JOIN investment_disposals d ON d.id = pr.disposal_id \
                WHERE d.investment_id = i.id AND d.status = 'posted') \
                AS disposal_proceeds_received, \
            count(*) OVER() AS total_count \
         FROM investments i WHERE i.status <> 'cancelled' \
         ORDER BY i.investment_number DESC, i.id LIMIT $1 OFFSET $2",
    )
    .bind(page_size)
    .bind((page - 1) * page_size)
    .fetch_all(pool)
    .await
}

// ------------------------------------------------------------------
// Social Aid — the (fund, financial_account) restriction dimension is
// preserved EXACTLY: restricted availability is derived per pair and
// reported per pair; a fund-level total is a convenience sum of those
// pairs, never a global bucket (docs/11).
// ------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
pub struct SocialAidFundRow {
    pub fund_id: Uuid,
    pub fund_number: i64,
    pub fund_name: String,
    pub fund_status: String,
    pub account_id: Uuid,
    pub account_name: String,
    pub donations_posted: Decimal,
    pub disbursements_posted: Decimal,
    /// Derived restricted availability for THIS (fund, account) pair —
    /// a classification of physical cash, never additional money.
    pub restricted_available: Decimal,
    pub total_count: i64,
}

pub async fn social_aid_report(
    pool: &PgPool,
    page: i64,
    page_size: i64,
) -> Result<Vec<SocialAidFundRow>, sqlx::Error> {
    sqlx::query_as::<_, SocialAidFundRow>(
        "SELECT f.id AS fund_id, f.fund_number, f.name AS fund_name, f.status AS fund_status, \
            a.id AS account_id, a.name AS account_name, \
            COALESCE(dn.donated,0) AS donations_posted, \
            COALESCE(db.spent,0) AS disbursements_posted, \
            COALESCE(dn.donated,0) - COALESCE(db.spent,0) AS restricted_available, \
            count(*) OVER() AS total_count \
         FROM social_aid_funds f \
         JOIN financial_accounts a ON a.id IN ( \
             SELECT financial_account_id FROM social_aid_donations \
             WHERE fund_id = f.id AND status = 'posted' \
             UNION \
             SELECT financial_account_id FROM social_aid_disbursements \
             WHERE fund_id = f.id AND status = 'posted') \
         LEFT JOIN (SELECT fund_id, financial_account_id, sum(amount) AS donated \
             FROM social_aid_donations WHERE status = 'posted' \
             GROUP BY fund_id, financial_account_id) dn \
             ON dn.fund_id = f.id AND dn.financial_account_id = a.id \
         LEFT JOIN (SELECT fund_id, financial_account_id, sum(amount) AS spent \
             FROM social_aid_disbursements WHERE status = 'posted' \
             GROUP BY fund_id, financial_account_id) db \
             ON db.fund_id = f.id AND db.financial_account_id = a.id \
         WHERE f.status <> 'cancelled' \
         ORDER BY f.fund_number DESC, a.name LIMIT $1 OFFSET $2",
    )
    .bind(page_size)
    .bind((page - 1) * page_size)
    .fetch_all(pool)
    .await
}

/// Funds with no money yet still appear — a zero-availability row with
/// a NULL account pair keeps fund existence visible without inventing
/// money (LEFT JOIN variant of the report above).
#[derive(Debug, sqlx::FromRow)]
pub struct SocialAidFundSummaryRow {
    pub fund_id: Uuid,
    pub fund_number: i64,
    pub fund_name: String,
    pub fund_status: String,
    pub donations_posted: Decimal,
    pub disbursements_posted: Decimal,
    pub restricted_available: Decimal,
}

pub async fn social_aid_fund_summary(
    pool: &PgPool,
) -> Result<Vec<SocialAidFundSummaryRow>, sqlx::Error> {
    sqlx::query_as::<_, SocialAidFundSummaryRow>(
        "SELECT f.id AS fund_id, f.fund_number, f.name AS fund_name, \
            f.status AS fund_status, \
            COALESCE(dn.donated,0) AS donations_posted, \
            COALESCE(db.spent,0) AS disbursements_posted, \
            COALESCE(dn.donated,0) - COALESCE(db.spent,0) AS restricted_available \
         FROM social_aid_funds f \
         LEFT JOIN (SELECT fund_id, sum(amount) AS donated \
             FROM social_aid_donations WHERE status = 'posted' \
             GROUP BY fund_id) dn ON dn.fund_id = f.id \
         LEFT JOIN (SELECT fund_id, sum(amount) AS spent \
             FROM social_aid_disbursements WHERE status = 'posted' \
             GROUP BY fund_id) db ON db.fund_id = f.id \
         WHERE f.status <> 'cancelled' \
         ORDER BY f.fund_number DESC",
    )
    .fetch_all(pool)
    .await
}

// ------------------------------------------------------------------
// Governance activity — recorded evidence counts only. No invented
// approval-rate/quorum arithmetic: outcomes are the recorded formal
// result, tallies are the frozen snapshots.
// ------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
pub struct GovernanceRecentDecisionRow {
    pub id: Uuid,
    pub decision_number: i64,
    pub body_name: String,
    pub title: String,
    pub status: String,
    pub decision_on: Date,
    pub effective_on: Option<Date>,
    pub eligible_count: Option<i64>,
    pub approve_count: Option<i64>,
    pub reject_count: Option<i64>,
    pub abstain_count: Option<i64>,
    pub finalized_at: Option<OffsetDateTime>,
}

pub async fn governance_recent_decisions(
    pool: &PgPool,
) -> Result<Vec<GovernanceRecentDecisionRow>, sqlx::Error> {
    sqlx::query_as::<_, GovernanceRecentDecisionRow>(
        "SELECT d.id, d.decision_number, b.name AS body_name, d.title, \
            d.status, d.decision_on, d.effective_on, \
            d.eligible_count::bigint, d.approve_count::bigint, d.reject_count::bigint, \
            d.abstain_count::bigint, d.finalized_at \
         FROM governance_decisions d \
         JOIN governance_bodies b ON b.id = d.body_id \
         WHERE d.status IN ('approved','rejected') \
         ORDER BY d.finalized_at DESC, d.id LIMIT 10",
    )
    .fetch_all(pool)
    .await
}

// ------------------------------------------------------------------
// Income / Expense monthly trend — buckets by the canonical BUSINESS
// date (occurred_at), never created_at. Posted rows only.
// ------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
pub struct MonthlyFlowRow {
    /// First day of the calendar month (YYYY-MM-DD).
    pub month: Date,
    pub income_total: Decimal,
    pub expense_total: Decimal,
}

pub async fn monthly_income_expense(
    pool: &PgPool,
    months: i64,
) -> Result<Vec<MonthlyFlowRow>, sqlx::Error> {
    sqlx::query_as::<_, MonthlyFlowRow>(
        "SELECT m.month::date AS month, \
            COALESCE(i.total,0) AS income_total, \
            COALESCE(e.total,0) AS expense_total \
         FROM generate_series( \
                date_trunc('month', now() AT TIME ZONE 'Europe/Istanbul')::date \
                    - (($1::int - 1) || ' months')::interval, \
                date_trunc('month', now() AT TIME ZONE 'Europe/Istanbul')::date, \
                interval '1 month') AS m(month) \
         LEFT JOIN ( \
             SELECT date_trunc('month', occurred_at \
                    AT TIME ZONE 'Europe/Istanbul')::date AS month, sum(amount) AS total \
             FROM income_entries WHERE status = 'posted' GROUP BY 1) i \
             ON i.month = m.month::date \
         LEFT JOIN ( \
             SELECT date_trunc('month', occurred_at \
                    AT TIME ZONE 'Europe/Istanbul')::date AS month, sum(amount) AS total \
             FROM expense_entries WHERE status = 'posted' GROUP BY 1) e \
             ON e.month = m.month::date \
         ORDER BY m.month",
    )
    .bind(months as i32)
    .fetch_all(pool)
    .await
}
