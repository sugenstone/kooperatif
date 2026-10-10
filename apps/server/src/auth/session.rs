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

use crate::auth::users::UserRow;

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

/// Find a session row by token hash (no validity checks).
pub async fn find_by_token_hash(
    pool: &sqlx::PgPool,
    token_hash: [u8; 32],
) -> Result<Option<SessionRow>, sqlx::Error> {
    sqlx::query_as::<_, SessionRow>(
        "SELECT id, user_id, token_hash, csrf_token, created_at, last_seen_at, \
                expires_at, idle_expires_at, revoked_at, revocation_reason, client_label, \
                active_cooperative_id \
         FROM user_sessions WHERE token_hash = $1",
    )
    .bind(token_hash.as_slice())
    .fetch_optional(pool)
    .await
}

/// Look up a session by token hash, load its owning User, and classify
/// validity (revoked / expired / idle / disabled). Two indexed point
/// lookups; classification happens in Rust so error semantics and audit
/// reasons stay explicit.
pub async fn classify_session(
    pool: &sqlx::PgPool,
    token_hash: [u8; 32],
    now: OffsetDateTime,
) -> Result<Result<(SessionRow, UserRow), SessionRejection>, sqlx::Error> {
    let Some(session) = find_by_token_hash(pool, token_hash).await? else {
        return Ok(Err(SessionRejection::Unknown));
    };
    let user = crate::auth::users::find_by_id(pool, session.user_id).await?;
    let Some(user) = user else {
        // FK integrity guarantees the user exists; treat defensively.
        return Ok(Err(SessionRejection::Unknown));
    };
    if session.revoked_at.is_some() {
        return Ok(Err(SessionRejection::Revoked));
    }
    if session.expires_at <= now {
        return Ok(Err(SessionRejection::Expired));
    }
    if session.idle_expires_at <= now {
        return Ok(Err(SessionRejection::IdleExpired));
    }
    if user.status != "active" {
        return Ok(Err(SessionRejection::UserDisabled));
    }
    Ok(Ok((session, user)))
}

/// Bounded touch: refresh `last_seen_at`/`idle_expires_at` only when the
/// stored last-seen is older than TOUCH_INTERVAL.
pub async fn touch_if_needed(
    pool: &sqlx::PgPool,
    session: &SessionRow,
    idle_ttl_secs: i64,
    now: OffsetDateTime,
) -> Result<bool, sqlx::Error> {
    let last_seen_age = (now - session.last_seen_at).whole_seconds();
    if last_seen_age < TOUCH_INTERVAL_SECS {
        return Ok(false);
    }
    let idle_expires_at = now + time::Duration::seconds(idle_ttl_secs);
    let result = sqlx::query(
        "UPDATE user_sessions SET last_seen_at = $2, idle_expires_at = $3 \
         WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(session.id)
    .bind(now)
    .bind(idle_expires_at)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Idempotent revocation of one session (double-revoke is safe).
pub async fn revoke_session(
    pool: &sqlx::PgPool,
    session_id: Uuid,
    owned_by: Option<Uuid>,
    reason: &str,
    now: OffsetDateTime,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE user_sessions SET revoked_at = $3, revocation_reason = $4 \
         WHERE id = $1 AND revoked_at IS NULL AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(session_id)
    .bind(owned_by)
    .bind(now)
    .bind(reason)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Revoke every other active session of a user. Returns the count.
pub async fn revoke_all_other_sessions(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    except_session_id: Uuid,
    reason: &str,
    now: OffsetDateTime,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE user_sessions SET revoked_at = $3, revocation_reason = $4 \
         WHERE user_id = $1 AND id <> $2 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .bind(except_session_id)
    .bind(now)
    .bind(reason)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Active (non-revoked, non-expired) sessions of a user, newest first.
pub async fn list_active_sessions(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    now: OffsetDateTime,
) -> Result<Vec<SessionRow>, sqlx::Error> {
    sqlx::query_as::<_, SessionRow>(
        "SELECT id, user_id, token_hash, csrf_token, created_at, last_seen_at, \
                expires_at, idle_expires_at, revoked_at, revocation_reason, client_label, \
                active_cooperative_id \
         FROM user_sessions \
         WHERE user_id = $1 AND revoked_at IS NULL \
           AND expires_at > $2 AND idle_expires_at > $2 \
         ORDER BY created_at DESC",
    )
    .bind(user_id)
    .bind(now)
    .fetch_all(pool)
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
