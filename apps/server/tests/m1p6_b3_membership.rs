//! M1-P6 Part A + B3 — reversible archive + cooperative membership
//! management tests (0070).
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same harness as tests/m1p6_b2_platform_lifecycle.rs).
//!
//! Under test:
//! - PART A: archived cooperatives restore ONLY through the platform
//!   reactivation lane, behind the same effective-administrator
//!   invariant; archived -> suspended stays invalid; nothing
//!   reactivates an archived row implicitly; history survives.
//! - PART B: POST /api/memberships requires cooperative-scoped
//!   memberships.manage, derives the cooperative from the sealed scope
//!   (never the request), refuses duplicates/ineligible users/non-live
//!   cooperatives, and stamps the sealed actor.
//! - The lifecycle routes accept memberships.manage alongside the
//!   legacy users.manage grant; last-administrator protection is
//!   unchanged.
//! - Memberships are per-(cooperative, user): the same global identity
//!   holds independent rows, role assignments stay inside their
//!   cooperative, and cooperative grants never satisfy platform lanes.
//! - Membership mutations mint attributed audit events and emit the
//!   signed `authz` invalidation signal.

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

const TEST_PASSWORD: &str = "p6b3-test-parola-123";
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
    users::create_user(pool, username, "B3 Test", &hash, TEST_PASSWORD)
        .await
        .expect("user")
        .id
}

/// NULL-scope platform lane grant (owner-lane fixture).
async fn grant_platform(pool: &PgPool, user: Uuid, perms: &[&str]) {
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, cooperative_id) VALUES ($1, NULL) RETURNING id",
    )
    .bind(format!("plat-b3-{user}"))
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
            .bind(format!("coop-b3-{user}-{coop}"))
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

/// M1-P6 B4: cooperative scope comes from the session — switch through
/// the audited workflow before operating in a non-bootstrap cooperative.
async fn switch_cooperative(app: &axum::Router, cookie: &str, csrf: &str, coop: Uuid) {
    let (status, body) = send(
        app,
        request_builder(
            "POST",
            "/api/auth/cooperative",
            cookie,
            Some(csrf),
            Some(json!({ "cooperativeId": coop })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "switch failed: {body}");
}

async fn coop_row(pool: &PgPool, id: Uuid) -> (String, bool) {
    sqlx::query_as::<_, (String, bool)>(
        "SELECT status, business_enabled FROM cooperatives WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
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

/// Full platform lifecycle roundtrip on a scratch cooperative:
/// create → activate with the given initial administrator.
async fn activated_cooperative(
    app: &axum::Router,
    cookie: &str,
    csrf: &str,
    name: &str,
    initial_admin: Uuid,
) -> Uuid {
    let (status, body) = send(
        app,
        request_builder(
            "POST",
            "/api/platform/cooperatives",
            cookie,
            Some(csrf),
            Some(json!({ "name": name })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let coop: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    let (status, body) = send(
        app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop}/activate"),
            cookie,
            Some(csrf),
            Some(json!({ "initialAdminUserId": initial_admin })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    coop
}

// ════════════════════════════════════════════════════════════════════
// PART A — reversible archive
// ════════════════════════════════════════════════════════════════════

/// archived -> active restores the cooperative through the platform
/// lane; history survives; audit + realtime invalidation fire.
#[tokio::test]
async fn archived_restore_roundtrip_preserves_history() {
    let test = setup().await;
    let admin_user = create_user(&test.pool, "p6b3-padmin1").await;
    grant_platform(&test.pool, admin_user, &["cooperatives.manage"]).await;
    let member = create_user(&test.pool, "p6b3-member1").await;
    let (cookie, csrf, actor) = login(&test.app, test.peer, "p6b3-padmin1").await;

    let coop = activated_cooperative(&test.app, &cookie, &csrf, "Arşiv Kooperatifi", member).await;

    // Business request before archival succeeds for the member —
    // /api/memberships is cooperative-general (TenantCtx), unlike the
    // bootstrap-pinned legacy business routes.
    let (member_cookie, member_csrf, _) = login(&test.app, test.peer, "p6b3-member1").await;
    switch_cooperative(&test.app, &member_cookie, &member_csrf, coop).await;
    let (status, body) = send(
        &test.app,
        scoped_request_builder("GET", "/api/memberships", &member_cookie, None, None, coop),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // active -> archived: business gate closes, every row survives.
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop}/archive"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (st, enabled) = coop_row(&test.pool, coop).await;
    assert_eq!((st.as_str(), enabled), ("archived", false));

    let (status, body) = send(
        &test.app,
        scoped_request_builder("GET", "/api/memberships", &member_cookie, None, None, coop),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "archived cooperative must deny business: {body}"
    );

    let memberships_before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cooperative_memberships WHERE cooperative_id = $1",
    )
    .bind(coop)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    let roles_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM roles WHERE cooperative_id = $1")
            .bind(coop)
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert!(memberships_before >= 1 && roles_before >= 1);

    // archived -> active restores business access.
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop}/reactivate"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (st, enabled) = coop_row(&test.pool, coop).await;
    assert_eq!((st.as_str(), enabled), ("active", true));

    let (status, _) = send(
        &test.app,
        scoped_request_builder("GET", "/api/memberships", &member_cookie, None, None, coop),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "business must resume");

    // Every historical row survived the archive/restore roundtrip.
    let memberships_after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cooperative_memberships WHERE cooperative_id = $1",
    )
    .bind(coop)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    let roles_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM roles WHERE cooperative_id = $1")
            .bind(coop)
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert_eq!(memberships_after, memberships_before);
    assert_eq!(roles_after, roles_before);

    // Audit: archive + restore mint attributed platform events.
    let events: Vec<(String, Uuid)> = sqlx::query_as(
        "SELECT event_type, user_id FROM security_events \
          WHERE event_type IN ('cooperative_archived','cooperative_reactivated') \
            AND metadata->>'cooperative_id' = $1::text",
    )
    .bind(coop.to_string())
    .fetch_all(&test.pool)
    .await
    .unwrap();
    let types: Vec<&str> = events.iter().map(|(t, _)| t.as_str()).collect();
    assert!(types.contains(&"cooperative_archived"), "{types:?}");
    assert!(
        types.contains(&"cooperative_reactivated"),
        "archived -> active must mint cooperative_reactivated: {types:?}"
    );
    for (_, uid) in &events {
        assert_eq!(*uid, actor);
    }

    test.cleanup().await;
}

/// Restore requires the effective-administrator invariant: an archived
/// cooperative whose only administrator lost the administration path
/// stays archived and answers 409.
#[tokio::test]
async fn archived_restore_requires_effective_admin() {
    let test = setup().await;
    let admin_user = create_user(&test.pool, "p6b3-padmin2").await;
    grant_platform(&test.pool, admin_user, &["cooperatives.manage"]).await;
    let member = create_user(&test.pool, "p6b3-member2").await;
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b3-padmin2").await;

    let coop = activated_cooperative(&test.app, &cookie, &csrf, "Kilitli Arşiv", member).await;

    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop}/archive"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Owner-lane fault injection: the only admin path disappears.
    sqlx::query(
        "UPDATE cooperative_memberships SET status = 'ended' \
         WHERE cooperative_id = $1 AND user_id = $2",
    )
    .bind(coop)
    .bind(member)
    .execute(&test.pool)
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT public.app_admin_path_count($1)")
            .bind(coop)
            .fetch_one(&test.pool)
            .await
            .unwrap(),
        0
    );

    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop}/reactivate"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (st, enabled) = coop_row(&test.pool, coop).await;
    assert_eq!(
        (st.as_str(), enabled),
        ("archived", false),
        "failed restore must leave the cooperative archived"
    );

    // Restore the admin path → restore succeeds.
    sqlx::query(
        "UPDATE cooperative_memberships SET status = 'active', ended_at = NULL \
         WHERE cooperative_id = $1 AND user_id = $2",
    )
    .bind(coop)
    .bind(member)
    .execute(&test.pool)
    .await
    .unwrap();
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop}/reactivate"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    test.cleanup().await;
}

/// archived -> suspended remains invalid; archived -> archived is the
/// idempotent no-op; restoration is never implicit.
#[tokio::test]
async fn archived_invalid_transitions_and_no_implicit_reactivation() {
    let test = setup().await;
    let admin_user = create_user(&test.pool, "p6b3-padmin3").await;
    grant_platform(&test.pool, admin_user, &["cooperatives.manage"]).await;
    let member = create_user(&test.pool, "p6b3-member3").await;
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b3-padmin3").await;

    let coop =
        activated_cooperative(&test.app, &cookie, &csrf, "Geçişsiz Kooperatif", member).await;
    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop}/archive"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // archived -> suspended is not a legal edge.
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop}/suspend"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // The administrator can still authenticate globally and can see
    // nothing implicitly reactivates the cooperative.
    let (member_cookie, member_csrf, _) = login(&test.app, test.peer, "p6b3-member3").await;
    let (status, _) = send(
        &test.app,
        scoped_request_builder("GET", "/api/memberships", &member_cookie, None, None, coop),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (st, enabled) = coop_row(&test.pool, coop).await;
    assert_eq!(
        (st.as_str(), enabled),
        ("archived", false),
        "login/membership access must never reactivate implicitly"
    );
    let _ = member_csrf;

    // archived -> archived is the idempotent no-op.
    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop}/archive"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    test.cleanup().await;
}

/// Nobody below NULL-scope cooperatives.manage may restore an archived
/// cooperative — not cooperative administrators, not coop-scoped
/// holders of the same key.
#[tokio::test]
async fn unauthorized_cannot_restore_archived_cooperative() {
    let test = setup().await;
    let bootstrap = bootstrap_coop(&test.pool).await;
    let admin_user = create_user(&test.pool, "p6b3-padmin4").await;
    grant_platform(&test.pool, admin_user, &["cooperatives.manage"]).await;
    let member = create_user(&test.pool, "p6b3-member4").await;
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b3-padmin4").await;

    let coop =
        activated_cooperative(&test.app, &cookie, &csrf, "Korumalı Kooperatif", member).await;
    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop}/archive"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Adversaries: the coop's own administrator (memberships + roles
    // manage inside the coop) and a bootstrap coop admin.
    grant_in_cooperative(
        &test.pool,
        coop,
        member,
        &["memberships.manage", "roles.manage", "users.manage"],
    )
    .await;
    let coop_admin = create_user(&test.pool, "p6b3-cadmin4").await;
    grant_in_cooperative(
        &test.pool,
        bootstrap,
        coop_admin,
        &["roles.manage", "users.manage"],
    )
    .await;
    let scoped_holder = create_user(&test.pool, "p6b3-scoped4").await;
    grant_cooperative(&test.pool, scoped_holder, &["cooperatives.manage"]).await;

    for (username, label) in [
        ("p6b3-member4", "archived coop's own administrator"),
        ("p6b3-cadmin4", "bootstrap cooperative administrator"),
        ("p6b3-scoped4", "coop-scoped cooperatives.manage holder"),
    ] {
        let (acookie, acsrf, _) = login(&test.app, test.peer, username).await;
        let (status, body) = send(
            &test.app,
            request_builder(
                "POST",
                &format!("/api/platform/cooperatives/{coop}/reactivate"),
                &acookie,
                Some(&acsrf),
                None,
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{label} must not restore: {body}"
        );
    }
    let (st, enabled) = coop_row(&test.pool, coop).await;
    assert_eq!((st.as_str(), enabled), ("archived", false));

    test.cleanup().await;
}

// ════════════════════════════════════════════════════════════════════
// PART B — membership management
// ════════════════════════════════════════════════════════════════════

/// memberships.manage holder creates a membership: active row,
/// actor attribution, audit event, realtime signal, list visibility.
#[tokio::test]
async fn memberships_manage_holder_creates_membership() {
    let test = setup().await;
    let coop = bootstrap_coop(&test.pool).await;
    let manager = create_user(&test.pool, "p6b3-mgr1").await;
    grant_cooperative(
        &test.pool,
        manager,
        &["memberships.manage", "memberships.read"],
    )
    .await;
    let target = create_user(&test.pool, "p6b3-target1").await;

    let mut listener = sqlx::postgres::PgListener::connect_with(&test.pool)
        .await
        .expect("listener");
    listener
        .listen("kooperatif_domain_changed")
        .await
        .expect("listen");

    let (cookie, csrf, actor) = login(&test.app, test.peer, "p6b3-mgr1").await;
    let (status, body) = send(
        &test.app,
        scoped_request_builder(
            "POST",
            "/api/memberships",
            &cookie,
            Some(&csrf),
            Some(json!({ "userId": target })),
            coop,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["userId"], json!(target));
    assert_eq!(body["status"], "active");
    assert_eq!(body["username"], "p6b3-target1");

    assert_eq!(
        membership_row(&test.pool, coop, target).await,
        Some(("active".into(), Some(actor)))
    );

    // Audit: cooperative_membership_granted attributed to the manager
    // and the cooperative.
    let audit: Vec<(Uuid, Value)> = sqlx::query_as(
        "SELECT user_id, metadata FROM security_events \
          WHERE event_type = 'cooperative_membership_granted' \
            AND metadata->>'target_user_id' = $1::text",
    )
    .bind(target.to_string())
    .fetch_all(&test.pool)
    .await
    .unwrap();
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0].0, actor);
    assert_eq!(
        audit[0].1["cooperative_id"],
        json!(coop),
        "membership grant must carry cooperative attribution"
    );

    // Realtime: the membership INSERT emitted a scoped authz signal.
    let notification = tokio::time::timeout(std::time::Duration::from_secs(5), listener.recv())
        .await
        .expect("authz signal must arrive")
        .expect("notification");
    let payload = notification.payload().to_string();
    assert!(
        payload.starts_with(&format!("v2|{coop}|authz|")),
        "expected scoped authz signal, got {payload}"
    );

    // GET lists the new member.
    let (status, body) = send(
        &test.app,
        scoped_request_builder("GET", "/api/memberships", &cookie, None, None, coop),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body.as_array().unwrap();
    assert!(rows
        .iter()
        .any(|r| r["userId"] == json!(target) && r["status"] == "active"));

    test.cleanup().await;
}

/// Membership creation is memberships.manage-ONLY: the legacy
/// users.manage grant authorizes the lifecycle routes but not
/// creation; plain members are denied outright.
#[tokio::test]
async fn creation_requires_memberships_manage() {
    let test = setup().await;
    let coop = bootstrap_coop(&test.pool).await;
    let users_mgr = create_user(&test.pool, "p6b3-umgr2").await;
    grant_cooperative(&test.pool, users_mgr, &["users.manage", "users.read"]).await;
    let plain = create_user(&test.pool, "p6b3-plain2").await;
    repo::add_member(&test.pool, "Ana Kooperatif", plain, None)
        .await
        .unwrap();
    let target = create_user(&test.pool, "p6b3-target2").await;

    for (username, label) in [
        ("p6b3-umgr2", "users.manage-only administrator"),
        ("p6b3-plain2", "plain member"),
    ] {
        let (cookie, csrf, _) = login(&test.app, test.peer, username).await;
        let (status, body) = send(
            &test.app,
            scoped_request_builder(
                "POST",
                "/api/memberships",
                &cookie,
                Some(&csrf),
                Some(json!({ "userId": target })),
                coop,
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{label} must not create memberships: {body}"
        );
        assert_eq!(body["error"]["code"], "permission_denied", "{label}");
    }
    assert!(
        membership_row(&test.pool, coop, target).await.is_none(),
        "denied attempts must leave no row"
    );

    test.cleanup().await;
}

/// The lifecycle routes accept the dedicated permission now AND keep
/// the legacy users.manage grant working — suspend/reactivate/end
/// succeed for either holder; history rows are preserved.
#[tokio::test]
async fn lifecycle_routes_accept_either_membership_permission() {
    let test = setup().await;
    let coop = bootstrap_coop(&test.pool).await;
    let mmgr = create_user(&test.pool, "p6b3-mmgr3").await;
    grant_cooperative(&test.pool, mmgr, &["memberships.manage"]).await;
    let umgr = create_user(&test.pool, "p6b3-umgr3").await;
    grant_cooperative(&test.pool, umgr, &["users.manage"]).await;
    // Preserve the sysadmin last-admin path so lifecycle mutations on
    // non-admin members are never lockout-affected.
    let member = create_user(&test.pool, "p6b3-target3").await;
    repo::add_member(&test.pool, "Ana Kooperatif", member, None)
        .await
        .unwrap();

    // memberships.manage suspends.
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b3-mmgr3").await;
    let (status, body) = send(
        &test.app,
        scoped_request_builder(
            "POST",
            &format!("/api/users/{member}/membership-suspend"),
            &cookie,
            Some(&csrf),
            None,
            coop,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "memberships.manage must suspend: {body}"
    );
    assert_eq!(
        membership_row(&test.pool, coop, member).await.map(|r| r.0),
        Some("suspended".into())
    );

    // users.manage reactivates — backward compatibility.
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b3-umgr3").await;
    let (status, body) = send(
        &test.app,
        scoped_request_builder(
            "POST",
            &format!("/api/users/{member}/membership-reactivate"),
            &cookie,
            Some(&csrf),
            None,
            coop,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "users.manage compat must reactivate: {body}"
    );
    assert_eq!(
        membership_row(&test.pool, coop, member).await.map(|r| r.0),
        Some("active".into())
    );

    // A plain member can do neither.
    let plain = create_user(&test.pool, "p6b3-plain3").await;
    repo::add_member(&test.pool, "Ana Kooperatif", plain, None)
        .await
        .unwrap();
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b3-plain3").await;
    let (status, _) = send(
        &test.app,
        scoped_request_builder(
            "POST",
            &format!("/api/users/{member}/membership-end"),
            &cookie,
            Some(&csrf),
            None,
            coop,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // History preserved — end then reactivate keeps the single row.
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b3-umgr3").await;
    let (status, _) = send(
        &test.app,
        scoped_request_builder(
            "POST",
            &format!("/api/users/{member}/membership-end"),
            &cookie,
            Some(&csrf),
            None,
            coop,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let ended_at: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT ended_at FROM cooperative_memberships \
         WHERE cooperative_id = $1 AND user_id = $2",
    )
    .bind(coop)
    .bind(member)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert!(ended_at.is_some(), "ended membership stamps ended_at");

    test.cleanup().await;
}

/// Duplicates and ineligible users are refused atomically — no rows,
/// no partial writes.
#[tokio::test]
async fn duplicate_and_ineligible_memberships_rejected() {
    let test = setup().await;
    let coop = bootstrap_coop(&test.pool).await;
    let manager = create_user(&test.pool, "p6b3-mgr4").await;
    grant_cooperative(&test.pool, manager, &["memberships.manage"]).await;
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b3-mgr4").await;

    // Duplicate: the manager already holds a membership row.
    let (status, body) = send(
        &test.app,
        scoped_request_builder(
            "POST",
            "/api/memberships",
            &cookie,
            Some(&csrf),
            Some(json!({ "userId": manager })),
            coop,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // Unknown user.
    let (status, body) = send(
        &test.app,
        scoped_request_builder(
            "POST",
            "/api/memberships",
            &cookie,
            Some(&csrf),
            Some(json!({ "userId": Uuid::new_v4() })),
            coop,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // Disabled global user.
    let disabled = create_user(&test.pool, "p6b3-disabled4").await;
    sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(disabled)
        .execute(&test.pool)
        .await
        .unwrap();
    let (status, body) = send(
        &test.app,
        scoped_request_builder(
            "POST",
            "/api/memberships",
            &cookie,
            Some(&csrf),
            Some(json!({ "userId": disabled })),
            coop,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(membership_row(&test.pool, coop, disabled).await.is_none());

    test.cleanup().await;
}

/// Membership creation is a business operation: a suspended
/// cooperative refuses creation until restored.
#[tokio::test]
async fn suspended_cooperative_refuses_membership_creation() {
    let test = setup().await;
    let coop = bootstrap_coop(&test.pool).await;
    let padmin = create_user(&test.pool, "p6b3-padmin5").await;
    grant_platform(&test.pool, padmin, &["cooperatives.manage"]).await;
    let manager = create_user(&test.pool, "p6b3-mgr5").await;
    grant_cooperative(&test.pool, manager, &["memberships.manage", "roles.manage"]).await;
    let target = create_user(&test.pool, "p6b3-target5").await;

    let (pcookie, pcsrf, _) = login(&test.app, test.peer, "p6b3-padmin5").await;
    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop}/suspend"),
            &pcookie,
            Some(&pcsrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The manager's session dies with the business gate — re-login and
    // attempt creation against the suspended cooperative.
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b3-mgr5").await;
    let (status, body) = send(
        &test.app,
        scoped_request_builder(
            "POST",
            "/api/memberships",
            &cookie,
            Some(&csrf),
            Some(json!({ "userId": target })),
            coop,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "suspended cooperative must refuse membership creation: {body}"
    );
    assert!(membership_row(&test.pool, coop, target).await.is_none());

    // Restore; creation succeeds again.
    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop}/reactivate"),
            &pcookie,
            Some(&pcsrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b3-mgr5").await;
    let (status, body) = send(
        &test.app,
        scoped_request_builder(
            "POST",
            "/api/memberships",
            &cookie,
            Some(&csrf),
            Some(json!({ "userId": target })),
            coop,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    test.cleanup().await;
}

/// Independent memberships: one global user in two cooperatives; the
/// manager of B cannot act inside A; suspension in B leaves A intact;
/// role assignments stay inside their cooperative.
#[tokio::test]
async fn cross_cooperative_membership_isolation() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;
    let padmin = create_user(&test.pool, "p6b3-padmin6").await;
    grant_platform(&test.pool, padmin, &["cooperatives.manage"]).await;
    let (pcookie, pcsrf, _) = login(&test.app, test.peer, "p6b3-padmin6").await;

    // Second cooperative (scratch test DB — activation permitted here).
    let admin_b = create_user(&test.pool, "p6b3-adminB").await;
    let coop_b =
        activated_cooperative(&test.app, &pcookie, &pcsrf, "İkinci Kooperatif", admin_b).await;
    grant_in_cooperative(
        &test.pool,
        coop_b,
        admin_b,
        &["memberships.manage", "memberships.read"],
    )
    .await;

    // Shared identity: X is a member of both cooperatives.
    let shared = create_user(&test.pool, "p6b3-shared6").await;
    let mgr_a = create_user(&test.pool, "p6b3-mgrA").await;
    grant_cooperative(
        &test.pool,
        mgr_a,
        &["memberships.manage", "memberships.read"],
    )
    .await;

    // A's manager creates A-membership for the shared user.
    let (cookie_a, csrf_a, _) = login(&test.app, test.peer, "p6b3-mgrA").await;
    let (status, body) = send(
        &test.app,
        scoped_request_builder(
            "POST",
            "/api/memberships",
            &cookie_a,
            Some(&csrf_a),
            Some(json!({ "userId": shared })),
            coop_a,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // B's manager creates B-membership for the same user.
    let (cookie_b, csrf_b, _) = login(&test.app, test.peer, "p6b3-adminB").await;
    switch_cooperative(&test.app, &cookie_b, &csrf_b, coop_b).await;
    let (status, body) = send(
        &test.app,
        scoped_request_builder(
            "POST",
            "/api/memberships",
            &cookie_b,
            Some(&csrf_b),
            Some(json!({ "userId": shared })),
            coop_b,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Two independent rows.
    assert_eq!(
        membership_row(&test.pool, coop_a, shared)
            .await
            .map(|r| r.0),
        Some("active".into())
    );
    assert_eq!(
        membership_row(&test.pool, coop_b, shared)
            .await
            .map(|r| r.0),
        Some("active".into())
    );

    // B's manager cannot act inside A — the scope header is pinned to
    // B's membership; A is not theirs to manage.
    let (status, body) = send(
        &test.app,
        scoped_request_builder(
            "POST",
            &format!("/api/users/{shared}/membership-suspend"),
            &cookie_b,
            Some(&csrf_b),
            None,
            coop_a,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "B's manager must not touch A's membership: {body}"
    );
    let (status, _) = send(
        &test.app,
        scoped_request_builder("GET", "/api/memberships", &cookie_b, None, None, coop_a),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "B's manager must not list A's members"
    );

    // Assign a role to the shared user inside B only.
    let role_b: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, cooperative_id) VALUES ('B Özel Rol', $1) RETURNING id",
    )
    .bind(coop_b)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
         VALUES ($1, $2, $3)",
    )
    .bind(shared)
    .bind(role_b)
    .bind(coop_b)
    .execute(&test.pool)
    .await
    .unwrap();

    // A's view shows no B-role; B's view shows it.
    let (status, body) = send(
        &test.app,
        scoped_request_builder("GET", "/api/memberships", &cookie_a, None, None, coop_a),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let shared_in_a = body
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["userId"] == json!(shared))
        .expect("shared member listed in A")
        .clone();
    assert!(
        shared_in_a["roles"].as_array().unwrap().is_empty(),
        "B's role must not leak into A: {shared_in_a}"
    );

    let (status, body) = send(
        &test.app,
        scoped_request_builder("GET", "/api/memberships", &cookie_b, None, None, coop_b),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let shared_in_b = body
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["userId"] == json!(shared))
        .expect("shared member listed in B")
        .clone();
    assert_eq!(
        shared_in_b["roles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["B Özel Rol"]
    );

    // Suspend the shared membership in B — A's row is untouched.
    let (status, body) = send(
        &test.app,
        scoped_request_builder(
            "POST",
            &format!("/api/users/{shared}/membership-suspend"),
            &cookie_b,
            Some(&csrf_b),
            None,
            coop_b,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        membership_row(&test.pool, coop_b, shared)
            .await
            .map(|r| r.0),
        Some("suspended".into())
    );
    assert_eq!(
        membership_row(&test.pool, coop_a, shared)
            .await
            .map(|r| r.0),
        Some("active".into()),
        "B's suspension must not touch A's membership"
    );

    test.cleanup().await;
}

/// memberships.manage does not authorize platform lanes and cannot
/// carry platform semantics — a coop-scoped grant stays cooperative.
#[tokio::test]
async fn membership_manager_cannot_reach_platform_lanes() {
    let test = setup().await;
    let manager = create_user(&test.pool, "p6b3-mgr7").await;
    grant_cooperative(&test.pool, manager, &["memberships.manage"]).await;
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b3-mgr7").await;

    let (status, _) = send(
        &test.app,
        request_builder("GET", "/api/platform/cooperatives", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/platform/cooperatives",
            &cookie,
            Some(&csrf),
            Some(json!({ "name": "Sızıntı Kooperatifi" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    test.cleanup().await;
}

/// Last-administrator protection holds for memberships.manage holders:
/// ending the only administrator's membership is refused.
#[tokio::test]
async fn last_admin_protection_applies_to_memberships_manage() {
    let test = setup().await;
    let padmin = create_user(&test.pool, "p6b3-padmin8").await;
    grant_platform(&test.pool, padmin, &["cooperatives.manage"]).await;
    let (pcookie, pcsrf, _) = login(&test.app, test.peer, "p6b3-padmin8").await;

    let admin = create_user(&test.pool, "p6b3-admin8").await;
    let coop_b =
        activated_cooperative(&test.app, &pcookie, &pcsrf, "Korunan Kooperatif", admin).await;
    grant_in_cooperative(&test.pool, coop_b, admin, &["memberships.manage"]).await;

    // The only administrator cannot end their own membership path.
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b3-admin8").await;
    switch_cooperative(&test.app, &cookie, &csrf, coop_b).await;
    let (status, body) = send(
        &test.app,
        scoped_request_builder(
            "POST",
            &format!("/api/users/{admin}/membership-end"),
            &cookie,
            Some(&csrf),
            None,
            coop_b,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "lockout_prevented");
    assert_eq!(
        membership_row(&test.pool, coop_b, admin).await.map(|r| r.0),
        Some("active".into())
    );

    test.cleanup().await;
}
