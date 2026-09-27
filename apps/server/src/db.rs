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
