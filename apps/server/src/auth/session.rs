//! Server-side session store (STEP-002 §10–§15, ADR-002).
//!
//! - Cookie carries only the raw opaque token; DB stores its SHA-256.
//! - Absolute expiry (`expires_at`) and idle expiry (`idle_expires_at`)
//!   are both enforced server-side on every authenticated request, using
//!   server timestamps as the only authority.
//! - Last-seen write amplification is bounded: a session is touched at
//!   most once per `TOUCH_INTERVAL` (5 minutes). Idle-timeout semantics
//!   are preserved: touch only happens for requests that arrive within
//!   the idle window, so an actually-idle session still expires on time.
//! - A disabled User invalidates existing sessions at validation time
//!   (strategy B of STEP-002 §6): the session lookup joins `users` and
//!   requires `status = 'active'`. No destructive cascade needed.
//! - All mutations are idempotent where callers may retry (revocation
//!   only fills `revoked_at` when NULL).

use time::OffsetDateTime;
use uuid::Uuid;

/// Minimum age of `last_seen_at` before a request touches it again.
pub const TOUCH_INTERVAL_SECS: i64 = 300;

#[derive(Debug, sqlx::FromRow)]
pub struct SessionRow {
    pub id: Uuid,
    pub user_id: Uuid,
    #[allow(dead_code)]
    pub token_hash: Vec<u8>,
    pub csrf_token: String,
    pub created_at: OffsetDateTime,
    pub last_seen_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub idle_expires_at: OffsetDateTime,
    pub revoked_at: Option<OffsetDateTime>,
    #[allow(dead_code)]
    pub revocation_reason: Option<String>,
    pub client_label: Option<String>,
    /// UX-level default cooperative (M1-K1): preselects the tenant
    /// context when a request carries no explicit `x-cooperative-id`.
    /// Never trusted alone — `TenantCtx` re-validates membership.
    pub active_cooperative_id: Option<Uuid>,
}

/// Why a presented token does not yield an authenticated session.
#[derive(Debug, PartialEq, Eq)]
pub enum SessionRejection {
    /// No session with that token hash at all.
    Unknown,
    Revoked,
    Expired,
    IdleExpired,
    UserDisabled,
}

/// Inputs of `create_session` (grouped to keep the call site readable).
pub struct NewSession<'a> {
    pub user_id: Uuid,
    pub token_hash: [u8; 32],
    pub csrf_token: &'a str,
    pub absolute_ttl_secs: i64,
    pub idle_ttl_secs: i64,
    pub client_label: Option<String>,
    pub now: OffsetDateTime,
}

pub async fn create_session(
    pool: &sqlx::PgPool,
    init: NewSession<'_>,
) -> Result<SessionRow, sqlx::Error> {
    let expires_at = init.now + time::Duration::seconds(init.absolute_ttl_secs);
    let idle_expires_at = init.now + time::Duration::seconds(init.idle_ttl_secs);
    let row = sqlx::query_as::<_, SessionRow>(
        "INSERT INTO user_sessions \
             (user_id, token_hash, csrf_token, created_at, last_seen_at, \
              expires_at, idle_expires_at, client_label) \
         VALUES ($1, $2, $3, $4, $4, $5, $6, $7) \
         RETURNING id, user_id, token_hash, csrf_token, created_at, \
                   last_seen_at, expires_at, idle_expires_at, revoked_at, \
                   revocation_reason, client_label, active_cooperative_id",
    )
    .bind(init.user_id)
    .bind(init.token_hash.as_slice())
    .bind(init.csrf_token)
    .bind(init.now)
    .bind(expires_at)
    .bind(idle_expires_at)
    .bind(init.client_label)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Authenticated session projection returned by
/// `app_session_resolve`. The runtime principal has no table privilege
/// on `user_sessions` (M1-P4-P3B); all session operations go through
/// credential-bound SECURITY DEFINER functions that never return or
/// enumerate `token_hash`.
#[derive(Debug)]
pub struct AuthenticatedSession {
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub username: String,
    pub display_name: String,
    pub csrf_token: String,
    pub created_at: OffsetDateTime,
    pub last_seen_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub active_cooperative_id: Option<Uuid>,
}

#[derive(Debug, sqlx::FromRow)]
struct ResolveRow {
    rejection: Option<String>,
    session_id: Option<Uuid>,
    user_id: Option<Uuid>,
    username: Option<String>,
    display_name: Option<String>,
    #[allow(dead_code)]
    user_status: Option<String>,
    csrf_token: Option<String>,
    created_at: Option<OffsetDateTime>,
    last_seen_at: Option<OffsetDateTime>,
    expires_at: Option<OffsetDateTime>,
    #[allow(dead_code)]
    idle_expires_at: Option<OffsetDateTime>,
    active_cooperative_id: Option<Uuid>,
    #[allow(dead_code)]
    client_label: Option<String>,
}

/// Resolve a presented token hash through `app_session_resolve`:
/// validity classification happens inside the definer function so the
/// runtime role never reads the session table.
pub async fn classify_session(
    pool: &sqlx::PgPool,
    token_hash: [u8; 32],
    now: OffsetDateTime,
) -> Result<Result<AuthenticatedSession, SessionRejection>, sqlx::Error> {
    let row = sqlx::query_as::<_, ResolveRow>(
        "SELECT rejection, session_id, user_id, username, display_name, \
                user_status, csrf_token, created_at, last_seen_at, expires_at, \
                idle_expires_at, active_cooperative_id, client_label \
         FROM public.app_session_resolve($1, $2)",
    )
    .bind(token_hash.as_slice())
    .bind(now)
    .fetch_one(pool)
    .await?;

    if let Some(rejection) = row.rejection.as_deref() {
        return Ok(Err(match rejection {
            "revoked" => SessionRejection::Revoked,
            "expired" => SessionRejection::Expired,
            "idle_expired" => SessionRejection::IdleExpired,
            "user_disabled" => SessionRejection::UserDisabled,
            _ => SessionRejection::Unknown,
        }));
    }
    Ok(Ok(AuthenticatedSession {
        session_id: row.session_id.unwrap_or_default(),
        user_id: row.user_id.unwrap_or_default(),
        username: row.username.unwrap_or_default(),
        display_name: row.display_name.unwrap_or_default(),
        csrf_token: row.csrf_token.unwrap_or_default(),
        created_at: row.created_at.unwrap_or(now),
        last_seen_at: row.last_seen_at.unwrap_or(now),
        expires_at: row.expires_at.unwrap_or(now),
        active_cooperative_id: row.active_cooperative_id,
    }))
}

/// Bounded last-seen refresh via `app_session_touch` — bound to the
/// presented token hash; touches at most once per TOUCH_INTERVAL.
pub async fn touch_if_needed(
    pool: &sqlx::PgPool,
    token_hash: [u8; 32],
    idle_ttl_secs: i64,
    now: OffsetDateTime,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>("SELECT public.app_session_touch($1, $2, $3, $4)")
        .bind(token_hash.as_slice())
        .bind(now)
        .bind(idle_ttl_secs as i32)
        .bind(TOUCH_INTERVAL_SECS as i32)
        .fetch_one(pool)
        .await
}

/// Result of a credential-bound session revocation.
#[derive(Debug, PartialEq, Eq)]
pub enum RevokeOutcome {
    Revoked,
    /// Own session already revoked — idempotent 204 for the caller.
    AlreadyRevoked,
    /// Foreign or unknown — caller answers an indistinguishable 404.
    NotFound,
}

/// Revoke one session owned by the presented token hash's session.
pub async fn revoke_session(
    pool: &sqlx::PgPool,
    token_hash: [u8; 32],
    session_id: Uuid,
    reason: &str,
    now: OffsetDateTime,
) -> Result<RevokeOutcome, sqlx::Error> {
    let outcome =
        sqlx::query_scalar::<_, String>("SELECT public.app_session_revoke($1, $2, $3, $4)")
            .bind(token_hash.as_slice())
            .bind(session_id)
            .bind(now)
            .bind(reason)
            .fetch_one(pool)
            .await?;
    Ok(match outcome.as_str() {
        "revoked" => RevokeOutcome::Revoked,
        "already_revoked" => RevokeOutcome::AlreadyRevoked,
        _ => RevokeOutcome::NotFound,
    })
}

/// Revoke every other active session of the hash owner. Returns count.
pub async fn revoke_all_other_sessions(
    pool: &sqlx::PgPool,
    token_hash: [u8; 32],
    reason: &str,
    now: OffsetDateTime,
) -> Result<u64, sqlx::Error> {
    let count = sqlx::query_scalar::<_, i64>("SELECT public.app_session_revoke_others($1, $2, $3)")
        .bind(token_hash.as_slice())
        .bind(now)
        .bind(reason)
        .fetch_one(pool)
        .await?;
    Ok(count.max(0) as u64)
}

/// Session summary for the self-service listing (never carries the
/// token hash).
#[derive(Debug, sqlx::FromRow)]
pub struct OwnedSessionRow {
    pub id: Uuid,
    pub created_at: OffsetDateTime,
    pub last_seen_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub client_label: Option<String>,
    pub is_current: bool,
}

/// Active sessions of the presented hash's owner, newest first.
pub async fn list_active_sessions(
    pool: &sqlx::PgPool,
    token_hash: [u8; 32],
    now: OffsetDateTime,
) -> Result<Vec<OwnedSessionRow>, sqlx::Error> {
    sqlx::query_as::<_, OwnedSessionRow>(
        "SELECT id, created_at, last_seen_at, expires_at, client_label, \
                is_current \
         FROM public.app_session_list($1, $2) ORDER BY created_at DESC",
    )
    .bind(token_hash.as_slice())
    .bind(now)
    .fetch_all(pool)
    .await
}

/// Session created by a credential-bound login.
#[derive(Debug, sqlx::FromRow)]
pub struct OpenedSession {
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub username: String,
    pub display_name: String,
    pub active_cooperative_id: Option<Uuid>,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

/// Inputs of `open_session` (grouped to keep the call site readable).
pub struct LoginAttempt<'a> {
    pub username: &'a str,
    pub password: &'a str,
    pub token_hash: [u8; 32],
    pub csrf_token: &'a str,
    pub absolute_ttl_secs: i64,
    pub idle_ttl_secs: i64,
    pub client_label: Option<String>,
    pub now: OffsetDateTime,
}

/// Credential-bound login: `app_session_open` verifies the plaintext
/// password against `users.login_verifier` inside the database, writes
/// the failed-login audit row itself, and returns the created session
/// only on success — a uniform `None` on any credential failure.
pub async fn open_session(
    pool: &sqlx::PgPool,
    attempt: LoginAttempt<'_>,
) -> Result<Option<OpenedSession>, sqlx::Error> {
    sqlx::query_as::<_, OpenedSession>(
        "SELECT session_id, user_id, username, display_name, \
                active_cooperative_id, created_at, expires_at \
         FROM public.app_session_open($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(attempt.username)
    .bind(attempt.password)
    .bind(attempt.token_hash.as_slice())
    .bind(attempt.csrf_token)
    .bind(attempt.now)
    .bind(attempt.absolute_ttl_secs as i32)
    .bind(attempt.idle_ttl_secs as i32)
    .bind(attempt.client_label)
    .fetch_optional(pool)
    .await
}

/// Coarse, privacy-conscious client label from a User-Agent header:
/// browser family + OS family, no versions, no raw UA storage, no
/// fingerprinting (STEP-002 §30/§48).
pub fn coarse_client_label(user_agent: Option<&str>) -> Option<String> {
    let ua = user_agent?.to_ascii_lowercase();
    if ua.is_empty() {
        return None;
    }
    let browser = if ua.contains("edg/") {
        "Edge"
    } else if ua.contains("firefox/") || ua.contains("fxios/") {
        "Firefox"
    } else if ua.contains("chrome/") || ua.contains("crios/") {
        "Chrome"
    } else if ua.contains("safari/") {
        "Safari"
    } else {
        "Diğer tarayıcı"
    };
    let os = if ua.contains("windows") {
        "Windows"
    } else if ua.contains("android") {
        "Android"
    } else if ua.contains("iphone") || ua.contains("ipad") {
        "iOS"
    } else if ua.contains("mac os") || ua.contains("macintosh") {
        "macOS"
    } else if ua.contains("linux") {
        "Linux"
    } else {
        "Bilinmeyen"
    };
    Some(format!("{browser} / {os}"))
}
