//! PostgreSQL connectivity and migrations (STEP-001 infrastructure layer).
//!
//! Migrations live in the repository-root `migrations/` directory and are
//! embedded into the binary at compile time. They are applied by the
//! explicit `migrate` subcommand — production migration is a controlled
//! deployment step, never an implicit side effect of serving (ADR-012).

use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// Connection pool defaults sized for the STEP-001 baseline.
const MAX_CONNECTIONS: u32 = 5;
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(3);

pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        .acquire_timeout(ACQUIRE_TIMEOUT)
        .connect(database_url)
        .await
}

/// Apply all pending migrations. Safe to re-run: applied migrations are
/// tracked in `_sqlx_migrations` and never re-applied.
pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("../../migrations").run(pool).await
}

/// Dev-hygiene garbage collection (M1-INFRA-001).
///
/// Every integration `setup()` creates a `kooperatif_test_<uuid>`
/// database; the happy path drops it in `TestApp::cleanup()`, but a
/// panic, assert failure or killed process leaks it. The shared dev
/// instance accumulated ~1,300 orphaned databases (~14 GB) because the
/// previous 24 h `pg_stat_file` mtime clause never matched: PostgreSQL
/// 17 gives `CREATE DATABASE` directories *fresh* mtimes, so only
/// day-old leftovers were collectable.
///
/// Ownership model — two-pass marker, no timestamps guessed:
/// a disposable-pattern database with ZERO connections is *marked*,
/// never dropped, on first sighting: `COMMENT ON DATABASE` writes
/// `kooperatif-gc-pending:<unix epoch>` into the shared catalog. Only
/// a database that still has zero connections on a LATER pass and whose
/// marker is older than [`ORPHAN_GRACE_SECS`] (30 min — ~20× the slowest
/// observed test) is dropped. A live test that is momentarily
/// disconnected (pool idle, worker pause) is therefore unreachable by
/// construction; an interrupted run's leftovers are reclaimed on the
/// next pass after the grace window.
///
/// Absolute guards, unchanged:
/// - exact harness pattern `kooperatif_test_<32 hex>` (regex-enforced,
///   so `kooperatif_test_seed`, `kooperatif_e2e_*`, audit fixtures and
///   every nonmatching name are never even scanned);
/// - `NOT datistemplate`;
/// - zero rows in `pg_stat_activity` — an actively connected test
///   database is skipped before any marker is even read.
///
/// The `postgres` maintenance database is the GC vantage point;
/// `COMMENT ON DATABASE`/`shobj_description` are shared-catalog
/// operations — no connection to the candidate, no schema touch.
const ORPHAN_MARKER_PREFIX: &str = "kooperatif-gc-pending:";
const ORPHAN_GRACE_SECS: i64 = 30 * 60;

pub async fn drop_stale_test_databases(admin: &PgPool) -> u64 {
    let now: i64 = match sqlx::query_scalar("SELECT extract(epoch from now())::bigint")
        .fetch_one(admin)
        .await
    {
        Ok(ts) => ts,
        Err(error) => {
            tracing::warn!(error = %error, "stale test-database clock read failed");
            return 0;
        }
    };
    let rows: Vec<(String, Option<String>)> = match sqlx::query_as(
        "SELECT d.datname, shobj_description(d.oid, 'pg_database') \
         FROM pg_database d \
         WHERE d.datname ~ '^kooperatif_test_[0-9a-f]{32}$' \
           AND NOT d.datistemplate \
           AND NOT EXISTS ( \
               SELECT 1 FROM pg_stat_activity a WHERE a.datname = d.datname \
           )",
    )
    .fetch_all(admin)
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(error = %error, "stale test-database scan failed");
            return 0;
        }
    };
    let mut dropped = 0u64;
    for (name, comment) in rows {
        // datname comes from the system catalog, not user input — the
        // identifier is still double-quoted defensively.
        let quoted = name.replace('"', "\"\"");
        let marked_at = comment
            .as_deref()
            .and_then(|c| c.strip_prefix(ORPHAN_MARKER_PREFIX))
            .and_then(|ts| ts.parse::<i64>().ok());
        match marked_at {
            // Second sighting, grace elapsed, still zero connections —
            // positively abandoned. A concurrent reconnect between scan
            // and DROP is caught by the engine's own connection check.
            Some(ts) if now - ts >= ORPHAN_GRACE_SECS => {
                if let Err(error) = sqlx::query(&format!("DROP DATABASE \"{quoted}\" WITH (FORCE)"))
                    .execute(admin)
                    .await
                {
                    tracing::warn!(database = %name, error = %error, "stale test-database drop skipped");
                    continue;
                }
                dropped += 1;
            }
            // Fresh marker — still within the disconnect grace window.
            Some(_) => continue,
            // First sighting: mark and move on. The mark is harmless if
            // a live test was caught in the create→connect gap — a
            // comment never blocks its owner.
            None => {
                let marker = format!("{ORPHAN_MARKER_PREFIX}{now}");
                if let Err(error) =
                    sqlx::query(&format!("COMMENT ON DATABASE \"{quoted}\" IS '{marker}'"))
                        .execute(admin)
                        .await
                {
                    tracing::warn!(database = %name, error = %error, "orphan marker write skipped");
                }
            }
        }
    }
    if dropped > 0 {
        tracing::info!(dropped, "stale test databases dropped");
    }
    dropped
}
