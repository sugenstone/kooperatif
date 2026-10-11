//! M1-P1-012 (P1-c3c2) — cooperative-scoped payments + financial
//! accounts authorization tests.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same harness as tests/periods_income_cooperative.rs). These tests
//! run the REAL router — the P0 business-membership gate included —
//! and prove that the migrated payments and financial_accounts
//! endpoints resolve authorization only within the verified bootstrap
//! scope:
//!   - headerless requests pin bootstrap; a foreign header never
//!     re-scopes;
//!   - malformed/unknown/non-bootstrap `x-cooperative-id` rejects;
//!   - suspended/ended bootstrap membership is denied on reads and
//!     mutations alike — denied mutations write zero rows;
//!   - permissions granted in Cooperative B never authorize bootstrap
//!     (A) payment, allocation, account or transfer operations;
//!   - `/api/financial-accounts/options` keeps its exact
//!     `financial_accounts.read OR payments.manage` union semantics —
//!     and only there;
//!   - NULL-attributed legacy roles keep working inside bootstrap;
//!   - mutations keep both CSRF layers and audit events carry
//!     cooperative attribution;
//!   - the full financial path (payment → movement, allocation,
//!     unapplied remainder, credits, transfers, reversals, idempotent
//!     replay) produces identical results under scoped auth;
//!   - the never-bootstrapped lane keeps global semantics untouched.
//!
//! Financial logic parity itself is proven by the unchanged
//! `payments.rs`, `financial_accounts.rs`, `credits.rs`,
//! `idempotency_retry.rs` and `reservation.rs` regression suites; the
//! parity tests here additionally exercise the same financial paths
//! THROUGH the migrated authorization layer.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
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

const TEST_PASSWORD: &str = "p1c3c2-test-parola-123";
const ORIGIN: &str = "http://localhost:5173";

fn test_database_url() -> Option<String> {
    let value = std::env::var("KOOPERATIF_TEST_DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "KOOPERATIF_TEST_DATABASE_URL is not set — required DB-gated \
             integration tests cannot silently pass; point it at a \
             disposable PostgreSQL database"
        )
    });
    let trimmed = value.trim().to_string();
    assert!(
        !trimmed.is_empty(),
        "KOOPERATIF_TEST_DATABASE_URL is empty — required DB-gated tests need a database"
    );
    Some(trimmed)
}

struct TestApp {
    _claim: app_db::TestDbClaim,
    app: Router,
    pool: PgPool,
    peer: SocketAddr,
    database_name: String,
    admin_url: String,
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
        if let Ok(admin) = PgPoolOptions::new()
            .max_connections(1)
            .connect(&self.admin_url)
            .await
        {
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

async fn setup() -> Option<TestApp> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .try_init();
    let _setup_slot = SETUP_SLOTS.acquire().await.expect("setup slot");
    let url = test_database_url()?;
    let suffix = Uuid::new_v4().simple();
    let database_name = format!("kooperatif_test_{suffix}");
    let admin_url = url
        .rsplit_once('/')
        .map(|(base, _)| format!("{base}/postgres"))
        .expect("url with database segment");
    let base_url = url
        .rsplit_once('/')
        .map(|(base, _)| base.to_string())
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
    let app = router(
        AppState {
            db: Some(pool.clone()),
            auth: Arc::new(AuthRuntime::new(
                AuthConfig {
                    allowed_origins: vec![ORIGIN.to_string()],
                    cookie_secure: false,
                    session_absolute_ttl_secs: 3600,
                    session_idle_ttl_secs: 600,
                    argon2_m_cost: ARGON2_M_COST_FLOOR,
                    argon2_t_cost: 1,
                    argon2_p_cost: 1,
                    login_window_secs: 60,
                    login_username_max_attempts: 5,
                    login_ip_max_attempts: 100_000,
                    forwarded_ip: false,
                },
                clock,
            )),
        },
        cors_layer(&[]),
    );
    let bytes = Uuid::new_v4();
    let octets = bytes.as_bytes();
    Some(TestApp {
        _claim: claim,
        app,
        pool,
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
    })
}

async fn create_user(pool: &PgPool, username: &str) -> Uuid {
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).expect("test parameters"),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).expect("hash");
    users::create_user(pool, username, "P1c3c2 Test", &hash)
        .await
        .expect("user created")
        .id
}

/// Role + assignment fixture. `coop` None writes the legacy NULL lane;
/// `Some(c)` writes composite-FK-consistent scoped rows (membership in
/// `c` must already exist).
async fn grant_role(
    pool: &PgPool,
    user: Uuid,
    name: &str,
    coop: Option<Uuid>,
    perms: &[&str],
) -> Uuid {
    let role: Uuid =
        sqlx::query_scalar("INSERT INTO roles (name, cooperative_id) VALUES ($1, $2) RETURNING id")
            .bind(name)
            .bind(coop)
            .fetch_one(pool)
            .await
            .expect("role");
    for key in perms {
        sqlx::query(
            "INSERT INTO role_permissions (role_id, permission_id) \
             SELECT $1, id FROM permissions WHERE key = $2",
        )
        .bind(role)
        .bind(key)
        .execute(pool)
        .await
        .expect("perm");
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
    .expect("assignment");
    role
}

/// Log in over HTTP; returns (cookie token, csrf token).
async fn login(app: &Router, peer: SocketAddr, username: &str) -> (String, String) {
    let mut request = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .header("user-agent", "Mozilla/5.0 P1c3c2")
        .body(Body::from(
            json!({ "username": username, "password": TEST_PASSWORD }).to_string(),
        ))
        .expect("valid request");
    request.extensions_mut().insert(ConnectInfo(peer));
    let response = app.clone().oneshot(request).await.expect("infallible");
    assert_eq!(response.status(), StatusCode::OK, "login failed");
    let cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .and_then(|pair| pair.split_once('=').map(|(_, v)| v.to_string()))
        .expect("cookie token");
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body: Value = serde_json::from_slice(&bytes).expect("json");
    (
        cookie,
        body["csrfToken"].as_str().expect("csrf").to_string(),
    )
}

async fn request_json(
    test: &TestApp,
    method: Method,
    path: &str,
    cookie: &str,
    csrf: Option<&str>,
    coop: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, String) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("cookie", format!("kooperatif_session={cookie}"))
        .header("origin", ORIGIN);
    if let Some(token) = csrf {
        builder = builder.header("x-csrf-token", token);
    }
    if let Some(c) = coop {
        builder = builder.header("x-cooperative-id", c);
    }
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let mut request = builder.body(payload).expect("request");
    request.extensions_mut().insert(ConnectInfo(test.peer));
    let response = test.app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn bootstrap(pool: &PgPool) -> Uuid {
    repo::bootstrap_initial_cooperative(pool, "Ana Kooperatif")
        .await
        .expect("bootstrap")
        .0
}

/// Every permission the payment/account surfaces can check — plus the
/// already-migrated parties/periods permissions needed by the scoped
/// parity flow (shareholder + assessment fixtures go through HTTP).
const OPERATOR_PERMS: &[&str] = &[
    "payments.read",
    "payments.manage",
    "financial_accounts.read",
    "financial_accounts.manage",
    "periods.read",
    "periods.manage",
    "assessments.read",
    "assessments.manage",
    "shareholders.manage",
];

/// Bootstrap-scoped operator fixture: user created BEFORE bootstrap
/// (auto-member) with a bootstrap-owned business role.
async fn bootstrap_operator(pool: &PgPool, username: &str) -> (Uuid, Uuid) {
    let user = create_user(pool, username).await;
    let coop = bootstrap(pool).await;
    grant_role(
        pool,
        user,
        &format!("op-{username}"),
        Some(coop),
        OPERATOR_PERMS,
    )
    .await;
    (user, coop)
}

fn account_body(name: &str) -> Value {
    json!({ "name": name, "accountType": "cash" })
}

fn payment_body(account: Uuid, amount: &str, key: &str, allocations: Vec<Value>) -> Value {
    json!({
        "payerFirstName": "Ödeyen",
        "payerLastName": "Kişi",
        "amount": amount,
        "method": "cash",
        "destinationAccountId": account.to_string(),
        "idempotencyKey": key,
        "allocations": allocations,
    })
}

fn transfer_body(source: Uuid, destination: Uuid, amount: &str, key: &str) -> Value {
    json!({
        "sourceAccountId": source.to_string(),
        "destinationAccountId": destination.to_string(),
        "amount": amount,
        "idempotencyKey": key,
    })
}

fn period_body(amount: &str) -> Value {
    json!({
        "name": "Tahsilat Dönemi",
        "collectionStartDate": "2026-10-01",
        "dueDate": "2026-10-15",
        "ruleType": "per_shareholder",
        "baseAmount": amount,
        "assessmentEffectiveDate": "2026-10-01",
    })
}

/// Create a cash account through the migrated endpoint.
async fn make_account(test: &TestApp, cookie: &str, csrf: &str, name: &str) -> Uuid {
    let (status, body) = request_json(
        test,
        Method::POST,
        "/api/financial-accounts",
        cookie,
        Some(csrf),
        None,
        Some(account_body(name)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

/// Two shareholders + open period + generated assessments fixture
/// (through the already-migrated routers). Two obligations are needed
/// because a payment may not allocate twice to the same assessment —
/// `(payment_id, assessment_id)` is unique.
async fn make_assessments(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    amount: &str,
) -> (Uuid, Vec<Uuid>) {
    let mut shareholder = Uuid::nil();
    for (first, seq) in [("Borçlu", 1), ("İkinci", 2)] {
        let (status, body) = request_json(
            test,
            Method::POST,
            "/api/shareholders",
            cookie,
            Some(csrf),
            None,
            Some(json!({
                "person": { "mode": "new", "firstName": first, "lastName": "Hissedar" },
                "family": { "mode": "new", "sequenceNumber": seq }
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        shareholder = serde_json::from_str::<Value>(&body).unwrap()["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
    }
    let (status, body) = request_json(
        test,
        Method::POST,
        "/api/periods",
        cookie,
        Some(csrf),
        None,
        Some(period_body(amount)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let period: Uuid = serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let (status, body) = request_json(
        test,
        Method::POST,
        &format!("/api/periods/{period}/generate-assessments"),
        cookie,
        Some(csrf),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = request_json(
        test,
        Method::GET,
        &format!("/api/periods/{period}/assessments"),
        cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let list = serde_json::from_str::<Value>(&body).unwrap();
    let assessments = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .collect();
    (shareholder, assessments)
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count")
}

#[tokio::test]
async fn bootstrap_operator_reaches_payments_and_accounts() {
    let Some(test) = setup().await else { return };
    let (_, coop) = bootstrap_operator(&test.pool, "operator").await;
    let (cookie, csrf) = login(&test.app, test.peer, "operator").await;

    // Headerless bootstrap reads on both routers.
    for path in [
        "/api/payments",
        "/api/financial-accounts",
        "/api/financial-accounts/summary",
        "/api/financial-accounts/options",
        "/api/account-transfers",
    ] {
        let (status, body) =
            request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }

    // Account + payment + transfer mutations under scoped auth.
    let account = make_account(&test, &cookie, &csrf, "Kasa").await;
    let second = make_account(&test, &cookie, &csrf, "Banka").await;
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/payments",
        &cookie,
        Some(&csrf),
        None,
        Some(payment_body(account, "100.00", "p-op-1", vec![])),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/account-transfers",
        &cookie,
        Some(&csrf),
        None,
        Some(transfer_body(account, second, "40.00", "t-op-1")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Explicit bootstrap header pins the same scope — allowed.
    let coop_str = coop.to_string();
    let (status, _) = request_json(
        &test,
        Method::GET,
        "/api/payments",
        &cookie,
        None,
        Some(&coop_str),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request_json(
        &test,
        Method::GET,
        "/api/financial-accounts",
        &cookie,
        None,
        Some(&coop_str),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    test.cleanup().await;
}

#[tokio::test]
async fn cooperative_header_rules_apply_to_both_routers() {
    let Some(test) = setup().await else { return };
    bootstrap_operator(&test.pool, "hdr_op").await;
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;
    let (cookie, csrf) = login(&test.app, test.peer, "hdr_op").await;
    let b_str = coop_b.to_string();
    let unknown = Uuid::new_v4().to_string();

    for path in ["/api/payments", "/api/financial-accounts"] {
        // Malformed UUID → 400.
        let (status, _) = request_json(
            &test,
            Method::GET,
            path,
            &cookie,
            None,
            Some("not-a-uuid"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
        // Explicit non-bootstrap cooperative → 403, never a silent
        // bootstrap fallback.
        let (status, _) =
            request_json(&test, Method::GET, path, &cookie, None, Some(&b_str), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        // Unknown-but-well-formed UUID → 403 (existence never leaked).
        let (status, _) = request_json(
            &test,
            Method::GET,
            path,
            &cookie,
            None,
            Some(&unknown),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    // Mutations obey the same rules — and write nothing.
    let (status, _) = request_json(
        &test,
        Method::POST,
        "/api/financial-accounts",
        &cookie,
        Some(&csrf),
        Some(&b_str),
        Some(account_body("B Kasası")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(count(&test.pool, "financial_accounts").await, 0);
    assert_eq!(count(&test.pool, "payments").await, 0);

    test.cleanup().await;
}

#[tokio::test]
async fn suspended_and_ended_memberships_are_denied() {
    let Some(test) = setup().await else { return };
    let (user, coop) = bootstrap_operator(&test.pool, "askida").await;
    let (cookie, csrf) = login(&test.app, test.peer, "askida").await;
    let account = make_account(&test, &cookie, &csrf, "Kasa").await;

    for status in ["suspended", "ended"] {
        sqlx::query(
            "UPDATE cooperative_memberships SET status = $1 \
             WHERE user_id = $2 AND cooperative_id = $3",
        )
        .bind(status)
        .bind(user)
        .bind(coop)
        .execute(&test.pool)
        .await
        .expect("membership status");
        // Reads denied on both routers.
        for path in ["/api/payments", "/api/financial-accounts"] {
            let (code, _) = request_json(&test, Method::GET, path, &cookie, None, None, None).await;
            assert_eq!(code, StatusCode::FORBIDDEN, "{status} {path}");
        }
        // Mutations denied — authorization precedes any write.
        let (code, _) = request_json(
            &test,
            Method::POST,
            "/api/payments",
            &cookie,
            Some(&csrf),
            None,
            Some(payment_body(account, "10.00", "p-denied", vec![])),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "{status} payment");
        let (code, _) = request_json(
            &test,
            Method::POST,
            "/api/financial-accounts",
            &cookie,
            Some(&csrf),
            None,
            Some(account_body("Yasak Kasa")),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "{status} account");
    }
    // No rows written through the denied mutations.
    assert_eq!(count(&test.pool, "payments").await, 0);
    assert_eq!(count(&test.pool, "payment_allocations").await, 0);
    assert_eq!(count(&test.pool, "account_movements").await, 0);
    assert_eq!(count(&test.pool, "account_transfers").await, 0);
    assert_eq!(count(&test.pool, "financial_accounts").await, 1); // fixture only

    test.cleanup().await;
}

#[tokio::test]
async fn foreign_cooperative_permissions_never_authorize_bootstrap_ops() {
    let Some(test) = setup().await else { return };
    let coop_a = bootstrap(&test.pool).await;
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;

    // A-member whose only rights live in B (B-admin): A sees nothing.
    let b_admin = create_user(&test.pool, "b_yetkili").await;
    repo::add_member(&test.pool, "Ana Kooperatif", b_admin, None)
        .await
        .expect("A member");
    repo::add_member(&test.pool, "B Kooperatifi", b_admin, None)
        .await
        .expect("B member");
    grant_role(&test.pool, b_admin, "b-admin", Some(coop_b), OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "b_yetkili").await;
    for path in ["/api/payments", "/api/financial-accounts"] {
        let (status, _) = request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    let (status, _) = request_json(
        &test,
        Method::POST,
        "/api/financial-accounts",
        &cookie,
        Some(&csrf),
        None,
        Some(account_body("B Parası")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(count(&test.pool, "financial_accounts").await, 0);

    // Inverse direction: A-viewer / B-admin — A's reads work, B's
    // manage power never crosses into A.
    let dual = create_user(&test.pool, "cift").await;
    repo::add_member(&test.pool, "Ana Kooperatif", dual, None)
        .await
        .expect("A member");
    repo::add_member(&test.pool, "B Kooperatifi", dual, None)
        .await
        .expect("B member");
    grant_role(
        &test.pool,
        dual,
        "a-izleyici",
        Some(coop_a),
        &["payments.read", "financial_accounts.read"],
    )
    .await;
    grant_role(&test.pool, dual, "b-admin2", Some(coop_b), OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "cift").await;
    for path in ["/api/payments", "/api/financial-accounts"] {
        let (status, body) =
            request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }
    // payments.manage + financial_accounts.manage exist only in B.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/financial-accounts",
        &cookie,
        Some(&csrf),
        None,
        Some(account_body("Sızıntı")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/payments",
        &cookie,
        Some(&csrf),
        None,
        Some(payment_body(Uuid::new_v4(), "10.00", "p-foreign", vec![])),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(count(&test.pool, "financial_accounts").await, 0);
    assert_eq!(count(&test.pool, "payments").await, 0);

    test.cleanup().await;
}

#[tokio::test]
async fn account_options_union_semantics_preserved() {
    let Some(test) = setup().await else { return };
    let coop = bootstrap(&test.pool).await;

    // Only payments.manage → options reachable (picker use case).
    let payer = create_user(&test.pool, "tahsilatci").await;
    repo::add_member(&test.pool, "Ana Kooperatif", payer, None)
        .await
        .expect("A member");
    grant_role(
        &test.pool,
        payer,
        "tahsilat-rol",
        Some(coop),
        &["payments.manage"],
    )
    .await;
    let (cookie, _) = login(&test.app, test.peer, "tahsilatci").await;
    let (status, body) = request_json(
        &test,
        Method::GET,
        "/api/financial-accounts/options",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // …but the union never bleeds into other endpoints.
    let (status, _) = request_json(
        &test,
        Method::GET,
        "/api/financial-accounts",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Only financial_accounts.read → options reachable.
    let viewer = create_user(&test.pool, "izleyici").await;
    repo::add_member(&test.pool, "Ana Kooperatif", viewer, None)
        .await
        .expect("A member");
    grant_role(
        &test.pool,
        viewer,
        "izleyici-rol",
        Some(coop),
        &["financial_accounts.read"],
    )
    .await;
    let (cookie, _) = login(&test.app, test.peer, "izleyici").await;
    let (status, body) = request_json(
        &test,
        Method::GET,
        "/api/financial-accounts/options",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // payments.read alone satisfies NEITHER arm → denied.
    let payviewer = create_user(&test.pool, "tahsilat-izle").await;
    repo::add_member(&test.pool, "Ana Kooperatif", payviewer, None)
        .await
        .expect("A member");
    grant_role(
        &test.pool,
        payviewer,
        "izle-rol",
        Some(coop),
        &["payments.read"],
    )
    .await;
    let (cookie, _) = login(&test.app, test.peer, "tahsilat-izle").await;
    let (status, _) = request_json(
        &test,
        Method::GET,
        "/api/financial-accounts/options",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    test.cleanup().await;
}

#[tokio::test]
async fn null_attributed_legacy_roles_keep_working() {
    let Some(test) = setup().await else { return };
    bootstrap(&test.pool).await;
    // Post-bootstrap user with NULL-cooperative role + assignment
    // (legacy lane) and an explicit bootstrap membership.
    let legacy = create_user(&test.pool, "mirasci").await;
    repo::add_member(&test.pool, "Ana Kooperatif", legacy, None)
        .await
        .expect("A member");
    grant_role(&test.pool, legacy, "miras-rol", None, OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "mirasci").await;
    let (status, body) = request_json(
        &test,
        Method::GET,
        "/api/payments",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/financial-accounts",
        &cookie,
        Some(&csrf),
        None,
        Some(account_body("Miras Kasa")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    test.cleanup().await;
}

#[tokio::test]
async fn unauthorized_reads_and_mutations_write_nothing() {
    let Some(test) = setup().await else { return };
    bootstrap(&test.pool).await;
    // Active bootstrap member with NO role at all.
    let nobody = create_user(&test.pool, "yetkisiz").await;
    repo::add_member(&test.pool, "Ana Kooperatif", nobody, None)
        .await
        .expect("A member");
    let (cookie, csrf) = login(&test.app, test.peer, "yetkisiz").await;
    let id = Uuid::new_v4();

    for path in [
        "/api/payments",
        &format!("/api/payments/{id}"),
        "/api/payments/payer-persons?search=a",
        "/api/financial-accounts",
        "/api/financial-accounts/options",
        "/api/financial-accounts/summary",
        &format!("/api/financial-accounts/{id}"),
        &format!("/api/financial-accounts/{id}/movements"),
        "/api/account-transfers",
        &format!("/api/account-transfers/{id}"),
        &format!("/api/assessments/{id}/payments"),
        &format!("/api/shareholders/{id}/open-assessments"),
        &format!("/api/shareholders/{id}/financial-summary"),
        &format!("/api/periods/{id}/financial-summary"),
        &format!("/api/families/{id}/collection-context"),
    ] {
        let (status, body) =
            request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body}");
    }

    // Every mutation surface denied — and every write zero.
    let account = Uuid::new_v4();
    for (method, path, body) in [
        (
            Method::POST,
            "/api/financial-accounts".to_string(),
            Some(account_body("X")),
        ),
        (
            Method::PATCH,
            format!("/api/financial-accounts/{account}"),
            Some(json!({ "name": "Y", "expectedUpdatedAt": "2026-01-01T00:00:00Z" })),
        ),
        (
            Method::POST,
            format!("/api/financial-accounts/{account}/status-change"),
            Some(json!({ "status": "inactive" })),
        ),
        (
            Method::POST,
            "/api/account-transfers".to_string(),
            Some(transfer_body(account, Uuid::new_v4(), "5.00", "unauth-t")),
        ),
        (
            Method::POST,
            format!("/api/account-transfers/{id}/reverse"),
            Some(json!({ "reason": "iptal" })),
        ),
        (
            Method::POST,
            "/api/payments".to_string(),
            Some(payment_body(account, "10.00", "unauth-p", vec![])),
        ),
        (
            Method::POST,
            format!("/api/payments/{id}/allocations"),
            Some(json!({
                "allocations": [{ "assessmentId": Uuid::new_v4(), "amount": "1.00" }]
            })),
        ),
        (
            Method::POST,
            format!("/api/payments/{id}/allocations/{id}/reverse"),
            Some(json!({ "reason": "iptal" })),
        ),
        (
            Method::POST,
            format!("/api/payments/{id}/reverse"),
            Some(json!({ "reason": "iptal" })),
        ),
    ] {
        let (status, out) =
            request_json(&test, method, &path, &cookie, Some(&csrf), None, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {out}");
    }
    assert_eq!(count(&test.pool, "payments").await, 0);
    assert_eq!(count(&test.pool, "payment_allocations").await, 0);
    assert_eq!(count(&test.pool, "shareholder_credits").await, 0);
    assert_eq!(count(&test.pool, "credit_applications").await, 0);
    assert_eq!(count(&test.pool, "financial_accounts").await, 0);
    assert_eq!(count(&test.pool, "account_movements").await, 0);
    assert_eq!(count(&test.pool, "account_transfers").await, 0);

    test.cleanup().await;
}

#[tokio::test]
async fn mutations_still_require_both_csrf_layers() {
    let Some(test) = setup().await else { return };
    bootstrap_operator(&test.pool, "csrf_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "csrf_op").await;
    let account = make_account(&test, &cookie, &csrf, "Kasa").await;

    // No synchronizer token → CSRF layer rejects before authorization.
    let (status, _) = request_json(
        &test,
        Method::POST,
        "/api/payments",
        &cookie,
        None,
        None,
        Some(payment_body(account, "10.00", "p-csrf", vec![])),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Wrong token → same.
    let (status, _) = request_json(
        &test,
        Method::POST,
        "/api/account-transfers",
        &cookie,
        Some("bogus-token"),
        None,
        Some(transfer_body(account, Uuid::new_v4(), "5.00", "t-csrf")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Correct token → succeeds (proves the prologue ordering).
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/payments",
        &cookie,
        Some(&csrf),
        None,
        Some(payment_body(account, "10.00", "p-csrf-ok", vec![])),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    test.cleanup().await;
}

#[tokio::test]
async fn audit_events_carry_verified_cooperative_attribution() {
    let Some(test) = setup().await else { return };
    let (_, coop) = bootstrap_operator(&test.pool, "audit_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "audit_op").await;

    let source = make_account(&test, &cookie, &csrf, "Kaynak").await;
    let destination = make_account(&test, &cookie, &csrf, "Hedef").await;
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/payments",
        &cookie,
        Some(&csrf),
        None,
        Some(payment_body(source, "50.00", "p-audit", vec![])),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/account-transfers",
        &cookie,
        Some(&csrf),
        None,
        Some(transfer_body(source, destination, "10.00", "t-audit")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Every coop-bound event carries the verified bootstrap id — never
    // a client-supplied value.
    let (scoped,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM security_events \
         WHERE cooperative_id = $1 \
           AND event_type IN ('financial_account_created','payment_posted','account_transfer_posted')",
    )
    .bind(coop)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert!(scoped >= 4, "expected coop-attributed events, got {scoped}");
    let (unscoped,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM security_events \
         WHERE cooperative_id IS NULL \
           AND event_type IN ('financial_account_created','payment_posted','account_transfer_posted')",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(unscoped, 0, "business audit events must be attributed");

    test.cleanup().await;
}

/// End-to-end financial parity through the migrated authorization:
/// payment → exactly one account inflow → partial allocation keeps
/// the unapplied remainder → add-allocations writes NO movement →
/// idempotent replay is side-effect-free → transfer posts exactly two
/// paired legs → transfer reversal restores accounting → payment
/// reversal marks historical rows. All under scoped auth.
#[tokio::test]
async fn scoped_financial_flow_parity() {
    let Some(test) = setup().await else { return };
    bootstrap_operator(&test.pool, "parity_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "parity_op").await;

    let source = make_account(&test, &cookie, &csrf, "Tahsilat Kasa").await;
    let second = make_account(&test, &cookie, &csrf, "Merkez Banka").await;
    let (_, assessments) = make_assessments(&test, &cookie, &csrf, "1000.00").await;
    let first_obligation = assessments[0];
    let second_obligation = assessments[1];

    // Payment 1500: 700 partially allocated to the first obligation,
    // 800 unapplied — and exactly ONE destination-account inflow.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/payments",
        &cookie,
        Some(&csrf),
        None,
        Some(payment_body(
            source,
            "1500.00",
            "p-parity",
            vec![json!({ "assessmentId": first_obligation, "amount": "700.00" })],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let payment = serde_json::from_str::<Value>(&body).unwrap()["payment"].clone();
    let payment_id = payment["id"].as_str().unwrap().to_string();
    assert_eq!(payment["unallocatedAmount"], "800.00");
    assert_eq!(count(&test.pool, "account_movements").await, 1);
    let (inflow,): (String,) = sqlx::query_as(
        "SELECT direction FROM account_movements \
         WHERE source_type = 'payment' AND account_id = $1",
    )
    .bind(source)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(inflow, "inflow");

    // Idempotent replay → 200, no second payment/movement/audit.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/payments",
        &cookie,
        Some(&csrf),
        None,
        Some(payment_body(
            source,
            "1500.00",
            "p-parity",
            vec![json!({ "assessmentId": first_obligation, "amount": "700.00" })],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(count(&test.pool, "payments").await, 1);
    assert_eq!(count(&test.pool, "account_movements").await, 1);
    let (posted_events,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM security_events WHERE event_type = 'payment_posted'")
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert_eq!(posted_events, 1, "idempotent replay must not re-audit");

    // Conflicting fingerprint under the same key → 409.
    let (status, _) = request_json(
        &test,
        Method::POST,
        "/api/payments",
        &cookie,
        Some(&csrf),
        None,
        Some(payment_body(source, "999.00", "p-parity", vec![])),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // add-allocations applies 300 to the SECOND obligation (a payment
    // may not allocate twice to the same assessment) — still NO new
    // movement (cash moved once, at posting).
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/payments/{payment_id}/allocations"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({
            "allocations": [{ "assessmentId": second_obligation, "amount": "300.00" }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(count(&test.pool, "account_movements").await, 1);
    let (status, body) = request_json(
        &test,
        Method::GET,
        &format!("/api/payments/{payment_id}"),
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let detail: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(detail["unallocatedAmount"], "500.00");

    // Transfer: one transfer row + exactly two paired movement legs.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/account-transfers",
        &cookie,
        Some(&csrf),
        None,
        Some(transfer_body(source, second, "200.00", "t-parity")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let transfer: Value = serde_json::from_str(&body).unwrap();
    let transfer_id = transfer["id"].as_str().unwrap().to_string();
    assert_eq!(count(&test.pool, "account_movements").await, 3);
    let (legs,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM account_movements \
         WHERE source_type = 'transfer' AND source_id = $1",
    )
    .bind(transfer["id"].as_str().unwrap().parse::<Uuid>().unwrap())
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(legs, 2);
    // Idempotent transfer replay → no duplicate legs.
    let (status, _) = request_json(
        &test,
        Method::POST,
        "/api/account-transfers",
        &cookie,
        Some(&csrf),
        None,
        Some(transfer_body(source, second, "200.00", "t-parity")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(count(&test.pool, "account_transfers").await, 1);
    assert_eq!(count(&test.pool, "account_movements").await, 3);

    // Transfer reversal preserves the reversal contract.
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/account-transfers/{transfer_id}/reverse"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reason": "yanlış hesap" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = request_json(
        &test,
        Method::GET,
        &format!("/api/account-transfers/{transfer_id}"),
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["status"],
        "reversed"
    );

    // Payment reversal: payment + active allocations become
    // historical; nothing is deleted.
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/payments/{payment_id}/reverse"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reason": "iade" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = request_json(
        &test,
        Method::GET,
        &format!("/api/payments/{payment_id}"),
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let reversed: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(reversed["status"], "reversed");
    let (active_allocs,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM payment_allocations \
         WHERE payment_id = $1 AND status = 'active'",
    )
    .bind(payment_id.parse::<Uuid>().unwrap())
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(active_allocs, 0);

    test.cleanup().await;
}

/// Two concurrent posts sharing one idempotency key: exactly one
/// payment row and exactly one account movement survive.
#[tokio::test]
async fn concurrent_idempotent_posts_single_write() {
    let Some(test) = setup().await else { return };
    bootstrap_operator(&test.pool, "race_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "race_op").await;
    let account = make_account(&test, &cookie, &csrf, "Kasa").await;

    let post = || {
        request_json(
            &test,
            Method::POST,
            "/api/payments",
            &cookie,
            Some(&csrf),
            None,
            Some(payment_body(account, "100.00", "p-race", vec![])),
        )
    };
    let (r1, r2) = tokio::join!(post(), post());
    let statuses = [r1.0, r2.0];
    assert!(
        statuses
            .iter()
            .all(|s| *s == StatusCode::CREATED || *s == StatusCode::OK),
        "expected idempotent outcomes, got {statuses:?}"
    );
    assert_eq!(count(&test.pool, "payments").await, 1);
    assert_eq!(count(&test.pool, "account_movements").await, 1);

    test.cleanup().await;
}

/// Credit parity under scoped auth: a payment whose credit has an
/// ACTIVE application is blocked from reversal (CreditEffectBlocked);
/// once the application is gone the same reversal succeeds and the
/// unconsumed credit is reversed alongside — identical semantics to
/// the unchanged global-auth regression suite.
#[tokio::test]
async fn credit_effect_blocked_parity() {
    let Some(test) = setup().await else { return };
    let (user, _) = bootstrap_operator(&test.pool, "kredi_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "kredi_op").await;

    let account = make_account(&test, &cookie, &csrf, "Kasa").await;
    let (shareholder, assessments) = make_assessments(&test, &cookie, &csrf, "1000.00").await;
    let assessment = assessments[0];

    // Overpayment: 1200 against 1000 → 200 unapplied.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/payments",
        &cookie,
        Some(&csrf),
        None,
        Some(payment_body(
            account,
            "1200.00",
            "p-credit",
            vec![json!({ "assessmentId": assessment, "amount": "1000.00" })],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let payment = serde_json::from_str::<Value>(&body).unwrap()["payment"].clone();
    let payment_id = payment["id"].as_str().unwrap().to_string();
    assert_eq!(payment["unallocatedAmount"], "200.00");

    // Fixture credit sourced from this payment + an ACTIVE
    // application against the assessment (the credits router is still
    // global-auth; fixtures must not depend on it).
    let credit: Uuid = sqlx::query_scalar(
        "INSERT INTO shareholder_credits \
         (source_payment_id, shareholder_id, amount, idempotency_key, idempotency_fingerprint, created_by) \
         VALUES ($1, $2, '200.00', 'fix-credit', 'fix-fp', $3) RETURNING id",
    )
    .bind(payment_id.parse::<Uuid>().unwrap())
    .bind(shareholder)
    .bind(user)
    .fetch_one(&test.pool)
    .await
    .expect("credit fixture");
    let application: Uuid = sqlx::query_scalar(
        "INSERT INTO credit_applications \
         (credit_id, assessment_id, amount, mode, created_by) \
         VALUES ($1, $2, '200.00', 'manual', $3) RETURNING id",
    )
    .bind(credit)
    .bind(assessment)
    .bind(user)
    .fetch_one(&test.pool)
    .await
    .expect("application fixture");

    // Consumed credit → reversal refused (CreditEffectBlocked → 409).
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/payments/{payment_id}/reverse"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reason": "iade" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    // Payment and credit untouched.
    let (status, body) = request_json(
        &test,
        Method::GET,
        &format!("/api/payments/{payment_id}"),
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["status"],
        "posted"
    );

    // Unconsume the application → reversal now succeeds and the
    // unconsumed credit is reversed together with the payment.
    sqlx::query("DELETE FROM credit_applications WHERE id = $1")
        .bind(application)
        .execute(&test.pool)
        .await
        .expect("unconsume");
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/payments/{payment_id}/reverse"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reason": "iade" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (credit_status,): (String,) =
        sqlx::query_as("SELECT status FROM shareholder_credits WHERE id = $1")
            .bind(credit)
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert_eq!(credit_status, "reversed");

    test.cleanup().await;
}

#[tokio::test]
async fn never_bootstrapped_lane_keeps_legacy_semantics() {
    let Some(test) = setup().await else { return };
    // No bootstrap at all — the documented legacy global lane.
    let user = create_user(&test.pool, "eski_serit").await;
    grant_role(&test.pool, user, "legacy-op", None, OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "eski_serit").await;
    let (status, body) = request_json(
        &test,
        Method::GET,
        "/api/payments",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let account = make_account(&test, &cookie, &csrf, "Eski Kasa").await;
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/payments",
        &cookie,
        Some(&csrf),
        None,
        Some(payment_body(account, "25.00", "p-legacy", vec![])),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(count(&test.pool, "account_movements").await, 1);

    test.cleanup().await;
}
