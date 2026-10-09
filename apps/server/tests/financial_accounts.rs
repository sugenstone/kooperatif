//! Database-gated Financial Accounts / Account Movements / Transfers
//! domain tests (STEP-008). Requires KOOPERATIF_TEST_DATABASE_URL; each
//! test runs in its own throwaway database.
//!
//! Invariants under test (docs/03, docs/07, docs/15, ADR-003/004/006):
//! - Balance is ALWAYS derived from active movements — never stored.
//! - A posted Payment produces exactly ONE account inflow atomically.
//! - A Transfer is ONE logical move with two paired legs, or none.
//! - Negative balances are rejected under deterministic row locks.
//! - Reversals preserve history: rows flip to `reversed`, never vanish.

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

async fn api_create_account(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    name: &str,
    account_type: &str,
) -> Uuid {
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/financial-accounts",
            cookie,
            Some(csrf),
            Some(json!({ "name": name, "accountType": account_type })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "account create: {detail}");
    detail["id"].as_str().unwrap().parse().unwrap()
}

async fn api_get_account(test: &TestApp, cookie: &str, id: Uuid) -> Value {
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
    detail
}

async fn api_movements(test: &TestApp, cookie: &str, id: Uuid) -> Vec<Value> {
    let (status, list) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/financial-accounts/{id}/movements?pageSize=100"),
            cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "movements: {list}");
    list["items"].as_array().unwrap().clone()
}

async fn api_balance(test: &TestApp, cookie: &str, id: Uuid) -> String {
    api_get_account(test, cookie, id).await["balance"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn api_post_payment(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    account: Uuid,
    amount: &str,
    key: &str,
) -> (StatusCode, Value) {
    send(
        &test.app,
        req(
            "POST",
            "/api/payments",
            cookie,
            Some(csrf),
            Some(json!({
                "payerFirstName": "Ödeyen", "payerLastName": "Kişi",
                "amount": amount, "method": "cash",
                "destinationAccountId": account.to_string(),
                "idempotencyKey": key, "allocations": []
            })),
        ),
    )
    .await
}

async fn api_post_transfer(
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
                "idempotencyKey": key,
            })),
        ),
    )
    .await
}

async fn movement_count(pool: &PgPool, status: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM account_movements WHERE status = $1")
        .bind(status)
        .fetch_one(pool)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------
// Account lifecycle
// ---------------------------------------------------------------------

#[tokio::test]
async fn account_crud_lifecycle_and_options() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    // Cash + Bank accounts; bank metadata only allowed on bank.
    let cash = api_create_account(&test, &cookie, &csrf, "Merkez Kasa", "cash").await;
    let bank = api_create_account(&test, &cookie, &csrf, "Vakıf TL", "bank").await;

    // Bank metadata on a cash account is rejected.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/financial-accounts",
            &cookie,
            Some(&csrf),
            Some(json!({
                "name": "Yanlış Kasa", "accountType": "cash",
                "bankName": "Vakıfbank"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Derived balance starts at exactly zero.
    let detail = api_get_account(&test, &cookie, cash).await;
    assert_eq!(detail["balance"], "0.00");
    assert_eq!(detail["status"], "active");
    assert_eq!(detail["accountType"], "cash");

    // Options endpoint lists active accounts for pickers.
    let (status, options) = send(
        &test.app,
        req(
            "GET",
            "/api/financial-accounts/options",
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(options.as_array().unwrap().len(), 2);

    // Deactivation: history stays, options drop it, new postings refused.
    let (status, deactivated) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/financial-accounts/{bank}/status-change"),
            &cookie,
            Some(&csrf),
            Some(json!({ "status": "inactive" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deactivated}");
    assert_eq!(deactivated["status"], "inactive");

    let (status, options) = send(
        &test.app,
        req(
            "GET",
            "/api/financial-accounts/options",
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(options.as_array().unwrap().len(), 1);

    // Posting into an inactive account is refused.
    let (status, _) = api_post_payment(&test, &cookie, &csrf, bank, "10.00", "k-inact").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Reactivation restores posting ability.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/financial-accounts/{bank}/status-change"),
            &cookie,
            Some(&csrf),
            Some(json!({ "status": "active" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, created) = api_post_payment(&test, &cookie, &csrf, bank, "10.00", "k-react").await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    // Update with optimistic concurrency: stale expectedUpdatedAt → 409.
    let detail = api_get_account(&test, &cookie, cash).await;
    let expected = detail["updatedAt"].as_str().unwrap().to_string();
    let (status, _) = send(
        &test.app,
        req(
            "PATCH",
            &format!("/api/financial-accounts/{cash}"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "name": "Merkez Kasa (Yeni)",
                "expectedUpdatedAt": "2000-01-01T00:00:00Z"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, updated) = send(
        &test.app,
        req(
            "PATCH",
            &format!("/api/financial-accounts/{cash}"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "name": "Merkez Kasa (Yeni)",
                "expectedUpdatedAt": expected
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["name"], "Merkez Kasa (Yeni)");
}

// ---------------------------------------------------------------------
// Payment -> account movement
// ---------------------------------------------------------------------

#[tokio::test]
async fn payment_posts_exactly_one_inflow_and_derives_balance() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Tahsilat Kasası", "cash").await;

    let (status, created) = api_post_payment(&test, &cookie, &csrf, account, "450.75", "k-1").await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let payment = &created["payment"];
    assert_eq!(payment["destinationAccountId"], account.to_string());
    assert_eq!(payment["destinationAccountName"], "Tahsilat Kasası");

    // Exactly ONE inflow movement, sourced from this Payment.
    let movements = api_movements(&test, &cookie, account).await;
    assert_eq!(movements.len(), 1);
    let movement = &movements[0];
    assert_eq!(movement["direction"], "inflow");
    assert_eq!(movement["amount"], "450.75");
    assert_eq!(movement["effect"], "450.75");
    assert_eq!(movement["sourceType"], "payment");
    assert_eq!(movement["sourceId"], payment["id"]);
    assert_eq!(movement["status"], "active");
    assert_eq!(api_balance(&test, &cookie, account).await, "450.75");

    // Idempotent replay: same key + payload → 200, still ONE movement.
    let (status, replay) = api_post_payment(&test, &cookie, &csrf, account, "450.75", "k-1").await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["replayed"], true);
    assert_eq!(movement_count(&test.pool, "active").await, 1);
}

#[tokio::test]
async fn payment_requires_a_valid_destination_account() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    // Missing destinationAccountId → request never reaches the domain.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/payments",
            &cookie,
            Some(&csrf),
            Some(json!({
                "payerFirstName": "Ödeyen", "payerLastName": "Kişi",
                "amount": "10.00", "method": "cash",
                "idempotencyKey": "k-noacc", "allocations": []
            })),
        ),
    )
    .await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "expected 4xx for missing destinationAccountId, got {status}"
    );

    // Nonexistent account → indistinguishable 404 (no existence leak).
    let (status, _) =
        api_post_payment(&test, &cookie, &csrf, Uuid::new_v4(), "10.00", "k-ghost").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // No Payment and no movement were persisted by any rejection.
    let payments: i64 = sqlx::query_scalar("SELECT count(*) FROM payments")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(payments, 0);
    assert_eq!(movement_count(&test.pool, "active").await, 0);
}

#[tokio::test]
async fn payment_reversal_reverses_the_movement_and_the_balance() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa", "cash").await;

    let (status, created) =
        api_post_payment(&test, &cookie, &csrf, account, "200.00", "k-r1").await;
    assert_eq!(status, StatusCode::CREATED);
    let payment_id = created["payment"]["id"].as_str().unwrap().to_string();
    assert_eq!(api_balance(&test, &cookie, account).await, "200.00");

    let (status, reversed) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/payments/{payment_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "yanlış tahsilat" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reversed}");

    // Original movement preserved as history (reversed), balance back to 0.
    let movements = api_movements(&test, &cookie, account).await;
    assert_eq!(movements.len(), 1);
    assert_eq!(movements[0]["status"], "reversed");
    assert_eq!(movements[0]["reversalReason"], "yanlış tahsilat");
    assert_eq!(api_balance(&test, &cookie, account).await, "0.00");
}

#[tokio::test]
async fn legacy_null_account_payment_reverses_without_account_effect() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let admin_id: Uuid = sqlx::query_scalar("SELECT id FROM users ORDER BY created_at LIMIT 1")
        .fetch_one(&test.pool)
        .await
        .unwrap();

    // Simulate a pre-STEP-008 row: posted Payment, NULL destination,
    // required idempotency columns filled.
    let payer: Uuid = sqlx::query_scalar(
        "INSERT INTO persons (first_name, last_name, search_name) \
         VALUES ('Eski', 'Kayıt', 'eski kayit') RETURNING id",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    let payment_id: Uuid = sqlx::query_scalar(
        "INSERT INTO payments \
            (payer_person_id, amount, currency, method, received_at, status, \
             idempotency_key, idempotency_fingerprint, created_by) \
         VALUES ($1, 100.00, 'TRY', 'cash', now(), 'posted', 'legacy-1', '{}', $2) \
         RETURNING id",
    )
    .bind(payer)
    .bind(admin_id)
    .fetch_one(&test.pool)
    .await
    .unwrap();

    let (status, reversed) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/payments/{payment_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "eski kayıt iptali" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reversed}");
    assert_eq!(reversed["status"], "reversed");
    assert_eq!(reversed["destinationAccountId"], Value::Null);
    assert_eq!(movement_count(&test.pool, "active").await, 0);
}

// ---------------------------------------------------------------------
// Transfers
// ---------------------------------------------------------------------

#[tokio::test]
async fn transfer_posts_paired_movements_atomically() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let cash = api_create_account(&test, &cookie, &csrf, "Kasa", "cash").await;
    let bank = api_create_account(&test, &cookie, &csrf, "Banka", "bank").await;

    let (status, _) = api_post_payment(&test, &cookie, &csrf, cash, "1000.00", "k-fund").await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, transfer) =
        api_post_transfer(&test, &cookie, &csrf, cash, bank, "600.25", "k-t1").await;
    assert_eq!(status, StatusCode::CREATED, "{transfer}");
    assert_eq!(transfer["status"], "posted");
    assert_eq!(transfer["amount"], "600.25");
    assert_eq!(transfer["sourceAccountId"], cash.to_string());
    assert_eq!(transfer["destinationAccountId"], bank.to_string());

    // Two paired legs: source -600.25, destination +600.25.
    let cash_movements = api_movements(&test, &cookie, cash).await;
    let bank_movements = api_movements(&test, &cookie, bank).await;
    assert_eq!(cash_movements.len(), 2);
    assert_eq!(bank_movements.len(), 1);
    let out = cash_movements
        .iter()
        .find(|m| m["sourceType"] == "transfer")
        .unwrap();
    assert_eq!(out["direction"], "outflow");
    assert_eq!(out["effect"], "-600.25");
    assert_eq!(bank_movements[0]["direction"], "inflow");
    assert_eq!(bank_movements[0]["effect"], "600.25");
    // Same logical source on both legs.
    assert_eq!(out["sourceId"], bank_movements[0]["sourceId"]);

    assert_eq!(api_balance(&test, &cookie, cash).await, "399.75");
    assert_eq!(api_balance(&test, &cookie, bank).await, "600.25");
}

#[tokio::test]
async fn transfer_validations_and_insufficient_funds() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let a = api_create_account(&test, &cookie, &csrf, "A", "cash").await;
    let b = api_create_account(&test, &cookie, &csrf, "B", "cash").await;
    let inactive = api_create_account(&test, &cookie, &csrf, "Pasif", "cash").await;
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/financial-accounts/{inactive}/status-change"),
            &cookie,
            Some(&csrf),
            Some(json!({ "status": "inactive" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Same source and destination → 400.
    let (status, _) = api_post_transfer(&test, &cookie, &csrf, a, a, "10.00", "k-same").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Inactive destination → 400.
    let (status, _) =
        api_post_transfer(&test, &cookie, &csrf, a, inactive, "10.00", "k-inact").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Insufficient source balance (empty account) → 409, NOTHING written.
    let (status, _) = api_post_transfer(&test, &cookie, &csrf, a, b, "10.00", "k-empty").await;
    assert_eq!(status, StatusCode::CONFLICT);
    let transfers: i64 = sqlx::query_scalar("SELECT count(*) FROM account_transfers")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(transfers, 0, "rejected transfer left no half-row");
    assert_eq!(movement_count(&test.pool, "active").await, 0);

    // Fund a, then overdraw attempt → 409 again.
    let (status, _) = api_post_payment(&test, &cookie, &csrf, a, "50.00", "k-f").await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = api_post_transfer(&test, &cookie, &csrf, a, b, "50.01", "k-over").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(api_balance(&test, &cookie, a).await, "50.00");
    assert_eq!(api_balance(&test, &cookie, b).await, "0.00");
}

#[tokio::test]
async fn transfer_idempotent_replay_and_conflict() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let a = api_create_account(&test, &cookie, &csrf, "A", "cash").await;
    let b = api_create_account(&test, &cookie, &csrf, "B", "cash").await;
    api_post_payment(&test, &cookie, &csrf, a, "100.00", "k-f").await;

    let (status, first) = api_post_transfer(&test, &cookie, &csrf, a, b, "40.00", "k-t").await;
    assert_eq!(status, StatusCode::CREATED, "{first}");

    // Byte-identical replay → 200, same transfer, no duplicate legs.
    let (status, replay) = api_post_transfer(&test, &cookie, &csrf, a, b, "40.00", "k-t").await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["id"], first["id"]);
    assert_eq!(movement_count(&test.pool, "active").await, 3); // 1 payment + 2 legs

    // Same key, different payload → 409.
    let (status, _) = api_post_transfer(&test, &cookie, &csrf, a, b, "41.00", "k-t").await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn transfer_reversal_reverses_both_legs_idempotently() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let a = api_create_account(&test, &cookie, &csrf, "A", "cash").await;
    let b = api_create_account(&test, &cookie, &csrf, "B", "cash").await;
    api_post_payment(&test, &cookie, &csrf, a, "100.00", "k-f").await;

    let (status, transfer) = api_post_transfer(&test, &cookie, &csrf, a, b, "60.00", "k-t").await;
    assert_eq!(status, StatusCode::CREATED);
    let transfer_id = transfer["id"].as_str().unwrap().to_string();
    assert_eq!(api_balance(&test, &cookie, a).await, "40.00");
    assert_eq!(api_balance(&test, &cookie, b).await, "60.00");

    // Reason required.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/account-transfers/{transfer_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "  " })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, reversed) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/account-transfers/{transfer_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "yanlış transfer" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reversed}");
    assert_eq!(reversed["status"], "reversed");
    assert_eq!(reversed["reversalReason"], "yanlış transfer");

    // BOTH legs flipped to reversed — history preserved, balances restored.
    let a_movements = api_movements(&test, &cookie, a).await;
    let b_movements = api_movements(&test, &cookie, b).await;
    let transfer_legs: Vec<&Value> = a_movements
        .iter()
        .chain(b_movements.iter())
        .filter(|m| m["sourceType"] == "transfer")
        .collect();
    assert_eq!(transfer_legs.len(), 2);
    assert!(transfer_legs.iter().all(|m| m["status"] == "reversed"));
    assert_eq!(api_balance(&test, &cookie, a).await, "100.00");
    assert_eq!(api_balance(&test, &cookie, b).await, "0.00");

    // Idempotent replay: original reason preserved, nothing re-reversed.
    let (status, again) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/account-transfers/{transfer_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "tekrar" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["reversalReason"], "yanlış transfer");
}

#[tokio::test]
async fn concurrent_transfers_serialize_without_overdraft() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let a = api_create_account(&test, &cookie, &csrf, "A", "cash").await;
    let b = api_create_account(&test, &cookie, &csrf, "B", "cash").await;
    api_post_payment(&test, &cookie, &csrf, a, "100.00", "k-f").await;

    // Two racing 80.00 transfers out of a 100.00 balance: exactly ONE
    // may win — the deterministic account lock order + under-lock
    // balance recompute serializes them (ADR-006).
    let first = api_post_transfer(&test, &cookie, &csrf, a, b, "80.00", "k-race-a");
    let second = api_post_transfer(&test, &cookie, &csrf, a, b, "80.00", "k-race-b");
    let (r1, r2) = tokio::join!(first, second);
    let statuses = [r1.0, r2.0];
    assert!(
        statuses.contains(&StatusCode::CREATED) && statuses.contains(&StatusCode::CONFLICT),
        "expected one winner + one insufficient-funds conflict, got {statuses:?} ({r1:?}) ({r2:?})"
    );
    assert_eq!(api_balance(&test, &cookie, a).await, "20.00");
    assert_eq!(api_balance(&test, &cookie, b).await, "80.00");
}

#[tokio::test]
async fn payment_reversal_blocked_when_funds_already_moved() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let a = api_create_account(&test, &cookie, &csrf, "A", "cash").await;
    let b = api_create_account(&test, &cookie, &csrf, "B", "cash").await;

    let (status, created) = api_post_payment(&test, &cookie, &csrf, a, "100.00", "k-p").await;
    assert_eq!(status, StatusCode::CREATED);
    let payment_id = created["payment"]["id"].as_str().unwrap().to_string();

    // The full amount left via a Transfer: reversing the Payment's
    // inflow would take the account NEGATIVE — rejected (docs/15
    // negative-balance rule, operator-approved policy).
    let (status, _) = api_post_transfer(&test, &cookie, &csrf, a, b, "100.00", "k-t").await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/payments/{payment_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "iptal denemesi" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Payment stays posted; no movement touched.
    let (status, detail) = send(
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
    assert_eq!(detail["status"], "posted");
    assert_eq!(movement_count(&test.pool, "reversed").await, 0);
}

// ---------------------------------------------------------------------
// Security matrix
// ---------------------------------------------------------------------

#[tokio::test]
async fn authorization_csrf_idor_and_no_arbitrary_movements() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa", "cash").await;

    // no-store on the financial surface.
    let response = test
        .app
        .clone()
        .oneshot(req("GET", "/api/financial-accounts", &cookie, None, None))
        .await
        .unwrap();
    let cache = response
        .headers()
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(cache.contains("no-store"), "cache-control: {cache}");

    // Unauthenticated → 401.
    let response = test
        .app
        .clone()
        .oneshot(
            Request::get("/api/financial-accounts")
                .header("origin", ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Plain user (no roles) → 403 on every surface.
    let plain = format!("usr.{}", Uuid::new_v4().simple());
    create_plain_user(&test.pool, &plain).await;
    let (plain_cookie, plain_csrf) = login(&test.app, test.peer(), &plain).await;
    for (method, path, body) in [
        ("GET", "/api/financial-accounts".to_string(), None),
        (
            "POST",
            "/api/financial-accounts".to_string(),
            Some(json!({ "name": "X", "accountType": "cash" })),
        ),
        ("GET", format!("/api/financial-accounts/{account}"), None),
        (
            "GET",
            format!("/api/financial-accounts/{account}/movements"),
            None,
        ),
        ("GET", "/api/account-transfers".to_string(), None),
        (
            "POST",
            "/api/account-transfers".to_string(),
            Some(json!({
                "sourceAccountId": account, "destinationAccountId": account,
                "amount": "1.00", "idempotencyKey": "k-p"
            })),
        ),
    ] {
        let (status, detail) = send(
            &test.app,
            req(method, &path, &plain_cookie, Some(&plain_csrf), body),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {detail}");
    }

    // CSRF: valid admin session, missing token → 403.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/financial-accounts",
            &cookie,
            None,
            Some(json!({ "name": "X", "accountType": "cash" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/financial-accounts/{account}/status-change"),
            &cookie,
            None,
            Some(json!({ "status": "inactive" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // IDOR: random ids → 404, never existence leaks.
    for (method, path, body) in [
        (
            "GET",
            format!("/api/financial-accounts/{}", Uuid::new_v4()),
            None,
        ),
        (
            "POST",
            format!("/api/financial-accounts/{}/status-change", Uuid::new_v4()),
            Some(json!({ "status": "inactive" })),
        ),
        (
            "GET",
            format!("/api/financial-accounts/{}/movements", Uuid::new_v4()),
            None,
        ),
        (
            "GET",
            format!("/api/account-transfers/{}", Uuid::new_v4()),
            None,
        ),
    ] {
        let (status, detail) =
            send(&test.app, req(method, &path, &cookie, Some(&csrf), body)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}: {detail}");
    }

    // There is deliberately NO arbitrary movement endpoint — movements
    // are produced only by domain commands (docs/07, ADR-003).
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/account-movements",
            &cookie,
            Some(&csrf),
            Some(json!({ "accountId": account, "amount": "1.00" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
