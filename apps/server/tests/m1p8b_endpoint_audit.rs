//! M1-P8-B Batch 2 — endpoint authorization coverage audit.
//!
//! The route inventory is DERIVED from source: this test scans
//! `src/**/*.rs` for `.route(...)` registrations inside `*_router`
//! builder functions, then maps every discovered router function to an
//! authorization lane via an explicit table. A route added to a known
//! router is classified automatically; a NEW router function that is
//! not in the lane table fails the audit — the manifest cannot drift
//! silently.
//!
//! Lanes and their enforcement (verified by probing):
//!   PUBLIC        — reachable without a session (/health, /ready,
//!                   /api/auth/login)
//!   AUTHENTICATED — session required, no cooperative needed
//!                   (auth session routes, tenant switch endpoints)
//!   COOPERATIVE   — business_membership_gate + BusinessAuth +
//!                   per-handler require_business (all business routers)
//!   PLATFORM      — NULL-scope platform permission enforced inside
//!                   the database definers (platform_router,
//!                   platform_audit_router)
//!
//! Probes (real router, real session machinery):
//!   anonymous → every non-public route: 401 authentication_required
//!   cooperative-only member → platform routes: denied, zero writes
//!   platform-only user → every cooperative route: 403
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL for the probe test; the
//! inventory classification test itself is DB-free.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
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

const TEST_PASSWORD: &str = "p8b-audit-parola-123";
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

// ═════════════════════════════════════════════════════════════════════
// Source-derived route inventory
// ═════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Lane {
    Public,
    Authenticated,
    Cooperative,
    Platform,
}

#[derive(Debug, Clone)]
struct RouteSpec {
    router_fn: String,
    path: String,
    methods: Vec<&'static str>,
}

/// Authorization lane of every router builder in the codebase.
/// The audit FAILS on any discovered router function missing here —
/// that is the mechanism that keeps coverage complete.
fn router_lane(router_fn: &str) -> Option<Lane> {
    Some(match router_fn {
        // http/mod.rs `router` registers /health and /ready directly.
        "router" => Lane::Public,
        "auth_router" => Lane::Authenticated,
        "tenant_router" => Lane::Authenticated,
        "platform_router" | "platform_audit_router" => Lane::Platform,
        "rbac_router"
        | "parties_router"
        | "shares_router"
        | "periods_router"
        | "payments_router"
        | "financial_accounts_router"
        | "credits_router"
        | "income_expense_router"
        | "share_returns_router"
        | "investments_router"
        | "social_aid_router"
        | "governance_router"
        | "reports_router"
        | "realtime_router"
        | "audit_router" => Lane::Cooperative,
        _ => return None,
    })
}

/// Per-path lane override: a router may mix lanes across routes.
fn lane_override(path: &str) -> Option<Lane> {
    match path {
        // Reachable without a session by contract.
        "/api/auth/login" => Some(Lane::Public),
        _ => None,
    }
}

/// Paths that intentionally succeed for anonymous callers: `logout` is
/// designed idempotent — no/invalid session still clears the cookie
/// with 204 and revokes nothing (see auth::routes::logout).
fn anonymous_idempotent(path: &str) -> bool {
    path == "/api/auth/logout"
}

fn collect_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("src readable") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Indices of module-level function definitions in `src`.
fn fn_boundaries(src: &str) -> Vec<(usize, String)> {
    let mut fns = Vec::new();
    for (pos, _) in src.match_indices('\n') {
        let line_start = pos + 1;
        let text = &src[line_start..];
        for prefix in [
            "pub fn ",
            "pub(crate) fn ",
            "fn ",
            "pub async fn ",
            "async fn ",
        ] {
            if let Some(rest) = text.strip_prefix(prefix) {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                fns.push((line_start, name));
                break;
            }
        }
    }
    fns
}

const METHOD_TOKENS: &[(&str, &str)] = &[
    ("get(", "GET"),
    ("post(", "POST"),
    ("put(", "PUT"),
    ("patch(", "PATCH"),
    ("delete(", "DELETE"),
];

/// Extract `METHOD`s from a `.route(path, <chain>)` argument chunk.
fn methods_of(chunk: &str) -> Vec<&'static str> {
    METHOD_TOKENS
        .iter()
        .filter(|(token, _)| chunk.contains(token))
        .map(|(_, method)| *method)
        .collect()
}

fn discover_routes() -> Vec<RouteSpec> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_sources(&root, &mut files);
    let mut routes = Vec::new();
    for file in files {
        let src = std::fs::read_to_string(&file).expect("source readable");
        let mut bounds = fn_boundaries(&src);
        bounds.push((src.len(), String::new()));
        for pair in bounds.windows(2) {
            let (start, name) = &pair[0];
            let end = pair[1].0;
            if !name.ends_with("_router") && name != "router" {
                continue;
            }
            let span = &src[*start..end];
            for (mpos, _) in span.match_indices(".route(") {
                let args = &span[mpos + ".route(".len()..];
                // Path argument: literal "..." or ALL_CAPS const name.
                let trimmed = args.trim_start();
                let path = if let Some(rest) = trimmed.strip_prefix('"') {
                    rest.split('"').next().unwrap_or_default().to_string()
                } else {
                    let const_name: String = trimmed
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    match const_name.as_str() {
                        "HEALTH_PATH" => "/health".to_string(),
                        "READY_PATH" => "/ready".to_string(),
                        other => {
                            panic!("route() with unresolved path token {other:?} in {name}")
                        }
                    }
                };
                // Method chain runs until the next .route( / .layer( /
                // router-builder statement terminator.
                let stop = args
                    .find(".route(")
                    .or_else(|| args.find(".layer("))
                    .unwrap_or(args.len());
                let methods = methods_of(&args[..stop]);
                if methods.is_empty() {
                    panic!("route {path} in {name} exposed no HTTP methods");
                }
                routes.push(RouteSpec {
                    router_fn: name.clone(),
                    path,
                    methods,
                });
            }
        }
    }
    routes
}

// ═════════════════════════════════════════════════════════════════════
// Test 1 — inventory completeness (DB-free)
// ═════════════════════════════════════════════════════════════════════

/// Every discovered router function must have a declared lane; every
/// discovered route inherits it (plus explicit per-path overrides).
/// Pinned route counts make silent additions visible in review.
#[test]
fn route_inventory_is_fully_classified() {
    let routes = discover_routes();
    let mut lanes: BTreeMap<Lane, usize> = BTreeMap::new();
    let mut unclassified = Vec::new();
    let mut classified = Vec::new();
    for route in &routes {
        let lane = lane_override(&route.path).or_else(|| router_lane(&route.router_fn));
        match lane {
            Some(lane) => {
                *lanes.entry(lane).or_default() += 1;
                classified.push((route.clone(), lane));
            }
            None => unclassified.push(route.router_fn.clone()),
        }
    }
    assert!(
        unclassified.is_empty(),
        "router functions without a declared authorization lane: {unclassified:?}"
    );
    let total: usize = lanes.values().sum();
    eprintln!("lane distribution: {lanes:?} (total {total})");
    // Sanity bounds, not exact pins: the audit's strength is the
    // unknown-router failure above, not a frozen count.
    assert!(
        lanes[&Lane::Cooperative] >= 100,
        "cooperative lane implausibly small: {lanes:?}"
    );
    // 10 lifecycle/audit routes + GET /api/platform/users (migration
    // 0080 picker feed — NULL-scope users.read, definer-enforced).
    assert_eq!(
        lanes[&Lane::Platform],
        11,
        "platform lane drifted: {lanes:?}"
    );
    assert_eq!(lanes[&Lane::Public], 3, "public lane drifted: {lanes:?}");
    assert!(
        lanes[&Lane::Authenticated] >= 8,
        "authenticated lane implausibly small: {lanes:?}"
    );
}

// ═════════════════════════════════════════════════════════════════════
// Test harness (same pattern as idor_matrix / header_attacks)
// ═════════════════════════════════════════════════════════════════════

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

async fn setup() -> TestApp {
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
    users::create_user(pool, username, "P8B Audit", &hash, TEST_PASSWORD)
        .await
        .expect("user")
        .id
}

async fn grant(pool: &PgPool, coop: Option<Uuid>, user: Uuid, perms: &[&str], member: bool) {
    if let (true, Some(coop)) = (member, coop) {
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
    }
    let role: Uuid =
        sqlx::query_scalar("INSERT INTO roles (name, cooperative_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("audit-{user}-{coop:?}"))
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

async fn login(app: &axum::Router, peer: SocketAddr, username: &str) -> (String, String) {
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
    )
}

/// Concrete path for a template like `/api/families/{id}` — any UUID
/// is equally unauthorized, existence is irrelevant for access probes.
fn concrete(path: &str) -> String {
    static MISSING: &str = "01900000-0000-7000-8000-000000000000";
    path.replace("{id}", MISSING)
}

async fn probe(
    app: &axum::Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    csrf: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(concrete(path))
        .header("origin", ORIGIN)
        .header("content-type", "application/json");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("kooperatif_session={cookie}"));
    }
    if let Some(csrf) = csrf {
        builder = builder.header("x-csrf-token", csrf);
    }
    let request = builder
        .body(Body::from(
            json!({
                "name": "Audit Probe", "initialAdminUserId": Uuid::new_v4(),
                "cooperativeId": Uuid::new_v4(), "reason": "audit"
            })
            .to_string(),
        ))
        .expect("valid");
    let response = app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

// ═════════════════════════════════════════════════════════════════════
// Test 2 — lane probes against the real router
// ═════════════════════════════════════════════════════════════════════

/// The derived inventory drives every probe — no hand-picked routes:
///
/// 1. Anonymous: every non-public route denies with 401
///    `authentication_required`; public routes are reachable.
/// 2. Cooperative member (full business grants, zero platform grants):
///    every platform route denies and writes nothing.
/// 3. Platform-only user (platform grant, zero memberships): every
///    cooperative route denies with 403 `cooperative_access_denied`.
#[tokio::test]
async fn every_route_enforces_its_authorization_lane() {
    let test = setup().await;
    let coop_a: Uuid = sqlx::query_scalar("SELECT id FROM cooperatives WHERE is_bootstrap")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    let coop_user = create_user(&test.pool, "p8b-audit-coop").await;
    grant(
        &test.pool,
        Some(coop_a),
        coop_user,
        ALL_BUSINESS_PERMS,
        true,
    )
    .await;
    let (coop_cookie, coop_csrf) = login(&test.app, test.peer, "p8b-audit-coop").await;
    let plat_user = create_user(&test.pool, "p8b-audit-plat").await;
    grant(&test.pool, None, plat_user, &["cooperatives.manage"], false).await;
    let (plat_cookie, plat_csrf) = login(&test.app, test.peer, "p8b-audit-plat").await;

    let routes = discover_routes();
    let mut counts: BTreeMap<Lane, usize> = BTreeMap::new();
    let coops_before: i64 = sqlx::query_scalar("SELECT count(*) FROM cooperatives")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    let disabled_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE status = 'disabled'")
            .fetch_one(&test.pool)
            .await
            .unwrap();

    for route in &routes {
        let lane = lane_override(&route.path).or_else(|| router_lane(&route.router_fn));
        let Some(lane) = lane else { continue };
        *counts.entry(lane).or_default() += 1;
        for method in &route.methods {
            // 1. Anonymous: non-public lanes must yield 401.
            let (status, body) = probe(&test.app, method, &route.path, None, None).await;
            match lane {
                Lane::Public => {
                    assert_ne!(
                        status,
                        StatusCode::UNAUTHORIZED,
                        "public route {method} {} rejected anonymous callers",
                        route.path
                    );
                }
                _ if route.path == "/api/realtime" => {
                    // WebSocket upgrade rejects before auth extraction
                    // (missing Upgrade headers → 400). Either way the
                    // anonymous caller is refused with no data.
                    assert!(
                        status.is_client_error(),
                        "realtime answered anonymous probe with {status}"
                    );
                }
                _ if anonymous_idempotent(&route.path) => {
                    assert!(
                        status.is_success(),
                        "idempotent route {method} {} rejected anonymous callers: {status}",
                        route.path
                    );
                }
                _ => {
                    assert_eq!(
                        status,
                        StatusCode::UNAUTHORIZED,
                        "{lane:?} route {method} {} answered anonymous request with {status}",
                        route.path
                    );
                    assert_eq!(
                        body["error"]["code"],
                        json!("authentication_required"),
                        "{method} {} anon denial code drifted: {body}",
                        route.path
                    );
                }
            }
            // 2. Cooperative-only member on platform routes: denied,
            //    never silently authorized by business grants.
            if lane == Lane::Platform {
                let (status, body) = probe(
                    &test.app,
                    method,
                    &route.path,
                    Some(&coop_cookie),
                    Some(&coop_csrf),
                )
                .await;
                assert!(
                    !status.is_success(),
                    "cooperative member reached platform {method} {} → {status}",
                    route.path
                );
                if *method == "GET" {
                    assert_eq!(
                        status,
                        StatusCode::FORBIDDEN,
                        "platform read {method} {} denied with wrong shape {status}",
                        route.path
                    );
                    assert_eq!(body["error"]["code"], json!("permission_denied"));
                }
            }
            // 3. Platform-only user on cooperative routes: the
            //    membership gate must deny — platform authority never
            //    implies tenant presence.
            if lane == Lane::Cooperative {
                let (status, body) = probe(
                    &test.app,
                    method,
                    &route.path,
                    Some(&plat_cookie),
                    Some(&plat_csrf),
                )
                .await;
                assert_eq!(
                    status,
                    StatusCode::FORBIDDEN,
                    "platform user reached cooperative {method} {} → {status}",
                    route.path
                );
                assert_eq!(
                    body["error"]["code"],
                    json!("cooperative_access_denied"),
                    "{method} {} platform-user denial code drifted: {body}",
                    route.path
                );
            }
        }
    }

    // Zero-write proof for every platform probe issued above.
    let coops_after: i64 = sqlx::query_scalar("SELECT count(*) FROM cooperatives")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert_eq!(
        coops_before, coops_after,
        "platform probe created cooperatives"
    );
    let disabled_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE status = 'disabled'")
            .fetch_one(&test.pool)
            .await
            .unwrap();
    assert_eq!(
        disabled_before, disabled_after,
        "platform probe disabled users"
    );

    eprintln!("probed route lanes: {counts:?}");
    test.cleanup().await;
}
