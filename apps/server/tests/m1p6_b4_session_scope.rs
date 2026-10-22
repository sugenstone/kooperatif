//! M1-P6 B4 — multi-cooperative business authorization (0071).
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database;
//! scratch cooperatives only — no real second cooperative is activated).
//!
//! Under test:
//! - The session's `active_cooperative_id` is the authoritative
//!   business scope (bootstrap while the session never switched);
//!   cooperatives A and B are operated through independently switched
//!   sessions with no read/write leakage.
//! - `x-cooperative-id`, request bodies, query parameters and forged or
//!   stale contexts cannot select or override scope — the header may
//!   only confirm it; the database refuses to stamp any other
//!   cooperative (restricted runtime, sealed context).
//! - Suspended/archived/provisioning cooperatives and suspended/ended
//!   memberships fail closed with no fallback to another cooperative.
//! - Permission changes take effect on the very next request.
//! - `memberships.read` / `memberships.manage` / `users.read` are
//!   separated; platform permissions stay platform-only.

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

use kooperatif_server::auth::{identity, token, users, AuthRuntime};
use kooperatif_server::clock::MutableClock;
use kooperatif_server::config::{AuthConfig, ARGON2_M_COST_FLOOR};
use kooperatif_server::db as app_db;
use kooperatif_server::http::{cors_layer, router, AppState};
use kooperatif_server::tenant::repo;

const TEST_PASSWORD: &str = "p6b4-test-parola-123";
const ORIGIN: &str = "http://localhost:5173";
const APP_ROLE: &str = "kooperatif_app";
/// Restricted-principal password: `KOOPERATIF_APP_TEST_PASSWORD`
/// overrides for CI; otherwise the documented local dev password.
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
    /// Router whose ONLY database connection is the real restricted
    /// runtime principal (`kooperatif_app`) — the sealed-context and
    /// permission gates inside the definers are what these tests
    /// exercise; an owner-lane pool would silently bypass them.
    app: axum::Router,
    /// Owner pool for fixtures/assertions only — never routed through
    /// the HTTP app.
    pool: PgPool,
    app_pool: PgPool,
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
        self.app_pool.close().await;
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

static SETUP_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

async fn setup() -> TestApp {
    let _setup_slot = SETUP_SLOTS.acquire().await.expect("setup slot");
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
        app_pool,
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
    users::create_user(pool, username, "B4 Test", &hash, TEST_PASSWORD)
        .await
        .expect("user")
        .id
}

/// NULL-scope platform lane grant (owner-lane fixture).
async fn grant_platform(pool: &PgPool, user: Uuid, perms: &[&str]) {
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, cooperative_id) VALUES ($1, NULL) RETURNING id",
    )
    .bind(format!("plat-b4-{user}"))
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
         VALUES ($1, $2, NULL)",
    )
    .bind(user)
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
}

/// Cooperative-scoped grant inside an explicit cooperative: membership
/// + coop-scoped role + scoped assignment.
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
            .bind(format!("coop-b4-{user}-{coop}"))
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

/// Cooperative-scoped grant on the bootstrap cooperative.
async fn grant_cooperative(pool: &PgPool, user: Uuid, perms: &[&str]) {
    let coop = bootstrap_coop(pool).await;
    grant_in_cooperative(pool, coop, user, perms).await;
}

async fn bootstrap_coop(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("SELECT id FROM cooperatives WHERE is_bootstrap")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Log in over HTTP; returns (cookie, csrf, user_id).
async fn login(app: &axum::Router, peer: SocketAddr, username: &str) -> (String, String, Uuid) {
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
        body["user"]["id"]
            .as_str()
            .expect("id")
            .parse()
            .expect("uuid"),
    )
}

fn request_builder(
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
        builder.body(Body::from(body.to_string())).expect("valid")
    } else {
        builder.body(Body::empty()).expect("valid")
    }
}

/// Same as request_builder but pins an explicit cooperative scope via
/// the `x-cooperative-id` context header.
fn scoped_request_builder(
    method: &str,
    path: &str,
    cookie: &str,
    csrf: Option<&str>,
    body: Option<Value>,
    cooperative_id: Uuid,
) -> Request<Body> {
    let mut request = request_builder(method, path, cookie, csrf, body);
    request.headers_mut().insert(
        "x-cooperative-id",
        cooperative_id.to_string().parse().expect("header value"),
    );
    request
}

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

async fn membership_row(pool: &PgPool, coop: Uuid, user: Uuid) -> Option<(String, Option<Uuid>)> {
    sqlx::query_as::<_, (String, Option<Uuid>)>(
        "SELECT status, created_by_user_id FROM cooperative_memberships \
         WHERE cooperative_id = $1 AND user_id = $2",
    )
    .bind(coop)
    .bind(user)
    .fetch_optional(pool)
    .await
    .unwrap()
}

async fn switch_cooperative(
    app: &axum::Router,
    cookie: &str,
    csrf: &str,
    coop: Uuid,
) -> StatusCode {
    send(
        app,
        request_builder(
            "POST",
            "/api/auth/cooperative",
            cookie,
            Some(csrf),
            Some(json!({ "cooperativeId": coop })),
        ),
    )
    .await
    .0
}

async fn category_coop(pool: &PgPool, name: &str) -> Option<Uuid> {
    sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT cooperative_id FROM financial_categories WHERE name = $1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await
    .unwrap()
    .flatten()
}

async fn create_category(
    app: &axum::Router,
    cookie: &str,
    csrf: &str,
    name: &str,
    header: Option<Uuid>,
) -> (StatusCode, Value) {
    let body = Some(json!({ "categoryType": "income", "name": name }));
    let request = match header {
        Some(coop) => scoped_request_builder(
            "POST",
            "/api/financial-categories",
            cookie,
            Some(csrf),
            body,
            coop,
        ),
        None => request_builder(
            "POST",
            "/api/financial-categories",
            cookie,
            Some(csrf),
            body,
        ),
    };
    send(app, request).await
}

async fn list_categories(app: &axum::Router, cookie: &str) -> (StatusCode, String) {
    let (status, body) = send(
        app,
        request_builder("GET", "/api/financial-categories", cookie, None, None),
    )
    .await;
    (status, body.to_string())
}

/// Fixture: bootstrap A + platform-activated scratch B (test DB only),
/// one shared user with income_expense rights in BOTH cooperatives.
struct TwoCoops {
    coop_a: Uuid,
    coop_b: Uuid,
    shared: Uuid,
}

async fn two_cooperatives(test: &TestApp, tag: &str) -> TwoCoops {
    let coop_a = bootstrap_coop(&test.pool).await;
    let padmin = create_user(&test.pool, &format!("p6b4-padmin-{tag}")).await;
    grant_platform(&test.pool, padmin, &["cooperatives.manage"]).await;
    let (pcookie, pcsrf, _) = login(&test.app, test.peer, &format!("p6b4-padmin-{tag}")).await;
    let admin_b = create_user(&test.pool, &format!("p6b4-adminb-{tag}")).await;
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/platform/cooperatives",
            &pcookie,
            Some(&pcsrf),
            Some(json!({ "name": format!("B4 Kooperatif {tag}") })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let coop_b: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop_b}/activate"),
            &pcookie,
            Some(&pcsrf),
            Some(json!({ "initialAdminUserId": admin_b })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let shared = create_user(&test.pool, &format!("p6b4-shared-{tag}")).await;
    let perms = ["income_expense.read", "income_expense.manage"];
    grant_in_cooperative(&test.pool, coop_a, shared, &perms).await;
    grant_in_cooperative(&test.pool, coop_b, shared, &perms).await;
    TwoCoops {
        coop_a,
        coop_b,
        shared,
    }
}

// ── Session scope is authoritative ──────────────────────────────────

/// A never-switched session operates in bootstrap A (unchanged
/// workflow); after the audited switch the SAME endpoints read and write
/// only cooperative B. No row crosses in either direction.
#[tokio::test]
async fn session_active_cooperative_scopes_business_reads_and_writes() {
    let test = setup().await;
    let f = two_cooperatives(&test, "s1").await;
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b4-shared-s1").await;

    // Bootstrap workflow: headerless, never switched → A.
    let (status, body) = create_category(&test.app, &cookie, &csrf, "b4-cat-A", None).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(category_coop(&test.pool, "b4-cat-A").await, Some(f.coop_a));

    assert_eq!(
        switch_cooperative(&test.app, &cookie, &csrf, f.coop_b).await,
        StatusCode::NO_CONTENT
    );
    let (status, body) = create_category(&test.app, &cookie, &csrf, "b4-cat-B", None).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        category_coop(&test.pool, "b4-cat-B").await,
        Some(f.coop_b),
        "write must land in the session cooperative"
    );
    let (status, listing) = list_categories(&test.app, &cookie).await;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert!(listing.contains("b4-cat-B"));
    assert!(
        !listing.contains("b4-cat-A"),
        "A rows must not leak into B: {listing}"
    );

    // Switch back: A visible, B invisible.
    assert_eq!(
        switch_cooperative(&test.app, &cookie, &csrf, f.coop_a).await,
        StatusCode::NO_CONTENT
    );
    let (status, listing) = list_categories(&test.app, &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert!(listing.contains("b4-cat-A"));
    assert!(
        !listing.contains("b4-cat-B"),
        "B rows must not leak into A: {listing}"
    );

    // Two independently authorized sessions of the same user operate in
    // A and B concurrently, each pinned to its own session scope.
    let (cookie2, csrf2, _) = login(&test.app, test.peer, "p6b4-shared-s1").await;
    assert_eq!(
        switch_cooperative(&test.app, &cookie2, &csrf2, f.coop_b).await,
        StatusCode::NO_CONTENT
    );
    let (_, listing_a) = list_categories(&test.app, &cookie).await;
    let (_, listing_b) = list_categories(&test.app, &cookie2).await;
    assert!(listing_a.contains("b4-cat-A") && !listing_a.contains("b4-cat-B"));
    assert!(listing_b.contains("b4-cat-B") && !listing_b.contains("b4-cat-A"));
    let _ = f.shared;

    test.cleanup().await;
}

/// `/api/auth/me` permission/role hints follow the session scope.
#[tokio::test]
async fn me_permissions_follow_session_scope() {
    let test = setup().await;
    let f = two_cooperatives(&test, "me").await;
    let only_b = create_user(&test.pool, "p6b4-onlyb").await;
    // A bootstrap membership without grants keeps login/me reachable.
    grant_in_cooperative(&test.pool, f.coop_a, only_b, &[]).await;
    grant_in_cooperative(&test.pool, f.coop_b, only_b, &["reports.read"]).await;
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b4-onlyb").await;

    let (_, me) = send(
        &test.app,
        request_builder("GET", "/api/auth/me", &cookie, None, None),
    )
    .await;
    assert!(
        !me["permissions"].to_string().contains("reports.read"),
        "B-only grant must not appear in the bootstrap-scoped hints: {me}"
    );
    assert_eq!(
        switch_cooperative(&test.app, &cookie, &csrf, f.coop_b).await,
        StatusCode::NO_CONTENT
    );
    let (_, me) = send(
        &test.app,
        request_builder("GET", "/api/auth/me", &cookie, None, None),
    )
    .await;
    assert!(
        me["permissions"].to_string().contains("reports.read"),
        "{me}"
    );
    assert_eq!(me["activeCooperativeId"], json!(f.coop_b));

    test.cleanup().await;
}

// ── Header / body / forged context cannot override ──────────────────

/// The header can confirm the session scope but never select another
/// cooperative — even one the caller is an active member of.
#[tokio::test]
async fn header_cannot_override_session_scope() {
    let test = setup().await;
    let f = two_cooperatives(&test, "hdr").await;
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b4-shared-hdr").await;

    // Never switched (scope A): header B is refused, nothing written.
    let (status, body) =
        create_category(&test.app, &cookie, &csrf, "b4-hdr-1", Some(f.coop_b)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "cooperative_access_denied");
    assert_eq!(category_coop(&test.pool, "b4-hdr-1").await, None);

    // Header equal to the session scope is a harmless assertion.
    let (status, body) =
        create_category(&test.app, &cookie, &csrf, "b4-hdr-2", Some(f.coop_a)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Switched to B: header A is refused for reads and writes.
    assert_eq!(
        switch_cooperative(&test.app, &cookie, &csrf, f.coop_b).await,
        StatusCode::NO_CONTENT
    );
    let (status, body) =
        create_category(&test.app, &cookie, &csrf, "b4-hdr-3", Some(f.coop_a)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(category_coop(&test.pool, "b4-hdr-3").await, None);
    let (status, _) = send(
        &test.app,
        scoped_request_builder(
            "GET",
            "/api/financial-categories",
            &cookie,
            None,
            None,
            f.coop_a,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Malformed header → 400; never parsed loosely.
    let mut bad = request_builder("GET", "/api/financial-categories", &cookie, None, None);
    bad.headers_mut()
        .insert("x-cooperative-id", "not-a-uuid".parse().unwrap());
    assert_eq!(send(&test.app, bad).await.0, StatusCode::BAD_REQUEST);

    // Request bodies cannot carry scope (strict body validation).
    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/financial-categories",
            &cookie,
            Some(&csrf),
            Some(json!({
                "categoryType": "income",
                "name": "b4-body",
                "cooperativeId": f.coop_a
            })),
        ),
    )
    .await;
    assert!(
        status.is_client_error(),
        "body scope must be rejected: {status}"
    );
    assert_eq!(category_coop(&test.pool, "b4-body").await, None);

    // Query parameters are never a scope source: listing stays B.
    let (status, body) = send(
        &test.app,
        request_builder(
            "GET",
            &format!("/api/financial-categories?cooperativeId={}", f.coop_a),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    if status == StatusCode::OK {
        assert!(
            !body.to_string().contains("b4-hdr-2"),
            "query param leaked A: {body}"
        );
    } else {
        assert!(status.is_client_error());
    }

    test.cleanup().await;
}

/// Database-level enforcement on the REAL restricted runtime role: a
/// sealed context can only be stamped for the session's scope — even a
/// genuine membership in another cooperative cannot be used, and a
/// forged GUC cannot pass the seal.
#[tokio::test]
async fn restricted_runtime_stamps_only_the_session_scope() {
    let test = setup().await;
    let f = two_cooperatives(&test, "db").await;
    let (raw, csrf, _) = login(&test.app, test.peer, "p6b4-shared-db").await;
    let hash = token::hash_token(&raw);
    let now = OffsetDateTime::parse(
        "2026-01-01T00:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();

    // Never switched → bootstrap A is stampable, B is not.
    assert!(app_db::request_tx(&test.app_pool, &hash, f.coop_a, now)
        .await
        .is_ok());
    assert!(matches!(
        app_db::request_tx(&test.app_pool, &hash, f.coop_b, now).await,
        Err(app_db::RequestContextError::NotMember)
    ));

    // After the switch the relation inverts.
    assert_eq!(
        switch_cooperative(&test.app, &raw, &csrf, f.coop_b).await,
        StatusCode::NO_CONTENT
    );
    assert!(app_db::request_tx(&test.app_pool, &hash, f.coop_b, now)
        .await
        .is_ok());
    assert!(
        matches!(
            app_db::request_tx(&test.app_pool, &hash, f.coop_a, now).await,
            Err(app_db::RequestContextError::NotMember)
        ),
        "a valid membership in A must not open A while the session is in B"
    );

    // Forged GUCs without the seal cannot stamp anything.
    let mut tx = test.app_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.user_id', $1, true)")
        .bind(f.shared.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM public.app_stamp_cooperative($1)")
        .bind(f.coop_b)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(rows, 0, "forged context must not stamp");
    tx.rollback().await.unwrap();

    // The runtime role has no direct write on the session row — scope
    // moves only through the audited switch definer.
    let direct = sqlx::query("UPDATE user_sessions SET active_cooperative_id = $1")
        .bind(f.coop_a)
        .execute(&test.app_pool)
        .await;
    assert!(
        direct.is_err(),
        "kooperatif_app must not rewrite session scope"
    );

    test.cleanup().await;
}

// ── Lifecycle / membership loss: fail closed, never fall back ───────

async fn set_membership_status(pool: &PgPool, coop: Uuid, user: Uuid, status: &str) {
    sqlx::query(
        "UPDATE cooperative_memberships SET status = $3, \
           ended_at = CASE WHEN $3 = 'ended' THEN now() ELSE NULL END \
         WHERE cooperative_id = $1 AND user_id = $2",
    )
    .bind(coop)
    .bind(user)
    .bind(status)
    .execute(pool)
    .await
    .unwrap();
}

async fn set_coop_state(pool: &PgPool, coop: Uuid, status: &str, enabled: bool) {
    sqlx::query("UPDATE cooperatives SET status = $2, business_enabled = $3 WHERE id = $1")
        .bind(coop)
        .bind(status)
        .bind(enabled)
        .execute(pool)
        .await
        .unwrap();
}

/// Stale session scope: once the session's cooperative or membership
/// stops qualifying, business access is denied — it never silently
/// falls back to bootstrap or another membership.
#[tokio::test]
async fn stale_session_scope_fails_closed_without_fallback() {
    let test = setup().await;
    let f = two_cooperatives(&test, "stale").await;
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b4-shared-stale").await;
    let (status, body) = create_category(&test.app, &cookie, &csrf, "b4-stale-a", None).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        switch_cooperative(&test.app, &cookie, &csrf, f.coop_b).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(list_categories(&test.app, &cookie).await.0, StatusCode::OK);

    for (idx, label) in [
        "suspended membership",
        "ended membership",
        "suspended cooperative",
        "archived cooperative",
        "business gate closed",
    ]
    .iter()
    .enumerate()
    {
        match idx {
            0 => set_membership_status(&test.pool, f.coop_b, f.shared, "suspended").await,
            1 => set_membership_status(&test.pool, f.coop_b, f.shared, "ended").await,
            2 => set_coop_state(&test.pool, f.coop_b, "suspended", false).await,
            3 => set_coop_state(&test.pool, f.coop_b, "archived", false).await,
            _ => set_coop_state(&test.pool, f.coop_b, "active", false).await,
        }
        let (status, body) = list_categories(&test.app, &cookie).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{label}: {body}");
        assert!(
            !body.contains("b4-stale-a"),
            "{label}: no data may be served from a fallback scope: {body}"
        );
        let name = format!("b4-stale-{idx}");
        let (status, _) = create_category(&test.app, &cookie, &csrf, &name, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{label}");
        assert_eq!(
            category_coop(&test.pool, &name).await,
            None,
            "{label}: nothing written anywhere"
        );
        // A header naming the still-valid bootstrap cooperative cannot
        // rescue the request.
        let (status, _) = send(
            &test.app,
            scoped_request_builder(
                "GET",
                "/api/financial-categories",
                &cookie,
                None,
                None,
                f.coop_a,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{label}: header rescue");
        if idx < 2 {
            set_membership_status(&test.pool, f.coop_b, f.shared, "active").await;
        } else {
            set_coop_state(&test.pool, f.coop_b, "active", true).await;
        }
        assert_eq!(
            list_categories(&test.app, &cookie).await.0,
            StatusCode::OK,
            "{label}: restore"
        );
    }

    // Switching into a non-qualifying cooperative is refused up front
    // and leaves the session scope unchanged.
    let provisioning = repo::create_cooperative(&test.pool, "B4 Hazırlık", None)
        .await
        .unwrap()
        .id;
    sqlx::query("INSERT INTO cooperative_memberships (cooperative_id, user_id) VALUES ($1, $2)")
        .bind(provisioning)
        .bind(f.shared)
        .execute(&test.pool)
        .await
        .unwrap();
    assert_eq!(
        switch_cooperative(&test.app, &cookie, &csrf, provisioning).await,
        StatusCode::FORBIDDEN,
        "provisioning cooperative is not a valid session scope"
    );
    let active: Option<Uuid> = sqlx::query_scalar(
        "SELECT active_cooperative_id FROM user_sessions WHERE user_id = $1 \
          ORDER BY created_at DESC LIMIT 1",
    )
    .bind(f.shared)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(
        active,
        Some(f.coop_b),
        "refused switch leaves scope unchanged"
    );

    test.cleanup().await;
}

/// Permission revocation during an active session takes effect on the
/// next request in that session's scope.
#[tokio::test]
async fn permission_change_applies_to_active_session_immediately() {
    let test = setup().await;
    let f = two_cooperatives(&test, "perm").await;
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b4-shared-perm").await;
    assert_eq!(
        switch_cooperative(&test.app, &cookie, &csrf, f.coop_b).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(list_categories(&test.app, &cookie).await.0, StatusCode::OK);

    sqlx::query("DELETE FROM user_role_assignments WHERE user_id = $1 AND cooperative_id = $2")
        .bind(f.shared)
        .bind(f.coop_b)
        .execute(&test.pool)
        .await
        .unwrap();
    let (status, body) = list_categories(&test.app, &cookie).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("permission_denied"), "{body}");

    // A's grant is untouched but cannot be borrowed while scoped to B.
    let (status, _) = send(
        &test.app,
        scoped_request_builder(
            "GET",
            "/api/financial-categories",
            &cookie,
            None,
            None,
            f.coop_a,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    test.cleanup().await;
}

// ── Membership permission separation ────────────────────────────────

#[tokio::test]
async fn membership_read_and_manage_are_separated() {
    let test = setup().await;
    let reader = create_user(&test.pool, "p6b4-mreader").await;
    grant_cooperative(&test.pool, reader, &["memberships.read"]).await;
    let manager = create_user(&test.pool, "p6b4-mmanager").await;
    grant_cooperative(&test.pool, manager, &["memberships.manage"]).await;
    let users_reader = create_user(&test.pool, "p6b4-ureader").await;
    grant_cooperative(&test.pool, users_reader, &["users.read"]).await;
    let target = create_user(&test.pool, "p6b4-mtarget").await;

    // memberships.read: list yes, writes no.
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b4-mreader").await;
    let (status, body) = send(
        &test.app,
        request_builder("GET", "/api/memberships", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/memberships",
            &cookie,
            Some(&csrf),
            Some(json!({ "userId": target })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "read must not imply manage");
    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/users/{users_reader}/membership-suspend"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // memberships.manage: create yes, list no (no implicit alias).
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b4-mmanager").await;
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/memberships",
            &cookie,
            Some(&csrf),
            Some(json!({ "userId": target })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, _) = send(
        &test.app,
        request_builder("GET", "/api/memberships", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "manage must not alias read");

    // users.read: existing /api/users unchanged; memberships list denied.
    let (cookie, _, _) = login(&test.app, test.peer, "p6b4-ureader").await;
    let (status, _) = send(
        &test.app,
        request_builder("GET", "/api/users", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "users.read keeps /api/users");
    let (status, _) = send(
        &test.app,
        request_builder("GET", "/api/memberships", &cookie, None, None),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "users.read is not memberships.read"
    );

    test.cleanup().await;
}

/// Migration-time grant audit: memberships.read exists only where
/// users.manage already was or on System Administrator roles (B5);
/// memberships.manage is confined to name-matched Sistem Yöneticisi
/// roles (0072); the platform role still carries exactly
/// cooperatives.manage.
#[tokio::test]
async fn membership_read_grants_are_least_privilege() {
    let test = setup().await;
    let holders: Vec<Uuid> = sqlx::query_scalar(
        "SELECT rp.role_id FROM role_permissions rp \
           JOIN permissions p ON p.id = rp.permission_id \
          WHERE p.key = 'memberships.read'",
    )
    .fetch_all(&test.pool)
    .await
    .unwrap();
    assert!(!holders.is_empty());
    for role in &holders {
        let has_users_manage: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM role_permissions rp \
               JOIN permissions p ON p.id = rp.permission_id \
              WHERE rp.role_id = $1 AND p.key = 'users.manage')",
        )
        .bind(role)
        .fetch_one(&test.pool)
        .await
        .unwrap();
        assert!(
            has_users_manage,
            "{role} got memberships.read without users.manage"
        );
    }
    // B5 (0072, approved): memberships.manage belongs exclusively to
    // name-matched Sistem Yöneticisi roles — the count must equal the
    // number of such roles and no other role may hold it.
    let manage_grants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_permissions rp JOIN permissions p ON p.id = rp.permission_id \
          WHERE p.key = 'memberships.manage'",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    let admin_roles: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM roles WHERE lower(name) = lower('Sistem Yöneticisi')",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(
        manage_grants, admin_roles,
        "memberships.manage must be confined to System Administrator roles"
    );
    let stray_manage: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_permissions rp \
           JOIN permissions p ON p.id = rp.permission_id \
           JOIN roles r ON r.id = rp.role_id \
          WHERE p.key = 'memberships.manage' \
            AND lower(r.name) <> lower('Sistem Yöneticisi')",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(stray_manage, 0);
    let platform_keys: Vec<String> = sqlx::query_scalar(
        "SELECT p.key FROM role_permissions rp JOIN roles r ON r.id = rp.role_id \
           JOIN permissions p ON p.id = rp.permission_id \
          WHERE r.name = 'platform-lifecycle' ORDER BY 1",
    )
    .fetch_all(&test.pool)
    .await
    .unwrap();
    assert_eq!(platform_keys, vec!["cooperatives.manage".to_string()]);

    test.cleanup().await;
}

// ── Platform isolation in a non-bootstrap scope ─────────────────────

/// A session scoped to B with every cooperative grant (including a
/// coop-scoped `cooperatives.manage`) still cannot reach platform lanes
/// nor touch A's memberships.
#[tokio::test]
async fn non_bootstrap_scope_never_yields_platform_or_foreign_authority() {
    let test = setup().await;
    let f = two_cooperatives(&test, "plat").await;
    let b_admin = create_user(&test.pool, "p6b4-badmin").await;
    grant_in_cooperative(
        &test.pool,
        f.coop_b,
        b_admin,
        &[
            "cooperatives.manage",
            "memberships.manage",
            "memberships.read",
            "users.manage",
            "roles.manage",
        ],
    )
    .await;
    let a_only_member = create_user(&test.pool, "p6b4-aonly").await;
    grant_in_cooperative(&test.pool, f.coop_a, a_only_member, &[]).await;
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b4-badmin").await;
    assert_eq!(
        switch_cooperative(&test.app, &cookie, &csrf, f.coop_b).await,
        StatusCode::NO_CONTENT
    );
    for (method, path) in [
        ("GET", "/api/platform/cooperatives".to_string()),
        (
            "POST",
            format!("/api/platform/cooperatives/{}/suspend", f.coop_a),
        ),
    ] {
        let (status, _) = send(
            &test.app,
            request_builder(method, &path, &cookie, Some(&csrf), None),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
    }
    // A header to A is refused; the B-scoped mutation of the shared
    // user touches only the B row.
    let (status, _) = send(
        &test.app,
        scoped_request_builder(
            "POST",
            &format!("/api/users/{}/membership-suspend", f.shared),
            &cookie,
            Some(&csrf),
            None,
            f.coop_a,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/users/{}/membership-suspend", f.shared),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        membership_row(&test.pool, f.coop_b, f.shared)
            .await
            .map(|r| r.0),
        Some("suspended".into())
    );
    assert_eq!(
        membership_row(&test.pool, f.coop_a, f.shared)
            .await
            .map(|r| r.0),
        Some("active".into()),
        "A untouched"
    );
    // Listing shows B members only.
    let (status, body) = send(
        &test.app,
        request_builder("GET", "/api/memberships", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ids: Vec<String> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["userId"].as_str().unwrap().to_string())
        .collect();
    let a_only: Vec<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM cooperative_memberships WHERE cooperative_id = $1 \
         EXCEPT SELECT user_id FROM cooperative_memberships WHERE cooperative_id = $2",
    )
    .bind(f.coop_a)
    .bind(f.coop_b)
    .fetch_all(&test.pool)
    .await
    .unwrap();
    assert!(!a_only.is_empty());
    for id in a_only {
        assert!(
            !ids.contains(&id.to_string()),
            "A-only member {id} leaked into B listing"
        );
    }

    test.cleanup().await;
}
