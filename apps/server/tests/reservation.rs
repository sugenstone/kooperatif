//! Database-gated HARD RESERVATION tests (PILOT-FIX-001 F1, docs/11,
//! docs/15, ADR-006). Requires KOOPERATIF_TEST_DATABASE_URL; each test
//! runs in its own throwaway database.
//!
//! Invariant under test (user-approved policy):
//!   for every financial account A:
//!       reserved(A) = sum(posted donations - posted disbursements
//!                     attributed to A, all funds)
//!       physical(A) >= reserved(A)    — always
//!       ordinary_outflow <= physical(A) - reserved(A)
//!
//! Ordinary outflows (transfer-out, expense, settlement, investment
//! funding) and inflow-removing reversals (payment, income,
//! investment-income, transfer destination leg) must never consume
//! Social Aid reservations. Authorized aid disbursements consume both
//! ledgers atomically and are unaffected.

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

async fn api_category(test: &TestApp, cookie: &str, kind: &str) -> String {
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
    assert_eq!(status, StatusCode::OK);
    list.as_array().unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn api_income(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    account: Uuid,
    amount: &str,
    key: &str,
) -> (StatusCode, Value) {
    let category = api_category(test, cookie, "income").await;
    send(
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
    .await
}

async fn api_expense(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    account: Uuid,
    amount: &str,
    key: &str,
) -> (StatusCode, Value) {
    let category = api_category(test, cookie, "expense").await;
    send(
        &test.app,
        req(
            "POST",
            "/api/expenses",
            cookie,
            Some(csrf),
            Some(json!({
                "financialAccountId": account.to_string(),
                "categoryId": category,
                "amount": amount,
                "description": "spend",
                "idempotencyKey": key,
            })),
        ),
    )
    .await
}

async fn api_transfer(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    source: Uuid,
    destination: Uuid,
    amount: &str,
    key: &str,
) -> (StatusCode, Value) {
    send(
        &test.app,
        req(
            "POST",
            "/api/account-transfers",
            cookie,
            Some(csrf),
            Some(json!({
                "sourceAccountId": source.to_string(),
                "destinationAccountId": destination.to_string(),
                "amount": amount,
                "occurredAt": "2026-01-14T10:00:00Z",
                "idempotencyKey": key,
            })),
        ),
    )
    .await
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
            Some(json!({ "name": name, "idempotencyKey": key })),
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

/// SQL-level derived reservation for one account — the same formula
/// the guard uses (posted donations - posted disbursements).
async fn sql_reserved(pool: &PgPool, account: Uuid) -> rust_decimal::Decimal {
    sqlx::query_scalar::<_, Option<rust_decimal::Decimal>>(
        "SELECT \
            COALESCE((SELECT sum(amount) FROM social_aid_donations \
                      WHERE financial_account_id = $1 AND status = 'posted'), 0) \
          - COALESCE((SELECT sum(amount) FROM social_aid_disbursements \
                      WHERE financial_account_id = $1 AND status = 'posted'), 0)",
    )
    .bind(account)
    .fetch_one(pool)
    .await
    .unwrap()
    .unwrap_or_default()
}

/// Fixture: account funded with `physical` ordinary cash plus `reserved`
/// donation-backed Social Aid cash.
async fn reserved_fixture(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    physical: &str,
    reserved: &str,
    tag: &str,
) -> (Uuid, Value) {
    let account = api_create_account(test, cookie, csrf, &format!("Kasa {tag}")).await;
    if physical != "0.00" {
        let (status, detail) =
            api_income(test, cookie, csrf, account, physical, &format!("inc-{tag}")).await;
        assert_eq!(status, StatusCode::CREATED, "funding: {detail}");
    }
    let fund = api_create_fund(
        test,
        cookie,
        csrf,
        &format!("Fon {tag}"),
        &format!("f-{tag}"),
    )
    .await;
    let fund_id = fund["id"].as_str().unwrap().to_string();
    if reserved != "0.00" {
        let (status, donation) = api_donate(
            test,
            cookie,
            csrf,
            &fund_id,
            account,
            reserved,
            &format!("d-{tag}"),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "donation: {donation}");
    }
    (account, fund)
}

fn assert_unrestricted_rejected(status: StatusCode, body: &Value) {
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        body["error"]["code"], "insufficient_unrestricted_funds",
        "{body}"
    );
}

// ---------------------------------------------------------------------
// Boundary: ordinary outflows vs the reservation
// ---------------------------------------------------------------------

#[tokio::test]
async fn no_restricted_funds_ordinary_operations_unchanged() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Serbest Kasa").await;
    let (status, _) = api_income(&test, &cookie, &csrf, account, "10000.00", "i1").await;
    assert_eq!(status, StatusCode::CREATED);

    // Full-balance expense is still legal when nothing is reserved.
    let (status, detail) = api_expense(&test, &cookie, &csrf, account, "10000.00", "e1").await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");
    assert_eq!(api_balance(&test, &cookie, account).await, "0.00");
}

#[tokio::test]
async fn expense_boundary_at_reservation() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    // physical 10,000 = 5,000 unrestricted income + 5,000 restricted.
    let (account, _fund) =
        reserved_fixture(&test, &cookie, &csrf, "5000.00", "5000.00", "b1").await;

    let (status, detail) = api_expense(&test, &cookie, &csrf, account, "5000.00", "e-ok").await;
    assert_eq!(status, StatusCode::CREATED, "spendable boundary: {detail}");

    let (status, detail) = api_expense(&test, &cookie, &csrf, account, "0.01", "e-over").await;
    assert_unrestricted_rejected(status, &detail);
    assert_eq!(api_balance(&test, &cookie, account).await, "5000.00");
    assert_eq!(
        sql_reserved(&test.pool, account).await.to_string(),
        "5000.00"
    );
}

#[tokio::test]
async fn transfer_boundary_at_reservation() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let (source, _fund) = reserved_fixture(&test, &cookie, &csrf, "5000.00", "5000.00", "t1").await;
    let destination = api_create_account(&test, &cookie, &csrf, "Hedef").await;

    let (status, detail) = api_transfer(
        &test,
        &cookie,
        &csrf,
        source,
        destination,
        "5000.00",
        "tr-ok",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "boundary transfer: {detail}");

    let (status, detail) = api_transfer(
        &test,
        &cookie,
        &csrf,
        source,
        destination,
        "0.01",
        "tr-over",
    )
    .await;
    assert_unrestricted_rejected(status, &detail);
    assert_eq!(api_balance(&test, &cookie, source).await, "5000.00");
    assert_eq!(api_balance(&test, &cookie, destination).await, "5000.00");
}

// ---------------------------------------------------------------------
// Reversals removing an inflow must respect the reservation
// ---------------------------------------------------------------------

#[tokio::test]
async fn payment_reversal_blocked_when_it_would_unback_reservation() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let (account, _fund) =
        reserved_fixture(&test, &cookie, &csrf, "5000.00", "5000.00", "p1").await;

    // Unrelated 7,000 payment lands in the same account (physical 17,000).
    let (status, created) = send(
        &test.app,
        req(
            "POST",
            "/api/payments",
            &cookie,
            Some(&csrf),
            Some(json!({
                "payerFirstName": "Ödeyen",
                "payerLastName": "Kişi",
                "amount": "7000.00",
                "method": "cash",
                "destinationAccountId": account.to_string(),
                "idempotencyKey": "pay-1",
                "allocations": [],
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let payment_id = created["payment"]["id"].as_str().unwrap();

    // Ordinary outflow drains unrestricted cash to exactly the
    // reservation (physical 15,000 = reserved 5,000 + spent 10,000? no —
    // spend 10,000 of the unrestricted 12,000, leaving 2,000 free).
    let (status, detail) = api_expense(&test, &cookie, &csrf, account, "10000.00", "e-drain").await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");
    // physical 7,000, reserved 5,000, spendable 2,000.

    // Reversing the 7,000 payment would leave physical 0 < reserved
    // 5,000 — MUST be rejected even though balance >= 0 is not violated
    // at a glance (7,000 - 7,000 = 0 >= 0).
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/payments/{payment_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "hatalı tahsilat" })),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "reversal must be blocked: {detail}"
    );

    // Payment stays posted; ledger untouched.
    let (status, payment) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/payments/{payment_id}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payment["status"], "posted");
    assert_eq!(api_balance(&test, &cookie, account).await, "7000.00");
}

#[tokio::test]
async fn income_reversal_blocked_when_it_would_unback_reservation() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let (account, _fund) =
        reserved_fixture(&test, &cookie, &csrf, "5000.00", "5000.00", "ir1").await;

    // A second, unrelated income raises physical to 17,000.
    let (status, income) = api_income(&test, &cookie, &csrf, account, "7000.00", "inc-2").await;
    assert_eq!(status, StatusCode::CREATED, "{income}");
    let income_id = income["id"].as_str().unwrap();

    // Drain unrestricted to the reservation floor: physical 10,000,
    // reserved 5,000, spendable 5,000.
    let (status, detail) = api_expense(&test, &cookie, &csrf, account, "7000.00", "e-drain2").await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");
    assert_eq!(api_balance(&test, &cookie, account).await, "10000.00");

    // Reversing the income would leave 10,000 - 7,000 = 3,000 physical
    // — non-negative, but BELOW the 5,000 reservation: the ordinary
    // balance check passes, only the reservation guard can reject.
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/incomes/{income_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "düzeltme" })),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "income reversal blocked: {detail}"
    );
}

// ---------------------------------------------------------------------
// Other ordinary outflow domains
// ---------------------------------------------------------------------

#[tokio::test]
async fn investment_funding_cannot_consume_reserved_cash() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let (account, _fund) =
        reserved_fixture(&test, &cookie, &csrf, "5000.00", "5000.00", "iv1").await;

    let (status, investment) = send(
        &test.app,
        req(
            "POST",
            "/api/investments",
            &cookie,
            Some(&csrf),
            Some(json!({
                "name": "Arsa Yatırımı",
                "investmentType": "real_estate",
                "acquiredAt": "2026-01-05",
                "idempotencyKey": "inv-1",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{investment}");
    let investment_id = investment["id"].as_str().unwrap();

    // 6,000 funding exceeds spendable (5,000) though within physical.
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/investments/{investment_id}/fundings"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "financialAccountId": account.to_string(),
                "amount": "6000.00",
                "occurredAt": "2026-01-10T10:00:00Z",
                "idempotencyKey": "f-over",
            })),
        ),
    )
    .await;
    assert_unrestricted_rejected(status, &detail);

    // Boundary: 5,000 exactly is legal.
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/investments/{investment_id}/fundings"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "financialAccountId": account.to_string(),
                "amount": "5000.00",
                "occurredAt": "2026-01-10T10:00:00Z",
                "idempotencyKey": "f-ok",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "boundary funding: {detail}");
}

#[tokio::test]
async fn investment_income_reversal_blocked_by_reservation() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let (account, _fund) =
        reserved_fixture(&test, &cookie, &csrf, "5000.00", "5000.00", "ivr").await;

    let (status, investment) = send(
        &test.app,
        req(
            "POST",
            "/api/investments",
            &cookie,
            Some(&csrf),
            Some(json!({
                "name": "İşletme",
                "investmentType": "business",
                "acquiredAt": "2026-01-05",
                "idempotencyKey": "inv-ivr",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{investment}");
    let investment_id = investment["id"].as_str().unwrap();

    // Investment income (inflow) raises physical to 15,000.
    let (status, income) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/investments/{investment_id}/incomes"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "financialAccountId": account.to_string(),
                "amount": "5000.00",
                "occurredAt": "2026-01-11T10:00:00Z",
                "description": "Kira tahsilatı",
                "idempotencyKey": "ivi-1",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{income}");
    let income_id = income["incomes"][0]["id"].as_str().unwrap();

    // Drain unrestricted: physical 15,000 -> expense 10,000 -> 5,000.
    let (status, detail) = api_expense(&test, &cookie, &csrf, account, "10000.00", "e-ivd").await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");
    assert_eq!(api_balance(&test, &cookie, account).await, "5000.00");

    // Reversing the 5,000 investment income -> physical 0 < reserved.
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/investment-incomes/{income_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "düzeltme" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "reversal blocked: {detail}");
}

#[tokio::test]
async fn share_return_settlement_cannot_consume_reserved_cash() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let (account, _fund) =
        reserved_fixture(&test, &cookie, &csrf, "5000.00", "5000.00", "sr1").await;

    // Shareholder + share + initiated return.
    let (status, shareholder) = send(
        &test.app,
        req(
            "POST",
            "/api/shareholders",
            &cookie,
            Some(&csrf),
            Some(json!({
                "person": { "mode": "new", "firstName": "İade", "lastName": "Eden" },
                "family": { "mode": "new", "sequenceNumber": 91001 }
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{shareholder}");
    let shareholder_id = shareholder["id"].as_str().unwrap();

    let (status, share) = send(
        &test.app,
        req(
            "POST",
            "/api/shares",
            &cookie,
            Some(&csrf),
            Some(json!({
                "shareholderId": shareholder_id,
                "acquisitionType": "founder",
                "effectiveAt": "2025-01-01T00:00:00Z",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{share}");
    let share_id = share["id"].as_str().unwrap();

    let (status, initiated) = send(
        &test.app,
        req(
            "POST",
            "/api/share-returns",
            &cookie,
            Some(&csrf),
            Some(json!({
                "shareId": share_id,
                "effectiveReturnDate": "2026-01-15",
                "reason": "üyelikten ayrılma",
                "idempotencyKey": "init-sr",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{initiated}");
    let return_id = initiated["id"].as_str().unwrap();

    let (status, finalized) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-returns/{return_id}/finalize"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "entitlements": [
                    { "entitlementType": "principal", "amount": "10000.00" }
                ],
                "expectedUpdatedAt": initiated["updatedAt"],
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{finalized}");
    let entitlement_id = finalized["entitlements"][0]["id"].as_str().unwrap();

    // 6,000 settlement exceeds the 5,000 unrestricted portion.
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-return-entitlements/{entitlement_id}/settlements"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "financialAccountId": account.to_string(),
                "amount": "6000.00",
                "idempotencyKey": "st-over",
            })),
        ),
    )
    .await;
    assert_unrestricted_rejected(status, &detail);

    // Boundary settlement works.
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-return-entitlements/{entitlement_id}/settlements"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "financialAccountId": account.to_string(),
                "amount": "5000.00",
                "idempotencyKey": "st-ok",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "boundary settlement: {detail}");
}

// ---------------------------------------------------------------------
// Social Aid stays functional + cross-fund isolation
// ---------------------------------------------------------------------

#[tokio::test]
async fn authorized_disbursement_and_cross_fund_isolation_hold() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Yardım Kasası").await;
    let (status, _) = api_income(&test, &cookie, &csrf, account, "1000.00", "seed").await;
    assert_eq!(status, StatusCode::CREATED);

    let fund_a = api_create_fund(&test, &cookie, &csrf, "Fon A", "fa").await;
    let fund_b = api_create_fund(&test, &cookie, &csrf, "Fon B", "fb").await;
    let fa = fund_a["id"].as_str().unwrap();
    let fb = fund_b["id"].as_str().unwrap();

    let (status, _) = api_donate(&test, &cookie, &csrf, fa, account, "5000.00", "da").await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = api_donate(&test, &cookie, &csrf, fb, account, "3000.00", "db").await;
    assert_eq!(status, StatusCode::CREATED);
    // physical 9,000; reserved 8,000 (A 5,000 + B 3,000); spendable 1,000.

    // Aid B cannot spend fund A's restricted balance.
    let (status, detail) =
        api_disburse(&test, &cookie, &csrf, fb, account, "4000.00", "x-fund").await;
    assert_eq!(status, StatusCode::CONFLICT, "{detail}");

    // Authorized disbursement consumes BOTH ledgers atomically.
    let (status, detail) =
        api_disburse(&test, &cookie, &csrf, fa, account, "5000.00", "aid-ok").await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");
    assert_eq!(api_balance(&test, &cookie, account).await, "4000.00");
    assert_eq!(
        sql_reserved(&test.pool, account).await.to_string(),
        "3000.00"
    );

    // Spendable is still 1,000 — the remaining unrestricted cash.
    let (status, detail) = api_expense(&test, &cookie, &csrf, account, "1000.00", "e-ok2").await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");
    let (status, detail) = api_expense(&test, &cookie, &csrf, account, "0.01", "e-over2").await;
    assert_unrestricted_rejected(status, &detail);
}

#[tokio::test]
async fn donation_reversal_preserves_both_ledgers() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let (account, fund) =
        reserved_fixture(&test, &cookie, &csrf, "2000.00", "5000.00", "dr1").await;
    let fund_id = fund["id"].as_str().unwrap();

    let (status, donations) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/social-aid/donations?fundId={fund_id}&pageSize=10"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let donation_id = donations["items"][0]["id"].as_str().unwrap();

    // Reversing the whole donation removes physical + reserved equally.
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/social-aid/donations/{donation_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "yanlış kayıt" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(api_balance(&test, &cookie, account).await, "2000.00");
    assert_eq!(sql_reserved(&test.pool, account).await.to_string(), "0");

    // Ordinary cash is fully spendable again.
    let (status, detail) = api_expense(&test, &cookie, &csrf, account, "2000.00", "e-free").await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");
}

// ---------------------------------------------------------------------
// Concurrency
// ---------------------------------------------------------------------

#[tokio::test]
async fn concurrent_ordinary_outflows_cannot_overspend_unrestricted() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let (account, _fund) =
        reserved_fixture(&test, &cookie, &csrf, "5000.00", "5000.00", "cc1").await;
    let movements_before = api_movement_count(&test, &cookie, account).await;

    // Two 5,000 expenses race for the single 5,000 unrestricted slot.
    let app = test.app.clone();
    let cookie2 = cookie.clone();
    let csrf2 = csrf.clone();
    let (first, second) = tokio::join!(
        api_expense(&test, &cookie, &csrf, account, "5000.00", "race-a"),
        async {
            let category = {
                let (status, list) = send(
                    &app,
                    req(
                        "GET",
                        "/api/financial-categories/options?categoryType=expense",
                        &cookie2,
                        None,
                        None,
                    ),
                )
                .await;
                assert_eq!(status, StatusCode::OK);
                list.as_array().unwrap()[0]["id"]
                    .as_str()
                    .unwrap()
                    .to_string()
            };
            send(
                &app,
                req(
                    "POST",
                    "/api/expenses",
                    &cookie2,
                    Some(&csrf2),
                    Some(json!({
                        "financialAccountId": account.to_string(),
                        "categoryId": category,
                        "amount": "5000.00",
                        "description": "race",
                        "idempotencyKey": "race-b",
                    })),
                ),
            )
            .await
        }
    );
    let statuses = [first.0, second.0];
    let created = statuses
        .iter()
        .filter(|s| **s == StatusCode::CREATED)
        .count();
    let conflicted = statuses
        .iter()
        .filter(|s| **s == StatusCode::CONFLICT)
        .count();
    assert_eq!((created, conflicted), (1, 1), "{statuses:?}");

    // Exactly one expense posted; no phantom movements.
    assert_eq!(
        api_movement_count(&test, &cookie, account).await,
        movements_before + 1
    );
    assert_eq!(api_balance(&test, &cookie, account).await, "5000.00");
    assert_eq!(
        sql_reserved(&test.pool, account).await.to_string(),
        "5000.00"
    );
}

#[tokio::test]
async fn concurrent_ordinary_and_aid_outflows_stay_safe() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let (account, fund) =
        reserved_fixture(&test, &cookie, &csrf, "5000.00", "5000.00", "cc2").await;
    let fund_id = fund["id"].as_str().unwrap().to_string();

    // Expense (unrestricted 5,000) races aid disbursement (reserved
    // 5,000): disjoint portions — both may succeed, leaving physical 0
    // and reserved 0.
    let (expense, aid) = tokio::join!(
        api_expense(&test, &cookie, &csrf, account, "5000.00", "cc-exp"),
        api_disburse(&test, &cookie, &csrf, &fund_id, account, "5000.00", "cc-aid")
    );
    assert_eq!(expense.0, StatusCode::CREATED, "{:?}", expense.1);
    assert_eq!(aid.0, StatusCode::CREATED, "{:?}", aid.1);
    assert_eq!(api_balance(&test, &cookie, account).await, "0.00");
    assert_eq!(sql_reserved(&test.pool, account).await.to_string(), "0");
}

#[tokio::test]
async fn concurrent_transfer_and_income_reversal_stay_safe() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa cc3").await;
    let (status, income) = api_income(&test, &cookie, &csrf, account, "5000.00", "inc-x").await;
    assert_eq!(status, StatusCode::CREATED, "{income}");
    let income_id = income["id"].as_str().unwrap().to_string();
    let fund = api_create_fund(&test, &cookie, &csrf, "Fon cc3", "f-cc3").await;
    let fund_id = fund["id"].as_str().unwrap();
    let (status, donation) =
        api_donate(&test, &cookie, &csrf, fund_id, account, "5000.00", "d-cc3").await;
    assert_eq!(status, StatusCode::CREATED, "{donation}");
    // physical 10,000, reserved 5,000, spendable 5,000.
    let other = api_create_account(&test, &cookie, &csrf, "Diğer").await;

    // Transfer 5,000 (all unrestricted) races reversing the 5,000
    // income: exactly one can win — the loser violates either the
    // balance or the reservation floor.
    let app = test.app.clone();
    let cookie2 = cookie.clone();
    let csrf2 = csrf.clone();
    let income_path = format!("/api/incomes/{income_id}/reverse");
    let (transfer, reversal) = tokio::join!(
        api_transfer(&test, &cookie, &csrf, account, other, "5000.00", "cc-tr"),
        async move {
            send(
                &app,
                req(
                    "POST",
                    &income_path,
                    &cookie2,
                    Some(&csrf2),
                    Some(json!({ "reason": "düzeltme" })),
                ),
            )
            .await
        }
    );
    let statuses = [transfer.0, reversal.0];
    let ok = statuses.iter().filter(|s| s.is_success()).count();
    let conflict = statuses
        .iter()
        .filter(|s| **s == StatusCode::CONFLICT)
        .count();
    assert_eq!((ok, conflict), (1, 1), "{statuses:?}");

    // Final state is consistent either way.
    let balance: rust_decimal::Decimal =
        api_balance(&test, &cookie, account).await.parse().unwrap();
    let reserved = sql_reserved(&test.pool, account).await;
    assert!(balance >= reserved, "{balance} >= {reserved}");
}

// ---------------------------------------------------------------------
// Ledger integrity after rejection + isolation
// ---------------------------------------------------------------------

#[tokio::test]
async fn rejected_operation_leaves_all_records_unchanged() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let (account, _fund) =
        reserved_fixture(&test, &cookie, &csrf, "5000.00", "5000.00", "rj1").await;
    let other = api_create_account(&test, &cookie, &csrf, "Yan Kasa").await;

    let movements_before = api_movement_count(&test, &cookie, account).await;
    let expense_count: i64 = sqlx::query_scalar("SELECT count(*) FROM expense_entries")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    let transfer_count: i64 = sqlx::query_scalar("SELECT count(*) FROM account_transfers")
        .fetch_one(&test.pool)
        .await
        .unwrap();

    let (status, detail) = api_expense(&test, &cookie, &csrf, account, "6000.00", "rej-e").await;
    assert_unrestricted_rejected(status, &detail);
    let (status, detail) =
        api_transfer(&test, &cookie, &csrf, account, other, "6000.00", "rej-t").await;
    assert_unrestricted_rejected(status, &detail);

    assert_eq!(
        api_movement_count(&test, &cookie, account).await,
        movements_before
    );
    let after_e: i64 = sqlx::query_scalar("SELECT count(*) FROM expense_entries")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    let after_t: i64 = sqlx::query_scalar("SELECT count(*) FROM account_transfers")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!((expense_count, transfer_count), (after_e, after_t));
}

#[tokio::test]
async fn reservation_is_per_account_not_global() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    // Account A: 5,000 ordinary + 5,000 reserved. Account B: 8,000 free.
    let (account_a, _fund) =
        reserved_fixture(&test, &cookie, &csrf, "5000.00", "5000.00", "ma").await;
    let account_b = api_create_account(&test, &cookie, &csrf, "B Kasası").await;
    let (status, _) = api_income(&test, &cookie, &csrf, account_b, "8000.00", "inc-b").await;
    assert_eq!(status, StatusCode::CREATED);

    // B's cash is fully spendable — A's reservation does not leak.
    let (status, detail) = api_expense(&test, &cookie, &csrf, account_b, "8000.00", "e-b").await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");

    // And B's unrestricted cash cannot satisfy A's reservation: a
    // transfer FROM A still hits the reservation floor.
    let (status, detail) = api_transfer(
        &test, &cookie, &csrf, account_a, account_b, "5001.00", "tr-cross",
    )
    .await;
    assert_unrestricted_rejected(status, &detail);
}

#[tokio::test]
async fn reversed_movements_excluded_from_physical_and_reserved() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let (account, fund) = reserved_fixture(&test, &cookie, &csrf, "0.00", "5000.00", "rv1").await;
    let fund_id = fund["id"].as_str().unwrap();

    // Disbursement spends 2,000 of the restricted cash, then both
    // events are reversed: physical AND reserved return to zero.
    let (status, disbursement) =
        api_disburse(&test, &cookie, &csrf, fund_id, account, "2000.00", "dis-rv").await;
    assert_eq!(status, StatusCode::CREATED, "{disbursement}");
    let disbursement_id = disbursement["id"].as_str().unwrap();
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/social-aid/disbursements/{disbursement_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "düzeltme" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, donations) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/social-aid/donations?fundId={fund_id}&pageSize=10"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let donation_id = donations["items"][0]["id"].as_str().unwrap();
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/social-aid/donations/{donation_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "düzeltme" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    assert_eq!(api_balance(&test, &cookie, account).await, "0.00");
    assert_eq!(sql_reserved(&test.pool, account).await.to_string(), "0");
}
