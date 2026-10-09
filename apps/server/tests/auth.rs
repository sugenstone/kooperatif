//! Database-gated authentication integration tests (STEP-002 §37–§43).
//!
//! Requires `KOOPERATIF_TEST_DATABASE_URL` pointing at a disposable
//! database (scripts/db-verify.sh and the CI database job provision a
//! clean one). Without the variable every test skips with an explicit
//! notice.
//!
//! Determinism: session/expiry/rate-limit time comes from an injected
//! MutableClock; database rows are manipulated directly instead of
//! wall-clock sleeps. Argon2 uses the enforced floor (still Argon2id,
//! m=8192) to keep the suite fast.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use sqlx::PgPool;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

use kooperatif_server::auth::{identity, users, AuthRuntime};
use kooperatif_server::clock::MutableClock;
use kooperatif_server::config::{AuthConfig, ARGON2_M_COST_FLOOR};
use kooperatif_server::db as app_db;
use kooperatif_server::http::{cors_layer, router, AppState};

const ORIGIN: &str = "http://localhost:5173";
const EVIL_ORIGIN: &str = "https://evil.example";
const TEST_PASSWORD: &str = "cok-gizli-parola-123";

/// Required DB-gated test gate (PILOT-FIX-001 / F5): a missing
/// KOOPERATIF_TEST_DATABASE_URL is an explicit FAILURE, never a
/// silent skip — CI and release gates must prove these tests ran.
/// Unit-only execution stays unaffected: this function is only
/// reached by database-gated setup paths.
fn test_database_url() -> Option<String> {
    let value = std::env::var("KOOPERATIF_TEST_DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "KOOPERATIF_TEST_DATABASE_URL is not set — required DB-gated              integration tests cannot silently pass; point it at a              disposable PostgreSQL database"
        )
    });
    let trimmed = value.trim().to_string();
    assert!(
        !trimmed.is_empty(),
        "KOOPERATIF_TEST_DATABASE_URL is empty — required DB-gated tests need a database"
    );
    Some(trimmed)
}

fn auth_config(username_max: u32, ip_max: u32) -> AuthConfig {
    AuthConfig {
        allowed_origins: vec![ORIGIN.to_string()],
        cookie_secure: false,
        session_absolute_ttl_secs: 3600,
        session_idle_ttl_secs: 600,
        argon2_m_cost: ARGON2_M_COST_FLOOR,
        argon2_t_cost: 1,
        argon2_p_cost: 1,
        login_window_secs: 60,
        login_username_max_attempts: username_max,
        login_ip_max_attempts: ip_max,
        forwarded_ip: false,
    }
}

struct TestApp {
    app: axum::Router,
    pool: PgPool,
    clock: Arc<MutableClock>,
    peer: SocketAddr,
}

impl TestApp {
    fn peer(&self) -> SocketAddr {
        self.peer
    }
}

/// Unique per-test peer address: tests run concurrently against one
/// database, and the login limiter keys on client IP — a shared fake IP
/// would let parallel tests throttle each other.
fn unique_peer() -> SocketAddr {
    let bytes = Uuid::new_v4();
    let octets = bytes.as_bytes();
    format!(
        "127.{}.{}.{}:{}",
        octets[0],
        octets[1],
        octets[2],
        40000 + (octets[3] as u16)
    )
    .parse()
    .expect("derived addr")
}

async fn setup(auth: AuthConfig) -> Option<TestApp> {
    let url = test_database_url()?;
    let pool = app_db::connect(&url).await.expect("database connectivity");
    app_db::run_migrations(&pool)
        .await
        .expect("migrations applied");
    let clock = Arc::new(MutableClock::new(
        OffsetDateTime::parse(
            "2026-01-01T00:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .expect("fixed epoch"),
    ));
    let runtime = AuthRuntime::new(auth, clock.clone());
    let app = router(
        AppState {
            db: Some(pool.clone()),
            auth: Arc::new(runtime),
        },
        cors_layer(&[]),
    );
    Some(TestApp {
        app,
        pool,
        clock,
        peer: unique_peer(),
    })
}

fn test_argon2() -> argon2::Argon2<'static> {
    argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).expect("test parameters"),
    )
}

/// Create a user directly (test factory); returns (user_id, username).
async fn create_test_user(pool: &PgPool, username: &str, password: &str) -> (Uuid, String) {
    let argon2 = test_argon2();
    let hash = identity::hash_password(&argon2, password).expect("hash");
    let user = users::create_user(pool, username, "Test Kullanıcı", &hash)
        .await
        .expect("user created");
    (user.id, user.username)
}

/// POST /api/auth/login with browser-shaped headers.
async fn login(
    app: &axum::Router,
    peer: SocketAddr,
    username: &str,
    password: &str,
    origin: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("user-agent", "Mozilla/5.0 (Windows NT 10.0) Chrome/126.0");
    if let Some(origin) = origin {
        builder = builder.header("origin", origin);
    }
    let mut request = builder
        .body(Body::from(
            serde_json::json!({ "username": username, "password": password }).to_string(),
        ))
        .expect("valid request");
    request.extensions_mut().insert(ConnectInfo(peer));
    app.clone().oneshot(request).await.expect("infallible")
}

/// Consume a response into (status, json, set-cookie) in one step.
async fn into_parts(response: axum::response::Response) -> (StatusCode, Value, Option<String>) {
    let status = response.status();
    let set_cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("readable body");
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json, set_cookie)
}

fn cookie_token(set_cookie: &str) -> String {
    set_cookie
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .map(|(_, value)| value.to_string())
        .expect("cookie pair")
}

fn csrf_of(json: &Value) -> String {
    json["csrfToken"]
        .as_str()
        .expect("csrfToken in body")
        .to_string()
}

fn session_id_of(json: &Value) -> Uuid {
    json["session"]["id"]
        .as_str()
        .expect("session id")
        .parse()
        .expect("uuid")
}

/// Build an authenticated request with browser-shaped cookie/CSRF/Origin.
fn authed(
    method: &axum::http::Method,
    path: &str,
    cookie_token_value: &str,
    csrf: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method.clone())
        .uri(path)
        .header("cookie", format!("kooperatif_session={cookie_token_value}"))
        .header("origin", ORIGIN);
    if let Some(csrf) = csrf {
        builder = builder.header("x-csrf-token", csrf);
    }
    builder.body(Body::empty()).expect("valid request")
}

async fn get_status(app: &axum::Router, path: &str, cookie_token_value: &str) -> StatusCode {
    app.clone()
        .oneshot(authed(
            &axum::http::Method::GET,
            path,
            cookie_token_value,
            None,
        ))
        .await
        .expect("infallible")
        .status()
}

// ------------------------------------------------------------------
// §37/§38: passwords, hashing, login
// ------------------------------------------------------------------

#[tokio::test]
async fn bootstrap_user_creation_and_login_roundtrip() {
    let Some(test) = setup(auth_config(50, 10_000)).await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let username = format!("ali.{}", Uuid::new_v4().simple());
    let (user_id, _normalized) = create_test_user(&test.pool, &username, TEST_PASSWORD).await;

    // Plaintext is never stored; stored form is Argon2id PHC.
    let stored: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&test.pool)
        .await
        .expect("hash readable");
    assert!(stored.starts_with("$argon2id$"));
    assert_ne!(stored, TEST_PASSWORD);
    assert!(!stored.contains(TEST_PASSWORD));

    let response = login(
        &test.app,
        test.peer(),
        &username,
        TEST_PASSWORD,
        Some(ORIGIN),
    )
    .await;
    let (status, json, _) = into_parts(response).await;
    assert_eq!(status, StatusCode::OK, "login must succeed: {json}");
    assert_eq!(json["user"]["username"], username.to_lowercase());

    // No secrets in JSON: no password/hash/token-hash/cookie fields.
    let serialized = json.to_string();
    assert!(
        !serialized.contains("password"),
        "no password fields: {serialized}"
    );
    assert!(!serialized.contains("hash"), "no hash fields: {serialized}");
    assert!(
        !serialized.contains("kooperatif_session"),
        "no cookie values: {serialized}"
    );
}

#[tokio::test]
async fn login_sets_expected_cookie_attributes() {
    let Some(test) = setup(auth_config(50, 10_000)).await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let username = format!("veli.{}", Uuid::new_v4().simple());
    create_test_user(&test.pool, &username, TEST_PASSWORD).await;

    let response = login(
        &test.app,
        test.peer(),
        &username,
        TEST_PASSWORD,
        Some(ORIGIN),
    )
    .await;
    let cache_control = response
        .headers()
        .get("cache-control")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let (_, _, set_cookie) = into_parts(response).await;
    assert_eq!(
        cache_control.as_deref(),
        Some("no-store"),
        "auth responses must never be cacheable"
    );
    let set_cookie = set_cookie.expect("set-cookie present");
    assert!(
        set_cookie.contains("HttpOnly"),
        "cookie must be HttpOnly: {set_cookie}"
    );
    assert!(
        set_cookie.contains("SameSite=Lax"),
        "cookie SameSite: {set_cookie}"
    );
    assert!(set_cookie.contains("Path=/"), "cookie Path: {set_cookie}");
    assert!(
        set_cookie.contains("Max-Age=3600"),
        "cookie Max-Age: {set_cookie}"
    );
    assert!(
        !set_cookie.contains("Secure"),
        "dev cookie is not Secure: {set_cookie}"
    );
}

#[tokio::test]
async fn login_failures_are_identical_and_generic() {
    let Some(test) = setup(auth_config(50, 10_000)).await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let username = format!("ayse.{}", Uuid::new_v4().simple());
    create_test_user(&test.pool, &username, TEST_PASSWORD).await;

    let wrong = login(
        &test.app,
        test.peer(),
        &username,
        "baska-bir-parola-9",
        Some(ORIGIN),
    )
    .await;
    let (wrong_status, wrong_body, _) = into_parts(wrong).await;

    let unknown = login(
        &test.app,
        test.peer(),
        "boyle-biri-yok-4711",
        "herhangi-parola-9",
        Some(ORIGIN),
    )
    .await;
    let (unknown_status, unknown_body, _) = into_parts(unknown).await;

    // Wrong password and unknown username are externally IDENTICAL.
    assert_eq!(wrong_status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown_status, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong_body, unknown_body);
    assert_eq!(wrong_body["error"]["code"], "authentication_failed");

    // Internally, the audit records the distinct safe reason categories.
    let reasons: Vec<String> = sqlx::query_scalar(
        "SELECT metadata->>'reason' FROM security_events \
         WHERE event_type = 'login_failed' ORDER BY occurred_at",
    )
    .fetch_all(&test.pool)
    .await
    .expect("audit readable");
    assert!(
        reasons.iter().any(|r| r == "invalid_credentials"),
        "{reasons:?}"
    );
    assert!(
        reasons.iter().any(|r| r == "unknown_username"),
        "{reasons:?}"
    );
}

#[tokio::test]
async fn disabled_user_cannot_login_nor_use_old_session() {
    let Some(test) = setup(auth_config(50, 10_000)).await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let username = format!("kadir.{}", Uuid::new_v4().simple());
    let (user_id, _) = create_test_user(&test.pool, &username, TEST_PASSWORD).await;

    let response = login(
        &test.app,
        test.peer(),
        &username,
        TEST_PASSWORD,
        Some(ORIGIN),
    )
    .await;
    let (status, _, set_cookie) = into_parts(response).await;
    assert_eq!(status, StatusCode::OK);
    let cookie = cookie_token(&set_cookie.expect("cookie"));

    // Disable the user (test flips status directly; no admin UI in scope).
    sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(user_id)
        .execute(&test.pool)
        .await
        .expect("disable");

    // Old session no longer authenticates (strategy B, STEP-002 §6).
    assert_eq!(
        get_status(&test.app, "/api/auth/me", &cookie).await,
        StatusCode::UNAUTHORIZED
    );

    // Fresh login with correct credentials also fails, generically.
    let response = login(
        &test.app,
        test.peer(),
        &username,
        TEST_PASSWORD,
        Some(ORIGIN),
    )
    .await;
    let (status, json, _) = into_parts(response).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{json}");
    assert_eq!(json["error"]["code"], "authentication_failed");

    let reasons: Vec<String> = sqlx::query_scalar(
        "SELECT metadata->>'reason' FROM security_events WHERE event_type = 'login_failed'",
    )
    .fetch_all(&test.pool)
    .await
    .expect("audit readable");
    assert!(reasons.iter().any(|r| r == "user_disabled"), "{reasons:?}");
}

// ------------------------------------------------------------------
// §15/§39: fixation, session lifecycle
// ------------------------------------------------------------------

#[tokio::test]
async fn login_creates_fresh_session_and_never_reuses_attacker_cookie() {
    let Some(test) = setup(auth_config(50, 10_000)).await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let username = format!("selin.{}", Uuid::new_v4().simple());
    create_test_user(&test.pool, &username, TEST_PASSWORD).await;

    let attacker_cookie = "saldirganin-secmis-oldugu-deger";
    let response = login(
        &test.app,
        test.peer(),
        &username,
        TEST_PASSWORD,
        Some(ORIGIN),
    )
    .await;
    let (status, json, set_cookie) = into_parts(response).await;
    assert_eq!(status, StatusCode::OK);
    let fresh_cookie = cookie_token(&set_cookie.expect("cookie"));
    assert_ne!(fresh_cookie, attacker_cookie);

    // Attacker's chosen value never authenticates.
    assert_eq!(
        get_status(&test.app, "/api/auth/me", attacker_cookie).await,
        StatusCode::UNAUTHORIZED
    );

    // The fresh one does, and /me returns the same csrfToken (so a page
    // refresh can restore mutation capability — §31).
    let me = test
        .app
        .clone()
        .oneshot(authed(
            &axum::http::Method::GET,
            "/api/auth/me",
            &fresh_cookie,
            None,
        ))
        .await
        .expect("infallible");
    assert_eq!(me.status(), StatusCode::OK);
    let (_, me_json, _) = into_parts(me).await;
    assert_eq!(me_json["csrfToken"], json["csrfToken"]);
}

#[tokio::test]
async fn session_rejections_cover_the_full_lifecycle() {
    let Some(test) = setup(auth_config(50, 10_000)).await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let username = format!("murat.{}", Uuid::new_v4().simple());
    create_test_user(&test.pool, &username, TEST_PASSWORD).await;

    // Missing session entirely.
    let me = test
        .app
        .clone()
        .oneshot(
            Request::get("/api/auth/me")
                .header("origin", ORIGIN)
                .body(Body::empty())
                .expect("valid"),
        )
        .await
        .expect("infallible");
    let (status, body, _) = into_parts(me).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "authentication_required");

    // Unknown token.
    assert_eq!(
        get_status(&test.app, "/api/auth/me", "hic-yok-boyle-token").await,
        StatusCode::UNAUTHORIZED
    );

    // Valid baseline session.
    let response = login(
        &test.app,
        test.peer(),
        &username,
        TEST_PASSWORD,
        Some(ORIGIN),
    )
    .await;
    let (_, json, set_cookie) = into_parts(response).await;
    let cookie = cookie_token(&set_cookie.expect("cookie"));
    let session_id = session_id_of(&json);

    // Absolute expiry: shift the stored timestamp relative to the session
    // itself (the app clock is frozen at creation time), never DB now().
    sqlx::query(
        "UPDATE user_sessions SET expires_at = created_at - interval '1 second' WHERE id = $1",
    )
    .bind(session_id)
    .execute(&test.pool)
    .await
    .expect("expire");
    let me = test
        .app
        .clone()
        .oneshot(authed(
            &axum::http::Method::GET,
            "/api/auth/me",
            &cookie,
            None,
        ))
        .await
        .expect("infallible");
    let (status, body, _) = into_parts(me).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "session_expired");

    // Idle expiry.
    sqlx::query(
        "UPDATE user_sessions SET expires_at = created_at + interval '1 hour', \
         idle_expires_at = created_at - interval '1 second' WHERE id = $1",
    )
    .bind(session_id)
    .execute(&test.pool)
    .await
    .expect("idle-expire");
    let me = test
        .app
        .clone()
        .oneshot(authed(
            &axum::http::Method::GET,
            "/api/auth/me",
            &cookie,
            None,
        ))
        .await
        .expect("infallible");
    let (status, body, _) = into_parts(me).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "session_expired");

    // Revoked session.
    sqlx::query(
        "UPDATE user_sessions SET idle_expires_at = created_at + interval '10 minutes', \
         revoked_at = created_at WHERE id = $1",
    )
    .bind(session_id)
    .execute(&test.pool)
    .await
    .expect("revoke");
    let me = test
        .app
        .clone()
        .oneshot(authed(
            &axum::http::Method::GET,
            "/api/auth/me",
            &cookie,
            None,
        ))
        .await
        .expect("infallible");
    let (status, body, _) = into_parts(me).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "authentication_required");
}

#[tokio::test]
async fn bounded_touch_refreshes_idle_window_without_write_amplification() {
    let Some(test) = setup(auth_config(50, 10_000)).await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let username = format!("deniz.{}", Uuid::new_v4().simple());
    create_test_user(&test.pool, &username, TEST_PASSWORD).await;

    let response = login(
        &test.app,
        test.peer(),
        &username,
        TEST_PASSWORD,
        Some(ORIGIN),
    )
    .await;
    let (_, json, set_cookie) = into_parts(response).await;
    let cookie = cookie_token(&set_cookie.expect("cookie"));
    let session_id = session_id_of(&json);

    let last_seen: OffsetDateTime =
        sqlx::query_scalar("SELECT last_seen_at FROM user_sessions WHERE id = $1")
            .bind(session_id)
            .fetch_one(&test.pool)
            .await
            .expect("last seen");

    // A request within the touch interval does NOT write.
    test.clock.advance_seconds(10);
    let _ = test
        .app
        .clone()
        .oneshot(authed(
            &axum::http::Method::GET,
            "/api/auth/me",
            &cookie,
            None,
        ))
        .await
        .expect("infallible");
    let unchanged: OffsetDateTime =
        sqlx::query_scalar("SELECT last_seen_at FROM user_sessions WHERE id = $1")
            .bind(session_id)
            .fetch_one(&test.pool)
            .await
            .expect("last seen");
    assert_eq!(unchanged, last_seen, "no touch under 5 minutes");

    // Beyond the touch interval (but within the idle window) it does,
    // extending idle_expires_at to now + idle TTL.
    test.clock.advance_seconds(400);
    let _ = test
        .app
        .clone()
        .oneshot(authed(
            &axum::http::Method::GET,
            "/api/auth/me",
            &cookie,
            None,
        ))
        .await
        .expect("infallible");
    let (touched, idle_expires): (OffsetDateTime, OffsetDateTime) =
        sqlx::query_as("SELECT last_seen_at, idle_expires_at FROM user_sessions WHERE id = $1")
            .bind(session_id)
            .fetch_one(&test.pool)
            .await
            .expect("touched");
    assert!(touched > unchanged);
    assert_eq!(idle_expires, touched + time::Duration::seconds(600));
}

// ------------------------------------------------------------------
// §21/§40: logout and session management
// ------------------------------------------------------------------

#[tokio::test]
async fn logout_revokes_clears_cookie_and_is_idempotent() {
    let Some(test) = setup(auth_config(50, 10_000)).await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let username = format!("cem.{}", Uuid::new_v4().simple());
    create_test_user(&test.pool, &username, TEST_PASSWORD).await;

    let response = login(
        &test.app,
        test.peer(),
        &username,
        TEST_PASSWORD,
        Some(ORIGIN),
    )
    .await;
    let (_, json, set_cookie) = into_parts(response).await;
    let cookie = cookie_token(&set_cookie.expect("cookie"));
    let csrf = csrf_of(&json);

    // CSRF applies to logout.
    let no_csrf = test
        .app
        .clone()
        .oneshot(authed(
            &axum::http::Method::POST,
            "/api/auth/logout",
            &cookie,
            None,
        ))
        .await
        .expect("infallible");
    assert_eq!(no_csrf.status(), StatusCode::FORBIDDEN);

    let logout = test
        .app
        .clone()
        .oneshot(authed(
            &axum::http::Method::POST,
            "/api/auth/logout",
            &cookie,
            Some(&csrf),
        ))
        .await
        .expect("infallible");
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    let clearing = logout
        .headers()
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .expect("clearing cookie");
    assert!(clearing.contains("Max-Age=0"), "{clearing}");

    // Old cookie no longer authenticates.
    assert_eq!(
        get_status(&test.app, "/api/auth/me", &cookie).await,
        StatusCode::UNAUTHORIZED
    );

    // Repeated logout stays 204 (idempotent).
    let again = test
        .app
        .clone()
        .oneshot(authed(
            &axum::http::Method::POST,
            "/api/auth/logout",
            &cookie,
            Some(&csrf),
        ))
        .await
        .expect("infallible");
    assert_eq!(again.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn session_listing_and_revocation_respect_ownership() {
    let Some(test) = setup(auth_config(50, 10_000)).await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let owner = format!("sahip.{}", Uuid::new_v4().simple());
    let other = format!("baska.{}", Uuid::new_v4().simple());
    create_test_user(&test.pool, &owner, TEST_PASSWORD).await;
    create_test_user(&test.pool, &other, TEST_PASSWORD).await;

    // Owner logs in twice, other user once.
    let first = login(&test.app, test.peer(), &owner, TEST_PASSWORD, Some(ORIGIN)).await;
    let (_, first_json, first_set) = into_parts(first).await;
    let first_cookie = cookie_token(&first_set.expect("cookie"));
    let first_id = session_id_of(&first_json);

    let second = login(&test.app, test.peer(), &owner, TEST_PASSWORD, Some(ORIGIN)).await;
    let (_, second_json, second_set) = into_parts(second).await;
    let second_cookie = cookie_token(&second_set.expect("cookie"));
    let second_csrf = csrf_of(&second_json);

    let foreign = login(&test.app, test.peer(), &other, TEST_PASSWORD, Some(ORIGIN)).await;
    let (_, foreign_json, _) = into_parts(foreign).await;
    let foreign_id = session_id_of(&foreign_json);

    // Listing shows only own sessions with exactly one current marker.
    let list = test
        .app
        .clone()
        .oneshot(authed(
            &axum::http::Method::GET,
            "/api/auth/sessions",
            &second_cookie,
            None,
        ))
        .await
        .expect("infallible");
    assert_eq!(list.status(), StatusCode::OK);
    let (_, list_json, _) = into_parts(list).await;
    assert_eq!(list_json["currentSessionId"], second_json["session"]["id"]);
    let sessions = list_json["sessions"].as_array().expect("sessions array");
    assert_eq!(sessions.len(), 2, "own sessions only");
    let listed_ids: Vec<&str> = sessions
        .iter()
        .map(|s| s["id"].as_str().expect("id string"))
        .collect();
    let foreign_id_str = foreign_id.to_string();
    assert!(!listed_ids.contains(&foreign_id_str.as_str()));
    assert_eq!(
        sessions
            .iter()
            .filter(|s| s["current"] == serde_json::json!(true))
            .count(),
        1,
        "exactly one current session"
    );

    // Revoking a foreign session answers 404 and does not revoke it.
    let attempt = test
        .app
        .clone()
        .oneshot(authed(
            &axum::http::Method::DELETE,
            &format!("/api/auth/sessions/{foreign_id}"),
            &second_cookie,
            Some(&second_csrf),
        ))
        .await
        .expect("infallible");
    assert_eq!(attempt.status(), StatusCode::NOT_FOUND);
    let still_active: bool =
        sqlx::query_scalar("SELECT revoked_at IS NULL FROM user_sessions WHERE id = $1")
            .bind(foreign_id)
            .fetch_one(&test.pool)
            .await
            .expect("row");
    assert!(still_active, "foreign session must survive");

    // Revoking another own session works and is idempotent.
    for _ in 0..2 {
        let revoke = test
            .app
            .clone()
            .oneshot(authed(
                &axum::http::Method::DELETE,
                &format!("/api/auth/sessions/{first_id}"),
                &second_cookie,
                Some(&second_csrf),
            ))
            .await
            .expect("infallible");
        assert_eq!(revoke.status(), StatusCode::NO_CONTENT);
    }
    // The revoked own session no longer authenticates.
    assert_eq!(
        get_status(&test.app, "/api/auth/me", &first_cookie).await,
        StatusCode::UNAUTHORIZED
    );

    // Revoke-others keeps the current session alive.
    let _ = login(&test.app, test.peer(), &owner, TEST_PASSWORD, Some(ORIGIN)).await; // third own session
    let revoke_others = test
        .app
        .clone()
        .oneshot(authed(
            &axum::http::Method::POST,
            "/api/auth/sessions/revoke-others",
            &second_cookie,
            Some(&second_csrf),
        ))
        .await
        .expect("infallible");
    assert_eq!(revoke_others.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        get_status(&test.app, "/api/auth/me", &second_cookie).await,
        StatusCode::OK,
        "current session survives revoke-others"
    );
}

// ------------------------------------------------------------------
// §41: CSRF at the HTTP level
// ------------------------------------------------------------------

#[tokio::test]
async fn csrf_protected_mutations_reject_cross_site_requests() {
    let Some(test) = setup(auth_config(50, 10_000)).await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let username = format!("csrf.{}", Uuid::new_v4().simple());
    create_test_user(&test.pool, &username, TEST_PASSWORD).await;

    // Login without Origin → 403 (browsers always send Origin on POST).
    let response = login(&test.app, test.peer(), &username, TEST_PASSWORD, None).await;
    let (status, body, _) = into_parts(response).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "csrf_failed");

    // Login with a foreign Origin → 403.
    let response = login(
        &test.app,
        test.peer(),
        &username,
        TEST_PASSWORD,
        Some(EVIL_ORIGIN),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // Correct origin logs in.
    let response = login(
        &test.app,
        test.peer(),
        &username,
        TEST_PASSWORD,
        Some(ORIGIN),
    )
    .await;
    let (_, json, set_cookie) = into_parts(response).await;
    let cookie = cookie_token(&set_cookie.expect("cookie"));
    let csrf = csrf_of(&json);

    // Safe read (GET /me) works without any CSRF token.
    assert_eq!(
        get_status(&test.app, "/api/auth/me", &cookie).await,
        StatusCode::OK
    );

    // Cross-origin authenticated mutation → 403.
    let mut cross = authed(
        &axum::http::Method::POST,
        "/api/auth/logout",
        &cookie,
        Some(&csrf),
    );
    cross
        .headers_mut()
        .insert("origin", axum::http::HeaderValue::from_static(EVIL_ORIGIN));
    let response = test.app.clone().oneshot(cross).await.expect("infallible");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // Same-origin mutation with a WRONG token → 403.
    let response = test
        .app
        .clone()
        .oneshot(authed(
            &axum::http::Method::POST,
            "/api/auth/logout",
            &cookie,
            Some("yanlis-token"),
        ))
        .await
        .expect("infallible");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // Same-origin mutation with the right token succeeds.
    let response = test
        .app
        .clone()
        .oneshot(authed(
            &axum::http::Method::POST,
            "/api/auth/logout",
            &cookie,
            Some(&csrf),
        ))
        .await
        .expect("infallible");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

// ------------------------------------------------------------------
// §42: rate limiting (injected clock; no sleeps)
// ------------------------------------------------------------------

#[tokio::test]
async fn username_dimension_blocks_after_configured_failures_and_recovers() {
    let Some(test) = setup(auth_config(3, 10_000)).await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let username = format!("limit.{}", Uuid::new_v4().simple());
    create_test_user(&test.pool, &username, TEST_PASSWORD).await;

    for _ in 0..3 {
        let response = login(
            &test.app,
            test.peer(),
            &username,
            "yanlis-parola-123",
            Some(ORIGIN),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    // Blocked — even with the CORRECT password (deliberate: the window
    // must not be bypassable by eventually guessing right).
    let response = login(
        &test.app,
        test.peer(),
        &username,
        TEST_PASSWORD,
        Some(ORIGIN),
    )
    .await;
    let (status, body, _) = into_parts(response).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "rate_limited");

    // Window rolls → automatic recovery, no permanent lockout.
    test.clock.advance_seconds(61);
    let response = login(
        &test.app,
        test.peer(),
        &username,
        TEST_PASSWORD,
        Some(ORIGIN),
    )
    .await;
    let (status, _, _) = into_parts(response).await;
    assert_eq!(status, StatusCode::OK, "window expiry restores access");
}

#[tokio::test]
async fn successful_login_resets_username_bucket_only() {
    let Some(test) = setup(auth_config(3, 10_000)).await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let username = format!("topar.{}", Uuid::new_v4().simple());
    create_test_user(&test.pool, &username, TEST_PASSWORD).await;

    for _ in 0..2 {
        let _ = login(
            &test.app,
            test.peer(),
            &username,
            "yanlis-parola-123",
            Some(ORIGIN),
        )
        .await;
    }
    // Legitimate user recovers (username bucket cleared on success).
    let response = login(
        &test.app,
        test.peer(),
        &username,
        TEST_PASSWORD,
        Some(ORIGIN),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    // Two more failures stay allowed because the bucket reset.
    for _ in 0..2 {
        let response = login(
            &test.app,
            test.peer(),
            &username,
            "yanlis-parola-123",
            Some(ORIGIN),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn ip_dimension_counts_all_attempts_across_usernames() {
    let Some(test) = setup(auth_config(10_000, 3)).await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let base = format!("spray.{}", Uuid::new_v4().simple());
    for i in 0..3 {
        let response = login(
            &test.app,
            test.peer(),
            &format!("{base}-{i}"),
            "yanlis-parola-123",
            Some(ORIGIN),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    // 4th attempt from the same IP — different username — is blocked.
    let response = login(
        &test.app,
        test.peer(),
        &format!("{base}-yeni"),
        "yanlis-parola-123",
        Some(ORIGIN),
    )
    .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
}

// ------------------------------------------------------------------
// §25/§43: audit events and database constraints
// ------------------------------------------------------------------

#[tokio::test]
async fn security_events_record_the_authentication_history() {
    let Some(test) = setup(auth_config(50, 10_000)).await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let username = format!("denetim.{}", Uuid::new_v4().simple());
    create_test_user(&test.pool, &username, TEST_PASSWORD).await;

    let _ = login(
        &test.app,
        test.peer(),
        &username,
        "yanlis-parola-123",
        Some(ORIGIN),
    )
    .await;
    let response = login(
        &test.app,
        test.peer(),
        &username,
        TEST_PASSWORD,
        Some(ORIGIN),
    )
    .await;
    let (_, json, set_cookie) = into_parts(response).await;
    let cookie = cookie_token(&set_cookie.expect("cookie"));
    let csrf = csrf_of(&json);

    let _ = test
        .app
        .clone()
        .oneshot(authed(
            &axum::http::Method::POST,
            "/api/auth/logout",
            &cookie,
            Some(&csrf),
        ))
        .await
        .expect("infallible");

    let event_types: Vec<String> =
        sqlx::query_scalar("SELECT event_type FROM security_events ORDER BY occurred_at")
            .fetch_all(&test.pool)
            .await
            .expect("events readable");
    // (user_created is written by the CLI bootstrap path, not by the
    // direct-repository test factory.)
    for expected in ["login_failed", "login_succeeded", "logout"] {
        assert!(
            event_types.iter().any(|e| e == expected),
            "missing {expected}: {event_types:?}"
        );
    }

    // No secret material ever reached the audit table.
    let metadata: Vec<String> = sqlx::query_scalar("SELECT metadata::text FROM security_events")
        .fetch_all(&test.pool)
        .await
        .expect("metadata readable");
    for blob in &metadata {
        assert!(
            !blob.contains(TEST_PASSWORD),
            "password leaked to audit: {blob}"
        );
        assert!(!blob.contains(&csrf), "csrf token leaked: {blob}");
        assert!(!blob.contains(&cookie), "session token leaked: {blob}");
    }
}

#[tokio::test]
async fn database_constraints_enforce_identity_integrity() {
    let Some(test) = setup(auth_config(50, 10_000)).await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let username = format!("benzersiz.{}", Uuid::new_v4().simple());
    create_test_user(&test.pool, &username, TEST_PASSWORD).await;

    // Duplicate username — even with different raw casing that
    // normalizes identically — is rejected by the database.
    let argon2 = test_argon2();
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).expect("hash");
    let result = users::create_user(&test.pool, &username.to_uppercase(), "Kopya", &hash).await;
    assert!(matches!(
        result,
        Err(users::CreateUserError::DuplicateUsername)
    ));

    // Session FK integrity: a session cannot reference a random user id.
    let foreign_insert = sqlx::query(
        "INSERT INTO user_sessions (user_id, token_hash, csrf_token) \
         VALUES ($1, gen_random_bytes(32), 'csrf')",
    )
    .bind(Uuid::new_v4())
    .execute(&test.pool)
    .await;
    assert!(foreign_insert.is_err(), "FK must reject orphan sessions");
}
