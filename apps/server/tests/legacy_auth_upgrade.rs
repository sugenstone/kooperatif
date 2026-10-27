//! Legacy auth upgrade compatibility — users whose rows predate the
//! crypt login_verifier lane (login_verifier IS NULL) cannot log in and
//! must be recovered through PD-14 platform reset. Also covers the
//! readiness gap check `db::legacy_verifier_gap`.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database).

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

const TEST_PASSWORD: &str = "legacy-test-parola-123";
const NEW_PASSWORD: &str = "legacy-yeni-parola-456";
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
    repo::bootstrap_initial_cooperative(&pool, "Legacy Kooperatif")
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

/// Argon2 hash for a password — the only credential material a legacy
/// (pre-login_verifier) row carries.
fn argon2_hash(password: &str) -> String {
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).expect("params"),
    );
    identity::hash_password(&argon2, password).expect("hash")
}

/// Seed a legacy-era user: Argon2id hash present, crypt verifier absent.
/// Mirrors v20/v33 rows observed in the upgrade rehearsal.
async fn seed_legacy_user(pool: &PgPool, username: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, login_verifier) \
         VALUES ($1, $2, $3, $4, NULL)",
    )
    .bind(id)
    .bind(username)
    .bind("Legacy Kullanıcı")
    .bind(argon2_hash(TEST_PASSWORD))
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn create_user(pool: &PgPool, username: &str) -> Uuid {
    users::create_user(
        pool,
        username,
        "M2 Test",
        &argon2_hash(TEST_PASSWORD),
        TEST_PASSWORD,
    )
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
            .bind(format!("coop-legacy-{user}-{coop}"))
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

async fn login_with(
    app: &axum::Router,
    peer: SocketAddr,
    username: &str,
    password: &str,
) -> (StatusCode, String, String) {
    let mut request = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .header("user-agent", "Mozilla/5.0 (Windows NT 10.0) Chrome/126.0")
        .body(Body::from(
            json!({ "username": username, "password": password }).to_string(),
        ))
        .expect("valid request");
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

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    (status, json)
}

/// Failure proof + supported recovery path:
/// legacy NULL-verifier user cannot log in (401), the gap check reports
/// them, and PD-14 platform reset produces a working credential that
/// rotates into a valid verifier through the forced-change lane.
#[tokio::test]
async fn legacy_user_login_fails_until_platform_reset_recovery() {
    let t = setup().await;
    let legacy = seed_legacy_user(&t.pool, "legacy_u1").await;
    let coop = bootstrap_coop(&t.pool).await;
    grant_in_cooperative(&t.pool, coop, legacy, &["users.read"]).await;
    let admin = create_user(&t.pool, "legacyadm").await;
    grant_platform(&t.pool, admin, &["users.manage"]).await;

    // PROOF: the Argon2-only credential cannot pass app_session_open.
    let (s, _, _) = login_with(&t.app, t.peer, "legacy_u1", TEST_PASSWORD).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "legacy verifier-less login");

    // Readiness check detects the affected row.
    let gap = app_db::legacy_verifier_gap(&t.pool)
        .await
        .expect("gap count");
    assert!(gap >= 1, "gap check should count the legacy user");

    // Recovery: platform admin reset -> temp password -> forced change.
    let (_, acookie, acsrf) = login_with(&t.app, t.peer, "legacyadm", TEST_PASSWORD).await;
    let (s, body) = send(
        &t.app,
        request_builder(
            "POST",
            &format!("/api/platform/users/{legacy}/reset-password"),
            &acookie,
            Some(&acsrf),
            None,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let temp = body["temporaryPassword"].as_str().unwrap().to_string();

    let (s, cookie1, csrf1) = login_with(&t.app, t.peer, "legacy_u1", &temp).await;
    assert_eq!(s, StatusCode::OK);
    // Forced-change gate: cooperative API denied until rotation.
    let (s, _) = send(
        &t.app,
        request_builder("GET", "/api/users", &cookie1, None, None),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let mut pw_req = request_builder(
        "POST",
        "/api/auth/password",
        &cookie1,
        Some(&csrf1),
        Some(json!({ "currentPassword": temp, "newPassword": NEW_PASSWORD })),
    );
    pw_req.extensions_mut().insert(ConnectInfo(t.peer));
    let (s, _) = send(&t.app, pw_req).await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    // Recovered: verifier material exists, normal login works, gap closed.
    let (s, _, _) = login_with(&t.app, t.peer, "legacy_u1", NEW_PASSWORD).await;
    assert_eq!(s, StatusCode::OK);
    let verifier: bool =
        sqlx::query_scalar("SELECT login_verifier IS NOT NULL FROM users WHERE id = $1")
            .bind(legacy)
            .fetch_one(&t.pool)
            .await
            .unwrap();
    assert!(verifier);
    let gap = app_db::legacy_verifier_gap(&t.pool)
        .await
        .expect("gap count");
    assert_eq!(gap, 0);

    t.cleanup().await;
}

/// Readiness check counts only verifier-less users; ordinary users do
/// not contribute to the gap.
#[tokio::test]
async fn legacy_verifier_gap_counts_only_null_verifier_users() {
    let t = setup().await;
    seed_legacy_user(&t.pool, "legacy_g1").await;
    seed_legacy_user(&t.pool, "legacy_g2").await;
    // Empty-string verifier is as unusable as NULL.
    let blank = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, login_verifier)          VALUES ($1, $2, $3, $4, '   ')",
    )
    .bind(blank)
    .bind("legacy_blank")
    .bind("Legacy Blank")
    .bind(argon2_hash(TEST_PASSWORD))
    .execute(&t.pool)
    .await
    .unwrap();
    create_user(&t.pool, "legacy_ok").await;
    let gap = app_db::legacy_verifier_gap(&t.pool).await.expect("gap");
    assert_eq!(gap, 3);
    t.cleanup().await;
}
