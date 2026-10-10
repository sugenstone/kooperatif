//! M1-P1-006 (P1-c2a) — cooperative-scoped RBAC API tests.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same harness as tests/rbac.rs). These tests run the REAL router —
//! the P0 business-membership gate included — and prove that migrated
//! RBAC endpoints (all reads + create/update/enable) resolve rows only
//! within the verified bootstrap scope:
//!   - bootstrap-attributed = roles/assignments owned by bootstrap OR
//!     NULL (verified legacy attribution); another cooperative's rows
//!     answer 404, never leak;
//!   - new roles carry cooperative_id — no new NULL writes;
//!   - audit events are cooperative-attributed;
//!   - B-scoped actors get nothing: not bootstrap members → 403 at
//!     extraction/gate;
//!   - deferred lockout-sensitive mutations (disable_role, PUT
//!     permissions, disable_user, PUT user-roles) keep the global
//!     guard untouched (P1-c2b).

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

const TEST_PASSWORD: &str = "p1c2a-test-parola-123";
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
    users::create_user(pool, username, "P1c2a Test", &hash)
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
        .header("user-agent", "Mozilla/5.0 P1c2a")
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

const ADMIN_PERMS: &[&str] = &["roles.read", "roles.manage", "users.read", "users.manage"];

/// Bootstrap-scoped administrator fixture: user created BEFORE
/// bootstrap (auto-member) with a bootstrap-owned admin role.
async fn bootstrap_admin(pool: &PgPool, username: &str) -> (Uuid, Uuid) {
    let user = create_user(pool, username).await;
    let coop = bootstrap(pool).await;
    grant_role(
        pool,
        user,
        &format!("admin-{username}"),
        Some(coop),
        ADMIN_PERMS,
    )
    .await;
    (user, coop)
}

#[tokio::test]
async fn scoped_admin_crud_and_no_new_null_writes() {
    let Some(test) = setup().await else { return };
    let (_, coop) = bootstrap_admin(&test.pool, "a_admin").await;
    let (cookie, csrf) = login(&test.app, test.peer, "a_admin").await;
    let coop_str = coop.to_string();

    // Create → carries the bootstrap cooperative_id, never NULL.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/roles",
        &cookie,
        Some(&csrf),
        Some(&coop_str),
        Some(json!({ "name": "Muhasebe Ekibi", "description": "deneme" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let role_id: Uuid = serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let written_coop: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM roles WHERE id = $1")
            .bind(role_id)
            .fetch_one(&test.pool)
            .await
            .expect("role row");
    assert_eq!(written_coop, Some(coop), "new role must carry the scope");

    // Audit attribution: cooperative_id recorded on the event.
    let event_coop: Option<Uuid> = sqlx::query_scalar(
        "SELECT cooperative_id FROM security_events \
         WHERE event_type = 'role_created' ORDER BY occurred_at DESC LIMIT 1",
    )
    .fetch_one(&test.pool)
    .await
    .expect("audit row");
    assert_eq!(event_coop, Some(coop), "RBAC audit must be attributed");

    // Update + enable in-scope.
    assert_eq!(
        request_json(
            &test,
            Method::PATCH,
            &format!("/api/roles/{role_id}"),
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "description": "guncellendi" })),
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request_json(
            &test,
            Method::POST,
            &format!("/api/roles/{role_id}/enable"),
            &cookie,
            Some(&csrf),
            None,
            None,
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );

    test.cleanup().await;
}

#[tokio::test]
async fn cross_cooperative_role_is_invisible() {
    let Some(test) = setup().await else { return };
    let (_, coop) = bootstrap_admin(&test.pool, "a_admin2").await;
    let (cookie, csrf) = login(&test.app, test.peer, "a_admin2").await;

    // A role owned by cooperative B — written directly (API never
    // writes foreign-scope rows).
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;
    let b_role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, cooperative_id) VALUES ('B Rolu', $1) RETURNING id",
    )
    .bind(coop_b)
    .fetch_one(&test.pool)
    .await
    .expect("b role");

    // List: B role absent; bootstrap + NULL rows present.
    let (status, body) =
        request_json(&test, Method::GET, "/api/roles", &cookie, None, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains(&b_role.to_string()), "B role leaked: {body}");

    // Direct access: 404 for every verb — existence never leaks.
    let path = format!("/api/roles/{b_role}");
    for (method, path, csrf_needed) in [
        (Method::GET, path.clone(), false),
        (Method::PATCH, path.clone(), true),
        (Method::POST, format!("{path}/enable"), true),
    ] {
        let (status, _) = request_json(
            &test,
            method,
            &path,
            &cookie,
            if csrf_needed { Some(&csrf) } else { None },
            None,
            if csrf_needed {
                Some(json!({ "description": "x" }))
            } else {
                None
            },
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path} must 404");
    }
    // Permission read on a foreign role → 404.
    assert_eq!(
        request_json(
            &test,
            Method::GET,
            &format!("/api/roles/{b_role}/permissions"),
            &cookie,
            None,
            None,
            None,
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    let _ = coop;
    test.cleanup().await;
}

#[tokio::test]
async fn b_admin_gets_nothing_in_bootstrap_scope() {
    let Some(test) = setup().await else { return };
    bootstrap(&test.pool).await;
    // Cooperative B administrator — full B-scoped admin role.
    let b_admin = create_user(&test.pool, "b_admin").await;
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;
    repo::add_member(&test.pool, "B Kooperatifi", b_admin, None)
        .await
        .expect("B membership");
    grant_role(
        &test.pool,
        b_admin,
        "b-admin-rol",
        Some(coop_b),
        ADMIN_PERMS,
    )
    .await;
    let (cookie, _csrf) = login(&test.app, test.peer, "b_admin").await;
    let coop_b_str = coop_b.to_string();

    // Headerless → pinned to bootstrap → not a member → 403.
    assert_eq!(
        request_json(&test, Method::GET, "/api/roles", &cookie, None, None, None,)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    // Explicit B header → non-bootstrap → 403.
    assert_eq!(
        request_json(
            &test,
            Method::GET,
            "/api/roles",
            &cookie,
            None,
            Some(&coop_b_str),
            None,
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );

    test.cleanup().await;
}

#[tokio::test]
async fn membership_and_header_rules_apply_to_rbac() {
    let Some(test) = setup().await else { return };
    let (admin, _) = bootstrap_admin(&test.pool, "a_admin3").await;
    let (cookie, _) = login(&test.app, test.peer, "a_admin3").await;

    // Suspended membership → 403.
    sqlx::query("UPDATE cooperative_memberships SET status='suspended' WHERE user_id=$1")
        .bind(admin)
        .execute(&test.pool)
        .await
        .expect("suspend");
    assert_eq!(
        request_json(&test, Method::GET, "/api/roles", &cookie, None, None, None,)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    // Ended (revoked) membership → 403.
    sqlx::query("UPDATE cooperative_memberships SET status='ended' WHERE user_id=$1")
        .bind(admin)
        .execute(&test.pool)
        .await
        .expect("end");
    assert_eq!(
        request_json(&test, Method::GET, "/api/roles", &cookie, None, None, None,)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    sqlx::query("UPDATE cooperative_memberships SET status='active' WHERE user_id=$1")
        .bind(admin)
        .execute(&test.pool)
        .await
        .expect("reactivate");

    // Malformed header → 400; unknown UUID → 403.
    assert_eq!(
        request_json(
            &test,
            Method::GET,
            "/api/roles",
            &cookie,
            None,
            Some("bozuk-uuid"),
            None,
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let unknown = Uuid::new_v4().to_string();
    assert_eq!(
        request_json(
            &test,
            Method::GET,
            "/api/roles",
            &cookie,
            None,
            Some(&unknown),
            None,
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );

    // Bootstrap member WITHOUT any role → membership passes, permission
    // denied (403).
    let plain = create_user(&test.pool, "izinsiz_uye").await;
    repo::add_member(&test.pool, "Ana Kooperatif", plain, None)
        .await
        .expect("membership");
    let (cookie2, _) = login(&test.app, test.peer, "izinsiz_uye").await;
    assert_eq!(
        request_json(&test, Method::GET, "/api/roles", &cookie2, None, None, None,)
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    test.cleanup().await;
}

#[tokio::test]
async fn null_legacy_roles_stay_manageable_in_bootstrap() {
    let Some(test) = setup().await else { return };
    bootstrap_admin(&test.pool, "a_admin4").await;
    let (cookie, csrf) = login(&test.app, test.peer, "a_admin4").await;

    // Legacy NULL role + NULL permission row.
    let null_role: Uuid =
        sqlx::query_scalar("INSERT INTO roles (name) VALUES ('Eski Rol') RETURNING id")
            .fetch_one(&test.pool)
            .await
            .expect("null role");
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission_id) \
         SELECT $1, id FROM permissions WHERE key = 'shares.read'",
    )
    .bind(null_role)
    .execute(&test.pool)
    .await
    .expect("perm");

    // Visible in list and direct read.
    let (status, body) =
        request_json(&test, Method::GET, "/api/roles", &cookie, None, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(&null_role.to_string()));
    assert_eq!(
        request_json(
            &test,
            Method::GET,
            &format!("/api/roles/{null_role}/permissions"),
            &cookie,
            None,
            None,
            None,
        )
        .await
        .0,
        StatusCode::OK
    );
    // Mutations allowed under verified attribution.
    assert_eq!(
        request_json(
            &test,
            Method::PATCH,
            &format!("/api/roles/{null_role}"),
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "description": "legacy" })),
        )
        .await
        .0,
        StatusCode::OK
    );

    test.cleanup().await;
}

#[tokio::test]
async fn user_listing_and_roles_are_scope_limited() {
    let Some(test) = setup().await else { return };
    let (admin, coop) = bootstrap_admin(&test.pool, "a_admin5").await;
    let (cookie, _) = login(&test.app, test.peer, "a_admin5").await;

    // A member holding BOTH a NULL role and a B-scoped role: in the
    // bootstrap view only the NULL-attributed role appears.
    let member = create_user(&test.pool, "karma_uye").await;
    repo::add_member(&test.pool, "Ana Kooperatif", member, None)
        .await
        .expect("bootstrap membership");
    let null_role = grant_role(&test.pool, member, "karma-null", None, &["shares.read"]).await;
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;
    repo::add_member(&test.pool, "B Kooperatifi", member, None)
        .await
        .expect("B membership");
    let b_role = grant_role(
        &test.pool,
        member,
        "karma-b",
        Some(coop_b),
        &["shares.manage"],
    )
    .await;

    // A user with NO bootstrap membership must not appear at all.
    let outsider = create_user(&test.pool, "dis_uye").await;
    repo::add_member(&test.pool, "B Kooperatifi", outsider, None)
        .await
        .expect("B membership");

    let (status, body) =
        request_json(&test, Method::GET, "/api/users", &cookie, None, None, None).await;
    assert_eq!(status, StatusCode::OK);
    let users: Value = serde_json::from_str(&body).expect("json");
    let listed: Vec<&str> = users
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|u| u["id"].as_str())
        .collect();
    assert!(listed.contains(&admin.to_string().as_str()));
    assert!(listed.contains(&member.to_string().as_str()));
    assert!(
        !listed.contains(&outsider.to_string().as_str()),
        "non-member must not be listed"
    );
    // In-scope roles only for the member.
    let member_entry = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"].as_str() == Some(&member.to_string()))
        .expect("member entry");
    let role_ids: Vec<&str> = member_entry["roles"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["id"].as_str())
        .collect();
    assert!(role_ids.contains(&null_role.to_string().as_str()));
    assert!(
        !role_ids.contains(&b_role.to_string().as_str()),
        "B-scoped role leaked into bootstrap view"
    );

    // Direct user-roles read on a non-member → 404.
    assert_eq!(
        request_json(
            &test,
            Method::GET,
            &format!("/api/users/{outsider}/roles"),
            &cookie,
            None,
            None,
            None,
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    let _ = coop;
    test.cleanup().await;
}

#[tokio::test]
async fn deferred_mutations_keep_their_global_guard() {
    let Some(test) = setup().await else { return };
    bootstrap_admin(&test.pool, "a_admin6").await;
    let (cookie, csrf) = login(&test.app, test.peer, "a_admin6").await;

    // PUT /roles/{id}/permissions stays on the GLOBAL path (P1-c2b):
    // a bootstrap admin still uses it, and stripping the last
    // administration permission is still lockout-prevented.
    let target: Uuid =
        sqlx::query_scalar("SELECT ura.role_id FROM user_role_assignments ura LIMIT 1")
            .fetch_one(&test.pool)
            .await
            .expect("admin role");
    let (status, _) = request_json(
        &test,
        Method::PUT,
        &format!("/api/roles/{target}/permissions"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "permissions": ["roles.read"] })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "lockout guard must still prevent last-admin stripping"
    );

    // disable_user likewise keeps the global guard: disabling the only
    // administrator is refused.
    let admin: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE username = 'a_admin6'")
        .fetch_one(&test.pool)
        .await
        .expect("admin id");
    assert_eq!(
        request_json(
            &test,
            Method::POST,
            &format!("/api/users/{admin}/disable"),
            &cookie,
            Some(&csrf),
            None,
            None,
        )
        .await
        .0,
        StatusCode::CONFLICT
    );

    test.cleanup().await;
}
