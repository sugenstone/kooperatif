//! Reporting API (STEP-015, docs/14, docs/15 §reporting invariants).
//!
//! - `GET /api/reports/overview`              — one consistent snapshot
//! - `GET /api/reports/financial-accounts`    — derived balances + the
//!   restricted social-aid classification of part of that balance
//! - `GET /api/reports/movements`             — provenance ledger
//!   (account/direction/source/date/status filters + filtered totals)
//! - `GET /api/reports/assessments`           — receivable detail
//! - `GET /api/reports/assessments/summary`   — full-filtered-set totals
//! - `GET /api/reports/periods`               — period collection report
//! - `GET /api/reports/shareholders`          — debtor-side financial
//!   position (assessed / cash / credit / remaining kept separate)
//! - `GET /api/reports/families`              — member-level aggregate
//!   presentation (family never owns debt)
//! - `GET /api/reports/payments`              — posted vs allocated vs
//!   credited (payer is never assumed to be the debtor)
//! - `GET /api/reports/payments/summary`
//! - `GET /api/reports/credits`               — credit origin/applied/
//!   available (a classification of received money, not new cash)
//! - `GET /api/reports/share-returns`         — entitlements with NULL
//!   preserved for undetermined rights
//! - `GET /api/reports/investments`           — funded / latest valuation
//!   / investment income / disposal — all separate dimensions
//! - `GET /api/reports/social-aid`            — (fund, account) pairs +
//!   per-fund summaries
//! - `GET /api/reports/governance`            — recorded evidence counts
//!   (no computed quorum/majority)
//! - `GET /api/reports/income-expense-trend`  — monthly posted income /
//!   expense by business date (occurred_at)
//!
//! ALL endpoints are read-only (`reports.read`), `no-store`, and never
//! write to any domain table. Money fields are decimal STRINGS
//! (ADR-004); NULL stays NULL end-to-end (an undetermined entitlement is
//! never rendered as 0.00).

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use time::format_description::FormatItem;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::auth::authz;
use crate::auth::extractor::CurrentAuth;
use crate::financial_accounts::model::MovementSource;
use crate::http::error::ApiError;
use crate::http::AppState;
use crate::parties::routes::{page_of, ListQuery, PaginatedDto};
use crate::payments::model::canonical_amount as canonical;

use super::repo as report_repo;

const DATE_FORMAT: &[FormatItem<'_>] = time::macros::format_description!("[year]-[month]-[day]");

pub fn reports_router() -> Router<AppState> {
    Router::new()
        .route("/api/reports/overview", get(overview))
        .route("/api/reports/financial-accounts", get(accounts))
        .route("/api/reports/movements", get(movements))
        .route("/api/reports/assessments", get(assessments))
        .route("/api/reports/assessments/summary", get(assessments_summary))
        .route("/api/reports/periods", get(periods))
        .route("/api/reports/shareholders", get(shareholders))
        .route("/api/reports/families", get(families))
        .route("/api/reports/payments", get(payments))
        .route("/api/reports/payments/summary", get(payments_summary))
        .route("/api/reports/credits", get(credits))
        .route("/api/reports/share-returns", get(share_returns))
        .route("/api/reports/investments", get(investments))
        .route("/api/reports/social-aid", get(social_aid))
        .route("/api/reports/governance", get(governance))
        .route(
            "/api/reports/income-expense-trend",
            get(income_expense_trend),
        )
        .layer(axum::middleware::from_fn(
            crate::auth::routes::no_store_cache_control,
        ))
}

fn pool_of(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state.db.as_ref().ok_or(ApiError::DependencyUnavailable)
}

fn db_err(context: &'static str) -> impl Fn(sqlx::Error) -> ApiError {
    move |error| {
        tracing::error!(error = %error, context);
        ApiError::Internal
    }
}

fn parse_date(raw: Option<&str>) -> Result<Option<Date>, ApiError> {
    raw.map(|value| Date::parse(value, DATE_FORMAT).map_err(|_| ApiError::ValidationFailed))
        .transpose()
}

fn canonical_opt(amount: Option<rust_decimal::Decimal>) -> Option<String> {
    amount.map(canonical)
}

fn date_to_string(date: Date) -> String {
    date.format(DATE_FORMAT).unwrap_or_default()
}

fn date_opt_to_string(date: Option<Date>) -> Option<String> {
    date.map(date_to_string)
}

// ------------------------------------------------------------------
// Overview
// ------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OverviewDto {
    /// Every number below is a derived metric over authoritative rows —
    /// labels name the MEANING, never an invented aggregate ("net
    /// worth"/"profit"/"assets" are deliberately absent, docs/17).
    currency: String,
    financial_accounts_balance: String,
    financial_accounts_count: i64,
    outstanding_assessment_debt: String,
    assessments_total: String,
    available_shareholder_credit: String,
    operational_income_total: String,
    operational_expense_total: String,
    /// income − expense is an OPERATIONAL flow difference, never a
    /// "cooperative profit" (docs/15: social-aid surplus is never
    /// profit; investment income is not operational income).
    operational_net: String,
    posted_payments_total: String,
    posted_payments_count: i64,
    outstanding_return_entitlement_determined: String,
    undetermined_entitlement_count: i64,
    return_settled_total: String,
    investment_total_funded: String,
    /// Latest recorded valuations summed — informational only, NEVER
    /// cash and never added to the account balance (docs/08/15).
    investment_latest_valuation_total: Option<String>,
    investment_income_total: String,
    investment_active_count: i64,
    /// Classification of physical cash already inside the account
    /// balance — restricted money is not additional money.
    social_aid_restricted_available: String,
    social_aid_donations_total: String,
    social_aid_disbursements_total: String,
    active_shareholder_count: i64,
    active_share_count: i64,
    active_body_count: i64,
    active_membership_count: i64,
    decisions_draft: i64,
    decisions_open: i64,
    decisions_approved: i64,
    decisions_rejected: i64,
    decisions_cancelled: i64,
    votes_total: i64,
}

async fn overview(State(state): State<AppState>, auth: CurrentAuth) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::REPORTS_READ).await?;
    let pool = pool_of(&state)?;
    let row = report_repo::overview(pool)
        .await
        .map_err(db_err("overview failed"))?;
    let operational_net = row.operational_income_total - row.operational_expense_total;
    Ok((
        StatusCode::OK,
        Json(OverviewDto {
            currency: "TRY".to_string(),
            financial_accounts_balance: canonical(row.financial_accounts_balance),
            financial_accounts_count: row.financial_accounts_count,
            outstanding_assessment_debt: canonical(row.outstanding_assessment_debt),
            assessments_total: canonical(row.assessments_total),
            available_shareholder_credit: canonical(row.available_shareholder_credit),
            operational_income_total: canonical(row.operational_income_total),
            operational_expense_total: canonical(row.operational_expense_total),
            operational_net: canonical(operational_net),
            posted_payments_total: canonical(row.posted_payments_total),
            posted_payments_count: row.posted_payments_count,
            outstanding_return_entitlement_determined: canonical(
                row.outstanding_return_entitlement_determined,
            ),
            undetermined_entitlement_count: row.undetermined_entitlement_count,
            return_settled_total: canonical(row.return_settled_total),
            investment_total_funded: canonical(row.investment_total_funded),
            investment_latest_valuation_total: canonical_opt(row.investment_latest_valuation_total),
            investment_income_total: canonical(row.investment_income_total),
            investment_active_count: row.investment_active_count,
            social_aid_restricted_available: canonical(row.social_aid_restricted_available),
            social_aid_donations_total: canonical(row.social_aid_donations_total),
            social_aid_disbursements_total: canonical(row.social_aid_disbursements_total),
            active_shareholder_count: row.active_shareholder_count,
            active_share_count: row.active_share_count,
            active_body_count: row.active_body_count,
            active_membership_count: row.active_membership_count,
            decisions_draft: row.decisions_draft,
            decisions_open: row.decisions_open,
            decisions_approved: row.decisions_approved,
            decisions_rejected: row.decisions_rejected,
            decisions_cancelled: row.decisions_cancelled,
            votes_total: row.votes_total,
        }),
    )
        .into_response())
}

// ------------------------------------------------------------------
// Financial accounts
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountsQuery {
    status: Option<String>,
    #[serde(flatten)]
    list: ListQuery,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountReportDto {
    id: Uuid,
    name: String,
    account_type: String,
    status: String,
    currency: String,
    balance: String,
    /// Restricted social-aid money physically inside `balance` — a
    /// classification of part of it, never additive.
    restricted_available: String,
}

async fn accounts(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<AccountsQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::REPORTS_READ).await?;
    let pool = pool_of(&state)?;
    let status = query
        .status
        .as_deref()
        .filter(|s| matches!(*s, "active" | "inactive"))
        .map(str::to_string);
    if query.status.is_some() && status.is_none() {
        return Err(ApiError::ValidationFailed);
    }
    let (page, page_size) = page_of(&query.list)?;
    let rows = report_repo::accounts_report(pool, status.as_deref(), page, page_size)
        .await
        .map_err(db_err("accounts report failed"))?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    let items = rows
        .into_iter()
        .map(|r| AccountReportDto {
            id: r.id,
            name: r.name,
            account_type: r.account_type,
            status: r.status,
            currency: r.currency,
            balance: canonical(r.balance),
            restricted_available: canonical(r.restricted_available),
        })
        .collect();
    Ok((
        StatusCode::OK,
        Json(PaginatedDto {
            items,
            page,
            page_size,
            total_count,
        }),
    )
        .into_response())
}

// ------------------------------------------------------------------
// Movements — provenance ledger.
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MovementsQuery {
    account_id: Option<Uuid>,
    source_type: Option<String>,
    direction: Option<String>,
    status: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    #[serde(flatten)]
    list: ListQuery,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MovementDto {
    id: Uuid,
    account_id: Uuid,
    account_name: String,
    direction: String,
    amount: String,
    source_type: String,
    source_id: Uuid,
    source_number: Option<i64>,
    #[serde(with = "time::serde::rfc3339")]
    occurred_at: OffsetDateTime,
    status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    reversed_at: Option<OffsetDateTime>,
    reversal_reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MovementsResponse {
    items: Vec<MovementDto>,
    page: i64,
    page_size: i64,
    total_count: i64,
    /// Totals over the FULL filtered set — the page never defines them.
    summary: MovementSummaryDto,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MovementSummaryDto {
    external_inflow: String,
    external_outflow: String,
    /// Internal transfer leg volume — location change only, excluded
    /// from both totals above (docs/15: a transfer is not income).
    internal_transfer_volume: String,
    movement_count: i64,
}

async fn movements(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<MovementsQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::REPORTS_READ).await?;
    let pool = pool_of(&state)?;
    let source_type = query
        .source_type
        .as_deref()
        .map(|s| {
            MovementSource::parse(s)
                .map(|v| v.as_str().to_string())
                .ok_or(ApiError::ValidationFailed)
        })
        .transpose()?;
    let direction = query
        .direction
        .as_deref()
        .filter(|d| matches!(*d, "inflow" | "outflow"));
    if query.direction.is_some() && direction.is_none() {
        return Err(ApiError::ValidationFailed);
    }
    let status = query
        .status
        .as_deref()
        .filter(|s| matches!(*s, "active" | "reversed"));
    if query.status.is_some() && status.is_none() {
        return Err(ApiError::ValidationFailed);
    }
    let filter = report_repo::MovementFilter {
        account_id: query.account_id,
        source_type,
        direction: direction.map(str::to_string),
        status: status.map(str::to_string),
        date_from: parse_date(query.date_from.as_deref())?,
        date_to: parse_date(query.date_to.as_deref())?,
    };
    // Totals describe CURRENT economic flow: with no explicit status
    // filter the summary covers ACTIVE movements only (reversed rows
    // remain traceable in the list but never inflate the totals,
    // docs/15). An explicit `status=reversed` filter instead reports
    // the reversed subset it lists.
    let summary_filter = report_repo::MovementFilter {
        account_id: filter.account_id,
        source_type: filter.source_type.clone(),
        direction: filter.direction.clone(),
        status: Some(
            filter
                .status
                .clone()
                .unwrap_or_else(|| "active".to_string()),
        ),
        date_from: filter.date_from,
        date_to: filter.date_to,
    };
    let (page, page_size) = page_of(&query.list)?;
    let (rows, summary) = tokio::try_join!(
        async {
            report_repo::movements_report(pool, &filter, page, page_size)
                .await
                .map_err(db_err("movements report failed"))
        },
        async {
            report_repo::movements_summary(pool, &summary_filter)
                .await
                .map_err(db_err("movements summary failed"))
        }
    )?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    let items = rows
        .into_iter()
        .map(|r| MovementDto {
            id: r.id,
            account_id: r.account_id,
            account_name: r.account_name,
            direction: r.direction,
            amount: canonical(r.amount),
            source_type: r.source_type,
            source_id: r.source_id,
            source_number: r.source_number,
            occurred_at: r.occurred_at,
            status: r.status,
            reversed_at: r.reversed_at,
            reversal_reason: r.reversal_reason,
        })
        .collect();
    Ok((
        StatusCode::OK,
        Json(MovementsResponse {
            items,
            page,
            page_size,
            total_count,
            summary: MovementSummaryDto {
                external_inflow: canonical(summary.external_inflow),
                external_outflow: canonical(summary.external_outflow),
                internal_transfer_volume: canonical(summary.internal_transfer_volume),
                movement_count: summary.movement_count,
            },
        }),
    )
        .into_response())
}

// ------------------------------------------------------------------
// Assessments — receivable detail + filtered totals.
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssessmentsQuery {
    period_id: Option<Uuid>,
    shareholder_id: Option<Uuid>,
    settlement: Option<String>,
    #[serde(flatten)]
    list: ListQuery,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AssessmentDto {
    id: Uuid,
    period_id: Uuid,
    period_number: i64,
    period_name: String,
    shareholder_id: Uuid,
    shareholder_name: String,
    family_sequence: Option<i64>,
    amount: String,
    paid_amount: String,
    /// The cash-allocation part of `paid_amount`.
    payment_allocated: String,
    /// The credit-application part of `paid_amount` — prior excess
    /// applied, never counted as new cash (docs/15 §period reporting).
    credit_applied: String,
    remaining_amount: String,
    assessment_effective_date: String,
    #[serde(with = "time::serde::rfc3339")]
    generated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AssessmentSummaryDto {
    assessment_count: i64,
    total_assessed: String,
    total_payment_allocated: String,
    total_credit_applied: String,
    total_outstanding: String,
    fully_paid_count: i64,
    partially_paid_count: i64,
    unpaid_count: i64,
}

fn assessment_filter(query: &AssessmentsQuery) -> Result<report_repo::AssessmentFilter, ApiError> {
    let settlement = query
        .settlement
        .as_deref()
        .filter(|s| matches!(*s, "outstanding" | "settled"))
        .map(str::to_string);
    if query.settlement.is_some() && settlement.is_none() {
        return Err(ApiError::ValidationFailed);
    }
    Ok(report_repo::AssessmentFilter {
        period_id: query.period_id,
        shareholder_id: query.shareholder_id,
        settlement,
    })
}

fn assessment_dto(r: report_repo::AssessmentReportRow) -> AssessmentDto {
    AssessmentDto {
        id: r.id,
        period_id: r.period_id,
        period_number: r.period_number,
        period_name: r.period_name,
        shareholder_id: r.shareholder_id,
        shareholder_name: r.shareholder_name,
        family_sequence: r.family_sequence,
        amount: canonical(r.amount),
        paid_amount: canonical(r.paid_amount),
        payment_allocated: canonical(r.payment_allocated),
        credit_applied: canonical(r.credit_applied),
        remaining_amount: canonical(r.remaining_amount),
        assessment_effective_date: date_to_string(r.assessment_effective_date),
        generated_at: r.generated_at,
    }
}

async fn assessments(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<AssessmentsQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::REPORTS_READ).await?;
    let pool = pool_of(&state)?;
    let filter = assessment_filter(&query)?;
    let (page, page_size) = page_of(&query.list)?;
    let rows = report_repo::assessments_report(pool, &filter, page, page_size)
        .await
        .map_err(db_err("assessments report failed"))?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    let items = rows.into_iter().map(assessment_dto).collect();
    Ok((
        StatusCode::OK,
        Json(PaginatedDto {
            items,
            page,
            page_size,
            total_count,
        }),
    )
        .into_response())
}

async fn assessments_summary(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<AssessmentsQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::REPORTS_READ).await?;
    let pool = pool_of(&state)?;
    let filter = assessment_filter(&query)?;
    let s = report_repo::assessments_summary(pool, &filter)
        .await
        .map_err(db_err("assessments summary failed"))?;
    Ok((
        StatusCode::OK,
        Json(AssessmentSummaryDto {
            assessment_count: s.assessment_count,
            total_assessed: canonical(s.total_assessed),
            total_payment_allocated: canonical(s.total_payment_allocated),
            total_credit_applied: canonical(s.total_credit_applied),
            total_outstanding: canonical(s.total_outstanding),
            fully_paid_count: s.fully_paid_count,
            partially_paid_count: s.partially_paid_count,
            unpaid_count: s.unpaid_count,
        }),
    )
        .into_response())
}

// ------------------------------------------------------------------
// Periods
// ------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PeriodReportDto {
    id: Uuid,
    period_number: i64,
    name: String,
    status: String,
    due_date: String,
    assessment_count: i64,
    total_assessed: String,
    /// New shareholder cash allocated to this period.
    payment_allocated: String,
    /// Prior excess applied — credit consumed, not new cash.
    credit_applied: String,
    total_satisfied: String,
    outstanding: String,
    fully_paid_count: i64,
    partially_paid_count: i64,
    unpaid_count: i64,
}

async fn periods(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<ListQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::REPORTS_READ).await?;
    let pool = pool_of(&state)?;
    let (page, page_size) = page_of(&query)?;
    let rows = report_repo::periods_report(pool, page, page_size)
        .await
        .map_err(db_err("periods report failed"))?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    let items = rows
        .into_iter()
        .map(|r| PeriodReportDto {
            id: r.id,
            period_number: r.period_number,
            name: r.name,
            status: r.status,
            due_date: date_to_string(r.due_date),
            assessment_count: r.assessment_count,
            total_assessed: canonical(r.total_assessed),
            payment_allocated: canonical(r.payment_allocated),
            credit_applied: canonical(r.credit_applied),
            total_satisfied: canonical(r.total_satisfied),
            outstanding: canonical(r.outstanding),
            fully_paid_count: r.fully_paid_count,
            partially_paid_count: r.partially_paid_count,
            unpaid_count: r.unpaid_count,
        })
        .collect();
    Ok((
        StatusCode::OK,
        Json(PaginatedDto {
            items,
            page,
            page_size,
            total_count,
        }),
    )
        .into_response())
}

// ------------------------------------------------------------------
// Shareholders
// ------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareholderReportDto {
    shareholder_id: Uuid,
    shareholder_name: String,
    shareholder_status: String,
    family_sequence: Option<i64>,
    active_share_count: i64,
    total_assessed: String,
    payment_allocated: String,
    credit_applied: String,
    remaining_debt: String,
    credit_available: String,
    /// Determined return entitlement outstanding — separate from both
    /// debt and credit; no netting exists (docs/17 unresolved).
    return_entitlement_determined: String,
    undetermined_entitlement_count: i64,
}

async fn shareholders(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<ListQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::REPORTS_READ).await?;
    let pool = pool_of(&state)?;
    let (page, page_size) = page_of(&query)?;
    let rows = report_repo::shareholders_report(pool, query.search.as_deref(), page, page_size)
        .await
        .map_err(db_err("shareholders report failed"))?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    let items = rows
        .into_iter()
        .map(|r| ShareholderReportDto {
            shareholder_id: r.shareholder_id,
            shareholder_name: r.shareholder_name,
            shareholder_status: r.shareholder_status,
            family_sequence: r.family_sequence,
            active_share_count: r.active_share_count,
            total_assessed: canonical(r.total_assessed),
            payment_allocated: canonical(r.payment_allocated),
            credit_applied: canonical(r.credit_applied),
            remaining_debt: canonical(r.remaining_debt),
            credit_available: canonical(r.credit_available),
            return_entitlement_determined: canonical(r.return_entitlement_determined),
            undetermined_entitlement_count: r.undetermined_entitlement_count,
        })
        .collect();
    Ok((
        StatusCode::OK,
        Json(PaginatedDto {
            items,
            page,
            page_size,
            total_count,
        }),
    )
        .into_response())
}

// ------------------------------------------------------------------
// Families — member-level aggregate presentation only.
// ------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FamilyReportDto {
    family_id: Uuid,
    family_sequence: i64,
    member_count: i64,
    member_total_assessed: String,
    member_remaining_debt: String,
    member_credit_available: String,
}

async fn families(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<ListQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::REPORTS_READ).await?;
    let pool = pool_of(&state)?;
    let (page, page_size) = page_of(&query)?;
    let rows = report_repo::families_report(pool, query.search.as_deref(), page, page_size)
        .await
        .map_err(db_err("families report failed"))?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    let items = rows
        .into_iter()
        .map(|r| FamilyReportDto {
            family_id: r.family_id,
            family_sequence: r.family_sequence,
            member_count: r.member_count,
            member_total_assessed: canonical(r.member_total_assessed),
            member_remaining_debt: canonical(r.member_remaining_debt),
            member_credit_available: canonical(r.member_credit_available),
        })
        .collect();
    Ok((
        StatusCode::OK,
        Json(PaginatedDto {
            items,
            page,
            page_size,
            total_count,
        }),
    )
        .into_response())
}

// ------------------------------------------------------------------
// Payments — posted vs allocated vs credited, payer vs debtor kept
// distinct.
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PaymentsQuery {
    status: Option<String>,
    method: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    #[serde(flatten)]
    list: ListQuery,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PaymentReportDto {
    id: Uuid,
    payment_number: i64,
    /// The Person who handed over the value — never assumed to be the
    /// debtor (docs/06 §12–§14).
    payer_name: String,
    method: String,
    amount: String,
    allocated_amount: String,
    credited_amount: String,
    unassigned_amount: String,
    /// Debtor shareholders whose assessments this payment settled.
    debtor_shareholder_names: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    received_at: OffsetDateTime,
    status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    reversed_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PaymentSummaryDto {
    payment_count: i64,
    posted_amount: String,
    allocated_amount: String,
    credited_amount: String,
    unassigned_amount: String,
}

fn payment_filter(query: &PaymentsQuery) -> Result<report_repo::PaymentFilter, ApiError> {
    let status = query
        .status
        .as_deref()
        .filter(|s| matches!(*s, "posted" | "reversed"))
        .map(str::to_string);
    if query.status.is_some() && status.is_none() {
        return Err(ApiError::ValidationFailed);
    }
    let method = query
        .method
        .as_deref()
        .filter(|m| matches!(*m, "cash" | "bank_transfer" | "card" | "other"))
        .map(str::to_string);
    if query.method.is_some() && method.is_none() {
        return Err(ApiError::ValidationFailed);
    }
    Ok(report_repo::PaymentFilter {
        status,
        method,
        date_from: parse_date(query.date_from.as_deref())?,
        date_to: parse_date(query.date_to.as_deref())?,
    })
}

async fn payments(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<PaymentsQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::REPORTS_READ).await?;
    let pool = pool_of(&state)?;
    let filter = payment_filter(&query)?;
    let (page, page_size) = page_of(&query.list)?;
    let (rows, summary) = tokio::try_join!(
        async {
            report_repo::payments_report(pool, &filter, page, page_size)
                .await
                .map_err(db_err("payments report failed"))
        },
        async {
            report_repo::payments_summary(pool, &filter)
                .await
                .map_err(db_err("payments summary failed"))
        }
    )?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    let items: Vec<_> = rows
        .into_iter()
        .map(|r| PaymentReportDto {
            id: r.id,
            payment_number: r.payment_number,
            payer_name: r.payer_name,
            method: r.method,
            amount: canonical(r.amount),
            allocated_amount: canonical(r.allocated_amount),
            credited_amount: canonical(r.credited_amount),
            unassigned_amount: canonical(r.unassigned_amount),
            debtor_shareholder_names: r.debtor_shareholder_names,
            received_at: r.received_at,
            status: r.status,
            reversed_at: r.reversed_at,
        })
        .collect();
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "items": items,
            "page": page,
            "pageSize": page_size,
            "totalCount": total_count,
            "summary": PaymentSummaryDto {
                payment_count: summary.payment_count,
                posted_amount: canonical(summary.posted_amount),
                allocated_amount: canonical(summary.allocated_amount),
                credited_amount: canonical(summary.credited_amount),
                unassigned_amount: canonical(summary.unassigned_amount),
            }
        })),
    )
        .into_response())
}

async fn payments_summary(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<PaymentsQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::REPORTS_READ).await?;
    let pool = pool_of(&state)?;
    let filter = payment_filter(&query)?;
    let s = report_repo::payments_summary(pool, &filter)
        .await
        .map_err(db_err("payments summary failed"))?;
    Ok((
        StatusCode::OK,
        Json(PaymentSummaryDto {
            payment_count: s.payment_count,
            posted_amount: canonical(s.posted_amount),
            allocated_amount: canonical(s.allocated_amount),
            credited_amount: canonical(s.credited_amount),
            unassigned_amount: canonical(s.unassigned_amount),
        }),
    )
        .into_response())
}

// ------------------------------------------------------------------
// Credits
// ------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreditReportDto {
    id: Uuid,
    credit_number: i64,
    shareholder_id: Uuid,
    shareholder_name: String,
    source_payment_number: i64,
    amount: String,
    applied_amount: String,
    available_amount: String,
    status: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreditSummaryDto {
    credit_count: i64,
    total_originated: String,
    total_applied: String,
    total_available: String,
}

async fn credits(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<ListQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::REPORTS_READ).await?;
    let pool = pool_of(&state)?;
    let (page, page_size) = page_of(&query)?;
    let (rows, summary) = tokio::try_join!(
        async {
            report_repo::credits_report(pool, page, page_size)
                .await
                .map_err(db_err("credits report failed"))
        },
        async {
            report_repo::credits_summary(pool)
                .await
                .map_err(db_err("credits summary failed"))
        }
    )?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    let items: Vec<_> = rows
        .into_iter()
        .map(|r| CreditReportDto {
            id: r.id,
            credit_number: r.credit_number,
            shareholder_id: r.shareholder_id,
            shareholder_name: r.shareholder_name,
            source_payment_number: r.source_payment_number,
            amount: canonical(r.amount),
            applied_amount: canonical(r.applied_amount),
            available_amount: canonical(r.available_amount),
            status: r.status,
            created_at: r.created_at,
        })
        .collect();
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "items": items,
            "page": page,
            "pageSize": page_size,
            "totalCount": total_count,
            "summary": CreditSummaryDto {
                credit_count: summary.credit_count,
                total_originated: canonical(summary.total_originated),
                total_applied: canonical(summary.total_applied),
                total_available: canonical(summary.total_available),
            }
        })),
    )
        .into_response())
}

// ------------------------------------------------------------------
// Share return entitlements — NULL amount stays NULL.
// ------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EntitlementDto {
    id: Uuid,
    entitlement_number: i64,
    return_id: Uuid,
    return_number: i64,
    share_number: i64,
    beneficiary_name: String,
    entitlement_type: String,
    /// NULL = undetermined (e.g. Profit Right) — never "0.00".
    amount: Option<String>,
    settled_amount: String,
    remaining_amount: Option<String>,
    due_date: Option<String>,
    due_state: String,
    status: String,
    #[serde(with = "time::serde::rfc3339")]
    recognized_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EntitlementSummaryDto {
    determined_outstanding: String,
    undetermined_count: i64,
    settled_total: String,
}

async fn share_returns(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<ListQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::REPORTS_READ).await?;
    let pool = pool_of(&state)?;
    let (page, page_size) = page_of(&query)?;
    let (rows, summary) = tokio::try_join!(
        async {
            report_repo::entitlements_report(pool, page, page_size)
                .await
                .map_err(db_err("entitlements report failed"))
        },
        async {
            report_repo::entitlements_summary(pool)
                .await
                .map_err(db_err("entitlements summary failed"))
        }
    )?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    let items: Vec<_> = rows
        .into_iter()
        .map(|r| EntitlementDto {
            id: r.id,
            entitlement_number: r.entitlement_number,
            return_id: r.return_id,
            return_number: r.return_number,
            share_number: r.share_number,
            beneficiary_name: r.beneficiary_name,
            entitlement_type: r.entitlement_type,
            amount: canonical_opt(r.amount),
            settled_amount: canonical(r.settled_amount),
            remaining_amount: canonical_opt(r.remaining_amount),
            due_date: date_opt_to_string(r.due_date),
            due_state: r.due_state,
            status: r.status,
            recognized_at: r.recognized_at,
        })
        .collect();
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "items": items,
            "page": page,
            "pageSize": page_size,
            "totalCount": total_count,
            "summary": EntitlementSummaryDto {
                determined_outstanding: canonical(summary.determined_outstanding),
                undetermined_count: summary.undetermined_count,
                settled_total: canonical(summary.settled_total),
            }
        })),
    )
        .into_response())
}

// ------------------------------------------------------------------
// Investments
// ------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InvestmentReportDto {
    id: Uuid,
    investment_number: i64,
    name: String,
    investment_type: String,
    status: String,
    /// Cash funded (posted fundings) — a cost dimension.
    total_funded: String,
    /// Informational only — never cash, never profit vs funding.
    latest_valuation: Option<String>,
    latest_valuation_date: Option<String>,
    /// Investment cash income — NOT operational income (STEP-012).
    income_total: String,
    /// Agreed consideration metadata — not received cash.
    disposal_consideration: Option<String>,
    /// Cash actually received via posted disposal proceeds.
    disposal_proceeds_received: Option<String>,
}

async fn investments(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<ListQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::REPORTS_READ).await?;
    let pool = pool_of(&state)?;
    let (page, page_size) = page_of(&query)?;
    let rows = report_repo::investments_report(pool, page, page_size)
        .await
        .map_err(db_err("investments report failed"))?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    let items = rows
        .into_iter()
        .map(|r| InvestmentReportDto {
            id: r.id,
            investment_number: r.investment_number,
            name: r.name,
            investment_type: r.investment_type,
            status: r.status,
            total_funded: canonical(r.total_funded),
            latest_valuation: canonical_opt(r.latest_valuation),
            latest_valuation_date: date_opt_to_string(r.latest_valuation_date),
            income_total: canonical(r.income_total),
            disposal_consideration: canonical_opt(r.disposal_consideration),
            disposal_proceeds_received: canonical_opt(r.disposal_proceeds_received),
        })
        .collect();
    Ok((
        StatusCode::OK,
        Json(PaginatedDto {
            items,
            page,
            page_size,
            total_count,
        }),
    )
        .into_response())
}

// ------------------------------------------------------------------
// Social Aid — (fund, account) pairs preserve the restriction dimension.
// ------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SocialAidPairDto {
    fund_id: Uuid,
    fund_number: i64,
    fund_name: String,
    fund_status: String,
    account_id: Uuid,
    account_name: String,
    donations_posted: String,
    disbursements_posted: String,
    /// Classification of physical cash — never additive to balance.
    restricted_available: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SocialAidFundSummaryDto {
    fund_id: Uuid,
    fund_number: i64,
    fund_name: String,
    fund_status: String,
    donations_posted: String,
    disbursements_posted: String,
    restricted_available: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SocialAidResponse {
    /// Fund-level convenience rows — a sum over the (fund, account)
    /// pairs below; the pairs remain the authoritative dimension.
    funds: Vec<SocialAidFundSummaryDto>,
    pairs: Vec<SocialAidPairDto>,
    page: i64,
    page_size: i64,
    total_count: i64,
}

async fn social_aid(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<ListQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::REPORTS_READ).await?;
    let pool = pool_of(&state)?;
    let (page, page_size) = page_of(&query)?;
    let (pairs, funds) = tokio::try_join!(
        async {
            report_repo::social_aid_report(pool, page, page_size)
                .await
                .map_err(db_err("social aid report failed"))
        },
        async {
            report_repo::social_aid_fund_summary(pool)
                .await
                .map_err(db_err("social aid summary failed"))
        }
    )?;
    let total_count = pairs.first().map(|r| r.total_count).unwrap_or(0);
    Ok((
        StatusCode::OK,
        Json(SocialAidResponse {
            funds: funds
                .into_iter()
                .map(|r| SocialAidFundSummaryDto {
                    fund_id: r.fund_id,
                    fund_number: r.fund_number,
                    fund_name: r.fund_name,
                    fund_status: r.fund_status,
                    donations_posted: canonical(r.donations_posted),
                    disbursements_posted: canonical(r.disbursements_posted),
                    restricted_available: canonical(r.restricted_available),
                })
                .collect(),
            pairs: pairs
                .into_iter()
                .map(|r| SocialAidPairDto {
                    fund_id: r.fund_id,
                    fund_number: r.fund_number,
                    fund_name: r.fund_name,
                    fund_status: r.fund_status,
                    account_id: r.account_id,
                    account_name: r.account_name,
                    donations_posted: canonical(r.donations_posted),
                    disbursements_posted: canonical(r.disbursements_posted),
                    restricted_available: canonical(r.restricted_available),
                })
                .collect(),
            page,
            page_size,
            total_count,
        }),
    )
        .into_response())
}

// ------------------------------------------------------------------
// Governance — evidence counts + the frozen snapshots of recent
// finalized decisions. Recorded outcome only; never recomputed.
// ------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GovernanceDecisionDto {
    id: Uuid,
    decision_number: i64,
    body_name: String,
    title: String,
    /// The RECORDED formal outcome — never a computed result.
    status: String,
    decision_on: String,
    effective_on: Option<String>,
    eligible_count: Option<i64>,
    approve_count: Option<i64>,
    reject_count: Option<i64>,
    abstain_count: Option<i64>,
    #[serde(with = "time::serde::rfc3339::option")]
    finalized_at: Option<OffsetDateTime>,
}

async fn governance(
    State(state): State<AppState>,
    auth: CurrentAuth,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::REPORTS_READ).await?;
    let pool = pool_of(&state)?;
    let overview = report_repo::overview(pool)
        .await
        .map_err(db_err("governance report failed"))?;
    let recent = report_repo::governance_recent_decisions(pool)
        .await
        .map_err(db_err("governance decisions failed"))?;
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "activeBodyCount": overview.active_body_count,
            "activeMembershipCount": overview.active_membership_count,
            "decisionsByStatus": {
                "draft": overview.decisions_draft,
                "open": overview.decisions_open,
                "approved": overview.decisions_approved,
                "rejected": overview.decisions_rejected,
                "cancelled": overview.decisions_cancelled
            },
            "votesTotal": overview.votes_total,
            "recentFinalized": recent.into_iter().map(|r| GovernanceDecisionDto {
                id: r.id,
                decision_number: r.decision_number,
                body_name: r.body_name,
                title: r.title,
                status: r.status,
                decision_on: date_to_string(r.decision_on),
                effective_on: date_opt_to_string(r.effective_on),
                eligible_count: r.eligible_count,
                approve_count: r.approve_count,
                reject_count: r.reject_count,
                abstain_count: r.abstain_count,
                finalized_at: r.finalized_at,
            }).collect::<Vec<_>>()
        })),
    )
        .into_response())
}

// ------------------------------------------------------------------
// Income/expense monthly trend — business-date buckets.
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrendQuery {
    months: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MonthlyFlowDto {
    /// Bucket = calendar month of `occurred_at` (business date), never
    /// `created_at`.
    month: String,
    income_total: String,
    expense_total: String,
}

async fn income_expense_trend(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<TrendQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::REPORTS_READ).await?;
    let pool = pool_of(&state)?;
    let months = query.months.unwrap_or(6);
    if !(1..=24).contains(&months) {
        return Err(ApiError::ValidationFailed);
    }
    let rows = report_repo::monthly_income_expense(pool, months)
        .await
        .map_err(db_err("income-expense trend failed"))?;
    let items: Vec<MonthlyFlowDto> = rows
        .into_iter()
        .map(|r| MonthlyFlowDto {
            month: date_to_string(r.month),
            income_total: canonical(r.income_total),
            expense_total: canonical(r.expense_total),
        })
        .collect();
    Ok((StatusCode::OK, Json(items)).into_response())
}
