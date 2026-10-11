//! PostgreSQL connectivity and migrations (STEP-001 infrastructure layer).
//!
//! Migrations live in the repository-root `migrations/` directory and are
//! embedded into the binary at compile time. They are applied by the
//! explicit `migrate` subcommand — production migration is a controlled
//! deployment step, never an implicit side effect of serving (ADR-012).

use std::time::Duration;

use sqlx::postgres::{PgConnection, PgPoolOptions};
use sqlx::{Connection, PgPool};

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

/// Dev-hygiene garbage collection (M1-INFRA-002).
///
/// Every integration `setup()` creates a `kooperatif_test_<uuid>`
/// database; the happy path drops it in `TestApp::cleanup()`, but a
/// panic, assert failure or killed process leaks it.
///
/// Ownership model — owner-maintained heartbeat lease:
/// `create_test_database`/`claim_test_database` write
/// `kooperatif-test-owned:<unix epoch>` into the database's shared
/// catalog comment and spawn a heartbeat task that renews it every
/// `HEARTBEAT_SECS`. The claim survives ANY connection gap because the
/// lease lives in the catalog, not in a session — a paused, idle or
/// temporarily disconnected test keeps renewing it as long as its
/// process lives. `TestDbClaim`'s `Drop` aborts the heartbeat; when the
/// owner process dies (panic, assert, kill), the lease simply stops
/// being renewed.
///
/// GC transitions a disposable database through explicit states, each
/// serialized with the owner heartbeat by `pg_advisory_lock` on a
/// per-name key — marker state is always re-read while holding the
/// lock, so a heartbeat can never interleave between the GC's read and
/// its transition:
///   owned:<fresh>            → live owner, skip
///   owned:<expired>          → demote to gc-pending:<now> (drop NEVER
///                              happens on first suspicion)
///   gc-pending:<fresh>       → suspicion in flight, skip
///   gc-pending:<expired>     → abandoned → plain `DROP DATABASE`
///   no/foreign comment       → first sighting → gc-pending:<now>
///
/// `DROP DATABASE` is deliberately NOT `WITH (FORCE)`: a connection
/// that appears between the scan and the drop makes the engine reject
/// it atomically (55006) — the drop is skipped, the owner unharmed.
/// FORCE would kill that late-arriving connection and is therefore
/// forbidden in automatic GC.
///
/// Absolute guards, unchanged:
/// - exact harness pattern `kooperatif_test_<32 hex>` (regex-enforced,
///   so `kooperatif_test_seed`, `kooperatif_e2e_*`, audit fixtures and
///   every nonmatching name are never even scanned);
/// - `NOT datistemplate`;
/// - zero rows in `pg_stat_activity` at scan AND re-verified under the
///   advisory lock before any transition.
///
/// The `postgres` maintenance database is the GC vantage point;
/// `COMMENT ON DATABASE`/`shobj_description` are shared-catalog
/// operations — no connection to the candidate, no schema touch.
const OWNED_MARKER_PREFIX: &str = "kooperatif-test-owned:";
const ORPHAN_MARKER_PREFIX: &str = "kooperatif-gc-pending:";
/// An `owned` marker this old is treated as a stopped heartbeat — a
/// dead or hopelessly hung owner. ~8 missed beats of margin.
const LEASE_TTL_SECS: i64 = 120;
const HEARTBEAT_SECS: u64 = 15;
/// A `gc-pending` marker must survive this long unrefreshed before the
/// database is considered abandoned (~20× the slowest observed test).
const ORPHAN_GRACE_SECS: i64 = 30 * 60;

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn quoted_ident(name: &str) -> String {
    name.replace('"', "\"\"")
}

/// Advisory-lock key shared by the owner heartbeat and GC transitions.
const GC_LOCK_KEY_SQL: &str = "hashtextextended($1, 0)";

/// Advisory-lock key for the GC leader election — one sweep at a time.
const GC_LEADER_KEY_SQL: &str = "hashtextextended('kooperatif-gc-leader', 0)";

/// Claim a disposable test database: writes the owned marker on a
/// DEDICATED admin connection (not the shared pool — the caller may
/// close it) and spawns the heartbeat task. The returned claim stops
/// the heartbeat when dropped.
async fn claim_inner(admin_url: &str, name: &str, heartbeat: Duration) -> TestDbClaim {
    let mut conn = PgConnection::connect(admin_url)
        .await
        .expect("claim connection");
    write_owned_marker(&mut conn, name)
        .await
        .expect("initial ownership claim");
    let owned_name = name.to_string();
    let task = tokio::spawn(async move {
        let mut tick = tokio::time::interval(heartbeat);
        tick.tick().await; // first tick fires immediately — skip it
        loop {
            tick.tick().await;
            if let Err(error) = write_owned_marker(&mut conn, &owned_name).await {
                tracing::warn!(database = %owned_name, error = %error, "test-database heartbeat stopped");
                break;
            }
        }
    });
    TestDbClaim { task }
}

/// Renew the owner's lease, serialized against GC transitions by the
/// per-name advisory lock on this (admin-database) connection.
async fn write_owned_marker(conn: &mut PgConnection, name: &str) -> Result<(), sqlx::Error> {
    sqlx::query(&format!("SELECT pg_advisory_lock({GC_LOCK_KEY_SQL})"))
        .bind(name)
        .execute(&mut *conn)
        .await?;
    let marker = format!("{OWNED_MARKER_PREFIX}{}", unix_now());
    let result = sqlx::query(&format!(
        "COMMENT ON DATABASE \"{}\" IS '{marker}'",
        quoted_ident(name)
    ))
    .execute(&mut *conn)
    .await;
    let _ = sqlx::query(&format!("SELECT pg_advisory_unlock({GC_LOCK_KEY_SQL})"))
        .bind(name)
        .execute(&mut *conn)
        .await;
    result?;
    Ok(())
}

/// RAII handle over the heartbeat task. `drop` (or `stop`) aborts it —
/// the lease then expires naturally and GC can reclaim the database
/// only after the full demote→pending→grace path.
pub struct TestDbClaim {
    task: tokio::task::JoinHandle<()>,
}

impl TestDbClaim {
    /// Explicit release — identical to `drop`, kept for readability.
    pub fn stop(self) {}
}

impl Drop for TestDbClaim {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Claim an already-created disposable test database (test fixtures).
pub async fn claim_test_database(admin_url: &str, database_name: &str) -> TestDbClaim {
    claim_inner(
        admin_url,
        database_name,
        Duration::from_secs(HEARTBEAT_SECS),
    )
    .await
}

/// Claim with a caller-chosen heartbeat interval — test-only speedup.
#[doc(hidden)]
pub async fn claim_test_database_with_heartbeat(
    admin_url: &str,
    database_name: &str,
    heartbeat: Duration,
) -> TestDbClaim {
    claim_inner(admin_url, database_name, heartbeat).await
}

/// Create a disposable test database AND claim it — the create→claim
/// window is a single helper call, and a racing GC pass can at worst
/// stamp a `gc-pending` marker that the claim immediately overwrites.
pub async fn create_test_database(
    admin: &PgPool,
    admin_url: &str,
    database_name: &str,
) -> TestDbClaim {
    sqlx::query(&format!(
        "CREATE DATABASE \"{}\"",
        quoted_ident(database_name)
    ))
    .execute(admin)
    .await
    .expect("test database created");
    claim_test_database(admin_url, database_name).await
}

enum GcState {
    /// `owned` marker still within the lease TTL — live owner.
    OwnedFresh,
    /// `owned` marker past the TTL — heartbeat stopped → demote.
    OwnedStale,
    /// `gc-pending` inside the grace window.
    PendingFresh,
    /// `gc-pending` past the grace window — abandoned → drop.
    PendingStale,
    /// No marker (or a foreign comment) — first sighting → mark.
    Unmarked,
}

fn gc_classify(comment: Option<&str>, now: i64) -> GcState {
    let marker_ts = |prefix: &str| {
        comment
            .and_then(|c| c.strip_prefix(prefix))
            .and_then(|ts| ts.parse::<i64>().ok())
    };
    if let Some(ts) = marker_ts(OWNED_MARKER_PREFIX) {
        return if now - ts < LEASE_TTL_SECS {
            GcState::OwnedFresh
        } else {
            GcState::OwnedStale
        };
    }
    if let Some(ts) = marker_ts(ORPHAN_MARKER_PREFIX) {
        return if now - ts < ORPHAN_GRACE_SECS {
            GcState::PendingFresh
        } else {
            GcState::PendingStale
        };
    }
    GcState::Unmarked
}

/// Deterministic test seam for the scan→drop race: with a gate
/// supplied, the GC pauses immediately before issuing a real
/// `DROP DATABASE` — but only for the named candidate, so unrelated
/// stale databases are never blocked — letting the test open a
/// connection to prove the engine-level rejection keeps it alive.
#[doc(hidden)]
pub struct DropGate {
    pub database: String,
    pub pause: tokio::sync::Barrier,
    pub proceed: tokio::sync::Barrier,
}

impl DropGate {
    pub fn for_database(name: &str) -> Self {
        Self {
            database: name.to_string(),
            pause: tokio::sync::Barrier::new(2),
            proceed: tokio::sync::Barrier::new(2),
        }
    }
}

/// Best-effort sweep entry point (test setups). Leader-elected: only
/// one sweep runs at a time across ALL callers/processes; losers
/// return immediately. Sweeps are idempotent — a lost pass costs
/// nothing because the winner (or the next caller) does the work.
pub async fn drop_stale_test_databases(admin: &PgPool) -> u64 {
    drop_stale_test_databases_impl(admin, None, true).await
}

/// Test-only variant that bypasses leader election — the GC behavior
/// tests need a pass that deterministically executes instead of
/// deferring to whichever sweeper won leadership.
#[doc(hidden)]
pub async fn drop_stale_test_databases_forced(admin: &PgPool) -> u64 {
    drop_stale_test_databases_impl(admin, None, false).await
}

#[doc(hidden)]
pub async fn drop_stale_test_databases_gated(admin: &PgPool, gate: Option<&DropGate>) -> u64 {
    drop_stale_test_databases_impl(admin, gate, false).await
}

async fn drop_stale_test_databases_impl(
    admin: &PgPool,
    gate: Option<&DropGate>,
    elect_leader: bool,
) -> u64 {
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
    let mut names: Vec<String> = match sqlx::query_scalar(
        "SELECT d.datname FROM pg_database d \
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
    // A gated pass exists to reach its one probed candidate promptly —
    // process it first so the sweep isn't O(candidates) under load.
    if let Some(gate) = gate {
        if let Some(index) = names.iter().position(|name| *name == gate.database) {
            names.swap(0, index);
        }
    }
    // Advisory locks are session-scoped — pin ONE connection so lock,
    // re-check, transition and unlock run on the same backend.
    let mut conn = match admin.acquire().await {
        Ok(conn) => conn,
        Err(error) => {
            tracing::warn!(error = %error, "stale test-database connection failed");
            return 0;
        }
    };
    // Leader election: GC sweeps are idempotent, so duplicate
    // concurrent passes only multiply cost — every candidate DROP
    // queues on the single checkpointer while holding the candidate's
    // advisory lock, starving the other sweepers' try-locks
    // (checkpoint convoy). One sweeper at a time; losers do nothing
    // and the next caller retries.
    if elect_leader {
        let won: bool =
            sqlx::query_scalar(&format!("SELECT pg_try_advisory_lock({GC_LEADER_KEY_SQL})"))
                .fetch_one(&mut *conn)
                .await
                .unwrap_or(false);
        if !won {
            return 0;
        }
    }
    let mut dropped = 0u64;
    for name in names {
        let locked: bool =
            match sqlx::query_scalar(&format!("SELECT pg_try_advisory_lock({GC_LOCK_KEY_SQL})"))
                .bind(&name)
                .fetch_one(&mut *conn)
                .await
            {
                Ok(locked) => locked,
                Err(error) => {
                    tracing::warn!(error = %error, "GC lock attempt failed");
                    if elect_leader {
                        let _ =
                            sqlx::query(&format!("SELECT pg_advisory_unlock({GC_LEADER_KEY_SQL})"))
                                .execute(&mut *conn)
                                .await;
                    }
                    return dropped;
                }
            };
        if !locked {
            // Owner mid-heartbeat — skip this pass, next pass retries.
            continue;
        }
        let outcome = gc_transition_locked(&mut conn, &name, now, gate).await;
        let _ = sqlx::query(&format!("SELECT pg_advisory_unlock({GC_LOCK_KEY_SQL})"))
            .bind(&name)
            .execute(&mut *conn)
            .await;
        if matches!(outcome, GcOutcome::Dropped) {
            dropped += 1;
        }
    }
    if elect_leader {
        let _ = sqlx::query(&format!("SELECT pg_advisory_unlock({GC_LEADER_KEY_SQL})"))
            .execute(&mut *conn)
            .await;
    }
    if dropped > 0 {
        tracing::info!(dropped, "stale test databases dropped");
    }
    dropped
}

enum GcOutcome {
    Skipped,
    Dropped,
}

/// The serialized per-database decision — everything the GC does to a
/// candidate happens while holding its advisory lock, so a heartbeat
/// can never interleave between the state read and the transition.
async fn gc_transition_locked(
    conn: &mut PgConnection,
    name: &str,
    now: i64,
    gate: Option<&DropGate>,
) -> GcOutcome {
    // Re-verify zero connections under the lock — the outer scan is
    // already stale by definition.
    let has_conn: bool = match sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname = $1)",
    )
    .bind(name)
    .fetch_one(&mut *conn)
    .await
    {
        Ok(v) => v,
        Err(_) => return GcOutcome::Skipped,
    };
    if has_conn {
        return GcOutcome::Skipped;
    }
    let comment: Option<String> = match sqlx::query_scalar(
        "SELECT shobj_description(oid, 'pg_database') FROM pg_database WHERE datname = $1",
    )
    .bind(name)
    .fetch_one(&mut *conn)
    .await
    {
        Ok(v) => v,
        Err(_) => return GcOutcome::Skipped,
    };
    let quoted = quoted_ident(name);
    match gc_classify(comment.as_deref(), now) {
        GcState::OwnedFresh | GcState::PendingFresh => GcOutcome::Skipped,
        GcState::OwnedStale | GcState::Unmarked => {
            let marker = format!("{ORPHAN_MARKER_PREFIX}{now}");
            let _ = sqlx::query(&format!("COMMENT ON DATABASE \"{quoted}\" IS '{marker}'"))
                .execute(&mut *conn)
                .await;
            GcOutcome::Skipped
        }
        GcState::PendingStale => {
            if let Some(gate) = gate {
                if gate.database == name {
                    gate.pause.wait().await;
                    gate.proceed.wait().await;
                }
            }
            // Plain DROP — the engine atomically rejects (55006) if a
            // connection landed between the re-check and now.
            match sqlx::query(&format!("DROP DATABASE \"{quoted}\""))
                .execute(&mut *conn)
                .await
            {
                Ok(_) => GcOutcome::Dropped,
                Err(error) => {
                    tracing::warn!(database = %name, error = %error, "stale test-database drop skipped");
                    GcOutcome::Skipped
                }
            }
        }
    }
}
