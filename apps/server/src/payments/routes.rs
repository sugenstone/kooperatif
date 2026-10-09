//! Payments / Tahsilat API (STEP-007, docs/06).
//!
//! - `GET  /api/payments`                                (payments.read)
//! - `POST /api/payments`                                (payments.manage; atomic posted Payment + allocations)
//! - `GET  /api/payments/payer-persons`                  (payments.read)
//! - `GET  /api/payments/{id}`                           (payments.read)
//! - `POST /api/payments/{id}/allocations`               (payments.manage; unallocated -> obligations)
//! - `POST /api/payments/{id}/allocations/{aid}/reverse` (payments.manage)
//! - `POST /api/payments/{id}/reverse`                   (payments.manage; full reversal)
//! - `GET  /api/assessments/{id}/payments`               (payments.read)
//! - `GET  /api/shareholders/{id}/open-assessments`      (payments.read)
//! - `GET  /api/shareholders/{id}/financial-summary`     (payments.read)
//! - `GET  /api/periods/{id}/financial-summary`          (payments.read)
//! - `GET  /api/families/{id}/collection-context`        (payments.read)
//!
//! Deliberately absent: Cashbox, Bank Account, Ledger, Receipt,
//! Collection Session and refund/disbursement surfaces. `amount`
//! fields are decimal STRINGS (ADR-004). Every mutation carries both
//! CSRF layers; derived balances are recomputed inside PostgreSQL
//! transactions under deterministic row locks (ADR-006).

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::auth::audit::{self, SecurityEventType};
use crate::auth::authz;
use crate::auth::csrf;
use crate::auth::extractor::CurrentAuth;
use crate::auth::routes::csrf_rejection_to_api_error;
use crate::credits::repo as credit_repo;
use crate::http::error::ApiError;
use crate::http::AppState;
use crate::parties::repo as party_repo;
use crate::parties::routes::{page_of, ListQuery, PaginatedDto};
use crate::shares::routes::{identity_dto, ShareholderIdentityDto};

use super::model as payment_model;
use super::repo as payment_repo;

pub fn payments_router() -> Router<AppState> {
    Router::new()
        .route("/api/payments", get(list_payments).post(create_payment))
        .route("/api/payments/payer-persons", get(payer_persons))
        .route("/api/payments/{id}", get(get_payment))
        .route("/api/payments/{id}/allocations", post(add_allocations))
        .route(
            "/api/payments/{id}/allocations/{allocation_id}/reverse",
            post(reverse_allocation),
        )
        .route("/api/payments/{id}/reverse", post(reverse_payment))
        .route("/api/assessments/{id}/payments", get(assessment_payments))
        .route(
            "/api/shareholders/{id}/open-assessments",
            get(shareholder_open_assessments),
        )
        .route(
            "/api/shareholders/{id}/financial-summary",
            get(shareholder_financial_summary),
        )
        .route(
            "/api/periods/{id}/financial-summary",
            get(period_financial_summary),
        )
        .route(
            "/api/families/{id}/collection-context",
            get(family_collection_context),
        )
        .layer(axum::middleware::from_fn(
            crate::auth::routes::no_store_cache_control,
        ))
}

fn pool_of(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state.db.as_ref().ok_or(ApiError::DependencyUnavailable)
}

/// Payment mutations share this prologue (CSRF layers + permission).
macro_rules! payments_mutation {
    ($state:expr, $headers:expr, $auth:expr, $method:expr) => {{
        csrf::validate_origin($headers, $method, &$state.auth.config.allowed_origins)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        csrf::validate_csrf_token($headers, &$auth.csrf_token)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        authz::require($state, $auth, authz::catalog::PAYMENTS_MANAGE).await?;
        pool_of($state)?
    }};
}

fn db_err(context: &'static str) -> impl Fn(sqlx::Error) -> ApiError {
    move |error| {
        tracing::error!(error = %error, context);
        ApiError::Internal
    }
}

fn command_error(error: payment_repo::PaymentCommandError) -> ApiError {
    use payment_repo::PaymentCommandError::*;
    match error {
        NotFound => ApiError::NotFound,
        // STEP-008: a destination account that does not exist or is not
        // usable is indistinguishable — no existence leak (IDOR rule).
        AccountNotFoundOrInactive => ApiError::NotFound,
        // Deterministic request errors — the same request would always
        // fail regardless of concurrent state.
        PayerNotFound | DuplicateTarget | PaymentOverAllocated => ApiError::ValidationFailed,
        // Race-sensitive or domain-state rejections — 409 (docs/21).
        InactiveAssessment
        | TargetAlreadyAllocated
        | AssessmentOverAllocated
        | IdempotencyConflict
        | NotPosted
        | AccountEffectBlocked
        | CreditEffectBlocked => ApiError::Conflict,
        Database(error) => {
            tracing::error!(error = %error, "payment command failed");
            ApiError::Internal
        }
    }
}

// ------------------------------------------------------------------
// DTOs (mirror @kooperatif/contracts/src/payments.ts)
// ------------------------------------------------------------------

/// Who handed over the value — a Person (never implicitly a debtor).
/// `shareholderId` is context when the same Person is a Shareholder.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayerDto {
    pub person_id: Uuid,
    pub full_name: String,
    pub shareholder_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentListItemDto {
    pub id: Uuid,
    pub payment_number: i64,
    pub status: String,
    pub payer: PayerDto,
    pub amount: String,
    /// Derived: SUM of ACTIVE allocations — never a stored flag.
    pub allocated_amount: String,
    /// STEP-009 derived: SUM of ACTIVE Shareholder Credits sourced
    /// from this Payment's remainder ("Fazla Ödeme" disposition).
    pub credited_amount: String,
    /// amount - allocatedAmount - creditedAmount; >= 0 by command
    /// invariant. Value held on the Payment awaiting explicit
    /// disposition (docs/06) — NEVER implied to belong to anyone.
    pub unallocated_amount: String,
    pub currency: String,
    pub method: String,
    #[serde(with = "time::serde::rfc3339")]
    pub received_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllocationDto {
    pub id: Uuid,
    pub assessment_id: Uuid,
    pub period_id: Uuid,
    pub period_number: i64,
    pub period_name: String,
    /// Canonical debtor identity — the obligation's Shareholder,
    /// distinct from the payer when a third party pays (§12–§14).
    pub debtor: ShareholderIdentityDto,
    /// Full obligation amount for context.
    pub assessment_amount: String,
    pub amount: String,
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentDetailDto {
    #[serde(flatten)]
    pub item: PaymentListItemDto,
    /// STEP-008: WHERE the received value is held — separate from
    /// `method` (HOW it arrived). NULL only on pre-STEP-008 rows.
    pub destination_account_id: Option<Uuid>,
    pub destination_account_name: Option<String>,
    pub note: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
    /// Audit surface: display names of the recording operator and the
    /// reversing actor (docs/19).
    pub created_by_name: Option<String>,
    pub reversed_by_name: Option<String>,
    /// Complete history — active AND reversed lines.
    pub allocations: Vec<AllocationDto>,
    /// STEP-009: Shareholder Credits sourced from this Payment's
    /// remainder ("Fazla Ödeme" disposition) — each names its explicit
    /// beneficiary Shareholder; a credit is never a Movement.
    pub credits: Vec<PaymentCreditDto>,
}

/// A Payment-remainder disposition line (STEP-009): the explicitly
/// chosen beneficiary Shareholder and the credit's derived state.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentCreditDto {
    pub id: Uuid,
    pub credit_number: i64,
    pub shareholder_id: Uuid,
    pub shareholder_name: String,
    pub amount: String,
    pub applied_amount: String,
    pub available_amount: String,
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePaymentResponse {
    /// True when the idempotency key replayed an identical earlier
    /// request — the returned Payment already existed (HTTP 200).
    pub replayed: bool,
    pub payment: PaymentDetailDto,
}

/// One application of received value on an Assessment (payment history
/// surface): which Payment, which payer, how much, active or reversed.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssessmentPaymentDto {
    pub allocation_id: Uuid,
    pub payment_id: Uuid,
    pub payment_number: i64,
    pub payment_status: String,
    pub amount: String,
    pub currency: String,
    pub allocation_status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub received_at: OffsetDateTime,
    pub payer_full_name: String,
    pub payer_shareholder_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
}

/// An obligation with its derived settlement state for debtor
/// selection and shareholder payment views.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAssessmentDto {
    pub id: Uuid,
    pub period_id: Uuid,
    pub period_number: i64,
    pub period_name: String,
    #[serde(with = "date_iso")]
    pub due_date: Date,
    pub amount: String,
    pub currency: String,
    pub paid_amount: String,
    pub remaining_amount: String,
    #[serde(with = "time::serde::rfc3339")]
    pub generated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FinancialSummaryDto {
    pub assessment_count: i64,
    pub open_assessment_count: i64,
    pub total_assessed: String,
    pub total_paid: String,
    pub total_remaining: String,
    pub currency: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeriodFinancialSummaryDto {
    pub period_id: Uuid,
    pub assessment_count: i64,
    pub total_assessed: String,
    pub total_collected: String,
    pub total_remaining: String,
    pub contributing_payment_count: i64,
    pub currency: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FamilyMemberContextDto {
    pub member: ShareholderIdentityDto,
    pub open_assessment_count: i64,
    pub remaining_amount: String,
    /// This member's OWN derived credit — attribution surface only;
    /// never an interchangeable family pool.
    pub credit_available: String,
    /// FUNC-FIX-002: member-level informational preference only — the
    /// family payment NEVER infers a receiving account from it.
    pub default_account: Option<crate::parties::routes::DefaultAccountDto>,
}

/// Family collection context (docs/06 §26): member-wise obligations —
/// the Family itself is NEVER the debtor and no aggregate debt row
/// exists.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FamilyCollectionContextDto {
    pub family_id: Uuid,
    pub sequence_number: i64,
    pub members: Vec<FamilyMemberContextDto>,
    pub total_remaining: String,
    pub currency: String,
}

/// Person candidate for payer selection — same-name persons carry
/// shareholder context so the operator can disambiguate.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayerCandidateDto {
    pub person_id: Uuid,
    pub full_name: String,
    pub shareholder_id: Option<Uuid>,
    pub shareholder_status: Option<String>,
    /// FUNC-FIX-002: when the candidate is itself a shareholder, its
    /// default collection account (with live status) travels along —
    /// the payment form uses the DEBTOR's preference, never the
    /// payer's, and only for a single-debtor payment.
    pub default_account: Option<crate::parties::routes::DefaultAccountDto>,
}

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
}

fn payer_dto(row: &payment_repo::PaymentRow) -> PayerDto {
    PayerDto {
        person_id: row.payer_person_id,
        full_name: format!("{} {}", row.payer_first_name, row.payer_last_name),
        shareholder_id: row.payer_shareholder_id,
    }
}

fn payment_item(row: &payment_repo::PaymentRow) -> PaymentListItemDto {
    PaymentListItemDto {
        id: row.id,
        payment_number: row.payment_number,
        status: row.status.clone(),
        payer: payer_dto(row),
        amount: payment_model::canonical_amount(row.amount),
        allocated_amount: payment_model::canonical_amount(row.allocated_amount),
        credited_amount: payment_model::canonical_amount(row.credited_amount),
        unallocated_amount: payment_model::canonical_amount(
            row.amount - row.allocated_amount - row.credited_amount,
        ),
        currency: row.currency.clone(),
        method: row.method.clone(),
        received_at: row.received_at,
        created_at: row.created_at,
    }
}

fn allocation_dto(row: &payment_repo::AllocationRow) -> AllocationDto {
    AllocationDto {
        id: row.id,
        assessment_id: row.assessment_id,
        period_id: row.period_id,
        period_number: row.period_number,
        period_name: row.period_name.clone(),
        debtor: identity_dto(
            Some(row.debtor.shareholder_id),
            &Some(row.debtor.first_name.clone()),
            &Some(row.debtor.last_name.clone()),
            &row.debtor.guardian_first_name,
            &row.debtor.guardian_last_name,
            row.debtor.family_sequence,
            Some(row.debtor.shareholder_status.as_str()),
        )
        .expect("allocation always joins its debtor shareholder"),
        assessment_amount: payment_model::canonical_amount(row.assessment_amount),
        amount: payment_model::canonical_amount(row.amount),
        status: row.status.clone(),
        created_at: row.created_at,
        reversed_at: row.reversed_at,
        reversal_reason: row.reversal_reason.clone(),
    }
}

// ------------------------------------------------------------------
// Requests
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllocationRequest {
    pub assessment_id: Uuid,
    pub amount: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePaymentRequest {
    /// Payer: an existing Person id XOR a new (firstName + lastName).
    pub payer_person_id: Option<Uuid>,
    pub payer_first_name: Option<String>,
    pub payer_last_name: Option<String>,
    pub amount: String,
    pub method: String,
    /// Optional RFC3339 — defaults to server now; backdating allowed,
    /// future rejected.
    pub received_at: Option<String>,
    pub note: Option<String>,
    /// STEP-008: required — every new Payment posts into exactly one
    /// Financial Account. Only rows predating the domain keep NULL.
    pub destination_account_id: Uuid,
    pub idempotency_key: String,
    /// May be EMPTY: the whole amount then stays unallocated on the
    /// Payment awaiting explicit disposition (never auto-attributed).
    pub allocations: Vec<AllocationRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddAllocationsRequest {
    pub allocations: Vec<AllocationRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReverseRequest {
    /// Required non-empty reason (docs/19).
    pub reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayerQuery {
    pub search: String,
    pub limit: Option<i64>,
}

struct ValidatedPayment {
    payer_person_id: Option<Uuid>,
    payer_first_name: Option<String>,
    payer_last_name: Option<String>,
    amount: Decimal,
    method: payment_model::PaymentMethod,
    received_at: OffsetDateTime,
    /// `Some` only when the client explicitly supplied `receivedAt` —
    /// the idempotency fingerprint hashes INTENT, not the
    /// server-generated default (PILOT-FIX-001 / F4).
    received_at_intent: Option<OffsetDateTime>,
    note: Option<String>,
    destination_account_id: Uuid,
    idempotency_key: String,
    allocations: Vec<payment_repo::NewAllocation>,
}

fn validated_payment(
    payload: CreatePaymentRequest,
    now: OffsetDateTime,
) -> Result<ValidatedPayment, ApiError> {
    // Payer: exactly one resolution path.
    let (payer_person_id, payer_first_name, payer_last_name) = match (
        payload.payer_person_id,
        payload.payer_first_name,
        payload.payer_last_name,
    ) {
        (Some(id), None, None) => (Some(id), None, None),
        (None, Some(first), Some(last)) => {
            let first = first.trim().to_string();
            let last = last.trim().to_string();
            if first.is_empty() || last.is_empty() {
                return Err(ApiError::ValidationFailed);
            }
            (None, Some(first), Some(last))
        }
        _ => return Err(ApiError::ValidationFailed),
    };

    let amount = payment_model::validate_positive_amount(&payload.amount)
        .map_err(|_| ApiError::ValidationFailed)?;
    let method =
        payment_model::PaymentMethod::parse(&payload.method).ok_or(ApiError::ValidationFailed)?;
    let received_at = match &payload.received_at {
        Some(raw) => {
            OffsetDateTime::parse(raw.trim(), &Rfc3339).map_err(|_| ApiError::ValidationFailed)?
        }
        None => now,
    };
    payment_model::validate_received_at(received_at, now)
        .map_err(|_| ApiError::ValidationFailed)?;
    let received_at_intent = payload.received_at.is_some().then_some(received_at);
    let note = payment_model::validate_note(payload.note.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let idempotency_key = payment_model::validate_idempotency_key(&payload.idempotency_key)
        .map_err(|_| ApiError::ValidationFailed)?;

    let mut allocations = Vec::with_capacity(payload.allocations.len());
    for allocation in &payload.allocations {
        let amount = payment_model::validate_positive_amount(&allocation.amount)
            .map_err(|_| ApiError::ValidationFailed)?;
        allocations.push(payment_repo::NewAllocation {
            assessment_id: allocation.assessment_id,
            amount,
        });
    }
    // A single Payment is a real-world receipt: bound the batch so a
    // runaway payload cannot degrade lock acquisition (ADR-006 §6).
    if allocations.len() > 500 {
        return Err(ApiError::ValidationFailed);
    }

    Ok(ValidatedPayment {
        payer_person_id,
        payer_first_name,
        payer_last_name,
        amount,
        method,
        received_at,
        received_at_intent,
        note,
        destination_account_id: payload.destination_account_id,
        idempotency_key,
        allocations,
    })
}

// ------------------------------------------------------------------
// Handlers
// ------------------------------------------------------------------

pub async fn list_payments(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<ListQuery>,
) -> Result<Json<PaginatedDto<PaymentListItemDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::PAYMENTS_READ).await?;
    let pool = pool_of(&state)?;
    let (page, page_size) = page_of(&query)?;
    let rows = payment_repo::list_payments(pool, query.search.as_deref(), page, page_size)
        .await
        .map_err(db_err("payment listing failed"))?;
    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    Ok(Json(PaginatedDto {
        items: rows.iter().map(payment_item).collect(),
        page,
        page_size,
        total_count,
    }))
}

/// Person lookup for payer selection — the payer may be any Person,
/// not only Shareholders (docs/06 §14). payments.read suffices: the
/// collection operator needs to find "who paid" without full parties
/// management rights.
pub async fn payer_persons(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<PayerQuery>,
) -> Result<Json<Vec<PayerCandidateDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::PAYMENTS_READ).await?;
    let pool = pool_of(&state)?;
    if query.search.trim().is_empty() {
        return Ok(Json(Vec::new()));
    }
    let limit = query.limit.unwrap_or(10).clamp(1, 25);
    let rows = party_repo::search_persons(pool, &query.search, limit)
        .await
        .map_err(db_err("payer search failed"))?;
    Ok(Json(
        rows.iter()
            .map(|r| PayerCandidateDto {
                person_id: r.id,
                full_name: format!("{} {}", r.first_name, r.last_name),
                shareholder_id: r.shareholder_id,
                shareholder_status: r.shareholder_status.clone(),
                default_account: r.default_account_id.map(|id| {
                    crate::parties::routes::DefaultAccountDto {
                        id,
                        name: r.default_account_name.clone().unwrap_or_default(),
                        status: r.default_account_status.clone().unwrap_or_default(),
                    }
                }),
            })
            .collect(),
    ))
}

/// THE collection command: ONE atomic transaction persists the posted
/// Payment plus every allocation line (docs/06 §30–§34). Nothing here
/// is two-phase — a Payment without its allocations, or allocations
/// without their Payment, can never exist.
pub async fn create_payment(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Json(body): Json<CreatePaymentRequest>,
) -> Result<Response, ApiError> {
    let pool = payments_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let validated = validated_payment(body, now)?;
    let fingerprint = payment_model::idempotency_fingerprint(&payment_model::PaymentFingerprint {
        payer_person_id: validated.payer_person_id,
        payer_first_name: validated.payer_first_name.as_deref(),
        payer_last_name: validated.payer_last_name.as_deref(),
        amount: validated.amount,
        method: validated.method,
        received_at: validated.received_at_intent,
        note: validated.note.as_deref(),
        destination_account_id: validated.destination_account_id,
        allocations: &validated
            .allocations
            .iter()
            .map(|a| (a.assessment_id, a.amount))
            .collect::<Vec<_>>(),
    });

    let outcome = payment_repo::create_payment(
        pool,
        auth.user_id,
        payment_repo::CreatePayment {
            payer_person_id: validated.payer_person_id,
            payer_first_name: validated.payer_first_name,
            payer_last_name: validated.payer_last_name,
            amount: validated.amount,
            method: validated.method.as_str().to_owned(),
            received_at: validated.received_at,
            note: validated.note.clone(),
            destination_account_id: validated.destination_account_id,
            idempotency_key: validated.idempotency_key,
            fingerprint,
            allocations: validated.allocations,
        },
    )
    .await
    .map_err(command_error)?;

    if !outcome.replayed {
        audit::record(
            pool,
            SecurityEventType::PaymentPosted,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "payment_id": outcome.payment_id,
                "payment_number": outcome.payment_number,
                "payer_person_id": outcome.payer_person_id,
                "created_person": outcome.created_person,
                "amount": payment_model::canonical_amount(validated.amount),
                "method": validated.method.as_str(),
                "destination_account_id": validated.destination_account_id,
                "allocation_count": outcome.allocation_count,
                "allocated_amount": payment_model::canonical_amount(outcome.allocated_amount),
                "unallocated_amount": payment_model::canonical_amount(outcome.unallocated_amount),
            }),
        )
        .await;
        tracing::info!(
            outcome = "payment_posted", id = %outcome.payment_id,
            number = outcome.payment_number,
            allocations = outcome.allocation_count, actor = %auth.user_id
        );
    }

    let detail = load_detail(pool, outcome.payment_id).await?;
    Ok((
        if outcome.replayed {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        },
        Json(CreatePaymentResponse {
            replayed: outcome.replayed,
            payment: detail,
        }),
    )
        .into_response())
}

async fn load_detail(pool: &sqlx::PgPool, id: Uuid) -> Result<PaymentDetailDto, ApiError> {
    let row = payment_repo::find_payment(pool, id)
        .await
        .map_err(db_err("payment reload failed"))?
        .ok_or(ApiError::Internal)?;
    let allocations = payment_repo::payment_allocations(pool, id)
        .await
        .map_err(db_err("payment allocations failed"))?;
    let credits = credit_repo::list_payment_credits(pool, id)
        .await
        .map_err(db_err("payment credits failed"))?;
    Ok(PaymentDetailDto {
        item: payment_item(&row),
        destination_account_id: row.destination_account_id,
        destination_account_name: row.destination_account_name.clone(),
        note: row.note.clone(),
        reversed_at: row.reversed_at,
        reversal_reason: row.reversal_reason.clone(),
        created_by_name: row.created_by_name.clone(),
        reversed_by_name: row.reversed_by_name.clone(),
        allocations: allocations.iter().map(allocation_dto).collect(),
        credits: credits
            .iter()
            .map(|c| PaymentCreditDto {
                id: c.id,
                credit_number: c.credit_number,
                shareholder_id: c.shareholder_id,
                shareholder_name: format!(
                    "{} {}",
                    c.shareholder_first_name, c.shareholder_last_name
                ),
                amount: payment_model::canonical_amount(c.amount),
                applied_amount: payment_model::canonical_amount(c.applied_amount),
                available_amount: payment_model::canonical_amount(c.available_amount),
                status: c.status.clone(),
                created_at: c.created_at,
                reversed_at: c.reversed_at,
                reversal_reason: c.reversal_reason.clone(),
            })
            .collect(),
    })
}

pub async fn get_payment(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<PaymentDetailDto>, ApiError> {
    authz::require(&state, &auth, authz::catalog::PAYMENTS_READ).await?;
    let pool = pool_of(&state)?;
    let exists = payment_repo::find_payment(pool, id)
        .await
        .map_err(db_err("payment load failed"))?
        .ok_or(ApiError::NotFound)?;
    let _ = exists;
    load_detail(pool, id).await.map(Json)
}

/// Apply (part of) the unallocated remainder to further obligations.
/// Explicit disposition — no automatic ordering or attribution.
pub async fn add_allocations(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<AddAllocationsRequest>,
) -> Result<Json<PaymentDetailDto>, ApiError> {
    let pool = payments_mutation!(&state, &headers, &auth, &Method::POST);
    if body.allocations.is_empty() || body.allocations.len() > 500 {
        return Err(ApiError::ValidationFailed);
    }
    let mut allocations = Vec::with_capacity(body.allocations.len());
    for allocation in &body.allocations {
        let amount = payment_model::validate_positive_amount(&allocation.amount)
            .map_err(|_| ApiError::ValidationFailed)?;
        allocations.push(payment_repo::NewAllocation {
            assessment_id: allocation.assessment_id,
            amount,
        });
    }

    let outcome = payment_repo::add_allocations(pool, auth.user_id, id, allocations)
        .await
        .map_err(command_error)?;
    audit::record(
        pool,
        SecurityEventType::PaymentAllocationsAdded,
        Some(auth.user_id),
        None,
        serde_json::json!({
            "payment_id": id,
            "added_allocation_count": outcome.allocation_count,
            "allocated_amount": payment_model::canonical_amount(outcome.allocated_amount),
            "unallocated_amount": payment_model::canonical_amount(outcome.unallocated_amount),
        }),
    )
    .await;
    load_detail(pool, id).await.map(Json)
}

/// Full reversal (docs/19): the Payment and every active allocation
/// are marked reversed — the rows remain as history. Repeat = replay.
pub async fn reverse_payment(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<ReverseRequest>,
) -> Result<Json<PaymentDetailDto>, ApiError> {
    let pool = payments_mutation!(&state, &headers, &auth, &Method::POST);
    let reason = payment_model::validate_reversal_reason(&body.reason)
        .map_err(|_| ApiError::ValidationFailed)?;
    let outcome =
        payment_repo::reverse_payment(pool, auth.user_id, id, reason, state.auth.clock.now())
            .await
            .map_err(command_error)?;
    if !outcome.replayed {
        audit::record(
            pool,
            SecurityEventType::PaymentReversed,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "payment_id": id,
                "payment_number": outcome.payment_number,
                "reversed_allocation_count": outcome.reversed_allocation_count,
                "reason": body.reason.trim(),
            }),
        )
        .await;
        tracing::info!(outcome = "payment_reversed", id = %id, actor = %auth.user_id);
    }
    load_detail(pool, id).await.map(Json)
}

/// Fine-grained correction: reverse ONE allocation line; the rest of
/// the Payment keeps satisfying its other obligations.
pub async fn reverse_allocation(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path((id, allocation_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<ReverseRequest>,
) -> Result<Json<PaymentDetailDto>, ApiError> {
    let pool = payments_mutation!(&state, &headers, &auth, &Method::POST);
    let reason = payment_model::validate_reversal_reason(&body.reason)
        .map_err(|_| ApiError::ValidationFailed)?;
    let outcome = payment_repo::reverse_allocation(
        pool,
        auth.user_id,
        id,
        allocation_id,
        reason,
        state.auth.clock.now(),
    )
    .await
    .map_err(command_error)?;
    if !outcome.replayed {
        audit::record(
            pool,
            SecurityEventType::PaymentAllocationReversed,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "payment_id": id,
                "allocation_id": allocation_id,
                "reason": body.reason.trim(),
            }),
        )
        .await;
    }
    load_detail(pool, id).await.map(Json)
}

/// Payment history of one Assessment — the "who paid, when, how much,
/// still valid?" surface (docs/06 §64).
pub async fn assessment_payments(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<AssessmentPaymentDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::PAYMENTS_READ).await?;
    let pool = pool_of(&state)?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM assessments WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(db_err("assessment check failed"))?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    let rows = payment_repo::assessment_payments(pool, id)
        .await
        .map_err(db_err("assessment payments failed"))?;
    Ok(Json(
        rows.iter()
            .map(|r| AssessmentPaymentDto {
                allocation_id: r.allocation_id,
                payment_id: r.payment_id,
                payment_number: r.payment_number,
                payment_status: r.payment_status.clone(),
                amount: payment_model::canonical_amount(r.amount),
                currency: r.currency.clone(),
                allocation_status: r.allocation_status.clone(),
                received_at: r.received_at,
                payer_full_name: r.payer_full_name.clone(),
                payer_shareholder_id: r.payer_shareholder_id,
                created_at: r.created_at,
                reversed_at: r.reversed_at,
                reversal_reason: r.reversal_reason.clone(),
            })
            .collect(),
    ))
}

/// Debtor-selection context for the collection flow AND the
/// shareholder payment surface: active obligations + derived settled /
/// remaining amounts.
pub async fn shareholder_open_assessments(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(shareholder_id): Path<Uuid>,
) -> Result<Json<Vec<OpenAssessmentDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::PAYMENTS_READ).await?;
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
    let rows = payment_repo::shareholder_open_assessments(pool, shareholder_id)
        .await
        .map_err(db_err("open assessments failed"))?;
    Ok(Json(
        rows.iter()
            .map(|r| OpenAssessmentDto {
                id: r.id,
                period_id: r.period_id,
                period_number: r.period_number,
                period_name: r.period_name.clone(),
                due_date: r.due_date,
                amount: payment_model::canonical_amount(r.amount),
                currency: r.currency.clone(),
                paid_amount: payment_model::canonical_amount(r.paid_amount),
                remaining_amount: payment_model::canonical_amount(r.remaining_amount),
                generated_at: r.generated_at,
            })
            .collect(),
    ))
}

/// Shareholder-level money picture: obligations vs settled value —
/// always derived, never a stored balance.
pub async fn shareholder_financial_summary(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(shareholder_id): Path<Uuid>,
) -> Result<Json<FinancialSummaryDto>, ApiError> {
    authz::require(&state, &auth, authz::catalog::PAYMENTS_READ).await?;
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
    let row = payment_repo::shareholder_financial_summary(pool, shareholder_id)
        .await
        .map_err(db_err("financial summary failed"))?;
    Ok(Json(FinancialSummaryDto {
        assessment_count: row.assessment_count,
        open_assessment_count: row.open_assessment_count,
        total_assessed: payment_model::canonical_amount(row.total_assessed),
        total_paid: payment_model::canonical_amount(row.total_paid),
        total_remaining: payment_model::canonical_amount(row.total_remaining),
        currency: "TRY".to_owned(),
    }))
}

/// Period-level collection summary (docs/06 §68): assessed vs settled
/// vs remaining for THIS period's obligations only — payments are never
/// silently attributed across periods.
pub async fn period_financial_summary(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(period_id): Path<Uuid>,
) -> Result<Json<PeriodFinancialSummaryDto>, ApiError> {
    authz::require(&state, &auth, authz::catalog::PAYMENTS_READ).await?;
    let pool = pool_of(&state)?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM periods WHERE id = $1)")
        .bind(period_id)
        .fetch_one(pool)
        .await
        .map_err(db_err("period check failed"))?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    let row = payment_repo::period_financial_summary(pool, period_id)
        .await
        .map_err(db_err("period summary failed"))?;
    Ok(Json(PeriodFinancialSummaryDto {
        period_id,
        assessment_count: row.assessment_count,
        total_assessed: payment_model::canonical_amount(row.total_assessed),
        total_collected: payment_model::canonical_amount(row.total_collected),
        total_remaining: payment_model::canonical_amount(row.total_remaining),
        contributing_payment_count: row.contributing_payment_count,
        currency: "TRY".to_owned(),
    }))
}

/// Family collection context (docs/06 §26): current members with
/// member-wise open obligations. Used by the family-oriented bulk
/// collection flow; the Family itself is never the debtor.
pub async fn family_collection_context(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(family_id): Path<Uuid>,
) -> Result<Json<FamilyCollectionContextDto>, ApiError> {
    authz::require(&state, &auth, authz::catalog::PAYMENTS_READ).await?;
    let pool = pool_of(&state)?;
    let family = party_repo::find_family(pool, family_id)
        .await
        .map_err(db_err("family check failed"))?
        .ok_or(ApiError::NotFound)?;
    let members = payment_repo::family_collection_context(pool, family_id)
        .await
        .map_err(db_err("family context failed"))?;
    let total_remaining: Decimal = members.iter().map(|m| m.remaining_amount).sum();
    Ok(Json(FamilyCollectionContextDto {
        family_id,
        sequence_number: family.sequence_number,
        members: members
            .iter()
            .map(|m| FamilyMemberContextDto {
                member: identity_dto(
                    Some(m.member.shareholder_id),
                    &Some(m.member.first_name.clone()),
                    &Some(m.member.last_name.clone()),
                    &m.member.guardian_first_name,
                    &m.member.guardian_last_name,
                    m.member.family_sequence,
                    Some(m.member.shareholder_status.as_str()),
                )
                .expect("member row always carries a shareholder"),
                open_assessment_count: m.open_assessment_count,
                remaining_amount: payment_model::canonical_amount(m.remaining_amount),
                credit_available: payment_model::canonical_amount(m.credit_available),
                default_account: m.default_account_id.map(|id| {
                    crate::parties::routes::DefaultAccountDto {
                        id,
                        name: m.default_account_name.clone().unwrap_or_default(),
                        status: m.default_account_status.clone().unwrap_or_default(),
                    }
                }),
            })
            .collect(),
        total_remaining: payment_model::canonical_amount(total_remaining),
        currency: "TRY".to_owned(),
    }))
}
