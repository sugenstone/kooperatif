//! Income & Expense API (STEP-010, docs/07).
//!
//! - `GET   /api/financial-categories`                    (income_expense.read)
//! - `GET   /api/financial-categories/options`            (income_expense.read)
//! - `POST  /api/financial-categories`                    (income_expense.manage)
//! - `PATCH /api/financial-categories/{id}`               (income_expense.manage)
//! - `POST  /api/financial-categories/{id}/status-change` (income_expense.manage)
//! - `GET   /api/incomes`                                 (income_expense.read)
//! - `POST  /api/incomes`                                 (income_expense.manage)
//! - `GET   /api/incomes/{id}`                            (income_expense.read)
//! - `POST  /api/incomes/{id}/reverse`                    (income_expense.manage)
//! - `GET   /api/expenses`                                (income_expense.read)
//! - `POST  /api/expenses`                                (income_expense.manage)
//! - `GET   /api/expenses/{id}`                           (income_expense.read)
//! - `POST  /api/expenses/{id}/reverse`                   (income_expense.manage)
//! - `GET   /api/income-expense/summary`                  (income_expense.read)
//!
//! There is deliberately NO DELETE — posted financial history is
//! corrected by reversal, never erased (docs/19). Each posting binds
//! exactly ONE Account Movement; entry tables hold WHY it happened,
//! the movement holds the authoritative money effect (ADR-003).
//! Money fields are decimal STRINGS (ADR-004).

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use time::format_description::FormatItem;
use time::{format_description::well_known::Rfc3339, Date, OffsetDateTime};
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

use super::model as entry_model;
use super::repo as entry_repo;
use entry_model::{CategoryStatus, CategoryType, EntryKind, EntryStatus};
use entry_repo::IncomeExpenseCommandError;

const DATE_FORMAT: &[FormatItem<'_>] = time::macros::format_description!("[year]-[month]-[day]");

pub fn income_expense_router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/financial-categories",
            get(list_categories).post(create_category),
        )
        .route("/api/financial-categories/options", get(category_options))
        .route(
            "/api/financial-categories/{id}",
            axum::routing::patch(update_category),
        )
        .route(
            "/api/financial-categories/{id}/status-change",
            post(change_category_status),
        )
        .route("/api/incomes", get(list_incomes).post(post_income))
        .route("/api/incomes/{id}", get(get_income))
        .route("/api/incomes/{id}/reverse", post(reverse_income))
        .route("/api/expenses", get(list_expenses).post(post_expense))
        .route("/api/expenses/{id}", get(get_expense))
        .route("/api/expenses/{id}/reverse", post(reverse_expense))
        .route("/api/income-expense/summary", get(operational_summary))
        .layer(axum::middleware::from_fn(
            crate::auth::routes::no_store_cache_control,
        ))
}

fn pool_of(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state.db.as_ref().ok_or(ApiError::DependencyUnavailable)
}

/// Entry/category mutations share the full CSRF + permission prologue.
macro_rules! income_expense_mutation {
    ($state:expr, $headers:expr, $auth:expr, $method:expr) => {{
        csrf::validate_origin($headers, $method, &$state.auth.config.allowed_origins)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        csrf::validate_csrf_token($headers, &$auth.csrf_token)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        authz::require($state, $auth, authz::catalog::INCOME_EXPENSE_MANAGE).await?;
        pool_of($state)?
    }};
}

fn db_err(context: &'static str) -> impl Fn(sqlx::Error) -> ApiError {
    move |error| {
        tracing::error!(error = %error, context);
        ApiError::Internal
    }
}

fn command_error(error: IncomeExpenseCommandError) -> ApiError {
    use IncomeExpenseCommandError::*;
    match error {
        NotFound => ApiError::NotFound,
        // Deterministic request faults — same request always fails.
        CategoryInvalid | InactiveAccount => ApiError::ValidationFailed,
        // State-dependent rejections (docs/21 conflict semantics).
        InsufficientFunds | IdempotencyConflict | DuplicateName => ApiError::Conflict,
        InsufficientUnrestrictedFunds => ApiError::InsufficientUnrestrictedFunds,
        StaleState => ApiError::StaleState,
        AccountInvariant(error) => {
            tracing::error!(error = ?error, "unexpected account-layer error in entry command");
            ApiError::Internal
        }
        Database(error) => {
            tracing::error!(error = %error, "income/expense command failed");
            ApiError::Internal
        }
    }
}

fn canonical(amount: Decimal) -> String {
    payment_model::canonical_amount(amount)
}

// ------------------------------------------------------------------
// DTOs (mirror @kooperatif/contracts/src/income_expense.ts)
// ------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FinancialCategoryDto {
    pub id: Uuid,
    pub category_type: String,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    /// How many posted/reversed entries reference this category —
    /// a hard delete is never possible anyway; the count makes the
    /// history-bearing nature visible to the operator.
    pub entry_count: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

fn category_dto(row: &entry_repo::CategoryRow) -> FinancialCategoryDto {
    FinancialCategoryDto {
        id: row.id,
        category_type: row.category_type.clone(),
        name: row.name.clone(),
        description: row.description.clone(),
        status: row.status.clone(),
        entry_count: row.entry_count,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// One Income/Expense entry — the business event bound 1:1 to its
/// Account Movement. `entryNumber` is the stable human identity
/// (Gelir No / Gider No).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryDto {
    pub id: Uuid,
    pub entry_number: i64,
    pub financial_account_id: Uuid,
    pub account_name: String,
    pub category_id: Uuid,
    pub category_name: String,
    pub amount: String,
    pub currency: String,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    pub description: String,
    /// Descriptive income source / expense payee — never a Person
    /// or Shareholder reference.
    pub counterparty: Option<String>,
    pub reference_no: Option<String>,
    /// Movement provenance: the exact Account Movement this entry
    /// produced (its live status is the authoritative money state).
    pub account_movement_id: Uuid,
    pub movement_status: String,
    pub status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

fn entry_dto(row: &entry_repo::EntryRow) -> EntryDto {
    EntryDto {
        id: row.id,
        entry_number: row.entry_number,
        financial_account_id: row.financial_account_id,
        account_name: row.account_name.clone(),
        category_id: row.category_id,
        category_name: row.category_name.clone(),
        amount: canonical(row.amount),
        currency: row.currency.clone(),
        occurred_at: row.occurred_at,
        description: row.description.clone(),
        counterparty: row.counterparty.clone(),
        reference_no: row.reference_no.clone(),
        account_movement_id: row.account_movement_id,
        movement_status: row.movement_status.clone(),
        status: row.status.clone(),
        reversed_at: row.reversed_at,
        reversal_reason: row.reversal_reason.clone(),
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// Operational totals over POSTED entries — NOT the account balance
/// (Payments, Transfers, future domains are excluded by construction).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationalSummaryDto {
    pub income_total: String,
    pub income_count: i64,
    pub expense_total: String,
    pub expense_count: i64,
    /// incomeTotal − expenseTotal, exact decimal subtraction.
    pub net: String,
    pub currency: String,
}

// ------------------------------------------------------------------
// Categories
// ------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CategoryListQuery {
    // Inlined instead of `#[serde(flatten)]`: serde_urlencoded cannot
    // coerce numeric types through flattened maps (`pageSize=20` →
    // "invalid type: string"), which broke filtered list requests.
    pub search: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub category_type: Option<String>,
    pub status: Option<String>,
}

pub async fn list_categories(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<CategoryListQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::INCOME_EXPENSE_READ).await?;
    let pool = pool_of(&state)?;
    let (page, page_size) = page_of(&ListQuery {
        search: query.search.clone(),
        page: query.page,
        page_size: query.page_size,
    })?;
    let category_type = match query.category_type.as_deref() {
        None | Some("") => None,
        Some(value) => Some(CategoryType::parse(value).ok_or(ApiError::ValidationFailed)?),
    };
    let status = match query.status.as_deref() {
        None | Some("") => None,
        Some(value) => Some(CategoryStatus::parse(value).ok_or(ApiError::ValidationFailed)?),
    };
    let rows = entry_repo::list_categories(
        pool,
        category_type,
        status,
        query.search.as_deref(),
        page,
        page_size,
    )
    .await
    .map_err(db_err("category list failed"))?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    Ok((
        StatusCode::OK,
        Json(PaginatedDto {
            items: rows.iter().map(category_dto).collect::<Vec<_>>(),
            page,
            page_size,
            total_count,
        }),
    )
        .into_response())
}

/// Active categories of one type for entry forms (`?type=income`).
pub async fn category_options(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<CategoryListQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::INCOME_EXPENSE_READ).await?;
    let pool = pool_of(&state)?;
    let category_type = CategoryType::parse(query.category_type.as_deref().unwrap_or_default())
        .ok_or(ApiError::ValidationFailed)?;
    let rows = entry_repo::list_category_options(pool, category_type)
        .await
        .map_err(db_err("category options failed"))?;
    Ok((
        StatusCode::OK,
        Json(rows.iter().map(category_dto).collect::<Vec<_>>()),
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCategoryRequest {
    pub category_type: String,
    pub name: String,
    pub description: Option<String>,
}

pub async fn create_category(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Json(body): Json<CreateCategoryRequest>,
) -> Result<Response, ApiError> {
    let pool = income_expense_mutation!(&state, &headers, &auth, &Method::POST);
    let category_type =
        CategoryType::parse(&body.category_type).ok_or(ApiError::ValidationFailed)?;
    let name = entry_model::validate_name(&body.name).map_err(|_| ApiError::ValidationFailed)?;
    let description = entry_model::validate_description(body.description.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;

    let id = entry_repo::create_category(
        pool,
        auth.user_id,
        entry_repo::CreateCategory {
            category_type,
            name,
            description,
        },
    )
    .await
    .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::FinancialCategoryCreated,
        Some(auth.user_id),
        None,
        serde_json::json!({ "categoryId": id, "categoryType": category_type.as_str() }),
    )
    .await;
    let row = entry_repo::get_category(pool, id)
        .await
        .map_err(db_err("get category failed"))?;
    let Some(row) = row else {
        return Err(ApiError::Internal);
    };
    Ok((StatusCode::CREATED, Json(category_dto(&row))).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCategoryRequest {
    pub name: String,
    pub description: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub expected_updated_at: OffsetDateTime,
}

/// Name/description only — `category_type` is immutable (it is part of
/// the composite FK that binds entries to the right kind of category).
pub async fn update_category(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateCategoryRequest>,
) -> Result<Response, ApiError> {
    let pool = income_expense_mutation!(&state, &headers, &auth, &Method::PATCH);
    let name = entry_model::validate_name(&body.name).map_err(|_| ApiError::ValidationFailed)?;
    let description = entry_model::validate_description(body.description.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;

    entry_repo::update_category(pool, id, name, description, body.expected_updated_at)
        .await
        .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::FinancialCategoryUpdated,
        Some(auth.user_id),
        None,
        serde_json::json!({ "categoryId": id }),
    )
    .await;
    let row = entry_repo::get_category(pool, id)
        .await
        .map_err(db_err("get category failed"))?;
    let Some(row) = row else {
        return Err(ApiError::Internal);
    };
    Ok((StatusCode::OK, Json(category_dto(&row))).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryStatusChangeRequest {
    pub status: String,
}

pub async fn change_category_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<CategoryStatusChangeRequest>,
) -> Result<Response, ApiError> {
    let pool = income_expense_mutation!(&state, &headers, &auth, &Method::POST);
    let target = CategoryStatus::parse(&body.status).ok_or(ApiError::ValidationFailed)?;
    entry_repo::change_category_status(pool, id, target)
        .await
        .map_err(command_error)?;
    audit::record(
        pool,
        SecurityEventType::FinancialCategoryStatusChanged,
        Some(auth.user_id),
        None,
        serde_json::json!({ "categoryId": id, "status": target.as_str() }),
    )
    .await;
    let row = entry_repo::get_category(pool, id)
        .await
        .map_err(db_err("get category failed"))?;
    let Some(row) = row else {
        return Err(ApiError::Internal);
    };
    Ok((StatusCode::OK, Json(category_dto(&row))).into_response())
}

// ------------------------------------------------------------------
// Entries
// ------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EntryListQuery {
    // Inlined instead of `#[serde(flatten)]` — see CategoryListQuery.
    pub search: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub financial_account_id: Option<String>,
    pub category_id: Option<String>,
    pub status: Option<String>,
}

fn parse_date(raw: Option<&str>) -> Result<Option<Date>, ApiError> {
    match raw {
        None | Some("") => Ok(None),
        Some(value) => Date::parse(value.trim(), DATE_FORMAT)
            .map(Some)
            .map_err(|_| ApiError::ValidationFailed),
    }
}

fn parse_uuid(raw: Option<&str>) -> Result<Option<Uuid>, ApiError> {
    match raw {
        None | Some("") => Ok(None),
        Some(value) => Uuid::parse_str(value.trim())
            .map(Some)
            .map_err(|_| ApiError::ValidationFailed),
    }
}

fn entry_filter(query: &EntryListQuery) -> Result<entry_repo::EntryFilter, ApiError> {
    let status = match query.status.as_deref() {
        None | Some("") => None,
        Some(value) => {
            let parsed = EntryStatus::parse(value).ok_or(ApiError::ValidationFailed)?;
            Some(parsed.as_str().to_string())
        }
    };
    let (date_from, date_to) = (
        parse_date(query.date_from.as_deref())?,
        parse_date(query.date_to.as_deref())?,
    );
    if let (Some(from), Some(to)) = (date_from, date_to) {
        if to < from {
            return Err(ApiError::ValidationFailed);
        }
    }
    Ok(entry_repo::EntryFilter {
        date_from,
        date_to,
        financial_account_id: parse_uuid(query.financial_account_id.as_deref())?,
        category_id: parse_uuid(query.category_id.as_deref())?,
        status,
        search: query.search.clone(),
    })
}

async fn list_entries_of(
    state: AppState,
    auth: CurrentAuth,
    kind: EntryKind,
    query: EntryListQuery,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::INCOME_EXPENSE_READ).await?;
    let pool = pool_of(&state)?;
    let (page, page_size) = page_of(&ListQuery {
        search: query.search.clone(),
        page: query.page,
        page_size: query.page_size,
    })?;
    let filter = entry_filter(&query)?;
    let rows = entry_repo::list_entries(pool, kind, &filter, page, page_size)
        .await
        .map_err(db_err("entry list failed"))?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    Ok((
        StatusCode::OK,
        Json(PaginatedDto {
            items: rows.iter().map(entry_dto).collect::<Vec<_>>(),
            page,
            page_size,
            total_count,
        }),
    )
        .into_response())
}

pub async fn list_incomes(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<EntryListQuery>,
) -> Result<Response, ApiError> {
    list_entries_of(state, auth, EntryKind::Income, query).await
}

pub async fn list_expenses(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<EntryListQuery>,
) -> Result<Response, ApiError> {
    list_entries_of(state, auth, EntryKind::Expense, query).await
}

async fn get_entry_of(
    state: AppState,
    auth: CurrentAuth,
    kind: EntryKind,
    id: Uuid,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::INCOME_EXPENSE_READ).await?;
    let pool = pool_of(&state)?;
    let row = entry_repo::get_entry(pool, kind, id)
        .await
        .map_err(db_err("get entry failed"))?;
    let Some(row) = row else {
        return Err(ApiError::NotFound);
    };
    Ok((StatusCode::OK, Json(entry_dto(&row))).into_response())
}

pub async fn get_income(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    get_entry_of(state, auth, EntryKind::Income, id).await
}

pub async fn get_expense(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    get_entry_of(state, auth, EntryKind::Expense, id).await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostEntryRequest {
    pub financial_account_id: Uuid,
    pub category_id: Uuid,
    /// Positive decimal string.
    pub amount: String,
    /// Optional RFC3339 — defaults to server now; backdating allowed.
    pub occurred_at: Option<String>,
    pub description: String,
    /// Income source / expense payee — descriptive text only.
    pub counterparty: Option<String>,
    /// Optional document reference ("Fatura No: …").
    pub reference_no: Option<String>,
    pub idempotency_key: String,
}

/// THE posting command: entry + exactly ONE movement, atomically
/// inside the account row lock. Replay → 200 with the existing entry.
async fn post_entry_of(
    state: AppState,
    headers: HeaderMap,
    auth: CurrentAuth,
    kind: EntryKind,
    body: PostEntryRequest,
) -> Result<Response, ApiError> {
    let pool = income_expense_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let amount = entry_model::validate_positive_amount(&body.amount)
        .map_err(|_| ApiError::ValidationFailed)?;
    let occurred_at = match &body.occurred_at {
        Some(raw) => {
            OffsetDateTime::parse(raw.trim(), &Rfc3339).map_err(|_| ApiError::ValidationFailed)?
        }
        None => now,
    };
    entry_model::validate_occurred_at(occurred_at, now).map_err(|_| ApiError::ValidationFailed)?;
    // Idempotency covers CLIENT INTENT: an omitted occurredAt must not
    // enter the fingerprint as the server-generated default, or a
    // retry-after-timeout would collide as a false 409 (F4).
    let occurred_at_intent = body.occurred_at.is_some().then_some(occurred_at);
    let description = entry_model::validate_entry_description(&body.description)
        .map_err(|_| ApiError::ValidationFailed)?;
    let counterparty = entry_model::validate_counterparty(body.counterparty.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let reference_no = entry_model::validate_reference_no(body.reference_no.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let idempotency_key = entry_model::validate_idempotency_key(&body.idempotency_key)
        .map_err(|_| ApiError::ValidationFailed)?;
    let fingerprint = entry_model::entry_fingerprint(&entry_model::EntryFingerprint {
        financial_account_id: body.financial_account_id,
        category_id: body.category_id,
        amount,
        occurred_at: occurred_at_intent,
        description: description.clone(),
        counterparty: counterparty.clone(),
        reference_no: reference_no.clone(),
    });

    let outcome = entry_repo::post_entry(
        pool,
        auth.user_id,
        kind,
        entry_repo::PostEntry {
            financial_account_id: body.financial_account_id,
            category_id: body.category_id,
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
            match kind {
                EntryKind::Income => SecurityEventType::IncomePosted,
                EntryKind::Expense => SecurityEventType::ExpensePosted,
            },
            Some(auth.user_id),
            None,
            serde_json::json!({
                "entryId": outcome.entry_id,
                "entryNumber": outcome.entry_number,
                "kind": kind.as_str(),
                "financialAccountId": body.financial_account_id,
                "categoryId": body.category_id,
                "amount": canonical(amount),
            }),
        )
        .await;
        tracing::info!(
            outcome = "entry_posted", kind = kind.as_str(),
            id = %outcome.entry_id, number = outcome.entry_number, actor = %auth.user_id
        );
    }

    let row = entry_repo::get_entry(pool, kind, outcome.entry_id)
        .await
        .map_err(db_err("get entry failed"))?;
    let Some(row) = row else {
        return Err(ApiError::Internal);
    };
    let status = if outcome.replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(entry_dto(&row))).into_response())
}

pub async fn post_income(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Json(body): Json<PostEntryRequest>,
) -> Result<Response, ApiError> {
    post_entry_of(state, headers, auth, EntryKind::Income, body).await
}

pub async fn post_expense(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Json(body): Json<PostEntryRequest>,
) -> Result<Response, ApiError> {
    post_entry_of(state, headers, auth, EntryKind::Expense, body).await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReverseEntryRequest {
    /// Required non-empty reason (docs/19 reversal contract).
    pub reason: String,
}

/// Reversal: entry row locked, movement neutralized, entry marked
/// `reversed` with actor/time/reason. Replays are safe.
async fn reverse_entry_of(
    state: AppState,
    headers: HeaderMap,
    auth: CurrentAuth,
    kind: EntryKind,
    id: Uuid,
    body: ReverseEntryRequest,
) -> Result<Response, ApiError> {
    let pool = income_expense_mutation!(&state, &headers, &auth, &Method::POST);
    let reason = entry_model::validate_reversal_reason(&body.reason)
        .map_err(|_| ApiError::ValidationFailed)?;
    let now = state.auth.clock.now();
    let (replayed, entry_number) =
        entry_repo::reverse_entry(pool, auth.user_id, kind, id, reason, now)
            .await
            .map_err(command_error)?;
    if !replayed {
        audit::record(
            pool,
            match kind {
                EntryKind::Income => SecurityEventType::IncomeReversed,
                EntryKind::Expense => SecurityEventType::ExpenseReversed,
            },
            Some(auth.user_id),
            None,
            serde_json::json!({
                "entryId": id,
                "entryNumber": entry_number,
                "kind": kind.as_str(),
            }),
        )
        .await;
    }
    let row = entry_repo::get_entry(pool, kind, id)
        .await
        .map_err(db_err("get entry failed"))?;
    let Some(row) = row else {
        return Err(ApiError::Internal);
    };
    Ok((StatusCode::OK, Json(entry_dto(&row))).into_response())
}

pub async fn reverse_income(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<ReverseEntryRequest>,
) -> Result<Response, ApiError> {
    reverse_entry_of(state, headers, auth, EntryKind::Income, id, body).await
}

pub async fn reverse_expense(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<ReverseEntryRequest>,
) -> Result<Response, ApiError> {
    reverse_entry_of(state, headers, auth, EntryKind::Expense, id, body).await
}

/// Operational Income/Expense summary over POSTED entries — NOT an
/// account balance and NOT a P&L (docs/15, STEP-010 §31).
pub async fn operational_summary(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<EntryListQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::INCOME_EXPENSE_READ).await?;
    let pool = pool_of(&state)?;
    let filter = entry_filter(&query)?;
    let summary = entry_repo::operational_summary(pool, &filter)
        .await
        .map_err(db_err("summary failed"))?;
    let net = summary.income_total - summary.expense_total;
    Ok((
        StatusCode::OK,
        Json(OperationalSummaryDto {
            income_total: canonical(summary.income_total),
            income_count: summary.income_count,
            expense_total: canonical(summary.expense_total),
            expense_count: summary.expense_count,
            net: canonical(net),
            currency: "TRY".to_string(),
        }),
    )
        .into_response())
}
