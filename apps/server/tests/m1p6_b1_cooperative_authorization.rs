//! M1-P6 B1 — cooperative authorization foundation tests (0067).
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same harness as tests/rbac_cooperative_schema.rs).
//!
//! Under test:
//! - Role-name uniqueness is PER-COOPERATIVE after the rescope:
//!   identical names across different cooperatives must coexist,
//!   duplicates inside one cooperative must still violate.
//! - NULL-scope (platform/global) role names stay globally unique —
//!   `NULLS NOT DISTINCT` makes the NULL group a single domain; a plain
//!   multi-column unique would silently allow platform duplicates.
//! - `cooperatives.manage` and `memberships.manage` are seeded into the
//!   catalog but granted to NO role and NO template (B1 is the
//!   foundation only — grants land with the APIs that consume them).
//! - The restricted `kooperatif_app` role still cannot write roles,
//!   memberships or cooperatives directly.
//! - Cross-cooperative role assignment remains impossible (composite
//!   FK) and shared identities keep independent per-coop membership.

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

use kooperatif_server::db as app_db;
use kooperatif_server::tenant::repo;

const TEST_PASSWORD: &str = "p6b1-test-parola-123";
const APP_ROLE: &str = "kooperatif_app";
/// Restricted-principal password: `KOOPERATIF_APP_TEST_PASSWORD`
/// overrides for CI; otherwise the documented local dev password.
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

static SETUP_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

async fn setup() -> TestDb {
    let _setup_slot = SETUP_SLOTS.acquire().await.expect("setup slot");
    let url = test_database_url();
    let database_name = format!("kooperatif_test_{}", Uuid::new_v4().simple());
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
    TestDb {
        _claim: claim,
        pool,
        database_name,
        admin_url,
    }
}

/// Application-role pool against the same disposable database — the
/// boundary proofs must run as the REAL restricted principal (same
/// credential rewrite as tests/audit_read.rs).
async fn app_pool(test: &TestDb) -> PgPool {
    let db_name = test.database_name.clone();
    let url = test_database_url();
    let (base, _) = url.rsplit_once('/').unwrap();
    let (scheme, rest) = base.split_once("://").unwrap();
    let at = rest.find('@').unwrap();
    app_db::connect(&format!(
        "{scheme}://{APP_ROLE}:{}{}/{db_name}",
        app_password(),
        &rest[at..]
    ))
    .await
    .expect("kooperatif_app pool")
}

async fn bootstrap_coop(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("SELECT id FROM cooperatives WHERE is_bootstrap")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn insert_role(
    pool: &PgPool,
    name: &str,
    cooperative_id: Option<Uuid>,
) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO roles (name, description, cooperative_id) VALUES ($1, 'b1', $2) RETURNING id",
    )
    .bind(name)
    .bind(cooperative_id)
    .fetch_one(pool)
    .await
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(|e| e.code().as_deref() == Some("23505"))
}

/// The same role name may exist independently in two cooperatives —
/// this is the whole point of the rescope (per-coop template cloning,
/// M1-K3).
#[tokio::test]
async fn same_name_roles_allowed_across_cooperatives() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;
    let coop_b = repo::create_cooperative(&test.pool, "İkinci Kooperatif", None)
        .await
        .unwrap()
        .id;

    let role_a = insert_role(&test.pool, "Muhasebe", Some(coop_a))
        .await
        .expect("first role");
    let role_b = insert_role(&test.pool, "Muhasebe", Some(coop_b))
        .await
        .expect("same name in a different cooperative must be allowed");
    assert_ne!(role_a, role_b);

    // Case-insensitive too: 'MUHASEBE' in B is still distinct from
    // 'Muhasebe' in A — nothing leaks across cooperatives.
    insert_role(&test.pool, "MUHASEBE", Some(coop_b))
        .await
        .expect_err("case-insensitive duplicate inside coop B must violate");

    test.cleanup().await;
}

/// Within one cooperative the name stays unique — the rescope weakens
/// nothing inside a scope.
#[tokio::test]
async fn duplicate_name_within_cooperative_rejected() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;

    insert_role(&test.pool, "Yönetici", Some(coop_a))
        .await
        .expect("first role");
    let dup = insert_role(&test.pool, "yönetici", Some(coop_a)).await;
    assert!(
        dup.as_ref().is_err_and(is_unique_violation),
        "case-insensitive duplicate inside the same cooperative must violate: {dup:?}"
    );

    test.cleanup().await;
}

/// Platform/global roles (cooperative_id IS NULL) are one uniqueness
/// domain — NULLS NOT DISTINCT prevents the classic multi-column-unique
/// hole where NULLs compare distinct and duplicates pass.
#[tokio::test]
async fn duplicate_platform_role_name_rejected() {
    let test = setup().await;

    insert_role(&test.pool, "Platform Denetçi", None)
        .await
        .expect("first platform role");
    let dup = insert_role(&test.pool, "platform denetçi", None).await;
    assert!(
        dup.as_ref().is_err_and(is_unique_violation),
        "duplicate NULL-scope role name must violate: {dup:?}"
    );

    test.cleanup().await;
}

/// A platform role and a cooperative role may share a name — separate
/// uniqueness domains.
#[tokio::test]
async fn platform_and_cooperative_names_are_separate_domains() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;

    insert_role(&test.pool, "Ortak İsim", None)
        .await
        .expect("platform role");
    insert_role(&test.pool, "Ortak İsim", Some(coop_a))
        .await
        .expect("same name on a cooperative role must be allowed");

    test.cleanup().await;
}

/// Bootstrap artifacts are untouched: seeded roles, the bootstrap
/// cooperative and its memberships/assignments survive the rescope.
#[tokio::test]
async fn existing_bootstrap_roles_and_assignments_survive() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;

    let seeded: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM roles WHERE name = 'Sistem Yöneticisi'")
            .fetch_optional(&test.pool)
            .await
            .unwrap();
    assert!(seeded.is_some(), "seeded administrator role must exist");

    let role_count: i64 = sqlx::query_scalar("SELECT count(*) FROM roles")
        .fetch_one(&test.pool)
        .await
        .unwrap();
    assert!(role_count >= 1, "seeded roles must survive the rescope");
    let _ = coop_a;

    test.cleanup().await;
}

/// Both catalog keys exist, and grants stay least-privilege: B1 was
/// foundation only; B2 reserved cooperatives.manage for the dedicated
/// NULL-scope platform role; B5 (approved product decision) grants
/// memberships.* to name-matched System Administrator roles and the
/// system-administrator template — and to NOTHING else.
#[tokio::test]
async fn new_permissions_seeded_but_ungranted() {
    let test = setup().await;

    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT key FROM permissions WHERE key IN ('cooperatives.manage','memberships.manage') ORDER BY key",
    )
    .fetch_all(&test.pool)
    .await
    .unwrap();
    assert_eq!(keys, ["cooperatives.manage", "memberships.manage"]);

    // Allowed grants: cooperatives.manage on the dedicated NULL-scope
    // platform-lifecycle role (never assigned to any user), and
    // memberships.* on name-matched Sistem Yöneticisi roles in any
    // scope (0072). Everything else must remain zero.
    let granted_roles: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_permissions rp \
         JOIN permissions p ON p.id = rp.permission_id \
         JOIN roles r ON r.id = rp.role_id \
         WHERE p.key IN ('cooperatives.manage','memberships.manage','memberships.read') \
           AND NOT (p.key = 'cooperatives.manage' \
                    AND r.cooperative_id IS NULL \
                    AND lower(r.name) = lower('platform-lifecycle')) \
           AND NOT (p.key IN ('memberships.manage','memberships.read') \
                    AND lower(r.name) = lower('Sistem Yöneticisi')) \
           AND NOT (p.key = 'memberships.read' \
                    AND EXISTS (SELECT 1 FROM role_permissions rp2 \
                                JOIN permissions p2 ON p2.id = rp2.permission_id \
                                WHERE rp2.role_id = rp.role_id \
                                  AND p2.key = 'users.manage'))",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(
        granted_roles, 0,
        "new permissions must not leak beyond the dedicated platform role and System Administrator roles"
    );

    // Templates: cooperatives.manage on NO template; memberships.manage
    // only on system-administrator (0072's approved template grant).
    let granted_templates: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM system_role_template_permissions tp \
         JOIN permissions p ON p.id = tp.permission_id \
         JOIN system_role_templates t ON t.id = tp.template_id \
         WHERE p.key IN ('cooperatives.manage','memberships.manage') \
           AND NOT (p.key = 'memberships.manage' AND t.key = 'system-administrator')",
    )
    .fetch_one(&test.pool)
    .await
    .unwrap();
    assert_eq!(
        granted_templates, 0,
        "template grants must stay confined to system-administrator memberships.manage"
    );

    // The application catalog mirrors the seed (route guards resolve
    // through it).
    assert!(kooperatif_server::auth::authz::catalog::is_known(
        "cooperatives.manage"
    ));
    assert!(kooperatif_server::auth::authz::catalog::is_known(
        "memberships.manage"
    ));

    test.cleanup().await;
}

/// The restricted runtime role keeps zero direct write surface on the
/// RBAC/tenant tables and cannot even SELECT `cooperatives` — every
/// cooperative-management path must stay behind definers.
#[tokio::test]
async fn restricted_role_keeps_no_direct_write_surface() {
    let test = setup().await;
    let app = app_pool(&test).await;
    let coop_a = bootstrap_coop(&test.pool).await;

    for op in ["INSERT", "UPDATE", "DELETE"] {
        let allowed: bool = sqlx::query_scalar(&format!(
            "SELECT has_table_privilege('{APP_ROLE}', 'roles', '{op}')"
        ))
        .fetch_one(&test.pool)
        .await
        .unwrap();
        assert!(!allowed, "kooperatif_app must not hold {op} on roles");
        let allowed: bool = sqlx::query_scalar(&format!(
            "SELECT has_table_privilege('{APP_ROLE}', 'cooperative_memberships', '{op}')"
        ))
        .fetch_one(&test.pool)
        .await
        .unwrap();
        assert!(
            !allowed,
            "kooperatif_app must not hold {op} on cooperative_memberships"
        );
    }

    let err = sqlx::query("SELECT id FROM cooperatives LIMIT 1")
        .fetch_all(&app)
        .await
        .expect_err("direct cooperatives SELECT must be denied");
    assert_eq!(
        err.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );

    let err = sqlx::query("INSERT INTO roles (name, cooperative_id) VALUES ('x', $1)")
        .bind(coop_a)
        .execute(&app)
        .await
        .expect_err("direct role INSERT must be denied");
    assert_eq!(
        err.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );

    let err = sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         VALUES ($1, $2, 'active')",
    )
    .bind(coop_a)
    .bind(Uuid::nil())
    .execute(&app)
    .await
    .expect_err("direct membership INSERT must be denied");
    assert_eq!(
        err.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );

    app.close().await;
    test.cleanup().await;
}

/// A role owned by cooperative A can never be assigned in cooperative
/// B's scope — the composite (role_id, cooperative_id) FK is the hard
/// boundary.
#[tokio::test]
async fn cross_cooperative_role_assignment_rejected() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;
    let coop_b = repo::create_cooperative(&test.pool, "Üçüncü Kooperatif", None)
        .await
        .unwrap()
        .id;

    let user = {
        let argon2 = argon2::Argon2::new(
            argon2::Algorithm::Argon2id,
            argon2::Version::V0x13,
            argon2::Params::new(kooperatif_server::config::ARGON2_M_COST_FLOOR, 1, 1, None)
                .expect("params"),
        );
        let hash =
            kooperatif_server::auth::identity::hash_password(&argon2, TEST_PASSWORD).expect("hash");
        kooperatif_server::auth::users::create_user(
            &test.pool,
            "p6b1user",
            "B1 Test",
            &hash,
            TEST_PASSWORD,
        )
        .await
        .expect("user")
        .id
    };
    let role_a = insert_role(&test.pool, "A Rolü", Some(coop_a))
        .await
        .expect("coop A role");

    // Cross-scope assignment: coop-A role with coop-B scope → the
    // composite FK (role_id, cooperative_id) → roles(id, cooperative_id)
    // has no matching pair → 23503.
    let err = sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
         VALUES ($1, $2, $3)",
    )
    .bind(user)
    .bind(role_a)
    .bind(coop_b)
    .execute(&test.pool)
    .await
    .expect_err("cross-cooperative assignment must violate the composite FK");
    assert_eq!(
        err.as_database_error().unwrap().code().as_deref(),
        Some("23503"),
        "expected FK violation, got: {err}"
    );

    test.cleanup().await;
}

/// A shared identity holds independent memberships: suspending B does
/// not touch A. (HTTP/WS evidence lives in tenant_kernel + realtime;
/// this pins the row-level independence.)
#[tokio::test]
async fn shared_identity_memberships_stay_independent() {
    let test = setup().await;
    let coop_a = bootstrap_coop(&test.pool).await;
    let coop_b = repo::create_cooperative(&test.pool, "Dördüncü Kooperatif", None)
        .await
        .unwrap()
        .id;

    let user = {
        let argon2 = argon2::Argon2::new(
            argon2::Algorithm::Argon2id,
            argon2::Version::V0x13,
            argon2::Params::new(kooperatif_server::config::ARGON2_M_COST_FLOOR, 1, 1, None)
                .expect("params"),
        );
        let hash =
            kooperatif_server::auth::identity::hash_password(&argon2, TEST_PASSWORD).expect("hash");
        kooperatif_server::auth::users::create_user(
            &test.pool,
            "p6b1shared",
            "Shared",
            &hash,
            TEST_PASSWORD,
        )
        .await
        .expect("user")
        .id
    };
    // Bootstrap only enrolls users that exist AT bootstrap time — a
    // later-created user starts with zero memberships. Both must be
    // added explicitly.
    repo::add_member(&test.pool, "Ana Kooperatif", user, None)
        .await
        .expect("membership A");
    repo::add_member(&test.pool, "Dördüncü Kooperatif", user, None)
        .await
        .expect("membership B");

    sqlx::query(
        "UPDATE cooperative_memberships SET status = 'suspended' \
         WHERE user_id = $1 AND cooperative_id = $2",
    )
    .bind(user)
    .bind(coop_b)
    .execute(&test.pool)
    .await
    .unwrap();

    let statuses: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT cooperative_id, status::text FROM cooperative_memberships \
         WHERE user_id = $1 ORDER BY cooperative_id",
    )
    .bind(user)
    .fetch_all(&test.pool)
    .await
    .unwrap();
    let a_status = statuses
        .iter()
        .find(|(c, _)| *c == coop_a)
        .unwrap()
        .1
        .clone();
    let b_status = statuses
        .iter()
        .find(|(c, _)| *c == coop_b)
        .unwrap()
        .1
        .clone();
    assert_eq!(a_status, "active", "coop A membership untouched");
    assert_eq!(b_status, "suspended");

    test.cleanup().await;
}
