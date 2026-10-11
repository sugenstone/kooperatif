//! M1-P1-c3c5 — cooperative-scoped social_aid + governance
//! authorization tests.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same harness as tests/shares_investments_cooperative.rs). These
//! tests run the REAL router and prove that the migrated social_aid and
//! governance endpoints resolve authorization only within the verified
//! bootstrap scope:
//!   - headerless requests pin bootstrap; a foreign header never
//!     re-scopes;
//!   - malformed/unknown/non-bootstrap `x-cooperative-id` rejects;
//!   - suspended/ended bootstrap membership is denied on reads and
//!     mutations alike — denied mutations write zero rows;
//!   - permissions granted in Cooperative B never authorize bootstrap
//!     (A) social-aid or governance operations;
//!   - NULL-attributed legacy roles keep working inside bootstrap;
//!   - mutations keep both CSRF layers and audit events carry
//!     cooperative attribution;
//!   - social-aid restricted-fund accounting (donation inflow,
//!     disbursement outflow, dual availability guards, reversals,
//!     fund close/cancel) stays byte-identical under scoped auth;
//!   - governance lifecycle (body → memberships → decision draft →
//!     open → votes → finalize snapshot; draft-only edit/cancel;
//!     body close guards) preserves every transition rule;
//!   - the never-bootstrapped lane keeps global semantics untouched.
//!
//! Financial/state-machine parity itself is proven by the unchanged
//! `social_aid.rs` and `governance.rs` regression suites; the parity
//! tests here exercise the same paths THROUGH the migrated
//! authorization layer.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

use kooperatif_server::auth::{identity, users, AuthRuntime};
use kooperatif_server::clock::MutableClock;
use kooperatif_server::config::{AuthConfig, ARGON2_M_COST_FLOOR};
use kooperatif_server::db as app_db;
use kooperatif_server::http::{cors_layer, router, AppState};
use kooperatif_server::tenant::repo;

const TEST_PASSWORD: &str = "p1c3c5-test-parola-123";
const ORIGIN: &str = "http://localhost:5173";

fn test_database_url() -> Option<String> {
    let value = std::env::var("KOOPERATIF_TEST_DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "KOOPERATIF_TEST_DATABASE_URL is not set — required DB-gated \
             integration tests cannot silently pass; point it at a \
             disposable PostgreSQL database"
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
    _claim: app_db::TestDbClaim,
    app: Router,
    pool: PgPool,
    peer: SocketAddr,
    database_name: String,
    admin_url: String,
}

impl TestApp {
    async fn cleanup(&self) {
        let _ = sqlx::query(&format!(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}' AND pid <> pg_backend_pid()",
            self.database_name
        ))
        .execute(&self.pool)
        .await;
        self.pool.close().await;
        if let Ok(admin) = PgPoolOptions::new()
            .max_connections(1)
            .connect(&self.admin_url)
            .await
        {
            let _ = sqlx::query(&format!(
                "DROP DATABASE IF EXISTS {} WITH (FORCE)",
                self.database_name
            ))
            .execute(&admin)
            .await;
            admin.close().await;
        }
    }
}

static SETUP_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

async fn setup() -> Option<TestApp> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .try_init();
    let _setup_slot = SETUP_SLOTS.acquire().await.expect("setup slot");
    let url = test_database_url()?;
    let suffix = Uuid::new_v4().simple();
    let database_name = format!("kooperatif_test_{suffix}");
    let admin_url = url
        .rsplit_once('/')
        .map(|(base, _)| format!("{base}/postgres"))
        .expect("url with database segment");
    let base_url = url
        .rsplit_once('/')
        .map(|(base, _)| base.to_string())
        .expect("url with database segment");
    let admin = app_db::connect(&admin_url)
        .await
        .expect("admin connectivity");
    let _ = app_db::drop_stale_test_databases(&admin).await;
    let claim = app_db::create_test_database(&admin, &admin_url, &database_name).await;
    admin.close().await;
    let pool = app_db::connect(&format!("{base_url}/{database_name}"))
        .await
        .expect("test database connectivity");
    app_db::run_migrations(&pool)
        .await
        .expect("migrations applied");
    let clock = Arc::new(MutableClock::new(
        OffsetDateTime::parse(
            "2026-01-01T00:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .expect("fixed epoch"),
    ));
    let app = router(
        AppState {
            db: Some(pool.clone()),
            auth: Arc::new(AuthRuntime::new(
                AuthConfig {
                    allowed_origins: vec![ORIGIN.to_string()],
                    cookie_secure: false,
                    session_absolute_ttl_secs: 3600,
                    session_idle_ttl_secs: 600,
                    argon2_m_cost: ARGON2_M_COST_FLOOR,
                    argon2_t_cost: 1,
                    argon2_p_cost: 1,
                    login_window_secs: 60,
                    login_username_max_attempts: 5,
                    login_ip_max_attempts: 100_000,
                    forwarded_ip: false,
                },
                clock,
            )),
        },
        cors_layer(&[]),
    );
    let bytes = Uuid::new_v4();
    let octets = bytes.as_bytes();
    Some(TestApp {
        _claim: claim,
        app,
        pool,
        database_name,
        admin_url,
        peer: format!(
            "127.{}.{}.{}:{}",
            octets[0],
            octets[1],
            octets[2],
            50000 + (octets[3] as u16)
        )
        .parse()
        .expect("derived addr"),
    })
}

async fn create_user(pool: &PgPool, username: &str) -> Uuid {
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).expect("test parameters"),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).expect("hash");
    users::create_user(pool, username, "P1c3c5 Test", &hash)
        .await
        .expect("user created")
        .id
}

/// Role + assignment fixture. `coop` None writes the legacy NULL lane;
/// `Some(c)` writes composite-FK-consistent scoped rows (membership in
/// `c` must already exist).
async fn grant_role(
    pool: &PgPool,
    user: Uuid,
    name: &str,
    coop: Option<Uuid>,
    perms: &[&str],
) -> Uuid {
    let role: Uuid =
        sqlx::query_scalar("INSERT INTO roles (name, cooperative_id) VALUES ($1, $2) RETURNING id")
            .bind(name)
            .bind(coop)
            .fetch_one(pool)
            .await
            .expect("role");
    for key in perms {
        sqlx::query(
            "INSERT INTO role_permissions (role_id, permission_id) \
             SELECT $1, id FROM permissions WHERE key = $2",
        )
        .bind(role)
        .bind(key)
        .execute(pool)
        .await
        .expect("perm");
    }
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
         VALUES ($1, $2, $3)",
    )
    .bind(user)
    .bind(role)
    .bind(coop)
    .execute(pool)
    .await
    .expect("assignment");
    role
}

/// Log in over HTTP; returns (cookie token, csrf token).
async fn login(app: &Router, peer: SocketAddr, username: &str) -> (String, String) {
    let mut request = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .header("user-agent", "Mozilla/5.0 P1c3c5")
        .body(Body::from(
            json!({ "username": username, "password": TEST_PASSWORD }).to_string(),
        ))
        .expect("valid request");
    request.extensions_mut().insert(ConnectInfo(peer));
    let response = app.clone().oneshot(request).await.expect("infallible");
    assert_eq!(response.status(), StatusCode::OK, "login failed");
    let cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .and_then(|pair| pair.split_once('=').map(|(_, v)| v.to_string()))
        .expect("cookie token");
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body: Value = serde_json::from_slice(&bytes).expect("json");
    (
        cookie,
        body["csrfToken"].as_str().expect("csrf").to_string(),
    )
}

async fn request_json(
    test: &TestApp,
    method: Method,
    path: &str,
    cookie: &str,
    csrf: Option<&str>,
    coop: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, String) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("cookie", format!("kooperatif_session={cookie}"))
        .header("origin", ORIGIN);
    if let Some(token) = csrf {
        builder = builder.header("x-csrf-token", token);
    }
    if let Some(c) = coop {
        builder = builder.header("x-cooperative-id", c);
    }
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let mut request = builder.body(payload).expect("request");
    request.extensions_mut().insert(ConnectInfo(test.peer));
    let response = test.app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn bootstrap(pool: &PgPool) -> Uuid {
    repo::bootstrap_initial_cooperative(pool, "Ana Kooperatif")
        .await
        .expect("bootstrap")
        .0
}

/// Every permission the social-aid/governance surfaces can check —
/// plus the already-migrated permissions the fixtures need (accounts +
/// income_expense for account funding and the unrestricted guard).
const OPERATOR_PERMS: &[&str] = &[
    "social_aid.read",
    "social_aid.manage",
    "governance.read",
    "governance.manage",
    "financial_accounts.read",
    "financial_accounts.manage",
    "income_expense.read",
    "income_expense.manage",
];

/// Bootstrap-scoped operator fixture: user created BEFORE bootstrap
/// (auto-member) with a bootstrap-owned business role.
async fn bootstrap_operator(pool: &PgPool, username: &str) -> (Uuid, Uuid) {
    let user = create_user(pool, username).await;
    let coop = bootstrap(pool).await;
    grant_role(
        pool,
        user,
        &format!("op-{username}"),
        Some(coop),
        OPERATOR_PERMS,
    )
    .await;
    (user, coop)
}

/// Canonical Person row — governance members and social-aid donors/
/// beneficiaries are Person facts, never derived from Users.
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
    .expect("person")
}

/// Cash account through the migrated endpoint.
async fn make_account(test: &TestApp, cookie: &str, csrf: &str, name: &str) -> Uuid {
    let (status, body) = request_json(
        test,
        Method::POST,
        "/api/financial-accounts",
        cookie,
        Some(csrf),
        None,
        Some(json!({ "name": name, "accountType": "cash" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

/// Fund an account with unrestricted cash through the migrated
/// income_expense router.
async fn fund_account(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    account: Uuid,
    amount: &str,
    key: &str,
) {
    let (status, list) = request_json(
        test,
        Method::GET,
        "/api/financial-categories/options?categoryType=income",
        cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let category = serde_json::from_str::<Value>(&list)
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Diğer Gelir")
        .expect("category")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, body) = request_json(
        test,
        Method::POST,
        "/api/incomes",
        cookie,
        Some(csrf),
        None,
        Some(json!({
            "financialAccountId": account.to_string(),
            "categoryId": category,
            "amount": amount,
            "description": "kasa fonlama",
            "idempotencyKey": key,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

// ---------------------------------------------------------------------
// Social Aid helpers
// ---------------------------------------------------------------------

/// Fund through the migrated social-aid router; returns the detail.
async fn make_fund(test: &TestApp, cookie: &str, csrf: &str, name: &str, key: &str) -> Value {
    let (status, body) = request_json(
        test,
        Method::POST,
        "/api/social-aid/funds",
        cookie,
        Some(csrf),
        None,
        Some(json!({
            "name": name,
            "idempotencyKey": key,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_str::<Value>(&body).unwrap()
}

fn donation_body(fund: &str, account: Uuid, amount: &str, key: &str) -> Value {
    json!({
        "fundId": fund,
        "donorDisplayName": "Hayırsever A.Ş.",
        "financialAccountId": account.to_string(),
        "amount": amount,
        "occurredAt": "2025-12-15T10:00:00Z",
        "idempotencyKey": key,
    })
}

fn disbursement_body(fund: &str, account: Uuid, amount: &str, key: &str) -> Value {
    json!({
        "fundId": fund,
        "beneficiaryDisplayName": "İhtiyaç Sahibi",
        "financialAccountId": account.to_string(),
        "amount": amount,
        "occurredAt": "2025-12-16T10:00:00Z",
        "reason": "gıda yardımı",
        "idempotencyKey": key,
    })
}

// ---------------------------------------------------------------------
// Governance helpers
// ---------------------------------------------------------------------

/// Body through the migrated governance router; returns the detail.
async fn make_body(test: &TestApp, cookie: &str, csrf: &str, name: &str, key: &str) -> Value {
    let (status, body) = request_json(
        test,
        Method::POST,
        "/api/governance/bodies",
        cookie,
        Some(csrf),
        None,
        Some(json!({
            "name": name,
            "bodyType": "Yönetim Kurulu",
            "idempotencyKey": key,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_str::<Value>(&body).unwrap()
}

fn membership_body(person: Uuid, key: &str) -> Value {
    json!({
        "personId": person.to_string(),
        "startedAt": "2025-06-01T09:00:00Z",
        "idempotencyKey": key,
    })
}

fn decision_body(body_id: &str, title: &str, key: &str) -> Value {
    json!({
        "bodyId": body_id,
        "title": title,
        "decisionText": "Karar metni: genel kurul gündemi.",
        "decisionOn": "2025-12-20",
        "idempotencyKey": key,
    })
}

fn vote_body(person: Uuid, choice: &str, key: &str) -> Value {
    json!({
        "personId": person.to_string(),
        "choice": choice,
        "idempotencyKey": key,
    })
}

// ---------------------------------------------------------------------
// Counters
// ---------------------------------------------------------------------

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count")
}

async fn movements_for(test: &TestApp, source_type: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM account_movements WHERE source_type = $1")
        .bind(source_type)
        .fetch_one(&test.pool)
        .await
        .expect("movements")
}

/// Posted (not reversed) movements of one source type.
async fn posted_movements(test: &TestApp, source_type: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM account_movements \
         WHERE source_type = $1 AND status = 'posted'",
    )
    .bind(source_type)
    .fetch_one(&test.pool)
    .await
    .expect("posted movements")
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[tokio::test]
async fn bootstrap_operator_reaches_social_aid_and_governance() {
    let Some(test) = setup().await else { return };
    let (_, coop) = bootstrap_operator(&test.pool, "operator").await;
    let (cookie, csrf) = login(&test.app, test.peer, "operator").await;

    // Headerless bootstrap reads on both routers.
    for path in [
        "/api/social-aid/funds",
        "/api/social-aid/donations",
        "/api/social-aid/disbursements",
        "/api/governance/bodies",
        "/api/governance/decisions",
    ] {
        let (status, body) =
            request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }

    // One mutation on each router under scoped auth.
    let fund = make_fund(&test, &cookie, &csrf, "Gıda Fonu", "f-op-1").await;
    assert_eq!(fund["status"], "active");
    let body = make_body(&test, &cookie, &csrf, "Yönetim Kurulu", "b-op-1").await;
    assert_eq!(body["status"], "active");

    // Detail reads see the created rows.
    for path in [
        format!("/api/social-aid/funds/{}", fund["id"].as_str().unwrap()),
        format!("/api/governance/bodies/{}", body["id"].as_str().unwrap()),
    ] {
        let (status, body) =
            request_json(&test, Method::GET, &path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }

    // Explicit bootstrap header pins the same scope — allowed.
    let coop_str = coop.to_string();
    for path in ["/api/social-aid/funds", "/api/governance/bodies"] {
        let (status, body) = request_json(
            &test,
            Method::GET,
            path,
            &cookie,
            None,
            Some(&coop_str),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }

    test.cleanup().await;
}

#[tokio::test]
async fn cooperative_header_rules_apply_to_both_routers() {
    let Some(test) = setup().await else { return };
    bootstrap_operator(&test.pool, "hdr_op").await;
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;
    let (cookie, csrf) = login(&test.app, test.peer, "hdr_op").await;
    let b_str = coop_b.to_string();
    let unknown = Uuid::new_v4().to_string();

    for path in [
        "/api/social-aid/funds",
        "/api/social-aid/donations",
        "/api/governance/bodies",
        "/api/governance/decisions",
        "/api/governance/persons?search=a",
        "/api/social-aid/persons?search=a",
    ] {
        // Malformed UUID → 400.
        let (status, _) = request_json(
            &test,
            Method::GET,
            path,
            &cookie,
            None,
            Some("not-a-uuid"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
        // Explicit non-bootstrap cooperative → 403, never a silent
        // bootstrap fallback.
        let (status, _) =
            request_json(&test, Method::GET, path, &cookie, None, Some(&b_str), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        // Unknown-but-well-formed UUID → 403 (existence never leaked).
        let (status, _) = request_json(
            &test,
            Method::GET,
            path,
            &cookie,
            None,
            Some(&unknown),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    // Mutations obey the same rules — and write nothing.
    for (path, body) in [
        (
            "/api/social-aid/funds",
            json!({ "name": "X", "idempotencyKey": "f-denied" }),
        ),
        (
            "/api/governance/bodies",
            json!({ "name": "X", "bodyType": "Denetim", "idempotencyKey": "b-denied" }),
        ),
    ] {
        let (status, _) = request_json(
            &test,
            Method::POST,
            path,
            &cookie,
            Some(&csrf),
            Some(&b_str),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    assert_eq!(count(&test.pool, "social_aid_funds").await, 0);
    assert_eq!(count(&test.pool, "governance_bodies").await, 0);

    test.cleanup().await;
}

#[tokio::test]
async fn suspended_and_ended_memberships_are_denied() {
    let Some(test) = setup().await else { return };
    let (user, coop) = bootstrap_operator(&test.pool, "askida").await;
    let (cookie, csrf) = login(&test.app, test.peer, "askida").await;
    let fund = make_fund(&test, &cookie, &csrf, "Fon", "f-sus-1").await;
    let fund_id = fund["id"].as_str().unwrap().to_string();
    let body = make_body(&test, &cookie, &csrf, "Kurul", "b-sus-1").await;
    let body_id = body["id"].as_str().unwrap().to_string();

    for status in ["suspended", "ended"] {
        sqlx::query(
            "UPDATE cooperative_memberships SET status = $1 \
             WHERE user_id = $2 AND cooperative_id = $3",
        )
        .bind(status)
        .bind(user)
        .bind(coop)
        .execute(&test.pool)
        .await
        .expect("membership status");
        // Reads denied on both routers.
        for path in [
            "/api/social-aid/funds".to_string(),
            "/api/governance/bodies".to_string(),
            format!("/api/social-aid/funds/{fund_id}"),
            format!("/api/governance/bodies/{body_id}"),
            "/api/governance/persons?search=a".to_string(),
        ] {
            let (code, _) =
                request_json(&test, Method::GET, &path, &cookie, None, None, None).await;
            assert_eq!(code, StatusCode::FORBIDDEN, "{status} {path}");
        }
        // Mutations denied — authorization precedes any write.
        let (code, _) = request_json(
            &test,
            Method::POST,
            "/api/social-aid/funds",
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "name": "Y", "idempotencyKey": "f-deny" })),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "{status} fund create");
        let (code, _) = request_json(
            &test,
            Method::POST,
            "/api/governance/bodies",
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "name": "Y", "bodyType": "Denetim", "idempotencyKey": "b-deny" })),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "{status} body create");
    }
    // Fixture rows only — the denied mutations added nothing.
    assert_eq!(count(&test.pool, "social_aid_funds").await, 1);
    assert_eq!(count(&test.pool, "social_aid_donations").await, 0);
    assert_eq!(count(&test.pool, "social_aid_disbursements").await, 0);
    assert_eq!(count(&test.pool, "governance_bodies").await, 1);
    assert_eq!(count(&test.pool, "governance_memberships").await, 0);
    assert_eq!(count(&test.pool, "governance_decisions").await, 0);
    assert_eq!(count(&test.pool, "governance_votes").await, 0);
    assert_eq!(count(&test.pool, "account_movements").await, 0);

    test.cleanup().await;
}

#[tokio::test]
async fn foreign_cooperative_permissions_never_authorize_bootstrap_ops() {
    let Some(test) = setup().await else { return };
    let coop_a = bootstrap(&test.pool).await;
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;

    // A-member whose only rights live in B (B-admin): A sees nothing.
    let b_admin = create_user(&test.pool, "b_yetkili").await;
    repo::add_member(&test.pool, "Ana Kooperatif", b_admin, None)
        .await
        .expect("A member");
    repo::add_member(&test.pool, "B Kooperatifi", b_admin, None)
        .await
        .expect("B member");
    grant_role(&test.pool, b_admin, "b-admin", Some(coop_b), OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "b_yetkili").await;
    for path in ["/api/social-aid/funds", "/api/governance/bodies"] {
        let (status, _) = request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    let (status, _) = request_json(
        &test,
        Method::POST,
        "/api/social-aid/funds",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "name": "X", "idempotencyKey": "f-fk" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(count(&test.pool, "social_aid_funds").await, 0);

    // Inverse direction: A-viewer / B-admin — A's reads work, B's
    // manage power never crosses into A.
    let dual = create_user(&test.pool, "cift").await;
    repo::add_member(&test.pool, "Ana Kooperatif", dual, None)
        .await
        .expect("A member");
    repo::add_member(&test.pool, "B Kooperatifi", dual, None)
        .await
        .expect("B member");
    grant_role(
        &test.pool,
        dual,
        "a-izleyici",
        Some(coop_a),
        &["social_aid.read", "governance.read"],
    )
    .await;
    grant_role(&test.pool, dual, "b-admin2", Some(coop_b), OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "cift").await;
    for path in ["/api/social-aid/funds", "/api/governance/bodies"] {
        let (status, body) =
            request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }
    // social_aid.manage + governance.manage exist only in B.
    for (path, body) in [
        (
            "/api/social-aid/funds",
            json!({ "name": "Y", "idempotencyKey": "f-fk2" }),
        ),
        (
            "/api/governance/bodies",
            json!({ "name": "Y", "bodyType": "Denetim", "idempotencyKey": "b-fk2" }),
        ),
    ] {
        let (status, _) = request_json(
            &test,
            Method::POST,
            path,
            &cookie,
            Some(&csrf),
            None,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    assert_eq!(count(&test.pool, "social_aid_funds").await, 0);
    assert_eq!(count(&test.pool, "governance_bodies").await, 0);

    test.cleanup().await;
}

#[tokio::test]
async fn null_attributed_legacy_roles_keep_working() {
    let Some(test) = setup().await else { return };
    let legacy = create_user(&test.pool, "eski_yetkili").await;
    let coop = bootstrap(&test.pool).await;
    grant_role(&test.pool, legacy, "legacy-op", None, OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "eski_yetkili").await;

    for path in ["/api/social-aid/funds", "/api/governance/bodies"] {
        let (status, body) =
            request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }
    let fund = make_fund(&test, &cookie, &csrf, "Miras Fon", "f-leg").await;
    assert_eq!(fund["status"], "active");
    // Attribution is still the verified bootstrap scope, not NULL.
    let (scoped,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM security_events \
         WHERE cooperative_id = $1 AND event_type = 'social_aid_fund_created'",
    )
    .bind(coop)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(scoped, 1);

    test.cleanup().await;
}

#[tokio::test]
async fn unauthorized_reads_and_mutations_denied_with_zero_writes() {
    let Some(test) = setup().await else { return };
    bootstrap_operator(&test.pool, "yetkili").await;
    let (cookie, csrf) = login(&test.app, test.peer, "yetkili").await;
    let fund = make_fund(&test, &cookie, &csrf, "Fon", "f-fixture").await;
    let fund_id = fund["id"].as_str().unwrap().to_string();
    let body = make_body(&test, &cookie, &csrf, "Kurul", "b-fixture").await;
    let body_id = body["id"].as_str().unwrap().to_string();
    let (status, dbody) = request_json(
        &test,
        Method::POST,
        "/api/governance/decisions",
        &cookie,
        Some(&csrf),
        None,
        Some(decision_body(&body_id, "Gündem", "d-fixture")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{dbody}");
    let decision_id = serde_json::from_str::<Value>(&dbody).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let fresh = Uuid::new_v4().to_string();

    // A bootstrap member with NO business role.
    let plain = create_user(&test.pool, "siradan").await;
    repo::add_member(&test.pool, "Ana Kooperatif", plain, None)
        .await
        .expect("member");
    let (pcookie, pcsrf) = login(&test.app, test.peer, "siradan").await;

    // Every read surface denies — including the manage-gated person
    // lookups.
    for path in [
        "/api/social-aid/funds".to_string(),
        format!("/api/social-aid/funds/{fund_id}"),
        "/api/social-aid/donations".to_string(),
        format!("/api/social-aid/donations/{fresh}"),
        "/api/social-aid/disbursements".to_string(),
        format!("/api/social-aid/disbursements/{fresh}"),
        "/api/social-aid/persons?search=a".to_string(),
        "/api/governance/bodies".to_string(),
        format!("/api/governance/bodies/{body_id}"),
        "/api/governance/decisions".to_string(),
        format!("/api/governance/decisions/{decision_id}"),
        "/api/governance/persons?search=a".to_string(),
    ] {
        let (status, _) = request_json(&test, Method::GET, &path, &pcookie, None, None, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    // Every mutation surface denies — nothing is written.
    for (path, body) in [
        (
            "/api/social-aid/funds".to_string(),
            json!({ "name": "X", "idempotencyKey": "f-deny" }),
        ),
        (format!("/api/social-aid/funds/{fund_id}/close"), json!({})),
        (
            format!("/api/social-aid/funds/{fund_id}/cancel"),
            json!({ "reason": "iptal" }),
        ),
        (
            "/api/social-aid/donations".to_string(),
            donation_body(&fund_id, Uuid::new_v4(), "10.00", "don-deny"),
        ),
        (
            format!("/api/social-aid/donations/{fresh}/reverse"),
            json!({ "reason": "geri" }),
        ),
        (
            "/api/social-aid/disbursements".to_string(),
            disbursement_body(&fund_id, Uuid::new_v4(), "10.00", "dis-deny"),
        ),
        (
            format!("/api/social-aid/disbursements/{fresh}/reverse"),
            json!({ "reason": "geri" }),
        ),
        (
            "/api/governance/bodies".to_string(),
            json!({ "name": "X", "bodyType": "Denetim", "idempotencyKey": "b-deny" }),
        ),
        (format!("/api/governance/bodies/{body_id}/close"), json!({})),
        (
            format!("/api/governance/bodies/{body_id}/memberships"),
            membership_body(Uuid::new_v4(), "m-deny"),
        ),
        (
            format!("/api/governance/memberships/{fresh}/end"),
            json!({ "endedAt": "2025-12-01T00:00:00Z", "reason": "ayrılık" }),
        ),
        (
            "/api/governance/decisions".to_string(),
            decision_body(&body_id, "X", "d-deny"),
        ),
        (
            format!("/api/governance/decisions/{decision_id}/update"),
            json!({ "title": "Y", "decisionText": "yeni metin", "decisionOn": "2025-12-21" }),
        ),
        (
            format!("/api/governance/decisions/{decision_id}/open"),
            json!({}),
        ),
        (
            format!("/api/governance/decisions/{decision_id}/cancel"),
            json!({ "reason": "geri" }),
        ),
        (
            format!("/api/governance/decisions/{decision_id}/votes"),
            vote_body(Uuid::new_v4(), "approve", "v-deny"),
        ),
        (
            format!("/api/governance/decisions/{decision_id}/finalize"),
            json!({ "outcome": "approved" }),
        ),
    ] {
        let (status, _) = request_json(
            &test,
            Method::POST,
            &path,
            &pcookie,
            Some(&pcsrf),
            None,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    // Fixture rows only — the denied mutations added nothing.
    assert_eq!(count(&test.pool, "social_aid_funds").await, 1);
    assert_eq!(count(&test.pool, "social_aid_donations").await, 0);
    assert_eq!(count(&test.pool, "social_aid_disbursements").await, 0);
    assert_eq!(count(&test.pool, "governance_bodies").await, 1);
    assert_eq!(count(&test.pool, "governance_memberships").await, 0);
    assert_eq!(count(&test.pool, "governance_decisions").await, 1);
    assert_eq!(count(&test.pool, "governance_votes").await, 0);
    assert_eq!(count(&test.pool, "account_movements").await, 0);

    test.cleanup().await;
}

#[tokio::test]
async fn mutations_still_require_both_csrf_layers() {
    let Some(test) = setup().await else { return };
    bootstrap_operator(&test.pool, "csrf_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "csrf_op").await;

    // No synchronizer token → CSRF layer rejects before authorization.
    for (path, body) in [
        (
            "/api/social-aid/funds",
            json!({ "name": "X", "idempotencyKey": "f-csrf" }),
        ),
        (
            "/api/governance/bodies",
            json!({ "name": "X", "bodyType": "Denetim", "idempotencyKey": "b-csrf" }),
        ),
    ] {
        let (status, _) =
            request_json(&test, Method::POST, path, &cookie, None, None, Some(body)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    // Wrong token → same.
    let (status, _) = request_json(
        &test,
        Method::POST,
        "/api/social-aid/funds",
        &cookie,
        Some("bogus-token"),
        None,
        Some(json!({ "name": "X", "idempotencyKey": "f-csrf2" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Correct token → succeeds (proves the prologue ordering).
    let fund = make_fund(&test, &cookie, &csrf, "Kalkan Fon", "f-csrf-ok").await;
    assert_eq!(fund["status"], "active");
    let body = make_body(&test, &cookie, &csrf, "Kurul", "b-csrf-ok").await;
    assert_eq!(body["status"], "active");

    test.cleanup().await;
}

#[tokio::test]
async fn mutations_record_cooperative_scoped_audit() {
    let Some(test) = setup().await else { return };
    let (_, coop) = bootstrap_operator(&test.pool, "audit_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "audit_op").await;
    let member = create_person(&test.pool, "Üye", "Bir").await;
    let account = make_account(&test, &cookie, &csrf, "Kasa").await;

    let fund = make_fund(&test, &cookie, &csrf, "Denetim Fonu", "f-audit").await;
    let fund_id = fund["id"].as_str().unwrap().to_string();
    let (status, dbody) = request_json(
        &test,
        Method::POST,
        "/api/social-aid/donations",
        &cookie,
        Some(&csrf),
        None,
        Some(donation_body(&fund_id, account, "100.00", "don-audit")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{dbody}");

    let body = make_body(&test, &cookie, &csrf, "Denetim Kurulu", "b-audit").await;
    let body_id = body["id"].as_str().unwrap().to_string();
    let (status, mbody) = request_json(
        &test,
        Method::POST,
        &format!("/api/governance/bodies/{body_id}/memberships"),
        &cookie,
        Some(&csrf),
        None,
        Some(membership_body(member, "m-audit")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{mbody}");
    let (status, dres) = request_json(
        &test,
        Method::POST,
        "/api/governance/decisions",
        &cookie,
        Some(&csrf),
        None,
        Some(decision_body(&body_id, "Gündem", "d-audit")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{dres}");
    let decision_id = serde_json::from_str::<Value>(&dres).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/governance/decisions/{decision_id}/open"),
        &cookie,
        Some(&csrf),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, vres) = request_json(
        &test,
        Method::POST,
        &format!("/api/governance/decisions/{decision_id}/votes"),
        &cookie,
        Some(&csrf),
        None,
        Some(vote_body(member, "approve", "v-audit")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{vres}");

    for event in [
        "social_aid_fund_created",
        "social_aid_donation_posted",
        "governance_body_created",
        "governance_membership_started",
        "governance_decision_created",
        "governance_decision_opened",
        "governance_vote_recorded",
    ] {
        let (scoped,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM security_events \
             WHERE cooperative_id = $1 AND event_type = $2",
        )
        .bind(coop)
        .bind(event)
        .fetch_one(&test.pool)
        .await
        .unwrap();
        assert_eq!(scoped, 1, "{event}");
    }
    // Nothing recorded in the NULL-attributed lane.
    let (null_lane,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM security_events \
         WHERE cooperative_id IS NULL AND event_type IN \
         ('social_aid_fund_created','social_aid_donation_posted',\
          'governance_body_created','governance_membership_started',\
          'governance_decision_created','governance_decision_opened',\
          'governance_vote_recorded')",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(null_lane, 0);

    test.cleanup().await;
}

/// Social-aid restricted-fund parity under scoped auth: donation = 1
/// inflow, disbursement = 1 outflow, dual availability guards, the
/// unrestricted-funds reservation protects restricted cash from
/// ordinary cooperative outflows, reversals restore state, close is
/// guarded by non-zero restricted availability.
#[tokio::test]
async fn scoped_social_aid_financial_flow_parity() {
    let Some(test) = setup().await else { return };
    bootstrap_operator(&test.pool, "fin_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "fin_op").await;
    let account = make_account(&test, &cookie, &csrf, "Kasa").await;
    let account_b = make_account(&test, &cookie, &csrf, "Banka").await;
    fund_account(&test, &cookie, &csrf, account, "500.00", "fa-1").await;
    let donor = create_person(&test.pool, "Bağışçı", "Kişi").await;
    let fund = make_fund(&test, &cookie, &csrf, "Gıda Fonu", "f-1").await;
    let fund_id = fund["id"].as_str().unwrap().to_string();

    // Donation: exactly ONE inflow; a canonical Person link is accepted.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/social-aid/donations",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({
            "fundId": fund_id,
            "donorPersonId": donor.to_string(),
            "financialAccountId": account.to_string(),
            "amount": "300.00",
            "occurredAt": "2025-12-15T10:00:00Z",
            "idempotencyKey": "don-1",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let donation_id = serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(movements_for(&test, "social_aid_donation").await, 1);

    // PILOT-FIX-001 reservation: an ordinary expense may consume only
    // the UNRESTRICTED 500 — never the restricted 300.
    let (status, cats) = request_json(
        &test,
        Method::GET,
        "/api/financial-categories/options?categoryType=expense",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cats}");
    let category = serde_json::from_str::<Value>(&cats)
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Diğer Gider")
        .expect("expense category")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/expenses",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({
            "financialAccountId": account.to_string(),
            "categoryId": category,
            "amount": "600.00",
            "description": "sıradan gider",
            "idempotencyKey": "exp-1",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // Disbursement: exactly ONE outflow — gated on BOTH restricted and
    // physical availability of THIS account.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/social-aid/disbursements",
        &cookie,
        Some(&csrf),
        None,
        Some(disbursement_body(&fund_id, account, "100.00", "dis-1")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let disb_id = serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(movements_for(&test, "social_aid_disbursement").await, 1);

    // Restricted money in account A never funds a disbursement from B.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/social-aid/disbursements",
        &cookie,
        Some(&csrf),
        None,
        Some(disbursement_body(&fund_id, account_b, "10.00", "dis-b")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    // …nor does a disbursement exceed the remaining restricted 200.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/social-aid/disbursements",
        &cookie,
        Some(&csrf),
        None,
        Some(disbursement_body(&fund_id, account, "250.00", "dis-over")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // Donation reversal refused while the restricted remainder (200)
    // no longer covers it — spent restricted money is never
    // invalidated.
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/social-aid/donations/{donation_id}/reverse"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reason": "yanlış kayıt" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // Disbursement reversal restores the restricted availability and
    // flips the movement — the row is preserved.
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/social-aid/disbursements/{disb_id}/reverse"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reason": "iade" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(posted_movements(&test, "social_aid_disbursement").await, 0);
    let (status, _) = sqlx::query_scalar::<_, String>(
        "SELECT status FROM social_aid_disbursements WHERE id = $1",
    )
    .bind(disb_id.parse::<Uuid>().unwrap())
    .fetch_one(&test.pool)
    .await
    .map(|s| (StatusCode::OK, s))
    .expect("disbursement row");
    let _ = status;

    // Fund close refused while restricted availability is non-zero.
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/social-aid/funds/{fund_id}/close"),
        &cookie,
        Some(&csrf),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // Now the donation reversal succeeds — restricted covers it again.
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/social-aid/donations/{donation_id}/reverse"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reason": "yanlış kayıt" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(posted_movements(&test, "social_aid_donation").await, 0);

    // Close now succeeds — every (fund, account) restricted is zero.
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/social-aid/funds/{fund_id}/close"),
        &cookie,
        Some(&csrf),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["status"],
        "closed"
    );

    // Reversal endpoints replay safely (200, no second mutation).
    let (status, body) = request_json(
        &test,
        Method::POST,
        &format!("/api/social-aid/donations/{donation_id}/reverse"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reason": "tekrar" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let detail = serde_json::from_str::<Value>(&body).unwrap();
    assert_eq!(detail["status"], "reversed");

    // Boundary: no social-aid command ever wrote a cooperative-finance
    // row — the only movements are the income fixture plus the two
    // reversed social-aid ones.
    assert_eq!(count(&test.pool, "payments").await, 0);
    assert_eq!(count(&test.pool, "income_entries").await, 1);
    let (total,): (i64,) = sqlx::query_as("SELECT count(*) FROM account_movements")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(total, 3); // income + donation + disbursement

    test.cleanup().await;
}

/// Governance lifecycle parity under scoped auth: body → memberships
/// → decision draft → open → votes → finalize snapshot; draft-only
/// edit/cancel; eligibility and one-vote-per-member enforced; close is
/// guarded by active memberships — zero account movements throughout.
#[tokio::test]
async fn scoped_governance_lifecycle_parity() {
    let Some(test) = setup().await else { return };
    bootstrap_operator(&test.pool, "gov_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "gov_op").await;
    let member_a = create_person(&test.pool, "Ayşe", "Yılmaz").await;
    let member_b = create_person(&test.pool, "Mehmet", "Demir").await;
    let outsider = create_person(&test.pool, "Dışarı", "Kişi").await;

    let body = make_body(&test, &cookie, &csrf, "Yönetim Kurulu", "b-1").await;
    let body_id = body["id"].as_str().unwrap().to_string();
    for (person, key) in [(member_a, "m-1"), (member_b, "m-2")] {
        let (status, res) = request_json(
            &test,
            Method::POST,
            &format!("/api/governance/bodies/{body_id}/memberships"),
            &cookie,
            Some(&csrf),
            None,
            Some(membership_body(person, key)),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{res}");
    }

    let (status, res) = request_json(
        &test,
        Method::POST,
        "/api/governance/decisions",
        &cookie,
        Some(&csrf),
        None,
        Some(decision_body(&body_id, "Olağan Gündem", "d-1")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{res}");
    let decision = serde_json::from_str::<Value>(&res).unwrap();
    let decision_id = decision["id"].as_str().unwrap().to_string();
    assert_eq!(decision["status"], "draft");

    // Votes are impossible while the decision is still a draft.
    let (status, res) = request_json(
        &test,
        Method::POST,
        &format!("/api/governance/decisions/{decision_id}/votes"),
        &cookie,
        Some(&csrf),
        None,
        Some(vote_body(member_a, "approve", "v-early")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");

    // Draft edit allowed; open flips the decision for voting.
    let (status, res) = request_json(
        &test,
        Method::POST,
        &format!("/api/governance/decisions/{decision_id}/update"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({
            "title": "Olağan Gündem (revize)",
            "decisionText": "Revize karar metni.",
            "decisionOn": "2025-12-21",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (status, res) = request_json(
        &test,
        Method::POST,
        &format!("/api/governance/decisions/{decision_id}/open"),
        &cookie,
        Some(&csrf),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    // Opened text is frozen — no silent rewrite while members vote.
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/governance/decisions/{decision_id}/update"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({
            "title": "X",
            "decisionText": "yeni",
            "decisionOn": "2025-12-22",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // And only drafts can be cancelled.
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/governance/decisions/{decision_id}/cancel"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "reason": "geri" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Eligible members vote; the seat proves eligibility, not the
    // login actor.
    for (person, choice, key) in [(member_a, "approve", "v-1"), (member_b, "abstain", "v-2")] {
        let (status, res) = request_json(
            &test,
            Method::POST,
            &format!("/api/governance/decisions/{decision_id}/votes"),
            &cookie,
            Some(&csrf),
            None,
            Some(vote_body(person, choice, key)),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{res}");
    }
    // One vote per member — a second choice is a conflict.
    let (status, res) = request_json(
        &test,
        Method::POST,
        &format!("/api/governance/decisions/{decision_id}/votes"),
        &cookie,
        Some(&csrf),
        None,
        Some(vote_body(member_a, "reject", "v-dup")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{res}");
    // No active seat → permission-flavored denial.
    let (status, res) = request_json(
        &test,
        Method::POST,
        &format!("/api/governance/decisions/{decision_id}/votes"),
        &cookie,
        Some(&csrf),
        None,
        Some(vote_body(outsider, "approve", "v-out")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{res}");

    // Body close is blocked by the active memberships.
    let (status, res) = request_json(
        &test,
        Method::POST,
        &format!("/api/governance/bodies/{body_id}/close"),
        &cookie,
        Some(&csrf),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{res}");

    // Finalize records the operator-stated outcome and freezes the
    // electorate/tally snapshot — never computed quorum arithmetic.
    let (status, res) = request_json(
        &test,
        Method::POST,
        &format!("/api/governance/decisions/{decision_id}/finalize"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "outcome": "approved" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let detail = serde_json::from_str::<Value>(&res).unwrap();
    assert_eq!(detail["status"], "approved");
    assert_eq!(detail["eligibleCount"], 2);
    assert_eq!(detail["approveCount"], 1);
    assert_eq!(detail["rejectCount"], 0);
    assert_eq!(detail["abstainCount"], 1);
    assert_eq!(detail["votes"].as_array().unwrap().len(), 2);
    // recordedBy is the operator user, never conflated with the voter.
    let operator: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE username = 'gov_op'")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    for vote in detail["votes"].as_array().unwrap() {
        assert_eq!(vote["recordedBy"].as_str().unwrap(), operator.to_string());
    }

    // A finalized decision accepts no more votes and no second
    // finalization.
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/governance/decisions/{decision_id}/votes"),
        &cookie,
        Some(&csrf),
        None,
        Some(vote_body(outsider, "approve", "v-late")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/governance/decisions/{decision_id}/finalize"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "outcome": "rejected" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // End both memberships, then the body closes.
    let membership_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM governance_memberships WHERE body_id = $1 AND ended_at IS NULL",
    )
    .bind(body_id.parse::<Uuid>().unwrap())
    .fetch_all(&test.pool)
    .await
    .unwrap();
    for membership in membership_ids {
        let (status, res) = request_json(
            &test,
            Method::POST,
            &format!("/api/governance/memberships/{membership}/end"),
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "endedAt": "2025-12-01T00:00:00Z", "reason": "görev süresi" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{res}");
    }
    let (status, res) = request_json(
        &test,
        Method::POST,
        &format!("/api/governance/bodies/{body_id}/close"),
        &cookie,
        Some(&csrf),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(
        serde_json::from_str::<Value>(&res).unwrap()["status"],
        "closed"
    );

    // The whole lifecycle is evidence-only — zero account movements.
    assert_eq!(count(&test.pool, "account_movements").await, 0);

    test.cleanup().await;
}

/// Idempotent replay + concurrent same-key posts on both routers:
/// one logical write each, conflicting fingerprint conflicts.
#[tokio::test]
async fn replay_and_concurrent_same_key_single_write() {
    let Some(test) = setup().await else { return };
    bootstrap_operator(&test.pool, "idem_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "idem_op").await;
    let member = create_person(&test.pool, "Üye", "Bir").await;
    let account = make_account(&test, &cookie, &csrf, "Kasa").await;
    let fund = make_fund(&test, &cookie, &csrf, "Fon", "f-1").await;
    let fund_id = fund["id"].as_str().unwrap().to_string();

    // Donation replay: same key + same payload → 200, one row, one
    // movement.
    let (status, first) = request_json(
        &test,
        Method::POST,
        "/api/social-aid/donations",
        &cookie,
        Some(&csrf),
        None,
        Some(donation_body(&fund_id, account, "50.00", "don-r")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let first_id = serde_json::from_str::<Value>(&first).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, replay) = request_json(
        &test,
        Method::POST,
        "/api/social-aid/donations",
        &cookie,
        Some(&csrf),
        None,
        Some(donation_body(&fund_id, account, "50.00", "don-r")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(
        serde_json::from_str::<Value>(&replay).unwrap()["id"]
            .as_str()
            .unwrap(),
        first_id
    );
    assert_eq!(count(&test.pool, "social_aid_donations").await, 1);
    assert_eq!(movements_for(&test, "social_aid_donation").await, 1);
    // Same key + different payload → 409.
    let (status, _) = request_json(
        &test,
        Method::POST,
        "/api/social-aid/donations",
        &cookie,
        Some(&csrf),
        None,
        Some(donation_body(&fund_id, account, "60.00", "don-r")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(count(&test.pool, "social_aid_donations").await, 1);

    // Concurrent same-key disbursement: one logical write.
    fund_account(&test, &cookie, &csrf, account, "500.00", "fa-c").await;
    let (a, b) = tokio::join!(
        request_json(
            &test,
            Method::POST,
            "/api/social-aid/disbursements",
            &cookie,
            Some(&csrf),
            None,
            Some(disbursement_body(&fund_id, account, "20.00", "dis-c")),
        ),
        request_json(
            &test,
            Method::POST,
            "/api/social-aid/disbursements",
            &cookie,
            Some(&csrf),
            None,
            Some(disbursement_body(&fund_id, account, "20.00", "dis-c")),
        )
    );
    for (status, body) in [&a, &b] {
        assert!(
            *status == StatusCode::CREATED || *status == StatusCode::OK,
            "{status}: {body}"
        );
    }
    assert_eq!(count(&test.pool, "social_aid_disbursements").await, 1);
    assert_eq!(movements_for(&test, "social_aid_disbursement").await, 1);

    // Governance vote: same key + same payload replays; concurrent
    // same-key votes write one row.
    let body = make_body(&test, &cookie, &csrf, "Kurul", "b-1").await;
    let body_id = body["id"].as_str().unwrap().to_string();
    let (status, res) = request_json(
        &test,
        Method::POST,
        &format!("/api/governance/bodies/{body_id}/memberships"),
        &cookie,
        Some(&csrf),
        None,
        Some(membership_body(member, "m-1")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{res}");
    let (status, res) = request_json(
        &test,
        Method::POST,
        "/api/governance/decisions",
        &cookie,
        Some(&csrf),
        None,
        Some(decision_body(&body_id, "Gündem", "d-1")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{res}");
    let decision_id = serde_json::from_str::<Value>(&res).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/governance/decisions/{decision_id}/open"),
        &cookie,
        Some(&csrf),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let votes_path = format!("/api/governance/decisions/{decision_id}/votes");
    let (a, b) = tokio::join!(
        request_json(
            &test,
            Method::POST,
            &votes_path,
            &cookie,
            Some(&csrf),
            None,
            Some(vote_body(member, "approve", "v-c")),
        ),
        request_json(
            &test,
            Method::POST,
            &votes_path,
            &cookie,
            Some(&csrf),
            None,
            Some(vote_body(member, "approve", "v-c")),
        )
    );
    for (status, body) in [&a, &b] {
        assert!(
            *status == StatusCode::CREATED || *status == StatusCode::OK,
            "{status}: {body}"
        );
    }
    assert_eq!(count(&test.pool, "governance_votes").await, 1);
    // Same key + different payload → 409.
    let (status, _) = request_json(
        &test,
        Method::POST,
        &format!("/api/governance/decisions/{decision_id}/votes"),
        &cookie,
        Some(&csrf),
        None,
        Some(vote_body(member, "reject", "v-c")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(count(&test.pool, "governance_votes").await, 1);

    test.cleanup().await;
}

#[tokio::test]
async fn never_bootstrapped_lane_keeps_legacy_semantics() {
    let Some(test) = setup().await else { return };
    // No bootstrap at all — the global lane is untouched.
    let legacy = create_user(&test.pool, "kuresel").await;
    grant_role(&test.pool, legacy, "global-op", None, OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "kuresel").await;

    for path in ["/api/social-aid/funds", "/api/governance/bodies"] {
        let (status, body) =
            request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }
    let fund = make_fund(&test, &cookie, &csrf, "Eski Fon", "f-g").await;
    assert_eq!(fund["status"], "active");
    let body = make_body(&test, &cookie, &csrf, "Eski Kurul", "b-g").await;
    assert_eq!(body["status"], "active");

    test.cleanup().await;
}
