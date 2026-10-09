//! REQ-006: strict API write validation — every mutating JSON request
//! DTO carries `#[serde(deny_unknown_fields)]`. An unknown top-level or
//! nested field must fail with 422 `validation_failed` BEFORE the
//! handler runs, so a rejected write never mutates the database.
//!
//! One negative test per mutating router family, plus the FUNC-AUDIT-001
//! §F probe (`currency:"USD"` on account creation must not silently
//! produce a TRY account) and nested-payload coverage (person refs,
//! payment allocations, entitlement specs).
//!
//! Requires `KOOPERATIF_TEST_DATABASE_URL`; each test runs in its own
//! throwaway database.

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
    peer: SocketAddr,
    _database_name: String,
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
        peer: format!(
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

async fn post(
    app: &axum::Router,
    path: &str,
    cookie: &str,
    csrf: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .header("cookie", format!("kooperatif_session={cookie}"))
                .header("origin", ORIGIN)
                .header("x-csrf-token", csrf)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(json!(null));
    (status, json)
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count")
}

/// Asserts the stable contract: 422 + `validation_failed`, nothing leaked.
fn assert_validation_failed(status: StatusCode, body: &Value) {
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
    assert_eq!(body["error"]["code"], "validation_failed", "body: {body}");
}

#[tokio::test]
async fn account_create_rejects_unsupported_currency_field() {
    // FUNC-AUDIT-001 §F probe: `currency` is not part of the contract —
    // it must fail loudly instead of silently creating a TRY account.
    let Some(t) = setup().await else { return };
    create_admin(&t.pool, "admin").await;
    let (cookie, csrf) = login(&t.app, t.peer, "admin").await;

    let (status, body) = post(
        &t.app,
        "/api/financial-accounts",
        &cookie,
        &csrf,
        json!({ "name": "Kasa", "accountType": "cash", "currency": "USD" }),
    )
    .await;

    assert_validation_failed(status, &body);
    assert_eq!(count(&t.pool, "financial_accounts").await, 0);
}

#[tokio::test]
async fn shareholder_create_rejects_unknown_top_level_field() {
    let Some(t) = setup().await else { return };
    create_admin(&t.pool, "admin").await;
    let (cookie, csrf) = login(&t.app, t.peer, "admin").await;

    let (status, body) = post(
        &t.app,
        "/api/shareholders",
        &cookie,
        &csrf,
        json!({
            "person": { "mode": "new", "firstName": "Ali", "lastName": "Veli" },
            "family": { "mode": "new", "sequenceNumber": 1 },
            "idempotencyKey": "must-not-be-accepted"
        }),
    )
    .await;

    assert_validation_failed(status, &body);
    assert_eq!(count(&t.pool, "shareholders").await, 0);
}

#[tokio::test]
async fn shareholder_create_rejects_unknown_nested_person_field() {
    let Some(t) = setup().await else { return };
    create_admin(&t.pool, "admin").await;
    let (cookie, csrf) = login(&t.app, t.peer, "admin").await;

    let (status, body) = post(
        &t.app,
        "/api/shareholders",
        &cookie,
        &csrf,
        json!({
            "person": { "mode": "new", "firstName": "Ali", "lastName": "Veli", "nickname": "x" },
            "family": { "mode": "new", "sequenceNumber": 1 }
        }),
    )
    .await;

    assert_validation_failed(status, &body);
    assert_eq!(count(&t.pool, "shareholders").await, 0);
    assert_eq!(count(&t.pool, "persons").await, 0);
}

#[tokio::test]
async fn period_create_rejects_unknown_field() {
    let Some(t) = setup().await else { return };
    create_admin(&t.pool, "admin").await;
    let (cookie, csrf) = login(&t.app, t.peer, "admin").await;

    let (status, body) = post(
        &t.app,
        "/api/periods",
        &cookie,
        &csrf,
        json!({
            "name": "Dönem", "collectionStartDate": "2026-01-01",
            "dueDate": "2026-01-31", "ruleType": "per_shareholder",
            "baseAmount": "100.00", "idempotencyKey": "nope"
        }),
    )
    .await;

    assert_validation_failed(status, &body);
    assert_eq!(count(&t.pool, "periods").await, 0);
}

#[tokio::test]
async fn payment_create_rejects_unknown_fields_top_and_nested() {
    let Some(t) = setup().await else { return };
    create_admin(&t.pool, "admin").await;
    let (cookie, csrf) = login(&t.app, t.peer, "admin").await;

    let (status, body) = post(
        &t.app,
        "/api/payments",
        &cookie,
        &csrf,
        json!({
            "payerPersonId": Uuid::new_v4(), "amount": "10.00", "method": "cash",
            "idempotencyKey": "k", "unexpected": true
        }),
    )
    .await;
    assert_validation_failed(status, &body);

    let (status, body) = post(
        &t.app,
        "/api/payments",
        &cookie,
        &csrf,
        json!({
            "payerPersonId": Uuid::new_v4(), "amount": "10.00", "method": "cash",
            "idempotencyKey": "k",
            "allocations": [{ "assessmentId": Uuid::new_v4(), "amount": "10.00", "extra": 1 }]
        }),
    )
    .await;
    assert_validation_failed(status, &body);
    assert_eq!(count(&t.pool, "payments").await, 0);
}

#[tokio::test]
async fn social_aid_fund_create_rejects_unknown_field() {
    let Some(t) = setup().await else { return };
    create_admin(&t.pool, "admin").await;
    let (cookie, csrf) = login(&t.app, t.peer, "admin").await;

    let (status, body) = post(
        &t.app,
        "/api/social-aid/funds",
        &cookie,
        &csrf,
        json!({ "name": "Fon", "idempotencyKey": "k", "bogus": 1 }),
    )
    .await;

    assert_validation_failed(status, &body);
    assert_eq!(count(&t.pool, "social_aid_funds").await, 0);
}

#[tokio::test]
async fn governance_body_create_rejects_unknown_field() {
    let Some(t) = setup().await else { return };
    create_admin(&t.pool, "admin").await;
    let (cookie, csrf) = login(&t.app, t.peer, "admin").await;

    let (status, body) = post(
        &t.app,
        "/api/governance/bodies",
        &cookie,
        &csrf,
        json!({ "name": "Genel Kurul", "idempotencyKey": "k", "bogus": 1 }),
    )
    .await;

    assert_validation_failed(status, &body);
    assert_eq!(count(&t.pool, "governance_bodies").await, 0);
}

#[tokio::test]
async fn income_entry_rejects_unknown_field() {
    let Some(t) = setup().await else { return };
    create_admin(&t.pool, "admin").await;
    let (cookie, csrf) = login(&t.app, t.peer, "admin").await;

    let (status, body) = post(
        &t.app,
        "/api/incomes",
        &cookie,
        &csrf,
        json!({
            "accountId": Uuid::new_v4(), "categoryId": Uuid::new_v4(),
            "amount": "10.00", "entryDate": "2026-01-01",
            "idempotencyKey": "k", "bogus": 1
        }),
    )
    .await;

    assert_validation_failed(status, &body);
    assert_eq!(count(&t.pool, "income_entries").await, 0);
}

#[tokio::test]
async fn investment_create_rejects_unknown_field() {
    let Some(t) = setup().await else { return };
    create_admin(&t.pool, "admin").await;
    let (cookie, csrf) = login(&t.app, t.peer, "admin").await;

    let (status, body) = post(
        &t.app,
        "/api/investments",
        &cookie,
        &csrf,
        json!({ "name": "Yatırım", "kind": "property", "idempotencyKey": "k", "bogus": 1 }),
    )
    .await;

    assert_validation_failed(status, &body);
    assert_eq!(count(&t.pool, "investments").await, 0);
}

#[tokio::test]
async fn share_create_rejects_unknown_field() {
    let Some(t) = setup().await else { return };
    create_admin(&t.pool, "admin").await;
    let (cookie, csrf) = login(&t.app, t.peer, "admin").await;

    let (status, body) = post(
        &t.app,
        "/api/shares",
        &cookie,
        &csrf,
        json!({
            "shareholderId": Uuid::new_v4(), "acquisitionType": "founder", "bogus": 1
        }),
    )
    .await;

    assert_validation_failed(status, &body);
    assert_eq!(count(&t.pool, "shares").await, 0);
}

#[tokio::test]
async fn share_return_rejects_unknown_nested_entitlement_field() {
    let Some(t) = setup().await else { return };
    create_admin(&t.pool, "admin").await;
    let (cookie, csrf) = login(&t.app, t.peer, "admin").await;

    let (status, body) = post(
        &t.app,
        "/api/share-returns",
        &cookie,
        &csrf,
        json!({
            "shareholderId": Uuid::new_v4(), "idempotencyKey": "k",
            "entitlementSpecs": [{ "kind": "principal", "bogus": 1 }]
        }),
    )
    .await;

    assert_validation_failed(status, &body);
    assert_eq!(count(&t.pool, "share_returns").await, 0);
}

#[tokio::test]
async fn credit_assign_rejects_unknown_field() {
    let Some(t) = setup().await else { return };
    create_admin(&t.pool, "admin").await;
    let (cookie, csrf) = login(&t.app, t.peer, "admin").await;

    let (status, body) = post(
        &t.app,
        &format!("/api/payments/{}/credits", Uuid::new_v4()),
        &cookie,
        &csrf,
        json!({
            "shareholderId": Uuid::new_v4(), "amount": "1.00",
            "idempotencyKey": "k", "bogus": 1
        }),
    )
    .await;

    assert_validation_failed(status, &body);
    assert_eq!(count(&t.pool, "shareholder_credits").await, 0);
}

#[tokio::test]
async fn account_transfer_rejects_unknown_field() {
    let Some(t) = setup().await else { return };
    create_admin(&t.pool, "admin").await;
    let (cookie, csrf) = login(&t.app, t.peer, "admin").await;

    let (status, body) = post(
        &t.app,
        "/api/account-transfers",
        &cookie,
        &csrf,
        json!({
            "fromAccountId": Uuid::new_v4(), "toAccountId": Uuid::new_v4(),
            "amount": "1.00", "idempotencyKey": "k", "bogus": 1
        }),
    )
    .await;

    assert_validation_failed(status, &body);
    assert_eq!(count(&t.pool, "account_transfers").await, 0);
}

#[tokio::test]
async fn role_create_rejects_unknown_field() {
    let Some(t) = setup().await else { return };
    create_admin(&t.pool, "admin").await;
    let (cookie, csrf) = login(&t.app, t.peer, "admin").await;

    let before = count(&t.pool, "roles").await;
    let (status, body) = post(
        &t.app,
        "/api/roles",
        &cookie,
        &csrf,
        json!({ "name": "Rol", "bogus": 1 }),
    )
    .await;

    assert_validation_failed(status, &body);
    assert_eq!(count(&t.pool, "roles").await, before);
}

#[tokio::test]
async fn login_rejects_unknown_field_and_creates_no_session() {
    let Some(t) = setup().await else { return };
    create_admin(&t.pool, "admin").await;

    let before = count(&t.pool, "user_sessions").await;
    let mut request = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .body(Body::from(
            json!({ "username": "admin", "password": TEST_PASSWORD, "bogus": 1 }).to_string(),
        ))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(t.peer));
    let response = t.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();

    // Login maps the JsonRejection itself to ApiError::ValidationFailed
    // (400) — the stable contract is the `validation_failed` code;
    // handler-level validation is 400, extractor-level rejection is 422.
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert_eq!(body["error"]["code"], "validation_failed", "body: {body}");
    assert_eq!(count(&t.pool, "user_sessions").await, before);
}
