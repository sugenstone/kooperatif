//! PostgreSQL connectivity and migrations (STEP-001 infrastructure layer).
//!
//! Migrations live in the repository-root `migrations/` directory and are
//! embedded into the binary at compile time. They are applied by the
//! explicit `migrate` subcommand — production migration is a controlled
//! deployment step, never an implicit side effect of serving (ADR-012).

use std::fmt;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use sqlx::postgres::{PgConnection, PgPoolOptions};
use sqlx::{Connection, Executor, PgPool};
use uuid::Uuid;

/// Connection pool defaults sized for the STEP-001 baseline.
const MAX_CONNECTIONS: u32 = 5;
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(3);

/// Custom GUC carrying the verified cooperative scope inside a business
/// transaction (M1-P4-P1). Read by RLS policies (P3) via
/// `current_setting('app.cooperative_id', true)`; absent/empty must
/// fail closed. Never session-persistent: `set_config(…, true)` scopes
/// it to the transaction, and `before_acquire` wipes any stray
/// session-level residue before a pooled connection is reused.
pub const GUC_COOPERATIVE: &str = "app.cooperative_id";
/// Companion GUC for the authenticated user — the pre-context
/// membership-resolution lane and cooperative-attributed audit writes.
pub const GUC_USER: &str = "app.user_id";

pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        .acquire_timeout(ACQUIRE_TIMEOUT)
        // M1-P4-P1/P2A: a pooled connection must never carry a previous
        // borrower's tenant context OR impersonated role. Wipe tenant
        // GUCs and any SET ROLE residue on checkout — tx-local SET LOCAL
        // cannot leak by construction, so this only removes pathological
        // session-level residue.
        .before_acquire(|conn, _meta| {
            Box::pin(async move {
                // Simple-protocol raw_sql: the extended protocol rejects
                // multi-statement strings ("cannot insert multiple
                // commands into a prepared statement").
                conn.execute(sqlx::raw_sql(
                    "RESET app.cooperative_id; RESET app.user_id; \
                     RESET app.session_id; RESET app.ctx_txid; \
                     RESET app.user_seal; RESET app.ctx_seal; \
                     RESET ROLE",
                ))
                .await?;
                Ok(true)
            })
        })
        .connect(database_url)
        .await
}

/// Boxed future alias for `DbSource` — avoids a `futures` dependency.
type DbFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = sqlx::Result<T>> + Send + 'a>>;

/// A checked-out database handle owned or borrowed by a repository
/// function (M1-P4-P2B). Derefs to `PgConnection`, so it can be used
/// directly as an executor (`&mut *conn`) or reborrowed into nested
/// helper calls.
pub enum DbGuard<'a> {
    /// Freshly acquired pool connection (`&PgPool` source).
    Owned(sqlx::pool::PoolConnection<sqlx::Postgres>),
    /// Reborrowed connection — inside a request transaction, a savepoint
    /// connection or a caller-owned handle.
    Borrowed(&'a mut PgConnection),
}

impl std::ops::Deref for DbGuard<'_> {
    type Target = PgConnection;
    fn deref(&self) -> &PgConnection {
        match self {
            Self::Owned(conn) => conn,
            Self::Borrowed(conn) => conn,
        }
    }
}

impl std::ops::DerefMut for DbGuard<'_> {
    fn deref_mut(&mut self) -> &mut PgConnection {
        match self {
            Self::Owned(conn) => conn,
            Self::Borrowed(conn) => conn,
        }
    }
}

/// A database execution source for repository functions (M1-P4-P2B).
///
/// `sqlx::Acquire` cannot be used here: its `&'c mut T: Acquire<'c>`
/// impls are lifetime-diagonal, so a `&mut Transaction` or
/// `&mut PgConnection` argument produces "implementation is not general
/// enough" when the caller's async future is checked for `Send` by the
/// Axum `Handler` bound. `DbSource` uses non-diagonal impls, so any of
/// `&PgPool` (tests, platform lanes), `&mut PgConnection`,
/// `&mut DbGuard` or `&mut Transaction` (the request-scoped verified
/// context) can be passed to a repository function inside a
/// `Send`-checked future.
///
/// `begin` preserves nested-transaction semantics: on a `&PgPool` it is
/// a real transaction; on a borrowed connection or live transaction it
/// becomes a savepoint, exactly matching `Acquire::begin`.
pub trait DbSource: Send {
    /// Borrow a connection for direct queries.
    fn conn(&mut self) -> DbFuture<'_, DbGuard<'_>>;
    /// Begin a transaction (savepoint when the source is already inside
    /// one).
    fn begin(&mut self) -> DbFuture<'_, sqlx::Transaction<'_, sqlx::Postgres>>;
}

impl DbSource for &PgPool {
    fn conn(&mut self) -> DbFuture<'_, DbGuard<'_>> {
        Box::pin(async move { Ok(DbGuard::Owned((*self).acquire().await?)) })
    }
    fn begin(&mut self) -> DbFuture<'_, sqlx::Transaction<'_, sqlx::Postgres>> {
        Box::pin(async move { (*self).begin().await })
    }
}

impl DbSource for &mut PgConnection {
    fn conn(&mut self) -> DbFuture<'_, DbGuard<'_>> {
        Box::pin(async move { Ok(DbGuard::Borrowed(self)) })
    }
    fn begin(&mut self) -> DbFuture<'_, sqlx::Transaction<'_, sqlx::Postgres>> {
        Box::pin(async move { sqlx::Acquire::begin(&mut **self).await })
    }
}

impl DbSource for &mut sqlx::Transaction<'_, sqlx::Postgres> {
    fn conn(&mut self) -> DbFuture<'_, DbGuard<'_>> {
        Box::pin(async move { Ok(DbGuard::Borrowed(self)) })
    }
    fn begin(&mut self) -> DbFuture<'_, sqlx::Transaction<'_, sqlx::Postgres>> {
        Box::pin(async move { sqlx::Acquire::begin(&mut **self).await })
    }
}

impl DbSource for &mut DbGuard<'_> {
    fn conn(&mut self) -> DbFuture<'_, DbGuard<'_>> {
        Box::pin(async move { Ok(DbGuard::Borrowed(self)) })
    }
    fn begin(&mut self) -> DbFuture<'_, sqlx::Transaction<'_, sqlx::Postgres>> {
        Box::pin(async move { sqlx::Acquire::begin(&mut ***self).await })
    }
}

impl DbSource for &mut sqlx::pool::PoolConnection<sqlx::Postgres> {
    fn conn(&mut self) -> DbFuture<'_, DbGuard<'_>> {
        Box::pin(async move { Ok(DbGuard::Borrowed(self)) })
    }
    fn begin(&mut self) -> DbFuture<'_, sqlx::Transaction<'_, sqlx::Postgres>> {
        Box::pin(async move { sqlx::Acquire::begin(&mut **self).await })
    }
}

/// Open a transaction stamped with the verified tenant context. Every
/// cooperative-scoped business operation must run inside this envelope
/// once RLS lands; today it is behavior-identical plumbing.
pub async fn tenant_tx(
    pool: &PgPool,
    cooperative_id: Option<Uuid>,
    user_id: Option<Uuid>,
) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "SELECT set_config('app.cooperative_id', $1::text, true), \
                set_config('app.user_id', $2::text, true)",
    )
    .bind(cooperative_id.map(|id| id.to_string()))
    .bind(user_id.map(|id| id.to_string()))
    .execute(&mut *tx)
    .await?;
    Ok(tx)
}

/// Verified request context returned by `request_tx` (M1-P4-P2B).
#[derive(Debug, Clone)]
pub struct RequestContext {
    /// User id DERIVED inside the database from the session row —
    /// never a caller-supplied value.
    pub user_id: Uuid,
    pub cooperative_id: Uuid,
    pub cooperative_name: String,
}

/// Why `request_tx`/`request_user_tx` refused to open a context.
#[derive(Debug)]
pub enum RequestContextError {
    /// Session token invalid, expired, revoked or user disabled.
    Unauthenticated,
    /// No active membership in an active cooperative.
    NotMember,
    /// Membership exists but the cooperative's business gate is closed.
    NotReady,
    /// Database failure during context establishment.
    Db(sqlx::Error),
}

impl fmt::Display for RequestContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthenticated => write!(f, "request context: unauthenticated"),
            Self::NotMember => write!(f, "request context: not a member"),
            Self::NotReady => write!(f, "request context: cooperative not ready"),
            Self::Db(e) => write!(f, "request context db error: {e}"),
        }
    }
}

impl std::error::Error for RequestContextError {}

/// Open a business transaction with a fully database-verified tenant
/// context (M1-P4-P2B):
///
///   1. `app_open_request_context(token_hash)` validates the session
///      row itself and stamps `app.user_id` tx-locally — the caller
///      supplies the token hash (the credential), never a user UUID.
///   2. `app_verify_membership` resolves the membership — its
///      caller-binding guard (0033) accepts only the stamped user.
///   3. Only after membership is proven is `app.cooperative_id`
///      stamped — a valid session can never hop into a foreign coop.
///
/// The transaction rolls back (drop) on any refusal.
pub async fn request_tx(
    pool: &PgPool,
    session_token_hash: &[u8],
    cooperative_id: Uuid,
    now: time::OffsetDateTime,
) -> Result<(sqlx::Transaction<'static, sqlx::Postgres>, RequestContext), RequestContextError> {
    request_tx_isolated(pool, session_token_hash, cooperative_id, now, false).await
}

/// Repeatable-read variant: `SET TRANSACTION` must be the first
/// statement of a transaction, so the isolation level is raised BEFORE
/// the context-stamping queries run. Used by report snapshots that must
/// observe one committed instant across multiple statements.
pub async fn request_tx_repeatable(
    pool: &PgPool,
    session_token_hash: &[u8],
    cooperative_id: Uuid,
    now: time::OffsetDateTime,
) -> Result<(sqlx::Transaction<'static, sqlx::Postgres>, RequestContext), RequestContextError> {
    request_tx_isolated(pool, session_token_hash, cooperative_id, now, true).await
}

async fn request_tx_isolated(
    pool: &PgPool,
    session_token_hash: &[u8],
    cooperative_id: Uuid,
    now: time::OffsetDateTime,
    repeatable_read: bool,
) -> Result<(sqlx::Transaction<'static, sqlx::Postgres>, RequestContext), RequestContextError> {
    let mut tx = pool.begin().await.map_err(RequestContextError::Db)?;

    if repeatable_read {
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await
            .map_err(RequestContextError::Db)?;
    }

    let user_id: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_open_request_context($1, $2)")
            .bind(session_token_hash)
            .bind(now)
            .fetch_one(&mut *tx)
            .await
            .map_err(RequestContextError::Db)?;
    let Some(user_id) = user_id else {
        return Err(RequestContextError::Unauthenticated);
    };

    // Seal-checked cooperative stamping (M1-P4-P3B): the definer
    // verifies the user seal and transaction freshness minted by
    // app_open_request_context, resolves membership, and only then
    // stamps app.cooperative_id plus the HMAC ctx_seal the pilot RLS
    // policies require.
    let row: Option<(String, String, String, bool)> = sqlx::query_as(
        "SELECT membership_status, cooperative_name, cooperative_status, \
                business_enabled \
           FROM public.app_stamp_cooperative($1)",
    )
    .bind(cooperative_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(RequestContextError::Db)?;

    let Some((membership_status, cooperative_name, cooperative_status, business_enabled)) = row
    else {
        return Err(RequestContextError::NotMember);
    };
    if membership_status != "active" || cooperative_status != "active" {
        return Err(RequestContextError::NotMember);
    }
    if !business_enabled {
        return Err(RequestContextError::NotReady);
    }

    Ok((
        tx,
        RequestContext {
            user_id,
            cooperative_id,
            cooperative_name,
        },
    ))
}

/// User-scoped context WITHOUT a cooperative — the NeverBootstrapped
/// legacy lane and pre-cooperative endpoints. Stamps only
/// `app.user_id` derived from the session row.
pub async fn request_user_tx(
    pool: &PgPool,
    session_token_hash: &[u8],
    now: time::OffsetDateTime,
) -> Result<(sqlx::Transaction<'static, sqlx::Postgres>, Uuid), RequestContextError> {
    request_user_tx_isolated(pool, session_token_hash, now, false).await
}

/// Repeatable-read variant of `request_user_tx` — see
/// `request_tx_repeatable` for why the isolation statement must run
/// before any context query.
pub async fn request_user_tx_repeatable(
    pool: &PgPool,
    session_token_hash: &[u8],
    now: time::OffsetDateTime,
) -> Result<(sqlx::Transaction<'static, sqlx::Postgres>, Uuid), RequestContextError> {
    request_user_tx_isolated(pool, session_token_hash, now, true).await
}

async fn request_user_tx_isolated(
    pool: &PgPool,
    session_token_hash: &[u8],
    now: time::OffsetDateTime,
    repeatable_read: bool,
) -> Result<(sqlx::Transaction<'static, sqlx::Postgres>, Uuid), RequestContextError> {
    let mut tx = pool.begin().await.map_err(RequestContextError::Db)?;
    if repeatable_read {
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await
            .map_err(RequestContextError::Db)?;
    }
    let user_id: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_open_request_context($1, $2)")
            .bind(session_token_hash)
            .bind(now)
            .fetch_one(&mut *tx)
            .await
            .map_err(RequestContextError::Db)?;
    user_id
        .map(|uid| (tx, uid))
        .ok_or(RequestContextError::Unauthenticated)
}

/// Extract the stable domain token of an R3C package-definer error
/// (migrations/0056): those functions raise `P0001`/`P0002` with a
/// `kooperatif:<token>` message so repositories can map them onto
/// their existing typed error variants without stringly-typed drift.
/// Returns `None` for every non-package error.
pub fn package_error_token(error: &sqlx::Error) -> Option<&str> {
    match error {
        sqlx::Error::Database(dbe)
            if matches!(dbe.code().as_deref(), Some("P0001") | Some("P0002")) =>
        {
            dbe.message().strip_prefix("kooperatif:")
        }
        _ => None,
    }
}

/// Read the tenant GUC on an executor — tests and P2 instrumentation.
pub async fn current_tenant_guc<'e, E>(executor: E) -> Result<Option<String>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query_scalar("SELECT current_setting('app.cooperative_id', true)")
        .fetch_one(executor)
        .await
}

/// The PostgreSQL principal a pool is actually connected as (M1-P4-P2A).
///
/// `privileged` is true when the role could defeat intended row-level
/// boundaries — SUPERUSER, BYPASSRLS, catalog-level escalation
/// (CREATEROLE/CREATEDB), or ownership of any public business table
/// (the owner can FORCE/disable RLS at will). Serving on such a
/// principal is only acceptable through the explicit
/// `KOOPERATIF_ALLOW_ADMIN_RUNTIME` development opt-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePrincipal {
    /// `current_user` at connection time.
    pub name: String,
    /// Role attribute rollup that can bypass RLS/policy enforcement.
    pub privileged: bool,
}

/// Inspect the connected role's flags and table ownership. The check is
/// evaluated against `current_user`, so it also reflects `SET ROLE`.
pub async fn inspect_runtime_principal(pool: &PgPool) -> Result<RuntimePrincipal, sqlx::Error> {
    let row: Option<(String, bool, bool, bool, bool, bool)> = sqlx::query_as(
        "SELECT r.rolname,
                r.rolsuper,
                r.rolbypassrls,
                r.rolcreaterole,
                r.rolcreatedb,
                EXISTS (
                    SELECT 1
                      FROM pg_class c
                      JOIN pg_namespace n ON n.oid = c.relnamespace
                     WHERE n.nspname = 'public'
                       AND c.relkind = 'r'
                       AND c.relowner = r.oid
                ) AS owns_tables
           FROM pg_roles r
          WHERE r.rolname = current_user",
    )
    .fetch_optional(pool)
    .await?;
    let Some((name, rolsuper, bypassrls, createrole, createdb, owns_tables)) = row else {
        return Err(sqlx::Error::RowNotFound);
    };
    Ok(RuntimePrincipal {
        name,
        privileged: rolsuper || bypassrls || createrole || createdb || owns_tables,
    })
}

/// Apply all pending migrations. Safe to re-run: applied migrations are
/// tracked in `_sqlx_migrations` and never re-applied.
pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("../../migrations").run(pool).await
}

/// Database-identity guard failure (M1-P2 B1, `KOOPERATIF_EXPECTED_DATABASE`).
#[derive(Debug)]
pub enum DatabaseIdentityError {
    /// `current_database()` on the pool does not match the expected name.
    Mismatch { expected: String, actual: String },
    /// The identity probe itself failed.
    Query(sqlx::Error),
}

impl fmt::Display for DatabaseIdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mismatch { expected, actual } => write!(
                f,
                "database identity mismatch: connected to '{actual}', expected '{expected}' — refusing to continue"
            ),
            Self::Query(error) => write!(f, "database identity probe failed: {error}"),
        }
    }
}

impl std::error::Error for DatabaseIdentityError {}

/// M1-P2-C5-2 — a unique violation raised by an idempotency unique
/// index, whether the scoped `*_coop_idempotency_uq` composite or the
/// still-preserved global `*_idempotency_key_key`.
///
/// Cooperative-scoped inserts use `ON CONFLICT (cooperative_id,
/// idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING` as
/// the arbiter. That clause only swallows conflicts detected on the
/// scoped index; a duplicate that lands on the preserved GLOBAL index —
/// e.g. a scoped row colliding with a legacy NULL-cooperative row's
/// key — surfaces as a plain unique-violation error. Callers map that
/// error to the same "not inserted → scoped re-read" path so replay /
/// conflict semantics stay identical across all lanes.
pub fn is_idempotency_unique_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db)
        if db.is_unique_violation()
            && db.constraint().is_some_and(|c| c.contains("idempotency")))
}

/// Fail-closed identity check for mutating CLI paths and server startup.
///
/// `SELECT current_database()` runs on a connection checked out of THIS
/// pool. The pool is bound to exactly one connection URL, so every
/// checkout — and every subsequent write through the pool — can only
/// target that database; the probe therefore covers the whole pool and
/// cannot be bypassed by a second pool inside this process.
pub async fn verify_expected_database(
    pool: &PgPool,
    expected: &str,
) -> Result<(), DatabaseIdentityError> {
    let actual: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(pool)
        .await
        .map_err(DatabaseIdentityError::Query)?;
    if actual == expected {
        Ok(())
    } else {
        Err(DatabaseIdentityError::Mismatch {
            expected: expected.to_string(),
            actual,
        })
    }
}

/// Test-database identity policy (M1-P2 Phase B).
///
/// Integration tests may ONLY ever connect to, migrate, create, claim or
/// drop databases matching the disposable convention — the allowlist
/// shape `kooperatif_test` (the standing direct-connect database used by
/// harnesses like `auth.rs`) or `kooperatif_test_<32 lowercase hex>` —
/// which inherently rejects every protected name: the historical
/// `kooperatif` main database, `kooperatif_dev`, `*_shadow*` clones,
/// `kooperatif_backup_verify`, `kooperatif_e2e*`, audit/gc-keep keeps,
/// and anything else an operator or stale env var might point at.
///
/// The check is fail-closed: `verify_test_database_identity` probes
/// `current_database()` on the actual pool, and `run_test_migrations`
/// invokes it BEFORE the migrator — a wrong target is refused before a
/// single migration statement is sent.
fn is_hex32_suffix(suffix: &str) -> bool {
    suffix.len() == 32
        && suffix
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn is_test_database_name(name: &str) -> bool {
    if name == "kooperatif_test" {
        return true;
    }
    name.strip_prefix("kooperatif_test_")
        .is_some_and(is_hex32_suffix)
}

/// Panic-level assertion for database names the harness itself creates,
/// claims or drops: disposable shape only (`kooperatif_test_<32 hex>`).
/// The standing `kooperatif_test` is deliberately excluded — it is never
/// created or dropped by the harness.
pub fn assert_disposable_test_database_name(name: &str) {
    assert!(
        name.strip_prefix("kooperatif_test_")
            .is_some_and(is_hex32_suffix),
        "refusing to create/claim/drop non-disposable database name '{name}' — \
         disposable test databases must be kooperatif_test_<32 lowercase hex>"
    );
}

/// Restricted-principal password test harnesses embed in the derived
/// `kooperatif_app` connection URL. Defaults to the documented
/// local-development password so developer runs are unchanged; CI
/// overrides it via `KOOPERATIF_APP_TEST_PASSWORD` after provisioning
/// the role's password in its own ephemeral cluster. The value is only
/// ever placed on the connection string — it is never logged.
pub fn test_app_role_password() -> String {
    std::env::var("KOOPERATIF_APP_TEST_PASSWORD")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "kooperatif_app_dev_password".to_string())
}

/// Fail-closed identity probe for test harnesses: the pool's database
/// must satisfy the disposable-test allowlist. Any other target —
/// including the historical `kooperatif` database, shadow databases and
/// the backup-verification database — is refused BEFORE any migration,
/// fixture mutation or cleanup statement runs.
pub async fn verify_test_database_identity(pool: &PgPool) -> Result<(), DatabaseIdentityError> {
    let actual: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(pool)
        .await
        .map_err(DatabaseIdentityError::Query)?;
    if is_test_database_name(&actual) {
        Ok(())
    } else {
        Err(DatabaseIdentityError::Mismatch {
            expected: "kooperatif_test or kooperatif_test_<hex> (disposable test database)"
                .to_string(),
            actual,
        })
    }
}

/// Failure of the guarded test-migration path.
#[derive(Debug)]
pub enum TestMigrationError {
    /// The pool is not bound to an allowed test database.
    Identity(DatabaseIdentityError),
    /// The migrator itself failed.
    Migrate(sqlx::migrate::MigrateError),
}

impl fmt::Display for TestMigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Identity(e) => write!(f, "test database identity refused: {e}"),
            Self::Migrate(e) => write!(f, "migration failed: {e}"),
        }
    }
}

impl std::error::Error for TestMigrationError {}

/// The ONLY migration entry point test suites may use: identity probe
/// first, migrator second. Production keeps using `run_migrations`
/// (guarded instead by `verify_expected_database` at startup).
pub async fn run_test_migrations(pool: &PgPool) -> Result<(), TestMigrationError> {
    verify_test_database_identity(pool)
        .await
        .map_err(TestMigrationError::Identity)?;
    run_migrations(pool)
        .await
        .map_err(TestMigrationError::Migrate)
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
///   owned:<expired>          → demote to gc-pending:<now> (deletion
///                              NEVER happens on first suspicion)
///   gc-pending:<fresh>       → suspicion in flight, skip
///   gc-pending:<expired>     → abandoned → retained; a plain
///                              `DROP DATABASE` is issued ONLY on the
///                              test-only authorized path AND only for
///                              names registered as current-run-owned
///                              by this process
///   no/foreign comment       → first sighting → gc-pending:<now>
///
/// DELETION POLICY: no automatic sweep can delete databases — a stale
/// marker only proves a PRIOR run created the database, outside the
/// current-run teardown boundary. There is intentionally NO
/// environment variable or flag that enables historical cleanup from
/// an ordinary test run; historical cleanup is a separate operator
/// procedure that reviews the exact target inventory first (docs/31).
///
/// On the authorized test-only path, `DROP DATABASE` is deliberately
/// NOT `WITH (FORCE)`: a connection that appears between the scan and
/// the drop makes the engine reject it atomically (55006) — the drop
/// is skipped, the owner unharmed. FORCE would kill that late-arriving
/// connection and is therefore forbidden in automatic GC.
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
    // Fail closed: the claim/heartbeat machinery may only ever stamp
    // disposable-convention databases — never a protected target.
    assert_disposable_test_database_name(name);
    register_current_run_database(name);
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
///
/// Fast path (test-infrastructure optimization): when a migrated
/// template cache exists, the database is created as a filesystem-level
/// `CREATE DATABASE … TEMPLATE` clone instead of an empty database that
/// every test then re-migrates from scratch. Each test still receives a
/// private, disposable database — isolation is unchanged; only the
/// one-time migration replay per migration-set is amortized. Opt out
/// with `KOOPERATIF_TEST_TEMPLATE=0`.
pub async fn create_test_database(
    admin: &PgPool,
    admin_url: &str,
    database_name: &str,
) -> TestDbClaim {
    // Fail closed BEFORE CREATE: only the disposable convention may be
    // created — protected names (kooperatif, *_shadow*, backup_verify,
    // e2e, audit, dev, gc-keep, ...) can never reach CREATE or the claim.
    assert_disposable_test_database_name(database_name);
    let mut created = false;
    if test_template_enabled() {
        if let Some(template) = ensure_test_template(admin_url).await {
            match sqlx::query(&format!(
                "CREATE DATABASE \"{}\" TEMPLATE \"{}\"",
                quoted_ident(database_name),
                quoted_ident(&template)
            ))
            .execute(admin)
            .await
            {
                Ok(_) => created = true,
                Err(error) => {
                    tracing::warn!(error = %error, "template clone failed; falling back to empty CREATE DATABASE")
                }
            }
        }
    }
    if !created {
        sqlx::query(&format!(
            "CREATE DATABASE \"{}\"",
            quoted_ident(database_name)
        ))
        .execute(admin)
        .await
        .expect("test database created");
    }
    claim_test_database(admin_url, database_name).await
}

/// `KOOPERATIF_TEST_TEMPLATE=0` (or false/off) disables the template
/// fast path entirely — useful when bisecting harness behavior.
fn test_template_enabled() -> bool {
    !matches!(
        std::env::var("KOOPERATIF_TEST_TEMPLATE")
            .unwrap_or_default()
            .as_str(),
        "0" | "false" | "off"
    )
}

/// Name of the shared migrated-template cache database. The name is
/// keyed by a hash of every embedded migration's (version, checksum),
/// so a changed migration set transparently yields a new template. The
/// `kooperatif_tpl_*` prefix never matches the disposable
/// `kooperatif_test_<32 hex>` convention, so GC and teardown never scan
/// or drop these caches — they are deliberately long-lived and bounded
/// (one per migration-set revision).
fn test_template_name() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    for migration in sqlx::migrate!("../../migrations").migrations.iter() {
        migration.version.hash(&mut hasher);
        migration.checksum.hash(&mut hasher);
    }
    format!("kooperatif_tpl_{:016x}", hasher.finish())
}

/// Ensure the migrated-template cache exists, returning its name.
/// Serialized by a `pg_advisory_lock` so only one process builds it —
/// concurrent callers wait, then observe the finished template. Any
/// failure returns `None` and the caller falls back to a plain
/// `CREATE DATABASE` + full migration replay.
async fn ensure_test_template(admin_url: &str) -> Option<String> {
    let template = test_template_name();
    let mut conn = PgConnection::connect(admin_url).await.ok()?;
    let lock_key = format!("kooperatif-test-template:{template}");
    sqlx::query(&format!("SELECT pg_advisory_lock({GC_LOCK_KEY_SQL})"))
        .bind(&lock_key)
        .execute(&mut conn)
        .await
        .ok()?;
    let result = ensure_test_template_locked(&mut conn, admin_url, &template).await;
    let _ = sqlx::query(&format!("SELECT pg_advisory_unlock({GC_LOCK_KEY_SQL})"))
        .bind(&lock_key)
        .execute(&mut conn)
        .await;
    result
}

async fn ensure_test_template_locked(
    conn: &mut PgConnection,
    admin_url: &str,
    template: &str,
) -> Option<String> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(template)
            .fetch_one(&mut *conn)
            .await
            .ok()?;
    if exists {
        return Some(template.to_string());
    }
    sqlx::query(&format!("CREATE DATABASE \"{}\"", quoted_ident(template)))
        .execute(&mut *conn)
        .await
        .ok()?;
    let template_url = admin_url
        .rsplit_once('/')
        .map(|(base, _)| format!("{base}/{template}"))?;
    let pool = connect(&template_url).await.ok()?;
    let migrated = run_migrations(&pool).await;
    pool.close().await;
    migrated.ok()?;
    // The COMMENT documents the cache's purpose; `IS_TEMPLATE` is the
    // conventional PostgreSQL marker for clone sources.
    sqlx::query(&format!(
        "COMMENT ON DATABASE \"{}\" IS 'kooperatif-test-template (migrated schema cache)'",
        quoted_ident(template)
    ))
    .execute(&mut *conn)
    .await
    .ok()?;
    sqlx::query(&format!(
        "ALTER DATABASE \"{}\" WITH IS_TEMPLATE true",
        quoted_ident(template)
    ))
    .execute(&mut *conn)
    .await
    .ok()?;
    Some(template.to_string())
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

/// Per-process throttle for elected sweeps: every test `setup()` calls
/// this entry point, but the GC is best-effort hygiene — one candidate
/// scan per process per interval is enough, and skipping redundant
/// scans keeps admin-pool pressure from scaling with the test count.
/// The forced/gated variants used by GC behavior tests are exempt.
static LAST_ELECTED_SWEEP: AtomicI64 = AtomicI64::new(0);
const ELECTED_SWEEP_MIN_INTERVAL_SECS: i64 = 20;

/// Names of disposable databases positively created or claimed by THIS
/// process during THIS run. This registry is the deletion boundary:
/// the authorized-delete GC path may only ever drop names present
/// here — it can never reach a database from a previous run, an
/// operator-created database, or anything the current process did not
/// itself provision. Membership is proven provenance, not a naming
/// convention.
static CURRENT_RUN_DATABASES: std::sync::OnceLock<
    std::sync::Mutex<std::collections::BTreeSet<String>>,
> = std::sync::OnceLock::new();

fn current_run_databases() -> &'static std::sync::Mutex<std::collections::BTreeSet<String>> {
    CURRENT_RUN_DATABASES.get_or_init(|| std::sync::Mutex::new(std::collections::BTreeSet::new()))
}

/// Register a database as belonging to the current test run. Called
/// internally by `create_test_database`/`claim_test_database`; exposed
/// (doc-hidden) for tests that create probe databases through raw SQL
/// and then want the authorized-delete path to be able to reach them.
/// Registration is fail-closed to the disposable convention, exactly
/// like create/claim/drop.
#[doc(hidden)]
pub fn register_current_run_database(name: &str) {
    assert_disposable_test_database_name(name);
    current_run_databases()
        .lock()
        .expect("current-run registry")
        .insert(name.to_string());
}

fn is_current_run_database(name: &str) -> bool {
    current_run_databases()
        .lock()
        .expect("current-run registry")
        .contains(name)
}

/// Best-effort sweep entry point (test setups). Leader-elected: only
/// one sweep runs at a time across ALL callers/processes; losers
/// return immediately. Sweeps are idempotent — a lost pass costs
/// nothing because the winner (or the next caller) does the work.
pub async fn drop_stale_test_databases(admin: &PgPool) -> u64 {
    let now = unix_now();
    let last = LAST_ELECTED_SWEEP.load(Ordering::Relaxed);
    if now - last < ELECTED_SWEEP_MIN_INTERVAL_SECS
        || LAST_ELECTED_SWEEP
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
    {
        return 0;
    }
    // Automatic/elected sweeps can never delete: they mark and demote
    // stale candidates only. There is deliberately no environment
    // variable or flag an ordinary test run could set to change this —
    // deletion of anything outside the current run is an operator
    // procedure, not a harness behavior.
    drop_stale_test_databases_impl(admin, None, true, false).await
}

/// Test-only variant that bypasses leader election — the GC behavior
/// tests need a pass that deterministically executes instead of
/// deferring to whichever sweeper won leadership. Never deletes.
#[doc(hidden)]
pub async fn drop_stale_test_databases_forced(admin: &PgPool) -> u64 {
    drop_stale_test_databases_impl(admin, None, false, false).await
}

#[doc(hidden)]
pub async fn drop_stale_test_databases_gated(admin: &PgPool, gate: Option<&DropGate>) -> u64 {
    drop_stale_test_databases_impl(admin, gate, false, false).await
}

/// Test-only variant that exercises the AUTHORIZED deletion path.
/// Even here, deletion is confined to names registered as
/// current-run-owned by THIS process — the helper cannot target a
/// database from a previous run or any other existing database, which
/// keeps the drop machinery (and its connection-race protections)
/// covered by tests without any reach beyond the current run.
#[doc(hidden)]
pub async fn drop_stale_test_databases_authorized_forced(admin: &PgPool) -> u64 {
    drop_stale_test_databases_impl(admin, None, false, true).await
}

/// Test-only authorized variant of the gated pass — same current-run
/// confinement as `drop_stale_test_databases_authorized_forced`.
#[doc(hidden)]
pub async fn drop_stale_test_databases_authorized_gated(
    admin: &PgPool,
    gate: Option<&DropGate>,
) -> u64 {
    drop_stale_test_databases_impl(admin, gate, false, true).await
}

async fn drop_stale_test_databases_impl(
    admin: &PgPool,
    gate: Option<&DropGate>,
    elect_leader: bool,
    allow_delete: bool,
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
        let outcome = gc_transition_locked(&mut conn, &name, now, gate, allow_delete).await;
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
    allow_delete: bool,
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
            // Deletion policy: automatic sweeps never delete, and even
            // the test-only authorized path may only drop databases
            // THIS process registered as current-run-owned. A
            // pending-stale marker on any other name proves a PRIOR
            // run created it — outside every programmatic deletion
            // boundary — so the candidate is retained and left visible
            // via its marker for authorized operator review.
            if !allow_delete || !is_current_run_database(name) {
                return GcOutcome::Skipped;
            }
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
