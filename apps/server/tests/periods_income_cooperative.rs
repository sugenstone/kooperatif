//! M1-P1-011 (P1-c3c1) — cooperative-scoped periods + income/expense
//! authorization tests.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same harness as tests/parties_reports_cooperative.rs). These tests
//! run the REAL router — the P0 business-membership gate included —
//! and prove that the migrated periods and income_expense endpoints
//! resolve authorization only within the verified bootstrap scope:
//!   - headerless requests pin bootstrap; a foreign header never
//!     re-scopes;
//!   - malformed/unknown/non-bootstrap `x-cooperative-id` rejects;
//!   - suspended/ended bootstrap membership is denied on reads and
//!     mutations alike — denied mutations write zero rows;
//!   - permissions granted in Cooperative B never authorize bootstrap
//!     (A) period, assessment, category or ledger operations;
//!   - NULL-attributed legacy roles keep working inside bootstrap;
//!   - mutations keep both CSRF layers and audit events carry
//!     cooperative attribution;
//!   - the financial flow (assessment generation, entry posting,
//!     ledger summary) produces identical results under scoped auth;
//!   - the never-bootstrapped lane keeps global semantics untouched.
//!
//! Financial logic parity itself is proven by the unchanged
//! `periods.rs`, `income_expense.rs` and `idempotency_retry.rs`
//! regression suites; the parity test here additionally exercises the
//! full financial path THROUGH the migrated authorization layer.

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

const TEST_PASSWORD: &str = "p1c3c1-test-parola-123";
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
    app_db::run_test_migrations(&pool)
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
    users::create_user(pool, username, "P1c3c1 Test", &hash, TEST_PASSWORD)
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
        .header("user-agent", "Mozilla/5.0 P1c3c1")
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

/// Every permission this router group can check — a full operator.
const OPERATOR_PERMS: &[&str] = &[
    "periods.read",
    "periods.manage",
    "assessments.read",
    "assessments.manage",
    "income_expense.read",
    "income_expense.manage",
    // Needed by the scoped parity flow (shareholder fixture via the
    // already-migrated parties router).
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

fn period_body() -> Value {
    json!({
        "name": "2026 Ekim Dönemi",
        "collectionStartDate": "2026-10-01",
        "dueDate": "2026-10-15",
        "ruleType": "per_shareholder",
        "baseAmount": "1000.00",
        "assessmentEffectiveDate": "2026-10-01",
    })
}

/// A cash account written directly — the `financial_accounts` router
/// is still on global authorization (P1-c3c2+), so test fixtures must
/// not depend on it. C3: bound to the CURRENT verified scope — the
/// bootstrap cooperative when bootstrapped, NULL otherwise (scoped
/// account locks never see NULL rows).
async fn make_account(pool: &PgPool, created_by: Uuid) -> Uuid {
    let coop: Option<Uuid> =
        sqlx::query_scalar("SELECT bootstrap_cooperative_id FROM installation_state")
            .fetch_one(pool)
            .await
            .expect("installation scope");
    sqlx::query_scalar(
        "INSERT INTO financial_accounts (name, account_type, created_by, cooperative_id) \
         VALUES ('Kasa', 'cash', $1, $2) RETURNING id",
    )
    .bind(created_by)
    .bind(coop)
    .fetch_one(pool)
    .await
    .expect("account")
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count")
}

/// Migration 0010 seeds 10 categories with `created_by IS NULL`; only
/// user-written rows prove a mutation leaked.
async fn count_user_categories(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM financial_categories WHERE created_by IS NOT NULL")
        .fetch_one(pool)
        .await
        .expect("count")
}

#[tokio::test]
async fn bootstrap_operator_reaches_periods_and_income_expense() {
    let Some(test) = setup().await else { return };
    let (user, coop) = bootstrap_operator(&test.pool, "operator").await;
    let (cookie, csrf) = login(&test.app, test.peer, "operator").await;

    // Headerless bootstrap read.
    let (status, body) = request_json(
        &test,
        Method::GET,
        "/api/periods",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(serde_json::from_str::<Value>(&body).unwrap()["items"].is_array());

    // Period mutation with CSRF.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/periods",
        &cookie,
        Some(&csrf),
        None,
        Some(period_body()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Income/expense reads + category mutation.
    let (status, body) = request_json(
        &test,
        Method::GET,
        "/api/financial-categories",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = request_json(
        &test,
        Method::GET,
        "/api/income-expense/summary",
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
        "/api/financial-categories",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "categoryType": "income", "name": "Bağış" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Explicit bootstrap header pins the same scope — allowed.
    let coop_str = coop.to_string();
    let (status, _) = request_json(
        &test,
        Method::GET,
        "/api/periods",
        &cookie,
        None,
        Some(&coop_str),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let _ = user;

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

    for path in ["/api/periods", "/api/income-expense/summary"] {
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
        "/api/periods",
        &cookie,
        Some(&csrf),
        Some(&b_str),
        Some(period_body()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(count(&test.pool, "periods").await, 0);

    test.cleanup().await;
}

#[tokio::test]
async fn suspended_and_ended_memberships_are_denied() {
    let Some(test) = setup().await else { return };
    let (user, coop) = bootstrap_operator(&test.pool, "askida").await;
    let (cookie, csrf) = login(&test.app, test.peer, "askida").await;

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
        for path in ["/api/periods", "/api/financial-categories"] {
            let (code, _) = request_json(&test, Method::GET, path, &cookie, None, None, None).await;
            assert_eq!(code, StatusCode::FORBIDDEN, "{status} {path}");
        }
        // Mutations denied — authorization precedes any write.
        let (code, _) = request_json(
            &test,
            Method::POST,
            "/api/periods",
            &cookie,
            Some(&csrf),
            None,
            Some(period_body()),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "{status} period");
        let (code, _) = request_json(
            &test,
            Method::POST,
            "/api/financial-categories",
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "categoryType": "expense", "name": "Fatura" })),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "{status} category");
    }
    // No rows written through the denied mutations.
    assert_eq!(count(&test.pool, "periods").await, 0);
    assert_eq!(count_user_categories(&test.pool).await, 0);
    assert_eq!(count(&test.pool, "income_entries").await, 0);
    assert_eq!(count(&test.pool, "account_movements").await, 0);

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
    for path in ["/api/periods", "/api/income-expense/summary"] {
        let (status, _) = request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    let (status, _) = request_json(
        &test,
        Method::POST,
        "/api/periods",
        &cookie,
        Some(&csrf),
        None,
        Some(period_body()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(count(&test.pool, "periods").await, 0);

    // Inverse direction: A-viewer / B-admin — A's read works, B's
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
        &["periods.read", "income_expense.read"],
    )
    .await;
    grant_role(&test.pool, dual, "b-admin2", Some(coop_b), OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "cift").await;
    let (status, body) = request_json(
        &test,
        Method::GET,
        "/api/periods",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = request_json(
        &test,
        Method::GET,
        "/api/financial-categories",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // periods.manage + income_expense.manage exist only in B → denied.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/periods",
        &cookie,
        Some(&csrf),
        None,
        Some(period_body()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/financial-categories",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "categoryType": "income", "name": "Aidat Dışı" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(count(&test.pool, "periods").await, 0);
    assert_eq!(count_user_categories(&test.pool).await, 0);

    test.cleanup().await;
}

#[tokio::test]
async fn null_attributed_legacy_roles_keep_working() {
    let Some(test) = setup().await else { return };
    bootstrap(&test.pool).await;
    // Post-bootstrap user with NULL-cooperative role + assignment
    // (legacy lane) and an explicit bootstrap membership — the
    // verified attribution keeps the grant usable.
    let legacy = create_user(&test.pool, "mirasci").await;
    repo::add_member(&test.pool, "Ana Kooperatif", legacy, None)
        .await
        .expect("A member");
    grant_role(&test.pool, legacy, "miras-rol", None, OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "mirasci").await;
    let (status, body) = request_json(
        &test,
        Method::GET,
        "/api/periods",
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
        "/api/periods",
        &cookie,
        Some(&csrf),
        None,
        Some(period_body()),
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
    let account = make_account(&test.pool, nobody).await;
    let (cookie, csrf) = login(&test.app, test.peer, "yetkisiz").await;

    for path in [
        "/api/periods",
        "/api/periods/00000000-0000-0000-0000-000000000000",
        "/api/periods/00000000-0000-0000-0000-000000000000/assessments",
        "/api/assessments/00000000-0000-0000-0000-000000000000",
        "/api/shareholders/00000000-0000-0000-0000-000000000000/assessments",
        "/api/financial-categories",
        "/api/financial-categories/options?categoryType=income",
        "/api/incomes",
        "/api/expenses",
        "/api/incomes/00000000-0000-0000-0000-000000000000",
        "/api/expenses/00000000-0000-0000-0000-000000000000",
        "/api/income-expense/summary",
    ] {
        let (status, body) =
            request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body}");
    }

    // Every mutation surface denied — and every write zero.
    let period = Uuid::new_v4();
    for (method, path, body) in [
        (
            Method::POST,
            "/api/periods".to_string(),
            Some(period_body()),
        ),
        (
            Method::POST,
            format!("/api/periods/{period}/assessment-preview"),
            None,
        ),
        (
            Method::POST,
            format!("/api/periods/{period}/generate-assessments"),
            None,
        ),
        (Method::POST, format!("/api/periods/{period}/close"), None),
        (
            Method::POST,
            "/api/financial-categories".to_string(),
            Some(json!({ "categoryType": "income", "name": "X" })),
        ),
        (
            Method::POST,
            "/api/incomes".to_string(),
            Some(json!({
                "financialAccountId": account.to_string(),
                "categoryId": Uuid::new_v4().to_string(),
                "amount": "10.00",
                "description": "deneme",
                "idempotencyKey": "unauth-income-1",
            })),
        ),
        (
            Method::POST,
            "/api/expenses".to_string(),
            Some(json!({
                "financialAccountId": account.to_string(),
                "categoryId": Uuid::new_v4().to_string(),
                "amount": "10.00",
                "description": "deneme",
                "idempotencyKey": "unauth-expense-1",
            })),
        ),
        (
            Method::POST,
            format!("/api/incomes/{}/reverse", Uuid::new_v4()),
            Some(json!({ "reason": "iptal" })),
        ),
    ] {
        let (status, out) =
            request_json(&test, method, &path, &cookie, Some(&csrf), None, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {out}");
    }
    assert_eq!(count(&test.pool, "periods").await, 0);
    assert_eq!(count(&test.pool, "assessments").await, 0);
    assert_eq!(count_user_categories(&test.pool).await, 0);
    assert_eq!(count(&test.pool, "income_entries").await, 0);
    assert_eq!(count(&test.pool, "expense_entries").await, 0);
    assert_eq!(count(&test.pool, "account_movements").await, 0);

    test.cleanup().await;
}

#[tokio::test]
async fn mutations_still_require_both_csrf_layers() {
    let Some(test) = setup().await else { return };
    bootstrap_operator(&test.pool, "csrf_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "csrf_op").await;

    // No synchronizer token → CSRF layer rejects before authorization.
    let (status, _) = request_json(
        &test,
        Method::POST,
        "/api/periods",
        &cookie,
        None,
        None,
        Some(period_body()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Wrong token → same.
    let (status, _) = request_json(
        &test,
        Method::POST,
        "/api/periods",
        &cookie,
        Some("bogus-token"),
        None,
        Some(period_body()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // The read-only preview command keeps its CSRF guard too.
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/periods/{}/assessment-preview", Uuid::new_v4()),
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Correct token → succeeds (proves the prologue ordering).
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/periods",
        &cookie,
        Some(&csrf),
        None,
        Some(period_body()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    test.cleanup().await;
}

#[tokio::test]
async fn audit_events_carry_verified_cooperative_attribution() {
    let Some(test) = setup().await else { return };
    let (user, coop) = bootstrap_operator(&test.pool, "audit_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "audit_op").await;
    let account = make_account(&test.pool, user).await;

    // Period create → period_created.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/periods",
        &cookie,
        Some(&csrf),
        None,
        Some(period_body()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // Category create → financial_category_created.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/financial-categories",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "categoryType": "income", "name": "Bağış" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let category: Uuid = serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    // Income posting → income_posted.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/incomes",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({
            "financialAccountId": account.to_string(),
            "categoryId": category.to_string(),
            "amount": "25.00",
            "description": "bağış geliri",
            "idempotencyKey": "audit-income-1",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Every coop-bound event carries the verified bootstrap id — never
    // a client-supplied value.
    let (scoped,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM security_events \
         WHERE cooperative_id = $1 \
           AND event_type IN ('period_created','financial_category_created','income_posted')",
    )
    .bind(coop)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert!(scoped >= 3, "expected coop-attributed events, got {scoped}");
    let (unscoped,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM security_events \
         WHERE cooperative_id IS NULL \
           AND event_type IN ('period_created','financial_category_created','income_posted')",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(unscoped, 0, "business audit events must be attributed");

    test.cleanup().await;
}

/// End-to-end financial flow through the migrated authorization:
/// shareholder → period → preview → generate → debt view, and
/// income + expense → operational summary. Proves the scoped context
/// produces identical financial outcomes.
#[tokio::test]
async fn scoped_financial_flow_parity() {
    let Some(test) = setup().await else { return };
    let (user, _) = bootstrap_operator(&test.pool, "parity_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "parity_op").await;

    // One shareholder (parties router already migrated — same gate).
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/shareholders",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({
            "person": { "mode": "new", "firstName": "Parite", "lastName": "Hissedar" },
            "family": { "mode": "new", "sequenceNumber": 7 }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let shareholder_id = serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Draft period, per_shareholder rule, 1000.00.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/periods",
        &cookie,
        Some(&csrf),
        None,
        Some(period_body()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let period_id = serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Preview: 1 eligible shareholder, total = baseAmount (no write).
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/periods/{period_id}/assessment-preview"),
        &cookie,
        Some(&csrf),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let preview: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(preview["eligibleShareholderCount"], 1);
    assert_eq!(preview["totalAmount"], "1000.00");
    assert_eq!(count(&test.pool, "assessments").await, 0);

    // Generate: atomic obligation creation through scoped auth.
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/periods/{period_id}/generate-assessments"),
        &cookie,
        Some(&csrf),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let generated: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(generated["assessmentCount"], 1);
    assert_eq!(generated["totalAssessment"], "1000.00");

    // Shareholder debt view: the obligation is durable and exact.
    let (status, body) = request_json(
        &test,
        Method::GET,
        &format!("/api/shareholders/{shareholder_id}/assessments"),
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let debts: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(debts.as_array().unwrap().len(), 1);
    assert_eq!(debts[0]["amount"], "1000.00");

    // Ledger: income 250.00 − expense 40.00 → net 210.00.
    let account = make_account(&test.pool, user).await;
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/financial-categories",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "categoryType": "income", "name": "Bağış" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let income_cat = serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/financial-categories",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "categoryType": "expense", "name": "Fatura" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let expense_cat = serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/incomes",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({
            "financialAccountId": account.to_string(),
            "categoryId": income_cat,
            "amount": "250.00",
            "description": "bağış",
            "idempotencyKey": "parity-income-1",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let entry: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(entry["amount"], "250.00");
    assert_eq!(entry["movementStatus"], "active");
    // Idempotent replay → 200, same entry, no second movement.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/incomes",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({
            "financialAccountId": account.to_string(),
            "categoryId": income_cat,
            "amount": "250.00",
            "description": "bağış",
            "idempotencyKey": "parity-income-1",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(count(&test.pool, "income_entries").await, 1);
    assert_eq!(count(&test.pool, "account_movements").await, 1);

    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/expenses",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({
            "financialAccountId": account.to_string(),
            "categoryId": expense_cat,
            "amount": "40.00",
            "description": "elektrik",
            "idempotencyKey": "parity-expense-1",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status, body) = request_json(
        &test,
        Method::GET,
        "/api/income-expense/summary",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let summary: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(summary["incomeTotal"], "250.00");
    assert_eq!(summary["expenseTotal"], "40.00");
    assert_eq!(summary["net"], "210.00");
    assert_eq!(summary["currency"], "TRY");

    test.cleanup().await;
}

#[tokio::test]
async fn never_bootstrapped_lane_reads_but_cannot_write_unowned() {
    let Some(test) = setup().await else { return };
    // No bootstrap at all — the legacy global lane still opens for
    // READS. C5-3 closed the write side: cooperative_id is NOT NULL on
    // all business tables, so an ownerless write cannot commit.
    let user = create_user(&test.pool, "eski_serit").await;
    grant_role(&test.pool, user, "legacy-op", None, OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "eski_serit").await;
    let (status, body) = request_json(
        &test,
        Method::GET,
        "/api/periods",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = request_json(
        &test,
        Method::GET,
        "/api/income-expense/summary",
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
        "/api/periods",
        &cookie,
        Some(&csrf),
        None,
        Some(period_body()),
    )
    .await;
    assert!(
        !status.is_success(),
        "unowned business write must fail, got {status}: {body}"
    );
    let periods: i64 = sqlx::query_scalar("SELECT count(*) FROM periods")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(periods, 0, "no unowned period may be written");

    test.cleanup().await;
}
