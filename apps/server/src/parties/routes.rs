//! Parties API (STEP-004 §40–§44).
//!
//! - `GET  /api/shareholders`            (shareholders.read)  search+pagination
//! - `POST /api/shareholders`            (shareholders.manage) transactional create
//! - `GET  /api/shareholders/{id}`       (shareholders.read)  detail + membership history
//! - `PATCH /api/shareholders/{id}`      (shareholders.manage) identity/guardian edit
//! - `POST /api/shareholders/{id}/status-change` (shareholders.manage)
//! - `POST /api/shareholders/{id}/family-change` (families.manage) Aile Değiştir
//! - `GET  /api/shareholders/duplicates` (shareholders.read)  duplicate candidates
//! - `GET  /api/persons`                 (shareholders.read)  person lookup (guardian selection)
//! - `GET  /api/families`                (families.read)
//! - `POST /api/families`                (families.manage)
//! - `GET  /api/families/{id}`           (families.read)
//!
//! All reads/writes `no-store`; mutations require both CSRF layers and
//! enforce permissions server-side (STEP-003 semantics).

use axum::extract::{Path, Query, State};
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
use crate::parties::model as party_model;
use crate::parties::repo as party_repo;

const DEFAULT_PAGE_SIZE: i64 = 20;
const MAX_PAGE_SIZE: i64 = 100;

// ------------------------------------------------------------------
// DTOs (mirror @kooperatif/contracts/src/parties.ts)
// ------------------------------------------------------------------

/// FUNC-FIX-002: the shareholder's default collection account as
/// exposed to list/detail surfaces. `status` travels with the id so a
/// later-inactivated preference renders as a warning instead of
/// disappearing or being silently used.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultAccountDto {
    pub id: Uuid,
    pub name: String,
    pub status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareholderListItemDto {
    pub id: Uuid,
    pub first_name: String,
    pub last_name: String,
    pub guardian_first_name: Option<String>,
    pub guardian_last_name: Option<String>,
    pub family_id: Option<Uuid>,
    pub family_sequence: Option<i64>,
    pub status: String,
    /// None = no preference → surfaces render "Atanmamış".
    pub default_account: Option<DefaultAccountDto>,
    /// Canonical display identity (STEP-004 §20/§21):
    /// `Ad Soyad · Vasi: Ad Soyad · Aile No 47` — composed server-side
    /// so every surface shows the same disambiguating context.
    pub display_label: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MembershipHistoryDto {
    pub family_id: Uuid,
    pub family_sequence: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub ended_at: Option<OffsetDateTime>,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareholderDetailDto {
    #[serde(flatten)]
    pub item: ShareholderListItemDto,
    pub person_id: Uuid,
    pub guardian_person_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub membership_started_at: Option<OffsetDateTime>,
    pub membership_history: Vec<MembershipHistoryDto>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FamilyListItemDto {
    pub id: Uuid,
    pub sequence_number: i64,
    pub member_count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaginatedDto<T> {
    pub items: Vec<T>,
    pub page: i64,
    pub page_size: i64,
    pub total_count: i64,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PersonDto {
    id: Uuid,
    first_name: String,
    last_name: String,
    shareholder_id: Option<Uuid>,
    shareholder_status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct PersonRefRequest {
    pub mode: String, // "existing" | "new"
    pub person_id: Option<Uuid>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct FamilyRefRequest {
    pub mode: String, // "existing" | "new"
    pub family_id: Option<Uuid>,
    pub sequence_number: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct CreateShareholderRequest {
    pub person: PersonRefRequest,
    pub guardian: Option<PersonRefRequest>,
    pub family: FamilyRefRequest,
    /// FUNC-FIX-002: optional default collection account — must be an
    /// existing 'active' account (validated under a row lock).
    pub default_collection_account_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct UpdateShareholderRequest {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub guardian: Option<Option<PersonRefRequest>>,
    /// FUNC-FIX-002 triple-state: absent = untouched, null = cleared,
    /// id = reassign (must be an existing 'active' account). The custom
    /// deserializer is REQUIRED — a plain `Option<Option<T>>` collapses
    /// an explicit JSON null into `None` and could never express
    /// "clear".
    #[serde(default, deserialize_with = "explicit_nullable_uuid")]
    pub default_collection_account_id: Option<Option<Uuid>>,
    /// Lost-update guard: PATCH succeeds only when this equals the
    /// stored updated_at (docs/21 stale-state contract).
    #[serde(with = "time::serde::rfc3339")]
    pub expected_updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct StatusChangeRequest {
    pub to: String,
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct FamilyChangeRequest {
    pub family: FamilyRefRequest,
    pub reason: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub expected_updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct CreateFamilyRequest {
    pub sequence_number: i64,
}

/// Tolerates both `?page=2` (number) and the stringified values a
/// `#[serde(flatten)]` parent delivers (serde_urlencoded buffers
/// flattened fields as untyped content, so a plain Option<i64> would
/// fail to deserialize "2").
fn query_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumOrStr {
        Num(i64),
        Str(String),
    }
    match Option::<NumOrStr>::deserialize(deserializer)? {
        Some(NumOrStr::Num(n)) => Ok(Some(n)),
        Some(NumOrStr::Str(raw)) => raw
            .trim()
            .parse()
            .map(Some)
            .map_err(serde::de::Error::custom),
        None => Ok(None),
    }
}

/// Triple-state helper: wraps the deserialized `Option<Uuid>` in an
/// outer `Some` so a present-but-null body field stays distinguishable
/// from an absent one (`#[serde(default)]` supplies the outer `None`).
fn explicit_nullable_uuid<'de, D>(deserializer: D) -> Result<Option<Option<Uuid>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<Uuid>::deserialize(deserializer)?))
}

/// Same stringify-tolerance as `query_i64`, for the FUNC-FIX-002
/// `?defaultAccountId=` shareholder-list filter.
fn query_uuid<'de, D>(deserializer: D) -> Result<Option<Uuid>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<String>::deserialize(deserializer)? {
        Some(raw) => raw
            .trim()
            .parse()
            .map(Some)
            .map_err(serde::de::Error::custom),
        None => Ok(None),
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    pub search: Option<String>,
    #[serde(default, deserialize_with = "query_uuid")]
    pub default_account_id: Option<Uuid>,
    #[serde(default, deserialize_with = "query_i64")]
    pub page: Option<i64>,
    #[serde(default, deserialize_with = "query_i64")]
    pub page_size: Option<i64>,
}

pub fn parties_router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/shareholders",
            get(list_shareholders).post(create_shareholder),
        )
        .route("/api/shareholders/duplicates", get(duplicate_check))
        .route(
            "/api/shareholders/{id}",
            get(get_shareholder).patch(update_shareholder),
        )
        .route("/api/shareholders/{id}/status-change", post(status_change))
        .route("/api/shareholders/{id}/family-change", post(family_change))
        .route("/api/persons", get(list_persons))
        .route("/api/families", get(list_families).post(create_family))
        .route("/api/families/{id}", get(get_family))
        .layer(axum::middleware::from_fn(
            crate::auth::routes::no_store_cache_control,
        ))
}

fn pool_of(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state.db.as_ref().ok_or(ApiError::DependencyUnavailable)
}

pub fn page_of(query: &ListQuery) -> Result<(i64, i64), ApiError> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(DEFAULT_PAGE_SIZE);
    if !(1..=MAX_PAGE_SIZE).contains(&page_size) {
        return Err(ApiError::ValidationFailed);
    }
    Ok((page, page_size))
}

/// Shareholder mutations share this prologue (CSRF layers + permission).
macro_rules! parties_mutation {
    ($state:expr, $headers:expr, $auth:expr, $permission:expr, $method:expr) => {{
        csrf::validate_origin($headers, $method, &$state.auth.config.allowed_origins)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        csrf::validate_csrf_token($headers, &$auth.csrf_token)
            .map_err(|rejection| csrf_rejection_to_api_error(&rejection))?;
        authz::require($state, $auth, $permission).await?;
        pool_of($state)?
    }};
}

/// Canonical shareholder identity (STEP-005 §5/§6/§75): guardian context
/// is part of EVERY shareholder's operational identity — not a
/// duplicate-name disambiguator. Absent guardian renders the explicit
/// "Vasi: Belirtilmemiş" segment; it is never silently omitted.
fn compose_display_label(item: &party_repo::ShareholderListRow) -> String {
    let mut label = format!("{} {}", item.first_name, item.last_name);
    match (&item.guardian_first_name, &item.guardian_last_name) {
        (Some(first), Some(last)) => label.push_str(&format!(" · Vasi: {first} {last}")),
        _ => label.push_str(" · Vasi: Belirtilmemiş"),
    }
    if let Some(sequence) = item.family_sequence {
        label.push_str(&format!(" · Aile No {sequence}"));
    }
    label
}

fn list_item_dto(row: &party_repo::ShareholderListRow) -> ShareholderListItemDto {
    ShareholderListItemDto {
        id: row.id,
        first_name: row.first_name.clone(),
        last_name: row.last_name.clone(),
        guardian_first_name: row.guardian_first_name.clone(),
        guardian_last_name: row.guardian_last_name.clone(),
        family_id: row.family_id,
        family_sequence: row.family_sequence,
        status: row.status.clone(),
        default_account: row.default_account_id.map(|id| DefaultAccountDto {
            id,
            name: row.default_account_name.clone().unwrap_or_default(),
            status: row.default_account_status.clone().unwrap_or_default(),
        }),
        display_label: compose_display_label(row),
    }
}

// ------------------------------------------------------------------
// Handlers
// ------------------------------------------------------------------

pub async fn list_shareholders(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<ListQuery>,
) -> Result<Json<PaginatedDto<ShareholderListItemDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::SHAREHOLDERS_READ).await?;
    let pool = pool_of(&state)?;
    let (page, page_size) = page_of(&query)?;
    let rows = party_repo::list_shareholders(
        pool,
        query.search.as_deref(),
        query.default_account_id,
        page,
        page_size,
    )
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "shareholder listing failed");
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

pub async fn duplicate_check(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<ShareholderListItemDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::SHAREHOLDERS_READ).await?;
    let pool = pool_of(&state)?;
    let first = query.search.as_deref().unwrap_or_default();
    // Reuse `search` as "firstName lastName" for the duplicate probe.
    let (last_name, remainder) = first
        .rsplit_once(' ')
        .map(|(rest, last)| (last.to_string(), rest.to_string()))
        .unwrap_or((String::new(), first.to_string()));
    let rows = party_repo::duplicate_candidates(pool, &remainder, &last_name)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "duplicate check failed");
            ApiError::Internal
        })?;
    Ok(Json(rows.iter().map(list_item_dto).collect()))
}

type ResolvedPersonRef = (Option<Uuid>, Option<String>, Option<String>);

fn resolve_person_ref(
    reference: &PersonRefRequest,
    what: &str,
) -> Result<ResolvedPersonRef, ApiError> {
    match reference.mode.as_str() {
        "existing" => {
            if reference.person_id.is_none() {
                tracing::info!(what, "existing person without id");
                return Err(ApiError::ValidationFailed);
            }
            Ok((reference.person_id, None, None))
        }
        "new" => {
            let first = reference
                .first_name
                .as_deref()
                .map(party_model::validate_name)
                .transpose()
                .map_err(|_| ApiError::ValidationFailed)?;
            let last = reference
                .last_name
                .as_deref()
                .map(party_model::validate_name)
                .transpose()
                .map_err(|_| ApiError::ValidationFailed)?;
            if first.is_none() || last.is_none() {
                tracing::info!(what, "new person without names");
                return Err(ApiError::ValidationFailed);
            }
            Ok((None, first, last))
        }
        _ => Err(ApiError::ValidationFailed),
    }
}

fn resolve_family_ref(reference: &FamilyRefRequest) -> Result<party_repo::FamilyRef, ApiError> {
    match reference.mode.as_str() {
        "existing" => reference
            .family_id
            .map(party_repo::FamilyRef::Existing)
            .ok_or(ApiError::ValidationFailed),
        "new" => reference
            .sequence_number
            .map(party_model::validate_family_sequence)
            .transpose()
            .map_err(|_| ApiError::ValidationFailed)?
            .map(party_repo::FamilyRef::New)
            .ok_or(ApiError::ValidationFailed),
        _ => Err(ApiError::ValidationFailed),
    }
}

pub async fn create_shareholder(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Json(body): Json<CreateShareholderRequest>,
) -> Result<Response, ApiError> {
    let pool = parties_mutation!(
        &state,
        &headers,
        &auth,
        authz::catalog::SHAREHOLDERS_MANAGE,
        &Method::POST
    );

    let (existing_person_id, person_first, person_last) =
        resolve_person_ref(&body.person, "person")?;
    let (guardian_person_id, guardian_first, guardian_last) = match &body.guardian {
        Some(reference) => {
            let resolved = resolve_person_ref(reference, "guardian")?;
            if resolved.0.is_some() || (resolved.1.is_some() && resolved.2.is_some()) {
                resolved
            } else {
                (None, None, None)
            }
        }
        None => (None, None, None),
    };
    // A guardian is by definition a different responsible person —
    // a shareholder cannot be their own guardian.
    if guardian_person_id.is_some() && guardian_person_id == existing_person_id {
        return Err(ApiError::ValidationFailed);
    }
    let family = resolve_family_ref(&body.family)?;

    let now = state.auth.clock.now();
    let created = party_repo::create_shareholder(
        pool,
        auth.user_id,
        party_repo::CreateShareholder {
            person_first_name: person_first,
            person_last_name: person_last,
            existing_person_id,
            guardian_person_id,
            guardian_first_name: guardian_first,
            guardian_last_name: guardian_last,
            family,
            default_collection_account_id: body.default_collection_account_id,
        },
        now,
    )
    .await
    .map_err(|error| match error {
        party_repo::CreateShareholderError::DuplicateSequence => {
            tracing::info!(outcome = "family_sequence_conflict");
            ApiError::Conflict
        }
        party_repo::CreateShareholderError::PersonAlreadyShareholder => {
            tracing::info!(outcome = "person_already_shareholder");
            ApiError::Conflict
        }
        party_repo::CreateShareholderError::PersonNotFound
        | party_repo::CreateShareholderError::GuardianNotFound
        | party_repo::CreateShareholderError::FamilyNotFound
        | party_repo::CreateShareholderError::DefaultAccountInvalid => ApiError::ValidationFailed,
        party_repo::CreateShareholderError::Database(error) => {
            tracing::error!(error = %error, "shareholder creation failed");
            ApiError::Internal
        }
    })?;

    // Audit: person(s), family and shareholder created — identity data
    // only, no unnecessary duplication of personal payloads.
    if created.created_person {
        audit::record(
            pool,
            SecurityEventType::PersonCreated,
            Some(auth.user_id),
            None,
            serde_json::json!({ "person_id": created.person_id }),
        )
        .await;
    }
    if created.created_guardian_person {
        audit::record(
            pool,
            SecurityEventType::PersonCreated,
            Some(auth.user_id),
            None,
            serde_json::json!({ "person_id": created.guardian_person_id, "role": "guardian" }),
        )
        .await;
    }
    if created.created_family {
        audit::record(
            pool,
            SecurityEventType::FamilyCreated,
            Some(auth.user_id),
            None,
            serde_json::json!({ "family_id": created.family_id, "source": "shareholder_create" }),
        )
        .await;
    }
    audit::record(
        pool,
        SecurityEventType::ShareholderCreated,
        Some(auth.user_id),
        None,
        serde_json::json!({
            "shareholder_id": created.shareholder_id,
            "person_id": created.person_id,
            "guardian_person_id": created.guardian_person_id,
            "family_id": created.family_id,
            "default_collection_account_id": body.default_collection_account_id
        }),
    )
    .await;
    tracing::info!(outcome = "shareholder_created", id = %created.shareholder_id, actor = %auth.user_id);

    let detail = load_detail(pool, created.shareholder_id)
        .await?
        .ok_or(ApiError::Internal)?;
    Ok((StatusCode::CREATED, Json(detail)).into_response())
}

async fn load_detail(
    pool: &sqlx::PgPool,
    id: Uuid,
) -> Result<Option<ShareholderDetailDto>, ApiError> {
    let Some(row) = party_repo::find_shareholder(pool, id)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "shareholder load failed");
            ApiError::Internal
        })?
    else {
        return Ok(None);
    };
    let history = party_repo::membership_history(pool, id)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "membership history failed");
            ApiError::Internal
        })?;
    Ok(Some(ShareholderDetailDto {
        item: list_item_dto(&row),
        person_id: row.person_id,
        guardian_person_id: row.guardian_person_id,
        membership_started_at: row.membership_started_at,
        membership_history: history
            .into_iter()
            .map(|entry| MembershipHistoryDto {
                family_id: entry.family_id,
                family_sequence: entry.sequence_number,
                started_at: entry.started_at,
                ended_at: entry.ended_at,
                reason: entry.reason,
            })
            .collect(),
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

pub async fn get_shareholder(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<ShareholderDetailDto>, ApiError> {
    authz::require(&state, &auth, authz::catalog::SHAREHOLDERS_READ).await?;
    let pool = pool_of(&state)?;
    load_detail(pool, id)
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

pub async fn update_shareholder(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateShareholderRequest>,
) -> Result<Json<ShareholderDetailDto>, ApiError> {
    let pool = parties_mutation!(
        &state,
        &headers,
        &auth,
        authz::catalog::SHAREHOLDERS_MANAGE,
        &Method::PATCH
    );

    let mut tx = pool.begin().await.map_err(|error| {
        tracing::error!(error = %error, "update tx failed");
        ApiError::Internal
    })?;

    type LockRow = (
        Uuid,
        String,
        String,
        Option<Uuid>,
        Option<Uuid>,
        OffsetDateTime,
    );
    let current: Option<LockRow> = sqlx::query_as(
        "SELECT s.id, p.first_name, p.last_name, s.guardian_person_id, \
             s.default_collection_account_id, s.updated_at \
         FROM shareholders s JOIN persons p ON p.id = s.person_id \
         WHERE s.id = $1 FOR UPDATE OF s",
    )
    .bind(id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "shareholder load failed");
        ApiError::Internal
    })?;
    let Some((person_id, old_first, old_last, old_guardian, old_default_account, updated_at)) =
        current
    else {
        return Err(ApiError::NotFound);
    };
    if updated_at != body.expected_updated_at {
        return Err(ApiError::StaleState);
    }

    if let (Some(first), Some(last)) = (&body.first_name, &body.last_name) {
        let first = party_model::validate_name(first).map_err(|_| ApiError::ValidationFailed)?;
        let last = party_model::validate_name(last).map_err(|_| ApiError::ValidationFailed)?;
        sqlx::query(
            "UPDATE persons SET first_name = $2, last_name = $3, \
             search_name = $4, updated_at = now() WHERE id = $1",
        )
        .bind(person_id)
        .bind(&first)
        .bind(&last)
        .bind(party_model::fold_search(&format!("{first} {last}")))
        .execute(tx.as_mut())
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "person update failed");
            ApiError::Internal
        })?;
        audit::record(
            pool,
            SecurityEventType::PersonUpdated,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "person_id": person_id,
                "before": { "first": old_first, "last": old_last },
                "after": { "first": first, "last": last }
            }),
        )
        .await;
    }

    if let Some(guardian_request) = &body.guardian {
        let new_guardian = match guardian_request {
            Some(reference) => {
                let (existing_id, first, last) = resolve_person_ref(reference, "guardian")?;
                if let Some(existing_id) = existing_id {
                    let exists: Option<Uuid> =
                        sqlx::query_scalar("SELECT id FROM persons WHERE id = $1")
                            .bind(existing_id)
                            .fetch_optional(tx.as_mut())
                            .await
                            .map_err(|error| {
                                tracing::error!(error = %error, "guardian check failed");
                                ApiError::Internal
                            })?;
                    exists.ok_or(ApiError::ValidationFailed)?
                } else {
                    let first = first.ok_or(ApiError::ValidationFailed)?;
                    let last = last.ok_or(ApiError::ValidationFailed)?;
                    let search = party_model::fold_search(&format!("{first} {last}"));
                    let created: Uuid = sqlx::query_scalar(
                        "INSERT INTO persons (first_name, last_name, search_name) \
                         VALUES ($1, $2, $3) RETURNING id",
                    )
                    .bind(&first)
                    .bind(&last)
                    .bind(&search)
                    .fetch_one(tx.as_mut())
                    .await
                    .map_err(|error| {
                        tracing::error!(error = %error, "guardian person create failed");
                        ApiError::Internal
                    })?;
                    audit::record(
                        pool,
                        SecurityEventType::PersonCreated,
                        Some(auth.user_id),
                        None,
                        serde_json::json!({ "person_id": created, "role": "guardian" }),
                    )
                    .await;
                    created
                }
            }
            None => Uuid::nil(), // sentinel cleared below
        };
        if new_guardian == person_id {
            return Err(ApiError::ValidationFailed);
        }
        if new_guardian == Uuid::nil() {
            sqlx::query(
                "UPDATE shareholders SET guardian_person_id = NULL, updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .execute(tx.as_mut())
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "guardian clear failed");
                ApiError::Internal
            })?;
        } else {
            sqlx::query(
                "UPDATE shareholders SET guardian_person_id = $2, updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(new_guardian)
            .execute(tx.as_mut())
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "guardian update failed");
                ApiError::Internal
            })?;
        }
        audit::record(
            pool,
            SecurityEventType::ShareholderUpdated,
            Some(auth.user_id),
            None,
            serde_json::json!({
                "shareholder_id": id,
                "change": "guardian",
                "before": old_guardian,
                "after": if new_guardian == Uuid::nil() { None } else { Some(new_guardian) }
            }),
        )
        .await;
    }

    // FUNC-FIX-002: default collection account change/clear. Absent
    // field = untouched; null = cleared; id = reassign to an existing
    // 'active' account — locked so a concurrent status change cannot
    // race the assignment. Only future payment proposals observe the
    // change; posted payments keep their destination account.
    if let Some(new_default) = &body.default_collection_account_id {
        if let Some(account_id) = new_default {
            let status: Option<String> = sqlx::query_scalar(
                "SELECT status FROM financial_accounts WHERE id = $1 FOR UPDATE",
            )
            .bind(account_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "default account check failed");
                ApiError::Internal
            })?;
            if status.as_deref() != Some("active") {
                return Err(ApiError::ValidationFailed);
            }
        }
        if *new_default != old_default_account {
            sqlx::query(
                "UPDATE shareholders SET default_collection_account_id = $2, \
                 updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(*new_default)
            .execute(tx.as_mut())
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "default account update failed");
                ApiError::Internal
            })?;
            audit::record(
                pool,
                SecurityEventType::ShareholderDefaultAccountChanged,
                Some(auth.user_id),
                None,
                serde_json::json!({
                    "shareholder_id": id,
                    "before": old_default_account,
                    "after": *new_default
                }),
            )
            .await;
        }
    }

    tx.commit().await.map_err(|error| {
        tracing::error!(error = %error, "update commit failed");
        ApiError::Internal
    })?;

    load_detail(pool, id)
        .await?
        .map(Json)
        .ok_or(ApiError::Internal)
}

pub async fn status_change(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<StatusChangeRequest>,
) -> Result<Response, ApiError> {
    let pool = parties_mutation!(
        &state,
        &headers,
        &auth,
        authz::catalog::SHAREHOLDERS_MANAGE,
        &Method::POST
    );

    let target =
        party_model::ShareholderStatus::parse(&body.to).ok_or(ApiError::ValidationFailed)?;

    // Load + transition validation + update under a row lock so two
    // concurrent transitions serialize: the second observes the
    // committed predecessor, not a stale one (docs/16, §49).
    let mut tx = pool.begin().await.map_err(|error| {
        tracing::error!(error = %error, "status tx failed");
        ApiError::Internal
    })?;
    let old: Option<String> =
        sqlx::query_scalar("SELECT status FROM shareholders WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "status load failed");
                ApiError::Internal
            })?;
    let Some(old) = old else {
        return Err(ApiError::NotFound);
    };
    let old_status = party_model::ShareholderStatus::parse(&old).ok_or(ApiError::Internal)?;
    if !old_status.can_transition_to(target) {
        tracing::info!(
            outcome = "invalid_status_transition",
            from = old,
            to = body.to
        );
        return Err(ApiError::ValidationFailed);
    }

    sqlx::query("UPDATE shareholders SET status = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(target.as_str())
        .execute(tx.as_mut())
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "status update failed");
            ApiError::Internal
        })?;
    tx.commit().await.map_err(|error| {
        tracing::error!(error = %error, "status commit failed");
        ApiError::Internal
    })?;

    audit::record(
        pool,
        SecurityEventType::ShareholderStatusChanged,
        Some(auth.user_id),
        None,
        serde_json::json!({
            "shareholder_id": id, "from": old, "to": target.as_str(), "reason": body.reason
        }),
    )
    .await;
    tracing::info!(outcome = "shareholder_status_changed", id = %id, from = old, to = target.as_str());

    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn family_change(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<FamilyChangeRequest>,
) -> Result<Response, ApiError> {
    let pool = parties_mutation!(
        &state,
        &headers,
        &auth,
        authz::catalog::FAMILIES_MANAGE,
        &Method::POST
    );

    let target = match body.family.mode.as_str() {
        "existing" => party_repo::FamilyChangeTarget::Existing(
            body.family.family_id.ok_or(ApiError::ValidationFailed)?,
        ),
        "new" => party_repo::FamilyChangeTarget::New(
            party_model::validate_family_sequence(
                body.family
                    .sequence_number
                    .ok_or(ApiError::ValidationFailed)?,
            )
            .map_err(|_| ApiError::ValidationFailed)?,
        ),
        _ => return Err(ApiError::ValidationFailed),
    };

    let now = state.auth.clock.now();
    let (new_family_id, old_sequence, new_sequence) = party_repo::change_family(
        pool,
        auth.user_id,
        id,
        target,
        body.reason.as_deref(),
        body.expected_updated_at,
        now,
    )
    .await
    .map_err(|error| match error {
        party_repo::FamilyChangeError::ShareholderNotFound
        | party_repo::FamilyChangeError::FamilyNotFound => ApiError::NotFound,
        party_repo::FamilyChangeError::DuplicateSequence => ApiError::Conflict,
        party_repo::FamilyChangeError::StaleState => ApiError::StaleState,
        party_repo::FamilyChangeError::Database(error) => {
            tracing::error!(error = %error, "family change failed");
            ApiError::Internal
        }
    })?;

    audit::record(
        pool,
        SecurityEventType::ShareholderFamilyChanged,
        Some(auth.user_id),
        None,
        serde_json::json!({
            "shareholder_id": id,
            "from_family_sequence": old_sequence,
            "to_family_sequence": new_sequence,
            "to_family_id": new_family_id,
            "reason": body.reason
        }),
    )
    .await;
    tracing::info!(outcome = "shareholder_family_changed", id = %id, from = old_sequence, to = new_sequence, actor = %auth.user_id);

    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn list_persons(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<PersonDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::SHAREHOLDERS_READ).await?;
    let pool = pool_of(&state)?;
    let search = query.search.as_deref().unwrap_or_default();
    let rows = party_repo::search_persons(pool, search, 20)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "person search failed");
            ApiError::Internal
        })?;
    Ok(Json(
        rows.into_iter()
            .map(|row| PersonDto {
                id: row.id,
                first_name: row.first_name,
                last_name: row.last_name,
                shareholder_id: row.shareholder_id,
                shareholder_status: row.shareholder_status,
            })
            .collect(),
    ))
}

pub async fn list_families(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<ListQuery>,
) -> Result<Json<PaginatedDto<FamilyListItemDto>>, ApiError> {
    authz::require(&state, &auth, authz::catalog::FAMILIES_READ).await?;
    let pool = pool_of(&state)?;
    let (page, page_size) = page_of(&query)?;
    let rows = party_repo::list_families(pool, query.search.as_deref(), page, page_size)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "family listing failed");
            ApiError::Internal
        })?;
    let total_count = rows.first().map(|row| row.total_count).unwrap_or(0);
    Ok(Json(PaginatedDto {
        items: rows
            .into_iter()
            .map(|row| FamilyListItemDto {
                id: row.id,
                sequence_number: row.sequence_number,
                member_count: row.member_count,
            })
            .collect(),
        page,
        page_size,
        total_count,
    }))
}

pub async fn create_family(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: CurrentAuth,
    Json(body): Json<CreateFamilyRequest>,
) -> Result<Response, ApiError> {
    let pool = parties_mutation!(
        &state,
        &headers,
        &auth,
        authz::catalog::FAMILIES_MANAGE,
        &Method::POST
    );
    let sequence = party_model::validate_family_sequence(body.sequence_number)
        .map_err(|_| ApiError::ValidationFailed)?;

    let id: Uuid =
        sqlx::query_scalar("INSERT INTO families (sequence_number) VALUES ($1) RETURNING id")
            .bind(sequence)
            .fetch_one(pool)
            .await
            .map_err(|error| match error {
                sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                    tracing::info!(outcome = "family_sequence_conflict", sequence);
                    ApiError::Conflict
                }
                other => {
                    tracing::error!(error = %other, "family creation failed");
                    ApiError::Internal
                }
            })?;

    audit::record(
        pool,
        SecurityEventType::FamilyCreated,
        Some(auth.user_id),
        None,
        serde_json::json!({ "family_id": id, "sequence_number": sequence, "source": "api" }),
    )
    .await;
    tracing::info!(outcome = "family_created", id = %id, sequence, actor = %auth.user_id);

    let row = party_repo::find_family(pool, id)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "family reload failed");
            ApiError::Internal
        })?
        .ok_or(ApiError::Internal)?;
    Ok((
        StatusCode::CREATED,
        Json(FamilyListItemDto {
            id: row.id,
            sequence_number: row.sequence_number,
            member_count: row.member_count,
        }),
    )
        .into_response())
}

pub async fn get_family(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    authz::require(&state, &auth, authz::catalog::FAMILIES_READ).await?;
    let pool = pool_of(&state)?;

    let family = party_repo::find_family(pool, id)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "family load failed");
            ApiError::Internal
        })?
        .ok_or(ApiError::NotFound)?;

    // Current members with full identity context (one query, no N+1).
    let members = party_repo::list_shareholders_by_family(pool, id)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "family members load failed");
            ApiError::Internal
        })?;

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct FamilyDetailDto {
        id: Uuid,
        sequence_number: i64,
        member_count: i64,
        members: Vec<ShareholderListItemDto>,
    }
    Ok(Json(FamilyDetailDto {
        id: family.id,
        sequence_number: family.sequence_number,
        member_count: family.member_count,
        members: members.iter().map(list_item_dto).collect(),
    })
    .into_response())
}
