//! Social Aid API (STEP-013, docs/11).
//!
//! - `GET  /api/social-aid/funds`                        (social_aid.read)
//! - `POST /api/social-aid/funds`                        (social_aid.manage)
//! - `GET  /api/social-aid/funds/{id}`                   (social_aid.read)
//! - `POST /api/social-aid/funds/{id}/close`             (social_aid.manage)
//! - `POST /api/social-aid/funds/{id}/cancel`            (social_aid.manage)
//! - `GET  /api/social-aid/donations`                    (social_aid.read)
//! - `POST /api/social-aid/donations`                    (social_aid.manage)
//! - `GET  /api/social-aid/donations/{id}`               (social_aid.read)
//! - `POST /api/social-aid/donations/{id}/reverse`       (social_aid.manage)
//! - `GET  /api/social-aid/disbursements`                (social_aid.read)
//! - `POST /api/social-aid/disbursements`                (social_aid.manage)
//! - `GET  /api/social-aid/disbursements/{id}`           (social_aid.read)
//! - `POST /api/social-aid/disbursements/{id}/reverse`   (social_aid.manage)
//! - `GET  /api/social-aid/persons`                      (social_aid.manage)
//!
//! A Donation posts exactly ONE inflow movement ('social_aid_donation')
//! and raises the fund's restricted availability in that account. An
//! Aid Disbursement posts exactly ONE outflow ('social_aid_disbursement')
//! gated on BOTH the fund's restricted availability in the chosen
//! account AND the physical account balance — restricted money cannot
//! be spent merely because unrelated cash exists. Neither touches
//! Payment/Allocation/Credit/Income/Expense/Transfer/ShareReturn/
//! Investment. There is deliberately NO DELETE — posted history is
//! corrected by reversal (docs/19). Money fields are decimal STRINGS
//! (ADR-004).

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
use crate::parties::repo as party_repo;
use crate::parties::routes::{page_of, ListQuery, PaginatedDto};
use crate::payments::model as payment_model;
use crate::social_aid::model as aid_model;
use crate::social_aid::repo as aid_repo;
use crate::social_aid::repo::SocialAidError;

pub fn social_aid_router() -> Router<AppState> {
    Router::new()
        .route("/api/social-aid/funds", get(list_funds).post(create_fund))
        .route("/api/social-aid/funds/{id}", get(get_fund))
        .route("/api/social-aid/funds/{id}/close", post(close_fund))
        .route("/api/social-aid/funds/{id}/cancel", post(cancel_fund))
        .route(
            "/api/social-aid/donations",
            get(list_donations).post(post_donation),
        )
        .route("/api/social-aid/donations/{id}", get(get_donation))
        .route(
            "/api/social-aid/donations/{id}/reverse",
            post(reverse_donation),
        )
        .route(
            "/api/social-aid/disbursements",
            get(list_disbursements).post(post_disbursement),
        )
        .route("/api/social-aid/disbursements/{id}", get(get_disbursement))
        .route(
            "/api/social-aid/disbursements/{id}/reverse",
            post(reverse_disbursement),
        )
        .route("/api/social-aid/persons", get(person_lookup))
        .layer(axum::middleware::from_fn(
            crate::auth::routes::no_store_cache_control,
        ))
}

fn pool_of(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state.db.as_ref().ok_or(ApiError::DependencyUnavailable)
}

/// Social Aid mutations share the full CSRF + permission prologue.
macro_rules! social_aid_mutation {
    ($state:expr, $headers:expr, $auth:expr, $method:expr) => {{
        csrf::validate_origin($headers, $method, &$state.auth.config.allowed_origins)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        csrf::validate_csrf_token($headers, &$auth.csrf_token)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        authz::require($state, $auth, authz::catalog::SOCIAL_AID_MANAGE).await?;
        pool_of($state)?
    }};
}

fn command_error(error: SocialAidError) -> ApiError {
    use SocialAidError as E;
    match error {
        E::NotFound => ApiError::NotFound,
        E::InvalidState | E::InactiveAccount => ApiError::ValidationFailed,
        E::HasRestrictedBalance
        | E::InsufficientFunds
        | E::InsufficientRestricted
        | E::IdempotencyConflict => ApiError::Conflict,
        E::AccountInvariant(inner) => {
            tracing::error!(error = ?inner, "unexpected account-layer error");
            ApiError::Internal
        }
        E::Database(error) => {
            tracing::error!(error = %error, "social aid command failed");
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
// DTOs (mirror @kooperatif/contracts/src/social-aid.ts)
// ------------------------------------------------------------------

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
pub struct FundListItemDto {
    pub id: Uuid,
    pub fund_number: i64,
    pub name: String,
    pub status: String,
    /// Derived restricted totals — never stored.
    pub total_donated: String,
    pub total_disbursed: String,
    pub available: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FundAccountDto {
    pub financial_account_id: Uuid,
    pub account_name: String,
    pub donated: String,
    pub disbursed: String,
    /// Restricted availability of this fund IN this account.
    pub available: String,
    /// Physical balance of the account (all money, every context).
    pub physical_balance: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DonationDto {
    pub id: Uuid,
    pub donation_number: i64,
    pub fund_id: Uuid,
    /// Canonical Person link — may be a shareholder, guardian or any
    /// registered person; membership is never required.
    pub donor_person_id: Option<Uuid>,
    /// Resolved person name when linked.
    pub donor_name: Option<String>,
    /// Free-text identity for external persons/organizations.
    pub donor_display_name: Option<String>,
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
pub struct DisbursementDto {
    pub id: Uuid,
    pub disbursement_number: i64,
    pub fund_id: Uuid,
    pub beneficiary_person_id: Option<Uuid>,
    pub beneficiary_name: Option<String>,
    pub beneficiary_display_name: Option<String>,
    pub financial_account_id: Uuid,
    pub account_name: String,
    pub amount: String,
    pub currency: String,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    /// Why the aid was granted — required (decision pipeline deferred).
    pub reason: String,
    pub reference: Option<String>,
    pub account_movement_id: Uuid,
    pub movement_status: String,
    pub status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FundDetailDto {
    pub id: Uuid,
    pub fund_number: i64,
    pub name: String,
    pub description: Option<String>,
    #[serde(with = "date_iso::option")]
    pub starts_on: Option<Date>,
    #[serde(with = "date_iso::option")]
    pub ends_on: Option<Date>,
    pub status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub closed_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub cancelled_at: Option<OffsetDateTime>,
    pub cancellation_reason: Option<String>,
    /// Derived restricted totals — never stored columns.
    pub total_donated: String,
    pub total_disbursed: String,
    pub available: String,
    pub accounts: Vec<FundAccountDto>,
    pub donations: Vec<DonationDto>,
    pub disbursements: Vec<DisbursementDto>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

fn donation_dto(row: &aid_repo::DonationRow) -> DonationDto {
    DonationDto {
        id: row.id,
        donation_number: row.donation_number,
        fund_id: row.fund_id,
        donor_person_id: row.donor_person_id,
        donor_name: row.donor_name.clone(),
        donor_display_name: row.donor_display_name.clone(),
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

fn disbursement_dto(row: &aid_repo::DisbursementRow) -> DisbursementDto {
    DisbursementDto {
        id: row.id,
        disbursement_number: row.disbursement_number,
        fund_id: row.fund_id,
        beneficiary_person_id: row.beneficiary_person_id,
        beneficiary_name: row.beneficiary_name.clone(),
        beneficiary_display_name: row.beneficiary_display_name.clone(),
        financial_account_id: row.financial_account_id,
        account_name: row.account_name.clone(),
        amount: canonical(row.amount),
        currency: "TRY".to_string(),
        occurred_at: row.occurred_at,
        reason: row.reason.clone(),
        reference: row.reference.clone(),
        account_movement_id: row.account_movement_id,
        movement_status: row.movement_status.clone(),
        status: row.status.clone(),
        reversed_at: row.reversed_at,
        reversal_reason: row.reversal_reason.clone(),
    }
}

fn detail_dto(detail: aid_repo::FundDetail) -> FundDetailDto {
    let total_donated: rust_decimal::Decimal = detail
        .donations
        .iter()
        .filter(|d| d.status == "posted")
        .map(|d| d.amount)
        .sum();
    let total_disbursed: rust_decimal::Decimal = detail
        .disbursements
        .iter()
        .filter(|d| d.status == "posted")
        .map(|d| d.amount)
        .sum();
    FundDetailDto {
        id: detail.fund.id,
        fund_number: detail.fund.fund_number,
        name: detail.fund.name,
        description: detail.fund.description,
        starts_on: detail.fund.starts_on,
        ends_on: detail.fund.ends_on,
        status: detail.fund.status,
        closed_at: detail.fund.closed_at,
        cancelled_at: detail.fund.cancelled_at,
        cancellation_reason: detail.fund.cancellation_reason,
        available: canonical(total_donated - total_disbursed),
        total_donated: canonical(total_donated),
        total_disbursed: canonical(total_disbursed),
        accounts: detail
            .accounts
            .iter()
            .map(|a| FundAccountDto {
                financial_account_id: a.financial_account_id,
                account_name: a.account_name.clone(),
                donated: canonical(a.donated),
                disbursed: canonical(a.disbursed),
                available: canonical(a.available),
                physical_balance: canonical(a.physical_balance),
            })
            .collect(),
        donations: detail.donations.iter().map(donation_dto).collect(),
        disbursements: detail.disbursements.iter().map(disbursement_dto).collect(),
        created_at: detail.fund.created_at,
        updated_at: detail.fund.updated_at,
    }
}

async fn load_detail(pool: &sqlx::PgPool, id: Uuid) -> Result<Option<FundDetailDto>, ApiError> {
    aid_repo::get_fund_detail(pool, id)
        .await
        .map_err(db_err("social aid fund detail query failed"))
        .map(|opt| opt.map(detail_dto))
}

// ------------------------------------------------------------------
// Fund list + detail (social_aid.read)
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FundListQuery {
    pub search: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub status: Option<String>,
}

pub async fn list_funds(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<FundListQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::SOCIAL_AID_READ).await?;
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
            aid_model::FundStatus::parse(s)
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

    let rows = aid_repo::list_funds(
        pool,
        &aid_repo::FundFilter {
            status,
            search,
            page,
            page_size,
        },
    )
    .await
    .map_err(db_err("social aid fund list query failed"))?;

    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    Ok(Json(PaginatedDto {
        items: rows
            .iter()
            .map(|r| FundListItemDto {
                id: r.id,
                fund_number: r.fund_number,
                name: r.name.clone(),
                status: r.status.clone(),
                total_donated: canonical(r.total_donated),
                total_disbursed: canonical(r.total_disbursed),
                available: canonical(r.available),
                created_at: r.created_at,
            })
            .collect(),
        page,
        page_size,
        total_count,
    })
    .into_response())
}

pub async fn get_fund(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::SOCIAL_AID_READ).await?;
    let pool = pool_of(&state)?;
    load_detail(pool, id)
        .await?
        .map(Json)
        .map(IntoResponse::into_response)
        .ok_or(ApiError::NotFound)
}

// ------------------------------------------------------------------
// Person lookup for donor/beneficiary selection (social_aid.manage)
// ------------------------------------------------------------------
// Mirrors `/api/payments/payer-persons`: a donor or beneficiary may be
// ANY Person — never only shareholders — and the aid operator must be
// able to resolve that canonical link without holding full parties
// read rights (docs/02 Party reuse, docs/11 privacy).

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonLookupQuery {
    pub search: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonLookupDto {
    pub id: Uuid,
    pub first_name: String,
    pub last_name: String,
    pub shareholder_id: Option<Uuid>,
    pub shareholder_status: Option<String>,
}

pub async fn person_lookup(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<PersonLookupQuery>,
) -> Result<Json<Vec<PersonLookupDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::SOCIAL_AID_MANAGE).await?;
    let pool = pool_of(&state)?;
    let search = query.search.as_deref().unwrap_or_default();
    if search.trim().is_empty() {
        return Ok(Json(Vec::new()));
    }
    let limit = query.limit.unwrap_or(10).clamp(1, 25);
    let rows = party_repo::search_persons(pool, search.trim(), limit)
        .await
        .map_err(db_err("social aid person lookup failed"))?;
    Ok(Json(
        rows.iter()
            .map(|r| PersonLookupDto {
                id: r.id,
                first_name: r.first_name.clone(),
                last_name: r.last_name.clone(),
                shareholder_id: r.shareholder_id,
                shareholder_status: r.shareholder_status.clone(),
            })
            .collect(),
    ))
}

// ------------------------------------------------------------------
// Donation + disbursement registers (social_aid.read)
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventListQuery {
    pub fund_id: Option<Uuid>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

pub async fn list_donations(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<EventListQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::SOCIAL_AID_READ).await?;
    let pool = pool_of(&state)?;
    let (page, page_size) = page_of(&ListQuery {
        search: None,
        page: query.page,
        page_size: query.page_size,
        default_account_id: None,
    })?;
    let (rows, total) = aid_repo::list_donations(
        pool,
        &aid_repo::DonationFilter {
            fund_id: query.fund_id,
            page,
            page_size,
        },
    )
    .await
    .map_err(db_err("social aid donation list query failed"))?;
    Ok(Json(PaginatedDto {
        items: rows.iter().map(donation_dto).collect::<Vec<_>>(),
        page,
        page_size,
        total_count: total,
    })
    .into_response())
}

pub async fn get_donation(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::SOCIAL_AID_READ).await?;
    let pool = pool_of(&state)?;
    aid_repo::get_donation(pool, id)
        .await
        .map_err(db_err("social aid donation query failed"))?
        .map(|r| Json(donation_dto(&r)).into_response())
        .ok_or(ApiError::NotFound)
}

pub async fn list_disbursements(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<EventListQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::SOCIAL_AID_READ).await?;
    let pool = pool_of(&state)?;
    let (page, page_size) = page_of(&ListQuery {
        search: None,
        page: query.page,
        page_size: query.page_size,
        default_account_id: None,
    })?;
    let (rows, total) = aid_repo::list_disbursements(
        pool,
        &aid_repo::DisbursementFilter {
            fund_id: query.fund_id,
            page,
            page_size,
        },
    )
    .await
    .map_err(db_err("social aid disbursement list query failed"))?;
    Ok(Json(PaginatedDto {
        items: rows.iter().map(disbursement_dto).collect::<Vec<_>>(),
        page,
        page_size,
        total_count: total,
    })
    .into_response())
}

pub async fn get_disbursement(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::SOCIAL_AID_READ).await?;
    let pool = pool_of(&state)?;
    aid_repo::get_disbursement(pool, id)
        .await
        .map_err(db_err("social aid disbursement query failed"))?
        .map(|r| Json(disbursement_dto(&r)).into_response())
        .ok_or(ApiError::NotFound)
}

// ------------------------------------------------------------------
// Fund commands (social_aid.manage)
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateFundRequest {
    pub name: String,
    pub description: Option<String>,
    /// Optional program window `YYYY-MM-DD` — metadata only.
    pub starts_on: Option<String>,
    pub ends_on: Option<String>,
    pub idempotency_key: String,
}

pub async fn create_fund(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Json(body): Json<CreateFundRequest>,
) -> Result<Response, ApiError> {
    let pool = social_aid_mutation!(&state, &headers, &auth, &Method::POST);

    let name = aid_model::validate_name(&body.name).map_err(|_| ApiError::ValidationFailed)?;
    let description = aid_model::validate_description(body.description.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let starts_on = body
        .starts_on
        .as_deref()
        .map(parse_business_date)
        .transpose()?;
    let ends_on = body
        .ends_on
        .as_deref()
        .map(parse_business_date)
        .transpose()?;
    let (starts_on, ends_on) =
        aid_model::validate_window(starts_on, ends_on).map_err(|_| ApiError::ValidationFailed)?;
    let idempotency_key = aid_model::validate_idempotency_key(&body.idempotency_key)
        .map_err(|_| ApiError::ValidationFailed)?;
    let fingerprint = aid_model::create_fund_fingerprint(&name, &description, starts_on, ends_on);

    let outcome = aid_repo::create_fund(
        pool,
        auth.user_id,
        aid_repo::CreateFund {
            name,
            description,
            starts_on,
            ends_on,
            idempotency_key,
            fingerprint,
        },
    )
    .await
    .map_err(command_error)?;

    if !outcome.replayed {
        audit::record(
            pool,
            SecurityEventType::SocialAidFundCreated,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "fundId": outcome.fund_id,
                "fundNumber": outcome.fund_number,
            }),
        )
        .await;
        tracing::info!(
            outcome = "social_aid_fund_created", id = %outcome.fund_id,
            number = outcome.fund_number, actor = %auth.user_id
        );
    }

    let detail = load_detail(pool, outcome.fund_id)
        .await?
        .ok_or(ApiError::Internal)?;
    let status = if outcome.replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(detail)).into_response())
}

/// Close is terminal (STEP-013: no reopen). Only allowed while every
/// (fund, account) restricted availability is zero.
pub async fn close_fund(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let pool = social_aid_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();

    let number = aid_repo::close_fund(pool, auth.user_id, id, now)
        .await
        .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::SocialAidFundClosed,
        Some(auth.user_id),
        None,
        serde_json::json!({ "fundId": id, "fundNumber": number }),
    )
    .await;

    let detail = load_detail(pool, id).await?.ok_or(ApiError::Internal)?;
    Ok((StatusCode::OK, Json(detail)).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelFundRequest {
    pub reason: String,
}

pub async fn cancel_fund(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<CancelFundRequest>,
) -> Result<Response, ApiError> {
    let pool = social_aid_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let reason =
        aid_model::validate_reason(&body.reason).map_err(|_| ApiError::ValidationFailed)?;

    let number = aid_repo::cancel_fund(pool, auth.user_id, id, reason, now)
        .await
        .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::SocialAidFundCancelled,
        Some(auth.user_id),
        None,
        serde_json::json!({ "fundId": id, "fundNumber": number }),
    )
    .await;

    let detail = load_detail(pool, id).await?.ok_or(ApiError::Internal)?;
    Ok((StatusCode::OK, Json(detail)).into_response())
}

// ------------------------------------------------------------------
// Donation command (social_aid.manage)
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostDonationRequest {
    pub fund_id: Uuid,
    /// Canonical Person link (shareholder/guardian/any person) — XOR
    /// with `donorDisplayName` semantics enforced below.
    pub donor_person_id: Option<Uuid>,
    /// External person/organization display name.
    pub donor_display_name: Option<String>,
    pub financial_account_id: Uuid,
    /// Exact decimal string (ADR-004).
    pub amount: String,
    /// RFC-3339 timestamp; may be backdated, never future.
    pub occurred_at: String,
    pub reference: Option<String>,
    pub note: Option<String>,
    pub idempotency_key: String,
}

pub async fn post_donation(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Json(body): Json<PostDonationRequest>,
) -> Result<Response, ApiError> {
    let pool = social_aid_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();

    let amount = aid_model::validate_positive_amount(&body.amount)
        .map_err(|_| ApiError::ValidationFailed)?;
    let occurred_at = aid_model::validate_occurred_at(parse_occurred_at(&body.occurred_at)?, now)
        .map_err(|_| ApiError::ValidationFailed)?;
    let donor_display_name = aid_model::validate_identity_name(body.donor_display_name.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    // Anonymous donations are an open decision (docs/11) — at least
    // one identity form is mandatory.
    if body.donor_person_id.is_none() && donor_display_name.is_none() {
        return Err(ApiError::ValidationFailed);
    }
    let reference = aid_model::validate_reference(body.reference.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let note =
        aid_model::validate_note(body.note.as_deref()).map_err(|_| ApiError::ValidationFailed)?;
    let idempotency_key = aid_model::validate_idempotency_key(&body.idempotency_key)
        .map_err(|_| ApiError::ValidationFailed)?;
    let fingerprint = aid_model::donation_fingerprint(&aid_model::DonationFingerprint {
        fund_id: body.fund_id,
        donor_person_id: body.donor_person_id,
        donor_display_name: donor_display_name.clone(),
        financial_account_id: body.financial_account_id,
        amount,
        occurred_at,
        reference: reference.clone(),
        note: note.clone(),
    });

    let outcome = aid_repo::post_donation(
        pool,
        auth.user_id,
        aid_repo::PostDonation {
            fund_id: body.fund_id,
            donor_person_id: body.donor_person_id,
            donor_display_name,
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
            SecurityEventType::SocialAidDonationPosted,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "donationId": outcome.event_id,
                "donationNumber": outcome.event_number,
                "fundId": body.fund_id,
            }),
        )
        .await;
        tracing::info!(
            outcome = "social_aid_donation_posted", id = %outcome.event_id,
            number = outcome.event_number, fund = %body.fund_id, actor = %auth.user_id
        );
    }

    let detail = aid_repo::get_donation(pool, outcome.event_id)
        .await
        .map_err(db_err("social aid donation query failed"))?
        .ok_or(ApiError::Internal)?;
    let status = if outcome.replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(donation_dto(&detail))).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReverseRequest {
    pub reason: String,
}

pub async fn reverse_donation(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<ReverseRequest>,
) -> Result<Response, ApiError> {
    let pool = social_aid_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let reason = aid_model::validate_reversal_reason(&body.reason)
        .map_err(|_| ApiError::ValidationFailed)?;

    let (already, number) = aid_repo::reverse_donation(pool, auth.user_id, id, reason, now)
        .await
        .map_err(command_error)?;

    if !already {
        audit::record(
            pool,
            SecurityEventType::SocialAidDonationReversed,
            Some(auth.user_id),
            None,
            serde_json::json!({ "donationId": id, "donationNumber": number }),
        )
        .await;
    }

    let detail = aid_repo::get_donation(pool, id)
        .await
        .map_err(db_err("social aid donation query failed"))?
        .ok_or(ApiError::Internal)?;
    Ok((StatusCode::OK, Json(donation_dto(&detail))).into_response())
}

// ------------------------------------------------------------------
// Aid disbursement command (social_aid.manage)
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostDisbursementRequest {
    pub fund_id: Uuid,
    pub beneficiary_person_id: Option<Uuid>,
    pub beneficiary_display_name: Option<String>,
    pub financial_account_id: Uuid,
    /// Exact decimal string (ADR-004).
    pub amount: String,
    /// RFC-3339 timestamp; may be backdated, never future.
    pub occurred_at: String,
    /// Why the aid was granted — required.
    pub reason: String,
    pub reference: Option<String>,
    pub idempotency_key: String,
}

pub async fn post_disbursement(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Json(body): Json<PostDisbursementRequest>,
) -> Result<Response, ApiError> {
    let pool = social_aid_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();

    let amount = aid_model::validate_positive_amount(&body.amount)
        .map_err(|_| ApiError::ValidationFailed)?;
    let occurred_at = aid_model::validate_occurred_at(parse_occurred_at(&body.occurred_at)?, now)
        .map_err(|_| ApiError::ValidationFailed)?;
    let beneficiary_display_name =
        aid_model::validate_identity_name(body.beneficiary_display_name.as_deref())
            .map_err(|_| ApiError::ValidationFailed)?;
    // A beneficiary identity is always required (docs/11 §privacy:
    // beneficiary data is sensitive and identity is preserved).
    if body.beneficiary_person_id.is_none() && beneficiary_display_name.is_none() {
        return Err(ApiError::ValidationFailed);
    }
    let reason =
        aid_model::validate_reason(&body.reason).map_err(|_| ApiError::ValidationFailed)?;
    let reference = aid_model::validate_reference(body.reference.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let idempotency_key = aid_model::validate_idempotency_key(&body.idempotency_key)
        .map_err(|_| ApiError::ValidationFailed)?;
    let fingerprint = aid_model::disbursement_fingerprint(&aid_model::DisbursementFingerprint {
        fund_id: body.fund_id,
        beneficiary_person_id: body.beneficiary_person_id,
        beneficiary_display_name: beneficiary_display_name.clone(),
        financial_account_id: body.financial_account_id,
        amount,
        occurred_at,
        reason: reason.clone(),
        reference: reference.clone(),
    });

    let outcome = aid_repo::post_disbursement(
        pool,
        auth.user_id,
        aid_repo::PostDisbursement {
            fund_id: body.fund_id,
            beneficiary_person_id: body.beneficiary_person_id,
            beneficiary_display_name,
            financial_account_id: body.financial_account_id,
            amount,
            occurred_at,
            reason,
            reference,
            idempotency_key,
            fingerprint,
        },
    )
    .await
    .map_err(command_error)?;

    if !outcome.replayed {
        audit::record(
            pool,
            SecurityEventType::SocialAidDisbursementPosted,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "disbursementId": outcome.event_id,
                "disbursementNumber": outcome.event_number,
                "fundId": body.fund_id,
            }),
        )
        .await;
        tracing::info!(
            outcome = "social_aid_disbursement_posted", id = %outcome.event_id,
            number = outcome.event_number, fund = %body.fund_id, actor = %auth.user_id
        );
    }

    let detail = aid_repo::get_disbursement(pool, outcome.event_id)
        .await
        .map_err(db_err("social aid disbursement query failed"))?
        .ok_or(ApiError::Internal)?;
    let status = if outcome.replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(disbursement_dto(&detail))).into_response())
}

pub async fn reverse_disbursement(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<ReverseRequest>,
) -> Result<Response, ApiError> {
    let pool = social_aid_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let reason = aid_model::validate_reversal_reason(&body.reason)
        .map_err(|_| ApiError::ValidationFailed)?;

    let (already, number) = aid_repo::reverse_disbursement(pool, auth.user_id, id, reason, now)
        .await
        .map_err(command_error)?;

    if !already {
        audit::record(
            pool,
            SecurityEventType::SocialAidDisbursementReversed,
            Some(auth.user_id),
            None,
            serde_json::json!({ "disbursementId": id, "disbursementNumber": number }),
        )
        .await;
    }

    let detail = aid_repo::get_disbursement(pool, id)
        .await
        .map_err(db_err("social aid disbursement query failed"))?
        .ok_or(ApiError::Internal)?;
    Ok((StatusCode::OK, Json(disbursement_dto(&detail))).into_response())
}
