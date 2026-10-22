//! M1-P6 B6 — adversarial security audit of the multi-cooperative
//! architecture (B1–B5). Negative tests with explicit expected-vs-
//! observed outcomes; nothing here weakens an existing assertion.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database;
//! scratch cooperatives only — no real second cooperative).
//!
//! Coverage:
//! - Cross-cooperative read/write attempts (IDOR on foreign rows,
//!   forged body fields, membership lifecycle on foreign users).
//! - Unauthorized cooperative switching and scope stability after a
//!   rejected switch; concurrent switches converge to a single scope.
//! - Role-boundary escalation: cooperative admin cannot reach platform
//!   lanes, cannot grant `cooperatives.manage`, and NULL-scope legacy
//!   `users.manage` cannot operate inside a foreign cooperative.
//! - Sealed-context forgery at the SQL layer: GUC spoofing on the
//!   restricted runtime principal yields zero rows and no stamp.
//! - Session invalidation: membership suspension fails closed on the
//!   very next request.

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

const TEST_PASSWORD: &str = "p6b6-test-parola-123";
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
    /// runtime principal (`kooperatif_app`).
    app: axum::Router,
    /// Owner pool for fixtures/assertions only — never routed.
    pool: PgPool,
    /// Restricted runtime pool — used for GUC-forgery probes.
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
    users::create_user(pool, username, "B6 Test", &hash, TEST_PASSWORD)
        .await
        .expect("user")
        .id
}

/// NULL-scope platform lane grant (owner-lane fixture).
async fn grant_platform(pool: &PgPool, user: Uuid, perms: &[&str]) {
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, cooperative_id) VALUES ($1, NULL) RETURNING id",
    )
    .bind(format!("plat-b6-{user}"))
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
            .bind(format!("coop-b6-{user}-{coop}"))
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

/// Resolved effective scope through the authoritative context endpoint.
async fn resolved_scope(app: &axum::Router, cookie: &str) -> Option<Uuid> {
    let (status, body) = send(
        app,
        request_builder("GET", "/api/auth/cooperative-context", cookie, None, None),
    )
    .await;
    if status != StatusCode::OK {
        return None;
    }
    body["cooperativeId"].as_str().and_then(|s| s.parse().ok())
}

/// Create + activate a second scratch cooperative inside the test DB.
/// Returns (coop_b, platform_admin_cookie, platform_admin_csrf).
async fn second_cooperative(test: &TestApp, tag: &str) -> (Uuid, Uuid) {
    let padmin = create_user(&test.pool, &format!("p6b6-padmin-{tag}")).await;
    grant_platform(&test.pool, padmin, &["cooperatives.manage"]).await;
    let (pcookie, pcsrf, _) = login(&test.app, test.peer, &format!("p6b6-padmin-{tag}")).await;
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/platform/cooperatives",
            &pcookie,
            Some(&pcsrf),
            Some(json!({ "name": format!("B6 Kooperatif {tag}") })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let coop_b: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    let admin_b = create_user(&test.pool, &format!("p6b6-adminb-{tag}")).await;
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
    (coop_b, padmin)
}

// ── 1. Cross-cooperative membership access (IDOR) ─────────────────────

/// EXPECTED: an A-scope administrator targeting a user whose only
/// membership lives in cooperative B gets 404 (no row in the verified
/// scope) — never 204, never a cross-scope mutation.
#[tokio::test]
async fn membership_lifecycle_on_foreign_cooperative_user_is_404() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;
    let (coop_b, _padmin) = second_cooperative(&test, "idor").await;

    let admin_a = create_user(&test.pool, "p6b6-admin-a-idor").await;
    grant_in_cooperative(&test.pool, coop_a, admin_a, &["memberships.manage"]).await;
    let (acookie, acsrf, _) = login(&test.app, test.peer, "p6b6-admin-a-idor").await;

    // B-only member — visible nowhere inside scope A.
    let victim_b = create_user(&test.pool, "p6b6-victim-b").await;
    sqlx::query("INSERT INTO cooperative_memberships (cooperative_id, user_id) VALUES ($1, $2)")
        .bind(coop_b)
        .bind(victim_b)
        .execute(&test.pool)
        .await
        .unwrap();

    for action in ["suspend", "reactivate", "end"] {
        let (status, body) = send(
            &test.app,
            request_builder(
                "POST",
                &format!("/api/users/{victim_b}/membership-{action}"),
                &acookie,
                Some(&acsrf),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{action} → {body}");
    }

    // OBSERVED: B's membership row is untouched (still active).
    let status: String =
        sqlx::query_scalar("SELECT status FROM cooperative_memberships WHERE user_id = $1")
            .bind(victim_b)
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert_eq!(status, "active");
    test.cleanup().await;
}

// ── 2. Forged cooperative field in a create body ─────────────────────

/// EXPECTED: a membership create carrying injected `cooperativeId`
/// fields is rejected outright (unknown-field validation) — and a
/// control request WITHOUT them proves the row would only ever land in
/// the sealed scope anyway.
#[tokio::test]
async fn forged_cooperative_field_in_membership_body_cannot_pin_scope() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;
    let (coop_b, _padmin) = second_cooperative(&test, "forge-body").await;

    let admin_a = create_user(&test.pool, "p6b6-admin-a-forge").await;
    grant_in_cooperative(&test.pool, coop_a, admin_a, &["memberships.manage"]).await;
    let (acookie, acsrf, _) = login(&test.app, test.peer, "p6b6-admin-a-forge").await;

    let candidate = create_user(&test.pool, "p6b6-candidate").await;
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/memberships",
            &acookie,
            Some(&acsrf),
            Some(json!({
                "userId": candidate,
                "cooperativeId": coop_b,   // forged field
                "cooperative_id": coop_b   // snake_case attempt
            })),
        ),
    )
    .await;
    // OBSERVED: rejected at the contract layer — unknown cooperative
    // fields never reach the scope-pinned definer.
    assert!(
        status.is_client_error(),
        "forged fields must be rejected → {status} {body}"
    );

    // Control: the same create without forged fields lands in scope A.
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/memberships",
            &acookie,
            Some(&acsrf),
            Some(json!({ "userId": candidate })),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "control create must succeed → {status} {body}"
    );
    let in_a: Option<i64> = sqlx::query_scalar(
        "SELECT count(*) FROM cooperative_memberships WHERE cooperative_id = $1 AND user_id = $2",
    )
    .bind(coop_a)
    .bind(candidate)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    let in_b: Option<i64> = sqlx::query_scalar(
        "SELECT count(*) FROM cooperative_memberships WHERE cooperative_id = $1 AND user_id = $2",
    )
    .bind(coop_b)
    .bind(candidate)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(in_a, Some(1), "row must land in the sealed scope A");
    assert_eq!(in_b, Some(0), "forged cooperativeId must not pin B");
    test.cleanup().await;
}

// ── 3. Cooperative admin cannot reach platform lanes ─────────────────

/// EXPECTED: a user holding every cooperative-scoped permission in A
/// (including memberships.manage + roles.manage) is denied on every
/// platform lifecycle endpoint — no platform authority derives from a
/// cooperative role.
#[tokio::test]
async fn cooperative_admin_cannot_reach_platform_lanes() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;
    let (coop_b, _padmin) = second_cooperative(&test, "plat-deny").await;

    let admin_a = create_user(&test.pool, "p6b6-admin-a-plat").await;
    grant_in_cooperative(
        &test.pool,
        coop_a,
        admin_a,
        &[
            "memberships.read",
            "memberships.manage",
            "roles.manage",
            "users.manage",
            "users.read",
        ],
    )
    .await;
    let (acookie, acsrf, _) = login(&test.app, test.peer, "p6b6-admin-a-plat").await;

    for (method, path, body) in [
        ("GET", "/api/platform/cooperatives".to_string(), None),
        (
            "POST",
            "/api/platform/cooperatives".to_string(),
            Some(json!({ "name": "Kaçak Kooperatif" })),
        ),
        (
            "POST",
            format!("/api/platform/cooperatives/{coop_b}/suspend"),
            None,
        ),
        (
            "POST",
            format!("/api/platform/cooperatives/{coop_b}/archive"),
            None,
        ),
    ] {
        let (status, body) = send(
            &test.app,
            request_builder(method, &path, &acookie, Some(&acsrf), body),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path} → {body}");
    }

    // OBSERVED: B untouched — still active.
    let status: String = sqlx::query_scalar("SELECT status FROM cooperatives WHERE id = $1")
        .bind(coop_b)
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(status, "active");
    test.cleanup().await;
}

// ── 4. Platform admin cannot read cooperative business data ──────────

/// EXPECTED: a platform administrator (cooperatives.manage, NO
/// membership anywhere) cannot read cooperative business rows —
/// platform authority is lifecycle-only.
#[tokio::test]
async fn platform_admin_cannot_read_cooperative_business_data() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;
    let (coop_b, padmin) = second_cooperative(&test, "plat-read").await;
    let _ = coop_b;

    // Seed a business row in A the platform admin must never see.
    sqlx::query(
        "INSERT INTO financial_categories (cooperative_id, category_type, name) \
         VALUES ($1, 'income', 'a-gizli-kategori')",
    )
    .bind(coop_a)
    .execute(&test.pool)
    .await
    .unwrap();

    let (pcookie, _pcsrf, _) = login(&test.app, test.peer, "p6b6-padmin-plat-read").await;
    let _ = padmin;

    // Bootstrap fallback scope → platform admin holds no membership →
    // business endpoints fail closed; a forged B header cannot select B.
    for path in ["/api/financial-categories", "/api/memberships"] {
        let (status, body) = send(
            &test.app,
            request_builder("GET", path, &pcookie, None, None),
        )
        .await;
        assert!(
            status.is_client_error(),
            "{path} must deny platform-only principal → {status} {body}"
        );
    }
    let (status, body) = send(
        &test.app,
        scoped_request_builder(
            "GET",
            "/api/financial-categories",
            &pcookie,
            None,
            None,
            coop_b,
        ),
    )
    .await;
    assert!(
        status.is_client_error(),
        "forged scope header must not open business data → {status} {body}"
    );
    test.cleanup().await;
}

// ── 5. Cooperative role cannot carry platform permission ─────────────

/// EXPECTED: PUT /api/roles/{coop-role}/permissions including
/// `cooperatives.manage` is rejected — platform authority can never be
/// attached to a cooperative-owned role.
#[tokio::test]
async fn cooperative_role_cannot_carry_platform_permission() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;

    let admin_a = create_user(&test.pool, "p6b6-admin-a-roles").await;
    grant_in_cooperative(&test.pool, coop_a, admin_a, &["roles.manage"]).await;
    let (acookie, acsrf, _) = login(&test.app, test.peer, "p6b6-admin-a-roles").await;

    // Own cooperative-scoped role to mutate.
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, cooperative_id) VALUES ('b6-rol', $1) RETURNING id",
    )
    .bind(coop_a)
    .fetch_one(&test.pool)
    .await
    .unwrap();

    let (status, body) = send(
        &test.app,
        request_builder(
            "PUT",
            &format!("/api/roles/{role}/permissions"),
            &acookie,
            Some(&acsrf),
            Some(json!({ "permissions": ["cooperatives.manage"] })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // OBSERVED: role holds no permissions.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM role_permissions WHERE role_id = $1")
        .bind(role)
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    test.cleanup().await;
}

/// EXPECTED: PUT /api/roles/{foreign-role}/permissions is rejected even
/// when the payload is innocuous — a role OWNED by B is outside A's
/// write scope (0069 strict write boundary).
#[tokio::test]
async fn cooperative_admin_cannot_mutate_foreign_role() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;
    let (coop_b, _padmin) = second_cooperative(&test, "foreign-role").await;

    let admin_a = create_user(&test.pool, "p6b6-admin-a-frole").await;
    grant_in_cooperative(&test.pool, coop_a, admin_a, &["roles.manage"]).await;
    let (acookie, acsrf, _) = login(&test.app, test.peer, "p6b6-admin-a-frole").await;

    let role_b: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, cooperative_id) VALUES ('b-rol', $1) RETURNING id",
    )
    .bind(coop_b)
    .fetch_one(&test.pool)
    .await
    .unwrap();

    let (status, body) = send(
        &test.app,
        request_builder(
            "PUT",
            &format!("/api/roles/{role_b}/permissions"),
            &acookie,
            Some(&acsrf),
            Some(json!({ "permissions": ["income_expense.read"] })),
        ),
    )
    .await;
    assert!(
        status.is_client_error(),
        "foreign role write must fail → {status} {body}"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM role_permissions WHERE role_id = $1")
        .bind(role_b)
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "foreign role permissions untouched");
    test.cleanup().await;
}

// ── 6. NULL-scope legacy users.manage — bounded to bootstrap ─────────

/// EXPECTED: a NULL-scope `users.manage` assignment (legacy global
/// grant) authorizes membership lifecycle ONLY on the bootstrap
/// cooperative — inside cooperative B it contributes nothing and the
/// same request is denied.
#[tokio::test]
async fn null_scope_users_manage_does_not_leak_cross_cooperative() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;
    let (coop_b, _padmin) = second_cooperative(&test, "null-lane").await;

    // Actor: NULL-scope users.manage, membership in A AND B (membership
    // is required for business scope — the grant alone never enters).
    let actor = create_user(&test.pool, "p6b6-null-lane-actor").await;
    grant_platform(&test.pool, actor, &["users.manage"]).await;
    for coop in [coop_a, coop_b] {
        sqlx::query(
            "INSERT INTO cooperative_memberships (cooperative_id, user_id) VALUES ($1, $2)",
        )
        .bind(coop)
        .bind(actor)
        .execute(&test.pool)
        .await
        .unwrap();
    }
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b6-null-lane-actor").await;

    // Victims in both scopes.
    let victim_a = create_user(&test.pool, "p6b6-victim-null-a").await;
    let victim_b = create_user(&test.pool, "p6b6-victim-null-b").await;
    for (coop, user) in [(coop_a, victim_a), (coop_b, victim_b)] {
        sqlx::query(
            "INSERT INTO cooperative_memberships (cooperative_id, user_id) VALUES ($1, $2)",
        )
        .bind(coop)
        .bind(user)
        .execute(&test.pool)
        .await
        .unwrap();
    }

    // Bootstrap scope (never switched): NULL-scope users.manage IS the
    // documented legacy lane → 204.
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/users/{victim_a}/membership-suspend"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "legacy lane must work in bootstrap scope → {body}"
    );

    // After switching to B the same actor loses the lane: NULL-scope
    // grants do not resolve in a non-bootstrap cooperative → denied.
    assert_eq!(
        switch_cooperative(&test.app, &cookie, &csrf, coop_b).await,
        StatusCode::NO_CONTENT
    );
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/users/{victim_b}/membership-suspend"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "NULL-scope users.manage must not leak into coop B → {body}"
    );
    let status_b: String =
        sqlx::query_scalar("SELECT status FROM cooperative_memberships WHERE user_id = $1")
            .bind(victim_b)
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert_eq!(status_b, "active");
    test.cleanup().await;
}

// ── 7. Membership suspension fails closed immediately ────────────────

/// EXPECTED: once a membership is suspended, the session's very next
/// business request is denied — no stale-scope grace period.
#[tokio::test]
async fn suspended_membership_fails_closed_on_next_request() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;

    let admin = create_user(&test.pool, "p6b6-susp-admin").await;
    grant_in_cooperative(&test.pool, coop_a, admin, &["memberships.manage"]).await;
    let member = create_user(&test.pool, "p6b6-susp-member").await;
    grant_in_cooperative(&test.pool, coop_a, member, &["income_expense.read"]).await;

    let (mcookie, _mcsrf, _) = login(&test.app, test.peer, "p6b6-susp-member").await;
    let (acookie, acsrf, _) = login(&test.app, test.peer, "p6b6-susp-admin").await;

    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/users/{member}/membership-suspend"),
            &acookie,
            Some(&acsrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // OBSERVED: member's existing session cannot read business data.
    let (status, body) = send(
        &test.app,
        request_builder("GET", "/api/financial-categories", &mcookie, None, None),
    )
    .await;
    assert!(
        status.is_client_error(),
        "suspended member must fail closed → {status} {body}"
    );
    test.cleanup().await;
}

// ── 8. Unauthorized switch + scope stability ──────────────────────────

/// EXPECTED: POST /api/auth/cooperative for a cooperative where the
/// session holds no active membership is rejected and the session's
/// resolved scope is unchanged.
#[tokio::test]
async fn unauthorized_switch_rejected_and_scope_unchanged() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;
    let (coop_b, _padmin) = second_cooperative(&test, "unauth-switch").await;

    let user = create_user(&test.pool, "p6b6-switch-user").await;
    grant_in_cooperative(&test.pool, coop_a, user, &["income_expense.read"]).await;
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b6-switch-user").await;

    // Not a member of B.
    assert_eq!(
        switch_cooperative(&test.app, &cookie, &csrf, coop_b).await,
        StatusCode::FORBIDDEN
    );
    // A random cooperative id is equally rejected.
    assert_eq!(
        switch_cooperative(&test.app, &cookie, &csrf, Uuid::new_v4()).await,
        StatusCode::FORBIDDEN
    );

    // OBSERVED: resolved scope is still bootstrap A.
    assert_eq!(resolved_scope(&test.app, &cookie).await, Some(coop_a));
    test.cleanup().await;
}

/// EXPECTED: concurrent switch requests converge to exactly one
/// authoritative scope — no torn state, and a subsequent request
/// succeeds precisely in the scope the session reports.
#[tokio::test]
async fn concurrent_switches_converge_to_single_authoritative_scope() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;
    let (coop_b, _padmin) = second_cooperative(&test, "race").await;

    let user = create_user(&test.pool, "p6b6-race-user").await;
    grant_in_cooperative(&test.pool, coop_a, user, &["income_expense.read"]).await;
    grant_in_cooperative(&test.pool, coop_b, user, &["income_expense.read"]).await;
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b6-race-user").await;

    // Fire an alternating burst of switches concurrently.
    let mut joins = Vec::new();
    for i in 0..10 {
        let app = test.app.clone();
        let cookie = cookie.clone();
        let csrf = csrf.clone();
        let target = if i % 2 == 0 { coop_a } else { coop_b };
        joins.push(tokio::spawn(async move {
            switch_cooperative(&app, &cookie, &csrf, target).await
        }));
    }
    for join in joins {
        let status = join.await.expect("switch task");
        assert!(
            status == StatusCode::NO_CONTENT || status == StatusCode::FORBIDDEN,
            "switch must settle deterministically → {status}"
        );
    }

    // OBSERVED: /me resolves exactly one scope and that scope actually
    // authorizes a business read (assertion header matching).
    let resolved = resolved_scope(&test.app, &cookie)
        .await
        .expect("a scope must resolve");
    assert!(resolved == coop_a || resolved == coop_b);
    let (status, body) = send(
        &test.app,
        scoped_request_builder(
            "GET",
            "/api/financial-categories",
            &cookie,
            None,
            None,
            resolved,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    test.cleanup().await;
}

// ── 9. Sealed-context forgery on the restricted principal ────────────

/// EXPECTED: setting every cooperative-context GUC by hand on
/// kooperatif_app — including a plausible (but uncomputable) seal —
/// yields ZERO rows on a cooperative-scoped table and the stamp
/// definer refuses to mint a context.
#[tokio::test]
async fn forged_gucs_grant_no_scope_on_restricted_principal() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;
    let (coop_b, _padmin) = second_cooperative(&test, "guc").await;

    // Seed rows in both scopes (owner lane).
    for (coop, name) in [(coop_a, "a-kategori"), (coop_b, "b-kategori")] {
        sqlx::query(
            "INSERT INTO financial_categories (cooperative_id, category_type, name) \
             VALUES ($1, 'income', $2)",
        )
        .bind(coop)
        .bind(name)
        .execute(&test.pool)
        .await
        .unwrap();
    }

    let mut conn = test.app_pool.acquire().await.expect("app conn");
    // Attacker sets every context GUC by hand — including the CURRENT
    // transaction id and a fabricated seal. Only app_seal_of (owner-
    // only) can produce a valid seal, so app_ctx_ok() must stay false.
    sqlx::query("BEGIN").execute(&mut *conn).await.unwrap();
    let txid: String = sqlx::query_scalar("SELECT txid_current()::text")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    for (key, value) in [
        ("app.user_id", Uuid::new_v4().to_string()),
        ("app.session_id", Uuid::new_v4().to_string()),
        ("app.cooperative_id", coop_b.to_string()),
        ("app.ctx_txid", txid),
        ("app.ctx_seal", "f".repeat(64)),
    ] {
        sqlx::query(&format!("SET LOCAL \"{key}\" = '{value}'"))
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let ok: bool = sqlx::query_scalar("SELECT public.app_ctx_ok()")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert!(!ok, "forged seal must fail app_ctx_ok()");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM financial_categories")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(rows, 0, "forged context must see no rows");

    // The stamp definer must refuse to mint a cooperative context
    // without a valid user seal (returns zero rows, stamps nothing).
    let stamped: i64 = sqlx::query_scalar("SELECT count(*) FROM public.app_stamp_cooperative($1)")
        .bind(coop_b)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(stamped, 0, "stamp definer must refuse unsealed callers");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM financial_categories")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(rows, 0);
    sqlx::query("ROLLBACK").execute(&mut *conn).await.unwrap();
    test.cleanup().await;
}

// ── 10. Audit visibility is scope-bound ───────────────────────────────

/// EXPECTED: the scoped audit-read lane returns ONLY the verified
/// scope's events — switching scopes changes the visible set; a foreign
/// cooperative's audit trail never leaks.
#[tokio::test]
async fn audit_reads_are_pinned_to_the_verified_scope() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;
    let (coop_b, _padmin) = second_cooperative(&test, "audit").await;

    let admin_a = create_user(&test.pool, "p6b6-audit-admin").await;
    grant_in_cooperative(&test.pool, coop_a, admin_a, &["audit.read"]).await;
    let (acookie, acsrf, _) = login(&test.app, test.peer, "p6b6-audit-admin").await;

    // Generate an audit event in B (owner lane).
    sqlx::query(
        "INSERT INTO security_events (cooperative_id, event_type, user_id, metadata, occurred_at) \
         VALUES ($1, 'b6_probe_event', NULL, '{}'::jsonb, now())",
    )
    .bind(coop_b)
    .execute(&test.pool)
    .await
    .unwrap();

    let (status, body) = send(
        &test.app,
        request_builder("GET", "/api/audit/events", &acookie, None, None),
    )
    .await;
    // A-scope audit.read either answers with A's events only or denies —
    // never contains B's probe event.
    if status == StatusCode::OK {
        let text = body.to_string();
        assert!(
            !text.contains("b6_probe_event"),
            "foreign cooperative audit event leaked: {text}"
        );
    } else {
        assert!(status.is_client_error());
    }
    let _ = acsrf;
    test.cleanup().await;
}
