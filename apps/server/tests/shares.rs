//! Database-gated Shares domain tests (STEP-005 §60–§78). Requires
//! KOOPERATIF_TEST_DATABASE_URL; each test runs in its own throwaway
//! database (full isolation, including the global seeded-role state).

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
const TEST_PASSWORD: &str = "domain-test-parola-1";

fn test_database_url() -> Option<String> {
    std::env::var("KOOPERATIF_TEST_DATABASE_URL")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

struct TestApp {
    app: axum::Router,
    pool: PgPool,
    _peer: SocketAddr,
    /// db-verify.sh sweeps kooperatif_test_* leftovers.
    _database_name: String,
}

impl TestApp {
    fn peer(&self) -> SocketAddr {
        self._peer
    }
}

async fn setup() -> Option<TestApp> {
    if std::env::var("SHARES_TEST_TRACE").is_ok() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("kooperatif_server=debug")
            .try_init();
    }
    let url = test_database_url()?;
    let suffix = Uuid::new_v4().simple();
    let database_name = format!("kooperatif_test_{suffix}");
    let admin_url = url
        .rsplit_once('/')
        .map(|(b, _)| format!("{b}/postgres"))
        .unwrap();
    let base_url = url.rsplit_once('/').map(|(b, _)| b.to_string()).unwrap();
    let admin = app_db::connect(&admin_url).await.expect("admin connect");
    sqlx::query(&format!("CREATE DATABASE {database_name}"))
        .execute(&admin)
        .await
        .expect("test db");
    admin.close().await;
    let pool = app_db::connect(&format!("{base_url}/{database_name}"))
        .await
        .expect("db");
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
            "2026-01-01T00:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .expect("epoch"),
    ));
    let runtime = AuthRuntime::new(config, clock);
    let app = router(
        AppState {
            db: Some(pool.clone()),
            auth: Arc::new(runtime),
        },
        cors_layer(&[]),
    );
    let octets = Uuid::new_v4().as_bytes().to_vec();
    Some(TestApp {
        app,
        pool,
        _database_name: database_name,
        _peer: format!(
            "127.{}.{}.{}:{}",
            octets[0],
            octets[1],
            octets[2],
            52000 + octets[3] as u16
        )
        .parse()
        .expect("addr"),
    })
}

async fn create_admin(pool: &PgPool, username: &str) -> Uuid {
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).unwrap(),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).unwrap();
    let user = users::create_user(pool, username, "Test", &hash)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id) \
                 SELECT $1, id FROM roles WHERE name = 'Sistem Yöneticisi'",
    )
    .bind(user.id)
    .execute(pool)
    .await
    .unwrap();
    user.id
}

async fn create_plain_user(pool: &PgPool, username: &str) -> Uuid {
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).unwrap(),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).unwrap();
    users::create_user(pool, username, "Plain", &hash)
        .await
        .unwrap()
        .id
}

async fn login(app: &axum::Router, peer: SocketAddr, username: &str) -> (String, String) {
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
    let set_cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    (
        set_cookie
            .split(';')
            .next()
            .and_then(|p| p.split_once('='))
            .map(|(_, v)| v.to_string())
            .unwrap(),
        body["csrfToken"].as_str().unwrap().to_string(),
    )
}

fn req(
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
        builder.body(Body::from(body.to_string())).unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    }
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

/// Create a shareholder via the parties API; returns the shareholder id.
async fn api_create_shareholder(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    first: &str,
    last: &str,
    guardian: Option<(&str, &str)>,
    family_sequence: i64,
) -> Uuid {
    let body = json!({
        "person": { "mode": "new", "firstName": first, "lastName": last },
        "guardian": guardian.map(|(g, gl)| json!({ "mode": "new", "firstName": g, "lastName": gl })),
        "family": { "mode": "new", "sequenceNumber": family_sequence }
    });
    let (status, detail) = send(
        &test.app,
        req("POST", "/api/shareholders", cookie, Some(csrf), Some(body)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {detail}");
    detail["id"].as_str().unwrap().parse().unwrap()
}

/// Create a share allocated to a shareholder; returns the share detail.
async fn api_create_share(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    shareholder_id: Uuid,
    acquisition_type: &str,
    fee: Option<&str>,
) -> Value {
    let body = json!({
        "shareholderId": shareholder_id,
        "acquisitionType": acquisition_type,
        "acquisitionFee": fee,
    });
    let (status, detail) = send(
        &test.app,
        req("POST", "/api/shares", cookie, Some(csrf), Some(body)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "share create failed: {detail}");
    detail
}

async fn api_get_share(test: &TestApp, cookie: &str, share_id: Uuid) -> Value {
    let (status, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/shares/{share_id}"),
            cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail
}

/// A baseline admin session in a fresh test database.
async fn admin_session(test: &TestApp) -> (String, String) {
    let name = format!("adm.{}", Uuid::new_v4().simple());
    create_admin(&test.pool, &name).await;
    login(&test.app, test.peer(), &name).await
}

// §62/§63: create + initial acquisition in one transaction; canonical
// identity always carries Vasi context ("Belirtilmemiş" when absent).
#[tokio::test]
async fn initial_allocation_creates_share_with_canonical_identity() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let (cookie, csrf) = admin_session(&test).await;
    let shareholder = api_create_shareholder(&test, &cookie, &csrf, "Ali", "Veli", None, 410).await;

    let detail = api_create_share(
        &test,
        &cookie,
        &csrf,
        shareholder,
        "founder",
        Some("50000.00"),
    )
    .await;

    assert_eq!(detail["shareNumber"], 1);
    assert_eq!(detail["status"], "active");
    assert_eq!(detail["acquisitionType"], "founder");
    let owner = &detail["owner"];
    assert_eq!(
        owner["shareholderId"].as_str().unwrap(),
        shareholder.to_string()
    );
    assert_eq!(
        owner["displayLabel"].as_str().unwrap(),
        "Ali Veli · Vasi: Belirtilmemiş · Aile No 410"
    );
    let events = detail["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["eventType"], "initial_acquisition");
    assert_eq!(events[0]["amount"], "50000.00");
    assert_eq!(events[0]["currency"], "TRY");

    // §66: shareholder's current-shares endpoint lists the open interval.
    let (status, held) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/shareholders/{shareholder}/shares"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let held = held.as_array().unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0]["shareNumber"], 1);
}

// §17/§64: NULL vs explicit zero preserved; bad inputs rejected.
#[tokio::test]
async fn amount_null_vs_zero_and_validation() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let (cookie, csrf) = admin_session(&test).await;
    let s1 = api_create_shareholder(&test, &cookie, &csrf, "A", "Bedel", None, 420).await;
    let s2 = api_create_shareholder(&test, &cookie, &csrf, "B", "Bedel", None, 421).await;

    // No fee → amount NULL (not "0").
    let detail = api_create_share(&test, &cookie, &csrf, s1, "later_acquisition", None).await;
    assert!(detail["events"][0]["amount"].is_null(), "{detail}");

    // Explicit zero stays explicit zero.
    let detail = api_create_share(&test, &cookie, &csrf, s2, "founder", Some("0")).await;
    assert_eq!(detail["events"][0]["amount"], "0.00");

    // Rejected amounts / types / targets.
    for body in [
        json!({"shareholderId": s1, "acquisitionType": "founder", "acquisitionFee": "-1"}),
        json!({"shareholderId": s1, "acquisitionType": "founder", "acquisitionFee": "1.005"}),
        json!({"shareholderId": s1, "acquisitionType": "founder", "acquisitionFee": "abc"}),
        // 'transfer'/'sale' are ownership-change types, not initial allocation.
        json!({"shareholderId": s1, "acquisitionType": "transfer"}),
        json!({"shareholderId": s1, "acquisitionType": "sale"}),
        // Future effective date.
        json!({"shareholderId": s1, "acquisitionType": "founder", "effectiveAt": "2999-01-01T00:00:00Z"}),
    ] {
        let (status, out) = send(
            &test.app,
            req("POST", "/api/shares", &cookie, Some(&csrf), Some(body)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");
    }

    // Unknown shareholder → stable 404.
    let (status, out) = send(
        &test.app,
        req(
            "POST",
            "/api/shares",
            &cookie,
            Some(&csrf),
            Some(json!({
                "shareholderId": Uuid::new_v4(),
                "acquisitionType": "founder"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{out}");
    assert_eq!(out["error"]["code"], "not_found");

    // Inactive shareholder cannot receive a share (§45).
    let inactive = api_create_shareholder(&test, &cookie, &csrf, "Pasif", "Üye", None, 422).await;
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shareholders/{inactive}/status-change"),
            &cookie,
            Some(&csrf),
            Some(json!({ "to": "inactive" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, out) = send(
        &test.app,
        req(
            "POST",
            "/api/shares",
            &cookie,
            Some(&csrf),
            Some(json!({"shareholderId": inactive, "acquisitionType": "founder"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");
}

// §56/§57: transfer and sale close the current interval and open the
// new one transactionally; history is append-only with both sides'
// canonical identities.
#[tokio::test]
async fn transfer_and_sale_append_temporal_ownership_history() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let (cookie, csrf) = admin_session(&test).await;
    let a = api_create_shareholder(
        &test,
        &cookie,
        &csrf,
        "Satıcı",
        "Bir",
        Some(("Vasi", "Bir")),
        430,
    )
    .await;
    let b = api_create_shareholder(&test, &cookie, &csrf, "Alan", "İki", None, 431).await;
    let c = api_create_shareholder(&test, &cookie, &csrf, "Alan", "Üç", None, 432).await;

    let share = api_create_share(&test, &cookie, &csrf, a, "founder", None).await;
    let share_id: Uuid = share["id"].as_str().unwrap().parse().unwrap();
    let updated_at = share["updatedAt"].as_str().unwrap().to_string();

    // Transfer A → B.
    let (status, out) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shares/{share_id}/transfer"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "toShareholderId": b,
                "reason": "aile içi devir",
                "expectedUpdatedAt": updated_at,
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{out}");

    // Sale B → C with an agreed price (metadata only, §18–§19).
    let detail = api_get_share(&test, &cookie, share_id).await;
    let (status, out) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shares/{share_id}/sale"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "toShareholderId": c,
                "saleAmount": "150000.50",
                "reason": "özel satış",
                "expectedUpdatedAt": detail["updatedAt"].as_str().unwrap(),
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{out}");

    let detail = api_get_share(&test, &cookie, share_id).await;
    assert_eq!(
        detail["owner"]["shareholderId"].as_str().unwrap(),
        c.to_string()
    );
    assert_eq!(detail["acquisitionType"], "sale");

    let events = detail["events"].as_array().unwrap();
    assert_eq!(events.len(), 3, "{events:?}");
    assert_eq!(events[0]["eventType"], "initial_acquisition");
    assert_eq!(events[1]["eventType"], "transfer");
    assert_eq!(events[2]["eventType"], "sale");
    // Canonical identity on both sides of the sale event.
    assert_eq!(
        events[2]["from"]["displayLabel"].as_str().unwrap(),
        "Alan İki · Vasi: Belirtilmemiş · Aile No 431"
    );
    assert_eq!(
        events[1]["from"]["displayLabel"].as_str().unwrap(),
        "Satıcı Bir · Vasi: Vasi Bir · Aile No 430"
    );
    assert_eq!(events[2]["amount"], "150000.50");
    assert!(events[1]["amount"].is_null(), "transfer has no amount");

    // Temporal intervals: exactly one open interval per share (DB-level).
    let open: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM share_ownerships WHERE share_id = $1 AND ended_at IS NULL",
    )
    .bind(share_id)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(open, 1);
    let closed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM share_ownerships WHERE share_id = $1 AND ended_at IS NOT NULL",
    )
    .bind(share_id)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(closed, 2);

    // A and B hold nothing now; C holds the share.
    for (holder, expected) in [(a, 0), (b, 0), (c, 1)] {
        let (_, held) = send(
            &test.app,
            req(
                "GET",
                &format!("/api/shareholders/{holder}/shares"),
                &cookie,
                None,
                None,
            ),
        )
        .await;
        assert_eq!(held.as_array().unwrap().len(), expected, "holder {holder}");
    }
}

// §47/§56: stale writes, chronology, and invalid targets rejected.
#[tokio::test]
async fn ownership_change_guards_reject_invalid_operations() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let (cookie, csrf) = admin_session(&test).await;
    let a = api_create_shareholder(&test, &cookie, &csrf, "A", "Koruma", None, 440).await;
    let b = api_create_shareholder(&test, &cookie, &csrf, "B", "Koruma", None, 441).await;
    let share = api_create_share(&test, &cookie, &csrf, a, "founder", None).await;
    let share_id: Uuid = share["id"].as_str().unwrap().parse().unwrap();
    let updated_at = share["updatedAt"].as_str().unwrap().to_string();

    // Stale precondition → 409 stale_state.
    let (status, out) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shares/{share_id}/transfer"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "toShareholderId": b,
                "expectedUpdatedAt": "2000-01-01T00:00:00Z",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{out}");
    assert_eq!(out["error"]["code"], "stale_state");

    // Same owner → validation failure.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shares/{share_id}/transfer"),
            &cookie,
            Some(&csrf),
            Some(json!({"toShareholderId": a, "expectedUpdatedAt": updated_at})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Chronology: cannot date a transfer before the ownership began.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shares/{share_id}/transfer"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "toShareholderId": b,
                "effectiveAt": "1999-01-01T00:00:00Z",
                "expectedUpdatedAt": updated_at,
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Unknown share / unknown target → stable 404s.
    let (status, out) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shares/{}/transfer", Uuid::new_v4()),
            &cookie,
            Some(&csrf),
            Some(json!({"toShareholderId": b, "expectedUpdatedAt": updated_at})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{out}");
    let (status, out) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shares/{share_id}/transfer"),
            &cookie,
            Some(&csrf),
            Some(json!({"toShareholderId": Uuid::new_v4(), "expectedUpdatedAt": updated_at})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{out}");

    // Suspended share cannot be transferred.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shares/{share_id}/status-change"),
            &cookie,
            Some(&csrf),
            Some(json!({ "to": "suspended" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let detail = api_get_share(&test, &cookie, share_id).await;
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shares/{share_id}/transfer"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "toShareholderId": b,
                "expectedUpdatedAt": detail["updatedAt"].as_str().unwrap(),
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// §56: concurrent owner changes serialize — exactly one succeeds.
#[tokio::test]
async fn concurrent_transfers_are_serialized() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let (cookie, csrf) = admin_session(&test).await;
    let a = api_create_shareholder(&test, &cookie, &csrf, "A", "Yarış", None, 450).await;
    let b = api_create_shareholder(&test, &cookie, &csrf, "B", "Yarış", None, 451).await;
    let c = api_create_shareholder(&test, &cookie, &csrf, "C", "Yarış", None, 452).await;
    let share = api_create_share(&test, &cookie, &csrf, a, "founder", None).await;
    let share_id: Uuid = share["id"].as_str().unwrap().parse().unwrap();
    let updated_at = share["updatedAt"].as_str().unwrap().to_string();

    let app2 = test.app.clone();
    let cookie2 = cookie.clone();
    let csrf2 = csrf.clone();
    let first = async {
        test.app
            .clone()
            .oneshot(req(
                "POST",
                &format!("/api/shares/{share_id}/transfer"),
                &cookie,
                Some(&csrf),
                Some(json!({"toShareholderId": b, "expectedUpdatedAt": updated_at})),
            ))
            .await
            .unwrap()
            .status()
    };
    let second = async {
        app2.oneshot(req(
            "POST",
            &format!("/api/shares/{share_id}/transfer"),
            &cookie2,
            Some(&csrf2),
            Some(json!({"toShareholderId": c, "expectedUpdatedAt": updated_at})),
        ))
        .await
        .unwrap()
        .status()
    };
    let (s1, s2) = tokio::join!(first, second);
    let mut statuses = [s1, s2];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::NO_CONTENT, StatusCode::CONFLICT],
        "one transfer wins, the loser sees stale_state"
    );

    // Exactly one open interval remains.
    let open: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM share_ownerships WHERE share_id = $1 AND ended_at IS NULL",
    )
    .bind(share_id)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(open, 1);
}

// §30/§31 + docs/16: lifecycle commands are explicit; voided is terminal
// and restricted to mistaken records with no downstream history.
#[tokio::test]
async fn status_lifecycle_and_void_rules() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let (cookie, csrf) = admin_session(&test).await;
    let a = api_create_shareholder(&test, &cookie, &csrf, "A", "Durum", None, 460).await;
    let b = api_create_shareholder(&test, &cookie, &csrf, "B", "Durum", None, 461).await;
    let share = api_create_share(&test, &cookie, &csrf, a, "founder", None).await;
    let share_id: Uuid = share["id"].as_str().unwrap().parse().unwrap();

    // active ↔ suspended
    for to in ["suspended", "active"] {
        let (status, out) = send(
            &test.app,
            req(
                "POST",
                &format!("/api/shares/{share_id}/status-change"),
                &cookie,
                Some(&csrf),
                Some(json!({ "to": to, "reason": "test" })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{out}");
    }

    // Invalid transitions rejected (no-op and unknown status).
    for to in ["active", "bogus"] {
        let (status, _) = send(
            &test.app,
            req(
                "POST",
                &format!("/api/shares/{share_id}/status-change"),
                &cookie,
                Some(&csrf),
                Some(json!({ "to": to })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // A share with transfer history CANNOT be voided (§30: void is for
    // mistaken records only, not business-history erasure).
    let detail = api_get_share(&test, &cookie, share_id).await;
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shares/{share_id}/transfer"),
            &cookie,
            Some(&csrf),
            Some(json!({"toShareholderId": b, "expectedUpdatedAt": detail["updatedAt"]})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, out) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shares/{share_id}/status-change"),
            &cookie,
            Some(&csrf),
            Some(json!({ "to": "voided" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");

    // A share with only its initial event CAN be voided; the open
    // interval closes and voided is terminal.
    let share2 = api_create_share(&test, &cookie, &csrf, a, "later_acquisition", None).await;
    let share2_id: Uuid = share2["id"].as_str().unwrap().parse().unwrap();
    let (status, out) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shares/{share2_id}/status-change"),
            &cookie,
            Some(&csrf),
            Some(json!({ "to": "voided", "reason": "hatalı kayıt" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{out}");

    let detail = api_get_share(&test, &cookie, share2_id).await;
    assert_eq!(detail["status"], "voided");
    assert!(detail["owner"].is_null(), "voided share has no owner");
    assert_eq!(detail["events"].as_array().unwrap().len(), 2);
    assert_eq!(detail["events"][1]["eventType"], "voided");
    assert_eq!(detail["events"][1]["statusFrom"], "active");
    assert_eq!(detail["events"][1]["statusTo"], "voided");

    // Voided is terminal: status change and transfer both rejected.
    for (method_suffix, body) in [
        ("status-change", json!({ "to": "active" })),
        (
            "transfer",
            json!({"toShareholderId": b, "expectedUpdatedAt": detail["updatedAt"]}),
        ),
    ] {
        let (status, _) = send(
            &test.app,
            req(
                "POST",
                &format!("/api/shares/{share2_id}/{method_suffix}"),
                &cookie,
                Some(&csrf),
                Some(body),
            ),
        )
        .await;
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::CONFLICT,
            "voided terminal: got {status}"
        );
    }
}

// §66/§67: backend-authoritative permissions; seeded admin received the
// new keys via migration; a custom role can hold shares.read.
#[tokio::test]
async fn shares_authorization_and_permission_upgrade() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let perms: Vec<String> = sqlx::query_scalar(
        "SELECT p.key FROM role_permissions rp JOIN permissions p ON p.id = rp.permission_id \
         JOIN roles r ON r.id = rp.role_id WHERE r.name = 'Sistem Yöneticisi' ORDER BY p.key",
    )
    .fetch_all(&test.pool)
    .await
    .unwrap();
    assert!(perms.contains(&"shares.read".to_string()), "{perms:?}");
    assert!(perms.contains(&"shares.manage".to_string()), "{perms:?}");
    assert_eq!(perms.len(), 31);

    let plain_name = format!("plain.{}", Uuid::new_v4().simple());
    let plain_id = create_plain_user(&test.pool, &plain_name).await;
    let (plain_cookie, plain_csrf) = login(&test.app, test.peer(), &plain_name).await;

    // Unauthenticated → 401.
    let (status, body) = send(&test.app, req("GET", "/api/shares", "yok", None, None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "authentication_required");

    // Authenticated without shares.read → 403 permission_denied.
    let (status, body) = send(
        &test.app,
        req("GET", "/api/shares", &plain_cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "permission_denied");

    // Mutation without shares.manage → 403.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/shares",
            &plain_cookie,
            Some(&plain_csrf),
            Some(json!({
                "shareholderId": Uuid::new_v4(),
                "acquisitionType": "founder"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Custom role grants take effect immediately (§66).
    let custom_role: Uuid =
        sqlx::query_scalar("INSERT INTO roles (name) VALUES ('Hisse Görüntüleyici') RETURNING id")
            .fetch_one(&test.pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission_id) \
                 SELECT $1, id FROM permissions WHERE key = 'shares.read'",
    )
    .bind(custom_role)
    .execute(&test.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_role_assignments (user_id, role_id) VALUES ($1, $2)")
        .bind(plain_id)
        .bind(custom_role)
        .execute(&test.pool)
        .await
        .unwrap();

    let (status, _) = send(
        &test.app,
        req("GET", "/api/shares", &plain_cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // read-only role still cannot mutate.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/shares",
            &plain_cookie,
            Some(&plain_csrf),
            Some(json!({
                "shareholderId": Uuid::new_v4(),
                "acquisitionType": "founder"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// §68: share mutations require both CSRF layers (token + origin).
#[tokio::test]
async fn shares_mutations_require_full_csrf() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let (cookie, csrf) = admin_session(&test).await;
    let a = api_create_shareholder(&test, &cookie, &csrf, "A", "Csrf", None, 470).await;
    let body = json!({"shareholderId": a, "acquisitionType": "founder"});

    // Missing token.
    let (status, out) = send(
        &test.app,
        req("POST", "/api/shares", &cookie, None, Some(body.clone())),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{out}");
    // Wrong token.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/shares",
            &cookie,
            Some("yanlis"),
            Some(body.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Disallowed origin.
    let mut evil = req(
        "POST",
        "/api/shares",
        &cookie,
        Some(&csrf),
        Some(body.clone()),
    );
    evil.headers_mut().insert(
        "origin",
        axum::http::HeaderValue::from_static("https://evil.example"),
    );
    let (status, _) = send(&test.app, evil).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Nothing was persisted by the rejected attempts.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM shares")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

// §63/§70: share list search (owner name, share/family number) and
// stable pagination.
#[tokio::test]
async fn share_list_search_and_stable_ordering() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let (cookie, csrf) = admin_session(&test).await;
    let aranan = api_create_shareholder(&test, &cookie, &csrf, "Aranan", "Kişi", None, 480).await;
    let e1 = api_create_shareholder(&test, &cookie, &csrf, "Ek481", "Kişi", None, 481).await;
    let e2 = api_create_shareholder(&test, &cookie, &csrf, "Ek482", "Kişi", None, 482).await;
    api_create_share(&test, &cookie, &csrf, aranan, "founder", None).await;
    api_create_share(&test, &cookie, &csrf, e1, "later_acquisition", None).await;
    api_create_share(&test, &cookie, &csrf, e2, "later_acquisition", None).await;

    // Owner-name search (Turkish fold) finds the share by owner.
    let (status, list) = send(
        &test.app,
        req("GET", "/api/shares?search=aranan", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().unwrap().len(), 1, "{list}");
    assert_eq!(list["items"][0]["shareNumber"], 1);

    // Family-number search resolves through the owner's current family.
    let (status, list) = send(
        &test.app,
        req("GET", "/api/shares?search=481", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().unwrap().len(), 1, "{list}");
    assert_eq!(list["items"][0]["shareNumber"], 2);

    // Share-number search.
    let (status, list) = send(
        &test.app,
        req("GET", "/api/shares?search=3", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["items"][0]["shareNumber"], 3);

    // Stable ordering by share_number.
    let (status, list) = send(
        &test.app,
        req("GET", "/api/shares?pageSize=10", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let numbers: Vec<i64> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["shareNumber"].as_i64().unwrap())
        .collect();
    assert_eq!(numbers, vec![1, 2, 3]);
}

// §52: security audit trail records share lifecycle mutations.
#[tokio::test]
async fn share_mutations_are_audited() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let (cookie, csrf) = admin_session(&test).await;
    let a = api_create_shareholder(&test, &cookie, &csrf, "A", "Denetim", None, 490).await;
    let b = api_create_shareholder(&test, &cookie, &csrf, "B", "Denetim", None, 491).await;
    let share = api_create_share(&test, &cookie, &csrf, a, "founder", Some("1000")).await;
    let share_id: Uuid = share["id"].as_str().unwrap().parse().unwrap();

    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shares/{share_id}/sale"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "toShareholderId": b,
                "saleAmount": "2000.75",
                "expectedUpdatedAt": share["updatedAt"],
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shares/{share_id}/status-change"),
            &cookie,
            Some(&csrf),
            Some(json!({ "to": "suspended" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let events: Vec<String> = sqlx::query_scalar(
        "SELECT event_type FROM security_events \
         WHERE event_type IN ('share_created','share_transferred','share_sold','share_status_changed') \
         ORDER BY occurred_at, id",
    )
    .fetch_all(&test.pool)
    .await
    .unwrap();
    assert_eq!(
        events,
        vec!["share_created", "share_sold", "share_status_changed"],
        "{events:?}"
    );

    // Audit payload carries the business facts (no secrets, no floats).
    let payload: Value =
        sqlx::query_scalar("SELECT metadata FROM security_events WHERE event_type = 'share_sold'")
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert_eq!(payload["share_id"].as_str().unwrap(), share_id.to_string());
    assert_eq!(
        payload["to_shareholder_id"].as_str().unwrap(),
        b.to_string()
    );
    assert_eq!(payload["amount"], "2000.75");
}
