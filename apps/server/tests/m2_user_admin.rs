//! M2 — cooperative user administration (0074 definers + scoped routes).
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same harness as tests/m1p6_b3_membership.rs).
//!
//! Under test:
//! - POST /api/users provisions a global identity + sealed-scope
//!   membership atomically behind memberships.manage/users.manage; the
//!   cooperative is NEVER a request field (cross-scope is structural).
//! - The admin-set credential lands must_change_password = true and is
//!   login-capable through the normal verifier lane.
//! - PATCH /api/users/{id} edits display_name only, 404s non-members
//!   without leaking existence, and mints audit rows.
//! - Permission separation: users.read alone cannot mutate; missing
//!   grants yield 403; duplicate normalized usernames yield 409.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

use kooperatif_server::auth::{identity, users, AuthRuntime};
use kooperatif_server::clock::MutableClock;
use kooperatif_server::config::{AuthConfig, ARGON2_M_COST_FLOOR};
use kooperatif_server::db as app_db;
use kooperatif_server::http::{cors_layer, router, AppState};
use kooperatif_server::tenant::repo;

const TEST_PASSWORD: &str = "m2-test-parola-123";
const ORIGIN: &str = "http://localhost:5173";
const APP_ROLE: &str = "kooperatif_app";

fn app_password() -> String {
    app_db::test_app_role_password()
}

fn test_database_url() -> String {
    std::env::var("KOOPERATIF_TEST_DATABASE_URL").expect("KOOPERATIF_TEST_DATABASE_URL is not set")
}

struct TestApp {
    _claim: app_db::TestDbClaim,
    app: axum::Router,
    pool: PgPool,
    app_pool: PgPool,
    peer: SocketAddr,
    database_name: String,
    admin_url: String,
}

fn app_url(base_db_url: &str) -> String {
    let (scheme, rest) = base_db_url.split_once("://").expect("url scheme");
    let at = rest.find('@').expect("userinfo in url");
    format!("{scheme}://{APP_ROLE}:{}{}", app_password(), &rest[at..])
}

impl TestApp {
    async fn cleanup(&self) {
        let _ = sqlx::query(&format!(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}' AND pid <> pg_backend_pid()",
            self.database_name
        ))
        .execute(&self.pool)
        .await;
        self.pool.close().await;
        self.app_pool.close().await;
        if let Ok(admin) = PgPoolOptions::new()
            .max_connections(1)
            .connect(&self.admin_url)
            .await
        {
            app_db::assert_disposable_test_database_name(&self.database_name);
            let _ = sqlx::query(&format!(
                "DROP DATABASE IF EXISTS {} WITH (FORCE)",
                self.database_name
            ))
            .execute(&admin)
            .await;
            admin.close().await;
        }
    }
}

static SETUP_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

async fn setup() -> TestApp {
    let _setup_slot = SETUP_SLOTS.acquire().await.expect("setup slot");
    let url = test_database_url();
    let suffix = Uuid::new_v4().simple();
    let database_name = format!("kooperatif_test_{suffix}");
    let (base_url, admin_url) = url
        .rsplit_once('/')
        .map(|(base, _)| (base.to_string(), format!("{base}/postgres")))
        .expect("url with database segment");
    let admin = app_db::connect(&admin_url)
        .await
        .expect("admin connectivity");
    let _ = app_db::drop_stale_test_databases(&admin).await;
    let claim = app_db::create_test_database(&admin, &admin_url, &database_name).await;
    admin.close().await;
    let pool = app_db::connect(&format!("{base_url}/{database_name}"))
        .await
        .expect("test database connectivity");
    app_db::run_test_migrations(&pool)
        .await
        .expect("migrations applied");
    repo::bootstrap_initial_cooperative(&pool, "Ana Kooperatif")
        .await
        .expect("bootstrap cooperative");
    let clock = Arc::new(MutableClock::new(
        OffsetDateTime::parse(
            "2026-01-01T00:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .expect("fixed epoch"),
    ));
    let config = AuthConfig {
        allowed_origins: vec![ORIGIN.to_string()],
        cookie_secure: false,
        session_absolute_ttl_secs: 3600,
        session_idle_ttl_secs: 600,
        argon2_m_cost: ARGON2_M_COST_FLOOR,
        argon2_t_cost: 1,
        argon2_p_cost: 1,
        login_window_secs: 60,
        login_username_max_attempts: 100,
        login_ip_max_attempts: 100_000,
        forwarded_ip: false,
    };
    let runtime = AuthRuntime::new(config, clock);
    let app_pool = app_db::connect(&app_url(&format!("{base_url}/{database_name}")))
        .await
        .expect("kooperatif_app pool");
    let app = router(
        AppState {
            db: Some(app_pool.clone()),
            auth: Arc::new(runtime),
        },
        cors_layer(&[]),
    );
    let bytes = Uuid::new_v4();
    let octets = bytes.as_bytes();
    TestApp {
        _claim: claim,
        app,
        pool,
        app_pool,
        database_name,
        admin_url,
        peer: format!(
            "127.{}.{}.{}:{}",
            octets[0],
            octets[1],
            octets[2],
            50000 + (octets[3] as u16)
        )
        .parse()
        .expect("derived addr"),
    }
}

async fn create_user(pool: &PgPool, username: &str) -> Uuid {
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).expect("params"),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).expect("hash");
    users::create_user(pool, username, "M2 Test", &hash, TEST_PASSWORD)
        .await
        .expect("user")
        .id
}

async fn bootstrap_coop(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("SELECT id FROM cooperatives WHERE is_bootstrap")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn grant_in_cooperative(pool: &PgPool, coop: Uuid, user: Uuid, perms: &[&str]) {
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id) \
         VALUES ($1, $2) ON CONFLICT (cooperative_id, user_id) \
         DO UPDATE SET status = 'active', ended_at = NULL",
    )
    .bind(coop)
    .bind(user)
    .execute(pool)
    .await
    .unwrap();
    let role: Uuid =
        sqlx::query_scalar("INSERT INTO roles (name, cooperative_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("coop-m2-{user}-{coop}"))
            .bind(coop)
            .fetch_one(pool)
            .await
            .unwrap();
    for key in perms {
        sqlx::query(
            "INSERT INTO role_permissions (role_id, permission_id) \
             SELECT $1, id FROM permissions WHERE key = $2",
        )
        .bind(role)
        .bind(key)
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
         VALUES ($1, $2, $3)",
    )
    .bind(user)
    .bind(role)
    .bind(coop)
    .execute(pool)
    .await
    .unwrap();
}

async fn login(app: &axum::Router, peer: SocketAddr, username: &str) -> (String, String, Uuid) {
    let mut request = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .header("user-agent", "Mozilla/5.0 (Windows NT 10.0) Chrome/126.0")
        .body(Body::from(
            json!({ "username": username, "password": TEST_PASSWORD }).to_string(),
        ))
        .expect("valid request");
    request.extensions_mut().insert(ConnectInfo(peer));
    let response = app.clone().oneshot(request).await.expect("infallible");
    assert_eq!(response.status(), StatusCode::OK);
    let set_cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .expect("cookie")
        .to_string();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body: Value = serde_json::from_slice(&bytes).expect("json");
    let cookie = set_cookie
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .map(|(_, v)| v.to_string())
        .expect("token");
    (
        cookie,
        body["csrfToken"].as_str().expect("csrf").to_string(),
        body["user"]["id"]
            .as_str()
            .expect("id")
            .parse()
            .expect("uuid"),
    )
}

/// Password POST with the required ConnectInfo extension — the lane
/// derives the rate-limit IP from it.
fn pw_request(
    cookie: &str,
    csrf: &str,
    current: &str,
    next: &str,
    peer: SocketAddr,
) -> Request<Body> {
    let mut request = request_builder(
        "POST",
        "/api/auth/password",
        cookie,
        Some(csrf),
        Some(json!({ "currentPassword": current, "newPassword": next })),
    );
    request.extensions_mut().insert(ConnectInfo(peer));
    request
}

fn request_builder(
    method: &str,
    path: &str,
    cookie: &str,
    csrf: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("cookie", format!("kooperatif_session={cookie}"))
        .header("origin", ORIGIN);
    if let Some(csrf) = csrf {
        builder = builder.header("x-csrf-token", csrf);
    }
    if let Some(body) = body {
        builder = builder.header("content-type", "application/json");
        builder.body(Body::from(body.to_string())).expect("valid")
    } else {
        builder.body(Body::empty()).expect("valid")
    }
}

/// Raw-text variant for non-JSON payloads (CSV export).
async fn send_text(app: &axum::Router, request: Request<Body>) -> (StatusCode, String) {
    let response = app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).to_string())
}

/// Adopt the cooperative scope through the audited session switch.
async fn switch_cooperative(app: &axum::Router, cookie: &str, coop: Uuid) {
    let (_, me) = send(
        app,
        request_builder("GET", "/api/auth/me", cookie, None, None),
    )
    .await;
    let csrf: String = me["csrfToken"].as_str().unwrap_or_default().into();
    let (status, _) = send(
        app,
        request_builder(
            "POST",
            "/api/auth/cooperative",
            cookie,
            Some(&csrf),
            Some(json!({ "cooperativeId": coop })),
        ),
    )
    .await;
    assert!(status.is_success(), "cooperative switch got {status}");
}

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

async fn audit_count(pool: &PgPool, event_type: &str, user_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM security_events \
         WHERE event_type = $1 AND metadata->>'user_id' = $2::text",
    )
    .bind(event_type)
    .bind(user_id.to_string())
    .fetch_one(pool)
    .await
    .unwrap()
}

// ── POST /api/users ──────────────────────────────────────────────────

#[tokio::test]
async fn create_user_provisions_identity_and_membership() {
    let t = setup().await;
    let admin = create_user(&t.pool, "m2admin1").await;
    let coop = bootstrap_coop(&t.pool).await;
    grant_in_cooperative(&t.pool, coop, admin, &["memberships.manage", "users.read"]).await;
    let (cookie, csrf, _) = login(&t.app, t.peer, "m2admin1").await;

    let (status, body) = send(
        &t.app,
        request_builder(
            "POST",
            "/api/users",
            &cookie,
            Some(&csrf),
            Some(json!({
                "username": "Case.Op",
                "displayName": "Operatör Bir",
                "password": "gecici-parola-1"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {body}");
    let new_id: Uuid = body["userId"].as_str().expect("userId").parse().unwrap();

    // Identity: normalized username, active, must_change_password set
    // (PD-14 recommended: operator credentials are one-time).
    let (username, status_, must_change): (String, String, bool) =
        sqlx::query_as("SELECT username, status, must_change_password FROM users WHERE id = $1")
            .bind(new_id)
            .fetch_one(&t.pool)
            .await
            .unwrap();
    assert_eq!(username, "case.op");
    assert_eq!(status_, "active");
    assert!(must_change, "operator credential must force rotation");

    // Membership in the SEALED scope only.
    let ms: Option<String> = sqlx::query_scalar(
        "SELECT status FROM cooperative_memberships \
         WHERE cooperative_id = $1 AND user_id = $2",
    )
    .bind(coop)
    .bind(new_id)
    .fetch_optional(&t.pool)
    .await
    .unwrap();
    assert_eq!(ms.as_deref(), Some("active"));

    // Audit: identity mint + membership grant.
    assert_eq!(audit_count(&t.pool, "user_created", new_id).await, 1);
    let grants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events \
         WHERE event_type = 'cooperative_membership_granted' \
           AND metadata->>'target_user_id' = $1::text",
    )
    .bind(new_id.to_string())
    .fetch_one(&t.pool)
    .await
    .unwrap();
    assert_eq!(grants, 1);

    // The provisioned credential authenticates through the normal lane.
    let mut request = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .header("user-agent", "Mozilla/5.0")
        .body(Body::from(
            json!({ "username": "case.op", "password": "gecici-parola-1" }).to_string(),
        ))
        .expect("valid");
    request.extensions_mut().insert(ConnectInfo(t.peer));
    let response = t.app.clone().oneshot(request).await.expect("infallible");
    assert_eq!(response.status(), StatusCode::OK);

    t.cleanup().await;
}

#[tokio::test]
async fn create_user_denied_without_manage() {
    let t = setup().await;
    let member = create_user(&t.pool, "m2reader1").await;
    let coop = bootstrap_coop(&t.pool).await;
    grant_in_cooperative(&t.pool, coop, member, &["users.read"]).await;
    let (cookie, csrf, _) = login(&t.app, t.peer, "m2reader1").await;

    let (status, _) = send(
        &t.app,
        request_builder(
            "POST",
            "/api/users",
            &cookie,
            Some(&csrf),
            Some(json!({
                "username": "noperm.user",
                "displayName": "No Perm",
                "password": "gecici-parola-1"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    t.cleanup().await;
}

#[tokio::test]
async fn create_user_rejects_duplicate_and_foreign_scope() {
    let t = setup().await;
    let admin = create_user(&t.pool, "m2admin2").await;
    let coop = bootstrap_coop(&t.pool).await;
    grant_in_cooperative(&t.pool, coop, admin, &["memberships.manage"]).await;
    let (cookie, csrf, _) = login(&t.app, t.peer, "m2admin2").await;

    // Duplicate normalized username (case-folded) -> 409.
    let existing = create_user(&t.pool, "Taken.Name").await;
    let _ = existing;
    let (status, _) = send(
        &t.app,
        request_builder(
            "POST",
            "/api/users",
            &cookie,
            Some(&csrf),
            Some(json!({
                "username": "taken.name",
                "displayName": "Taken Two",
                "password": "gecici-parola-1"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // A caller-supplied cooperative id is not a scope override — strict
    // body validation rejects unknown fields outright.
    let (status, _) = send(
        &t.app,
        request_builder(
            "POST",
            "/api/users",
            &cookie,
            Some(&csrf),
            Some(json!({
                "username": "scope.sneak",
                "displayName": "Sneak",
                "password": "gecici-parola-1",
                "cooperativeId": Uuid::new_v4()
            })),
        ),
    )
    .await;
    assert!(
        status.is_client_error(),
        "foreign scope field must be rejected: {status}"
    );
    t.cleanup().await;
}

// ── PATCH /api/users/{id} ────────────────────────────────────────────

#[tokio::test]
async fn update_user_edits_display_name_in_scope() {
    let t = setup().await;
    let admin = create_user(&t.pool, "m2admin3").await;
    let target = create_user(&t.pool, "m2target1").await;
    let coop = bootstrap_coop(&t.pool).await;
    grant_in_cooperative(&t.pool, coop, admin, &["memberships.manage"]).await;
    grant_in_cooperative(&t.pool, coop, target, &["users.read"]).await;
    let (cookie, csrf, _) = login(&t.app, t.peer, "m2admin3").await;

    let (status, _) = send(
        &t.app,
        request_builder(
            "PATCH",
            &format!("/api/users/{target}"),
            &cookie,
            Some(&csrf),
            Some(json!({ "displayName": "Yeni Ad Soyad" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let name: String = sqlx::query_scalar("SELECT display_name FROM users WHERE id = $1")
        .bind(target)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(name, "Yeni Ad Soyad");
    assert_eq!(audit_count(&t.pool, "user_updated", target).await, 1);
    t.cleanup().await;
}

#[tokio::test]
async fn update_user_non_member_is_not_found_no_leak() {
    let t = setup().await;
    let admin = create_user(&t.pool, "m2admin4").await;
    let outsider = create_user(&t.pool, "m2outsider").await; // no membership
    let coop = bootstrap_coop(&t.pool).await;
    grant_in_cooperative(&t.pool, coop, admin, &["memberships.manage"]).await;
    let (cookie, csrf, _) = login(&t.app, t.peer, "m2admin4").await;

    let (status, _) = send(
        &t.app,
        request_builder(
            "PATCH",
            &format!("/api/users/{outsider}"),
            &cookie,
            Some(&csrf),
            Some(json!({ "displayName": "Ghost" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(
        &t.app,
        request_builder(
            "PATCH",
            &format!("/api/users/{outsider}"),
            &cookie,
            Some(&csrf),
            Some(json!({ "displayName": "  " })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    t.cleanup().await;
}

#[tokio::test]
async fn multi_cooperative_membership_isolation() {
    let t = setup().await;
    // Same global identity, two cooperatives, independent grants:
    // admin powers in coop A must not appear in coop B.
    let shared = create_user(&t.pool, "m2shared").await;
    let coop_a = bootstrap_coop(&t.pool).await;
    let coop_b: Uuid = sqlx::query_scalar(
        "INSERT INTO cooperatives (name, status, business_enabled) \
         VALUES ('Ikinci', 'active', true) RETURNING id",
    )
    .fetch_one(&t.pool)
    .await
    .unwrap();
    grant_in_cooperative(&t.pool, coop_a, shared, &["memberships.manage"]).await;
    grant_in_cooperative(&t.pool, coop_b, shared, &["users.read"]).await;

    let (cookie, csrf, _) = login(&t.app, t.peer, "m2shared").await;
    // In coop A (default bootstrap scope) creation works.
    let (status, _) = send(
        &t.app,
        request_builder(
            "POST",
            "/api/users",
            &cookie,
            Some(&csrf),
            Some(json!({
                "username": "in.a.scope",
                "displayName": "A Scope",
                "password": "gecici-parola-1"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Switch to coop B where the identity holds only users.read.
    let (status, body) = send(
        &t.app,
        request_builder(
            "POST",
            "/api/auth/cooperative",
            &cookie,
            Some(&csrf),
            Some(json!({ "cooperativeId": coop_b })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "switch: {body}");

    let (status, _) = send(
        &t.app,
        request_builder(
            "POST",
            "/api/users",
            &cookie,
            Some(&csrf),
            Some(json!({
                "username": "in.b.scope",
                "displayName": "B Scope",
                "password": "gecici-parola-1"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "scope B lacks manage");
    t.cleanup().await;
}

// ── POST /api/auth/password (M2 self rotation, 0075) ────────────────

async fn login_with(
    app: &axum::Router,
    peer: SocketAddr,
    username: &str,
    password: &str,
) -> (StatusCode, String, String) {
    let mut request = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .header("user-agent", "Mozilla/5.0")
        .body(Body::from(
            json!({ "username": username, "password": password }).to_string(),
        ))
        .expect("valid");
    request.extensions_mut().insert(ConnectInfo(peer));
    let response = app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let set_cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let cookie = set_cookie
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .map(|(_, v)| v.to_string())
        .unwrap_or_default();
    (
        status,
        cookie,
        body["csrfToken"].as_str().unwrap_or_default().to_string(),
    )
}

#[tokio::test]
async fn password_change_rotates_credential_and_revokes_sessions() {
    let t = setup().await;
    let user = create_user(&t.pool, "m2pw1").await;
    let coop = bootstrap_coop(&t.pool).await;
    grant_in_cooperative(&t.pool, coop, user, &["users.read"]).await;
    let (status, cookie, csrf) = login_with(&t.app, t.peer, "m2pw1", TEST_PASSWORD).await;
    assert_eq!(status, StatusCode::OK);

    // Correct current -> 204 + cleared cookie.
    let (status, _) = send(
        &t.app,
        pw_request(
            &cookie,
            &csrf,
            TEST_PASSWORD,
            "yeni-guvenli-parola-789",
            t.peer,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Old session is dead; old credential fails; new credential works.
    let (status, _) = send(
        &t.app,
        request_builder("GET", "/api/auth/me", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = login_with(&t.app, t.peer, "m2pw1", TEST_PASSWORD).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = login_with(&t.app, t.peer, "m2pw1", "yeni-guvenli-parola-789").await;
    assert_eq!(status, StatusCode::OK);

    // Columns + audit.
    let (must, changed): (bool, bool) = sqlx::query_as(
        "SELECT must_change_password, password_changed_at IS NOT NULL FROM users WHERE id = $1",
    )
    .bind(user)
    .fetch_one(&t.pool)
    .await
    .unwrap();
    assert!(!must && changed);
    assert_eq!(audit_count(&t.pool, "password_changed", user).await, 1);
    t.cleanup().await;
}

#[tokio::test]
async fn password_change_wrong_current_is_refused_and_audited() {
    let t = setup().await;
    let user = create_user(&t.pool, "m2pw2").await;
    let coop = bootstrap_coop(&t.pool).await;
    grant_in_cooperative(&t.pool, coop, user, &["users.read"]).await;
    let (status, cookie, csrf) = login_with(&t.app, t.peer, "m2pw2", TEST_PASSWORD).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = send(
        &t.app,
        pw_request(
            &cookie,
            &csrf,
            "yanlis-parola",
            "yeni-guvenli-parola-789",
            t.peer,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        audit_count(&t.pool, "password_change_failed", user).await,
        1
    );

    // Session survives — the credential was untouched.
    let (status, _) = send(
        &t.app,
        request_builder("GET", "/api/auth/me", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    t.cleanup().await;
}

#[tokio::test]
async fn must_change_password_gates_everything_but_rotation() {
    let t = setup().await;
    let user = create_user(&t.pool, "m2pw3").await;
    let coop = bootstrap_coop(&t.pool).await;
    grant_in_cooperative(&t.pool, coop, user, &["memberships.manage", "users.read"]).await;
    sqlx::query("UPDATE users SET must_change_password = true WHERE id = $1")
        .bind(user)
        .execute(&t.pool)
        .await
        .unwrap();
    let (status, cookie, csrf) = login_with(&t.app, t.peer, "m2pw3", TEST_PASSWORD).await;
    assert_eq!(status, StatusCode::OK);

    // Business APIs are refused at session resolution; the rotation
    // lane, logout and me stay reachable.
    let (status, _) = send(
        &t.app,
        request_builder("GET", "/api/users", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = send(
        &t.app,
        request_builder("GET", "/api/auth/me", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mustChangePassword"].as_bool(), Some(true));

    let (status, _) = send(
        &t.app,
        pw_request(
            &cookie,
            &csrf,
            TEST_PASSWORD,
            "yeni-guvenli-parola-789",
            t.peer,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Fresh login carries no gate.
    let (status, cookie2, _) = login_with(&t.app, t.peer, "m2pw3", "yeni-guvenli-parola-789").await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send(
        &t.app,
        request_builder("GET", "/api/auth/me", &cookie2, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mustChangePassword"].as_bool(), Some(false));
    let (status, _) = send(
        &t.app,
        request_builder("GET", "/api/users", &cookie2, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    t.cleanup().await;
}

// ── PD-14 platform reset single-use + PD-15 CSV export ───────────────

async fn grant_platform(pool: &PgPool, user: Uuid, perms: &[&str]) {
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, cooperative_id) VALUES ($1, NULL) RETURNING id",
    )
    .bind(format!("plat-{user}"))
    .fetch_one(pool)
    .await
    .unwrap();
    for key in perms {
        sqlx::query(
            "INSERT INTO role_permissions (role_id, permission_id) \
             SELECT $1, id FROM permissions WHERE key = $2",
        )
        .bind(role)
        .bind(key)
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
         VALUES ($1, $2, NULL)",
    )
    .bind(user)
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn platform_reset_single_use_and_forced_rotation() {
    let t = setup().await;
    let admin = create_user(&t.pool, "m2adm").await;
    grant_platform(&t.pool, admin, &["users.manage"]).await;
    let target = create_user(&t.pool, "m2tgt").await;
    let coop = bootstrap_coop(&t.pool).await;
    grant_in_cooperative(&t.pool, coop, target, &["users.read"]).await;
    let (_, acookie, acsrf) = login_with(&t.app, t.peer, "m2adm", TEST_PASSWORD).await;
    // Target's existing session gets revoked by the reset.
    let (_, oldcookie, _) = login_with(&t.app, t.peer, "m2tgt", TEST_PASSWORD).await;

    let (status, body) = send(
        &t.app,
        request_builder(
            "POST",
            &format!("/api/platform/users/{target}/reset-password"),
            &acookie,
            Some(&acsrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let temp = body["temporaryPassword"].as_str().unwrap().to_string();
    assert!(temp.len() >= 12);

    // Prior session dead; old credential dead.
    let (s, _) = send(
        &t.app,
        request_builder("GET", "/api/auth/me", &oldcookie, None, None),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _, _) = login_with(&t.app, t.peer, "m2tgt", TEST_PASSWORD).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // Temp credential: FIRST login succeeds, SECOND fails (single-use).
    let (s, cookie1, csrf1) = login_with(&t.app, t.peer, "m2tgt", &temp).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = login_with(&t.app, t.peer, "m2tgt", &temp).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // Forced-change gate active on the consumed session.
    let (s, _) = send(
        &t.app,
        request_builder("GET", "/api/users", &cookie1, None, None),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = send(
        &t.app,
        pw_request(&cookie1, &csrf1, &temp, "yeni-parola-son-123", t.peer),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _, _) = login_with(&t.app, t.peer, "m2tgt", "yeni-parola-son-123").await;
    assert_eq!(s, StatusCode::OK);

    // Audit minted; the plaintext temp never entered metadata.
    let resets: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events          WHERE event_type = 'password_reset_by_admin'            AND metadata->>'target_user_id' = $1::text",
    )
    .bind(target)
    .fetch_one(&t.pool)
    .await
    .unwrap();
    assert_eq!(resets, 1);
    let leak: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM security_events \
         WHERE event_type = 'password_reset_by_admin' AND metadata::text LIKE '%r-%')",
    )
    .fetch_one(&t.pool)
    .await
    .unwrap();
    assert!(!leak);
    t.cleanup().await;
}

#[tokio::test]
async fn platform_reset_concurrent_logins_single_winner() {
    let t = setup().await;
    let admin = create_user(&t.pool, "m2rca").await;
    grant_platform(&t.pool, admin, &["users.manage"]).await;
    let target = create_user(&t.pool, "m2rct").await;
    let (_, acookie, acsrf) = login_with(&t.app, t.peer, "m2rca", TEST_PASSWORD).await;
    let (status, body) = send(
        &t.app,
        request_builder(
            "POST",
            &format!("/api/platform/users/{target}/reset-password"),
            &acookie,
            Some(&acsrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let temp = body["temporaryPassword"].as_str().unwrap().to_string();

    // Two racing logins: exactly one consumes the credential.
    let (a, b) = tokio::join!(
        login_with(&t.app, t.peer, "m2rct", &temp),
        login_with(&t.app, t.peer, "m2rct", &temp),
    );
    let wins = [a.0, b.0].iter().filter(|s| **s == StatusCode::OK).count();
    assert_eq!(wins, 1);
    t.cleanup().await;
}

#[tokio::test]
async fn platform_reset_denied_for_cooperative_admin() {
    let t = setup().await;
    let admin = create_user(&t.pool, "m2cad").await;
    let coop = bootstrap_coop(&t.pool).await;
    grant_in_cooperative(
        &t.pool,
        coop,
        admin,
        &["users.manage", "memberships.manage"],
    )
    .await;
    let target = create_user(&t.pool, "m2ctg").await;
    let (_, cookie, csrf) = login_with(&t.app, t.peer, "m2cad", TEST_PASSWORD).await;
    let (status, _) = send(
        &t.app,
        request_builder(
            "POST",
            &format!("/api/platform/users/{target}/reset-password"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    t.cleanup().await;
}

#[tokio::test]
async fn audit_export_requires_export_permission_and_masks() {
    let t = setup().await;
    let coop = bootstrap_coop(&t.pool).await;
    sqlx::query("UPDATE cooperatives SET business_enabled = true WHERE id = $1")
        .bind(coop)
        .execute(&t.pool)
        .await
        .unwrap();
    // audit.read alone must NOT reach the export lane.
    let reader = create_user(&t.pool, "m2ard").await;
    grant_in_cooperative(&t.pool, coop, reader, &["audit.read"]).await;
    let (_, cookie, _) = login_with(&t.app, t.peer, "m2ard", TEST_PASSWORD).await;
    switch_cooperative(&t.app, &cookie, coop).await;
    let (status, _) = send(
        &t.app,
        request_builder("GET", "/api/audit/events/export", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // audit.export holder gets CSV; hostile cells are ' prefixed.
    let exporter = create_user(&t.pool, "m2aex").await;
    grant_in_cooperative(&t.pool, coop, exporter, &["audit.export"]).await;
    let (_, cookie2, _) = login_with(&t.app, t.peer, "m2aex", TEST_PASSWORD).await;
    // Rename AFTER login: the actor cell must carry the hostile value.
    sqlx::query("UPDATE users SET username = '=cmd' WHERE id = $1")
        .bind(exporter)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO security_events (id, event_type, cooperative_id, user_id, occurred_at, metadata) \
         VALUES ($1, 'password_changed', $2, $3, now(), $4)",
    )
    .bind(Uuid::new_v4())
    .bind(coop)
    .bind(exporter)
    .bind(json!({"user_id": exporter.to_string(), "secret": "nope"}))
    .execute(&t.pool)
    .await
    .unwrap();
    switch_cooperative(&t.app, &cookie2, coop).await;
    let (status, csv) = send_text(
        &t.app,
        request_builder("GET", "/api/audit/events/export", &cookie2, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Metadata allowlist dropped 'secret'; the '=' actor got ' prefixed.
    assert!(!csv.contains("nope"));
    assert!(csv.contains("'=cmd"));
    let exports: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events          WHERE event_type = 'audit_exported' AND user_id = $1",
    )
    .bind(exporter)
    .fetch_one(&t.pool)
    .await
    .unwrap();
    assert_eq!(exports, 1);
    t.cleanup().await;
}

/// 0081: only a platform administrator may add/remove the canonical
/// System Administrator role; cooperative users.manage is insufficient
/// for either direction, and denied attempts mutate nothing.
#[tokio::test]
async fn sysadmin_assignment_requires_platform_admin() {
    let t = setup().await;
    let coop = bootstrap_coop(&t.pool).await;
    // Canonical coop-scoped clone: name matches the system-administrator
    // template (the linkage activation itself uses).
    let sysadmin: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, description, cooperative_id) \
         VALUES ('Sistem Yöneticisi', 'clone', $1) RETURNING id",
    )
    .bind(coop)
    .fetch_one(&t.pool)
    .await
    .unwrap();
    let target = create_user(&t.pool, "m2-sa-target").await;
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         VALUES ($1, $2, 'active')",
    )
    .bind(coop)
    .bind(target)
    .execute(&t.pool)
    .await
    .unwrap();

    async fn role_assigned(pool: &PgPool, target: Uuid, sysadmin: Uuid, coop: Uuid) -> bool {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM user_role_assignments \
              WHERE user_id = $1 AND role_id = $2 AND cooperative_id = $3)",
        )
        .bind(target)
        .bind(sysadmin)
        .bind(coop)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    // Cooperative users.manage+roles.manage operator (keeps the coop's
    // administration path non-empty so the lockout check stays quiet):
    // still NOT a platform administrator — sysadmin writes must fail.
    let op = create_user(&t.pool, "m2-sa-op").await;
    grant_in_cooperative(&t.pool, coop, op, &["users.manage", "roles.manage"]).await;
    let (_, cookie, csrf) = login_with(&t.app, t.peer, "m2-sa-op", TEST_PASSWORD).await;
    switch_cooperative(&t.app, &cookie, coop).await;
    let (denied_assign, _) = send(
        &t.app,
        request_builder(
            "PUT",
            &format!("/api/users/{target}/roles"),
            &cookie,
            Some(&csrf),
            Some(json!({ "roleIds": [sysadmin] })),
        ),
    )
    .await;
    assert_eq!(denied_assign, StatusCode::FORBIDDEN);
    assert!(
        !role_assigned(&t.pool, target, sysadmin, coop).await,
        "denied assign mutated state"
    );

    // Revocation denied the same way: fixture-assign, then empty the set.
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
         VALUES ($1, $2, $3)",
    )
    .bind(target)
    .bind(sysadmin)
    .bind(coop)
    .execute(&t.pool)
    .await
    .unwrap();
    let (denied_revoke, _) = send(
        &t.app,
        request_builder(
            "PUT",
            &format!("/api/users/{target}/roles"),
            &cookie,
            Some(&csrf),
            Some(json!({ "roleIds": [] })),
        ),
    )
    .await;
    assert_eq!(denied_revoke, StatusCode::FORBIDDEN);
    assert!(
        role_assigned(&t.pool, target, sysadmin, coop).await,
        "denied revoke mutated state"
    );

    // Fixture-assign proved revoke is denied; clear it so the ordinary
    // path runs on a clean assignment set (the guard only cares about
    // the sysadmin role's diff, not unrelated roles).
    sqlx::query("DELETE FROM user_role_assignments WHERE user_id = $1 AND cooperative_id = $2")
        .bind(target)
        .bind(coop)
        .execute(&t.pool)
        .await
        .unwrap();

    // Ordinary role still assignable by the same operator.
    let ordinary: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, cooperative_id) VALUES ('m2-uye', $1) RETURNING id",
    )
    .bind(coop)
    .fetch_one(&t.pool)
    .await
    .unwrap();
    let (ordinary_ok, _) = send(
        &t.app,
        request_builder(
            "PUT",
            &format!("/api/users/{target}/roles"),
            &cookie,
            Some(&csrf),
            Some(json!({ "roleIds": [sysadmin, ordinary] })),
        ),
    )
    .await;
    assert_eq!(
        ordinary_ok,
        StatusCode::FORBIDDEN,
        "sysadmin in set must deny"
    );
    // The denied set-write was atomic: ordinary role was NOT persisted.
    assert!(!sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM user_role_assignments           WHERE user_id = $1 AND role_id = $2)",
    )
    .bind(target)
    .bind(ordinary)
    .fetch_one(&t.pool)
    .await
    .unwrap());
    let (ordinary_ok, _) = send(
        &t.app,
        request_builder(
            "PUT",
            &format!("/api/users/{target}/roles"),
            &cookie,
            Some(&csrf),
            Some(json!({ "roleIds": [ordinary] })),
        ),
    )
    .await;
    assert_eq!(ordinary_ok, StatusCode::NO_CONTENT);

    // Platform administrator (NULL users.manage + membership): allowed.
    let padmin = create_user(&t.pool, "m2-sa-padmin").await;
    grant_platform(&t.pool, padmin, &["users.manage"]).await;
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         VALUES ($1, $2, 'active')",
    )
    .bind(coop)
    .bind(padmin)
    .execute(&t.pool)
    .await
    .unwrap();
    let (_, pcookie, pcsrf) = login_with(&t.app, t.peer, "m2-sa-padmin", TEST_PASSWORD).await;
    switch_cooperative(&t.app, &pcookie, coop).await;
    let (allow_assign, _) = send(
        &t.app,
        request_builder(
            "PUT",
            &format!("/api/users/{target}/roles"),
            &pcookie,
            Some(&pcsrf),
            Some(json!({ "roleIds": [sysadmin] })),
        ),
    )
    .await;
    assert_eq!(allow_assign, StatusCode::NO_CONTENT);
    assert!(role_assigned(&t.pool, target, sysadmin, coop).await);
    let (allow_revoke, _) = send(
        &t.app,
        request_builder(
            "PUT",
            &format!("/api/users/{target}/roles"),
            &pcookie,
            Some(&pcsrf),
            Some(json!({ "roleIds": [] })),
        ),
    )
    .await;
    assert_eq!(allow_revoke, StatusCode::NO_CONTENT);
    assert!(!role_assigned(&t.pool, target, sysadmin, coop).await);
}

/// 0081 §B: a cooperative scope admin may only delegate permissions
/// they hold in that scope; platform-only keys and unowned keys are
/// denied atomically; platform administrators keep the full catalog.
#[tokio::test]
async fn role_permission_delegation_boundary() {
    let t = setup().await;
    let coop = bootstrap_coop(&t.pool).await;
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, cooperative_id) VALUES ('m2-deleg', $1) RETURNING id",
    )
    .bind(coop)
    .fetch_one(&t.pool)
    .await
    .unwrap();
    async fn keys_of(pool: &PgPool, role: Uuid) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM role_permissions WHERE role_id = $1")
            .bind(role)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    let op = create_user(&t.pool, "m2-dg-op").await;
    grant_in_cooperative(&t.pool, coop, op, &["users.manage", "roles.manage"]).await;
    let (_, cookie, csrf) = login_with(&t.app, t.peer, "m2-dg-op", TEST_PASSWORD).await;
    switch_cooperative(&t.app, &cookie, coop).await;

    // Unowned permission key → denied, nothing written.
    let (denied, _) = send(
        &t.app,
        request_builder(
            "PUT",
            &format!("/api/roles/{role}/permissions"),
            &cookie,
            Some(&csrf),
            Some(json!({ "permissions": ["users.manage", "payments.manage"] })),
        ),
    )
    .await;
    assert_eq!(denied, StatusCode::FORBIDDEN);
    assert_eq!(
        keys_of(&t.pool, role).await,
        0,
        "denied set mutated permissions"
    );

    // Platform-only key → denied by the 0069 lane guard.
    let (platform_denied, _) = send(
        &t.app,
        request_builder(
            "PUT",
            &format!("/api/roles/{role}/permissions"),
            &cookie,
            Some(&csrf),
            Some(json!({ "permissions": ["cooperatives.manage"] })),
        ),
    )
    .await;
    assert_eq!(platform_denied, StatusCode::FORBIDDEN);
    assert_eq!(keys_of(&t.pool, role).await, 0);

    // Owned keys → allowed.
    let (allowed, _) = send(
        &t.app,
        request_builder(
            "PUT",
            &format!("/api/roles/{role}/permissions"),
            &cookie,
            Some(&csrf),
            Some(json!({ "permissions": ["users.manage", "roles.manage"] })),
        ),
    )
    .await;
    assert_eq!(allowed, StatusCode::NO_CONTENT);
    assert_eq!(keys_of(&t.pool, role).await, 2);

    // Platform administrator: catalog stays open.
    let padmin = create_user(&t.pool, "m2-dg-padmin").await;
    grant_platform(&t.pool, padmin, &["users.manage", "roles.manage"]).await;
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         VALUES ($1, $2, 'active')",
    )
    .bind(coop)
    .bind(padmin)
    .execute(&t.pool)
    .await
    .unwrap();
    let (_, pcookie, pcsrf) = login_with(&t.app, t.peer, "m2-dg-padmin", TEST_PASSWORD).await;
    switch_cooperative(&t.app, &pcookie, coop).await;
    let (plat_ok, _) = send(
        &t.app,
        request_builder(
            "PUT",
            &format!("/api/roles/{role}/permissions"),
            &pcookie,
            Some(&pcsrf),
            Some(json!({ "permissions": ["payments.manage"] })),
        ),
    )
    .await;
    assert_eq!(plat_ok, StatusCode::NO_CONTENT);
    assert_eq!(keys_of(&t.pool, role).await, 1);
}
