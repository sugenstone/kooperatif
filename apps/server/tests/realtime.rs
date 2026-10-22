//! Database-gated real-time tests (STEP-016, ADR-005, docs/22).
//! Requires KOOPERATIF_TEST_DATABASE_URL; each test runs in its own
//! throwaway database and against a REAL TCP server + REAL WebSocket
//! client (tokio-tungstenite) — no in-memory handshake fakes.
//!
//! Invariants under test:
//! - POST-COMMIT ONLY: a rolled-back INSERT emits no `data-changed`;
//!   the identical committed INSERT does (transactional pg_notify).
//! - AUTHENTICATED: anonymous and wrong-Origin upgrades are rejected
//!   before the socket opens.
//! - SCOPED: a socket subscribed to `payments.read` never receives a
//!   `governance` signal; `reports.read` receives every domain.
//! - NON-PERMANENT AUTHZ: session revocation and permission revocation
//!   terminate delivery on the very next signal — no stale sockets.
//! - CROSS-WRITER: a commit performed through a DIFFERENT connection
//!   (raw pool INSERT, not this process's HTTP handler) is still
//!   signaled — LISTEN/NOTIFY is writer-agnostic and multi-instance safe.
//! - SIGNAL, NOT TRUTH: envelopes carry only {type, domain, occurredAt}
//!   — no amounts, no person data, no identifiers of mutated rows.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;
use uuid::Uuid;

use kooperatif_server::auth::{identity, users, AuthRuntime};
use kooperatif_server::clock::MutableClock;
use kooperatif_server::config::{AuthConfig, ARGON2_M_COST_FLOOR};
use kooperatif_server::db as app_db;
use kooperatif_server::http::{cors_layer, router, AppState};
use kooperatif_server::realtime;
use kooperatif_server::tenant::repo;

/// The fan-out hub is process-global by design (one per server
/// process); tests therefore serialize so one test's commits cannot
/// surface on another test's socket.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const ORIGIN: &str = "http://localhost:5173";
const TEST_PASSWORD: &str = "rt-test-parola-1";
/// Upper bound for "event must arrive" assertions — generous against
/// CI jitter, far below heartbeat cadence.
const EVENT_TIMEOUT: Duration = Duration::from_secs(15);
/// Bound for "event must NOT arrive" assertions.
const SILENCE_TIMEOUT: Duration = Duration::from_secs(4);

/// Held for the test's duration; the guard's Drop releases the lock.
struct SerialGuard(#[allow(dead_code)] tokio::sync::MutexGuard<'static, ()>);

struct TestApp {
    _claim: app_db::TestDbClaim,
    _serial: SerialGuard,
    /// Kept for `oneshot` requests (login, canonical re-reads) that do
    /// not need a real TCP hop.
    app: axum::Router,
    pool: PgPool,
    addr: SocketAddr,
    peer: SocketAddr,
    _server: tokio::task::JoinHandle<()>,
    _listener: tokio::task::JoinHandle<()>,
}

async fn setup() -> Option<TestApp> {
    // PILOT-FIX-001 / F5: missing test DB URL is an explicit failure.
    let url = std::env::var("KOOPERATIF_TEST_DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "KOOPERATIF_TEST_DATABASE_URL is not set — required DB-gated              integration tests cannot silently pass"
        )
    });
    let serial = SerialGuard(SERIAL.lock().await);
    let database_name = format!("kooperatif_test_{}", Uuid::new_v4().simple());
    let (base_url, admin_url) = url
        .rsplit_once('/')
        .map(|(b, _)| (b.to_string(), format!("{b}/postgres")))?;
    let admin = app_db::connect(&admin_url).await.expect("admin connect");
    // F11: bound per-test database accumulation (24 h cutoff, no connections).
    let _ = app_db::drop_stale_test_databases(&admin).await;
    let claim = app_db::create_test_database(&admin, &admin_url, &database_name).await;
    admin.close().await;
    let db_url = format!("{base_url}/{database_name}");
    let pool = app_db::connect(&db_url).await.expect("db");
    app_db::run_test_migrations(&pool)
        .await
        .expect("migrations");
    // C5-3: cooperative_id is NOT NULL on every business table — the
    // NULL legacy lane is closed. Bootstrap up front (idempotent:
    // tests that call it again get the same cooperative).
    let _ = repo::bootstrap_initial_cooperative(&pool, "Ana Kooperatif")
        .await
        .expect("bootstrap cooperative");

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
    let clock = Arc::new(MutableClock::new(
        OffsetDateTime::parse(
            "2026-01-15T00:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .expect("epoch"),
    ));
    let app = router(
        AppState {
            db: Some(pool.clone()),
            auth: Arc::new(AuthRuntime::new(config, clock)),
        },
        cors_layer(&[]),
    );

    // Real listener + real TCP server: NOTIFY crosses a real PG
    // connection, the socket crosses a real TCP socket.
    let listener = tokio::spawn({
        let url = db_url.clone();
        async move {
            realtime::listen_loop(url).await;
        }
    });
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp.local_addr().unwrap();
    let octets = Uuid::new_v4().as_bytes().to_vec();
    let peer: SocketAddr = format!(
        "127.{}.{}.{}:{}",
        octets[0],
        octets[1],
        octets[2],
        52000 + octets[3] as u16
    )
    .parse()
    .expect("addr");
    let serving = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            tcp,
            serving.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Some(TestApp {
        _claim: claim,
        _serial: serial,
        app,
        pool,
        addr,
        peer,
        _server: server,
        _listener: listener,
    })
}

async fn create_user(pool: &PgPool, username: &str, permissions: &[&str]) -> Uuid {
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).unwrap(),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).unwrap();
    let user = users::create_user(pool, username, "RT", &hash, TEST_PASSWORD)
        .await
        .unwrap();
    let role_id: Uuid =
        sqlx::query_scalar("INSERT INTO roles (name, description) VALUES ($1, 'rt') RETURNING id")
            .bind(format!("rol-{}", Uuid::new_v4().simple()))
            .fetch_one(pool)
            .await
            .unwrap();
    for key in permissions {
        sqlx::query(
            "INSERT INTO role_permissions (role_id, permission_id) \
             SELECT $1, id FROM permissions WHERE key = $2",
        )
        .bind(role_id)
        .bind(key)
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::query("INSERT INTO user_role_assignments (user_id, role_id) VALUES ($1, $2)")
        .bind(user.id)
        .bind(role_id)
        .execute(pool)
        .await
        .unwrap();
    // Post-C5-3: signals resolve inside the bootstrap cooperative —
    // membership is required to open a socket.
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         SELECT id, $1, 'active' FROM cooperatives WHERE is_bootstrap",
    )
    .bind(user.id)
    .execute(pool)
    .await
    .unwrap();
    user.id
}

async fn login(app: &TestApp, username: &str) -> String {
    let mut request = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .body(Body::from(
            json!({ "username": username, "password": TEST_PASSWORD }).to_string(),
        ))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(app.peer));
    let response = app.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn ws_connect(
    app: &TestApp,
    cookie: Option<&str>,
    origin: Option<&str>,
) -> Result<Ws, tokio_tungstenite::tungstenite::Error> {
    ws_connect_scoped(app, cookie, origin, None).await
}

/// `cookie` expects a full `name=value` pair; `coop` is sent verbatim
/// as `x-cooperative-id` (None = headerless bootstrap pinning).
async fn ws_connect_scoped(
    app: &TestApp,
    cookie: Option<&str>,
    origin: Option<&str>,
    coop: Option<&str>,
) -> Result<Ws, tokio_tungstenite::tungstenite::Error> {
    let mut request = format!("ws://{}/api/realtime", app.addr)
        .into_client_request()
        .unwrap();
    let headers = request.headers_mut();
    if let Some(cookie) = cookie {
        headers.insert("cookie", cookie.parse().unwrap());
    }
    if let Some(origin) = origin {
        headers.insert("origin", origin.parse().unwrap());
    }
    if let Some(coop) = coop {
        headers.insert("x-cooperative-id", coop.parse().unwrap());
    }
    Ok(tokio_tungstenite::connect_async(request).await?.0)
}

/// Role + assignment fixture. `coop` Some writes composite-FK-consistent
/// scoped rows (the membership row must already exist); None writes the
/// legacy NULL-attributed lane.
async fn grant_role(pool: &PgPool, user: Uuid, name: &str, coop: Option<Uuid>, perms: &[&str]) {
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
}

async fn subscribe(ws: &mut Ws, scopes: &[&str]) -> Value {
    ws.send(Message::Text(
        json!({ "type": "subscribe", "scopes": scopes })
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    // In-flight signed domain events may interleave before the ack
    // (fixture commits race the subscribe write) — skip them.
    let deadline = tokio::time::Instant::now() + EVENT_TIMEOUT;
    loop {
        let message = tokio::time::timeout_at(deadline, ws.next())
            .await
            .expect("subscribed ack timeout")
            .expect("stream ended")
            .expect("frame");
        if let Ok(frame) = serde_json::from_str::<Value>(message.to_text().unwrap()) {
            if frame["type"] == "subscribed" {
                return frame;
            }
        }
    }
}

/// Reads the next `data-changed` domain within the timeout.
async fn next_domain(ws: &mut Ws, timeout: Duration) -> Option<String> {
    loop {
        let message = tokio::time::timeout(timeout, ws.next()).await.ok()??;
        let message = message.ok()?;
        match message {
            Message::Text(text) => {
                let frame: Value = serde_json::from_str(&text).unwrap();
                match frame["type"].as_str() {
                    Some("data-changed") => {
                        assert!(frame["occurredAt"].is_string());
                        return Some(frame["domain"].as_str().unwrap().to_string());
                    }
                    Some(_) => continue, // resync/subscribed — keep waiting
                    None => return None,
                }
            }
            Message::Ping(_) | Message::Pong(_) | Message::Binary(_) => continue,
            Message::Close(_) => return None,
            _ => continue,
        }
    }
}

async fn commit_period(pool: &PgPool, name: &str) {
    sqlx::query(
        "INSERT INTO periods (name, collection_start_date, due_date, cooperative_id, period_number) \
         VALUES ($1, '2026-01-01', '2026-02-01', \
         (SELECT id FROM cooperatives WHERE is_bootstrap), \
         allocate_document_number((SELECT id FROM cooperatives WHERE is_bootstrap), \
             'periods.period_number'))",
    )
    .bind(name)
    .execute(pool)
    .await
    .unwrap();
}

async fn commit_category(pool: &PgPool, name: &str) {
    sqlx::query(
        "INSERT INTO financial_categories (category_type, name, cooperative_id) \
         VALUES ('income', $1, (SELECT id FROM cooperatives WHERE is_bootstrap))",
    )
    .bind(name)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn anonymous_upgrade_rejected() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let result = ws_connect(&app, None, Some(ORIGIN)).await;
    assert!(result.is_err(), "anonymous WebSocket must be rejected");
}

#[tokio::test]
async fn wrong_origin_upgrade_rejected() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    create_user(&app.pool, "rt-origin", &["periods.read"]).await;
    let cookie = login(&app, "rt-origin").await;
    let result = ws_connect(&app, Some(&cookie), Some("https://evil.example")).await;
    assert!(result.is_err(), "non-allowlisted Origin must be rejected");
    // Missing Origin entirely is likewise rejected.
    let result = ws_connect(&app, Some(&cookie), None).await;
    assert!(result.is_err(), "missing Origin must be rejected");
}

#[tokio::test]
async fn committed_insert_signals_rollback_does_not() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    create_user(&app.pool, "rt-commit", &["periods.read"]).await;
    let cookie = login(&app, "rt-commit").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    let ack = subscribe(&mut ws, &["periods.read"]).await;
    assert_eq!(ack["granted"], json!(["periods.read"]));

    // ROLLBACK — transactional pg_notify is discarded with the txn.
    let mut txn = app.pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO periods (name, collection_start_date, due_date, cooperative_id, period_number) \
         VALUES ('rolled-back', '2026-01-01', '2026-02-01', \
         (SELECT id FROM cooperatives WHERE is_bootstrap), \
         allocate_document_number((SELECT id FROM cooperatives WHERE is_bootstrap), \
             'periods.period_number'))",
    )
    .execute(&mut *txn)
    .await
    .unwrap();
    txn.rollback().await.unwrap();
    assert_eq!(
        next_domain(&mut ws, SILENCE_TIMEOUT).await,
        None,
        "rollback must emit nothing"
    );

    // COMMIT — same write, now observable.
    commit_period(&app.pool, "committed-period").await;
    assert_eq!(
        next_domain(&mut ws, EVENT_TIMEOUT).await.as_deref(),
        Some("periods")
    );
}

#[tokio::test]
async fn cross_writer_commit_is_signaled() {
    // The mutation below never touches this process's HTTP handlers —
    // a raw pool INSERT. The socket still receives the signal: delivery
    // is database-driven, not request-path-driven (multi-instance safe).
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    create_user(&app.pool, "rt-xwrite", &["income_expense.read"]).await;
    let cookie = login(&app, "rt-xwrite").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws, &["income_expense.read"]).await;
    commit_category(&app.pool, "external-writer").await;
    assert_eq!(
        next_domain(&mut ws, EVENT_TIMEOUT).await.as_deref(),
        Some("income_expense")
    );
}

#[tokio::test]
async fn unauthorized_scope_receives_nothing() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    // Holds ONLY periods.read; asks for governance.read too — the
    // granted set must silently drop it, and a governance commit must
    // never reach this socket.
    create_user(&app.pool, "rt-scoped", &["periods.read"]).await;
    let cookie = login(&app, "rt-scoped").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    let ack = subscribe(&mut ws, &["periods.read", "governance.read"]).await;
    assert_eq!(ack["granted"], json!(["periods.read"]));

    sqlx::query(
        "INSERT INTO governance_bodies (name, body_type, idempotency_key, idempotency_fingerprint, created_by, cooperative_id, body_number) \
         SELECT 'rt-body', 'board', $1, $1, u.id, c.id, allocate_document_number(c.id, 'governance_bodies.body_number') \
         FROM users u CROSS JOIN (SELECT id FROM cooperatives WHERE is_bootstrap) c LIMIT 1",
    )
    .bind(Uuid::new_v4().to_string())
    .execute(&app.pool)
    .await
    .unwrap();
    assert_eq!(
        next_domain(&mut ws, SILENCE_TIMEOUT).await,
        None,
        "unauthorized domain must never be delivered"
    );
    commit_period(&app.pool, "allowed-scope").await;
    assert_eq!(
        next_domain(&mut ws, EVENT_TIMEOUT).await.as_deref(),
        Some("periods")
    );
}

#[tokio::test]
async fn reports_scope_receives_all_domains() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    create_user(&app.pool, "rt-reports", &["reports.read"]).await;
    let cookie = login(&app, "rt-reports").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws, &["reports.read"]).await;
    commit_period(&app.pool, "via-reports-scope").await;
    assert_eq!(
        next_domain(&mut ws, EVENT_TIMEOUT).await.as_deref(),
        Some("periods")
    );
    commit_category(&app.pool, "via-reports-scope").await;
    assert_eq!(
        next_domain(&mut ws, EVENT_TIMEOUT).await.as_deref(),
        Some("income_expense")
    );
}

#[tokio::test]
async fn session_revocation_terminates_delivery() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    create_user(&app.pool, "rt-revoke", &["periods.read"]).await;
    let cookie = login(&app, "rt-revoke").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws, &["periods.read"]).await;

    // Revoke every session of that user — as logout-all does.
    sqlx::query(
        "UPDATE user_sessions SET revoked_at = now() \
         WHERE user_id = (SELECT id FROM users WHERE username = 'rt-revoke')",
    )
    .execute(&app.pool)
    .await
    .unwrap();

    // Next commit triggers revalidation → policy close, no event.
    commit_period(&app.pool, "after-revoke").await;
    let closed = tokio::time::timeout(EVENT_TIMEOUT, async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(_))) | None => return true,
                Some(Ok(Message::Text(t))) => {
                    assert_ne!(
                        serde_json::from_str::<Value>(&t).unwrap()["type"],
                        "data-changed",
                        "no signal may be delivered after revocation"
                    );
                }
                Some(_) => continue,
            }
        }
    })
    .await
    .expect("socket must close after session revocation");
    assert!(closed);
}

#[tokio::test]
async fn permission_revocation_stops_domain_signals() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let user = create_user(&app.pool, "rt-perm", &["periods.read"]).await;
    let cookie = login(&app, "rt-perm").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws, &["periods.read"]).await;

    // Revoke the read permission while the socket is open.
    sqlx::query(
        "DELETE FROM role_permissions WHERE role_id IN \
         (SELECT role_id FROM user_role_assignments WHERE user_id = $1)",
    )
    .bind(user)
    .execute(&app.pool)
    .await
    .unwrap();

    // The revoke COMMIT emits `authz` (0064); the permission recheck
    // then stops domain delivery.
    assert_eq!(
        next_domain(&mut ws, EVENT_TIMEOUT).await.as_deref(),
        Some("authz"),
        "permission revoke must emit an in-scope authz signal"
    );
    commit_period(&app.pool, "after-perm-revoke").await;
    assert_eq!(
        next_domain(&mut ws, SILENCE_TIMEOUT).await,
        None,
        "revoked permission must stop delivery mid-connection"
    );
}

#[tokio::test]
async fn reconnect_resubscribes_and_canonical_api_is_truth() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    create_user(&app.pool, "rt-reconn", &["periods.read", "reports.read"]).await;
    let cookie = login(&app, "rt-reconn").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws, &["reports.read"]).await;

    // Gap: socket down → commit occurs → missed by design (no durable
    // replay claimed). Reconnect → fresh subscribe → canonical re-read.
    drop(ws);
    commit_period(&app.pool, "during-gap").await;

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM periods WHERE name = 'during-gap'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    let mut ws2 = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    let ack = subscribe(&mut ws2, &["reports.read"]).await;
    assert_eq!(ack["granted"], json!(["reports.read"]));
    // Canonical API answers the resync — the socket is never the truth.
    let overview_req = Request::get("/api/reports/overview")
        .header("cookie", &cookie)
        .header("origin", ORIGIN)
        .body(Body::empty())
        .unwrap();
    let overview_resp = app.app.clone().oneshot(overview_req).await.unwrap();
    assert_eq!(overview_resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(overview_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let overview: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(overview["assessmentsTotal"].is_string());
    // Post-reconnect commits flow again.
    commit_period(&app.pool, "after-reconnect").await;
    assert_eq!(
        next_domain(&mut ws2, EVENT_TIMEOUT).await.as_deref(),
        Some("periods")
    );
}

#[tokio::test]
async fn event_envelope_is_minimal() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    create_user(&app.pool, "rt-shape", &["periods.read"]).await;
    let cookie = login(&app, "rt-shape").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws, &["periods.read"]).await;
    commit_period(&app.pool, "envelope-shape").await;
    let domain = next_domain(&mut ws, EVENT_TIMEOUT).await;
    assert_eq!(domain.as_deref(), Some("periods"));
    // Fetch raw frame shape by triggering once more and inspecting keys.
    commit_period(&app.pool, "envelope-shape-2").await;
    let message = tokio::time::timeout(EVENT_TIMEOUT, ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let frame: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
    let mut keys: Vec<&str> = frame
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        ["domain", "occurredAt", "type"],
        "envelope must stay minimal"
    );
}

// ---- M1-P1-c3a — cooperative-scoped realtime authorization ----
//
// Signals are bootstrap-scoped invalidations: authorization resolves in
// the VERIFIED bootstrap cooperative (x-cooperative-id is honored but
// only the bootstrap UUID is accepted, exactly like the HTTP extractor).
// A foreign cooperative's roles must never grant or deliver signals;
// membership in the bootstrap cooperative is required to open a socket
// and is re-verified on every signal and heartbeat.

#[tokio::test]
async fn foreign_cooperative_roles_never_authorize_bootstrap_signals() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let coop_a = repo::bootstrap_initial_cooperative(&app.pool, "A Kooperatifi")
        .await
        .expect("bootstrap")
        .0;
    let coop_b = repo::create_cooperative(&app.pool, "B Kooperatifi", None)
        .await
        .expect("B")
        .id;
    // Raw user: create_user grants bootstrap membership automatically,
    // but this test needs a user with NO bootstrap membership first.
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).unwrap(),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).unwrap();
    let user = users::create_user(&app.pool, "rt-scope", "RT", &hash, TEST_PASSWORD)
        .await
        .unwrap()
        .id;

    // B member with B-scoped periods.read but NO bootstrap membership —
    // the handshake itself is rejected (membership verified at upgrade).
    repo::add_member(&app.pool, "B Kooperatifi", user, None)
        .await
        .expect("B member");
    grant_role(
        &app.pool,
        user,
        "b-izleyici",
        Some(coop_b),
        &["periods.read"],
    )
    .await;
    let cookie = login(&app, "rt-scope").await;
    assert!(
        ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.is_err(),
        "no bootstrap membership must reject the handshake"
    );

    // Member of A AND B, rights only in B: the socket opens but B's
    // periods.read must neither grant nor deliver.
    repo::add_member(&app.pool, "Ana Kooperatif", user, None)
        .await
        .expect("A member");
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    let ack = subscribe(&mut ws, &["periods.read"]).await;
    assert_eq!(ack["granted"], json!([]));
    commit_period(&app.pool, "foreign-scope").await;
    assert_eq!(next_domain(&mut ws, SILENCE_TIMEOUT).await, None);

    // The same permission key scoped to the bootstrap cooperative
    // authorizes normally — proving scoping, not the key, drives access.
    grant_role(
        &app.pool,
        user,
        "a-izleyici",
        Some(coop_a),
        &["periods.read"],
    )
    .await;
    let ack = subscribe(&mut ws, &["periods.read"]).await;
    assert_eq!(ack["granted"], json!(["periods.read"]));
    commit_period(&app.pool, "bootstrap-scope").await;
    // The grant's own `authz` signal may arrive first — skip it.
    while next_domain(&mut ws, EVENT_TIMEOUT).await.as_deref() != Some("periods") {}
}

#[tokio::test]
async fn header_rules_apply_to_realtime_upgrade() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let coop_a = repo::bootstrap_initial_cooperative(&app.pool, "A Kooperatifi")
        .await
        .expect("bootstrap")
        .0;
    let coop_b = repo::create_cooperative(&app.pool, "B Kooperatifi", None)
        .await
        .expect("B")
        .id;
    let user = create_user(&app.pool, "rt-hdr", &["periods.read"]).await;
    repo::add_member(&app.pool, "Ana Kooperatif", user, None)
        .await
        .expect("A member");
    let cookie = login(&app, "rt-hdr").await;

    // Malformed UUID → extractor rejects before upgrade.
    assert!(
        ws_connect_scoped(&app, Some(&cookie), Some(ORIGIN), Some("bogus"))
            .await
            .is_err(),
        "malformed cooperative header must reject"
    );
    // Explicit non-bootstrap cooperative → reject, never silently
    // fall back to bootstrap.
    assert!(
        ws_connect_scoped(&app, Some(&cookie), Some(ORIGIN), Some(&coop_b.to_string()))
            .await
            .is_err(),
        "non-bootstrap cooperative header must reject"
    );
    // Explicit bootstrap header pins the same verified scope — accepted.
    ws_connect_scoped(&app, Some(&cookie), Some(ORIGIN), Some(&coop_a.to_string()))
        .await
        .expect("bootstrap cooperative header must be accepted");
    // Unknown-but-well-formed UUID → unknown cooperative rejection.
    assert!(
        ws_connect_scoped(
            &app,
            Some(&cookie),
            Some(ORIGIN),
            Some(&Uuid::new_v4().to_string())
        )
        .await
        .is_err(),
        "unknown cooperative header must reject"
    );
}

#[tokio::test]
async fn membership_suspension_stops_delivery_and_reconnect() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let coop_a = repo::bootstrap_initial_cooperative(&app.pool, "A Kooperatifi")
        .await
        .expect("bootstrap")
        .0;
    let user = create_user(&app.pool, "rt-susp", &[]).await;
    repo::add_member(&app.pool, "Ana Kooperatif", user, None)
        .await
        .expect("A member");
    grant_role(
        &app.pool,
        user,
        "a-izleyici",
        Some(coop_a),
        &["periods.read"],
    )
    .await;
    let cookie = login(&app, "rt-susp").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    let ack = subscribe(&mut ws, &["periods.read"]).await;
    assert_eq!(ack["granted"], json!(["periods.read"]));
    commit_period(&app.pool, "before-suspend").await;
    assert_eq!(
        next_domain(&mut ws, EVENT_TIMEOUT).await.as_deref(),
        Some("periods")
    );

    // Suspend the bootstrap membership mid-connection: the next signal
    // fails scoped authorization and the socket is closed.
    sqlx::query(
        "UPDATE cooperative_memberships SET status = 'suspended' \
         WHERE user_id = $1 AND cooperative_id = $2",
    )
    .bind(user)
    .bind(coop_a)
    .execute(&app.pool)
    .await
    .unwrap();
    commit_period(&app.pool, "after-suspend").await;
    let closed = tokio::time::timeout(EVENT_TIMEOUT, async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(_))) | None => return true,
                Some(Ok(Message::Text(t))) => {
                    assert_ne!(
                        serde_json::from_str::<Value>(&t).unwrap()["type"],
                        "data-changed",
                        "no signal may be delivered after suspension"
                    );
                }
                Some(_) => continue,
            }
        }
    })
    .await
    .expect("socket must close after membership suspension");
    assert!(closed);

    // Reconnect while suspended: the handshake itself refuses.
    assert!(
        ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.is_err(),
        "suspended membership must reject reconnection"
    );
}

#[tokio::test]
async fn scoped_role_revocation_stops_delivery_mid_connection() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let coop_a = repo::bootstrap_initial_cooperative(&app.pool, "A Kooperatifi")
        .await
        .expect("bootstrap")
        .0;
    let user = create_user(&app.pool, "rt-scoperev", &[]).await;
    repo::add_member(&app.pool, "Ana Kooperatif", user, None)
        .await
        .expect("A member");
    grant_role(
        &app.pool,
        user,
        "a-izleyici",
        Some(coop_a),
        &["periods.read"],
    )
    .await;
    let cookie = login(&app, "rt-scoperev").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    let ack = subscribe(&mut ws, &["periods.read"]).await;
    assert_eq!(ack["granted"], json!(["periods.read"]));

    // Revoke the scoped permission mid-connection — the revoke COMMIT
    // emits `authz` (0064), then delivery must stop.
    sqlx::query(
        "DELETE FROM role_permissions WHERE role_id IN \
         (SELECT role_id FROM user_role_assignments WHERE user_id = $1 \
          AND cooperative_id = $2)",
    )
    .bind(user)
    .bind(coop_a)
    .execute(&app.pool)
    .await
    .unwrap();
    assert_eq!(
        next_domain(&mut ws, EVENT_TIMEOUT).await.as_deref(),
        Some("authz"),
        "permission revoke must emit an in-scope authz signal"
    );
    commit_period(&app.pool, "after-scoped-revoke").await;
    assert_eq!(
        next_domain(&mut ws, SILENCE_TIMEOUT).await,
        None,
        "revoked scoped permission must stop delivery mid-connection"
    );
}

#[tokio::test]
async fn null_attributed_legacy_role_authorizes_in_bootstrap() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    repo::bootstrap_initial_cooperative(&app.pool, "A Kooperatifi")
        .await
        .expect("bootstrap");
    // create_user writes NULL-cooperative role + assignment (legacy
    // lane); with active bootstrap membership the resolver attributes
    // them — the migration keeps legacy grants working.
    let user = create_user(&app.pool, "rt-null", &["periods.read"]).await;
    repo::add_member(&app.pool, "Ana Kooperatif", user, None)
        .await
        .expect("A member");
    let cookie = login(&app, "rt-null").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    let ack = subscribe(&mut ws, &["periods.read"]).await;
    assert_eq!(ack["granted"], json!(["periods.read"]));
    commit_period(&app.pool, "null-attributed").await;
    assert_eq!(
        next_domain(&mut ws, EVENT_TIMEOUT).await.as_deref(),
        Some("periods")
    );
}

#[tokio::test]
async fn reconnect_after_session_revocation_is_rejected() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    create_user(&app.pool, "rt-reconnrev", &["periods.read"]).await;
    let cookie = login(&app, "rt-reconnrev").await;
    let ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    sqlx::query(
        "UPDATE user_sessions SET revoked_at = now() \
         WHERE user_id = (SELECT id FROM users WHERE username = 'rt-reconnrev')",
    )
    .execute(&app.pool)
    .await
    .unwrap();
    drop(ws);
    assert!(
        ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.is_err(),
        "revoked session must not open a new socket"
    );
}

// ======================================================================
// M1-P5 B1 — cooperative-scoped, MAC-signed realtime delivery.
//
// The v1 payload is `v1|<cooperative_id|->|<domain>|<hmac-sha256>`:
// the cooperative is derived from the mutated ROW inside the trigger
// (never client input), and the HMAC is keyed by an owner-only
// `app_secrets` entry, so a principal that can run `pg_notify`
// (discovery RT-2: PostgreSQL grants no privilege over NOTIFY) can no
// longer forge a deliverable signal. The listener drops every
// unverifiable payload; the socket additionally filters on
// `event.cooperative == socket.scope` before doing any work.
// ======================================================================

const APP_ROLE: &str = "kooperatif_app";
/// Restricted-principal password: `KOOPERATIF_APP_TEST_PASSWORD`
/// overrides for CI; otherwise the documented local dev password.
fn app_password() -> String {
    app_db::test_app_role_password()
}

/// Restricted-principal URL for the test database (same rewrite the
/// rbac_cooperative harness uses).
async fn app_url_for(app: &TestApp) -> String {
    let db_name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let url = std::env::var("KOOPERATIF_TEST_DATABASE_URL").unwrap();
    let (base, _) = url.rsplit_once('/').unwrap();
    let (scheme, rest) = base.split_once("://").unwrap();
    let at = rest.find('@').unwrap();
    format!(
        "{scheme}://{APP_ROLE}:{}{}/{db_name}",
        app_password(),
        &rest[at..]
    )
}

async fn app_pool_for(app: &TestApp) -> PgPool {
    app_db::connect(&app_url_for(app).await)
        .await
        .expect("kooperatif_app pool")
}

/// INSERT into an rt_-triggered table owned by an arbitrary cooperative.
async fn commit_category_in(pool: &PgPool, name: &str, coop: Uuid) {
    sqlx::query(
        "INSERT INTO financial_categories (category_type, name, cooperative_id) \
         VALUES ('income', $1, $2)",
    )
    .bind(name)
    .bind(coop)
    .execute(pool)
    .await
    .unwrap();
}

/// A commit in cooperative B must produce NO delivery on a socket bound
/// to bootstrap cooperative A — and the socket must stay usable.
#[tokio::test]
async fn foreign_cooperative_commit_is_not_delivered() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let coop_b = repo::create_cooperative(&app.pool, "B Kooperatifi", None)
        .await
        .expect("cooperative B")
        .id;

    create_user(&app.pool, "rt-b1iso", &["income_expense.read"]).await;
    let cookie = login(&app, "rt-b1iso").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws, &["income_expense.read"]).await;

    // A second cooperative exists in the SAME database; its commit must
    // never invalidate a socket verified into the bootstrap scope.
    commit_category_in(&app.pool, "b-only-category", coop_b).await;
    assert_eq!(
        next_domain(&mut ws, SILENCE_TIMEOUT).await,
        None,
        "a cooperative-B commit must not reach an A-scoped socket"
    );

    // Control: the in-scope commit still delivers — silence above was
    // isolation, not a dead socket.
    commit_category(&app.pool, "a-category").await;
    assert_eq!(
        next_domain(&mut ws, EVENT_TIMEOUT).await.as_deref(),
        Some("income_expense")
    );
}

/// Real NOTIFY through the REAL listener: the restricted runtime role
/// can still call pg_notify (RT-2 — no privilege boundary exists on the
/// channel), but every unverifiable payload is dropped before the hub.
#[tokio::test]
async fn forged_and_malformed_notify_payloads_are_dropped() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let coop_a: Uuid = sqlx::query_scalar("SELECT id FROM cooperatives WHERE is_bootstrap")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let coop_b = repo::create_cooperative(&app.pool, "B Kooperatifi", None)
        .await
        .expect("cooperative B")
        .id;
    let app_pool = app_pool_for(&app).await;

    create_user(&app.pool, "rt-b1forge", &["payments.read"]).await;
    let cookie = login(&app, "rt-b1forge").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws, &["payments.read"]).await;

    let now_ms = OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
    let nonce = Uuid::new_v4();
    let forged: Vec<String> = vec![
        // Valid v2 shape, wrong MAC — impersonating cooperative A.
        format!("v2|{coop_a}|payments|{nonce}|{now_ms}|{}", "0".repeat(64)),
        // Valid v2 shape, wrong MAC — impersonating cooperative B.
        format!("v2|{coop_b}|payments|{nonce}|{now_ms}|{}", "f".repeat(64)),
        // Bad nonce / bad timestamp field types.
        format!(
            "v2|{coop_a}|payments|not-a-uuid|{now_ms}|{}",
            "a".repeat(64)
        ),
        format!(
            "v2|{coop_a}|payments|{nonce}|not-a-number|{}",
            "a".repeat(64)
        ),
        // Not a uuid cooperative.
        "v2|not-a-uuid|payments|0000".to_string(),
        // Legacy v1 payload (pre-B1 contract — no downgrade lane).
        format!("v1|{coop_a}|payments|{}", "0".repeat(64)),
        // Bare-tag payload (pre-B1 contract).
        "payments".to_string(),
        // Missing MAC field entirely.
        format!("v2|{coop_a}|payments|{nonce}|{now_ms}"),
        // Garbage.
        "definitely-not-an-envelope".to_string(),
    ];
    for payload in &forged {
        sqlx::query("SELECT pg_notify('kooperatif_domain_changed', $1)")
            .bind(payload)
            .execute(&app_pool)
            .await
            .expect("kooperatif_app can still pg_notify — no privilege lane exists");
    }
    assert_eq!(
        next_domain(&mut ws, SILENCE_TIMEOUT).await,
        None,
        "unsigned or malformed payloads must never reach a socket"
    );

    // Control: the authentic path still works — listener is alive and
    // verification did not wedge it.
    commit_category(&app.pool, "control-category").await;
    let user2 = create_user(&app.pool, "rt-b1ctrl", &["income_expense.read"]).await;
    let _ = user2; // fixture-side symmetry only
    let cookie2 = login(&app, "rt-b1ctrl").await;
    let mut ws2 = ws_connect(&app, Some(&cookie2), Some(ORIGIN))
        .await
        .unwrap();
    subscribe(&mut ws2, &["income_expense.read"]).await;
    commit_category(&app.pool, "control-category-2").await;
    assert_eq!(
        next_domain(&mut ws2, EVENT_TIMEOUT).await.as_deref(),
        Some("income_expense")
    );
}

/// Wire-contract proof: capture the RAW notification on a dedicated
/// listener and assert the v2 envelope — no business data — then show
/// the restricted role can VERIFY a fresh authentic payload but cannot
/// authenticate a tampered one, and that verification is single-use
/// (the nonce is consumed by the first verifier).
#[tokio::test]
async fn notify_payload_is_signed_minimal_and_verifiable() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let coop_a: Uuid = sqlx::query_scalar("SELECT id FROM cooperatives WHERE is_bootstrap")
        .fetch_one(&app.pool)
        .await
        .unwrap();

    let mut raw = sqlx::postgres::PgListener::connect_with(&app.pool)
        .await
        .expect("raw listener");
    raw.listen("kooperatif_domain_changed").await.unwrap();

    commit_category(&app.pool, "payload-shape").await;
    let note = tokio::time::timeout(EVENT_TIMEOUT, raw.recv())
        .await
        .expect("notify timeout")
        .expect("notification");
    let payload = note.payload();

    // v2|<uuid>|<domain>|<nonce>|<ts_ms>|<64-hex> — and nothing else.
    let parts: Vec<&str> = payload.split('|').collect();
    assert_eq!(parts.len(), 6, "envelope: {payload}");
    assert_eq!(parts[0], "v2");
    assert_eq!(parts[1], coop_a.to_string(), "coop must be row-derived");
    assert_eq!(parts[2], "income_expense");
    parts[3].parse::<Uuid>().expect("nonce must be a uuid");
    let ts_ms: i64 = parts[4].parse().expect("ts must be epoch ms");
    let now_ms = OffsetDateTime::now_utc().unix_timestamp_nanos() as i64 / 1_000_000;
    assert!(
        (now_ms - ts_ms).abs() < 30_000,
        "emit timestamp implausible"
    );
    assert_eq!(parts[5].len(), 64);
    assert!(
        parts[5].chars().all(|c| c.is_ascii_hexdigit()),
        "MAC must be lowercase hex"
    );

    // The restricted principal verifies a FRESH authentic payload —
    // crafted here rather than re-verifying the captured one, whose
    // nonce the live server listener may already have consumed.
    let app_pool = app_pool_for(&app).await;
    let crafted = craft_payload(&app.pool, coop_a, "payments", 0).await;
    // Same backend connection for both calls — dedup is per verifying
    // backend (pg_backend_pid), so a pool checkout keeps one pid.
    let mut conn = app_pool.acquire().await.unwrap();
    let verified: Option<(Option<Uuid>, String)> =
        sqlx::query_as("SELECT cooperative_id, domain FROM public.app_notify_verify($1)")
            .bind(&crafted)
            .fetch_optional(&mut *conn)
            .await
            .unwrap();
    assert_eq!(verified, Some((Some(coop_a), "payments".to_string())));

    // The same payload verified again on the SAME backend — the nonce
    // is spent for this listener (0063: per-backend, not global).
    let second: Option<(Option<Uuid>, String)> =
        sqlx::query_as("SELECT cooperative_id, domain FROM public.app_notify_verify($1)")
            .bind(&crafted)
            .fetch_optional(&mut *conn)
            .await
            .unwrap();
    assert_eq!(second, None, "same-backend verification must be single-use");

    // …and any tampering fails — flipped domain, foreign coop, bad MAC.
    let cparts: Vec<&str> = crafted.split('|').collect();
    for tampered in [
        format!(
            "v2|{coop_a}|income_expense|{}|{}|{}",
            cparts[3], cparts[4], cparts[5]
        ),
        format!(
            "v2|{}|payments|{}|{}|{}",
            Uuid::new_v4(),
            cparts[3],
            cparts[4],
            cparts[5]
        ),
        format!(
            "v2|{coop_a}|payments|{}|{}|{}",
            cparts[3],
            cparts[4],
            "9".repeat(64)
        ),
        format!("v2|-|payments|{}|{}|{}", cparts[3], cparts[4], cparts[5]),
    ] {
        let out: Option<(Option<Uuid>, String)> =
            sqlx::query_as("SELECT cooperative_id, domain FROM public.app_notify_verify($1)")
                .bind(&tampered)
                .fetch_optional(&app_pool)
                .await
                .unwrap();
        assert_eq!(out, None, "tampered payload verified: {tampered}");
    }
}

/// A membership suspension mid-connection still terminates delivery —
/// the scope filter did not weaken per-signal revalidation.
#[tokio::test]
async fn membership_suspension_still_closes_scoped_socket() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let user = create_user(&app.pool, "rt-b1susp", &["periods.read"]).await;
    let cookie = login(&app, "rt-b1susp").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws, &["periods.read"]).await;

    sqlx::query(
        "UPDATE cooperative_memberships SET status = 'suspended' \
         WHERE user_id = $1 AND cooperative_id = \
         (SELECT id FROM cooperatives WHERE is_bootstrap)",
    )
    .bind(user)
    .execute(&app.pool)
    .await
    .unwrap();

    commit_period(&app.pool, "suspend-check").await;
    let mut saw_close = false;
    let deadline = tokio::time::Instant::now() + EVENT_TIMEOUT;
    while let Some(message) = tokio::time::timeout_at(deadline, ws.next())
        .await
        .ok()
        .flatten()
    {
        if matches!(message, Ok(Message::Close(_))) {
            saw_close = true;
            break;
        }
        if let Ok(Message::Text(text)) = &message {
            let frame: Value = serde_json::from_str(text).unwrap();
            if frame["type"] == "data-changed" {
                panic!("suspended member received a domain signal");
            }
        }
    }
    assert!(saw_close, "socket must close once membership is suspended");
}

// ── M1-P5 B2: replay / amplification hardening ──────────────────────

/// Build a wire-valid v2 payload as the DB owner (the fixture pool
/// holds owner rights, so it can read the signing key). `ts_offset_ms`
/// shifts the emit timestamp — used to craft stale/future payloads
/// that are still correctly signed.
async fn craft_payload(pool: &PgPool, coop: Uuid, domain: &str, ts_offset_ms: i64) -> String {
    sqlx::query_scalar::<_, String>(
        "WITH k AS (SELECT value FROM app_secrets \
                    WHERE name = 'realtime_notify_key'), \
              n AS (SELECT gen_random_uuid() AS nonce, \
                    (floor(extract(epoch FROM clock_timestamp()) * 1000)::bigint + $3) AS ts) \
         SELECT 'v2|' || $1::text || '|' || $2 || '|' || n.nonce::text || '|' || n.ts || '|' || \
                encode(hmac(convert_to($2 || '|' || $1::text || '|' || n.nonce::text || '|' || \
                            n.ts, 'UTF8'), k.value, 'sha256'), 'hex') \
         FROM k, n",
    )
    .bind(coop)
    .bind(domain)
    .bind(ts_offset_ms)
    .fetch_one(pool)
    .await
    .expect("craft v2 payload")
}

/// RT-2 replay: a captured, validly-signed payload replayed through
/// the channel the restricted role can write collapses to zero
/// additional deliveries — across a replay flood AND across two
/// simultaneous sockets — while fresh commits keep delivering.
#[tokio::test]
async fn replayed_payloads_collapse_and_flood_stays_silent() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let app_pool = app_pool_for(&app).await;

    create_user(&app.pool, "rt-b2replay", &["income_expense.read"]).await;
    let cookie = login(&app, "rt-b2replay").await;
    let mut ws1 = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws1, &["income_expense.read"]).await;
    let mut ws2 = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws2, &["income_expense.read"]).await;

    // Capture the authentic payload on a separate raw listener — this
    // is exactly what a kooperatif_app-level attacker can read.
    let mut raw = sqlx::postgres::PgListener::connect_with(&app.pool)
        .await
        .expect("raw listener");
    raw.listen("kooperatif_domain_changed").await.unwrap();
    commit_category(&app.pool, "capture-me").await;
    let payload = tokio::time::timeout(EVENT_TIMEOUT, raw.recv())
        .await
        .expect("notify timeout")
        .expect("notification")
        .payload()
        .to_string();

    // Both sockets receive the authentic event — the nonce is spent.
    assert_eq!(
        next_domain(&mut ws1, EVENT_TIMEOUT).await.as_deref(),
        Some("income_expense")
    );
    assert_eq!(
        next_domain(&mut ws2, EVENT_TIMEOUT).await.as_deref(),
        Some("income_expense")
    );

    // Replay flood: the payload is authentic, fresh and correctly
    // signed — and still worthless after first delivery.
    for _ in 0..30 {
        sqlx::query("SELECT pg_notify('kooperatif_domain_changed', $1)")
            .bind(&payload)
            .execute(&app_pool)
            .await
            .expect("replay is publishable — dedup must stop it");
    }
    assert_eq!(
        next_domain(&mut ws1, SILENCE_TIMEOUT).await,
        None,
        "replayed payload must not re-deliver (socket 1)"
    );
    assert_eq!(
        next_domain(&mut ws2, SILENCE_TIMEOUT).await,
        None,
        "replayed payload must not re-deliver (socket 2)"
    );

    // Control: a fresh legitimate commit still reaches both sockets.
    commit_category(&app.pool, "post-flood").await;
    assert_eq!(
        next_domain(&mut ws1, EVENT_TIMEOUT).await.as_deref(),
        Some("income_expense")
    );
    assert_eq!(
        next_domain(&mut ws2, EVENT_TIMEOUT).await.as_deref(),
        Some("income_expense")
    );
}

/// Freshness is enforced by the database clock, not by the caller's
/// claims: a correctly-signed payload 3 minutes old passes the
/// listener's cheap local bound and is still rejected by the
/// authoritative 120 s window; an hour-old one dies even earlier.
#[tokio::test]
async fn stale_signed_payload_is_rejected_through_the_wire() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let coop_a: Uuid = sqlx::query_scalar("SELECT id FROM cooperatives WHERE is_bootstrap")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let app_pool = app_pool_for(&app).await;

    create_user(&app.pool, "rt-b2stale", &["payments.read"]).await;
    let cookie = login(&app, "rt-b2stale").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws, &["payments.read"]).await;

    // Correctly signed, but emitted 3 minutes ago — beyond the
    // authoritative 120 s window.
    let stale = craft_payload(&app.pool, coop_a, "payments", -180_000).await;
    let direct: Option<(Option<Uuid>, String)> =
        sqlx::query_as("SELECT cooperative_id, domain FROM public.app_notify_verify($1)")
            .bind(&stale)
            .fetch_optional(&app_pool)
            .await
            .unwrap();
    assert_eq!(direct, None, "stale signed payload verified");

    // Through the real wire: also dropped (and its nonce is never
    // consumed, so it cannot even poison the dedup set).
    sqlx::query("SELECT pg_notify('kooperatif_domain_changed', $1)")
        .bind(&stale)
        .execute(&app_pool)
        .await
        .unwrap();
    // Far-future skew — correctly signed, implausible emit time.
    let future = craft_payload(&app.pool, coop_a, "payments", 3_600_000).await;
    sqlx::query("SELECT pg_notify('kooperatif_domain_changed', $1)")
        .bind(&future)
        .execute(&app_pool)
        .await
        .unwrap();

    assert_eq!(
        next_domain(&mut ws, SILENCE_TIMEOUT).await,
        None,
        "expired or future-skewed payloads must never deliver"
    );
}

/// Distinct legitimate commits are never deduplicated: each emits a
/// fresh nonce, so rapid successive changes all deliver.
#[tokio::test]
async fn rapid_legitimate_commits_all_deliver() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    create_user(&app.pool, "rt-b2rapid", &["income_expense.read"]).await;
    let cookie = login(&app, "rt-b2rapid").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws, &["income_expense.read"]).await;

    commit_category(&app.pool, "rapid-1").await;
    commit_category(&app.pool, "rapid-2").await;

    assert_eq!(
        next_domain(&mut ws, EVENT_TIMEOUT).await.as_deref(),
        Some("income_expense"),
        "first rapid commit lost"
    );
    assert_eq!(
        next_domain(&mut ws, EVENT_TIMEOUT).await.as_deref(),
        Some("income_expense"),
        "second rapid commit must not be deduplicated"
    );
}

/// The dedup set is the bounded-memory story: it lives in the
/// database, is invisible to the runtime role, and prunes entries
/// beyond the replay horizon on every verification.
#[tokio::test]
async fn dedup_set_is_bounded_and_owner_only() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let coop_a: Uuid = sqlx::query_scalar("SELECT id FROM cooperatives WHERE is_bootstrap")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let app_pool = app_pool_for(&app).await;

    // The runtime role cannot even read the seen-set.
    let denied = sqlx::query("SELECT count(*) FROM public.realtime_notify_dedup")
        .fetch_one(&app_pool)
        .await
        .expect_err("kooperatif_app must not read the dedup set");
    assert!(
        denied.to_string().contains("42501") || denied.to_string().contains("permission denied"),
        "expected permission denied, got: {denied}"
    );

    // Seed a stale row as the owner, then run one real verification —
    // the amortized prune must remove it.
    sqlx::query(
        "INSERT INTO public.realtime_notify_dedup (nonce, seen_at) \
         VALUES ($1, clock_timestamp() - interval '1 hour')",
    )
    .bind(Uuid::new_v4())
    .execute(&app.pool)
    .await
    .unwrap();

    let crafted = craft_payload(&app.pool, coop_a, "payments", 0).await;
    let verified: Option<(Option<Uuid>, String)> =
        sqlx::query_as("SELECT cooperative_id, domain FROM public.app_notify_verify($1)")
            .bind(&crafted)
            .fetch_optional(&app_pool)
            .await
            .unwrap();
    assert!(verified.is_some(), "fresh valid payload must verify");

    let stale_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.realtime_notify_dedup \
         WHERE seen_at < clock_timestamp() - interval '30 minutes'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(stale_rows, 0, "dedup horizon was not pruned");
}

// ── M1-P5 B2 multi-instance correctness (0063) ──────────────────────

/// Dedup is namespaced by the verifying backend's pg_backend_pid, not
/// global: one authentic payload verifies exactly once PER listener
/// backend — NOTIFY broadcasts to every instance, so each must get
/// its own single delivery.
#[tokio::test]
async fn verify_dedups_per_listener_backend_not_globally() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let coop_a: Uuid = sqlx::query_scalar("SELECT id FROM cooperatives WHERE is_bootstrap")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let app_pool = app_pool_for(&app).await;
    let crafted = craft_payload(&app.pool, coop_a, "payments", 0).await;

    // Listener backend A: first verify delivers, replay dedups.
    let mut conn_a = app_pool.acquire().await.unwrap();
    let first: Option<(Option<Uuid>, String)> =
        sqlx::query_as("SELECT cooperative_id, domain FROM public.app_notify_verify($1)")
            .bind(&crafted)
            .fetch_optional(&mut *conn_a)
            .await
            .unwrap();
    assert_eq!(first, Some((Some(coop_a), "payments".to_string())));
    let again: Option<(Option<Uuid>, String)> =
        sqlx::query_as("SELECT cooperative_id, domain FROM public.app_notify_verify($1)")
            .bind(&crafted)
            .fetch_optional(&mut *conn_a)
            .await
            .unwrap();
    assert_eq!(again, None, "same-backend replay must dedup");

    // Listener backend B (a different connection = different
    // pg_backend_pid): the SAME payload must still verify — a second
    // app instance is entitled to its own single delivery.
    let mut conn_b = app_pool.acquire().await.unwrap();
    let second: Option<(Option<Uuid>, String)> =
        sqlx::query_as("SELECT cooperative_id, domain FROM public.app_notify_verify($1)")
            .bind(&crafted)
            .fetch_optional(&mut *conn_b)
            .await
            .unwrap();
    assert_eq!(
        second,
        Some((Some(coop_a), "payments".to_string())),
        "a second listener instance must not lose the event to global dedup"
    );
    let second_again: Option<(Option<Uuid>, String)> =
        sqlx::query_as("SELECT cooperative_id, domain FROM public.app_notify_verify($1)")
            .bind(&crafted)
            .fetch_optional(&mut *conn_b)
            .await
            .unwrap();
    assert_eq!(second_again, None, "backend B dedups its own replays");
}

/// Two independent application listener/hub instances (separate
/// PgListener + separate verify backend + separate hub) each receive
/// the same commit exactly once — the B2 fix for global nonce
/// consumption.
#[tokio::test]
async fn multi_instance_listeners_each_deliver_once() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let coop_a: Uuid = sqlx::query_scalar("SELECT id FROM cooperatives WHERE is_bootstrap")
        .fetch_one(&app.pool)
        .await
        .unwrap();

    // Instance B: own listener connection, own verify backend pid,
    // own hub — connected as the restricted runtime role, matching
    // production wiring.
    let hub2 = realtime::RealtimeHub::new();
    let mut rx2 = hub2.subscribe();
    let _listener2 = tokio::spawn(realtime::listen_loop_with_hub(
        app_url_for(&app).await,
        hub2,
    ));
    // Let instance B connect and LISTEN before the commit.
    tokio::time::sleep(Duration::from_millis(800)).await;

    create_user(&app.pool, "rt-b2multi", &["income_expense.read"]).await;
    let cookie = login(&app, "rt-b2multi").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws, &["income_expense.read"]).await;

    commit_category(&app.pool, "both-instances").await;

    // Instance A delivers to its WebSocket client — skip earlier
    // authz signals fired by the fixture's membership/role writes.
    while next_domain(&mut ws, EVENT_TIMEOUT).await.as_deref() != Some("income_expense") {}
    // …and instance B delivers the SAME event on its own hub —
    // before 0063 this recv() would have hung (nonce already consumed).
    let deadline = tokio::time::Instant::now() + EVENT_TIMEOUT;
    loop {
        let signal = tokio::time::timeout_at(deadline, rx2.recv())
            .await
            .expect("instance B never received the event — global dedup bug")
            .expect("hub2 closed");
        match signal {
            realtime::HubSignal::Changed {
                domain: realtime::Domain::IncomeExpense,
                cooperative,
            } => {
                assert_eq!(cooperative, Some(coop_a));
                break;
            }
            _ => continue, // authz from fixture writes — keep waiting
        }
    }

    // Exactly once: neither instance re-delivers without a new commit.
    assert!(
        tokio::time::timeout(SILENCE_TIMEOUT, rx2.recv())
            .await
            .is_err(),
        "instance B delivered the same event twice"
    );
    assert_eq!(
        next_domain(&mut ws, SILENCE_TIMEOUT).await,
        None,
        "instance A delivered the same event twice"
    );
}

// ── M1-P5 B3: membership / authorization invalidation (0064) ────────

/// The suspension COMMIT itself emits `authz`: the socket revalidates
/// against the now-suspended membership and closes promptly — no
/// domain commit or 30 s heartbeat wait required.
#[tokio::test]
async fn membership_suspension_closes_socket_promptly_via_authz() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let user = create_user(&app.pool, "rt-b3susp", &["periods.read"]).await;
    let cookie = login(&app, "rt-b3susp").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws, &["periods.read"]).await;

    sqlx::query(
        "UPDATE cooperative_memberships SET status = 'suspended' \
         WHERE user_id = $1 AND cooperative_id = \
         (SELECT id FROM cooperatives WHERE is_bootstrap)",
    )
    .bind(user)
    .execute(&app.pool)
    .await
    .unwrap();

    // The authz event alone must drive the close — nothing else was
    // committed.
    let mut saw_close = false;
    let deadline = tokio::time::Instant::now() + EVENT_TIMEOUT;
    while let Some(message) = tokio::time::timeout_at(deadline, ws.next())
        .await
        .ok()
        .flatten()
    {
        if matches!(message, Ok(Message::Close(_))) {
            saw_close = true;
            break;
        }
        if let Ok(Message::Text(text)) = &message {
            let frame: Value = serde_json::from_str(text).unwrap();
            // authz may be delivered right before revalidation catches
            // the suspension — never a business domain.
            if frame["type"] == "data-changed" {
                assert_eq!(
                    frame["domain"], "authz",
                    "non-authz signal after suspension"
                );
            }
        }
    }
    assert!(saw_close, "suspend must close the socket promptly");
}

/// Authorization churn in cooperative B (a membership insert there)
/// must never reach a socket bound to A — authz honors the same
/// cooperative scope filter as data domains.
#[tokio::test]
async fn foreign_cooperative_membership_change_is_not_delivered() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let coop_b = repo::create_cooperative(&app.pool, "B Kooperatifi", None)
        .await
        .expect("cooperative B")
        .id;

    create_user(&app.pool, "rt-b3iso", &["income_expense.read"]).await;
    let cookie = login(&app, "rt-b3iso").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws, &["income_expense.read"]).await;

    // A brand-new user joining B — an authz event scoped to B.
    let stranger = users::create_user(
        &app.pool,
        "rt-b3other",
        "OB",
        &identity::hash_password(
            &argon2::Argon2::new(
                argon2::Algorithm::Argon2id,
                argon2::Version::V0x13,
                argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).unwrap(),
            ),
            TEST_PASSWORD,
        )
        .unwrap(),
        TEST_PASSWORD,
    )
    .await
    .unwrap()
    .id;
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         VALUES ($1, $2, 'active')",
    )
    .bind(coop_b)
    .bind(stranger)
    .execute(&app.pool)
    .await
    .unwrap();

    assert_eq!(
        next_domain(&mut ws, SILENCE_TIMEOUT).await,
        None,
        "a coop-B membership change must not reach an A-scoped socket"
    );

    // Control: in-scope commit still delivers.
    commit_category(&app.pool, "b3-control").await;
    assert_eq!(
        next_domain(&mut ws, EVENT_TIMEOUT).await.as_deref(),
        Some("income_expense")
    );
}

/// Role revocation emits `authz` even though the socket lost the
/// domain permission — the signal itself is not permission-gated —
/// and subsequent revoked-domain commits stop delivering.
#[tokio::test]
async fn role_revocation_emits_authz_then_stops_domain_delivery() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let user = create_user(&app.pool, "rt-b3revoke", &["income_expense.read"]).await;
    let cookie = login(&app, "rt-b3revoke").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws, &["income_expense.read"]).await;

    // Control: an in-scope commit delivers while the role stands.
    commit_category(&app.pool, "pre-revoke").await;
    assert_eq!(
        next_domain(&mut ws, EVENT_TIMEOUT).await.as_deref(),
        Some("income_expense")
    );

    // Revoke the assignment — the DELETE fires rt_authz_role_assignments.
    sqlx::query("DELETE FROM user_role_assignments WHERE user_id = $1")
        .bind(user)
        .execute(&app.pool)
        .await
        .unwrap();

    // The authz signal IS delivered despite the permission being gone.
    assert_eq!(
        next_domain(&mut ws, EVENT_TIMEOUT).await.as_deref(),
        Some("authz"),
        "authz must reach the socket so it can revalidate"
    );

    // The socket stays open (membership still active) but
    // income_expense commits no longer deliver — live permission
    // recheck on every signal.
    commit_category(&app.pool, "post-revoke").await;
    assert_eq!(
        next_domain(&mut ws, SILENCE_TIMEOUT).await,
        None,
        "revoked domain must stop delivering"
    );
}

/// Role-permission edits fire `authz` through the role's cooperative —
/// the trigger derives scope via roles.id, not client input.
#[tokio::test]
async fn role_permission_change_emits_scoped_authz() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    create_user(&app.pool, "rt-b3perm", &["income_expense.read"]).await;
    let cookie = login(&app, "rt-b3perm").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws, &["income_expense.read"]).await;

    // Removing the role's permission row emits authz for that coop.
    sqlx::query(
        "DELETE FROM role_permissions WHERE role_id IN \
         (SELECT role_id FROM user_role_assignments WHERE user_id = \
          (SELECT id FROM users WHERE username = 'rt-b3perm'))",
    )
    .execute(&app.pool)
    .await
    .unwrap();

    assert_eq!(
        next_domain(&mut ws, EVENT_TIMEOUT).await.as_deref(),
        Some("authz")
    );
}

/// Global identity disable broadcasts `authz` to EVERY cooperative
/// where the user holds an active membership — two coops → two signed
/// events — and live sockets close promptly.
#[tokio::test]
async fn global_disable_broadcasts_authz_to_all_membership_coops() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let coop_a: Uuid = sqlx::query_scalar("SELECT id FROM cooperatives WHERE is_bootstrap")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let coop_b = repo::create_cooperative(&app.pool, "B Kooperatifi", None)
        .await
        .expect("cooperative B")
        .id;

    // Shared user: active in A (fixture) and B.
    let user = create_user(&app.pool, "rt-b3global", &["income_expense.read"]).await;
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         VALUES ($1, $2, 'active')",
    )
    .bind(coop_b)
    .bind(user)
    .execute(&app.pool)
    .await
    .unwrap();

    // A-scoped socket for that user.
    let cookie_a = login(&app, "rt-b3global").await;
    let mut ws_a = ws_connect(&app, Some(&cookie_a), Some(ORIGIN))
        .await
        .unwrap();
    subscribe(&mut ws_a, &["income_expense.read"]).await;

    // Raw listener captures the wire payloads.
    let mut raw = sqlx::postgres::PgListener::connect_with(&app.pool)
        .await
        .expect("raw listener");
    raw.listen("kooperatif_domain_changed").await.unwrap();

    sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(user)
        .execute(&app.pool)
        .await
        .unwrap();

    // Exactly one signed authz per active-membership cooperative,
    // plus the '-' never-bootstrapped lane.
    let mut coops = std::collections::HashSet::new();
    for _ in 0..3 {
        let note = tokio::time::timeout(EVENT_TIMEOUT, raw.recv())
            .await
            .expect("authz notify timeout")
            .expect("notification");
        let parts: Vec<&str> = note.payload().split('|').collect();
        assert_eq!(parts.len(), 6, "v2 envelope: {}", note.payload());
        assert_eq!(parts[0], "v2");
        assert_eq!(parts[2], "authz");
        coops.insert(parts[1].to_string());
    }
    assert_eq!(
        coops,
        [coop_a.to_string(), coop_b.to_string(), "-".to_string()]
            .into_iter()
            .collect(),
        "one authz per affected cooperative plus the '-' lane"
    );

    // The live socket revalidates and closes promptly — session/user
    // is disabled.
    let mut saw_close = false;
    let deadline = tokio::time::Instant::now() + EVENT_TIMEOUT;
    while let Some(message) = tokio::time::timeout_at(deadline, ws_a.next())
        .await
        .ok()
        .flatten()
    {
        if matches!(message, Ok(Message::Close(_))) {
            saw_close = true;
            break;
        }
        if let Ok(Message::Text(text)) = &message {
            let frame: Value = serde_json::from_str(text).unwrap();
            if frame["type"] == "data-changed" {
                assert_eq!(frame["domain"], "authz");
            }
        }
    }
    assert!(saw_close, "global disable must close live coop sockets");
}

/// Session revocation emits `authz` to the session's active coop and
/// the revoked session's socket closes on that signal — not at the
/// next heartbeat.
#[tokio::test]
async fn session_revocation_emits_authz_and_closes_socket() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let user = create_user(&app.pool, "rt-b3sess", &["periods.read"]).await;
    let cookie = login(&app, "rt-b3sess").await;
    let mut ws = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws, &["periods.read"]).await;

    sqlx::query(
        "UPDATE user_sessions SET revoked_at = now(), \
         revocation_reason = 'test' WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user)
    .execute(&app.pool)
    .await
    .unwrap();

    let mut saw_close = false;
    let deadline = tokio::time::Instant::now() + EVENT_TIMEOUT;
    while let Some(message) = tokio::time::timeout_at(deadline, ws.next())
        .await
        .ok()
        .flatten()
    {
        if matches!(message, Ok(Message::Close(_))) {
            saw_close = true;
            break;
        }
        if let Ok(Message::Text(text)) = &message {
            let frame: Value = serde_json::from_str(text).unwrap();
            if frame["type"] == "data-changed" {
                assert_eq!(frame["domain"], "authz");
            }
        }
    }
    assert!(saw_close, "revocation must close the socket via authz");
}

/// B5: cooperative switching rewrites `active_cooperative_id`; the
/// socket bound to the PREVIOUS cooperative must close promptly on the
/// authz signal, and a reconnect under the new scope works while old
/// cooperative events stay undelivered. Measures switch→close latency.
#[tokio::test]
async fn cooperative_switch_closes_stale_socket_and_new_scope_connects() {
    let Some(app) = setup().await else {
        panic!("KOOPERATIF_TEST_DATABASE_URL required")
    };
    let coop_a: Uuid = sqlx::query_scalar("SELECT id FROM cooperatives WHERE is_bootstrap")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let coop_b = repo::create_cooperative(&app.pool, "B5 Kooperatifi", None)
        .await
        .expect("cooperative B")
        .id;
    // The switch endpoint requires an active, business-enabled target.
    sqlx::query("UPDATE cooperatives SET status = 'active', business_enabled = true WHERE id = $1")
        .bind(coop_b)
        .execute(&app.pool)
        .await
        .unwrap();

    // Shared user: active membership in A (fixture) and B.
    let user = create_user(&app.pool, "rt-b5switch", &["income_expense.read"]).await;
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         VALUES ($1, $2, 'active')",
    )
    .bind(coop_b)
    .bind(user)
    .execute(&app.pool)
    .await
    .unwrap();

    let cookie = login(&app, "rt-b5switch").await;
    let mut ws_a = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws_a, &["income_expense.read"]).await;

    // Sanity: the bound scope delivers before the switch.
    commit_category(&app.pool, "b5-before-switch").await;
    assert_eq!(
        next_domain(&mut ws_a, EVENT_TIMEOUT).await.as_deref(),
        Some("income_expense"),
        "pre-switch socket must receive coop-A domain"
    );

    // CSRF token via /api/auth/me, then POST the switch to B.
    let me_req = Request::get("/api/auth/me")
        .header("cookie", &cookie)
        .header("origin", ORIGIN)
        .body(Body::empty())
        .unwrap();
    let me_resp = app.app.clone().oneshot(me_req).await.unwrap();
    assert_eq!(me_resp.status(), StatusCode::OK);
    let me_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(me_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let csrf = me_body["csrfToken"].as_str().unwrap().to_string();

    let switch_req = Request::post("/api/auth/cooperative")
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .header("cookie", &cookie)
        .header("x-csrf-token", &csrf)
        .body(Body::from(json!({ "cooperativeId": coop_b }).to_string()))
        .unwrap();
    let switch_started = tokio::time::Instant::now();
    let switch_resp = app.app.clone().oneshot(switch_req).await.unwrap();
    assert_eq!(switch_resp.status(), StatusCode::NO_CONTENT);

    // The stale socket gets the authz signal and closes promptly.
    let mut saw_close = false;
    let mut close_after_ms = None;
    let deadline = switch_started + EVENT_TIMEOUT;
    while let Some(message) = tokio::time::timeout_at(deadline, ws_a.next())
        .await
        .ok()
        .flatten()
    {
        if matches!(message, Ok(Message::Close(_))) {
            saw_close = true;
            close_after_ms = Some(switch_started.elapsed().as_millis());
            break;
        }
        if let Ok(Message::Text(text)) = &message {
            let frame: Value = serde_json::from_str(text).unwrap();
            if frame["type"] == "data-changed" {
                assert_eq!(
                    frame["domain"], "authz",
                    "stale socket may only see the authz invalidation after switch"
                );
            }
        }
    }
    assert!(saw_close, "cooperative switch must close the stale socket");
    println!(
        "switch→close latency: {} ms",
        close_after_ms.unwrap_or_default()
    );

    // M1-P6 B4: the handshake binds the SESSION scope — a fresh
    // headerless connect after the switch is bound to B. It receives
    // B's commits and never A's (scope filter before any revalidation).
    grant_role(
        &app.pool,
        user,
        "rt-b4-b-reader",
        Some(coop_b),
        &["income_expense.read"],
    )
    .await;
    let mut ws_b = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    let granted = subscribe(&mut ws_b, &["income_expense.read"]).await;
    assert!(
        granted.to_string().contains("income_expense.read"),
        "B-scoped grant must authorize the B-bound socket: {granted}"
    );
    commit_category(&app.pool, "b4-after-switch-a").await;
    let mut leaked = false;
    let deadline2 = tokio::time::Instant::now() + SILENCE_TIMEOUT;
    while tokio::time::Instant::now() < deadline2 {
        let message = match tokio::time::timeout_at(deadline2, ws_b.next()).await {
            Ok(Some(Ok(m))) => m,
            _ => break,
        };
        if let Message::Text(text) = message {
            let frame: Value = serde_json::from_str(&text).unwrap();
            if frame["type"] == "data-changed" && frame["domain"] == "income_expense" {
                leaked = true;
                break;
            }
        }
    }
    assert!(!leaked, "coop-A commit must never reach the B-bound socket");
    commit_category_in(&app.pool, "b4-after-switch-b", coop_b).await;
    assert_eq!(
        next_domain(&mut ws_b, EVENT_TIMEOUT).await.as_deref(),
        Some("income_expense"),
        "B-bound socket must receive coop-B commits"
    );
    // An explicit header naming A cannot rebind the socket to A.
    assert!(
        ws_connect_scoped(&app, Some(&cookie), Some(ORIGIN), Some(&coop_a.to_string()))
            .await
            .is_err(),
        "header must not override the session scope at handshake"
    );

    // Switching back rebinds new sockets to the bootstrap scope:
    // a new socket works again, and a coop-B commit never reaches it.
    let back_req = Request::post("/api/auth/cooperative")
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .header("cookie", &cookie)
        .header("x-csrf-token", &csrf)
        .body(Body::from(json!({ "cooperativeId": coop_a }).to_string()))
        .unwrap();
    let back_resp = app.app.clone().oneshot(back_req).await.unwrap();
    assert_eq!(back_resp.status(), StatusCode::NO_CONTENT);

    let mut ws_a3 = ws_connect(&app, Some(&cookie), Some(ORIGIN)).await.unwrap();
    subscribe(&mut ws_a3, &["income_expense.read"]).await;
    commit_category_in(&app.pool, "b5-foreign-b", coop_b).await;
    // The switch-back itself emits an in-scope `authz` invalidation
    // (the session row changed), which may still be in flight when the
    // coop-B commit lands. Only a non-authz domain would be a leak.
    let mut saw_foreign = false;
    let deadline3 = tokio::time::Instant::now() + SILENCE_TIMEOUT;
    while tokio::time::Instant::now() < deadline3 {
        let message = match tokio::time::timeout_at(deadline3, ws_a3.next()).await {
            Ok(Some(Ok(m))) => m,
            _ => break,
        };
        if let Message::Text(text) = message {
            let frame: Value = serde_json::from_str(&text).unwrap();
            if frame["type"] == "data-changed" && frame["domain"] != "authz" {
                saw_foreign = true;
                break;
            }
        }
    }
    assert!(
        !saw_foreign,
        "foreign cooperative commit must never reach the socket"
    );
    commit_category(&app.pool, "b5-after-switch-back").await;
    assert_eq!(
        next_domain(&mut ws_a3, EVENT_TIMEOUT).await.as_deref(),
        Some("income_expense"),
        "realigned scope must deliver after reconnect"
    );
}
