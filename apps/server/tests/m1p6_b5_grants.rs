//! M1-P6 B5 Part A — System Administrator membership grants (0072).
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database;
//! scratch cooperatives only — no real second cooperative is activated).
//!
//! Under test:
//! - The `system-administrator` template and every role name-matched to
//!   it (cooperative clones AND the NULL-scope legacy role that administers
//!   the bootstrap cooperative) carry memberships.read + memberships.manage.
//! - Activation clones inherit both keys; a reused eponymous admin role
//!   is forward-synced to the template.
//! - Least privilege: no other role or template gains memberships.manage;
//!   no cooperative-scoped role ever holds a platform key.
//! - End-to-end: the cloned administrator can actually create and list
//!   memberships inside its cooperative via the session-scoped API.

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

const TEST_PASSWORD: &str = "p6b5-test-parola-123";
const ORIGIN: &str = "http://localhost:5173";
const APP_ROLE: &str = "kooperatif_app";
/// Restricted-principal password: `KOOPERATIF_APP_TEST_PASSWORD`
/// overrides for CI; otherwise the documented local dev password.
fn app_password() -> String {
    app_db::test_app_role_password()
}
const ADMIN_TEMPLATE: &str = "system-administrator";
const LEGACY_ADMIN_ROLE: &str = "00000000-0000-4000-8000-000000000001";

fn test_database_url() -> String {
    let value = std::env::var("KOOPERATIF_TEST_DATABASE_URL").unwrap_or_else(|_| {
        panic!("KOOPERATIF_TEST_DATABASE_URL is not set — DB-gated tests cannot silently pass")
    });
    let trimmed = value.trim().to_string();
    assert!(!trimmed.is_empty(), "KOOPERATIF_TEST_DATABASE_URL is empty");
    trimmed
}

struct TestApp {
    _claim: app_db::TestDbClaim,
    /// Router bound ONLY to the restricted runtime principal — the
    /// sealed-context permission checks are what these tests exercise.
    app: axum::Router,
    /// Owner pool for fixtures/assertions only.
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
        .map(|base| (base.0.to_string(), format!("{}/postgres", base.0)))
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
    users::create_user(pool, username, "B5 Test", &hash, TEST_PASSWORD)
        .await
        .expect("user")
        .id
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
        builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("valid")
    } else {
        builder.body(Body::empty()).expect("valid")
    }
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

/// Permission keys of a role row (owner-lane catalog assertion).
async fn role_permission_keys(pool: &PgPool, role: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT p.key FROM role_permissions rp \
         JOIN permissions p ON p.id = rp.permission_id \
         WHERE rp.role_id = $1 ORDER BY p.key",
    )
    .bind(role)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// Create a cooperative in `provisioning` via the platform lane; returns
/// (platform_admin_cookie, csrf, cooperative id).
async fn provision_cooperative(test: &TestApp, tag: &str) -> (String, String, Uuid) {
    let padmin = create_user(&test.pool, &format!("p6b5-padmin-{tag}")).await;
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, cooperative_id) VALUES ($1, NULL) RETURNING id",
    )
    .bind(format!("plat-b5-{padmin}"))
    .fetch_one(&test.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission_id) \
         SELECT $1, id FROM permissions WHERE key = 'cooperatives.manage'",
    )
    .bind(role)
    .execute(&test.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
         VALUES ($1, $2, NULL)",
    )
    .bind(padmin)
    .bind(role)
    .execute(&test.pool)
    .await
    .unwrap();
    let (cookie, csrf) = login(&test.app, test.peer, &format!("p6b5-padmin-{tag}")).await;
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/platform/cooperatives",
            &cookie,
            Some(&csrf),
            Some(json!({ "name": format!("B5 Kooperatif {tag}") })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let coop: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    (cookie, csrf, coop)
}

// ── Grant coverage ─────────────────────────────────────────────────

/// The template and the NULL-scope legacy admin role both carry the
/// membership pair after 0072.
#[tokio::test]
async fn template_and_legacy_admin_role_carry_both_keys() {
    let test = setup().await;

    let missing: Vec<String> = sqlx::query_scalar(
        "SELECT p.key FROM permissions p \
         WHERE p.key IN ('memberships.read','memberships.manage') \
           AND NOT EXISTS ( \
               SELECT 1 FROM system_role_template_permissions tp \
               JOIN system_role_templates t ON t.id = tp.template_id \
               WHERE t.key = $1 AND tp.permission_id = p.id)",
    )
    .bind(ADMIN_TEMPLATE)
    .fetch_all(&test.pool)
    .await
    .unwrap();
    assert!(missing.is_empty(), "template missing: {missing:?}");

    let legacy: Uuid = LEGACY_ADMIN_ROLE.parse().unwrap();
    let keys = role_permission_keys(&test.pool, legacy).await;
    for key in ["memberships.read", "memberships.manage"] {
        assert!(
            keys.contains(&key.to_string()),
            "legacy admin missing {key}"
        );
    }

    test.cleanup().await;
}

/// A newly activated cooperative's cloned administrator receives both
/// keys and can actually manage memberships through the session API.
#[tokio::test]
async fn activated_admin_role_gets_keys_and_manages_memberships() {
    let test = setup().await;
    let (pcookie, pcsrf, coop_b) = provision_cooperative(&test, "act").await;
    let admin_b = create_user(&test.pool, "p6b5-adminb-act").await;
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

    // The cloned cooperative-scoped Sistem Yöneticisi carries both keys.
    let role: Uuid = sqlx::query_scalar(
        "SELECT id FROM roles WHERE cooperative_id = $1 AND lower(name) = 'sistem yöneticisi'",
    )
    .bind(coop_b)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    let keys = role_permission_keys(&test.pool, role).await;
    for key in ["memberships.read", "memberships.manage"] {
        assert!(keys.contains(&key.to_string()), "clone missing {key}");
    }

    // End-to-end: the admin switches into B, then creates + lists a
    // membership through the approved APIs.
    let member = create_user(&test.pool, "p6b5-member-act").await;
    let (cookie, csrf) = login(&test.app, test.peer, "p6b5-adminb-act").await;
    assert_eq!(
        switch_cooperative(&test.app, &cookie, &csrf, coop_b).await,
        StatusCode::NO_CONTENT
    );
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/memberships",
            &cookie,
            Some(&csrf),
            Some(json!({ "userId": member })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = send(
        &test.app,
        request_builder("GET", "/api/memberships", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.to_string().contains(&member.to_string()));

    test.cleanup().await;
}

/// An eponymous admin role that predates B5 (roles.manage only) is
/// forward-synced to the template on reuse — never silently adopted and
/// never left behind the approved baseline.
#[tokio::test]
async fn reused_admin_role_is_forward_synced() {
    let test = setup().await;
    let (pcookie, pcsrf, coop_b) = provision_cooperative(&test, "reuse").await;
    let admin_b = create_user(&test.pool, "p6b5-adminb-reuse").await;

    // Pre-create the eponymous cooperative role with only roles.manage —
    // exactly the reuse-qualifying pre-B5 shape.
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, cooperative_id) \
         VALUES ('Sistem Yöneticisi', $1) RETURNING id",
    )
    .bind(coop_b)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission_id) \
         SELECT $1, id FROM permissions WHERE key = 'roles.manage'",
    )
    .bind(role)
    .execute(&test.pool)
    .await
    .unwrap();

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

    let keys = role_permission_keys(&test.pool, role).await;
    for key in ["memberships.read", "memberships.manage"] {
        assert!(
            keys.contains(&key.to_string()),
            "reused admin role must be forward-synced, missing {key}"
        );
    }

    test.cleanup().await;
}

/// Least privilege: unrelated roles and templates never gain the
/// membership keys, and no cooperative role holds a platform key.
#[tokio::test]
async fn no_other_role_receives_membership_permissions() {
    let test = setup().await;
    let (pcookie, pcsrf, coop_b) = provision_cooperative(&test, "least").await;
    let admin_b = create_user(&test.pool, "p6b5-adminb-least").await;
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

    // A custom cooperative role created after activation: no membership
    // keys appear automatically.
    let (cookie, csrf) = login(&test.app, test.peer, "p6b5-adminb-least").await;
    assert_eq!(
        switch_cooperative(&test.app, &cookie, &csrf, coop_b).await,
        StatusCode::NO_CONTENT
    );
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/roles",
            &cookie,
            Some(&csrf),
            Some(json!({ "name": "Veznedar", "description": "kasiyer" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let custom: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    let keys = role_permission_keys(&test.pool, custom).await;
    assert!(!keys.iter().any(|k| k.starts_with("memberships.")));

    // Catalog-wide invariant: memberships.manage lives ONLY on
    // name-matched Sistem Yöneticisi roles (any scope) and the
    // system-administrator template.
    let stray: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_permissions rp \
         JOIN permissions p ON p.id = rp.permission_id \
         JOIN roles r ON r.id = rp.role_id \
         WHERE p.key = 'memberships.manage' \
           AND lower(r.name) <> 'sistem yöneticisi'",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(stray, 0, "memberships.manage leaked outside admin roles");

    let stray_tpl: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM system_role_template_permissions tp \
         JOIN permissions p ON p.id = tp.permission_id \
         JOIN system_role_templates t ON t.id = tp.template_id \
         WHERE p.key = 'memberships.manage' AND t.key <> $1",
    )
    .bind(ADMIN_TEMPLATE)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(stray_tpl, 0);

    // Platform boundary: no cooperative role holds cooperatives.manage.
    let platform_leak: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_permissions rp \
         JOIN permissions p ON p.id = rp.permission_id \
         JOIN roles r ON r.id = rp.role_id \
         WHERE p.key = 'cooperatives.manage' AND r.cooperative_id IS NOT NULL",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(platform_leak, 0);

    test.cleanup().await;
}

/// Bootstrap compatibility: a user holding the NULL-scope legacy
/// Sistem Yöneticisi role manages memberships in the bootstrap
/// cooperative through the dedicated key (not the users.manage lane).
#[tokio::test]
async fn bootstrap_admin_manages_memberships_via_admin_role() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;
    let admin = create_user(&test.pool, "p6b5-admin-boot").await;
    let member = create_user(&test.pool, "p6b5-member-boot").await;

    // Bootstrap cooperative membership + NULL-scope legacy admin URA.
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id) \
         VALUES ($1, $2)",
    )
    .bind(coop_a)
    .bind(admin)
    .execute(&test.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
         VALUES ($1, $2, NULL)",
    )
    .bind(admin)
    .bind(LEGACY_ADMIN_ROLE.parse::<Uuid>().unwrap())
    .execute(&test.pool)
    .await
    .unwrap();

    let (cookie, csrf) = login(&test.app, test.peer, "p6b5-admin-boot").await;
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/memberships",
            &cookie,
            Some(&csrf),
            Some(json!({ "userId": member })),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "bootstrap admin must manage memberships via memberships.manage: {body}"
    );
    let (status, body) = send(
        &test.app,
        request_builder("GET", "/api/memberships", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.to_string().contains(&member.to_string()));

    test.cleanup().await;
}
