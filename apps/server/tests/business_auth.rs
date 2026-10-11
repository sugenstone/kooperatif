//! M1-P1-c1 — verified business authorization context tests.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same harness as tests/rbac.rs). These tests prove `BusinessAuth`
//! extraction and `require_business` semantics on a dedicated probe
//! router — legacy business endpoints are intentionally NOT touched in
//! P1-c1 (they migrate in P1-c3):
//!   - explicit header: must name the bootstrap cooperative exactly —
//!     malformed → 400, non-bootstrap/unknown → 403, never a silent
//!     fallback;
//!   - headerless: pinned to bootstrap, session default ignored;
//!   - bootstrap missing: legacy lane opens only when the installation
//!     provably never bootstrapped (zero cooperatives AND zero
//!     coop-attributed audit events); residue → fail closed;
//!   - NULL-cooperative rows are bootstrap-attributable ONLY (verified
//!     legacy attribution), never in another cooperative.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

use kooperatif_server::auth::extractor::CurrentAuth;
use kooperatif_server::auth::{authz, identity, session, token, users, AuthRuntime};
use kooperatif_server::clock::MutableClock;
use kooperatif_server::config::{AuthConfig, ARGON2_M_COST_FLOOR};
use kooperatif_server::db as app_db;
use kooperatif_server::http::error::ApiError;
use kooperatif_server::http::AppState;
use kooperatif_server::tenant::{repo, BusinessAuth};

const TEST_PASSWORD: &str = "p1c1-test-parola-123";

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

// Probe handlers — what a migrated legacy handler will look like in
// P1-c3 (extractor + require_business), without touching real routes.
async fn probe_scope(ctx: BusinessAuth) -> String {
    match ctx.cooperative_id() {
        Some(id) => id.to_string(),
        None => "legacy".to_string(),
    }
}

async fn probe_perm(
    State(state): State<AppState>,
    ctx: BusinessAuth,
) -> Result<&'static str, ApiError> {
    authz::require_business(&state, &ctx, "payments.read").await?;
    Ok("granted")
}

struct TestApp {
    _claim: app_db::TestDbClaim,
    app: Router,
    pool: PgPool,
    state: AppState,
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

// Bounds concurrent provisioning against postgres max_connections (see
// reservation.rs, P0-002).
static SETUP_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

async fn setup() -> Option<TestApp> {
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
    let state = AppState {
        db: Some(pool.clone()),
        auth: Arc::new(AuthRuntime::new(
            AuthConfig {
                allowed_origins: vec!["http://localhost:5173".to_string()],
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
    };
    let app = Router::new()
        .route("/probe", get(probe_scope))
        .route("/probe-perm", get(probe_perm))
        .with_state(state.clone());
    let bytes = Uuid::new_v4();
    let octets = bytes.as_bytes();
    Some(TestApp {
        _claim: claim,
        app,
        pool,
        state,
        peer: format!(
            "127.{}.{}.{}:{}",
            octets[0],
            octets[1],
            octets[2],
            50000 + (octets[3] as u16)
        )
        .parse()
        .expect("derived addr"),
        database_name,
        admin_url,
    })
}

async fn create_user(pool: &PgPool, username: &str) -> Uuid {
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).expect("test parameters"),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).expect("hash");
    users::create_user(pool, username, "P1c1 Test", &hash)
        .await
        .expect("user created")
        .id
}

/// Create a live session and return the raw cookie token.
async fn session_for(pool: &PgPool, user_id: Uuid) -> (String, Uuid) {
    let generated = token::generate_token();
    let row = session::create_session(
        pool,
        session::NewSession {
            user_id,
            token_hash: generated.hash,
            csrf_token: "csrf",
            absolute_ttl_secs: 3600,
            idle_ttl_secs: 600,
            client_label: None,
            now: OffsetDateTime::now_utc(),
        },
    )
    .await
    .expect("session created");
    (generated.raw, row.id)
}

async fn request_get(
    app: &Router,
    peer: SocketAddr,
    path: &str,
    cookie: &str,
    coop_header: Option<&str>,
) -> (StatusCode, String) {
    let mut builder = Request::get(path)
        .header("cookie", format!("kooperatif_session={cookie}"))
        .header("origin", "http://localhost:5173");
    if let Some(coop) = coop_header {
        builder = builder.header("x-cooperative-id", coop);
    }
    let mut request = builder.body(Body::empty()).expect("request");
    request.extensions_mut().insert(ConnectInfo(peer));
    let response = app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn grant_perm(pool: &PgPool, user: Uuid, key: &str, scoped_to: Option<Uuid>) {
    let role: Uuid =
        sqlx::query_scalar("INSERT INTO roles (name, cooperative_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("rol-{user}-{key}"))
            .bind(scoped_to)
            .fetch_one(pool)
            .await
            .expect("role");
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission_id) \
         SELECT $1, id FROM permissions WHERE key = $2",
    )
    .bind(role)
    .bind(key)
    .execute(pool)
    .await
    .expect("perm");
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
         VALUES ($1, $2, $3)",
    )
    .bind(user)
    .bind(role)
    .bind(scoped_to)
    .execute(pool)
    .await
    .expect("assignment");
}

async fn bootstrap(pool: &PgPool) -> Uuid {
    repo::bootstrap_initial_cooperative(pool, "Ana Kooperatif")
        .await
        .expect("bootstrap")
        .0
}

#[tokio::test]
async fn explicit_bootstrap_header_resolves_bootstrap_scope() {
    let Some(test) = setup().await else { return };
    let user = create_user(&test.pool, "acik_bootstrap").await;
    let coop = bootstrap(&test.pool).await;
    let (cookie, _) = session_for(&test.pool, user).await;
    let coop_a_str = coop.to_string();

    let (status, body) =
        request_get(&test.app, test.peer, "/probe", &cookie, Some(&coop_a_str)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, coop.to_string());

    test.cleanup().await;
}

#[tokio::test]
async fn headerless_pins_bootstrap_and_ignores_session_default() {
    let Some(test) = setup().await else { return };
    let user = create_user(&test.pool, "headersiz").await;
    let coop_a = bootstrap(&test.pool).await;
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;
    repo::add_member(&test.pool, "B Kooperatifi", user, None)
        .await
        .expect("B membership");
    let (cookie, session_id) = session_for(&test.pool, user).await;
    // Session default points at B — legacy endpoints must ignore it.
    repo::set_session_cooperative(&test.pool, session_id, Some(coop_b))
        .await
        .expect("session default");

    let (status, body) = request_get(&test.app, test.peer, "/probe", &cookie, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, coop_a.to_string(), "session default must not win");

    test.cleanup().await;
}

#[tokio::test]
async fn explicit_non_bootstrap_and_bad_headers_are_rejected() {
    let Some(test) = setup().await else { return };
    let user = create_user(&test.pool, "red_edilen").await;
    bootstrap(&test.pool).await;
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;
    repo::add_member(&test.pool, "B Kooperatifi", user, None)
        .await
        .expect("B membership");
    let (cookie, _) = session_for(&test.pool, user).await;
    let coop_b_str = coop_b.to_string();

    // Member of both — explicit B still rejected, never silently
    // re-pinned to bootstrap.
    let (status, _) = request_get(&test.app, test.peer, "/probe", &cookie, Some(&coop_b_str)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "non-bootstrap must deny");
    // Unknown UUID → 403 (existence is never leaked).
    let unknown = Uuid::new_v4().to_string();
    let (status, _) = request_get(&test.app, test.peer, "/probe", &cookie, Some(&unknown)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Malformed → 400.
    let (status, _) =
        request_get(&test.app, test.peer, "/probe", &cookie, Some("not-a-uuid")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Explicit invalid header + otherwise valid bootstrap membership →
    // still denied (no silent bootstrap fallback).
    assert_eq!(
        request_get(&test.app, test.peer, "/probe", &cookie, Some("garbage"))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );

    test.cleanup().await;
}

#[tokio::test]
async fn membership_state_gates_the_bootstrap_scope() {
    let Some(test) = setup().await else { return };
    let user = create_user(&test.pool, "uye_durum").await;
    let coop = bootstrap(&test.pool).await;
    let (cookie, _) = session_for(&test.pool, user).await;

    // Suspended → 403.
    sqlx::query("UPDATE cooperative_memberships SET status='suspended' WHERE user_id=$1")
        .bind(user)
        .execute(&test.pool)
        .await
        .expect("suspend");
    assert_eq!(
        request_get(&test.app, test.peer, "/probe", &cookie, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    // Ended → 403.
    sqlx::query("UPDATE cooperative_memberships SET status='ended' WHERE user_id=$1")
        .bind(user)
        .execute(&test.pool)
        .await
        .expect("end");
    assert_eq!(
        request_get(&test.app, test.peer, "/probe", &cookie, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    // Zero membership (user created post-bootstrap) → 403.
    let nobody = create_user(&test.pool, "uyesiz_2").await;
    let (cookie2, _) = session_for(&test.pool, nobody).await;
    assert_eq!(
        request_get(&test.app, test.peer, "/probe", &cookie2, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    // Disabled bootstrap (business_enabled off) → 403 not_ready.
    sqlx::query("UPDATE cooperatives SET business_enabled=false WHERE id=$1")
        .bind(coop)
        .execute(&test.pool)
        .await
        .expect("disable business");
    sqlx::query("UPDATE cooperative_memberships SET status='active' WHERE user_id=$1")
        .bind(user)
        .execute(&test.pool)
        .await
        .expect("reactivate");
    assert_eq!(
        request_get(&test.app, test.peer, "/probe", &cookie, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    test.cleanup().await;
}

#[tokio::test]
async fn never_bootstrapped_opens_documented_legacy_lane() {
    let Some(test) = setup().await else { return };
    // Fresh DB: zero cooperatives, zero coop-attributed audit events —
    // provably never bootstrapped.
    let user = create_user(&test.pool, "pre_tenant").await;
    grant_perm(&test.pool, user, "payments.read", None).await;
    let (cookie, _) = session_for(&test.pool, user).await;

    let (status, body) = request_get(&test.app, test.peer, "/probe", &cookie, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "legacy");
    // Legacy lane → global authorization path.
    assert_eq!(
        request_get(&test.app, test.peer, "/probe-perm", &cookie, None)
            .await
            .0,
        StatusCode::OK
    );

    test.cleanup().await;
}

#[tokio::test]
async fn missing_bootstrap_with_residue_fails_closed() {
    let Some(test) = setup().await else { return };
    let user = create_user(&test.pool, "bozuk_durum").await;
    let (cookie, _) = session_for(&test.pool, user).await;

    // A cooperative exists but none is_bootstrap — missing bootstrap row
    // is NOT proof of never-bootstrapped; the state is inconsistent →
    // deny rather than opening the legacy lane.
    repo::create_cooperative(&test.pool, "Yalniz Kooperatif", None)
        .await
        .expect("non-bootstrap coop");
    assert_eq!(
        request_get(&test.app, test.peer, "/probe", &cookie, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    // Stronger: a real bootstrap whose flag was stripped — cooperative
    // residue without bootstrap → still denied.
    let coop = bootstrap(&test.pool).await;
    sqlx::query("UPDATE cooperatives SET is_bootstrap=false WHERE id=$1")
        .bind(coop)
        .execute(&test.pool)
        .await
        .expect("strip flag");
    assert_eq!(
        request_get(&test.app, test.peer, "/probe", &cookie, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    test.cleanup().await;
}

#[tokio::test]
async fn require_business_applies_verified_legacy_attribution() {
    let Some(test) = setup().await else { return };
    let user = create_user(&test.pool, "legacy_atama").await;
    let coop = bootstrap(&test.pool).await;
    // NULL-coop role + NULL-coop assignment: only attributable to
    // bootstrap.
    grant_perm(&test.pool, user, "payments.read", None).await;
    let (cookie, _) = session_for(&test.pool, user).await;

    // Headerless → bootstrap scope → NULL row grants (verified legacy
    // attribution preserves existing access).
    assert_eq!(
        request_get(&test.app, test.peer, "/probe-perm", &cookie, None)
            .await
            .0,
        StatusCode::OK
    );

    // Same user, a permission they do NOT hold → denied.
    let missing = authz::require_business(
        &test.state,
        &BusinessAuth {
            auth: CurrentAuth {
                user_id: user,
                username: "x".into(),
                display_name: "x".into(),
                session_id: Uuid::new_v4(),
                session_created_at: OffsetDateTime::now_utc(),
                session_expires_at: OffsetDateTime::now_utc(),
                csrf_token: "c".into(),
                active_cooperative_id: None,
            },
            scope: kooperatif_server::tenant::BusinessScope::Bootstrap {
                cooperative_id: coop,
                cooperative_name: "Ana".into(),
            },
        },
        "roles.manage",
    )
    .await;
    assert!(matches!(missing, Err(ApiError::PermissionDenied)));

    // A B-only member with a NULL global assignment gets nothing in the
    // bootstrap scope either (membership join blocks).
    let b_only = create_user(&test.pool, "sadece_b_p1c1").await;
    repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B");
    repo::add_member(&test.pool, "B Kooperatifi", b_only, None)
        .await
        .expect("B membership");
    grant_perm(&test.pool, b_only, "payments.manage", None).await;
    let (cookie_b, _) = session_for(&test.pool, b_only).await;
    assert_eq!(
        request_get(&test.app, test.peer, "/probe-perm", &cookie_b, None)
            .await
            .0,
        StatusCode::FORBIDDEN,
        "B-only member must not inherit bootstrap attribution"
    );

    test.cleanup().await;
}

#[tokio::test]
async fn concurrent_requests_hold_independent_contexts() {
    let Some(test) = setup().await else { return };
    let user_a = create_user(&test.pool, "eszamanli_a").await;
    let coop = bootstrap(&test.pool).await;
    let user_b = create_user(&test.pool, "eszamanli_b").await;
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;
    repo::add_member(&test.pool, "B Kooperatifi", user_b, None)
        .await
        .expect("B membership");
    let (cookie_a, _) = session_for(&test.pool, user_a).await;
    let (cookie_b, _) = session_for(&test.pool, user_b).await;
    let coop_a_str = coop.to_string();
    let coop_b_str = coop_b.to_string();

    let (ra, rb, rc) = tokio::join!(
        request_get(&test.app, test.peer, "/probe", &cookie_a, None),
        request_get(&test.app, test.peer, "/probe", &cookie_b, Some(&coop_b_str)),
        request_get(&test.app, test.peer, "/probe", &cookie_b, Some(&coop_a_str)),
    );
    assert_eq!(ra.0, StatusCode::OK);
    assert_eq!(ra.1, coop.to_string());
    assert_eq!(rb.0, StatusCode::FORBIDDEN, "B header always denied");
    // user_b was created AFTER bootstrap → not a bootstrap member →
    // even the explicit bootstrap header requires real membership.
    assert_eq!(rc.0, StatusCode::FORBIDDEN, "membership still required");

    test.cleanup().await;
}

// ------------------------------------------------------------------
// M1-P1-005 — bootstrap lifecycle marker hardening
// ------------------------------------------------------------------

/// The marker FK makes the bootstrap cooperative undeletable while it
/// is referenced: a previously bootstrapped database can never degrade
/// to "bootstrap row missing" through a DELETE.
#[tokio::test]
async fn marker_makes_bootstrap_cooperative_undeletable() {
    let Some(test) = setup().await else { return };
    let coop = bootstrap(&test.pool).await;

    let err = sqlx::query("DELETE FROM cooperatives WHERE id = $1")
        .bind(coop)
        .execute(&test.pool)
        .await
        .expect_err("FK must reject deleting the marked bootstrap row");
    let sqlx::Error::Database(db_err) = err else {
        panic!("expected FK violation");
    };
    assert_eq!(
        db_err.constraint(),
        Some("installation_state_bootstrap_cooperative_id_fkey")
    );

    // Business access is unaffected — the marker still resolves.
    let user = create_user(&test.pool, "korunan").await;
    repo::add_member(&test.pool, "Ana Kooperatif", user, None)
        .await
        .expect("membership");
    let (cookie, _) = session_for(&test.pool, user).await;
    assert_eq!(
        request_get(&test.app, test.peer, "/probe", &cookie, None)
            .await
            .0,
        StatusCode::OK
    );

    test.cleanup().await;
}

/// Audit evidence is no longer load-bearing for an already-bootstrapped
/// installation: deleting every security_event cannot strip the
/// bootstrap context (the marker is authoritative).
#[tokio::test]
async fn audit_evidence_loss_keeps_bootstrap_authority() {
    let Some(test) = setup().await else { return };
    let user = create_user(&test.pool, "denetim_kaybi").await;
    let coop = bootstrap(&test.pool).await;
    let (cookie, _) = session_for(&test.pool, user).await;

    sqlx::query("DELETE FROM security_events")
        .execute(&test.pool)
        .await
        .expect("wipe audit");

    let (status, body) = request_get(&test.app, test.peer, "/probe", &cookie, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, coop.to_string(), "marker outlives audit loss");

    test.cleanup().await;
}

/// A marker pointing at a non-bootstrap cooperative is inconsistent —
/// every business context must fail closed, never re-pick a row.
#[tokio::test]
async fn inconsistent_marker_and_flag_fail_closed() {
    let Some(test) = setup().await else { return };
    let user = create_user(&test.pool, "tutarsiz").await;
    bootstrap(&test.pool).await;
    let (cookie, _) = session_for(&test.pool, user).await;

    // Flag stripped: marker still points at the row but `is_bootstrap`
    // is gone → resolution is inconsistent → deny (not legacy lane).
    sqlx::query("UPDATE cooperatives SET is_bootstrap = false WHERE is_bootstrap")
        .execute(&test.pool)
        .await
        .expect("strip flag");
    assert_eq!(
        request_get(&test.app, test.peer, "/probe", &cookie, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    // Marker repointed at a real non-bootstrap cooperative → deny.
    sqlx::query("UPDATE cooperatives SET is_bootstrap = true WHERE is_bootstrap = false")
        .execute(&test.pool)
        .await
        .expect("restore flag");
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;
    sqlx::query("UPDATE installation_state SET bootstrap_cooperative_id = $1")
        .bind(coop_b)
        .execute(&test.pool)
        .await
        .expect("repoint marker");
    assert_eq!(
        request_get(&test.app, test.peer, "/probe", &cookie, None)
            .await
            .0,
        StatusCode::FORBIDDEN,
        "non-bootstrap marker target must deny"
    );

    test.cleanup().await;
}

/// Previously bootstrapped but marker wiped (NULL or row deleted):
/// cooperative residue exists → the legacy lane must NOT reopen.
#[tokio::test]
async fn wiped_or_missing_marker_fails_closed() {
    let Some(test) = setup().await else { return };
    let user = create_user(&test.pool, "marker_silindi").await;
    bootstrap(&test.pool).await;
    let (cookie, _) = session_for(&test.pool, user).await;

    // Marker NULL + cooperative residue → deny.
    sqlx::query("UPDATE installation_state SET bootstrap_cooperative_id = NULL")
        .execute(&test.pool)
        .await
        .expect("wipe marker value");
    assert_eq!(
        request_get(&test.app, test.peer, "/probe", &cookie, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    // Marker record itself deleted (partial migration / tamper) → deny.
    sqlx::query("DELETE FROM installation_state")
        .execute(&test.pool)
        .await
        .expect("delete marker row");
    assert_eq!(
        request_get(&test.app, test.peer, "/probe", &cookie, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    // Total wipe (marker row + cooperatives + audit): indistinguishable
    // from never-bootstrapped — this is the documented direct-SQL
    // residual; verify only that the cooperative residue case above
    // already fails (the lane's conditions are asserted by
    // `never_bootstrapped_opens_documented_legacy_lane`).
    test.cleanup().await;
}

/// A cooperative row with no bootstrap marker (partial provisioning /
/// migration) is residue, not a context: deny for any request.
#[tokio::test]
async fn partially_provisioned_cooperative_fails_closed() {
    let Some(test) = setup().await else { return };
    let user = create_user(&test.pool, "kismi_kurulum").await;
    repo::create_cooperative(&test.pool, "Taslak Kooperatif", None)
        .await
        .expect("non-bootstrap coop");
    let (cookie, _) = session_for(&test.pool, user).await;

    assert_eq!(
        request_get(&test.app, test.peer, "/probe", &cookie, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    test.cleanup().await;
}

/// NULL writers still exist (rbac.rs / grant-role until P1-c2/P1-d):
/// a NULL-cooperative role+assignment created AFTER bootstrap remains
/// bootstrap-attributed — the documented, temporary compatibility —
/// while never leaking to another cooperative's member.
#[tokio::test]
async fn post_bootstrap_null_assignment_stays_bootstrap_scoped() {
    let Some(test) = setup().await else { return };
    let coop = bootstrap(&test.pool).await;

    // Bootstrap member with a fresh NULL assignment → granted in
    // bootstrap scope (verified legacy attribution).
    let admin = create_user(&test.pool, "yeni_null_admin").await;
    repo::add_member(&test.pool, "Ana Kooperatif", admin, None)
        .await
        .expect("bootstrap membership");
    grant_perm(&test.pool, admin, "payments.read", None).await;
    let (cookie_a, _) = session_for(&test.pool, admin).await;
    assert_eq!(
        request_get(&test.app, test.peer, "/probe-perm", &cookie_a, None)
            .await
            .0,
        StatusCode::OK
    );

    // A different-cooperative member holding the same NULL grant gets
    // nothing in the bootstrap scope.
    let b_member = create_user(&test.pool, "b_uyesi_null").await;
    repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B");
    repo::add_member(&test.pool, "B Kooperatifi", b_member, None)
        .await
        .expect("B membership");
    grant_perm(&test.pool, b_member, "payments.read", None).await;
    let (cookie_b, _) = session_for(&test.pool, b_member).await;
    assert_eq!(
        request_get(&test.app, test.peer, "/probe-perm", &cookie_b, None)
            .await
            .0,
        StatusCode::FORBIDDEN,
        "NULL assignment never creates cross-cooperative access"
    );

    // Scope still resolves to the bootstrap cooperative id.
    let (status, body) = request_get(&test.app, test.peer, "/probe", &cookie_a, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, coop.to_string());

    test.cleanup().await;
}
