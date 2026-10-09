//! Investment API (STEP-012, docs/08).
//!
//! - `GET  /api/investments`                            (investments.read)
//! - `POST /api/investments`                            (investments.manage)
//! - `GET  /api/investments/{id}`                       (investments.read)
//! - `POST /api/investments/{id}/cancel`                (investments.manage)
//! - `POST /api/investments/{id}/fundings`              (investments.manage)
//! - `POST /api/investment-fundings/{id}/reverse`       (investments.manage)
//! - `POST /api/investments/{id}/valuations`            (investments.manage)
//! - `POST /api/investment-valuations/{id}/cancel`      (investments.manage)
//! - `POST /api/investments/{id}/incomes`               (investments.manage)
//! - `POST /api/investment-incomes/{id}/reverse`        (investments.manage)
//! - `POST /api/investments/{id}/dispose`               (investments.manage)
//!
//! Acquisition funding is NOT Expense/Transfer/Payment — each leg posts
//! exactly ONE outflow movement ('investment_funding'). Investment
//! income posts exactly ONE inflow ('investment_income') and never
//! creates a STEP-010 `incomes` row. Valuations move ZERO money.
//! Disposal records ACTUAL proceeds per leg ('investment_disposal');
//! the agreed consideration is informational — no profit is computed
//! (docs/08/17: realized-gain formula UNRESOLVED). There is
//! deliberately NO DELETE — posted history is corrected by reversal
//! (docs/19). Money fields are decimal STRINGS (ADR-004).

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::auth::audit::{self, SecurityEventType};
use crate::auth::authz;
use crate::auth::csrf;
use crate::auth::extractor::CurrentAuth;
use crate::auth::routes::csrf_rejection_to_api_error;
use crate::http::error::ApiError;
use crate::http::AppState;
use crate::investments::model as inv_model;
use crate::investments::repo as inv_repo;
use crate::investments::repo::InvestmentCommandError;
use crate::parties::routes::{page_of, ListQuery, PaginatedDto};
use crate::payments::model as payment_model;

pub fn investments_router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/investments",
            get(list_investments).post(create_investment),
        )
        .route("/api/investments/{id}", get(get_investment))
        .route("/api/investments/{id}/cancel", post(cancel_investment))
        .route("/api/investments/{id}/fundings", post(post_funding))
        .route(
            "/api/investment-fundings/{id}/reverse",
            post(reverse_funding),
        )
        .route("/api/investments/{id}/valuations", post(record_valuation))
        .route(
            "/api/investment-valuations/{id}/cancel",
            post(cancel_valuation),
        )
        .route("/api/investments/{id}/incomes", post(post_income))
        .route("/api/investment-incomes/{id}/reverse", post(reverse_income))
        .route("/api/investments/{id}/dispose", post(dispose_investment))
        .layer(axum::middleware::from_fn(
            crate::auth::routes::no_store_cache_control,
        ))
}

fn pool_of(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state.db.as_ref().ok_or(ApiError::DependencyUnavailable)
}

/// Investment mutations share the full CSRF + permission prologue.
macro_rules! investments_mutation {
    ($state:expr, $headers:expr, $auth:expr, $method:expr) => {{
        csrf::validate_origin($headers, $method, &$state.auth.config.allowed_origins)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        csrf::validate_csrf_token($headers, &$auth.csrf_token)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        authz::require($state, $auth, authz::catalog::INVESTMENTS_MANAGE).await?;
        pool_of($state)?
    }};
}

fn command_error(error: InvestmentCommandError) -> ApiError {
    use InvestmentCommandError as E;
    match error {
        E::NotFound => ApiError::NotFound,
        E::StaleState => ApiError::StaleState,
        E::InvalidState | E::InactiveAccount => ApiError::ValidationFailed,
        E::HasFinancialEvents | E::InsufficientFunds | E::IdempotencyConflict => ApiError::Conflict,
        E::InsufficientUnrestrictedFunds => ApiError::InsufficientUnrestrictedFunds,
        E::AccountInvariant(inner) => {
            tracing::error!(error = ?inner, "unexpected account-layer error");
            ApiError::Internal
        }
        E::Database(error) => {
            tracing::error!(error = %error, "investment command failed");
            ApiError::Internal
        }
    }
}

fn db_err(context: &'static str) -> impl Fn(sqlx::Error) -> ApiError {
    move |error| {
        tracing::error!(error = %error, "{context}");
        ApiError::Internal
    }
}

fn canonical(amount: rust_decimal::Decimal) -> String {
    payment_model::canonical_amount(amount)
}

fn canonical_opt(amount: Option<rust_decimal::Decimal>) -> Option<String> {
    amount.map(payment_model::canonical_amount)
}

fn istanbul_today(now: OffsetDateTime) -> Date {
    now.to_offset(time::UtcOffset::from_hms(3, 0, 0).expect("UTC+3"))
        .date()
}

fn parse_business_date(raw: &str) -> Result<Date, ApiError> {
    Date::parse(
        raw.trim(),
        &time::macros::format_description!("[year]-[month]-[day]"),
    )
    .map_err(|_| ApiError::ValidationFailed)
}

fn parse_occurred_at(raw: &str) -> Result<OffsetDateTime, ApiError> {
    OffsetDateTime::parse(raw.trim(), &Rfc3339).map_err(|_| ApiError::ValidationFailed)
}

// ------------------------------------------------------------------
// DTOs (mirror @kooperatif/contracts/src/investments.ts)
// ------------------------------------------------------------------

/// `time`'s serde modules only cover `OffsetDateTime`; calendar dates
/// cross the API as ISO `YYYY-MM-DD` strings.
mod date_iso {
    use serde::Serializer;
    use time::{format_description::well_known::Iso8601, Date};

    pub fn serialize<S: Serializer>(date: &Date, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(
            &date
                .format(&Iso8601::DATE)
                .map_err(serde::ser::Error::custom)?,
        )
    }

    pub mod option {
        use super::*;

        pub fn serialize<S: Serializer>(date: &Option<Date>, s: S) -> Result<S::Ok, S::Error> {
            match date {
                Some(d) => super::serialize(d, s),
                None => s.serialize_none(),
            }
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvestmentListItemDto {
    pub id: Uuid,
    pub investment_number: i64,
    pub name: String,
    pub investment_type: String,
    pub status: String,
    #[serde(with = "date_iso::option")]
    pub acquired_at: Option<Date>,
    pub total_funded: String,
    /// Latest recorded valuation — informational, NEVER cash. NULL
    /// when no recorded valuation exists.
    pub latest_valuation: Option<String>,
    pub total_income: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FundingDto {
    pub id: Uuid,
    pub funding_number: i64,
    pub investment_id: Uuid,
    pub financial_account_id: Uuid,
    pub account_name: String,
    pub amount: String,
    pub currency: String,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    pub reference: Option<String>,
    pub note: Option<String>,
    pub account_movement_id: Uuid,
    pub movement_status: String,
    pub status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValuationDto {
    pub id: Uuid,
    pub valuation_number: i64,
    pub investment_id: Uuid,
    #[serde(with = "date_iso")]
    pub valuation_date: Date,
    pub amount: String,
    pub currency: String,
    pub method: Option<String>,
    pub source: Option<String>,
    pub note: Option<String>,
    pub status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub cancelled_at: Option<OffsetDateTime>,
    pub cancellation_reason: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomeDto {
    pub id: Uuid,
    pub income_number: i64,
    pub investment_id: Uuid,
    pub financial_account_id: Uuid,
    pub account_name: String,
    pub amount: String,
    pub currency: String,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    pub description: String,
    pub counterparty: Option<String>,
    pub reference_no: Option<String>,
    pub account_movement_id: Uuid,
    pub movement_status: String,
    pub status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProceedsDto {
    pub id: Uuid,
    pub financial_account_id: Uuid,
    pub account_name: String,
    pub amount: String,
    pub currency: String,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    pub reference: Option<String>,
    pub account_movement_id: Uuid,
    pub movement_status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisposalDto {
    pub id: Uuid,
    pub disposal_number: i64,
    pub investment_id: Uuid,
    #[serde(with = "date_iso")]
    pub disposed_at: Date,
    /// Agreed sale price — informational metadata; actual cash is on
    /// the proceeds legs. No realized-gain formula exists (UNRESOLVED).
    pub consideration_amount: Option<String>,
    pub currency: String,
    pub counterparty_name: Option<String>,
    pub reference: Option<String>,
    pub note: Option<String>,
    pub status: String,
    pub total_proceeds: String,
    pub proceeds: Vec<ProceedsDto>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvestmentDetailDto {
    pub id: Uuid,
    pub investment_number: i64,
    pub name: String,
    pub investment_type: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub reference: Option<String>,
    pub counterparty_name: Option<String>,
    #[serde(with = "date_iso::option")]
    pub acquired_at: Option<Date>,
    pub status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub disposed_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub cancelled_at: Option<OffsetDateTime>,
    pub cancellation_reason: Option<String>,
    /// Derived: sum of posted funding legs (actual acquisition cash).
    pub total_funded: String,
    /// Derived: newest recorded valuation — informational only.
    pub latest_valuation: Option<String>,
    /// Derived: sum of posted cash-income legs.
    pub total_income: String,
    /// Derived: sum of disposal proceeds legs (actual cash received).
    pub total_proceeds: String,
    pub fundings: Vec<FundingDto>,
    pub valuations: Vec<ValuationDto>,
    pub incomes: Vec<IncomeDto>,
    pub disposal: Option<DisposalDto>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

fn funding_dto(row: &inv_repo::FundingRow) -> FundingDto {
    FundingDto {
        id: row.id,
        funding_number: row.funding_number,
        investment_id: row.investment_id,
        financial_account_id: row.financial_account_id,
        account_name: row.account_name.clone(),
        amount: canonical(row.amount),
        currency: "TRY".to_string(),
        occurred_at: row.occurred_at,
        reference: row.reference.clone(),
        note: row.note.clone(),
        account_movement_id: row.account_movement_id,
        movement_status: row.movement_status.clone(),
        status: row.status.clone(),
        reversed_at: row.reversed_at,
        reversal_reason: row.reversal_reason.clone(),
    }
}

fn valuation_dto(row: &inv_repo::ValuationRow) -> ValuationDto {
    ValuationDto {
        id: row.id,
        valuation_number: row.valuation_number,
        investment_id: row.investment_id,
        valuation_date: row.valuation_date,
        amount: canonical(row.amount),
        currency: "TRY".to_string(),
        method: row.method.clone(),
        source: row.source.clone(),
        note: row.note.clone(),
        status: row.status.clone(),
        cancelled_at: row.cancelled_at,
        cancellation_reason: row.cancellation_reason.clone(),
        created_at: row.created_at,
    }
}

fn income_dto(row: &inv_repo::IncomeRow) -> IncomeDto {
    IncomeDto {
        id: row.id,
        income_number: row.income_number,
        investment_id: row.investment_id,
        financial_account_id: row.financial_account_id,
        account_name: row.account_name.clone(),
        amount: canonical(row.amount),
        currency: "TRY".to_string(),
        occurred_at: row.occurred_at,
        description: row.description.clone(),
        counterparty: row.counterparty.clone(),
        reference_no: row.reference_no.clone(),
        account_movement_id: row.account_movement_id,
        movement_status: row.movement_status.clone(),
        status: row.status.clone(),
        reversed_at: row.reversed_at,
        reversal_reason: row.reversal_reason.clone(),
    }
}

fn detail_dto(detail: inv_repo::InvestmentDetail) -> InvestmentDetailDto {
    let disposal = detail.disposal.map(|d| DisposalDto {
        total_proceeds: canonical(
            d.proceeds
                .iter()
                .map(|p| p.amount)
                .fold(rust_decimal::Decimal::ZERO, |a, b| a + b),
        ),
        proceeds: d
            .proceeds
            .iter()
            .map(|p| ProceedsDto {
                id: p.id,
                financial_account_id: p.financial_account_id,
                account_name: p.account_name.clone(),
                amount: canonical(p.amount),
                currency: "TRY".to_string(),
                occurred_at: p.occurred_at,
                reference: p.reference.clone(),
                account_movement_id: p.account_movement_id,
                movement_status: p.movement_status.clone(),
            })
            .collect(),
        id: d.id,
        disposal_number: d.disposal_number,
        investment_id: d.investment_id,
        disposed_at: d.disposed_at,
        consideration_amount: canonical_opt(d.consideration_amount),
        currency: "TRY".to_string(),
        counterparty_name: d.counterparty_name,
        reference: d.reference,
        note: d.note,
        status: d.status,
        created_at: d.created_at,
    });
    InvestmentDetailDto {
        id: detail.investment.id,
        investment_number: detail.investment.investment_number,
        name: detail.investment.name,
        investment_type: detail.investment.investment_type,
        description: detail.investment.description,
        location: detail.investment.location,
        reference: detail.investment.reference,
        counterparty_name: detail.investment.counterparty_name,
        acquired_at: detail.investment.acquired_at,
        status: detail.investment.status,
        disposed_at: detail.investment.disposed_at,
        cancelled_at: detail.investment.cancelled_at,
        cancellation_reason: detail.investment.cancellation_reason,
        total_funded: canonical(detail.total_funded),
        latest_valuation: canonical_opt(detail.latest_valuation),
        total_income: canonical(detail.total_income),
        total_proceeds: canonical(detail.total_proceeds),
        fundings: detail.fundings.iter().map(funding_dto).collect(),
        valuations: detail.valuations.iter().map(valuation_dto).collect(),
        incomes: detail.incomes.iter().map(income_dto).collect(),
        disposal,
        created_at: detail.investment.created_at,
        updated_at: detail.investment.updated_at,
    }
}

async fn load_detail(
    pool: &sqlx::PgPool,
    id: Uuid,
) -> Result<Option<InvestmentDetailDto>, ApiError> {
    inv_repo::get_investment_detail(pool, id)
        .await
        .map_err(db_err("investment detail query failed"))
        .map(|opt| opt.map(detail_dto))
}

// ------------------------------------------------------------------
// List + detail (investments.read)
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvestmentListQuery {
    // Inlined instead of `#[serde(flatten)]`: serde_urlencoded cannot
    // coerce numeric types through flattened maps (`pageSize=20` →
    // "invalid type: string"), which broke filtered list requests.
    pub search: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub status: Option<String>,
    pub investment_type: Option<String>,
}

pub async fn list_investments(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<InvestmentListQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::INVESTMENTS_READ).await?;
    let pool = pool_of(&state)?;
    let list = ListQuery {
        search: query.search.clone(),
        page: query.page,
        page_size: query.page_size,
        default_account_id: None,
    };
    let (page, page_size) = page_of(&list)?;
    let status = query
        .status
        .as_deref()
        .map(|s| {
            inv_model::InvestmentStatus::parse(s)
                .map(|v| v.as_str().to_string())
                .ok_or(ApiError::ValidationFailed)
        })
        .transpose()?;
    let investment_type = query
        .investment_type
        .as_deref()
        .map(|t| {
            inv_model::InvestmentType::parse(t)
                .map(|v| v.as_str().to_string())
                .ok_or(ApiError::ValidationFailed)
        })
        .transpose()?;
    let search = list
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let rows = inv_repo::list_investments(
        pool,
        &inv_repo::InvestmentFilter {
            status,
            investment_type,
            search,
            page,
            page_size,
        },
    )
    .await
    .map_err(db_err("investment list query failed"))?;

    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    Ok(Json(PaginatedDto {
        items: rows
            .iter()
            .map(|r| InvestmentListItemDto {
                id: r.id,
                investment_number: r.investment_number,
                name: r.name.clone(),
                investment_type: r.investment_type.clone(),
                status: r.status.clone(),
                acquired_at: r.acquired_at,
                total_funded: canonical(r.total_funded),
                latest_valuation: canonical_opt(r.latest_valuation),
                total_income: canonical(r.total_income),
                created_at: r.created_at,
            })
            .collect(),
        page,
        page_size,
        total_count,
    })
    .into_response())
}

pub async fn get_investment(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::INVESTMENTS_READ).await?;
    let pool = pool_of(&state)?;
    load_detail(pool, id)
        .await?
        .map(Json)
        .map(IntoResponse::into_response)
        .ok_or(ApiError::NotFound)
}

// ------------------------------------------------------------------
// Create + cancel
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct CreateInvestmentRequest {
    pub name: String,
    pub investment_type: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub reference: Option<String>,
    pub counterparty_name: Option<String>,
    /// Optional business DATE `YYYY-MM-DD` — identity metadata only;
    /// cash timing lives on funding legs.
    pub acquired_at: Option<String>,
    pub idempotency_key: String,
}

pub async fn create_investment(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Json(body): Json<CreateInvestmentRequest>,
) -> Result<Response, ApiError> {
    let pool = investments_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let today = istanbul_today(now);

    let name = inv_model::validate_name(&body.name).map_err(|_| ApiError::ValidationFailed)?;
    let investment_type = inv_model::InvestmentType::parse(&body.investment_type)
        .ok_or(ApiError::ValidationFailed)?;
    let description = inv_model::validate_description(body.description.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let location = inv_model::validate_location(body.location.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let reference = inv_model::validate_reference(body.reference.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let counterparty_name = inv_model::validate_counterparty(body.counterparty_name.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let acquired_at = body
        .acquired_at
        .as_deref()
        .map(|raw| {
            parse_business_date(raw).and_then(|d| {
                inv_model::validate_business_date(d, today).map_err(|_| ApiError::ValidationFailed)
            })
        })
        .transpose()?;
    let idempotency_key = inv_model::validate_idempotency_key(&body.idempotency_key)
        .map_err(|_| ApiError::ValidationFailed)?;
    let fingerprint = inv_model::create_fingerprint(&inv_model::CreateInvestmentFingerprint {
        name: name.clone(),
        investment_type,
        description: description.clone(),
        location: location.clone(),
        reference: reference.clone(),
        counterparty_name: counterparty_name.clone(),
        acquired_at,
    });

    let outcome = inv_repo::create_investment(
        pool,
        auth.user_id,
        inv_repo::CreateInvestment {
            name,
            investment_type,
            description,
            location,
            reference,
            counterparty_name,
            acquired_at,
            idempotency_key,
            fingerprint,
        },
    )
    .await
    .map_err(command_error)?;

    if !outcome.replayed {
        audit::record(
            pool,
            SecurityEventType::InvestmentCreated,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "investmentId": outcome.investment_id,
                "investmentNumber": outcome.investment_number,
            }),
        )
        .await;
        tracing::info!(
            outcome = "investment_created", id = %outcome.investment_id,
            number = outcome.investment_number, actor = %auth.user_id
        );
    }

    let detail = load_detail(pool, outcome.investment_id)
        .await?
        .ok_or(ApiError::Internal)?;
    let status = if outcome.replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(detail)).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct CancelInvestmentRequest {
    pub reason: String,
}

pub async fn cancel_investment(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<CancelInvestmentRequest>,
) -> Result<Response, ApiError> {
    let pool = investments_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let reason = inv_model::validate_reversal_reason(&body.reason)
        .map_err(|_| ApiError::ValidationFailed)?;

    let number = inv_repo::cancel_investment(pool, auth.user_id, id, reason, now)
        .await
        .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::InvestmentCancelled,
        Some(auth.user_id),
        None,
        serde_json::json!({ "investmentId": id, "investmentNumber": number }),
    )
    .await;

    let detail = load_detail(pool, id).await?.ok_or(ApiError::Internal)?;
    Ok((StatusCode::OK, Json(detail)).into_response())
}

// ------------------------------------------------------------------
// Acquisition funding
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct PostFundingRequest {
    pub financial_account_id: Uuid,
    /// Exact decimal string (ADR-004).
    pub amount: String,
    /// RFC-3339 timestamp; may be backdated, never future.
    pub occurred_at: String,
    pub reference: Option<String>,
    pub note: Option<String>,
    pub idempotency_key: String,
}

pub async fn post_funding(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<PostFundingRequest>,
) -> Result<Response, ApiError> {
    let pool = investments_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();

    let amount = inv_model::validate_positive_amount(&body.amount)
        .map_err(|_| ApiError::ValidationFailed)?;
    let occurred_at = inv_model::validate_occurred_at(parse_occurred_at(&body.occurred_at)?, now)
        .map_err(|_| ApiError::ValidationFailed)?;
    let reference = inv_model::validate_reference(body.reference.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let note =
        inv_model::validate_note(body.note.as_deref()).map_err(|_| ApiError::ValidationFailed)?;
    let idempotency_key = inv_model::validate_idempotency_key(&body.idempotency_key)
        .map_err(|_| ApiError::ValidationFailed)?;
    let fingerprint = inv_model::funding_fingerprint(
        id,
        body.financial_account_id,
        amount,
        occurred_at,
        &reference,
        &note,
    );

    let outcome = inv_repo::post_funding(
        pool,
        auth.user_id,
        inv_repo::PostFunding {
            investment_id: id,
            financial_account_id: body.financial_account_id,
            amount,
            occurred_at,
            reference,
            note,
            idempotency_key,
            fingerprint,
        },
    )
    .await
    .map_err(command_error)?;

    if !outcome.replayed {
        audit::record(
            pool,
            SecurityEventType::InvestmentFundingPosted,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "fundingId": outcome.event_id,
                "fundingNumber": outcome.event_number,
                "investmentId": id,
                "amount": canonical(amount),
            }),
        )
        .await;
        tracing::info!(
            outcome = "investment_funding_posted", id = %outcome.event_id,
            investment = %id, actor = %auth.user_id
        );
    }

    let detail = load_detail(pool, id).await?.ok_or(ApiError::Internal)?;
    let status = if outcome.replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(detail)).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct ReverseRequest {
    pub reason: String,
}

pub async fn reverse_funding(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<ReverseRequest>,
) -> Result<Response, ApiError> {
    let pool = investments_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let reason = inv_model::validate_reversal_reason(&body.reason)
        .map_err(|_| ApiError::ValidationFailed)?;

    let funding: Option<(Uuid, i64)> = sqlx::query_as(
        "SELECT investment_id, funding_number FROM investment_fundings WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err("funding lookup failed"))?;
    let Some((investment_id, _number)) = funding else {
        return Err(ApiError::NotFound);
    };

    let (replayed, number) = inv_repo::reverse_funding(pool, auth.user_id, id, reason, now)
        .await
        .map_err(command_error)?;

    if !replayed {
        audit::record(
            pool,
            SecurityEventType::InvestmentFundingReversed,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "fundingId": id,
                "fundingNumber": number,
                "investmentId": investment_id,
            }),
        )
        .await;
    }

    let detail = load_detail(pool, investment_id)
        .await?
        .ok_or(ApiError::Internal)?;
    Ok((StatusCode::OK, Json(detail)).into_response())
}

// ------------------------------------------------------------------
// Valuations (informational — zero money)
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct RecordValuationRequest {
    /// Business DATE `YYYY-MM-DD`; may be backdated, never future.
    pub valuation_date: String,
    /// Exact decimal string (ADR-004); strictly positive.
    pub amount: String,
    pub method: Option<String>,
    pub source: Option<String>,
    pub note: Option<String>,
    pub idempotency_key: String,
}

pub async fn record_valuation(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<RecordValuationRequest>,
) -> Result<Response, ApiError> {
    let pool = investments_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let today = istanbul_today(now);

    let valuation_date =
        inv_model::validate_business_date(parse_business_date(&body.valuation_date)?, today)
            .map_err(|_| ApiError::ValidationFailed)?;
    let amount = inv_model::validate_positive_amount(&body.amount)
        .map_err(|_| ApiError::ValidationFailed)?;
    let method = inv_model::validate_method(body.method.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let source = inv_model::validate_source(body.source.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let note =
        inv_model::validate_note(body.note.as_deref()).map_err(|_| ApiError::ValidationFailed)?;
    let idempotency_key = inv_model::validate_idempotency_key(&body.idempotency_key)
        .map_err(|_| ApiError::ValidationFailed)?;
    let fingerprint =
        inv_model::valuation_fingerprint(id, valuation_date, amount, &method, &source, &note);

    let outcome = inv_repo::record_valuation(
        pool,
        auth.user_id,
        inv_repo::RecordValuation {
            investment_id: id,
            valuation_date,
            amount,
            method,
            source,
            note,
            idempotency_key,
            fingerprint,
        },
    )
    .await
    .map_err(command_error)?;

    if !outcome.replayed {
        audit::record(
            pool,
            SecurityEventType::InvestmentValuationRecorded,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "valuationId": outcome.event_id,
                "valuationNumber": outcome.event_number,
                "investmentId": id,
                "amount": canonical(amount),
            }),
        )
        .await;
        tracing::info!(
            outcome = "investment_valuation_recorded", id = %outcome.event_id,
            investment = %id, actor = %auth.user_id
        );
    }

    let detail = load_detail(pool, id).await?.ok_or(ApiError::Internal)?;
    let status = if outcome.replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(detail)).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct CancelValuationRequest {
    pub reason: String,
}

pub async fn cancel_valuation(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<CancelValuationRequest>,
) -> Result<Response, ApiError> {
    let pool = investments_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let reason = inv_model::validate_reversal_reason(&body.reason)
        .map_err(|_| ApiError::ValidationFailed)?;

    let row: Option<(Uuid, i64)> = sqlx::query_as(
        "SELECT investment_id, valuation_number FROM investment_valuations WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err("valuation lookup failed"))?;
    let Some((investment_id, _number)) = row else {
        return Err(ApiError::NotFound);
    };

    let (replayed, number) = inv_repo::cancel_valuation(pool, auth.user_id, id, reason, now)
        .await
        .map_err(command_error)?;

    if !replayed {
        audit::record(
            pool,
            SecurityEventType::InvestmentValuationCancelled,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "valuationId": id,
                "valuationNumber": number,
                "investmentId": investment_id,
            }),
        )
        .await;
    }

    let detail = load_detail(pool, investment_id)
        .await?
        .ok_or(ApiError::Internal)?;
    Ok((StatusCode::OK, Json(detail)).into_response())
}

// ------------------------------------------------------------------
// Investment cash income
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct PostIncomeRequest {
    pub financial_account_id: Uuid,
    /// Exact decimal string (ADR-004); strictly positive.
    pub amount: String,
    pub occurred_at: String,
    pub description: String,
    pub counterparty: Option<String>,
    pub reference_no: Option<String>,
    pub idempotency_key: String,
}

pub async fn post_income(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<PostIncomeRequest>,
) -> Result<Response, ApiError> {
    let pool = investments_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();

    let amount = inv_model::validate_positive_amount(&body.amount)
        .map_err(|_| ApiError::ValidationFailed)?;
    let occurred_at = inv_model::validate_occurred_at(parse_occurred_at(&body.occurred_at)?, now)
        .map_err(|_| ApiError::ValidationFailed)?;
    let description = inv_model::validate_income_description(&body.description)
        .map_err(|_| ApiError::ValidationFailed)?;
    let counterparty = inv_model::validate_counterparty(body.counterparty.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let reference_no = inv_model::validate_reference(body.reference_no.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let idempotency_key = inv_model::validate_idempotency_key(&body.idempotency_key)
        .map_err(|_| ApiError::ValidationFailed)?;
    let fingerprint = inv_model::income_fingerprint(
        id,
        body.financial_account_id,
        amount,
        occurred_at,
        &description,
        &counterparty,
        &reference_no,
    );

    let outcome = inv_repo::post_income(
        pool,
        auth.user_id,
        inv_repo::PostIncome {
            investment_id: id,
            financial_account_id: body.financial_account_id,
            amount,
            occurred_at,
            description,
            counterparty,
            reference_no,
            idempotency_key,
            fingerprint,
        },
    )
    .await
    .map_err(command_error)?;

    if !outcome.replayed {
        audit::record(
            pool,
            SecurityEventType::InvestmentIncomePosted,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "incomeId": outcome.event_id,
                "incomeNumber": outcome.event_number,
                "investmentId": id,
                "amount": canonical(amount),
            }),
        )
        .await;
        tracing::info!(
            outcome = "investment_income_posted", id = %outcome.event_id,
            investment = %id, actor = %auth.user_id
        );
    }

    let detail = load_detail(pool, id).await?.ok_or(ApiError::Internal)?;
    let status = if outcome.replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(detail)).into_response())
}

pub async fn reverse_income(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<ReverseRequest>,
) -> Result<Response, ApiError> {
    let pool = investments_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let reason = inv_model::validate_reversal_reason(&body.reason)
        .map_err(|_| ApiError::ValidationFailed)?;

    let row: Option<(Uuid, i64)> =
        sqlx::query_as("SELECT investment_id, income_number FROM investment_incomes WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(db_err("income lookup failed"))?;
    let Some((investment_id, _number)) = row else {
        return Err(ApiError::NotFound);
    };

    let (replayed, number) = inv_repo::reverse_income(pool, auth.user_id, id, reason, now)
        .await
        .map_err(command_error)?;

    if !replayed {
        audit::record(
            pool,
            SecurityEventType::InvestmentIncomeReversed,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "incomeId": id,
                "incomeNumber": number,
                "investmentId": investment_id,
            }),
        )
        .await;
    }

    let detail = load_detail(pool, investment_id)
        .await?
        .ok_or(ApiError::Internal)?;
    Ok((StatusCode::OK, Json(detail)).into_response())
}

// ------------------------------------------------------------------
// Disposal
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct ProceedsLegRequest {
    pub financial_account_id: Uuid,
    /// Exact decimal string (ADR-004); strictly positive.
    pub amount: String,
    pub occurred_at: String,
    pub reference: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct DisposeInvestmentRequest {
    /// Business DATE `YYYY-MM-DD`; may be backdated, never future.
    pub disposed_at: String,
    /// Agreed sale price — informational metadata, decimal string.
    pub consideration_amount: Option<String>,
    pub counterparty_name: Option<String>,
    pub reference: Option<String>,
    pub note: Option<String>,
    /// Actual cash legs — at least one; each posts exactly ONE inflow
    /// movement.
    pub proceeds: Vec<ProceedsLegRequest>,
    pub idempotency_key: String,
}

pub async fn dispose_investment(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<DisposeInvestmentRequest>,
) -> Result<Response, ApiError> {
    let pool = investments_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let today = istanbul_today(now);

    if body.proceeds.is_empty() {
        return Err(ApiError::ValidationFailed);
    }
    let disposed_at =
        inv_model::validate_business_date(parse_business_date(&body.disposed_at)?, today)
            .map_err(|_| ApiError::ValidationFailed)?;
    let consideration_amount =
        inv_model::validate_consideration(body.consideration_amount.as_deref())
            .map_err(|_| ApiError::ValidationFailed)?;
    let counterparty_name = inv_model::validate_counterparty(body.counterparty_name.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let reference = inv_model::validate_reference(body.reference.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let note =
        inv_model::validate_note(body.note.as_deref()).map_err(|_| ApiError::ValidationFailed)?;
    let idempotency_key = inv_model::validate_idempotency_key(&body.idempotency_key)
        .map_err(|_| ApiError::ValidationFailed)?;

    let mut legs = Vec::with_capacity(body.proceeds.len());
    let mut fingerprint_legs = Vec::with_capacity(body.proceeds.len());
    for leg in &body.proceeds {
        let amount = inv_model::validate_positive_amount(&leg.amount)
            .map_err(|_| ApiError::ValidationFailed)?;
        let occurred_at =
            inv_model::validate_occurred_at(parse_occurred_at(&leg.occurred_at)?, now)
                .map_err(|_| ApiError::ValidationFailed)?;
        let leg_reference = inv_model::validate_reference(leg.reference.as_deref())
            .map_err(|_| ApiError::ValidationFailed)?;
        fingerprint_legs.push((
            leg.financial_account_id,
            amount,
            occurred_at,
            leg_reference.clone(),
        ));
        legs.push(inv_repo::ProceedsLeg {
            financial_account_id: leg.financial_account_id,
            amount,
            occurred_at,
            reference: leg_reference,
        });
    }
    let fingerprint = inv_model::disposal_fingerprint(
        id,
        disposed_at,
        &consideration_amount,
        &counterparty_name,
        &reference,
        &note,
        &fingerprint_legs,
    );

    let outcome = inv_repo::dispose_investment(
        pool,
        auth.user_id,
        inv_repo::DisposeInvestment {
            investment_id: id,
            disposed_at,
            consideration_amount,
            counterparty_name,
            reference,
            note,
            proceeds: legs,
            idempotency_key,
            fingerprint,
        },
        now,
    )
    .await
    .map_err(command_error)?;

    if !outcome.replayed {
        audit::record(
            pool,
            SecurityEventType::InvestmentDisposed,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "disposalId": outcome.event_id,
                "disposalNumber": outcome.event_number,
                "investmentId": id,
            }),
        )
        .await;
        tracing::info!(
            outcome = "investment_disposed", id = %outcome.event_id,
            investment = %id, actor = %auth.user_id
        );
    }

    let detail = load_detail(pool, id).await?.ok_or(ApiError::Internal)?;
    let status = if outcome.replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(detail)).into_response())
}
