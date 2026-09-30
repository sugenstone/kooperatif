//! Database-gated Income/Expense domain tests (STEP-010). Requires
//! KOOPERATIF_TEST_DATABASE_URL; each test runs in its own throwaway
//! database.
//!
//! Invariants under test (docs/07, docs/15, docs/19, ADR-003/004/006):
//! - A posted Income produces exactly ONE inflow Account Movement;
//!   a posted Expense exactly ONE outflow — atomically, or nothing.
//! - Income/Expense are NOT Payments, Transfers or Credits: summaries
//!   never double-count other movement sources.
//! - Account balance stays movement-derived; income-expense net is a
//!   different concept.
//! - Negative balances are forbidden; reversals preserve history.

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

fn test_database_url() -> Option<String> {
    std::env::var("KOOPERATIF_TEST_DATABASE_URL")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
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
    let url = test_database_url()?;
    let suffix = Uuid::new_v4().simple();
    let database_name = format!("kooperatif_test_{suffix}");
    let admin_url = url
        .rsplit_once('/')
        .map(|(b, _)| format!("{b}/postgres"))
        .unwrap();
    let base_url = url.rsplit_once('/').map(|(b, _)| b.to_string()).unwrap();
    let admin = app_db::connect(&admin_url).await.expect("admin connect");
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
    let clock = Arc::new(MutableClock::new(
        OffsetDateTime::parse(
            "2026-01-01T00:00:00Z",
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
    assert_eq!(status, StatusCode::CREATED, "account create: {detail}");
    detail["id"].as_str().unwrap().parse().unwrap()
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
    assert_eq!(status, StatusCode::OK, "account get: {detail}");
    detail["balance"].as_str().unwrap().to_string()
}

async fn api_movements(test: &TestApp, cookie: &str, account: Uuid) -> Vec<Value> {
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
    list["items"].as_array().unwrap().clone()
}

async fn api_category_options(test: &TestApp, cookie: &str, kind: &str) -> Vec<Value> {
    let (status, list) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/financial-categories/options?categoryType={kind}"),
            cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "category options: {list}");
    list.as_array().unwrap().clone()
}

async fn api_create_category(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    kind: &str,
    name: &str,
) -> (StatusCode, Value) {
    send(
        &test.app,
        req(
            "POST",
            "/api/financial-categories",
            cookie,
            Some(csrf),
            Some(json!({ "categoryType": kind, "name": name })),
        ),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn post_entry(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    kind: &str,
    account: Uuid,
    category: Uuid,
    amount: &str,
    description: &str,
    key: &str,
) -> (StatusCode, Value) {
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/{kind}s"),
            cookie,
            Some(csrf),
            Some(json!({
                "financialAccountId": account.to_string(),
                "categoryId": category.to_string(),
                "amount": amount,
                "description": description,
                "idempotencyKey": key,
            })),
        ),
    )
    .await
}

async fn post_income(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    account: Uuid,
    category: Uuid,
    amount: &str,
    key: &str,
) -> (StatusCode, Value) {
    post_entry(
        test,
        cookie,
        csrf,
        "income",
        account,
        category,
        amount,
        "kira geliri",
        key,
    )
    .await
}

async fn post_expense(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    account: Uuid,
    category: Uuid,
    amount: &str,
    key: &str,
) -> (StatusCode, Value) {
    post_entry(
        test,
        cookie,
        csrf,
        "expense",
        account,
        category,
        amount,
        "elektrik faturası",
        key,
    )
    .await
}

async fn reverse_entry(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    kind: &str,
    id: &str,
    reason: &str,
) -> (StatusCode, Value) {
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/{kind}s/{id}/reverse"),
            cookie,
            Some(csrf),
            Some(json!({ "reason": reason })),
        ),
    )
    .await
}

async fn summary(test: &TestApp, cookie: &str) -> Value {
    let (status, body) = send(
        &test.app,
        req("GET", "/api/income-expense/summary", cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "summary: {body}");
    body
}

fn income_category(options: &[Value]) -> String {
    options.iter().find(|c| c["name"] == "Diğer Gelir").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn expense_category(options: &[Value]) -> String {
    options.iter().find(|c| c["name"] == "Genel Gider").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

// ---------------------------------------------------------------------
// Posting invariants
// ---------------------------------------------------------------------

#[tokio::test]
async fn income_posts_exactly_one_inflow_movement() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let category: Uuid = income_category(&api_category_options(&test, &cookie, "income").await)
        .parse()
        .unwrap();

    let (status, entry) =
        post_income(&test, &cookie, &csrf, account, category, "5000.00", "k-i1").await;
    assert_eq!(status, StatusCode::CREATED, "{entry}");
    assert_eq!(entry["amount"], "5000.00");
    assert_eq!(entry["currency"], "TRY");
    assert_eq!(entry["status"], "posted");
    assert_eq!(entry["categoryName"], "Diğer Gelir");
    assert!(entry["entryNumber"].as_i64().unwrap() > 0);

    // Exactly ONE active inflow movement, linked both ways.
    let movements = api_movements(&test, &cookie, account).await;
    assert_eq!(movements.len(), 1);
    assert_eq!(movements[0]["direction"], "inflow");
    assert_eq!(movements[0]["amount"], "5000.00");
    assert_eq!(movements[0]["sourceType"], "income");
    assert_eq!(movements[0]["sourceId"], entry["id"]);
    assert_eq!(movements[0]["sourceNumber"], entry["entryNumber"]);
    assert_eq!(movements[0]["id"], entry["accountMovementId"]);
    assert_eq!(api_balance(&test, &cookie, account).await, "5000.00");

    // Audit evidence exists.
    let audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events WHERE event_type = 'income_posted'",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(audit, 1);
}

#[tokio::test]
async fn expense_posts_exactly_one_outflow_and_checks_funds() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let inc_cat: Uuid = income_category(&api_category_options(&test, &cookie, "income").await)
        .parse()
        .unwrap();
    let exp_cat: Uuid = expense_category(&api_category_options(&test, &cookie, "expense").await)
        .parse()
        .unwrap();

    // Fund the account first (income), then spend part of it.
    post_income(&test, &cookie, &csrf, account, inc_cat, "1000.00", "k-f1").await;
    let (status, entry) =
        post_expense(&test, &cookie, &csrf, account, exp_cat, "300.00", "k-e1").await;
    assert_eq!(status, StatusCode::CREATED, "{entry}");
    assert_eq!(entry["status"], "posted");
    assert_eq!(api_balance(&test, &cookie, account).await, "700.00");

    // Overspend is rejected atomically: no entry, no movement.
    let (status, _) = post_expense(&test, &cookie, &csrf, account, exp_cat, "800.00", "k-e2").await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "insufficient funds must conflict"
    );
    assert_eq!(api_balance(&test, &cookie, account).await, "700.00");
    let entries: i64 = sqlx::query_scalar("SELECT count(*) FROM expense_entries")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(entries, 1, "failed expense must not survive");
}

#[tokio::test]
async fn validation_rejects_bad_inputs() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let inc_cat: Uuid = income_category(&api_category_options(&test, &cookie, "income").await)
        .parse()
        .unwrap();
    let exp_cat: Uuid = expense_category(&api_category_options(&test, &cookie, "expense").await)
        .parse()
        .unwrap();

    // Wrong category type for the entry kind.
    let (status, _) = post_income(&test, &cookie, &csrf, account, exp_cat, "10.00", "k-wt").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "expense category on income"
    );
    let (status, _) = post_expense(&test, &cookie, &csrf, account, inc_cat, "10.00", "k-wt2").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "income category on expense"
    );

    // Unknown account / unknown category.
    let (status, _) = post_income(
        &test,
        &cookie,
        &csrf,
        Uuid::new_v4(),
        inc_cat,
        "10.00",
        "k-ua",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = post_income(
        &test,
        &cookie,
        &csrf,
        account,
        Uuid::new_v4(),
        "10.00",
        "k-uc",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Zero / negative / malformed / future-dated.
    for amount in ["0.00", "-5.00", "10.001", "abc"] {
        let (status, _) =
            post_income(&test, &cookie, &csrf, account, inc_cat, amount, "k-am").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "amount {amount}");
    }
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/incomes",
            &cookie,
            Some(&csrf),
            Some(json!({
                "financialAccountId": account.to_string(),
                "categoryId": inc_cat.to_string(),
                "amount": "10.00",
                "description": "gelecek",
                "occurredAt": "2030-01-01T00:00:00Z",
                "idempotencyKey": "k-future"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "future occurredAt");

    // Empty description rejected; oversized counterparty rejected.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/incomes",
            &cookie,
            Some(&csrf),
            Some(json!({
                "financialAccountId": account.to_string(),
                "categoryId": inc_cat.to_string(),
                "amount": "10.00",
                "description": "   ",
                "idempotencyKey": "k-desc"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn idempotent_replay_and_conflict() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let inc_cat: Uuid = income_category(&api_category_options(&test, &cookie, "income").await)
        .parse()
        .unwrap();

    let first = post_income(&test, &cookie, &csrf, account, inc_cat, "250.00", "k-idem").await;
    assert_eq!(first.0, StatusCode::CREATED);
    // Same key + same payload → safe replay (200), no duplicate money.
    let replay = post_income(&test, &cookie, &csrf, account, inc_cat, "250.00", "k-idem").await;
    assert_eq!(replay.0, StatusCode::OK);
    assert_eq!(replay.1["id"], first.1["id"]);
    assert_eq!(api_balance(&test, &cookie, account).await, "250.00");
    let movements = api_movements(&test, &cookie, account).await;
    assert_eq!(movements.len(), 1, "replay must not double-post");

    // Same key + different payload → 409.
    let conflict = post_income(&test, &cookie, &csrf, account, inc_cat, "300.00", "k-idem").await;
    assert_eq!(conflict.0, StatusCode::CONFLICT);
    assert_eq!(api_balance(&test, &cookie, account).await, "250.00");
}

// ---------------------------------------------------------------------
// Reversal
// ---------------------------------------------------------------------

#[tokio::test]
async fn reversal_neutralizes_movement_and_preserves_history() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let inc_cat: Uuid = income_category(&api_category_options(&test, &cookie, "income").await)
        .parse()
        .unwrap();
    let exp_cat: Uuid = expense_category(&api_category_options(&test, &cookie, "expense").await)
        .parse()
        .unwrap();

    post_income(&test, &cookie, &csrf, account, inc_cat, "1000.00", "k-r-i").await;
    let (_, expense) =
        post_expense(&test, &cookie, &csrf, account, exp_cat, "400.00", "k-r-e").await;
    assert_eq!(api_balance(&test, &cookie, account).await, "600.00");

    // Reverse the expense: balance restored, both rows preserved as
    // `reversed` with actor/time/reason.
    let (status, reversed) = reverse_entry(
        &test,
        &cookie,
        &csrf,
        "expense",
        expense["id"].as_str().unwrap(),
        "yanlış tutar",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reversed}");
    assert_eq!(reversed["status"], "reversed");
    assert_eq!(reversed["reversalReason"], "yanlış tutar");
    assert!(reversed["reversedAt"].is_string());
    assert_eq!(reversed["movementStatus"], "reversed");
    assert_eq!(api_balance(&test, &cookie, account).await, "1000.00");

    // Reversal is idempotent: replay returns the same reversed entry.
    let (status, again) = reverse_entry(
        &test,
        &cookie,
        &csrf,
        "expense",
        expense["id"].as_str().unwrap(),
        "tekrar",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["reversalReason"], "yanlış tutar");

    // Reversing the income after funds still exist: allowed.
    let (_, income_list) = send(
        &test.app,
        req("GET", "/api/incomes?pageSize=10", &cookie, None, None),
    )
    .await;
    let income_id = income_list["items"][0]["id"].as_str().unwrap().to_string();
    let (status, reversed_income) =
        reverse_entry(&test, &cookie, &csrf, "income", &income_id, "iptal").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reversed_income["status"], "reversed");
    assert_eq!(api_balance(&test, &cookie, account).await, "0.00");

    // History survives: both movements exist, marked reversed.
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM account_movements")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    let active: i64 =
        sqlx::query_scalar("SELECT count(*) FROM account_movements WHERE status = 'active'")
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert_eq!(total, 2);
    assert_eq!(active, 0);
}

#[tokio::test]
async fn income_reversal_refused_when_funds_gone() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let inc_cat: Uuid = income_category(&api_category_options(&test, &cookie, "income").await)
        .parse()
        .unwrap();
    let exp_cat: Uuid = expense_category(&api_category_options(&test, &cookie, "expense").await)
        .parse()
        .unwrap();

    let (_, income) =
        post_income(&test, &cookie, &csrf, account, inc_cat, "500.00", "k-ir-i").await;
    // Spend the money — reversing the income would fabricate a
    // negative balance, so it must be refused.
    post_expense(&test, &cookie, &csrf, account, exp_cat, "500.00", "k-ir-e").await;
    assert_eq!(api_balance(&test, &cookie, account).await, "0.00");

    let (status, _) = reverse_entry(
        &test,
        &cookie,
        &csrf,
        "income",
        income["id"].as_str().unwrap(),
        "geri al",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "reversal would go negative");
    assert_eq!(api_balance(&test, &cookie, account).await, "0.00");
}

#[tokio::test]
async fn concurrent_expenses_never_overspend() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let inc_cat: Uuid = income_category(&api_category_options(&test, &cookie, "income").await)
        .parse()
        .unwrap();
    let exp_cat: Uuid = expense_category(&api_category_options(&test, &cookie, "expense").await)
        .parse()
        .unwrap();
    post_income(
        &test, &cookie, &csrf, account, inc_cat, "1000.00", "k-c-fund",
    )
    .await;

    // Two racing 700-expenses against a 1000 balance: exactly ONE may
    // win — the account row lock serializes the fund check (ADR-006).
    let first = post_expense(&test, &cookie, &csrf, account, exp_cat, "700.00", "k-c-a");
    let second = post_expense(&test, &cookie, &csrf, account, exp_cat, "700.00", "k-c-b");
    let (r1, r2) = tokio::join!(first, second);
    let statuses = [r1.0, r2.0];
    assert!(
        statuses.contains(&StatusCode::CREATED) && statuses.contains(&StatusCode::CONFLICT),
        "expected one winner + one insufficient-funds conflict, got {statuses:?}"
    );
    assert_eq!(api_balance(&test, &cookie, account).await, "300.00");
}

#[tokio::test]
async fn concurrent_reversal_single_writer() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let inc_cat: Uuid = income_category(&api_category_options(&test, &cookie, "income").await)
        .parse()
        .unwrap();
    let (_, income) = post_income(&test, &cookie, &csrf, account, inc_cat, "100.00", "k-cr").await;
    let id = income["id"].as_str().unwrap().to_string();

    let (r1, r2) = tokio::join!(
        reverse_entry(&test, &cookie, &csrf, "income", &id, "a"),
        reverse_entry(&test, &cookie, &csrf, "income", &id, "b"),
    );
    assert_eq!(r1.0, StatusCode::OK);
    assert_eq!(r2.0, StatusCode::OK);
    // Exactly one reversal persisted — deterministic winner, no
    // double reversal effect.
    let reversed_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM income_entries WHERE id = $1 AND status = 'reversed'",
    )
    .bind(Uuid::parse_str(&id).unwrap())
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(reversed_count, 1);
}

// ---------------------------------------------------------------------
// Boundaries: no double counting, no deletion, categories
// ---------------------------------------------------------------------

#[tokio::test]
async fn payments_transfers_and_credits_are_not_income_or_expense() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let other = api_create_account(&test, &cookie, &csrf, "Kasa 2").await;
    let inc_cat: Uuid = income_category(&api_category_options(&test, &cookie, "income").await)
        .parse()
        .unwrap();

    // A Payment inflow + a Transfer out + one Income. The account
    // balance reflects all three; the income summary only the Income.
    send(
        &test.app,
        req(
            "POST",
            "/api/payments",
            &cookie,
            Some(&csrf),
            Some(json!({
                "payerFirstName": "Ödeyen", "payerLastName": "Kişi",
                "amount": "500.00", "method": "cash",
                "destinationAccountId": account.to_string(),
                "idempotencyKey": "k-pay", "allocations": []
            })),
        ),
    )
    .await;
    post_income(&test, &cookie, &csrf, account, inc_cat, "1000.00", "k-inc").await;
    send(
        &test.app,
        req(
            "POST",
            "/api/account-transfers",
            &cookie,
            Some(&csrf),
            Some(json!({
                "sourceAccountId": account.to_string(),
                "destinationAccountId": other.to_string(),
                "amount": "200.00",
                "idempotencyKey": "k-tr",
            })),
        ),
    )
    .await;

    // Balance = payment 500 + income 1000 - transfer 200 = 1300.
    assert_eq!(api_balance(&test, &cookie, account).await, "1300.00");

    // Operational summary sees ONLY the income entry — payments and
    // transfers are never income/expense (docs/15 invariants).
    let s = summary(&test, &cookie).await;
    assert_eq!(s["incomeTotal"], "1000.00");
    assert_eq!(s["expenseTotal"], "0.00");
    assert_eq!(s["net"], "1000.00");
    assert_eq!(s["incomeCount"], 1);

    // Income list contains exactly the income entry — no payment.
    let (_, incomes) = send(
        &test.app,
        req("GET", "/api/incomes?pageSize=100", &cookie, None, None),
    )
    .await;
    assert_eq!(incomes["items"].as_array().unwrap().len(), 1);
    assert_eq!(incomes["totalCount"], 1);

    // Movement provenance stays typed (frozen test clock → order by
    // set, not position).
    let movements = api_movements(&test, &cookie, account).await;
    let mut sources: Vec<&str> = movements
        .iter()
        .map(|m| m["sourceType"].as_str().unwrap())
        .collect();
    sources.sort_unstable();
    assert_eq!(sources, ["income", "payment", "transfer"]);
}

#[tokio::test]
async fn category_lifecycle_and_history_stability() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;

    // Create + duplicate-name rejection (within type).
    let (status, cat) =
        api_create_category(&test, &cookie, &csrf, "income", "Bağış Dışı Gelir").await;
    assert_eq!(status, StatusCode::CREATED, "{cat}");
    let cat_id = cat["id"].as_str().unwrap().to_string();
    let (status, _) =
        api_create_category(&test, &cookie, &csrf, "income", "bağış dışı gelir").await;
    assert_eq!(status, StatusCode::CONFLICT, "case-insensitive duplicate");

    // Same name is fine under the OTHER type.
    let (status, _) =
        api_create_category(&test, &cookie, &csrf, "expense", "Bağış Dışı Gelir").await;
    assert_eq!(status, StatusCode::CREATED);

    // Post an entry under it, then deactivate the category.
    post_income(
        &test,
        &cookie,
        &csrf,
        account,
        cat_id.parse().unwrap(),
        "10.00",
        "k-cat",
    )
    .await;
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/financial-categories/{cat_id}/status-change"),
            &cookie,
            Some(&csrf),
            Some(json!({ "status": "inactive" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Inactive category refuses NEW entries…
    let (status, _) = post_income(
        &test,
        &cookie,
        &csrf,
        account,
        cat_id.parse().unwrap(),
        "10.00",
        "k-cat2",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // …but leaves pickers/options and the historical entry intact.
    let options = api_category_options(&test, &cookie, "income").await;
    assert!(options.iter().all(|c| c["id"] != cat_id));
    let (_, incomes) = send(
        &test.app,
        req("GET", "/api/incomes?pageSize=10", &cookie, None, None),
    )
    .await;
    assert_eq!(incomes["items"][0]["categoryName"], "Bağış Dışı Gelir");

    // No DELETE endpoint exists for posted history.
    let (status, _) = send(
        &test.app,
        req(
            "DELETE",
            &format!(
                "/api/incomes/{}",
                incomes["items"][0]["id"].as_str().unwrap()
            ),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::METHOD_NOT_ALLOWED,
        "hard delete must not exist: {status}"
    );
}

#[tokio::test]
async fn filters_pagination_and_summary_boundaries() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let inc_cat: Uuid = income_category(&api_category_options(&test, &cookie, "income").await)
        .parse()
        .unwrap();

    for (i, amount) in ["10.00", "20.00", "30.00"].iter().enumerate() {
        post_income(
            &test,
            &cookie,
            &csrf,
            account,
            inc_cat,
            amount,
            &format!("k-page-{i}"),
        )
        .await;
    }
    // Reverse one — it must fall out of posted totals.
    let (_, incomes) = send(
        &test.app,
        req("GET", "/api/incomes?pageSize=10", &cookie, None, None),
    )
    .await;
    let first_id = incomes["items"][2]["id"].as_str().unwrap().to_string();
    reverse_entry(&test, &cookie, &csrf, "income", &first_id, "iptal").await;

    let s = summary(&test, &cookie).await;
    assert_eq!(s["incomeTotal"], "50.00");
    assert_eq!(s["incomeCount"], 2);

    // Status filter sees the reversed row.
    let (_, reversed) = send(
        &test.app,
        req(
            "GET",
            "/api/incomes?status=reversed&pageSize=10",
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(reversed["totalCount"], 1);

    // Account filter.
    let (_, filtered) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/incomes?financialAccountId={account}&pageSize=10"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(filtered["totalCount"], 3);

    // Search hits description/counterparty/reference.
    let (_, searched) = send(
        &test.app,
        req(
            "GET",
            "/api/incomes?search=geliri&pageSize=10",
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(searched["totalCount"], 3);

    // Page size bound.
    let (status, _) = send(
        &test.app,
        req("GET", "/api/incomes?pageSize=200", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn authorization_and_csrf_are_enforced() {
    let Some(test) = setup().await else { return };
    let (admin_cookie, _csrf) = admin_session(&test).await;
    let plain = format!("plain.{}", Uuid::new_v4().simple());
    create_plain_user(&test.pool, &plain).await;
    let (plain_cookie, plain_csrf) = login(&test.app, test.peer(), &plain).await;

    // Plain user has no income_expense permission.
    let (status, _) = send(
        &test.app,
        req("GET", "/api/incomes", &plain_cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/expenses",
            &plain_cookie,
            Some(&plain_csrf),
            Some(json!({
                "financialAccountId": Uuid::new_v4().to_string(),
                "categoryId": Uuid::new_v4().to_string(),
                "amount": "10.00",
                "description": "x",
                "idempotencyKey": "k-plain"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Admin without CSRF token is rejected before domain logic.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/incomes",
            &admin_cookie,
            None,
            Some(json!({
                "financialAccountId": Uuid::new_v4().to_string(),
                "categoryId": Uuid::new_v4().to_string(),
                "amount": "10.00",
                "description": "x",
                "idempotencyKey": "k-nocsrf"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Bad origin is rejected (body must deserialize so the handler's
    // CSRF prologue actually runs).
    let mut bad_origin = req(
        "POST",
        "/api/incomes",
        &admin_cookie,
        Some("x"),
        Some(json!({
            "financialAccountId": Uuid::new_v4().to_string(),
            "categoryId": Uuid::new_v4().to_string(),
            "amount": "10.00",
            "description": "x",
            "idempotencyKey": "k-origin"
        })),
    );
    bad_origin
        .headers_mut()
        .insert("origin", "http://evil.example".parse().unwrap());
    let (status, _) = send(&test.app, bad_origin).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn category_update_uses_optimistic_concurrency() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let (_, cat) = api_create_category(&test, &cookie, &csrf, "expense", "Kırtasiye").await;
    let cat_id = cat["id"].as_str().unwrap();
    let updated_at = cat["updatedAt"].as_str().unwrap().to_string();

    // Rename with the fresh precondition.
    let (status, renamed) = send(
        &test.app,
        req(
            "PATCH",
            &format!("/api/financial-categories/{cat_id}"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "name": "Kırtasiye ve Ofis",
                "expectedUpdatedAt": updated_at,
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["name"], "Kırtasiye ve Ofis");

    // Replay with the stale precondition → 409 stale_state.
    let (status, _) = send(
        &test.app,
        req(
            "PATCH",
            &format!("/api/financial-categories/{cat_id}"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "name": "Yine Başka",
                "expectedUpdatedAt": updated_at,
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

/// STEP-010 infra fix: `serde_urlencoded` cannot coerce numerics
/// through `#[serde(flatten)]` maps — list query structs inline
/// page/pageSize fields instead. Proves the STEP-008 account list
/// (which had the latent defect) and the new lists accept filters.
#[tokio::test]
async fn list_endpoints_accept_pagination_and_filters() {
    let Some(test) = setup().await else { return };
    let (cookie, _csrf) = admin_session(&test).await;
    for path in [
        "/api/financial-accounts?pageSize=20",
        "/api/financial-accounts?pageSize=20&accountType=cash&status=active",
        "/api/incomes?pageSize=10&dateFrom=2026-01-01&dateTo=2026-12-31",
        "/api/expenses?page=1&pageSize=10&status=posted",
        "/api/financial-categories?pageSize=10&categoryType=income",
    ] {
        let (status, body) = send(&test.app, req("GET", path, &cookie, None, None)).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }
}
