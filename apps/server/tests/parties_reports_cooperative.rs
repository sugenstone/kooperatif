//! M1-P1-010 (P1-c3b) — cooperative-scoped parties + reports tests.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same harness as tests/rbac_cooperative.rs). These tests run the REAL
//! router — the P0 business-membership gate included — and prove that
//! the migrated parties and reports endpoints resolve authorization
//! only within the verified bootstrap scope:
//!   - headerless requests pin bootstrap; a stale session default or a
//!     foreign header never re-scopes;
//!   - malformed/unknown/non-bootstrap `x-cooperative-id` rejects;
//!   - suspended/ended bootstrap membership is denied on reads and
//!     mutations alike;
//!   - permissions granted in Cooperative B never authorize bootstrap
//!     (A) parties or reports operations;
//!   - NULL-attributed legacy roles keep working inside bootstrap;
//!   - mutations keep both CSRF layers and audit events carry
//!     cooperative attribution;
//!   - the never-bootstrapped lane keeps global semantics untouched.

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

const TEST_PASSWORD: &str = "p1c3b-test-parola-123";
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
    app_db::run_test_migrations(&pool)
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
    users::create_user(pool, username, "P1c3b Test", &hash, TEST_PASSWORD)
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
        .header("user-agent", "Mozilla/5.0 P1c3b")
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

const OPERATOR_PERMS: &[&str] = &[
    "shareholders.read",
    "shareholders.manage",
    "families.read",
    "families.manage",
    "reports.read",
];

/// Bootstrap-scoped operator fixture: user created BEFORE bootstrap
/// (auto-member) with a bootstrap-owned business role.
async fn bootstrap_operator(pool: &PgPool, username: &str) -> (Uuid, Uuid) {
    let user = create_user(pool, username).await;
    let coop = bootstrap(pool).await;
    grant_role(
        pool,
        user,
        &format!("op-{username}"),
        Some(coop),
        OPERATOR_PERMS,
    )
    .await;
    (user, coop)
}

#[tokio::test]
async fn bootstrap_operator_reaches_parties_and_reports() {
    let Some(test) = setup().await else { return };
    let (_, coop) = bootstrap_operator(&test.pool, "operator").await;
    let (cookie, csrf) = login(&test.app, test.peer, "operator").await;

    // Headerless bootstrap access on a read.
    let (status, body) = request_json(
        &test,
        Method::GET,
        "/api/shareholders",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(serde_json::from_str::<Value>(&body).unwrap()["items"].is_array());

    // Mutation with CSRF — creates a family.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/families",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "sequenceNumber": 42 })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Reports read.
    let (status, body) = request_json(
        &test,
        Method::GET,
        "/api/reports/overview",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let overview: Value = serde_json::from_str(&body).unwrap();
    assert!(overview["assessmentsTotal"].is_string());
    assert!(overview["financialAccountsBalance"].is_string());

    // Explicit bootstrap header pins the same scope — allowed.
    let coop_str = coop.to_string();
    let (status, _) = request_json(
        &test,
        Method::GET,
        "/api/shareholders",
        &cookie,
        None,
        Some(&coop_str),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    test.cleanup().await;
}

#[tokio::test]
async fn cooperative_header_rules_apply_to_parties_and_reports() {
    let Some(test) = setup().await else { return };
    bootstrap_operator(&test.pool, "hdr_op").await;
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;
    let (cookie, csrf) = login(&test.app, test.peer, "hdr_op").await;
    let b_str = coop_b.to_string();
    let unknown = Uuid::new_v4().to_string();

    for path in ["/api/shareholders", "/api/reports/overview"] {
        // Malformed UUID → 400.
        let (status, _) = request_json(
            &test,
            Method::GET,
            path,
            &cookie,
            None,
            Some("not-a-uuid"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
        // Explicit non-bootstrap cooperative → 403, never a silent
        // bootstrap fallback.
        let (status, _) =
            request_json(&test, Method::GET, path, &cookie, None, Some(&b_str), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        // Unknown-but-well-formed UUID → 403 (existence never leaked).
        let (status, _) = request_json(
            &test,
            Method::GET,
            path,
            &cookie,
            None,
            Some(&unknown),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    // Mutations obey the same rules.
    let (status, _) = request_json(
        &test,
        Method::POST,
        "/api/families",
        &cookie,
        Some(&csrf),
        Some(&b_str),
        Some(json!({ "sequenceNumber": 7 })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    test.cleanup().await;
}

#[tokio::test]
async fn suspended_and_ended_memberships_are_denied() {
    let Some(test) = setup().await else { return };
    let (user, coop) = bootstrap_operator(&test.pool, "askida").await;
    let (cookie, csrf) = login(&test.app, test.peer, "askida").await;

    for status in ["suspended", "ended"] {
        sqlx::query(
            "UPDATE cooperative_memberships SET status = $1 \
             WHERE user_id = $2 AND cooperative_id = $3",
        )
        .bind(status)
        .bind(user)
        .bind(coop)
        .execute(&test.pool)
        .await
        .expect("membership status");
        // Read denied.
        let (code, _) = request_json(
            &test,
            Method::GET,
            "/api/shareholders",
            &cookie,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "{status} read");
        // Report denied.
        let (code, _) = request_json(
            &test,
            Method::GET,
            "/api/reports/overview",
            &cookie,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "{status} report");
        // Mutation denied — authorization precedes any write.
        let (code, _) = request_json(
            &test,
            Method::POST,
            "/api/families",
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "sequenceNumber": 55 })),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN, "{status} mutation");
    }
    // No family was written through the denied mutations.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM families WHERE sequence_number = 55")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);

    test.cleanup().await;
}

#[tokio::test]
async fn foreign_cooperative_permissions_never_authorize_bootstrap_ops() {
    let Some(test) = setup().await else { return };
    let coop_a = bootstrap(&test.pool).await;
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;

    // A-member whose only rights live in B (B-admin): A sees nothing.
    let b_admin = create_user(&test.pool, "b_yetkili").await;
    repo::add_member(&test.pool, "Ana Kooperatif", b_admin, None)
        .await
        .expect("A member");
    repo::add_member(&test.pool, "B Kooperatifi", b_admin, None)
        .await
        .expect("B member");
    grant_role(&test.pool, b_admin, "b-admin", Some(coop_b), OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "b_yetkili").await;
    let (status, _) = request_json(
        &test,
        Method::GET,
        "/api/shareholders",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = request_json(
        &test,
        Method::GET,
        "/api/reports/overview",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = request_json(
        &test,
        Method::POST,
        "/api/families",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "sequenceNumber": 66 })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Inverse direction: A-viewer / B-admin — A's read works, B's
    // manage power never crosses into A.
    let dual = create_user(&test.pool, "cift").await;
    repo::add_member(&test.pool, "Ana Kooperatif", dual, None)
        .await
        .expect("A member");
    repo::add_member(&test.pool, "B Kooperatifi", dual, None)
        .await
        .expect("B member");
    grant_role(
        &test.pool,
        dual,
        "a-izleyici",
        Some(coop_a),
        &["families.read"],
    )
    .await;
    grant_role(&test.pool, dual, "b-admin2", Some(coop_b), OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "cift").await;
    let (status, _) = request_json(
        &test,
        Method::GET,
        "/api/families",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // families.manage exists only in B → A mutation denied.
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/families",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "sequenceNumber": 77 })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    test.cleanup().await;
}

#[tokio::test]
async fn null_attributed_legacy_roles_keep_working() {
    let Some(test) = setup().await else { return };
    bootstrap(&test.pool).await;
    // Post-bootstrap user with NULL-cooperative role + assignment
    // (legacy lane) and an explicit bootstrap membership — the
    // verified attribution keeps the grant usable.
    let legacy = create_user(&test.pool, "mirasci").await;
    repo::add_member(&test.pool, "Ana Kooperatif", legacy, None)
        .await
        .expect("A member");
    grant_role(&test.pool, legacy, "miras-rol", None, OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "mirasci").await;
    let (status, body) = request_json(
        &test,
        Method::GET,
        "/api/shareholders",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/families",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "sequenceNumber": 88 })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    test.cleanup().await;
}

#[tokio::test]
async fn unauthorized_reads_mutations_and_reports_are_denied() {
    let Some(test) = setup().await else { return };
    bootstrap(&test.pool).await;
    // Active bootstrap member with NO role at all.
    let nobody = create_user(&test.pool, "yetkisiz").await;
    repo::add_member(&test.pool, "Ana Kooperatif", nobody, None)
        .await
        .expect("A member");
    let (cookie, csrf) = login(&test.app, test.peer, "yetkisiz").await;

    for path in [
        "/api/shareholders",
        "/api/persons",
        "/api/families",
        "/api/reports/overview",
        "/api/reports/movements",
        "/api/reports/assessments",
    ] {
        let (status, body) =
            request_json(&test, Method::GET, path, &cookie, None, None, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body}");
    }
    for (path, body) in [
        ("/api/families", json!({ "sequenceNumber": 99 })),
        (
            "/api/shareholders",
            json!({
                "person": { "mode": "new", "firstName": "X", "lastName": "Y" },
                "family": { "mode": "new", "sequenceNumber": 99 }
            }),
        ),
    ] {
        let (status, body_str) = request_json(
            &test,
            Method::POST,
            path,
            &cookie,
            Some(&csrf),
            None,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body_str}");
    }

    test.cleanup().await;
}

#[tokio::test]
async fn mutations_still_require_both_csrf_layers() {
    let Some(test) = setup().await else { return };
    bootstrap_operator(&test.pool, "csrf_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "csrf_op").await;
    let payload = json!({ "sequenceNumber": 123 });

    // No synchronizer token → CSRF layer rejects before authorization.
    let (status, _) = request_json(
        &test,
        Method::POST,
        "/api/families",
        &cookie,
        None,
        None,
        Some(payload.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Wrong token → same.
    let (status, _) = request_json(
        &test,
        Method::POST,
        "/api/families",
        &cookie,
        Some("bogus-token"),
        None,
        Some(payload.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Correct token → succeeds (proves the prologue ordering).
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/families",
        &cookie,
        Some(&csrf),
        None,
        Some(payload),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    test.cleanup().await;
}

#[tokio::test]
async fn audit_events_carry_verified_cooperative_attribution() {
    let Some(test) = setup().await else { return };
    let (_, coop) = bootstrap_operator(&test.pool, "audit_op").await;
    let (cookie, csrf) = login(&test.app, test.peer, "audit_op").await;

    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/families",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "sequenceNumber": 321 })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/shareholders",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({
            "person": { "mode": "new", "firstName": "Denetim", "lastName": "Örnek" },
            "family": { "mode": "new", "sequenceNumber": 322 }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Every coop-bound event carries the verified bootstrap id — never
    // a client-supplied value.
    let (scoped,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM security_events \
         WHERE cooperative_id = $1 \
           AND event_type IN ('family_created','shareholder_created','person_created')",
    )
    .bind(coop)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert!(scoped >= 3, "expected coop-attributed events, got {scoped}");
    let (unscoped,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM security_events \
         WHERE cooperative_id IS NULL \
           AND event_type IN ('family_created','shareholder_created','person_created')",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(unscoped, 0, "business audit events must be attributed");

    test.cleanup().await;
}

#[tokio::test]
async fn never_bootstrapped_lane_reads_but_cannot_write_unowned() {
    let Some(test) = setup().await else { return };
    // No bootstrap at all — the legacy global lane still opens for
    // READS. C5-3 closed the write side: cooperative_id is NOT NULL on
    // all business tables, so an ownerless write cannot commit.
    let user = create_user(&test.pool, "eski_serit").await;
    grant_role(&test.pool, user, "legacy-op", None, OPERATOR_PERMS).await;
    let (cookie, csrf) = login(&test.app, test.peer, "eski_serit").await;
    let (status, body) = request_json(
        &test,
        Method::GET,
        "/api/shareholders",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = request_json(
        &test,
        Method::GET,
        "/api/reports/overview",
        &cookie,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = request_json(
        &test,
        Method::POST,
        "/api/families",
        &cookie,
        Some(&csrf),
        None,
        Some(json!({ "sequenceNumber": 5 })),
    )
    .await;
    assert!(
        !status.is_success(),
        "unowned business write must fail, got {status}: {body}"
    );
    let families: i64 = sqlx::query_scalar("SELECT count(*) FROM families")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(families, 0, "no unowned family may be written");

    test.cleanup().await;
}
