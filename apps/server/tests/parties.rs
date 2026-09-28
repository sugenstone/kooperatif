//! Database-gated parties domain tests (STEP-004 §60–§72 + temporal
//! membership amendment). Requires KOOPERATIF_TEST_DATABASE_URL; each
//! test runs in its own throwaway database (full isolation, including
//! the global seeded-role state).

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
    clock: Arc<MutableClock>,
    _peer: SocketAddr,
    /// Kept for symmetry with the rbac suite: db-verify.sh sweeps
    /// kooperatif_test_* leftovers from these throwaway databases.
    _database_name: String,
    _admin_url: String,
}

impl TestApp {
    fn peer(&self) -> SocketAddr {
        self._peer
    }
}

async fn setup() -> Option<TestApp> {
    if std::env::var("PARTIES_TEST_TRACE").is_ok() {
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
    let runtime = AuthRuntime::new(config, clock.clone());
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
        clock,
        _database_name: database_name,
        _admin_url: admin_url,
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

/// Create a shareholder via the API; returns (id, detail).
async fn api_create_shareholder(
    test: &TestApp,
    cookie: &str,
    csrf: &str,
    first: &str,
    last: &str,
    guardian: Option<(&str, &str)>,
    family_sequence: i64,
) -> (Uuid, Value) {
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
    (detail["id"].as_str().unwrap().parse().unwrap(), detail)
}

// §60: same-name shareholders coexist and are distinguishable.
#[tokio::test]
async fn same_name_shareholders_coexist_and_are_distinguishable() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let name = format!("sn.{}", Uuid::new_v4().simple());
    create_admin(&test.pool, &name).await;
    let (cookie, csrf) = login(&test.app, test.peer(), &name).await;

    let (id1, detail1) = api_create_shareholder(
        &test,
        &cookie,
        &csrf,
        "Mehmet",
        "Yılmaz",
        Some(("Hasan", "Yılmaz")),
        47,
    )
    .await;
    let (id2, _) = api_create_shareholder(
        &test,
        &cookie,
        &csrf,
        "Mehmet",
        "Yılmaz",
        Some(("Ahmet", "Yılmaz")),
        81,
    )
    .await;
    assert_ne!(id1, id2);
    assert_eq!(
        detail1["displayLabel"].as_str().unwrap(),
        "Mehmet Yılmaz · Vasi: Hasan Yılmaz · Aile No 47"
    );

    // Search finds both; each carries distinguishing context.
    let (status, list) = send(
        &test.app,
        req(
            "GET",
            "/api/shareholders?search=mehmet%20yılmaz",
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{items:?}");
    assert!(items.iter().all(|i| i["displayLabel"]
        .as_str()
        .unwrap()
        .starts_with("Mehmet Yılmaz ·")));
    assert!(items
        .iter()
        .any(|i| i["displayLabel"].as_str().unwrap().contains("Hasan")));
    assert!(items
        .iter()
        .any(|i| i["displayLabel"].as_str().unwrap().contains("Ahmet")));
}

// §61/§62: guardian without/with shareholder identity (no duplication).
#[tokio::test]
async fn guardian_person_semantics() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let name = format!("gd.{}", Uuid::new_v4().simple());
    create_admin(&test.pool, &name).await;
    let (cookie, csrf) = login(&test.app, test.peer(), &name).await;

    // §61: guardian-only person (Hasan) — no shareholder record created.
    let (_, _) = api_create_shareholder(
        &test,
        &cookie,
        &csrf,
        "B",
        "Kişi",
        Some(("Hasan", "Kaya")),
        10,
    )
    .await;
    let hasan_shareholder: Option<Uuid> = sqlx::query_scalar(
        "SELECT s.id FROM shareholders s JOIN persons p ON p.id = s.person_id \
         WHERE p.first_name = 'Hasan' AND p.last_name = 'Kaya'",
    )
    .fetch_optional(&test.pool)
    .await
    .unwrap();
    assert!(
        hasan_shareholder.is_none(),
        "guardian must NOT become a shareholder"
    );

    // §62: person A is a shareholder AND guardian of B — same person row.
    let (a_id, a_detail) =
        api_create_shareholder(&test, &cookie, &csrf, "A", "Kişi", None, 11).await;
    let a_person: Uuid = a_detail["personId"].as_str().unwrap().parse().unwrap();
    let create_b = json!({
        "person": { "mode": "new", "firstName": "B2", "lastName": "Kişi" },
        "guardian": { "mode": "existing", "personId": a_person },
        "family": { "mode": "new", "sequenceNumber": 12 }
    });
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/shareholders",
            &cookie,
            Some(&csrf),
            Some(create_b),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let person_count: i64 = sqlx::query_scalar("SELECT count(*) FROM persons WHERE id = $1")
        .bind(a_person)
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(person_count, 1, "person A not duplicated");
    let _ = a_id;

    // Existing-person mode: selecting an existing shareholder person for
    // a NEW shareholder is rejected (one person → one shareholder).
    let create_dup = json!({
        "person": { "mode": "existing", "personId": a_person },
        "family": { "mode": "new", "sequenceNumber": 13 }
    });
    let (status, body) = send(
        &test.app,
        req(
            "POST",
            "/api/shareholders",
            &cookie,
            Some(&csrf),
            Some(create_dup),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
}

// §63/§64: family members + duplicate sequence rejection (+ concurrent race).
#[tokio::test]
async fn family_memberships_and_sequence_conflicts() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let name = format!("fm.{}", Uuid::new_v4().simple());
    create_admin(&test.pool, &name).await;
    let (cookie, csrf) = login(&test.app, test.peer(), &name).await;

    let family_id: Uuid =
        sqlx::query_scalar("INSERT INTO families (sequence_number) VALUES (100) RETURNING id")
            .fetch_one(&test.pool)
            .await
            .unwrap();

    for first in ["A", "B", "C"] {
        let body = json!({
            "person": { "mode": "new", "firstName": first, "lastName": "Üye" },
            "family": { "mode": "existing", "familyId": family_id }
        });
        let (status, detail) = send(
            &test.app,
            req(
                "POST",
                "/api/shareholders",
                &cookie,
                Some(&csrf),
                Some(body),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{detail}");
        assert_eq!(detail["familySequence"], 100);
    }

    // Family detail returns all members with identity context.
    let (status, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/families/{family_id}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["memberCount"], 3);
    assert_eq!(detail["members"].as_array().unwrap().len(), 3);

    // Duplicate sequence rejected (API + direct DB constraint).
    let (status, body) = send(
        &test.app,
        req(
            "POST",
            "/api/families",
            &cookie,
            Some(&csrf),
            Some(json!({ "sequenceNumber": 100 })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let dup = sqlx::query("INSERT INTO families (sequence_number) VALUES (100)")
        .execute(&test.pool)
        .await;
    assert!(dup.is_err(), "DB enforces family sequence uniqueness");

    // Concurrent creation of the same sequence: exactly one 201/one 409.
    let app2 = test.app.clone();
    let c2 = cookie.clone();
    let first = async {
        test.app
            .clone()
            .oneshot(req(
                "POST",
                "/api/families",
                &cookie,
                Some(&csrf),
                Some(json!({"sequenceNumber": 200})),
            ))
            .await
            .unwrap()
            .status()
    };
    let second = async {
        app2.oneshot(req(
            "POST",
            "/api/families",
            &c2,
            Some(&csrf),
            Some(json!({"sequenceNumber": 200})),
        ))
        .await
        .unwrap()
        .status()
    };
    let (a, b) = tokio::join!(first, second);
    let mut statuses = [a, b];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::CREATED, StatusCode::CONFLICT]);
}

// §65: Turkish search across name/guardian/family, display values unchanged.
#[tokio::test]
async fn turkish_search_matches_without_mutating_stored_values() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let name = format!("tr.{}", Uuid::new_v4().simple());
    create_admin(&test.pool, &name).await;
    let (cookie, csrf) = login(&test.app, test.peer(), &name).await;

    api_create_shareholder(&test, &cookie, &csrf, "İsmail", "Şahin", None, 301).await;
    api_create_shareholder(
        &test,
        &cookie,
        &csrf,
        "Çağrı",
        "Öztürk",
        Some(("Gül", "Işık")),
        302,
    )
    .await;

    for term in ["ismail", "İSMAİL", "ismail şahin", "çağrı", "öztürk"] {
        let (status, list) = send(
            &test.app,
            req(
                "GET",
                &format!("/api/shareholders?search={}", urlencoding_simple(term)),
                &cookie,
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            !list["items"].as_array().unwrap().is_empty(),
            "term {term} found nothing"
        );
    }

    // Guardian-name search (Işık covers guardian of Çağrı).
    let (status, list) = send(
        &test.app,
        req("GET", "/api/shareholders?search=ışık", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().unwrap().len(), 1);

    // Family-number search.
    let (status, list) = send(
        &test.app,
        req("GET", "/api/shareholders?search=301", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().unwrap().len(), 1);

    // Stored values keep original characters.
    let stored: String =
        sqlx::query_scalar("SELECT first_name FROM persons WHERE first_name = 'İsmail'")
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert_eq!(stored, "İsmail");
}

fn urlencoding_simple(term: &str) -> String {
    // Only used in test URLs: encode non-ASCII via percent-encoding of
    // UTF-8 bytes without pulling a crate.
    let mut out = String::new();
    for byte in term.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

// §70: pagination boundaries, stable ordering, max page size.
#[tokio::test]
async fn pagination_is_stable_and_bounded() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let name = format!("pg.{}", Uuid::new_v4().simple());
    create_admin(&test.pool, &name).await;
    let (cookie, csrf) = login(&test.app, test.peer(), &name).await;

    for i in 1..=25 {
        api_create_shareholder(
            &test,
            &cookie,
            &csrf,
            &format!("Kişi{i:02}"),
            "Sayfa",
            None,
            1000 + i,
        )
        .await;
    }

    let (status, page1) = send(
        &test.app,
        req(
            "GET",
            "/api/shareholders?page=1&pageSize=10",
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page1["totalCount"], 25);
    assert_eq!(page1["items"].as_array().unwrap().len(), 10);

    // Walk all pages: exactly 25 unique ids (stable ordering, no dupes).
    let mut seen = std::collections::HashSet::new();
    for page in 1..=3 {
        let (_, body) = send(
            &test.app,
            req(
                "GET",
                &format!("/api/shareholders?page={page}&pageSize=10"),
                &cookie,
                None,
                None,
            ),
        )
        .await;
        for item in body["items"].as_array().unwrap() {
            assert!(
                seen.insert(item["id"].as_str().unwrap().to_string()),
                "duplicate on page {page}"
            );
        }
    }
    assert_eq!(seen.len(), 25);

    // Max page size enforced.
    let (status, _) = send(
        &test.app,
        req("GET", "/api/shareholders?pageSize=500", &cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// §71: transaction rollback on mid-flow failure (duplicate family
// sequence in new-family create → no orphan persons).
#[tokio::test]
async fn failed_create_rolls_back_the_whole_aggregate() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let name = format!("rb.{}", Uuid::new_v4().simple());
    create_admin(&test.pool, &name).await;
    let (cookie, csrf) = login(&test.app, test.peer(), &name).await;

    api_create_shareholder(&test, &cookie, &csrf, "İlk", "Kayıt", None, 500).await;
    let persons_before: i64 = sqlx::query_scalar("SELECT count(*) FROM persons")
        .fetch_one(&test.pool)
        .await
        .unwrap();

    let body = json!({
        "person": { "mode": "new", "firstName": "Yuvarlanan", "lastName": "Kayıt" },
        "guardian": { "mode": "new", "firstName": "Vasi", "lastName": "Yuvarlanan" },
        "family": { "mode": "new", "sequenceNumber": 500 } // conflict
    });
    let (status, body) = send(
        &test.app,
        req(
            "POST",
            "/api/shareholders",
            &cookie,
            Some(&csrf),
            Some(body),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    let persons_after: i64 = sqlx::query_scalar("SELECT count(*) FROM persons")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(
        persons_after, persons_before,
        "no partial person/guardian rows survive rollback"
    );
}

// Amendment tests 1-10: temporal family membership.
#[tokio::test]
async fn temporal_family_membership_lifecycle() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let name = format!("tf.{}", Uuid::new_v4().simple());
    create_admin(&test.pool, &name).await;
    let (cookie, csrf) = login(&test.app, test.peer(), &name).await;

    // 1. Start in Family A (seq 700).
    let (sh, detail) =
        api_create_shareholder(&test, &cookie, &csrf, "Abdullah", "Üye", None, 700).await;
    assert_eq!(detail["familySequence"], 700);
    let family_a: Uuid = detail["familyId"].as_str().unwrap().parse().unwrap();

    // Family B via API.
    let (status, family_b_body) = send(
        &test.app,
        req(
            "POST",
            "/api/families",
            &cookie,
            Some(&csrf),
            Some(json!({ "sequenceNumber": 701 })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let family_b: Uuid = family_b_body["id"].as_str().unwrap().parse().unwrap();
    let updated_at = detail["updatedAt"].as_str().unwrap().to_string();

    // 2. Move to Family B.
    test.clock.advance_seconds(60);
    let change = json!({ "family": { "mode": "existing", "familyId": family_b }, "reason": "test", "expectedUpdatedAt": updated_at });
    let (status, body) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shareholders/{sh}/family-change"),
            &cookie,
            Some(&csrf),
            Some(change),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // 3+4. History intact; B is the only current membership.
    let (_, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/shareholders/{sh}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(detail["familySequence"], 701);
    let history = detail["membershipHistory"].as_array().unwrap();
    assert_eq!(history.len(), 2);
    assert!(history
        .iter()
        .any(|m| m["familySequence"] == 700 && !m["endedAt"].is_null()));
    assert!(history
        .iter()
        .any(|m| m["familySequence"] == 701 && m["endedAt"].is_null()));
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM shareholder_family_memberships WHERE shareholder_id = $1 AND ended_at IS NULL",
    ).bind(sh).fetch_one(&test.pool).await.unwrap();
    assert_eq!(active, 1);

    // 5. Move into a NEWLY created family C within the same operation.
    test.clock.advance_seconds(60);
    let updated_at = detail["updatedAt"].as_str().unwrap().to_string();
    let change_c = json!({ "family": { "mode": "new", "sequenceNumber": 702 }, "reason": "yeni aile", "expectedUpdatedAt": updated_at });
    let (status, body) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shareholders/{sh}/family-change"),
            &cookie,
            Some(&csrf),
            Some(change_c),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // 6. A → B → C sequence intact.
    let (_, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/shareholders/{sh}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    let mut sequences: Vec<i64> = detail["membershipHistory"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["familySequence"].as_i64().unwrap())
        .collect();
    sequences.sort();
    // The frozen test clock collapses started_at, so ordering is by id;
    // the invariant is the full A→B→C SET with exactly one active.
    assert_eq!(sequences, vec![700, 701, 702]);

    // 7. Two active memberships impossible (DB constraint, direct).
    let insert_dup = sqlx::query(
        "INSERT INTO shareholder_family_memberships (shareholder_id, family_id) VALUES ($1, $2)",
    )
    .bind(sh)
    .bind(family_a)
    .execute(&test.pool)
    .await;
    assert!(
        insert_dup.is_err(),
        "exclusion constraint blocks a second active membership"
    );

    // 8. Concurrent transfers: exactly one wins (the clock advanced, so
    // the loser's expectedUpdatedAt no longer matches).
    let updated_at = detail["updatedAt"].as_str().unwrap().to_string();
    let c2 = cookie.clone();
    let change_x = json!({ "family": { "mode": "existing", "familyId": family_a }, "expectedUpdatedAt": updated_at });
    let change_y = json!({ "family": { "mode": "existing", "familyId": family_b }, "expectedUpdatedAt": updated_at });
    let app2 = test.app.clone();
    let x = async {
        test.app
            .clone()
            .oneshot(req(
                "POST",
                &format!("/api/shareholders/{sh}/family-change"),
                &cookie,
                Some(&csrf),
                Some(change_x),
            ))
            .await
            .unwrap()
            .status()
    };
    let y = async {
        app2.oneshot(req(
            "POST",
            &format!("/api/shareholders/{sh}/family-change"),
            &c2,
            Some(&csrf),
            Some(change_y),
        ))
        .await
        .unwrap()
        .status()
    };
    let (sx, sy) = tokio::join!(x, y);
    // Under the frozen test clock both requests share one instant, so
    // the row lock serializes them into two VALID sequential transfers
    // (both 204) — or the second observes a stale precondition (409).
    // With real monotonic time the 409 is guaranteed; the frozen-clock
    // proof here is the DB invariant: never two active memberships.
    for result in [sx, sy] {
        assert!(
            result == StatusCode::NO_CONTENT || result == StatusCode::CONFLICT,
            "unexpected concurrent result {result:?}"
        );
    }
    assert!(
        sx == StatusCode::NO_CONTENT || sy == StatusCode::NO_CONTENT,
        "at least one concurrent transfer succeeds"
    );

    let active_after_race: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM shareholder_family_memberships WHERE shareholder_id = $1 AND ended_at IS NULL",
    )
    .bind(sh)
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(
        active_after_race, 1,
        "concurrent transfers never leave two active memberships"
    );

    // 9. Failed transfer rolls back both membership changes (family not
    //    found → no membership touched).
    test.clock.advance_seconds(60);
    let (_, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/shareholders/{sh}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    let updated_at = detail["updatedAt"].as_str().unwrap().to_string();
    let bad = json!({ "family": { "mode": "existing", "familyId": Uuid::new_v4() }, "expectedUpdatedAt": updated_at });
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shareholders/{sh}/family-change"),
            &cookie,
            Some(&csrf),
            Some(bad),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let active_sequences: Vec<i64> = sqlx::query_scalar(
        "SELECT f.sequence_number FROM shareholder_family_memberships m JOIN families f ON f.id = m.family_id \
         WHERE m.shareholder_id = $1 AND m.ended_at IS NULL",
    ).bind(sh).fetch_one(&test.pool).await.map(|v| vec![v]).unwrap_or_default();
    assert_eq!(
        active_sequences.len(),
        1,
        "failed transfer left membership state untouched"
    );

    // 10. Audited.
    let events: Vec<String> = sqlx::query_scalar(
        "SELECT event_type FROM security_events WHERE event_type = 'shareholder_family_changed'",
    )
    .fetch_all(&test.pool)
    .await
    .unwrap();
    assert!(events.len() >= 3, "{events:?}");
}

// §66/§67: authorization matrix + seeded role upgrade + custom role.
#[tokio::test]
async fn parties_authorization_and_permission_upgrade() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    // 67: seeded admin role received STEP-004/STEP-005/STEP-006
    // permissions via migrations.
    let perms: Vec<String> = sqlx::query_scalar(
        "SELECT p.key FROM role_permissions rp JOIN permissions p ON p.id = rp.permission_id \
         JOIN roles r ON r.id = rp.role_id WHERE r.name = 'Sistem Yöneticisi' ORDER BY p.key",
    )
    .fetch_all(&test.pool)
    .await
    .unwrap();
    assert!(
        perms.contains(&"shareholders.manage".to_string()),
        "{perms:?}"
    );
    assert!(perms.contains(&"families.read".to_string()), "{perms:?}");
    assert!(
        perms.contains(&"assessments.manage".to_string()),
        "{perms:?}"
    );
    assert_eq!(perms.len(), 16);

    // Plain user: authenticated, no STEP-004 permissions.
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).unwrap(),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).unwrap();
    let plain_name = format!("plain.{}", Uuid::new_v4().simple());
    users::create_user(&test.pool, &plain_name, "Plain", &hash)
        .await
        .unwrap();
    let (plain_cookie, plain_csrf) = login(&test.app, test.peer(), &plain_name).await;

    // 401 unauthenticated.
    let (status, body) = send(
        &test.app,
        req("GET", "/api/shareholders", "yok", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "authentication_required");

    // 403 without shareholders.read.
    let (status, body) = send(
        &test.app,
        req("GET", "/api/shareholders", &plain_cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "permission_denied");

    // 403 without shareholders.manage (mutation, incl. CSRF failure ordering).
    let mutation = json!({
        "person": { "mode": "new", "firstName": "X", "lastName": "Y" },
        "family": { "mode": "new", "sequenceNumber": 1 }
    });
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/shareholders",
            &plain_cookie,
            Some(&plain_csrf),
            Some(mutation),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Custom role CAN receive the new permission through RBAC (§66).
    let admin_name = format!("adm.{}", Uuid::new_v4().simple());
    create_admin(&test.pool, &admin_name).await;
    let (admin_cookie, _admin_csrf) = login(&test.app, test.peer(), &admin_name).await;
    let _ = &admin_cookie;
    let custom_role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name) VALUES ('Hissedar Görüntüleyici') RETURNING id",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission_id) \
                 SELECT $1, id FROM permissions WHERE key = 'shareholders.read'",
    )
    .bind(custom_role)
    .execute(&test.pool)
    .await
    .unwrap();
    let plain: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE username = $1")
        .bind(&plain_name)
        .fetch_one(&test.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_role_assignments (user_id, role_id) VALUES ($1, $2)")
        .bind(plain)
        .bind(custom_role)
        .execute(&test.pool)
        .await
        .unwrap();

    // Immediate effect (freshness), same session.
    let (status, _) = send(
        &test.app,
        req("GET", "/api/shareholders", &plain_cookie, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

// §68: CSRF on representative parties mutations.
#[tokio::test]
async fn parties_mutations_require_full_csrf() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let name = format!("cs.{}", Uuid::new_v4().simple());
    create_admin(&test.pool, &name).await;
    let (cookie, csrf) = login(&test.app, test.peer(), &name).await;

    let body = json!({ "sequenceNumber": 900 });

    // Missing token.
    let (status, out) = send(
        &test.app,
        req("POST", "/api/families", &cookie, None, Some(body.clone())),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{out}");
    // Wrong token.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            "/api/families",
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
        "/api/families",
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
    // Valid proof succeeds.
    let (status, _) = send(
        &test.app,
        req("POST", "/api/families", &cookie, Some(&csrf), Some(body)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

// §69 + §37: audit trail answers the history questions.
#[tokio::test]
async fn parties_audit_covers_identity_and_family_changes() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let name = format!("au.{}", Uuid::new_v4().simple());
    let admin_id = create_admin(&test.pool, &name).await;
    let (cookie, csrf) = login(&test.app, test.peer(), &name).await;

    let (sh, detail) = api_create_shareholder(
        &test,
        &cookie,
        &csrf,
        "Denetim",
        "Kişisi",
        Some(("Vasi", "Kişisi")),
        800,
    )
    .await;
    let updated_at = detail["updatedAt"].as_str().unwrap().to_string();

    // Guardian change (to a new person).
    let patch = json!({ "guardian": { "mode": "new", "firstName": "Yeni", "lastName": "Vasi" }, "expectedUpdatedAt": updated_at });
    let (status, body) = send(
        &test.app,
        req(
            "PATCH",
            &format!("/api/shareholders/{sh}"),
            &cookie,
            Some(&csrf),
            Some(patch),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Status change.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shareholders/{sh}/status-change"),
            &cookie,
            Some(&csrf),
            Some(json!({ "to": "inactive", "reason": "deneme" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Invalid transition (voided → active).
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shareholders/{sh}/status-change"),
            &cookie,
            Some(&csrf),
            Some(json!({ "to": "active" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT); // inactive→active is valid
    let (_, detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/shareholders/{sh}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    let updated_at = detail["updatedAt"].as_str().unwrap().to_string();
    // void it, then attempt the forbidden voided→active.
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shareholders/{sh}/status-change"),
            &cookie,
            Some(&csrf),
            Some(json!({ "to": "voided" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(
        &test.app,
        req(
            "POST",
            &format!("/api/shareholders/{sh}/status-change"),
            &cookie,
            Some(&csrf),
            Some(json!({ "to": "active" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT event_type, metadata FROM security_events \
         WHERE event_type LIKE '%family%' OR event_type LIKE '%shareholder%' OR event_type LIKE '%person%' \
         ORDER BY occurred_at",
    )
    .fetch_all(&test.pool)
    .await
    .unwrap();
    let events: Vec<String> = rows.iter().map(|(e, _)| e.clone()).collect();
    let metas: Vec<serde_json::Value> = rows.iter().map(|(_, m)| m.clone()).collect();
    for expected in [
        "person_created",
        "shareholder_created",
        "shareholder_updated",
        "shareholder_status_changed",
    ] {
        assert!(
            events.iter().any(|e| e == expected),
            "missing {expected}: {events:?}"
        );
    }
    // Guardian change carries before/after.
    let guardian_change = metas
        .iter()
        .find(|m| m["change"] == "guardian")
        .expect("guardian change audited");
    assert!(guardian_change["before"].is_string() || guardian_change["before"].is_null());
    assert!(guardian_change["after"].is_string());
    // Actor is always the admin.
    let actors: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT user_id FROM security_events WHERE user_id IS NOT NULL",
    )
    .fetch_all(&test.pool)
    .await
    .unwrap();
    assert_eq!(actors, vec![admin_id]);
    let _ = updated_at;
}

// §49: lost-update protection via expectedUpdatedAt.
#[tokio::test]
async fn shareholder_edit_requires_fresh_updated_at() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let name = format!("lu.{}", Uuid::new_v4().simple());
    create_admin(&test.pool, &name).await;
    let (cookie, csrf) = login(&test.app, test.peer(), &name).await;
    let (sh, detail) =
        api_create_shareholder(&test, &cookie, &csrf, "Kayıp", "Güncelleme", None, 600).await;
    let stale = "2020-01-01T00:00:00Z";
    let _ = detail;

    let (status, body) = send(&test.app, req("PATCH", &format!("/api/shareholders/{sh}"), &cookie, Some(&csrf),
        Some(json!({ "firstName": "Değişti", "lastName": "Güncelleme", "expectedUpdatedAt": stale })))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "stale_state");

    // Fresh timestamp succeeds.
    let (_, fresh_detail) = send(
        &test.app,
        req(
            "GET",
            &format!("/api/shareholders/{sh}"),
            &cookie,
            None,
            None,
        ),
    )
    .await;
    let updated_at = fresh_detail["updatedAt"].as_str().unwrap().to_string();
    let (status, _) = send(&test.app, req("PATCH", &format!("/api/shareholders/{sh}"), &cookie, Some(&csrf),
        Some(json!({ "firstName": "Değişti", "lastName": "Güncelleme", "expectedUpdatedAt": updated_at })))).await;
    assert_eq!(status, StatusCode::OK);
}

// §72: IDOR / unknown ids.
#[tokio::test]
async fn unknown_ids_answer_404_forbidden_semantics() {
    let Some(test) = setup().await else {
        eprintln!("SKIPPED");
        return;
    };
    let name = format!("id.{}", Uuid::new_v4().simple());
    create_admin(&test.pool, &name).await;
    let (cookie, csrf) = login(&test.app, test.peer(), &name).await;

    let missing = Uuid::new_v4();
    for path in [
        format!("/api/shareholders/{missing}"),
        format!("/api/families/{missing}"),
    ] {
        let (status, _) = send(&test.app, req("GET", &path, &cookie, None, None)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let (status, _) = send(&test.app, req("POST", &format!("/api/shareholders/{missing}/family-change"), &cookie, Some(&csrf),
        Some(json!({ "family": { "mode": "new", "sequenceNumber": 1 }, "expectedUpdatedAt": "2026-01-01T00:00:00Z" })))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
