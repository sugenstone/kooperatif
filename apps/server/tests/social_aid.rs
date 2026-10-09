//! Database-gated Social Aid tests (STEP-013, docs/11, docs/15,
//! docs/16, docs/19). Requires KOOPERATIF_TEST_DATABASE_URL; each test
//! runs in its own throwaway database.
//!
//! Invariants under test:
//! - FUND ≠ FINANCIAL ACCOUNT: creating a fund moves zero money; a
//!   donation posts exactly ONE `social_aid_donation` inflow; an aid
//!   disbursement posts exactly ONE `social_aid_disbursement` outflow.
//! - Restricted availability is derived per (fund, account): aid can
//!   never exceed it, and restricted money cannot be spent merely
//!   because unrelated cooperative cash or another account's
//!   restricted balance exists.
//! - Donations/disbursements create NO Payment / Allocation /
//!   Shareholder Credit / Income / Expense / Transfer / Share Return /
//!   Investment rows — Social Aid is a separate financial context.
//! - Idempotent commands replay safely; concurrent disbursements can
//!   never overspend the fund or the account; posted history reverses
//!   instead of deleting, and donation reversal can never invalidate
//!   consumed restricted money or overdraw the account.

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

async fn create_person(pool: &PgPool, first: &str, last: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO persons (first_name, last_name, search_name) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(first)
    .bind(last)
    .bind(format!("{first} {last}").to_lowercase())
    .fetch_one(pool)
    .await
    .unwrap()
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

/// Operational Income is the only generic funding rail the system
/// exposes — used to place cooperative cash into an account.
async fn api_fund_account(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    account: Uuid,
    amount: &str,
    key: &str,
) {
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
    let category = list
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

async fn api_create_fund(test: &TestApp, cookie: &str, csrf: &str, name: &str, key: &str) -> Value {
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/social-aid/funds",
            cookie,
            Some(csrf),
            Some(json!({
                "name": name,
                "idempotencyKey": key,
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "fund: {detail}");
    detail
}

async fn api_donate(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    fund: &str,
    account: Uuid,
    amount: &str,
    key: &str,
) -> (StatusCode, Value) {
    send(
        &test.app,
        req(
            "POST",
            "/api/social-aid/donations",
            cookie,
            Some(csrf),
            Some(json!({
                "fundId": fund,
                "donorDisplayName": "Hayırsever A.Ş.",
                "financialAccountId": account.to_string(),
                "amount": amount,
                "occurredAt": "2026-01-10T10:00:00Z",
                "idempotencyKey": key,
            })),
        ),
    )
    .await
}

async fn api_disburse(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    fund: &str,
    account: Uuid,
    amount: &str,
    key: &str,
) -> (StatusCode, Value) {
    send(
        &test.app,
        req(
            "POST",
            "/api/social-aid/disbursements",
            cookie,
            Some(csrf),
            Some(json!({
                "fundId": fund,
                "beneficiaryDisplayName": "İhtiyaç Sahibi Aile",
                "financialAccountId": account.to_string(),
                "amount": amount,
                "occurredAt": "2026-01-12T10:00:00Z",
                "reason": "Eğitim bursu ödemesi",
                "idempotencyKey": key,
            })),
        ),
    )
    .await
}

async fn api_fund_detail(test: &TestApp, cookie: &str, fund: &str) -> Value {
    let (status, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/social-aid/funds/{fund}"),
            cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail
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
async fn create_fund_moves_no_money() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Sosyal Yardım Kasası").await;
    api_fund_account(&test, &cookie, &csrf, account, "50000.00", "fund-1").await;
    let before = api_movement_count(&test, &cookie, account).await;

    let detail = api_create_fund(&test, &cookie, &csrf, "Eğitim Yardımı", "f-1").await;
    assert_eq!(detail["status"], "active");
    assert_eq!(detail["totalDonated"], "0.00");
    assert_eq!(detail["totalDisbursed"], "0.00");
    assert_eq!(detail["available"], "0.00");
    assert_eq!(detail["accounts"].as_array().unwrap().len(), 0);

    // Creating fund metadata invented ZERO money.
    assert_eq!(api_balance(&test, &cookie, account).await, "50000.00");
    assert_eq!(api_movement_count(&test, &cookie, account).await, before);
}

#[tokio::test]
async fn donation_posts_exactly_one_inflow_without_cooperative_effects() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Yardım Banka").await;
    let fund = api_create_fund(&test, &cookie, &csrf, "Gıda Yardımı", "f-1").await;
    let fund_id = fund["id"].as_str().unwrap();

    let (status, donation) =
        api_donate(&test, &cookie, &csrf, fund_id, account, "20000.00", "d-1").await;
    assert_eq!(status, StatusCode::CREATED, "{donation}");
    assert_eq!(donation["status"], "posted");
    assert_eq!(donation["donorDisplayName"], "Hayırsever A.Ş.");
    assert_eq!(donation["movementStatus"], "active");

    // Physical account balance grows — real money entered.
    assert_eq!(api_balance(&test, &cookie, account).await, "20000.00");

    // Exactly ONE authoritative inflow with donation provenance.
    let movements: Vec<(String, String)> = sqlx::query_as(
        "SELECT source_type, direction FROM account_movements \
         WHERE account_id = $1 AND status = 'active'",
    )
    .bind(account)
    .fetch_all(&test.pool)
    .await
    .unwrap();
    assert_eq!(movements.len(), 1);
    assert_eq!(movements[0].0, "social_aid_donation");
    assert_eq!(movements[0].1, "inflow");

    // Restricted availability grows for this fund in this account.
    let detail = api_fund_detail(&test, &cookie, fund_id).await;
    assert_eq!(detail["available"], "20000.00");
    assert_eq!(detail["totalDonated"], "20000.00");
    assert_eq!(detail["accounts"][0]["available"], "20000.00");
    assert_eq!(detail["accounts"][0]["physicalBalance"], "20000.00");

    // A donation is NOT Payment / Allocation / Credit / Income /
    // Transfer / Share Return / Investment (docs/11, docs/15).
    assert_eq!(table_count(&test.pool, "payments").await, 0);
    assert_eq!(table_count(&test.pool, "payment_allocations").await, 0);
    assert_eq!(table_count(&test.pool, "shareholder_credits").await, 0);
    assert_eq!(table_count(&test.pool, "income_entries").await, 0);
    assert_eq!(table_count(&test.pool, "account_transfers").await, 0);
    assert_eq!(table_count(&test.pool, "share_returns").await, 0);
    assert_eq!(table_count(&test.pool, "investments").await, 0);
}

#[tokio::test]
async fn donor_and_beneficiary_accept_person_or_external_identity() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let fund = api_create_fund(&test, &cookie, &csrf, "Genel Yardım", "f-1").await;
    let fund_id = fund["id"].as_str().unwrap();

    // Person-linked donor: a canonical Person — NOT a shareholder,
    // membership never required.
    let person = create_person(&test.pool, "Ayşe", "Yılmaz").await;
    let (status, donation) = send(
        &test.app,
        req(
            "POST",
            "/api/social-aid/donations",
            &cookie,
            Some(&csrf),
            Some(json!({
                "fundId": fund_id,
                "donorPersonId": person.to_string(),
                "financialAccountId": account.to_string(),
                "amount": "5000.00",
                "occurredAt": "2026-01-10T10:00:00Z",
                "idempotencyKey": "d-person",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{donation}");
    assert_eq!(donation["donorPersonId"], person.to_string());
    assert_eq!(donation["donorName"], "Ayşe Yılmaz");

    // Person-linked beneficiary: external person, never gains
    // membership — the disbursement carries only the person link.
    let beneficiary = create_person(&test.pool, "Mehmet", "Demir").await;
    let (status, disbursement) = send(
        &test.app,
        req(
            "POST",
            "/api/social-aid/disbursements",
            &cookie,
            Some(&csrf),
            Some(json!({
                "fundId": fund_id,
                "beneficiaryPersonId": beneficiary.to_string(),
                "financialAccountId": account.to_string(),
                "amount": "3000.00",
                "occurredAt": "2026-01-12T10:00:00Z",
                "reason": "Kira desteği",
                "idempotencyKey": "b-person",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{disbursement}");
    assert_eq!(disbursement["beneficiaryPersonId"], beneficiary.to_string());
    assert_eq!(disbursement["beneficiaryName"], "Mehmet Demir");
    // No cooperative membership was created for either person.
    assert_eq!(table_count(&test.pool, "shareholders").await, 0);

    // Anonymous recording is not silently allowed: no identity -> 400.
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/social-aid/donations",
            &cookie,
            Some(&csrf),
            Some(json!({
                "fundId": fund_id,
                "financialAccountId": account.to_string(),
                "amount": "100.00",
                "occurredAt": "2026-01-10T10:00:00Z",
                "idempotencyKey": "d-anon",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{detail}");

    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/social-aid/disbursements",
            &cookie,
            Some(&csrf),
            Some(json!({
                "fundId": fund_id,
                "financialAccountId": account.to_string(),
                "amount": "100.00",
                "occurredAt": "2026-01-12T10:00:00Z",
                "reason": "x",
                "idempotencyKey": "b-anon",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{detail}");

    // A dangling person reference is a 404, not a silent insert.
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/social-aid/donations",
            &cookie,
            Some(&csrf),
            Some(json!({
                "fundId": fund_id,
                "donorPersonId": Uuid::new_v4().to_string(),
                "financialAccountId": account.to_string(),
                "amount": "100.00",
                "occurredAt": "2026-01-10T10:00:00Z",
                "idempotencyKey": "d-ghost",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{detail}");
}

#[tokio::test]
async fn disbursement_posts_one_outflow_and_reduces_restricted_availability() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Yardım Kasa").await;
    let fund = api_create_fund(&test, &cookie, &csrf, "Acil Destek", "f-1").await;
    let fund_id = fund["id"].as_str().unwrap();
    let (status, _) = api_donate(&test, &cookie, &csrf, fund_id, account, "8000.00", "d-1").await;
    assert_eq!(status, StatusCode::CREATED);

    let expense_count = table_count(&test.pool, "expense_entries").await;
    let (status, disbursement) =
        api_disburse(&test, &cookie, &csrf, fund_id, account, "5000.00", "b-1").await;
    assert_eq!(status, StatusCode::CREATED, "{disbursement}");
    assert_eq!(disbursement["status"], "posted");
    assert_eq!(disbursement["reason"], "Eğitim bursu ödemesi");
    assert_eq!(disbursement["movementStatus"], "active");

    assert_eq!(api_balance(&test, &cookie, account).await, "3000.00");

    let movements: Vec<(String, String)> = sqlx::query_as(
        "SELECT source_type, direction FROM account_movements \
         WHERE account_id = $1 AND status = 'active' ORDER BY occurred_at",
    )
    .bind(account)
    .fetch_all(&test.pool)
    .await
    .unwrap();
    assert_eq!(movements.len(), 2);
    assert_eq!(
        movements[0],
        ("social_aid_donation".into(), "inflow".into())
    );
    assert_eq!(
        movements[1],
        ("social_aid_disbursement".into(), "outflow".into())
    );

    let detail = api_fund_detail(&test, &cookie, fund_id).await;
    assert_eq!(detail["available"], "3000.00");
    assert_eq!(detail["totalDisbursed"], "5000.00");

    // Aid is NOT operational Expense.
    assert_eq!(
        table_count(&test.pool, "expense_entries").await,
        expense_count
    );
    assert_eq!(table_count(&test.pool, "payments").await, 0);
    assert_eq!(table_count(&test.pool, "account_transfers").await, 0);
}

#[tokio::test]
async fn disbursement_can_never_exceed_restricted_availability() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa A").await;
    // Unrelated cooperative cash sits in the SAME account — restricted
    // money must never be spendable "just because" it is there.
    api_fund_account(&test, &cookie, &csrf, account, "100000.00", "income-1").await;
    let fund = api_create_fund(&test, &cookie, &csrf, "Burs Fonu", "f-1").await;
    let fund_id = fund["id"].as_str().unwrap();
    let (status, _) = api_donate(&test, &cookie, &csrf, fund_id, account, "20000.00", "d-1").await;
    assert_eq!(status, StatusCode::CREATED);

    // Fund available 20,000; account physical 120,000 -> 30,000 aid REJECTED.
    let (status, detail) = api_disburse(
        &test, &cookie, &csrf, fund_id, account, "30000.00", "b-over",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{detail}");

    // And restricted money never lands in a different account: a
    // second account with plenty of cash but no fund credit rejects.
    let account_b = api_create_account(&test, &cookie, &csrf, "Kasa B").await;
    api_fund_account(&test, &cookie, &csrf, account_b, "50000.00", "income-2").await;
    let (status, detail) = api_disburse(
        &test, &cookie, &csrf, fund_id, account_b, "10000.00", "b-cross",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{detail}");

    // A different fund cannot spend this fund's restriction either.
    let other_fund = api_create_fund(&test, &cookie, &csrf, "Sağlık Fonu", "f-2").await;
    let (status, detail) = api_disburse(
        &test,
        &cookie,
        &csrf,
        other_fund["id"].as_str().unwrap(),
        account,
        "1000.00",
        "b-other",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{detail}");

    // Nothing moved.
    assert_eq!(api_balance(&test, &cookie, account).await, "120000.00");
    assert_eq!(api_balance(&test, &cookie, account_b).await, "50000.00");
    let detail = api_fund_detail(&test, &cookie, fund_id).await;
    assert_eq!(detail["available"], "20000.00");
}

#[tokio::test]
async fn disbursement_can_never_exceed_physical_account_balance() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Dar Kasa").await;
    let fund = api_create_fund(&test, &cookie, &csrf, "Fon", "f-1").await;
    let fund_id = fund["id"].as_str().unwrap();
    // Restricted availability 20,000 in the account plus 5,000 of
    // unrestricted cooperative cash.
    let (status, _) = api_donate(&test, &cookie, &csrf, fund_id, account, "20000.00", "d-1").await;
    assert_eq!(status, StatusCode::CREATED);
    api_fund_account(&test, &cookie, &csrf, account, "5000.00", "income-1").await;
    let (status, categories) = send(
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
    assert_eq!(status, StatusCode::OK);
    let category = categories
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Diğer Gider")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    // PILOT-FIX-001 hard reservation: an operational expense may only
    // consume the UNRESTRICTED 5,000 — the restricted 20,000 can never
    // be drained by an ordinary outflow.
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/expenses",
            &cookie,
            Some(&csrf),
            Some(json!({
                "financialAccountId": account.to_string(),
                "categoryId": category,
                "amount": "20000.00",
                "description": "ordinary expense",
                "idempotencyKey": "exp-1",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{detail}");
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/expenses",
            &cookie,
            Some(&csrf),
            Some(json!({
                "financialAccountId": account.to_string(),
                "categoryId": category,
                "amount": "5000.00",
                "description": "ordinary expense",
                "idempotencyKey": "exp-2",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");
    assert_eq!(api_balance(&test, &cookie, account).await, "20000.00");

    // The account now holds ONLY restricted cash (physical 20,000 =
    // reserved 20,000). An authorized disbursement consumes both
    // dimensions atomically; a disbursement beyond the physically
    // backed restricted remainder is still rejected.
    let (status, detail) =
        api_disburse(&test, &cookie, &csrf, fund_id, account, "15000.00", "b-1").await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");
    assert_eq!(api_balance(&test, &cookie, account).await, "5000.00");
    let (status, detail) =
        api_disburse(&test, &cookie, &csrf, fund_id, account, "10000.00", "b-2").await;
    assert_eq!(status, StatusCode::CONFLICT, "{detail}");
    assert_eq!(api_balance(&test, &cookie, account).await, "5000.00");
}

#[tokio::test]
async fn idempotent_replay_and_conflicting_key() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let fund = api_create_fund(&test, &cookie, &csrf, "Fon", "f-1").await;
    let fund_id = fund["id"].as_str().unwrap();

    // Fund creation replay: same key + same payload -> same fund.
    let replayed = api_create_fund_replay(&test, &cookie, &csrf, "Fon", "f-1").await;
    assert_eq!(replayed["fundNumber"], fund["fundNumber"]);
    assert_eq!(table_count(&test.pool, "social_aid_funds").await, 1);

    let (status, first) =
        api_donate(&test, &cookie, &csrf, fund_id, account, "7000.00", "d-1").await;
    assert_eq!(status, StatusCode::CREATED);
    // Identical replay returns the ORIGINAL donation — one movement.
    let (status, replay) =
        api_donate(&test, &cookie, &csrf, fund_id, account, "7000.00", "d-1").await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["id"], first["id"]);
    assert_eq!(api_movement_count(&test, &cookie, account).await, 1);
    assert_eq!(api_balance(&test, &cookie, account).await, "7000.00");

    // Same key, different payload -> 409, no money moved.
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/social-aid/donations",
            &cookie,
            Some(&csrf),
            Some(json!({
                "fundId": fund_id,
                "donorDisplayName": "Hayırsever A.Ş.",
                "financialAccountId": account.to_string(),
                "amount": "999.00",
                "occurredAt": "2026-01-10T10:00:00Z",
                "idempotencyKey": "d-1",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{detail}");
    assert_eq!(api_balance(&test, &cookie, account).await, "7000.00");

    // Disbursement replay is symmetric.
    let (status, first) =
        api_disburse(&test, &cookie, &csrf, fund_id, account, "2000.00", "b-1").await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, replay) =
        api_disburse(&test, &cookie, &csrf, fund_id, account, "2000.00", "b-1").await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["id"], first["id"]);
    assert_eq!(api_movement_count(&test, &cookie, account).await, 2);
}

async fn api_create_fund_replay(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    name: &str,
    key: &str,
) -> Value {
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/social-aid/funds",
            cookie,
            Some(csrf),
            Some(json!({ "name": name, "idempotencyKey": key })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail
}

#[tokio::test]
async fn reversals_restore_and_preserve_history() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let fund = api_create_fund(&test, &cookie, &csrf, "Fon", "f-1").await;
    let fund_id = fund["id"].as_str().unwrap();
    let (status, donation) =
        api_donate(&test, &cookie, &csrf, fund_id, account, "10000.00", "d-1").await;
    assert_eq!(status, StatusCode::CREATED);
    let donation_id = donation["id"].as_str().unwrap();
    let (status, disbursement) =
        api_disburse(&test, &cookie, &csrf, fund_id, account, "4000.00", "b-1").await;
    assert_eq!(status, StatusCode::CREATED);
    let disbursement_id = disbursement["id"].as_str().unwrap();

    // Aid reversal restores physical cash AND restricted availability.
    let (status, reversed) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/social-aid/disbursements/{disbursement_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "Yanlış tutar" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reversed}");
    assert_eq!(reversed["status"], "reversed");
    assert_eq!(reversed["movementStatus"], "reversed");
    assert_eq!(api_balance(&test, &cookie, account).await, "10000.00");
    let detail = api_fund_detail(&test, &cookie, fund_id).await;
    assert_eq!(detail["available"], "10000.00");
    // The reversed rows remain in history — never deleted.
    assert_eq!(detail["disbursements"].as_array().unwrap().len(), 1);

    // Donation reversal removes the inflow under both bounds.
    let (status, reversed) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/social-aid/donations/{donation_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "Hatalı giriş" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reversed}");
    assert_eq!(reversed["status"], "reversed");
    assert_eq!(api_balance(&test, &cookie, account).await, "0.00");
    let detail = api_fund_detail(&test, &cookie, fund_id).await;
    assert_eq!(detail["available"], "0.00");
    assert_eq!(detail["donations"].as_array().unwrap().len(), 1);

    // Reversal is idempotent at the status level — a second reverse
    // replays the stored record instead of double-correcting.
    let (status, replay) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/social-aid/donations/{donation_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "Tekrar" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["status"], "reversed");
    assert_eq!(api_balance(&test, &cookie, account).await, "0.00");
}

#[tokio::test]
async fn donation_reversal_cannot_invalidate_consumed_restricted_money() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let fund = api_create_fund(&test, &cookie, &csrf, "Fon", "f-1").await;
    let fund_id = fund["id"].as_str().unwrap();
    let (status, donation) =
        api_donate(&test, &cookie, &csrf, fund_id, account, "10000.00", "d-1").await;
    assert_eq!(status, StatusCode::CREATED);
    let donation_id = donation["id"].as_str().unwrap();
    // Spend 8,000 of the restricted money — only 2,000 remains.
    let (status, _) = api_disburse(&test, &cookie, &csrf, fund_id, account, "8000.00", "b-1").await;
    assert_eq!(status, StatusCode::CREATED);

    // Reversing the whole 10,000 donation would invalidate consumed
    // restricted money -> rejected (docs/11, docs/19).
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/social-aid/donations/{donation_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "İade" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{detail}");
    assert_eq!(api_balance(&test, &cookie, account).await, "2000.00");
}

#[tokio::test]
async fn fund_lifecycle_close_requires_zero_availability() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let fund = api_create_fund(&test, &cookie, &csrf, "Kapanacak Fon", "f-1").await;
    let fund_id = fund["id"].as_str().unwrap();
    let (status, _) = api_donate(&test, &cookie, &csrf, fund_id, account, "6000.00", "d-1").await;
    assert_eq!(status, StatusCode::CREATED);

    // Close refused while restricted availability remains — money must
    // never become ownerless.
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/social-aid/funds/{fund_id}/close"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{detail}");

    // Cancel refused once financial events exist.
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/social-aid/funds/{fund_id}/cancel"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "Yanlış açıldı" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{detail}");

    // Spend it all, then close succeeds and blocks further postings.
    let (status, _) = api_disburse(&test, &cookie, &csrf, fund_id, account, "6000.00", "b-1").await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/social-aid/funds/{fund_id}/close"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["status"], "closed");
    let (status, detail) =
        api_donate(&test, &cookie, &csrf, fund_id, account, "100.00", "d-2").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{detail}");

    // A never-used fund can be cancelled.
    let empty = api_create_fund(&test, &cookie, &csrf, "Boş Fon", "f-2").await;
    let empty_id = empty["id"].as_str().unwrap();
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/social-aid/funds/{empty_id}/cancel"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "Yanlış açıldı" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["status"], "cancelled");
}

#[tokio::test]
async fn concurrent_disbursements_cannot_overspend_fund() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let fund = api_create_fund(&test, &cookie, &csrf, "Yarış Fonu", "f-1").await;
    let fund_id = fund["id"].as_str().unwrap().to_string();
    let (status, _) = api_donate(&test, &cookie, &csrf, &fund_id, account, "10000.00", "d-1").await;
    assert_eq!(status, StatusCode::CREATED);

    // Two parallel disbursements of 7,000 each — only one can win.
    let app = test.app.clone();
    let cookie_a = cookie.clone();
    let csrf_a = csrf.clone();
    let fund_a = fund_id.clone();
    let first = tokio::spawn(async move {
        send(
            &app,
            req(
                "POST",
                "/api/social-aid/disbursements",
                &cookie_a,
                Some(&csrf_a),
                Some(json!({
                    "fundId": fund_a,
                    "beneficiaryDisplayName": "A",
                    "financialAccountId": account.to_string(),
                    "amount": "7000.00",
                    "occurredAt": "2026-01-12T10:00:00Z",
                    "reason": "r",
                    "idempotencyKey": "race-a",
                })),
            ),
        )
        .await
        .0
    });
    let app = test.app.clone();
    let fund_b = fund_id.clone();
    let cookie_b = cookie.clone();
    let csrf_b = csrf.clone();
    let second = tokio::spawn(async move {
        send(
            &app,
            req(
                "POST",
                "/api/social-aid/disbursements",
                &cookie_b,
                Some(&csrf_b),
                Some(json!({
                    "fundId": fund_b,
                    "beneficiaryDisplayName": "B",
                    "financialAccountId": account.to_string(),
                    "amount": "7000.00",
                    "occurredAt": "2026-01-12T10:00:00Z",
                    "reason": "r",
                    "idempotencyKey": "race-b",
                })),
            ),
        )
        .await
        .0
    });
    let (a, b) = tokio::join!(first, second);
    let statuses = [a.unwrap(), b.unwrap()];
    assert!(
        statuses.contains(&StatusCode::CREATED) && statuses.contains(&StatusCode::CONFLICT),
        "exactly one wins: {statuses:?}"
    );
    assert_eq!(api_balance(&test, &cookie, account).await, "3000.00");
    let detail = api_fund_detail(&test, &cookie, &fund_id).await;
    assert_eq!(detail["available"], "3000.00");
}

#[tokio::test]
async fn authorization_csrf_and_no_store_are_enforced() {
    let Some(test) = setup().await else { return };
    let (admin_cookie, admin_csrf) = admin_session(&test).await;
    let fund = api_create_fund(&test, &admin_cookie, &admin_csrf, "Fon", "f-1").await;
    let fund_id = fund["id"].as_str().unwrap();

    // Unauthenticated -> 401.
    let (status, _) = send(
        &test.app,
        req("GET", "/api/social-aid/funds", "", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Plain user without permissions -> 403 on read AND mutation.
    let plain_name = format!("plain.{}", Uuid::new_v4().simple());
    create_plain_user(&test.pool, &plain_name).await;
    let (cookie, csrf) = login(&test.app, test.peer(), &plain_name).await;
    let (status, _) = send(
        &test.app,
        req("GET", "/api/social-aid/funds", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/social-aid/donations",
            &cookie,
            Some(&csrf),
            Some(json!({
                "fundId": fund_id,
                "donorDisplayName": "X",
                "financialAccountId": Uuid::new_v4().to_string(),
                "amount": "10.00",
                "occurredAt": "2026-01-10T10:00:00Z",
                "idempotencyKey": "denied-1",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{detail}");

    // Person lookup is scoped to social_aid.manage — a social-aid
    // operator resolves the canonical Person WITHOUT parties rights.
    let (status, _) = send(
        &test.app,
        req("GET", "/api/social-aid/persons?search=a", "", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(
        &test.app,
        req(
            "GET",
            "/api/social-aid/persons?search=a",
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, people) = send(
        &test.app,
        req(
            "GET",
            "/api/social-aid/persons?search=a",
            &admin_cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{people}");

    // Missing CSRF token -> rejected even for the admin.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/social-aid/funds",
            &admin_cookie,
            None,
            Some(json!({ "name": "X", "idempotencyKey": "csrf-1" })),
        ),
    )
    .await;
    assert!(matches!(
        status,
        StatusCode::FORBIDDEN | StatusCode::BAD_REQUEST
    ));

    // Sensitive list endpoint answers no-store.
    let response = test
        .app
        .clone()
        .oneshot(req(
            "GET",
            "/api/social-aid/donations",
            &admin_cookie,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
}

#[tokio::test]
async fn register_lists_and_validation() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let fund_a = api_create_fund(&test, &cookie, &csrf, "A Fonu", "f-1").await;
    let fund_b = api_create_fund(&test, &cookie, &csrf, "B Fonu", "f-2").await;
    let fund_a_id = fund_a["id"].as_str().unwrap();
    let fund_b_id = fund_b["id"].as_str().unwrap();
    let (status, _) = api_donate(&test, &cookie, &csrf, fund_a_id, account, "4000.00", "d-1").await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = api_donate(&test, &cookie, &csrf, fund_b_id, account, "1000.00", "d-2").await;
    assert_eq!(status, StatusCode::CREATED);

    // Fund list derives totals without stored balance columns.
    let (status, list) = send(
        &test.app,
        req(
            "GET",
            "/api/social-aid/funds?pageSize=10",
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["totalCount"], 2);
    assert_eq!(list["items"][0]["fundNumber"], 2);
    assert_eq!(list["items"][0]["available"], "1000.00");

    let (status, filtered) = send(
        &test.app,
        req(
            "GET",
            "/api/social-aid/funds?search=A%20Fonu&status=active",
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(filtered["totalCount"], 1);

    // Donation register filtered by fund.
    let (status, donations) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/social-aid/donations?fundId={fund_a_id}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(donations["totalCount"], 1);
    assert_eq!(donations["items"][0]["amount"], "4000.00");

    // Validation: bad status filter, inverted window, future
    // occurred_at and zero amount all fail closed.
    let (status, _) = send(
        &test.app,
        req(
            "GET",
            "/api/social-aid/funds?status=gold",
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/social-aid/funds",
            &cookie,
            Some(&csrf),
            Some(json!({
                "name": "Ters", "startsOn": "2026-01-10",
                "endsOn": "2026-01-01", "idempotencyKey": "f-bad",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{detail}");
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/social-aid/donations",
            &cookie,
            Some(&csrf),
            Some(json!({
                "fundId": fund_a_id,
                "donorDisplayName": "X",
                "financialAccountId": account.to_string(),
                "amount": "0.00",
                "occurredAt": "2026-01-10T10:00:00Z",
                "idempotencyKey": "d-zero",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{detail}");
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/social-aid/donations",
            &cookie,
            Some(&csrf),
            Some(json!({
                "fundId": fund_a_id,
                "donorDisplayName": "X",
                "financialAccountId": account.to_string(),
                "amount": "10.00",
                "occurredAt": "2026-02-01T10:00:00Z",
                "idempotencyKey": "d-future",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{detail}");
}

#[tokio::test]
async fn inactive_account_and_inactive_fund_reject_postings() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let fund = api_create_fund(&test, &cookie, &csrf, "Fon", "f-1").await;
    let fund_id = fund["id"].as_str().unwrap();

    // Deactivate the account -> donations and disbursements refuse.
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
    let (status, detail) =
        api_donate(&test, &cookie, &csrf, fund_id, account, "100.00", "d-1").await;
    assert!(
        matches!(status, StatusCode::BAD_REQUEST | StatusCode::CONFLICT),
        "{detail}"
    );
}
