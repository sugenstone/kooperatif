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

/// Dev-hygiene garbage collection (PILOT-FIX-001 F11).
///
/// Every integration `setup()` creates a `kooperatif_test_<uuid>`
/// database and never drops it — the shared dev instance accumulated
/// ~1,300 orphaned databases (~14 GB). Each setup calls this on its
/// admin connection so accumulation stays bounded.
///
/// Safety rule: a database is dropped only when ALL of these hold —
/// it matches the exact harness prefix, it is not a template, it has
/// zero active connections, and its on-disk catalog is older than 24 h.
/// A still-running test's database is always younger than the cutoff,
/// so concurrent runs can never drop each other's databases. Any other
/// naming class (`kooperatif`, `kooperatif_e2e_*`, audit fixtures) is
/// never touched here.
pub async fn drop_stale_test_databases(admin: &PgPool) -> u64 {
    let names: Vec<String> = match sqlx::query_scalar(
        "SELECT d.datname FROM pg_database d \
         WHERE d.datname LIKE 'kooperatif\\_test\\_%' ESCAPE '\\' \
           AND NOT d.datistemplate \
           AND NOT EXISTS ( \
               SELECT 1 FROM pg_stat_activity a WHERE a.datname = d.datname \
           ) \
           AND (pg_stat_file('base/' || d.oid || '/PG_VERSION')).modification \
               < now() - interval '24 hours'",
    )
    .fetch_all(admin)
    .await
    {
        Ok(names) => names,
        Err(error) => {
            tracing::warn!(error = %error, "stale test-database scan failed");
            return 0;
        }
    };
    let mut dropped = 0u64;
    for name in names {
        // datname comes from the system catalog, not user input — the
        // identifier is still double-quoted defensively.
        let quoted = name.replace('"', "\"\"");
        if let Err(error) = sqlx::query(&format!("DROP DATABASE \"{quoted}\""))
            .execute(admin)
            .await
        {
            // A database can acquire a connection between the scan and
            // the drop — skip it rather than fail the test setup.
            tracing::warn!(database = %name, error = %error, "stale test-database drop skipped");
            continue;
        }
        dropped += 1;
    }
    if dropped > 0 {
        tracing::info!(dropped, "stale test databases dropped");
    }
    dropped
}
