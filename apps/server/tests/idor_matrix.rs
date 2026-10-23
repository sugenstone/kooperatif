//! M1-P8-B Batch 1 — hostile cross-cooperative IDOR matrix over the
//! HTTP surface: foreign-resource reads, existence-oracle equivalence,
//! cross-tenant writes with zero-side-effect DB proof, and crafted
//! report/audit filters. Two real cooperatives (bootstrap A + a
//! platform-provisioned B); every fixture row is created through the
//! real repository layer so the denial can only be scope, never a
//! malformed fixture.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database;
//! scratch cooperatives only — no real second cooperative).
//!
//! Contract under test: resource lookups are pinned to the verified
//! cooperative context — a foreign id is indistinguishable from a
//! nonexistent one (same status, same body shape), mutations on a
//! foreign id write NOTHING (proven by row-level DB assertions), and
//! list/report endpoints ignore forged cooperative selectors.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use time::{Date, Month, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

use kooperatif_server::auth::{identity, users, AuthRuntime};
use kooperatif_server::clock::MutableClock;
use kooperatif_server::config::{AuthConfig, ARGON2_M_COST_FLOOR};
use kooperatif_server::db as app_db;
use kooperatif_server::financial_accounts::model::AccountType;
use kooperatif_server::financial_accounts::repo as account_repo;
use kooperatif_server::governance::repo as gov_repo;
use kooperatif_server::http::{cors_layer, router, AppState};
use kooperatif_server::income_expense::model::{CategoryType, EntryKind};
use kooperatif_server::income_expense::repo as entry_repo;
use kooperatif_server::investments::model::InvestmentType;
use kooperatif_server::investments::repo as inv_repo;
use kooperatif_server::parties::repo as party_repo;
use kooperatif_server::payments::repo as payment_repo;
use kooperatif_server::periods::model::AssessmentRuleType;
use kooperatif_server::periods::repo as period_repo;
use kooperatif_server::shares::model::AcquisitionType;
use kooperatif_server::shares::repo as share_repo;
use kooperatif_server::social_aid::repo as aid_repo;
use kooperatif_server::tenant::repo;

const TEST_PASSWORD: &str = "p8b-test-parola-123";
const ORIGIN: &str = "http://localhost:5173";
const APP_ROLE: &str = "kooperatif_app";

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
    app: axum::Router,
    pool: PgPool,
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

async fn create_user(pool: &PgPool, username: &str) -> Uuid {
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).expect("params"),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).expect("hash");
    users::create_user(pool, username, "P8B Test", &hash, TEST_PASSWORD)
        .await
        .expect("user")
        .id
}

async fn grant_platform(pool: &PgPool, user: Uuid, perms: &[&str]) {
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, cooperative_id) VALUES ($1, NULL) RETURNING id",
    )
    .bind(format!("plat-p8b-{user}"))
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

async fn grant_in_cooperative(pool: &PgPool, coop: Uuid, user: Uuid, perms: &[&str]) {
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id) \
         VALUES ($1, $2) ON CONFLICT (cooperative_id, user_id) \
         DO UPDATE SET status = 'active', ended_at = NULL",
    )
    .bind(coop)
    .bind(user)
    .execute(pool)
    .await
    .unwrap();
    let role: Uuid =
        sqlx::query_scalar("INSERT INTO roles (name, cooperative_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("coop-p8b-{user}-{coop}"))
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

/// Every business permission the matrix touches — user A is a FULL
/// cooperative-A administrator, so a denial can only ever come from the
/// scope boundary, never from a missing grant.
const ALL_BUSINESS_PERMS: &[&str] = &[
    "users.read",
    "users.manage",
    "memberships.read",
    "roles.read",
    "roles.manage",
    "shareholders.read",
    "shareholders.manage",
    "families.read",
    "families.manage",
    "shares.read",
    "shares.manage",
    "periods.read",
    "periods.manage",
    "assessments.read",
    "assessments.manage",
    "payments.read",
    "payments.manage",
    "financial_accounts.read",
    "financial_accounts.manage",
    "credits.read",
    "credits.manage",
    "income_expense.read",
    "income_expense.manage",
    "share_returns.read",
    "share_returns.manage",
    "investments.read",
    "investments.manage",
    "social_aid.read",
    "social_aid.manage",
    "governance.read",
    "governance.manage",
    "reports.read",
    "audit.read",
];

async fn bootstrap_coop(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("SELECT id FROM cooperatives WHERE is_bootstrap")
        .fetch_one(pool)
        .await
        .unwrap()
}

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

fn scoped_request(
    method: &str,
    path: &str,
    cookie: &str,
    csrf: Option<&str>,
    body: Option<Value>,
    cooperative_id: Uuid,
) -> Request<Body> {
    let mut request = request_builder(method, path, cookie, csrf, body);
    request.headers_mut().insert(
        "x-cooperative-id",
        cooperative_id.to_string().parse().expect("header value"),
    );
    request
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

async fn resolved_scope(app: &axum::Router, cookie: &str) -> Option<Uuid> {
    let (status, body) = send(
        app,
        request_builder("GET", "/api/auth/cooperative-context", cookie, None, None),
    )
    .await;
    if status != StatusCode::OK {
        return None;
    }
    body["cooperativeId"].as_str().and_then(|s| s.parse().ok())
}

/// Create + activate a second scratch cooperative; returns its id.
async fn second_cooperative(test: &TestApp, tag: &str) -> Uuid {
    let padmin = create_user(&test.pool, &format!("p8b-padmin-{tag}")).await;
    grant_platform(&test.pool, padmin, &["cooperatives.manage"]).await;
    let (pcookie, pcsrf, _) = login(&test.app, test.peer, &format!("p8b-padmin-{tag}")).await;
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/platform/cooperatives",
            &pcookie,
            Some(&pcsrf),
            Some(json!({ "name": format!("P8B Kooperatif {tag}") })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let coop_b: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    let admin_b = create_user(&test.pool, &format!("p8b-adminb-{tag}")).await;
    let (status, body) = send(
        &test.app,
        request_builder(
            "POST",
            &format!("/api/platform/cooperatives/{coop_b}/activate"),
            &pcookie,
            Some(&pcsrf),
            Some(json!({ "initialAdminUserId": admin_b })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    coop_b
}

fn date(y: i32, m: u8, d: u8) -> Date {
    Date::from_calendar_date(y, Month::try_from(m).unwrap(), d).unwrap()
}

/// One seeded domain aggregate in a single cooperative — every row is
/// produced by the real repository commands so it is indistinguishable
/// from API-created production data.
struct DomainSeed {
    shareholder_id: Uuid,
    person_id: Uuid,
    family_id: Uuid,
    share_id: Uuid,
    account_id: Uuid,
    period_id: Uuid,
    assessment_id: Uuid,
    payment_id: Uuid,
    income_id: Uuid,
    expense_id: Uuid,
    body_id: Uuid,
    fund_id: Uuid,
    investment_id: Uuid,
}

async fn seed_domain(
    pool: &PgPool,
    coop: Uuid,
    actor: Uuid,
    tag: &str,
    income_amount: i64,
) -> DomainSeed {
    let sh = party_repo::create_shareholder(
        pool,
        Some(coop),
        actor,
        party_repo::CreateShareholder {
            person_first_name: Some(format!("{tag}-Kişi")),
            person_last_name: Some("Soyad".to_string()),
            existing_person_id: None,
            guardian_person_id: None,
            guardian_first_name: None,
            guardian_last_name: None,
            family: party_repo::FamilyRef::New(11),
            default_collection_account_id: None,
        },
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("shareholder");
    let (share_id, _) = share_repo::create_share(
        pool,
        Some(coop),
        actor,
        share_repo::CreateShare {
            shareholder_id: sh.shareholder_id,
            acquisition_type: AcquisitionType::Founder,
            fee: None,
            effective_at: OffsetDateTime::now_utc(),
            reason: None,
        },
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("share");
    let account_id = account_repo::create_account(
        pool,
        Some(coop),
        actor,
        account_repo::CreateAccount {
            name: format!("{tag} Kasa"),
            account_type: AccountType::Cash,
            description: None,
            bank_name: None,
            iban: None,
        },
    )
    .await
    .expect("account");
    let (period_id, _) = period_repo::create_period(
        pool,
        Some(coop),
        actor,
        period_repo::CreatePeriod {
            name: format!("{tag} Dönem"),
            collection_start_date: date(2025, 1, 1),
            due_date: date(2025, 3, 31),
            rule_type: AssessmentRuleType::PerShareholder,
            base_amount: Decimal::from(1000),
            assessment_effective_date: date(2025, 1, 15),
        },
    )
    .await
    .expect("period");
    period_repo::generate_assessments(pool, Some(coop), actor, period_id)
        .await
        .expect("assessments");
    let assessment_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM assessments WHERE period_id = $1 AND cooperative_id = $2 LIMIT 1",
    )
    .bind(period_id)
    .bind(coop)
    .fetch_one(pool)
    .await
    .expect("assessment id");
    let payment = payment_repo::create_payment(
        pool,
        Some(coop),
        actor,
        payment_repo::CreatePayment {
            payer_person_id: Some(sh.person_id),
            payer_first_name: None,
            payer_last_name: None,
            amount: Decimal::from(250),
            method: "cash".to_string(),
            received_at: OffsetDateTime::now_utc(),
            note: None,
            destination_account_id: account_id,
            idempotency_key: format!("pay-{tag}"),
            fingerprint: format!("fp-pay-{tag}"),
            allocations: vec![],
        },
    )
    .await
    .expect("payment");
    let income_cat = entry_repo::create_category(
        pool,
        Some(coop),
        actor,
        entry_repo::CreateCategory {
            category_type: CategoryType::Income,
            name: format!("{tag} Gelir"),
            description: None,
        },
    )
    .await
    .expect("income category");
    let expense_cat = entry_repo::create_category(
        pool,
        Some(coop),
        actor,
        entry_repo::CreateCategory {
            category_type: CategoryType::Expense,
            name: format!("{tag} Gider"),
            description: None,
        },
    )
    .await
    .expect("expense category");
    let income = entry_repo::post_entry(
        pool,
        Some(coop),
        actor,
        EntryKind::Income,
        entry_repo::PostEntry {
            financial_account_id: account_id,
            category_id: income_cat,
            amount: Decimal::from(income_amount),
            occurred_at: OffsetDateTime::now_utc(),
            description: format!("{tag} gelir"),
            counterparty: None,
            reference_no: None,
            idempotency_key: format!("inc-{tag}"),
            fingerprint: format!("fp-inc-{tag}"),
        },
    )
    .await
    .expect("income");
    let expense = entry_repo::post_entry(
        pool,
        Some(coop),
        actor,
        EntryKind::Expense,
        entry_repo::PostEntry {
            financial_account_id: account_id,
            category_id: expense_cat,
            amount: Decimal::from(120),
            occurred_at: OffsetDateTime::now_utc(),
            description: format!("{tag} gider"),
            counterparty: None,
            reference_no: None,
            idempotency_key: format!("exp-{tag}"),
            fingerprint: format!("fp-exp-{tag}"),
        },
    )
    .await
    .expect("expense");
    let body = gov_repo::create_body(
        pool,
        Some(coop),
        actor,
        gov_repo::CreateBody {
            name: format!("{tag} Yönetim"),
            body_type: "Yönetim Kurulu".to_string(),
            description: None,
            idempotency_key: format!("body-{tag}"),
            fingerprint: format!("fp-body-{tag}"),
        },
    )
    .await
    .expect("body");
    let fund = aid_repo::create_fund(
        pool,
        Some(coop),
        actor,
        aid_repo::CreateFund {
            name: format!("{tag} Fon"),
            description: None,
            starts_on: None,
            ends_on: None,
            idempotency_key: format!("fund-{tag}"),
            fingerprint: format!("fp-fund-{tag}"),
        },
    )
    .await
    .expect("fund");
    let investment = inv_repo::create_investment(
        pool,
        Some(coop),
        actor,
        inv_repo::CreateInvestment {
            name: format!("{tag} Yatırım"),
            investment_type: InvestmentType::RealEstate,
            description: None,
            location: None,
            reference: None,
            counterparty_name: None,
            acquired_at: None,
            idempotency_key: format!("inv-{tag}"),
            fingerprint: format!("fp-inv-{tag}"),
        },
    )
    .await
    .expect("investment");
    DomainSeed {
        shareholder_id: sh.shareholder_id,
        person_id: sh.person_id,
        family_id: sh.family_id,
        share_id,
        account_id,
        period_id,
        assessment_id,
        payment_id: payment.payment_id,
        income_id: income.entry_id,
        expense_id: expense.entry_id,
        body_id: body.body_id,
        fund_id: fund.fund_id,
        investment_id: investment.investment_id,
    }
}

/// Two cooperatives, two full domain aggregates, one all-powerful
/// coop-A admin session. Returns everything the matrix needs.
struct Matrix {
    coop_a: Uuid,
    coop_b: Uuid,
    seed_a: DomainSeed,
    seed_b: DomainSeed,
    cookie_a: String,
    csrf_a: String,
}

async fn matrix(tag: &str) -> (TestApp, Matrix) {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;
    let coop_b = second_cooperative(&test, tag).await;
    let user_a = create_user(&test.pool, &format!("p8b-usera-{tag}")).await;
    let actor = user_a;
    let seed_a = seed_domain(&test.pool, coop_a, actor, &format!("{tag}a"), 500).await;
    let seed_b = seed_domain(&test.pool, coop_b, actor, &format!("{tag}b"), 90_000).await;
    grant_in_cooperative(&test.pool, coop_a, user_a, ALL_BUSINESS_PERMS).await;
    let (cookie_a, csrf_a, _) = login(&test.app, test.peer, &format!("p8b-usera-{tag}")).await;
    // Positive control: the session resolves to cooperative A.
    assert_eq!(resolved_scope(&test.app, &cookie_a).await, Some(coop_a));
    (
        test,
        Matrix {
            coop_a,
            coop_b,
            seed_a,
            seed_b,
            cookie_a,
            csrf_a,
        },
    )
}

/// Row-level counter for zero-unauthorized-write proofs.
async fn count(pool: &PgPool, sql: &str, id: Uuid) -> i64 {
    sqlx::query_scalar(sql)
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

// ═════════════════════════════════════════════════════════════════════
// A. Foreign-resource reads — indistinguishable from nonexistent.
// ═════════════════════════════════════════════════════════════════════

/// Positive control first: the same admin reads coop A's own family.
/// Then B's family id AND a random uuid must both return the identical
/// denial shape — no existence oracle.
#[tokio::test]
async fn family_detail_foreign_id_is_indistinguishable_from_missing() {
    let (test, m) = matrix("fam").await;
    let (status, _) = send(
        &test.app,
        scoped_request(
            "GET",
            &format!("/api/families/{}", m.seed_a.family_id),
            &m.cookie_a,
            None,
            None,
            m.coop_a,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "own family readable");

    let (foreign_status, foreign_body) = send(
        &test.app,
        scoped_request(
            "GET",
            &format!("/api/families/{}", m.seed_b.family_id),
            &m.cookie_a,
            None,
            None,
            m.coop_a,
        ),
    )
    .await;
    let (missing_status, missing_body) = send(
        &test.app,
        scoped_request(
            "GET",
            &format!("/api/families/{}", Uuid::new_v4()),
            &m.cookie_a,
            None,
            None,
            m.coop_a,
        ),
    )
    .await;
    assert_eq!(
        foreign_status, missing_status,
        "status oracle must not differ"
    );
    assert_eq!(foreign_body, missing_body, "body oracle must not differ");
    assert_ne!(foreign_status, StatusCode::OK);
    test.cleanup().await;
}

/// Share detail + status-change: read is 404, mutation is rejected and
/// the B share keeps its exact status — zero unauthorized writes.
#[tokio::test]
async fn share_detail_and_mutation_on_foreign_id_leave_zero_writes() {
    let (test, m) = matrix("share").await;
    let before: String = sqlx::query_scalar("SELECT status FROM shares WHERE id = $1")
        .bind(m.seed_b.share_id)
        .fetch_one(&test.pool)
        .await
        .unwrap();

    let (status, _) = send(
        &test.app,
        scoped_request(
            "GET",
            &format!("/api/shares/{}", m.seed_b.share_id),
            &m.cookie_a,
            None,
            None,
            m.coop_a,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(
        &test.app,
        scoped_request(
            "POST",
            &format!("/api/shares/{}/status-change", m.seed_b.share_id),
            &m.cookie_a,
            Some(&m.csrf_a),
            Some(json!({ "to": "cancelled", "reason": "idor attempt" })),
            m.coop_a,
        ),
    )
    .await;
    assert!(
        !status.is_success(),
        "foreign share mutation must be rejected"
    );
    let after: String = sqlx::query_scalar("SELECT status FROM shares WHERE id = $1")
        .bind(m.seed_b.share_id)
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(before, after, "foreign share status mutated");

    // Indirect shareholder lookups are pinned too — the documented
    // contract differs per endpoint: the shares list is a scoped
    // collection query (200 with ZERO items for a foreign holder), the
    // credit ledger checks holder existence in-scope first (404). Both
    // leak nothing — assert each contract's empty shape explicitly.
    let (status, body) = send(
        &test.app,
        scoped_request(
            "GET",
            &format!("/api/shareholders/{}/shares", m.seed_b.shareholder_id),
            &m.cookie_a,
            None,
            None,
            m.coop_a,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!([]),
        "foreign shareholder's shares leaked: {body}"
    );
    let (status, _) = send(
        &test.app,
        scoped_request(
            "GET",
            &format!("/api/shareholders/{}/credits", m.seed_b.shareholder_id),
            &m.cookie_a,
            None,
            None,
            m.coop_a,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    test.cleanup().await;
}

/// Period close on a foreign period is rejected and B's period stays
/// open; foreign assessment detail and its payments list are 404.
#[tokio::test]
async fn period_and_assessment_foreign_ids_are_untouchable() {
    let (test, m) = matrix("period").await;
    let (status, _) = send(
        &test.app,
        scoped_request(
            "GET",
            &format!("/api/periods/{}/assessments", m.seed_b.period_id),
            &m.cookie_a,
            None,
            None,
            m.coop_a,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(
        &test.app,
        scoped_request(
            "POST",
            &format!("/api/periods/{}/close", m.seed_b.period_id),
            &m.cookie_a,
            Some(&m.csrf_a),
            None,
            m.coop_a,
        ),
    )
    .await;
    assert!(
        !status.is_success(),
        "foreign period close must be rejected"
    );
    let open: Option<String> = sqlx::query_scalar("SELECT status FROM periods WHERE id = $1")
        .bind(m.seed_b.period_id)
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(open.as_deref(), Some("open"), "foreign period was closed");

    let (status, _) = send(
        &test.app,
        scoped_request(
            "GET",
            &format!("/api/assessments/{}", m.seed_b.assessment_id),
            &m.cookie_a,
            None,
            None,
            m.coop_a,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &test.app,
        scoped_request(
            "GET",
            &format!("/api/assessments/{}/payments", m.seed_b.assessment_id),
            &m.cookie_a,
            None,
            None,
            m.coop_a,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    test.cleanup().await;
}

/// Payment: foreign detail is 404, allocations and reversal are
/// rejected, and B's payment keeps zero allocations and its status.
#[tokio::test]
async fn payment_foreign_id_rejects_all_mutations_with_zero_writes() {
    let (test, m) = matrix("pay").await;
    let pay_status_before: String = sqlx::query_scalar("SELECT status FROM payments WHERE id = $1")
        .bind(m.seed_b.payment_id)
        .fetch_one(&test.pool)
        .await
        .unwrap();
    let allocs_before = count(
        &test.pool,
        "SELECT count(*) FROM payment_allocations WHERE payment_id = $1",
        m.seed_b.payment_id,
    )
    .await;

    let (status, _) = send(
        &test.app,
        scoped_request(
            "GET",
            &format!("/api/payments/{}", m.seed_b.payment_id),
            &m.cookie_a,
            None,
            None,
            m.coop_a,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(
        &test.app,
        scoped_request(
            "POST",
            &format!("/api/payments/{}/allocations", m.seed_b.payment_id),
            &m.cookie_a,
            Some(&m.csrf_a),
            Some(json!({ "allocations": [
                { "assessmentId": m.seed_b.assessment_id, "amount": "10.00" }
            ] })),
            m.coop_a,
        ),
    )
    .await;
    assert!(
        !status.is_success(),
        "foreign payment allocation must be rejected"
    );

    let (status, _) = send(
        &test.app,
        scoped_request(
            "POST",
            &format!("/api/payments/{}/reverse", m.seed_b.payment_id),
            &m.cookie_a,
            Some(&m.csrf_a),
            Some(json!({ "reason": "idor reversal" })),
            m.coop_a,
        ),
    )
    .await;
    assert!(
        !status.is_success(),
        "foreign payment reversal must be rejected"
    );

    let allocs_after = count(
        &test.pool,
        "SELECT count(*) FROM payment_allocations WHERE payment_id = $1",
        m.seed_b.payment_id,
    )
    .await;
    assert_eq!(
        allocs_before, allocs_after,
        "foreign payment gained allocations"
    );
    let pay_status_after: String = sqlx::query_scalar("SELECT status FROM payments WHERE id = $1")
        .bind(m.seed_b.payment_id)
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(
        pay_status_before, pay_status_after,
        "foreign payment status changed"
    );
    test.cleanup().await;
}

/// Income/expense: foreign detail is 404, reversal writes nothing —
/// no reversal row, no account movement delta.
#[tokio::test]
async fn income_expense_foreign_reversal_leaves_no_trace() {
    let (test, m) = matrix("incexp").await;
    let movements_before = count(
        &test.pool,
        "SELECT count(*) FROM account_movements WHERE cooperative_id = $1",
        m.coop_b,
    )
    .await;

    let (status, _) = send(
        &test.app,
        scoped_request(
            "GET",
            &format!("/api/incomes/{}", m.seed_b.income_id),
            &m.cookie_a,
            None,
            None,
            m.coop_a,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &test.app,
        scoped_request(
            "GET",
            &format!("/api/expenses/{}", m.seed_b.expense_id),
            &m.cookie_a,
            None,
            None,
            m.coop_a,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(
        &test.app,
        scoped_request(
            "POST",
            &format!("/api/incomes/{}/reverse", m.seed_b.income_id),
            &m.cookie_a,
            Some(&m.csrf_a),
            Some(json!({ "reason": "idor reversal" })),
            m.coop_a,
        ),
    )
    .await;
    assert!(
        !status.is_success(),
        "foreign income reversal must be rejected"
    );

    let entry_status: String =
        sqlx::query_scalar("SELECT status FROM income_entries WHERE id = $1")
            .bind(m.seed_b.income_id)
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert_eq!(entry_status, "posted", "foreign income entry state changed");
    let movements_after = count(
        &test.pool,
        "SELECT count(*) FROM account_movements WHERE cooperative_id = $1",
        m.coop_b,
    )
    .await;
    assert_eq!(
        movements_before, movements_after,
        "cross-cooperative reversal produced account movements"
    );
    test.cleanup().await;
}

/// Governance body: foreign detail 404, close rejected, body stays
/// active — social-aid fund and investment same treatment.
#[tokio::test]
async fn governance_investment_fund_foreign_mutations_are_inert() {
    let (test, m) = matrix("gov").await;
    for (path, body) in [
        (
            format!("/api/governance/bodies/{}/close", m.seed_b.body_id),
            json!(null),
        ),
        (
            format!("/api/investments/{}/cancel", m.seed_b.investment_id),
            json!({ "reason": "idor" }),
        ),
        (
            format!("/api/social-aid/funds/{}/cancel", m.seed_b.fund_id),
            json!({ "reason": "idor" }),
        ),
    ] {
        let (status, _) = send(
            &test.app,
            scoped_request(
                "POST",
                &path,
                &m.cookie_a,
                Some(&m.csrf_a),
                Some(body),
                m.coop_a,
            ),
        )
        .await;
        assert!(!status.is_success(), "foreign mutation accepted on {path}");
    }
    for (table, id) in [
        ("governance_bodies", m.seed_b.body_id),
        ("investments", m.seed_b.investment_id),
        ("social_aid_funds", m.seed_b.fund_id),
    ] {
        let status: String =
            sqlx::query_scalar(&format!("SELECT status FROM {table} WHERE id = $1"))
                .bind(id)
                .fetch_one(&test.pool)
                .await
                .unwrap();
        assert!(
            !matches!(status.as_str(), "closed" | "cancelled" | "canceled"),
            "foreign {table} row reached terminal status: {status}"
        );
    }
    let (status, _) = send(
        &test.app,
        scoped_request(
            "GET",
            &format!("/api/governance/bodies/{}", m.seed_b.body_id),
            &m.cookie_a,
            None,
            None,
            m.coop_a,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &test.app,
        scoped_request(
            "GET",
            &format!("/api/investments/{}", m.seed_b.investment_id),
            &m.cookie_a,
            None,
            None,
            m.coop_a,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &test.app,
        scoped_request(
            "GET",
            &format!("/api/social-aid/funds/{}", m.seed_b.fund_id),
            &m.cookie_a,
            None,
            None,
            m.coop_a,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    test.cleanup().await;
}

// ═════════════════════════════════════════════════════════════════════
// B. Enumeration oracle — foreign vs nonexistent are identical.
// ═════════════════════════════════════════════════════════════════════

/// Across the resource-detail surface the (status, body) pair for a
/// foreign id equals that of a random uuid — the endpoint leaks no
/// existence signal. Statuses are asserted equal, not hardcoded, so a
/// future contract change updates in one place.
#[tokio::test]
async fn detail_endpoints_leak_no_existence_oracle() {
    let (test, m) = matrix("oracle").await;
    let foreign_ids = [
        format!("/api/families/{}", m.seed_b.family_id),
        format!("/api/shares/{}", m.seed_b.share_id),
        format!("/api/payments/{}", m.seed_b.payment_id),
        format!("/api/assessments/{}", m.seed_b.assessment_id),
        format!("/api/incomes/{}", m.seed_b.income_id),
        format!("/api/expenses/{}", m.seed_b.expense_id),
        format!("/api/investments/{}", m.seed_b.investment_id),
        format!("/api/governance/bodies/{}", m.seed_b.body_id),
        format!("/api/social-aid/funds/{}", m.seed_b.fund_id),
        format!("/api/share-returns/{}", Uuid::new_v4()), // no B seed — still uniform
    ];
    for path in foreign_ids {
        let foreign_path = path.clone();
        let missing_path = path
            .rsplit_once('/')
            .map(|(base, _)| format!("{base}/{}", Uuid::new_v4()))
            .unwrap();
        let (fs, fb) = send(
            &test.app,
            scoped_request("GET", &foreign_path, &m.cookie_a, None, None, m.coop_a),
        )
        .await;
        let (ms, mb) = send(
            &test.app,
            scoped_request("GET", &missing_path, &m.cookie_a, None, None, m.coop_a),
        )
        .await;
        assert_eq!(fs, ms, "status oracle differs on {foreign_path}");
        assert_eq!(fb, mb, "body oracle differs on {foreign_path}");
        assert!(
            !fs.is_success(),
            "foreign id unexpectedly readable on {foreign_path}"
        );
    }
    test.cleanup().await;
}

// ═════════════════════════════════════════════════════════════════════
// C. Cross-cooperative writes — rejected, zero side effects in the DB.
// ═════════════════════════════════════════════════════════════════════

/// A forged `cooperativeId` inside a POST body can never pin the scope:
/// the payment lands in the VERIFIED context (A) and nowhere else —
/// verified at the row level.
#[tokio::test]
async fn forged_body_scope_field_cannot_redirect_a_write() {
    let (test, m) = matrix("forge").await;
    let b_payments_before = count(
        &test.pool,
        "SELECT count(*) FROM payments WHERE cooperative_id = $1",
        m.coop_b,
    )
    .await;

    let (status, body) = send(
        &test.app,
        scoped_request(
            "POST",
            "/api/payments",
            &m.cookie_a,
            Some(&m.csrf_a),
            Some(json!({
                "cooperativeId": m.coop_b,
                "payerPersonId": m.seed_b.person_id,
                "amount": "10.00",
                "method": "cash",
                "receivedAt": "2026-01-02T10:00:00Z",
                "destinationAccountId": m.seed_a.account_id,
                "idempotencyKey": "idor-forge-1",
                "allocations": []
            })),
            m.coop_a,
        ),
    )
    .await;

    // Whatever the outcome, cooperative B must gain NOTHING: if the
    // request was honored it ran inside scope A (foreign payer id is
    // then a NotFound — also acceptable), never inside scope B.
    let b_payments_after = count(
        &test.pool,
        "SELECT count(*) FROM payments WHERE cooperative_id = $1",
        m.coop_b,
    )
    .await;
    assert_eq!(
        b_payments_before, b_payments_after,
        "forged body scope wrote into cooperative B (status {status}, body {body})"
    );
    if status.is_success() {
        let new_id: Uuid = body["id"].as_str().expect("payment id").parse().unwrap();
        let coop: Option<Uuid> =
            sqlx::query_scalar("SELECT cooperative_id FROM payments WHERE id = $1")
                .bind(new_id)
                .fetch_one(&test.pool)
                .await
                .unwrap();
        assert_eq!(coop, Some(m.coop_a), "write escaped the verified scope");
    }
    test.cleanup().await;
}

/// No unauthorized cooperative-context switch: a burst of foreign-id
/// probes must leave the session's resolved scope exactly at A.
#[tokio::test]
async fn foreign_probes_never_mutate_session_scope() {
    let (test, m) = matrix("scope").await;
    for path in [
        format!("/api/payments/{}", m.seed_b.payment_id),
        format!("/api/families/{}", m.seed_b.family_id),
        format!("/api/shares/{}", m.seed_b.share_id),
    ] {
        let _ = send(
            &test.app,
            scoped_request("GET", &path, &m.cookie_a, None, None, m.coop_a),
        )
        .await;
    }
    // Forged switch attempt: B's id is not in user A's membership set.
    let (status, _) = send(
        &test.app,
        request_builder(
            "POST",
            "/api/auth/cooperative",
            &m.cookie_a,
            Some(&m.csrf_a),
            Some(json!({ "cooperativeId": m.coop_b })),
        ),
    )
    .await;
    assert!(
        !status.is_success(),
        "switch to non-member cooperative accepted"
    );
    assert_eq!(
        resolved_scope(&test.app, &m.cookie_a).await,
        Some(m.coop_a),
        "session scope drifted after hostile probes"
    );
    test.cleanup().await;
}

// ═════════════════════════════════════════════════════════════════════
// D. Crafted report/audit filters — forged selectors stay pinned to A.
// ═════════════════════════════════════════════════════════════════════

/// Reports: a forged `cooperative_id` query selector cannot pivot the
/// snapshot into B — A's known income (500.00) is returned, never B's
/// (90000.00). Positive control: plain report reflects A exactly.
#[tokio::test]
async fn report_snapshots_ignore_forged_cooperative_selectors() {
    let (test, m) = matrix("reports").await;
    for path in [
        "/api/reports/overview".to_string(),
        format!("/api/reports/overview?cooperative_id={}", m.coop_b),
        format!("/api/reports/overview?cooperativeId={}", m.coop_b),
        format!("/api/reports/movements?cooperative_id={}", m.coop_b),
        format!("/api/reports/payments?cooperative_id={}", m.coop_b),
    ] {
        let (status, body) = send(
            &test.app,
            scoped_request("GET", &path, &m.cookie_a, None, None, m.coop_a),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{path}");
        let text = body.to_string();
        assert!(
            !text.contains("90000"),
            "cooperative B financials leaked through {path}: {text}"
        );
    }
    // A's overview must carry A's real numbers — denial-by-empty would
    // also hide a leak, so prove positive content.
    let (_, body) = send(
        &test.app,
        scoped_request(
            "GET",
            "/api/reports/overview",
            &m.cookie_a,
            None,
            None,
            m.coop_a,
        ),
    )
    .await;
    assert_eq!(
        body["operationalIncomeTotal"].as_str().unwrap_or_default(),
        "500.00",
        "cooperative A income missing from overview: {body}"
    );
    test.cleanup().await;
}

/// Audit read path: the scoped events endpoint stays pinned to A even
/// with a forged cooperative selector — and never returns B-attributed
/// rows.
#[tokio::test]
async fn audit_events_stay_pinned_under_forged_selectors() {
    let (test, m) = matrix("audit").await;
    let (status, body) = send(
        &test.app,
        scoped_request(
            "GET",
            &format!("/api/audit/events?cooperative_id={}", m.coop_b),
            &m.cookie_a,
            None,
            None,
            m.coop_a,
        ),
    )
    .await;
    assert!(
        status.is_success() || status == StatusCode::FORBIDDEN,
        "unexpected audit status {status}"
    );
    if status.is_success() {
        assert!(
            !body.to_string().contains(&m.coop_b.to_string()),
            "audit response carries cooperative B attribution: {body}"
        );
    }
    test.cleanup().await;
}
