//! M1-P6 B2 — platform cooperative lifecycle tests (0068).
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same harness as tests/rbac.rs).
//!
//! Under test:
//! - Only NULL-scope `cooperatives.manage` holders reach the platform
//!   lifecycle API — cooperative administrators and cooperative-scoped
//!   grants of the SAME key are denied.
//! - New cooperatives land in `provisioning` + `business_enabled=false`.
//! - Activation is atomic: initial-administrator membership +
//!   cooperative-scoped admin role assignment + transition, or nothing.
//! - Invalid transitions and ineligible initial administrators fail
//!   closed with 409; the reactivation admin invariant is enforced.
//! - Suspension closes the business gate end-to-end (HTTP 403 on the
//!   bootstrap cooperative) while preserving every row.
//! - Lifecycle mutations mint attributed audit events and emit the
//!   signed `authz` realtime invalidation signal.
//! - The restricted `kooperatif_app` principal cannot write
//!   `cooperatives` directly nor invoke the definers unsealed/forged.

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

const TEST_PASSWORD: &str = "p6b2-test-parola-123";
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

/// Restricted-principal pool — the REAL kooperatif_app role (direct
/// SQL access, distinct from the HTTP app's restricted pool).
async fn app_pool(test: &TestApp) -> PgPool {
    app_db::connect(&app_url(&format!(
        "{}/{}",
        test.admin_url.rsplit_once('/').map(|(b, _)| b).unwrap(),
        test.database_name
    )))
    .await
    .expect("kooperatif_app pool")
}

async fn create_user(pool: &PgPool, username: &str) -> Uuid {
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).expect("params"),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).expect("hash");
    users::create_user(pool, username, "B2 Test", &hash, TEST_PASSWORD)
        .await
        .expect("user")
        .id
}

/// NULL-scope platform lane grant: a dedicated NULL-scope role carrying
/// exactly the requested keys + a NULL-scope assignment.
async fn grant_platform(pool: &PgPool, user: Uuid, perms: &[&str]) {
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, cooperative_id) VALUES ($1, NULL) RETURNING id",
    )
    .bind(format!("plat-b2-{user}"))
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

/// Cooperative-scoped grant on the bootstrap cooperative: active
/// membership + coop-scoped role + scoped assignment — the shape a
/// cooperative administrator actually holds.
async fn grant_cooperative(pool: &PgPool, user: Uuid, perms: &[&str]) {
    let coop = bootstrap_coop(pool).await;
    repo::add_member(pool, "Ana Kooperatif", user, None)
        .await
        .expect("membership");
    let role: Uuid =
        sqlx::query_scalar("INSERT INTO roles (name, cooperative_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("coop-b2-{user}"))
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

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
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

// ── Happy path ──────────────────────────────────────────────────────

/// Platform administrator creates (provisioning) and activates with an
/// initial administrator — membership, admin role assignment and the
/// transition land atomically.
#[tokio::test]
async fn platform_admin_creates_and_activates_with_initial_admin() {
    let test = setup().await;
    let admin_user = create_user(&test.pool, "p6b2-padmin").await;
    grant_platform(&test.pool, admin_user, &["cooperatives.manage"]).await;
    let member = create_user(&test.pool, "p6b2-member1").await;
    let (cookie, csrf, actor) = login(&test.app, test.peer, "p6b2-padmin").await;

    // Create → provisioning + gate closed.
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/platform/cooperatives",
            &cookie,
            Some(&csrf),
            Some(json!({ "name": "Yeni Kooperatif", "legalName": "Yeni Kooperatif A.Ş." })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let coop: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(body["status"], "provisioning");
    let (st, enabled) = coop_row(&test.pool, coop).await;
    assert_eq!((st.as_str(), enabled), ("provisioning", false));

    // Business gate inputs pre-activation: no membership row exists
    // yet and business_enabled=false — app_session_business_membership_ok
    // cannot return true on either count (the end-to-end 403 is proven
    // by suspension_blocks_business_and_reactivation_restores).
    let pre_membership: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cooperative_memberships WHERE cooperative_id = $1",
    )
    .bind(coop)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(pre_membership, 0);

    // Activate with the initial administrator.
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop}/activate"),
            &cookie,
            Some(&csrf),
            Some(json!({ "initialAdminUserId": member })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (st, enabled) = coop_row(&test.pool, coop).await;
    assert_eq!((st.as_str(), enabled), ("active", true));

    // Membership: active, attributed to the sealed platform actor.
    let (m_status, created_by): (String, Option<Uuid>) = sqlx::query_as(
        "SELECT status, created_by_user_id FROM cooperative_memberships \
         WHERE cooperative_id = $1 AND user_id = $2",
    )
    .bind(coop)
    .bind(member)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(m_status, "active");
    assert_eq!(
        created_by,
        Some(actor),
        "membership must attribute the platform actor"
    );

    // Cooperative-scoped administrator role cloned from the template
    // and assigned with cooperative attribution.
    let role: (Uuid, Option<Uuid>) = sqlx::query_as(
        "SELECT id, cooperative_id FROM roles \
         WHERE cooperative_id = $1 AND lower(name) = lower('Sistem Yöneticisi')",
    )
    .bind(coop)
    .fetch_one(&test.pool)
    .await
    .expect("cloned administrator role");
    assert_eq!(role.1, Some(coop));
    let has_admin_perm: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM role_permissions rp \
         JOIN permissions p ON p.id = rp.permission_id \
         WHERE rp.role_id = $1 AND p.key = 'roles.manage')",
    )
    .bind(role.0)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert!(has_admin_perm, "cloned role must carry roles.manage");
    // The platform-only key must NOT leak into the cooperative clone.
    let has_platform_perm: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM role_permissions rp \
         JOIN permissions p ON p.id = rp.permission_id \
         WHERE rp.role_id = $1 AND p.key = 'cooperatives.manage')",
    )
    .bind(role.0)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert!(
        !has_platform_perm,
        "template clone must never carry cooperatives.manage"
    );

    let assigned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM user_role_assignments \
         WHERE user_id = $1 AND role_id = $2 AND cooperative_id = $3)",
    )
    .bind(member)
    .bind(role.0)
    .bind(coop)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert!(assigned, "initial administrator assignment must exist");

    // Effective administrator path holds.
    let admin_paths: i64 = sqlx::query_scalar("SELECT public.app_admin_path_count($1)")
        .bind(coop)
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert!(
        admin_paths >= 1,
        "activated cooperative must keep an effective admin path"
    );

    test.cleanup().await;
}

// ── Authorization boundary ──────────────────────────────────────────

/// Nobody without a NULL-scope cooperatives.manage grant may touch the
/// platform lane — not plain members, not cooperative administrators,
/// not holders of the same key granted at cooperative scope.
#[tokio::test]
async fn cooperative_scope_never_satisfies_platform_lifecycle() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;

    let plain = create_user(&test.pool, "p6b2-plain").await;
    repo::add_member(&test.pool, "Ana Kooperatif", plain, None)
        .await
        .unwrap();
    let coop_admin = create_user(&test.pool, "p6b2-cadmin").await;
    grant_cooperative(&test.pool, coop_admin, &["roles.manage", "users.manage"]).await;
    // The adversary: the SAME key granted at cooperative scope.
    let scoped_holder = create_user(&test.pool, "p6b2-scoped").await;
    grant_cooperative(&test.pool, scoped_holder, &["cooperatives.manage"]).await;
    // users.manage at NULL scope is platform authority but NOT the
    // lifecycle permission — least privilege.
    let identity_admin = create_user(&test.pool, "p6b2-idadmin").await;
    grant_platform(&test.pool, identity_admin, &["users.manage"]).await;

    for (username, label) in [
        ("p6b2-plain", "plain member"),
        ("p6b2-cadmin", "cooperative administrator"),
        ("p6b2-scoped", "coop-scoped cooperatives.manage holder"),
        ("p6b2-idadmin", "platform users.manage holder"),
    ] {
        let (cookie, csrf, _) = login(&test.app, test.peer, username).await;
        for (method, path, body) in [
            (
                "POST",
                "/api/platform/cooperatives".to_string(),
                Some(json!({ "name": "Yetkisiz Kooperatif" })),
            ),
            (
                "POST",
                format!("/api/platform/cooperatives/{coop_a}/suspend"),
                None,
            ),
            (
                "POST",
                format!("/api/platform/cooperatives/{coop_a}/archive"),
                None,
            ),
        ] {
            let (status, body) = send(
                &test.app,
                request_builder(method, &path, &cookie, Some(&csrf), body),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{label} must be denied on {method} {path}: {body}"
            );
            assert_eq!(body["error"]["code"], "permission_denied", "{label}");
        }
        // Read lane is equally closed.
        let (status, _) = send(
            &test.app,
            request_builder("GET", "/api/platform/cooperatives", &cookie, None, None),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{label} must not list cooperatives"
        );
    }

    // And nothing was created: the unauthorized create attempts left
    // no trace.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM cooperatives WHERE NOT is_bootstrap")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);

    test.cleanup().await;
}

// ── Atomic activation failures ──────────────────────────────────────

/// An ineligible initial administrator (unknown / disabled global user)
/// fails the ENTIRE package — no membership, no role, no transition.
#[tokio::test]
async fn activation_with_ineligible_admin_rolls_back_completely() {
    let test = setup().await;
    let admin_user = create_user(&test.pool, "p6b2-padmin2").await;
    grant_platform(&test.pool, admin_user, &["cooperatives.manage"]).await;
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b2-padmin2").await;

    let coop = repo::create_cooperative(&test.pool, "Kırmızı Kooperatif", None)
        .await
        .unwrap()
        .id;

    // Unknown user id.
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop}/activate"),
            &cookie,
            Some(&csrf),
            Some(json!({ "initialAdminUserId": Uuid::new_v4() })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // Disabled user.
    let disabled = create_user(&test.pool, "p6b2-disabled").await;
    sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(disabled)
        .execute(&test.pool)
        .await
        .unwrap();
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop}/activate"),
            &cookie,
            Some(&csrf),
            Some(json!({ "initialAdminUserId": disabled })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // Atomicity proof: NOTHING persisted from either attempt.
    let (st, enabled) = coop_row(&test.pool, coop).await;
    assert_eq!((st.as_str(), enabled), ("provisioning", false));
    let leftovers: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM cooperative_memberships WHERE cooperative_id = $1) \
         + (SELECT count(*) FROM roles WHERE cooperative_id = $1) \
         + (SELECT count(*) FROM user_role_assignments WHERE cooperative_id = $1)",
    )
    .bind(coop)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(
        leftovers, 0,
        "failed activation must roll back every side effect"
    );

    test.cleanup().await;
}

// ── Transition matrix ───────────────────────────────────────────────

/// Every invalid edge of the state machine is rejected; idempotent
/// same-state calls succeed; the bootstrap cooperative cannot be
/// archived.
#[tokio::test]
async fn invalid_transitions_rejected_bootstrap_archive_blocked() {
    let test = setup().await;
    let admin_user = create_user(&test.pool, "p6b2-padmin3").await;
    grant_platform(&test.pool, admin_user, &["cooperatives.manage"]).await;
    let member = create_user(&test.pool, "p6b2-member3").await;
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b2-padmin3").await;

    let coop = repo::create_cooperative(&test.pool, "Geçiş Kooperatifi", None)
        .await
        .unwrap()
        .id;
    let bootstrap = bootstrap_coop(&test.pool).await;

    // provisioning → suspended: invalid (suspension is defined for
    // active cooperatives only).
    let (status, _) = send(
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
    assert_eq!(status, StatusCode::CONFLICT);

    // suspended → active on a never-suspended coop: invalid.
    let (status, _) = send(
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
    assert_eq!(status, StatusCode::CONFLICT);

    // provisioning → archived is allowed (cleanup of a mistaken
    // creation) — but the BOOTSTRAP cooperative can never be archived:
    // no in-product recovery exists.
    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{bootstrap}/archive"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "bootstrap archive must be refused"
    );

    // Real activation; then active → active via set_status is
    // idempotent, activate on an active coop is invalid.
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop}/activate"),
            &cookie,
            Some(&csrf),
            Some(json!({ "initialAdminUserId": member })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop}/activate"),
            &cookie,
            Some(&csrf),
            Some(json!({ "initialAdminUserId": member })),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "re-activation must be rejected"
    );

    // Unknown cooperative → 404 (never an existence oracle issue for
    // platform actors, and non-members cannot reach the lane anyway).
    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{}/suspend", Uuid::new_v4()),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    test.cleanup().await;
}

// ── Suspension blocks business; archive preserves history ───────────

/// Suspending the bootstrap cooperative immediately denies business
/// access end-to-end; reactivation restores it; all rows survive.
#[tokio::test]
async fn suspension_blocks_business_and_reactivation_restores() {
    let test = setup().await;
    let bootstrap = bootstrap_coop(&test.pool).await;
    let admin_user = create_user(&test.pool, "p6b2-padmin4").await;
    grant_platform(&test.pool, admin_user, &["cooperatives.manage"]).await;
    // A bootstrap member holding the seeded sysadmin role so a
    // privileged business endpoint is provable. Post-bootstrap users
    // get NO automatic membership — enroll explicitly first.
    let member = create_user(&test.pool, "p6b2-member4").await;
    repo::add_member(&test.pool, "Ana Kooperatif", member, None)
        .await
        .expect("membership");
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id) \
         SELECT $1, id FROM roles WHERE lower(name) = lower('Sistem Yöneticisi')",
    )
    .bind(member)
    .execute(&test.pool)
    .await
    .unwrap();

    let (member_cookie, _, _) = login(&test.app, test.peer, "p6b2-member4").await;
    // Business request succeeds while active.
    let (status, _) = send(
        &test.app,
        request_builder("GET", "/api/roles", &member_cookie, None, None),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "business request must pass on active coop"
    );

    let (admin_cookie, admin_csrf, _) = login(&test.app, test.peer, "p6b2-padmin4").await;
    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{bootstrap}/suspend"),
            &admin_cookie,
            Some(&admin_csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (st, enabled) = coop_row(&test.pool, bootstrap).await;
    assert_eq!((st.as_str(), enabled), ("suspended", false));

    // Historical + RBAC rows preserved — suspension is a status flip.
    let memberships: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cooperative_memberships WHERE cooperative_id = $1",
    )
    .bind(bootstrap)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert!(
        memberships >= 1,
        "membership history must survive suspension"
    );

    // Business is denied immediately — membership is still 'active'
    // but the cooperative itself is not.
    let (status, body) = send(
        &test.app,
        request_builder("GET", "/api/roles", &member_cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "cooperative_access_denied");

    // Reactivation restores business access.
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{bootstrap}/reactivate"),
            &admin_cookie,
            Some(&admin_csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (st, enabled) = coop_row(&test.pool, bootstrap).await;
    assert_eq!((st.as_str(), enabled), ("active", true));
    let (status, _) = send(
        &test.app,
        request_builder("GET", "/api/roles", &member_cookie, None, None),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "business must resume after reactivation"
    );

    test.cleanup().await;
}

/// Reactivation requires a live effective-administrator path:
/// suspending the only administrator's membership makes reactivation
/// fail with 409 until the path is restored.
#[tokio::test]
async fn reactivation_validates_effective_admin_invariant() {
    let test = setup().await;
    let admin_user = create_user(&test.pool, "p6b2-padmin5").await;
    grant_platform(&test.pool, admin_user, &["cooperatives.manage"]).await;
    let member = create_user(&test.pool, "p6b2-member5").await;
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b2-padmin5").await;

    let coop = repo::create_cooperative(&test.pool, "Kilit Kooperatifi", None)
        .await
        .unwrap()
        .id;
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop}/activate"),
            &cookie,
            Some(&csrf),
            Some(json!({ "initialAdminUserId": member })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (status, _) = send(
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
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Owner-lane fault injection: the only admin path disappears.
    sqlx::query(
        "UPDATE cooperative_memberships SET status = 'suspended' \
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
    let (st, _) = coop_row(&test.pool, coop).await;
    assert_eq!(
        st, "suspended",
        "failed reactivation must leave the cooperative suspended"
    );

    // Restore the admin path → reactivation succeeds.
    sqlx::query(
        "UPDATE cooperative_memberships SET status = 'active' \
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
    let (st, enabled) = coop_row(&test.pool, coop).await;
    assert_eq!((st.as_str(), enabled), ("active", true));

    test.cleanup().await;
}

// ── Audit attribution + realtime invalidation ───────────────────────

/// Lifecycle mutations mint attributed audit events (platform actor,
/// NULL cooperative attribution, target in metadata) and emit the
/// signed `authz` invalidation signal scoped to the cooperative.
#[tokio::test]
async fn lifecycle_events_are_audited_and_signaled() {
    let test = setup().await;
    let admin_user = create_user(&test.pool, "p6b2-padmin6").await;
    grant_platform(&test.pool, admin_user, &["cooperatives.manage"]).await;
    let member = create_user(&test.pool, "p6b2-member6").await;
    let (cookie, csrf, actor) = login(&test.app, test.peer, "p6b2-padmin6").await;

    // Capture signed domain notifications for the suspension emit.
    let mut listener = sqlx::postgres::PgListener::connect_with(&test.pool)
        .await
        .expect("listener");
    listener
        .listen("kooperatif_domain_changed")
        .await
        .expect("listen");

    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/platform/cooperatives",
            &cookie,
            Some(&csrf),
            Some(json!({ "name": "Denetim Kooperatifi" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let coop: Uuid = body["id"].as_str().unwrap().parse().unwrap();

    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop}/activate"),
            &cookie,
            Some(&csrf),
            Some(json!({ "initialAdminUserId": member })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = send(
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
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Audit rows: every event is attributed to the sealed platform
    // actor with NULL cooperative attribution; the target lives in
    // metadata.
    let events: Vec<(String, Option<Uuid>, Uuid, Value)> = sqlx::query_as(
        "SELECT event_type, cooperative_id, user_id, metadata \
           FROM security_events \
          WHERE event_type IN ('cooperative_created','cooperative_activated',\
                               'cooperative_suspended','cooperative_membership_granted',\
                               'role_created','role_assigned') \
            AND metadata->>'cooperative_id' = $1::text \
          ORDER BY occurred_at",
    )
    .bind(coop.to_string())
    .fetch_all(&test.pool)
    .await
    .unwrap();
    let types: Vec<&str> = events.iter().map(|(t, _, _, _)| t.as_str()).collect();
    for expected in [
        "cooperative_created",
        "cooperative_membership_granted",
        "role_created",
        "role_assigned",
        "cooperative_activated",
        "cooperative_suspended",
    ] {
        assert!(
            types.contains(&expected),
            "missing audit event {expected}: {types:?}"
        );
    }
    for (t, coop_attr, uid, _) in &events {
        assert_eq!(*uid, actor, "{t} must be attributed to the platform actor");
        assert!(
            coop_attr.is_none(),
            "{t} must be platform-attributed (NULL cooperative)"
        );
    }
    let granted_meta = events
        .iter()
        .find(|(t, _, _, _)| t == "cooperative_membership_granted")
        .map(|(_, _, _, m)| m.clone())
        .unwrap();
    assert_eq!(granted_meta["target_user_id"], json!(member));

    // Realtime invalidation: the suspension emitted a signed `authz`
    // signal scoped to the cooperative — bound sockets revalidate and
    // close (verify_business_membership rejects suspended coops).
    let notification = tokio::time::timeout(std::time::Duration::from_secs(5), listener.recv())
        .await
        .expect("authz signal must arrive promptly")
        .expect("notification");
    let payload = notification.payload().to_string();
    assert!(
        payload.starts_with(&format!("v2|{coop}|authz|")),
        "expected v2 authz signal scoped to the cooperative, got {payload}"
    );

    test.cleanup().await;
}

// ── Read lane ───────────────────────────────────────────────────────

/// The platform list/detail read model exposes the allowlisted
/// projection to lifecycle administrators only.
#[tokio::test]
async fn platform_read_model_scoped_and_allowlisted() {
    let test = setup().await;
    let bootstrap = bootstrap_coop(&test.pool).await;
    let admin_user = create_user(&test.pool, "p6b2-padmin7").await;
    grant_platform(&test.pool, admin_user, &["cooperatives.manage"]).await;
    let member = create_user(&test.pool, "p6b2-member7").await;
    let (cookie, _, _) = login(&test.app, test.peer, "p6b2-padmin7").await;

    let other = repo::create_cooperative(&test.pool, "Listelenen Kooperatif", None)
        .await
        .unwrap()
        .id;

    let (status, body) = send(
        &test.app,
        request_builder("GET", "/api/platform/cooperatives", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body.as_array().expect("array");
    assert_eq!(rows.len(), 2, "bootstrap + the new cooperative");
    let listed = rows
        .iter()
        .find(|r| r["id"].as_str() == Some(&other.to_string()))
        .expect("created cooperative listed");
    assert_eq!(listed["status"], "provisioning");
    assert_eq!(listed["businessEnabled"], false);
    assert_eq!(listed["memberCount"], 0);

    // Detail by id + status filter.
    let (status, body) = send(
        &test.app,
        request_builder(
            "GET",
            &format!("/api/platform/cooperatives/{other}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "Listelenen Kooperatif");
    let (status, _) = send(
        &test.app,
        request_builder(
            "GET",
            &format!("/api/platform/cooperatives/{}", Uuid::new_v4()),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = send(
        &test.app,
        request_builder(
            "GET",
            "/api/platform/cooperatives?status=provisioning",
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);
    let _ = bootstrap;
    let _ = member;

    test.cleanup().await;
}

// ── Restricted-runtime bypass resistance ────────────────────────────

/// The real `kooperatif_app` principal has no direct write surface on
/// `cooperatives` and cannot invoke the lifecycle definers without a
/// sealed request context — forged GUCs fail the seal check.
#[tokio::test]
async fn restricted_runtime_cannot_bypass_sealed_authorization() {
    let test = setup().await;
    let bootstrap = bootstrap_coop(&test.pool).await;
    let app = app_pool(&test).await;

    // Direct table writes: no privilege at all.
    for stmt in [
        "INSERT INTO cooperatives (name) VALUES ('Sızıntı')".to_string(),
        format!("UPDATE cooperatives SET status = 'suspended' WHERE id = '{bootstrap}'"),
        format!("DELETE FROM cooperatives WHERE id = '{bootstrap}'"),
    ] {
        let err = sqlx::query(&stmt)
            .execute(&app)
            .await
            .expect_err("direct write must be denied");
        assert_eq!(
            err.as_database_error().unwrap().code().as_deref(),
            Some("42501"),
            "{stmt}"
        );
    }

    // Definer calls without a seal: the definer itself rejects.
    for call in [
        "SELECT public.app_cooperative_create('X', NULL)",
        &format!("SELECT public.app_cooperative_set_status('{bootstrap}', 'suspended')"),
        &format!("SELECT public.app_cooperative_activate('{bootstrap}', gen_random_uuid())"),
    ] {
        let err = sqlx::query(call)
            .execute(&app)
            .await
            .expect_err("unsealed definer call must be denied");
        assert_eq!(
            err.as_database_error().unwrap().code().as_deref(),
            Some("42501"),
            "{call}"
        );
    }

    // Forged GUCs on the restricted lane still fail: the HMAC seal
    // over (user, session, NULL coop, txid) cannot be fabricated.
    let mut conn = app.acquire().await.unwrap();
    sqlx::raw_sql(
        "SELECT set_config('app.user_id', gen_random_uuid()::text, true), \
                set_config('app.session_id', gen_random_uuid()::text, true), \
                set_config('app.ctx_txid', txid_current()::text, true), \
                set_config('app.user_seal', 'forged', true)",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    let err = sqlx::query("SELECT public.app_cooperative_create('Y', NULL)")
        .execute(&mut *conn)
        .await
        .expect_err("forged seal must be denied");
    assert_eq!(
        err.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );

    // Nothing changed.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM cooperatives")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "only the bootstrap cooperative may exist");

    app.close().await;
    test.cleanup().await;
}

// ── Least-privilege seed ────────────────────────────────────────────

/// The dedicated platform role exists (NULL-scope, exactly
/// cooperatives.manage, no assignments) and the permission never
/// appears on cooperative-scoped roles or templates.
#[tokio::test]
async fn platform_lifecycle_role_is_least_privilege() {
    let test = setup().await;

    let (scope, perm_count): (Option<Uuid>, i64) = sqlx::query_as(
        "SELECT r.cooperative_id, \
                (SELECT count(*) FROM role_permissions rp WHERE rp.role_id = r.id) \
           FROM roles r WHERE r.cooperative_id IS NULL \
             AND lower(r.name) = lower('platform-lifecycle')",
    )
    .fetch_one(&test.pool)
    .await
    .expect("platform lifecycle role");
    assert!(scope.is_none());
    assert_eq!(perm_count, 1, "the role must carry exactly one permission");

    let assignments: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_role_assignments ura \
          JOIN roles r ON r.id = ura.role_id \
         WHERE lower(r.name) = lower('platform-lifecycle')",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(assignments, 0, "no automatic assignment");

    let leaked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_permissions rp \
          JOIN permissions p ON p.id = rp.permission_id \
          JOIN roles r ON r.id = rp.role_id \
         WHERE p.key = 'cooperatives.manage' AND r.cooperative_id IS NOT NULL",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(
        leaked, 0,
        "cooperatives.manage must never be cooperative-scoped"
    );

    test.cleanup().await;
}

// ── Platform role boundary hardening (0069) ─────────────────────────
//
// Bootstrap scope READ-inherits NULL-scope rows (legacy lane), but
// must never MUTATE them. Before 0069 the boundary definers used the
// read-inheritance predicate for writes too, letting a bootstrap
// administrator rename/disable/permission-strip the platform
// 'platform-lifecycle' role and delete NULL-scope assignments.
// These tests pin the hardened boundary.

async fn platform_lifecycle_role(pool: &PgPool) -> Uuid {
    sqlx::query_scalar(
        "SELECT id FROM roles WHERE cooperative_id IS NULL \
           AND lower(name) = lower('platform-lifecycle')",
    )
    .fetch_one(pool)
    .await
    .expect("platform-lifecycle seed role")
}

/// Cooperative-scoped request: same sealed session, but the tenant
/// extractor binds the bootstrap cooperative.
fn scoped_request(
    method: &str,
    path: &str,
    cookie: &str,
    csrf: Option<&str>,
    coop: Uuid,
    body: Option<Value>,
) -> Request<Body> {
    let mut request = request_builder(method, path, cookie, csrf, body);
    request.headers_mut().insert(
        "x-cooperative-id",
        axum::http::HeaderValue::from_str(&coop.to_string()).unwrap(),
    );
    request
}

/// A bootstrap-scope administrator holding roles.manage + users.manage
/// cannot rename, disable or permission-strip the NULL-scope platform
/// role — through the boundary definers on the restricted lane, nor
/// through the HTTP routes. The row must survive untouched.
#[tokio::test]
async fn bootstrap_admin_cannot_mutate_platform_role() {
    let test = setup().await;
    let coop = bootstrap_coop(&test.pool).await;
    let admin = create_user(&test.pool, "p6b2-badmin").await;
    grant_cooperative(&test.pool, admin, &["roles.manage", "users.manage"]).await;
    let role = platform_lifecycle_role(&test.pool).await;

    // Sealed bootstrap-cooperative session ON THE RESTRICTED PRINCIPAL —
    // the exact authority a bootstrap administrator exercises inside the
    // boundary definers (the API layer is not the enforcement surface).
    let token = vec![0xB2u8; 32];
    sqlx::query(
        "INSERT INTO user_sessions \
           (user_id, token_hash, csrf_token, created_at, last_seen_at, expires_at, idle_expires_at) \
         VALUES ($1,$2,'csrf',now(),now(),now() + interval '1 day',now() + interval '1 day')",
    )
    .bind(admin)
    .bind(&token)
    .execute(&test.pool)
    .await
    .unwrap();

    // Rename via the boundary definer — must return NULL (outside
    // write scope), not touch the row.
    let (mut tx, _ctx) =
        app_db::request_tx(&test.app_pool, &token, coop, OffsetDateTime::now_utc())
            .await
            .expect("sealed bootstrap tx");
    let renamed: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_role_update($1, 'coop-takeover', NULL)")
            .bind(role)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(
        renamed.is_none(),
        "platform role rename through the boundary definer must be refused"
    );
    tx.rollback().await.unwrap();

    // Disable via the boundary definer — must not touch the row.
    let (mut tx, _ctx) =
        app_db::request_tx(&test.app_pool, &token, coop, OffsetDateTime::now_utc())
            .await
            .expect("sealed bootstrap tx");
    let disabled: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_role_set_status($1, 'disabled')")
            .bind(role)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(
        disabled.is_none(),
        "platform role disable through the boundary definer must be refused"
    );
    tx.rollback().await.unwrap();

    // Permission-set rewrite via the boundary definer — must raise.
    let (mut tx, _ctx) =
        app_db::request_tx(&test.app_pool, &token, coop, OffsetDateTime::now_utc())
            .await
            .expect("sealed bootstrap tx");
    let stripped = sqlx::query("SELECT public.app_role_permissions_set($1, '{}'::text[])")
        .bind(role)
        .execute(&mut *tx)
        .await;
    assert!(
        stripped.is_err(),
        "platform role permission rewrite must raise"
    );
    tx.rollback().await.unwrap();

    // The HTTP surface stays closed as well (route-level check).
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b2-badmin").await;
    let (status, _) = send(
        &test.app,
        scoped_request(
            "PATCH",
            &format!("/api/roles/{role}"),
            &cookie,
            Some(&csrf),
            coop,
            Some(json!({ "name": "coop-takeover" })),
        ),
    )
    .await;
    assert!(
        !status.is_success(),
        "platform role rename must be denied, got {status}"
    );
    let (status, _) = send(
        &test.app,
        scoped_request(
            "POST",
            &format!("/api/roles/{role}/disable"),
            &cookie,
            Some(&csrf),
            coop,
            None,
        ),
    )
    .await;
    // The route may answer an idempotent NO_CONTENT for a visible-but-
    // unwritable row — the authoritative check is that the role still
    // reads 'active' below (the boundary definer refused the write).
    assert!(
        matches!(status, StatusCode::NO_CONTENT) || !status.is_success(),
        "unexpected status for platform role disable: {status}"
    );

    // Everything survived byte-identical.
    let (name, status_db, perm_count): (String, String, i64) = sqlx::query_as(
        "SELECT r.name, r.status, \
                (SELECT count(*) FROM role_permissions rp \
                   JOIN permissions p ON p.id = rp.permission_id \
                  WHERE rp.role_id = r.id AND p.key = 'cooperatives.manage') \
           FROM roles r WHERE r.id = $1",
    )
    .bind(role)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(name, "platform-lifecycle");
    assert_eq!(status_db, "active");
    assert_eq!(perm_count, 1, "cooperatives.manage must survive");

    test.cleanup().await;
}

/// The cooperative user-roles replace must never strip NULL-scope
/// assignments: a platform administrator who is also a bootstrap member
/// keeps the platform grant (and stays operational) after the
/// cooperative replace runs.
#[tokio::test]
async fn bootstrap_admin_cannot_strip_platform_assignment() {
    let test = setup().await;
    let coop = bootstrap_coop(&test.pool).await;

    // Platform administrator: NULL-scope URA on the seeded platform role
    // — the exact shape an operator-granted lifecycle admin holds.
    let padmin = create_user(&test.pool, "p6b2-padminB").await;
    repo::add_member(&test.pool, "Ana Kooperatif", padmin, None)
        .await
        .unwrap();
    let role = platform_lifecycle_role(&test.pool).await;
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
         VALUES ($1, $2, NULL)",
    )
    .bind(padmin)
    .bind(role)
    .execute(&test.pool)
    .await
    .unwrap();

    let badmin = create_user(&test.pool, "p6b2-badminB").await;
    grant_cooperative(&test.pool, badmin, &["roles.manage", "users.manage"]).await;
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b2-badminB").await;

    // Cooperative-scope replace of the platform admin's roles: the
    // request itself is legitimate (target is a bootstrap member), so
    // it succeeds — but must not touch the NULL-scope assignment.
    let coop_role: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE cooperative_id = $1 LIMIT 1")
            .bind(coop)
            .fetch_one(&test.pool)
            .await
            .unwrap();
    let (status, body) = send(
        &test.app,
        scoped_request(
            "PUT",
            &format!("/api/users/{padmin}/roles"),
            &cookie,
            Some(&csrf),
            coop,
            Some(json!({ "roleIds": [coop_role] })),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "cooperative-scope role replace is legitimate: {status} {body}"
    );

    let null_ura: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_role_assignments \
          WHERE user_id = $1 AND role_id = $2 AND cooperative_id IS NULL",
    )
    .bind(padmin)
    .bind(role)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(null_ura, 1, "NULL-scope platform assignment must survive");

    // And the platform administrator remains operational end-to-end.
    let (pcookie, _, _) = login(&test.app, test.peer, "p6b2-padminB").await;
    let (status, _) = send(
        &test.app,
        request_builder("GET", "/api/platform/cooperatives", &pcookie, None, None),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "platform grant must still authorize"
    );

    test.cleanup().await;
}

/// The platform role is not assignable from any cooperative scope, and
/// a cooperative-owned role cannot receive the platform-only key —
/// closing the indirect 'cooperatives.manage' route.
#[tokio::test]
async fn platform_role_and_key_are_ungrantable_from_cooperative_scope() {
    let test = setup().await;
    let coop = bootstrap_coop(&test.pool).await;

    let member = create_user(&test.pool, "p6b2-memberX").await;
    repo::add_member(&test.pool, "Ana Kooperatif", member, None)
        .await
        .unwrap();
    let badmin = create_user(&test.pool, "p6b2-badminC").await;
    grant_cooperative(&test.pool, badmin, &["roles.manage", "users.manage"]).await;
    let (cookie, csrf, _) = login(&test.app, test.peer, "p6b2-badminC").await;
    let platform_role = platform_lifecycle_role(&test.pool).await;

    // Assigning the NULL-scope platform role via the cooperative
    // user-roles endpoint must fail (invalid_role_assignment).
    let (status, _) = send(
        &test.app,
        scoped_request(
            "PUT",
            &format!("/api/users/{member}/roles"),
            &cookie,
            Some(&csrf),
            coop,
            Some(json!({ "roleIds": [platform_role] })),
        ),
    )
    .await;
    assert!(
        !status.is_success(),
        "platform role must not be assignable from cooperative scope, got {status}"
    );

    // Indirect route: grant cooperatives.manage to a COOPERATIVE-owned
    // role through the permissions endpoint — denied outright (0069
    // platform-key guard).
    let coop_role: Uuid = sqlx::query_scalar(
        "SELECT role_id FROM user_role_assignments \
          WHERE user_id = $1 AND cooperative_id = $2 LIMIT 1",
    )
    .bind(badmin)
    .bind(coop)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    let (status, _) = send(
        &test.app,
        scoped_request(
            "PUT",
            &format!("/api/roles/{coop_role}/permissions"),
            &cookie,
            Some(&csrf),
            coop,
            Some(json!({ "permissions": ["roles.manage", "cooperatives.manage"] })),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "cooperatives.manage must be ungrantable inside a cooperative"
    );

    let leaked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_permissions rp \
           JOIN permissions p ON p.id = rp.permission_id \
          WHERE rp.role_id = $1 AND p.key = 'cooperatives.manage'",
    )
    .bind(coop_role)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(leaked, 0, "platform key must never land on a coop role");

    test.cleanup().await;
}

// ── M1-P7-B: /me capability-flag matrix ─────────────────────────────
//
// `platformCooperativesManage` follows NULL-scope `cooperatives.manage`
// only; `platformAdmin` follows NULL-scope `users.manage` only. Neither
// platform key implies the other, and a cooperative-scoped (even
// forged) `cooperatives.manage` grant never lights the flag.
#[tokio::test]
async fn me_platform_capability_flags_follow_their_own_null_scope_keys() {
    let test = setup().await;

    // A: NULL-scope cooperatives.manage only — the seeded
    //    `platform-lifecycle` least-privilege shape.
    let lifecycle = create_user(&test.pool, "p6b2-lconly").await;
    grant_platform(&test.pool, lifecycle, &["cooperatives.manage"]).await;
    // B: NULL-scope users.manage only — the legacy bootstrap-admin
    //    shape (cooperatives.manage is deliberately never granted to it).
    let users_admin = create_user(&test.pool, "p6b2-usronly").await;
    grant_platform(&test.pool, users_admin, &["users.manage"]).await;
    // C: both keys at NULL scope.
    let both = create_user(&test.pool, "p6b2-both").await;
    grant_platform(&test.pool, both, &["users.manage", "cooperatives.manage"]).await;
    // D: neither key.
    create_user(&test.pool, "p6b2-none").await;
    // E: forged cooperative-SCOPED cooperatives.manage — the 0069
    //    boundary rejects it in definers, so this raw INSERT models an
    //    out-of-band grant; it must still resolve to neither flag.
    let scoped = create_user(&test.pool, "p6b2-scoped").await;
    grant_cooperative(&test.pool, scoped, &["cooperatives.manage"]).await;

    for (username, expect_admin, expect_coops) in [
        ("p6b2-lconly", false, true),
        ("p6b2-usronly", true, false),
        ("p6b2-both", true, true),
        ("p6b2-none", false, false),
        ("p6b2-scoped", false, false),
    ] {
        let (cookie, _, _) = login(&test.app, test.peer, username).await;
        let (status, me) = send(
            &test.app,
            request_builder("GET", "/api/auth/me", &cookie, None, None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{username}: {me}");
        assert_eq!(
            me["platformAdmin"].as_bool().unwrap(),
            expect_admin,
            "{username} platformAdmin"
        );
        assert_eq!(
            me["platformCooperativesManage"].as_bool().unwrap(),
            expect_coops,
            "{username} platformCooperativesManage"
        );
    }
}
