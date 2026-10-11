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
//!   - M1-P1-c2b: the four administration-path-sensitive mutations
//!     (disable_role, PUT permissions, disable_user, PUT user-roles)
//!     run under the same verified scope with a cooperative-scoped
//!     advisory lock + cooperative-scoped administrator-continuity
//!     count; assignments are written WITH cooperative_id — the API
//!     never creates NULL-cooperative rows.

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
async fn migrated_mutations_keep_last_admin_protection() {
    let Some(test) = setup().await else { return };
    bootstrap_admin(&test.pool, "a_admin6").await;
    let (cookie, csrf) = login(&test.app, test.peer, "a_admin6").await;

    // P1-c2b: PUT /roles/{id}/permissions now runs on the SCOPED path —
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

    // disable_user likewise runs the scoped guard: disabling the only
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

// ------------------------------------------------------------------
// M1-P1-007 (P1-c2b) — critical RBAC mutations + cooperative-scoped
// administrator continuity.
// ------------------------------------------------------------------

/// Look up the bootstrap-scoped administrator count the way
/// `administration_path_count(scope)` computes it (independent SQL —
/// test asserts against its own re-derivation, never the app's).
async fn scoped_admin_count(pool: &PgPool, coop: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(DISTINCT ura.user_id) \
         FROM user_role_assignments ura \
         JOIN roles r ON r.id = ura.role_id AND r.status = 'active' \
                      AND r.cooperative_id = $1 \
         JOIN role_permissions rp ON rp.role_id = r.id \
         JOIN permissions p ON p.id = rp.permission_id AND p.key = 'roles.manage' \
         JOIN users u ON u.id = ura.user_id AND u.status = 'active' \
         JOIN cooperative_memberships m \
           ON m.user_id = ura.user_id AND m.cooperative_id = $1 \
          AND m.status = 'active' \
         WHERE ura.cooperative_id = $1",
    )
    .bind(coop)
    .fetch_one(pool)
    .await
    .expect("scoped count")
}

/// Every one of the four critical mutations refuses to remove the
/// verified scope's last administration path (409 lockout_prevented),
/// and the refusal is audited to the cooperative.
#[tokio::test]
async fn scoped_lockout_covers_all_four_mutations() {
    let Some(test) = setup().await else { return };
    let (admin, coop) = bootstrap_admin(&test.pool, "son_yonetici").await;
    let (cookie, csrf) = login(&test.app, test.peer, "son_yonetici").await;
    let admin_role: Uuid =
        sqlx::query_scalar("SELECT role_id FROM user_role_assignments WHERE user_id = $1")
            .bind(admin)
            .fetch_one(&test.pool)
            .await
            .expect("admin role");

    let coop_str = coop.to_string();
    for (method, path, body) in [
        (
            Method::PUT,
            format!("/api/users/{admin}/roles"),
            Some(json!({ "roleIds": [] })),
        ),
        (Method::POST, format!("/api/users/{admin}/disable"), None),
        (
            Method::POST,
            format!("/api/roles/{admin_role}/disable"),
            None,
        ),
        (
            Method::PUT,
            format!("/api/roles/{admin_role}/permissions"),
            Some(json!({ "permissions": ["users.read"] })),
        ),
    ] {
        let (status, body) = request_json(
            &test,
            method,
            &path,
            &cookie,
            Some(&csrf),
            Some(&coop_str),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{path}: {body}");
        assert!(
            body.contains("lockout_prevented"),
            "{path} must fail with lockout_prevented: {body}"
        );
    }

    // All four refusals are audited AND cooperative-attributed.
    let (events, attributed): (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE cooperative_id = $1) \
         FROM security_events WHERE event_type = 'lockout_prevented'",
    )
    .bind(coop)
    .fetch_one(&test.pool)
    .await
    .expect("audit");
    assert_eq!(events, 4);
    assert_eq!(attributed, 4, "lockout audit must carry the scope");

    // State untouched: role + assignment + user survive.
    assert_eq!(scoped_admin_count(&test.pool, coop).await, 1);

    test.cleanup().await;
}

/// Cooperative-scoped counting (P1-K5): another cooperative's
/// administrator path must NEVER satisfy this scope's lockout guard.
#[tokio::test]
async fn other_cooperative_admin_path_does_not_satisfy_lockout() {
    let Some(test) = setup().await else { return };
    let (admin, coop) = bootstrap_admin(&test.pool, "a_tek_admin").await;

    // Cooperative B has its own administrator — globally TWO
    // administration paths exist, but only ONE in the bootstrap scope.
    let b_admin = create_user(&test.pool, "b_yonetici").await;
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
    assert_eq!(scoped_admin_count(&test.pool, coop).await, 1);
    assert_eq!(scoped_admin_count(&test.pool, coop_b).await, 1);

    let (cookie, csrf) = login(&test.app, test.peer, "a_tek_admin").await;
    let admin_role: Uuid =
        sqlx::query_scalar("SELECT role_id FROM user_role_assignments WHERE user_id = $1")
            .bind(admin)
            .fetch_one(&test.pool)
            .await
            .expect("admin role");

    // Under the OLD global count this would have been allowed; the
    // scoped guard must still refuse every variant.
    for (method, path, body) in [
        (
            Method::PUT,
            format!("/api/users/{admin}/roles"),
            Some(json!({ "roleIds": [] })),
        ),
        (Method::POST, format!("/api/users/{admin}/disable"), None),
        (
            Method::POST,
            format!("/api/roles/{admin_role}/disable"),
            None,
        ),
        (
            Method::PUT,
            format!("/api/roles/{admin_role}/permissions"),
            Some(json!({ "permissions": ["users.read"] })),
        ),
    ] {
        let (status, body) =
            request_json(&test, method, &path, &cookie, Some(&csrf), None, body).await;
        assert_eq!(status, StatusCode::CONFLICT, "{path}: {body}");
    }

    // B's administrator was never disturbed.
    assert_eq!(scoped_admin_count(&test.pool, coop_b).await, 1);
    assert_eq!(scoped_admin_count(&test.pool, coop).await, 1);

    test.cleanup().await;
}

/// Cross-cooperative mutation attempts: a role or user owned only by
/// cooperative B is invisible to the bootstrap scope (404 for every
/// critical verb — existence never leaks); a B-owned role id in an
/// assignment payload is rejected (400, same code as unknown).
#[tokio::test]
async fn cross_cooperative_targets_are_invisible_to_mutations() {
    let Some(test) = setup().await else { return };
    bootstrap_admin(&test.pool, "a_admin7").await;
    let (cookie, csrf) = login(&test.app, test.peer, "a_admin7").await;

    let b_user = create_user(&test.pool, "b_kullanici").await;
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;
    repo::add_member(&test.pool, "B Kooperatifi", b_user, None)
        .await
        .expect("B membership");
    let b_role = grant_role(&test.pool, b_user, "b-rolu", Some(coop_b), ADMIN_PERMS).await;

    // Every critical mutation against B-owned targets → 404.
    for (method, path, body) in [
        (Method::POST, format!("/api/roles/{b_role}/disable"), None),
        (
            Method::PUT,
            format!("/api/roles/{b_role}/permissions"),
            Some(json!({ "permissions": ["users.read"] })),
        ),
        (Method::POST, format!("/api/users/{b_user}/disable"), None),
        (
            Method::PUT,
            format!("/api/users/{b_user}/roles"),
            Some(json!({ "roleIds": [] })),
        ),
    ] {
        let (status, body) =
            request_json(&test, method, &path, &cookie, Some(&csrf), None, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
    }
    // And B's state is untouched.
    let b_role_status: String = sqlx::query_scalar("SELECT status FROM roles WHERE id = $1")
        .bind(b_role)
        .fetch_one(&test.pool)
        .await
        .expect("b role");
    assert_eq!(b_role_status, "active");
    let b_user_status: String = sqlx::query_scalar("SELECT status FROM users WHERE id = $1")
        .bind(b_user)
        .fetch_one(&test.pool)
        .await
        .expect("b user");
    assert_eq!(b_user_status, "active");

    // Assigning a B-owned role to a bootstrap member → 400, nothing
    // written.
    let member = create_user(&test.pool, "a_uye").await;
    repo::add_member(&test.pool, "Ana Kooperatif", member, None)
        .await
        .expect("membership");
    let (status, body) = request_json(
        &test,
        Method::PUT,
        &format!("/api/users/{member}/roles"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "roleIds": [b_role.to_string()] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let written: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_role_assignments WHERE user_id = $1")
            .bind(member)
            .fetch_one(&test.pool)
            .await
            .expect("count");
    assert_eq!(written, 0, "cross-cooperative assignment must not persist");

    test.cleanup().await;
}

/// Two concurrent administrator self-removals in the SAME cooperative
/// scope: the cooperative-scoped advisory lock serializes them — exactly
/// one commits, the other observes zero remaining paths and aborts.
#[tokio::test]
async fn concurrent_admin_removals_leave_one_scoped_path() {
    let Some(test) = setup().await else { return };
    let (first_admin, coop) = bootstrap_admin(&test.pool, "esz_a").await;
    let second_admin = create_user(&test.pool, "esz_b").await;
    repo::add_member(&test.pool, "Ana Kooperatif", second_admin, None)
        .await
        .expect("membership");
    grant_role(
        &test.pool,
        second_admin,
        "esz-b-rol",
        Some(coop),
        ADMIN_PERMS,
    )
    .await;

    let (first_cookie, first_csrf) = login(&test.app, test.peer, "esz_a").await;
    let (second_cookie, second_csrf) = login(&test.app, test.peer, "esz_b").await;

    let first_path = format!("/api/users/{first_admin}/roles");
    let second_path = format!("/api/users/{second_admin}/roles");
    let first_req = request_json(
        &test,
        Method::PUT,
        &first_path,
        &first_cookie,
        Some(&first_csrf),
        None,
        Some(json!({ "roleIds": [] })),
    );
    let second_req = request_json(
        &test,
        Method::PUT,
        &second_path,
        &second_cookie,
        Some(&second_csrf),
        None,
        Some(json!({ "roleIds": [] })),
    );
    let (first, second) = tokio::join!(first_req, second_req);
    let mut statuses = [first.0, second.0];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::NO_CONTENT, StatusCode::CONFLICT],
        "exactly one removal must commit: {statuses:?}"
    );

    assert_eq!(
        scoped_admin_count(&test.pool, coop).await,
        1,
        "exactly one administration path survives in the scope"
    );

    test.cleanup().await;
}

/// PUT /users/{id}/roles writes cooperative-scoped assignment rows —
/// the RBAC API never creates new NULL-cooperative assignments.
#[tokio::test]
async fn api_assignments_carry_cooperative_never_null() {
    let Some(test) = setup().await else { return };
    let (_, coop) = bootstrap_admin(&test.pool, "a_admin8").await;
    let (cookie, csrf) = login(&test.app, test.peer, "a_admin8").await;

    // A second admin so the lockout guard is not in the way.
    let second = create_user(&test.pool, "a_ikinci").await;
    repo::add_member(&test.pool, "Ana Kooperatif", second, None)
        .await
        .expect("membership");

    // A scoped, assignable non-admin role.
    let member_role =
        grant_role(&test.pool, second, "uye-rolu", Some(coop), &["shares.read"]).await;

    let target = create_user(&test.pool, "hedef_uye").await;
    repo::add_member(&test.pool, "Ana Kooperatif", target, None)
        .await
        .expect("membership");

    let nulls_before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_role_assignments WHERE cooperative_id IS NULL",
    )
    .fetch_one(&test.pool)
    .await
    .expect("null count");

    let (status, body) = request_json(
        &test,
        Method::PUT,
        &format!("/api/users/{target}/roles"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "roleIds": [member_role.to_string()] })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let written: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM user_role_assignments WHERE user_id = $1")
            .bind(target)
            .fetch_one(&test.pool)
            .await
            .expect("assignment row");
    assert_eq!(written, Some(coop), "API assignment must carry the scope");
    let nulls_after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_role_assignments WHERE cooperative_id IS NULL",
    )
    .fetch_one(&test.pool)
    .await
    .expect("null count");
    assert_eq!(nulls_after, nulls_before, "no new NULL assignment rows");

    test.cleanup().await;
}

/// NULL-attributed legacy rows in the bootstrap scope: manageable
/// (disable / permission-replace succeed when they are not the last
/// admin path), but NOT assignable — the composite FK can only pair
/// scope-owned roles, and the API refuses to write NULL assignments.
/// A user's NULL-attributed assignment IS replaced in place by the
/// scoped rewrite.
#[tokio::test]
async fn null_legacy_rows_manageable_but_not_assignable() {
    let Some(test) = setup().await else { return };
    let (_, coop) = bootstrap_admin(&test.pool, "a_admin9").await;
    let (cookie, csrf) = login(&test.app, test.peer, "a_admin9").await;
    let second = create_user(&test.pool, "yedek_admin").await;
    repo::add_member(&test.pool, "Ana Kooperatif", second, None)
        .await
        .expect("membership");
    grant_role(
        &test.pool,
        second,
        "yedek-admin-rol",
        Some(coop),
        ADMIN_PERMS,
    )
    .await;

    // Legacy NULL role with an innocuous permission.
    let null_role = grant_role(&test.pool, second, "eski-rol", None, &["shares.read"]).await;
    // Careful: grant_role wrote a NULL-cooperative ASSIGNMENT for
    // `second` — that is the legacy lane the test exercises; delete it
    // so `second` stays a clean admin path through its scoped role.
    sqlx::query("DELETE FROM user_role_assignments WHERE user_id = $1 AND cooperative_id IS NULL")
        .bind(second)
        .execute(&test.pool)
        .await
        .expect("cleanup null assignment");

    // NULL role: disable + permission replace work (manageable).
    for (method, path, body) in [
        (
            Method::POST,
            format!("/api/roles/{null_role}/disable"),
            None,
        ),
        (
            Method::PUT,
            format!("/api/roles/{null_role}/permissions"),
            Some(json!({ "permissions": ["families.read"] })),
        ),
    ] {
        let (status, body) =
            request_json(&test, method, &path, &cookie, Some(&csrf), None, body).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{path}: {body}");
    }

    // NULL role is NOT assignable: same 400 as unknown/foreign roles.
    let target = create_user(&test.pool, "atama_hedefi").await;
    repo::add_member(&test.pool, "Ana Kooperatif", target, None)
        .await
        .expect("membership");
    let (status, body) = request_json(
        &test,
        Method::PUT,
        &format!("/api/users/{target}/roles"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "roleIds": [null_role.to_string()] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // A user's legacy NULL assignment is inside the scope's rewrite
    // window: PUT replaces it with a scoped row.
    let legacy_target = create_user(&test.pool, "eski_atamali").await;
    repo::add_member(&test.pool, "Ana Kooperatif", legacy_target, None)
        .await
        .expect("membership");
    let scoped_role = grant_role(
        &test.pool,
        legacy_target,
        "kapsamli-rol",
        Some(coop),
        &["shares.read"],
    )
    .await;
    sqlx::query("INSERT INTO user_role_assignments (user_id, role_id) VALUES ($1, $2)")
        .bind(legacy_target)
        .bind(null_role)
        .execute(&test.pool)
        .await
        .expect("null assignment");
    let (status, body) = request_json(
        &test,
        Method::PUT,
        &format!("/api/users/{legacy_target}/roles"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "roleIds": [scoped_role.to_string()] })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let rows: Vec<Option<Uuid>> =
        sqlx::query_scalar("SELECT cooperative_id FROM user_role_assignments WHERE user_id = $1")
            .bind(legacy_target)
            .fetch_all(&test.pool)
            .await
            .expect("assignments");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0],
        Some(coop),
        "NULL assignment must be replaced by a scoped one"
    );

    test.cleanup().await;
}

/// A suspended/revoked membership (a) denies the actor on the very next
/// request and (b) no longer counts as an administration path — the
/// last-admin guard must not treat a suspended admin as a recovery
/// route.
#[tokio::test]
async fn suspended_membership_denies_and_stops_counting() {
    let Some(test) = setup().await else { return };
    let (first_admin, coop) = bootstrap_admin(&test.pool, "uye_a").await;
    let second_admin = create_user(&test.pool, "uye_b").await;
    repo::add_member(&test.pool, "Ana Kooperatif", second_admin, None)
        .await
        .expect("membership");
    let second_role = grant_role(
        &test.pool,
        second_admin,
        "uye-b-admin-rol",
        Some(coop),
        ADMIN_PERMS,
    )
    .await;
    assert_eq!(scoped_admin_count(&test.pool, coop).await, 2);

    // Suspend the second admin: next request is denied at extraction.
    sqlx::query("UPDATE cooperative_memberships SET status = 'suspended' WHERE user_id = $1")
        .bind(second_admin)
        .execute(&test.pool)
        .await
        .expect("suspend");
    let (second_cookie, second_csrf) = login(&test.app, test.peer, "uye_b").await;
    let (status, _) = request_json(
        &test,
        Method::PUT,
        &format!("/api/users/{first_admin}/roles"),
        &second_cookie,
        Some(&second_csrf),
        None,
        Some(json!({ "roleIds": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The suspended admin no longer counts as a path: admin #1 cannot
    // remove their own last path now.
    assert_eq!(scoped_admin_count(&test.pool, coop).await, 1);
    let (cookie, csrf) = login(&test.app, test.peer, "uye_a").await;
    let (status, body) = request_json(
        &test,
        Method::PUT,
        &format!("/api/users/{first_admin}/roles"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "roleIds": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // But admin #1 CAN still strip the suspended admin's role (the
    // surviving active path is enough).
    let (status, body) = request_json(
        &test,
        Method::PUT,
        &format!("/api/users/{second_admin}/roles"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "roleIds": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let _ = second_role;

    test.cleanup().await;
}

/// Audit attribution (P1-c5 groundwork): every migrated mutation's
/// security event carries the verified cooperative_id.
#[tokio::test]
async fn migrated_mutations_attribute_audit_to_scope() {
    let Some(test) = setup().await else { return };
    let (_, coop) = bootstrap_admin(&test.pool, "denetim_admin").await;
    let (cookie, csrf) = login(&test.app, test.peer, "denetim_admin").await;
    let second = create_user(&test.pool, "yedek").await;
    repo::add_member(&test.pool, "Ana Kooperatif", second, None)
        .await
        .expect("membership");
    let second_role = grant_role(&test.pool, second, "yedek-admin", Some(coop), ADMIN_PERMS).await;
    let plain_role = grant_role(&test.pool, second, "duz-rol", Some(coop), &["shares.read"]).await;

    // PUT permissions on the non-admin role.
    assert_eq!(
        request_json(
            &test,
            Method::PUT,
            &format!("/api/roles/{plain_role}/permissions"),
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "permissions": ["families.read"] })),
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    // PUT user roles (assign delta → role_assigned) + disable_user on a
    // non-admin member.
    let target = create_user(&test.pool, "denetim_hedef").await;
    repo::add_member(&test.pool, "Ana Kooperatif", target, None)
        .await
        .expect("membership");
    assert_eq!(
        request_json(
            &test,
            Method::PUT,
            &format!("/api/users/{target}/roles"),
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "roleIds": [plain_role.to_string()] })),
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request_json(
            &test,
            Method::POST,
            &format!("/api/users/{target}/disable"),
            &cookie,
            Some(&csrf),
            None,
            None,
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    // disable_role last: the role must be active while assigned above.
    assert_eq!(
        request_json(
            &test,
            Method::POST,
            &format!("/api/roles/{plain_role}/disable"),
            &cookie,
            Some(&csrf),
            None,
            None,
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );

    let expected = [
        "role_disabled",
        "role_permissions_changed",
        "role_assigned",
        "user_disabled",
    ];
    for event in expected {
        let (count, scoped): (i64, i64) = sqlx::query_as(
            "SELECT count(*), count(*) FILTER (WHERE cooperative_id = $1) \
             FROM security_events WHERE event_type = $2",
        )
        .bind(coop)
        .bind(event)
        .fetch_one(&test.pool)
        .await
        .expect("audit");
        assert_eq!(count, 1, "{event} missing");
        assert_eq!(scoped, 1, "{event} must be cooperative-attributed");
    }
    let _ = second_role;

    test.cleanup().await;
}

/// Header rules on the migrated critical mutations: malformed → 400,
/// unknown → 403, explicit non-bootstrap → 403, headerless → bootstrap.
#[tokio::test]
async fn header_rules_apply_to_critical_mutations() {
    let Some(test) = setup().await else { return };
    bootstrap_admin(&test.pool, "header_admin").await;
    let (cookie, csrf) = login(&test.app, test.peer, "header_admin").await;
    let second = create_user(&test.pool, "yedek2").await;
    repo::add_member(&test.pool, "Ana Kooperatif", second, None)
        .await
        .expect("membership");
    let role = grant_role(&test.pool, second, "yedek-admin2", None, ADMIN_PERMS).await;

    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;

    // Malformed → 400; unknown → 403; non-bootstrap → 403.
    for (header, expected) in [
        (Some("bozuk-uuid".to_string()), StatusCode::BAD_REQUEST),
        (Some(Uuid::new_v4().to_string()), StatusCode::FORBIDDEN),
        (Some(coop_b.to_string()), StatusCode::FORBIDDEN),
    ] {
        let (status, body) = request_json(
            &test,
            Method::POST,
            &format!("/api/roles/{role}/disable"),
            &cookie,
            Some(&csrf),
            header.as_deref(),
            None,
        )
        .await;
        assert_eq!(status, expected, "header {header:?}: {body}");
    }

    // Headerless → bootstrap scope → the NULL-attributed role is
    // manageable (a second admin path exists, so disabling is allowed).
    assert_eq!(
        request_json(
            &test,
            Method::POST,
            &format!("/api/roles/{role}/disable"),
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

/// Dual-membership actor (admin in bootstrap, viewer in B): exercises
/// admin powers in the bootstrap scope — headerless AND explicit
/// bootstrap header — but never in B. A bootstrap "viewer" reads but
/// cannot mutate; a B-only viewer gets nothing at all.
#[tokio::test]
async fn dual_membership_and_viewer_isolation() {
    let Some(test) = setup().await else { return };
    let (dual, coop) = bootstrap_admin(&test.pool, "cift_uye").await;
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;
    repo::add_member(&test.pool, "B Kooperatifi", dual, None)
        .await
        .expect("B membership");
    grant_role(
        &test.pool,
        dual,
        "b-izleyici-rol",
        Some(coop_b),
        &["roles.read"],
    )
    .await;

    let (cookie, csrf) = login(&test.app, test.peer, "cift_uye").await;
    let coop_str = coop.to_string();

    // Headerless: admin powers in bootstrap scope.
    let target = create_user(&test.pool, "cift_hedef").await;
    repo::add_member(&test.pool, "Ana Kooperatif", target, None)
        .await
        .expect("membership");
    assert_eq!(
        request_json(
            &test,
            Method::POST,
            &format!("/api/users/{target}/disable"),
            &cookie,
            Some(&csrf),
            None,
            None,
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    // Explicit bootstrap header: same.
    let target2 = create_user(&test.pool, "cift_hedef2").await;
    repo::add_member(&test.pool, "Ana Kooperatif", target2, None)
        .await
        .expect("membership");
    assert_eq!(
        request_json(
            &test,
            Method::POST,
            &format!("/api/users/{target2}/disable"),
            &cookie,
            Some(&csrf),
            Some(&coop_str),
            None,
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    // Explicit B header → 403 even though the user IS a B member.
    assert_eq!(
        request_json(
            &test,
            Method::POST,
            &format!("/api/users/{target}/enable"),
            &cookie,
            Some(&csrf),
            Some(&coop_b.to_string()),
            None,
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );

    // Bootstrap viewer (roles.read only in A; full ADMIN in B): reads
    // fine in A, mutations 403 — B powers can never leak into the
    // bootstrap scope.
    let viewer = create_user(&test.pool, "izleyici").await;
    repo::add_member(&test.pool, "Ana Kooperatif", viewer, None)
        .await
        .expect("membership");
    grant_role(
        &test.pool,
        viewer,
        "izleyici-rol",
        Some(coop),
        &["roles.read"],
    )
    .await;
    repo::add_member(&test.pool, "B Kooperatifi", viewer, None)
        .await
        .expect("B membership");
    grant_role(
        &test.pool,
        viewer,
        "izleyici-b-admin",
        Some(coop_b),
        ADMIN_PERMS,
    )
    .await;
    let (viewer_cookie, viewer_csrf) = login(&test.app, test.peer, "izleyici").await;
    assert_eq!(
        request_json(
            &test,
            Method::GET,
            "/api/roles",
            &viewer_cookie,
            None,
            None,
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    for (method, path, body) in [
        (
            Method::PUT,
            format!("/api/users/{target}/roles"),
            Some(json!({ "roleIds": [] })),
        ),
        (Method::POST, format!("/api/users/{target}/disable"), None),
    ] {
        let (status, body) = request_json(
            &test,
            method,
            &path,
            &viewer_cookie,
            Some(&viewer_csrf),
            None,
            body,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body}");
    }

    // B-only viewer: nothing in the bootstrap scope.
    let b_only = create_user(&test.pool, "b_izleyici").await;
    repo::add_member(&test.pool, "B Kooperatifi", b_only, None)
        .await
        .expect("B membership");
    grant_role(
        &test.pool,
        b_only,
        "b-only-rol",
        Some(coop_b),
        &["roles.read"],
    )
    .await;
    let (b_cookie, _) = login(&test.app, test.peer, "b_izleyici").await;
    assert_eq!(
        request_json(
            &test,
            Method::GET,
            "/api/roles",
            &b_cookie,
            None,
            None,
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );

    test.cleanup().await;
}

/// Never-bootstrapped lane (legacy compatibility): before the initial
/// cooperative exists, the four mutations keep their documented legacy
/// semantics — global guard, NULL-cooperative writes (the only possible
/// representation pre-tenancy).
#[tokio::test]
async fn never_bootstrapped_lane_keeps_legacy_semantics() {
    let Some(test) = setup().await else { return };

    // No bootstrap: users + NULL roles in the pre-tenant lane.
    let admin = create_user(&test.pool, "miras_admin").await;
    let admin_role = grant_role(&test.pool, admin, "Miras Admin", None, ADMIN_PERMS).await;
    let second = create_user(&test.pool, "miras_ikinci").await;
    grant_role(&test.pool, second, "Miras Ikinci", None, ADMIN_PERMS).await;
    let target = create_user(&test.pool, "miras_hedef").await;
    let member_role = grant_role(&test.pool, target, "Uye", None, &["shares.read"]).await;

    let (cookie, csrf) = login(&test.app, test.peer, "miras_admin").await;

    // PUT user roles works and writes NULL — the pre-tenant lane cannot
    // do anything else (no cooperative exists).
    let (status, body) = request_json(
        &test,
        Method::PUT,
        &format!("/api/users/{target}/roles"),
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "roleIds": [member_role.to_string()] })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let written: Option<Uuid> = sqlx::query_scalar(
        "SELECT cooperative_id FROM user_role_assignments WHERE user_id = $1 AND role_id = $2",
    )
    .bind(target)
    .bind(member_role)
    .fetch_one(&test.pool)
    .await
    .expect("assignment");
    assert_eq!(written, None, "pre-tenant lane keeps NULL writes");

    // Global lockout still applies: strip the second admin's roles →
    // allowed while `admin` remains; then disabling `admin` would
    // remove the LAST global path → refused.
    assert_eq!(
        request_json(
            &test,
            Method::PUT,
            &format!("/api/users/{second}/roles"),
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "roleIds": [] })),
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
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
    let _ = admin_role;

    test.cleanup().await;
}

// ---- M1-P1-c3a — /api/auth/me bootstrap-scoped hints ----

#[tokio::test]
async fn me_returns_bootstrap_scoped_authorization_hints() {
    let Some(test) = setup().await else { return };
    let (_, coop) = bootstrap_admin(&test.pool, "me_admin").await;
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("B")
        .id;

    // Dual member: viewer in bootstrap A, full administrator in B.
    // me must report ONLY the bootstrap-attributable hints — a B-scoped
    // role can never imply authority over A's business surface.
    let dual = create_user(&test.pool, "cift_uye").await;
    repo::add_member(&test.pool, "Ana Kooperatif", dual, None)
        .await
        .expect("A member");
    repo::add_member(&test.pool, "B Kooperatifi", dual, None)
        .await
        .expect("B member");
    let a_role = grant_role(&test.pool, dual, "a-izleyici", Some(coop), &["roles.read"]).await;
    let b_role = grant_role(&test.pool, dual, "b-admin", Some(coop_b), ADMIN_PERMS).await;

    let (cookie, _) = login(&test.app, test.peer, "cift_uye").await;
    let (status, body) = request_json(
        &test,
        Method::GET,
        "/api/auth/me",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let body: Value = serde_json::from_str(&body).expect("json");
    let perms: Vec<String> = body["permissions"]
        .as_array()
        .expect("permissions array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(perms.contains(&"roles.read".to_string()), "{perms:?}");
    assert!(
        !perms.contains(&"roles.manage".to_string()),
        "B-scoped admin permission must not appear in hints: {perms:?}"
    );
    assert!(
        !perms.contains(&"users.manage".to_string()),
        "B-scoped permission must not appear in hints: {perms:?}"
    );
    let role_ids: Vec<String> = body["roles"]
        .as_array()
        .expect("roles array")
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    assert!(role_ids.contains(&a_role.to_string()));
    assert!(
        !role_ids.contains(&b_role.to_string()),
        "B role must not appear in me hints: {role_ids:?}"
    );

    // B-only member: hints are empty but the cooperatives list still
    // reports the membership — the contract shape is unchanged.
    let b_only = create_user(&test.pool, "sadece_b").await;
    repo::add_member(&test.pool, "B Kooperatifi", b_only, None)
        .await
        .expect("B member");
    grant_role(&test.pool, b_only, "b-admin-2", Some(coop_b), ADMIN_PERMS).await;
    let (b_cookie, _) = login(&test.app, test.peer, "sadece_b").await;
    let (b_status, b_body) = request_json(
        &test,
        Method::GET,
        "/api/auth/me",
        &b_cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(b_status, StatusCode::OK, "{b_body}");
    let b_body: Value = serde_json::from_str(&b_body).expect("json");
    assert_eq!(b_body["permissions"].as_array().unwrap().len(), 0);
    assert_eq!(b_body["roles"].as_array().unwrap().len(), 0);
    assert!(
        !b_body["cooperatives"].as_array().unwrap().is_empty(),
        "memberships must still be reported"
    );

    // NULL-attributed legacy role: resolvable inside the verified
    // bootstrap scope — legacy grants keep working after bootstrap.
    let legacy = create_user(&test.pool, "miras").await;
    repo::add_member(&test.pool, "Ana Kooperatif", legacy, None)
        .await
        .expect("A member");
    grant_role(&test.pool, legacy, "miras-rol", None, ADMIN_PERMS).await;
    let (l_cookie, _) = login(&test.app, test.peer, "miras").await;
    let (l_status, l_body) = request_json(
        &test,
        Method::GET,
        "/api/auth/me",
        &l_cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(l_status, StatusCode::OK, "{l_body}");
    let l_body: Value = serde_json::from_str(&l_body).expect("json");
    let l_perms: Vec<String> = l_body["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(
        l_perms.contains(&"roles.manage".to_string()),
        "NULL-attributed legacy grant must still resolve: {l_perms:?}"
    );

    test.cleanup().await;
}
