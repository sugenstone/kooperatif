//! Governance API (STEP-014, docs/09).
//!
//! - `GET  /api/governance/bodies`                       (governance.read)
//! - `POST /api/governance/bodies`                       (governance.manage)
//! - `GET  /api/governance/bodies/{id}`                  (governance.read)
//! - `POST /api/governance/bodies/{id}/close`            (governance.manage)
//! - `POST /api/governance/bodies/{id}/memberships`      (governance.manage)
//! - `POST /api/governance/memberships/{id}/end`         (governance.manage)
//! - `GET  /api/governance/decisions`                    (governance.read)
//! - `POST /api/governance/decisions`                    (governance.manage)
//! - `GET  /api/governance/decisions/{id}`               (governance.read)
//! - `POST /api/governance/decisions/{id}/update`        (governance.manage)
//! - `POST /api/governance/decisions/{id}/open`          (governance.manage)
//! - `POST /api/governance/decisions/{id}/cancel`        (governance.manage)
//! - `POST /api/governance/decisions/{id}/votes`         (governance.manage)
//! - `POST /api/governance/decisions/{id}/finalize`      (governance.manage)
//! - `GET  /api/governance/persons`                      (governance.manage)
//!
//! Every governance command is EVIDENCE only — none moves money.
//! `governance.manage` lets an operator RECORD governance facts
//! (bodies, memberships, votes, outcomes); it never makes the operator
//! a member or voter — vote eligibility is a `persons` fact checked
//! server-side (docs/18: RBAC != governance, User != Person).
//! There is deliberately NO DELETE — history is corrected by explicit
//! lifecycle transitions (docs/19).

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
use crate::governance::model as gov_model;
use crate::governance::repo as gov_repo;
use crate::governance::repo::GovernanceRepoError;
use crate::http::error::ApiError;
use crate::http::AppState;
use crate::parties::repo as party_repo;
use crate::parties::routes::{page_of, ListQuery, PaginatedDto};

pub fn governance_router() -> Router<AppState> {
    Router::new()
        .route("/api/governance/bodies", get(list_bodies).post(create_body))
        .route("/api/governance/bodies/{id}", get(get_body))
        .route("/api/governance/bodies/{id}/close", post(close_body))
        .route(
            "/api/governance/bodies/{id}/memberships",
            post(create_membership),
        )
        .route("/api/governance/memberships/{id}/end", post(end_membership))
        .route(
            "/api/governance/decisions",
            get(list_decisions).post(create_decision),
        )
        .route("/api/governance/decisions/{id}", get(get_decision))
        .route(
            "/api/governance/decisions/{id}/update",
            post(update_decision),
        )
        .route("/api/governance/decisions/{id}/open", post(open_decision))
        .route(
            "/api/governance/decisions/{id}/cancel",
            post(cancel_decision),
        )
        .route("/api/governance/decisions/{id}/votes", post(cast_vote))
        .route(
            "/api/governance/decisions/{id}/finalize",
            post(finalize_decision),
        )
        .route("/api/governance/persons", get(person_lookup))
        .layer(axum::middleware::from_fn(
            crate::auth::routes::no_store_cache_control,
        ))
}

fn pool_of(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state.db.as_ref().ok_or(ApiError::DependencyUnavailable)
}

/// Governance mutations share the full CSRF + permission prologue.
macro_rules! governance_mutation {
    ($state:expr, $headers:expr, $auth:expr, $method:expr) => {{
        csrf::validate_origin($headers, $method, &$state.auth.config.allowed_origins)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        csrf::validate_csrf_token($headers, &$auth.csrf_token)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        authz::require($state, $auth, authz::catalog::GOVERNANCE_MANAGE).await?;
        pool_of($state)?
    }};
}

fn command_error(error: GovernanceRepoError) -> ApiError {
    use GovernanceRepoError as E;
    match error {
        E::NotFound => ApiError::NotFound,
        E::InvalidState | E::InactiveBody => ApiError::ValidationFailed,
        E::BodyBusy | E::ActiveMembershipExists | E::AlreadyVoted | E::IdempotencyConflict => {
            ApiError::Conflict
        }
        // Ineligible voter is an authorization-flavored denial, not a
        // conflict: the person simply holds no active seat.
        E::NotEligible => ApiError::PermissionDenied,
        E::Database(error) => {
            tracing::error!(error = %error, "governance command failed");
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

fn parse_business_date(raw: &str) -> Result<Date, ApiError> {
    Date::parse(
        raw.trim(),
        &time::macros::format_description!("[year]-[month]-[day]"),
    )
    .map_err(|_| ApiError::ValidationFailed)
}

fn parse_optional_business_date(raw: Option<&str>) -> Result<Option<Date>, ApiError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(text) => Ok(Some(parse_business_date(text)?)),
    }
}

fn parse_timestamp(raw: &str) -> Result<OffsetDateTime, ApiError> {
    OffsetDateTime::parse(raw.trim(), &Rfc3339).map_err(|_| ApiError::ValidationFailed)
}

// ------------------------------------------------------------------
// DTOs (mirror @kooperatif/contracts/src/governance.ts)
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
pub struct BodyListItemDto {
    pub id: Uuid,
    pub body_number: i64,
    pub name: String,
    pub body_type: String,
    pub status: String,
    /// Derived active-seat count — never stored.
    pub active_members: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MembershipDto {
    pub id: Uuid,
    pub body_id: Uuid,
    pub person_id: Uuid,
    pub person_name: String,
    /// Bounded free-text seat label — carries NO powers.
    pub title: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub ended_at: Option<OffsetDateTime>,
    pub end_reason: Option<String>,
    /// Derived: true while the term is open (evidence convenience).
    pub active: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

fn membership_dto(row: &gov_repo::MembershipRow) -> MembershipDto {
    MembershipDto {
        id: row.id,
        body_id: row.body_id,
        person_id: row.person_id,
        person_name: row.person_name.clone(),
        title: row.title.clone(),
        started_at: row.started_at,
        ended_at: row.ended_at,
        end_reason: row.end_reason.clone(),
        active: row.ended_at.is_none(),
        created_at: row.created_at,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyDetailDto {
    pub id: Uuid,
    pub body_number: i64,
    pub name: String,
    pub body_type: String,
    pub description: Option<String>,
    pub status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub closed_at: Option<OffsetDateTime>,
    /// Full membership timeline — current AND historical.
    pub memberships: Vec<MembershipDto>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionListItemDto {
    pub id: Uuid,
    pub decision_number: i64,
    pub body_id: Uuid,
    pub body_name: String,
    pub title: String,
    pub status: String,
    #[serde(with = "date_iso")]
    pub decision_on: Date,
    #[serde(with = "date_iso::option")]
    pub effective_on: Option<Date>,
    pub vote_count: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoteDto {
    pub id: Uuid,
    pub decision_id: Uuid,
    /// The membership row that established eligibility at cast time —
    /// survives later term changes as historical evidence.
    pub membership_id: Uuid,
    pub person_id: Uuid,
    pub person_name: String,
    pub membership_title: Option<String>,
    pub choice: String,
    #[serde(with = "time::serde::rfc3339")]
    pub cast_at: OffsetDateTime,
    pub note: Option<String>,
    /// The login actor who RECORDED the vote — distinct from the
    /// voting Person (docs/18: User != Person).
    pub recorded_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

fn vote_dto(row: &gov_repo::VoteRow) -> VoteDto {
    VoteDto {
        id: row.id,
        decision_id: row.decision_id,
        membership_id: row.membership_id,
        person_id: row.person_id,
        person_name: row.person_name.clone(),
        membership_title: row.membership_title.clone(),
        choice: row.choice.clone(),
        cast_at: row.cast_at,
        note: row.note.clone(),
        recorded_by: row.recorded_by,
        created_at: row.created_at,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionDetailDto {
    pub id: Uuid,
    pub decision_number: i64,
    pub body_id: Uuid,
    pub body_name: String,
    pub title: String,
    pub decision_text: String,
    /// Formal Decision Date — distinct from Effective Date (docs/15).
    #[serde(with = "date_iso")]
    pub decision_on: Date,
    #[serde(with = "date_iso::option")]
    pub effective_on: Option<Date>,
    pub status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub opened_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub finalized_at: Option<OffsetDateTime>,
    /// Frozen finalization snapshot (None until finalized).
    pub eligible_count: Option<i32>,
    pub approve_count: Option<i32>,
    pub reject_count: Option<i32>,
    pub abstain_count: Option<i32>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub cancelled_at: Option<OffsetDateTime>,
    pub cancellation_reason: Option<String>,
    pub votes: Vec<VoteDto>,
    /// Currently-active memberships of the body — the electorate an
    /// open decision accepts votes from.
    pub eligible_members: Vec<MembershipDto>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

fn decision_detail_dto(detail: gov_repo::DecisionDetail) -> DecisionDetailDto {
    DecisionDetailDto {
        id: detail.decision.id,
        decision_number: detail.decision.decision_number,
        body_id: detail.decision.body_id,
        body_name: detail.decision.body_name,
        title: detail.decision.title,
        decision_text: detail.decision.decision_text,
        decision_on: detail.decision.decision_on,
        effective_on: detail.decision.effective_on,
        status: detail.decision.status,
        opened_at: detail.decision.opened_at,
        finalized_at: detail.decision.finalized_at,
        eligible_count: detail.decision.eligible_count,
        approve_count: detail.decision.approve_count,
        reject_count: detail.decision.reject_count,
        abstain_count: detail.decision.abstain_count,
        cancelled_at: detail.decision.cancelled_at,
        cancellation_reason: detail.decision.cancellation_reason,
        votes: detail.votes.iter().map(vote_dto).collect(),
        eligible_members: detail.eligible_members.iter().map(membership_dto).collect(),
        created_at: detail.decision.created_at,
        updated_at: detail.decision.updated_at,
    }
}

// ------------------------------------------------------------------
// Bodies
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyListQuery {
    pub status: Option<String>,
    pub search: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

pub async fn list_bodies(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<BodyListQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::GOVERNANCE_READ).await?;
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
            gov_model::BodyStatus::parse(s)
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

    let rows = gov_repo::list_bodies(
        pool,
        &gov_repo::BodyFilter {
            status,
            search,
            page,
            page_size,
        },
    )
    .await
    .map_err(db_err("governance body list query failed"))?;

    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    Ok(Json(PaginatedDto {
        items: rows
            .iter()
            .map(|r| BodyListItemDto {
                id: r.id,
                body_number: r.body_number,
                name: r.name.clone(),
                body_type: r.body_type.clone(),
                status: r.status.clone(),
                active_members: r.active_members,
                created_at: r.created_at,
            })
            .collect(),
        page,
        page_size,
        total_count,
    })
    .into_response())
}

pub async fn get_body(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::GOVERNANCE_READ).await?;
    let pool = pool_of(&state)?;
    gov_repo::get_body_detail(pool, id)
        .await
        .map_err(db_err("governance body detail query failed"))?
        .map(|d| {
            Json(BodyDetailDto {
                id: d.body.id,
                body_number: d.body.body_number,
                name: d.body.name,
                body_type: d.body.body_type,
                description: d.body.description,
                status: d.body.status,
                closed_at: d.body.closed_at,
                memberships: d.memberships.iter().map(membership_dto).collect(),
                created_at: d.body.created_at,
                updated_at: d.body.updated_at,
            })
            .into_response()
        })
        .ok_or(ApiError::NotFound)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBodyRequest {
    pub name: String,
    pub body_type: String,
    pub description: Option<String>,
    pub idempotency_key: String,
}

pub async fn create_body(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Json(body): Json<CreateBodyRequest>,
) -> Result<Response, ApiError> {
    let pool = governance_mutation!(&state, &headers, &auth, &Method::POST);
    let name = gov_model::validate_name(&body.name).map_err(|_| ApiError::ValidationFailed)?;
    let body_type =
        gov_model::validate_body_type(&body.body_type).map_err(|_| ApiError::ValidationFailed)?;
    let description = gov_model::validate_description(body.description.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let idempotency_key = gov_model::validate_idempotency_key(&body.idempotency_key)
        .map_err(|_| ApiError::ValidationFailed)?;
    let fingerprint = gov_model::create_body_fingerprint(&name, &body_type, &description);

    let outcome = gov_repo::create_body(
        pool,
        auth.user_id,
        gov_repo::CreateBody {
            name,
            body_type,
            description,
            idempotency_key,
            fingerprint,
        },
    )
    .await
    .map_err(command_error)?;

    if !outcome.replayed {
        audit::record(
            pool,
            SecurityEventType::GovernanceBodyCreated,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "bodyId": outcome.body_id,
                "bodyNumber": outcome.body_number,
            }),
        )
        .await;
        tracing::info!(
            outcome = "governance_body_created", id = %outcome.body_id,
            number = outcome.body_number, actor = %auth.user_id
        );
    }

    let detail = gov_repo::get_body_detail(pool, outcome.body_id)
        .await
        .map_err(db_err("governance body detail query failed"))?
        .ok_or(ApiError::Internal)?;
    let status = if outcome.replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((
        status,
        Json(BodyDetailDto {
            id: detail.body.id,
            body_number: detail.body.body_number,
            name: detail.body.name,
            body_type: detail.body.body_type,
            description: detail.body.description,
            status: detail.body.status,
            closed_at: detail.body.closed_at,
            memberships: detail.memberships.iter().map(membership_dto).collect(),
            created_at: detail.body.created_at,
            updated_at: detail.body.updated_at,
        }),
    )
        .into_response())
}

pub async fn close_body(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let pool = governance_mutation!(&state, &headers, &auth, &Method::POST);
    gov_repo::close_body(pool, auth.user_id, id)
        .await
        .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::GovernanceBodyClosed,
        Some(auth.user_id),
        None,
        serde_json::json!({ "bodyId": id }),
    )
    .await;

    get_body(State(state), auth, Path(id)).await
}

// ------------------------------------------------------------------
// Memberships
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateMembershipRequest {
    pub person_id: Uuid,
    pub title: Option<String>,
    pub started_at: String,
    pub idempotency_key: String,
}

pub async fn create_membership(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(body_id): Path<Uuid>,
    Json(body): Json<CreateMembershipRequest>,
) -> Result<Response, ApiError> {
    let pool = governance_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let title = gov_model::validate_membership_title(body.title.as_deref())
        .map_err(|_| ApiError::ValidationFailed)?;
    let started_at = parse_timestamp(&body.started_at)?;
    gov_model::validate_not_future(
        started_at,
        now,
        gov_model::GovernanceError::InvalidStartedAt,
    )
    .map_err(|_| ApiError::ValidationFailed)?;
    let idempotency_key = gov_model::validate_idempotency_key(&body.idempotency_key)
        .map_err(|_| ApiError::ValidationFailed)?;
    let fingerprint =
        gov_model::create_membership_fingerprint(body_id, body.person_id, &title, started_at);

    let outcome = gov_repo::create_membership(
        pool,
        auth.user_id,
        gov_repo::CreateMembership {
            body_id,
            person_id: body.person_id,
            title,
            started_at,
            idempotency_key,
            fingerprint,
        },
    )
    .await
    .map_err(command_error)?;

    if !outcome.replayed {
        audit::record(
            pool,
            SecurityEventType::GovernanceMembershipStarted,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "membershipId": outcome.membership_id,
                "bodyId": body_id,
                "personId": body.person_id,
            }),
        )
        .await;
    }

    let status = if outcome.replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((
        status,
        Json(serde_json::json!({
            "membershipId": outcome.membership_id,
        })),
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndMembershipRequest {
    pub ended_at: String,
    pub reason: String,
}

pub async fn end_membership(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<EndMembershipRequest>,
) -> Result<Response, ApiError> {
    let pool = governance_mutation!(&state, &headers, &auth, &Method::POST);
    let now = state.auth.clock.now();
    let ended_at = parse_timestamp(&body.ended_at)?;
    gov_model::validate_not_future(ended_at, now, gov_model::GovernanceError::InvalidEndedAt)
        .map_err(|_| ApiError::ValidationFailed)?;
    let reason =
        gov_model::validate_reason(&body.reason).map_err(|_| ApiError::ValidationFailed)?;

    gov_repo::end_membership(pool, auth.user_id, id, ended_at, &reason)
        .await
        .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::GovernanceMembershipEnded,
        Some(auth.user_id),
        None,
        serde_json::json!({ "membershipId": id }),
    )
    .await;

    Ok(StatusCode::OK.into_response())
}

// ------------------------------------------------------------------
// Decisions
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionListQuery {
    pub body_id: Option<Uuid>,
    pub status: Option<String>,
    pub search: Option<String>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

pub async fn list_decisions(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<DecisionListQuery>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::GOVERNANCE_READ).await?;
    let pool = pool_of(&state)?;
    let (page, page_size) = page_of(&ListQuery {
        search: query.search.clone(),
        page: query.page,
        page_size: query.page_size,
        default_account_id: None,
    })?;
    let status = query
        .status
        .as_deref()
        .map(|s| {
            gov_model::DecisionStatus::parse(s)
                .map(|v| v.as_str().to_string())
                .ok_or(ApiError::ValidationFailed)
        })
        .transpose()?;
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let rows = gov_repo::list_decisions(
        pool,
        &gov_repo::DecisionFilter {
            body_id: query.body_id,
            status,
            search,
            page,
            page_size,
        },
    )
    .await
    .map_err(db_err("governance decision list query failed"))?;

    let total_count = rows.first().map(|r| r.total_count).unwrap_or(0);
    Ok(Json(PaginatedDto {
        items: rows
            .iter()
            .map(|r| DecisionListItemDto {
                id: r.id,
                decision_number: r.decision_number,
                body_id: r.body_id,
                body_name: r.body_name.clone(),
                title: r.title.clone(),
                status: r.status.clone(),
                decision_on: r.decision_on,
                effective_on: r.effective_on,
                vote_count: r.vote_count,
                created_at: r.created_at,
            })
            .collect(),
        page,
        page_size,
        total_count,
    })
    .into_response())
}

pub async fn get_decision(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::GOVERNANCE_READ).await?;
    let pool = pool_of(&state)?;
    gov_repo::get_decision_detail(pool, id)
        .await
        .map_err(db_err("governance decision detail query failed"))?
        .map(|d| Json(decision_detail_dto(d)).into_response())
        .ok_or(ApiError::NotFound)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDecisionRequest {
    pub body_id: Uuid,
    pub title: String,
    pub decision_text: String,
    /// Formal Decision Date (YYYY-MM-DD) — Decision Date != Effective
    /// Date (docs/15).
    pub decision_on: String,
    pub effective_on: Option<String>,
    pub idempotency_key: String,
}

pub async fn create_decision(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Json(body): Json<CreateDecisionRequest>,
) -> Result<Response, ApiError> {
    let pool = governance_mutation!(&state, &headers, &auth, &Method::POST);
    let title = gov_model::validate_title(&body.title).map_err(|_| ApiError::ValidationFailed)?;
    let decision_text = gov_model::validate_decision_text(&body.decision_text)
        .map_err(|_| ApiError::ValidationFailed)?;
    let decision_on = parse_business_date(&body.decision_on)?;
    let effective_on = parse_optional_business_date(body.effective_on.as_deref())?;
    let idempotency_key = gov_model::validate_idempotency_key(&body.idempotency_key)
        .map_err(|_| ApiError::ValidationFailed)?;
    let fingerprint = gov_model::create_decision_fingerprint(
        body.body_id,
        &title,
        &decision_text,
        decision_on,
        effective_on,
    );

    let outcome = gov_repo::create_decision(
        pool,
        auth.user_id,
        gov_repo::CreateDecision {
            body_id: body.body_id,
            title,
            decision_text,
            decision_on,
            effective_on,
            idempotency_key,
            fingerprint,
        },
    )
    .await
    .map_err(command_error)?;

    if !outcome.replayed {
        audit::record(
            pool,
            SecurityEventType::GovernanceDecisionCreated,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "decisionId": outcome.decision_id,
                "decisionNumber": outcome.decision_number,
                "bodyId": body.body_id,
            }),
        )
        .await;
    }

    let detail = gov_repo::get_decision_detail(pool, outcome.decision_id)
        .await
        .map_err(db_err("governance decision detail query failed"))?
        .ok_or(ApiError::Internal)?;
    let status = if outcome.replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(decision_detail_dto(detail))).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDecisionRequest {
    pub title: String,
    pub decision_text: String,
    pub decision_on: String,
    pub effective_on: Option<String>,
}

/// Draft edit — rejected the moment the decision leaves 'draft', so
/// members never vote on text that can be silently rewritten.
pub async fn update_decision(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateDecisionRequest>,
) -> Result<Response, ApiError> {
    let pool = governance_mutation!(&state, &headers, &auth, &Method::POST);
    let title = gov_model::validate_title(&body.title).map_err(|_| ApiError::ValidationFailed)?;
    let decision_text = gov_model::validate_decision_text(&body.decision_text)
        .map_err(|_| ApiError::ValidationFailed)?;
    let decision_on = parse_business_date(&body.decision_on)?;
    let effective_on = parse_optional_business_date(body.effective_on.as_deref())?;

    gov_repo::update_decision(pool, id, &title, &decision_text, decision_on, effective_on)
        .await
        .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::GovernanceDecisionUpdated,
        Some(auth.user_id),
        None,
        serde_json::json!({ "decisionId": id }),
    )
    .await;

    get_decision(State(state), auth, Path(id)).await
}

pub async fn open_decision(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let pool = governance_mutation!(&state, &headers, &auth, &Method::POST);
    gov_repo::open_decision(pool, auth.user_id, id)
        .await
        .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::GovernanceDecisionOpened,
        Some(auth.user_id),
        None,
        serde_json::json!({ "decisionId": id }),
    )
    .await;

    get_decision(State(state), auth, Path(id)).await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelDecisionRequest {
    pub reason: String,
}

/// Draft cancel — only drafts may be withdrawn; opened/finalized
/// decisions are permanent evidence.
pub async fn cancel_decision(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<CancelDecisionRequest>,
) -> Result<Response, ApiError> {
    let pool = governance_mutation!(&state, &headers, &auth, &Method::POST);
    let reason =
        gov_model::validate_reason(&body.reason).map_err(|_| ApiError::ValidationFailed)?;

    gov_repo::cancel_decision(pool, auth.user_id, id, &reason)
        .await
        .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::GovernanceDecisionCancelled,
        Some(auth.user_id),
        None,
        serde_json::json!({ "decisionId": id }),
    )
    .await;

    get_decision(State(state), auth, Path(id)).await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CastVoteRequest {
    /// The voting Person — eligibility is verified server-side against
    /// the body's ACTIVE memberships; this field alone grants nothing.
    pub person_id: Uuid,
    pub choice: String,
    pub note: Option<String>,
    pub idempotency_key: String,
}

/// Records a vote on behalf of an eligible member (recorded-vote
/// model). The login actor is stored as `recorded_by` — never conflated
/// with the voter. Self-voting is not a separate path: a logged-in
/// operator with governance.manage records member votes.
pub async fn cast_vote(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<CastVoteRequest>,
) -> Result<Response, ApiError> {
    let pool = governance_mutation!(&state, &headers, &auth, &Method::POST);
    let choice = gov_model::VoteChoice::parse(&body.choice).ok_or(ApiError::ValidationFailed)?;
    let note =
        gov_model::validate_note(body.note.as_deref()).map_err(|_| ApiError::ValidationFailed)?;
    let idempotency_key = gov_model::validate_idempotency_key(&body.idempotency_key)
        .map_err(|_| ApiError::ValidationFailed)?;
    let fingerprint = gov_model::cast_vote_fingerprint(id, body.person_id, choice, &note);

    let outcome = gov_repo::cast_vote(
        pool,
        auth.user_id,
        gov_repo::CastVote {
            decision_id: id,
            person_id: body.person_id,
            choice,
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
            SecurityEventType::GovernanceVoteRecorded,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "voteId": outcome.vote_id,
                "decisionId": id,
                "personId": body.person_id,
                "choice": choice.as_str(),
            }),
        )
        .await;
    }

    let status = if outcome.replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((
        status,
        Json(serde_json::json!({ "voteId": outcome.vote_id })),
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalizeDecisionRequest {
    /// The formally decided outcome — recorded, never computed.
    /// Quorum/majority rules are open decisions (docs/09) and are NOT
    /// silently implemented.
    pub outcome: String,
}

pub async fn finalize_decision(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<FinalizeDecisionRequest>,
) -> Result<Response, ApiError> {
    let pool = governance_mutation!(&state, &headers, &auth, &Method::POST);
    let outcome =
        gov_model::DecisionOutcome::parse(&body.outcome).ok_or(ApiError::ValidationFailed)?;

    gov_repo::finalize_decision(
        pool,
        auth.user_id,
        gov_repo::FinalizeDecision {
            decision_id: id,
            outcome,
        },
    )
    .await
    .map_err(command_error)?;

    audit::record(
        pool,
        SecurityEventType::GovernanceDecisionFinalized,
        Some(auth.user_id),
        None,
        serde_json::json!({
            "decisionId": id,
            "outcome": outcome.as_str(),
        }),
    )
    .await;

    get_decision(State(state), auth, Path(id)).await
}

// ------------------------------------------------------------------
// Person lookup for membership/voter selection (governance.manage)
// ------------------------------------------------------------------
// Mirrors `/api/social-aid/persons`: a governance member is a canonical
// Person — never derived from User/Shareholder login facts — so the
// operator resolves the link without holding full parties read rights.

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
    authz::require(&state, &auth, authz::catalog::GOVERNANCE_MANAGE).await?;
    let pool = pool_of(&state)?;
    let search = query.search.as_deref().unwrap_or_default();
    if search.trim().is_empty() {
        return Ok(Json(Vec::new()));
    }
    let limit = query.limit.unwrap_or(10).clamp(1, 25);
    let rows = party_repo::search_persons(pool, search.trim(), limit)
        .await
        .map_err(db_err("governance person lookup failed"))?;
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
