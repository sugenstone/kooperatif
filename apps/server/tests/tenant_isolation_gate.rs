//! M1-P0-002 §1 — legacy business-endpoint security audit (REQ-024 gate).
//!
//! P0 introduced cooperatives + memberships but business endpoints are
//! still tenant-unscoped (P2 retrofit pending). The M1 §5 restriction
//! therefore requires a backend guard: a user must not reach business
//! modules through global (pre-tenant) RBAC alone when every
//! cooperative they belong to is business-disabled.
//!
//! These tests assert the DESIRED gate behavior with real HTTP requests
//! against the real router. They cover:
//!   - member of the bootstrap (business-enabled) cooperative → allowed;
//!   - member of ONLY a business-disabled cooperative → denied on
//!     every probed business family, read AND write;
//!   - member of both → allowed;
//!   - suspended membership → denied;
//!   - user with zero memberships → allowed (pre-tenant transition,
//!     single-cooperative compatibility);
//!   - explicit `x-cooperative-id` naming a non-enabled cooperative →
//!     denied even when the user holds an enabled membership elsewhere.

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
const TEST_PASSWORD: &str = "tenant-gate-parola-123";

fn test_database_url() -> Option<String> {
    let value = std::env::var("KOOPERATIF_TEST_DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "KOOPERATIF_TEST_DATABASE_URL is not set — required DB-gated integration tests cannot silently pass"
        )
    });
    let trimmed = value.trim().to_string();
    assert!(!trimmed.is_empty(), "KOOPERATIF_TEST_DATABASE_URL is empty");
    Some(trimmed)
}

struct TestApp {
    app: axum::Router,
    pool: PgPool,
    _peer: SocketAddr,
    database_name: String,
    admin_url: String,
}

impl TestApp {
    fn peer(&self) -> SocketAddr {
        self._peer
    }

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

async fn create_user(pool: &PgPool, username: &str) -> Uuid {
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).expect("test parameters"),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).expect("hash");
    users::create_user(pool, username, "Test Kullanıcı", &hash)
        .await
        .expect("user created")
        .id
}

/// Global (pre-tenant) role carrying every business permission — the
/// exact exposure vector §1 of M1-P0-002 asks about.
async fn grant_full_business_role(pool: &PgPool, user_id: Uuid) {
    let role: Uuid = sqlx::query_scalar("INSERT INTO roles (name) VALUES ($1) RETURNING id")
        .bind(format!("denetim-rol-{}", Uuid::new_v4().simple()))
        .fetch_one(pool)
        .await
        .expect("role created");
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission_id) \
         SELECT $1, id FROM permissions",
    )
    .bind(role)
    .execute(pool)
    .await
    .expect("all permissions granted");
    sqlx::query("INSERT INTO user_role_assignments (user_id, role_id) VALUES ($1, $2)")
        .bind(user_id)
        .bind(role)
        .execute(pool)
        .await
        .expect("assignment");
}

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

async fn send(
    app: &axum::Router,
    method: &str,
    path: &str,
    cookie: &str,
    csrf: Option<&str>,
    coop: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("cookie", format!("kooperatif_session={cookie}"))
        .header("origin", ORIGIN);
    if let Some(csrf) = csrf {
        builder = builder.header("x-csrf-token", csrf);
    }
    if let Some(coop) = coop {
        builder = builder.header("x-cooperative-id", coop);
    }
    let request = if let Some(body) = body {
        builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("valid")
    } else {
        builder.body(Body::empty()).expect("valid")
    };
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

/// Every business family required by M1-P0-002 §1 (read probes).
const READ_PROBES: &[&str] = &[
    "/api/persons",
    "/api/families",
    "/api/shareholders",
    "/api/shares",
    "/api/share-return-entitlements",
    "/api/periods",
    "/api/payments",
    "/api/financial-accounts",
    "/api/incomes",
    "/api/investments",
    "/api/social-aid/funds",
    "/api/reports/overview",
    "/api/governance/bodies",
    "/api/users",
];

/// A business-disabled second cooperative with `status='active'` —
/// the worst case: only `business_enabled=false` protects business
/// modules.
async fn create_disabled_business_coop(pool: &PgPool, name: &str) -> Uuid {
    let coop = kooperatif_server::tenant::repo::create_cooperative(pool, name, None)
        .await
        .expect("second coop");
    assert!(!coop.business_enabled);
    sqlx::query("UPDATE cooperatives SET status = 'active' WHERE id = $1")
        .bind(coop.id)
        .execute(pool)
        .await
        .expect("activate coop");
    coop.id
}

async fn assert_all_denied(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    coop: Option<&str>,
    label: &str,
) {
    for path in READ_PROBES {
        let (status, body) = send(&test.app, "GET", path, cookie, Some(csrf), coop, None).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{label}: GET {path} must deny a user without an enabled membership (body: {body})"
        );
        assert_eq!(
            body["error"]["code"], "cooperative_access_denied",
            "{label}: GET {path} must use the stable denial code"
        );
    }
    // Write attempt: the guard must reject BEFORE the handler's own
    // validation — a deliberately incomplete body would otherwise be a
    // 400, so a 403 here proves ordering.
    let (status, body) = send(
        &test.app,
        "POST",
        "/api/families",
        cookie,
        Some(csrf),
        coop,
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "{label}: POST /api/families must be denied before payload validation (body: {body})"
    );
    assert_eq!(body["error"]["code"], "cooperative_access_denied");
}

#[tokio::test]
async fn member_of_only_disabled_cooperative_cannot_reach_business_endpoints() {
    let Some(test) = setup().await else { return };
    // Order matters: bootstrap enrolls every user existing at that
    // moment. Bootstrap FIRST, then create the user so their only
    // membership is the disabled cooperative.
    bootstrap_enabled(&test.pool).await;
    let second = create_disabled_business_coop(&test.pool, "Ikinci Koop").await;
    let user = create_user(&test.pool, "sadece_b_uyesi").await;
    grant_full_business_role(&test.pool, user).await;
    kooperatif_server::tenant::repo::add_member(&test.pool, "Ikinci Koop", user, None)
        .await
        .expect("member of second coop only");
    // NOT a member of the bootstrap cooperative.

    let (cookie, csrf, _) = login(&test.app, test.peer(), "sadece_b_uyesi").await;
    // Without any header: global permissions must NOT rescue access.
    assert_all_denied(&test, &cookie, &csrf, None, "B-only member").await;
    // Explicitly naming their (disabled) cooperative: same denial.
    assert_all_denied(
        &test,
        &cookie,
        &csrf,
        Some(&second.to_string()),
        "B-only member + B header",
    )
    .await;
    test.cleanup().await;
}

#[tokio::test]
async fn member_of_enabled_cooperative_keeps_legacy_access() {
    let Some(test) = setup().await else { return };
    let user = create_user(&test.pool, "ana_uye").await;
    grant_full_business_role(&test.pool, user).await;
    bootstrap_enabled(&test.pool).await;

    let (cookie, csrf, _) = login(&test.app, test.peer(), "ana_uye").await;
    for path in READ_PROBES {
        let (status, _) = send(&test.app, "GET", path, &cookie, Some(&csrf), None, None).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "bootstrap member must keep legacy access to GET {path}"
        );
    }
    test.cleanup().await;
}

#[tokio::test]
async fn dual_member_is_allowed_but_stale_disabled_header_is_denied() {
    let Some(test) = setup().await else { return };
    bootstrap_enabled(&test.pool).await;
    let second = create_disabled_business_coop(&test.pool, "Ikinci Koop").await;
    let user = create_user(&test.pool, "cift_uye").await;
    grant_full_business_role(&test.pool, user).await;
    // The user was created AFTER bootstrap, so enrollment is explicit.
    kooperatif_server::tenant::repo::add_member(&test.pool, "Ana Kooperatif", user, None)
        .await
        .expect("bootstrap membership");
    kooperatif_server::tenant::repo::add_member(&test.pool, "Ikinci Koop", user, None)
        .await
        .expect("second membership");

    let (cookie, csrf, _) = login(&test.app, test.peer(), "cift_uye").await;
    // Enabled membership exists → business endpoints stay reachable.
    let (status, _) = send(
        &test.app,
        "GET",
        "/api/shareholders",
        &cookie,
        Some(&csrf),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // But an explicit context naming the not-ready cooperative must
    // fail closed rather than silently serving another cooperative's
    // (still unscoped) data.
    let (status, body) = send(
        &test.app,
        "GET",
        "/api/shareholders",
        &cookie,
        Some(&csrf),
        Some(&second.to_string()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "cooperative_access_denied");
    test.cleanup().await;
}

#[tokio::test]
async fn suspended_membership_denies_business_endpoints() {
    let Some(test) = setup().await else { return };
    let user = create_user(&test.pool, "askiya_alinan").await;
    grant_full_business_role(&test.pool, user).await;
    let coop_id = bootstrap_enabled(&test.pool).await;
    let (cookie, csrf, _) = login(&test.app, test.peer(), "askiya_alinan").await;

    // Sanity: access works before suspension.
    let (status, _) = send(
        &test.app,
        "GET",
        "/api/shareholders",
        &cookie,
        Some(&csrf),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    sqlx::query(
        "UPDATE cooperative_memberships SET status = 'suspended' \
         WHERE user_id = $1 AND cooperative_id = $2",
    )
    .bind(user)
    .bind(coop_id)
    .execute(&test.pool)
    .await
    .expect("suspend");

    // Global role is still intact — the suspension must still cut off
    // every business module.
    assert_all_denied(&test, &cookie, &csrf, None, "suspended member").await;
    test.cleanup().await;
}

#[tokio::test]
async fn membershipless_user_keeps_pre_tenant_access() {
    let Some(test) = setup().await else { return };
    let user = create_user(&test.pool, "uyesiz_admin").await;
    grant_full_business_role(&test.pool, user).await;
    bootstrap_enabled(&test.pool).await;
    // Deliberately remove this user's membership (a user that predates
    // or was never enrolled into the tenant model).
    sqlx::query("DELETE FROM cooperative_memberships WHERE user_id = $1")
        .bind(user)
        .execute(&test.pool)
        .await
        .expect("strip membership");

    let (cookie, csrf, _) = login(&test.app, test.peer(), "uyesiz_admin").await;
    let (status, _) = send(
        &test.app,
        "GET",
        "/api/shareholders",
        &cookie,
        Some(&csrf),
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "zero-membership users keep single-cooperative compatibility"
    );
    test.cleanup().await;
}

#[tokio::test]
async fn auth_and_tenant_endpoints_stay_reachable_for_disabled_coop_members() {
    let Some(test) = setup().await else { return };
    bootstrap_enabled(&test.pool).await;
    create_disabled_business_coop(&test.pool, "Ikinci Koop").await;
    let user = create_user(&test.pool, "b_uyesi_auth").await;
    kooperatif_server::tenant::repo::add_member(&test.pool, "Ikinci Koop", user, None)
        .await
        .expect("second membership");

    let (cookie, csrf, _) = login(&test.app, test.peer(), "b_uyesi_auth").await;
    // /api/auth/me must still answer — a locked-out member needs to
    // see their memberships to understand the denial.
    let (status, body) = send(
        &test.app,
        "GET",
        "/api/auth/me",
        &cookie,
        Some(&csrf),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["cooperatives"].as_array().unwrap().len(), 1);
    // Logout must always work.
    let (status, _) = send(
        &test.app,
        "POST",
        "/api/auth/logout",
        &cookie,
        Some(&csrf),
        None,
        None,
    )
    .await;
    assert!(
        status == StatusCode::NO_CONTENT || status == StatusCode::OK,
        "logout must stay reachable for gated users"
    );
    test.cleanup().await;
}

async fn bootstrap_enabled(pool: &PgPool) -> Uuid {
    kooperatif_server::tenant::repo::bootstrap_initial_cooperative(pool, "Ana Kooperatif")
        .await
        .expect("bootstrap")
        .0
}
