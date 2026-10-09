//! Periods & Assessments API (STEP-006, docs/05 §60–§66).
//!
//! - `GET    /api/periods`                              (periods.read)
//! - `POST   /api/periods`                              (periods.manage)
//! - `GET    /api/periods/{id}`                         (periods.read)
//! - `PATCH  /api/periods/{id}`                         (periods.manage, draft only)
//! - `DELETE /api/periods/{id}`                         (periods.manage, draft only)
//! - `POST   /api/periods/{id}/assessment-preview`      (assessments.read; read-only)
//! - `POST   /api/periods/{id}/generate-assessments`    (assessments.manage; atomic+idempotent)
//! - `POST   /api/periods/{id}/close`                   (periods.manage)
//! - `GET    /api/periods/{id}/assessments`             (assessments.read)
//! - `GET    /api/assessments/{id}`                     (assessments.read; + share provenance)
//! - `GET    /api/shareholders/{id}/assessments`        (assessments.read)
//!
//! Deliberately absent: payment, collection, allocation, cashbox, bank
//! or ledger operations. Obligations are durable; money does not move.
//! `amount`/`baseAmount` are decimal STRINGS (ADR-004); the preview is
//! an explicit POST so it is CSRF-guarded like every command.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use time::{format_description::well_known::Iso8601, Date, OffsetDateTime};
use uuid::Uuid;

use crate::auth::audit::{self, SecurityEventType};
use crate::auth::authz;
use crate::auth::csrf;
use crate::auth::extractor::CurrentAuth;
use crate::auth::routes::csrf_rejection_to_api_error;
use crate::http::error::ApiError;
use crate::http::AppState;
use crate::parties::routes::{page_of, ListQuery, PaginatedDto};
use crate::shares::routes::{identity_dto, ShareholderIdentityDto};

use super::model as period_model;
use super::repo as period_repo;

pub fn periods_router() -> Router<AppState> {
    Router::new()
        .route("/api/periods", get(list_periods).post(create_period))
        .route(
            "/api/periods/{id}",
            get(get_period).patch(update_period).delete(delete_period),
        )
        .route(
            "/api/periods/{id}/assessment-preview",
            post(preview_assessments),
        )
        .route(
            "/api/periods/{id}/generate-assessments",
            post(generate_assessments),
        )
        .route("/api/periods/{id}/close", post(close_period))
        .route("/api/periods/{id}/assessments", get(list_assessments))
        .route("/api/assessments/{id}", get(get_assessment))
        .route(
            "/api/shareholders/{id}/assessments",
            get(list_shareholder_assessments),
        )
        .layer(axum::middleware::from_fn(
            crate::auth::routes::no_store_cache_control,
        ))
}

fn pool_of(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state.db.as_ref().ok_or(ApiError::DependencyUnavailable)
}

/// Period mutations share this prologue (CSRF layers + permission).
macro_rules! periods_mutation {
    ($state:expr, $headers:expr, $auth:expr, $method:expr, $permission:expr) => {{
        csrf::validate_origin($headers, $method, &$state.auth.config.allowed_origins)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        csrf::validate_csrf_token($headers, &$auth.csrf_token)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        authz::require($state, $auth, $permission).await?;
        pool_of($state)?
    }};
}

fn db_err(context: &'static str) -> impl Fn(sqlx::Error) -> ApiError {
    move |error| {
        tracing::error!(error = %error, context);
        ApiError::Internal
    }
}

fn command_error(error: period_repo::PeriodCommandError) -> ApiError {
    use period_repo::PeriodCommandError::*;
    match error {
        NotFound => ApiError::NotFound,
        // Finalized-state protection and lifecycle guards are 409
        // conflicts (docs/21): the request is well-formed but the
        // domain state forbids it.
        NotDraft | NotOpen | AlreadyFinalized | MissingRule => ApiError::Conflict,
        StaleState => ApiError::StaleState,
        Database(error) => {
            tracing::error!(error = %error, "period command failed");
            ApiError::Internal
        }
    }
}

// ------------------------------------------------------------------
// DTOs (mirror @kooperatif/contracts/src/periods.ts)
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
pub struct PeriodListItemDto {
    pub id: Uuid,
    pub period_number: i64,
    pub name: String,
    pub status: String,
    #[serde(with = "date_iso")]
    pub collection_start_date: Date,
    #[serde(with = "date_iso")]
    pub due_date: Date,
    pub rule_type: Option<String>,
    /// Decimal string; NULL while the rule is somehow absent.
    pub base_amount: Option<String>,
    pub currency: Option<String>,
    #[serde(with = "date_iso::option")]
    pub assessment_effective_date: Option<Date>,
    pub assessment_count: i64,
    /// NULL before generation — never rendered as "0" (NULL ≠ 0).
    pub total_assessment: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeriodDetailDto {
    #[serde(flatten)]
    pub item: PeriodListItemDto,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

fn period_item(row: &period_repo::PeriodRow) -> PeriodListItemDto {
    PeriodListItemDto {
        id: row.id,
        period_number: row.period_number,
        name: row.name.clone(),
        status: row.status.clone(),
        collection_start_date: row.collection_start_date,
        due_date: row.due_date,
        rule_type: row.rule_type.clone(),
        base_amount: row.base_amount.map(period_model::canonical_amount),
        currency: row.currency.clone(),
        assessment_effective_date: row.assessment_effective_date,
        assessment_count: row.assessment_count,
        total_assessment: row.total_assessment.map(period_model::canonical_amount),
        updated_at: row.updated_at,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssessmentListItemDto {
    pub id: Uuid,
    pub period_id: Uuid,
    /// Canonical debtor identity at read time; the obligation itself is
    /// frozen by `shareholder_id` + stored amounts (§35/§36).
    pub shareholder: ShareholderIdentityDto,
    /// Snapshotted rule — survives later rule edits on the draft record.
    pub rule_type: String,
    pub base_amount: String,
    pub amount: String,
    pub currency: String,
    pub status: String,
    #[serde(with = "date_iso")]
    pub assessment_effective_date: Date,
    /// Contributing share count; 0 for the per_shareholder rule.
    pub share_count: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub generated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssessmentSourceDto {
    pub share_id: Uuid,
    pub share_number: i64,
    pub amount_component: String,
    #[serde(with = "time::serde::rfc3339")]
    pub ownership_started_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssessmentDetailDto {
    #[serde(flatten)]
    pub item: AssessmentListItemDto,
    /// Per-Share provenance (§23); empty for the per_shareholder rule.
    pub sources: Vec<AssessmentSourceDto>,
}

fn assessment_item(row: &period_repo::AssessmentRow) -> AssessmentListItemDto {
    AssessmentListItemDto {
        id: row.id,
        period_id: row.period_id,
        shareholder: identity_dto(
            Some(row.shareholder.shareholder_id),
            &Some(row.shareholder.first_name.clone()),
            &Some(row.shareholder.last_name.clone()),
            &row.shareholder.guardian_first_name,
            &row.shareholder.guardian_last_name,
            row.shareholder.family_sequence,
            Some(row.shareholder.shareholder_status.as_str()),
        )
        .expect("assessment always joins its shareholder"),
        rule_type: row.rule_type.clone(),
        base_amount: period_model::canonical_amount(row.base_amount),
        amount: period_model::canonical_amount(row.amount),
        currency: row.currency.clone(),
        status: row.status.clone(),
        assessment_effective_date: row.assessment_effective_date,
        share_count: row.share_count,
        generated_at: row.generated_at,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareholderAssessmentDto {
    pub id: Uuid,
    pub period_id: Uuid,
    pub period_number: i64,
    pub period_name: String,
    #[serde(with = "date_iso")]
    pub due_date: Date,
    pub rule_type: String,
    pub amount: String,
    pub currency: String,
    pub share_count: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub generated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRowDto {
    pub shareholder: ShareholderIdentityDto,
    pub share_count: i64,
    pub amount: String,
}

/// Read-only preview (§28/§29): rule, effective point, eligible counts
/// and expected totals — a snapshot explanation that persists nothing.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewResponse {
    pub period_id: Uuid,
    pub rule_type: String,
    pub base_amount: String,
    pub currency: String,
    #[serde(with = "date_iso")]
    pub assessment_effective_date: Date,
    pub eligible_shareholder_count: i64,
    pub eligible_share_count: i64,
    pub assessment_count: i64,
    pub total_amount: String,
    pub page: i64,
    pub page_size: i64,
    pub total_rows: i64,
    pub rows: Vec<PreviewRowDto>,
}

// ------------------------------------------------------------------
// Requests
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct PeriodRequest {
    pub name: String,
    pub collection_start_date: String,
    pub due_date: String,
    pub rule_type: String,
    pub base_amount: String,
    pub assessment_effective_date: String,
    /// Required on PATCH (optimistic concurrency); ignored on create.
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub expected_updated_at: Option<OffsetDateTime>,
}

struct ValidatedPeriod {
    name: String,
    collection_start_date: Date,
    due_date: Date,
    rule_type: period_model::AssessmentRuleType,
    base_amount: Decimal,
    assessment_effective_date: Date,
    expected_updated_at: Option<OffsetDateTime>,
}

fn parse_date(raw: &str) -> Result<Date, ApiError> {
    Date::parse(raw.trim(), &Iso8601::DATE).map_err(|_| ApiError::ValidationFailed)
}

fn validated(payload: PeriodRequest, require_expected: bool) -> Result<ValidatedPeriod, ApiError> {
    let name =
        period_model::validate_name(&payload.name).map_err(|_| ApiError::ValidationFailed)?;
    let start = parse_date(&payload.collection_start_date)?;
    let due = parse_date(&payload.due_date)?;
    period_model::validate_dates(start, due).map_err(|_| ApiError::ValidationFailed)?;
    let rule_type = period_model::AssessmentRuleType::parse(&payload.rule_type)
        .ok_or(ApiError::ValidationFailed)?;
    let base_amount = period_model::validate_base_amount(&payload.base_amount)
        .map_err(|_| ApiError::ValidationFailed)?;
    // The effective date is the "ownership as of" snapshot point (§26);
    // it needs no ordering relation to the collection window.
    let effective = parse_date(&payload.assessment_effective_date)?;
    if require_expected && payload.expected_updated_at.is_none() {
        return Err(ApiError::ValidationFailed);
    }
    Ok(ValidatedPeriod {
        name,
        collection_start_date: start,
        due_date: due,
        rule_type,
        base_amount,
        assessment_effective_date: effective,
        expected_updated_at: payload.expected_updated_at,
    })
}

// ------------------------------------------------------------------
// Handlers
// ------------------------------------------------------------------

pub async fn list_periods(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<ListQuery>,
) -> Result<Json<PaginatedDto<PeriodListItemDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::PERIODS_READ).await?;
    let pool = pool_of(&state)?;
    let (page, page_size) = page_of(&query)?;
    let rows = period_repo::list_periods(pool, query.search.as_deref(), page, page_size)
        .await
        .map_err(db_err("period listing failed"))?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    Ok(Json(PaginatedDto {
        items: rows.iter().map(period_item).collect(),
        page,
        page_size,
        total_count,
    }))
}

pub async fn create_period(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Json(body): Json<PeriodRequest>,
) -> Result<Response, ApiError> {
    let pool = periods_mutation!(
        &state,
        &headers,
        &auth,
        &Method::POST,
        authz::catalog::PERIODS_MANAGE
    );
    let validated = validated(body, false)?;
    let audit_name = validated.name.clone();

    let (period_id, period_number) = period_repo::create_period(
        pool,
        auth.user_id,
        period_repo::CreatePeriod {
            name: validated.name,
            collection_start_date: validated.collection_start_date,
            due_date: validated.due_date,
            rule_type: validated.rule_type,
            base_amount: validated.base_amount,
            assessment_effective_date: validated.assessment_effective_date,
        },
    )
    .await
    .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::PeriodCreated,
        Some(auth.user_id),
        None,
        serde_json::json!({
            "period_id": period_id,
            "period_number": period_number,
            "name": audit_name,
            "rule_type": validated.rule_type.as_str(),
            "base_amount": period_model::canonical_amount(validated.base_amount),
            "assessment_effective_date": validated.assessment_effective_date.to_string(),
        }),
    )
    .await;
    tracing::info!(outcome = "period_created", id = %period_id, number = period_number, actor = %auth.user_id);

    let row = period_repo::find_period(pool, period_id)
        .await
        .map_err(db_err("period reload failed"))?
        .ok_or(ApiError::Internal)?;
    Ok((
        StatusCode::CREATED,
        Json(PeriodDetailDto {
            item: period_item(&row),
            created_at: row.created_at,
        }),
    )
        .into_response())
}

pub async fn get_period(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<PeriodDetailDto>, ApiError> {
    authz::require(&state, &auth, authz::catalog::PERIODS_READ).await?;
    let pool = pool_of(&state)?;
    period_repo::find_period(pool, id)
        .await
        .map_err(db_err("period load failed"))?
        .map(|row| {
            Json(PeriodDetailDto {
                item: period_item(&row),
                created_at: row.created_at,
            })
        })
        .ok_or(ApiError::NotFound)
}

pub async fn update_period(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<PeriodRequest>,
) -> Result<Json<PeriodDetailDto>, ApiError> {
    let pool = periods_mutation!(
        &state,
        &headers,
        &auth,
        &Method::PATCH,
        authz::catalog::PERIODS_MANAGE
    );
    let validated = validated(body, true)?;

    period_repo::update_period(
        pool,
        id,
        period_repo::UpdatePeriod {
            name: validated.name,
            collection_start_date: validated.collection_start_date,
            due_date: validated.due_date,
            rule_type: validated.rule_type,
            base_amount: validated.base_amount,
            assessment_effective_date: validated.assessment_effective_date,
            expected_updated_at: validated.expected_updated_at.expect("validated"),
        },
        state.auth.clock.now(),
    )
    .await
    .map_err(command_error)?;

    let row = period_repo::find_period(pool, id)
        .await
        .map_err(db_err("period reload failed"))?
        .ok_or(ApiError::NotFound)?;
    audit::record(
        pool,
        SecurityEventType::PeriodUpdated,
        Some(auth.user_id),
        None,
        serde_json::json!({
            "period_id": id,
            "period_number": row.period_number,
            "rule_type": validated.rule_type.as_str(),
            "base_amount": period_model::canonical_amount(validated.base_amount),
            "assessment_effective_date": validated.assessment_effective_date.to_string(),
        }),
    )
    .await;
    Ok(Json(PeriodDetailDto {
        item: period_item(&row),
        created_at: row.created_at,
    }))
}

pub async fn delete_period(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let pool = periods_mutation!(
        &state,
        &headers,
        &auth,
        &Method::DELETE,
        authz::catalog::PERIODS_MANAGE
    );
    let period_number = period_repo::delete_period(pool, id)
        .await
        .map_err(command_error)?;
    audit::record(
        pool,
        SecurityEventType::PeriodDeleted,
        Some(auth.user_id),
        None,
        serde_json::json!({ "period_id": id, "period_number": period_number }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Explicit `open -> closed` transition (docs/16): ordinary mutation is
/// locked; obligations remain readable forever.
pub async fn close_period(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<PeriodDetailDto>, ApiError> {
    let pool = periods_mutation!(
        &state,
        &headers,
        &auth,
        &Method::POST,
        authz::catalog::PERIODS_MANAGE
    );
    let period_number = period_repo::close_period(pool, id, state.auth.clock.now())
        .await
        .map_err(command_error)?;
    audit::record(
        pool,
        SecurityEventType::PeriodClosed,
        Some(auth.user_id),
        None,
        serde_json::json!({ "period_id": id, "period_number": period_number }),
    )
    .await;
    tracing::info!(outcome = "period_closed", id = %id, number = period_number, actor = %auth.user_id);
    let row = period_repo::find_period(pool, id)
        .await
        .map_err(db_err("period reload failed"))?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(PeriodDetailDto {
        item: period_item(&row),
        created_at: row.created_at,
    }))
}

/// POST preview (§63): read-only command — CSRF-guarded like every
/// mutation because it reveals financial population data, but it
/// persists nothing and never mutates state.
pub async fn preview_assessments(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<PreviewResponse>, ApiError> {
    let pool = periods_mutation!(
        &state,
        &headers,
        &auth,
        &Method::POST,
        authz::catalog::ASSESSMENTS_READ
    );
    let (page, page_size) = page_of(&query)?;
    let plan = period_repo::preview_assessments(pool, id)
        .await
        .map_err(command_error)?;

    let total_rows = plan.rows.len() as i64;
    let start = ((page - 1) * page_size).max(0) as usize;
    let end = (start + page_size as usize).min(plan.rows.len());
    let rows = plan.rows[start.min(plan.rows.len())..end]
        .iter()
        .map(|(shareholder, share_count, amount)| PreviewRowDto {
            shareholder: identity_dto(
                Some(shareholder.shareholder_id),
                &Some(shareholder.first_name.clone()),
                &Some(shareholder.last_name.clone()),
                &shareholder.guardian_first_name,
                &shareholder.guardian_last_name,
                shareholder.family_sequence,
                Some(shareholder.shareholder_status.as_str()),
            )
            .expect("preview row always carries a shareholder"),
            share_count: *share_count,
            amount: period_model::canonical_amount(*amount),
        })
        .collect();

    Ok(Json(PreviewResponse {
        period_id: id,
        rule_type: plan.rule_type.as_str().to_owned(),
        base_amount: period_model::canonical_amount(plan.base_amount),
        currency: "TRY".to_owned(),
        assessment_effective_date: plan.effective_date,
        eligible_shareholder_count: total_rows,
        eligible_share_count: plan.eligible_share_count,
        assessment_count: total_rows,
        total_amount: period_model::canonical_amount(plan.total_amount),
        page,
        page_size,
        total_rows,
        rows,
    }))
}

/// POST finalization (§30–§33): atomic, idempotent obligation
/// generation. A second attempt answers 409.
pub async fn generate_assessments(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let pool = periods_mutation!(
        &state,
        &headers,
        &auth,
        &Method::POST,
        authz::catalog::ASSESSMENTS_MANAGE
    );
    let summary = period_repo::generate_assessments(pool, auth.user_id, id)
        .await
        .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::AssessmentsGenerated,
        Some(auth.user_id),
        None,
        serde_json::json!({
            "period_id": id,
            "assessment_count": summary.assessment_count,
            "share_source_count": summary.share_source_count,
            "total_amount": period_model::canonical_amount(summary.total_amount),
            "credit_applied_amount":
                period_model::canonical_amount(summary.credit_applied_amount),
            "credit_application_count": summary.credit_application_count,
        }),
    )
    .await;
    tracing::info!(
        outcome = "assessments_generated", id = %id,
        assessments = summary.assessment_count, actor = %auth.user_id
    );

    let row = period_repo::find_period(pool, id)
        .await
        .map_err(db_err("period reload failed"))?
        .ok_or(ApiError::Internal)?;
    Ok((
        StatusCode::CREATED,
        Json(PeriodDetailDto {
            item: period_item(&row),
            created_at: row.created_at,
        }),
    )
        .into_response())
}

pub async fn list_assessments(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(period_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<PaginatedDto<AssessmentListItemDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::ASSESSMENTS_READ).await?;
    let pool = pool_of(&state)?;
    period_repo::find_period(pool, period_id)
        .await
        .map_err(db_err("period load failed"))?
        .ok_or(ApiError::NotFound)?;
    let (page, page_size) = page_of(&query)?;
    let rows =
        period_repo::list_assessments(pool, period_id, query.search.as_deref(), page, page_size)
            .await
            .map_err(db_err("assessment listing failed"))?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    Ok(Json(PaginatedDto {
        items: rows.iter().map(assessment_item).collect(),
        page,
        page_size,
        total_count,
    }))
}

pub async fn get_assessment(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<AssessmentDetailDto>, ApiError> {
    authz::require(&state, &auth, authz::catalog::ASSESSMENTS_READ).await?;
    let pool = pool_of(&state)?;
    let Some(row) = period_repo::find_assessment(pool, id)
        .await
        .map_err(db_err("assessment load failed"))?
    else {
        return Err(ApiError::NotFound);
    };
    let sources = period_repo::assessment_sources(pool, id)
        .await
        .map_err(db_err("assessment sources failed"))?
        .into_iter()
        .map(|s| AssessmentSourceDto {
            share_id: s.share_id,
            share_number: s.share_number,
            amount_component: period_model::canonical_amount(s.amount_component),
            ownership_started_at: s.ownership_started_at,
        })
        .collect();
    Ok(Json(AssessmentDetailDto {
        item: assessment_item(&row),
        sources,
    }))
}

/// Shareholder-scoped obligation history (§62): the "Hissedar
/// yükümlülük görünümü" surface — never recomputed from current
/// ownership.
pub async fn list_shareholder_assessments(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(shareholder_id): Path<Uuid>,
) -> Result<Json<Vec<ShareholderAssessmentDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::ASSESSMENTS_READ).await?;
    let pool = pool_of(&state)?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM shareholders WHERE id = $1)")
            .bind(shareholder_id)
            .fetch_one(pool)
            .await
            .map_err(db_err("shareholder check failed"))?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    let items = period_repo::list_assessments_by_shareholder(pool, shareholder_id)
        .await
        .map_err(db_err("shareholder assessments failed"))?
        .iter()
        .map(|r| ShareholderAssessmentDto {
            id: r.id,
            period_id: r.period_id,
            period_number: r.period_number,
            period_name: r.period_name.clone(),
            due_date: r.due_date,
            rule_type: r.rule_type.clone(),
            amount: period_model::canonical_amount(r.amount),
            currency: r.currency.clone(),
            share_count: r.share_count,
            generated_at: r.generated_at,
        })
        .collect();
    Ok(Json(items))
}
