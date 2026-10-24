//! M1-P8-B Batch 2 — hostile `x-cooperative-id` header matrix.
//!
//! Contract under test (tenant/extractor.rs B4): the SESSION is the
//! only cooperative-scope authority. The header is an ASSERTION — it
//! must equal the session scope or the request fails closed:
//!
//!   malformed / non-UTF8 / empty / comma-joined / duplicate → 400
//!     (cooperative_context_required)
//!   any UUID ≠ session scope (foreign, unknown, stale, case-smuggled)
//!     → 403 (cooperative_access_denied)
//!   equal to session scope (incl. trimmed whitespace) → proceed
//!
//! Security properties proven here:
//!   - a header can never pivot the session into a foreign cooperative
//!   - ambiguous multi-value forms cannot smuggle a second assertion
//!   - header-name casing cannot bypass the assertion check
//!   - denial shapes are identical for foreign and unknown ids
//!   - zero writes occur under any hostile header
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database;
//! scratch cooperatives only).

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderName, HeaderValue, Request, StatusCode};
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

const TEST_PASSWORD: &str = "p8b-hdr-parola-123";
const ORIGIN: &str = "http://localhost:5173";
const APP_ROLE: &str = "kooperatif_app";
const HDR: &str = "x-cooperative-id";

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
            "2026-01-01T00:00:00Z",
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
    users::create_user(pool, username, "P8B Hdr", &hash, TEST_PASSWORD)
        .await
        .expect("user")
        .id
}

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
            .bind(format!("hdr-{user}-{coop}"))
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

/// GET probe with a caller-supplied raw header set — bypasses the
/// typed-header helpers so hostile/duplicate values reach the wire.
async fn raw_get(
    app: &axum::Router,
    cookie: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method("GET")
        .uri("/api/reports/overview")
        .header("cookie", format!("kooperatif_session={cookie}"))
        .header("origin", ORIGIN);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(Body::empty()).expect("valid");
    let response = app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn resolved_scope(app: &axum::Router, cookie: &str) -> Option<Uuid> {
    let (status, body) = {
        let request = Request::builder()
            .method("GET")
            .uri("/api/auth/cooperative-context")
            .header("cookie", format!("kooperatif_session={cookie}"))
            .header("origin", ORIGIN)
            .body(Body::empty())
            .expect("valid");
        let response = app.clone().oneshot(request).await.expect("infallible");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        (
            status,
            serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
        )
    };
    if status != StatusCode::OK {
        return None;
    }
    body["cooperativeId"].as_str().and_then(|s| s.parse().ok())
}

/// Fixture: bootstrap coop A (session scope) + platform-provisioned B;
/// session user is a member of A only.
struct Ctx {
    coop_a: Uuid,
    coop_b: Uuid,
    cookie: String,
    csrf: String,
}

async fn ctx(test: &TestApp, tag: &str) -> Ctx {
    let coop_a = bootstrap_coop(&test.pool).await;
    // Platform-provision a second cooperative so B is a REAL, active
    // tenant — the denial can only be scope, never a dangling id.
    let padmin = create_user(&test.pool, &format!("p8b-hdr-pa-{tag}")).await;
    let plat_role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, cooperative_id) VALUES ($1, NULL) RETURNING id",
    )
    .bind(format!("hdr-plat2-{tag}"))
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
    let (pcookie, pcsrf) = login(&test.app, test.peer, &format!("p8b-hdr-pa-{tag}")).await;
    let response = {
        let request = Request::builder()
            .method("POST")
            .uri("/api/platform/cooperatives")
            .header("cookie", format!("kooperatif_session={pcookie}"))
            .header("origin", ORIGIN)
            .header("x-csrf-token", &pcsrf)
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "name": format!("P8B Hdr Kooperatif {tag}") }).to_string(),
            ))
            .expect("valid");
        test.app.clone().oneshot(request).await.expect("infallible")
    };
    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body: Value = serde_json::from_slice(&bytes).expect("json");
    let coop_b: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    let admin_b = create_user(&test.pool, &format!("p8b-hdr-ab-{tag}")).await;
    let response = {
        let request = Request::builder()
            .method("POST")
            .uri(format!("/api/platform/cooperatives/{coop_b}/activate"))
            .header("cookie", format!("kooperatif_session={pcookie}"))
            .header("origin", ORIGIN)
            .header("x-csrf-token", &pcsrf)
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "initialAdminUserId": admin_b }).to_string(),
            ))
            .expect("valid");
        test.app.clone().oneshot(request).await.expect("infallible")
    };
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let user = create_user(&test.pool, &format!("p8b-hdr-u-{tag}")).await;
    grant_in_cooperative(&test.pool, coop_a, user, &["reports.read"]).await;
    let (cookie, csrf) = login(&test.app, test.peer, &format!("p8b-hdr-u-{tag}")).await;
    assert_eq!(resolved_scope(&test.app, &cookie).await, Some(coop_a));
    Ctx {
        coop_a,
        coop_b,
        cookie,
        csrf,
    }
}

// ═════════════════════════════════════════════════════════════════════
// Assertion semantics — the header must equal the session scope.
// ═════════════════════════════════════════════════════════════════════

/// Correct id proceeds; foreign, unknown and stale-but-valid ids all
/// deny with the SAME (403, code) shape — no membership oracle.
#[tokio::test]
async fn assertion_must_equal_session_scope_foreign_and_unknown_deny_identically() {
    let test = setup().await;
    let c = ctx(&test, "assert").await;

    // Positive control: no header at all resolves to session scope A.
    let (status, _) = raw_get(&test.app, &c.cookie, &[]).await;
    assert_eq!(status, StatusCode::OK, "headerless baseline must pass");

    // Positive control: asserting the real scope also passes.
    let (status, _) = raw_get(&test.app, &c.cookie, &[(HDR, &c.coop_a.to_string())]).await;
    assert_eq!(status, StatusCode::OK, "matching assertion must pass");

    let mut shapes = Vec::new();
    for probe in [c.coop_b.to_string(), Uuid::new_v4().to_string()] {
        let (status, body) = raw_get(&test.app, &c.cookie, &[(HDR, &probe)]).await;
        shapes.push((status, body["error"]["code"].clone()));
    }
    // Foreign-member-B id and never-existed id deny IDENTICALLY —
    // the response cannot distinguish "exists, not yours" from
    // "does not exist".
    assert_eq!(shapes[0], shapes[1], "foreign/unknown denial shapes differ");
    assert_eq!(shapes[0].0, StatusCode::FORBIDDEN);
    assert_eq!(shapes[0].1, json!("cooperative_access_denied"));
    test.cleanup().await;
}

/// Whitespace-padded form of the REAL scope is accepted (documented
/// `.trim()` tolerance); the same padding around a foreign id is
/// still denied — tolerance can never widen scope.
#[tokio::test]
async fn whitespace_tolerance_never_widens_scope() {
    let test = setup().await;
    let c = ctx(&test, "ws").await;
    let padded_a = format!("  {} \t", c.coop_a);
    let (status, _) = raw_get(&test.app, &c.cookie, &[(HDR, &padded_a)]).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "trimmed matching assertion should be accepted per contract"
    );
    let padded_b = format!(" \t{}  ", c.coop_b);
    let (status, body) = raw_get(&test.app, &c.cookie, &[(HDR, &padded_b)]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], json!("cooperative_access_denied"));
    test.cleanup().await;
}

/// Malformed representations fail closed with 400 — never silently
/// ignored into a scope-bearing request.
#[tokio::test]
async fn malformed_representations_fail_closed() {
    let test = setup().await;
    let c = ctx(&test, "mal").await;
    for value in [
        "not-a-uuid",
        "",
        "   ",
        "0",
        &format!("{},{}", c.coop_a, c.coop_b), // comma-joined pair
        &format!("{};", c.coop_a),             // suffix smuggling
        &format!("\"{}\"", c.coop_a),          // quoted
    ] {
        let (status, body) = raw_get(&test.app, &c.cookie, &[(HDR, value)]).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "hostile value {value:?} must fail closed (got {status})"
        );
        assert_eq!(body["error"]["code"], json!("cooperative_context_required"));
    }
    // Non-UTF8 value: same 400 lane.
    let mut request = Request::builder()
        .method("GET")
        .uri("/api/reports/overview")
        .header("cookie", format!("kooperatif_session={}", c.cookie))
        .header("origin", ORIGIN)
        .body(Body::empty())
        .expect("valid");
    request.headers_mut().insert(
        HeaderName::from_static(HDR),
        HeaderValue::from_bytes(&[0xff, 0xfe]).expect("raw bytes"),
    );
    let response = test.app.clone().oneshot(request).await.expect("infallible");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    test.cleanup().await;
}

/// Header-NAME casing cannot smuggle an unasserted scope: HTTP field
/// names are case-insensitive, so `X-Cooperative-Id` IS the assertion
/// header and must be enforced identically.
#[tokio::test]
async fn header_name_casing_variants_are_enforced_identically() {
    let test = setup().await;
    let c = ctx(&test, "case").await;
    for name in ["X-Cooperative-Id", "X-COOPERATIVE-ID", "x-Cooperative-Id"] {
        let (status, body) = raw_get(&test.app, &c.cookie, &[(name, &c.coop_b.to_string())]).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "case-variant {name} bypassed the assertion check"
        );
        assert_eq!(body["error"]["code"], json!("cooperative_access_denied"));
    }
    test.cleanup().await;
}

/// DUPLICATE header fields: exactly ONE `x-cooperative-id` value is
/// permitted — ANY second value (identical, foreign, malformed, or
/// scope-matching) fails closed with 400 BEFORE parsing. The header
/// remains confirm-only; ambiguity is never silently resolved.
#[tokio::test]
async fn duplicate_headers_fail_closed_regardless_of_order_or_equality() {
    let test = setup().await;
    let c = ctx(&test, "dup").await;
    let a = c.coop_a.to_string();
    let b = c.coop_b.to_string();
    for (first, second) in [
        (a.as_str(), a.as_str()), // identical repeat
        (a.as_str(), b.as_str()), // scope then foreign
        (b.as_str(), a.as_str()), // foreign then scope
        (a.as_str(), "garbage"),  // scope then malformed
        ("garbage", a.as_str()),  // malformed then scope
    ] {
        let (status, body) = raw_get(&test.app, &c.cookie, &[(HDR, first), (HDR, second)]).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "duplicate pair {first:?},{second:?} must fail closed (got {status})"
        );
        assert_eq!(body["error"]["code"], json!("cooperative_context_required"));
    }
    // Triple headers — count >1 is what matters, not which pair.
    let (status, _) = raw_get(&test.app, &c.cookie, &[(HDR, &a), (HDR, &a), (HDR, &a)]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Session scope is unmoved after every ambiguous probe.
    assert_eq!(resolved_scope(&test.app, &c.cookie).await, Some(c.coop_a));
    test.cleanup().await;
}

/// The header never performs an implicit switch: after hostile probes
/// (foreign id, malformed, duplicates) the session still resolves to A,
/// and the audited switch endpoint remains the only migration path.
#[tokio::test]
async fn headers_cannot_pivot_or_switch_session_scope() {
    let test = setup().await;
    let c = ctx(&test, "pivot").await;
    for value in [
        c.coop_b.to_string(),
        "garbage".to_string(),
        format!(" {}", c.coop_b),
    ] {
        let _ = raw_get(&test.app, &c.cookie, &[(HDR, &value)]).await;
        assert_eq!(
            resolved_scope(&test.app, &c.cookie).await,
            Some(c.coop_a),
            "scope drifted after hostile header {value:?}"
        );
    }
    // Forged direct switch to a non-member cooperative: denied, and
    // the scope stays A afterwards.
    let request = Request::builder()
        .method("POST")
        .uri("/api/auth/cooperative")
        .header("cookie", format!("kooperatif_session={}", c.cookie))
        .header("origin", ORIGIN)
        .header("x-csrf-token", &c.csrf)
        .header("content-type", "application/json")
        .body(Body::from(json!({ "cooperativeId": c.coop_b }).to_string()))
        .expect("valid");
    let response = test.app.clone().oneshot(request).await.expect("infallible");
    assert!(!response.status().is_success());
    assert_eq!(resolved_scope(&test.app, &c.cookie).await, Some(c.coop_a));
    test.cleanup().await;
}

/// Cross-member truth: a user who IS a member of both A and B, with
/// session scope B, is still denied by a header asserting A — the
/// header confirms the SESSION scope, never the membership set.
#[tokio::test]
async fn assertion_targets_session_not_membership() {
    let test = setup().await;
    let c = ctx(&test, "dual").await;
    let dual = create_user(&test.pool, "p8b-hdr-dual").await;
    grant_in_cooperative(&test.pool, c.coop_a, dual, &["reports.read"]).await;
    grant_in_cooperative(&test.pool, c.coop_b, dual, &["reports.read"]).await;
    let (dcookie, dcsrf) = login(&test.app, test.peer, "p8b-hdr-dual").await;
    // Switch the session to B through the audited endpoint.
    let request = Request::builder()
        .method("POST")
        .uri("/api/auth/cooperative")
        .header("cookie", format!("kooperatif_session={dcookie}"))
        .header("origin", ORIGIN)
        .header("x-csrf-token", &dcsrf)
        .header("content-type", "application/json")
        .body(Body::from(json!({ "cooperativeId": c.coop_b }).to_string()))
        .expect("valid");
    let response = test.app.clone().oneshot(request).await.expect("infallible");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(resolved_scope(&test.app, &dcookie).await, Some(c.coop_b));
    // Member of A asserting A while scoped to B: denied — membership
    // does not loosen the assertion contract.
    let (status, body) = raw_get(&test.app, &dcookie, &[(HDR, &c.coop_a.to_string())]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], json!("cooperative_access_denied"));
    // Asserting the live scope B: accepted.
    let (status, _) = raw_get(&test.app, &dcookie, &[(HDR, &c.coop_b.to_string())]).await;
    assert_eq!(status, StatusCode::OK);
    test.cleanup().await;
}

/// Hostile headers on a MUTATION: denial happens before any write —
/// verified at the row level, in BOTH cooperatives.
#[tokio::test]
async fn hostile_headers_on_mutations_write_nothing() {
    let test = setup().await;
    let c = ctx(&test, "mut").await;
    let count_payments = |coop: Uuid| {
        let pool = test.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM payments WHERE cooperative_id = $1")
                .bind(coop)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    let a_before = count_payments(c.coop_a).await;
    let b_before = count_payments(c.coop_b).await;
    for (name, value) in [
        (HDR, c.coop_b.to_string()),
        (HDR, Uuid::new_v4().to_string()),
        (HDR, "garbage".to_string()),
        ("X-COOPERATIVE-ID", c.coop_b.to_string()),
    ] {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/payments")
            .header("cookie", format!("kooperatif_session={}", c.cookie))
            .header("origin", ORIGIN)
            .header("x-csrf-token", &c.csrf)
            .header("content-type", "application/json")
            .header(name, &value)
            .body(Body::from(
                json!({
                    "amount": "10.00", "method": "cash",
                    "receivedAt": "2026-01-02T10:00:00Z",
                    "idempotencyKey": "hdr-probe", "allocations": []
                })
                .to_string(),
            ))
            .expect("valid");
        request.extensions_mut().insert(ConnectInfo(test.peer));
        let response = test.app.clone().oneshot(request).await.expect("infallible");
        assert!(
            !response.status().is_success(),
            "hostile header {name}={value} reached a write"
        );
    }
    assert_eq!(count_payments(c.coop_a).await, a_before, "A gained rows");
    assert_eq!(count_payments(c.coop_b).await, b_before, "B gained rows");
    test.cleanup().await;
}
