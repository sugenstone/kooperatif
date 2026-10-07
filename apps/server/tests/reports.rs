//! Database-gated Reporting tests (STEP-015, docs/14, docs/15
//! §reporting invariants, docs/17). Requires KOOPERATIF_TEST_DATABASE_URL;
//! each test runs in its own throwaway database.
//!
//! Invariants under test:
//! - REPORTING IS READ-ONLY: browsing every report endpoint mutates zero
//!   domain rows.
//! - Every metric reuses the canonical derivation of its owning domain:
//!   account balance = SUM(active movements); assessment remaining =
//!   amount − active allocations − active credit applications; credit
//!   available = active credits − applications; restricted availability
//!   = posted donations − posted disbursements per (fund, account).
//! - Money categories never merge: Payment is not operational Income;
//!   investment funding/income/disposal are not operational flows;
//!   social-aid donation/disbursement are not operational flows; share
//!   return settlement is not operational expense; internal transfers
//!   never count as cooperative-wide income/expense.
//! - NULL is never rendered as "0.00" — an undetermined Profit Right
//!   stays NULL end-to-end.
//! - Reported totals span the full filtered set regardless of page, and
//!   filtered list rows agree with filtered summary totals.
//! - Reversed/cancelled records contribute nothing to current totals but
//!   remain traceable in drill-downs.
//! - `reports.read` is a separate cross-domain grant: holding a domain
//!   read permission does NOT unlock reports.

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

struct TestApp {
    app: axum::Router,
    pool: PgPool,
    _database_name: String,
    _peer: SocketAddr,
}

impl TestApp {
    fn peer(&self) -> SocketAddr {
        self._peer
    }
}

async fn setup() -> Option<TestApp> {
    let url = std::env::var("KOOPERATIF_TEST_DATABASE_URL").ok()?;
    let database_name = format!("kooperatif_test_{}", Uuid::new_v4().simple());
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

/// A user holding ONLY the permission keys given — proves cross-domain
/// reports reject single-domain readers (and accept reports.read).
async fn create_scoped_user(pool: &PgPool, username: &str, permission_keys: &[&str]) -> Uuid {
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).unwrap(),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).unwrap();
    let user = users::create_user(pool, username, "Scoped", &hash)
        .await
        .unwrap();
    let role_id: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, description) VALUES ($1, 'test') RETURNING id",
    )
    .bind(format!("rol-{}", Uuid::new_v4().simple()))
    .fetch_one(pool)
    .await
    .unwrap();
    for key in permission_keys {
        sqlx::query(
            "INSERT INTO role_permissions (role_id, permission_id) \
             SELECT $1, id FROM permissions WHERE key = $2",
        )
        .bind(role_id)
        .bind(key)
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::query("INSERT INTO user_role_assignments (user_id, role_id) VALUES ($1, $2)")
        .bind(user.id)
        .bind(role_id)
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
        .header("origin", ORIGIN)
        .header("cookie", format!("kooperatif_session={cookie}"));
    if let Some(token) = csrf {
        builder = builder.header("x-csrf-token", token);
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

async fn get(test: &TestApp, cookie: &str, path: &str) -> (StatusCode, Value) {
    send(&test.app, req("GET", path, cookie, None, None)).await
}

async fn table_count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------
// Domain fixture helpers (real API commands — never direct inserts).
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
    assert_eq!(status, StatusCode::CREATED, "account: {detail}");
    detail["id"].as_str().unwrap().parse().unwrap()
}

async fn api_create_shareholder(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    first: &str,
    last: &str,
    family: Value,
) -> Uuid {
    let body = json!({
        "person": { "mode": "new", "firstName": first, "lastName": last },
        "family": family
    });
    let (status, detail) = send(
        &test.app,
        req("POST", "/api/shareholders", cookie, Some(csrf), Some(body)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "shareholder: {detail}");
    detail["id"].as_str().unwrap().parse().unwrap()
}

async fn api_open_period(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    base_amount: &str,
    key: &str,
) -> Value {
    let body = json!({
        "name": "Ocak 2026",
        "collectionStartDate": "2026-01-01",
        "dueDate": "2026-01-31",
        "ruleType": "per_shareholder",
        "baseAmount": base_amount,
        "assessmentEffectiveDate": "2026-01-01",
        "idempotencyKey": key,
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
            Some(json!({ "idempotencyKey": format!("{key}-gen") })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "generate: {detail}");
    period
}

async fn api_period_assessments(test: &TestApp, cookie: &str, period_id: &str) -> Vec<Value> {
    let (status, list) = get(
        test,
        cookie,
        &format!("/api/periods/{period_id}/assessments?pageSize=100"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["items"].as_array().unwrap().clone()
}

async fn api_post_payment(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    account: Uuid,
    amount: &str,
    allocations: Value,
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
                "payerFirstName": "Veli", "payerLastName": "Ödeyen",
                "amount": amount, "method": "cash",
                "destinationAccountId": account.to_string(),
                "allocations": allocations,
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
) -> StatusCode {
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
                "amount": amount, "idempotencyKey": key,
            })),
        ),
    )
    .await
    .0
}

async fn api_category(test: &TestApp, cookie: &str, csrf: &str, name: &str, kind: &str) -> Uuid {
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/financial-categories",
            cookie,
            Some(csrf),
            Some(json!({ "name": name, "categoryType": kind })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "category: {detail}");
    detail["id"].as_str().unwrap().parse().unwrap()
}

#[allow(clippy::too_many_arguments)]
async fn api_entry(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    kind: &str,
    account: Uuid,
    category: Uuid,
    amount: &str,
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
                "description": format!("{kind} kaydı"),
                "idempotencyKey": key,
            })),
        ),
    )
    .await
}

async fn api_assign_credit(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    payment_id: &str,
    shareholder_id: Uuid,
    amount: &str,
    key: &str,
) -> StatusCode {
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/payments/{payment_id}/credits"),
            cookie,
            Some(csrf),
            Some(json!({
                "shareholderId": shareholder_id, "amount": amount,
                "idempotencyKey": key
            })),
        ),
    )
    .await
    .0
}

async fn api_initiate_return(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    shareholder_id: Uuid,
    key: &str,
) -> Value {
    let (status, share) = send(
        &test.app,
        req(
            "POST",
            "/api/shares",
            cookie,
            Some(csrf),
            Some(json!({
                "shareholderId": shareholder_id,
                "acquisitionType": "founder",
                "effectiveAt": "2025-01-01T00:00:00Z",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "share: {share}");
    let share_id = share["id"].as_str().unwrap();
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/share-returns",
            cookie,
            Some(csrf),
            Some(json!({
                "shareId": share_id,
                "effectiveReturnDate": "2026-01-15",
                "reason": "üyelikten ayrılma",
                "idempotencyKey": key,
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "initiate: {detail}");
    detail
}

async fn api_finalize_return(
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

async fn api_settle(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    entitlement_id: &str,
    account: Uuid,
    amount: &str,
    key: &str,
) -> StatusCode {
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-return-entitlements/{entitlement_id}/settlements"),
            cookie,
            Some(csrf),
            Some(json!({
                "financialAccountId": account.to_string(),
                "amount": amount, "idempotencyKey": key,
            })),
        ),
    )
    .await
    .0
}

async fn api_create_investment(
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
            "/api/investments",
            cookie,
            Some(csrf),
            Some(json!({
                "name": name,
                "investmentType": "real_estate",
                "acquiredAt": "2026-01-05",
                "idempotencyKey": key,
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "investment: {detail}");
    detail
}

#[allow(clippy::too_many_arguments)]
async fn api_investment_event(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    investment_id: &str,
    kind: &str,
    account: Option<Uuid>,
    amount: &str,
    key: &str,
) -> StatusCode {
    let body = match kind {
        "valuations" => json!({
            "valuationDate": "2026-01-14", "amount": amount,
            "method": "Ekspertiz", "idempotencyKey": key,
        }),
        _ => json!({
            "financialAccountId": account.unwrap().to_string(),
            "amount": amount,
            "occurredAt": "2026-01-12T10:00:00Z",
            "description": "yatırım olayı",
            "idempotencyKey": key,
        }),
    };
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/investments/{investment_id}/{kind}"),
            cookie,
            Some(csrf),
            Some(body),
        ),
    )
    .await
    .0
}

async fn api_create_fund(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    name: &str,
    key: &str,
) -> String {
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
    detail["id"].as_str().unwrap().to_string()
}

#[allow(clippy::too_many_arguments)]
async fn api_aid(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    kind: &str,
    fund: &str,
    account: Uuid,
    amount: &str,
    key: &str,
) -> StatusCode {
    let party = if kind == "donations" {
        json!({ "donorDisplayName": "Hayırsever" })
    } else {
        json!({ "beneficiaryDisplayName": "İhtiyaç Sahibi", "reason": "burs" })
    };
    let mut body = json!({
        "fundId": fund,
        "financialAccountId": account.to_string(),
        "amount": amount,
        "occurredAt": "2026-01-12T10:00:00Z",
        "idempotencyKey": key,
    });
    for (k, v) in party.as_object().unwrap() {
        body[k] = v.clone();
    }
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/social-aid/{kind}"),
            cookie,
            Some(csrf),
            Some(body),
        ),
    )
    .await
    .0
}

async fn api_governance_body(test: &TestApp, cookie: &str, csrf: &str, key: &str) -> String {
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/governance/bodies",
            cookie,
            Some(csrf),
            Some(json!({
                "name": "Yönetim Kurulu", "bodyType": "YK",
                "idempotencyKey": key,
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {detail}");
    detail["id"].as_str().unwrap().to_string()
}

// ---------------------------------------------------------------------
// 1. Golden fixture — every overview metric asserted by hand-computed
//    expectations (never derived via the code under test).
// ---------------------------------------------------------------------

/// Builds the full cross-domain fixture. Returns ids needed by tests.
struct Golden {
    banka: Uuid,
    sh_a: Uuid,
    sh_r: Uuid,
}

async fn build_golden(test: &TestApp, cookie: &str, csrf: &str) -> Golden {
    let kasa = api_create_account(test, cookie, csrf, "Merkez Kasa", "cash").await;
    let banka = api_create_account(test, cookie, csrf, "Vakıf TL", "bank").await;

    // Family 901 holds TWO members; family 902 one. Family aggregates
    // are member-level sums only.
    let family901 = json!({ "mode": "new", "sequenceNumber": 901 });
    let sh_a = api_create_shareholder(test, cookie, csrf, "Ayşe", "Üye", family901.clone()).await;
    let (status, a_detail) = get(test, cookie, &format!("/api/shareholders/{sh_a}")).await;
    assert_eq!(status, StatusCode::OK, "{a_detail}");
    let family901_existing = json!({ "mode": "existing", "familyId": a_detail["familyId"] });
    let sh_b =
        api_create_shareholder(test, cookie, csrf, "Mehmet", "Üye", family901_existing).await;
    let sh_c = api_create_shareholder(
        test,
        cookie,
        csrf,
        "Can",
        "Üye",
        json!({ "mode": "new", "sequenceNumber": 902 }),
    )
    .await;

    // Period: per-shareholder 1 000,00 → three assessments, 3 000 total.
    let period = api_open_period(test, cookie, csrf, "1000.00", "g-p1").await;
    let period_id = period["id"].as_str().unwrap().to_string();
    let assessments = api_period_assessments(test, cookie, &period_id).await;
    assert_eq!(assessments.len(), 3);
    let find_a = |sid: Uuid| {
        assessments
            .iter()
            .find(|a| a["shareholder"]["shareholderId"] == sid.to_string())
            .unwrap()
            .clone()
    };
    let a_asm = find_a(sh_a);
    let b_asm = find_a(sh_b);
    let c_asm = find_a(sh_c);

    // P1: payer is a NEW Person (not any debtor) — 1 200,00 cash into
    // Kasa, allocated 800→A + 400→B.
    let (status, p1) = api_post_payment(
        test,
        cookie,
        csrf,
        kasa,
        "1200.00",
        json!([
            { "assessmentId": a_asm["id"], "amount": "800.00" },
            { "assessmentId": b_asm["id"], "amount": "400.00" }
        ]),
        "g-pay1",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{p1}");

    // P2: 900,00 into Banka, 600→C allocated; 300 becomes A's credit.
    let (status, p2) = api_post_payment(
        test,
        cookie,
        csrf,
        banka,
        "900.00",
        json!([{ "assessmentId": c_asm["id"], "amount": "600.00" }]),
        "g-pay2",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{p2}");
    let p2_id = p2["payment"]["id"].as_str().unwrap();
    // STEP-009 auto-offset sweep: assigning A's 300,00 credit
    // immediately applies it FIFO to A's open assessments — it settles
    // A's remaining 200,00 in full, leaving 100,00 available. The whole
    // sweep moves ZERO money (entitlement, not a transfer).
    let movements_before = table_count(&test.pool, "account_movements").await;
    assert_eq!(
        api_assign_credit(test, cookie, csrf, p2_id, sh_a, "300.00", "g-cr").await,
        StatusCode::CREATED
    );
    assert_eq!(
        table_count(&test.pool, "account_movements").await,
        movements_before,
        "credit assignment + auto-sweep must create ZERO movements"
    );

    // Operational income/expense — distinct from every payment.
    let inc_cat = api_category(test, cookie, csrf, "Aidat Dışı Gelir", "income").await;
    let exp_cat = api_category(test, cookie, csrf, "Kırtasiye", "expense").await;
    let (status, _) = api_entry(
        test, cookie, csrf, "income", kasa, inc_cat, "250.00", "g-inc",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, e1) = api_entry(
        test, cookie, csrf, "expense", kasa, exp_cat, "75.50", "g-exp",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{e1}");

    // Internal transfer Kasa→Banka 400 — location only.
    assert_eq!(
        api_transfer(test, cookie, csrf, kasa, banka, "400.00", "g-tr").await,
        StatusCode::CREATED
    );

    // Share return for shareholder R (created AFTER the period so R has
    // no assessment): principal 500 determined + undetermined profit
    // right; settlement 200 from Kasa.
    let sh_r = api_create_shareholder(
        test,
        cookie,
        csrf,
        "İade",
        "Eden",
        json!({ "mode": "new", "sequenceNumber": 903 }),
    )
    .await;
    let initiated = api_initiate_return(test, cookie, csrf, sh_r, "g-ret").await;
    let return_id = initiated["id"].as_str().unwrap().to_string();
    let (status, fin) = api_finalize_return(
        test,
        cookie,
        csrf,
        &return_id,
        json!([
            { "entitlementType": "principal", "amount": "500.00",
              "dueDate": "2026-03-01", "policyReference": "YK-1" },
            { "entitlementType": "profit" }
        ]),
        initiated["updatedAt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "finalize: {fin}");
    let principal_id = fin["entitlements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["entitlementType"] == "principal")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        api_settle(test, cookie, csrf, &principal_id, kasa, "200.00", "g-set").await,
        StatusCode::CREATED
    );

    // Investment: funded 1 000 from Banka; valuation 1 500 (no cash);
    // income 120 into Banka (investment income ≠ operational income).
    let inv = api_create_investment(test, cookie, csrf, "Arsa Parseli", "g-inv").await;
    let inv_id = inv["id"].as_str().unwrap().to_string();
    assert_eq!(
        api_investment_event(
            test,
            cookie,
            csrf,
            &inv_id,
            "fundings",
            Some(banka),
            "1000.00",
            "g-fund"
        )
        .await,
        StatusCode::CREATED
    );
    assert_eq!(
        api_investment_event(
            test,
            cookie,
            csrf,
            &inv_id,
            "valuations",
            None,
            "1500.00",
            "g-val"
        )
        .await,
        StatusCode::CREATED
    );
    assert_eq!(
        api_investment_event(
            test,
            cookie,
            csrf,
            &inv_id,
            "incomes",
            Some(banka),
            "120.00",
            "g-iinc"
        )
        .await,
        StatusCode::CREATED
    );

    // Social aid: fund + donation 800 + disbursement 200 on Banka —
    // restricted 600, a classification of the same physical cash.
    let fund = api_create_fund(test, cookie, csrf, "Eğitim Fonu", "g-fund-sa").await;
    assert_eq!(
        api_aid(
            test,
            cookie,
            csrf,
            "donations",
            &fund,
            banka,
            "800.00",
            "g-don"
        )
        .await,
        StatusCode::CREATED
    );
    assert_eq!(
        api_aid(
            test,
            cookie,
            csrf,
            "disbursements",
            &fund,
            banka,
            "200.00",
            "g-dis"
        )
        .await,
        StatusCode::CREATED
    );

    // Governance: body + decided decision — zero money.
    let body_id = api_governance_body(test, cookie, csrf, "g-body").await;
    let (status, decision) = send(
        &test.app,
        req(
            "POST",
            "/api/governance/decisions",
            cookie,
            Some(csrf),
            Some(json!({
                "bodyId": body_id, "title": "Bütçe", "decisionText": "x",
                "decisionOn": "2026-01-14", "idempotencyKey": "g-dec",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "decision: {decision}");
    let decision_id = decision["id"].as_str().unwrap();
    for path in ["open"] {
        let (status, _) = send(
            &test.app,
            req(
                "POST",
                &format!("/api/governance/decisions/{decision_id}/{path}"),
                cookie,
                Some(csrf),
                Some(json!({})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/governance/decisions/{decision_id}/finalize"),
            cookie,
            Some(csrf),
            Some(json!({ "outcome": "approved" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let _ = (kasa, sh_b, sh_c, period_id);
    Golden { banka, sh_a, sh_r }
}

#[tokio::test]
async fn overview_returns_hand_computed_metrics() {
    let Some(test) = setup().await else { return };
    let (cookie, _csrf) = admin_session(&test).await;
    let g = build_golden(&test, &cookie, &_csrf).await;
    let _ = &g;

    let (status, o) = get(&test, &cookie, "/api/reports/overview").await;
    assert_eq!(status, StatusCode::OK, "{o}");

    // Cash: Kasa = 1200 +250 −75.50 −400 −200 = 774.50;
    //       Banka = 900 +400 −1000 +120 +800 −200 = 1020.00.
    // Total derived balance = 1794.50 — the sum of active movements.
    assert_eq!(o["financialAccountsBalance"], "1794.50");
    assert_eq!(o["financialAccountsCount"], 2);

    // Receivables: 3000 assessed − (1800 cash + 200 credit) = 1000.
    assert_eq!(o["assessmentsTotal"], "3000.00");
    assert_eq!(o["outstandingAssessmentDebt"], "1000.00");

    // Credit: 300 originated − 200 auto-applied = 100 available. Not cash.
    assert_eq!(o["availableShareholderCredit"], "100.00");

    // Operational flows: 250 income, 75.50 expense, net 174.50 —
    // payments/donations/investment income are NOT here.
    assert_eq!(o["operationalIncomeTotal"], "250.00");
    assert_eq!(o["operationalExpenseTotal"], "75.50");
    assert_eq!(o["operationalNet"], "174.50");

    // Payments posted: 1200 + 900.
    assert_eq!(o["postedPaymentsTotal"], "2100.00");
    assert_eq!(o["postedPaymentsCount"], 2);

    // Entitlements: 500 determined − 200 settled = 300 outstanding;
    // the undetermined profit right is COUNTED, never zeroed.
    assert_eq!(o["outstandingReturnEntitlementDetermined"], "300.00");
    assert_eq!(o["undeterminedEntitlementCount"], 1);
    assert_eq!(o["returnSettledTotal"], "200.00");

    // Investments: funding (cash-out) vs valuation (informational) vs
    // income (cash-in, non-operational) stay separate.
    assert_eq!(o["investmentTotalFunded"], "1000.00");
    assert_eq!(o["investmentLatestValuationTotal"], "1500.00");
    assert_eq!(o["investmentIncomeTotal"], "120.00");
    assert_eq!(o["investmentActiveCount"], 1);

    // Restricted availability = 800 − 200 = 600, a classification of
    // physical Banka cash — it is NOT added to any total.
    assert_eq!(o["socialAidRestrictedAvailable"], "600.00");
    assert_eq!(o["socialAidDonationsTotal"], "800.00");
    assert_eq!(o["socialAidDisbursementsTotal"], "200.00");

    // Identity counts.
    assert_eq!(o["activeShareholderCount"], 4);
    assert_eq!(o["activeShareCount"], 0); // R's share closed on finalize.
    assert_eq!(o["activeBodyCount"], 1);
    assert_eq!(o["activeMembershipCount"], 0);
    assert_eq!(o["decisionsApproved"], 1);
    assert_eq!(o["votesTotal"], 0);

    // No invented aggregates exist anywhere in the payload.
    let payload = serde_json::to_string(&o).unwrap();
    for banned in [
        "netWorth",
        "totalAssets",
        "profit",
        "nav",
        "balanceSheet",
        "realizedGain",
        "freeCash",
    ] {
        assert!(
            !payload.contains(banned),
            "invented metric leaked: {banned}"
        );
    }
}

// ---------------------------------------------------------------------
// 2. Accounts + movement provenance.
// ---------------------------------------------------------------------

#[tokio::test]
async fn accounts_and_movements_reports_trace_money() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let g = build_golden(&test, &cookie, &csrf).await;

    let (status, list) = get(&test, &cookie, "/api/reports/financial-accounts").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let items = list["items"].as_array().unwrap();
    let kasa = items.iter().find(|a| a["name"] == "Merkez Kasa").unwrap();
    let banka = items.iter().find(|a| a["name"] == "Vakıf TL").unwrap();
    assert_eq!(kasa["balance"], "774.50");
    assert_eq!(kasa["restrictedAvailable"], "0.00");
    assert_eq!(banka["balance"], "1020.00");
    // Restricted aid money classifies PART of Banka's physical balance.
    assert_eq!(banka["restrictedAvailable"], "600.00");

    // Movement ledger: every row carries provenance (source type+id+no).
    let (status, mv) = get(&test, &cookie, "/api/reports/movements?pageSize=100").await;
    assert_eq!(status, StatusCode::OK, "{mv}");
    let items = mv["items"].as_array().unwrap();
    assert!(items.iter().all(|m| m["sourceNumber"].is_number()));

    // Cross-domain classification: external inflows = payment 1200 +
    // payment 900 + income 250 + investment income 120 + donation 800
    // = 3270.00; external outflows = expense 75.50 + settlement 200 +
    // funding 1000 + disbursement 200 = 1475.50; internal transfer leg
    // volume = 400 (counted ONCE, inflow leg only).
    assert_eq!(mv["summary"]["externalInflow"], "3270.00");
    assert_eq!(mv["summary"]["externalOutflow"], "1475.50");
    assert_eq!(mv["summary"]["internalTransferVolume"], "400.00");

    // Filtered view agrees with itself.
    let (status, filtered) = get(
        &test,
        &cookie,
        "/api/reports/movements?sourceType=transfer&pageSize=5",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{filtered}");
    let items = filtered["items"].as_array().unwrap();
    assert_eq!(items.len(), 2); // outflow leg + inflow leg
    assert_eq!(filtered["summary"]["internalTransferVolume"], "400.00");
    assert_eq!(filtered["summary"]["externalInflow"], "0.00");

    // Account filter isolates one side.
    let (status, one) = get(
        &test,
        &cookie,
        &format!("/api/reports/movements?accountId={}", g.banka),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(one["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["accountId"] == g.banka.to_string()));
}

// ---------------------------------------------------------------------
// 3. Assessments / periods / shareholders / families — receivable
//    semantics.
// ---------------------------------------------------------------------

#[tokio::test]
async fn assessment_report_uses_canonical_settled_formula() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let g = build_golden(&test, &cookie, &csrf).await;

    let (status, summary) = get(&test, &cookie, "/api/reports/assessments/summary").await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["assessmentCount"], 3);
    assert_eq!(summary["totalAssessed"], "3000.00");
    assert_eq!(summary["totalPaymentAllocated"], "1800.00");
    assert_eq!(summary["totalCreditApplied"], "200.00");
    assert_eq!(summary["totalOutstanding"], "1000.00");
    assert_eq!(summary["fullyPaidCount"], 1);
    assert_eq!(summary["partiallyPaidCount"], 2);
    assert_eq!(summary["unpaidCount"], 0);

    // Row-level: A = 1000 − (800 cash + 200 credit) = 0 remaining.
    let (status, rows) = get(
        &test,
        &cookie,
        &format!("/api/reports/assessments?shareholderId={}", g.sh_a),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    let row = &rows["items"][0];
    assert_eq!(row["amount"], "1000.00");
    assert_eq!(row["paymentAllocated"], "800.00");
    assert_eq!(row["creditApplied"], "200.00");
    assert_eq!(row["remainingAmount"], "0.00");
    assert_eq!(row["familySequence"], 901);

    // Filtered summary must match the filtered dataset exactly.
    let (_status, fs) = get(
        &test,
        &cookie,
        &format!("/api/reports/assessments/summary?shareholderId={}", g.sh_a),
    )
    .await;
    assert_eq!(fs["totalOutstanding"], "0.00");
    assert_eq!(fs["assessmentCount"], 1);

    // Outstanding-only filter: A is fully settled — only B and C remain.
    let (status, out) = get(
        &test,
        &cookie,
        "/api/reports/assessments?settlement=outstanding&pageSize=1",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["totalCount"], 2);
    // Page=1 returns at most pageSize rows; the summary still spans all.
    assert_eq!(out["items"].as_array().unwrap().len(), 1);
    let (_status, out_summary) = get(
        &test,
        &cookie,
        "/api/reports/assessments/summary?settlement=outstanding",
    )
    .await;
    assert_eq!(out_summary["totalOutstanding"], "1000.00");
}

#[tokio::test]
async fn period_report_separates_cash_from_prior_excess() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let _g = build_golden(&test, &cookie, &csrf).await;

    let (status, rows) = get(&test, &cookie, "/api/reports/periods").await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    let p = &rows["items"][0];
    assert_eq!(p["totalAssessed"], "3000.00");
    assert_eq!(p["paymentAllocated"], "1800.00");
    assert_eq!(p["creditApplied"], "200.00"); // Prior excess — never new cash.
    assert_eq!(p["totalSatisfied"], "2000.00");
    assert_eq!(p["outstanding"], "1000.00");
    assert_eq!(p["assessmentCount"], 3);
    assert_eq!(p["fullyPaidCount"], 1);
    assert_eq!(p["partiallyPaidCount"], 2);
}

#[tokio::test]
async fn shareholder_report_never_nets_dimensions() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let g = build_golden(&test, &cookie, &csrf).await;

    let (status, rows) = get(&test, &cookie, "/api/reports/shareholders?pageSize=50").await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    let items = rows["items"].as_array().unwrap();
    let a = items
        .iter()
        .find(|r| r["shareholderId"] == g.sh_a.to_string())
        .unwrap();
    assert_eq!(a["totalAssessed"], "1000.00");
    assert_eq!(a["paymentAllocated"], "800.00");
    assert_eq!(a["creditApplied"], "200.00");
    assert_eq!(a["remainingDebt"], "0.00");
    assert_eq!(a["creditAvailable"], "100.00");
    // Debt 0, credit 100 — NO "net -100" column exists.
    assert!(a.get("netBalance").is_none());

    let r = items
        .iter()
        .find(|x| x["shareholderId"] == g.sh_r.to_string())
        .unwrap();
    assert_eq!(r["returnEntitlementDetermined"], "300.00");
    assert_eq!(r["undeterminedEntitlementCount"], 1);
    assert_eq!(r["remainingDebt"], "0.00");
}

#[tokio::test]
async fn family_report_aggregates_members_without_owning_debt() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let _g = build_golden(&test, &cookie, &csrf).await;

    let (status, rows) = get(&test, &cookie, "/api/reports/families").await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    let items = rows["items"].as_array().unwrap();
    let f901 = items.iter().find(|f| f["familySequence"] == 901).unwrap();
    // Members A (0 remaining) + B (600 remaining) = 600 member-sum.
    assert_eq!(f901["memberCount"], 2);
    assert_eq!(f901["memberTotalAssessed"], "2000.00");
    assert_eq!(f901["memberRemainingDebt"], "600.00");
    assert_eq!(f901["memberCreditAvailable"], "100.00");
}

// ---------------------------------------------------------------------
// 4. Payments vs credits — posted vs allocated, payer vs debtor.
// ---------------------------------------------------------------------

#[tokio::test]
async fn payment_report_separates_posted_allocated_credited() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let _g = build_golden(&test, &cookie, &csrf).await;

    let (status, rows) = get(&test, &cookie, "/api/reports/payments?pageSize=10").await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    let items = rows["items"].as_array().unwrap();
    let p1 = items.iter().find(|p| p["paymentNumber"] == 1).unwrap();
    let p2 = items.iter().find(|p| p["paymentNumber"] == 2).unwrap();

    assert_eq!(p1["amount"], "1200.00");
    assert_eq!(p1["allocatedAmount"], "1200.00");
    assert_eq!(p1["unassignedAmount"], "0.00");
    // Payer is a distinct Person; debtors are Ayşe + Mehmet.
    assert_eq!(p1["payerName"], "Veli Ödeyen");
    let debtors = p1["debtorShareholderNames"].as_str().unwrap();
    assert!(debtors.contains("Ayşe Üye") && debtors.contains("Mehmet Üye"));

    // P2: 600 allocated + 300 crystallized into credit = 0 unassigned.
    assert_eq!(p2["allocatedAmount"], "600.00");
    assert_eq!(p2["creditedAmount"], "300.00");
    assert_eq!(p2["unassignedAmount"], "0.00");

    // Summary spans the full filtered set (both pages' worth).
    assert_eq!(rows["summary"]["postedAmount"], "2100.00");
    assert_eq!(rows["summary"]["allocatedAmount"], "1800.00");
    assert_eq!(rows["summary"]["creditedAmount"], "300.00");
    assert_eq!(rows["summary"]["unassignedAmount"], "0.00");
}

#[tokio::test]
async fn credit_report_is_a_classification_not_cash() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let _g = build_golden(&test, &cookie, &csrf).await;

    let (status, rows) = get(&test, &cookie, "/api/reports/credits").await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    let c = &rows["items"][0];
    assert_eq!(c["amount"], "300.00");
    assert_eq!(c["appliedAmount"], "200.00");
    assert_eq!(c["availableAmount"], "100.00");
    assert_eq!(c["sourcePaymentNumber"], 2);
    assert_eq!(rows["summary"]["totalAvailable"], "100.00");
}

// ---------------------------------------------------------------------
// 5. Entitlements — NULL stays NULL.
// ---------------------------------------------------------------------

#[tokio::test]
async fn share_return_report_preserves_undetermined_null() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let _g = build_golden(&test, &cookie, &csrf).await;

    let (status, rows) = get(&test, &cookie, "/api/reports/share-returns").await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    let items = rows["items"].as_array().unwrap();
    let principal = items
        .iter()
        .find(|e| e["entitlementType"] == "principal")
        .unwrap();
    let profit = items
        .iter()
        .find(|e| e["entitlementType"] == "profit")
        .unwrap();

    assert_eq!(principal["amount"], "500.00");
    assert_eq!(principal["settledAmount"], "200.00");
    assert_eq!(principal["remainingAmount"], "300.00");

    // Undetermined right: amount NULL, remaining NULL — never "0.00".
    assert_eq!(profit["amount"], Value::Null);
    assert_eq!(profit["remainingAmount"], Value::Null);
    assert_eq!(profit["dueState"], "undetermined");

    assert_eq!(rows["summary"]["determinedOutstanding"], "300.00");
    assert_eq!(rows["summary"]["undeterminedCount"], 1);
    assert_eq!(rows["summary"]["settledTotal"], "200.00");
}

// ---------------------------------------------------------------------
// 6. Investments — funded vs valuation vs income vs disposal separate.
// ---------------------------------------------------------------------

#[tokio::test]
async fn investment_report_keeps_cost_valuation_income_separate() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let _g = build_golden(&test, &cookie, &csrf).await;

    let (status, rows) = get(&test, &cookie, "/api/reports/investments").await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    let inv = &rows["items"][0];
    assert_eq!(inv["totalFunded"], "1000.00");
    assert_eq!(inv["latestValuation"], "1500.00");
    assert_eq!(inv["latestValuationDate"], "2026-01-14");
    assert_eq!(inv["incomeTotal"], "120.00");
    assert_eq!(inv["disposalConsideration"], Value::Null);
    assert_eq!(inv["disposalProceedsReceived"], Value::Null);
    // No gain/loss field may exist.
    assert!(inv.get("gain").is_none() && inv.get("profit").is_none());
}

// ---------------------------------------------------------------------
// 7. Social Aid — (fund, account) dimension preserved.
// ---------------------------------------------------------------------

#[tokio::test]
async fn social_aid_report_preserves_fund_account_dimension() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let g = build_golden(&test, &cookie, &csrf).await;

    let (status, report) = get(&test, &cookie, "/api/reports/social-aid").await;
    assert_eq!(status, StatusCode::OK, "{report}");
    let fund = &report["funds"][0];
    assert_eq!(fund["fundName"], "Eğitim Fonu");
    assert_eq!(fund["donationsPosted"], "800.00");
    assert_eq!(fund["disbursementsPosted"], "200.00");
    assert_eq!(fund["restrictedAvailable"], "600.00");

    let pair = &report["pairs"][0];
    assert_eq!(pair["accountId"], g.banka.to_string());
    assert_eq!(pair["restrictedAvailable"], "600.00");
}

// ---------------------------------------------------------------------
// 8. Governance — recorded outcomes, never recomputed.
// ---------------------------------------------------------------------

#[tokio::test]
async fn governance_report_shows_recorded_evidence() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let _g = build_golden(&test, &cookie, &csrf).await;

    let (status, report) = get(&test, &cookie, "/api/reports/governance").await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["activeBodyCount"], 1);
    assert_eq!(report["decisionsByStatus"]["approved"], 1);
    let recent = &report["recentFinalized"][0];
    assert_eq!(recent["status"], "approved");
    assert_eq!(recent["eligibleCount"], 0);
    // Governance produced zero money — overview still shows the fixture.
    let (_, o) = get(&test, &cookie, "/api/reports/overview").await;
    assert_eq!(o["financialAccountsBalance"], "1794.50");
}

// ---------------------------------------------------------------------
// 9. Reversal — excluded from totals, still traceable.
// ---------------------------------------------------------------------

#[tokio::test]
async fn reversed_rows_leave_totals_but_stay_traceable() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let kasa = api_create_account(&test, &cookie, &csrf, "Kasa", "cash").await;
    let inc_cat = api_category(&test, &cookie, &csrf, "Gelir", "income").await;

    let (status, i1) = api_entry(
        &test, &cookie, &csrf, "income", kasa, inc_cat, "300.00", "r-i1",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let i1_id = i1["id"].as_str().unwrap().to_string();
    let (status, _i2) = api_entry(
        &test, &cookie, &csrf, "income", kasa, inc_cat, "50.00", "r-i2",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Reverse the 300 income — the movement turns 'reversed'.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/incomes/{i1_id}/reverse"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "yanlış kayıt" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, o) = get(&test, &cookie, "/api/reports/overview").await;
    assert_eq!(o["operationalIncomeTotal"], "50.00");
    assert_eq!(o["financialAccountsBalance"], "50.00");

    // The reversed movement is still VISIBLE as evidence (status filter
    // shows it; default totals exclude it).
    let (_, all) = get(&test, &cookie, "/api/reports/movements?status=reversed").await;
    let rev = &all["items"][0];
    assert_eq!(rev["status"], "reversed");
    assert_eq!(rev["amount"], "300.00");
    assert_eq!(rev["reversalReason"], "yanlış kayıt");
    // Active-only totals exclude it.
    let (_, act) = get(&test, &cookie, "/api/reports/movements?status=active").await;
    assert_eq!(act["summary"]["externalInflow"], "50.00");
}

// ---------------------------------------------------------------------
// 10. Exact money — decimal strings, no float artifacts.
// ---------------------------------------------------------------------

#[tokio::test]
async fn exact_decimal_aggregation_never_drifts() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let kasa = api_create_account(&test, &cookie, &csrf, "Kasa", "cash").await;
    let inc = api_category(&test, &cookie, &csrf, "G", "income").await;

    // 0.10 + 0.20 must read "0.30" — never a float artifact.
    for (amount, key) in [("0.10", "d-1"), ("0.20", "d-2")] {
        let (status, _) = api_entry(&test, &cookie, &csrf, "income", kasa, inc, amount, key).await;
        assert_eq!(status, StatusCode::CREATED);
    }
    // Large value near NUMERIC(19,2) expectations — exact scale kept.
    let (status, _) = api_entry(
        &test,
        &cookie,
        &csrf,
        "income",
        kasa,
        inc,
        "999999999999999.99",
        "d-3",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (_, o) = get(&test, &cookie, "/api/reports/overview").await;
    assert_eq!(o["operationalIncomeTotal"], "1000000000000000.29");
    assert_eq!(o["financialAccountsBalance"], "1000000000000000.29");
}

// ---------------------------------------------------------------------
// 11. Read-only proof — browsing mutates nothing.
// ---------------------------------------------------------------------

#[tokio::test]
async fn browsing_reports_mutates_nothing() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let _g = build_golden(&test, &cookie, &csrf).await;

    const TABLES: &[&str] = &[
        "account_movements",
        "payments",
        "payment_allocations",
        "shareholder_credits",
        "credit_applications",
        "income_entries",
        "expense_entries",
        "share_return_settlements",
        "investment_fundings",
        "investment_valuations",
        "investment_incomes",
        "social_aid_donations",
        "social_aid_disbursements",
        "governance_votes",
        "governance_decisions",
        "assessments",
    ];
    let mut before = Vec::new();
    for table in TABLES {
        before.push(table_count(&test.pool, table).await);
    }
    // Login + fixture writes legitimately emit audit rows — the
    // invariant is that READS add none, so snapshot the baseline.
    let events_before: i64 = sqlx::query_scalar("SELECT count(*) FROM security_events")
        .fetch_one(&test.pool)
        .await
        .unwrap();

    // Browse every report endpoint.
    for path in [
        "/api/reports/overview",
        "/api/reports/financial-accounts",
        "/api/reports/movements",
        "/api/reports/assessments",
        "/api/reports/assessments/summary",
        "/api/reports/periods",
        "/api/reports/shareholders",
        "/api/reports/families",
        "/api/reports/payments",
        "/api/reports/payments/summary",
        "/api/reports/credits",
        "/api/reports/share-returns",
        "/api/reports/investments",
        "/api/reports/social-aid",
        "/api/reports/governance",
        "/api/reports/income-expense-trend",
    ] {
        let (status, body) = get(&test, &cookie, path).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }

    for (i, table) in TABLES.iter().enumerate() {
        assert_eq!(
            table_count(&test.pool, table).await,
            before[i],
            "report browsing mutated {table}"
        );
    }
    // No audit noise either: reads must not emit security events.
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM security_events")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(
        events, events_before,
        "report reads must not emit audit events"
    );
}

// ---------------------------------------------------------------------
// 12. Permissions — reports.read is its own cross-domain grant.
// ---------------------------------------------------------------------

#[tokio::test]
async fn reports_require_their_own_permission() {
    let Some(test) = setup().await else { return };
    let _admin = admin_session(&test).await;

    // A user with NO roles: denied everywhere.
    let plain = format!("plain.{}", Uuid::new_v4().simple());
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).unwrap(),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).unwrap();
    users::create_user(&test.pool, &plain, "Plain", &hash)
        .await
        .unwrap();
    let (cookie, _) = login(&test.app, test.peer(), &plain).await;
    for path in [
        "/api/reports/overview",
        "/api/reports/movements",
        "/api/reports/shareholders",
        "/api/reports/social-aid",
    ] {
        let (status, _) = get(&test, &cookie, path).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path} must deny plain user");
    }

    // A user holding only a DOMAIN read permission still cannot read
    // cross-domain reports.
    let scoped = format!("scoped.{}", Uuid::new_v4().simple());
    create_scoped_user(&test.pool, &scoped, &["payments.read"]).await;
    let (cookie, _) = login(&test.app, test.peer(), &scoped).await;
    let (status, _) = get(&test, &cookie, "/api/reports/overview").await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // reports.read alone opens every report (but no domain mutation).
    let reader = format!("reader.{}", Uuid::new_v4().simple());
    create_scoped_user(&test.pool, &reader, &["reports.read"]).await;
    let (cookie, _) = login(&test.app, test.peer(), &reader).await;
    let (status, _) = get(&test, &cookie, "/api/reports/overview").await;
    assert_eq!(status, StatusCode::OK);
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
    // CSRF/permission still enforced on mutations — reporting access
    // grants nothing beyond reading.
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED,
        "reader mutation must be denied: {status}"
    );
}

// ---------------------------------------------------------------------
// 13. Filter/date validation + no-store header.
// ---------------------------------------------------------------------

#[tokio::test]
async fn filters_are_validated_and_responses_no_store() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let _g = build_golden(&test, &cookie, &csrf).await;

    // Invalid enums/dates rejected instead of silently widened.
    for path in [
        "/api/reports/movements?sourceType=bogus",
        "/api/reports/movements?direction=sideways",
        "/api/reports/movements?status=deleted",
        "/api/reports/movements?dateFrom=15.01.2026",
        "/api/reports/assessments?settlement=maybe",
        "/api/reports/payments?method=gold",
        "/api/reports/financial-accounts?status=closed",
        "/api/reports/income-expense-trend?months=99",
    ] {
        let (status, body) = get(&test, &cookie, path).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {body}");
    }

    // Date filter on business date (occurred_at): investment income at
    // 2026-01-12 is inside Jan 10..15; payment received_at=now (Jan 15).
    let (_, in_range) = get(
        &test,
        &cookie,
        "/api/reports/movements?dateFrom=2026-01-10&dateTo=2026-01-15",
    )
    .await;
    assert_eq!(in_range["summary"]["externalInflow"], "3270.00");

    // A narrow Jan-15-only window: payments (2100) + operational income
    // (250) default to `now`; the Jan-12 events (investment income,
    // donation) fall outside.
    let (_, narrow) = get(
        &test,
        &cookie,
        "/api/reports/movements?dateFrom=2026-01-15&dateTo=2026-01-15",
    )
    .await;
    assert_eq!(narrow["summary"]["externalInflow"], "2350.00");

    // no-store on report responses.
    let request = req("GET", "/api/reports/overview", &cookie, None, None);
    let response = test.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
}

// ---------------------------------------------------------------------
// 14. Income/expense monthly trend buckets by business date.
// ---------------------------------------------------------------------

#[tokio::test]
async fn monthly_trend_buckets_by_business_date() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let kasa = api_create_account(&test, &cookie, &csrf, "Kasa", "cash").await;
    let inc = api_category(&test, &cookie, &csrf, "G", "income").await;

    // Backdated income belongs to December's bucket, not January's.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/incomes",
            &cookie,
            Some(&csrf),
            Some(json!({
                "financialAccountId": kasa.to_string(),
                "categoryId": inc.to_string(),
                "amount": "40.00",
                "description": "aralık geliri",
                "occurredAt": "2025-12-20T10:00:00Z",
                "idempotencyKey": "t-dec",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = api_entry(&test, &cookie, &csrf, "income", kasa, inc, "60.00", "t-jan").await;
    assert_eq!(status, StatusCode::CREATED);

    // Buckets are generated relative to the REAL database clock
    // (now()), not the frozen test clock — request the maximum window
    // so 2025-12/2026-01 are always inside it.
    let (status, trend) = get(
        &test,
        &cookie,
        "/api/reports/income-expense-trend?months=24",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{trend}");
    let rows = trend.as_array().unwrap();
    let dec = rows.iter().find(|r| r["month"] == "2025-12-01").unwrap();
    let jan = rows.iter().find(|r| r["month"] == "2026-01-01").unwrap();
    assert_eq!(dec["incomeTotal"], "40.00");
    assert_eq!(jan["incomeTotal"], "60.00");
}
