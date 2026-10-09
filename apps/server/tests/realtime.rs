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
    sqlx::query(&format!("CREATE DATABASE {database_name}"))
        .execute(&admin)
        .await
        .expect("test db");
    admin.close().await;
    let db_url = format!("{base_url}/{database_name}");
    let pool = app_db::connect(&db_url).await.expect("db");
    app_db::run_migrations(&pool).await.expect("migrations");

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
    let user = users::create_user(pool, username, "RT", &hash)
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
    Ok(tokio_tungstenite::connect_async(request).await?.0)
}

async fn subscribe(ws: &mut Ws, scopes: &[&str]) -> Value {
    ws.send(Message::Text(
        json!({ "type": "subscribe", "scopes": scopes })
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let message = tokio::time::timeout(EVENT_TIMEOUT, ws.next())
        .await
        .expect("subscribed ack timeout")
        .expect("stream ended")
        .expect("frame");
    let frame = serde_json::from_str::<Value>(message.to_text().unwrap()).unwrap();
    assert_eq!(frame["type"], "subscribed");
    frame
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
        "INSERT INTO periods (name, collection_start_date, due_date) \
         VALUES ($1, '2026-01-01', '2026-02-01')",
    )
    .bind(name)
    .execute(pool)
    .await
    .unwrap();
}

async fn commit_category(pool: &PgPool, name: &str) {
    sqlx::query("INSERT INTO financial_categories (category_type, name) VALUES ('income', $1)")
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
        "INSERT INTO periods (name, collection_start_date, due_date) \
         VALUES ('rolled-back', '2026-01-01', '2026-02-01')",
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
        "INSERT INTO governance_bodies (name, body_type, idempotency_key, idempotency_fingerprint, created_by) \
         SELECT 'rt-body', 'board', $1, $1, id FROM users LIMIT 1",
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

    // The commit itself still happens (events are global); delivery to
    // this socket must stop — permission is re-read per signal.
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
