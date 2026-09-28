//! Database-gated RBAC integration tests (STEP-003 §49–§57).
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (scripts/db-verify.sh and the
//! CI database job provision a clean database, including the migration
//! 0003 permission/role seeds). Deterministic: no wall-clock sleeps;
//! lockout concurrency proven through the advisory-lock guard.

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
const TEST_PASSWORD: &str = "rbac-test-parola-123";
/// The migration-0003 seeded administrative role (fixed UUID).
fn sysadmin_role_id() -> Uuid {
    Uuid::parse_str("00000000-0000-4000-8000-000000000001").expect("fixed seed uuid")
}

fn test_database_url() -> Option<String> {
    std::env::var("KOOPERATIF_TEST_DATABASE_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

struct TestApp {
    app: axum::Router,
    pool: PgPool,
    _peer: SocketAddr,
    /// Dedicated per-test database (dropped by `cleanup`); guarantees the
    /// GLOBAL last-administration-path invariant is observed in
    /// isolation — parallel tests can never pollute each other.
    database_name: String,
    admin_url: String,
}

impl TestApp {
    fn peer(&self) -> SocketAddr {
        self._peer
    }

    /// Drop the dedicated test database. Called at the end of happy-path
    /// tests; failed runs leak their database and db-verify.sh sweeps
    /// kooperatif_test_% leftovers on the next gate run.
    #[allow(dead_code)]
    async fn cleanup(&self) {
        let _ = sqlx::query(&format!(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}' AND pid <> pg_backend_pid()",
            self.database_name
        ))
        .execute(&self.pool)
        .await;
        self.pool.close().await;
        if let Ok(admin) = sqlx::PgPool::connect(&self.admin_url).await {
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

async fn setup() -> Option<TestApp> {
    let url = test_database_url()?;
    // Fresh, per-test database: replace the database segment of the URL.
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
    sqlx::query(&format!("CREATE DATABASE {database_name}"))
        .execute(&admin)
        .await
        .expect("test database created");
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
    let app = router(
        AppState {
            db: Some(pool.clone()),
            auth: Arc::new(runtime),
        },
        cors_layer(&[]),
    );
    let bytes = Uuid::new_v4();
    let octets = bytes.as_bytes();
    Some(TestApp {
        app,
        pool,
        database_name,
        admin_url,
        _peer: format!(
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

/// Create a user; optionally grant the seeded administrative role.
async fn create_user(pool: &PgPool, username: &str, sysadmin: bool) -> Uuid {
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).expect("test parameters"),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).expect("hash");
    let user = users::create_user(pool, username, "Test Kullanıcı", &hash)
        .await
        .expect("user created");
    if sysadmin {
        sqlx::query("INSERT INTO user_role_assignments (user_id, role_id) VALUES ($1, $2)")
            .bind(user.id)
            .bind(sysadmin_role_id())
            .execute(pool)
            .await
            .expect("role granted");
    }
    user.id
}

async fn sqlx_helper_user_id(pool: &PgPool, username: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE username = $1")
        .bind(username)
        .fetch_one(pool)
        .await
        .expect("user id")
}

/// Create an empty (permission-less) role; returns its id.
async fn create_role(pool: &PgPool, name: &str, permissions: &[&str]) -> Uuid {
    let id: Uuid = sqlx::query_scalar("INSERT INTO roles (name) VALUES ($1) RETURNING id")
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("role created");
    for key in permissions {
        sqlx::query(
            "INSERT INTO role_permissions (role_id, permission_id) \
             SELECT $1, id FROM permissions WHERE key = $2",
        )
        .bind(id)
        .bind(key)
        .execute(pool)
        .await
        .expect("permission granted");
    }
    id
}

async fn assign_role(pool: &PgPool, user_id: Uuid, role_id: Uuid) {
    sqlx::query("INSERT INTO user_role_assignments (user_id, role_id) VALUES ($1, $2)")
        .bind(user_id)
        .bind(role_id)
        .execute(pool)
        .await
        .expect("assignment");
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
    let status = response.status();
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
    let _ = status;
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

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

// ------------------------------------------------------------------
// §49: permission resolution
// ------------------------------------------------------------------

#[tokio::test]
async fn effective_permissions_follow_the_union_and_default_deny() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    // User with NO roles → denied everywhere.
    let plain = format!("plain.{}", Uuid::new_v4().simple());
    let plain_id = create_user(&test.pool, &plain, false).await;
    let permissions = kooperatif_server::auth::authz::effective_permissions(&test.pool, plain_id)
        .await
        .expect("resolution");
    assert!(permissions.is_empty(), "no roles → no permissions");

    // Role WITHOUT the permission → still denied.
    let viewer_role = create_role(
        &test.pool,
        &format!("Görüntüleyici {}", Uuid::new_v4().simple()),
        &["users.read"],
    )
    .await;
    assign_role(&test.pool, plain_id, viewer_role).await;
    let permissions = kooperatif_server::auth::authz::effective_permissions(&test.pool, plain_id)
        .await
        .expect("resolution");
    assert_eq!(permissions.len(), 1);
    assert!(permissions.contains("users.read"));
    assert!(!permissions.contains("roles.read"), "absent → denied");

    // A SECOND role contributes a union member.
    let second_role = create_role(
        &test.pool,
        &format!("Roller {}", Uuid::new_v4().simple()),
        &["roles.read"],
    )
    .await;
    assign_role(&test.pool, plain_id, second_role).await;
    let permissions = kooperatif_server::auth::authz::effective_permissions(&test.pool, plain_id)
        .await
        .expect("resolution");
    assert!(permissions.contains("users.read") && permissions.contains("roles.read"));

    // Removing the role removes its permissions (freshness at the
    // resolution layer).
    sqlx::query("DELETE FROM user_role_assignments WHERE user_id = $1 AND role_id = $2")
        .bind(plain_id)
        .bind(second_role)
        .execute(&test.pool)
        .await
        .expect("unassign");
    let permissions = kooperatif_server::auth::authz::effective_permissions(&test.pool, plain_id)
        .await
        .expect("resolution");
    assert!(
        !permissions.contains("roles.read"),
        "removed role → permission disappears"
    );

    // Removing a role_permission removes it for every holder.
    sqlx::query("DELETE FROM role_permissions WHERE role_id = $1")
        .bind(viewer_role)
        .execute(&test.pool)
        .await
        .expect("strip");
    let permissions = kooperatif_server::auth::authz::effective_permissions(&test.pool, plain_id)
        .await
        .expect("resolution");
    assert!(permissions.is_empty(), "stripped role → nothing granted");

    // Disabled roles contribute nothing (§10 invariant).
    assign_role(&test.pool, plain_id, sysadmin_role_id()).await;
    sqlx::query("UPDATE roles SET status = 'disabled' WHERE id = $1")
        .bind(sysadmin_role_id())
        .execute(&test.pool)
        .await
        .expect("disable");
    let permissions = kooperatif_server::auth::authz::effective_permissions(&test.pool, plain_id)
        .await
        .expect("resolution");
    assert!(permissions.is_empty(), "disabled role must grant nothing");
    sqlx::query("UPDATE roles SET status = 'active' WHERE id = $1")
        .bind(sysadmin_role_id())
        .execute(&test.pool)
        .await
        .expect("re-enable");
    let permissions = kooperatif_server::auth::authz::effective_permissions(&test.pool, plain_id)
        .await
        .expect("resolution");
    assert_eq!(permissions.len(), 4, "re-enabled role grants again");
}

// ------------------------------------------------------------------
// §50: server-side HTTP enforcement + §56 CSRF
// ------------------------------------------------------------------

#[tokio::test]
async fn rbac_endpoints_enforce_authentication_and_permission_server_side() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let plain_name = format!("http.plain.{}", Uuid::new_v4().simple());
    let _plain_id = create_user(&test.pool, &plain_name, false).await;
    let admin_name = format!("http.admin.{}", Uuid::new_v4().simple());
    let _admin_id = create_user(&test.pool, &admin_name, true).await;

    // Unauthenticated → 401 (never 403).
    let (status, body) = send(
        &test.app,
        request_builder("GET", "/api/roles", "gecersiz-token", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "authentication_required");

    // Authenticated WITHOUT roles.read → 403 permission_denied (§19).
    let (plain_cookie, plain_csrf, _) = login(&test.app, test.peer(), &plain_name).await;
    let (status, body) = send(
        &test.app,
        request_builder("GET", "/api/roles", &plain_cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "permission_denied");

    // Authenticated WITH permission → success.
    let (admin_cookie, admin_csrf, _) = login(&test.app, test.peer(), &admin_name).await;
    let (status, body) = send(
        &test.app,
        request_builder("GET", "/api/roles", &admin_cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.as_array().is_some());

    // Permission catalog readable with roles.read.
    let (status, catalog) = send(
        &test.app,
        request_builder("GET", "/api/permissions", &admin_cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(catalog.as_array().expect("catalog").len(), 4);

    // users.read gated likewise.
    let (status, _) = send(
        &test.app,
        request_builder("GET", "/api/users", &plain_cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &test.app,
        request_builder("GET", "/api/users", &admin_cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // CSRF (§56): mutation without token → 403 csrf_failed; with foreign
    // Origin → 403.
    let role_name = format!("CSRF Rol {}", Uuid::new_v4().simple());
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/roles",
            &admin_cookie,
            None,
            Some(json!({ "name": role_name })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "csrf_failed");

    let mut evil = request_builder(
        "POST",
        "/api/roles",
        &admin_cookie,
        Some(&admin_csrf),
        Some(json!({ "name": role_name })),
    );
    evil.headers_mut().insert(
        "origin",
        axum::http::HeaderValue::from_static("https://evil.example"),
    );
    let (status, _) = send(&test.app, evil).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Same-origin + token succeeds.
    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/roles",
            &admin_cookie,
            Some(&admin_csrf),
            Some(json!({ "name": role_name })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let _ = plain_csrf;
}

// ------------------------------------------------------------------
// §51: freshness through the HTTP surface (no re-login)
// ------------------------------------------------------------------

#[tokio::test]
async fn permission_changes_take_effect_immediately_without_relogin() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let name = format!("taze.{}", Uuid::new_v4().simple());
    let user_id = create_user(&test.pool, &name, false).await;
    let (cookie, csrf, _) = login(&test.app, test.peer(), &name).await;

    // Denied before grant…
    let (status, _) = send(
        &test.app,
        request_builder("GET", "/api/roles", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // …allowed after grant (same session, same cookie).
    assign_role(&test.pool, user_id, sysadmin_role_id()).await;
    let (status, _) = send(
        &test.app,
        request_builder("GET", "/api/roles", &cookie, None, None),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "grant must be effective immediately"
    );

    // …denied again after the role is removed.
    sqlx::query("DELETE FROM user_role_assignments WHERE user_id = $1 AND role_id = $2")
        .bind(user_id)
        .bind(sysadmin_role_id())
        .execute(&test.pool)
        .await
        .expect("unassign");
    let (status, _) = send(
        &test.app,
        request_builder("GET", "/api/roles", &cookie, None, None),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "revocation must be immediate"
    );

    // …and denied when the role is disabled while held.
    assign_role(&test.pool, user_id, sysadmin_role_id()).await;
    sqlx::query("UPDATE roles SET status = 'disabled' WHERE id = $1")
        .bind(sysadmin_role_id())
        .execute(&test.pool)
        .await
        .expect("disable");
    let (status, _) = send(
        &test.app,
        request_builder("GET", "/api/roles", &cookie, None, None),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "disabled role must stop granting"
    );
    sqlx::query("UPDATE roles SET status = 'active' WHERE id = $1")
        .bind(sysadmin_role_id())
        .execute(&test.pool)
        .await
        .expect("re-enable");
    let _ = csrf;

    // /me reflects the authorization context (§21).
    let (status, me) = send(
        &test.app,
        request_builder("GET", "/api/auth/me", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["permissions"].as_array().expect("permissions").len(), 4);
    assert!(!me["roles"].as_array().expect("roles").is_empty());
}

// ------------------------------------------------------------------
// §52: role management
// ------------------------------------------------------------------

#[tokio::test]
async fn role_lifecycle_management_semantics() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let admin_name = format!("rol.admin.{}", Uuid::new_v4().simple());
    let _admin_id = create_user(&test.pool, &admin_name, true).await;
    let (cookie, csrf, _) = login(&test.app, test.peer(), &admin_name).await;

    // Create.
    let unique = Uuid::new_v4().simple();
    let name = format!("Muhasebe {unique}");
    let (status, created) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/roles",
            &cookie,
            Some(&csrf),
            Some(json!({ "name": name, "description": "Deneme" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let role_id: Uuid = created["id"].as_str().expect("id").parse().expect("uuid");
    assert_eq!(created["status"], "active");

    // Duplicate normalized name rejected (409) — even with different case.
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/roles",
            &cookie,
            Some(&csrf),
            Some(json!({ "name": name.to_uppercase() })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "conflict");

    // Unknown permission key rejected (§26) — nothing applied.
    let (status, body) = send(
        &test.app,
        request_builder(
            "PUT",
            &format!("/api/roles/{role_id}/permissions"),
            &cookie,
            Some(&csrf),
            Some(json!({ "permissions": ["payments.reverse"] })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT p.key FROM role_permissions rp JOIN permissions p ON p.id = rp.permission_id WHERE rp.role_id = $1",
    )
    .bind(role_id)
    .fetch_all(&test.pool)
    .await
    .expect("keys");
    assert!(
        keys.is_empty(),
        "rejected request must apply nothing: {keys:?}"
    );

    // Set permissions (replace semantics) + idempotent repeat.
    for _ in 0..2 {
        let (status, _) = send(
            &test.app,
            request_builder(
                "PUT",
                &format!("/api/roles/{role_id}/permissions"),
                &cookie,
                Some(&csrf),
                Some(json!({ "permissions": ["users.read", "roles.read"] })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT p.key FROM role_permissions rp JOIN permissions p ON p.id = rp.permission_id WHERE rp.role_id = $1 ORDER BY p.key",
    )
    .bind(role_id)
    .fetch_all(&test.pool)
    .await
    .expect("keys");
    assert_eq!(
        keys,
        vec!["roles.read".to_string(), "users.read".to_string()]
    );

    // Rename (authorization identity unaffected — it is the id).
    let (status, updated) = send(
        &test.app,
        request_builder(
            "PATCH",
            &format!("/api/roles/{role_id}"),
            &cookie,
            Some(&csrf),
            Some(json!({ "name": format!("Muhasebe Yeni {unique}") })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["id"].as_str().expect("id"), role_id.to_string());

    // Disable / enable (idempotent) — this role is not the last admin path.
    for expected in [StatusCode::NO_CONTENT, StatusCode::NO_CONTENT] {
        let (status, _) = send(
            &test.app,
            request_builder(
                "POST",
                &format!("/api/roles/{role_id}/disable"),
                &cookie,
                Some(&csrf),
                None,
            ),
        )
        .await;
        assert_eq!(status, expected);
    }
    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/roles/{role_id}/enable"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Unknown role → 404.
    let (status, _) = send(
        &test.app,
        request_builder(
            "GET",
            &format!("/api/roles/{}", Uuid::new_v4()),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ------------------------------------------------------------------
// §53: user role assignment
// ------------------------------------------------------------------

#[tokio::test]
async fn user_role_assignment_semantics() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let admin_name = format!("atama.admin.{}", Uuid::new_v4().simple());
    let _admin_id = create_user(&test.pool, &admin_name, true).await;
    let (cookie, csrf, _) = login(&test.app, test.peer(), &admin_name).await;

    let target_name = format!("atama.hedef.{}", Uuid::new_v4().simple());
    let target_id = create_user(&test.pool, &target_name, false).await;

    let role_a = create_role(
        &test.pool,
        &format!("Rol A {}", Uuid::new_v4().simple()),
        &["users.read"],
    )
    .await;
    let disabled_role = create_role(
        &test.pool,
        &format!("Kapalı {}", Uuid::new_v4().simple()),
        &[],
    )
    .await;
    sqlx::query("UPDATE roles SET status = 'disabled' WHERE id = $1")
        .bind(disabled_role)
        .execute(&test.pool)
        .await
        .expect("disable");

    // Assign (replace-set). Repeat is idempotent (no duplicates — the
    // DB constraint also forbids them, §34).
    for _ in 0..2 {
        let (status, _) = send(
            &test.app,
            request_builder(
                "PUT",
                &format!("/api/users/{target_id}/roles"),
                &cookie,
                Some(&csrf),
                Some(json!({ "roleIds": [role_a.to_string()] })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_role_assignments WHERE user_id = $1 AND role_id = $2",
    )
    .bind(target_id)
    .bind(role_a)
    .fetch_one(&test.pool)
    .await
    .expect("count");
    assert_eq!(count, 1, "idempotent replace-set creates exactly one row");

    // Unknown role id rejected.
    let (status, _) = send(
        &test.app,
        request_builder(
            "PUT",
            &format!("/api/users/{target_id}/roles"),
            &cookie,
            Some(&csrf),
            Some(json!({ "roleIds": [Uuid::new_v4().to_string()] })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Disabled role assignment rejected.
    let (status, body) = send(
        &test.app,
        request_builder(
            "PUT",
            &format!("/api/users/{target_id}/roles"),
            &cookie,
            Some(&csrf),
            Some(json!({ "roleIds": [disabled_role.to_string()] })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // Cannot modify without users.manage (server-side).
    let limited_name = format!("atama.sinirli.{}", Uuid::new_v4().simple());
    let _limited_id = create_user(&test.pool, &limited_name, false).await;
    let limited_role = create_role(
        &test.pool,
        &format!("Rol B {}", Uuid::new_v4().simple()),
        &["users.read", "roles.read"],
    )
    .await;
    assign_role(
        &test.pool,
        sqlx_helper_user_id(&test.pool, &limited_name).await,
        limited_role,
    )
    .await;
    let (limited_cookie, limited_csrf, _) = login(&test.app, test.peer(), &limited_name).await;
    let (status, body) = send(
        &test.app,
        request_builder(
            "PUT",
            &format!("/api/users/{target_id}/roles"),
            &limited_cookie,
            Some(&limited_csrf),
            Some(json!({ "roleIds": [] })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "permission_denied");

    // GET assignments visible with users.read.
    let (status, roles) = send(
        &test.app,
        request_builder(
            "GET",
            &format!("/api/users/{target_id}/roles"),
            &limited_cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(roles.as_array().expect("roles").len(), 1);
}

// ------------------------------------------------------------------
// §54: last-administration-path protection (incl. concurrency)
// ------------------------------------------------------------------

#[tokio::test]
async fn last_administration_path_is_protected() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let admin_name = format!("kilit.admin.{}", Uuid::new_v4().simple());
    let admin_id = create_user(&test.pool, &admin_name, true).await;
    let (cookie, csrf, _) = login(&test.app, test.peer(), &admin_name).await;

    // The seeded admin role currently holds roles.manage AND the admin
    // holds it: removing the admin's roles would leave zero paths.
    let (status, body) = send(
        &test.app,
        request_builder(
            "PUT",
            &format!("/api/users/{admin_id}/roles"),
            &cookie,
            Some(&csrf),
            Some(json!({ "roleIds": [] })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "lockout_prevented");
    // Nothing changed.
    let still: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_role_assignments WHERE user_id = $1")
            .bind(admin_id)
            .fetch_one(&test.pool)
            .await
            .expect("count");
    assert_eq!(still, 1);

    // Disabling the last administrative role is refused.
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/roles/{}/disable", sysadmin_role_id()),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // Stripping roles.manage from the last administrative role refused.
    let (status, body) = send(
        &test.app,
        request_builder(
            "PUT",
            &format!("/api/roles/{}/permissions", sysadmin_role_id()),
            &cookie,
            Some(&csrf),
            Some(json!({ "permissions": ["users.read"] })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT p.key FROM role_permissions rp JOIN permissions p ON p.id = rp.permission_id WHERE rp.role_id = $1 ORDER BY p.key",
    )
    .bind(sysadmin_role_id())
    .fetch_all(&test.pool)
    .await
    .expect("keys");
    assert_eq!(keys.len(), 4, "permission strip must be rolled back");

    // With a SECOND administration path, the same mutations succeed.
    let second_name = format!("kilit.ikinci.{}", Uuid::new_v4().simple());
    let second_id = create_user(&test.pool, &second_name, true).await;
    let _ = second_id;

    // Now removing the first admin's roles is allowed (second path remains).
    let (status, _) = send(
        &test.app,
        request_builder(
            "PUT",
            &format!("/api/users/{admin_id}/roles"),
            &cookie,
            Some(&csrf),
            Some(json!({ "roleIds": [] })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // And granting it back works through the second admin (freshness of
    // administration itself).
    let (second_cookie, second_csrf, _) = login(&test.app, test.peer(), &second_name).await;
    let (status, _) = send(
        &test.app,
        request_builder(
            "PUT",
            &format!("/api/users/{admin_id}/roles"),
            &second_cookie,
            Some(&second_csrf),
            Some(json!({ "roleIds": [sysadmin_role_id().to_string()] })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn concurrent_lockout_mutations_leave_one_administration_path() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let first_name = format!("eszamanli.bir.{}", Uuid::new_v4().simple());
    let second_name = format!("eszamanli.iki.{}", Uuid::new_v4().simple());
    let first_id = create_user(&test.pool, &first_name, true).await;
    let second_id = create_user(&test.pool, &second_name, true).await;

    let (first_cookie, first_csrf, _) = login(&test.app, test.peer(), &first_name).await;
    let (second_cookie, second_csrf, _) = login(&test.app, test.peer(), &second_name).await;

    let app_one = test.app.clone();
    let app_two = test.app.clone();
    let first_request = request_builder(
        "PUT",
        &format!("/api/users/{first_id}/roles"),
        &first_cookie,
        Some(&first_csrf),
        Some(json!({ "roleIds": [] })),
    );
    let second_request = request_builder(
        "PUT",
        &format!("/api/users/{second_id}/roles"),
        &second_cookie,
        Some(&second_csrf),
        Some(json!({ "roleIds": [] })),
    );

    // Two simultaneous self-removals of the only two administration
    // paths: the advisory-lock guard serializes them; exactly one
    // commits, the other observes zero remaining paths and aborts.
    let (first_result, second_result) = tokio::join!(
        async {
            app_one
                .oneshot(first_request)
                .await
                .expect("infallible")
                .status()
        },
        async {
            app_two
                .oneshot(second_request)
                .await
                .expect("infallible")
                .status()
        },
    );
    let mut statuses = [first_result, second_result];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::NO_CONTENT, StatusCode::CONFLICT],
        "exactly one self-removal must succeed"
    );

    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT ura.user_id) \
         FROM user_role_assignments ura \
         JOIN roles r ON r.id = ura.role_id AND r.status = 'active' \
         JOIN role_permissions rp ON rp.role_id = r.id \
         JOIN permissions p ON p.id = rp.permission_id AND p.key = 'roles.manage'",
    )
    .fetch_one(&test.pool)
    .await
    .expect("count");
    assert_eq!(remaining, 1, "exactly one administration path survives");
}

// ------------------------------------------------------------------
// §55/§57: audit events and database constraints
// ------------------------------------------------------------------

#[tokio::test]
async fn rbac_mutations_are_audited_with_actor() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };
    let admin_name = format!("denetim.admin.{}", Uuid::new_v4().simple());
    let admin_id = create_user(&test.pool, &admin_name, true).await;
    let (cookie, csrf, _) = login(&test.app, test.peer(), &admin_name).await;

    let target_name = format!("denetim.hedef.{}", Uuid::new_v4().simple());
    let target_id = create_user(&test.pool, &target_name, false).await;

    let unique = Uuid::new_v4().simple();
    let (status, created) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/roles",
            &cookie,
            Some(&csrf),
            Some(json!({ "name": format!("Denetim {unique}") })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let new_role: Uuid = created["id"].as_str().expect("id").parse().expect("uuid");

    send(
        &test.app,
        request_builder(
            "PUT",
            &format!("/api/roles/{new_role}/permissions"),
            &cookie,
            Some(&csrf),
            Some(json!({ "permissions": ["users.read"] })),
        ),
    )
    .await;
    send(
        &test.app,
        request_builder(
            "PUT",
            &format!("/api/users/{target_id}/roles"),
            &cookie,
            Some(&csrf),
            Some(json!({ "roleIds": [new_role.to_string()] })),
        ),
    )
    .await;
    // Lockout rejection is auditable too.
    send(
        &test.app,
        request_builder(
            "PUT",
            &format!("/api/users/{admin_id}/roles"),
            &cookie,
            Some(&csrf),
            Some(json!({ "roleIds": [] })),
        ),
    )
    .await;

    let rows: Vec<(String, Option<uuid::Uuid>)> = sqlx::query_as(
        "SELECT event_type, user_id FROM security_events \
         WHERE event_type LIKE 'role%' OR event_type LIKE 'lockout%' OR event_type LIKE 'bootstrap%' \
         ORDER BY occurred_at",
    )
    .fetch_all(&test.pool)
    .await
    .expect("events");
    let events: Vec<String> = rows.iter().map(|(event, _)| event.clone()).collect();
    let actors: Vec<Option<uuid::Uuid>> = rows.iter().map(|(_, actor)| *actor).collect();
    for expected in [
        "role_created",
        "role_permissions_changed",
        "role_assigned",
        "lockout_prevented",
    ] {
        assert!(
            events.iter().any(|e| e == expected),
            "missing {expected}: {events:?}"
        );
    }
    assert!(
        actors.iter().all(|actor| actor.as_ref() == Some(&admin_id)),
        "actor must be the mutating admin: {actors:?}"
    );
}

#[tokio::test]
async fn database_constraints_enforce_rbac_integrity() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED: KOOPERATIF_TEST_DATABASE_URL is not set");
        return;
    };

    // permission.key unique — the catalog is a contract.
    let duplicate_permission =
        sqlx::query("INSERT INTO permissions (key, name) VALUES ('users.read', 'Sahte')")
            .execute(&test.pool)
            .await;
    assert!(
        duplicate_permission.is_err(),
        "duplicate permission key must fail"
    );

    // Role normalized-name uniqueness (case-insensitive).
    let unique = Uuid::new_v4().simple();
    sqlx::query("INSERT INTO roles (name) VALUES ($1)")
        .bind(format!("Benzersiz {unique}"))
        .execute(&test.pool)
        .await
        .expect("first role");
    let duplicate_role = sqlx::query("INSERT INTO roles (name) VALUES ($1)")
        .bind(format!("BENZERSİZ {unique}"))
        .execute(&test.pool)
        .await;
    assert!(
        duplicate_role.is_err(),
        "duplicate normalized role name must fail"
    );

    // (role_id, permission_id) unique.
    let role_id: Uuid = sqlx::query_scalar("SELECT id FROM roles WHERE name = $1")
        .bind(format!("Benzersiz {unique}"))
        .fetch_one(&test.pool)
        .await
        .expect("role");
    let permission_id: Uuid =
        sqlx::query_scalar("SELECT id FROM permissions WHERE key = 'users.read'")
            .fetch_one(&test.pool)
            .await
            .expect("permission");
    sqlx::query("INSERT INTO role_permissions (role_id, permission_id) VALUES ($1, $2)")
        .bind(role_id)
        .bind(permission_id)
        .execute(&test.pool)
        .await
        .expect("first grant");
    let duplicate_grant =
        sqlx::query("INSERT INTO role_permissions (role_id, permission_id) VALUES ($1, $2)")
            .bind(role_id)
            .bind(permission_id)
            .execute(&test.pool)
            .await;
    assert!(
        duplicate_grant.is_err(),
        "duplicate role permission must fail"
    );

    // (user_id, role_id) unique + FK integrity.
    let user_name = format!("kisit.{}", Uuid::new_v4().simple());
    let user_id = create_user(&test.pool, &user_name, false).await;
    sqlx::query("INSERT INTO user_role_assignments (user_id, role_id) VALUES ($1, $2)")
        .bind(user_id)
        .bind(role_id)
        .execute(&test.pool)
        .await
        .expect("assignment");
    let duplicate_assignment =
        sqlx::query("INSERT INTO user_role_assignments (user_id, role_id) VALUES ($1, $2)")
            .bind(user_id)
            .bind(role_id)
            .execute(&test.pool)
            .await;
    assert!(
        duplicate_assignment.is_err(),
        "duplicate assignment must fail"
    );

    let orphan_assignment =
        sqlx::query("INSERT INTO user_role_assignments (user_id, role_id) VALUES ($1, $2)")
            .bind(Uuid::new_v4())
            .bind(role_id)
            .execute(&test.pool)
            .await;
    assert!(
        orphan_assignment.is_err(),
        "FK must reject orphan assignments"
    );

    let orphan_permission =
        sqlx::query("INSERT INTO role_permissions (role_id, permission_id) VALUES ($1, $2)")
            .bind(role_id)
            .bind(Uuid::new_v4())
            .execute(&test.pool)
            .await;
    assert!(
        orphan_permission.is_err(),
        "FK must reject unknown permission"
    );
}
