//! M1-P1-a — cooperative-scoped RBAC schema foundation tests (P1-K1..K5).
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same harness as tests/rbac.rs). Scope honesty: these tests prove the
//! 0019 schema layer only — cooperative ownership columns, composite
//! foreign keys that make cross-cooperative assignments impossible,
//! bootstrap backfill semantics, administrator-continuity assertion and
//! system role-template storage (P1-K4). They do NOT prove cooperative
//! permission evaluation (P1-b) or scoped admin APIs (P1-c); the columns
//! remain NULLABLE until the contract migration lands.

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

use kooperatif_server::db as app_db;
use kooperatif_server::tenant::repo;

const TEST_PASSWORD: &str = "p1a-test-parola-123";

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

struct TestDb {
    _claim: app_db::TestDbClaim,
    pool: PgPool,
    database_name: String,
    admin_url: String,
}

impl TestDb {
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

// Bounds concurrent provisioning: each setup opens an admin pool plus an
// app pool while other tests hold theirs, so unlimited parallelism can
// exceed postgres max_connections (see reservation.rs, P0-002).
static SETUP_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

async fn setup() -> Option<TestDb> {
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
    app_db::run_test_migrations(&pool)
        .await
        .expect("migrations applied");
    Some(TestDb {
        _claim: claim,
        pool,
        database_name,
        admin_url,
    })
}

async fn create_user(pool: &PgPool, username: &str) -> Uuid {
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(kooperatif_server::config::ARGON2_M_COST_FLOOR, 1, 1, None)
            .expect("test parameters"),
    );
    let hash =
        kooperatif_server::auth::identity::hash_password(&argon2, TEST_PASSWORD).expect("hash");
    kooperatif_server::auth::users::create_user(pool, username, "P1a Test", &hash, TEST_PASSWORD)
        .await
        .expect("user created")
        .id
}

async fn insert_role(pool: &PgPool, name: &str, cooperative_id: Option<Uuid>) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO roles (name, description, cooperative_id) VALUES ($1, 'test', $2) RETURNING id",
    )
    .bind(name)
    .bind(cooperative_id)
    .fetch_one(pool)
    .await
    .expect("role insert")
}

async fn insert_assignment(
    pool: &PgPool,
    user_id: Uuid,
    role_id: Uuid,
    cooperative_id: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) VALUES ($1, $2, $3)",
    )
    .bind(user_id)
    .bind(role_id)
    .bind(cooperative_id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Byte-identical replay of the 0019 backfill statements so the
/// migration-time data migration is exercised against realistic fixture
/// ordering (roles/assignments created before bootstrap). Must stay in
/// sync with migrations/0019_cooperative_rbac_foundation.sql.
async fn replay_0019_backfill(pool: &PgPool, bootstrap: Uuid) {
    // The 0019 statement is replayed byte-identically EXCEPT for the
    // deliberate exclusion of the M1-P6-B2 platform lifecycle role:
    // 'platform-lifecycle' did not exist at 0019 time, is a NULL-scope
    // (platform) role by contract, and must never be re-homed into a
    // cooperative by this historical-replay fixture.
    sqlx::query(
        "UPDATE roles SET cooperative_id = $1 WHERE cooperative_id IS NULL \
           AND lower(name) <> lower('platform-lifecycle')",
    )
    .bind(bootstrap)
    .execute(pool)
    .await
    .expect("roles backfill");
    sqlx::query(
        "UPDATE user_role_assignments ura \
            SET cooperative_id = $1 \
          WHERE ura.cooperative_id IS NULL \
            AND EXISTS (SELECT 1 FROM roles r \
                         WHERE r.id = ura.role_id AND r.cooperative_id = $1) \
            AND EXISTS (SELECT 1 FROM cooperative_memberships m \
                         WHERE m.user_id = ura.user_id \
                           AND m.cooperative_id = $1)",
    )
    .bind(bootstrap)
    .execute(pool)
    .await
    .expect("assignments backfill");
}

/// Byte-identical replay of the 0019 administrator-path queries.
/// Returns (before, after): global working admin paths vs scoped usable
/// bootstrap admin paths.
async fn admin_path_counts(pool: &PgPool, bootstrap: Uuid) -> (i64, i64) {
    let before: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT ura.user_id) \
           FROM user_role_assignments ura \
           JOIN roles r ON r.id = ura.role_id AND r.status = 'active' \
           JOIN role_permissions rp ON rp.role_id = r.id \
           JOIN permissions p ON p.id = rp.permission_id AND p.key = 'roles.manage' \
           JOIN users u ON u.id = ura.user_id AND u.status = 'active'",
    )
    .fetch_one(pool)
    .await
    .expect("admin before count");
    let after: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT ura.user_id) \
           FROM user_role_assignments ura \
           JOIN roles r ON r.id = ura.role_id AND r.status = 'active' \
                        AND r.cooperative_id = $1 \
           JOIN role_permissions rp ON rp.role_id = r.id \
           JOIN permissions p ON p.id = rp.permission_id AND p.key = 'roles.manage' \
           JOIN users u ON u.id = ura.user_id AND u.status = 'active' \
           JOIN cooperative_memberships m \
             ON m.user_id = ura.user_id AND m.cooperative_id = $1 \
            AND m.status = 'active' \
          WHERE ura.cooperative_id = $1",
    )
    .bind(bootstrap)
    .fetch_one(pool)
    .await
    .expect("admin after count");
    (before, after)
}

async fn seeded_admin_assignment(pool: &PgPool, user_id: Uuid, coop: Option<Uuid>) {
    let admin_role = Uuid::parse_str("00000000-0000-4000-8000-000000000001").expect("seed role id");
    if let Some(coop_id) = coop {
        // Composite FK requires the role to carry the same cooperative.
        sqlx::query("UPDATE roles SET cooperative_id = $1 WHERE id = $2")
            .bind(coop_id)
            .bind(admin_role)
            .execute(pool)
            .await
            .expect("seed role scoped");
    }
    insert_assignment(pool, user_id, admin_role, coop)
        .await
        .expect("seeded admin assignment");
}

#[tokio::test]
async fn cooperative_columns_and_template_seed_exist() {
    let Some(test) = setup().await else { return };

    let nullable: bool = sqlx::query_scalar(
        "SELECT count(*) = 2 FROM information_schema.columns \
          WHERE table_name IN ('roles','user_role_assignments') \
            AND column_name = 'cooperative_id' AND is_nullable = 'YES'",
    )
    .fetch_one(&test.pool)
    .await
    .expect("nullability check");
    assert!(
        nullable,
        "cooperative_id columns must exist and stay NULLABLE in P1-a"
    );

    let template_perms: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM system_role_template_permissions tp \
          JOIN system_role_templates t ON t.id = tp.template_id \
          WHERE t.key = 'system-administrator' AND t.status = 'active'",
    )
    .fetch_one(&test.pool)
    .await
    .expect("template seed");
    // M1-P6-B1 refined the contract: the template mirrors the
    // COOPERATIVE-scoped catalog. One key is excluded:
    // - `cooperatives.manage` is platform authority — it must never
    //   appear on a cooperative template (permanent exclusion).
    // B5 (0072, approved) added memberships.manage to the template, so
    // the catalog exclusion now covers the platform key only.
    let catalog: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM permissions WHERE key NOT IN ('cooperatives.manage')",
    )
    .fetch_one(&test.pool)
    .await
    .expect("catalog size");
    assert!(catalog > 0);
    assert_eq!(
        template_perms, catalog,
        "admin template mirrors the cooperative-scoped catalog"
    );

    // Templates are not roles and can never be assigned.
    let leaked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM roles WHERE lower(name) = 'sistem yöneticisi' \
                        AND id <> '00000000-0000-4000-8000-000000000001')",
    )
    .fetch_one(&test.pool)
    .await
    .expect("no duplicate admin role");
    assert!(!leaked);

    test.cleanup().await;
}

#[tokio::test]
async fn backfill_assigns_existing_rows_to_bootstrap() {
    let Some(test) = setup().await else { return };

    // Fixture ordering mirrors a real installation: data predates the
    // bootstrap cooperative.
    let user = create_user(&test.pool, "p1a_admin_uyesi").await;
    let role = insert_role(&test.pool, "P1a Kasa Rolü", None).await;
    insert_assignment(&test.pool, user, role, None)
        .await
        .expect("legacy assignment");
    seeded_admin_assignment(&test.pool, user, None).await;

    let (bootstrap, members) = repo::bootstrap_initial_cooperative(&test.pool, "Ana Kooperatif")
        .await
        .expect("bootstrap");
    assert!(members >= 1);

    replay_0019_backfill(&test.pool, bootstrap).await;

    let role_coop: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM roles WHERE id = $1")
            .bind(role)
            .fetch_one(&test.pool)
            .await
            .expect("role backfilled");
    assert_eq!(role_coop, Some(bootstrap));

    let scoped: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_role_assignments WHERE cooperative_id = $1")
            .bind(bootstrap)
            .fetch_one(&test.pool)
            .await
            .expect("assignments backfilled");
    assert_eq!(scoped, 2, "both legacy assignments migrate to bootstrap");

    let (before, after) = admin_path_counts(&test.pool, bootstrap).await;
    assert!(before >= 1 && after >= 1, "administrator path preserved");

    test.cleanup().await;
}

#[tokio::test]
async fn backfill_skips_assignment_without_membership() {
    let Some(test) = setup().await else { return };

    let (bootstrap, _) = repo::bootstrap_initial_cooperative(&test.pool, "Ana Kooperatif")
        .await
        .expect("bootstrap");
    // User created AFTER bootstrap — no membership (grant-role CLI could
    // have assigned a role to such a user before P1).
    let orphan = create_user(&test.pool, "p1a_uyesiz").await;
    let role = insert_role(&test.pool, "P1a Üyesiz Rol", None).await;
    insert_assignment(&test.pool, orphan, role, None)
        .await
        .expect("legacy assignment");

    replay_0019_backfill(&test.pool, bootstrap).await;

    // Role is bootstrap-owned; the assignment stays NULL (legacy lane)
    // rather than being silently re-homed into a cooperative the user
    // does not belong to.
    let ura_coop: Option<Uuid> = sqlx::query_scalar(
        "SELECT cooperative_id FROM user_role_assignments WHERE user_id = $1 AND role_id = $2",
    )
    .bind(orphan)
    .bind(role)
    .fetch_one(&test.pool)
    .await
    .expect("unscoped row readable");
    assert_eq!(ura_coop, None, "membership-less assignment must stay NULL");
    let role_coop: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM roles WHERE id = $1")
            .bind(role)
            .fetch_one(&test.pool)
            .await
            .expect("role readable");
    assert_eq!(role_coop, Some(bootstrap));

    test.cleanup().await;
}

#[tokio::test]
async fn cross_cooperative_role_assignment_is_rejected() {
    let Some(test) = setup().await else { return };

    let user = create_user(&test.pool, "p1a_cross_user").await;
    let (bootstrap, _) = repo::bootstrap_initial_cooperative(&test.pool, "Ana Kooperatif")
        .await
        .expect("bootstrap");
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("second cooperative")
        .id;
    repo::add_member(&test.pool, "B Kooperatifi", user, None)
        .await
        .expect("B membership");

    let role_a = insert_role(&test.pool, "A Rolü", Some(bootstrap)).await;
    // Membership in B + role owned by A → composite FK must reject.
    let err = insert_assignment(&test.pool, user, role_a, Some(coop_b))
        .await
        .expect_err("cross-coop role reference must be rejected");
    let constraint = match err {
        sqlx::Error::Database(db_err) => db_err.constraint().map(str::to_string),
        other => panic!("expected FK violation, got {other:?}"),
    };
    assert_eq!(
        constraint.as_deref(),
        Some("user_role_assignments_coop_role_fk")
    );

    test.cleanup().await;
}

#[tokio::test]
async fn cross_cooperative_membership_mismatch_is_rejected() {
    let Some(test) = setup().await else { return };

    let user = create_user(&test.pool, "p1a_mm_user").await;
    repo::bootstrap_initial_cooperative(&test.pool, "Ana Kooperatif")
        .await
        .expect("bootstrap");
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("second cooperative")
        .id;
    // User is member of bootstrap ONLY; role B exists but user has no
    // membership in B → (B, user) is not a memberships row.
    let role_b = insert_role(&test.pool, "B Rolü", Some(coop_b)).await;
    let err = insert_assignment(&test.pool, user, role_b, Some(coop_b))
        .await
        .expect_err("assignment without membership must be rejected");
    let constraint = match err {
        sqlx::Error::Database(db_err) => db_err.constraint().map(str::to_string),
        other => panic!("expected FK violation, got {other:?}"),
    };
    assert_eq!(
        constraint.as_deref(),
        Some("user_role_assignments_coop_user_fk")
    );

    test.cleanup().await;
}

#[tokio::test]
async fn invalid_cooperative_reference_is_rejected() {
    let Some(test) = setup().await else { return };

    let user = create_user(&test.pool, "p1a_fk_user").await;
    repo::bootstrap_initial_cooperative(&test.pool, "Ana Kooperatif")
        .await
        .expect("bootstrap");
    let role = insert_role(&test.pool, "P1a FK Rol", None).await;
    let err = insert_assignment(&test.pool, user, role, Some(Uuid::new_v4()))
        .await
        .expect_err("nonexistent cooperative must be rejected");
    match err {
        sqlx::Error::Database(db_err) => {
            assert_eq!(
                db_err.code().as_deref(),
                Some("23503"),
                "foreign key violation"
            )
        }
        other => panic!("expected FK violation, got {other:?}"),
    }
    let err = sqlx::query("INSERT INTO roles (name, cooperative_id) VALUES ('x', $1)")
        .bind(Uuid::new_v4())
        .execute(&test.pool)
        .await
        .expect_err("nonexistent cooperative on roles must be rejected");
    match err {
        sqlx::Error::Database(db_err) => {
            assert_eq!(
                db_err.code().as_deref(),
                Some("23503"),
                "foreign key violation"
            )
        }
        other => panic!("expected FK violation, got {other:?}"),
    }

    test.cleanup().await;
}

#[tokio::test]
async fn legacy_unscoped_rows_still_work() {
    let Some(test) = setup().await else { return };

    // Temporary lane: NULL cooperative_id rows remain valid so existing
    // code paths (role CRUD, grant-role) behave identically in P1-a.
    let user = create_user(&test.pool, "p1a_legacy_user").await;
    let role = insert_role(&test.pool, "P1a Legacy Rol", None).await;
    insert_assignment(&test.pool, user, role, None)
        .await
        .expect("NULL cooperative_id stays insertable during P1-a");

    // Duplicate role name still rejected globally (roles_name_key
    // unchanged in P1-a; the per-coop swap is a P1-c contract step).
    let dup = sqlx::query("INSERT INTO roles (name) VALUES ('P1a Legacy Rol')")
        .execute(&test.pool)
        .await;
    assert!(dup.is_err(), "global name uniqueness still enforced");

    test.cleanup().await;
}

#[tokio::test]
async fn administrator_continuity_assertion_detects_lockout() {
    let Some(test) = setup().await else { return };

    // Craft the failure shape the DO-block guards against: an admin
    // assignment whose user holds no bootstrap membership. The scoped
    // count drops to zero while a global path existed → the invariant
    // comparison the migration performs would fail loudly.
    let orphan_admin = create_user(&test.pool, "p1a_kilit_admin").await;
    let (bootstrap, _) = repo::bootstrap_initial_cooperative(&test.pool, "Ana Kooperatif")
        .await
        .expect("bootstrap (enrolls the admin)");
    replay_0019_backfill(&test.pool, bootstrap).await;
    seeded_admin_assignment(&test.pool, orphan_admin, Some(bootstrap)).await;
    // Reverse the safe order: unscope the assignment first (releases the
    // composite FK), then remove the membership row.
    sqlx::query("UPDATE user_role_assignments SET cooperative_id = NULL WHERE user_id = $1")
        .bind(orphan_admin)
        .execute(&test.pool)
        .await
        .expect("unscoped admin row");
    sqlx::query("DELETE FROM cooperative_memberships WHERE user_id = $1")
        .bind(orphan_admin)
        .execute(&test.pool)
        .await
        .expect("membership removed");

    let (before, after) = admin_path_counts(&test.pool, bootstrap).await;
    assert!(before >= 1, "global admin path exists");
    assert_eq!(
        after, 0,
        "no usable scoped admin path → migration would abort"
    );

    test.cleanup().await;
}

#[tokio::test]
async fn migration_replay_is_idempotent() {
    let Some(test) = setup().await else { return };

    app_db::run_test_migrations(&test.pool)
        .await
        .expect("migration re-run is a no-op");
    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&test.pool)
        .await
        .expect("ledger readable");
    assert!(applied >= 19, "0019 recorded, got {applied}");

    test.cleanup().await;
}
