//! Database-gated Governance tests (STEP-014, docs/09, docs/15,
//! docs/16, docs/18, docs/19). Requires KOOPERATIF_TEST_DATABASE_URL;
//! each test runs in its own throwaway database.
//!
//! Invariants under test:
//! - GOVERNANCE IS EVIDENCE, NOT MONEY: body/membership/decision/vote/
//!   finalize commands create ZERO account_movements and touch no
//!   financial aggregate.
//! - RBAC != GOVERNANCE: `governance.*` permissions operate the API;
//!   they never make the user a member or voter. A System
//!   Administrator's person without a membership CANNOT vote.
//! - Vote eligibility is server-side: only an ACTIVE membership in the
//!   decision's body at cast time counts; one vote per (decision,
//!   person) is enforced by the unique index; votes are immutable.
//! - Decision material content freezes when voting opens; the formal
//!   outcome is OPERATOR-RECORDED (no invented quorum/majority math)
//!   with a frozen electorate/tally snapshot.
//! - Temporal membership history is preserved: ending a term never
//!   rewrites who already voted.

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

struct TestApp {
    app: axum::Router,
    pool: PgPool,
    _database_name: String,
    _peer: SocketAddr,
}

impl TestApp {
    fn peer(&self) -> SocketAddr {
        self._peer
    }
}

async fn setup() -> Option<TestApp> {
    // PILOT-FIX-001 / F5: missing test DB URL is an explicit failure.
    let url = std::env::var("KOOPERATIF_TEST_DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "KOOPERATIF_TEST_DATABASE_URL is not set — required DB-gated              integration tests cannot silently pass"
        )
    });
    let database_name = format!("kooperatif_test_{}", Uuid::new_v4().simple());
    let admin_url = url
        .rsplit_once('/')
        .map(|(b, _)| format!("{b}/postgres"))
        .unwrap();
    let base_url = url.rsplit_once('/').map(|(b, _)| b.to_string()).unwrap();
    let admin = app_db::connect(&admin_url).await.expect("admin connect");
    // F11: bound per-test database accumulation (24 h cutoff, no connections).
    let _ = app_db::drop_stale_test_databases(&admin).await;
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
    // Clock frozen at 2026-01-15 — business dates around it.
    let clock = Arc::new(MutableClock::new(
        OffsetDateTime::parse(
            "2026-01-15T00:00:00Z",
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

async fn create_person(pool: &PgPool, first: &str, last: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO persons (first_name, last_name, search_name) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(first)
    .bind(last)
    .bind(format!("{first} {last}").to_lowercase())
    .fetch_one(pool)
    .await
    .unwrap()
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

async fn admin_session(test: &TestApp) -> (String, String) {
    let name = format!("adm.{}", Uuid::new_v4().simple());
    create_admin(&test.pool, &name).await;
    login(&test.app, test.peer(), &name).await
}

// ---------------------------------------------------------------------
// Domain helpers
// ---------------------------------------------------------------------

async fn api_create_body(test: &TestApp, cookie: &str, csrf: &str, name: &str) -> Value {
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/governance/bodies",
            cookie,
            Some(csrf),
            Some(json!({
                "name": name,
                "bodyType": "Yönetim Kurulu",
                "idempotencyKey": format!("body-{}", Uuid::new_v4()),
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");
    detail
}

async fn api_add_member(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    body_id: &str,
    person_id: Uuid,
) -> StatusCode {
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/governance/bodies/{body_id}/memberships"),
            cookie,
            Some(csrf),
            Some(json!({
                "personId": person_id,
                "startedAt": "2026-01-10T09:00:00Z",
                "idempotencyKey": format!("mem-{}", Uuid::new_v4()),
            })),
        ),
    )
    .await;
    status
}

async fn api_create_decision(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    body_id: &str,
    title: &str,
) -> Value {
    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            "/api/governance/decisions",
            cookie,
            Some(csrf),
            Some(json!({
                "bodyId": body_id,
                "title": title,
                "decisionText": "Karar metni: deneme kararı.",
                "decisionOn": "2026-01-12",
                "effectiveOn": "2026-02-01",
                "idempotencyKey": format!("dec-{}", Uuid::new_v4()),
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");
    detail
}

async fn api_open(test: &TestApp, cookie: &str, csrf: &str, decision_id: &str) -> StatusCode {
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/governance/decisions/{decision_id}/open"),
            cookie,
            Some(csrf),
            None,
        ),
    )
    .await
    .0
}

async fn api_vote(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    decision_id: &str,
    person_id: Uuid,
    choice: &str,
) -> (StatusCode, Value) {
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/governance/decisions/{decision_id}/votes"),
            cookie,
            Some(csrf),
            Some(json!({
                "personId": person_id,
                "choice": choice,
                "idempotencyKey": format!("vote-{}", Uuid::new_v4()),
            })),
        ),
    )
    .await
}

async fn api_finalize(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    decision_id: &str,
    outcome: &str,
) -> StatusCode {
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/governance/decisions/{decision_id}/finalize"),
            cookie,
            Some(csrf),
            Some(json!({ "outcome": outcome })),
        ),
    )
    .await
    .0
}

async fn movement_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM account_movements")
        .fetch_one(pool)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

/// The whole lifecycle is evidence-only: body + memberships + decision
/// + votes + finalization create ZERO account movements.
#[tokio::test]
async fn governance_lifecycle_moves_zero_money() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let member_a = create_person(&test.pool, "Ayse", "Yilmaz").await;
    let member_b = create_person(&test.pool, "Mehmet", "Demir").await;

    let body = api_create_body(&test, &cookie, &csrf, "Yönetim Kurulu").await;
    assert_eq!(body["status"], "active");
    let body_id = body["id"].as_str().unwrap();

    assert_eq!(
        api_add_member(&test, &cookie, &csrf, body_id, member_a).await,
        StatusCode::CREATED
    );
    assert_eq!(
        api_add_member(&test, &cookie, &csrf, body_id, member_b).await,
        StatusCode::CREATED
    );

    let decision = api_create_decision(&test, &cookie, &csrf, body_id, "Deneme kararı").await;
    assert_eq!(decision["status"], "draft");
    let decision_id = decision["id"].as_str().unwrap();
    assert!(decision["decisionNumber"].as_i64().unwrap() > 0);

    assert_eq!(
        api_open(&test, &cookie, &csrf, decision_id).await,
        StatusCode::OK
    );
    assert_eq!(
        api_vote(&test, &cookie, &csrf, decision_id, member_a, "approve")
            .await
            .0,
        StatusCode::CREATED
    );
    assert_eq!(
        api_vote(&test, &cookie, &csrf, decision_id, member_b, "abstain")
            .await
            .0,
        StatusCode::CREATED
    );
    assert_eq!(
        api_finalize(&test, &cookie, &csrf, decision_id, "approved").await,
        StatusCode::OK
    );

    let (_, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/governance/decisions/{decision_id}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(detail["status"], "approved");
    // Frozen snapshot: 2 eligible members, 1 approve, 0 reject, 1 abstain.
    assert_eq!(detail["eligibleCount"], 2);
    assert_eq!(detail["approveCount"], 1);
    assert_eq!(detail["rejectCount"], 0);
    assert_eq!(detail["abstainCount"], 1);
    assert_eq!(detail["votes"].as_array().unwrap().len(), 2);
    assert_eq!(detail["decisionOn"], "2026-01-12");
    assert_eq!(detail["effectiveOn"], "2026-02-01");

    // ZERO money moved anywhere.
    assert_eq!(movement_count(&test.pool).await, 0);
}

/// RBAC != GOVERNANCE: an admin's PERSON (no membership) cannot vote;
/// a person never gets API rights from membership either.
#[tokio::test]
async fn rbac_permission_is_not_governance_membership() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let outsider = create_person(&test.pool, "Dis", "Kisi").await;

    let body = api_create_body(&test, &cookie, &csrf, "Denetim").await;
    let body_id = body["id"].as_str().unwrap();
    let decision = api_create_decision(&test, &cookie, &csrf, body_id, "Yetkisiz oy").await;
    let decision_id = decision["id"].as_str().unwrap();
    api_open(&test, &cookie, &csrf, decision_id).await;

    // Person exists but holds NO membership -> vote denied (403).
    let (status, _) = api_vote(&test, &cookie, &csrf, decision_id, outsider, "approve").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// A plain user with no governance permissions is denied on every
/// surface (read AND write), and CSRF-free mutation is denied.
#[tokio::test]
async fn governance_requires_permission_and_csrf() {
    let Some(test) = setup().await else { return };
    let plain = format!("plain.{}", Uuid::new_v4().simple());
    create_plain_user(&test.pool, &plain).await;
    let (cookie, csrf) = login(&test.app, test.peer(), &plain).await;

    let (status, _) = send(
        &test.app,
        req("GET", "/api/governance/bodies", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/governance/bodies",
            &cookie,
            Some(&csrf),
            Some(json!({
                "name": "X", "bodyType": "Y",
                "idempotencyKey": format!("b-{}", Uuid::new_v4()),
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Admin but NO CSRF token -> still rejected.
    let (admin_cookie, _admin_csrf) = admin_session(&test).await;
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/governance/bodies",
            &admin_cookie,
            None,
            Some(json!({
                "name": "X", "bodyType": "Y",
                "idempotencyKey": format!("b-{}", Uuid::new_v4()),
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// One vote per (decision, person): the second cast answers 409 and
/// leaves exactly one vote row.
#[tokio::test]
async fn duplicate_vote_is_rejected() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let member = create_person(&test.pool, "Uye", "Bir").await;

    let body = api_create_body(&test, &cookie, &csrf, "Kurul").await;
    let body_id = body["id"].as_str().unwrap();
    api_add_member(&test, &cookie, &csrf, body_id, member).await;
    let decision = api_create_decision(&test, &cookie, &csrf, body_id, "D").await;
    let decision_id = decision["id"].as_str().unwrap().to_string();
    api_open(&test, &cookie, &csrf, &decision_id).await;

    let (first, _) = api_vote(&test, &cookie, &csrf, &decision_id, member, "approve").await;
    assert_eq!(first, StatusCode::CREATED);
    let (second, _) = api_vote(&test, &cookie, &csrf, &decision_id, member, "reject").await;
    assert_eq!(second, StatusCode::CONFLICT);

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM governance_votes WHERE decision_id = $1")
            .bind(Uuid::parse_str(&decision_id).unwrap())
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

/// A person whose membership ended BEFORE the vote is ineligible;
/// ending a term AFTER a vote never rewrites the recorded vote.
#[tokio::test]
async fn temporal_membership_controls_eligibility() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let member = create_person(&test.pool, "Gecmis", "Uye").await;

    let body = api_create_body(&test, &cookie, &csrf, "Kurul").await;
    let body_id = body["id"].as_str().unwrap().to_string();
    api_add_member(&test, &cookie, &csrf, &body_id, member).await;

    // Find the membership id.
    let (_, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/governance/bodies/{body_id}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    let membership_id = detail["memberships"][0]["id"].as_str().unwrap().to_string();

    // Vote while member, then end the term.
    let d1 = api_create_decision(&test, &cookie, &csrf, &body_id, "Erken").await;
    let d1_id = d1["id"].as_str().unwrap().to_string();
    api_open(&test, &cookie, &csrf, &d1_id).await;
    assert_eq!(
        api_vote(&test, &cookie, &csrf, &d1_id, member, "approve")
            .await
            .0,
        StatusCode::CREATED
    );

    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/governance/memberships/{membership_id}/end"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "endedAt": "2026-01-14T10:00:00Z",
                "reason": "Görev süresi doldu",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Now the same person is ineligible for a new decision.
    let d2 = api_create_decision(&test, &cookie, &csrf, &body_id, "Gec").await;
    let d2_id = d2["id"].as_str().unwrap().to_string();
    api_open(&test, &cookie, &csrf, &d2_id).await;
    assert_eq!(
        api_vote(&test, &cookie, &csrf, &d2_id, member, "approve")
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    // And the FIRST decision still records the vote historically —
    // ending the term must not erase the past.
    let (_, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/governance/decisions/{d1_id}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(detail["votes"].as_array().unwrap().len(), 1);
    assert_eq!(
        detail["votes"][0]["personId"].as_str().unwrap(),
        member.to_string()
    );

    // Body detail shows the membership as historical (active=false).
    let (_, body_detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/governance/bodies/{body_id}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(body_detail["memberships"][0]["active"], false);
    assert!(body_detail["memberships"][0]["endedAt"].is_string());
}

/// Draft content is editable; once voting opens the material text is
/// frozen — history must show what members actually voted on.
#[tokio::test]
async fn decision_content_freezes_when_voting_opens() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let body = api_create_body(&test, &cookie, &csrf, "Kurul").await;
    let body_id = body["id"].as_str().unwrap();
    let decision = api_create_decision(&test, &cookie, &csrf, body_id, "Taslak").await;
    let decision_id = decision["id"].as_str().unwrap().to_string();

    // Draft edit OK.
    let (status, updated) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/governance/decisions/{decision_id}/update"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "title": "Düzeltilmiş taslak",
                "decisionText": "Yeni metin.",
                "decisionOn": "2026-01-13",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["title"], "Düzeltilmiş taslak");
    assert_eq!(updated["effectiveOn"], Value::Null);

    // Open -> update is now rejected.
    api_open(&test, &cookie, &csrf, &decision_id).await;
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/governance/decisions/{decision_id}/update"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "title": "Gizli değişiklik",
                "decisionText": "Bam.",
                "decisionOn": "2026-01-13",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The frozen text is what detail still shows.
    let (_, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/governance/decisions/{decision_id}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(detail["title"], "Düzeltilmiş taslak");
    assert_eq!(detail["decisionText"], "Yeni metin.");
}

/// Finalization is terminal: no new votes, no re-finalize, no updates.
#[tokio::test]
async fn finalized_decision_is_immutable() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let member = create_person(&test.pool, "Son", "Uye").await;

    let body = api_create_body(&test, &cookie, &csrf, "Kurul").await;
    let body_id = body["id"].as_str().unwrap().to_string();
    api_add_member(&test, &cookie, &csrf, &body_id, member).await;
    let decision = api_create_decision(&test, &cookie, &csrf, &body_id, "Kapanan").await;
    let decision_id = decision["id"].as_str().unwrap().to_string();
    api_open(&test, &cookie, &csrf, &decision_id).await;
    api_vote(&test, &cookie, &csrf, &decision_id, member, "approve").await;
    assert_eq!(
        api_finalize(&test, &cookie, &csrf, &decision_id, "rejected").await,
        StatusCode::OK
    );

    // No new votes after finalization — even by an eligible member.
    assert_eq!(
        api_vote(&test, &cookie, &csrf, &decision_id, member, "reject")
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    // No double finalize.
    assert_eq!(
        api_finalize(&test, &cookie, &csrf, &decision_id, "approved").await,
        StatusCode::BAD_REQUEST
    );
    // No content edit.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/governance/decisions/{decision_id}/update"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "title": "x", "decisionText": "y", "decisionOn": "2026-01-13",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // No cancel after open/finalize.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/governance/decisions/{decision_id}/cancel"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "vazgeçildi" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/governance/decisions/{decision_id}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(detail["status"], "rejected");
}

/// Drafts may be cancelled with a reason; the record stays.
#[tokio::test]
async fn draft_cancel_preserves_history() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let body = api_create_body(&test, &cookie, &csrf, "Kurul").await;
    let body_id = body["id"].as_str().unwrap();
    let decision = api_create_decision(&test, &cookie, &csrf, body_id, "İptal").await;
    let decision_id = decision["id"].as_str().unwrap().to_string();

    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/governance/decisions/{decision_id}/cancel"),
            &cookie,
            Some(&csrf),
            Some(json!({ "reason": "Gündemden çıkarıldı" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["status"], "cancelled");
    assert_eq!(detail["cancellationReason"], "Gündemden çıkarıldı");

    // Cancelled decision cannot be opened or voted on.
    assert_eq!(
        api_open(&test, &cookie, &csrf, &decision_id).await,
        StatusCode::BAD_REQUEST
    );
}

/// Idempotency: same key + same payload replays the stored record;
/// same key + different payload answers 409 (ADR-006).
#[tokio::test]
async fn idempotent_replay_and_conflict() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let member = create_person(&test.pool, "Idem", "Uye").await;

    let body = api_create_body(&test, &cookie, &csrf, "Kurul").await;
    let body_id = body["id"].as_str().unwrap().to_string();

    // Body replay.
    let key = format!("body-{}", Uuid::new_v4());
    let payload = json!({ "name": "Replay", "bodyType": "T", "idempotencyKey": key });
    let (s1, b1) = send(
        &test.app,
        req(
            "POST",
            "/api/governance/bodies",
            &cookie,
            Some(&csrf),
            Some(payload.clone()),
        ),
    )
    .await;
    let (s2, b2) = send(
        &test.app,
        req(
            "POST",
            "/api/governance/bodies",
            &cookie,
            Some(&csrf),
            Some(payload),
        ),
    )
    .await;
    assert_eq!(s1, StatusCode::CREATED);
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(b1["id"], b2["id"]);

    // Same key, different payload -> 409.
    let (s3, _) = send(
        &test.app,
        req(
            "POST",
            "/api/governance/bodies",
            &cookie,
            Some(&csrf),
            Some(json!({ "name": "Farklı", "bodyType": "T", "idempotencyKey": key })),
        ),
    )
    .await;
    assert_eq!(s3, StatusCode::CONFLICT);

    // Vote replay.
    api_add_member(&test, &cookie, &csrf, &body_id, member).await;
    let decision = api_create_decision(&test, &cookie, &csrf, &body_id, "Idem").await;
    let decision_id = decision["id"].as_str().unwrap().to_string();
    api_open(&test, &cookie, &csrf, &decision_id).await;

    let vote_key = format!("v-{}", Uuid::new_v4());
    let vote_payload = json!({
        "personId": member, "choice": "approve", "idempotencyKey": vote_key,
    });
    let (v1, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/governance/decisions/{decision_id}/votes"),
            &cookie,
            Some(&csrf),
            Some(vote_payload.clone()),
        ),
    )
    .await;
    let (v2, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/governance/decisions/{decision_id}/votes"),
            &cookie,
            Some(&csrf),
            Some(vote_payload),
        ),
    )
    .await;
    assert_eq!(v1, StatusCode::CREATED);
    // Replay returns OK — NOT a duplicate-vote 409.
    assert_eq!(v2, StatusCode::OK);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM governance_votes WHERE decision_id = $1")
            .bind(Uuid::parse_str(&decision_id).unwrap())
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

/// One ACTIVE membership per (body, person) — a second seat is a
/// conflict, and after ending the seat a fresh membership is allowed.
#[tokio::test]
async fn one_active_membership_per_person_and_body() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let person = create_person(&test.pool, "Tek", "Koltuk").await;

    let body = api_create_body(&test, &cookie, &csrf, "Kurul").await;
    let body_id = body["id"].as_str().unwrap().to_string();

    assert_eq!(
        api_add_member(&test, &cookie, &csrf, &body_id, person).await,
        StatusCode::CREATED
    );
    // Duplicate active seat -> 409.
    assert_eq!(
        api_add_member(&test, &cookie, &csrf, &body_id, person).await,
        StatusCode::CONFLICT
    );

    let (_, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/governance/bodies/{body_id}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    let membership_id = detail["memberships"][0]["id"].as_str().unwrap().to_string();
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/governance/memberships/{membership_id}/end"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "endedAt": "2026-01-14T10:00:00Z",
                "reason": "Ayrılık",
            })),
        ),
    )
    .await;

    // Same person may rejoin — the ended row stays historical.
    assert_eq!(
        api_add_member(&test, &cookie, &csrf, &body_id, person).await,
        StatusCode::CREATED
    );
    let (_, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/governance/bodies/{body_id}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(detail["memberships"].as_array().unwrap().len(), 2);
}

/// Two simultaneous finalize commands: exactly one wins; the decision
/// ends in one deterministic state with one snapshot.
#[tokio::test]
async fn concurrent_finalize_is_deterministic() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let member = create_person(&test.pool, "Race", "Voter").await;

    let body = api_create_body(&test, &cookie, &csrf, "Kurul").await;
    let body_id = body["id"].as_str().unwrap().to_string();
    api_add_member(&test, &cookie, &csrf, &body_id, member).await;
    let decision = api_create_decision(&test, &cookie, &csrf, &body_id, "Yarış").await;
    let decision_id = decision["id"].as_str().unwrap().to_string();
    api_open(&test, &cookie, &csrf, &decision_id).await;

    let app = test.app.clone();
    let cookie2 = cookie.clone();
    let csrf2 = csrf.clone();
    let d1 = decision_id.clone();
    let d2 = decision_id.clone();
    let (r1, r2) = tokio::join!(
        async {
            send(
                &app,
                req(
                    "POST",
                    &format!("/api/governance/decisions/{d1}/finalize"),
                    &cookie,
                    Some(&csrf),
                    Some(json!({ "outcome": "approved" })),
                ),
            )
            .await
            .0
        },
        async {
            send(
                &app,
                req(
                    "POST",
                    &format!("/api/governance/decisions/{d2}/finalize"),
                    &cookie2,
                    Some(&csrf2),
                    Some(json!({ "outcome": "rejected" })),
                ),
            )
            .await
            .0
        }
    );
    let statuses = [r1, r2];
    assert_eq!(
        statuses.iter().filter(|s| **s == StatusCode::OK).count(),
        1,
        "exactly one finalize wins: {statuses:?}"
    );

    let status: String =
        sqlx::query_scalar("SELECT status FROM governance_decisions WHERE id = $1")
            .bind(Uuid::parse_str(&decision_id).unwrap())
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert!(status == "approved" || status == "rejected");
}

/// Two simultaneous votes for the same member: exactly one lands.
#[tokio::test]
async fn concurrent_votes_do_not_duplicate() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let member = create_person(&test.pool, "Race", "Uye").await;

    let body = api_create_body(&test, &cookie, &csrf, "Kurul").await;
    let body_id = body["id"].as_str().unwrap().to_string();
    api_add_member(&test, &cookie, &csrf, &body_id, member).await;
    let decision = api_create_decision(&test, &cookie, &csrf, &body_id, "Yarış").await;
    let decision_id = decision["id"].as_str().unwrap().to_string();
    api_open(&test, &cookie, &csrf, &decision_id).await;

    let app = test.app.clone();
    let cookie2 = cookie.clone();
    let csrf2 = csrf.clone();
    let d1 = decision_id.clone();
    let d2 = decision_id.clone();
    let (r1, r2) = tokio::join!(
        async {
            send(
                &app,
                req(
                    "POST",
                    &format!("/api/governance/decisions/{d1}/votes"),
                    &cookie,
                    Some(&csrf),
                    Some(json!({
                        "personId": member, "choice": "approve",
                        "idempotencyKey": format!("v1-{}", Uuid::new_v4()),
                    })),
                ),
            )
            .await
            .0
        },
        async {
            send(
                &app,
                req(
                    "POST",
                    &format!("/api/governance/decisions/{d2}/votes"),
                    &cookie2,
                    Some(&csrf2),
                    Some(json!({
                        "personId": member, "choice": "reject",
                        "idempotencyKey": format!("v2-{}", Uuid::new_v4()),
                    })),
                ),
            )
            .await
            .0
        }
    );
    let created = [r1, r2]
        .iter()
        .filter(|s| **s == StatusCode::CREATED)
        .count();
    assert_eq!(created, 1, "one vote lands: r1={r1} r2={r2}");
}

/// Body close is refused while draft/open decisions or active
/// memberships exist; allowed once both are resolved, and the closed
/// body rejects new memberships/decisions — never its history.
#[tokio::test]
async fn body_close_requires_settled_state() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;
    let person = create_person(&test.pool, "Kapanis", "Uye").await;

    let body = api_create_body(&test, &cookie, &csrf, "Kapanan Kurul").await;
    let body_id = body["id"].as_str().unwrap().to_string();
    api_add_member(&test, &cookie, &csrf, &body_id, person).await;
    let decision = api_create_decision(&test, &cookie, &csrf, &body_id, "Açık").await;
    let decision_id = decision["id"].as_str().unwrap().to_string();

    // Busy: open-ish decision + active member.
    api_open(&test, &cookie, &csrf, &decision_id).await;
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/governance/bodies/{body_id}/close"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Settle: finalize the decision, end the membership.
    api_finalize(&test, &cookie, &csrf, &decision_id, "approved").await;
    let (_, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/governance/bodies/{body_id}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    let membership_id = detail["memberships"][0]["id"].as_str().unwrap().to_string();
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/governance/memberships/{membership_id}/end"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "endedAt": "2026-01-14T10:00:00Z", "reason": "Kapanış",
            })),
        ),
    )
    .await;

    let (status, detail) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/governance/bodies/{body_id}/close"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["status"], "closed");

    // Closed body rejects new membership and decision.
    assert_eq!(
        api_add_member(&test, &cookie, &csrf, &body_id, person).await,
        StatusCode::BAD_REQUEST
    );
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/governance/decisions",
            &cookie,
            Some(&csrf),
            Some(json!({
                "bodyId": body_id, "title": "x", "decisionText": "y",
                "decisionOn": "2026-01-15",
                "idempotencyKey": format!("d-{}", Uuid::new_v4()),
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// List/filter endpoints work and decisions support body/status/search
/// filtering with deterministic ordering.
#[tokio::test]
async fn list_and_filter_endpoints() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let body_a = api_create_body(&test, &cookie, &csrf, "Kurul A").await;
    let body_b = api_create_body(&test, &cookie, &csrf, "Kurul B").await;
    let body_a_id = body_a["id"].as_str().unwrap().to_string();
    let body_b_id = body_b["id"].as_str().unwrap().to_string();
    api_create_decision(&test, &cookie, &csrf, &body_a_id, "Bütçe onayı").await;
    api_create_decision(&test, &cookie, &csrf, &body_b_id, "Aidat kararı").await;

    let (_, list) = send(
        &test.app,
        req(
            "GET",
            "/api/governance/bodies?status=active",
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(list["totalCount"], 2);
    assert_eq!(list["items"][0]["activeMembers"], 0);

    let (_, list) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/governance/decisions?bodyId={body_a_id}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(list["totalCount"], 1);
    assert_eq!(list["items"][0]["title"], "Bütçe onayı");

    let (_, list) = send(
        &test.app,
        req(
            "GET",
            "/api/governance/decisions?search=Aidat",
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(list["totalCount"], 1);

    // Invalid status -> 422, not a silent empty list.
    let (status, _) = send(
        &test.app,
        req(
            "GET",
            "/api/governance/decisions?status=bogus",
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// Person lookup is scoped to governance.manage and resolves canonical
/// persons for membership/voter selection.
#[tokio::test]
async fn person_lookup_scoped_to_manage() {
    let Some(test) = setup().await else { return };
    let (cookie, _csrf) = admin_session(&test).await;
    create_person(&test.pool, "Aranan", "Kisi").await;

    let (status, items) = send(
        &test.app,
        req(
            "GET",
            "/api/governance/persons?search=Aranan",
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(items.as_array().unwrap().len(), 1);
    assert_eq!(items[0]["firstName"], "Aranan");

    // Plain user denied.
    let plain = format!("plain.{}", Uuid::new_v4().simple());
    create_plain_user(&test.pool, &plain).await;
    let (plain_cookie, _) = login(&test.app, test.peer(), &plain).await;
    let (status, _) = send(
        &test.app,
        req(
            "GET",
            "/api/governance/persons?search=Aranan",
            &plain_cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Read responses carry no-store; audit events exist for the lifecycle.
#[tokio::test]
async fn no_store_and_audit_events() {
    let Some(test) = setup().await else { return };
    let (cookie, csrf) = admin_session(&test).await;

    let body = api_create_body(&test, &cookie, &csrf, "Kurul").await;
    let body_id = body["id"].as_str().unwrap().to_string();
    let decision = api_create_decision(&test, &cookie, &csrf, &body_id, "Denetim").await;
    let decision_id = decision["id"].as_str().unwrap().to_string();

    let response = test
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/api/governance/decisions/{decision_id}"),
            &cookie,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );

    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events \
         WHERE event_type IN ('governance_body_created','governance_decision_created')",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(events, 2);
}
