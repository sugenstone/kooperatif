//! Share Return API (STEP-011).
//!
//! - `GET  /api/share-returns`                      (share_returns.read)
//! - `POST /api/share-returns`                      (share_returns.manage) initiate
//! - `GET  /api/share-returns/{id}`                 (share_returns.read)
//! - `POST /api/share-returns/{id}/finalize`        (share_returns.manage)
//! - `POST /api/share-returns/{id}/cancel`          (share_returns.manage)
//! - `POST /api/share-returns/{id}/entitlements`    (share_returns.manage)
//! - `GET  /api/share-return-entitlements`          (share_returns.read)
//! - `POST /api/share-return-entitlements/{id}/determine` (manage)
//! - `POST /api/share-return-entitlements/{id}/cancel`    (manage)
//! - `POST /api/share-return-entitlements/{id}/settlements` (manage)
//! - `POST /api/share-return-settlements/{id}/reverse`    (manage)
//!
//! All reads/writes `no-store`; mutations require both CSRF layers and
//! enforce permissions server-side. Amounts are decimal STRINGS
//! (ADR-004); dates are `YYYY-MM-DD` business dates — effective return
//! date and due dates are DATE semantics, never instants.
//!
//! Settlement is NOT Expense/Payment/Transfer — it posts exactly ONE
//! `share_return_settlement`-sourced outflow movement (docs/10).

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
use crate::parties::routes::PaginatedDto;
use crate::payments::model as payment_model;
use crate::share_returns::model as return_model;
use crate::share_returns::repo as return_repo;

pub fn share_returns_router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/share-returns",
            get(list_returns).post(initiate_return),
        )
        .route("/api/share-returns/{id}", get(get_return))
        .route("/api/share-returns/{id}/finalize", post(finalize_return))
        .route("/api/share-returns/{id}/cancel", post(cancel_return))
        .route(
            "/api/share-returns/{id}/entitlements",
            post(recognize_entitlement),
        )
        .route("/api/share-return-entitlements", get(list_entitlements))
        .route(
            "/api/share-return-entitlements/{id}/determine",
            post(determine_entitlement),
        )
        .route(
            "/api/share-return-entitlements/{id}/cancel",
            post(cancel_entitlement),
        )
        .route(
            "/api/share-return-entitlements/{id}/settlements",
            post(post_settlement),
        )
        .route(
            "/api/share-return-settlements/{id}/reverse",
            post(reverse_settlement),
        )
        .layer(axum::middleware::from_fn(
            crate::auth::routes::no_store_cache_control,
        ))
}

fn pool_of(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state.db.as_ref().ok_or(ApiError::DependencyUnavailable)
}

/// Share-return mutations share this prologue (CSRF layers +
/// permission).
macro_rules! returns_mutation {
    ($state:expr, $headers:expr, $auth:expr, $method:expr) => {{
        csrf::validate_origin($headers, $method, &$state.auth.config.allowed_origins)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        csrf::validate_csrf_token($headers, &$auth.csrf_token)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        authz::require($state, $auth, authz::catalog::SHARE_RETURNS_MANAGE).await?;
        pool_of($state)?
    }};
}

fn command_error(error: return_repo::ShareReturnCommandError) -> ApiError {
    use return_repo::ShareReturnCommandError as E;
    match error {
        E::NotFound => ApiError::NotFound,
        E::StaleState => ApiError::StaleState,
        E::ShareNotEligible | E::InvalidEffectiveDate | E::InvalidState | E::NotSettleable => {
            ApiError::ValidationFailed
        }
        E::ActiveReturnExists
        | E::EntitlementExists
        | E::OverSettlement
        | E::HasSettlements
        | E::InactiveAccount
        | E::InsufficientFunds
        | E::IdempotencyConflict => ApiError::Conflict,
        E::InsufficientUnrestrictedFunds => ApiError::InsufficientUnrestrictedFunds,
        E::AccountInvariant(inner) => {
            tracing::error!(error = ?inner, "unexpected account-layer error");
            ApiError::Internal
        }
        E::Database(error) => {
            tracing::error!(error = %error, "share return command failed");
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

// ------------------------------------------------------------------
// DTOs (mirror @kooperatif/contracts/src/share_returns.ts)
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
pub struct ShareReturnListItemDto {
    pub id: Uuid,
    pub return_number: i64,
    pub share_id: Uuid,
    pub share_number: i64,
    pub shareholder_id: Uuid,
    pub owner_display_name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub requested_at: OffsetDateTime,
    #[serde(with = "date_iso")]
    pub effective_return_date: Date,
    pub status: String,
    pub entitlement_count: i64,
    /// Sum of determined non-cancelled entitlement amounts minus posted
    /// settlements; NULL when no determined entitlement exists.
    pub outstanding_amount: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementDto {
    pub id: Uuid,
    pub entitlement_number: i64,
    pub share_return_id: Uuid,
    pub return_number: i64,
    pub share_id: Uuid,
    pub share_number: i64,
    pub entitlement_type: String,
    pub beneficiary_shareholder_id: Uuid,
    pub beneficiary_display_name: String,
    /// Crystallized amount — NULL = right exists, not yet determined.
    pub amount: Option<String>,
    pub currency: String,
    #[serde(with = "date_iso::option")]
    pub due_date: Option<Date>,
    pub policy_reference: Option<String>,
    pub description: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub recognized_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub determined_at: Option<OffsetDateTime>,
    pub status: String,
    pub settled_amount: String,
    /// Derived due state (never stored): undetermined | not_due | due |
    /// overdue | settled | cancelled.
    pub due_state: String,
    pub remaining_amount: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettlementDto {
    pub id: Uuid,
    pub settlement_number: i64,
    pub entitlement_id: Uuid,
    pub entitlement_type: String,
    pub share_return_id: Uuid,
    pub return_number: i64,
    pub financial_account_id: Uuid,
    pub account_name: String,
    pub amount: String,
    pub currency: String,
    #[serde(with = "time::serde::rfc3339")]
    pub settled_at: OffsetDateTime,
    pub account_movement_id: Uuid,
    pub movement_status: String,
    pub status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareReturnDetailDto {
    pub id: Uuid,
    pub return_number: i64,
    pub share_id: Uuid,
    pub share_number: i64,
    pub share_status: String,
    pub shareholder_id: Uuid,
    pub owner_display_name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub ownership_started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub requested_at: OffsetDateTime,
    #[serde(with = "date_iso")]
    pub effective_return_date: Date,
    pub reason: Option<String>,
    pub status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub finalized_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub cancelled_at: Option<OffsetDateTime>,
    pub cancellation_reason: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    pub entitlements: Vec<EntitlementDto>,
    pub settlements: Vec<SettlementDto>,
}

fn entitlement_dto(row: &return_repo::EntitlementRow) -> EntitlementDto {
    let remaining = row.amount.map(|amount| {
        let rest = amount - row.settled_amount;
        canonical(if rest < rust_decimal::Decimal::ZERO {
            rust_decimal::Decimal::ZERO
        } else {
            rest
        })
    });
    EntitlementDto {
        id: row.id,
        entitlement_number: row.entitlement_number,
        share_return_id: row.share_return_id,
        return_number: row.return_number,
        share_id: row.share_id,
        share_number: row.share_number,
        entitlement_type: row.entitlement_type.clone(),
        beneficiary_shareholder_id: row.beneficiary_shareholder_id,
        beneficiary_display_name: row.beneficiary_display_name.clone(),
        amount: canonical_opt(row.amount),
        currency: row.currency.clone(),
        due_date: row.due_date,
        policy_reference: row.policy_reference.clone(),
        description: row.description.clone(),
        recognized_at: row.recognized_at,
        determined_at: row.determined_at,
        status: row.status.clone(),
        settled_amount: canonical(row.settled_amount),
        due_state: row.due_state.clone(),
        remaining_amount: remaining,
        updated_at: row.updated_at,
    }
}

fn settlement_dto(row: &return_repo::SettlementRow) -> SettlementDto {
    SettlementDto {
        id: row.id,
        settlement_number: row.settlement_number,
        entitlement_id: row.entitlement_id,
        entitlement_type: row.entitlement_type.clone(),
        share_return_id: row.share_return_id,
        return_number: row.return_number,
        financial_account_id: row.financial_account_id,
        account_name: row.account_name.clone(),
        amount: canonical(row.amount),
        currency: row.currency.clone(),
        settled_at: row.settled_at,
        account_movement_id: row.account_movement_id,
        movement_status: row.movement_status.clone(),
        status: row.status.clone(),
        reversed_at: row.reversed_at,
        reversal_reason: row.reversal_reason.clone(),
    }
}

async fn load_detail(
    pool: &sqlx::PgPool,
    id: Uuid,
) -> Result<Option<ShareReturnDetailDto>, ApiError> {
    let Some(row) = return_repo::get_return(pool, id)
        .await
        .map_err(db_err("get return failed"))?
    else {
        return Ok(None);
    };
    let entitlements = return_repo::list_entitlements_of_return(pool, id)
        .await
        .map_err(db_err("return entitlements failed"))?
        .iter()
        .map(entitlement_dto)
        .collect();
    let settlements = return_repo::list_settlements_of_return(pool, id)
        .await
        .map_err(db_err("return settlements failed"))?
        .iter()
        .map(settlement_dto)
        .collect();
    Ok(Some(ShareReturnDetailDto {
        id: row.id,
        return_number: row.return_number,
        share_id: row.share_id,
        share_number: row.share_number,
        share_status: row.share_status,
        shareholder_id: row.shareholder_id,
        owner_display_name: row.owner_display_name,
        ownership_started_at: row.ownership_started_at,
        requested_at: row.requested_at,
        effective_return_date: row.effective_return_date,
        reason: row.reason,
        status: row.status,
        finalized_at: row.finalized_at,
        cancelled_at: row.cancelled_at,
        cancellation_reason: row.cancellation_reason,
        updated_at: row.updated_at,
        entitlements,
        settlements,
    }))
}

// ------------------------------------------------------------------
// Handlers
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReturnListQuery {
    pub status: Option<String>,
    pub share_id: Option<Uuid>,
    pub shareholder_id: Option<Uuid>,
    pub search: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

pub async fn list_returns(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<ReturnListQuery>,
) -> Result<Json<PaginatedDto<ShareReturnListItemDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::SHARE_RETURNS_READ).await?;
    let pool = pool_of(&state)?;
    let status = query
        .status
        .as_deref()
        .map(|s| return_model::ReturnStatus::parse(s).ok_or(ApiError::ValidationFailed))
        .transpose()?;
    let (page, page_size) = crate::parties::routes::page_of(&crate::parties::routes::ListQuery {
        search: None,
        page: query.page,
        page_size: query.page_size,
        default_account_id: None,
    })?;
    let rows = return_repo::list_returns(
        pool,
        status,
        query.share_id,
        query.shareholder_id,
        query.search.as_deref(),
        page,
        page_size,
    )
    .await
    .map_err(db_err("return listing failed"))?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    Ok(Json(PaginatedDto {
        items: rows
            .iter()
            .map(|r| ShareReturnListItemDto {
                id: r.id,
                return_number: r.return_number,
                share_id: r.share_id,
                share_number: r.share_number,
                shareholder_id: r.shareholder_id,
                owner_display_name: r.owner_display_name.clone(),
                requested_at: r.requested_at,
                effective_return_date: r.effective_return_date,
                status: r.status.clone(),
                entitlement_count: r.entitlement_count,
                outstanding_amount: canonical_opt(r.outstanding_amount),
            })
            .collect(),
        page,
        page_size,
        total_count,
    }))
}

pub async fn get_return(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::SHARE_RETURNS_READ).await?;
    let pool = pool_of(&state)?;
    load_detail(pool, id)
        .await?
        .map(Json)
        .map(IntoResponse::into_response)
        .ok_or(ApiError::NotFound)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct InitiateReturnRequest {
    pub share_id: Uuid,
    /// Business DATE `YYYY-MM-DD` — the economic cutoff; ownership ends
    /// at the start of this day (Europe/Istanbul). Not in the future.
    pub effective_return_date: String,
    pub reason: Option<String>,
    pub idempotency_key: String,
}

fn parse_business_date(raw: &str) -> Result<Date, ApiError> {
    Date::parse(
        raw.trim(),
        &time::macros::format_description!("[year]-[month]-[day]"),
    )
    .map_err(|_| ApiError::ValidationFailed)
}

pub async fn initiate_return(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Json(body): Json<InitiateReturnRequest>,
) -> Result<Response, ApiError> {
    let pool = returns_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let effective_return_date = parse_business_date(&body.effective_return_date)?;
    let reason = return_model::validate_reason(body.reason.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let idempotency_key = return_model::validate_idempotency_key(&body.idempotency_key)
        .map_err(|_| ApiError::ValidationFailed)?;
    let fingerprint =
        return_model::initiate_fingerprint(body.share_id, effective_return_date, &reason);

    let outcome = return_repo::initiate_return(
        pool,
        auth.user_id,
        return_repo::InitiateReturn {
            share_id: body.share_id,
            effective_return_date,
            reason,
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
            SecurityEventType::ShareReturnRequested,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "returnId": outcome.return_id,
                "returnNumber": outcome.return_number,
                "shareId": body.share_id,
                "effectiveReturnDate": effective_return_date,
            }),
        )
        .await;
        tracing::info!(
            outcome = "share_return_initiated", id = %outcome.return_id,
            number = outcome.return_number, actor = %auth.user_id
        );
    }

    let detail = load_detail(pool, outcome.return_id)
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
pub struct EntitlementSpecRequest {
    pub entitlement_type: String,
    /// Crystallized amount — decimal string; NULL = right exists but
    /// not yet determined (docs/17: formulas unresolved, operator
    /// supplies the cooperative-approved amount).
    pub amount: Option<String>,
    pub due_date: Option<String>,
    pub policy_reference: Option<String>,
    pub description: Option<String>,
}

fn parse_entitlement_spec(
    body: &EntitlementSpecRequest,
) -> Result<return_repo::EntitlementSpec, ApiError> {
    let entitlement_type = return_model::EntitlementType::parse(&body.entitlement_type)
        .ok_or(ApiError::ValidationFailed)?;
    let amount = return_model::validate_entitlement_amount(body.amount.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let due_date = body
        .due_date
        .as_deref()
        .map(parse_business_date)
        .transpose()?;
    let policy_reference =
        return_model::validate_policy_reference(body.policy_reference.as_deref())
            .map_err(|_| ApiError::ValidationFailed)?;
    let description = return_model::validate_description(body.description.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    Ok(return_repo::EntitlementSpec {
        entitlement_type,
        amount,
        due_date,
        policy_reference,
        description,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct FinalizeReturnRequest {
    /// Rights to crystallize — at most one per type; each may carry a
    /// NULL amount (undetermined right).
    pub entitlements: Vec<EntitlementSpecRequest>,
    #[serde(with = "time::serde::rfc3339")]
    pub expected_updated_at: OffsetDateTime,
}

pub async fn finalize_return(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<FinalizeReturnRequest>,
) -> Result<Response, ApiError> {
    let pool = returns_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let specs: Vec<return_repo::EntitlementSpec> = body
        .entitlements
        .iter()
        .map(parse_entitlement_spec)
        .collect::<Result<_, _>>()?;

    let outcome = return_repo::finalize_return(
        pool,
        auth.user_id,
        id,
        body.expected_updated_at,
        &specs,
        now,
    )
    .await
    .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::ShareReturnFinalized,
        Some(auth.user_id),
        None,
        serde_json::json!({
            "returnId": id,
            "returnNumber": outcome.return_number,
            "entitlementIds": outcome.entitlement_ids,
        }),
    )
    .await;
    tracing::info!(
        outcome = "share_return_finalized", id = %id,
        number = outcome.return_number, actor = %auth.user_id
    );

    let detail = load_detail(pool, id).await?.ok_or(ApiError::Internal)?;
    Ok((StatusCode::OK, Json(detail)).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct CancelReturnRequest {
    /// Required non-empty reason (docs/19).
    pub reason: String,
}

pub async fn cancel_return(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<CancelReturnRequest>,
) -> Result<Response, ApiError> {
    let pool = returns_mutation!(&state, &headers, &auth, &Method::POST);
    let reason = return_model::validate_required_reason(&body.reason)
        .map_err(|_| ApiError::ValidationFailed)?;
    let now = state.auth.clock.now();

    let (_replayed, return_number) =
        return_repo::cancel_return(pool, auth.user_id, id, reason.clone(), now)
            .await
            .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::ShareReturnCancelled,
        Some(auth.user_id),
        None,
        serde_json::json!({
            "returnId": id,
            "returnNumber": return_number,
            "reason": reason,
        }),
    )
    .await;
    tracing::info!(
        outcome = "share_return_cancelled", id = %id,
        number = return_number, actor = %auth.user_id
    );

    let detail = load_detail(pool, id).await?.ok_or(ApiError::Internal)?;
    Ok((StatusCode::OK, Json(detail)).into_response())
}

/// Recognize an additional right on a FINALIZED return — e.g. the
/// Profit Right crystallizes years later (docs/10).
pub async fn recognize_entitlement(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<EntitlementSpecRequest>,
) -> Result<Response, ApiError> {
    let pool = returns_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let spec = parse_entitlement_spec(&body)?;

    let entitlement_id = return_repo::recognize_entitlement(pool, auth.user_id, id, spec, now)
        .await
        .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::ShareReturnEntitlementRecognized,
        Some(auth.user_id),
        None,
        serde_json::json!({
            "returnId": id,
            "entitlementId": entitlement_id,
            "entitlementType": body.entitlement_type,
        }),
    )
    .await;

    let row = return_repo::get_entitlement(pool, entitlement_id)
        .await
        .map_err(db_err("get entitlement failed"))?
        .ok_or(ApiError::Internal)?;
    Ok((StatusCode::CREATED, Json(entitlement_dto(&row))).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementListQuery {
    pub entitlement_type: Option<String>,
    pub status: Option<String>,
    pub beneficiary_shareholder_id: Option<Uuid>,
    pub share_return_id: Option<Uuid>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

pub async fn list_entitlements(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<EntitlementListQuery>,
) -> Result<Json<PaginatedDto<EntitlementDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::SHARE_RETURNS_READ).await?;
    let pool = pool_of(&state)?;
    let entitlement_type = query
        .entitlement_type
        .as_deref()
        .map(|t| return_model::EntitlementType::parse(t).ok_or(ApiError::ValidationFailed))
        .transpose()?;
    let status = query
        .status
        .as_deref()
        .map(|s| return_model::EntitlementStatus::parse(s).ok_or(ApiError::ValidationFailed))
        .transpose()?;
    let (page, page_size) = crate::parties::routes::page_of(&crate::parties::routes::ListQuery {
        search: None,
        page: query.page,
        page_size: query.page_size,
        default_account_id: None,
    })?;
    let rows = return_repo::list_entitlements(
        pool,
        entitlement_type,
        status,
        query.beneficiary_shareholder_id,
        query.share_return_id,
        page,
        page_size,
    )
    .await
    .map_err(db_err("entitlement listing failed"))?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    Ok(Json(PaginatedDto {
        items: rows.iter().map(entitlement_dto).collect(),
        page,
        page_size,
        total_count,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct DetermineEntitlementRequest {
    /// The cooperative-approved amount — REQUIRED here (a determined
    /// right must have a number; use cancel for a mistaken row).
    pub amount: String,
    pub due_date: Option<String>,
    pub policy_reference: Option<String>,
    pub description: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub expected_updated_at: OffsetDateTime,
}

pub async fn determine_entitlement(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<DetermineEntitlementRequest>,
) -> Result<Response, ApiError> {
    let pool = returns_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let amount = return_model::validate_entitlement_amount(Some(&body.amount))
        .map_err(|_| ApiError::ValidationFailed)?
        .ok_or(ApiError::ValidationFailed)?;
    let due_date = body
        .due_date
        .as_deref()
        .map(parse_business_date)
        .transpose()?;
    let policy_reference =
        return_model::validate_policy_reference(body.policy_reference.as_deref())
            .map_err(|_| ApiError::ValidationFailed)?;
    let description = return_model::validate_description(body.description.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;

    let number = return_repo::determine_entitlement(
        pool,
        auth.user_id,
        id,
        amount,
        due_date,
        policy_reference,
        description,
        body.expected_updated_at,
        now,
    )
    .await
    .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::ShareReturnEntitlementDetermined,
        Some(auth.user_id),
        None,
        serde_json::json!({
            "entitlementId": id,
            "entitlementNumber": number,
            "amount": canonical(amount),
        }),
    )
    .await;

    let row = return_repo::get_entitlement(pool, id)
        .await
        .map_err(db_err("get entitlement failed"))?
        .ok_or(ApiError::Internal)?;
    Ok((StatusCode::OK, Json(entitlement_dto(&row))).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct CancelEntitlementRequest {
    /// Required non-empty reason (docs/19).
    pub reason: String,
}

pub async fn cancel_entitlement(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<CancelEntitlementRequest>,
) -> Result<Response, ApiError> {
    let pool = returns_mutation!(&state, &headers, &auth, &Method::POST);
    let reason = return_model::validate_required_reason(&body.reason)
        .map_err(|_| ApiError::ValidationFailed)?;
    let now = state.auth.clock.now();

    let number = return_repo::cancel_entitlement(pool, auth.user_id, id, reason.clone(), now)
        .await
        .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::ShareReturnEntitlementCancelled,
        Some(auth.user_id),
        None,
        serde_json::json!({
            "entitlementId": id,
            "entitlementNumber": number,
            "reason": reason,
        }),
    )
    .await;

    let row = return_repo::get_entitlement(pool, id)
        .await
        .map_err(db_err("get entitlement failed"))?
        .ok_or(ApiError::Internal)?;
    Ok((StatusCode::OK, Json(entitlement_dto(&row))).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct PostSettlementRequest {
    pub financial_account_id: Uuid,
    /// Positive decimal string; <= entitlement remaining.
    pub amount: String,
    /// Optional RFC3339 — defaults to server now; backdating allowed.
    pub settled_at: Option<String>,
    pub idempotency_key: String,
}

/// THE settlement command: one outflow movement + one settlement row,
/// atomically. Replay → 200 with the existing settlement.
pub async fn post_settlement(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<PostSettlementRequest>,
) -> Result<Response, ApiError> {
    let pool = returns_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let amount = return_model::validate_settlement_amount(&body.amount)
        .map_err(|_| ApiError::ValidationFailed)?;
    let settled_at = match &body.settled_at {
        Some(raw) => {
            OffsetDateTime::parse(raw.trim(), &Rfc3339).map_err(|_| ApiError::ValidationFailed)?
        }
        None => now,
    };
    if settled_at > now {
        return Err(ApiError::ValidationFailed);
    }
    let settled_at_intent = body.settled_at.is_some().then_some(settled_at);
    let idempotency_key = return_model::validate_idempotency_key(&body.idempotency_key)
        .map_err(|_| ApiError::ValidationFailed)?;
    let fingerprint = return_model::settlement_fingerprint(
        id,
        body.financial_account_id,
        amount,
        settled_at_intent,
    );

    let outcome = return_repo::post_settlement(
        pool,
        auth.user_id,
        return_repo::PostSettlement {
            entitlement_id: id,
            financial_account_id: body.financial_account_id,
            amount,
            settled_at,
            idempotency_key,
            fingerprint,
        },
    )
    .await
    .map_err(command_error)?;

    if !outcome.replayed {
        audit::record(
            pool,
            SecurityEventType::ShareReturnSettlementPosted,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "settlementId": outcome.settlement_id,
                "settlementNumber": outcome.settlement_number,
                "entitlementId": id,
                "financialAccountId": body.financial_account_id,
                "amount": canonical(amount),
            }),
        )
        .await;
        tracing::info!(
            outcome = "share_return_settled", id = %outcome.settlement_id,
            number = outcome.settlement_number, actor = %auth.user_id
        );
    }

    let row = return_repo::get_settlement(pool, outcome.settlement_id)
        .await
        .map_err(db_err("get settlement failed"))?
        .ok_or(ApiError::Internal)?;
    let status = if outcome.replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(settlement_dto(&row))).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct ReverseSettlementRequest {
    /// Required non-empty reason (docs/19 reversal contract).
    pub reason: String,
}

pub async fn reverse_settlement(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<ReverseSettlementRequest>,
) -> Result<Response, ApiError> {
    let pool = returns_mutation!(&state, &headers, &auth, &Method::POST);
    let reason = return_model::validate_required_reason(&body.reason)
        .map_err(|_| ApiError::ValidationFailed)?;
    let now = state.auth.clock.now();

    let (replayed, number) =
        return_repo::reverse_settlement(pool, auth.user_id, id, reason.clone(), now)
            .await
            .map_err(command_error)?;

    if !replayed {
        audit::record(
            pool,
            SecurityEventType::ShareReturnSettlementReversed,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "settlementId": id,
                "settlementNumber": number,
                "reason": reason,
            }),
        )
        .await;
    }

    let row = return_repo::get_settlement(pool, id)
        .await
        .map_err(db_err("get settlement failed"))?
        .ok_or(ApiError::Internal)?;
    Ok((StatusCode::OK, Json(settlement_dto(&row))).into_response())
}
