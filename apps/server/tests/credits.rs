//! Database-gated Shareholder Credit (Excess Payment) domain tests
//! (STEP-009). Requires KOOPERATIF_TEST_DATABASE_URL; each test runs
//! in its own throwaway database.
//!
//! Invariants under test (docs/05, docs/06, docs/15, docs/19,
//! ADR-003/004/006):
//! - A credit is backed ONLY by a posted Payment's remainder; the
//!   beneficiary is explicit, never inferred (payer ≠ beneficiary).
//! - Credit assignment/application move NO Financial Account money —
//!   account movements are untouched by the whole domain.
//! - settled(assessment) = active allocations + active applications.
//! - Auto-offset is deterministic (credit FIFO, oldest-due-first),
//!   concurrency-safe and idempotent.
//! - Reversal is status-based; a Payment reversal refuses to orphan a
//!   consumed credit and unwinds an unused one.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio::sync::Mutex;
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
    default_account: Mutex<Option<Uuid>>,
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
        default_account: Mutex::new(None),
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

async fn api_create_shareholder(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    first: &str,
    last: &str,
) -> Uuid {
    let sequence = (Uuid::new_v4().as_u128() % 900_000) as i64 + 100_000;
    let body = json!({
        "person": { "mode": "new", "firstName": first, "lastName": last },
        "family": { "mode": "new", "sequenceNumber": sequence }
    });
    let (status, detail) = send(
        &test.app,
        req("POST", "/api/shareholders", cookie, Some(csrf), Some(body)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {detail}");
    detail["id"].as_str().unwrap().parse().unwrap()
}

/// Create a draft period with the given per-shareholder amount and
/// finalize it (auto-offset runs inside generation).
async fn api_generate_period(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    amount: &str,
    due_date: &str,
) -> Uuid {
    let body = json!({
        "name": format!("Dönem {due_date}"),
        "collectionStartDate": "2026-01-01",
        "dueDate": due_date,
        "ruleType": "per_shareholder",
        "baseAmount": amount,
        "assessmentEffectiveDate": "2026-01-01",
    });
    let (status, period) = send(
        &test.app,
        req("POST", "/api/periods", cookie, Some(csrf), Some(body)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{period}");
    let period_id = period["id"].as_str().unwrap().to_string();
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/periods/{period_id}/generate-assessments"),
            cookie,
            Some(csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "generate failed: {detail}");
    period_id.parse().unwrap()
}

/// The single assessment of `shareholder_id` inside `period_id`.
async fn assessment_of(
    test: &TestApp,
    cookie: &str,
    period_id: Uuid,
    shareholder_id: Uuid,
) -> Value {
    let (status, list) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/periods/{period_id}/assessments?pageSize=100"),
            cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["shareholder"]["shareholderId"].as_str().unwrap() == shareholder_id.to_string())
        .cloned()
        .expect("assessment exists")
}

async fn default_account(test: &TestApp, cookie: &str, csrf: &str) -> Uuid {
    let mut guard = test.default_account.lock().await;
    if let Some(id) = *guard {
        return id;
    }
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/financial-accounts",
            cookie,
            Some(csrf),
            Some(json!({ "name": "Test Kasası", "accountType": "cash" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "account create: {detail}");
    let id: Uuid = detail["id"].as_str().unwrap().parse().unwrap();
    *guard = Some(id);
    id
}

/// Post a fully-unallocated Payment of `amount` (payer is a new
/// unrelated Person — payer identity never implies credit ownership).
async fn post_unallocated_payment(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    amount: &str,
    key: &str,
) -> Value {
    let account = default_account(test, cookie, csrf).await;
    let body = json!({
        "payerFirstName": "Üçüncü",
        "payerLastName": "Taraf",
        "amount": amount,
        "method": "cash",
        "destinationAccountId": account,
        "idempotencyKey": key,
        "allocations": []
    });
    let (status, created) = send(
        &test.app,
        req("POST", "/api/payments", cookie, Some(csrf), Some(body)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["payment"].clone()
}

async fn post_payment_with_allocations(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    amount: &str,
    key: &str,
    allocations: Vec<Value>,
) -> Value {
    let account = default_account(test, cookie, csrf).await;
    let body = json!({
        "payerFirstName": "Üçüncü",
        "payerLastName": "Taraf",
        "amount": amount,
        "method": "cash",
        "destinationAccountId": account,
        "idempotencyKey": key,
        "allocations": allocations
    });
    let (status, created) = send(
        &test.app,
        req("POST", "/api/payments", cookie, Some(csrf), Some(body)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["payment"].clone()
}

async fn assign_credit(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    payment_id: Uuid,
    shareholder_id: Uuid,
    amount: &str,
    key: &str,
) -> (StatusCode, Value) {
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/payments/{payment_id}/credits"),
            cookie,
            Some(csrf),
            Some(json!({
                "shareholderId": shareholder_id,
                "amount": amount,
                "idempotencyKey": key
            })),
        ),
    )
    .await
}

async fn apply_credit(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    assessment_id: Uuid,
    amount: &str,
    key: &str,
) -> (StatusCode, Value) {
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/assessments/{assessment_id}/credit-applications"),
            cookie,
            Some(csrf),
            Some(json!({ "amount": amount, "idempotencyKey": key })),
        ),
    )
    .await
}

async fn credit_ledger(test: &TestApp, cookie: &str, shareholder_id: Uuid) -> Value {
    let (status, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/shareholders/{shareholder_id}/credits"),
            cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail
}

async fn payment_detail(test: &TestApp, cookie: &str, payment_id: Uuid) -> Value {
    let (status, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/payments/{payment_id}"),
            cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail
}

async fn assessment_applications(test: &TestApp, cookie: &str, assessment_id: Uuid) -> Vec<Value> {
    let (status, list) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/assessments/{assessment_id}/credit-applications"),
            cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list.as_array().unwrap().clone()
}

/// Derived remaining debt of one assessment via the open list.
async fn open_remaining(
    test: &TestApp,
    cookie: &str,
    shareholder_id: Uuid,
    assessment_id: Uuid,
) -> String {
    let (status, list) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/shareholders/{shareholder_id}/open-assessments"),
            cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let found = list
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"].as_str().unwrap() == assessment_id.to_string());
    match found {
        Some(row) => row["remainingAmount"].as_str().unwrap().to_string(),
        None => "0.00".to_string(), // fully settled rows leave the list
    }
}

async fn account_balance(test: &TestApp, cookie: &str, account_id: Uuid) -> String {
    let (status, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/financial-accounts/{account_id}"),
            cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail["balance"].as_str().unwrap().to_string()
}

// ---------------------------------------------------------------------
// The user-supplied scenario: 10.000 in, 7.000 debt, 3.000 credit;
// a later 2.000 obligation is auto-offset; 1.000 credit remains.
// ---------------------------------------------------------------------

#[tokio::test]
async fn payment_remainder_becomes_explicit_credit_and_auto_offsets_later_debt() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let shareholder = api_create_shareholder(&test, &cookie, &csrf, "Veli", "Ortak").await;

    // Existing obligation: 7.000
    let period1 = api_generate_period(&test, &cookie, &csrf, "7000.00", "2026-01-31").await;
    let a1 = assessment_of(&test, &cookie, period1, shareholder).await;
    let a1_id: Uuid = a1["id"].as_str().unwrap().parse().unwrap();

    // Payment 10.000 — 7.000 allocated to debt, 3.000 stays unassigned.
    let payment = post_payment_with_allocations(
        &test,
        &cookie,
        &csrf,
        "10000.00",
        "key-p1",
        vec![json!({ "assessmentId": a1["id"], "amount": "7000.00" })],
    )
    .await;
    let payment_id: Uuid = payment["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(payment["allocatedAmount"], "7000.00");
    assert_eq!(payment["creditedAmount"], "0.00");
    assert_eq!(payment["unallocatedAmount"], "3000.00");

    // Explicit attribution to the Shareholder (payer is unrelated).
    let (status, outcome) = assign_credit(
        &test,
        &cookie,
        &csrf,
        payment_id,
        shareholder,
        "3000.00",
        "key-c1",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{outcome}");
    assert_eq!(outcome["appliedAmount"], "0.00");

    // Money location unchanged: the account still holds 10.000 —
    // credit assignment is NOT an Account Movement.
    let account = default_account(&test, &cookie, &csrf).await;
    assert_eq!(account_balance(&test, &cookie, account).await, "10000.00");

    let ledger = credit_ledger(&test, &cookie, shareholder).await;
    assert_eq!(ledger["summary"]["totalOriginated"], "3000.00");
    assert_eq!(ledger["summary"]["available"], "3000.00");
    assert_eq!(
        ledger["credits"][0]["sourcePaymentId"],
        payment_id.to_string()
    );

    let payment = payment_detail(&test, &cookie, payment_id).await;
    assert_eq!(payment["creditedAmount"], "3000.00");
    assert_eq!(payment["unallocatedAmount"], "0.00");
    assert_eq!(payment["credits"][0]["amount"], "3000.00");
    assert_eq!(
        payment["credits"][0]["shareholderId"],
        shareholder.to_string()
    );

    // Later obligation: 2.000 — auto-offset inside generation.
    let period2 = api_generate_period(&test, &cookie, &csrf, "2000.00", "2026-02-28").await;
    let a2 = assessment_of(&test, &cookie, period2, shareholder).await;
    let a2_id: Uuid = a2["id"].as_str().unwrap().parse().unwrap();

    let apps = assessment_applications(&test, &cookie, a2_id).await;
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0]["amount"], "2000.00");
    assert_eq!(apps[0]["mode"], "automatic");

    let ledger = credit_ledger(&test, &cookie, shareholder).await;
    assert_eq!(ledger["summary"]["available"], "1000.00");
    assert_eq!(ledger["summary"]["totalApplied"], "2000.00");
    assert_eq!(
        open_remaining(&test, &cookie, shareholder, a2_id).await,
        "0.00"
    );
    // First debt unaffected.
    assert_eq!(
        open_remaining(&test, &cookie, shareholder, a1_id).await,
        "0.00"
    );
    // Still no extra account effect: balance unchanged.
    assert_eq!(account_balance(&test, &cookie, account).await, "10000.00");
}

#[tokio::test]
async fn credit_assignment_sweeps_existing_open_debt_oldest_due_first() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let shareholder = api_create_shareholder(&test, &cookie, &csrf, "Can", "Borçlu").await;
    let p1 = api_generate_period(&test, &cookie, &csrf, "1000.00", "2026-01-31").await;
    let p2 = api_generate_period(&test, &cookie, &csrf, "1000.00", "2026-02-28").await;
    let a1 = assessment_of(&test, &cookie, p1, shareholder).await;
    let a2 = assessment_of(&test, &cookie, p2, shareholder).await;
    let a1_id: Uuid = a1["id"].as_str().unwrap().parse().unwrap();
    let a2_id: Uuid = a2["id"].as_str().unwrap().parse().unwrap();

    let payment = post_unallocated_payment(&test, &cookie, &csrf, "1500.00", "key-p2").await;
    let payment_id: Uuid = payment["id"].as_str().unwrap().parse().unwrap();

    let (status, outcome) = assign_credit(
        &test,
        &cookie,
        &csrf,
        payment_id,
        shareholder,
        "1500.00",
        "key-c2",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{outcome}");
    assert_eq!(outcome["appliedAmount"], "1500.00");
    assert_eq!(outcome["applicationCount"], 2);

    // Oldest due date consumed first: a1 fully, a2 partially.
    let apps1 = assessment_applications(&test, &cookie, a1_id).await;
    assert_eq!(apps1[0]["amount"], "1000.00");
    let apps2 = assessment_applications(&test, &cookie, a2_id).await;
    assert_eq!(apps2[0]["amount"], "500.00");
    assert_eq!(
        open_remaining(&test, &cookie, shareholder, a2_id).await,
        "500.00"
    );
    let ledger = credit_ledger(&test, &cookie, shareholder).await;
    assert_eq!(ledger["summary"]["available"], "0.00");
}

#[tokio::test]
async fn manual_application_uses_same_fifo_and_settles_partially() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let shareholder = api_create_shareholder(&test, &cookie, &csrf, "Deniz", "El").await;
    // No open assessments yet: credit parks.
    let payment = post_unallocated_payment(&test, &cookie, &csrf, "400.00", "key-p3").await;
    let payment_id: Uuid = payment["id"].as_str().unwrap().parse().unwrap();
    let (status, outcome) = assign_credit(
        &test,
        &cookie,
        &csrf,
        payment_id,
        shareholder,
        "400.00",
        "key-c3",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{outcome}");
    assert_eq!(outcome["appliedAmount"], "0.00");

    // A new assessment is generated WITHOUT auto-consume disabled —
    // it consumes automatically; to exercise the manual path we need
    // debt that outlives credit: instead create debt BIGGER than
    // credit. Generation auto-applies 400; remaining 600 is then
    // manually applied from a SECOND credit.
    let period = api_generate_period(&test, &cookie, &csrf, "1000.00", "2026-03-31").await;
    let a = assessment_of(&test, &cookie, period, shareholder).await;
    let a_id: Uuid = a["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        open_remaining(&test, &cookie, shareholder, a_id).await,
        "600.00"
    );

    let payment2 = post_unallocated_payment(&test, &cookie, &csrf, "600.00", "key-p4").await;
    let p2_id: Uuid = payment2["id"].as_str().unwrap().parse().unwrap();
    // Assign WITHOUT auto-sweep interference? The sweep WILL run —
    // which is the approved policy — so manual apply is exercised by
    // assigning to a DIFFERENT shareholder's debt? No: debt belongs to
    // the same shareholder. The sweep already covers it; instead the
    // manual path is proven by reversing an automatic application then
    // re-applying by hand.
    let (status, outcome) = assign_credit(
        &test,
        &cookie,
        &csrf,
        p2_id,
        shareholder,
        "600.00",
        "key-c4",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{outcome}");
    assert_eq!(outcome["appliedAmount"], "600.00");
    assert_eq!(
        open_remaining(&test, &cookie, shareholder, a_id).await,
        "0.00"
    );

    // Reverse the newest manual-equivalent application (automatic from
    // the sweep) — then re-apply manually.
    let apps = assessment_applications(&test, &cookie, a_id).await;
    let latest = apps
        .iter()
        .find(|x| x["status"] == "active" && x["amount"] == "600.00")
        .unwrap();
    let app_id = latest["id"].as_str().unwrap();
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/credit-applications/{app_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reversalReason": "yanlış mahsup" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        open_remaining(&test, &cookie, shareholder, a_id).await,
        "600.00"
    );
    let ledger = credit_ledger(&test, &cookie, shareholder).await;
    assert_eq!(ledger["summary"]["available"], "600.00");

    // Manual application — same engine, mode = 'manual'.
    let (status, applied) = apply_credit(&test, &cookie, &csrf, a_id, "600.00", "key-m1").await;
    assert_eq!(status, StatusCode::CREATED, "{applied}");
    assert_eq!(applied["appliedAmount"], "600.00");
    assert_eq!(applied["assessmentRemaining"], "0.00");
    let apps = assessment_applications(&test, &cookie, a_id).await;
    assert!(apps
        .iter()
        .any(|x| x["mode"] == "manual" && x["status"] == "active"));
}

// ---------------------------------------------------------------------
// Rejections
// ---------------------------------------------------------------------

#[tokio::test]
async fn credit_cannot_exceed_payment_remainder_and_is_bounded() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let shareholder = api_create_shareholder(&test, &cookie, &csrf, "Ece", "Sınır").await;
    let payment = post_unallocated_payment(&test, &cookie, &csrf, "100.00", "key-p5").await;
    let payment_id: Uuid = payment["id"].as_str().unwrap().parse().unwrap();

    // Over-remainder → 409.
    let (status, _) = assign_credit(
        &test,
        &cookie,
        &csrf,
        payment_id,
        shareholder,
        "100.01",
        "key-c5a",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // First assignment consumes 60 — only 40 remains.
    let (status, _) = assign_credit(
        &test,
        &cookie,
        &csrf,
        payment_id,
        shareholder,
        "60.00",
        "key-c5b",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = assign_credit(
        &test,
        &cookie,
        &csrf,
        payment_id,
        shareholder,
        "40.01",
        "key-c5c",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    // Exact remainder still assignable.
    let (status, _) = assign_credit(
        &test,
        &cookie,
        &csrf,
        payment_id,
        shareholder,
        "40.00",
        "key-c5d",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Non-posted payment cannot source credit: reverse it first.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/payments/{payment_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "iptal" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = assign_credit(
        &test,
        &cookie,
        &csrf,
        payment_id,
        shareholder,
        "10.00",
        "key-c5e",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn assign_idempotency_replays_and_conflicts() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let s1 = api_create_shareholder(&test, &cookie, &csrf, "Fatih", "Bir").await;
    let s2 = api_create_shareholder(&test, &cookie, &csrf, "Gamze", "İki").await;
    let payment = post_unallocated_payment(&test, &cookie, &csrf, "500.00", "key-p6").await;
    let payment_id: Uuid = payment["id"].as_str().unwrap().parse().unwrap();

    let (status, first) =
        assign_credit(&test, &cookie, &csrf, payment_id, s1, "200.00", "key-c6").await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let credit_id = first["creditId"].as_str().unwrap().to_string();

    // Same key + same payload → replay, HTTP 200, no second row.
    let (status, replay) =
        assign_credit(&test, &cookie, &csrf, payment_id, s1, "200.00", "key-c6").await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["creditId"], credit_id);
    let ledger = credit_ledger(&test, &cookie, s1).await;
    assert_eq!(ledger["credits"].as_array().unwrap().len(), 1);

    // Same key + different beneficiary → 409.
    let (status, _) =
        assign_credit(&test, &cookie, &csrf, payment_id, s2, "200.00", "key-c6").await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn concurrent_assignments_never_over_create_credit() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let shareholder = api_create_shareholder(&test, &cookie, &csrf, "Hakan", "Eş").await;
    let payment = post_unallocated_payment(&test, &cookie, &csrf, "100.00", "key-p7").await;
    let payment_id: Uuid = payment["id"].as_str().unwrap().parse().unwrap();

    // Two concurrent 100-assignments against a 100 remainder: exactly
    // ONE may succeed — the payment row lock serializes them.
    let (r1, r2) = tokio::join!(
        assign_credit(
            &test,
            &cookie,
            &csrf,
            payment_id,
            shareholder,
            "100.00",
            "key-r1"
        ),
        assign_credit(
            &test,
            &cookie,
            &csrf,
            payment_id,
            shareholder,
            "100.00",
            "key-r2"
        ),
    );
    let statuses = [r1.0, r2.0];
    assert!(statuses.contains(&StatusCode::CREATED));
    assert!(statuses.contains(&StatusCode::CONFLICT));

    let ledger = credit_ledger(&test, &cookie, shareholder).await;
    assert_eq!(ledger["summary"]["totalOriginated"], "100.00");
    assert_eq!(ledger["credits"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn payment_reversal_reverses_unused_credit_but_blocks_consumed() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let shareholder = api_create_shareholder(&test, &cookie, &csrf, "Işıl", "Geri").await;

    // CASE A: unused credit — payment reversal reverses it cleanly.
    let p1 = post_unallocated_payment(&test, &cookie, &csrf, "300.00", "key-p8a").await;
    let p1_id: Uuid = p1["id"].as_str().unwrap().parse().unwrap();
    let (status, _) = assign_credit(
        &test,
        &cookie,
        &csrf,
        p1_id,
        shareholder,
        "300.00",
        "key-c8a",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/payments/{p1_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "yanlış tahsilat" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ledger = credit_ledger(&test, &cookie, shareholder).await;
    assert_eq!(ledger["summary"]["available"], "0.00");
    assert_eq!(ledger["credits"][0]["status"], "reversed");

    // CASE B: consumed credit — reversal is REFUSED (409) until the
    // application is corrected; no orphaned settlement is possible.
    let period = api_generate_period(&test, &cookie, &csrf, "200.00", "2026-04-30").await;
    let a = assessment_of(&test, &cookie, period, shareholder).await;
    let p2 = post_unallocated_payment(&test, &cookie, &csrf, "200.00", "key-p8b").await;
    let p2_id: Uuid = p2["id"].as_str().unwrap().parse().unwrap();
    let (status, outcome) = assign_credit(
        &test,
        &cookie,
        &csrf,
        p2_id,
        shareholder,
        "200.00",
        "key-c8b",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{outcome}");
    assert_eq!(outcome["appliedAmount"], "200.00");
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/payments/{p2_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "iptal" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Correct the application first → reversal then succeeds and the
    // credit is reversed with the payment.
    let apps =
        assessment_applications(&test, &cookie, a["id"].as_str().unwrap().parse().unwrap()).await;
    let app_id = apps[0]["id"].as_str().unwrap();
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/credit-applications/{app_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reversalReason": "mahsup iade" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/payments/{p2_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "iptal" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ledger = credit_ledger(&test, &cookie, shareholder).await;
    assert_eq!(ledger["summary"]["available"], "0.00");
    // Debt reopens: the assessment owes 200.00 again.
    assert_eq!(
        open_remaining(
            &test,
            &cookie,
            shareholder,
            a["id"].as_str().unwrap().parse().unwrap()
        )
        .await,
        "200.00"
    );
}

#[tokio::test]
async fn credit_origin_reversal_requires_unconsumed_and_restores_remainder() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let shareholder = api_create_shareholder(&test, &cookie, &csrf, "Jale", "Kök").await;
    let payment = post_unallocated_payment(&test, &cookie, &csrf, "250.00", "key-p9").await;
    let payment_id: Uuid = payment["id"].as_str().unwrap().parse().unwrap();
    let (status, outcome) = assign_credit(
        &test,
        &cookie,
        &csrf,
        payment_id,
        shareholder,
        "250.00",
        "key-c9",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let credit_id = outcome["creditId"].as_str().unwrap();

    // Reversing the ORIGIN frees the payment remainder again.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/credits/{credit_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reversalReason": "yanlış atama" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let ledger = credit_ledger(&test, &cookie, shareholder).await;
    assert_eq!(ledger["summary"]["available"], "0.00");
    let payment = payment_detail(&test, &cookie, payment_id).await;
    assert_eq!(payment["unallocatedAmount"], "250.00");
    assert_eq!(payment["credits"][0]["status"], "reversed");

    // Idempotent replay of the reversal.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/credits/{credit_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reversalReason": "tekrar" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The freed remainder is assignable again.
    let (status, _) = assign_credit(
        &test,
        &cookie,
        &csrf,
        payment_id,
        shareholder,
        "250.00",
        "key-c9b",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn validation_guards_and_unassigned_remainder_never_implied() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let shareholder = api_create_shareholder(&test, &cookie, &csrf, "Kerem", "Doğr").await;
    let payment = post_unallocated_payment(&test, &cookie, &csrf, "100.00", "key-p10").await;
    let payment_id: Uuid = payment["id"].as_str().unwrap().parse().unwrap();

    for (amount, key) in [
        ("0.00", "v1"),
        ("-10.00", "v2"),
        ("10.001", "v3"),
        ("abc", "v4"),
    ] {
        let (status, _) =
            assign_credit(&test, &cookie, &csrf, payment_id, shareholder, amount, key).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "amount {amount}");
    }
    // Missing/empty idempotency key → 422.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/payments/{payment_id}/credits"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "shareholderId": shareholder, "amount": "10.00", "idempotencyKey": ""
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Unknown shareholder → 422 (explicit beneficiary validation).
    let (status, _) = assign_credit(
        &test,
        &cookie,
        &csrf,
        payment_id,
        Uuid::new_v4(),
        "10.00",
        "v5",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Unknown payment → 404, indistinguishable.
    let (status, _) = assign_credit(
        &test,
        &cookie,
        &csrf,
        Uuid::new_v4(),
        shareholder,
        "10.00",
        "v6",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The unassigned remainder is still 100 — nothing was implied.
    let payment = payment_detail(&test, &cookie, payment_id).await;
    assert_eq!(payment["unallocatedAmount"], "100.00");
}

#[tokio::test]
async fn apply_guards_and_idempotent_replay() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let s1 = api_create_shareholder(&test, &cookie, &csrf, "Lale", "Bir").await;
    let s2 = api_create_shareholder(&test, &cookie, &csrf, "Mert", "İki").await;

    // Credit belongs to s1 — s2's debt can never consume it.
    let payment = post_unallocated_payment(&test, &cookie, &csrf, "500.00", "key-p11").await;
    let p_id: Uuid = payment["id"].as_str().unwrap().parse().unwrap();
    let (status, _) = assign_credit(&test, &cookie, &csrf, p_id, s1, "500.00", "key-c11").await;
    assert_eq!(status, StatusCode::CREATED);

    let period = api_generate_period(&test, &cookie, &csrf, "1000.00", "2026-05-31").await;
    let a1 = assessment_of(&test, &cookie, period, s1).await;
    let a2 = assessment_of(&test, &cookie, period, s2).await;
    let a1_id: Uuid = a1["id"].as_str().unwrap().parse().unwrap();
    let a2_id: Uuid = a2["id"].as_str().unwrap().parse().unwrap();

    // Generation auto-applied s1's 500 credit to a1 (remaining 500).
    assert_eq!(open_remaining(&test, &cookie, s1, a1_id).await, "500.00");
    // s2 has no credit → manual apply on a2 fails 409.
    let (status, _) = apply_credit(&test, &cookie, &csrf, a2_id, "100.00", "key-m2").await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Over-applying more than the remaining debt → 409.
    let (status, _) = apply_credit(&test, &cookie, &csrf, a1_id, "501.00", "key-m3").await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Replay of the automatic path's idempotency is durable; now the
    // manual command itself: s1 needs more credit first.
    let payment2 = post_unallocated_payment(&test, &cookie, &csrf, "500.00", "key-p12").await;
    let p2_id: Uuid = payment2["id"].as_str().unwrap().parse().unwrap();
    let (status, out) = assign_credit(&test, &cookie, &csrf, p2_id, s1, "500.00", "key-c12").await;
    assert_eq!(status, StatusCode::CREATED, "{out}");
    // The assignment sweep auto-applied to a1 — settle is complete.
    assert_eq!(open_remaining(&test, &cookie, s1, a1_id).await, "0.00");

    // Manual apply on the fully settled assessment → 409.
    let (status, _) = apply_credit(&test, &cookie, &csrf, a1_id, "10.00", "key-m4").await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Reverse the latest application and re-apply manually with a key;
    // a replay with the same key returns the same outcome.
    let apps = assessment_applications(&test, &cookie, a1_id).await;
    let latest = apps
        .iter()
        .find(|x| x["status"] == "active" && x["amount"] == "500.00")
        .unwrap();
    let app_id = latest["id"].as_str().unwrap();
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/credit-applications/{app_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reversalReason": "düzeltme" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, applied) = apply_credit(&test, &cookie, &csrf, a1_id, "300.00", "key-m5").await;
    assert_eq!(status, StatusCode::CREATED, "{applied}");
    let (status, replay) = apply_credit(&test, &cookie, &csrf, a1_id, "300.00", "key-m5").await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["replayed"], true);
    // Same key, different amount → 409.
    let (status, _) = apply_credit(&test, &cookie, &csrf, a1_id, "200.00", "key-m5").await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn authorization_csrf_idor_and_cache_control() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let shareholder = api_create_shareholder(&test, &cookie, &csrf, "Naz", "Yetki").await;
    let payment = post_unallocated_payment(&test, &cookie, &csrf, "100.00", "key-p13").await;
    let payment_id: Uuid = payment["id"].as_str().unwrap().parse().unwrap();

    // Unauthenticated → 401.
    let (status, _) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/shareholders/{shareholder}/credits"),
            "no-such-cookie",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Authenticated without permission → 403.
    let plain = format!("plain.{}", Uuid::new_v4().simple());
    create_plain_user(&test.pool, &plain).await;
    let (pcookie, pcsrf) = login(&test.app, test.peer(), &plain).await;
    let (status, _) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/shareholders/{shareholder}/credits"),
            &pcookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = assign_credit(
        &test,
        &pcookie,
        &pcsrf,
        payment_id,
        shareholder,
        "10.00",
        "key-pc",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Missing CSRF → 403.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/payments/{payment_id}/credits"),
            &cookie,
            None,
            Some(json!({
                "shareholderId": shareholder, "amount": "10.00", "idempotencyKey": "k"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // no-store on the sensitive read surface.
    let response = test
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/api/shareholders/{shareholder}/credits"),
            &cookie,
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
            .unwrap()
            .to_str()
            .unwrap(),
        "no-store"
    );
}
