//! M1-P8-B Batch 3 — security test-quality hardening.
//!
//! Batch 2 pinned the lane-level contract (which authorization lane a
//! route belongs to, session-authoritative cooperative context). Batch
//! 3 locks the FINE-GRAINED shape contracts that Batch 1/2 left open:
//!
//!   1. STATUS LOCKING: resource-level denial is 404 `not_found`;
//!      permission-level denial is 403 `permission_denied`; assertion
//!      denial is 403 `cooperative_access_denied`; malformed input is
//!      400 `validation_failed`. The lanes never blur.
//!   2. EXISTENCE ORACLE RESISTANCE on the share-return surface: a
//!      foreign-tenant id and a never-existed id produce BYTE-IDENTICAL
//!      responses — status, error code and body — so an attacker cannot
//!      distinguish "exists in another cooperative" from "does not
//!      exist". Mutations on those ids are equally identical and write
//!      NOTHING in either cooperative.
//!   3. PERMISSION LANE: the permission check precedes any existence
//!      lookup, so a member WITHOUT `share_returns.*` gets the same
//!      403 for existing, foreign and nonexistent ids — the denial
//!      lane itself leaks nothing.
//!   4. PARSED-NUMERIC rejection: hostile numeric representations in
//!      bodies (amount strings), query params (page/page_size) and
//!      path segments (non-UUID ids) all fail closed through the
//!      sanitized `{error:{code}}` contract — never panic, never
//!      silently coerce into a write.
//!   5. ERROR-RESPONSE HYGIENE: every error body is exactly
//!      `{"error":{"code":<stable code>}}` — no message, detail,
//!      stack, SQL or reflection of attacker-controlled input.
//!   6. AUDIT LANE: scoped audit reads cannot enumerate foreign
//!      cooperatives (`?cooperativeId=<foreign|random>` deny
//!      identically) and scoped results never contain foreign rows.
//!
//! Every hostile probe carries a SAME-ENDPOINT positive control so a
//! pass can never be produced by a blanket "always reject" regression.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database;
//! scratch cooperatives only).
//!
//! DOCUMENTED NON-EXPLOITABLE DEVIATIONS (Batch 2 carry-over, no
//! behavior change in this batch):
//!   - Duplicate `x-cooperative-id` fields: ANY second value now fails
//!     closed with 400 (contract hardening, `fix/tenant-header-dupes`).
//!     Previously `HeaderMap::get` read the first value only.
//!   - `page <= 0` query values CLAMP to page 1 rather than erroring
//!     (documented tolerance lane below); `page_size` outside
//!     1..=100 is a hard `validation_failed`.
//!   - Financial amounts use a canonical-only wire grammar (contract
//!     hardening P2): ASCII digits, optional '.', 1-2 fraction digits.
//!     Previously tolerated forms (scientific notation, `+`, `_`
//!     separators, surrounding whitespace) now fail closed.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
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

const TEST_PASSWORD: &str = "p8b-b3-parola-123";
const ORIGIN: &str = "http://localhost:5173";
const APP_ROLE: &str = "kooperatif_app";

/// Sentinel that must NEVER appear in any response body — if an error
/// path echoed attacker input, this string would show up.
const SENTINEL: &str = "B3PROBE-ZQ7X-NOREFLECT";

fn app_password() -> String {
    app_db::test_app_role_password()
}

fn test_database_url() -> String {
    let value = std::env::var("KOOPERATIF_TEST_DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "KOOPERATIF_TEST_DATABASE_URL is not set — required DB-gated \
             integration tests cannot silently pass"
        )
    });
    let trimmed = value.trim().to_string();
    assert!(!trimmed.is_empty(), "KOOPERATIF_TEST_DATABASE_URL is empty");
    trimmed
}

struct TestApp {
    _claim: app_db::TestDbClaim,
    app: axum::Router,
    pool: PgPool,
    peer: SocketAddr,
    database_name: String,
    admin_url: String,
}

fn app_url(base_db_url: &str) -> String {
    let (scheme, rest) = base_db_url.split_once("://").expect("url scheme");
    let at = rest.find('@').expect("userinfo in url");
    format!("{scheme}://{APP_ROLE}:{}{}", app_password(), &rest[at..])
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
            app_db::assert_disposable_test_database_name(&self.database_name);
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

async fn setup() -> TestApp {
    let url = test_database_url();
    let suffix = Uuid::new_v4().simple();
    let database_name = format!("kooperatif_test_{suffix}");
    let (base_url, admin_url) = url
        .rsplit_once('/')
        .map(|(base, _)| (base.to_string(), format!("{base}/postgres")))
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
    app_db::run_test_migrations(&pool)
        .await
        .expect("migrations applied");
    repo::bootstrap_initial_cooperative(&pool, "Ana Kooperatif")
        .await
        .expect("bootstrap cooperative");
    let clock = Arc::new(MutableClock::new(
        OffsetDateTime::parse(
            "2026-01-15T00:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .expect("fixed epoch"),
    ));
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
    let runtime = AuthRuntime::new(config, clock);
    let app_pool = app_db::connect(&app_url(&format!("{base_url}/{database_name}")))
        .await
        .expect("kooperatif_app pool");
    let app = router(
        AppState {
            db: Some(app_pool.clone()),
            auth: Arc::new(runtime),
        },
        cors_layer(&[]),
    );
    let bytes = Uuid::new_v4();
    let octets = bytes.as_bytes();
    TestApp {
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
    }
}

async fn create_user(pool: &PgPool, username: &str) -> Uuid {
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).expect("params"),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).expect("hash");
    users::create_user(pool, username, "P8B B3", &hash, TEST_PASSWORD)
        .await
        .expect("user")
        .id
}

/// Membership + ad-hoc role carrying exactly `perms` inside `coop`.
async fn grant_in_cooperative(pool: &PgPool, coop: Uuid, user: Uuid, perms: &[&str]) {
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id) \
         VALUES ($1, $2) ON CONFLICT (cooperative_id, user_id) \
         DO UPDATE SET status = 'active', ended_at = NULL",
    )
    .bind(coop)
    .bind(user)
    .execute(pool)
    .await
    .unwrap();
    let role: Uuid =
        sqlx::query_scalar("INSERT INTO roles (name, cooperative_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("b3-{user}-{coop}"))
            .bind(coop)
            .fetch_one(pool)
            .await
            .unwrap();
    for key in perms {
        sqlx::query(
            "INSERT INTO role_permissions (role_id, permission_id) \
             SELECT $1, id FROM permissions WHERE key = $2",
        )
        .bind(role)
        .bind(key)
        .execute(pool)
        .await
        .unwrap();
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
    .unwrap();
}

/// Fixture administrator for the BOOTSTRAP cooperative: membership +
/// the legacy NULL-scoped `Sistem Yöneticisi` role (it resolves in
/// bootstrap scope only — never platform authority; see 0072 §2).
/// Used ONLY to build fixtures and run positive controls.
async fn grant_admin_in_bootstrap(pool: &PgPool, user: Uuid) {
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         SELECT id, $1, 'active' FROM cooperatives WHERE is_bootstrap",
    )
    .bind(user)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id) \
         SELECT $1, id FROM roles WHERE name = 'Sistem Yöneticisi'",
    )
    .bind(user)
    .execute(pool)
    .await
    .unwrap();
}

async fn bootstrap_coop(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("SELECT id FROM cooperatives WHERE is_bootstrap")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn login(app: &axum::Router, peer: SocketAddr, username: &str) -> (String, String) {
    let mut request = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .header("user-agent", "Mozilla/5.0 (Windows NT 10.0) Chrome/126.0")
        .body(Body::from(
            json!({ "username": username, "password": TEST_PASSWORD }).to_string(),
        ))
        .expect("valid request");
    request.extensions_mut().insert(ConnectInfo(peer));
    let response = app.clone().oneshot(request).await.expect("infallible");
    assert_eq!(response.status(), StatusCode::OK);
    let set_cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .expect("cookie")
        .to_string();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body: Value = serde_json::from_slice(&bytes).expect("json");
    let cookie = set_cookie
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .map(|(_, v)| v.to_string())
        .expect("token");
    (
        cookie,
        body["csrfToken"].as_str().expect("csrf").to_string(),
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
        .header("cookie", format!("kooperatif_session={cookie}"))
        .header("origin", ORIGIN);
    if let Some(csrf) = csrf {
        builder = builder.header("x-csrf-token", csrf);
    }
    match body {
        Some(payload) => builder
            .header("content-type", "application/json")
            .body(Body::from(payload.to_string()))
            .expect("valid"),
        None => builder.body(Body::empty()).expect("valid"),
    }
}

/// (status, raw body bytes) — byte comparison is the oracle test:
/// identical BYTES, not merely identical codes.
async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, Vec<u8>) {
    let response = app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, bytes.to_vec())
}

fn json_body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

/// The error contract: exactly `{"error":{"code":<string>}}` — nothing
/// else may cross the boundary, ever.
fn assert_error_contract(bytes: &[u8], context: &str) -> String {
    let body = json_body(bytes);
    let error = body
        .as_object()
        .filter(|o| o.len() == 1)
        .and_then(|_| body.get("error"))
        .and_then(|e| e.as_object())
        .unwrap_or_else(|| panic!("{context}: error body is not the contract shape: {body}"));
    assert_eq!(
        error.len(),
        1,
        "{context}: error object carries extra fields: {body}"
    );
    let code = error["code"]
        .as_str()
        .unwrap_or_else(|| panic!("{context}: missing error.code: {body}"))
        .to_string();
    // Attacker-controlled input is never reflected back.
    let text = String::from_utf8_lossy(bytes);
    assert!(
        !text.contains(SENTINEL),
        "{context}: response reflected the sentinel: {text}"
    );
    for banned in [
        "SELECT ",
        "INSERT ",
        "relation \"",
        "column \"",
        "pg_",
        "sqlx",
        "panic",
    ] {
        assert!(
            !text.to_lowercase().contains(&banned.to_lowercase()),
            "{context}: response leaked internal detail {banned:?}: {text}"
        );
    }
    code
}

// ═════════════════════════════════════════════════════════════════════
// World fixture: two real cooperatives + a probe member of A only.
// ═════════════════════════════════════════════════════════════════════

struct World {
    coop_a: Uuid,
    coop_b: Uuid,
    /// System-admin session in A — fixture building + positive control.
    admin_a: (String, String),
    /// Initial-admin session in B — fixture building only.
    admin_b: (String, String),
    /// Member of A with share_returns.read+manage — the probe identity.
    probe: (String, String),
    /// Member of A with only reports.read — permission-lane probe.
    restricted: (String, String),
}

async fn world(test: &TestApp, tag: &str) -> World {
    let coop_a = bootstrap_coop(&test.pool).await;

    // Platform admin provisions a real, active second cooperative.
    let padmin = create_user(&test.pool, &format!("p8b-b3-pa-{tag}")).await;
    let plat_role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, cooperative_id) VALUES ($1, NULL) RETURNING id",
    )
    .bind(format!("b3-plat-{tag}"))
    .fetch_one(&test.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission_id) \
         SELECT $1, id FROM permissions WHERE key = 'cooperatives.manage'",
    )
    .bind(plat_role)
    .execute(&test.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
         VALUES ($1, $2, NULL)",
    )
    .bind(padmin)
    .bind(plat_role)
    .execute(&test.pool)
    .await
    .unwrap();
    let (pcookie, pcsrf) = login(&test.app, test.peer, &format!("p8b-b3-pa-{tag}")).await;
    let (status, bytes) = send(
        &test.app,
        req(
            "POST",
            "/api/platform/cooperatives",
            &pcookie,
            Some(&pcsrf),
            Some(json!({ "name": format!("B3 Kooperatif {tag}") })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "provision B: {:?}", bytes);
    let coop_b: Uuid = json_body(&bytes)["id"].as_str().unwrap().parse().unwrap();
    let admin_b_user = create_user(&test.pool, &format!("p8b-b3-ab-{tag}")).await;
    let (status, bytes) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/platform/cooperatives/{coop_b}/activate"),
            &pcookie,
            Some(&pcsrf),
            Some(json!({ "initialAdminUserId": admin_b_user })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "activate B: {:?}", bytes);

    let admin_a_user = create_user(&test.pool, &format!("p8b-b3-aa-{tag}")).await;
    grant_admin_in_bootstrap(&test.pool, admin_a_user).await;
    let probe_user = create_user(&test.pool, &format!("p8b-b3-pr-{tag}")).await;
    grant_in_cooperative(
        &test.pool,
        coop_a,
        probe_user,
        &["share_returns.read", "share_returns.manage"],
    )
    .await;
    let restricted_user = create_user(&test.pool, &format!("p8b-b3-rs-{tag}")).await;
    grant_in_cooperative(&test.pool, coop_a, restricted_user, &["reports.read"]).await;

    let admin_a = login(&test.app, test.peer, &format!("p8b-b3-aa-{tag}")).await;
    let admin_b = login(&test.app, test.peer, &format!("p8b-b3-ab-{tag}")).await;
    // admin_b's only membership is B, but a fresh session's scope is the
    // bootstrap fallback (A) — an explicit audited switch is required.
    let (status, bytes) = send(
        &test.app,
        req(
            "POST",
            "/api/auth/cooperative",
            &admin_b.0,
            Some(&admin_b.1),
            Some(json!({ "cooperativeId": coop_b })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "scope→B: {:?}", bytes);

    World {
        coop_a,
        coop_b,
        admin_a,
        admin_b,
        probe: login(&test.app, test.peer, &format!("p8b-b3-pr-{tag}")).await,
        restricted: login(&test.app, test.peer, &format!("p8b-b3-rs-{tag}")).await,
    }
}

/// Shareholder → share → initiated (pending) return inside the
/// session's own cooperative. Returns the return id + updatedAt.
async fn make_pending_return(
    test: &TestApp,
    session: &(String, String),
    tag: &str,
) -> (Uuid, String) {
    let (cookie, csrf) = session;
    let (status, bytes) = send(
        &test.app,
        req(
            "POST",
            "/api/shareholders",
            cookie,
            Some(csrf),
            Some(json!({
                "person": { "mode": "new", "firstName": "B3", "lastName": format!("Üye {tag}") },
                "family": { "mode": "new", "sequenceNumber": 1 }
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "shareholder: {:?}", bytes);
    let shareholder: Uuid = json_body(&bytes)["id"].as_str().unwrap().parse().unwrap();

    let (status, bytes) = send(
        &test.app,
        req(
            "POST",
            "/api/shares",
            cookie,
            Some(csrf),
            Some(json!({
                "shareholderId": shareholder,
                "acquisitionType": "founder",
                // Backdated so a same-day effective return is legal.
                "effectiveAt": "2025-01-01T00:00:00Z",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "share: {:?}", bytes);
    let share: Uuid = json_body(&bytes)["id"].as_str().unwrap().parse().unwrap();

    let (status, bytes) = send(
        &test.app,
        req(
            "POST",
            "/api/share-returns",
            cookie,
            Some(csrf),
            Some(json!({
                "shareId": share,
                "effectiveReturnDate": "2026-01-15",
                "reason": format!("üyelikten ayrılma {tag}"),
                "idempotencyKey": format!("b3-init-{tag}"),
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "initiate: {:?}", bytes);
    let body = json_body(&bytes);
    (
        body["id"].as_str().unwrap().parse().unwrap(),
        body["updatedAt"].as_str().unwrap().to_string(),
    )
}

/// Finalize a pending return in the session's coop with one determined
/// principal entitlement; returns the entitlement id.
async fn finalize_with_entitlement(
    test: &TestApp,
    session: &(String, String),
    return_id: Uuid,
    expected_updated_at: &str,
) -> Uuid {
    let (cookie, csrf) = session;
    let (status, bytes) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-returns/{return_id}/finalize"),
            cookie,
            Some(csrf),
            Some(json!({
                "entitlements": [{
                    "entitlementType": "principal",
                    "amount": "100.00",
                    "dueDate": "2026-03-01",
                    "policyReference": "YK-2026-03",
                    "description": "ana para iadesi"
                }],
                "expectedUpdatedAt": expected_updated_at,
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "finalize: {:?}", bytes);
    let body = json_body(&bytes);
    body["entitlements"][0]["id"]
        .as_str()
        .expect("entitlement id")
        .parse()
        .expect("uuid")
}

async fn table_count(pool: &PgPool, table: &str, coop: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>(&format!(
        "SELECT count(*) FROM {table} WHERE cooperative_id = $1"
    ))
    .bind(coop)
    .fetch_one(pool)
    .await
    .unwrap()
}

// ═════════════════════════════════════════════════════════════════════
// 1–2. Status locking + existence-oracle resistance on reads.
// ═════════════════════════════════════════════════════════════════════

/// READ lane: foreign real id vs never-existed id → byte-identical 404;
/// own id → 200 on the SAME endpoint (positive control).
#[tokio::test]
async fn share_return_detail_foreign_and_nonexistent_are_byte_identical() {
    let test = setup().await;
    let w = world(&test, "detail").await;
    let (ret_a, _) = make_pending_return(&test, &w.admin_a, "a").await;
    let (ret_b, _) = make_pending_return(&test, &w.admin_b, "b").await;
    let missing = Uuid::new_v4();

    // Positive control — same endpoint, own resource.
    let (status, bytes) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/share-returns/{ret_a}"),
            &w.probe.0,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "own detail: {:?}", bytes);
    assert_eq!(json_body(&bytes)["id"], json!(ret_a.to_string()));

    // Hostile: B's real return id vs a never-existing id.
    let foreign = send(
        &test.app,
        req(
            "GET",
            &format!("/api/share-returns/{ret_b}"),
            &w.probe.0,
            None,
            None,
        ),
    )
    .await;
    let nonexistent = send(
        &test.app,
        req(
            "GET",
            &format!("/api/share-returns/{missing}"),
            &w.probe.0,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(foreign.0, StatusCode::NOT_FOUND);
    assert_eq!(nonexistent.0, StatusCode::NOT_FOUND);
    assert_eq!(
        foreign.1, nonexistent.1,
        "foreign vs nonexistent response bytes differ — existence oracle"
    );
    let code = assert_error_contract(&foreign.1, "foreign detail");
    assert_eq!(code, "not_found");
    // The body must not leak the foreign id it was probed with.
    let text = String::from_utf8_lossy(&foreign.1);
    assert!(!text.contains(&ret_b.to_string()));
    test.cleanup().await;
}

/// MUTATION lane: finalize/cancel on foreign vs nonexistent ids are
/// byte-identical 404s and write NOTHING in either cooperative —
/// verified at the row level, not just by status.
#[tokio::test]
async fn share_return_mutations_foreign_and_nonexistent_identical_write_nothing() {
    let test = setup().await;
    let w = world(&test, "mut").await;
    let (ret_a, _) = make_pending_return(&test, &w.admin_a, "a").await;
    let (ret_b, _) = make_pending_return(&test, &w.admin_b, "b").await;
    let missing = Uuid::new_v4();

    let counts = |test: &TestApp, coop: Uuid| {
        let pool = test.pool.clone();
        async move {
            (
                table_count(&pool, "share_returns", coop).await,
                table_count(&pool, "share_return_entitlements", coop).await,
                table_count(&pool, "share_return_settlements", coop).await,
            )
        }
    };
    let before_a = counts(&test, w.coop_a).await;
    let before_b = counts(&test, w.coop_b).await;

    for verb in ["finalize", "cancel"] {
        let payload = if verb == "finalize" {
            json!({
                "entitlements": [{"entitlementType": "principal", "amount": "1.00"}],
                "expectedUpdatedAt": "2026-01-15T00:00:00Z",
            })
        } else {
            json!({ "reason": format!("{SENTINEL} cancel") })
        };
        let mut shapes = Vec::new();
        for target in [ret_b, missing] {
            let (status, bytes) = send(
                &test.app,
                req(
                    "POST",
                    &format!("/api/share-returns/{target}/{verb}"),
                    &w.probe.0,
                    Some(&w.probe.1),
                    Some(payload.clone()),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{verb} on {target}");
            shapes.push(bytes);
        }
        assert_eq!(
            shapes[0], shapes[1],
            "{verb}: foreign vs nonexistent bytes differ — oracle"
        );
        assert_eq!(assert_error_contract(&shapes[0], verb), "not_found");
    }

    // Row-level truth: nothing changed ANYWHERE; B's return is still pending.
    assert_eq!(counts(&test, w.coop_a).await, before_a, "A drifted");
    assert_eq!(counts(&test, w.coop_b).await, before_b, "B drifted");
    let b_status: String = sqlx::query_scalar("SELECT status FROM share_returns WHERE id = $1")
        .bind(ret_b)
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(b_status, "pending");

    // Positive control: the SAME mutation on the OWN pending return works.
    let (status, bytes) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-returns/{ret_a}/cancel"),
            &w.probe.0,
            Some(&w.probe.1),
            Some(json!({ "reason": "vazgeçildi" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "own cancel: {:?}", bytes);
    assert_eq!(json_body(&bytes)["status"], json!("cancelled"));
    test.cleanup().await;
}

// ═════════════════════════════════════════════════════════════════════
// 3. Permission lane: 403 fires before existence — no oracle there either.
// ═════════════════════════════════════════════════════════════════════

/// A member WITHOUT share_returns.* gets the same 403 for own, foreign
/// and nonexistent ids — the permission lane never becomes an
/// existence oracle. Positive control: the same session CAN read
/// reports (its actual grant).
#[tokio::test]
async fn permission_denied_lane_carries_no_existence_signal() {
    let test = setup().await;
    let w = world(&test, "perm").await;
    let (ret_a, _) = make_pending_return(&test, &w.admin_a, "a").await;
    let (ret_b, _) = make_pending_return(&test, &w.admin_b, "b").await;
    let missing = Uuid::new_v4();

    let mut shapes = Vec::new();
    for target in [ret_a, ret_b, missing] {
        let (status, bytes) = send(
            &test.app,
            req(
                "GET",
                &format!("/api/share-returns/{target}"),
                &w.restricted.0,
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "target {target}");
        shapes.push(bytes);
    }
    assert_eq!(shapes[0], shapes[1], "own vs foreign 403 differ");
    assert_eq!(shapes[0], shapes[2], "own vs nonexistent 403 differ");
    assert_eq!(
        assert_error_contract(&shapes[0], "permission lane"),
        "permission_denied"
    );

    // Mutations deny identically too.
    let mut mut_shapes = Vec::new();
    for target in [ret_a, missing] {
        let (status, bytes) = send(
            &test.app,
            req(
                "POST",
                &format!("/api/share-returns/{target}/cancel"),
                &w.restricted.0,
                Some(&w.restricted.1),
                Some(json!({ "reason": "x" })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        mut_shapes.push(bytes);
    }
    assert_eq!(mut_shapes[0], mut_shapes[1]);

    // Positive control — the restricted session's real grant still works.
    let (status, _) = send(
        &test.app,
        req("GET", "/api/reports/overview", &w.restricted.0, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "reports.read positive control");
    test.cleanup().await;
}

// ═════════════════════════════════════════════════════════════════════
// 4. Entitlement / settlement surfaces — same oracle contract.
// ═════════════════════════════════════════════════════════════════════

/// Entitlement commands on a foreign (real, open) entitlement vs a
/// never-existing id: byte-identical 404, zero writes, B state frozen.
#[tokio::test]
async fn entitlement_and_settlement_surfaces_deny_identically() {
    let test = setup().await;
    let w = world(&test, "ent").await;
    let (ret_b, ret_b_updated) = make_pending_return(&test, &w.admin_b, "b").await;
    let ent_b = finalize_with_entitlement(&test, &w.admin_b, ret_b, &ret_b_updated).await;
    let missing = Uuid::new_v4();

    let ents_before = table_count(&test.pool, "share_return_entitlements", w.coop_b).await;
    let setts_before = table_count(&test.pool, "share_return_settlements", w.coop_b).await;

    // determine / cancel on foreign vs nonexistent entitlement.
    for (verb, payload) in [
        (
            "determine",
            json!({ "amount": "10.00", "expectedUpdatedAt": "2026-01-15T00:00:00Z" }),
        ),
        ("cancel", json!({ "reason": format!("{SENTINEL} iptal") })),
        (
            "settlements",
            json!({
                "financialAccountId": Uuid::new_v4(),
                "amount": "1.00",
                "idempotencyKey": format!("b3-set-{SENTINEL}"),
            }),
        ),
    ] {
        let mut shapes = Vec::new();
        for target in [ent_b, missing] {
            let (status, bytes) = send(
                &test.app,
                req(
                    "POST",
                    &format!("/api/share-return-entitlements/{target}/{verb}"),
                    &w.probe.0,
                    Some(&w.probe.1),
                    Some(payload.clone()),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{verb} on {target}");
            shapes.push(bytes);
        }
        assert_eq!(shapes[0], shapes[1], "{verb}: existence oracle");
        assert_eq!(assert_error_contract(&shapes[0], verb), "not_found");
    }

    // Settlement reverse on a nonexistent settlement id.
    let (status, bytes) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-return-settlements/{missing}/reverse"),
            &w.probe.0,
            Some(&w.probe.1),
            Some(json!({ "reason": "geri" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(assert_error_contract(&bytes, "reverse"), "not_found");

    // B's entitlement is untouched at the row level.
    assert_eq!(
        table_count(&test.pool, "share_return_entitlements", w.coop_b).await,
        ents_before
    );
    assert_eq!(
        table_count(&test.pool, "share_return_settlements", w.coop_b).await,
        setts_before
    );
    let ent_status: String =
        sqlx::query_scalar("SELECT status FROM share_return_entitlements WHERE id = $1")
            .bind(ent_b)
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert_eq!(ent_status, "open");

    // Positive control: the SAME surface accepts the OWN entitlement —
    // build the A pipeline and cancel the entitlement we just created.
    let (ret_a, ret_a_updated) = make_pending_return(&test, &w.admin_a, "a").await;
    let ent_a = finalize_with_entitlement(&test, &w.admin_a, ret_a, &ret_a_updated).await;
    let (status, bytes) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/share-return-entitlements/{ent_a}/cancel"),
            &w.probe.0,
            Some(&w.probe.1),
            Some(json!({ "reason": "haklı iptal" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "own cancel: {:?}", bytes);
    assert_eq!(json_body(&bytes)["status"], json!("cancelled"));
    test.cleanup().await;
}

// ═════════════════════════════════════════════════════════════════════
// 5. Parsed-numeric rejection — bodies, query params, path segments.
// ═════════════════════════════════════════════════════════════════════

/// Hostile numeric representations fail closed; tolerated aliases are
/// normalized canonically; NO request produces a row.
#[tokio::test]
async fn parsed_numeric_inputs_fail_closed_and_never_write() {
    let test = setup().await;
    let w = world(&test, "num").await;

    // A real destination account so a VALID payment can actually write
    // (positive control) — hostile amounts must fail BEFORE that.
    let (status, bytes) = send(
        &test.app,
        req(
            "POST",
            "/api/financial-accounts",
            &w.admin_a.0,
            Some(&w.admin_a.1),
            Some(json!({ "name": format!("B3 Kasa {SENTINEL}"), "accountType": "cash" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "account: {:?}", bytes);
    let account: Uuid = json_body(&bytes)["id"].as_str().unwrap().parse().unwrap();

    let payments_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payments WHERE cooperative_id = $1")
            .bind(w.coop_a)
            .fetch_one(&test.pool)
            .await
            .unwrap();

    let payment = |amount: Value| {
        json!({
            "payerFirstName": "B3",
            "payerLastName": SENTINEL,
            "amount": amount,
            "method": "cash",
            "receivedAt": "2026-01-10T10:00:00Z",
            "destinationAccountId": account,
            "idempotencyKey": format!("b3-pay-{}", Uuid::new_v4().simple()),
            "allocations": [],
        })
    };

    // ── Hostile amount representations → stable rejection, zero rows.
    for amount in [
        json!("abc"),
        json!(""),
        json!("   "),
        json!("-10.00"),
        json!("-0.01"),
        json!("0.00"),                     // zero settles nothing
        json!("10.005"),                   // scale > 2
        json!("999999999999999999999.00"), // over NUMERIC(19,2)
        json!("1e300"),                    // scientific overflow
        json!("NaN"),
        json!("Infinity"),
        json!("0x10"),
        json!("10,00"), // tr-TR display form — not wire format
        json!("10.00.01"),
        json!(" 50.00 "), // whitespace — canonical-only grammar (P2)
        json!("1e2"),     // scientific notation — rejected (P2)
        json!("+10.00"),  // leading sign — rejected (P2)
        json!("1_000"),   // group separator — rejected (P2)
        json!(".5"),      // missing integer part — rejected (P2)
        json!("5."),      // empty fraction — rejected (P2)
        json!(10),        // wrong JSON type entirely
        json!(null),
        json!(["10.00"]),
        json!({"v": "10.00"}),
    ] {
        let (status, bytes) = send(
            &test.app,
            req(
                "POST",
                "/api/payments",
                &w.admin_a.0,
                Some(&w.admin_a.1),
                Some(payment(amount.clone())),
            ),
        )
        .await;
        assert!(
            status.is_client_error(),
            "amount {amount:?} was not rejected ({status})"
        );
        let code = assert_error_contract(&bytes, "numeric rejection");
        assert_eq!(
            code, "validation_failed",
            "amount {amount:?}: wrong code {code}"
        );
    }

    // ── Canonical-only wire grammar (contract hardening P2): ASCII
    //    digits + optional '.' with 1-2 fraction digits; accepted
    //    values still normalize to canonical scale-2.
    for (input, canonical) in [("50", "50.00")] {
        let (status, bytes) = send(
            &test.app,
            req(
                "POST",
                "/api/payments",
                &w.admin_a.0,
                Some(&w.admin_a.1),
                Some(payment(json!(input))),
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "alias {input:?}: {:?}",
            String::from_utf8_lossy(&bytes)
        );
        assert_eq!(
            json_body(&bytes)["payment"]["amount"],
            json!(canonical),
            "alias {input:?} did not normalize canonically"
        );
    }

    let accepted: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payments WHERE cooperative_id = $1")
            .bind(w.coop_a)
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert_eq!(
        accepted - payments_before,
        1,
        "exactly the one canonical alias may write"
    );

    // ── Hostile query numerics on a list endpoint (same-endpoint positive).
    let (status, _) = send(
        &test.app,
        req(
            "GET",
            "/api/share-returns?page=1&pageSize=10",
            &w.probe.0,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "valid pagination positive control");

    for uri in [
        "/api/share-returns?page=abc",
        "/api/share-returns?page=1e9",
        "/api/share-returns?page=99999999999999999999", // i64 overflow
        "/api/share-returns?page=1.5",
        "/api/share-returns?pageSize=0",
        "/api/share-returns?pageSize=-3",
        "/api/share-returns?pageSize=101", // MAX_PAGE_SIZE = 100
        "/api/share-returns?pageSize=abc",
        "/api/share-returns?limit=1", // unknown param tolerated or rejected — pinned below
    ] {
        let (status, bytes) = send(&test.app, req("GET", uri, &w.probe.0, None, None)).await;
        if uri.contains("limit=") {
            continue; // unknown keys are ignored by serde — see pin below
        }
        assert!(
            status.is_client_error(),
            "{uri} was not rejected ({status})"
        );
        assert_eq!(assert_error_contract(&bytes, uri), "validation_failed");
    }
    // Documented tolerance: unknown query keys are IGNORED (serde default)
    // — not an oracle, but pinned so tightening is explicit.
    let (status, _) = send(
        &test.app,
        req(
            "GET",
            "/api/share-returns?limit=1&bogus=x",
            &w.probe.0,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "unknown keys are ignored (pinned)");

    // Negative/zero page CLAMPS to 1 (documented tolerance lane).
    for uri in ["/api/share-returns?page=0", "/api/share-returns?page=-5"] {
        let (status, _) = send(&test.app, req("GET", uri, &w.probe.0, None, None)).await;
        assert_eq!(status, StatusCode::OK, "{uri}: page clamps to 1");
    }

    // ── Path numerics: non-UUID segments reject before scope logic.
    for uri in [
        "/api/share-returns/not-a-uuid",
        "/api/share-returns/12345",
        "/api/share-returns/0",
    ] {
        let (status, bytes) = send(&test.app, req("GET", uri, &w.probe.0, None, None)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(assert_error_contract(&bytes, uri), "validation_failed");
    }

    test.cleanup().await;
}

// ═════════════════════════════════════════════════════════════════════
// 6. Error-response hygiene — contract shape, no reflection, no internals.
// ═════════════════════════════════════════════════════════════════════

/// Every error lane emits exactly `{"error":{"code":...}}`: framework
/// rejections (malformed JSON, wrong content-type), fallbacks, and
/// assertion denials alike. No body ever reflects attacker input or
/// internals.
#[tokio::test]
async fn error_bodies_are_minimal_never_reflective_never_internal() {
    let test = setup().await;
    let w = world(&test, "err").await;

    // Malformed JSON body → sanitized to the contract shape.
    let request = Request::builder()
        .method("POST")
        .uri("/api/payments")
        .header("cookie", format!("kooperatif_session={}", w.admin_a.0))
        .header("origin", ORIGIN)
        .header("x-csrf-token", &w.admin_a.1)
        .header("content-type", "application/json")
        .body(Body::from(format!("{{\"amount\":\"{SENTINEL}\"")))
        .expect("valid");
    let (status, bytes) = send(&test.app, request).await;
    assert!(status.is_client_error());
    assert_error_contract(&bytes, "malformed json");

    // Wrong content-type on a JSON endpoint.
    let request = Request::builder()
        .method("POST")
        .uri("/api/payments")
        .header("cookie", format!("kooperatif_session={}", w.admin_a.0))
        .header("origin", ORIGIN)
        .header("x-csrf-token", &w.admin_a.1)
        .header("content-type", "text/plain")
        .body(Body::from(SENTINEL))
        .expect("valid");
    let (status, bytes) = send(&test.app, request).await;
    assert!(status.is_client_error());
    assert_error_contract(&bytes, "wrong content-type");

    // Unmatched route → contract 404.
    let (status, bytes) = send(
        &test.app,
        req("GET", "/api/definitely-not-a-route", &w.probe.0, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(assert_error_contract(&bytes, "fallback"), "not_found");

    // Hostile cooperative assertion carrying the sentinel.
    let mut request = Request::builder()
        .method("GET")
        .uri("/api/reports/overview")
        .header("cookie", format!("kooperatif_session={}", w.probe.0))
        .header("origin", ORIGIN)
        .body(Body::empty())
        .expect("valid");
    request.headers_mut().insert(
        axum::http::HeaderName::from_static("x-cooperative-id"),
        axum::http::HeaderValue::from_str(SENTINEL).expect("header value"),
    );
    let (status, bytes) = send(&test.app, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error_contract(&bytes, "sentinel header");

    // Validation body containing the sentinel → still not reflected.
    let (status, bytes) = send(
        &test.app,
        req(
            "POST",
            "/api/share-returns",
            &w.probe.0,
            Some(&w.probe.1),
            Some(json!({
                "shareId": Uuid::new_v4(),
                "effectiveReturnDate": format!("{SENTINEL}-date"),
                "reason": SENTINEL,
                "idempotencyKey": format!("b3-{}", SENTINEL),
            })),
        ),
    )
    .await;
    assert!(status.is_client_error());
    assert_error_contract(&bytes, "sentinel body");

    test.cleanup().await;
}

// ═════════════════════════════════════════════════════════════════════
// 7. Audit lane — scoped reads cannot enumerate foreign cooperatives.
// ═════════════════════════════════════════════════════════════════════

/// `?cooperativeId=` on the scoped lane: foreign-real vs never-existed
/// deny identically (403 — claimed-scope assertion, not resource
/// lookup), and the scoped result set never contains B rows.
#[tokio::test]
async fn scoped_audit_lane_no_foreign_enumeration_no_foreign_rows() {
    let test = setup().await;
    let w = world(&test, "aud").await;
    // Generate audit rows inside B (activate + finalize produce entries).
    let (ret_b, ret_b_updated) = make_pending_return(&test, &w.admin_b, "b").await;
    let _ = finalize_with_entitlement(&test, &w.admin_b, ret_b, &ret_b_updated).await;

    // Scoped list as the A probe? — audit.read is not in its grants.
    // The restricted/member identity gets a clean 403, then the A-admin
    // (who DOES hold audit.read via Sistem Yöneticisi) does the probes.
    let (status, bytes) = send(
        &test.app,
        req("GET", "/api/audit/events", &w.probe.0, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        assert_error_contract(&bytes, "audit perm"),
        "permission_denied"
    );

    // Foreign coop id vs never-existed id as query claim → identical 403.
    let mut shapes = Vec::new();
    for claim in [w.coop_b, Uuid::new_v4()] {
        let (status, bytes) = send(
            &test.app,
            req(
                "GET",
                &format!("/api/audit/events?cooperativeId={claim}"),
                &w.admin_a.0,
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "claim {claim}");
        shapes.push(bytes);
    }
    assert_eq!(shapes[0], shapes[1], "cooperativeId claim is an oracle");
    assert_eq!(
        assert_error_contract(&shapes[0], "audit claim"),
        "permission_denied"
    );

    // Positive control: own scope reads; every row belongs to A.
    let (status, bytes) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/audit/events?cooperativeId={}", w.coop_a),
            &w.admin_a.0,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "own audit list: {:?}", bytes);
    let body = json_body(&bytes);
    let items = body["items"].as_array().expect("items array");
    for item in items {
        assert_eq!(
            item["cooperativeId"],
            json!(w.coop_a.to_string()),
            "foreign audit row leaked into the scoped lane"
        );
    }
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        !text.contains(&w.coop_b.to_string()),
        "B's cooperative id leaked into A's audit page"
    );
    test.cleanup().await;
}
