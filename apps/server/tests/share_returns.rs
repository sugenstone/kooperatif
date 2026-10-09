//! Database-gated Share Return / Entitlement / Settlement tests
//! (STEP-011, docs/10, docs/16, docs/19). Requires
//! KOOPERATIF_TEST_DATABASE_URL; each test runs in its own throwaway
//! database.
//!
//! Invariants under test:
//! - THREE separate layers: share lifecycle (active -> return_pending
//!   -> closed) ≠ entitlement (cooperative obligation) ≠ settlement
//!   (real money outflow).
//! - Recognition/crystallization moves ZERO money; settlement posts
//!   exactly ONE `share_return_settlement` outflow movement — never
//!   income, expense, payment or transfer.
//! - Ownership ends at the effective return date; history, debts and
//!   credits are preserved; unrelated shares are untouched.
//! - Entitlement amounts are operator-entered snapshots (formulas are
//!   POLICY_DRIVEN); NULL amount = undetermined right, never zero.
//! - Partial settlement derives remaining; over-settlement, account
//!   overspend and concurrent double-pay are impossible.
//! - Reversal preserves history and restores remaining; idempotent
//!   commands replay safely.

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
    // Clock frozen at 2026-01-15 — effective return dates around it.
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

async fn api_create_shareholder(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    first: &str,
    last: &str,
    family_sequence: i64,
) -> Uuid {
    let body = json!({
        "person": { "mode": "new", "firstName": first, "lastName": last },
        "family": { "mode": "new", "sequenceNumber": family_sequence }
    });
    let (status, detail) = send(
        &test.app,
        req("POST", "/api/shareholders", cookie, Some(csrf), Some(body)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "shareholder: {detail}");
    detail["id"].as_str().unwrap().parse().unwrap()
}

async fn api_create_share(test: &TestApp, cookie: &str, csrf: &str, shareholder_id: Uuid) -> Value {
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/shares",
            cookie,
            Some(csrf),
            Some(json!({
                "shareholderId": shareholder_id,
                "acquisitionType": "founder",
                // Backdated so a same-day effective return is legal
                // (cutoff = effective date 00:00 Istanbul must not
                // precede the ownership start).
                "effectiveAt": "2025-01-01T00:00:00Z",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "share create: {detail}");
    detail
}

async fn api_get_share(test: &TestApp, cookie: &str, share_id: &str) -> Value {
    let (status, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/shares/{share_id}"),
            cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail
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

async fn initiate(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    share_id: &str,
    effective_date: &str,
    key: &str,
) -> (StatusCode, Value) {
    send(
        &test.app,
        req(
            "POST",
            "/api/share-returns",
            cookie,
            Some(csrf),
            Some(json!({
                "shareId": share_id,
                "effectiveReturnDate": effective_date,
                "reason": "üyelikten ayrılma",
                "idempotencyKey": key,
            })),
        ),
    )
    .await
}

async fn finalize(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    return_id: &str,
    entitlements: Value,
    expected_updated_at: &str,
) -> (StatusCode, Value) {
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-returns/{return_id}/finalize"),
            cookie,
            Some(csrf),
            Some(json!({
                "entitlements": entitlements,
                "expectedUpdatedAt": expected_updated_at,
            })),
        ),
    )
    .await
}

async fn settle(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    entitlement_id: &str,
    account: Uuid,
    amount: &str,
    key: &str,
) -> (StatusCode, Value) {
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-return-entitlements/{entitlement_id}/settlements"),
            cookie,
            Some(csrf),
            Some(json!({
                "financialAccountId": account.to_string(),
                "amount": amount,
                "idempotencyKey": key,
            })),
        ),
    )
    .await
}

/// Full pipeline: shareholder + share + initiated return at 2026-01-15.
async fn pending_return(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    family_sequence: i64,
) -> (Uuid, Value) {
    let shareholder =
        api_create_shareholder(test, cookie, csrf, "İade", "Eden", family_sequence).await;
    let share = api_create_share(test, cookie, csrf, shareholder).await;
    let share_id = share["id"].as_str().unwrap();
    let (status, detail) = initiate(
        test,
        cookie,
        csrf,
        share_id,
        "2026-01-15",
        &format!("init-{family_sequence}"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "initiate: {detail}");
    (shareholder, detail)
}

// ---------------------------------------------------------------------
// Initiation
// ---------------------------------------------------------------------

#[tokio::test]
async fn initiate_moves_share_to_return_pending_and_replays() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let (_sh, detail) = pending_return(&test, &cookie, &csrf, 700).await;

    assert_eq!(detail["status"], "pending");
    assert_eq!(detail["effectiveReturnDate"], "2026-01-15");
    assert!(detail["returnNumber"].as_i64().unwrap() > 0);
    assert_eq!(detail["entitlements"].as_array().unwrap().len(), 0);

    // Share is return_pending; ownership still open (cutoff is later).
    let share = api_get_share(&test, &cookie, detail["shareId"].as_str().unwrap()).await;
    assert_eq!(share["status"], "return_pending");
    assert!(share["owner"].is_object(), "owner remains until finalize");
    let has_return_event = share["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["eventType"] == "return_requested");
    assert!(has_return_event, "return_requested event recorded");

    // Idempotent replay: same key + payload → 200 same return.
    let (status, replay) = initiate(
        &test,
        &cookie,
        &csrf,
        detail["shareId"].as_str().unwrap(),
        "2026-01-15",
        "init-700",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "replay: {replay}");
    assert_eq!(replay["id"], detail["id"]);

    // Same key, different payload → 409.
    let (status, _) = initiate(
        &test,
        &cookie,
        &csrf,
        detail["shareId"].as_str().unwrap(),
        "2026-01-14",
        "init-700",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "key+payload drift");

    // Second return on the same share → rejected (share not eligible
    // while a return is pending, or the one-pending unique index).
    let (status, conflict) = initiate(
        &test,
        &cookie,
        &csrf,
        detail["shareId"].as_str().unwrap(),
        "2026-01-15",
        "init-700-b",
    )
    .await;
    assert!(
        status == StatusCode::CONFLICT || status == StatusCode::BAD_REQUEST,
        "{conflict}"
    );

    // Audit evidence exists once.
    let audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events WHERE event_type = 'share_return_requested'",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(audit, 1);
}

#[tokio::test]
async fn initiate_rejects_ineligible_share_and_bad_dates() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let shareholder = api_create_shareholder(&test, &cookie, &csrf, "X", "Y", 701).await;
    let share = api_create_share(&test, &cookie, &csrf, shareholder).await;
    let share_id = share["id"].as_str().unwrap();

    // Future effective date rejected.
    let (status, _) = initiate(&test, &cookie, &csrf, share_id, "2026-06-01", "fut").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "future date");
    // Malformed date.
    let (status, _) = initiate(&test, &cookie, &csrf, share_id, "15.01.2026", "bad").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "bad date");
    // Date before ownership start (share created at clock 2026-01-15).
    let (status, _) = initiate(&test, &cookie, &csrf, share_id, "2020-01-01", "early").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "before ownership");

    // Suspended share is not eligible.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shares/{share_id}/status-change"),
            &cookie,
            Some(&csrf),
            Some(json!({ "to": "suspended" })),
        ),
    )
    .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::NO_CONTENT,
        "suspend: {status}"
    );
    let (status, _) = initiate(&test, &cookie, &csrf, share_id, "2026-01-15", "susp").await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::CONFLICT,
        "suspended share must not enter return: {status}"
    );

    // Unknown share → 404/422.
    let (status, _) = initiate(
        &test,
        &cookie,
        &csrf,
        &Uuid::new_v4().to_string(),
        "2026-01-15",
        "ghost",
    )
    .await;
    assert!(status == StatusCode::NOT_FOUND || status == StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn cancel_pending_restores_active_share() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let (_sh, detail) = pending_return(&test, &cookie, &csrf, 702).await;
    let return_id = detail["id"].as_str().unwrap();
    let share_id = detail["shareId"].as_str().unwrap();

    // Empty reason rejected.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-returns/{return_id}/cancel"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "  " })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, cancelled) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-returns/{return_id}/cancel"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "yanlış talep" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");
    assert_eq!(cancelled["status"], "cancelled");
    assert!(cancelled["cancelledAt"].is_string());

    let share = api_get_share(&test, &cookie, share_id).await;
    assert_eq!(share["status"], "active");

    // The share can enter a fresh return afterwards.
    let (status, again) =
        initiate(&test, &cookie, &csrf, share_id, "2026-01-15", "init-702b").await;
    assert_eq!(status, StatusCode::CREATED, "{again}");

    // A cancelled return cannot be finalized.
    let (status, _) = finalize(
        &test,
        &cookie,
        &csrf,
        return_id,
        json!([]),
        cancelled["updatedAt"].as_str().unwrap(),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "cancelled is terminal");
}

// ---------------------------------------------------------------------
// Finalize — crystallization + share closure + ownership end
// ---------------------------------------------------------------------

#[tokio::test]
async fn finalize_crystallizes_rights_closes_share_preserves_history() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let shareholder = api_create_shareholder(&test, &cookie, &csrf, "Çıkan", "Üye", 703).await;
    let share1 = api_create_share(&test, &cookie, &csrf, shareholder).await;
    let share2 = api_create_share(&test, &cookie, &csrf, shareholder).await;
    let share1_id = share1["id"].as_str().unwrap().to_string();

    let (status, detail) =
        initiate(&test, &cookie, &csrf, &share1_id, "2026-01-10", "init-703").await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");
    let return_id = detail["id"].as_str().unwrap();

    // Stale expectedUpdatedAt → conflict.
    let (status, _) = finalize(
        &test,
        &cookie,
        &csrf,
        return_id,
        json!([{"entitlementType": "principal", "amount": "1000.00"}]),
        "2020-01-01T00:00:00Z",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "stale precondition");

    // Finalize: principal determined now, profit right undetermined.
    let (status, fin) = finalize(
        &test,
        &cookie,
        &csrf,
        return_id,
        json!([
            {
                "entitlementType": "principal",
                "amount": "1000.00",
                "dueDate": "2026-03-01",
                "policyReference": "YK-2026-07",
                "description": "ana para iadesi"
            },
            {
                "entitlementType": "profit",
                "dueDate": null,
                "policyReference": "YK-2026-08"
            }
        ]),
        detail["updatedAt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "finalize: {fin}");
    assert_eq!(fin["status"], "finalized");
    assert!(fin["finalizedAt"].is_string());

    let ents = fin["entitlements"].as_array().unwrap();
    assert_eq!(ents.len(), 2);
    let principal = ents
        .iter()
        .find(|e| e["entitlementType"] == "principal")
        .unwrap();
    let profit = ents
        .iter()
        .find(|e| e["entitlementType"] == "profit")
        .unwrap();
    assert_eq!(principal["amount"], "1000.00");
    assert_eq!(principal["dueDate"], "2026-03-01");
    assert_eq!(principal["policyReference"], "YK-2026-07");
    assert_eq!(principal["status"], "open");
    assert_eq!(principal["remainingAmount"], "1000.00");
    assert_eq!(principal["settledAmount"], "0.00");
    assert_eq!(
        principal["beneficiaryShareholderId"],
        shareholder.to_string()
    );
    // NULL ≠ 0.00: the profit right exists but is undetermined.
    assert!(profit["amount"].is_null());
    assert!(profit["remainingAmount"].is_null());
    assert_eq!(profit["status"], "open");
    assert_eq!(profit["dueState"], "undetermined");

    // Share closed; ownership interval ended at the effective date.
    let closed = api_get_share(&test, &cookie, &share1_id).await;
    assert_eq!(closed["status"], "closed");
    assert!(closed["owner"].is_null(), "closed share has no owner");
    assert!(closed["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["eventType"] == "return_finalized"));
    let ended: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT ended_at FROM share_ownerships WHERE share_id = $1")
            .bind(share1_id.parse::<Uuid>().unwrap())
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert!(ended.is_some(), "ownership ended at the effective date");

    // The other share of the same shareholder is untouched.
    let other = api_get_share(&test, &cookie, share2["id"].as_str().unwrap()).await;
    assert_eq!(other["status"], "active");
    assert!(other["owner"].is_object());

    // Crystallization created ZERO movements anywhere.
    let movement_count: i64 = sqlx::query_scalar("SELECT count(*) FROM account_movements")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(movement_count, 0, "recognition is not money movement");
}

#[tokio::test]
async fn finalize_rejects_duplicate_types_and_allows_no_rights() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let (_sh, detail) = pending_return(&test, &cookie, &csrf, 704).await;
    let return_id = detail["id"].as_str().unwrap();
    let expected = detail["updatedAt"].as_str().unwrap();

    // Duplicate type → 409 (one right per type per case).
    let (status, _) = finalize(
        &test,
        &cookie,
        &csrf,
        return_id,
        json!([
            {"entitlementType": "principal", "amount": "10.00"},
            {"entitlementType": "principal", "amount": "20.00"}
        ]),
        expected,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "duplicate type");

    // Bad amount → 400.
    let (status, _) = finalize(
        &test,
        &cookie,
        &csrf,
        return_id,
        json!([{"entitlementType": "profit", "amount": "-5.00"}]),
        expected,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "negative amount");

    // An explicit empty rights list is allowed: the share closes with
    // no crystallized obligation (a "no rights owed" decision).
    let (status, fin) = finalize(&test, &cookie, &csrf, return_id, json!([]), expected).await;
    assert_eq!(status, StatusCode::OK, "empty spec finalize: {fin}");
    assert_eq!(fin["status"], "finalized");
    assert_eq!(fin["entitlements"].as_array().unwrap().len(), 0);
}

// ---------------------------------------------------------------------
// Settlement
// ---------------------------------------------------------------------

#[tokio::test]
async fn settlement_posts_exactly_one_outflow_is_neither_income_nor_payment() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    api_fund_account(&test, &cookie, &csrf, account, "5000.00", "fund-s1").await;
    let (_sh, detail) = pending_return(&test, &cookie, &csrf, 705).await;
    let return_id = detail["id"].as_str().unwrap();
    let (_status, fin) = finalize(
        &test,
        &cookie,
        &csrf,
        return_id,
        json!([{"entitlementType": "principal", "amount": "1000.00", "dueDate": "2026-02-01"}]),
        detail["updatedAt"].as_str().unwrap(),
    )
    .await;
    let entitlement = &fin["entitlements"][0];
    let ent_id = entitlement["id"].as_str().unwrap();

    // Partial settlement 400 → one outflow movement with dedicated source.
    let (status, st) = settle(&test, &cookie, &csrf, ent_id, account, "400.00", "st-1").await;
    assert_eq!(status, StatusCode::CREATED, "settle: {st}");
    assert_eq!(st["amount"], "400.00");
    assert_eq!(st["status"], "posted");
    assert_eq!(st["entitlementType"], "principal");
    assert_eq!(api_balance(&test, &cookie, account).await, "4600.00");

    let movements = api_movements(&test, &cookie, account).await;
    let outflows: Vec<_> = movements
        .iter()
        .filter(|m| m["sourceType"] == "share_return_settlement")
        .collect();
    assert_eq!(outflows.len(), 1, "exactly one settlement movement");
    assert_eq!(outflows[0]["direction"], "outflow");
    assert_eq!(outflows[0]["amount"], "400.00");
    assert_eq!(outflows[0]["sourceId"], st["id"]);
    assert_eq!(outflows[0]["id"], st["accountMovementId"]);

    // NOT payment / expense / transfer / income.
    let counts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT source_type::text, count(*)::bigint FROM account_movements GROUP BY source_type",
    )
    .fetch_all(&test.pool)
    .await
    .unwrap();
    let settlement_sources = counts
        .iter()
        .filter(|(s, _)| s == "share_return_settlement")
        .count();
    assert_eq!(settlement_sources, 1);
    let payments: i64 = sqlx::query_scalar("SELECT count(*) FROM payments")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    let expenses: i64 = sqlx::query_scalar("SELECT count(*) FROM expense_entries")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    let transfers: i64 = sqlx::query_scalar("SELECT count(*) FROM account_transfers")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    let credits: i64 = sqlx::query_scalar("SELECT count(*) FROM shareholder_credits")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!((payments, expenses, transfers, credits), (0, 0, 0, 0));

    // Income/expense summary unchanged (funding 5000 income, 0 expense).
    let (status, summary) = send(
        &test.app,
        req("GET", "/api/income-expense/summary", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(summary["incomeTotal"], "5000.00");
    assert_eq!(summary["expenseTotal"], "0.00");

    // Remaining derived: 1000 - 400 = 600, partially_settled.
    let (status, detail2) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/share-returns/{return_id}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ent = &detail2["entitlements"][0];
    assert_eq!(ent["settledAmount"], "400.00");
    assert_eq!(ent["remainingAmount"], "600.00");
    assert_eq!(ent["status"], "partially_settled");

    // Second settlement of the remainder → settled.
    let (status, st2) = settle(&test, &cookie, &csrf, ent_id, account, "600.00", "st-2").await;
    assert_eq!(status, StatusCode::CREATED, "{st2}");
    let (status, detail3) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/share-returns/{return_id}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail3["entitlements"][0]["status"], "settled");
    assert_eq!(detail3["entitlements"][0]["remainingAmount"], "0.00");
    assert_eq!(api_balance(&test, &cookie, account).await, "4000.00");
}

#[tokio::test]
async fn settlement_boundaries_over_amount_undetermined_insufficient() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    api_fund_account(&test, &cookie, &csrf, account, "500.00", "fund-b1").await;
    let (_sh, detail) = pending_return(&test, &cookie, &csrf, 706).await;
    let return_id = detail["id"].as_str().unwrap();
    let (_s, fin) = finalize(
        &test,
        &cookie,
        &csrf,
        return_id,
        json!([
            {"entitlementType": "principal", "amount": "800.00"},
            {"entitlementType": "profit"}
        ]),
        detail["updatedAt"].as_str().unwrap(),
    )
    .await;
    let principal = fin["entitlements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["entitlementType"] == "principal")
        .unwrap()
        .clone();
    let profit = fin["entitlements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["entitlementType"] == "profit")
        .unwrap()
        .clone();

    // Undetermined amount → not settleable.
    let (status, _) = settle(
        &test,
        &cookie,
        &csrf,
        profit["id"].as_str().unwrap(),
        account,
        "100.00",
        "st-u",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "undetermined amount");

    // Over-remaining → 409.
    let (status, _) = settle(
        &test,
        &cookie,
        &csrf,
        principal["id"].as_str().unwrap(),
        account,
        "800.01",
        "st-over",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "over settlement");

    // Insufficient account funds → 409, no partial effects.
    let (status, _) = settle(
        &test,
        &cookie,
        &csrf,
        principal["id"].as_str().unwrap(),
        account,
        "600.00",
        "st-nsf",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "insufficient funds");
    assert_eq!(api_balance(&test, &cookie, account).await, "500.00");
    let settlements: i64 = sqlx::query_scalar("SELECT count(*) FROM share_return_settlements")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(settlements, 0, "failed settlement must not survive");

    // Unknown account → not found / validation.
    let (status, _) = settle(
        &test,
        &cookie,
        &csrf,
        principal["id"].as_str().unwrap(),
        Uuid::new_v4(),
        "100.00",
        "st-na",
    )
    .await;
    assert!(
        status == StatusCode::NOT_FOUND
            || status == StatusCode::BAD_REQUEST
            || status == StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn settlement_idempotent_replay_and_conflict() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    api_fund_account(&test, &cookie, &csrf, account, "2000.00", "fund-i").await;
    let (_sh, detail) = pending_return(&test, &cookie, &csrf, 707).await;
    let (_s, fin) = finalize(
        &test,
        &cookie,
        &csrf,
        detail["id"].as_str().unwrap(),
        json!([{"entitlementType": "principal", "amount": "1000.00"}]),
        detail["updatedAt"].as_str().unwrap(),
    )
    .await;
    let ent_id = fin["entitlements"][0]["id"].as_str().unwrap();

    let first = settle(&test, &cookie, &csrf, ent_id, account, "100.00", "st-idem").await;
    assert_eq!(first.0, StatusCode::CREATED);
    let replay = settle(&test, &cookie, &csrf, ent_id, account, "100.00", "st-idem").await;
    assert_eq!(replay.0, StatusCode::OK);
    assert_eq!(replay.1["id"], first.1["id"]);
    assert_eq!(api_balance(&test, &cookie, account).await, "1900.00");
    let movements = api_movements(&test, &cookie, account).await;
    assert_eq!(
        movements
            .iter()
            .filter(|m| m["sourceType"] == "share_return_settlement")
            .count(),
        1,
        "replay must not double-post"
    );

    let conflict = settle(&test, &cookie, &csrf, ent_id, account, "200.00", "st-idem").await;
    assert_eq!(conflict.0, StatusCode::CONFLICT);
}

#[tokio::test]
async fn concurrent_settlements_never_overpay_entitlement_or_account() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    api_fund_account(&test, &cookie, &csrf, account, "700.00", "fund-c").await;
    let (_sh, detail) = pending_return(&test, &cookie, &csrf, 708).await;
    let (_s, fin) = finalize(
        &test,
        &cookie,
        &csrf,
        detail["id"].as_str().unwrap(),
        json!([{"entitlementType": "principal", "amount": "1000.00"}]),
        detail["updatedAt"].as_str().unwrap(),
    )
    .await;
    let ent_id = fin["entitlements"][0]["id"].as_str().unwrap().to_string();

    // Three concurrent settlements of 600 each: entitlement caps at
    // 1000 and account at 700 — at most one can win.
    let app = test.app.clone();
    let cookie2 = cookie.clone();
    let csrf2 = csrf.clone();
    let ent = ent_id.clone();
    let h1 = tokio::spawn({
        let app = app.clone();
        async move {
            send(
                &app,
                req(
                    "POST",
                    &format!("/api/share-return-entitlements/{ent}/settlements"),
                    &cookie2,
                    Some(&csrf2),
                    Some(json!({
                        "financialAccountId": account.to_string(),
                        "amount": "600.00",
                        "idempotencyKey": "cc-1",
                    })),
                ),
            )
            .await
        }
    });
    let ent2 = ent_id.clone();
    let h2 = tokio::spawn({
        let app = app.clone();
        let cookie = cookie.clone();
        let csrf = csrf.clone();
        async move {
            send(
                &app,
                req(
                    "POST",
                    &format!("/api/share-return-entitlements/{ent2}/settlements"),
                    &cookie,
                    Some(&csrf),
                    Some(json!({
                        "financialAccountId": account.to_string(),
                        "amount": "600.00",
                        "idempotencyKey": "cc-2",
                    })),
                ),
            )
            .await
        }
    });
    let r1 = h1.await.unwrap();
    let r2 = h2.await.unwrap();
    let winners = [r1, r2]
        .iter()
        .filter(|(s, _)| *s == StatusCode::CREATED)
        .count();
    assert_eq!(winners, 1, "exactly one settlement may win");
    assert_eq!(api_balance(&test, &cookie, account).await, "100.00");
    let total: i64 =
        sqlx::query_scalar("SELECT count(*) FROM share_return_settlements WHERE status = 'posted'")
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert_eq!(total, 1);
}

// ---------------------------------------------------------------------
// Reversal
// ---------------------------------------------------------------------

#[tokio::test]
async fn settlement_reversal_preserves_history_and_restores_remaining() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    api_fund_account(&test, &cookie, &csrf, account, "2000.00", "fund-r").await;
    let (_sh, detail) = pending_return(&test, &cookie, &csrf, 709).await;
    let return_id = detail["id"].as_str().unwrap();
    let (_s, fin) = finalize(
        &test,
        &cookie,
        &csrf,
        return_id,
        json!([{"entitlementType": "principal", "amount": "500.00"}]),
        detail["updatedAt"].as_str().unwrap(),
    )
    .await;
    let ent_id = fin["entitlements"][0]["id"].as_str().unwrap();

    let (status, st) = settle(&test, &cookie, &csrf, ent_id, account, "500.00", "st-r1").await;
    assert_eq!(status, StatusCode::CREATED);
    let settlement_id = st["id"].as_str().unwrap();
    assert_eq!(api_balance(&test, &cookie, account).await, "1500.00");

    // Reverse without reason → 400.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-return-settlements/{settlement_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, rev) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-return-settlements/{settlement_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "yanlış hesap" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "reverse: {rev}");
    assert_eq!(rev["status"], "reversed");
    assert!(rev["reversedAt"].is_string());
    assert_eq!(rev["reversalReason"], "yanlış hesap");
    assert_eq!(api_balance(&test, &cookie, account).await, "2000.00");

    // Movement reversed but preserved (no hard delete).
    let movements = api_movements(&test, &cookie, account).await;
    let settlement_movements: Vec<_> = movements
        .iter()
        .filter(|m| m["sourceType"] == "share_return_settlement")
        .collect();
    assert_eq!(settlement_movements.len(), 1);
    assert_eq!(settlement_movements[0]["status"], "reversed");

    // Entitlement reopened with full remaining.
    let (status, detail2) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/share-returns/{return_id}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail2["entitlements"][0]["status"], "open");
    assert_eq!(detail2["entitlements"][0]["remainingAmount"], "500.00");
    assert_eq!(detail2["entitlements"][0]["settledAmount"], "0.00");
    assert_eq!(detail2["settlements"][0]["status"], "reversed");

    // Second reversal → idempotent, no double credit.
    let (status, rev2) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-return-settlements/{settlement_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "tekrar" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "reversal replays: {rev2}");
    assert_eq!(api_balance(&test, &cookie, account).await, "2000.00");
}

// ---------------------------------------------------------------------
// Entitlement lifecycle beyond finalize
// ---------------------------------------------------------------------

#[tokio::test]
async fn profit_right_determined_later_and_cancel_blocked_after_settlement() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let account = api_create_account(&test, &cookie, &csrf, "Kasa").await;
    api_fund_account(&test, &cookie, &csrf, account, "3000.00", "fund-p").await;
    let (_sh, detail) = pending_return(&test, &cookie, &csrf, 710).await;
    let return_id = detail["id"].as_str().unwrap();
    let (_s, fin) = finalize(
        &test,
        &cookie,
        &csrf,
        return_id,
        json!([{"entitlementType": "profit"}]),
        detail["updatedAt"].as_str().unwrap(),
    )
    .await;
    let profit_id = fin["entitlements"][0]["id"].as_str().unwrap().to_string();
    let expected = fin["entitlements"][0]["updatedAt"]
        .as_str()
        .unwrap()
        .to_string();

    // Determine the undetermined profit right later (docs/10 deferred).
    let (status, det) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-return-entitlements/{profit_id}/determine"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "amount": "250.00",
                "dueDate": "2027-01-15",
                "policyReference": "GK-2026/01 kar dağıtım kararı",
                "expectedUpdatedAt": expected,
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "determine: {det}");
    assert_eq!(det["amount"], "250.00");
    assert!(det["determinedAt"].is_string());
    assert_eq!(det["dueDate"], "2027-01-15");

    // Duplicate principal recognition on the same return → conflict.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-returns/{return_id}/entitlements"),
            &cookie,
            Some(&csrf),
            Some(json!({"entitlementType": "profit", "amount": "1.00"})),
        ),
    )
    .await;
    assert!(
        status == StatusCode::CONFLICT || status == StatusCode::BAD_REQUEST,
        "duplicate type must be rejected"
    );

    // Settle part of it.
    let (status, st) = settle(
        &test, &cookie, &csrf, &profit_id, account, "100.00", "st-p1",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{st}");

    // Now cancellation must be refused (posted settlement exists).
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-return-entitlements/{profit_id}/cancel"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "iptal" })),
        ),
    )
    .await;
    assert!(
        status == StatusCode::CONFLICT || status == StatusCode::BAD_REQUEST,
        "settled entitlement locked: {status}"
    );
}

// ---------------------------------------------------------------------
// Listing / authorization / CSRF
// ---------------------------------------------------------------------

#[tokio::test]
async fn list_endpoints_and_status_filters_work() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let (_sh, detail) = pending_return(&test, &cookie, &csrf, 711).await;

    let (status, list) = send(
        &test.app,
        req("GET", "/api/share-returns?pageSize=10", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["totalCount"], 1);
    assert_eq!(list["items"][0]["id"], detail["id"]);
    assert_eq!(list["items"][0]["status"], "pending");
    assert_eq!(list["items"][0]["entitlementCount"], 0);

    // Status filter.
    let (status, empty) = send(
        &test.app,
        req(
            "GET",
            "/api/share-returns?status=finalized",
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty["totalCount"], 0);

    // Unknown id → 404.
    let (status, _) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/share-returns/{}", Uuid::new_v4()),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Entitlement listing exists and is empty.
    let (status, ents) = send(
        &test.app,
        req(
            "GET",
            "/api/share-return-entitlements?pageSize=10",
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ents["totalCount"], 0);
}

#[tokio::test]
async fn authorization_csrf_and_no_store_enforced() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let (_sh, detail) = pending_return(&test, &cookie, &csrf, 712).await;
    let return_id = detail["id"].as_str().unwrap();

    let plain = format!("plain.{}", Uuid::new_v4().simple());
    create_plain_user(&test.pool, &plain).await;
    let (pcookie, pcsrf) = login(&test.app, test.peer(), &plain).await;

    // Reads require share_returns.read.
    let (status, _) = send(
        &test.app,
        req("GET", "/api/share-returns", &pcookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Mutations require the permission too.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-returns/{return_id}/cancel"),
            &pcookie,
            Some(&pcsrf),
            Some(json!({ "reason": "x" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Unauthenticated → 401.
    let (status, _) = send(
        &test.app,
        req("GET", "/api/share-returns", "bogus", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Missing CSRF token → 403.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-returns/{return_id}/cancel"),
            &cookie,
            None,
            Some(json!({ "reason": "x" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Wrong origin → 403.
    let mut bad_origin = req(
        "POST",
        &format!("/api/share-returns/{return_id}/cancel"),
        &cookie,
        Some(&csrf),
        Some(json!({ "reason": "x" })),
    );
    bad_origin
        .headers_mut()
        .insert("origin", "https://evil.example".parse().unwrap());
    let (status, _) = send(&test.app, bad_origin).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // no-store on list response.
    let response = test
        .app
        .clone()
        .oneshot(req("GET", "/api/share-returns", &cookie, None, None))
        .await
        .unwrap();
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
}

// ---------------------------------------------------------------------
// Effective-date correctness (assessment eligibility after return)
// ---------------------------------------------------------------------

#[tokio::test]
async fn closed_share_excluded_by_effective_date_not_by_current_status() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let shareholder = api_create_shareholder(&test, &cookie, &csrf, "Tarih", "Testi", 713).await;
    let share = api_create_share(&test, &cookie, &csrf, shareholder).await;
    let share_id = share["id"].as_str().unwrap().to_string();

    // Return finalized at 2026-01-10 — share is now CLOSED.
    let (_s, detail) = initiate(&test, &cookie, &csrf, &share_id, "2026-01-10", "init-713").await;
    let (_s, _fin) = finalize(
        &test,
        &cookie,
        &csrf,
        detail["id"].as_str().unwrap(),
        json!([{"entitlementType": "principal", "amount": "100.00"}]),
        detail["updatedAt"].as_str().unwrap(),
    )
    .await;

    // A period effective BEFORE the cutoff must still assess the share
    // (ownership ended 2026-01-10 00:00 Istanbul; 2026-01-05 < cutoff).
    let (status, period) = send(
        &test.app,
        req(
            "POST",
            "/api/periods",
            &cookie,
            Some(&csrf),
            Some(json!({
                "name": "Ocak İlk Yarı",
                "collectionStartDate": "2026-01-01",
                "dueDate": "2026-01-09",
                "ruleType": "per_share",
                "baseAmount": "50.00",
                "assessmentEffectiveDate": "2026-01-05",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{period}");
    let (status, gen) = send(
        &test.app,
        req(
            "POST",
            &format!(
                "/api/periods/{}/generate-assessments",
                period["id"].as_str().unwrap()
            ),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "generate: {gen}");
    assert!(
        gen["assessmentCount"].as_i64().unwrap() >= 1,
        "closed share still owes pre-cutoff assessments: {gen}"
    );

    // A period effective AT/AFTER the cutoff must NOT assess it.
    let (_s, period2) = send(
        &test.app,
        req(
            "POST",
            "/api/periods",
            &cookie,
            Some(&csrf),
            Some(json!({
                "name": "Ocak İkinci Yarı",
                "collectionStartDate": "2026-01-10",
                "dueDate": "2026-01-31",
                "ruleType": "per_share",
                "baseAmount": "50.00",
                "assessmentEffectiveDate": "2026-01-10",
            })),
        ),
    )
    .await;
    let (status, gen2) = send(
        &test.app,
        req(
            "POST",
            &format!(
                "/api/periods/{}/generate-assessments",
                period2["id"].as_str().unwrap()
            ),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "generate2: {gen2}");
    assert_eq!(
        gen2["assessmentCount"].as_i64().unwrap(),
        0,
        "post-cutoff period must not assess the closed share"
    );
}

// ---------------------------------------------------------------------
// Debts / credits survive the return
// ---------------------------------------------------------------------

#[tokio::test]
async fn return_preserves_existing_debts_and_credits() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let shareholder = api_create_shareholder(&test, &cookie, &csrf, "Borçlu", "Üye", 714).await;
    let share = api_create_share(&test, &cookie, &csrf, shareholder).await;

    // An assessed debt (per_share period effective today) + an excess
    // payment credit must both survive the return untouched.
    let (_s, period) = send(
        &test.app,
        req(
            "POST",
            "/api/periods",
            &cookie,
            Some(&csrf),
            Some(json!({
                "name": "Borç Dönemi",
                "collectionStartDate": "2026-01-01",
                "dueDate": "2026-01-31",
                "ruleType": "per_share",
                "baseAmount": "500.00",
                "assessmentEffectiveDate": "2026-01-15",
            })),
        ),
    )
    .await;
    assert_eq!(_s, StatusCode::CREATED);
    let (_s2, _gen) = send(
        &test.app,
        req(
            "POST",
            &format!(
                "/api/periods/{}/generate-assessments",
                period["id"].as_str().unwrap()
            ),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;

    let share_id = share["id"].as_str().unwrap();
    let (_s3, detail) = initiate(&test, &cookie, &csrf, share_id, "2026-01-15", "init-714").await;
    let (_s4, _fin) = finalize(
        &test,
        &cookie,
        &csrf,
        detail["id"].as_str().unwrap(),
        json!([{"entitlementType": "principal", "amount": "300.00"}]),
        detail["updatedAt"].as_str().unwrap(),
    )
    .await;

    // The assessment row still exists, unpaid, attached to the same
    // shareholder/debtor.
    let open: i64 = sqlx::query_scalar("SELECT count(*) FROM assessments WHERE status = 'active'")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(open, 1, "existing debt must survive the return");

    // Shareholder-level open assessments endpoint still reports it.
    let (status, open_list) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/shareholders/{shareholder}/open-assessments"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(open_list.as_array().unwrap().len(), 1);
}
