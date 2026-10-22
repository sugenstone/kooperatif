//! M1-P5 B4 — audit read API adversarial tests (migration 0065).
//! Two lanes under test: `GET /api/audit/events` (cooperative-scoped,
//! `audit.read` in the verified scope) and `GET /api/platform/audit/events`
//! (NULL-scope `audit.read` only). Requires KOOPERATIF_TEST_DATABASE_URL;
//! each test runs in its own throwaway database.

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
use kooperatif_server::tenant::repo;

const ORIGIN: &str = "http://localhost:5173";
const TEST_PASSWORD: &str = "audit-test-parola-1";
const APP_ROLE: &str = "kooperatif_app";
/// Restricted-principal password: `KOOPERATIF_APP_TEST_PASSWORD`
/// overrides for CI; otherwise the documented local dev password.
fn app_password() -> String {
    app_db::test_app_role_password()
}

fn test_database_url() -> String {
    std::env::var("KOOPERATIF_TEST_DATABASE_URL")
        .unwrap_or_else(|_| panic!("KOOPERATIF_TEST_DATABASE_URL is not set"))
}

struct TestApp {
    _claim: app_db::TestDbClaim,
    app: axum::Router,
    pool: PgPool,
    peer: SocketAddr,
}

async fn setup() -> TestApp {
    let url = test_database_url();
    let suffix = Uuid::new_v4().simple();
    let database_name = format!("kooperatif_test_{suffix}");
    let admin_url = url
        .rsplit_once('/')
        .map(|(b, _)| format!("{b}/postgres"))
        .unwrap();
    let base_url = url.rsplit_once('/').map(|(b, _)| b.to_string()).unwrap();
    let admin = app_db::connect(&admin_url).await.expect("admin connect");
    let _ = app_db::drop_stale_test_databases(&admin).await;
    let claim = app_db::create_test_database(&admin, &admin_url, &database_name).await;
    admin.close().await;
    let pool = app_db::connect(&format!("{base_url}/{database_name}"))
        .await
        .expect("db");
    app_db::run_test_migrations(&pool)
        .await
        .expect("migrations");
    repo::bootstrap_initial_cooperative(&pool, "Ana Kooperatif")
        .await
        .expect("bootstrap");

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
    let clock = Arc::new(MutableClock::new(OffsetDateTime::now_utc()));
    let runtime = AuthRuntime::new(config, clock.clone());
    let app = router(
        AppState {
            db: Some(pool.clone()),
            auth: Arc::new(runtime),
        },
        cors_layer(&[]),
    );
    let peer: SocketAddr = "127.0.0.9:44291".parse().unwrap();
    TestApp {
        _claim: claim,
        app,
        pool,
        peer,
    }
}

async fn create_user(pool: &PgPool, username: &str) -> Uuid {
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).unwrap(),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).unwrap();
    users::create_user(pool, username, "Denetim", &hash, TEST_PASSWORD)
        .await
        .unwrap()
        .id
}

/// Active bootstrap membership + a coop-scoped role holding `perms`.
async fn grant_scoped(pool: &PgPool, user: Uuid, perms: &[&str]) {
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         SELECT id, $1, 'active' FROM cooperatives WHERE is_bootstrap \
         ON CONFLICT (cooperative_id, user_id) DO UPDATE SET status = 'active'",
    )
    .bind(user)
    .execute(pool)
    .await
    .unwrap();
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, cooperative_id) \
         VALUES ($1, (SELECT id FROM cooperatives WHERE is_bootstrap)) RETURNING id",
    )
    .bind(format!("rol-{user}"))
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
         VALUES ($1, $2, (SELECT id FROM cooperatives WHERE is_bootstrap))",
    )
    .bind(user)
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
}

/// NULL-scope role + assignment — the platform lane.
async fn grant_platform(pool: &PgPool, user: Uuid, perms: &[&str]) {
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, cooperative_id) VALUES ($1, NULL) RETURNING id",
    )
    .bind(format!("plat-{user}"))
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

async fn login(app: &axum::Router, peer: SocketAddr, username: &str) -> String {
    let mut request = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .body(Body::from(
            json!({ "username": username, "password": TEST_PASSWORD }).to_string(),
        ))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(peer));
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .split(';')
        .next()
        .and_then(|p| p.split_once('='))
        .map(|(_, v)| v.to_string())
        .unwrap()
}

fn get(path: &str, cookie: Option<&str>) -> Request<Body> {
    let mut builder = Request::get(path).header("origin", ORIGIN);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("kooperatif_session={cookie}"));
    }
    builder.body(Body::empty()).unwrap()
}

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Owner-lane fixture write — audit rows are seeded directly because
/// the runtime role can never write them.
async fn seed_event(
    pool: &PgPool,
    coop: Option<Uuid>,
    event_type: &str,
    user: Option<Uuid>,
    metadata: Value,
    at: OffsetDateTime,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO security_events \
             (event_type, cooperative_id, user_id, metadata, occurred_at) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(event_type)
    .bind(coop)
    .bind(user)
    .bind(metadata)
    .bind(at)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn coop_a(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("SELECT id FROM cooperatives WHERE is_bootstrap")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Restricted-principal pool (same rewrite as the realtime harness).
async fn app_pool(app: &TestApp) -> PgPool {
    let db_name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let url = test_database_url();
    let (base, _) = url.rsplit_once('/').unwrap();
    let (scheme, rest) = base.split_once("://").unwrap();
    let at = rest.find('@').unwrap();
    app_db::connect(&format!(
        "{scheme}://{APP_ROLE}:{}{}/{db_name}",
        app_password(),
        &rest[at..]
    ))
    .await
    .expect("kooperatif_app pool")
}

fn at(minute: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_760_000_000 + minute * 60).unwrap()
}

// ── Tests ────────────────────────────────────────────────────────────

/// A cooperative administrator WITHOUT audit.read is denied — the
/// permission is explicit, never implied by users.manage.
#[tokio::test]
async fn admin_without_audit_read_is_denied() {
    let app = setup().await;
    let user = create_user(&app.pool, "aud-noperm").await;
    grant_scoped(&app.pool, user, &["users.read", "users.manage"]).await;
    let cookie = login(&app.app, app.peer, "aud-noperm").await;
    let (status, _) = send(&app.app, get("/api/audit/events", Some(&cookie))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// With a coop-scoped audit.read the lane opens; the response is the
/// allowlisted DTO shape and metadata is filtered per event type.
#[tokio::test]
async fn scoped_admin_reads_own_cooperative_events() {
    let app = setup().await;
    let coop = coop_a(&app.pool).await;
    let user = create_user(&app.pool, "aud-reader").await;
    grant_scoped(&app.pool, user, &["audit.read"]).await;

    let seed_user = create_user(&app.pool, "aud-actor").await;
    seed_event(
        &app.pool,
        Some(coop),
        "payment_posted",
        Some(seed_user),
        json!({
            "payment_id": Uuid::new_v4(),
            "payment_number": "P-1",
            "payer_person_id": Uuid::new_v4(),
            "amount": "150.00",
            "internal_debug_secret": "must-never-leak"
        }),
        at(1),
    )
    .await;

    let cookie = login(&app.app, app.peer, "aud-reader").await;
    let (status, body) = send(&app.app, get("/api/audit/events", Some(&cookie))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["items"].as_array().expect("items");
    let event = items
        .iter()
        .find(|e| e["eventType"] == "payment_posted")
        .expect("seeded event must be visible");

    assert_eq!(event["actorUsername"], "aud-actor");
    assert_eq!(event["cooperativeId"], coop.to_string().as_str());
    assert!(event["occurredAt"].is_string());
    assert!(event["actorUserId"].is_string());
    // DTO allowlist: no session material, no unlisted metadata keys.
    assert!(event.get("sessionId").is_none());
    assert!(event.get("session_id").is_none());
    let metadata = event["metadata"].as_object().unwrap();
    assert!(metadata.contains_key("amount"));
    assert!(metadata.contains_key("payment_id"));
    assert!(!metadata.contains_key("internal_debug_secret"));
}

/// Cooperative B events never reach a cooperative-A reader — both via
/// the default lane and via an explicit foreign cooperativeId.
#[tokio::test]
async fn cross_cooperative_events_and_filters_are_denied() {
    let app = setup().await;
    let coop = coop_a(&app.pool).await;
    let coop_b = repo::create_cooperative(&app.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;
    let user = create_user(&app.pool, "aud-iso").await;
    grant_scoped(&app.pool, user, &["audit.read"]).await;

    seed_event(
        &app.pool,
        Some(coop),
        "period_created",
        None,
        json!({}),
        at(1),
    )
    .await;
    let b_event = seed_event(
        &app.pool,
        Some(coop_b),
        "payment_posted",
        None,
        json!({ "amount": "999.00" }),
        at(2),
    )
    .await;

    let cookie = login(&app.app, app.peer, "aud-iso").await;

    // Default scoped read never carries B rows.
    let (status, body) = send(&app.app, get("/api/audit/events", Some(&cookie))).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_string())
        .collect();
    assert!(
        !ids.contains(&b_event.to_string()),
        "cooperative B event leaked into A's audit read"
    );

    // An explicit foreign cooperativeId is rejected, never served.
    let (status, _) = send(
        &app.app,
        get(
            &format!("/api/audit/events?cooperativeId={coop_b}"),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The verified coop itself is accepted (no enumeration asymmetry).
    let (status, _) = send(
        &app.app,
        get(
            &format!("/api/audit/events?cooperativeId={coop}"),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// Platform events (cooperative_id IS NULL) are invisible on the
/// cooperative lane — coop-scoped audit.read cannot see platform rows.
#[tokio::test]
async fn scoped_lane_never_returns_platform_events() {
    let app = setup().await;
    let user = create_user(&app.pool, "aud-scope").await;
    grant_scoped(&app.pool, user, &["audit.read"]).await;

    // login_failed is a NULL-attributed platform event.
    let platform_event = seed_event(
        &app.pool,
        None,
        "login_failed",
        None,
        json!({ "username": "x", "reason": "invalid_credentials" }),
        at(1),
    )
    .await;

    let cookie = login(&app.app, app.peer, "aud-scope").await;
    let (status, body) = send(
        &app.app,
        get("/api/audit/events?eventType=login_failed", Some(&cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert!(
        !items
            .iter()
            .any(|e| e["id"].as_str() == Some(&platform_event.to_string())),
        "platform event leaked into the cooperative lane"
    );
}

/// Platform lane: NULL-scope audit.read sees everything (D4) including
/// cooperative rows — with an optional cooperativeId filter.
#[tokio::test]
async fn platform_lane_requires_null_scope_audit_read() {
    let app = setup().await;
    let coop = coop_a(&app.pool).await;

    // Coop-scoped audit.read is NOT platform authority.
    let scoped_user = create_user(&app.pool, "aud-notplat").await;
    grant_scoped(&app.pool, scoped_user, &["audit.read"]).await;
    let scoped_cookie = login(&app.app, app.peer, "aud-notplat").await;
    let (status, _) = send(
        &app.app,
        get("/api/platform/audit/events", Some(&scoped_cookie)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "coop-scoped audit.read must not reach the platform lane"
    );

    // A platform administrator with NULL-scope users.manage but NO
    // audit.read is still denied — no implicit bundling.
    let plain_admin = create_user(&app.pool, "aud-usermanage").await;
    grant_platform(&app.pool, plain_admin, &["users.manage"]).await;
    let admin_cookie = login(&app.app, app.peer, "aud-usermanage").await;
    let (status, _) = send(
        &app.app,
        get("/api/platform/audit/events", Some(&admin_cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // NULL-scope audit.read opens the lane.
    let auditor = create_user(&app.pool, "aud-platform").await;
    grant_platform(&app.pool, auditor, &["audit.read"]).await;
    let coop_event = seed_event(
        &app.pool,
        Some(coop),
        "payment_posted",
        None,
        json!({ "amount": "10.00" }),
        at(3),
    )
    .await;
    let other_event = seed_event(&app.pool, None, "logout", None, json!({}), at(4)).await;

    let cookie = login(&app.app, app.peer, "aud-platform").await;
    let (status, body) = send(&app.app, get("/api/platform/audit/events", Some(&cookie))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ids: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&coop_event.to_string()));
    assert!(ids.contains(&other_event.to_string()));

    // CooperativeId filter narrows correctly.
    let (status, body) = send(
        &app.app,
        get(
            &format!("/api/platform/audit/events?cooperativeId={coop}"),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert!(!items.is_empty());
    assert!(items
        .iter()
        .all(|e| e["cooperativeId"].as_str() == Some(&coop.to_string())));
}

/// Keyset pagination is stable under concurrent inserts and cursor
/// tampering is a client error — never a silent page reset.
#[tokio::test]
async fn cursor_pagination_is_stable_and_tamper_safe() {
    let app = setup().await;
    let coop = coop_a(&app.pool).await;
    let user = create_user(&app.pool, "aud-pager").await;
    grant_scoped(&app.pool, user, &["audit.read"]).await;

    for i in 0..5 {
        seed_event(
            &app.pool,
            Some(coop),
            "period_created",
            None,
            json!({ "period_number": i }),
            at(10 + i),
        )
        .await;
    }

    let cookie = login(&app.app, app.peer, "aud-pager").await;

    // Page 1 — newest two.
    let (status, page1) = send(
        &app.app,
        get(
            "/api/audit/events?pageSize=2&eventType=period_created",
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page1}");
    assert_eq!(page1["items"].as_array().unwrap().len(), 2);
    let cursor = page1["nextCursor"].as_str().expect("continuation");

    // Insert a NEWER event mid-iteration: page 2 must be unaffected.
    seed_event(
        &app.pool,
        Some(coop),
        "period_created",
        None,
        json!({ "period_number": 99 }),
        at(50),
    )
    .await;

    let (status, page2) = send(
        &app.app,
        get(
            &format!("/api/audit/events?pageSize=2&eventType=period_created&cursor={cursor}"),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let page2_items = page2["items"].as_array().unwrap();
    assert_eq!(page2_items.len(), 2);
    let page2_cursor = page2["nextCursor"].as_str().expect("continuation");

    let (status, page3) = send(
        &app.app,
        get(
            &format!("/api/audit/events?pageSize=2&eventType=period_created&cursor={page2_cursor}"),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let page3_items = page3["items"].as_array().unwrap();
    // 5 seeded rows: pages of 2/2/1; the mid-iteration insert sits
    // BEFORE the first cursor so it never appears on later pages.
    assert_eq!(page3_items.len(), 1);
    assert!(page3["nextCursor"].is_null());

    // Strictly descending occurred_at across the collected pages.
    let mut seen: Vec<String> = Vec::new();
    for page in [&page1, &page2, &page3] {
        for e in page["items"].as_array().unwrap() {
            seen.push(e["occurredAt"].as_str().unwrap().to_string());
        }
    }
    let mut sorted = seen.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(seen, sorted, "pages must be strictly time-descending");

    // Cursor tampering — garbage, truncated and foreign-shaped values.
    for bad in ["garbage", "AAAA", "bm90LWFjdXJzb3I", ""] {
        let (status, _) = send(
            &app.app,
            get(&format!("/api/audit/events?cursor={bad}"), Some(&cookie)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "cursor {bad:?}");
    }
}

/// Filter validation: unknown event types, bad UUIDs, inverted windows
/// and out-of-range page sizes are 400 — never silently ignored.
#[tokio::test]
async fn invalid_filters_are_rejected() {
    let app = setup().await;
    let user = create_user(&app.pool, "aud-filters").await;
    grant_scoped(&app.pool, user, &["audit.read"]).await;
    let cookie = login(&app.app, app.peer, "aud-filters").await;

    for path in [
        "/api/audit/events?eventType=not_a_real_type",
        "/api/audit/events?actorUserId=not-a-uuid",
        "/api/audit/events?from=2099-01-01T00:00:00Z&to=2020-01-01T00:00:00Z",
        "/api/audit/events?pageSize=0",
        "/api/audit/events?pageSize=101",
        "/api/audit/events?from=not-a-date",
    ] {
        let (status, _) = send(&app.app, get(path, Some(&cookie))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
    }
}

/// Unknown event types are filterable-rejected (400) but rows minted
/// with unlisted types degrade to minimal metadata on read.
#[tokio::test]
async fn unknown_event_type_metadata_is_minimal() {
    let app = setup().await;
    let coop = coop_a(&app.pool).await;
    let user = create_user(&app.pool, "aud-unknown").await;
    grant_scoped(&app.pool, user, &["audit.read"]).await;

    seed_event(
        &app.pool,
        Some(coop),
        "hypothetical_future_event",
        None,
        json!({ "payload": "anything" }),
        at(1),
    )
    .await;

    let cookie = login(&app.app, app.peer, "aud-unknown").await;
    let (status, body) = send(&app.app, get("/api/audit/events", Some(&cookie))).await;
    assert_eq!(status, StatusCode::OK);
    let event = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["eventType"] == "hypothetical_future_event")
        .expect("row still listed — only its payload is minimized");
    assert_eq!(event["metadata"], json!({}));
}

/// The restricted runtime role still holds ZERO table privileges and
/// can only reach audit data through the credential-bound readers —
/// which reject a forged context outright.
#[tokio::test]
async fn restricted_role_has_no_direct_audit_access() {
    let app = setup().await;
    let restricted = app_pool(&app).await;

    for op in ["SELECT", "INSERT", "UPDATE", "DELETE"] {
        let allowed: bool = sqlx::query_scalar(&format!(
            "SELECT has_table_privilege('{APP_ROLE}', 'security_events', '{op}')"
        ))
        .fetch_one(&app.pool)
        .await
        .unwrap();
        assert!(!allowed, "kooperatif_app holds {op} on security_events");
    }

    let err = sqlx::query("SELECT id FROM security_events LIMIT 1")
        .fetch_all(&restricted)
        .await
        .expect_err("direct SELECT must be denied");
    assert_eq!(
        err.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );

    // Writers stay sealed: an UPDATE through the runtime role fails.
    sqlx::query("UPDATE security_events SET event_type = 'forged'")
        .execute(&restricted)
        .await
        .expect_err("audit rows are immutable to the runtime role");

    // The reader itself demands a real session — a garbage token gets
    // insufficient_privilege, never rows.
    let err = sqlx::query(
        "SELECT id FROM public.app_audit_read_scoped(\
             decode('00', 'hex'), \
             (SELECT id FROM cooperatives WHERE is_bootstrap), now(), \
             NULL, NULL, NULL, NULL, NULL, NULL, 10)",
    )
    .fetch_all(&restricted)
    .await
    .expect_err("forged token must be rejected");
    assert_eq!(
        err.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
}

/// A suspended membership closes the read immediately — the reader
/// re-verifies membership on every call, not at token issue.
#[tokio::test]
async fn suspended_membership_loses_audit_access() {
    let app = setup().await;
    let coop = coop_a(&app.pool).await;
    let user = create_user(&app.pool, "aud-susp").await;
    grant_scoped(&app.pool, user, &["audit.read"]).await;
    let cookie = login(&app.app, app.peer, "aud-susp").await;

    let (status, _) = send(&app.app, get("/api/audit/events", Some(&cookie))).await;
    assert_eq!(status, StatusCode::OK);

    sqlx::query(
        "UPDATE cooperative_memberships SET status = 'suspended' \
         WHERE user_id = $1 AND cooperative_id = $2",
    )
    .bind(user)
    .bind(coop)
    .execute(&app.pool)
    .await
    .unwrap();

    let (status, _) = send(&app.app, get("/api/audit/events", Some(&cookie))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// B5: the definer re-verifies the COOPERATIVE's status too — a
/// suspended cooperative closes the read even while the membership row
/// itself is still active.
#[tokio::test]
async fn suspended_cooperative_loses_audit_access() {
    let app = setup().await;
    let coop = coop_a(&app.pool).await;
    let user = create_user(&app.pool, "aud-coopoff").await;
    grant_scoped(&app.pool, user, &["audit.read"]).await;
    let cookie = login(&app.app, app.peer, "aud-coopoff").await;

    let (status, _) = send(&app.app, get("/api/audit/events", Some(&cookie))).await;
    assert_eq!(status, StatusCode::OK);

    sqlx::query("UPDATE cooperatives SET status = 'suspended' WHERE id = $1")
        .bind(coop)
        .execute(&app.pool)
        .await
        .unwrap();

    let (status, _) = send(&app.app, get("/api/audit/events", Some(&cookie))).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "suspended cooperative must close audit reads"
    );
}

/// B5: a globally disabled identity loses audit access — session
/// ownership re-verifies `users.status` on every call.
#[tokio::test]
async fn disabled_user_loses_audit_access() {
    let app = setup().await;
    let user = create_user(&app.pool, "aud-disabled").await;
    grant_scoped(&app.pool, user, &["audit.read"]).await;
    let cookie = login(&app.app, app.peer, "aud-disabled").await;

    let (status, _) = send(&app.app, get("/api/audit/events", Some(&cookie))).await;
    assert_eq!(status, StatusCode::OK);

    sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(user)
        .execute(&app.pool)
        .await
        .unwrap();

    // A disabled identity invalidates the session itself — the request
    // is unauthenticated, never merely unauthorized.
    let (status, _) = send(&app.app, get("/api/audit/events", Some(&cookie))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// B5: DTO shape never carries session material, and the
/// `person_updated` metadata is bounded to the mint-site name fields —
/// the allowlist may not widen into arbitrary person attributes.
#[tokio::test]
async fn dto_shape_excludes_session_fields_and_bounds_metadata() {
    let app = setup().await;
    let coop = coop_a(&app.pool).await;
    let user = create_user(&app.pool, "aud-shape").await;
    grant_scoped(&app.pool, user, &["audit.read"]).await;
    let cookie = login(&app.app, app.peer, "aud-shape").await;

    // Seed a raw event with MORE than the minted shape — the allowlist
    // must project `before`/`after` verbatim (mint-bounded) but never
    // invent keys, and the DTO must not leak session identifiers even
    // when the row carries one.
    let session_id: Uuid =
        sqlx::query_scalar("SELECT id FROM user_sessions ORDER BY created_at DESC LIMIT 1")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO security_events \
             (event_type, cooperative_id, user_id, session_id, metadata, occurred_at) \
         VALUES ('person_updated', $1, $2, $3, \
                 jsonb_build_object('person_id', gen_random_uuid()::text, \
                     'before', jsonb_build_object('first_name','Eski','last_name','Ad'), \
                     'after',  jsonb_build_object('first_name','Yeni','last_name','Ad'), \
                     'injected', 'never-minted-key'), \
                 now())",
    )
    .bind(coop)
    .bind(user)
    .bind(session_id)
    .execute(&app.pool)
    .await
    .unwrap();

    let (status, body) = send(
        &app.app,
        get("/api/audit/events?eventType=person_updated", Some(&cookie)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["items"].as_array().unwrap();
    assert!(!items.is_empty());
    for event in items {
        let obj = event.as_object().unwrap();
        assert!(
            !obj.contains_key("sessionId") && !obj.contains_key("session_id"),
            "session identifiers must never be serialized: {event}"
        );
        assert!(
            !event["metadata"]
                .as_object()
                .unwrap()
                .contains_key("injected"),
            "non-minted metadata keys must be filtered: {event}"
        );
        let before = event["metadata"]["before"].as_object().unwrap();
        assert_eq!(
            before.keys().map(String::as_str).collect::<Vec<_>>(),
            ["first_name", "last_name"],
            "before/after must stay bounded to the minted name fields"
        );
    }
}
