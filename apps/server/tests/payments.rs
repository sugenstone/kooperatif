//! Database-gated Payments & Allocations domain tests (STEP-007).
//! Requires KOOPERATIF_TEST_DATABASE_URL; each test runs in its own
//! throwaway database (full isolation, including seeded-role state).

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
    /// Lazily-created default destination Financial Account (STEP-008):
    /// ONE account per test database so idempotent replays fingerprint
    /// identically.
    default_account: tokio::sync::Mutex<Option<Uuid>>,
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
        default_account: tokio::sync::Mutex::new(None),
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

async fn api_create_family(test: &TestApp, cookie: &str, csrf: &str, sequence: i64) -> Uuid {
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/families",
            cookie,
            Some(csrf),
            Some(json!({ "sequenceNumber": sequence })),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "family create failed: {detail}"
    );
    detail["id"].as_str().unwrap().parse().unwrap()
}

async fn api_create_shareholder(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    first: &str,
    last: &str,
    family_id: Option<Uuid>,
    family_sequence: i64,
) -> Uuid {
    let family = match family_id {
        Some(id) => json!({ "mode": "existing", "familyId": id }),
        None => json!({ "mode": "new", "sequenceNumber": family_sequence }),
    };
    let body = json!({
        "person": { "mode": "new", "firstName": first, "lastName": last },
        "family": family
    });
    let (status, detail) = send(
        &test.app,
        req("POST", "/api/shareholders", cookie, Some(csrf), Some(body)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {detail}");
    detail["id"].as_str().unwrap().parse().unwrap()
}

async fn api_generate_open_period(test: &TestApp, cookie: &str, csrf: &str, amount: &str) -> Value {
    let body = json!({
        "name": "Tahsilat Dönemi",
        "collectionStartDate": "2026-01-01",
        "dueDate": "2026-01-31",
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
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!(
                "/api/periods/{}/generate-assessments",
                period["id"].as_str().unwrap()
            ),
            cookie,
            Some(csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "generate failed: {detail}");
    detail
}

async fn assessments_of(test: &TestApp, cookie: &str, period_id: &str) -> Vec<Value> {
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
    list["items"].as_array().unwrap().clone()
}

fn payment_body(
    payer_person: Option<Uuid>,
    amount: &str,
    key: &str,
    allocations: Vec<Value>,
) -> Value {
    match payer_person {
        Some(id) => json!({
            "payerPersonId": id,
            "amount": amount,
            "method": "cash",
            "idempotencyKey": key,
            "allocations": allocations,
        }),
        None => json!({
            "payerFirstName": "Ödeyen",
            "payerLastName": "Kişi",
            "amount": amount,
            "method": "cash",
            "idempotencyKey": key,
            "allocations": allocations,
        }),
    }
}

/// STEP-008: every new Payment requires a destination Financial
/// Account. Tests that don't care about WHICH account get one lazily
/// created cash account per test database — stable across idempotent
/// replays (the account id is part of the payload fingerprint).
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

async fn create_payment(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    mut body: Value,
) -> (StatusCode, Value) {
    if body.get("destinationAccountId").is_none() {
        let account = default_account(test, cookie, csrf).await;
        body["destinationAccountId"] = json!(account.to_string());
    }
    send(
        &test.app,
        req("POST", "/api/payments", cookie, Some(csrf), Some(body)),
    )
    .await
}

async fn shareholder_summary(test: &TestApp, cookie: &str, shareholder_id: Uuid) -> Value {
    let (status, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/shareholders/{shareholder_id}/financial-summary"),
            cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail
}

async fn person_id_of_shareholder(pool: &PgPool, shareholder_id: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT person_id FROM shareholders WHERE id = $1")
        .bind(shareholder_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------
// Happy paths: partial, full, multi-payment, multi-assessment, family
// ---------------------------------------------------------------------

#[tokio::test]
async fn partial_then_full_settlement_and_assessment_history() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let debtor = api_create_shareholder(&test, &cookie, &csrf, "Borçlu", "Bir", None, 1).await;
    api_generate_open_period(&test, &cookie, &csrf, "1000.00").await;
    let period = get_open_period(&test, &cookie).await;
    let assessments = assessments_of(&test, &cookie, period["id"].as_str().unwrap()).await;
    let assessment = &assessments[0];

    // Partial payment by a NEW payer person (payer != debtor).
    let (status, created) = create_payment(
        &test,
        &cookie,
        &csrf,
        payment_body(
            None,
            "1500.00",
            "key-partial-1",
            vec![json!({ "assessmentId": assessment["id"], "amount": "800.00" })],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["replayed"], false);
    let payment = &created["payment"];
    assert_eq!(payment["status"], "posted");
    assert_eq!(payment["allocatedAmount"], "800.00");
    assert_eq!(payment["unallocatedAmount"], "700.00");
    // Payer is a distinct person, not the debtor shareholder's person.
    let payer_person: Uuid = payment["payer"]["personId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_ne!(
        payer_person,
        person_id_of_shareholder(&test.pool, debtor).await
    );
    assert_eq!(payment["payer"]["shareholderId"], Value::Null);

    let summary = shareholder_summary(&test, &cookie, debtor).await;
    assert_eq!(summary["totalAssessed"], "1000.00");
    assert_eq!(summary["totalPaid"], "800.00");
    assert_eq!(summary["totalRemaining"], "200.00");

    // Second payment settles the remainder.
    let (status, _) = create_payment(
        &test,
        &cookie,
        &csrf,
        payment_body(
            None,
            "200.00",
            "key-partial-2",
            vec![json!({ "assessmentId": assessment["id"], "amount": "200.00" })],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let summary = shareholder_summary(&test, &cookie, debtor).await;
    assert_eq!(summary["totalPaid"], "1000.00");
    assert_eq!(summary["totalRemaining"], "0.00");
    assert_eq!(summary["openAssessmentCount"], 0);

    // Assessment payment history lists both applications.
    let (status, history) = send(
        &test.app,
        req(
            "GET",
            &format!(
                "/api/assessments/{}/payments",
                assessment["id"].as_str().unwrap()
            ),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{history}");
    assert_eq!(history.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn one_payment_across_multiple_assessments_of_one_family() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let family = api_create_family(&test, &cookie, &csrf, 42).await;
    let member_a =
        api_create_shareholder(&test, &cookie, &csrf, "Ali", "Aile", Some(family), 0).await;
    let member_b =
        api_create_shareholder(&test, &cookie, &csrf, "Ayşe", "Aile", Some(family), 0).await;
    let period = api_generate_open_period(&test, &cookie, &csrf, "500.00").await;
    let assessments = assessments_of(&test, &cookie, period["id"].as_str().unwrap()).await;
    assert_eq!(assessments.len(), 2);
    let a = assessments
        .iter()
        .find(|x| x["shareholder"]["shareholderId"].as_str().unwrap() == member_a.to_string())
        .unwrap();
    let b = assessments
        .iter()
        .find(|x| x["shareholder"]["shareholderId"].as_str().unwrap() == member_b.to_string())
        .unwrap();

    // ONE receipt covering TWO members' obligations — one payment row,
    // two allocations, no family-as-debtor.
    let (status, created) = create_payment(
        &test,
        &cookie,
        &csrf,
        payment_body(
            None,
            "900.00",
            "key-family-1",
            vec![
                json!({ "assessmentId": a["id"], "amount": "400.00" }),
                json!({ "assessmentId": b["id"], "amount": "500.00" }),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let payment = &created["payment"];
    assert_eq!(payment["allocations"].as_array().unwrap().len(), 2);
    assert_eq!(payment["unallocatedAmount"], "0.00");

    let summary_a = shareholder_summary(&test, &cookie, member_a).await;
    let summary_b = shareholder_summary(&test, &cookie, member_b).await;
    assert_eq!(summary_a["totalRemaining"], "100.00");
    assert_eq!(summary_b["totalRemaining"], "0.00");

    // Family context: member-wise, no aggregate debtor row.
    let (status, context) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/families/{family}/collection-context"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{context}");
    assert_eq!(context["members"].as_array().unwrap().len(), 2);
    assert_eq!(context["totalRemaining"], "100.00");

    // Period summary: collected = both members' settled amounts.
    let (status, psummary) = send(
        &test.app,
        req(
            "GET",
            &format!(
                "/api/periods/{}/financial-summary",
                period["id"].as_str().unwrap()
            ),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{psummary}");
    assert_eq!(psummary["totalAssessed"], "1000.00");
    assert_eq!(psummary["totalCollected"], "900.00");
    assert_eq!(psummary["contributingPaymentCount"], 1);
}

#[tokio::test]
async fn fully_unallocated_payment_is_explicit_and_later_distributable() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    api_create_shareholder(&test, &cookie, &csrf, "Borçlu", "İki", None, 5).await;
    let period = api_generate_open_period(&test, &cookie, &csrf, "300.00").await;
    let assessments = assessments_of(&test, &cookie, period["id"].as_str().unwrap()).await;
    let assessment = &assessments[0];

    // Money received with NO allocation — everything unallocated.
    let (status, created) = create_payment(
        &test,
        &cookie,
        &csrf,
        payment_body(None, "300.00", "key-unalloc-1", vec![]),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let payment_id = created["payment"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["payment"]["unallocatedAmount"], "300.00");

    // Later: explicit disposition via add-allocations.
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/payments/{payment_id}/allocations"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "allocations": [{ "assessmentId": assessment["id"], "amount": "300.00" }]
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["unallocatedAmount"], "0.00");
    assert_eq!(detail["allocatedAmount"], "300.00");

    // A second line to the same (payment, assessment) is impossible.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/payments/{payment_id}/allocations"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "allocations": [{ "assessmentId": assessment["id"], "amount": "1.00" }]
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

// ---------------------------------------------------------------------
// Rejection paths
// ---------------------------------------------------------------------

#[tokio::test]
async fn over_allocation_rejections() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    api_create_shareholder(&test, &cookie, &csrf, "Borçlu", "Üç", None, 6).await;
    let period = api_generate_open_period(&test, &cookie, &csrf, "100.00").await;
    let assessments = assessments_of(&test, &cookie, period["id"].as_str().unwrap()).await;
    let assessment = &assessments[0];

    // Σ allocations > payment amount → deterministic 400.
    let (status, _) = create_payment(
        &test,
        &cookie,
        &csrf,
        payment_body(
            None,
            "50.00",
            "key-over-1",
            vec![json!({ "assessmentId": assessment["id"], "amount": "60.00" })],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // allocation > assessment remaining → 409.
    let (status, _) = create_payment(
        &test,
        &cookie,
        &csrf,
        payment_body(
            None,
            "150.00",
            "key-over-2",
            vec![json!({ "assessmentId": assessment["id"], "amount": "150.00" })],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Nothing persisted on any rejection (rollback evidence).
    let payments: i64 = sqlx::query_scalar("SELECT count(*) FROM payments")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    let allocations: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_allocations")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!((payments, allocations), (0, 0));
}

#[tokio::test]
async fn invalid_targets_and_inputs_are_rejected_and_atomic() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    api_create_shareholder(&test, &cookie, &csrf, "Borçlu", "Dört", None, 7).await;
    let period = api_generate_open_period(&test, &cookie, &csrf, "100.00").await;
    let assessments = assessments_of(&test, &cookie, period["id"].as_str().unwrap()).await;
    let assessment = &assessments[0];

    // Nonexistent assessment target → 409 + full rollback.
    let ghost = Uuid::new_v4().to_string();
    let (status, _) = create_payment(
        &test,
        &cookie,
        &csrf,
        payment_body(
            None,
            "200.00",
            "key-ghost",
            vec![
                json!({ "assessmentId": assessment["id"], "amount": "50.00" }),
                json!({ "assessmentId": ghost, "amount": "50.00" }),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Voided assessment can never receive value.
    sqlx::query("UPDATE assessments SET status = 'voided' WHERE id = $1")
        .bind(assessment["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .execute(&test.pool)
        .await
        .unwrap();
    let (status, _) = create_payment(
        &test,
        &cookie,
        &csrf,
        payment_body(
            None,
            "100.00",
            "key-voided",
            vec![json!({ "assessmentId": assessment["id"], "amount": "100.00" })],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Zero/negative/too-precise amounts → 400.
    for amount in ["0", "0.00", "-5", "10.005"] {
        let (status, _) = create_payment(
            &test,
            &cookie,
            &csrf,
            payment_body(None, amount, &format!("key-amt-{amount}"), vec![]),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "amount {amount}");
    }

    // Future received_at → 400.
    let (status, _) = create_payment(
        &test,
        &cookie,
        &csrf,
        json!({
            "payerFirstName": "Gelecek", "payerLastName": "Kişi",
            "amount": "10.00", "method": "cash",
            "receivedAt": "2030-01-01T00:00:00Z",
            "idempotencyKey": "key-future", "allocations": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Nonexistent payer person → 400.
    let (status, _) = create_payment(
        &test,
        &cookie,
        &csrf,
        payment_body(Some(Uuid::new_v4()), "10.00", "key-payer-ghost", vec![]),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Duplicate target inside one command → 400.
    let (status, _) = create_payment(
        &test,
        &cookie,
        &csrf,
        payment_body(
            None,
            "20.00",
            "key-dupe",
            vec![
                json!({ "assessmentId": assessment["id"], "amount": "10.00" }),
                json!({ "assessmentId": assessment["id"], "amount": "10.00" }),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let payments: i64 = sqlx::query_scalar("SELECT count(*) FROM payments")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    let allocations: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_allocations")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(
        (payments, allocations),
        (0, 0),
        "every rejection rolled back"
    );
}

#[tokio::test]
async fn idempotent_replay_and_payload_mismatch() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    api_create_shareholder(&test, &cookie, &csrf, "Borçlu", "Beş", None, 8).await;
    let period = api_generate_open_period(&test, &cookie, &csrf, "100.00").await;
    let assessments = assessments_of(&test, &cookie, period["id"].as_str().unwrap()).await;
    let assessment = &assessments[0];

    let body = payment_body(
        None,
        "100.00",
        "key-replay",
        vec![json!({ "assessmentId": assessment["id"], "amount": "100.00" })],
    );
    let (status, first) = create_payment(&test, &cookie, &csrf, body.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let payment_id = first["payment"]["id"].as_str().unwrap().to_string();

    // Byte-identical retry → 200 replay, same row, no duplicate.
    let (status, replay) = create_payment(&test, &cookie, &csrf, body).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["payment"]["id"], payment_id);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM payments")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // Same key, different payload → 409.
    let (status, _) = create_payment(
        &test,
        &cookie,
        &csrf,
        payment_body(None, "99.00", "key-replay", vec![]),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn concurrent_over_allocation_is_serialized() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    api_create_shareholder(&test, &cookie, &csrf, "Borçlu", "Altı", None, 9).await;
    let period = api_generate_open_period(&test, &cookie, &csrf, "1000.00").await;
    let assessments = assessments_of(&test, &cookie, period["id"].as_str().unwrap()).await;
    let assessment = &assessments[0];

    // Two racing collections each trying to allocate 800 of the same
    // 1000-remaining obligation: exactly ONE may win (row-lock
    // serialization + under-lock remaining recompute).
    let body = |key: &str| {
        payment_body(
            None,
            "800.00",
            key,
            vec![json!({ "assessmentId": assessment["id"], "amount": "800.00" })],
        )
    };
    let first = create_payment(&test, &cookie, &csrf, body("race-a"));
    let second = create_payment(&test, &cookie, &csrf, body("race-b"));
    let (r1, r2) = tokio::join!(first, second);
    let statuses = [r1.0, r2.0];
    assert!(
        statuses.contains(&StatusCode::CREATED) && statuses.contains(&StatusCode::CONFLICT),
        "expected one winner + one conflict, got {statuses:?} ({r1:?}) ({r2:?})"
    );

    // The obligation's settled amount never exceeded its amount.
    let paid: rust_decimal::Decimal = sqlx::query_scalar(
        "SELECT COALESCE(sum(amount), 0) FROM payment_allocations WHERE status = 'active'",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(paid.to_string(), "800.00");
}

#[tokio::test]
async fn reversals_are_historical_and_idempotent() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let debtor = api_create_shareholder(&test, &cookie, &csrf, "Borçlu", "Yedi", None, 10).await;
    let period = api_generate_open_period(&test, &cookie, &csrf, "300.00").await;
    let assessments = assessments_of(&test, &cookie, period["id"].as_str().unwrap()).await;
    let assessment = &assessments[0];

    let (status, created) = create_payment(
        &test,
        &cookie,
        &csrf,
        payment_body(
            None,
            "300.00",
            "key-rev-1",
            vec![json!({ "assessmentId": assessment["id"], "amount": "300.00" })],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let payment_id = created["payment"]["id"].as_str().unwrap().to_string();
    let allocation_id = created["payment"]["allocations"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Reason is REQUIRED.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/payments/{payment_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "   " })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Full reversal: rows remain as history.
    let (status, reversed) = send(
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
    assert_eq!(status, StatusCode::OK, "{reversed}");
    assert_eq!(reversed["status"], "reversed");
    assert_eq!(reversed["allocations"][0]["status"], "reversed");
    assert_eq!(reversed["reversalReason"], "hatalı tahsilat");
    let summary = shareholder_summary(&test, &cookie, debtor).await;
    assert_eq!(summary["totalPaid"], "0.00");
    assert_eq!(summary["totalRemaining"], "300.00");

    // Repeated reversal = idempotent replay, still 200.
    let (status, again) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/payments/{payment_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "yine" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["status"], "reversed");
    // Original reversal reason preserved — history never rewritten.
    assert_eq!(again["reversalReason"], "hatalı tahsilat");

    // Add-allocations on a reversed payment → 409.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/payments/{payment_id}/allocations"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "allocations": [{ "assessmentId": assessment["id"], "amount": "1.00" }]
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let _ = allocation_id;
}

#[tokio::test]
async fn single_allocation_reversal_preserves_the_rest() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let family = api_create_family(&test, &cookie, &csrf, 77).await;
    api_create_shareholder(&test, &cookie, &csrf, "Üye", "Bir", Some(family), 0).await;
    api_create_shareholder(&test, &cookie, &csrf, "Üye", "İki", Some(family), 0).await;
    let period = api_generate_open_period(&test, &cookie, &csrf, "100.00").await;
    let assessments = assessments_of(&test, &cookie, period["id"].as_str().unwrap()).await;
    let (a1, a2) = (&assessments[0], &assessments[1]);

    let (status, created) = create_payment(
        &test,
        &cookie,
        &csrf,
        payment_body(
            None,
            "200.00",
            "key-rev-single",
            vec![
                json!({ "assessmentId": a1["id"], "amount": "100.00" }),
                json!({ "assessmentId": a2["id"], "amount": "100.00" }),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let payment_id = created["payment"]["id"].as_str().unwrap().to_string();
    let allocation_id = created["payment"]["allocations"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/payments/{payment_id}/allocations/{allocation_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "yanlış dağılım" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["status"], "posted", "payment itself stays posted");
    assert_eq!(detail["allocatedAmount"], "100.00");
    let statuses: Vec<&str> = detail["allocations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["status"].as_str().unwrap())
        .collect();
    assert!(statuses.contains(&"reversed") && statuses.contains(&"active"));

    // Repeating the single reversal also replays idempotently.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/payments/{payment_id}/allocations/{allocation_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "tekrar" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

// ---------------------------------------------------------------------
// Security: authorization, CSRF, IDOR, no-store
// ---------------------------------------------------------------------

#[tokio::test]
async fn authorization_csrf_idor_and_cache_control() {
    let Some(test) = setup().await else { return };
    let (cookie, _csrf) = admin_session(&test).await;

    // No-store on the financial list surface.
    let response = test
        .app
        .clone()
        .oneshot(req("GET", "/api/payments", &cookie, None, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
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
            Request::get("/api/payments")
                .header("origin", ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Authenticated plain user (no roles) → 403 on reads and mutations.
    let plain = format!("usr.{}", Uuid::new_v4().simple());
    create_plain_user(&test.pool, &plain).await;
    let (plain_cookie, plain_csrf) = login(&test.app, test.peer(), &plain).await;
    for (method, path, body) in [
        ("GET", "/api/payments".to_string(), None),
        (
            "POST",
            "/api/payments".to_string(),
            Some(json!({
                "payerFirstName": "Yetkisiz", "payerLastName": "Kişi",
                "amount": "10.00", "method": "cash",
                "destinationAccountId": Uuid::new_v4().to_string(),
                "idempotencyKey": "k-plain", "allocations": []
            })),
        ),
        (
            "GET",
            "/api/payments/payer-persons?search=a".to_string(),
            None,
        ),
        (
            "GET",
            format!("/api/shareholders/{}/financial-summary", Uuid::new_v4()),
            None,
        ),
    ] {
        let (status, detail) = send(
            &test.app,
            req(method, &path, &plain_cookie, Some(&plain_csrf), body),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {detail}");
    }

    // CSRF: admin session, missing token → 403.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/payments",
            &cookie,
            None,
            Some(json!({
                "payerFirstName": "Csrf", "payerLastName": "Kişi",
                "amount": "10.00", "method": "cash",
                "destinationAccountId": Uuid::new_v4().to_string(),
                "idempotencyKey": "k-csrf", "allocations": []
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // IDOR: random ids → 404, never data of others.
    for path in [
        format!("/api/payments/{}", Uuid::new_v4()),
        format!("/api/assessments/{}/payments", Uuid::new_v4()),
        format!("/api/families/{}/collection-context", Uuid::new_v4()),
        format!("/api/periods/{}/financial-summary", Uuid::new_v4()),
        format!("/api/shareholders/{}/open-assessments", Uuid::new_v4()),
    ] {
        let (status, detail) = send(&test.app, req("GET", &path, &cookie, None, None)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {detail}");
    }
}

#[tokio::test]
async fn payer_search_and_decimals_preserve_exactness() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let debtor = api_create_shareholder(&test, &cookie, &csrf, "Nokta", "Hassas", None, 11).await;
    let period = api_generate_open_period(&test, &cookie, &csrf, "333.33").await;
    let assessments = assessments_of(&test, &cookie, period["id"].as_str().unwrap()).await;
    let assessment = &assessments[0];

    // Payer candidates endpoint finds the debtor's person too —
    // a shareholder can also be a payer (of someone's debt).
    let (status, candidates) = send(
        &test.app,
        req(
            "GET",
            "/api/payments/payer-persons?search=Nokta",
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{candidates}");
    assert_eq!(candidates.as_array().unwrap().len(), 1);
    let payer_person: Uuid = candidates[0]["personId"].as_str().unwrap().parse().unwrap();

    // Exact cents-precise amounts round-trip canonically.
    let (status, created) = create_payment(
        &test,
        &cookie,
        &csrf,
        payment_body(
            Some(payer_person),
            "111.11",
            "key-exact",
            vec![json!({ "assessmentId": assessment["id"], "amount": "111.11" })],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["payment"]["amount"], "111.11");
    assert_eq!(created["payment"]["allocations"][0]["amount"], "111.11");
    // Payer == debtor person here — both surfaces still agree.
    assert_eq!(
        created["payment"]["payer"]["personId"].as_str().unwrap(),
        person_id_of_shareholder(&test.pool, debtor)
            .await
            .to_string()
    );
    let summary = shareholder_summary(&test, &cookie, debtor).await;
    assert_eq!(summary["totalPaid"], "111.11");
    assert_eq!(summary["totalRemaining"], "222.22");
}

async fn get_open_period(test: &TestApp, cookie: &str) -> Value {
    let (status, list) = send(
        &test.app,
        req("GET", "/api/periods?pageSize=50", cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["status"] == "open")
        .unwrap()
        .clone()
}
