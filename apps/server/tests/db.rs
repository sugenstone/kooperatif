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

    db::run_test_migrations(&pool)
        .await
        .expect("migrations apply");
    // Re-running must be a no-op (deterministic, idempotent mechanism).
    db::run_test_migrations(&pool)
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
    db::assert_disposable_test_database_name(name);
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(admin)
        .await
        .expect("probe database created");
}

async fn drop_probe(admin: &sqlx::PgPool, name: &str) {
    db::assert_disposable_test_database_name(name);
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

/// Run authorized-delete GC passes until the probe is gone
/// (deadline-bounded). The authorized path is still confined to
/// databases this process registered as current-run-owned, so tests
/// register their probe before reclamation is expected. Eventual
/// reclamation within a generous deadline is the contract, not pass
/// count.
async fn dropped_eventually(admin: &sqlx::PgPool, name: &str) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        let _ = db::drop_stale_test_databases_authorized_forced(admin).await;
        if !probe_exists(admin, name).await {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    let comment = probe_comment(admin, name).await;
    let conns: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname = $1")
        .bind(name)
        .fetch_one(admin)
        .await
        .unwrap_or(-1);
    panic!("probe {name} survived 120s of GC passes: comment={comment:?} conns={conns}")
}

#[tokio::test]
async fn gc_marks_on_first_sighting_then_drops_after_grace() {
    let admin = admin_pool().await;
    let name = disposable_name();
    create_probe(&admin, &name).await;

    // First pass: zero connections, no marker → marked, NOT dropped.
    // (Probe-state assertions only — a parallel pass may legitimately
    // drop unrelated stale databases, making the count unreliable.)
    let _ = db::drop_stale_test_databases_forced(&admin).await;
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
    let _ = db::drop_stale_test_databases_forced(&admin).await;
    assert!(probe_exists(&admin, &name).await);

    // Backdate the marker past the 30-minute grace → under the default
    // policy the candidate is STILL retained: automatic sweeps never
    // delete (previous-run ownership is outside the current-run
    // teardown boundary). Only the authorized-delete variant reclaims.
    set_probe_comment(&admin, &name, "kooperatif-gc-pending:1").await;
    let _ = db::drop_stale_test_databases_forced(&admin).await;
    assert!(
        probe_exists(&admin, &name).await,
        "default-policy sweep must retain even a stale-pending database"
    );
    // The probe stays UNREGISTERED until every retention assertion is
    // done: CURRENT_RUN_DATABASES is process-global, so registering
    // earlier would let a sibling test's authorized sweep legally
    // reclaim this probe mid-assertion. Unregistered names sit outside
    // the only deletion boundary — no sweep can reach them.
    db::register_current_run_database(&name);
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
    // Deliberate harness bypass: this test's whole point is proving GC
    // ignores foreign-name databases, so the foreign probe is created by
    // raw admin SQL — the guarded helpers refuse it by design.
    sqlx::query(&format!("CREATE DATABASE \"{nonmatching}\""))
        .execute(&admin)
        .await
        .expect("nonmatching probe database created");

    // A live pool on the matching DB = active run → not even marked.
    let url = test_database_url().unwrap();
    let base = url.rsplit_once('/').map(|(b, _)| b.to_string()).unwrap();
    let live = db::connect(&format!("{base}/{matching}"))
        .await
        .expect("live connection");
    sqlx::query("SELECT 1").execute(&live).await.unwrap();

    let _ = db::drop_stale_test_databases_forced(&admin).await;
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
    let _ = db::drop_stale_test_databases_forced(&admin).await;
    assert!(probe_exists(&admin, &matching).await);
    // The nonmatching name must not carry a marker either.
    assert!(probe_comment(&admin, &nonmatching).await.is_none());

    drop_probe(&admin, &matching).await;
    sqlx::query(&format!(
        "DROP DATABASE IF EXISTS \"{nonmatching}\" WITH (FORCE)"
    ))
    .execute(&admin)
    .await
    .expect("nonmatching probe dropped");
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
    let _ = db::drop_stale_test_databases_forced(&admin).await;
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
    let _ = db::drop_stale_test_databases_forced(&admin).await;
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
    db::register_current_run_database(&name);

    // Simulate a dead owner: an owned marker far past the lease TTL.
    set_probe_comment(&admin, &name, "kooperatif-test-owned:1").await;
    let _ = db::drop_stale_test_databases_forced(&admin).await;
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
    let _ = db::drop_stale_test_databases_forced(&admin).await;
    assert!(probe_exists(&admin, &name).await);

    // Age the pending marker past the grace → finally reclaimed on the
    // authorized current-run-confined path.
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
    db::register_current_run_database(&name);

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
    let _ = db::drop_stale_test_databases_forced(&admin).await;
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

/// The authorized-delete GC path is confined to databases THIS process
/// registered as current-run-owned: a foreign candidate — shaped
/// exactly like a previous-run leftover (stale `gc-pending` marker,
/// zero connections) — is retained even under the authorized sweep,
/// while a registered probe created by this test is reclaimed.
#[tokio::test]
async fn authorized_sweep_cannot_target_unregistered_databases() {
    let admin = admin_pool().await;
    let foreign = disposable_name();
    let owned = disposable_name();
    create_probe(&admin, &foreign).await;
    create_probe(&admin, &owned).await;
    db::register_current_run_database(&owned);

    set_probe_comment(&admin, &foreign, "kooperatif-gc-pending:1").await;
    set_probe_comment(&admin, &owned, "kooperatif-gc-pending:1").await;

    let _ = db::drop_stale_test_databases_authorized_forced(&admin).await;
    assert!(
        probe_exists(&admin, &foreign).await,
        "authorized sweep must never reach unregistered databases"
    );
    // A single pass may legitimately skip the probe (transient
    // pg_stat_activity row, held advisory lock, racing DROP rejection)
    // — eventual reclamation within the deadline is the contract, not
    // first-pass deletion. Sibling tests use the same helper.
    assert!(
        dropped_eventually(&admin, &owned).await,
        "registered current-run probe is reclaimable"
    );

    drop_probe(&admin, &foreign).await;
}

/// M1-INFRA-002 — deterministic scan→drop race on the authorized-delete
/// path (the only path that can still DROP, and only for names this
/// process registered as current-run-owned): the GC pauses inside its
/// advisory
/// lock immediately before the real DROP, the test then opens a
/// connection to the candidate, and the plain (non-FORCE)
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
        // The gated authorized pass may only reach current-run names.
        db::register_current_run_database(&name);
        set_probe_comment(&admin, &name, "kooperatif-gc-pending:1").await;

        let gate = db::DropGate::for_database(&name);
        let mut live = None;
        // The race needs a real DROP attempt → the explicitly
        // authorized variant (default-policy sweeps never drop).
        let gc = db::drop_stale_test_databases_authorized_gated(&admin, Some(&gate));
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
        let Ok((_dropped, ())) = outcome else {
            // Another pass stole the probe — recreate and retry.
            continue;
        };

        // `dropped` counts every candidate this pass reclaimed —
        // unrelated expired stale databases legitimately drop alongside
        // the probe (the file's probe-state convention already treats
        // the count as unreliable). The race contract is: the probed
        // database survives and the late connection is never killed.
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
