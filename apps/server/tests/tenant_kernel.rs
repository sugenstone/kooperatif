//! M1-P0 tenant-kernel integration tests (REQ-024.1/024.3 foundation).
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same harness as tests/rbac.rs). Scope honesty: these tests prove the
//! P0 kernel only — cooperative entity, memberships, tenant-context
//! resolution and the business_enabled gate. They do NOT prove
//! cooperative-scoped authorization (P1), business-table isolation
//! (P2) or RLS enforcement (P4); those matrices live in
//! implementation/M1-TENANT-ISOLATION-TEST-MATRIX.md.

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
const TEST_PASSWORD: &str = "tenant-test-parola-123";

fn test_database_url() -> Option<String> {
    let value = std::env::var("KOOPERATIF_TEST_DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "KOOPERATIF_TEST_DATABASE_URL is not set — required DB-gated              integration tests cannot silently pass; point it at a              disposable PostgreSQL database"
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
    coop: Option<&str>,
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
    if let Some(coop) = coop {
        builder = builder.header("x-cooperative-id", coop);
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

async fn bootstrap(pool: &PgPool, name: &str) -> (Uuid, i64) {
    kooperatif_server::tenant::repo::bootstrap_initial_cooperative(pool, name)
        .await
        .expect("bootstrap")
}

// --- Schema / bootstrap -------------------------------------------------

#[tokio::test]
async fn tenant_tables_and_columns_exist_after_migration() {
    let Some(test) = setup().await else { return };
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name IN \
             ('cooperatives','cooperative_memberships') ORDER BY 1",
    )
    .fetch_all(&test.pool)
    .await
    .expect("catalog");
    assert_eq!(tables, vec!["cooperative_memberships", "cooperatives"]);

    for (table, column) in [
        ("user_sessions", "active_cooperative_id"),
        ("security_events", "cooperative_id"),
    ] {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM information_schema.columns \
             WHERE table_schema='public' AND table_name=$1 AND column_name=$2)",
        )
        .bind(table)
        .bind(column)
        .fetch_one(&test.pool)
        .await
        .expect("column check");
        assert!(exists, "{table}.{column} missing");
    }
    test.cleanup().await;
}

#[tokio::test]
async fn bootstrap_is_idempotent_and_enrolls_every_user() {
    let Some(test) = setup().await else { return };
    create_user(&test.pool, "kullanici_bir").await;
    create_user(&test.pool, "kullanici_iki").await;

    let (coop_id, count) = bootstrap(&test.pool, "Ana Kooperatif").await;
    assert_eq!(count, 2);

    // Repeat: no duplicate cooperative, no duplicate memberships.
    let (again, count2) = bootstrap(&test.pool, "Ana Kooperatif").await;
    assert_eq!(again, coop_id);
    assert_eq!(count2, 2);

    let coops: i64 = sqlx::query_scalar("SELECT count(*) FROM cooperatives")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(coops, 1, "bootstrap must never duplicate the cooperative");

    let bootstrap_flags: i64 =
        sqlx::query_scalar("SELECT count(*) FROM cooperatives WHERE is_bootstrap")
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert_eq!(bootstrap_flags, 1);
    test.cleanup().await;
}

#[tokio::test]
async fn second_bootstrap_marker_is_rejected_by_constraint() {
    let Some(test) = setup().await else { return };
    bootstrap(&test.pool, "Ana Kooperatif").await;
    let result = sqlx::query(
        "INSERT INTO cooperatives (name, status, is_bootstrap, business_enabled) \
         VALUES ('İkinci', 'active', true, true)",
    )
    .execute(&test.pool)
    .await;
    assert!(
        result.is_err(),
        "partial unique index must reject a second bootstrap cooperative"
    );
    test.cleanup().await;
}

#[tokio::test]
async fn create_cooperative_rejects_duplicate_name_case_insensitive() {
    let Some(test) = setup().await else { return };
    kooperatif_server::tenant::repo::create_cooperative(&test.pool, "Deneme Koop", None)
        .await
        .expect("first create");
    let dup =
        kooperatif_server::tenant::repo::create_cooperative(&test.pool, "deneme koop", None).await;
    assert!(matches!(
        dup,
        Err(kooperatif_server::tenant::repo::CreateCooperativeError::DuplicateName)
    ));
    test.cleanup().await;
}

#[tokio::test]
async fn membership_is_unique_per_cooperative_and_reactivatable() {
    let Some(test) = setup().await else { return };
    let user = create_user(&test.pool, "uye_bir").await;
    kooperatif_server::tenant::repo::create_cooperative(&test.pool, "Koop A", None)
        .await
        .unwrap();
    let first = kooperatif_server::tenant::repo::add_member(&test.pool, "Koop A", user, None)
        .await
        .expect("member added");
    let second = kooperatif_server::tenant::repo::add_member(&test.pool, "koop a", user, None)
        .await
        .expect("re-add resolves to same row");
    assert_eq!(first.id, second.id, "membership upsert must not duplicate");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM cooperative_memberships WHERE user_id = $1")
            .bind(user)
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    test.cleanup().await;
}

// --- HTTP context endpoints ---------------------------------------------

#[tokio::test]
async fn me_reports_cooperatives_and_session_default() {
    let Some(test) = setup().await else { return };
    create_user(&test.pool, "me_kullanici").await;
    let (coop_id, _) = bootstrap(&test.pool, "Ana Kooperatif").await;

    let (cookie, csrf, _) = login(&test.app, test.peer(), "me_kullanici").await;
    let (status, body) = send(
        &test.app,
        request_builder("GET", "/api/auth/me", &cookie, Some(&csrf), None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let coops = body["cooperatives"].as_array().expect("cooperatives");
    assert_eq!(coops.len(), 1);
    assert_eq!(
        coops[0]["cooperativeId"].as_str().unwrap(),
        coop_id.to_string()
    );
    assert_eq!(coops[0]["membershipStatus"], "active");
    assert_eq!(coops[0]["businessEnabled"], true);
    assert!(body["activeCooperativeId"].is_null());
    test.cleanup().await;
}

#[tokio::test]
async fn cooperative_switch_and_context_resolution() {
    let Some(test) = setup().await else { return };
    create_user(&test.pool, "gecis_kullanici").await;
    let (coop_id, _) = bootstrap(&test.pool, "Ana Kooperatif").await;
    let (cookie, csrf, _) = login(&test.app, test.peer(), "gecis_kullanici").await;

    // Missing context fails closed (no header, no session default yet).
    let (status, body) = send(
        &test.app,
        request_builder(
            "GET",
            "/api/auth/cooperative-context",
            &cookie,
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "cooperative_context_required");

    // Malformed header is rejected, never parsed loosely.
    let (status, _) = send(
        &test.app,
        request_builder(
            "GET",
            "/api/auth/cooperative-context",
            &cookie,
            None,
            Some("not-a-uuid"),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Explicit selection persists the session default (audited).
    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/auth/cooperative",
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "cooperativeId": coop_id })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Session default now resolves the context without a header.
    let (status, body) = send(
        &test.app,
        request_builder(
            "GET",
            "/api/auth/cooperative-context",
            &cookie,
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["cooperativeId"].as_str().unwrap(), coop_id.to_string());
    assert_eq!(body["cooperativeName"], "Ana Kooperatif");

    // Explicit header wins (same coop here — precedence proven by the
    // header path in the foreign-coop test below).
    let (status, _) = send(
        &test.app,
        request_builder(
            "GET",
            "/api/auth/cooperative-context",
            &cookie,
            None,
            Some(&coop_id.to_string()),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let audited: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM security_events \
         WHERE event_type = 'cooperative_context_switched' AND cooperative_id = $1)",
    )
    .bind(coop_id)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert!(audited, "cooperative switch must be audited with coop id");
    test.cleanup().await;
}

#[tokio::test]
async fn unknown_or_foreign_cooperative_context_is_denied() {
    let Some(test) = setup().await else { return };
    create_user(&test.pool, "deneme_u1").await;
    bootstrap(&test.pool, "Ana Kooperatif").await;
    let (cookie, csrf, _) = login(&test.app, test.peer(), "deneme_u1").await;

    // Random UUID: same 403 as a real-but-foreign coop — existence is
    // never distinguished (docs/21:86).
    let random = Uuid::new_v4();
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/auth/cooperative",
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "cooperativeId": random })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "cooperative_access_denied");

    let (status, body) = send(
        &test.app,
        request_builder(
            "GET",
            "/api/auth/cooperative-context",
            &cookie,
            None,
            Some(&random.to_string()),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "cooperative_access_denied");
    test.cleanup().await;
}

#[tokio::test]
async fn suspended_membership_denies_context_immediately() {
    let Some(test) = setup().await else { return };
    let user_id = create_user(&test.pool, "askili_u").await;
    let (coop_id, _) = bootstrap(&test.pool, "Ana Kooperatif").await;
    let (cookie, csrf, _) = login(&test.app, test.peer(), "askili_u").await;
    send(
        &test.app,
        request_builder(
            "POST",
            "/api/auth/cooperative",
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "cooperativeId": coop_id })),
        ),
    )
    .await;

    sqlx::query(
        "UPDATE cooperative_memberships SET status = 'suspended' \
         WHERE user_id = $1 AND cooperative_id = $2",
    )
    .bind(user_id)
    .bind(coop_id)
    .execute(&test.pool)
    .await
    .expect("suspend");

    // Stale session default must not rescue a suspended membership:
    // every request re-validates (PD-02).
    let (status, body) = send(
        &test.app,
        request_builder(
            "GET",
            "/api/auth/cooperative-context",
            &cookie,
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "cooperative_access_denied");
    test.cleanup().await;
}

#[tokio::test]
async fn non_enabled_cooperative_is_unreachable_even_for_members() {
    let Some(test) = setup().await else { return };
    let user_id = create_user(&test.pool, "coklu_u").await;
    let (main_coop, _) = bootstrap(&test.pool, "Ana Kooperatif").await;
    // Second cooperative exists (P0 provisioning) but is NOT
    // business-enabled — the M1 §5 feature gate.
    let second = kooperatif_server::tenant::repo::create_cooperative(&test.pool, "Yeni Koop", None)
        .await
        .expect("second coop");
    assert!(!second.business_enabled);
    // The gate scenario: an *active* cooperative whose business
    // retrofit is pending — business_enabled=false must still block.
    sqlx::query("UPDATE cooperatives SET status = 'active' WHERE id = $1")
        .bind(second.id)
        .execute(&test.pool)
        .await
        .expect("activate coop");
    kooperatif_server::tenant::repo::add_member(&test.pool, "Yeni Koop", user_id, None)
        .await
        .expect("member of second coop");

    let (cookie, csrf, _) = login(&test.app, test.peer(), "coklu_u").await;

    // me lists both memberships; the not-ready one is flagged.
    let (_, body) = send(
        &test.app,
        request_builder("GET", "/api/auth/me", &cookie, Some(&csrf), None, None),
    )
    .await;
    let coops = body["cooperatives"].as_array().unwrap();
    assert_eq!(coops.len(), 2);
    let disabled = coops
        .iter()
        .find(|c| c["cooperativeId"] == json!(second.id.to_string()))
        .expect("second coop listed");
    assert_eq!(disabled["businessEnabled"], false);

    // Selecting the gated cooperative is refused.
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/auth/cooperative",
            &cookie,
            Some(&csrf),
            None,
            Some(json!({ "cooperativeId": second.id })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "cooperative_not_ready");

    // Header-scoped context to the gated coop is refused the same way.
    let (status, body) = send(
        &test.app,
        request_builder(
            "GET",
            "/api/auth/cooperative-context",
            &cookie,
            None,
            Some(&second.id.to_string()),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "cooperative_not_ready");

    // The enabled bootstrap coop still resolves fine — one user, two
    // memberships, independent reachability (PD-02).
    let (status, body) = send(
        &test.app,
        request_builder(
            "GET",
            "/api/auth/cooperative-context",
            &cookie,
            None,
            Some(&main_coop.to_string()),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["cooperativeId"].as_str().unwrap(),
        main_coop.to_string()
    );
    test.cleanup().await;
}

#[tokio::test]
async fn existing_single_cooperative_endpoints_are_unaffected() {
    // M0 regression safety for P0: legacy (non-tenant) endpoints keep
    // working with no cooperative context at all — the tenant layer is
    // additive, not wired into business routers yet (P2).
    let Some(test) = setup().await else { return };
    create_user(&test.pool, "eski_u").await;
    bootstrap(&test.pool, "Ana Kooperatif").await;
    let (cookie, csrf, _) = login(&test.app, test.peer(), "eski_u").await;

    let (status, _) = send(
        &test.app,
        request_builder(
            "GET",
            "/api/auth/sessions",
            &cookie,
            Some(&csrf),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // A cooperative header on a non-tenant endpoint is inert: it must
    // neither break nor alter the response contract.
    let (status, _) = send(
        &test.app,
        request_builder(
            "GET",
            "/api/auth/sessions",
            &cookie,
            Some(&csrf),
            Some(&Uuid::new_v4().to_string()),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    test.cleanup().await;
}
