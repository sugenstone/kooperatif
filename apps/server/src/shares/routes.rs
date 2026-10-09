//! Shares API (STEP-005 §60–§66).
//!
//! - `GET  /api/shares`                   (shares.read)   search+pagination
//! - `POST /api/shares`                   (shares.manage) create + initial allocation, one tx
//! - `GET  /api/shares/{id}`              (shares.read)   detail + event history
//! - `POST /api/shares/{id}/transfer`     (shares.manage)
//! - `POST /api/shares/{id}/sale`         (shares.manage)
//! - `POST /api/shares/{id}/status-change`(shares.manage) suspend/reactivate/void
//! - `GET  /api/shareholders/{id}/shares` (shares.read)   currently held shares
//!
//! All reads/writes `no-store`; mutations require both CSRF layers and
//! enforce permissions server-side. Ownership mutations carry an
//! `expectedUpdatedAt` optimistic-concurrency precondition.
//! `acquisitionFee`/`saleAmount` are decimal STRINGS (ADR-004: no lossy
//! floats in contracts) — NULL ≠ 0 semantics preserved end to end.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::audit::{self, SecurityEventType};
use crate::auth::authz;
use crate::auth::csrf;
use crate::auth::extractor::CurrentAuth;
use crate::auth::routes::csrf_rejection_to_api_error;
use crate::http::error::ApiError;
use crate::http::AppState;
use crate::parties::routes::{ListQuery, PaginatedDto};
use crate::shares::model as share_model;
use crate::shares::repo as share_repo;
use crate::shares::repo::ShareEventRow;

const EVENT_HISTORY_LIMIT: i64 = 200;

// ------------------------------------------------------------------
// DTOs (mirror @kooperatif/contracts/src/shares.ts)
// ------------------------------------------------------------------

/// Canonical Shareholder identity (§7): the reusable structure every
/// Share surface embeds. `displayLabel` is composed server-side and
/// always carries `Vasi:` context (Belirtilmemiş when absent).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareholderIdentityDto {
    pub shareholder_id: Uuid,
    pub full_name: String,
    pub guardian_name: Option<String>,
    pub family_sequence_number: Option<i64>,
    pub status: String,
    pub display_label: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareListItemDto {
    pub id: Uuid,
    pub share_number: i64,
    pub status: String,
    pub owner: Option<ShareholderIdentityDto>,
    /// How the CURRENT ownership interval began (founder /
    /// later_acquisition / transfer / sale); NULL if ownerless.
    pub acquisition_type: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub ownership_started_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareEventDto {
    pub id: Uuid,
    pub event_type: String,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    pub from: Option<ShareholderIdentityDto>,
    pub to: Option<ShareholderIdentityDto>,
    /// Initial-allocation classification (initial_acquisition only).
    pub acquisition_type: Option<String>,
    /// Agreed business value (fee / sale price) as a decimal string.
    /// NULL = not specified; "0.00" = explicitly zero (§17).
    pub amount: Option<String>,
    pub currency: String,
    pub status_from: Option<String>,
    pub status_to: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareDetailDto {
    #[serde(flatten)]
    pub item: ShareListItemDto,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub events: Vec<ShareEventDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct CreateShareRequest {
    pub shareholder_id: Uuid,
    pub acquisition_type: String, // "founder" | "later_acquisition"
    pub acquisition_fee: Option<String>,
    pub effective_at: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct OwnershipChangeRequest {
    pub to_shareholder_id: Uuid,
    pub effective_at: Option<String>,
    pub reason: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub expected_updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct SaleRequest {
    pub to_shareholder_id: Uuid,
    pub sale_amount: Option<String>,
    pub effective_at: Option<String>,
    pub reason: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub expected_updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct ShareStatusChangeRequest {
    pub to: String,
    pub reason: Option<String>,
}

pub fn shares_router() -> Router<AppState> {
    Router::new()
        .route("/api/shares", get(list_shares).post(create_share))
        .route("/api/shares/{id}", get(get_share))
        .route("/api/shares/{id}/transfer", post(transfer_share))
        .route("/api/shares/{id}/sale", post(sell_share))
        .route("/api/shares/{id}/status-change", post(change_share_status))
        .route("/api/shareholders/{id}/shares", get(shareholder_shares))
        .layer(axum::middleware::from_fn(
            crate::auth::routes::no_store_cache_control,
        ))
}

fn pool_of(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state.db.as_ref().ok_or(ApiError::DependencyUnavailable)
}

/// Share mutations share this prologue (CSRF layers + permission).
macro_rules! shares_mutation {
    ($state:expr, $headers:expr, $auth:expr, $method:expr) => {{
        csrf::validate_origin($headers, $method, &$state.auth.config.allowed_origins)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        csrf::validate_csrf_token($headers, &$auth.csrf_token)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        authz::require($state, $auth, authz::catalog::SHARES_MANAGE).await?;
        pool_of($state)?
    }};
}

/// Compose the canonical Shareholder identity DTO from joined columns.
/// Shared with the periods/assessments domain (STEP-006) so every
/// obligation surface renders the same `Vasi:`/`Aile No` context.
pub fn identity_dto(
    shareholder_id: Option<Uuid>,
    first: &Option<String>,
    last: &Option<String>,
    guardian_first: &Option<String>,
    guardian_last: &Option<String>,
    family_sequence: Option<i64>,
    status: Option<&str>,
) -> Option<ShareholderIdentityDto> {
    let shareholder_id = shareholder_id?;
    let full_name = format!(
        "{} {}",
        first.clone().unwrap_or_default(),
        last.clone().unwrap_or_default()
    );
    let guardian_name = match (guardian_first, guardian_last) {
        (Some(gf), Some(gl)) => Some(format!("{gf} {gl}")),
        _ => None,
    };
    let mut label = full_name.clone();
    label.push_str(&match &guardian_name {
        Some(name) => format!(" · Vasi: {name}"),
        None => " · Vasi: Belirtilmemiş".to_string(),
    });
    if let Some(sequence) = family_sequence {
        label.push_str(&format!(" · Aile No {sequence}"));
    }
    Some(ShareholderIdentityDto {
        shareholder_id,
        full_name,
        guardian_name,
        family_sequence_number: family_sequence,
        status: status.unwrap_or("active").to_string(),
        display_label: label,
    })
}

fn list_item_dto(row: &share_repo::ShareListRow) -> ShareListItemDto {
    ShareListItemDto {
        id: row.id,
        share_number: row.share_number,
        status: row.status.clone(),
        owner: identity_dto(
            row.owner_shareholder_id,
            &row.owner_first_name,
            &row.owner_last_name,
            &row.owner_guardian_first_name,
            &row.owner_guardian_last_name,
            row.owner_family_sequence,
            row.owner_shareholder_status.as_deref(),
        ),
        acquisition_type: row.acquisition_type.clone(),
        ownership_started_at: row.ownership_started_at,
        updated_at: row.updated_at,
    }
}

// ------------------------------------------------------------------
// Handlers
// ------------------------------------------------------------------

pub async fn list_shares(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<ListQuery>,
) -> Result<Json<PaginatedDto<ShareListItemDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::SHARES_READ).await?;
    let pool = pool_of(&state)?;
    let (page, page_size) = crate::parties::routes::page_of(&query)?;
    let rows = share_repo::list_shares(pool, query.search.as_deref(), page, page_size)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "share listing failed");
            ApiError::Internal
        })?;
    let total_count = rows.first().map(|row| row.total_count).unwrap_or(0);
    Ok(Json(PaginatedDto {
        items: rows.iter().map(list_item_dto).collect(),
        page,
        page_size,
        total_count,
    }))
}

pub async fn get_share(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<ShareDetailDto>, ApiError> {
    authz::require(&state, &auth, authz::catalog::SHARES_READ).await?;
    let pool = pool_of(&state)?;
    load_detail(pool, id)
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

async fn load_detail(pool: &sqlx::PgPool, id: Uuid) -> Result<Option<ShareDetailDto>, ApiError> {
    let Some(row) = share_repo::find_share(pool, id).await.map_err(|error| {
        tracing::error!(error = %error, "share load failed");
        ApiError::Internal
    })?
    else {
        return Ok(None);
    };
    let events = share_repo::share_event_history(pool, id, EVENT_HISTORY_LIMIT)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "share history failed");
            ApiError::Internal
        })?
        .into_iter()
        .map(event_dto)
        .collect();
    Ok(Some(ShareDetailDto {
        item: list_item_dto(&row),
        created_at: row.created_at,
        events,
    }))
}

fn event_dto(row: ShareEventRow) -> ShareEventDto {
    ShareEventDto {
        id: row.id,
        event_type: row.event_type,
        occurred_at: row.occurred_at,
        from: identity_dto(
            row.from_shareholder_id,
            &row.from_first_name,
            &row.from_last_name,
            &row.from_guardian_first_name,
            &row.from_guardian_last_name,
            row.from_family_sequence,
            Some("active"),
        ),
        to: identity_dto(
            row.to_shareholder_id,
            &row.to_first_name,
            &row.to_last_name,
            &row.to_guardian_first_name,
            &row.to_guardian_last_name,
            row.to_family_sequence,
            Some("active"),
        ),
        acquisition_type: row.acquisition_type,
        amount: row.amount.map(share_model::canonical_amount),
        currency: row.currency,
        status_from: row.status_from,
        status_to: row.status_to,
        reason: row.reason,
    }
}

fn parse_effective_at(raw: Option<&str>) -> Result<Option<OffsetDateTime>, ApiError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(text) => OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
            .map(Some)
            .map_err(|_| ApiError::ValidationFailed),
    }
}

fn parse_amount(raw: Option<&String>) -> Result<Option<Decimal>, ApiError> {
    match raw {
        None => Ok(None),
        Some(text) => share_model::validate_amount(text)
            .map(Some)
            .map_err(|_| ApiError::ValidationFailed),
    }
}

pub async fn create_share(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Json(body): Json<CreateShareRequest>,
) -> Result<Response, ApiError> {
    let pool = shares_mutation!(&state, &headers, &auth, &Method::POST);

    let acquisition_type = share_model::AcquisitionType::parse_initial(&body.acquisition_type)
        .ok_or(ApiError::ValidationFailed)?;
    let fee = parse_amount(body.acquisition_fee.as_ref())?;
    let reason = share_model::validate_reason(body.reason.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let effective_at =
        parse_effective_at(body.effective_at.as_deref())?.unwrap_or_else(|| state.auth.clock.now());
    let now = state.auth.clock.now();

    let (share_id, share_number) = share_repo::create_share(
        pool,
        auth.user_id,
        share_repo::CreateShare {
            shareholder_id: body.shareholder_id,
            acquisition_type,
            fee,
            effective_at,
            reason,
        },
        now,
    )
    .await
    .map_err(|error| match error {
        share_repo::CreateShareError::ShareholderNotFound => ApiError::NotFound,
        share_repo::CreateShareError::ShareholderNotActive
        | share_repo::CreateShareError::InvalidAcquisitionType
        | share_repo::CreateShareError::FutureEffectiveAt => ApiError::ValidationFailed,
        share_repo::CreateShareError::Database(error) => {
            tracing::error!(error = %error, "share creation failed");
            ApiError::Internal
        }
    })?;

    // Audit: share created + initial ownership allocated + fee
    // condition — one durable event (§52).
    audit::record(
        pool,
        SecurityEventType::ShareCreated,
        Some(auth.user_id),
        None,
        serde_json::json!({
            "share_id": share_id,
            "share_number": share_number,
            "to_shareholder_id": body.shareholder_id,
            "acquisition_type": acquisition_type.as_str(),
            "acquisition_fee": fee.map(share_model::canonical_amount),
            "effective_at": effective_at,
        }),
    )
    .await;
    tracing::info!(outcome = "share_created", id = %share_id, number = share_number, actor = %auth.user_id);

    let detail = load_detail(pool, share_id)
        .await?
        .ok_or(ApiError::Internal)?;
    Ok((StatusCode::CREATED, Json(detail)).into_response())
}

/// Parsed owner-change inputs (transfer and sale converge here).
struct OwnerChange {
    share_id: Uuid,
    to_shareholder_id: Uuid,
    kind: share_model::AcquisitionType,
    amount: Option<Decimal>,
    effective_at: Option<OffsetDateTime>,
    reason: Option<String>,
    expected_updated_at: OffsetDateTime,
}

async fn change_owner_impl(
    state: &AppState,
    headers: &HeaderMap,
    auth: &CurrentAuth,
    change: OwnerChange,
) -> Result<Response, ApiError> {
    let pool = shares_mutation!(state, headers, auth, &Method::POST);
    let reason = share_model::validate_reason(change.reason.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let now = state.auth.clock.now();
    let effective_at = change.effective_at.unwrap_or(now);
    let amount = change.amount;

    let (from_id, share_number) = share_repo::change_owner(
        pool,
        auth.user_id,
        share_repo::ChangeOwner {
            share_id: change.share_id,
            to_shareholder_id: change.to_shareholder_id,
            kind: change.kind,
            amount,
            reason: reason.clone(),
            effective_at,
            expected_updated_at: change.expected_updated_at,
            now,
        },
    )
    .await
    .map_err(|error| match error {
        share_repo::OwnershipChangeError::ShareNotFound
        | share_repo::OwnershipChangeError::ShareholderNotFound => ApiError::NotFound,
        share_repo::OwnershipChangeError::StaleState => ApiError::StaleState,
        share_repo::OwnershipChangeError::ShareNotActive
        | share_repo::OwnershipChangeError::NoCurrentOwner
        | share_repo::OwnershipChangeError::SameOwner
        | share_repo::OwnershipChangeError::ShareholderNotActive
        | share_repo::OwnershipChangeError::InvalidEffectiveAt
        | share_repo::OwnershipChangeError::FutureEffectiveAt => ApiError::ValidationFailed,
        share_repo::OwnershipChangeError::Database(error) => {
            tracing::error!(error = %error, "ownership change failed");
            ApiError::Internal
        }
    })?;

    audit::record(
        pool,
        match change.kind {
            share_model::AcquisitionType::Transfer => SecurityEventType::ShareTransferred,
            _ => SecurityEventType::ShareSold,
        },
        Some(auth.user_id),
        None,
        serde_json::json!({
            "share_id": change.share_id,
            "share_number": share_number,
            "from_shareholder_id": from_id,
            "to_shareholder_id": change.to_shareholder_id,
            "amount": amount.map(share_model::canonical_amount),
            "reason": reason,
            "effective_at": effective_at,
        }),
    )
    .await;
    tracing::info!(
        outcome = "share_ownership_changed", id = %change.share_id, kind = event_type_str(change.kind),
        from = %from_id, to = %change.to_shareholder_id, actor = %auth.user_id
    );

    Ok(StatusCode::NO_CONTENT.into_response())
}

fn event_type_str(kind: share_model::AcquisitionType) -> &'static str {
    kind.as_str()
}

pub async fn transfer_share(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<OwnershipChangeRequest>,
) -> Result<Response, ApiError> {
    change_owner_impl(
        &state,
        &headers,
        &auth,
        OwnerChange {
            share_id: id,
            to_shareholder_id: body.to_shareholder_id,
            kind: share_model::AcquisitionType::Transfer,
            amount: None,
            effective_at: parse_effective_at(body.effective_at.as_deref())?,
            reason: body.reason,
            expected_updated_at: body.expected_updated_at,
        },
    )
    .await
}

pub async fn sell_share(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<SaleRequest>,
) -> Result<Response, ApiError> {
    change_owner_impl(
        &state,
        &headers,
        &auth,
        OwnerChange {
            share_id: id,
            to_shareholder_id: body.to_shareholder_id,
            kind: share_model::AcquisitionType::Sale,
            amount: parse_amount(body.sale_amount.as_ref())?,
            effective_at: parse_effective_at(body.effective_at.as_deref())?,
            reason: body.reason,
            expected_updated_at: body.expected_updated_at,
        },
    )
    .await
}

pub async fn change_share_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<ShareStatusChangeRequest>,
) -> Result<Response, ApiError> {
    let pool = shares_mutation!(&state, &headers, &auth, &Method::POST);
    let target = share_model::ShareStatus::parse(&body.to).ok_or(ApiError::ValidationFailed)?;
    let reason = share_model::validate_reason(body.reason.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let now = state.auth.clock.now();

    let share_number =
        share_repo::change_status(pool, auth.user_id, id, target, reason.clone(), now)
            .await
            .map_err(|error| match error {
                share_repo::StatusChangeError::ShareNotFound => ApiError::NotFound,
                share_repo::StatusChangeError::InvalidTransition
                | share_repo::StatusChangeError::HasHistory => ApiError::ValidationFailed,
                share_repo::StatusChangeError::Database(error) => {
                    tracing::error!(error = %error, "share status change failed");
                    ApiError::Internal
                }
            })?;

    audit::record(
        pool,
        SecurityEventType::ShareStatusChanged,
        Some(auth.user_id),
        None,
        serde_json::json!({
            "share_id": id,
            "share_number": share_number,
            "to": target.as_str(),
            "reason": reason,
        }),
    )
    .await;
    tracing::info!(outcome = "share_status_changed", id = %id, to = target.as_str(), actor = %auth.user_id);

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Current shares of one shareholder (§66): focused endpoint so the
/// Shareholder page renders its "Hisseler" section without N requests.
pub async fn shareholder_shares(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ShareListItemDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::SHARES_READ).await?;
    let pool = pool_of(&state)?;
    let rows = share_repo::list_shares_by_shareholder(pool, id)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "shareholder shares failed");
            ApiError::Internal
        })?;
    Ok(Json(rows.iter().map(list_item_dto).collect()))
}
