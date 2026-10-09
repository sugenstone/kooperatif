//! Database-gated Investment & cash-flow tests (STEP-012, docs/08,
//! docs/15, docs/16, docs/19). Requires KOOPERATIF_TEST_DATABASE_URL;
//! each test runs in its own throwaway database.
//!
//! Invariants under test:
//! - Investment IDENTITY ≠ acquisition COST ≠ estimated VALUE ≠ cash
//!   RESULT: creating an investment moves zero money, a funding leg
//!   posts exactly ONE `investment_funding` outflow, valuations move
//!   zero money, income/disposal legs post `investment_income` /
//!   `investment_disposal` inflows.
//! - Acquisition is NOT operational Expense; investment income is NOT
//!   STEP-010 operational Income; sale proceeds are real cash, not a
//!   computed profit (no gain formula exists — UNRESOLVED).
//! - No event creates Payment / Allocation / Credit / Transfer /
//!   Share-Return side effects.
//! - Idempotent commands replay safely; concurrent outflows can never
//!   overdraw an account; posted history reverses instead of deleting.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
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
const TEST_PASSWORD: &str = "domain-test-parola-1";

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

struct TestApp {
    app: axum::Router,
    pool: PgPool,
    _peer: SocketAddr,
    _database_name: String,
}

impl TestApp {
    fn peer(&self) -> SocketAddr {
        self._peer
    }
}

async fn setup() -> Option<TestApp> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("kooperatif_server=error")
        .try_init();
    let url = test_database_url()?;
    let suffix = Uuid::new_v4().simple();
    let database_name = format!("kooperatif_test_{suffix}");
    let admin_url = url
        .rsplit_once('/')
        .map(|(b, _)| format!("{b}/postgres"))
        .unwrap();
    let base_url = url.rsplit_once('/').map(|(b, _)| b.to_string()).unwrap();
    let admin = app_db::connect(&admin_url).await.expect("admin connect");
    // F11: bound per-test database accumulation (24 h cutoff, no connections).
    let _ = app_db::drop_stale_test_databases(&admin).await;
    sqlx::query(&format!("CREATE DATABASE {database_name}"))
        .execute(&admin)
        .await
        .expect("test db");
    admin.close().await;
    let pool = app_db::connect(&format!("{base_url}/{database_name}"))
        .await
        .expect("db");
    app_db::run_migrations(&pool).await.expect("migrations");

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
    // Clock frozen at 2026-01-15 — business dates around it.
    let clock = Arc::new(MutableClock::new(
        OffsetDateTime::parse(
            "2026-01-15T00:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .expect("epoch"),
    ));
    let runtime = AuthRuntime::new(config, clock);
    let app = router(
        AppState {
            db: Some(pool.clone()),
            auth: Arc::new(runtime),
        },
        cors_layer(&[]),
    );
    let octets = Uuid::new_v4().as_bytes().to_vec();
    Some(TestApp {
        app,
        pool,
        _database_name: database_name,
        _peer: format!(
            "127.{}.{}.{}:{}",
            octets[0],
            octets[1],
            octets[2],
            52000 + octets[3] as u16
        )
        .parse()
        .expect("addr"),
    })
}

async fn create_admin(pool: &PgPool, username: &str) -> Uuid {
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).unwrap(),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).unwrap();
    let user = users::create_user(pool, username, "Test", &hash)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id) \
                 SELECT $1, id FROM roles WHERE name = 'Sistem Yöneticisi'",
    )
    .bind(user.id)
    .execute(pool)
    .await
    .unwrap();
    user.id
}

async fn create_plain_user(pool: &PgPool, username: &str) -> Uuid {
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).unwrap(),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).unwrap();
    users::create_user(pool, username, "Plain", &hash)
        .await
        .unwrap()
        .id
}

async fn login(app: &axum::Router, peer: SocketAddr, username: &str) -> (String, String) {
    let mut request = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .body(Body::from(
            json!({ "username": username, "password": TEST_PASSWORD }).to_string(),
        ))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(peer));
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let set_cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    (
        set_cookie
            .split(';')
            .next()
            .and_then(|p| p.split_once('='))
            .map(|(_, v)| v.to_string())
            .unwrap(),
        body["csrfToken"].as_str().unwrap().to_string(),
    )
}

fn req(
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
        builder.body(Body::from(body.to_string())).unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    }
}

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn admin_session(test: &TestApp) -> (String, String) {
    let name = format!("adm.{}", Uuid::new_v4().simple());
    create_admin(&test.pool, &name).await;
    login(&test.app, test.peer(), &name).await
}

// ---------------------------------------------------------------------
// Domain helpers
// ---------------------------------------------------------------------

async fn api_create_account(test: &TestApp, cookie: &str, csrf: &str, name: &str) -> Uuid {
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/financial-accounts",
            cookie,
            Some(csrf),
            Some(json!({ "name": name, "accountType": "cash" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "account: {detail}");
    detail["id"].as_str().unwrap().parse().unwrap()
}

async fn api_fund_account(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    account: Uuid,
    amount: &str,
    key: &str,
) {
    let options = {
        let (status, list) = send(
            &test.app,
            req(
                "GET",
                "/api/financial-categories/options?categoryType=income",
                cookie,
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        list
    };
    let category = options
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Diğer Gelir")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/incomes",
            cookie,
            Some(csrf),
            Some(json!({
                "financialAccountId": account.to_string(),
                "categoryId": category,
                "amount": amount,
                "description": "funding",
                "idempotencyKey": key,
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "funding: {detail}");
}

async fn api_balance(test: &TestApp, cookie: &str, id: Uuid) -> String {
    let (status, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/financial-accounts/{id}"),
            cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail["balance"].as_str().unwrap().to_string()
}

async fn api_movement_count(test: &TestApp, cookie: &str, account: Uuid) -> usize {
    let (status, list) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/financial-accounts/{account}/movements?pageSize=100"),
            cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "movements: {list}");
    list["items"].as_array().unwrap().len()
}

async fn api_create_investment(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    name: &str,
    investment_type: &str,
    key: &str,
) -> Value {
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/investments",
            cookie,
            Some(csrf),
            Some(json!({
                "name": name,
                "investmentType": investment_type,
                "acquiredAt": "2026-01-05",
                "idempotencyKey": key,
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "investment: {detail}");
    detail
}

async fn api_fund_investment(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    investment: &str,
    account: Uuid,
    amount: &str,
    key: &str,
) -> (StatusCode, Value) {
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/investments/{investment}/fundings"),
            cookie,
            Some(csrf),
            Some(json!({
                "financialAccountId": account.to_string(),
                "amount": amount,
                "occurredAt": "2026-01-10T10:00:00Z",
                "idempotencyKey": key,
            })),
        ),
    )
    .await
}

async fn api_income_investment(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    investment: &str,
    account: Uuid,
    amount: &str,
    key: &str,
) -> (StatusCode, Value) {
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/investments/{investment}/incomes"),
            cookie,
            Some(csrf),
            Some(json!({
                "financialAccountId": account.to_string(),
                "amount": amount,
                "occurredAt": "2026-01-12T10:00:00Z",
                "description": "Kira tahsilatı",
                "idempotencyKey": key,
            })),
        ),
    )
    .await
}

async fn api_valuation(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    investment: &str,
    date: &str,
    amount: &str,
    key: &str,
) -> (StatusCode, Value) {
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/investments/{investment}/valuations"),
            cookie,
            Some(csrf),
            Some(json!({
                "valuationDate": date,
                "amount": amount,
                "method": "Ekspertiz",
                "source": "Bağımsız değerleme",
                "idempotencyKey": key,
            })),
        ),
    )
    .await
}

async fn table_count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[tokio::test]
async fn create_investment_moves_no_money() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    api_fund_account(&test, &cookie, &csrf, account, "3000000.00", "fund-1").await;
    let before = api_movement_count(&test, &cookie, account).await;

    let detail = api_create_investment(
        &test,
        &cookie,
        &csrf,
        "Merkez Daire",
        "real_estate",
        "inv-1",
    )
    .await;
    assert_eq!(detail["status"], "active");
    assert_eq!(detail["investmentType"], "real_estate");
    assert_eq!(detail["totalFunded"], "0.00");
    assert!(detail["latestValuation"].is_null());
    assert_eq!(detail["totalIncome"], "0.00");
    assert_eq!(detail["totalProceeds"], "0.00");

    // Creating identity invented ZERO money.
    assert_eq!(api_balance(&test, &cookie, account).await, "3000000.00");
    assert_eq!(api_movement_count(&test, &cookie, account).await, before);
}

#[tokio::test]
async fn list_filters_and_validation() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    api_create_investment(
        &test,
        &cookie,
        &csrf,
        "Arsa Yatırımı",
        "real_estate",
        "inv-a",
    )
    .await;
    api_create_investment(
        &test,
        &cookie,
        &csrf,
        "Market İşletmesi",
        "business",
        "inv-b",
    )
    .await;

    let (status, list) = send(
        &test.app,
        req("GET", "/api/investments?pageSize=10", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["totalCount"], 2);
    assert_eq!(list["items"][0]["investmentNumber"], 2);

    let (status, filtered) = send(
        &test.app,
        req(
            "GET",
            "/api/investments?investmentType=business&status=active",
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(filtered["totalCount"], 1);
    assert_eq!(filtered["items"][0]["name"], "Market İşletmesi");

    let (status, searched) = send(
        &test.app,
        req("GET", "/api/investments?search=Arsa", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(searched["totalCount"], 1);

    // Invalid type/status rejected; future acquired date rejected.
    let (status, _) = send(
        &test.app,
        req("GET", "/api/investments?status=gold", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/investments",
            &cookie,
            Some(&csrf),
            Some(json!({
                "name": "X", "investmentType": "gold", "idempotencyKey": "bad-1"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{detail}");
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/investments",
            &cookie,
            Some(&csrf),
            Some(json!({
                "name": "Gelecek", "investmentType": "business",
                "acquiredAt": "2026-02-01", "idempotencyKey": "bad-2"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{detail}");
}

#[tokio::test]
async fn funding_posts_exactly_one_outflow_and_is_not_expense() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Banka").await;
    api_fund_account(&test, &cookie, &csrf, account, "3000000.00", "fund-1").await;
    let inv = api_create_investment(&test, &cookie, &csrf, "Dükkan", "real_estate", "inv-1").await;
    let inv_id = inv["id"].as_str().unwrap();

    let expense_count = table_count(&test.pool, "expense_entries").await;
    let (status, detail) = api_fund_investment(
        &test,
        &cookie,
        &csrf,
        inv_id,
        account,
        "2000000.00",
        "fnd-1",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");
    assert_eq!(detail["totalFunded"], "2000000.00");
    assert_eq!(detail["fundings"][0]["status"], "posted");

    assert_eq!(api_balance(&test, &cookie, account).await, "1000000.00");

    // Exactly ONE authoritative outflow with investment provenance.
    let movements: Vec<(String, String)> = sqlx::query_as(
        "SELECT source_type, direction FROM account_movements \
         WHERE account_id = $1 AND status = 'active'",
    )
    .bind(account)
    .fetch_all(&test.pool)
    .await
    .unwrap();
    let funding_legs: Vec<_> = movements
        .iter()
        .filter(|(s, _)| s == "investment_funding")
        .collect();
    assert_eq!(funding_legs.len(), 1);
    assert_eq!(funding_legs[0].1, "outflow");

    // Acquisition is NOT operational Expense (docs/03, docs/15).
    assert_eq!(
        table_count(&test.pool, "expense_entries").await,
        expense_count
    );
    // And it is NOT Payment / Credit / Transfer.
    assert_eq!(table_count(&test.pool, "payments").await, 0);
    assert_eq!(table_count(&test.pool, "payment_allocations").await, 0);
    assert_eq!(table_count(&test.pool, "shareholder_credits").await, 0);
    assert_eq!(table_count(&test.pool, "account_transfers").await, 0);
    assert_eq!(table_count(&test.pool, "share_returns").await, 0);
}

#[tokio::test]
async fn multi_account_funding_and_partial_legs() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let acc_a = api_create_account(&test, &cookie, &csrf, "İstanbul TL").await;
    let acc_b = api_create_account(&test, &cookie, &csrf, "Köy TL").await;
    api_fund_account(&test, &cookie, &csrf, acc_a, "700000.00", "fa-1").await;
    api_fund_account(&test, &cookie, &csrf, acc_b, "500000.00", "fb-1").await;
    let inv = api_create_investment(&test, &cookie, &csrf, "Tarla", "real_estate", "inv-1").await;
    let inv_id = inv["id"].as_str().unwrap();

    let (s1, _) =
        api_fund_investment(&test, &cookie, &csrf, inv_id, acc_a, "600000.00", "fnd-a").await;
    assert_eq!(s1, StatusCode::CREATED);
    let (s2, detail) =
        api_fund_investment(&test, &cookie, &csrf, inv_id, acc_b, "400000.00", "fnd-b").await;
    assert_eq!(s2, StatusCode::CREATED);

    assert_eq!(detail["totalFunded"], "1000000.00");
    assert_eq!(api_balance(&test, &cookie, acc_a).await, "100000.00");
    assert_eq!(api_balance(&test, &cookie, acc_b).await, "100000.00");
    // Two real cash legs -> two movements.
    let legs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_movements \
         WHERE source_type = 'investment_funding'",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(legs, 2);
}

#[tokio::test]
async fn funding_insufficient_funds_is_atomic() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    api_fund_account(&test, &cookie, &csrf, account, "500000.00", "fund-1").await;
    let inv = api_create_investment(&test, &cookie, &csrf, "Ofis", "business", "inv-1").await;
    let inv_id = inv["id"].as_str().unwrap();

    let (status, _) =
        api_fund_investment(&test, &cookie, &csrf, inv_id, account, "600000.00", "fnd-1").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(api_balance(&test, &cookie, account).await, "500000.00");
    assert_eq!(table_count(&test.pool, "investment_fundings").await, 0);
    let movements: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_movements WHERE source_type = 'investment_funding'",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(movements, 0, "failed funding must leave no movement");
}

#[tokio::test]
async fn funding_idempotency_replay_and_conflict() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    api_fund_account(&test, &cookie, &csrf, account, "100000.00", "fund-1").await;
    let inv = api_create_investment(&test, &cookie, &csrf, "Makine", "business", "inv-1").await;
    let inv_id = inv["id"].as_str().unwrap();

    let (s1, d1) =
        api_fund_investment(&test, &cookie, &csrf, inv_id, account, "40000.00", "k-1").await;
    assert_eq!(s1, StatusCode::CREATED);
    // Same key + same payload -> replay (200, same funding).
    let (s2, d2) =
        api_fund_investment(&test, &cookie, &csrf, inv_id, account, "40000.00", "k-1").await;
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(d1["fundings"][0]["id"], d2["fundings"][0]["id"]);
    assert_eq!(api_balance(&test, &cookie, account).await, "60000.00");
    // Same key + different payload -> 409.
    let (s3, _) =
        api_fund_investment(&test, &cookie, &csrf, inv_id, account, "45000.00", "k-1").await;
    assert_eq!(s3, StatusCode::CONFLICT);
    assert_eq!(api_balance(&test, &cookie, account).await, "60000.00");

    // Investment creation itself is idempotent too.
    let (s4, c1) = send(
        &test.app,
        req(
            "POST",
            "/api/investments",
            &cookie,
            Some(&csrf),
            Some(json!({
                "name": "Makine", "investmentType": "business",
                "acquiredAt": "2026-01-05", "idempotencyKey": "inv-1"
            })),
        ),
    )
    .await;
    assert_eq!(s4, StatusCode::OK);
    assert_eq!(c1["id"], inv["id"]);
}

#[tokio::test]
async fn concurrent_fundings_cannot_overdraw() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    api_fund_account(&test, &cookie, &csrf, account, "1000.00", "fund-1").await;
    let inv = api_create_investment(&test, &cookie, &csrf, "Parsel", "real_estate", "inv-1").await;
    let inv_id = inv["id"].as_str().unwrap().to_string();

    let app = test.app.clone();
    let c1 = cookie.clone();
    let csrf1 = csrf.clone();
    let inv1 = inv_id.clone();
    let t1 = tokio::spawn(async move {
        send(
            &app,
            req(
                "POST",
                &format!("/api/investments/{inv1}/fundings"),
                &c1,
                Some(&csrf1),
                Some(json!({
                    "financialAccountId": account.to_string(),
                    "amount": "700.00",
                    "occurredAt": "2026-01-10T10:00:00Z",
                    "idempotencyKey": "race-1"
                })),
            ),
        )
        .await
    });
    let app = test.app.clone();
    let inv2 = inv_id.clone();
    let cookie2 = cookie.clone();
    let csrf2 = csrf.clone();
    let t2 = tokio::spawn(async move {
        send(
            &app,
            req(
                "POST",
                &format!("/api/investments/{inv2}/fundings"),
                &cookie2,
                Some(&csrf2),
                Some(json!({
                    "financialAccountId": account.to_string(),
                    "amount": "700.00",
                    "occurredAt": "2026-01-10T10:00:00Z",
                    "idempotencyKey": "race-2"
                })),
            ),
        )
        .await
    });
    let (r1, r2) = tokio::join!(t1, t2);
    let (s1, _) = r1.unwrap();
    let (s2, _) = r2.unwrap();
    let winners = [s1, s2]
        .iter()
        .filter(|s| **s == StatusCode::CREATED)
        .count();
    assert_eq!(winners, 1, "exactly one funding may win");
    assert_eq!(api_balance(&test, &cookie, account).await, "300.00");
}

#[tokio::test]
async fn valuation_moves_zero_money_and_keeps_history() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    api_fund_account(&test, &cookie, &csrf, account, "3000000.00", "fund-1").await;
    let inv = api_create_investment(&test, &cookie, &csrf, "Daire", "real_estate", "inv-1").await;
    let inv_id = inv["id"].as_str().unwrap();
    api_fund_investment(
        &test,
        &cookie,
        &csrf,
        inv_id,
        account,
        "2000000.00",
        "fnd-1",
    )
    .await;
    let movements_before = api_movement_count(&test, &cookie, account).await;
    let income_rows = table_count(&test.pool, "income_entries").await;
    let expense_rows = table_count(&test.pool, "expense_entries").await;

    // Valuation above cost — the unrealized difference moves nothing.
    let (s1, d1) = api_valuation(
        &test,
        &cookie,
        &csrf,
        inv_id,
        "2026-01-11",
        "2700000.00",
        "v-1",
    )
    .await;
    assert_eq!(s1, StatusCode::CREATED, "{d1}");
    assert_eq!(d1["latestValuation"], "2700000.00");
    assert_eq!(api_balance(&test, &cookie, account).await, "1000000.00");
    assert_eq!(
        api_movement_count(&test, &cookie, account).await,
        movements_before
    );
    assert_eq!(table_count(&test.pool, "income_entries").await, income_rows);
    assert_eq!(
        table_count(&test.pool, "expense_entries").await,
        expense_rows
    );
    assert_eq!(table_count(&test.pool, "payments").await, 0);
    assert_eq!(table_count(&test.pool, "account_transfers").await, 0);
    assert_eq!(table_count(&test.pool, "shareholder_credits").await, 0);

    // History accumulates; newest wins.
    api_valuation(
        &test,
        &cookie,
        &csrf,
        inv_id,
        "2026-01-13",
        "2750000.00",
        "v-2",
    )
    .await;
    let (status, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/investments/{inv_id}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["latestValuation"], "2750000.00");
    assert_eq!(detail["valuations"].as_array().unwrap().len(), 2);

    // Cancelling the newest exposes the earlier valuation again.
    let newest = detail["valuations"][0]["id"].as_str().unwrap();
    let (status, d3) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/investment-valuations/{newest}/cancel"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "Yanlış giriş" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{d3}");
    assert_eq!(d3["latestValuation"], "2700000.00");
    // Second cancel is an idempotent no-op, history preserved.
    let (status, d4) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/investment-valuations/{newest}/cancel"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "Yanlış giriş" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(d4["valuations"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn investment_income_posts_one_inflow_not_operational_income() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    api_fund_account(&test, &cookie, &csrf, account, "100.00", "fund-1").await;
    let inv = api_create_investment(&test, &cookie, &csrf, "Dükkan", "real_estate", "inv-1").await;
    let inv_id = inv["id"].as_str().unwrap();
    let income_rows = table_count(&test.pool, "income_entries").await;

    let (status, detail) =
        api_income_investment(&test, &cookie, &csrf, inv_id, account, "50000.00", "inc-1").await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");
    assert_eq!(detail["totalIncome"], "50000.00");
    assert_eq!(api_balance(&test, &cookie, account).await, "50100.00");

    // Exactly ONE inflow movement, investment_income provenance.
    let legs: Vec<(String, String)> = sqlx::query_as(
        "SELECT source_type, direction FROM account_movements \
         WHERE account_id = $1 AND status = 'active'",
    )
    .bind(account)
    .fetch_all(&test.pool)
    .await
    .unwrap();
    let income_legs: Vec<_> = legs
        .iter()
        .filter(|(s, _)| s == "investment_income")
        .collect();
    assert_eq!(income_legs.len(), 1);
    assert_eq!(income_legs[0].1, "inflow");
    // NOT a STEP-010 operational income row (no double counting).
    assert_eq!(table_count(&test.pool, "income_entries").await, income_rows);
    // No Payment/Credit/Transfer/Share-Return side effects.
    assert_eq!(table_count(&test.pool, "payments").await, 0);
    assert_eq!(table_count(&test.pool, "account_transfers").await, 0);
    assert_eq!(table_count(&test.pool, "shareholder_credits").await, 0);
    assert_eq!(
        table_count(&test.pool, "share_return_entitlements").await,
        0
    );
}

#[tokio::test]
async fn income_reversal_preserves_history_and_respects_funds() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let inv = api_create_investment(&test, &cookie, &csrf, "Arsa", "real_estate", "inv-1").await;
    let inv_id = inv["id"].as_str().unwrap();
    let (status, detail) =
        api_income_investment(&test, &cookie, &csrf, inv_id, account, "500.00", "inc-1").await;
    assert_eq!(status, StatusCode::CREATED);
    let income_id = detail["incomes"][0]["id"].as_str().unwrap();
    assert_eq!(api_balance(&test, &cookie, account).await, "500.00");

    // Reverse: money must be present (500 still is).
    let (status, d2) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/investment-incomes/{income_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "Hatalı kayıt" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{d2}");
    assert_eq!(d2["incomes"][0]["status"], "reversed");
    assert_eq!(d2["incomes"][0]["movementStatus"], "reversed");
    assert_eq!(api_balance(&test, &cookie, account).await, "0.00");

    // Idempotent second reversal: no second financial effect.
    let (status, d3) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/investment-incomes/{income_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "Hatalı kayıt" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(api_balance(&test, &cookie, account).await, "0.00");
    assert_eq!(d3["incomes"].as_array().unwrap().len(), 1);

    // New income, spend the money, then reversal must be refused.
    let (status, detail2) =
        api_income_investment(&test, &cookie, &csrf, inv_id, account, "500.00", "inc-2").await;
    assert_eq!(status, StatusCode::CREATED);
    let income2 = detail2["incomes"][1]["id"].as_str().unwrap();
    let options = {
        let (s, l) = send(
            &test.app,
            req(
                "GET",
                "/api/financial-categories/options?categoryType=expense",
                &cookie,
                None,
                None,
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        l
    };
    let expense_category = options.as_array().unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/expenses",
            &cookie,
            Some(&csrf),
            Some(json!({
                "financialAccountId": account.to_string(),
                "categoryId": expense_category,
                "amount": "400.00",
                "description": "bakım",
                "idempotencyKey": "exp-1"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/investment-incomes/{income2}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "Hatalı kayıt" })),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "reversal must not make the account negative"
    );
    assert_eq!(api_balance(&test, &cookie, account).await, "100.00");
}

#[tokio::test]
async fn funding_reversal_restores_balance_and_keeps_history() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    api_fund_account(&test, &cookie, &csrf, account, "1000.00", "fund-1").await;
    let inv = api_create_investment(&test, &cookie, &csrf, "Depo", "business", "inv-1").await;
    let inv_id = inv["id"].as_str().unwrap();
    let (status, detail) =
        api_fund_investment(&test, &cookie, &csrf, inv_id, account, "600.00", "fnd-1").await;
    assert_eq!(status, StatusCode::CREATED);
    let funding_id = detail["fundings"][0]["id"].as_str().unwrap();

    let (status, d2) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/investment-fundings/{funding_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "Yanlış hesap" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{d2}");
    assert_eq!(d2["fundings"][0]["status"], "reversed");
    assert_eq!(d2["fundings"][0]["movementStatus"], "reversed");
    assert_eq!(api_balance(&test, &cookie, account).await, "1000.00");
    assert_eq!(d2["totalFunded"], "0.00");
}

#[tokio::test]
async fn disposal_posts_actual_proceeds_and_closes_investment() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let acc_a = api_create_account(&test, &cookie, &csrf, "Banka").await;
    let acc_b = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    api_fund_account(&test, &cookie, &csrf, acc_a, "2000000.00", "fa-1").await;
    let inv =
        api_create_investment(&test, &cookie, &csrf, "Apartman", "real_estate", "inv-1").await;
    let inv_id = inv["id"].as_str().unwrap();
    api_fund_investment(&test, &cookie, &csrf, inv_id, acc_a, "2000000.00", "fnd-1").await;
    api_valuation(
        &test,
        &cookie,
        &csrf,
        inv_id,
        "2026-01-12",
        "2700000.00",
        "v-1",
    )
    .await;
    let income_rows = table_count(&test.pool, "income_entries").await;

    // Sold for 2.5M across two accounts — proceeds are real cash, the
    // 700k valuation increase never became money by itself.
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/investments/{inv_id}/dispose"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "disposedAt": "2026-01-14",
                "considerationAmount": "2500000.00",
                "counterpartyName": "Alıcı A.Ş.",
                "proceeds": [
                    {
                        "financialAccountId": acc_a.to_string(),
                        "amount": "1500000.00",
                        "occurredAt": "2026-01-14T10:00:00Z"
                    },
                    {
                        "financialAccountId": acc_b.to_string(),
                        "amount": "1000000.00",
                        "occurredAt": "2026-01-14T10:00:00Z"
                    }
                ],
                "idempotencyKey": "disp-1"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");
    assert_eq!(detail["status"], "disposed");
    assert_eq!(detail["disposal"]["considerationAmount"], "2500000.00");
    assert_eq!(detail["totalProceeds"], "2500000.00");
    assert_eq!(api_balance(&test, &cookie, acc_a).await, "1500000.00");
    assert_eq!(api_balance(&test, &cookie, acc_b).await, "1000000.00");

    // Two legs -> two investment_disposal movements; sale proceeds are
    // NOT operational Income.
    let legs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_movements \
         WHERE source_type = 'investment_disposal' AND direction = 'inflow'",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(legs, 2);
    assert_eq!(table_count(&test.pool, "income_entries").await, income_rows);

    // A disposed investment accepts no more events.
    let (status, _) =
        api_fund_investment(&test, &cookie, &csrf, inv_id, acc_a, "1.00", "fnd-late").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = api_valuation(
        &test,
        &cookie,
        &csrf,
        inv_id,
        "2026-01-14",
        "1.00",
        "v-late",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) =
        api_income_investment(&test, &cookie, &csrf, inv_id, acc_a, "1.00", "inc-late").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Idempotent replay: same key -> same disposal, no extra cash.
    let (status, replay) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/investments/{inv_id}/dispose"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "disposedAt": "2026-01-14",
                "considerationAmount": "2500000.00",
                "counterpartyName": "Alıcı A.Ş.",
                "proceeds": [
                    {
                        "financialAccountId": acc_a.to_string(),
                        "amount": "1500000.00",
                        "occurredAt": "2026-01-14T10:00:00Z"
                    },
                    {
                        "financialAccountId": acc_b.to_string(),
                        "amount": "1000000.00",
                        "occurredAt": "2026-01-14T10:00:00Z"
                    }
                ],
                "idempotencyKey": "disp-1"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["disposal"]["id"], detail["disposal"]["id"]);
    assert_eq!(api_balance(&test, &cookie, acc_a).await, "1500000.00");

    // Same key + different payload -> 409.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/investments/{inv_id}/dispose"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "disposedAt": "2026-01-14",
                "proceeds": [
                    {
                        "financialAccountId": acc_a.to_string(),
                        "amount": "999.00",
                        "occurredAt": "2026-01-14T10:00:00Z"
                    }
                ],
                "idempotencyKey": "disp-1"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn cancel_requires_no_financial_events() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let inv = api_create_investment(&test, &cookie, &csrf, "Hatalı", "business", "inv-1").await;
    let inv_id = inv["id"].as_str().unwrap();

    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/investments/{inv_id}/cancel"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "Yanlışlıkla açıldı" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["status"], "cancelled");

    // With a posted funding, cancellation is refused.
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    api_fund_account(&test, &cookie, &csrf, account, "100.00", "fund-1").await;
    let inv2 = api_create_investment(&test, &cookie, &csrf, "Gerçek", "business", "inv-2").await;
    let inv2_id = inv2["id"].as_str().unwrap();
    api_fund_investment(&test, &cookie, &csrf, inv2_id, account, "50.00", "fnd-1").await;
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/investments/{inv2_id}/cancel"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "deneme" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn disposal_requires_proceeds_and_validates() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let inv = api_create_investment(&test, &cookie, &csrf, "Hisse", "business", "inv-1").await;
    let inv_id = inv["id"].as_str().unwrap();

    // Empty proceeds -> validation error (cash-free sale = deferred
    // receivable modelling, not STEP-012).
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/investments/{inv_id}/dispose"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "disposedAt": "2026-01-14",
                "proceeds": [],
                "idempotencyKey": "disp-0"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Inactive account refused.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/financial-accounts/{account}/status-change"),
            &cookie,
            Some(&csrf),
            Some(json!({ "status": "inactive" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/investments/{inv_id}/dispose"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "disposedAt": "2026-01-14",
                "proceeds": [{
                    "financialAccountId": account.to_string(),
                    "amount": "10.00",
                    "occurredAt": "2026-01-14T10:00:00Z"
                }],
                "idempotencyKey": "disp-2"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn permissions_csrf_and_no_store_are_enforced() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let inv =
        api_create_investment(&test, &cookie, &csrf, "Yetki Testi", "business", "inv-1").await;
    let inv_id = inv["id"].as_str().unwrap();

    // Plain user: no investments.read -> 403 on read AND mutation.
    let plain = format!("pln.{}", Uuid::new_v4().simple());
    create_plain_user(&test.pool, &plain).await;
    let (pcookie, pcsrf) = login(&test.app, test.peer(), &plain).await;
    let (status, _) = send(
        &test.app,
        req("GET", "/api/investments", &pcookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/investments/{inv_id}"),
            &pcookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/investments/{inv_id}/cancel"),
            &pcookie,
            Some(&pcsrf),
            Some(json!({ "reason": "deneme" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Admin without CSRF -> 403.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/investments/{inv_id}/cancel"),
            &cookie,
            None,
            Some(json!({ "reason": "deneme" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // no-store on reads.
    let response = test
        .app
        .clone()
        .oneshot(req("GET", "/api/investments", &cookie, None, None))
        .await
        .unwrap();
    let cache = response
        .headers()
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(cache.contains("no-store"), "cache-control: {cache}");

    // Unknown investment id -> 404.
    let (status, _) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/investments/{}", Uuid::new_v4()),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
