//! Database-gated Periods & Assessments domain tests (STEP-006).
//! Requires KOOPERATIF_TEST_DATABASE_URL; each test runs in its own
//! throwaway database (full isolation, including seeded-role state).

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
    _database_name: String,
}

impl TestApp {
    fn peer(&self) -> SocketAddr {
        self._peer
    }
}

async fn setup() -> Option<TestApp> {
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

async fn api_create_share(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    shareholder_id: Uuid,
    effective_at: Option<&str>,
) -> Value {
    let body = json!({
        "shareholderId": shareholder_id,
        "acquisitionType": "founder",
        "effectiveAt": effective_at,
    });
    let (status, detail) = send(
        &test.app,
        req("POST", "/api/shares", cookie, Some(csrf), Some(body)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "share create failed: {detail}");
    detail
}

async fn admin_session(test: &TestApp) -> (String, String) {
    let name = format!("adm.{}", Uuid::new_v4().simple());
    create_admin(&test.pool, &name).await;
    login(&test.app, test.peer(), &name).await
}

fn period_body(rule: &str, amount: &str, effective: &str) -> Value {
    json!({
        "name": "2026 Ekim Dönemi",
        "collectionStartDate": "2026-10-01",
        "dueDate": "2026-10-15",
        "ruleType": rule,
        "baseAmount": amount,
        "assessmentEffectiveDate": effective,
    })
}

async fn create_period(test: &TestApp, cookie: &str, csrf: &str, body: Value) -> Value {
    let (status, detail) = send(
        &test.app,
        req("POST", "/api/periods", cookie, Some(csrf), Some(body)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "period create failed: {detail}"
    );
    detail
}

async fn get_period(test: &TestApp, cookie: &str, id: &str) -> Value {
    let (status, detail) = send(
        &test.app,
        req("GET", &format!("/api/periods/{id}"), cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail
}

async fn preview(test: &TestApp, cookie: &str, csrf: &str, id: &str) -> (StatusCode, Value) {
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/periods/{id}/assessment-preview"),
            cookie,
            Some(csrf),
            None,
        ),
    )
    .await
}

async fn generate(test: &TestApp, cookie: &str, csrf: &str, id: &str) -> (StatusCode, Value) {
    send(
        &test.app,
        req(
            "POST",
            &format!("/api/periods/{id}/generate-assessments"),
            cookie,
            Some(csrf),
            None,
        ),
    )
    .await
}

// ---------------------------------------------------------------------
// Lifecycle, validation, permissions, CSRF, 404
// ---------------------------------------------------------------------

#[tokio::test]
async fn create_lists_detail_and_audits() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let (cookie, csrf) = admin_session(&test).await;

    let created = create_period(
        &test,
        &cookie,
        &csrf,
        period_body("per_shareholder", "1000.00", "2026-10-01"),
    )
    .await;
    assert_eq!(created["periodNumber"], 1);
    assert_eq!(created["status"], "draft");
    assert_eq!(created["ruleType"], "per_shareholder");
    assert_eq!(created["baseAmount"], "1000.00");
    assert_eq!(created["currency"], "TRY");
    assert_eq!(created["assessmentCount"], 0);
    assert!(created["totalAssessment"].is_null(), "{created}");

    let id = created["id"].as_str().unwrap();
    let detail = get_period(&test, &cookie, id).await;
    assert_eq!(detail["collectionStartDate"], "2026-10-01");
    assert_eq!(detail["dueDate"], "2026-10-15");

    let (status, list) = send(
        &test.app,
        req("GET", "/api/periods?search=ekim", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["totalCount"], 1);

    let event_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events WHERE event_type = 'period_created'",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(event_count, 1);
}

#[tokio::test]
async fn create_validation_and_security_guards() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let (cookie, csrf) = admin_session(&test).await;

    for body in [
        // due < start (§11)
        json!({"name":"X","collectionStartDate":"2026-10-10","dueDate":"2026-10-01",
               "ruleType":"per_shareholder","baseAmount":"1.00","assessmentEffectiveDate":"2026-10-01"}),
        // unknown rule type — never guessed (§13)
        period_body("per_family", "1.00", "2026-10-01"),
        period_body("weighted", "1.00", "2026-10-01"),
        // zero/negative/malformed base amount (§41, NUMERIC bound)
        period_body("per_shareholder", "0", "2026-10-01"),
        period_body("per_shareholder", "0.00", "2026-10-01"),
        period_body("per_shareholder", "-5.00", "2026-10-01"),
        period_body("per_shareholder", "1.005", "2026-10-01"),
        period_body("per_shareholder", "999999999999999999.99", "2026-10-01"),
        // malformed dates / empty name
        json!({"name":" ","collectionStartDate":"2026-10-01","dueDate":"2026-10-15",
               "ruleType":"per_shareholder","baseAmount":"1.00","assessmentEffectiveDate":"2026-10-01"}),
        json!({"name":"X","collectionStartDate":"01/10/2026","dueDate":"2026-10-15",
               "ruleType":"per_shareholder","baseAmount":"1.00","assessmentEffectiveDate":"2026-10-01"}),
    ] {
        let (status, out) = send(
            &test.app,
            req("POST", "/api/periods", &cookie, Some(&csrf), Some(body)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");
    }

    // Missing CSRF token on a mutation → 403 csrf_failed.
    let (status, out) = send(
        &test.app,
        req(
            "POST",
            "/api/periods",
            &cookie,
            None,
            Some(period_body("per_shareholder", "1.00", "2026-10-01")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(out["error"]["code"], "csrf_failed");

    // Unauthenticated → 401.
    let (status, _out) = send(
        &test.app,
        req("GET", "/api/periods", "bogus-cookie", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Plain user (no roles) → 403 permission_denied on read AND manage.
    let plain = format!("plain.{}", Uuid::new_v4().simple());
    create_plain_user(&test.pool, &plain).await;
    let (pcookie, pcsrf) = login(&test.app, test.peer(), &plain).await;
    let (status, out) = send(&test.app, req("GET", "/api/periods", &pcookie, None, None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(out["error"]["code"], "permission_denied");
    let (status, out) = send(
        &test.app,
        req(
            "POST",
            "/api/periods",
            &pcookie,
            Some(&pcsrf),
            Some(period_body("per_shareholder", "1.00", "2026-10-01")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(out["error"]["code"], "permission_denied");

    // Unknown ids → stable 404 (no existence leak).
    let random = Uuid::new_v4();
    let (status, out) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/periods/{random}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(out["error"]["code"], "not_found");
    let (status, out) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/assessments/{random}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(out["error"]["code"], "not_found");
}

// ---------------------------------------------------------------------
// per_shareholder: preview → finalize → immutability
// ---------------------------------------------------------------------

#[tokio::test]
async fn per_shareholder_preview_generate_and_guards() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let (cookie, csrf) = admin_session(&test).await;

    let a = api_create_shareholder(&test, &cookie, &csrf, "A", "Üye", None, 500).await;
    let b =
        api_create_shareholder(&test, &cookie, &csrf, "B", "Üye", Some(("Vasi", "B")), 501).await;
    // Inactive shareholder is NOT eligible (§25).
    let inactive = api_create_shareholder(&test, &cookie, &csrf, "Pasif", "Üye", None, 502).await;
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

    let period = create_period(
        &test,
        &cookie,
        &csrf,
        period_body("per_shareholder", "500.00", "2026-10-01"),
    )
    .await;
    let id = period["id"].as_str().unwrap().to_string();

    // Preview: explains the pending run, persists NOTHING.
    let (status, view) = preview(&test, &cookie, &csrf, &id).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["eligibleShareholderCount"], 2);
    assert_eq!(view["eligibleShareCount"], 0);
    assert_eq!(view["assessmentCount"], 2);
    assert_eq!(view["totalAmount"], "1000.00");
    assert_eq!(view["ruleType"], "per_shareholder");
    let rows = view["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    // Canonical identity on every preview row (Vasi context always).
    let b_row = rows
        .iter()
        .find(|r| r["shareholder"]["shareholderId"].as_str().unwrap() == b.to_string())
        .unwrap();
    assert_eq!(
        b_row["shareholder"]["displayLabel"].as_str().unwrap(),
        "B Üye · Vasi: Vasi B · Aile No 501"
    );
    assert_eq!(b_row["amount"], "500.00");
    // Preview did not persist obligations or change lifecycle.
    let detail = get_period(&test, &cookie, &id).await;
    assert_eq!(detail["status"], "draft");
    assert_eq!(detail["assessmentCount"], 0);

    // Finalize: durable obligations appear, period opens.
    let (status, out) = generate(&test, &cookie, &csrf, &id).await;
    assert_eq!(status, StatusCode::CREATED, "{out}");
    assert_eq!(out["status"], "open");
    assert_eq!(out["assessmentCount"], 2);
    assert_eq!(out["totalAssessment"], "1000.00");

    // §32/§33: double generation is impossible — 409 both commands.
    let (status, out) = generate(&test, &cookie, &csrf, &id).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(out["error"]["code"], "conflict");
    let (status, _out) = preview(&test, &cookie, &csrf, &id).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Assessment list: canonical debtor identity + snapshot fields.
    let (status, list) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/periods/{id}/assessments"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    for item in items {
        assert_eq!(item["amount"], "500.00");
        assert_eq!(item["baseAmount"], "500.00");
        assert_eq!(item["ruleType"], "per_shareholder");
        assert_eq!(item["shareCount"], 0);
        assert_eq!(item["status"], "active");
    }
    let a_item = items
        .iter()
        .find(|i| i["shareholder"]["shareholderId"].as_str().unwrap() == a.to_string())
        .unwrap();
    assert!(a_item["shareholder"]["displayLabel"]
        .as_str()
        .unwrap()
        .contains("Vasi: Belirtilmemiş"));

    // Shareholder-scoped history endpoint (§62).
    let (status, mine) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/shareholders/{a}/assessments"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mine = mine.as_array().unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0]["amount"], "500.00");
    assert_eq!(mine[0]["periodNumber"], 1);

    // Audit: one generation event with counts/total.
    let gen: Value = sqlx::query_scalar(
        "SELECT metadata FROM security_events WHERE event_type = 'assessments_generated'",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(gen["assessment_count"], 2);
    assert_eq!(gen["total_amount"], "1000.00");
}

// ---------------------------------------------------------------------
// per_share: eligible shares, provenance, effective-date resolution
// ---------------------------------------------------------------------

#[tokio::test]
async fn per_share_generation_provenance_and_effective_point() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let (cookie, csrf) = admin_session(&test).await;

    let a = api_create_shareholder(&test, &cookie, &csrf, "Önceki", "Sahip", None, 510).await;
    let b = api_create_shareholder(&test, &cookie, &csrf, "Yeni", "Sahip", None, 511).await;

    // A acquires on 2025-12-01; transfers to B on 2025-12-20.
    let share = api_create_share(&test, &cookie, &csrf, a, Some("2025-12-01T00:00:00Z")).await;
    let share_id = share["id"].as_str().unwrap().to_string();
    let (status, out) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shares/{share_id}/transfer"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "toShareholderId": b,
                "effectiveAt": "2025-12-20T00:00:00Z",
                "expectedUpdatedAt": share["updatedAt"].as_str().unwrap(),
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{out}");

    // Effective date INSIDE A's ownership interval → A owes, even though
    // B currently holds the share (§26/§27: ownership as of the point).
    let period = create_period(
        &test,
        &cookie,
        &csrf,
        period_body("per_share", "250.00", "2025-12-10"),
    )
    .await;
    let id = period["id"].as_str().unwrap().to_string();

    let (status, view) = preview(&test, &cookie, &csrf, &id).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["eligibleShareCount"], 1);
    assert_eq!(view["eligibleShareholderCount"], 1);
    assert_eq!(view["totalAmount"], "250.00");
    assert_eq!(
        view["rows"][0]["shareholder"]["shareholderId"]
            .as_str()
            .unwrap(),
        a.to_string()
    );

    let (status, out) = generate(&test, &cookie, &csrf, &id).await;
    assert_eq!(status, StatusCode::CREATED, "{out}");

    let (status, list) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/periods/{id}/assessments"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(
        item["shareholder"]["shareholderId"].as_str().unwrap(),
        a.to_string()
    );
    assert_eq!(item["amount"], "250.00");
    assert_eq!(item["shareCount"], 1);
    assert_eq!(item["assessmentEffectiveDate"], "2025-12-10");

    // Provenance: the source row pins the share AND the ownership
    // interval that produced the obligation (§23).
    let (status, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/assessments/{}", item["id"].as_str().unwrap()),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    let sources = detail["sources"].as_array().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0]["shareId"].as_str().unwrap(), share_id);
    assert_eq!(sources[0]["amountComponent"], "250.00");
}

#[tokio::test]
async fn per_share_multiple_shares_and_eligibility_exclusions() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let (cookie, csrf) = admin_session(&test).await;

    let a = api_create_shareholder(&test, &cookie, &csrf, "Çok", "Hisse", None, 520).await;
    let b = api_create_shareholder(&test, &cookie, &csrf, "Tek", "Hisse", None, 521).await;
    let inactive = api_create_shareholder(&test, &cookie, &csrf, "Pasif", "Hisse", None, 522).await;

    // A holds 2 shares; B holds 1; the inactive shareholder's share is
    // created while still active, then the shareholder is deactivated
    // — generation must exclude the shareholder (§25).
    let s1 = api_create_share(&test, &cookie, &csrf, a, None).await;
    api_create_share(&test, &cookie, &csrf, a, None).await;
    api_create_share(&test, &cookie, &csrf, b, None).await;
    let suspended_share = api_create_share(&test, &cookie, &csrf, b, None).await;
    api_create_share(&test, &cookie, &csrf, inactive, None).await;

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
    // Suspended shares are excluded (§25).
    let (status, out) = send(
        &test.app,
        req(
            "POST",
            &format!(
                "/api/shares/{}/status-change",
                suspended_share["id"].as_str().unwrap()
            ),
            &cookie,
            Some(&csrf),
            Some(json!({ "to": "suspended" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{out}");

    let period = create_period(
        &test,
        &cookie,
        &csrf,
        period_body("per_share", "100.00", "2026-01-01"),
    )
    .await;
    let id = period["id"].as_str().unwrap().to_string();

    let (status, view) = preview(&test, &cookie, &csrf, &id).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    // 3 eligible shares (A×2, B×1); suspended + inactive excluded.
    assert_eq!(view["eligibleShareCount"], 3);
    assert_eq!(view["eligibleShareholderCount"], 2);
    assert_eq!(view["totalAmount"], "300.00");

    let (status, out) = generate(&test, &cookie, &csrf, &id).await;
    assert_eq!(status, StatusCode::CREATED, "{out}");

    let (status, list) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/periods/{id}/assessments"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let a_item = items
        .iter()
        .find(|i| i["shareholder"]["shareholderId"].as_str().unwrap() == a.to_string())
        .unwrap();
    let b_item = items
        .iter()
        .find(|i| i["shareholder"]["shareholderId"].as_str().unwrap() == b.to_string())
        .unwrap();
    assert_eq!(a_item["amount"], "200.00");
    assert_eq!(a_item["shareCount"], 2);
    assert_eq!(b_item["amount"], "100.00");
    assert_eq!(b_item["shareCount"], 1);

    // Provenance on the multi-share row: two pinned source shares.
    let (status, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/assessments/{}", a_item["id"].as_str().unwrap()),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let sources = detail["sources"].as_array().unwrap();
    assert_eq!(sources.len(), 2);
    let source_ids: Vec<String> = sources
        .iter()
        .map(|s| s["shareId"].as_str().unwrap().to_string())
        .collect();
    assert!(source_ids.contains(&s1["id"].as_str().unwrap().to_string()));
    assert!(
        !source_ids.contains(&suspended_share["id"].as_str().unwrap().to_string()),
        "suspended share must never source an obligation"
    );
}

// ---------------------------------------------------------------------
// Historical stability: later mutations never rewrite obligations
// ---------------------------------------------------------------------

#[tokio::test]
async fn generated_assessments_are_historically_stable() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let (cookie, csrf) = admin_session(&test).await;

    let a = api_create_shareholder(&test, &cookie, &csrf, "Eski", "Ad", None, 530).await;
    let b = api_create_shareholder(&test, &cookie, &csrf, "Devralan", "Kişi", None, 531).await;
    let share = api_create_share(&test, &cookie, &csrf, a, None).await;
    let share_id = share["id"].as_str().unwrap().to_string();

    let period = create_period(
        &test,
        &cookie,
        &csrf,
        period_body("per_share", "750.00", "2026-01-01"),
    )
    .await;
    let id = period["id"].as_str().unwrap().to_string();
    let (status, out) = generate(&test, &cookie, &csrf, &id).await;
    assert_eq!(status, StatusCode::CREATED, "{out}");

    let (status, list) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/periods/{id}/assessments"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let item = &list["items"][0];
    let assessment_id = item["id"].as_str().unwrap().to_string();
    assert_eq!(
        item["shareholder"]["shareholderId"].as_str().unwrap(),
        a.to_string()
    );

    // §35: sell the share to B AFTER generation.
    let current = send(
        &test.app,
        req(
            "GET",
            &format!("/api/shares/{share_id}"),
            &cookie,
            None,
            None,
        ),
    )
    .await
    .1;
    let (status, out) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shares/{share_id}/sale"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "toShareholderId": b,
                "saleAmount": "9000.00",
                "expectedUpdatedAt": current["updatedAt"].as_str().unwrap(),
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{out}");

    // §35: change family + guardian + name AFTER generation.
    let a_detail = send(
        &test.app,
        req(
            "GET",
            &format!("/api/shareholders/{a}"),
            &cookie,
            None,
            None,
        ),
    )
    .await
    .1;
    let (status, out) = send(
        &test.app,
        req(
            "PATCH",
            &format!("/api/shareholders/{a}"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "firstName": "Değişmiş",
                "lastName": "İsim",
                "guardian": { "mode": "new", "firstName": "Yeni", "lastName": "Vasi" },
                "expectedUpdatedAt": a_detail["updatedAt"].as_str().unwrap(),
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, out) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shareholders/{a}/family-change"),
            &cookie,
            Some(&csrf),
            Some(json!({
                "family": { "mode": "new", "sequenceNumber": 999 },
                "expectedUpdatedAt": out_after_patch(&test, &cookie, &a).await,
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{out}");

    // The obligation still belongs to A: amount, rule snapshot, debtor
    // and provenance are unchanged.
    let (status, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/assessments/{assessment_id}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(
        detail["shareholder"]["shareholderId"].as_str().unwrap(),
        a.to_string()
    );
    assert_eq!(detail["amount"], "750.00");
    assert_eq!(detail["ruleType"], "per_share");
    assert_eq!(detail["sources"][0]["shareId"].as_str().unwrap(), share_id);

    // B (the current share owner) owes nothing for this period.
    let (status, b_assessments) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/shareholders/{b}/assessments"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(b_assessments.as_array().unwrap().len(), 0);
}

async fn out_after_patch(test: &TestApp, cookie: &str, shareholder: &Uuid) -> String {
    send(
        &test.app,
        req(
            "GET",
            &format!("/api/shareholders/{shareholder}"),
            cookie,
            None,
            None,
        ),
    )
    .await
    .1["updatedAt"]
        .as_str()
        .unwrap()
        .to_string()
}

// ---------------------------------------------------------------------
// Draft mutation guards, stale edits, delete, close
// ---------------------------------------------------------------------

#[tokio::test]
async fn draft_update_delete_close_lifecycle() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let (cookie, csrf) = admin_session(&test).await;
    let a = api_create_shareholder(&test, &cookie, &csrf, "A", "Yaşam", None, 540).await;
    // The draft PATCH below switches the rule to per_share; give the
    // shareholder one share owned before the effective point so the
    // generated obligation count stays 1.
    api_create_share(&test, &cookie, &csrf, a, None).await;

    let period = create_period(
        &test,
        &cookie,
        &csrf,
        period_body("per_shareholder", "100.00", "2026-10-01"),
    )
    .await;
    let id = period["id"].as_str().unwrap().to_string();
    let updated_at = period["updatedAt"].as_str().unwrap().to_string();

    // Draft PATCH without expectedUpdatedAt → 400; with stale → 409.
    let mut body = period_body("per_share", "200.00", "2026-10-02");
    body["name"] = json!("Güncel Dönem");
    let (status, _) = send(
        &test.app,
        req(
            "PATCH",
            &format!("/api/periods/{id}"),
            &cookie,
            Some(&csrf),
            Some(body.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    body["expectedUpdatedAt"] = json!("2000-01-01T00:00:00Z");
    let (status, out) = send(
        &test.app,
        req(
            "PATCH",
            &format!("/api/periods/{id}"),
            &cookie,
            Some(&csrf),
            Some(body.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(out["error"]["code"], "stale_state");

    // Correct precondition → rule + dates update in place.
    body["expectedUpdatedAt"] = json!(updated_at);
    let (status, out) = send(
        &test.app,
        req(
            "PATCH",
            &format!("/api/periods/{id}"),
            &cookie,
            Some(&csrf),
            Some(body),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["ruleType"], "per_share");
    assert_eq!(out["baseAmount"], "200.00");
    assert_eq!(out["name"], "Güncel Dönem");

    // Close on a draft is an invalid transition → 409.
    let (status, _out) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/periods/{id}/close"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Finalize, then PATCH/DELETE must be rejected (§37/§38).
    let (status, _) = generate(&test, &cookie, &csrf, &id).await;
    assert_eq!(status, StatusCode::CREATED);
    let fresh = get_period(&test, &cookie, &id).await;
    let mut patch = period_body("per_share", "200.00", "2026-10-02");
    patch["expectedUpdatedAt"] = fresh["updatedAt"].clone();
    let (status, out) = send(
        &test.app,
        req(
            "PATCH",
            &format!("/api/periods/{id}"),
            &cookie,
            Some(&csrf),
            Some(patch),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{out}");
    let (status, out) = send(
        &test.app,
        req(
            "DELETE",
            &format!("/api/periods/{id}"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{out}");

    // Close the open period → locked; a second close → 409.
    let (status, out) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/periods/{id}/close"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["status"], "closed");
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/periods/{id}/close"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    // Obligations remain readable on the closed period.
    let (status, list) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/periods/{id}/assessments"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn draft_delete_removes_safe_record_only() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let (cookie, csrf) = admin_session(&test).await;

    let period = create_period(
        &test,
        &cookie,
        &csrf,
        period_body("per_shareholder", "10.00", "2026-10-01"),
    )
    .await;
    let id = period["id"].as_str().unwrap().to_string();

    let (status, _) = send(
        &test.app,
        req(
            "DELETE",
            &format!("/api/periods/{id}"),
            &cookie,
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, out) = send(
        &test.app,
        req("GET", &format!("/api/periods/{id}"), &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(out["error"]["code"], "not_found");
}

// ---------------------------------------------------------------------
// Concurrency: serialized finalization is single-winner
// ---------------------------------------------------------------------

#[tokio::test]
async fn concurrent_finalization_single_winner() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let (cookie, csrf) = admin_session(&test).await;
    api_create_shareholder(&test, &cookie, &csrf, "Tek", "Kazanan", None, 550).await;

    let period = create_period(
        &test,
        &cookie,
        &csrf,
        period_body("per_shareholder", "100.00", "2026-10-01"),
    )
    .await;
    let id = period["id"].as_str().unwrap().to_string();

    // Two concurrent finalize commands on separate pool connections:
    // exactly one wins, the loser gets 409 (§84/§85).
    let app = test.app.clone();
    let cookie2 = cookie.clone();
    let csrf2 = csrf.clone();
    let id2 = id.clone();
    let first = tokio::spawn(async move {
        send(
            &app,
            req(
                "POST",
                &format!("/api/periods/{id2}/generate-assessments"),
                &cookie2,
                Some(&csrf2),
                None,
            ),
        )
        .await
        .0
    });
    let second = generate(&test, &cookie, &csrf, &id).await.0;
    let first = first.await.unwrap();

    let outcomes = [first, second];
    assert!(
        outcomes.contains(&StatusCode::CREATED),
        "one finalize must win: {outcomes:?}"
    );
    assert!(
        outcomes.contains(&StatusCode::CONFLICT),
        "the loser must conflict: {outcomes:?}"
    );

    // Exactly one obligation set exists — no duplicates ever materialized.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM assessments")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}
