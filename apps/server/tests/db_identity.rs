//! M1-P2 B1 database-identity guard — DB-gated integration tests.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same convention as the other integration suites).

use kooperatif_server::db as app_db;
use sqlx::PgPool;
use uuid::Uuid;

fn test_database_url() -> String {
    let value = std::env::var("KOOPERATIF_TEST_DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "KOOPERATIF_TEST_DATABASE_URL is not set — required DB-gated integration tests cannot silently pass; point it at a disposable PostgreSQL database"
        )
    });
    let trimmed = value.trim().to_string();
    assert!(
        !trimmed.is_empty(),
        "KOOPERATIF_TEST_DATABASE_URL is empty — required DB-gated tests need a database"
    );
    trimmed
}

/// One disposable `kooperatif_test_<uuid>` database; dropped on cleanup.
struct TestDb {
    _claim: app_db::TestDbClaim,
    pool: PgPool,
    database_name: String,
    admin_url: String,
}

impl TestDb {
    async fn cleanup(&self) {
        self.pool.close().await;
        if let Ok(admin) = sqlx::PgPool::connect(&self.admin_url).await {
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

async fn setup() -> TestDb {
    let url = test_database_url();
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
    TestDb {
        _claim: claim,
        pool,
        database_name,
        admin_url,
    }
}

#[tokio::test]
async fn guard_accepts_the_database_the_pool_is_connected_to() {
    let db = setup().await;

    app_db::verify_expected_database(&db.pool, &db.database_name)
        .await
        .expect("matching database must pass the guard");

    db.cleanup().await;
}

#[tokio::test]
async fn guard_rejects_a_wrong_database_before_any_write() {
    let db = setup().await;

    let error = app_db::verify_expected_database(&db.pool, "kooperatif_dev")
        .await
        .expect_err("mismatched database must be refused");
    match error {
        app_db::DatabaseIdentityError::Mismatch { expected, actual } => {
            assert_eq!(expected, "kooperatif_dev");
            assert_eq!(actual, db.database_name);
        }
        other => panic!("expected a Mismatch refusal, got: {other}"),
    }

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// M1-P2 Phase B: the test-harness identity guard (disposable-only
// allowlist) — refuses protected/non-convention targets BEFORE the
// migrator runs.
// ---------------------------------------------------------------------

/// Pure name-policy coverage: every protected shape must be refused.
#[test]
fn test_database_name_policy_rejects_everything_but_the_convention() {
    for allowed in [
        "kooperatif_test",
        "kooperatif_test_0123456789abcdef0123456789abcdef",
    ] {
        assert!(
            app_db::is_test_database_name(allowed),
            "{allowed} must be allowed"
        );
    }
    for rejected in [
        "kooperatif",
        "kooperatif_dev",
        "kooperatif_e2e",
        "kooperatif_c1_shadow",
        "kooperatif_c1_shadow2",
        "kooperatif_backup_verify",
        "kooperatif_audit_001",
        "kooperatif_funcaudit_001",
        "kooperatif_mig12",
        "kooperatif_verify",
        "kooperatif_gc_keep_219f2770d9ef4cfdaa8752c80acab888",
        "postgres",
        "kooperatif_test_too_short",
        "kooperatif_test_0123456789ABCDEF0123456789ABCDEF", // uppercase hex
        "kooperatif_test_extra_suffix_chars_not_hex_zzzz",
        "kooperatif_test_", // bare prefix
        "xkooperatif_test_0123456789abcdef0123456789abcdef",
    ] {
        assert!(
            !app_db::is_test_database_name(rejected),
            "{rejected} must be rejected"
        );
    }
}

/// The standing direct-connect database is allowed for harnesses but can
/// never be created/dropped by them.
#[test]
fn disposable_name_assertion_excludes_the_standing_database() {
    app_db::assert_disposable_test_database_name(
        "kooperatif_test_0123456789abcdef0123456789abcdef",
    );
    let result = std::panic::catch_unwind(|| {
        app_db::assert_disposable_test_database_name("kooperatif_test")
    });
    assert!(result.is_err(), "standing db name must panic");
    for rejected in ["kooperatif", "kooperatif_dev", "kooperatif_c1_shadow"] {
        let r = std::panic::catch_unwind(|| app_db::assert_disposable_test_database_name(rejected));
        assert!(r.is_err(), "{rejected} must panic");
    }
}

/// `run_test_migrations` accepts the disposable database it was given.
#[tokio::test]
async fn test_migrations_accept_a_disposable_database() {
    let db = setup().await;
    app_db::run_test_migrations(&db.pool)
        .await
        .expect("migrations on a disposable database must pass");
    db.cleanup().await;
}

/// The incident this phase prevents: a pool bound to a NON-disposable
/// database must be refused by `run_test_migrations` before the migrator
/// sends a single statement — proven by `_sqlx_migrations` never
/// materialising on the rejected target.
#[tokio::test]
async fn test_migrations_reject_a_protected_shaped_target_before_migration() {
    let db = setup().await; // disposable harness pieces: admin access + name
    let rogue_name = format!("kooperatif_nottest_{}", Uuid::new_v4().simple());
    let admin = app_db::connect(&db.admin_url)
        .await
        .expect("admin connectivity");
    // Create the rogue database with raw admin SQL — the guarded harness
    // path (create_test_database) must itself refuse it, proven below.
    sqlx::query(&format!("CREATE DATABASE \"{rogue_name}\""))
        .execute(&admin)
        .await
        .expect("rogue probe database created");

    let base_url = db
        .admin_url
        .rsplit_once('/')
        .map(|(base, _)| base.to_string())
        .expect("url with database segment");
    let pool = app_db::connect(&format!("{base_url}/{rogue_name}"))
        .await
        .expect("rogue database connectivity");

    let outcome = app_db::run_test_migrations(&pool).await;
    match outcome {
        Err(app_db::TestMigrationError::Identity(app_db::DatabaseIdentityError::Mismatch {
            actual,
            ..
        })) => assert_eq!(actual, rogue_name),
        other => panic!("expected Identity refusal, got: {other:?}"),
    }
    let migrated: bool = sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
        .fetch_one(&pool)
        .await
        .expect("migration marker probe");
    assert!(
        !migrated,
        "a refused database must never receive migration state"
    );
    pool.close().await;

    // The guarded create path must refuse the same name outright.
    let create_result =
        std::panic::catch_unwind(|| app_db::assert_disposable_test_database_name(&rogue_name));
    assert!(create_result.is_err());

    sqlx::query(&format!(
        "DROP DATABASE IF EXISTS \"{rogue_name}\" WITH (FORCE)"
    ))
    .execute(&admin)
    .await
    .expect("rogue probe dropped");
    admin.close().await;
    db.cleanup().await;
}
