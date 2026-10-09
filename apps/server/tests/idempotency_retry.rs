//! PILOT-FIX-001 / F4 — Idempotent retry after a timed-out request.
//!
//! The audit found that OPTIONAL timestamp fields (`occurredAt`,
//! `receivedAt`, `settledAt`) are server-defaulted to `now` and the
//! DEFAULTED value was hashed into the idempotency fingerprint. A
//! client that timed out after the commit and retried the identical
//! logical request then produced a DIFFERENT fingerprint — a false
//! 409 instead of a replayed 200.
//!
//! The fix fingerprints CLIENT INTENT: the Option as expressed in the
//! request (`None` → the field was omitted). These tests advance the
//! injected MutableClock between the original request and the retry —
//! the exact production timeout scenario the frozen clock in the older
//! replay tests could never exercise.

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
    clock: Arc<MutableClock>,
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
    let runtime = AuthRuntime::new(config, clock.clone());
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
        clock,
        _peer: format!(
            "127.{}.{}.{}:{}",
            octets[0],
            octets[1],
            octets[2],
            54000 + octets[3] as u16
        )
        .parse()
        .expect("addr"),
        _database_name: database_name,
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
    assert_eq!(response.status(), StatusCode::OK, "login failed");
    let set_cookie = response
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: Value = serde_json::from_slice(&bytes).unwrap();
    (
        set_cookie,
        payload["csrfToken"].as_str().unwrap().to_string(),
    )
}

fn req(
    method: &str,
    uri: &str,
    cookie: &str,
    csrf: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("cookie", cookie)
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
    create_admin(&test.pool, "idempotencyadmin").await;
    login(&test.app, test.peer(), "idempotencyadmin").await
}

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

async fn api_category(test: &TestApp, cookie: &str, kind: &str, name: &str) -> String {
    let (status, categories) = send(
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
    categories
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("category {name} missing"))["id"]
        .as_str()
        .unwrap()
        .to_string()
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
    assert_eq!(status, StatusCode::OK, "{list}");
    list["items"].as_array().unwrap().len()
}

fn entry_body(account: Uuid, category: &str, amount: &str, key: &str) -> Value {
    json!({
        "financialAccountId": account.to_string(),
        "categoryId": category,
        "amount": amount,
        "description": "idem test kaydı",
        "idempotencyKey": key,
    })
}

// ---------------------------------------------------------------------
// F4 — omitted timestamp + retried identical request must REPLAY
// ---------------------------------------------------------------------

#[tokio::test]
async fn income_retry_after_timeout_replays_despite_new_server_now() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let category = api_category(&test, &cookie, "income", "Diğer Gelir").await;

    // Original request: occurredAt OMITTED — the server defaulted it.
    let body = entry_body(account, &category, "750.00", "inc-retry");
    let (status, first) = send(
        &test.app,
        req(
            "POST",
            "/api/incomes",
            &cookie,
            Some(&csrf),
            Some(body.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");

    // Timeout: the client resends the SAME logical request minutes
    // later — server `now` has moved on (MutableClock advanced).
    test.clock.advance_seconds(120);
    let (status, replay) = send(
        &test.app,
        req("POST", "/api/incomes", &cookie, Some(&csrf), Some(body)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "retry of a committed request must replay, not 409: {replay}"
    );
    assert_eq!(replay["id"], first["id"]);
    assert_eq!(api_balance(&test, &cookie, account).await, "750.00");
    assert_eq!(api_movement_count(&test, &cookie, account).await, 1);
}

#[tokio::test]
async fn expense_retry_after_timeout_replays_despite_new_server_now() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let inc_category = api_category(&test, &cookie, "income", "Diğer Gelir").await;
    let exp_category = api_category(&test, &cookie, "expense", "Diğer Gider").await;
    let (status, income) = send(
        &test.app,
        req(
            "POST",
            "/api/incomes",
            &cookie,
            Some(&csrf),
            Some(entry_body(account, &inc_category, "1000.00", "inc-fund")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{income}");

    let body = entry_body(account, &exp_category, "400.00", "exp-retry");
    let (status, first) = send(
        &test.app,
        req(
            "POST",
            "/api/expenses",
            &cookie,
            Some(&csrf),
            Some(body.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");

    test.clock.advance_seconds(120);
    let (status, replay) = send(
        &test.app,
        req("POST", "/api/expenses", &cookie, Some(&csrf), Some(body)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "retry must replay: {replay}");
    assert_eq!(replay["id"], first["id"]);
    assert_eq!(api_balance(&test, &cookie, account).await, "600.00");
    assert_eq!(api_movement_count(&test, &cookie, account).await, 2);
}

#[tokio::test]
async fn transfer_retry_after_timeout_replays_despite_new_server_now() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let a = api_create_account(&test, &cookie, &csrf, "A").await;
    let b = api_create_account(&test, &cookie, &csrf, "B").await;
    let inc_category = api_category(&test, &cookie, "income", "Diğer Gelir").await;
    let (status, income) = send(
        &test.app,
        req(
            "POST",
            "/api/incomes",
            &cookie,
            Some(&csrf),
            Some(entry_body(a, &inc_category, "500.00", "inc-t")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{income}");

    let body = json!({
        "sourceAccountId": a.to_string(),
        "destinationAccountId": b.to_string(),
        "amount": "200.00",
        "idempotencyKey": "tr-retry",
    });
    let (status, first) = send(
        &test.app,
        req(
            "POST",
            "/api/account-transfers",
            &cookie,
            Some(&csrf),
            Some(body.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");

    test.clock.advance_seconds(120);
    let (status, replay) = send(
        &test.app,
        req(
            "POST",
            "/api/account-transfers",
            &cookie,
            Some(&csrf),
            Some(body),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "retry must replay: {replay}");
    assert_eq!(replay["id"], first["id"]);
    assert_eq!(api_balance(&test, &cookie, a).await, "300.00");
    assert_eq!(api_balance(&test, &cookie, b).await, "200.00");
}

#[tokio::test]
async fn payment_retry_after_timeout_replays_despite_new_server_now() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;

    let body = json!({
        "payerFirstName": "Ödeyen",
        "payerLastName": "Kişi",
        "amount": "1250.00",
        "method": "cash",
        "destinationAccountId": account.to_string(),
        "idempotencyKey": "pay-retry",
        "allocations": [],
    });
    let (status, first) = send(
        &test.app,
        req(
            "POST",
            "/api/payments",
            &cookie,
            Some(&csrf),
            Some(body.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");

    test.clock.advance_seconds(120);
    let (status, replay) = send(
        &test.app,
        req("POST", "/api/payments", &cookie, Some(&csrf), Some(body)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "retry must replay: {replay}");
    assert_eq!(replay["id"], first["id"]);
    assert_eq!(api_balance(&test, &cookie, account).await, "1250.00");
    assert_eq!(api_movement_count(&test, &cookie, account).await, 1);
}

#[tokio::test]
async fn settlement_retry_after_timeout_replays_despite_new_server_now() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let inc_category = api_category(&test, &cookie, "income", "Diğer Gelir").await;
    let (status, income) = send(
        &test.app,
        req(
            "POST",
            "/api/incomes",
            &cookie,
            Some(&csrf),
            Some(entry_body(account, &inc_category, "5000.00", "inc-s")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{income}");

    // Shareholder + share + initiated + finalized return → entitlement.
    let (status, shareholder) = send(
        &test.app,
        req(
            "POST",
            "/api/shareholders",
            &cookie,
            Some(&csrf),
            Some(json!({
                "person": { "mode": "new", "firstName": "İade", "lastName": "Eden" },
                "family": { "mode": "new", "sequenceNumber": 92001 }
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{shareholder}");
    let (status, share) = send(
        &test.app,
        req(
            "POST",
            "/api/shares",
            &cookie,
            Some(&csrf),
            Some(json!({
                "shareholderId": shareholder["id"],
                "acquisitionType": "founder",
                "effectiveAt": "2025-01-01T00:00:00Z",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{share}");
    let (status, initiated) = send(
        &test.app,
        req(
            "POST",
            "/api/share-returns",
            &cookie,
            Some(&csrf),
            Some(json!({
                "shareId": share["id"],
                "effectiveReturnDate": "2026-01-15",
                "reason": "üyelikten ayrılma",
                "idempotencyKey": "init-r",
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
                    { "entitlementType": "principal", "amount": "3000.00" }
                ],
                "expectedUpdatedAt": initiated["updatedAt"],
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{finalized}");
    let entitlement_id = finalized["entitlements"][0]["id"].as_str().unwrap();

    let body = json!({
        "financialAccountId": account.to_string(),
        "amount": "1000.00",
        "idempotencyKey": "st-retry",
    });
    let (status, first) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-return-entitlements/{entitlement_id}/settlements"),
            &cookie,
            Some(&csrf),
            Some(body.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");

    test.clock.advance_seconds(120);
    let (status, replay) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-return-entitlements/{entitlement_id}/settlements"),
            &cookie,
            Some(&csrf),
            Some(body),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "retry must replay: {replay}");
    assert_eq!(replay["id"], first["id"]);
    assert_eq!(api_balance(&test, &cookie, account).await, "4000.00");
}

// ---------------------------------------------------------------------
// Intent semantics preserved
// ---------------------------------------------------------------------

#[tokio::test]
async fn explicit_timestamp_retries_replay_after_clock_advances() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let category = api_category(&test, &cookie, "income", "Diğer Gelir").await;

    let mut body = entry_body(account, &category, "300.00", "inc-exp");
    body["occurredAt"] = json!("2026-01-14T12:00:00Z");
    let (status, first) = send(
        &test.app,
        req(
            "POST",
            "/api/incomes",
            &cookie,
            Some(&csrf),
            Some(body.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");

    test.clock.advance_seconds(120);
    let (status, replay) = send(
        &test.app,
        req("POST", "/api/incomes", &cookie, Some(&csrf), Some(body)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["id"], first["id"]);
    assert_eq!(api_balance(&test, &cookie, account).await, "300.00");
}

#[tokio::test]
async fn different_payload_same_key_still_conflicts() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let category = api_category(&test, &cookie, "income", "Diğer Gelir").await;

    let (status, first) = send(
        &test.app,
        req(
            "POST",
            "/api/incomes",
            &cookie,
            Some(&csrf),
            Some(entry_body(account, &category, "300.00", "inc-mix")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");

    test.clock.advance_seconds(60);
    // Same key, DIFFERENT amount → still 409.
    let (status, conflict) = send(
        &test.app,
        req(
            "POST",
            "/api/incomes",
            &cookie,
            Some(&csrf),
            Some(entry_body(account, &category, "301.00", "inc-mix")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict}");

    // Same key, omitted-vs-explicit timestamp → different intent → 409.
    let mut explicit = entry_body(account, &category, "300.00", "inc-mix");
    explicit["occurredAt"] = json!("2026-01-14T12:00:00Z");
    let (status, conflict2) = send(
        &test.app,
        req("POST", "/api/incomes", &cookie, Some(&csrf), Some(explicit)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict2}");
    assert_eq!(api_balance(&test, &cookie, account).await, "300.00");
    assert_eq!(api_movement_count(&test, &cookie, account).await, 1);
}

#[tokio::test]
async fn concurrent_same_key_requests_produce_single_effect() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    let category = api_category(&test, &cookie, "income", "Diğer Gelir").await;

    let body = entry_body(account, &category, "500.00", "inc-cc");
    let app = test.app.clone();
    let c1 = cookie.clone();
    let s1 = csrf.clone();
    let b1 = body.clone();
    let h1 = tokio::spawn(async move {
        send(&app, req("POST", "/api/incomes", &c1, Some(&s1), Some(b1))).await
    });
    let h2 = tokio::spawn({
        let app = test.app.clone();
        let cookie = cookie.clone();
        let csrf = csrf.clone();
        async move {
            send(
                &app,
                req("POST", "/api/incomes", &cookie, Some(&csrf), Some(body)),
            )
            .await
        }
    });
    let (r1, r2) = (h1.await.unwrap(), h2.await.unwrap());
    let statuses: Vec<StatusCode> = vec![r1.0, r2.0];
    assert!(
        statuses.contains(&StatusCode::CREATED),
        "one request must commit: {statuses:?}"
    );
    assert!(
        statuses
            .iter()
            .all(|s| *s == StatusCode::CREATED || *s == StatusCode::OK),
        "both requests must succeed (one commits, the other replays): {statuses:?}"
    );
    assert_eq!(api_balance(&test, &cookie, account).await, "500.00");
    assert_eq!(api_movement_count(&test, &cookie, account).await, 1);
}
