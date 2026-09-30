//! Shareholder Credit API (STEP-009, docs/06 §"Existing/New excess",
//! docs/05 §"Automatic use of existing excess", docs/19).
//!
//! - `GET  /api/shareholders/{id}/credits`                (credits.read)
//! - `GET  /api/payments/{id}/credits`                    (credits.read)
//! - `POST /api/payments/{id}/credits`                    (credits.manage)
//! - `POST /api/credits/{id}/reverse`                     (credits.manage)
//! - `GET  /api/assessments/{id}/credit-applications`     (credits.read)
//! - `POST /api/assessments/{id}/credit-applications`     (credits.manage)
//! - `POST /api/credit-applications/{id}/reverse`         (credits.manage)
//!
//! There is deliberately NO endpoint that edits a credit balance —
//! availability is always derived. No endpoint creates cash: assigning
//! a credit only re-attributes an already-posted Payment remainder,
//! and applying credit settles debt without an Account Movement.
//! Money fields are decimal STRINGS (ADR-004).

use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
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
use crate::payments::model as payment_model;

use super::model as credit_model;
use super::repo as credit_repo;
use credit_repo::CreditCommandError;

pub fn credits_router() -> Router<AppState> {
    Router::new()
        .route("/api/shareholders/{id}/credits", get(shareholder_credits))
        .route(
            "/api/payments/{id}/credits",
            get(payment_credits).post(assign_credit),
        )
        .route("/api/credits/{id}/reverse", post(reverse_credit))
        .route(
            "/api/assessments/{id}/credit-applications",
            get(assessment_applications).post(apply_credit),
        )
        .route(
            "/api/credit-applications/{id}/reverse",
            post(reverse_application),
        )
        .layer(axum::middleware::from_fn(
            crate::auth::routes::no_store_cache_control,
        ))
}

fn pool_of(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state.db.as_ref().ok_or(ApiError::DependencyUnavailable)
}

/// Credit mutations share this prologue (CSRF layers + permission).
macro_rules! credits_mutation {
    ($state:expr, $headers:expr, $auth:expr, $method:expr) => {{
        csrf::validate_origin($headers, $method, &$state.auth.config.allowed_origins)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        csrf::validate_csrf_token($headers, &$auth.csrf_token)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        authz::require($state, $auth, authz::catalog::CREDITS_MANAGE).await?;
        pool_of($state)?
    }};
}

fn db_err(context: &'static str) -> impl Fn(sqlx::Error) -> ApiError {
    move |error| {
        tracing::error!(error = %error, context);
        ApiError::Internal
    }
}

fn command_error(error: CreditCommandError) -> ApiError {
    use CreditCommandError::*;
    match error {
        NotFound => ApiError::NotFound,
        // Deterministic request errors — same request always fails.
        BeneficiaryInvalid | InactiveAssessment => ApiError::ValidationFailed,
        // Race-sensitive or domain-state rejections — 409 (docs/21).
        PaymentNotPosted
        | InsufficientRemainder
        | AssessmentSettled
        | InsufficientCredit
        | IdempotencyConflict
        | Consumed => ApiError::Conflict,
        IdempotencyRace => ApiError::Internal,
        Database(error) => {
            tracing::error!(error = %error, "credit command failed");
            ApiError::Internal
        }
    }
}

// ------------------------------------------------------------------
// DTOs (mirror @kooperatif/contracts/src/credits.ts)
// ------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditDto {
    pub id: Uuid,
    pub credit_number: i64,
    pub source_payment_id: Uuid,
    pub payment_number: i64,
    pub shareholder_id: Uuid,
    pub shareholder_name: String,
    /// The entitlement created from the Payment remainder.
    pub amount: String,
    /// Derived: SUM of ACTIVE applications against this credit.
    pub applied_amount: String,
    /// Derived: amount - appliedAmount (0 once reversed). Never stored.
    pub available_amount: String,
    pub currency: String,
    pub status: String,
    pub note: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
    pub created_by_name: Option<String>,
}

fn credit_dto(row: &credit_model::CreditRow) -> CreditDto {
    CreditDto {
        id: row.id,
        credit_number: row.credit_number,
        source_payment_id: row.source_payment_id,
        payment_number: row.payment_number,
        shareholder_id: row.shareholder_id,
        shareholder_name: format!(
            "{} {}",
            row.shareholder_first_name, row.shareholder_last_name
        ),
        amount: payment_model::canonical_amount(row.amount),
        applied_amount: payment_model::canonical_amount(row.applied_amount),
        available_amount: payment_model::canonical_amount(row.available_amount),
        currency: "TRY".to_string(),
        status: row.status.clone(),
        note: row.note.clone(),
        created_at: row.created_at,
        reversed_at: row.reversed_at,
        reversal_reason: row.reversal_reason.clone(),
        created_by_name: row.created_by_name.clone(),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationDto {
    pub id: Uuid,
    pub credit_id: Uuid,
    pub credit_number: i64,
    pub assessment_id: Uuid,
    pub period_name: String,
    pub amount: String,
    /// `automatic` (generation/assignment engine) | `manual`.
    pub mode: String,
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
    pub created_by_name: Option<String>,
}

fn application_dto(row: &credit_model::ApplicationRow) -> ApplicationDto {
    ApplicationDto {
        id: row.id,
        credit_id: row.credit_id,
        credit_number: row.credit_number,
        assessment_id: row.assessment_id,
        period_name: row.period_name.clone(),
        amount: payment_model::canonical_amount(row.amount),
        mode: row.mode.clone(),
        status: row.status.clone(),
        created_at: row.created_at,
        reversed_at: row.reversed_at,
        reversal_reason: row.reversal_reason.clone(),
        created_by_name: row.created_by_name.clone(),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditSummaryDto {
    pub credit_count: i64,
    pub total_originated: String,
    pub total_applied: String,
    /// Σ active credits − Σ active applications — derived, never stored.
    pub available: String,
    pub currency: String,
}

/// Shareholder credit ledger (docs/13 "Fazla Ödemeler"): origins,
/// derived summary and application history in one surface.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareholderCreditsDto {
    pub summary: CreditSummaryDto,
    pub credits: Vec<CreditDto>,
    pub applications: Vec<ApplicationDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignCreditResponse {
    /// True when the idempotency key replayed an identical earlier
    /// assignment — the returned Credit already existed (HTTP 200).
    pub replayed: bool,
    pub credit_id: Uuid,
    pub credit_number: i64,
    /// Total auto-offset onto the beneficiary's open assessments
    /// applied inside the same transaction.
    pub applied_amount: String,
    pub application_count: i64,
    /// Complete post-command credit ledger of the beneficiary.
    pub ledger: ShareholderCreditsDto,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyCreditResponse {
    pub replayed: bool,
    pub applied_amount: String,
    /// Derived remaining debt after the application.
    pub assessment_remaining: String,
}

// ------------------------------------------------------------------
// Requests
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignCreditRequest {
    /// Explicit beneficiary Shareholder — NEVER implied by payer,
    /// guardian or family (docs/06 §"New excess").
    pub shareholder_id: Uuid,
    /// Decimal string ≤ the Payment's unassigned remainder.
    pub amount: String,
    #[serde(default)]
    pub note: Option<String>,
    pub idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyCreditRequest {
    /// Decimal string ≤ the assessment's remaining AND the
    /// beneficiary's available credit.
    pub amount: String,
    pub idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReverseRequest {
    /// Required — docs/19 reversal contract: actor + time + reason.
    pub reversal_reason: String,
}

// ------------------------------------------------------------------
// Handlers
// ------------------------------------------------------------------

/// Shareholder credit ledger: derived summary + origins + application
/// history. The Shareholder is the ONLY owner dimension — a Family
/// balance is never derived here.
pub async fn shareholder_credits(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(shareholder_id): Path<Uuid>,
) -> Result<Json<ShareholderCreditsDto>, ApiError> {
    authz::require(&state, &auth, authz::catalog::CREDITS_READ).await?;
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
    load_shareholder_ledger(pool, shareholder_id)
        .await
        .map(Json)
}

async fn load_shareholder_ledger(
    pool: &sqlx::PgPool,
    shareholder_id: Uuid,
) -> Result<ShareholderCreditsDto, ApiError> {
    let summary = credit_repo::shareholder_credit_summary(pool, shareholder_id)
        .await
        .map_err(db_err("credit summary failed"))?;
    let credits = credit_repo::list_shareholder_credits(pool, shareholder_id)
        .await
        .map_err(db_err("credit list failed"))?;
    let applications = credit_repo::list_shareholder_applications(pool, shareholder_id)
        .await
        .map_err(db_err("credit applications failed"))?;
    Ok(ShareholderCreditsDto {
        summary: CreditSummaryDto {
            credit_count: summary.credit_count,
            total_originated: payment_model::canonical_amount(summary.total_originated),
            total_applied: payment_model::canonical_amount(summary.total_applied),
            available: payment_model::canonical_amount(summary.available),
            currency: "TRY".to_string(),
        },
        credits: credits.iter().map(credit_dto).collect(),
        applications: applications.iter().map(application_dto).collect(),
    })
}

/// Credit origins sourced from one Payment (disposition surface).
pub async fn payment_credits(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(payment_id): Path<Uuid>,
) -> Result<Json<Vec<CreditDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::CREDITS_READ).await?;
    let pool = pool_of(&state)?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM payments WHERE id = $1)")
        .bind(payment_id)
        .fetch_one(pool)
        .await
        .map_err(db_err("payment check failed"))?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    let rows = credit_repo::list_payment_credits(pool, payment_id)
        .await
        .map_err(db_err("payment credits failed"))?;
    Ok(Json(rows.iter().map(credit_dto).collect()))
}

/// Application history against one Assessment — kept strictly
/// separate from `payment_allocations` (docs/06: two different kinds
/// of settlement, never blended into one line list).
pub async fn assessment_applications(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(assessment_id): Path<Uuid>,
) -> Result<Json<Vec<ApplicationDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::CREDITS_READ).await?;
    let pool = pool_of(&state)?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM assessments WHERE id = $1)")
        .bind(assessment_id)
        .fetch_one(pool)
        .await
        .map_err(db_err("assessment check failed"))?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    let rows = credit_repo::list_assessment_applications(pool, assessment_id)
        .await
        .map_err(db_err("assessment applications failed"))?;
    Ok(Json(rows.iter().map(application_dto).collect()))
}

/// Explicit remainder attribution (docs/06 §"New excess"): assigns
/// part of a posted Payment's unassigned remainder as a Shareholder
/// Credit, then auto-offsets the beneficiary's existing open
/// Assessments — all inside ONE transaction. No cash is created and
/// no Account Movement is produced.
pub async fn assign_credit(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(payment_id): Path<Uuid>,
    Json(body): Json<AssignCreditRequest>,
) -> Result<Response, ApiError> {
    let pool = credits_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();

    let amount =
        credit_model::validate_amount(&body.amount).map_err(|_| ApiError::ValidationFailed)?;
    let note = credit_model::validate_note(body.note.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let key = credit_model::validate_idempotency_key(&body.idempotency_key)
        .map_err(|_| ApiError::ValidationFailed)?;
    let fingerprint = credit_model::assignment_fingerprint(
        payment_id,
        body.shareholder_id,
        amount,
        note.as_deref(),
    );

    let outcome = credit_repo::assign_credit(
        pool,
        auth.user_id,
        credit_repo::AssignCredit {
            payment_id,
            shareholder_id: body.shareholder_id,
            amount,
            note,
            idempotency_key: key,
            fingerprint,
        },
        now,
    )
    .await
    .map_err(command_error)?;

    if !outcome.replayed {
        audit::record(
            pool,
            SecurityEventType::ShareholderCreditAssigned,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "credit_id": outcome.credit_id,
                "credit_number": outcome.credit_number,
                "source_payment_id": payment_id,
                "shareholder_id": body.shareholder_id,
                "amount": payment_model::canonical_amount(amount),
                "applied_amount": payment_model::canonical_amount(outcome.applied_amount),
                "application_count": outcome.application_count,
            }),
        )
        .await;
        if outcome.application_count > 0 {
            audit::record(
                pool,
                SecurityEventType::CreditApplied,
                Some(auth.user_id),
                None,
                serde_json::json!({
                    "mode": "automatic",
                    "trigger": "credit_assignment",
                    "credit_id": outcome.credit_id,
                    "shareholder_id": body.shareholder_id,
                    "applied_amount":
                        payment_model::canonical_amount(outcome.applied_amount),
                    "application_count": outcome.application_count,
                }),
            )
            .await;
        }
        tracing::info!(
            outcome = "shareholder_credit_assigned", id = %outcome.credit_id,
            number = outcome.credit_number, actor = %auth.user_id
        );
    }

    let ledger = load_shareholder_ledger(pool, body.shareholder_id).await?;
    Ok((
        if outcome.replayed {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        },
        Json(AssignCreditResponse {
            replayed: outcome.replayed,
            credit_id: outcome.credit_id,
            credit_number: outcome.credit_number,
            applied_amount: payment_model::canonical_amount(outcome.applied_amount),
            application_count: outcome.application_count,
            ledger,
        }),
    )
        .into_response())
}

/// Manual apply-credit command (docs/05): settles part of an ACTIVE
/// Assessment from ITS OWN Shareholder's available credit. Same engine
/// as the automatic path — never a second arithmetic.
pub async fn apply_credit(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(assessment_id): Path<Uuid>,
    Json(body): Json<ApplyCreditRequest>,
) -> Result<Response, ApiError> {
    let pool = credits_mutation!(&state, &headers, &auth, &Method::POST);

    let amount =
        credit_model::validate_amount(&body.amount).map_err(|_| ApiError::ValidationFailed)?;
    let key = credit_model::validate_idempotency_key(&body.idempotency_key)
        .map_err(|_| ApiError::ValidationFailed)?;
    let fingerprint = credit_model::application_fingerprint(assessment_id, amount);

    let outcome = credit_repo::apply_credit(
        pool,
        auth.user_id,
        credit_repo::ApplyCredit {
            assessment_id,
            amount,
            idempotency_key: key,
            fingerprint,
        },
    )
    .await
    .map_err(command_error)?;

    if !outcome.replayed {
        audit::record(
            pool,
            SecurityEventType::CreditApplied,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "mode": "manual",
                "assessment_id": assessment_id,
                "applied_amount": payment_model::canonical_amount(outcome.applied_amount),
                "assessment_remaining":
                    payment_model::canonical_amount(outcome.assessment_remaining),
            }),
        )
        .await;
        tracing::info!(
            outcome = "credit_applied", assessment = %assessment_id,
            actor = %auth.user_id
        );
    }

    Ok((
        if outcome.replayed {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        },
        Json(ApplyCreditResponse {
            replayed: outcome.replayed,
            applied_amount: payment_model::canonical_amount(outcome.applied_amount),
            assessment_remaining: payment_model::canonical_amount(outcome.assessment_remaining),
        }),
    )
        .into_response())
}

/// Credit-origin reversal (docs/19): the original row stays, marked
/// `reversed` with actor/time/reason. Rejected while the credit still
/// funds active applications — reverse those first.
pub async fn reverse_credit(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(credit_id): Path<Uuid>,
    Json(body): Json<ReverseRequest>,
) -> Result<Response, ApiError> {
    let pool = credits_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let reason = credit_model::validate_reversal_reason(&body.reversal_reason)
        .map_err(|_| ApiError::ValidationFailed)?;

    let replayed = credit_repo::reverse_credit(pool, auth.user_id, credit_id, &reason, now)
        .await
        .map_err(command_error)?;

    if !replayed {
        audit::record(
            pool,
            SecurityEventType::ShareholderCreditReversed,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "credit_id": credit_id,
                "reversal_reason": reason,
            }),
        )
        .await;
        tracing::info!(
            outcome = "shareholder_credit_reversed", id = %credit_id,
            actor = %auth.user_id
        );
    }

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Application reversal (docs/19): the original application survives
/// marked `reversed`; the Assessment's derived debt reopens and the
/// credit's derived availability returns. Never cash, never a Movement.
pub async fn reverse_application(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(application_id): Path<Uuid>,
    Json(body): Json<ReverseRequest>,
) -> Result<Response, ApiError> {
    let pool = credits_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let reason = credit_model::validate_reversal_reason(&body.reversal_reason)
        .map_err(|_| ApiError::ValidationFailed)?;

    let replayed =
        credit_repo::reverse_application(pool, auth.user_id, application_id, &reason, now)
            .await
            .map_err(command_error)?;

    if !replayed {
        audit::record(
            pool,
            SecurityEventType::CreditApplicationReversed,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "application_id": application_id,
                "reversal_reason": reason,
            }),
        )
        .await;
        tracing::info!(
            outcome = "credit_application_reversed", id = %application_id,
            actor = %auth.user_id
        );
    }

    Ok(StatusCode::NO_CONTENT.into_response())
}
