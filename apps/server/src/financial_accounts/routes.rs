//! Financial Accounts & Transfers API (STEP-008, docs/07).
//!
//! - `GET   /api/financial-accounts`                    (financial_accounts.read)
//! - `POST  /api/financial-accounts`                    (financial_accounts.manage)
//! - `GET   /api/financial-accounts/options`            (financial_accounts.read)
//! - `GET   /api/financial-accounts/summary`            (financial_accounts.read)
//! - `GET   /api/financial-accounts/{id}`               (financial_accounts.read)
//! - `PATCH /api/financial-accounts/{id}`               (financial_accounts.manage)
//! - `POST  /api/financial-accounts/{id}/status-change` (financial_accounts.manage)
//! - `GET   /api/financial-accounts/{id}/movements`     (financial_accounts.read)
//! - `GET   /api/account-transfers`                    (financial_accounts.read)
//! - `POST  /api/account-transfers`                     (financial_accounts.manage)
//! - `GET   /api/account-transfers/{id}`                (financial_accounts.read)
//! - `POST  /api/account-transfers/{id}/reverse`        (financial_accounts.manage)
//!
//! There is deliberately NO `POST /api/account-movements` — movements
//! are produced only by domain commands (Payment posting, Transfer).
//! Arbitrary operator movements would bypass domain controls.
//! Money fields are decimal STRINGS (ADR-004).

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

use crate::auth::audit::{self, SecurityEventType};
use crate::auth::authz;
use crate::auth::csrf;
use crate::auth::extractor::CurrentAuth;
use crate::auth::routes::csrf_rejection_to_api_error;
use crate::http::error::ApiError;
use crate::http::AppState;
use crate::parties::routes::{page_of, ListQuery, PaginatedDto};
use crate::payments::model as payment_model;

use super::model as account_model;
use super::repo as account_repo;
use account_model::{AccountStatus, AccountType, MovementDirection};
use account_repo::AccountCommandError;

pub fn financial_accounts_router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/financial-accounts",
            get(list_accounts).post(create_account),
        )
        .route("/api/financial-accounts/options", get(account_options))
        .route("/api/financial-accounts/summary", get(accounts_summary))
        .route(
            "/api/financial-accounts/{id}",
            get(get_account).patch(update_account),
        )
        .route(
            "/api/financial-accounts/{id}/status-change",
            post(change_status),
        )
        .route(
            "/api/financial-accounts/{id}/movements",
            get(list_movements),
        )
        .route(
            "/api/account-transfers",
            get(list_transfers).post(post_transfer),
        )
        .route("/api/account-transfers/{id}", get(get_transfer))
        .route(
            "/api/account-transfers/{id}/reverse",
            post(reverse_transfer),
        )
        .layer(axum::middleware::from_fn(
            crate::auth::routes::no_store_cache_control,
        ))
}

fn pool_of(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state.db.as_ref().ok_or(ApiError::DependencyUnavailable)
}

/// Account mutations share the full CSRF + permission prologue.
macro_rules! accounts_mutation {
    ($state:expr, $headers:expr, $auth:expr, $method:expr) => {{
        csrf::validate_origin($headers, $method, &$state.auth.config.allowed_origins)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        csrf::validate_csrf_token($headers, &$auth.csrf_token)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        authz::require($state, $auth, authz::catalog::FINANCIAL_ACCOUNTS_MANAGE).await?;
        pool_of($state)?
    }};
}

fn command_error(error: AccountCommandError) -> ApiError {
    match error {
        AccountCommandError::NotFound => ApiError::NotFound,
        // Deterministic request faults — same request always fails.
        AccountCommandError::InactiveAccount
        | AccountCommandError::SameAccount
        | AccountCommandError::CurrencyMismatch => ApiError::ValidationFailed,
        // State-dependent rejections (docs/21 conflict semantics).
        AccountCommandError::InsufficientFunds
        | AccountCommandError::IdempotencyConflict
        | AccountCommandError::NotPosted => ApiError::Conflict,
        AccountCommandError::StaleState => ApiError::StaleState,
        AccountCommandError::Database(error) => {
            tracing::error!(error = %error, "financial account command failed");
            ApiError::Internal
        }
    }
}

// ------------------------------------------------------------------
// DTOs
// ------------------------------------------------------------------

fn canonical(amount: Decimal) -> String {
    payment_model::canonical_amount(amount)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FinancialAccountDto {
    pub id: Uuid,
    pub name: String,
    pub account_type: String,
    pub currency: String,
    pub status: String,
    pub description: Option<String>,
    pub bank_name: Option<String>,
    pub iban: Option<String>,
    pub balance: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

fn account_dto(row: &account_repo::FinancialAccountRow) -> FinancialAccountDto {
    FinancialAccountDto {
        id: row.id,
        name: row.name.clone(),
        account_type: row.account_type.clone(),
        currency: row.currency.clone(),
        status: row.status.clone(),
        description: row.description.clone(),
        bank_name: row.bank_name.clone(),
        iban: row.iban.clone(),
        balance: canonical(row.balance),
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountOptionDto {
    pub id: Uuid,
    pub name: String,
    pub account_type: String,
    pub currency: String,
    pub balance: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountMovementDto {
    pub id: Uuid,
    pub direction: String,
    pub amount: String,
    /// Signed effect for display convenience — derived, never stored.
    pub effect: String,
    pub source_type: String,
    pub source_id: Uuid,
    pub source_number: Option<i64>,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    pub status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountTransferDto {
    pub id: Uuid,
    pub transfer_number: i64,
    pub source_account_id: Uuid,
    pub source_account_name: String,
    pub destination_account_id: Uuid,
    pub destination_account_name: String,
    pub amount: String,
    pub currency: String,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    pub note: Option<String>,
    pub status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

fn transfer_dto(row: &account_repo::AccountTransferRow) -> AccountTransferDto {
    AccountTransferDto {
        id: row.id,
        transfer_number: row.transfer_number,
        source_account_id: row.source_account_id,
        source_account_name: row.source_account_name.clone(),
        destination_account_id: row.destination_account_id,
        destination_account_name: row.destination_account_name.clone(),
        amount: canonical(row.amount),
        currency: row.currency.clone(),
        occurred_at: row.occurred_at,
        note: row.note.clone(),
        status: row.status.clone(),
        reversed_at: row.reversed_at,
        reversal_reason: row.reversal_reason.clone(),
        created_at: row.created_at,
    }
}

// ------------------------------------------------------------------
// Account reads
// ------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AccountListQuery {
    #[serde(flatten)]
    pub list: ListQuery,
    pub account_type: Option<String>,
    pub status: Option<String>,
}

pub async fn list_accounts(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<AccountListQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::FINANCIAL_ACCOUNTS_READ).await?;
    let pool = pool_of(&state)?;
    let (page, page_size) = page_of(&query.list)?;
    let account_type = match query.account_type.as_deref() {
        None | Some("") => None,
        Some(value) => Some(AccountType::parse(value).ok_or(ApiError::ValidationFailed)?),
    };
    let status = match query.status.as_deref() {
        None | Some("") => None,
        Some(value) => Some(AccountStatus::parse(value).ok_or(ApiError::ValidationFailed)?),
    };
    let rows = account_repo::list_accounts(
        pool,
        query.list.search.as_deref(),
        account_type,
        status,
        page,
        page_size,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "list accounts failed");
        ApiError::Internal
    })?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    Ok((
        StatusCode::OK,
        Json(PaginatedDto {
            items: rows.iter().map(account_dto).collect::<Vec<_>>(),
            page,
            page_size,
            total_count,
        }),
    )
        .into_response())
}

/// Active accounts for pickers (payment destination / transfer
/// endpoints) — payments.manage operators need the same list, so the
/// union of the two read surfaces is allowed.
pub async fn account_options(
    State(state): State<AppState>,
    auth: CurrentAuth,
) -> Result<Response, ApiError> {
    // Either capability suffices: viewing active accounts or posting
    // collections into them.
    if authz::require(&state, &auth, authz::catalog::FINANCIAL_ACCOUNTS_READ)
        .await
        .is_err()
    {
        authz::require(&state, &auth, authz::catalog::PAYMENTS_MANAGE).await?;
    }
    let pool = pool_of(&state)?;
    let rows = account_repo::list_active_account_options(pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "account options failed");
            ApiError::Internal
        })?;
    Ok((
        StatusCode::OK,
        Json(
            rows.iter()
                .map(|r| AccountOptionDto {
                    id: r.id,
                    name: r.name.clone(),
                    account_type: r.account_type.clone(),
                    currency: r.currency.clone(),
                    balance: canonical(r.balance),
                })
                .collect::<Vec<_>>(),
        ),
    )
        .into_response())
}

/// Cooperative-wide totals grouped BY CURRENCY — never a meaningless
/// cross-currency sum (STEP-008 is TRY-only; the grouping is explicit).
pub async fn accounts_summary(
    State(state): State<AppState>,
    auth: CurrentAuth,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::FINANCIAL_ACCOUNTS_READ).await?;
    let pool = pool_of(&state)?;
    let rows = account_repo::total_balance_by_currency(pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "accounts summary failed");
            ApiError::Internal
        })?;
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "totals": rows.iter().map(|(currency, balance)| serde_json::json!({
                "currency": currency,
                "balance": canonical(*balance),
            })).collect::<Vec<_>>(),
        })),
    )
        .into_response())
}

pub async fn get_account(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::FINANCIAL_ACCOUNTS_READ).await?;
    let pool = pool_of(&state)?;
    let row = account_repo::get_account(pool, id).await.map_err(|e| {
        tracing::error!(error = %e, "get account failed");
        ApiError::Internal
    })?;
    let Some(row) = row else {
        return Err(ApiError::NotFound);
    };
    Ok((StatusCode::OK, Json(account_dto(&row))).into_response())
}

pub async fn list_movements(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::FINANCIAL_ACCOUNTS_READ).await?;
    let pool = pool_of(&state)?;
    if account_repo::get_account(pool, id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "get account failed");
            ApiError::Internal
        })?
        .is_none()
    {
        return Err(ApiError::NotFound);
    }
    let (page, page_size) = page_of(&query)?;
    let rows = account_repo::list_movements(pool, id, page, page_size)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "list movements failed");
            ApiError::Internal
        })?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    Ok((
        StatusCode::OK,
        Json(PaginatedDto {
            items: rows
                .iter()
                .map(|m| {
                    let effect = MovementDirection::parse(&m.direction)
                        .unwrap_or(MovementDirection::Inflow)
                        .effect(m.amount);
                    AccountMovementDto {
                        id: m.id,
                        direction: m.direction.clone(),
                        amount: canonical(m.amount),
                        effect: canonical(effect),
                        source_type: m.source_type.clone(),
                        source_id: m.source_id,
                        source_number: m.source_number,
                        occurred_at: m.occurred_at,
                        status: m.status.clone(),
                        reversed_at: m.reversed_at,
                        reversal_reason: m.reversal_reason.clone(),
                        created_at: m.created_at,
                    }
                })
                .collect::<Vec<_>>(),
            page,
            page_size,
            total_count,
        }),
    )
        .into_response())
}

// ------------------------------------------------------------------
// Account commands
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAccountRequest {
    pub name: String,
    pub account_type: String,
    pub description: Option<String>,
    pub bank_name: Option<String>,
    pub iban: Option<String>,
}

pub async fn create_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Json(body): Json<CreateAccountRequest>,
) -> Result<Response, ApiError> {
    let pool = accounts_mutation!(&state, &headers, &auth, &Method::POST);
    let name = account_model::validate_name(&body.name).map_err(|_| ApiError::ValidationFailed)?;
    let account_type = AccountType::parse(&body.account_type).ok_or(ApiError::ValidationFailed)?;
    let description = account_model::validate_description(body.description.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let bank_name = account_model::validate_bank_name(body.bank_name.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let iban = account_model::validate_iban(body.iban.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    // Bank metadata is meaningful only on bank accounts.
    if account_type != AccountType::Bank && (bank_name.is_some() || iban.is_some()) {
        return Err(ApiError::ValidationFailed);
    }

    let id = account_repo::create_account(
        pool,
        auth.user_id,
        account_repo::CreateAccount {
            name,
            account_type,
            description,
            bank_name,
            iban,
        },
    )
    .await
    .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::FinancialAccountCreated,
        Some(auth.user_id),
        None,
        serde_json::json!({ "accountId": id }),
    )
    .await;
    let row = account_repo::get_account(pool, id).await.map_err(|e| {
        tracing::error!(error = %e, "get account failed");
        ApiError::Internal
    })?;
    let Some(row) = row else {
        return Err(ApiError::Internal);
    };
    Ok((StatusCode::CREATED, Json(account_dto(&row))).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAccountRequest {
    pub name: String,
    pub description: Option<String>,
    pub bank_name: Option<String>,
    pub iban: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub expected_updated_at: OffsetDateTime,
}

/// Safe metadata only — type and currency are immutable by design;
/// attempting to send them is rejected at the contract layer (they are
/// simply not fields of this request).
pub async fn update_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateAccountRequest>,
) -> Result<Response, ApiError> {
    let pool = accounts_mutation!(&state, &headers, &auth, &Method::PATCH);
    let name = account_model::validate_name(&body.name).map_err(|_| ApiError::ValidationFailed)?;
    let description = account_model::validate_description(body.description.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let bank_name = account_model::validate_bank_name(body.bank_name.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let iban = account_model::validate_iban(body.iban.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let current = account_repo::get_account(pool, id).await.map_err(|e| {
        tracing::error!(error = %e, "get account failed");
        ApiError::Internal
    })?;
    let Some(current) = current else {
        return Err(ApiError::NotFound);
    };
    if current.account_type != AccountType::Bank.as_str() && (bank_name.is_some() || iban.is_some())
    {
        return Err(ApiError::ValidationFailed);
    }

    account_repo::update_account(
        pool,
        id,
        name,
        description,
        bank_name,
        iban,
        body.expected_updated_at,
    )
    .await
    .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::FinancialAccountUpdated,
        Some(auth.user_id),
        None,
        serde_json::json!({ "accountId": id }),
    )
    .await;
    let row = account_repo::get_account(pool, id).await.map_err(|e| {
        tracing::error!(error = %e, "get account failed");
        ApiError::Internal
    })?;
    let Some(row) = row else {
        return Err(ApiError::Internal);
    };
    Ok((StatusCode::OK, Json(account_dto(&row))).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusChangeRequest {
    pub status: String,
}

pub async fn change_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<StatusChangeRequest>,
) -> Result<Response, ApiError> {
    let pool = accounts_mutation!(&state, &headers, &auth, &Method::POST);
    let target = AccountStatus::parse(&body.status).ok_or(ApiError::ValidationFailed)?;
    account_repo::change_account_status(pool, id, target)
        .await
        .map_err(command_error)?;
    audit::record(
        pool,
        SecurityEventType::FinancialAccountStatusChanged,
        Some(auth.user_id),
        None,
        serde_json::json!({ "accountId": id, "status": target.as_str() }),
    )
    .await;
    let row = account_repo::get_account(pool, id).await.map_err(|e| {
        tracing::error!(error = %e, "get account failed");
        ApiError::Internal
    })?;
    let Some(row) = row else {
        return Err(ApiError::Internal);
    };
    Ok((StatusCode::OK, Json(account_dto(&row))).into_response())
}

// ------------------------------------------------------------------
// Transfers
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostTransferRequest {
    pub source_account_id: Uuid,
    pub destination_account_id: Uuid,
    pub amount: String,
    pub occurred_at: Option<String>,
    pub note: Option<String>,
    pub idempotency_key: String,
}

pub async fn list_transfers(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<ListQuery>,
) -> Result<Json<PaginatedDto<AccountTransferDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::FINANCIAL_ACCOUNTS_READ).await?;
    let pool = pool_of(&state)?;
    let (page, page_size) = page_of(&query)?;
    let rows = account_repo::list_transfers(pool, page, page_size)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "transfer listing failed");
            ApiError::Internal
        })?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    Ok(Json(PaginatedDto {
        items: rows.iter().map(transfer_dto).collect(),
        page,
        page_size,
        total_count,
    }))
}

/// THE transfer command: ONE transaction creates the transfer row and
/// its two paired movement legs — never a half transfer.
pub async fn post_transfer(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Json(body): Json<PostTransferRequest>,
) -> Result<Response, ApiError> {
    let pool = accounts_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let amount = account_model::validate_positive_amount(&body.amount)
        .map_err(|_| ApiError::ValidationFailed)?;
    let occurred_at = match &body.occurred_at {
        Some(raw) => {
            OffsetDateTime::parse(raw.trim(), &Rfc3339).map_err(|_| ApiError::ValidationFailed)?
        }
        None => now,
    };
    account_model::validate_occurred_at(occurred_at, now)
        .map_err(|_| ApiError::ValidationFailed)?;
    let note = account_model::validate_note(body.note.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let idempotency_key = account_model::validate_idempotency_key(&body.idempotency_key)
        .map_err(|_| ApiError::ValidationFailed)?;
    if body.source_account_id == body.destination_account_id {
        return Err(ApiError::ValidationFailed);
    }
    let fingerprint = account_model::transfer_fingerprint(&account_model::TransferFingerprint {
        source_account_id: body.source_account_id,
        destination_account_id: body.destination_account_id,
        amount,
        occurred_at,
        note: note.clone(),
    });

    let outcome = account_repo::post_transfer(
        pool,
        auth.user_id,
        account_repo::PostTransfer {
            source_account_id: body.source_account_id,
            destination_account_id: body.destination_account_id,
            amount,
            occurred_at,
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
            SecurityEventType::AccountTransferPosted,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "transferId": outcome.transfer_id,
                "transferNumber": outcome.transfer_number,
                "sourceAccountId": body.source_account_id,
                "destinationAccountId": body.destination_account_id,
                "amount": canonical(amount),
            }),
        )
        .await;
    }
    let row = account_repo::get_transfer(pool, outcome.transfer_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "get transfer failed");
            ApiError::Internal
        })?;
    let Some(row) = row else {
        return Err(ApiError::Internal);
    };
    let status = if outcome.replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(transfer_dto(&row))).into_response())
}

pub async fn get_transfer(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::FINANCIAL_ACCOUNTS_READ).await?;
    let pool = pool_of(&state)?;
    let row = account_repo::get_transfer(pool, id).await.map_err(|e| {
        tracing::error!(error = %e, "get transfer failed");
        ApiError::Internal
    })?;
    let Some(row) = row else {
        return Err(ApiError::NotFound);
    };
    Ok((StatusCode::OK, Json(transfer_dto(&row))).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReverseTransferRequest {
    pub reason: String,
}

pub async fn reverse_transfer(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<ReverseTransferRequest>,
) -> Result<Response, ApiError> {
    let pool = accounts_mutation!(&state, &headers, &auth, &Method::POST);
    let reason = account_model::validate_reversal_reason(&body.reason)
        .map_err(|_| ApiError::ValidationFailed)?;
    let now = state.auth.clock.now();
    let (replayed, transfer_number) =
        account_repo::reverse_transfer(pool, auth.user_id, id, reason, now)
            .await
            .map_err(command_error)?;
    if !replayed {
        audit::record(
            pool,
            SecurityEventType::AccountTransferReversed,
            Some(auth.user_id),
            None,
            serde_json::json!({ "transferId": id, "transferNumber": transfer_number }),
        )
        .await;
    }
    let row = account_repo::get_transfer(pool, id).await.map_err(|e| {
        tracing::error!(error = %e, "get transfer failed");
        ApiError::Internal
    })?;
    let Some(row) = row else {
        return Err(ApiError::Internal);
    };
    Ok((StatusCode::OK, Json(transfer_dto(&row))).into_response())
}
