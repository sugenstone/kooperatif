//! Audit read endpoints (M1-P5 B4).
//!
//! - `GET /api/audit/events`          — cooperative lane
//! - `GET /api/platform/audit/events` — platform lane (D4)
//!
//! Everything the response carries passes an explicit DTO allowlist;
//! raw rows are never serialized. `session_id` is never exposed (D5).

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64ct::{Base64UrlUnpadded, Encoding};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::{authz, extractor::CurrentAuth};
use crate::http::error::ApiError;
use crate::http::AppState;
use crate::tenant::BusinessAuth;

const DEFAULT_PAGE_SIZE: i64 = 50;
const MAX_PAGE_SIZE: i64 = 100;

pub fn audit_router() -> Router<AppState> {
    Router::new()
        .route("/api/audit/events", get(list_events_scoped))
        // Audit data is per-user sensitive: never cacheable.
        .layer(axum::middleware::from_fn(
            crate::auth::routes::no_store_cache_control,
        ))
}

/// Platform lane — mounted OUTSIDE the business membership gate in
/// http/mod.rs, mirroring `platform_router`.
pub fn platform_audit_router() -> Router<AppState> {
    Router::new()
        .route("/api/platform/audit/events", get(list_events_platform))
        .layer(axum::middleware::from_fn(
            crate::auth::routes::no_store_cache_control,
        ))
}

// ── Query contract ───────────────────────────────────────────────────

// serde_urlencoded does not support deny_unknown_fields — filters are
// allowlisted below and anything else is ignored (never honored).
#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    /// Event-type filter; must be a known type (400 otherwise).
    #[serde(rename = "eventType")]
    pub event_type: Option<String>,
    #[serde(rename = "actorUserId")]
    pub actor_user_id: Option<Uuid>,
    /// RFC3339 window bounds (inclusive).
    pub from: Option<String>,
    pub to: Option<String>,
    /// Opaque continuation cursor from a previous `nextCursor`.
    pub cursor: Option<String>,
    /// Platform lane only: restrict to one cooperative (D4). Ignored —
    /// never honored — on the cooperative lane, where scope is verified.
    #[serde(rename = "cooperativeId")]
    pub cooperative_id: Option<Uuid>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<i64>,
}

struct ParsedQuery {
    event_type: Option<String>,
    actor_user_id: Option<Uuid>,
    cooperative_id: Option<Uuid>,
    from: Option<OffsetDateTime>,
    to: Option<OffsetDateTime>,
    before_at: Option<OffsetDateTime>,
    before_id: Option<Uuid>,
    page_size: i64,
}

fn parse_query(query: AuditQuery, allow_coop_filter: bool) -> Result<ParsedQuery, ApiError> {
    if let Some(key) = &query.event_type {
        if !is_known_event_type(key) {
            return Err(ApiError::ValidationFailed);
        }
    }
    let from = parse_timestamp(query.from.as_deref())?;
    let to = parse_timestamp(query.to.as_deref())?;
    if let (Some(f), Some(t)) = (from, to) {
        if f > t {
            return Err(ApiError::ValidationFailed);
        }
    }
    let (before_at, before_id) = match query.cursor.as_deref() {
        None => (None, None),
        Some(raw) => decode_cursor(raw)?,
    };
    let page_size = query.page_size.unwrap_or(DEFAULT_PAGE_SIZE);
    if !(1..=MAX_PAGE_SIZE).contains(&page_size) {
        return Err(ApiError::ValidationFailed);
    }
    Ok(ParsedQuery {
        event_type: query.event_type,
        actor_user_id: query.actor_user_id,
        cooperative_id: if allow_coop_filter {
            query.cooperative_id
        } else {
            None
        },
        from,
        to,
        before_at,
        before_id,
        page_size,
    })
}

fn parse_timestamp(raw: Option<&str>) -> Result<Option<OffsetDateTime>, ApiError> {
    match raw {
        None => Ok(None),
        Some(value) => OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
            .map(Some)
            .map_err(|_| ApiError::ValidationFailed),
    }
}

// Opaque keyset cursor: base64url("<unix-micros>|<uuid>"). Malformed,
// wrong-shaped or non-decoding cursors are a client error, never a
// silent reset to page one.
fn decode_cursor(raw: &str) -> Result<(Option<OffsetDateTime>, Option<Uuid>), ApiError> {
    let bytes = Base64UrlUnpadded::decode_vec(raw).map_err(|_| ApiError::ValidationFailed)?;
    let text = String::from_utf8(bytes).map_err(|_| ApiError::ValidationFailed)?;
    let (micros, id) = text.split_once('|').ok_or(ApiError::ValidationFailed)?;
    let micros: i64 = micros.parse().map_err(|_| ApiError::ValidationFailed)?;
    let at = OffsetDateTime::from_unix_timestamp_nanos(micros as i128 * 1_000)
        .map_err(|_| ApiError::ValidationFailed)?;
    let id = Uuid::parse_str(id).map_err(|_| ApiError::ValidationFailed)?;
    Ok((Some(at), Some(id)))
}

fn encode_cursor(at: OffsetDateTime, id: Uuid) -> String {
    let micros = at.unix_timestamp_nanos() / 1_000;
    Base64UrlUnpadded::encode_string(format!("{micros}|{id}").as_bytes())
}

// ── DTO ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditEventDto {
    id: Uuid,
    occurred_at: String,
    event_type: String,
    actor_user_id: Option<Uuid>,
    actor_username: Option<String>,
    cooperative_id: Option<Uuid>,
    metadata: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditPageDto {
    items: Vec<AuditEventDto>,
    next_cursor: Option<String>,
}

struct AuditRow {
    id: Uuid,
    occurred_at: OffsetDateTime,
    event_type: String,
    actor_user_id: Option<Uuid>,
    actor_username: Option<String>,
    cooperative_id: Option<Uuid>,
    metadata: serde_json::Value,
}

fn to_dto(row: AuditRow) -> AuditEventDto {
    AuditEventDto {
        id: row.id,
        occurred_at: row
            .occurred_at
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| row.occurred_at.to_string()),
        event_type: row.event_type.clone(),
        actor_user_id: row.actor_user_id,
        actor_username: row.actor_username,
        cooperative_id: row.cooperative_id,
        metadata: filter_metadata(&row.event_type, row.metadata),
    }
}

/// We always fetch `page_size + 1` rows; a surplus row means a
/// continuation exists and the cursor points at the last RETURNED row.
fn to_page(mut rows: Vec<AuditRow>, page_size: i64) -> AuditPageDto {
    let next_cursor = if rows.len() as i64 > page_size {
        rows.truncate(page_size as usize);
        rows.last()
            .map(|row| encode_cursor(row.occurred_at, row.id))
    } else {
        None
    };
    AuditPageDto {
        items: rows.into_iter().map(to_dto).collect(),
        next_cursor,
    }
}

// ── Metadata allowlist (D1) ──────────────────────────────────────────
//
// Every key listed here was verified against its mint site
// (0057/0060 transition trigger, sealed lane writers, route
// record_tx callers, owner CLI mints). Unlisted keys are dropped;
// unknown event types degrade to `{}` — minimal metadata.
fn metadata_allowlist(event_type: &str) -> &'static [&'static str] {
    match event_type {
        // Authentication lane (platform events)
        "login_succeeded" => &["username"],
        "login_failed" => &["username", "reason"],
        "logout" => &[],
        "session_revoked" => &["source"],
        "sessions_revoked_all_others" => &["revoked_count"],
        "cooperative_context_switched" => &["cooperative_name"],
        // Identity / RBAC administration
        "user_created" => &["username", "source"],
        "user_disabled" | "user_enabled" => &["user_id"],
        "role_created" => &["role_id", "name"],
        "role_updated" => &["role_id", "name", "before", "after"],
        "role_enabled" | "role_disabled" => &["role_id"],
        "role_permissions_changed" => &["role_id", "before", "after"],
        "role_assigned" | "role_unassigned" => &["role_id", "role_name", "target_user_id"],
        "bootstrap_role_granted" => &[
            "target_user_id",
            "username",
            "role_id",
            "role_name",
            "source",
        ],
        "lockout_prevented" => &["operation", "target_id"],
        "admin_sessions_revoked" => &["target_user_id", "revoked_count"],
        // Tenancy lifecycle
        "cooperative_created" => &["name", "source"],
        "cooperative_bootstrapped" => &["name", "member_count", "source"],
        "cooperative_membership_granted" => &["username", "source"],
        "cooperative_membership_suspended"
        | "cooperative_membership_ended"
        | "cooperative_membership_reactivated" => &[
            "membership_id",
            "target_user_id",
            "cooperative_id",
            "from",
            "to",
        ],
        // Parties — person name before/after is the identity-change
        // record the audit exists for (D1: justified, coop-internal PII).
        "person_created" => &["person_id"],
        "person_updated" => &["person_id", "before", "after"],
        "family_created" => &["family_id"],
        "shareholder_created" => &["shareholder_id", "person_id"],
        "shareholder_updated" => &["shareholder_id", "before", "after", "change"],
        "shareholder_status_changed" => &["shareholder_id", "from", "to"],
        "shareholder_default_account_changed" => {
            &["shareholder_id", "default_collection_account_id"]
        }
        "shareholder_family_membership_ended" => {
            &["membership_id", "shareholder_id", "family_id", "reason"]
        }
        "shareholder_family_changed" => &[
            "shareholder_id",
            "from_family_sequence",
            "to_family_sequence",
            "to_family_id",
            "reason",
        ],
        // Shares
        "share_created" | "share_status_changed" => &["share_id", "share_number", "from", "to"],
        "share_transferred" | "share_sold" => &[
            "share_id",
            "share_number",
            "from_shareholder_id",
            "to_shareholder_id",
            "amount",
            "effective_at",
            "reason",
        ],
        // Periods & assessments
        "period_created" | "period_updated" | "period_opened" | "period_closed"
        | "period_deleted" => &["period_id", "period_number", "name"],
        "assessments_generated" => &[
            "period_id",
            "assessment_count",
            "share_source_count",
            "total_amount",
            "credit_applied_amount",
            "credit_application_count",
        ],
        // Payments & allocations — amounts are the audited fact.
        "payment_posted" => &["payment_id", "payment_number", "payer_person_id", "amount"],
        "payment_reversed" => &[
            "payment_id",
            "payment_number",
            "reason",
            "reversed_allocation_count",
        ],
        "payment_allocations_added" => &["payment_id", "allocation_id", "assessment_id", "amount"],
        "payment_allocation_reversed" => &["allocation_id", "payment_id", "reason"],
        // Financial accounts & transfers
        "financial_account_created" => &["account_id", "account_type", "name"],
        "financial_account_updated" => &["account_id"],
        "financial_account_status_changed" => &["account_id", "from", "to"],
        "account_transfer_posted" => &[
            "transfer_id",
            "transfer_number",
            "source_account_id",
            "destination_account_id",
            "amount",
        ],
        "account_transfer_reversed" => &["transfer_id", "transfer_number", "reason"],
        "financial_category_created" => &["category_id", "category_type", "name"],
        "financial_category_updated" => &["category_id"],
        "financial_category_status_changed" => &["category_id", "from", "to"],
        // Credits
        "shareholder_credit_assigned" => &[
            "credit_id",
            "credit_number",
            "shareholder_id",
            "amount",
            "reason",
        ],
        "shareholder_credit_reversed" => &["credit_id", "credit_number", "reason"],
        "credit_applied" => &[
            "application_id",
            "credit_id",
            "assessment_id",
            "amount",
            "mode",
        ],
        "credit_application_reversed" => &["application_id", "credit_id", "reason"],
        // Income / expense
        "income_posted" => &[
            "income_id",
            "income_number",
            "financial_account_id",
            "amount",
        ],
        "income_reversed" => &["income_id", "income_number", "reason"],
        "expense_posted" => &[
            "expense_id",
            "expense_number",
            "financial_account_id",
            "amount",
        ],
        "expense_reversed" => &["expense_id", "expense_number", "reason"],
        // Share returns
        "share_return_requested" => &[
            "return_id",
            "return_number",
            "share_id",
            "shareholder_id",
            "reason",
        ],
        "share_return_cancelled" => &["return_id", "return_number", "reason"],
        "share_return_finalized" => &["return_id", "return_number"],
        "share_return_entitlement_recognized" => &[
            "entitlement_id",
            "share_return_id",
            "entitlement_type",
            "amount",
            "reason",
        ],
        "share_return_entitlement_determined" => {
            &["entitlement_id", "entitlement_type", "amount", "due_date"]
        }
        "share_return_entitlement_cancelled" => &["entitlement_id", "reason"],
        "share_return_settlement_posted" => {
            &["settlement_id", "entitlement_id", "amount", "reason"]
        }
        "share_return_settlement_reversed" => &["settlement_id", "reason"],
        // Investments
        "investment_created" => &["investment_id", "investment_number", "name", "reason"],
        "investment_cancelled" => &["investment_id", "investment_number", "reason"],
        "investment_funding_posted" => &["funding_id", "investment_id", "amount", "reason"],
        "investment_funding_reversed" => &["funding_id", "reason"],
        "investment_valuation_recorded" => &[
            "valuation_id",
            "investment_id",
            "amount",
            "valuation_date",
            "reason",
        ],
        "investment_valuation_cancelled" => &["valuation_id", "reason"],
        "investment_income_posted" => &["income_id", "investment_id", "amount", "reason"],
        "investment_income_reversed" => &["income_id", "reason"],
        "investment_disposed" => &["disposal_id", "investment_id", "consideration_amount"],
        // Social aid — amount + fund identity only; beneficiary PII is
        // never minted into metadata (verified at the trigger).
        "social_aid_fund_created" => &["fund_id", "fund_number", "name"],
        "social_aid_fund_closed" => &["fund_id", "fund_number"],
        "social_aid_fund_cancelled" => &["fund_id", "fund_number", "reason"],
        "social_aid_donation_posted" => &["donation_id", "fund_id", "amount"],
        "social_aid_donation_reversed" => &["donation_id", "reason"],
        "social_aid_disbursement_posted" => &["disbursement_id", "fund_id", "amount"],
        "social_aid_disbursement_reversed" => &["disbursement_id", "reason"],
        // Governance — votes carry `choice`; member ids and counts are
        // the auditable record.
        "governance_body_created" => &["body_id", "body_number", "name"],
        "governance_body_closed" => &["body_id", "body_number"],
        "governance_membership_started" => &["membership_id", "body_id", "person_id", "title"],
        "governance_membership_ended" => &["membership_id", "body_id", "reason"],
        "governance_decision_created" => &["decision_id", "decision_number", "title"],
        "governance_decision_updated" => &["decision_id", "decision_number"],
        "governance_decision_opened" => &["decision_id", "decision_number"],
        "governance_decision_cancelled" => &["decision_id", "decision_number", "reason"],
        "governance_decision_finalized" => &[
            "decision_number",
            "outcome",
            "approve_count",
            "reject_count",
            "abstain_count",
        ],
        "governance_vote_recorded" => &["vote_id", "decision_id", "membership_id", "choice"],
        // D1: unknown or unlisted event types expose minimal metadata.
        _ => &[],
    }
}

/// Distinct from the allowlist: a KNOWN event type may still expose an
/// empty metadata allowlist (e.g. `logout`). Unknown types are rejected
/// at validation before any query runs.
fn is_known_event_type(key: &str) -> bool {
    KNOWN_EVENT_TYPES.contains(&key)
}

const KNOWN_EVENT_TYPES: &[&str] = &[
    "account_transfer_posted",
    "account_transfer_reversed",
    "admin_sessions_revoked",
    "assessments_generated",
    "bootstrap_role_granted",
    "cooperative_bootstrapped",
    "cooperative_context_switched",
    "cooperative_created",
    "cooperative_membership_ended",
    "cooperative_membership_granted",
    "cooperative_membership_reactivated",
    "cooperative_membership_suspended",
    "credit_application_reversed",
    "credit_applied",
    "expense_posted",
    "expense_reversed",
    "family_created",
    "financial_account_created",
    "financial_account_status_changed",
    "financial_account_updated",
    "financial_category_created",
    "financial_category_status_changed",
    "financial_category_updated",
    "governance_body_closed",
    "governance_body_created",
    "governance_decision_cancelled",
    "governance_decision_created",
    "governance_decision_finalized",
    "governance_decision_opened",
    "governance_decision_updated",
    "governance_membership_ended",
    "governance_membership_started",
    "governance_vote_recorded",
    "income_posted",
    "income_reversed",
    "investment_cancelled",
    "investment_created",
    "investment_disposed",
    "investment_funding_posted",
    "investment_funding_reversed",
    "investment_income_posted",
    "investment_income_reversed",
    "investment_valuation_cancelled",
    "investment_valuation_recorded",
    "lockout_prevented",
    "login_failed",
    "login_succeeded",
    "logout",
    "payment_allocation_reversed",
    "payment_allocations_added",
    "payment_posted",
    "payment_reversed",
    "period_closed",
    "period_created",
    "period_deleted",
    "period_opened",
    "period_updated",
    "person_created",
    "person_updated",
    "role_assigned",
    "role_created",
    "role_disabled",
    "role_enabled",
    "role_permissions_changed",
    "role_unassigned",
    "role_updated",
    "session_revoked",
    "sessions_revoked_all_others",
    "share_created",
    "share_return_cancelled",
    "share_return_entitlement_cancelled",
    "share_return_entitlement_determined",
    "share_return_entitlement_recognized",
    "share_return_finalized",
    "share_return_requested",
    "share_return_settlement_posted",
    "share_return_settlement_reversed",
    "share_sold",
    "share_status_changed",
    "share_transferred",
    "shareholder_created",
    "shareholder_credit_assigned",
    "shareholder_credit_reversed",
    "shareholder_default_account_changed",
    "shareholder_family_changed",
    "shareholder_family_membership_ended",
    "shareholder_status_changed",
    "shareholder_updated",
    "social_aid_disbursement_posted",
    "social_aid_disbursement_reversed",
    "social_aid_donation_posted",
    "social_aid_donation_reversed",
    "social_aid_fund_cancelled",
    "social_aid_fund_closed",
    "social_aid_fund_created",
    "user_created",
    "user_disabled",
    "user_enabled",
];

/// Project `metadata` down to the event-type allowlist. Nested objects
/// (before/after) pass through whole when their key is allowed — their
/// inner contents were reviewed at the mint site.
fn filter_metadata(event_type: &str, metadata: serde_json::Value) -> serde_json::Value {
    let keys = metadata_allowlist(event_type);
    let Some(map) = metadata.as_object() else {
        return serde_json::json!({});
    };
    let filtered: serde_json::Map<String, serde_json::Value> = map
        .iter()
        .filter(|(key, _)| keys.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    serde_json::Value::Object(filtered)
}

// ── Handlers ─────────────────────────────────────────────────────────

/// Cooperative lane: the cooperative comes from the verified
/// BusinessAuth scope. A `cooperativeId` parameter that differs from
/// the verified scope is rejected — this lane is never an enumeration
/// oracle for foreign cooperatives.
async fn list_events_scoped(
    State(state): State<AppState>,
    ctx: BusinessAuth,
    Query(query): Query<AuditQuery>,
) -> Result<Response, ApiError> {
    authz::require_business(&state, &ctx, authz::catalog::AUDIT_READ).await?;
    if let Some(claimed) = query.cooperative_id {
        if Some(claimed) != ctx.cooperative_id() {
            return Err(ApiError::PermissionDenied);
        }
    }
    let parsed = parse_query(query, false)?;
    let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;
    // `None` scope = the provably never-bootstrapped lane; the
    // definer's legacy arm serves NULL-attributed events there.
    let rows = read_scoped(
        pool,
        &ctx.auth.session_token_hash,
        ctx.cooperative_id(),
        &parsed,
        state.auth.clock.now(),
    )
    .await?;
    Ok(Json(to_page(rows, parsed.page_size)).into_response())
}

/// Platform lane (D4): no cooperative extraction at all. The definer
/// re-verifies the NULL-scope `audit.read` grant; a coop-scoped grant
/// or a coop-scoped session alone never suffices.
async fn list_events_platform(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Query(query): Query<AuditQuery>,
) -> Result<Response, ApiError> {
    let parsed = parse_query(query, true)?;
    let pool = state.db.as_ref().ok_or(ApiError::DependencyUnavailable)?;
    let rows = sqlx::query_as::<_, AuditRowDb>(
        "SELECT id, occurred_at, event_type, actor_user_id, \
                actor_username, cooperative_id, metadata \
           FROM public.app_audit_read_platform($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(&auth.session_token_hash[..])
    .bind(auth.authed_at)
    .bind(parsed.cooperative_id)
    .bind(&parsed.event_type)
    .bind(parsed.actor_user_id)
    .bind(parsed.from)
    .bind(parsed.to)
    .bind(parsed.before_at)
    .bind(parsed.before_id)
    .bind(parsed.page_size as i32 + 1)
    .fetch_all(pool)
    .await
    .map_err(map_reader_error)?;
    Ok(Json(to_page(
        rows.into_iter().map(Into::into).collect(),
        parsed.page_size,
    ))
    .into_response())
}

async fn read_scoped(
    pool: &PgPool,
    token_hash: &[u8; 32],
    cooperative_id: Option<Uuid>,
    parsed: &ParsedQuery,
    now: OffsetDateTime,
) -> Result<Vec<AuditRow>, ApiError> {
    let rows = sqlx::query_as::<_, AuditRowDb>(
        "SELECT id, occurred_at, event_type, actor_user_id, \
                actor_username, cooperative_id, metadata \
           FROM public.app_audit_read_scoped($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(&token_hash[..])
    .bind(cooperative_id)
    .bind(now)
    .bind(&parsed.event_type)
    .bind(parsed.actor_user_id)
    .bind(parsed.from)
    .bind(parsed.to)
    .bind(parsed.before_at)
    .bind(parsed.before_id)
    .bind(parsed.page_size as i32 + 1)
    .fetch_all(pool)
    .await
    .map_err(map_reader_error)?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// insufficient_privilege (42501) from the reader means authentication
/// expired between extraction and the definer call, or the verified
/// grant is missing — always a 403, never a 404 oracle.
fn map_reader_error(error: sqlx::Error) -> ApiError {
    match error {
        sqlx::Error::Database(db_err) if db_err.code().as_deref() == Some("42501") => {
            ApiError::PermissionDenied
        }
        other => {
            tracing::error!(error = %other, "audit read failed");
            ApiError::Internal
        }
    }
}

#[derive(sqlx::FromRow)]
struct AuditRowDb {
    id: Uuid,
    occurred_at: OffsetDateTime,
    event_type: String,
    actor_user_id: Option<Uuid>,
    actor_username: Option<String>,
    cooperative_id: Option<Uuid>,
    metadata: serde_json::Value,
}

impl From<AuditRowDb> for AuditRow {
    fn from(row: AuditRowDb) -> Self {
        AuditRow {
            id: row.id,
            occurred_at: row.occurred_at,
            event_type: row.event_type,
            actor_user_id: row.actor_user_id,
            actor_username: row.actor_username,
            cooperative_id: row.cooperative_id,
            metadata: row.metadata,
        }
    }
}
