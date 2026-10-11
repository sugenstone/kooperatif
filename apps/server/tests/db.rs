//! Database-gated integration tests (STEP-001 §18: database
//! connectivity/migration).
//!
//! Requires `KOOPERATIF_TEST_DATABASE_URL` pointing at a disposable
//! database (scripts/db-verify.sh and the CI database job provision one).
//! Without the variable these tests skip with an explicit notice —
//! plain `cargo test` stays deterministic and dependency-free.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

use std::sync::Arc;

use kooperatif_server::auth::AuthRuntime;
use kooperatif_server::clock::MutableClock;
use kooperatif_server::config::AuthConfig;
use kooperatif_server::db;
use kooperatif_server::http::{cors_layer, router, AppState};

/// Required DB-gated test gate (PILOT-FIX-001 / F5): a missing
/// KOOPERATIF_TEST_DATABASE_URL is an explicit FAILURE, never a
/// silent skip — CI and release gates must prove these tests ran.
/// Unit-only execution stays unaffected: this function is only
/// reached by database-gated setup paths.
fn test_database_url() -> Option<String> {
    let value = std::env::var("KOOPERATIF_TEST_DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "KOOPERATIF_TEST_DATABASE_URL is not set — required DB-gated              integration tests cannot silently pass; point it at a              disposable PostgreSQL database"
        )
    });
    let trimmed = value.trim().to_string();
    assert!(
        !trimmed.is_empty(),
        "KOOPERATIF_TEST_DATABASE_URL is empty — required DB-gated tests need a database"
    );
    Some(trimmed)
}

#[tokio::test]
async fn migrations_apply_to_a_clean_database_and_readiness_reports_ready() {
    // PILOT-FIX-001 / F5: test_database_url() fails loudly when unset.
    let url = test_database_url().expect("test database url");

    let pool = db::connect(&url).await.expect("database connectivity");

    db::run_migrations(&pool).await.expect("migrations apply");
    // Re-running must be a no-op (deterministic, idempotent mechanism).
    db::run_migrations(&pool)
        .await
        .expect("migrations re-run is a no-op");

    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .expect("migration ledger readable");
    assert!(
        applied >= 1,
        "at least one migration must be recorded, got {applied}"
    );

    let app = router(
        AppState {
            db: Some(pool),
            auth: Arc::new(AuthRuntime::new(
                AuthConfig {
                    allowed_origins: vec!["http://localhost:5173".to_string()],
                    cookie_secure: false,
                    session_absolute_ttl_secs: 3600,
                    session_idle_ttl_secs: 600,
                    argon2_m_cost: kooperatif_server::config::ARGON2_M_COST_FLOOR,
                    argon2_t_cost: 1,
                    argon2_p_cost: 1,
                    login_window_secs: 60,
                    login_username_max_attempts: 5,
                    login_ip_max_attempts: 100,
                    forwarded_ip: false,
                },
                Arc::new(MutableClock::new(time::OffsetDateTime::now_utc())),
            )),
        },
        cors_layer(&[]),
    );
    let response = app
        .oneshot(
            Request::get("/ready")
                .body(Body::empty())
                .expect("valid request"),
        )
        .await
        .expect("infallible");

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("readable body");
    let readiness: Value = serde_json::from_slice(&body).expect("JSON body");
    assert_eq!(
        readiness,
        serde_json::json!({ "status": "ready", "checks": { "database": "ok" } })
    );
}

// ---------------------------------------------------------------------
// M1-INFRA-001/002 — heartbeat-leased ownership GC: claim lifecycle,
// demote→pending→grace reclamation, connection guard, deterministic
// scan→drop race, naming-contract preservation.
// ---------------------------------------------------------------------

/// Admin pool on the `postgres` maintenance database — the GC vantage
/// point. Derived from KOOPERATIF_TEST_DATABASE_URL.
async fn admin_pool() -> sqlx::PgPool {
    let url = test_database_url().expect("test database url");
    let admin_url = url
        .rsplit_once('/')
        .map(|(base, _)| format!("{base}/postgres"))
        .expect("url with database segment");
    db::connect(&admin_url).await.expect("admin connectivity")
}

fn disposable_name() -> String {
    format!("kooperatif_test_{}", Uuid::new_v4().simple())
}

async fn create_probe(admin: &sqlx::PgPool, name: &str) {
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(admin)
        .await
        .expect("probe database created");
}

async fn drop_probe(admin: &sqlx::PgPool, name: &str) {
    sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"))
        .execute(admin)
        .await
        .expect("probe database dropped");
}

async fn probe_exists(admin: &sqlx::PgPool, name: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)")
        .bind(name)
        .fetch_one(admin)
        .await
        .expect("existence check")
}

async fn probe_comment(admin: &sqlx::PgPool, name: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT shobj_description(oid, 'pg_database') FROM pg_database WHERE datname = $1",
    )
    .bind(name)
    .fetch_one(admin)
    .await
    .expect("comment read")
}

async fn set_probe_comment(admin: &sqlx::PgPool, name: &str, comment: &str) {
    sqlx::query(&format!("COMMENT ON DATABASE {name} IS '{comment}'"))
        .execute(admin)
        .await
        .expect("probe marker write");
}

fn probe_admin_url() -> String {
    let url = test_database_url().expect("test database url");
    url.rsplit_once('/')
        .map(|(base, _)| format!("{base}/postgres"))
        .expect("url with database segment")
}

/// Run GC passes until the probe is gone (bounded). Under parallel
/// load a pass may skip a droppable database when a concurrent pass
/// holds its advisory lock — eventual reclamation is the contract.
async fn dropped_eventually(admin: &sqlx::PgPool, name: &str) -> bool {
    for _ in 0..5 {
        let _ = db::drop_stale_test_databases(admin).await;
        if !probe_exists(admin, name).await {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    false
}

#[tokio::test]
async fn gc_marks_on_first_sighting_then_drops_after_grace() {
    let admin = admin_pool().await;
    let name = disposable_name();
    create_probe(&admin, &name).await;

    // First pass: zero connections, no marker → marked, NOT dropped.
    // (Probe-state assertions only — a parallel pass may legitimately
    // drop unrelated stale databases, making the count unreliable.)
    let _ = db::drop_stale_test_databases(&admin).await;
    assert!(
        probe_exists(&admin, &name).await,
        "probe must survive pass 1"
    );
    let comment = probe_comment(&admin, &name).await.unwrap_or_default();
    assert!(
        comment.starts_with("kooperatif-gc-pending:"),
        "expected GC marker, got {comment:?}"
    );

    // Second pass inside the grace window → still protected.
    let _ = db::drop_stale_test_databases(&admin).await;
    assert!(probe_exists(&admin, &name).await);

    // Backdate the marker past the 30-minute grace → reclaimed.
    // Several passes: a concurrent GC may legitimately hold the
    // probe's advisory lock mid-classification (try-lock skip is the
    // DESIGNED safe path), so reclamation is eventual, not one-shot.
    set_probe_comment(&admin, &name, "kooperatif-gc-pending:1").await;
    assert!(
        dropped_eventually(&admin, &name).await,
        "stale pending marker must be reclaimed"
    );

    drop_probe(&admin, &name).await; // idempotent safety
}

#[tokio::test]
async fn gc_never_touches_connected_or_nonmatching_databases() {
    let admin = admin_pool().await;
    let matching = disposable_name();
    let nonmatching = format!("kooperatif_gc_keep_{}", Uuid::new_v4().simple());

    create_probe(&admin, &matching).await;
    create_probe(&admin, &nonmatching).await;

    // A live pool on the matching DB = active run → not even marked.
    let url = test_database_url().unwrap();
    let base = url.rsplit_once('/').map(|(b, _)| b.to_string()).unwrap();
    let live = db::connect(&format!("{base}/{matching}"))
        .await
        .expect("live connection");
    sqlx::query("SELECT 1").execute(&live).await.unwrap();

    let _ = db::drop_stale_test_databases(&admin).await;
    assert!(
        probe_exists(&admin, &matching).await,
        "connected DB survived"
    );
    // The owner stays usable even if a racing pass caught its
    // create→connect gap and stamped a harmless marker.
    sqlx::query("SELECT 1").execute(&live).await.unwrap();
    assert!(probe_exists(&admin, &nonmatching).await);

    // Now idle it: close the pool — first sighting marks, never drops.
    live.close().await;
    let _ = db::drop_stale_test_databases(&admin).await;
    assert!(probe_exists(&admin, &matching).await);
    // The nonmatching name must not carry a marker either.
    assert!(probe_comment(&admin, &nonmatching).await.is_none());

    drop_probe(&admin, &matching).await;
    drop_probe(&admin, &nonmatching).await;
}

/// M1-INFRA-002 — a claimed (heartbeat-owned) database survives an
/// arbitrary number of GC passes with ZERO connections: the lease in
/// the shared catalog is the liveness proof, not a session.
#[tokio::test]
async fn claimed_database_survives_indefinite_idle_without_connections() {
    let admin = admin_pool().await;
    let name = disposable_name();
    create_probe(&admin, &name).await;

    let claim = db::claim_test_database_with_heartbeat(
        &probe_admin_url(),
        &name,
        std::time::Duration::from_millis(100),
    )
    .await;

    // Stay disconnected far longer than a single heartbeat cycle.
    tokio::time::sleep(std::time::Duration::from_millis(450)).await;
    let _ = db::drop_stale_test_databases(&admin).await;
    assert!(
        probe_exists(&admin, &name).await,
        "claimed DB must never be dropped"
    );
    let comment = probe_comment(&admin, &name).await.unwrap_or_default();
    assert!(
        comment.starts_with("kooperatif-test-owned:"),
        "expected owned lease, got {comment:?}"
    );

    // More heartbeats + another pass — still owned, still alive.
    tokio::time::sleep(std::time::Duration::from_millis(350)).await;
    let _ = db::drop_stale_test_databases(&admin).await;
    assert!(probe_exists(&admin, &name).await);

    claim.stop();
    drop_probe(&admin, &name).await;
}

/// M1-INFRA-002 — an expired lease is DEMOTED to gc-pending, never
/// dropped outright: a still-fresh pending marker then survives the
/// grace window, only a stale one is reclaimed.
#[tokio::test]
async fn expired_lease_demotes_to_pending_before_any_drop() {
    let admin = admin_pool().await;
    let name = disposable_name();
    create_probe(&admin, &name).await;

    // Simulate a dead owner: an owned marker far past the lease TTL.
    set_probe_comment(&admin, &name, "kooperatif-test-owned:1").await;
    let _ = db::drop_stale_test_databases(&admin).await;
    assert!(
        probe_exists(&admin, &name).await,
        "expired lease must demote, not drop"
    );
    let comment = probe_comment(&admin, &name).await.unwrap_or_default();
    assert!(
        comment.starts_with("kooperatif-gc-pending:"),
        "expected demotion marker, got {comment:?}"
    );

    // Fresh pending marker → still protected inside the grace window.
    let _ = db::drop_stale_test_databases(&admin).await;
    assert!(probe_exists(&admin, &name).await);

    // Age the pending marker past the grace → finally reclaimed.
    set_probe_comment(&admin, &name, "kooperatif-gc-pending:1").await;
    assert!(
        dropped_eventually(&admin, &name).await,
        "probe must be dropped"
    );

    drop_probe(&admin, &name).await;
}

/// M1-INFRA-002 — an interrupted run (claim dropped, heartbeat
/// aborted) is reclaimed through the full owned→demote→pending→drop
/// path; each pass is a separate sighting.
#[tokio::test]
async fn interrupted_run_is_reclaimed_after_pending_grace() {
    let admin = admin_pool().await;
    let name = disposable_name();
    create_probe(&admin, &name).await;

    // Claim, then simulate process death by releasing the claim.
    let claim = db::claim_test_database_with_heartbeat(
        &probe_admin_url(),
        &name,
        std::time::Duration::from_millis(50),
    )
    .await;
    claim.stop();

    // With the heartbeat dead the lease goes stale (backdated here so
    // the test stays fast — the transition path is identical).
    set_probe_comment(&admin, &name, "kooperatif-test-owned:1").await;
    let _ = db::drop_stale_test_databases(&admin).await;
    assert!(
        probe_exists(&admin, &name).await,
        "first sighting of dead lease demotes"
    );

    set_probe_comment(&admin, &name, "kooperatif-gc-pending:1").await;
    assert!(
        dropped_eventually(&admin, &name).await,
        "abandoned run must be reclaimed"
    );

    drop_probe(&admin, &name).await;
}

/// M1-INFRA-002 — deterministic scan→drop race: the GC pauses inside
/// its advisory lock immediately before the real DROP, the test then
/// opens a connection to the candidate, and the plain (non-FORCE)
/// `DROP DATABASE` must be rejected atomically by the engine (55006) —
/// the database survives and the late connection is never killed.
#[tokio::test]
async fn scan_to_drop_connection_race_is_survived() {
    let admin = admin_pool().await;
    let url = test_database_url().unwrap();
    let base = url.rsplit_once('/').map(|(b, _)| b.to_string()).unwrap();
    let name = disposable_name();

    // A parallel GC pass may legitimately reclaim the probe before our
    // gated pass reaches it — retry until OUR pass lands the race.
    for _attempt in 0..3 {
        if !probe_exists(&admin, &name).await {
            create_probe(&admin, &name).await;
        }
        set_probe_comment(&admin, &name, "kooperatif-gc-pending:1").await;

        let gate = db::DropGate::for_database(&name);
        let mut live = None;
        let gc = db::drop_stale_test_databases_gated(&admin, Some(&gate));
        let driver = async {
            // GC is parked between its own zero-connection re-check
            // and the DROP — inject the racy reconnect deterministically.
            gate.pause.wait().await;
            let conn = db::connect(&format!("{base}/{name}"))
                .await
                .expect("late connection");
            sqlx::query("SELECT 1").execute(&conn).await.unwrap();
            live = Some(conn);
            gate.proceed.wait().await;
        };
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            tokio::join!(gc, driver)
        })
        .await;
        let Ok((dropped, ())) = outcome else {
            // Another pass stole the probe — recreate and retry.
            continue;
        };

        assert_eq!(dropped, 0, "a connected database must not be dropped");
        assert!(
            probe_exists(&admin, &name).await,
            "probe must survive the raced drop attempt"
        );
        // The late connection is alive — FORCE would have killed it.
        sqlx::query("SELECT 1")
            .execute(live.as_ref().unwrap())
            .await
            .expect("late connection remains usable");

        live.unwrap().close().await;
        drop_probe(&admin, &name).await;
        return;
    }
    panic!("probe kept being stolen by concurrent GC passes");
}
