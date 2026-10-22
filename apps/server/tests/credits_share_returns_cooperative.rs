//! M1-P1-013 (P1-c3c3) — cooperative-scoped credits + share returns
//! authorization tests.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same harness as tests/payments_accounts_cooperative.rs). These tests
//! run the REAL router and prove that the migrated credits and
//! share_returns endpoints resolve authorization only within the
//! verified bootstrap scope:
//!   - headerless requests pin bootstrap; a foreign header never
//!     re-scopes;
//!   - malformed/unknown/non-bootstrap `x-cooperative-id` rejects;
//!   - suspended/ended bootstrap membership is denied on reads and
//!     mutations alike — denied mutations write zero rows;
//!   - permissions granted in Cooperative B never authorize bootstrap
//!     (A) credit or share-return operations;
//!   - NULL-attributed legacy roles keep working inside bootstrap;
//!   - mutations keep both CSRF layers and audit events carry
//!     cooperative attribution;
//!   - the full financial paths (payment remainder → credit →
//!     automatic + manual application → reversals; share → return →
//!     entitlements → settlement movement → reversal) produce identical
//!     results under scoped auth, with every state transition exercised
//!     only where the state machine permits it;
//!   - the never-bootstrapped lane keeps global semantics untouched.
//!
//! Financial logic parity itself is proven by the unchanged
//! `credits.rs` and `share_returns.rs` regression suites; the parity
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

const TEST_PASSWORD: &str = "p1c3c3-test-parola-123";
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
    users::create_user(pool, username, "P1c3c3 Test", &hash, TEST_PASSWORD)
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
        .header("user-agent", "Mozilla/5.0 P1c3c3")
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

/// Every permission the credit/return surfaces can check — plus the
/// already-migrated parties/periods/payments/accounts/income_expense
/// permissions needed by the scoped parity flows (fixtures go through
/// HTTP where a migrated router exists; shares go through SQL because
/// the shares router is still on global auth by design).
const OPERATOR_PERMS: &[&str] = &[
    "credits.read",
    "credits.manage",
    "share_returns.read",
    "share_returns.manage",
    "payments.read",
    "payments.manage",
    "financial_accounts.read",
    "financial_accounts.manage",
    "income_expense.read",
    "income_expense.manage",
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

fn assign_credit_body(shareholder: Uuid, amount: &str, key: &str) -> Value {
    json!({
        "shareholderId": shareholder.to_string(),
        "amount": amount,
        "idempotencyKey": key,
    })
}

fn initiate_body(share: Uuid, date: &str, key: &str) -> Value {
    json!({
        "shareId": share.to_string(),
        "effectiveReturnDate": date,
        "reason": "üyelikten ayrılma",
        "idempotencyKey": key,
    })
}

fn settle_body(account: Uuid, amount: &str, key: &str) -> Value {
    json!({
        "financialAccountId": account.to_string(),
        "amount": amount,
        "idempotencyKey": key,
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

/// One draft period + generated assessments; returns the generated
/// assessment ids (per_shareholder rule → one per shareholder).
async fn make_assessments(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    name: &str,
    amount: &str,
) -> Vec<Uuid> {
    let (status, body) = request_json(
        test,
        Method::POST,
        "/api/periods",
        cookie,
        Some(csrf),
        None,
        Some(json!({
            "name": name,
            "collectionStartDate": "2026-10-01",
            "dueDate": "2026-10-15",
            "ruleType": "per_shareholder",
            "baseAmount": amount,
            "assessmentEffectiveDate": "2026-10-01",
        })),
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
    serde_json::from_str::<Value>(&body).unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .collect()
}

/// Posted payment with a fully unallocated remainder (no allocations)
/// — the credit source.
async fn make_payment(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    account: Uuid,
    amount: &str,
    key: &str,
) -> Uuid {
    let (status, body) = request_json(
        test,
        Method::POST,
        "/api/payments",
        cookie,
        Some(csrf),
        None,
        Some(payment_body(account, amount, key, vec![])),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_str::<Value>(&body).unwrap()["payment"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

/// Return-eligible share fixture written directly to the database —
/// the shares router is still on global authorization by design, so
/// fixtures replicate exactly what shares::repo::create writes:
/// share + initial_acquisition event + open ownership interval
/// starting 2025-01-01 (before the 2026-01-01 effective return date).
///
/// C3: rows are bound to the CURRENT verified scope — the bootstrap
/// cooperative when bootstrapped, NULL in the never-bootstrapped lane
/// (scoped queries never see NULL rows, so NULL fixtures would be
/// indistinguishable from absent ones).
async fn make_share(pool: &PgPool, actor: Uuid, shareholder: Uuid) -> Uuid {
    let coop: Option<Uuid> =
        sqlx::query_scalar("SELECT bootstrap_cooperative_id FROM installation_state")
            .fetch_one(pool)
            .await
            .expect("installation scope");
    let share_id: Uuid = sqlx::query_scalar(
        "INSERT INTO shares (created_by, cooperative_id, share_number) \
         VALUES ($1, $2, allocate_document_number($2,'shares.share_number')) RETURNING id",
    )
    .bind(actor)
    .bind(coop)
    .fetch_one(pool)
    .await
    .expect("share");
    let event_id: Uuid = sqlx::query_scalar(
        "INSERT INTO share_events \
             (share_id, event_type, occurred_at, to_shareholder_id, acquisition_type, \
              amount, currency, reason, actor_user_id, cooperative_id) \
         VALUES ($1, 'initial_acquisition', $2, $3, 'founder', $4, 'TRY', 'kurucu', $5, $6) \
         RETURNING id",
    )
    .bind(share_id)
    .bind(
        OffsetDateTime::parse(
            "2025-01-01T00:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap(),
    )
    .bind(shareholder)
    .bind(rust_decimal::Decimal::new(100, 2))
    .bind(actor)
    .bind(coop)
    .fetch_one(pool)
    .await
    .expect("share event");
    sqlx::query(
        "INSERT INTO share_ownerships \
             (share_id, shareholder_id, started_at, acquisition_type, source_event_id, created_by, \
              cooperative_id) \
         VALUES ($1, $2, $3, 'founder', $4, $5, $6)",
    )
    .bind(share_id)
    .bind(shareholder)
    .bind(
        OffsetDateTime::parse(
            "2025-01-01T00:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap(),
    )
    .bind(event_id)
    .bind(actor)
    .bind(coop)
    .execute(pool)
    .await
    .expect("ownership");
    share_id
}

/// Initiate + finalize a return with the given entitlement specs;
/// returns (return_id, entitlements json array).
async fn finalized_return(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    share: Uuid,
    key: &str,
    specs: Value,
) -> (Uuid, Value) {
    let (status, body) = request_json(
        test,
        Method::POST,
        "/api/share-returns",
        cookie,
        Some(csrf),
        None,
        Some(initiate_body(share, "2026-01-01", key)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "initiate: {body}");
    let detail = serde_json::from_str::<Value>(&body).unwrap();
    let return_id: Uuid = detail["id"].as_str().unwrap().parse().unwrap();
    let (status, body) = request_json(
        test,
        Method::POST,
        &format!("/api/share-returns/{return_id}/finalize"),
        cookie,
        Some(csrf),
        None,
        Some(json!({
            "entitlements": specs,
            "expectedUpdatedAt": detail["updatedAt"],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "finalize: {body}");
    (
        return_id,
        serde_json::from_str::<Value>(&body).unwrap()["entitlements"].clone(),
    )
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
async fn bootstrap_operator_reaches_credits_and_returns() {
    let Some(test) = setup().await else { return };
    let (user, coop) = bootstrap_operator(&test.pool, "operator").await;
    let (cookie, csrf) = login(&test.app, test.peer, "operator").await;

    let account = make_account(&test, &cookie, &csrf, "Kasa").await;
    let shareholder = make_shareholder(&test, &cookie, &csrf, "İade", 1).await;
    let payment = make_payment(&test, &cookie, &csrf, account, "100.00", "p-op-1").await;
    let assessments = make_assessments(&test, &cookie, &csrf, "D1", "10.00").await;

    // Headerless bootstrap reads on both routers.
    for path in [
        "/api/share-returns",
        "/api/share-return-entitlements",
        &format!("/api/shareholders/{shareholder}/credits"),
        &format!("/api/payments/{payment}/credits"),
        &format!("/api/assessments/{}/credit-applications", assessments[0]),
    ] {
        let (status, body) =
            request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }

    // One mutation on each router under scoped auth.
    let share = make_share(&test.pool, user, shareholder).await;
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/share-returns",
        &cookie,
        Some(&csrf),
        None,
        Some(initiate_body(share, "2026-01-01", "r-op-1")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/payments/{payment}/credits"),
        &cookie,
        Some(&csrf),
        None,
        Some(assign_credit_body(shareholder, "50.00", "c-op-1")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Explicit bootstrap header pins the same scope — allowed.
    let coop_str = coop.to_string();
    for path in ["/api/share-returns", "/api/share-return-entitlements"] {
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
    let (user, _) = bootstrap_operator(&test.pool, "hdr_op").await;
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;
    let (cookie, csrf) = login(&test.app, test.peer, "hdr_op").await;
    let shareholder = make_shareholder(&test, &cookie, &csrf, "Borçlu", 1).await;
    let share = make_share(&test.pool, user, shareholder).await;
    let b_str = coop_b.to_string();
    let unknown = Uuid::new_v4().to_string();

    for path in [
        "/api/share-returns",
        "/api/share-return-entitlements",
        &format!("/api/shareholders/{shareholder}/credits"),
    ] {
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
        "/api/share-returns",
        &cookie,
        Some(&csrf),
        Some(&b_str),
        Some(initiate_body(share, "2026-01-01", "r-denied")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(count(&test.pool, "share_returns").await, 0);
    assert_eq!(count(&test.pool, "shareholder_credits").await, 0);

    test.cleanup().await;
}

#[tokio::test]
async fn suspended_and_ended_memberships_are_denied() {
    let Some(test) = setup().await else { return };
    let (user, coop) = bootstrap_operator(&test.pool, "askida").await;
    let (cookie, csrf) = login(&test.app, test.peer, "askida").await;
    let shareholder = make_shareholder(&test, &cookie, &csrf, "Askı", 1).await;
    let share = make_share(&test.pool, user, shareholder).await;

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
            "/api/share-returns",
            "/api/share-return-entitlements",
            &format!("/api/shareholders/{shareholder}/credits"),
        ] {
            let (code, _) = request_json(&test, Method::GET, path, &cookie, None, None, None).await;
            assert_eq!(code, StatusCode::FORBIDDEN, "{status} {path}");
        }
        // Mutations denied — authorization precedes any write.
        let (code, _) = request_json(
            &test,
            Method::POST,
            "/api/share-returns",
            &cookie,
            Some(&csrf),
            None,
            Some(initiate_body(share, "2026-01-01", "r-denied")),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "{status} initiate");
        let (code, _) = request_json(
            &test,
            Method::POST,
            &format!("/api/payments/{}/credits", Uuid::new_v4()),
            &cookie,
            Some(&csrf),
            None,
            Some(assign_credit_body(shareholder, "10.00", "c-denied")),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "{status} assign");
    }
    // No rows written through the denied mutations.
    assert_eq!(count(&test.pool, "share_returns").await, 0);
    assert_eq!(count(&test.pool, "share_return_entitlements").await, 0);
    assert_eq!(count(&test.pool, "share_return_settlements").await, 0);
    assert_eq!(count(&test.pool, "shareholder_credits").await, 0);
    assert_eq!(count(&test.pool, "credit_applications").await, 0);
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
    for path in ["/api/share-returns", "/api/share-return-entitlements"] {
        let (status, _) = request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    let (status, _) = request_json(
        &test,
        Method::POST,
        "/api/share-returns",
        &cookie,
        Some(&csrf),
        None,
        Some(initiate_body(Uuid::new_v4(), "2026-01-01", "r-fk")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(count(&test.pool, "share_returns").await, 0);

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
        &["credits.read", "share_returns.read"],
    )
    .await;
    grant_role(&test.pool, dual, "b-admin2", Some(coop_b), OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "cift").await;
    for path in ["/api/share-returns", "/api/share-return-entitlements"] {
        let (status, body) =
            request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }
    // credits.manage + share_returns.manage exist only in B.
    for (path, body) in [
        (
            "/api/share-returns",
            initiate_body(Uuid::new_v4(), "2026-01-01", "r-fk2"),
        ),
        (
            "/api/assessments/00000000-0000-0000-0000-000000000000/credit-applications",
            json!({ "amount": "1.00", "idempotencyKey": "a-fk" }),
        ),
    ] {
        let (status, _) = request_json(
            &test,
            Method::POST,
            path,
            &cookie,
            Some(&csrf),
            None,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    assert_eq!(count(&test.pool, "share_returns").await, 0);
    assert_eq!(count(&test.pool, "credit_applications").await, 0);

    test.cleanup().await;
}

#[tokio::test]
async fn null_attributed_legacy_roles_keep_working() {
    let Some(test) = setup().await else { return };
    let legacy = create_user(&test.pool, "eski_yetkili").await;
    let coop = bootstrap(&test.pool).await;
    grant_role(&test.pool, legacy, "legacy-op", None, OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "eski_yetkili").await;

    for path in ["/api/share-returns", "/api/share-return-entitlements"] {
        let (status, body) =
            request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }
    let shareholder = make_shareholder(&test, &cookie, &csrf, "Miras", 1).await;
    let share = make_share(&test.pool, legacy, shareholder).await;
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/share-returns",
        &cookie,
        Some(&csrf),
        None,
        Some(initiate_body(share, "2026-01-01", "r-legacy")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // Attribution is still the verified bootstrap scope, not NULL.
    let (scoped,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM security_events \
         WHERE cooperative_id = $1 AND event_type = 'share_return_requested'",
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
    let (user, _) = bootstrap_operator(&test.pool, "yetkili").await;
    let (cookie, csrf) = login(&test.app, test.peer, "yetkili").await;
    let shareholder = make_shareholder(&test, &cookie, &csrf, "Hedef", 1).await;
    let share = make_share(&test.pool, user, shareholder).await;
    let (_, entitlements) = finalized_return(
        &test,
        &cookie,
        &csrf,
        share,
        "r-fixture",
        json!([{"entitlementType": "principal", "amount": "500.00"}]),
    )
    .await;
    let entitlement = entitlements[0]["id"].as_str().unwrap().to_string();
    let account = make_account(&test, &cookie, &csrf, "Kasa").await;
    let payment = make_payment(&test, &cookie, &csrf, account, "100.00", "p-fixture").await;
    let assessments = make_assessments(&test, &cookie, &csrf, "D2", "20.00").await;
    let return_id: String = {
        let (status, body) = request_json(
            &test,
            Method::GET,
            "/api/share-returns",
            &cookie,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        serde_json::from_str::<Value>(&body).unwrap()["items"][0]["id"]
            .as_str()
            .unwrap()
            .to_string()
    };

    // A bootstrap member with NO business role.
    let plain = create_user(&test.pool, "siradan").await;
    repo::add_member(&test.pool, "Ana Kooperatif", plain, None)
        .await
        .expect("member");
    let (pcookie, pcsrf) = login(&test.app, test.peer, "siradan").await;

    // Every read surface denies.
    for path in [
        "/api/share-returns",
        &format!("/api/share-returns/{return_id}"),
        "/api/share-return-entitlements",
        &format!("/api/shareholders/{shareholder}/credits"),
        &format!("/api/payments/{payment}/credits"),
        &format!("/api/assessments/{}/credit-applications", assessments[0]),
    ] {
        let (status, _) = request_json(&test, Method::GET, path, &pcookie, None, None, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    // Every mutation surface denies — nothing is written.
    for (path, body) in [
        (
            "/api/share-returns",
            initiate_body(share, "2026-01-01", "r-deny"),
        ),
        (
            "/api/share-returns/{id}/finalize",
            json!({ "entitlements": [], "expectedUpdatedAt": "2026-01-01T00:00:00Z" }),
        ),
        (
            "/api/share-returns/{id}/cancel",
            json!({ "reason": "vazgeç" }),
        ),
        (
            "/api/share-returns/{id}/entitlements",
            json!({ "entitlementType": "profit", "amount": "1.00" }),
        ),
        (
            "/api/share-return-entitlements/{ent}/determine",
            json!({ "amount": "1.00", "expectedUpdatedAt": "2026-01-01T00:00:00Z" }),
        ),
        (
            "/api/share-return-entitlements/{ent}/cancel",
            json!({ "reason": "hata" }),
        ),
        (
            "/api/share-return-entitlements/{ent}/settlements",
            settle_body(account, "10.00", "s-deny"),
        ),
        (
            "/api/share-return-settlements/{sid}/reverse",
            json!({ "reason": "geri" }),
        ),
        (
            "/api/payments/{pay}/credits",
            assign_credit_body(shareholder, "10.00", "c-deny"),
        ),
        (
            "/api/assessments/{asmt}/credit-applications",
            json!({ "amount": "5.00", "idempotencyKey": "a-deny" }),
        ),
        (
            "/api/credits/{cid}/reverse",
            json!({ "reversalReason": "geri" }),
        ),
        (
            "/api/credit-applications/{app}/reverse",
            json!({ "reversalReason": "geri" }),
        ),
    ] {
        let path = path
            .replace("{id}", &return_id)
            .replace("{ent}", &entitlement)
            .replace("{sid}", &Uuid::new_v4().to_string())
            .replace("{pay}", &payment.to_string())
            .replace("{asmt}", &assessments[0].to_string())
            .replace("{cid}", &Uuid::new_v4().to_string())
            .replace("{app}", &Uuid::new_v4().to_string());
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
    assert_eq!(count(&test.pool, "share_returns").await, 1);
    assert_eq!(count(&test.pool, "share_return_entitlements").await, 1);
    assert_eq!(count(&test.pool, "share_return_settlements").await, 0);
    assert_eq!(count(&test.pool, "shareholder_credits").await, 0);
    assert_eq!(count(&test.pool, "credit_applications").await, 0);
    // The payment's inflow is the only movement that exists.
    assert_eq!(count(&test.pool, "account_movements").await, 1);

    test.cleanup().await;
}

#[tokio::test]
async fn mutations_still_require_both_csrf_layers() {
    let Some(test) = setup().await else { return };
    let (user, _) = bootstrap_operator(&test.pool, "csrf_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "csrf_op").await;
    let shareholder = make_shareholder(&test, &cookie, &csrf, "Kalkan", 1).await;
    let share = make_share(&test.pool, user, shareholder).await;
    let account = make_account(&test, &cookie, &csrf, "Kasa").await;
    let payment = make_payment(&test, &cookie, &csrf, account, "10.00", "p-csrf").await;

    // No synchronizer token → CSRF layer rejects before authorization.
    for (path, body) in [
        (
            "/api/share-returns".to_string(),
            initiate_body(share, "2026-01-01", "r-csrf"),
        ),
        (
            format!("/api/payments/{payment}/credits"),
            assign_credit_body(shareholder, "5.00", "c-csrf"),
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
        "/api/share-returns",
        &cookie,
        Some("bogus-token"),
        None,
        Some(initiate_body(share, "2026-01-01", "r-csrf2")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Correct token → succeeds (proves the prologue ordering).
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/share-returns",
        &cookie,
        Some(&csrf),
        None,
        Some(initiate_body(share, "2026-01-01", "r-csrf-ok")),
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

    let account = make_account(&test, &cookie, &csrf, "Kasa").await;
    let shareholder = make_shareholder(&test, &cookie, &csrf, "Denet", 1).await;
    let payment = make_payment(&test, &cookie, &csrf, account, "80.00", "p-audit").await;
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/payments/{payment}/credits"),
        &cookie,
        Some(&csrf),
        None,
        Some(assign_credit_body(shareholder, "40.00", "c-audit")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let share = make_share(&test.pool, user, shareholder).await;
    let (_, entitlements) = finalized_return(
        &test,
        &cookie,
        &csrf,
        share,
        "r-audit",
        json!([{"entitlementType": "principal", "amount": "30.00"}]),
    )
    .await;
    let entitlement = entitlements[0]["id"].as_str().unwrap().to_string();
    // Fund the account via a payment inflow, then settle.
    make_payment(&test, &cookie, &csrf, account, "100.00", "p-audit-fund").await;
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/share-return-entitlements/{entitlement}/settlements"),
        &cookie,
        Some(&csrf),
        None,
        Some(settle_body(account, "30.00", "s-audit")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Every coop-bound event carries the verified bootstrap id — never
    // a client-supplied value.
    let (scoped,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM security_events \
         WHERE cooperative_id = $1 \
           AND event_type IN ('shareholder_credit_assigned','share_return_requested',\
                              'share_return_finalized','share_return_settlement_posted')",
    )
    .bind(coop)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert!(scoped >= 4, "expected coop-attributed events, got {scoped}");
    let (unscoped,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM security_events \
         WHERE cooperative_id IS NULL \
           AND event_type IN ('shareholder_credit_assigned','share_return_requested',\
                              'share_return_finalized','share_return_settlement_posted')",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(unscoped, 0, "business audit events must be attributed");

    test.cleanup().await;
}

#[tokio::test]
async fn scoped_credit_flow_parity() {
    let Some(test) = setup().await else { return };
    bootstrap_operator(&test.pool, "parity_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "parity_op").await;

    let account = make_account(&test, &cookie, &csrf, "Kasa").await;
    let shareholder = make_shareholder(&test, &cookie, &csrf, "Borçlu", 1).await;
    // Two open assessments BEFORE the credit — the assignment engine
    // auto-offsets all open debt in the same tx (and a later
    // generate-assessments would auto-apply again, so manual apply is
    // exercised by reopening debt through an application reversal).
    let _a1 = make_assessments(&test, &cookie, &csrf, "P1", "50.00").await;
    let second = make_assessments(&test, &cookie, &csrf, "P2", "40.00").await;
    let assessment2 = second[0];
    // Payment posts one inflow; 100 remainder stays unallocated.
    let payment = make_payment(&test, &cookie, &csrf, account, "100.00", "p-c1").await;
    assert_eq!(count(&test.pool, "account_movements").await, 1);

    // Assign 80 of the remainder as the shareholder's credit — the
    // engine auto-offsets 50 + 30 across the two open assessments.
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/payments/{payment}/credits"),
        &cookie,
        Some(&csrf),
        None,
        Some(assign_credit_body(shareholder, "80.00", "c-1")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let assigned = serde_json::from_str::<Value>(&body).unwrap();
    assert_eq!(assigned["appliedAmount"], "80.00");
    assert_eq!(assigned["applicationCount"], 2);
    let credit_id = assigned["creditId"].as_str().unwrap();
    // Ledger is derived: 80 originated, 80 applied, 0 available.
    assert_eq!(assigned["ledger"]["summary"]["available"], "0.00");
    // Credit bookkeeping never creates cash — still only the payment
    // inflow exists.
    assert_eq!(count(&test.pool, "account_movements").await, 1);

    // Idempotent replay returns the same credit, writes nothing.
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/payments/{payment}/credits"),
        &cookie,
        Some(&csrf),
        None,
        Some(assign_credit_body(shareholder, "80.00", "c-1")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["replayed"],
        true
    );
    assert_eq!(count(&test.pool, "shareholder_credits").await, 1);
    assert_eq!(count(&test.pool, "credit_applications").await, 2);
    // Conflicting fingerprint under the same key → 409.
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/payments/{payment}/credits"),
        &cookie,
        Some(&csrf),
        None,
        Some(assign_credit_body(shareholder, "60.00", "c-1")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // No credit left → any further application is InsufficientCredit.
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/assessments/{assessment2}/credit-applications"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "amount": "1.00", "idempotencyKey": "a-x" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Consumed credit blocks reversal while it funds active
    // applications → 409.
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/credits/{credit_id}/reverse"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reversalReason": "yanlış atama" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Payment reversal is also blocked while a consumed credit exists
    // — cross-module coordination through the migrated layer.
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/payments/{payment}/reverse"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reason": "geri al" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Reverse the auto-application on assessment2: its debt reopens
    // and the credit's derived availability returns — still no cash
    // movement. The auto-offset split across same-due-date
    // assessments is intentionally not asserted per-assessment (the
    // engine orders by due date then row id); what matters is the
    // recorded amount, which the assertions below reuse.
    let applications: Vec<(Uuid, Uuid, rust_decimal::Decimal)> = sqlx::query_as(
        "SELECT id, assessment_id, amount FROM credit_applications \
         WHERE credit_id = $1 AND assessment_id = $2 AND status = 'active'",
    )
    .bind(Uuid::parse_str(credit_id).unwrap())
    .bind(assessment2)
    .fetch_all(&test.pool)
    .await
    .unwrap();
    assert_eq!(applications.len(), 1);
    let auto_app = applications[0].0;
    let auto_amount = applications[0].2;
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/credit-applications/{auto_app}/reverse"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reversalReason": "mahsup iade" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(count(&test.pool, "account_movements").await, 1);

    // Manual application succeeds against the reopened debt — the
    // same FIFO engine as the automatic path, but a manual command
    // applies EXACTLY the requested amount (owner-approved rule): we
    // request the reopened amount and expect exactly that back.
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/assessments/{assessment2}/credit-applications"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "amount": auto_amount.to_string(), "idempotencyKey": "a-1" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let applied = serde_json::from_str::<Value>(&body).unwrap();
    assert_eq!(
        applied["appliedAmount"]
            .as_str()
            .unwrap()
            .parse::<rust_decimal::Decimal>(),
        Ok(auto_amount)
    );
    assert_eq!(
        applied["assessmentRemaining"]
            .as_str()
            .unwrap()
            .parse::<rust_decimal::Decimal>(),
        Ok(rust_decimal::Decimal::new(4000, 2) - auto_amount)
    );
    assert_eq!(count(&test.pool, "account_movements").await, 1);

    // Reverse every remaining active application — the Consumed guard
    // then lifts.
    let remaining: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM credit_applications WHERE credit_id = $1 AND status = 'active'",
    )
    .bind(Uuid::parse_str(credit_id).unwrap())
    .fetch_all(&test.pool)
    .await
    .unwrap();
    assert_eq!(remaining.len(), 2);
    for (app_id,) in &remaining {
        let (status, body) = request_json(
            &test,
            Method::POST,
            &format!("/api/credit-applications/{app_id}/reverse"),
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "reversalReason": "mahsup iade" })),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    }
    // Application-reversal replay is a no-op success, not a second
    // mutation.
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/credit-applications/{}/reverse", remaining[0].0),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reversalReason": "tekrar" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Now the payment reversal unblocks — payment, allocations and the
    // unconsumed credit all become historical in one tx.
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/payments/{payment}/reverse"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reason": "geri al" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (credit_status,): (String,) =
        sqlx::query_as("SELECT status FROM shareholder_credits WHERE id = $1")
            .bind(Uuid::parse_str(credit_id).unwrap())
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert_eq!(credit_status, "reversed");

    // Standalone credit reversal (the never-consumed path): a second
    // shareholder carries no open debt, so its assigned credit stays
    // fully available and reverses directly → 204.
    let creditor = make_shareholder(&test, &cookie, &csrf, "Alacaklı", 2).await;
    let payment2 = make_payment(&test, &cookie, &csrf, account, "60.00", "p-c2").await;
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/payments/{payment2}/credits"),
        &cookie,
        Some(&csrf),
        None,
        Some(assign_credit_body(creditor, "25.00", "c-2")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let credit2 = serde_json::from_str::<Value>(&body).unwrap()["creditId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["applicationCount"],
        0
    );
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/credits/{credit2}/reverse"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reversalReason": "yanlış atama" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    // Inflow movements only: two payments, nothing else — credit
    // bookkeeping never moved cash.
    assert_eq!(count(&test.pool, "account_movements").await, 2);

    test.cleanup().await;
}

#[tokio::test]
async fn scoped_share_return_flow_parity() {
    let Some(test) = setup().await else { return };
    let (user, _) = bootstrap_operator(&test.pool, "ret_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "ret_op").await;

    let account = make_account(&test, &cookie, &csrf, "Kasa").await;
    let shareholder = make_shareholder(&test, &cookie, &csrf, "İade", 1).await;
    let share = make_share(&test.pool, user, shareholder).await;

    // initiate → finalize with a determined principal and an
    // undetermined profit right.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/share-returns",
        &cookie,
        Some(&csrf),
        None,
        Some(initiate_body(share, "2026-01-01", "r-1")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let detail = serde_json::from_str::<Value>(&body).unwrap();
    let return_id = detail["id"].as_str().unwrap().to_string();
    // Initiate replay → 200, same return.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/share-returns",
        &cookie,
        Some(&csrf),
        None,
        Some(initiate_body(share, "2026-01-01", "r-1")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(count(&test.pool, "share_returns").await, 1);

    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/share-returns/{return_id}/finalize"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({
            "entitlements": [
                {"entitlementType": "principal", "amount": "1000.00"},
                {"entitlementType": "profit"}
            ],
            "expectedUpdatedAt": detail["updatedAt"],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "finalize: {body}");
    let fin = serde_json::from_str::<Value>(&body).unwrap();
    // Finalization closed the share and ended the ownership interval.
    let (share_status,): (String,) = sqlx::query_as("SELECT status FROM shares WHERE id = $1")
        .bind(share)
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(share_status, "closed");
    let (ended,): (bool,) =
        sqlx::query_as("SELECT ended_at IS NOT NULL FROM share_ownerships WHERE share_id = $1")
            .bind(share)
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert!(ended, "ownership ended at the effective date");

    // The state machine forbids cancelling a finalized return.
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/share-returns/{return_id}/cancel"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reason": "vazgeç" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "cancel after finalize");

    let principal = fin["entitlements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["entitlementType"] == "principal")
        .unwrap();
    let profit = fin["entitlements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["entitlementType"] == "profit")
        .unwrap();
    let principal_id = principal["id"].as_str().unwrap();
    let profit_id = profit["id"].as_str().unwrap().to_string();

    // An undetermined (NULL amount) right is not settleable → 400.
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/share-return-entitlements/{profit_id}/settlements"),
        &cookie,
        Some(&csrf),
        None,
        Some(settle_body(account, "10.00", "s-null")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "undetermined settle");

    // Determine the profit right (expectedUpdatedAt optimistic check).
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/share-return-entitlements/{profit_id}/determine"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({
            "amount": "250.00",
            "expectedUpdatedAt": profit["updatedAt"],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "determine: {body}");

    // Fund the account to 300 only — enough for a 250 settlement.
    make_payment(&test, &cookie, &csrf, account, "300.00", "p-fund").await;
    assert_eq!(movements_for(&test, "payment").await, 1);

    // Settlement posts exactly one outflow movement bound 1:1.
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/share-return-entitlements/{profit_id}/settlements"),
        &cookie,
        Some(&csrf),
        None,
        Some(settle_body(account, "250.00", "s-1")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "settle: {body}");
    let settlement = serde_json::from_str::<Value>(&body).unwrap();
    assert_eq!(settlement["status"], "posted");
    assert_eq!(movements_for(&test, "share_return_settlement").await, 1);
    // Idempotent replay → 200, same settlement, no duplicate movement.
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/share-return-entitlements/{profit_id}/settlements"),
        &cookie,
        Some(&csrf),
        None,
        Some(settle_body(account, "250.00", "s-1")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["id"],
        settlement["id"]
    );
    assert_eq!(movements_for(&test, "share_return_settlement").await, 1);
    assert_eq!(count(&test.pool, "share_return_settlements").await, 1);

    // Insufficient funds on the principal entitlement: 400 remaining
    // of 1000 requested against a 50 balance → 409.
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/share-return-entitlements/{principal_id}/settlements"),
        &cookie,
        Some(&csrf),
        None,
        Some(settle_body(account, "400.00", "s-poor")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "insufficient funds");

    // Over-settlement guard: the entitlement caps at its crystallized
    // amount even with funds available.
    make_payment(&test, &cookie, &csrf, account, "1000.00", "p-fund2").await;
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/share-return-entitlements/{principal_id}/settlements"),
        &cookie,
        Some(&csrf),
        None,
        Some(settle_body(account, "1001.00", "s-over")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "over settlement");

    // Settlement reversal flips the movement to reversed and reopens
    // the entitlement — replay is a safe no-op.
    let settlement_id = settlement["id"].as_str().unwrap().to_string();
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/share-return-settlements/{settlement_id}/reverse"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reason": "hatalı ödeme" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["movementStatus"],
        "reversed"
    );
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/share-return-settlements/{settlement_id}/reverse"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reason": "tekrar" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "reversal replay");
    let (posted,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM account_movements \
         WHERE source_type = 'share_return_settlement' AND status = 'posted'",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(posted, 0, "reversed settlement leaves no posted leg");
    // No entitlement cancellation once settlements existed is not
    // exercised here — cancel requires zero posted settlements; with
    // only a reversed one the guard allows it, per repo semantics.

    // Permitted transition: a SECOND initiated-but-not-finalized
    // return cancels cleanly and the share returns to active.
    let share2 = make_share(&test.pool, user, shareholder).await;
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/share-returns",
        &cookie,
        Some(&csrf),
        None,
        Some(initiate_body(share2, "2026-01-01", "r-2")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let return2 = serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/share-returns/{return2}/cancel"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reason": "vazgeç" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (s2_status,): (String,) = sqlx::query_as("SELECT status FROM shares WHERE id = $1")
        .bind(share2)
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(s2_status, "active", "cancelled return restores the share");

    test.cleanup().await;
}

#[tokio::test]
async fn concurrent_idempotent_posts_single_write() {
    let Some(test) = setup().await else { return };
    let (user, _) = bootstrap_operator(&test.pool, "race_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "race_op").await;

    let account = make_account(&test, &cookie, &csrf, "Kasa").await;
    let shareholder = make_shareholder(&test, &cookie, &csrf, "Yarış", 1).await;
    let payment = make_payment(&test, &cookie, &csrf, account, "200.00", "p-race").await;

    // Two concurrent assign_credit calls with the same key → exactly
    // one credit, one assignment audit event.
    let assign_path = format!("/api/payments/{payment}/credits");
    let (a, b) = tokio::join!(
        request_json(
            &test,
            Method::POST,
            &assign_path,
            &cookie,
            Some(&csrf),
            None,
            Some(assign_credit_body(shareholder, "50.00", "c-race")),
        ),
        request_json(
            &test,
            Method::POST,
            &assign_path,
            &cookie,
            Some(&csrf),
            None,
            Some(assign_credit_body(shareholder, "50.00", "c-race")),
        ),
    );
    assert!(
        [a.0, b.0]
            .iter()
            .all(|s| *s == StatusCode::CREATED || *s == StatusCode::OK),
        "concurrent assign: {a:?} {b:?}"
    );
    assert_eq!(count(&test.pool, "shareholder_credits").await, 1);
    let (events,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM security_events \
         WHERE event_type = 'shareholder_credit_assigned'",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(events, 1, "concurrent assign audited exactly once");

    // Two concurrent settlements with the same key → one settlement,
    // one movement. The entitlement is larger than the settlement so
    // the loser still sees a settleable remainder under the row lock
    // and reaches the idempotent ON CONFLICT → replay path (a loser
    // that finds the entitlement already 'settled' is correctly
    // rejected NotSettleable instead — both are one-write).
    let share = make_share(&test.pool, user, shareholder).await;
    let (_, entitlements) = finalized_return(
        &test,
        &cookie,
        &csrf,
        share,
        "r-race",
        json!([{"entitlementType": "principal", "amount": "200.00"}]),
    )
    .await;
    let entitlement = entitlements[0]["id"].as_str().unwrap().to_string();
    let settle_path = format!("/api/share-return-entitlements/{entitlement}/settlements");
    let (a, b) = tokio::join!(
        request_json(
            &test,
            Method::POST,
            &settle_path,
            &cookie,
            Some(&csrf),
            None,
            Some(settle_body(account, "100.00", "s-race")),
        ),
        request_json(
            &test,
            Method::POST,
            &settle_path,
            &cookie,
            Some(&csrf),
            None,
            Some(settle_body(account, "100.00", "s-race")),
        ),
    );
    assert!(
        [a.0, b.0]
            .iter()
            .all(|s| *s == StatusCode::CREATED || *s == StatusCode::OK),
        "concurrent settle: {a:?} {b:?}"
    );
    assert_eq!(count(&test.pool, "share_return_settlements").await, 1);
    assert_eq!(movements_for(&test, "share_return_settlement").await, 1);

    test.cleanup().await;
}

#[tokio::test]
async fn never_bootstrapped_lane_reads_but_cannot_write_unowned() {
    let Some(test) = setup().await else { return };
    // No cooperative exists at all — the legacy global lane still
    // opens for READS. C5-3 closed the write side: cooperative_id is
    // NOT NULL on all business tables, so an ownerless write cannot
    // commit.
    let legacy = create_user(&test.pool, "tek_alan").await;
    grant_role(&test.pool, legacy, "global-op", None, OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "tek_alan").await;

    for path in ["/api/share-returns", "/api/share-return-entitlements"] {
        let (status, body) =
            request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }

    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/share-returns",
        &cookie,
        Some(&csrf),
        None,
        Some(initiate_body(Uuid::new_v4(), "2026-01-01", "r-legacy-lane")),
    )
    .await;
    assert!(
        !status.is_success(),
        "unowned business write must fail, got {status}: {body}"
    );
    let returns: i64 = sqlx::query_scalar("SELECT count(*) FROM share_returns")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(returns, 0, "no unowned share return may be written");

    test.cleanup().await;
}
