//! M1-P1-c3c4 — cooperative-scoped shares + investments authorization
//! tests.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same harness as tests/payments_accounts_cooperative.rs). These tests
//! run the REAL router and prove that the migrated shares and
//! investments endpoints resolve authorization only within the
//! verified bootstrap scope:
//!   - headerless requests pin bootstrap; a foreign header never
//!     re-scopes;
//!   - malformed/unknown/non-bootstrap `x-cooperative-id` rejects;
//!   - suspended/ended bootstrap membership is denied on reads and
//!     mutations alike — denied mutations write zero rows;
//!   - permissions granted in Cooperative B never authorize bootstrap
//!     (A) share or investment operations;
//!   - NULL-attributed legacy roles keep working inside bootstrap;
//!   - mutations keep both CSRF layers and audit events carry
//!     cooperative attribution;
//!   - share ownership transitions (create → transfer → sale →
//!     suspend/reactivate → void) stay byte-identical under scoped
//!     auth and never create account movements;
//!   - investment lifecycle (create → funding outflow → valuation →
//!     income inflow → reversals → disposal legs) preserves movement
//!     cardinality exactly;
//!   - the never-bootstrapped lane keeps global semantics untouched.
//!
//! Financial logic parity itself is proven by the unchanged
//! `shares.rs` and `investments.rs` regression suites; the parity
//! tests here additionally exercise the same paths THROUGH the
//! migrated authorization layer.

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

const TEST_PASSWORD: &str = "p1c3c4-test-parola-123";
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
    users::create_user(pool, username, "P1c3c4 Test", &hash, TEST_PASSWORD)
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
        .header("user-agent", "Mozilla/5.0 P1c3c4")
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

/// Every permission the share/investment surfaces can check — plus
/// the already-migrated permissions the fixtures need (parties for
/// shareholder creation, accounts + income_expense for account
/// funding).
const OPERATOR_PERMS: &[&str] = &[
    "shares.read",
    "shares.manage",
    "investments.read",
    "investments.manage",
    "shareholders.manage",
    "financial_accounts.read",
    "financial_accounts.manage",
    "income_expense.read",
    "income_expense.manage",
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

/// One shareholder through the migrated parties router; `seq` is the
/// unique family sequence number within the test database.
async fn make_shareholder(test: &TestApp, cookie: &str, csrf: &str, first: &str, seq: i64) -> Uuid {
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
    serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

/// Founder share through the migrated shares router; returns the
/// parsed detail (id, shareNumber, status, updatedAt).
async fn make_share(test: &TestApp, cookie: &str, csrf: &str, shareholder: Uuid) -> Value {
    let (status, body) = request_json(
        test,
        Method::POST,
        "/api/shares",
        cookie,
        Some(csrf),
        None,
        Some(json!({
            "shareholderId": shareholder.to_string(),
            "acquisitionType": "founder",
            "reason": "kurucu kayıt",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_str::<Value>(&body).unwrap()
}

/// Cash account through the migrated endpoint.
async fn make_account(test: &TestApp, cookie: &str, csrf: &str, name: &str) -> Uuid {
    let (status, body) = request_json(
        test,
        Method::POST,
        "/api/financial-accounts",
        cookie,
        Some(csrf),
        None,
        Some(json!({ "name": name, "accountType": "cash" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

/// Fund an account with unrestricted cash through the migrated
/// income_expense router.
async fn fund_account(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    account: Uuid,
    amount: &str,
    key: &str,
) {
    let (status, list) = request_json(
        test,
        Method::GET,
        "/api/financial-categories/options?categoryType=income",
        cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let category = serde_json::from_str::<Value>(&list)
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Diğer Gelir")
        .expect("category")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, body) = request_json(
        test,
        Method::POST,
        "/api/incomes",
        cookie,
        Some(csrf),
        None,
        Some(json!({
            "financialAccountId": account.to_string(),
            "categoryId": category,
            "amount": amount,
            "description": "kasa fonlama",
            "idempotencyKey": key,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

/// Investment through the migrated router; returns the detail id.
async fn make_investment(test: &TestApp, cookie: &str, csrf: &str, name: &str, key: &str) -> Uuid {
    let (status, body) = request_json(
        test,
        Method::POST,
        "/api/investments",
        cookie,
        Some(csrf),
        None,
        Some(json!({
            "name": name,
            "investmentType": "real_estate",
            "idempotencyKey": key,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

fn funding_body(account: Uuid, amount: &str, key: &str) -> Value {
    json!({
        "financialAccountId": account.to_string(),
        "amount": amount,
        "occurredAt": "2025-12-31T12:00:00Z",
        "idempotencyKey": key,
    })
}

fn income_body(account: Uuid, amount: &str, key: &str) -> Value {
    json!({
        "financialAccountId": account.to_string(),
        "amount": amount,
        "occurredAt": "2025-12-31T12:00:00Z",
        "description": "kira geliri",
        "idempotencyKey": key,
    })
}

fn valuation_body(amount: &str, key: &str) -> Value {
    json!({
        "valuationDate": "2025-12-31",
        "amount": amount,
        "method": "piyasa",
        "idempotencyKey": key,
    })
}

fn disposal_body(account: Uuid, legs: &[&str], key: &str) -> Value {
    let proceeds: Vec<Value> = legs
        .iter()
        .map(|amount| {
            json!({
                "financialAccountId": account.to_string(),
                "amount": amount,
                "occurredAt": "2025-12-31T12:00:00Z",
            })
        })
        .collect();
    json!({
        "disposedAt": "2025-12-31",
        "proceeds": proceeds,
        "idempotencyKey": key,
    })
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count")
}

async fn movements_for(test: &TestApp, source_type: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM account_movements WHERE source_type = $1")
        .bind(source_type)
        .fetch_one(&test.pool)
        .await
        .expect("movements")
}

#[tokio::test]
async fn bootstrap_operator_reaches_shares_and_investments() {
    let Some(test) = setup().await else { return };
    let (_, coop) = bootstrap_operator(&test.pool, "operator").await;
    let (cookie, csrf) = login(&test.app, test.peer, "operator").await;
    let shareholder = make_shareholder(&test, &cookie, &csrf, "Ortak", 1).await;

    // Headerless bootstrap reads on both routers.
    for path in [
        "/api/shares",
        "/api/investments",
        &format!("/api/shareholders/{shareholder}/shares"),
    ] {
        let (status, body) =
            request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }

    // One mutation on each router under scoped auth.
    let share = make_share(&test, &cookie, &csrf, shareholder).await;
    assert_eq!(share["status"], "active");
    let investment = make_investment(&test, &cookie, &csrf, "Arsa", "i-op-1").await;

    // Detail reads see the created rows.
    for path in [
        format!("/api/shares/{}", share["id"].as_str().unwrap()),
        format!("/api/investments/{investment}"),
    ] {
        let (status, body) =
            request_json(&test, Method::GET, &path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }

    // Explicit bootstrap header pins the same scope — allowed.
    let coop_str = coop.to_string();
    for path in ["/api/shares", "/api/investments"] {
        let (status, body) = request_json(
            &test,
            Method::GET,
            path,
            &cookie,
            None,
            Some(&coop_str),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }

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
    let shareholder = make_shareholder(&test, &cookie, &csrf, "Ortak", 1).await;
    let b_str = coop_b.to_string();
    let unknown = Uuid::new_v4().to_string();

    for path in [
        "/api/shares".to_string(),
        "/api/investments".to_string(),
        format!("/api/shareholders/{shareholder}/shares"),
    ] {
        // Malformed UUID → 400.
        let (status, _) = request_json(
            &test,
            Method::GET,
            &path,
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
            request_json(&test, Method::GET, &path, &cookie, None, Some(&b_str), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        // Unknown-but-well-formed UUID → 403 (existence never leaked).
        let (status, _) = request_json(
            &test,
            Method::GET,
            &path,
            &cookie,
            None,
            Some(&unknown),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    // Mutations obey the same rules — and write nothing.
    for (path, body) in [
        (
            "/api/shares".to_string(),
            json!({ "shareholderId": shareholder.to_string(), "acquisitionType": "founder" }),
        ),
        (
            "/api/investments".to_string(),
            json!({ "name": "X", "investmentType": "business", "idempotencyKey": "i-denied" }),
        ),
    ] {
        let (status, _) = request_json(
            &test,
            Method::POST,
            &path,
            &cookie,
            Some(&csrf),
            Some(&b_str),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    assert_eq!(count(&test.pool, "shares").await, 0);
    assert_eq!(count(&test.pool, "investments").await, 0);

    test.cleanup().await;
}

#[tokio::test]
async fn suspended_and_ended_memberships_are_denied() {
    let Some(test) = setup().await else { return };
    let (user, coop) = bootstrap_operator(&test.pool, "askida").await;
    let (cookie, csrf) = login(&test.app, test.peer, "askida").await;
    let shareholder = make_shareholder(&test, &cookie, &csrf, "Askı", 1).await;
    let share = make_share(&test, &cookie, &csrf, shareholder).await;
    let share_id = share["id"].as_str().unwrap().to_string();
    let investment = make_investment(&test, &cookie, &csrf, "Arsa", "i-sus-1").await;

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
        for path in [
            "/api/shares".to_string(),
            "/api/investments".to_string(),
            format!("/api/shares/{share_id}"),
            format!("/api/investments/{investment}"),
        ] {
            let (code, _) =
                request_json(&test, Method::GET, &path, &cookie, None, None, None).await;
            assert_eq!(code, StatusCode::FORBIDDEN, "{status} {path}");
        }
        // Mutations denied — authorization precedes any write.
        let (code, _) = request_json(
            &test,
            Method::POST,
            "/api/shares",
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "shareholderId": shareholder.to_string(), "acquisitionType": "founder" })),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "{status} share create");
        let (code, _) = request_json(
            &test,
            Method::POST,
            "/api/investments",
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "name": "Y", "investmentType": "business", "idempotencyKey": "i-deny" })),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "{status} investment create");
    }
    // Fixture rows only — the denied mutations added nothing.
    assert_eq!(count(&test.pool, "shares").await, 1);
    assert_eq!(count(&test.pool, "share_ownerships").await, 1);
    assert_eq!(count(&test.pool, "share_events").await, 1);
    assert_eq!(count(&test.pool, "investments").await, 1);
    assert_eq!(count(&test.pool, "investment_fundings").await, 0);
    assert_eq!(count(&test.pool, "investment_valuations").await, 0);
    assert_eq!(count(&test.pool, "investment_incomes").await, 0);
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
    for path in ["/api/shares", "/api/investments"] {
        let (status, _) = request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    let (status, _) = request_json(
        &test,
        Method::POST,
        "/api/investments",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "name": "X", "investmentType": "business", "idempotencyKey": "i-fk" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(count(&test.pool, "investments").await, 0);

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
        &["shares.read", "investments.read"],
    )
    .await;
    grant_role(&test.pool, dual, "b-admin2", Some(coop_b), OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "cift").await;
    for path in ["/api/shares", "/api/investments"] {
        let (status, body) =
            request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }
    // shares.manage + investments.manage exist only in B.
    for (path, body) in [
        (
            "/api/shares".to_string(),
            json!({ "shareholderId": Uuid::new_v4().to_string(), "acquisitionType": "founder" }),
        ),
        (
            "/api/investments".to_string(),
            json!({ "name": "Y", "investmentType": "business", "idempotencyKey": "i-fk2" }),
        ),
    ] {
        let (status, _) = request_json(
            &test,
            Method::POST,
            &path,
            &cookie,
            Some(&csrf),
            None,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    assert_eq!(count(&test.pool, "shares").await, 0);
    assert_eq!(count(&test.pool, "investments").await, 0);

    test.cleanup().await;
}

#[tokio::test]
async fn null_attributed_legacy_roles_keep_working() {
    let Some(test) = setup().await else { return };
    let legacy = create_user(&test.pool, "eski_yetkili").await;
    let coop = bootstrap(&test.pool).await;
    grant_role(&test.pool, legacy, "legacy-op", None, OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "eski_yetkili").await;

    for path in ["/api/shares", "/api/investments"] {
        let (status, body) =
            request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }
    let shareholder = make_shareholder(&test, &cookie, &csrf, "Miras", 1).await;
    let share = make_share(&test, &cookie, &csrf, shareholder).await;
    assert_eq!(share["status"], "active");
    // Attribution is still the verified bootstrap scope, not NULL.
    let (scoped,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM security_events \
         WHERE cooperative_id = $1 AND event_type = 'share_created'",
    )
    .bind(coop)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(scoped, 1);

    test.cleanup().await;
}

#[tokio::test]
async fn unauthorized_reads_and_mutations_denied_with_zero_writes() {
    let Some(test) = setup().await else { return };
    bootstrap_operator(&test.pool, "yetkili").await;
    let (cookie, csrf) = login(&test.app, test.peer, "yetkili").await;
    let shareholder = make_shareholder(&test, &cookie, &csrf, "Hedef", 1).await;
    let share = make_share(&test, &cookie, &csrf, shareholder).await;
    let share_id = share["id"].as_str().unwrap().to_string();
    let share_updated = share["updatedAt"].as_str().unwrap().to_string();
    let investment = make_investment(&test, &cookie, &csrf, "Arsa", "i-fixture").await;

    // A bootstrap member with NO business role.
    let plain = create_user(&test.pool, "siradan").await;
    repo::add_member(&test.pool, "Ana Kooperatif", plain, None)
        .await
        .expect("member");
    let (pcookie, pcsrf) = login(&test.app, test.peer, "siradan").await;

    // Every read surface denies.
    for path in [
        "/api/shares".to_string(),
        format!("/api/shares/{share_id}"),
        format!("/api/shareholders/{shareholder}/shares"),
        "/api/investments".to_string(),
        format!("/api/investments/{investment}"),
    ] {
        let (status, _) = request_json(&test, Method::GET, &path, &pcookie, None, None, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    // Every mutation surface denies — nothing is written.
    let fresh = Uuid::new_v4().to_string();
    for (path, body) in [
        (
            "/api/shares".to_string(),
            json!({ "shareholderId": shareholder.to_string(), "acquisitionType": "founder" }),
        ),
        (
            format!("/api/shares/{share_id}/transfer"),
            json!({ "toShareholderId": shareholder.to_string(), "expectedUpdatedAt": share_updated }),
        ),
        (
            format!("/api/shares/{share_id}/sale"),
            json!({ "toShareholderId": shareholder.to_string(), "saleAmount": "10.00", "expectedUpdatedAt": share_updated }),
        ),
        (
            format!("/api/shares/{share_id}/status-change"),
            json!({ "to": "suspended", "reason": "askı" }),
        ),
        (
            "/api/investments".to_string(),
            json!({ "name": "X", "investmentType": "business", "idempotencyKey": "i-deny" }),
        ),
        (
            format!("/api/investments/{investment}/cancel"),
            json!({ "reason": "vazgeç" }),
        ),
        (
            format!("/api/investments/{investment}/fundings"),
            funding_body(Uuid::new_v4(), "10.00", "f-deny"),
        ),
        (
            format!("/api/investment-fundings/{fresh}/reverse"),
            json!({ "reason": "geri" }),
        ),
        (
            format!("/api/investments/{investment}/valuations"),
            valuation_body("10.00", "v-deny"),
        ),
        (
            format!("/api/investment-valuations/{fresh}/cancel"),
            json!({ "reason": "geri" }),
        ),
        (
            format!("/api/investments/{investment}/incomes"),
            income_body(Uuid::new_v4(), "10.00", "n-deny"),
        ),
        (
            format!("/api/investment-incomes/{fresh}/reverse"),
            json!({ "reason": "geri" }),
        ),
        (
            format!("/api/investments/{investment}/dispose"),
            disposal_body(Uuid::new_v4(), &["10.00"], "d-deny"),
        ),
    ] {
        let (status, _) = request_json(
            &test,
            Method::POST,
            &path,
            &pcookie,
            Some(&pcsrf),
            None,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    // Fixture rows only — the denied mutations added nothing.
    assert_eq!(count(&test.pool, "shares").await, 1);
    assert_eq!(count(&test.pool, "share_ownerships").await, 1);
    assert_eq!(count(&test.pool, "share_events").await, 1);
    assert_eq!(count(&test.pool, "investments").await, 1);
    assert_eq!(count(&test.pool, "investment_fundings").await, 0);
    assert_eq!(count(&test.pool, "investment_valuations").await, 0);
    assert_eq!(count(&test.pool, "investment_incomes").await, 0);
    assert_eq!(count(&test.pool, "investment_disposals").await, 0);
    assert_eq!(count(&test.pool, "account_movements").await, 0);

    test.cleanup().await;
}

#[tokio::test]
async fn mutations_still_require_both_csrf_layers() {
    let Some(test) = setup().await else { return };
    bootstrap_operator(&test.pool, "csrf_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "csrf_op").await;
    let shareholder = make_shareholder(&test, &cookie, &csrf, "Kalkan", 1).await;

    // No synchronizer token → CSRF layer rejects before authorization.
    for (path, body) in [
        (
            "/api/shares".to_string(),
            json!({ "shareholderId": shareholder.to_string(), "acquisitionType": "founder" }),
        ),
        (
            "/api/investments".to_string(),
            json!({ "name": "X", "investmentType": "business", "idempotencyKey": "i-csrf" }),
        ),
    ] {
        let (status, _) =
            request_json(&test, Method::POST, &path, &cookie, None, None, Some(body)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    // Wrong token → same.
    let (status, _) = request_json(
        &test,
        Method::POST,
        "/api/shares",
        &cookie,
        Some("bogus-token"),
        None,
        Some(json!({ "shareholderId": shareholder.to_string(), "acquisitionType": "founder" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Correct token → succeeds (proves the prologue ordering).
    let share = make_share(&test, &cookie, &csrf, shareholder).await;
    assert_eq!(share["status"], "active");

    test.cleanup().await;
}

#[tokio::test]
async fn mutations_record_cooperative_scoped_audit() {
    let Some(test) = setup().await else { return };
    let (_, coop) = bootstrap_operator(&test.pool, "audit_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "audit_op").await;
    let shareholder = make_shareholder(&test, &cookie, &csrf, "Denetim", 1).await;
    make_share(&test, &cookie, &csrf, shareholder).await;
    let account = make_account(&test, &cookie, &csrf, "Kasa").await;
    fund_account(&test, &cookie, &csrf, account, "100.00", "fa-audit").await;
    let investment = make_investment(&test, &cookie, &csrf, "Arsa", "i-audit").await;
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/investments/{investment}/fundings"),
        &cookie,
        Some(&csrf),
        None,
        Some(funding_body(account, "50.00", "f-audit")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    for event in [
        "share_created",
        "investment_created",
        "investment_funding_posted",
    ] {
        let (scoped,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM security_events \
             WHERE cooperative_id = $1 AND event_type = $2",
        )
        .bind(coop)
        .bind(event)
        .fetch_one(&test.pool)
        .await
        .unwrap();
        assert_eq!(scoped, 1, "{event}");
    }
    // Nothing recorded in the NULL-attributed lane.
    let (null_lane,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM security_events \
         WHERE cooperative_id IS NULL AND event_type IN \
         ('share_created','investment_created','investment_funding_posted')",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(null_lane, 0);

    test.cleanup().await;
}

#[tokio::test]
async fn scoped_share_ownership_flow_parity() {
    let Some(test) = setup().await else { return };
    bootstrap_operator(&test.pool, "parity_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "parity_op").await;
    let s1 = make_shareholder(&test, &cookie, &csrf, "Birinci", 1).await;
    let s2 = make_shareholder(&test, &cookie, &csrf, "İkinci", 2).await;

    // Create: share + open interval + initial event — zero cash.
    let share = make_share(&test, &cookie, &csrf, s1).await;
    let share_id = share["id"].as_str().unwrap().to_string();
    let share_uuid: Uuid = share_id.parse().unwrap();
    assert_eq!(share["status"], "active");
    assert_eq!(share["events"].as_array().unwrap().len(), 1);
    assert_eq!(count(&test.pool, "account_movements").await, 0);
    let mut updated = share["updatedAt"].as_str().unwrap().to_string();

    // Transfer s1 → s2: the interval closes and a new one opens.
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/shares/{share_id}/transfer"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "toShareholderId": s2.to_string(), "expectedUpdatedAt": updated, "reason": "hibe" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (open,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM share_ownerships WHERE share_id = $1 AND ended_at IS NULL",
    )
    .bind(share_uuid)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(open, 1);
    let (owner,): (Uuid,) = sqlx::query_as(
        "SELECT shareholder_id FROM share_ownerships WHERE share_id = $1 AND ended_at IS NULL",
    )
    .bind(share_uuid)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(owner, s2);
    assert_eq!(count(&test.pool, "account_movements").await, 0);

    // Stale expectedUpdatedAt → 409, no second transition.
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/shares/{share_id}/transfer"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "toShareholderId": s1.to_string(), "expectedUpdatedAt": updated })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Sale back s2 → s1 with an agreed amount — still zero cash.
    let (status, body) = request_json(
        &test,
        Method::GET,
        &format!("/api/shares/{share_id}"),
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    updated = serde_json::from_str::<Value>(&body).unwrap()["updatedAt"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/shares/{share_id}/sale"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "toShareholderId": s1.to_string(), "saleAmount": "5000.00", "expectedUpdatedAt": updated, "reason": "devir satış" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (owner,): (Uuid,) = sqlx::query_as(
        "SELECT shareholder_id FROM share_ownerships WHERE share_id = $1 AND ended_at IS NULL",
    )
    .bind(share_uuid)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(owner, s1);
    assert_eq!(count(&test.pool, "account_movements").await, 0);

    // Suspend → reactivate through status-change.
    for target in ["suspended", "active"] {
        let (status, body) = request_json(
            &test,
            Method::POST,
            &format!("/api/shares/{share_id}/status-change"),
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "to": target, "reason": "durum" })),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{target}: {body}");
    }
    // The lifecycle statuses owned by share_returns are NOT reachable
    // through status-change — the state machine rejects them.
    for target in ["return_pending", "closed"] {
        let (status, _) = request_json(
            &test,
            Method::POST,
            &format!("/api/shares/{share_id}/status-change"),
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "to": target, "reason": "yok" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{target}");
    }
    // Void requires a history-free share — this one has events.
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/shares/{share_id}/status-change"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "to": "voided", "reason": "iptal" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // A fresh share CAN be voided — its interval closes too.
    let fresh = make_share(&test, &cookie, &csrf, s2).await;
    let fresh_id = fresh["id"].as_str().unwrap().to_string();
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/shares/{fresh_id}/status-change"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "to": "voided", "reason": "yanlış kayıt" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (open,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM share_ownerships \
         WHERE share_id = $1 AND ended_at IS NULL",
    )
    .bind(fresh_id.parse::<Uuid>().unwrap())
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(open, 0);
    // History preserved: create + transfer + sale + 2 status events.
    let (events,): (i64,) = sqlx::query_as("SELECT count(*) FROM share_events WHERE share_id = $1")
        .bind(share_uuid)
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(events, 5);
    assert_eq!(count(&test.pool, "account_movements").await, 0);

    test.cleanup().await;
}

#[tokio::test]
async fn scoped_investment_financial_flow_parity() {
    let Some(test) = setup().await else { return };
    bootstrap_operator(&test.pool, "inv_parity").await;
    let (cookie, csrf) = login(&test.app, test.peer, "inv_parity").await;
    let account = make_account(&test, &cookie, &csrf, "Kasa").await;
    fund_account(&test, &cookie, &csrf, account, "1000.00", "fa-1").await;
    assert_eq!(movements_for(&test, "income").await, 1);

    let investment = make_investment(&test, &cookie, &csrf, "Arsa", "i-1").await;

    // Funding: exactly ONE outflow movement.
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/investments/{investment}/fundings"),
        &cookie,
        Some(&csrf),
        None,
        Some(funding_body(account, "100.00", "f-1")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(movements_for(&test, "investment_funding").await, 1);
    // Idempotent replay: same key → 200, no second row/movement.
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/investments/{investment}/fundings"),
        &cookie,
        Some(&csrf),
        None,
        Some(funding_body(account, "100.00", "f-1")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(count(&test.pool, "investment_fundings").await, 1);
    assert_eq!(movements_for(&test, "investment_funding").await, 1);

    // Valuation: informational — ZERO money.
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/investments/{investment}/valuations"),
        &cookie,
        Some(&csrf),
        None,
        Some(valuation_body("150.00", "v-1")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(count(&test.pool, "account_movements").await, 2);

    // Income: exactly ONE inflow; never an income_entries row.
    let incomes_before = count(&test.pool, "income_entries").await;
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/investments/{investment}/incomes"),
        &cookie,
        Some(&csrf),
        None,
        Some(income_body(account, "10.00", "n-1")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(movements_for(&test, "investment_income").await, 1);
    assert_eq!(count(&test.pool, "income_entries").await, incomes_before);
    let income_id = serde_json::from_str::<Value>(&body).unwrap()["incomes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Funding reversal reverses the movement, preserves the row.
    let funding_id = serde_json::from_str::<Value>(&body).unwrap()["fundings"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/investment-fundings/{funding_id}/reverse"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reason": "yanlış tutar" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (posted,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM account_movements \
         WHERE source_type = 'investment_funding' AND status = 'posted'",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(posted, 0);
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/investment-incomes/{income_id}/reverse"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reason": "iade" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Disposal: two proceeds legs → exactly TWO inflow movements.
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/investments/{investment}/dispose"),
        &cookie,
        Some(&csrf),
        None,
        Some(disposal_body(account, &["80.00", "20.00"], "d-1")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(movements_for(&test, "investment_disposal").await, 2);
    let detail = serde_json::from_str::<Value>(&body).unwrap();
    assert_eq!(detail["status"], "disposed");

    // Cancel is allowed only on a history-free investment.
    let second = make_investment(&test, &cookie, &csrf, "Ofis", "i-2").await;
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/investments/{second}/cancel"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reason": "vazgeç" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    test.cleanup().await;
}

#[tokio::test]
async fn concurrent_idempotent_posts_single_write() {
    let Some(test) = setup().await else { return };
    bootstrap_operator(&test.pool, "conc_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "conc_op").await;
    let account = make_account(&test, &cookie, &csrf, "Kasa").await;
    fund_account(&test, &cookie, &csrf, account, "1000.00", "fa-c").await;
    let investment = make_investment(&test, &cookie, &csrf, "Arsa", "i-c").await;
    let s1 = make_shareholder(&test, &cookie, &csrf, "Yarış", 1).await;
    let s2 = make_shareholder(&test, &cookie, &csrf, "Rakip", 2).await;
    let share = make_share(&test, &cookie, &csrf, s1).await;
    let share_id = share["id"].as_str().unwrap().to_string();
    let updated = share["updatedAt"].as_str().unwrap().to_string();

    // Concurrent same-key funding posts: one logical write.
    let path = format!("/api/investments/{investment}/fundings");
    let (a, b) = tokio::join!(
        request_json(
            &test,
            Method::POST,
            &path,
            &cookie,
            Some(&csrf),
            None,
            Some(funding_body(account, "40.00", "f-conc")),
        ),
        request_json(
            &test,
            Method::POST,
            &path,
            &cookie,
            Some(&csrf),
            None,
            Some(funding_body(account, "40.00", "f-conc")),
        )
    );
    assert!(
        a.0 == StatusCode::CREATED || a.0 == StatusCode::OK,
        "{}: {}",
        a.0,
        a.1
    );
    assert!(
        b.0 == StatusCode::CREATED || b.0 == StatusCode::OK,
        "{}: {}",
        b.0,
        b.1
    );
    assert_eq!(count(&test.pool, "investment_fundings").await, 1);
    assert_eq!(movements_for(&test, "investment_funding").await, 1);

    // Concurrent same-precondition transfers: exactly ONE transition
    // commits; the loser sees the optimistic-concurrency conflict.
    let path = format!("/api/shares/{share_id}/transfer");
    let (a, b) = tokio::join!(
        request_json(
            &test,
            Method::POST,
            &path,
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "toShareholderId": s2.to_string(), "expectedUpdatedAt": updated })),
        ),
        request_json(
            &test,
            Method::POST,
            &path,
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "toShareholderId": s2.to_string(), "expectedUpdatedAt": updated })),
        )
    );
    let statuses = [a.0, b.0];
    assert!(
        statuses.contains(&StatusCode::NO_CONTENT) && statuses.contains(&StatusCode::CONFLICT),
        "{statuses:?}: {} / {}",
        a.1,
        b.1
    );
    let (open,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM share_ownerships \
         WHERE share_id = $1 AND ended_at IS NULL",
    )
    .bind(share_id.parse::<Uuid>().unwrap())
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(open, 1);
    let (transfers,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM share_events \
         WHERE share_id = $1 AND event_type = 'transfer'",
    )
    .bind(share_id.parse::<Uuid>().unwrap())
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(transfers, 1);

    test.cleanup().await;
}

#[tokio::test]
async fn never_bootstrapped_lane_reads_but_cannot_write_unowned() {
    let Some(test) = setup().await else { return };
    // No bootstrap at all — the global lane still opens for READS.
    // C5-3 closed the write side: cooperative_id is NOT NULL on all
    // business tables, so an ownerless write cannot commit.
    let legacy = create_user(&test.pool, "kuresel").await;
    grant_role(&test.pool, legacy, "global-op", None, OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "kuresel").await;

    for path in ["/api/shares", "/api/investments"] {
        let (status, body) =
            request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }

    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/investments",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({
            "name": "Arsa",
            "investmentType": "real_estate",
            "idempotencyKey": "i-legacy",
        })),
    )
    .await;
    assert!(
        !status.is_success(),
        "unowned business write must fail, got {status}: {body}"
    );
    let investments: i64 = sqlx::query_scalar("SELECT count(*) FROM investments")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(investments, 0, "no unowned investment may be written");

    test.cleanup().await;
}
