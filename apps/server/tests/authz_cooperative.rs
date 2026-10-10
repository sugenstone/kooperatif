//! M1-P1-b — cooperative-scoped authorization core tests (P1-K2/K3).
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same harness as tests/rbac.rs). These tests prove the scoped resolver
//! (`effective_permissions_in_cooperative`, `require_in_cooperative`,
//! `active_role_summaries_in_cooperative`): membership-bound, per-coop,
//! NULL-lane-excluding, cache-free resolution. They also pin the
//! documented compatibility boundary: the legacy GLOBAL
//! `effective_permissions`/`require` path is intentionally unchanged
//! until P1-c migrates its ~90 call sites (M1-P1-RBAC-DESIGN.md §4).

use std::collections::HashSet;
use std::sync::Arc;

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use kooperatif_server::auth::extractor::CurrentAuth;
use kooperatif_server::auth::{authz, identity, users, AuthRuntime};
use kooperatif_server::clock::MutableClock;
use kooperatif_server::config::{AuthConfig, ARGON2_M_COST_FLOOR};
use kooperatif_server::db as app_db;
use kooperatif_server::http::error::ApiError;
use kooperatif_server::http::AppState;
use kooperatif_server::tenant::{repo, TenantCtx};

const TEST_PASSWORD: &str = "p1b-test-parola-123";

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
    pool: PgPool,
    state: AppState,
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
        if let Ok(admin) = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&self.admin_url)
            .await
        {
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

// Bounds concurrent provisioning against postgres max_connections (see
// reservation.rs, P0-002).
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
    sqlx::query(&format!("CREATE DATABASE {database_name}"))
        .execute(&admin)
        .await
        .expect("test database created");
    admin.close().await;
    let pool = app_db::connect(&format!("{base_url}/{database_name}"))
        .await
        .expect("test database connectivity");
    app_db::run_migrations(&pool)
        .await
        .expect("migrations applied");
    let clock = Arc::new(MutableClock::new(
        OffsetDateTime::parse(
            "2026-01-01T00:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .expect("fixed epoch"),
    ));
    let state = AppState {
        db: Some(pool.clone()),
        auth: Arc::new(AuthRuntime::new(
            AuthConfig {
                allowed_origins: vec!["http://localhost:5173".to_string()],
                cookie_secure: false,
                session_absolute_ttl_secs: 3600,
                session_idle_ttl_secs: 600,
                argon2_m_cost: ARGON2_M_COST_FLOOR,
                argon2_t_cost: 1,
                argon2_p_cost: 1,
                login_window_secs: 60,
                login_username_max_attempts: 5,
                login_ip_max_attempts: 100_000,
                forwarded_ip: false,
            },
            clock,
        )),
    };
    Some(TestDb {
        pool,
        state,
        database_name,
        admin_url,
    })
}

async fn create_user(pool: &PgPool, username: &str) -> Uuid {
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(ARGON2_M_COST_FLOOR, 1, 1, None).expect("test parameters"),
    );
    let hash = identity::hash_password(&argon2, TEST_PASSWORD).expect("hash");
    users::create_user(pool, username, "P1b Test", &hash)
        .await
        .expect("user created")
        .id
}

async fn scoped_role(
    pool: &PgPool,
    name: &str,
    cooperative_id: Uuid,
    permissions: &[&str],
) -> Uuid {
    let role_id: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (name, description, cooperative_id) \
         VALUES ($1, 'p1b test', $2) RETURNING id",
    )
    .bind(name)
    .bind(cooperative_id)
    .fetch_one(pool)
    .await
    .expect("scoped role insert");
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission_id) \
         SELECT $1, id FROM permissions WHERE key = ANY($2)",
    )
    .bind(role_id)
    .bind(permissions)
    .execute(pool)
    .await
    .expect("role permissions");
    role_id
}

async fn assign(pool: &PgPool, user_id: Uuid, role_id: Uuid, cooperative_id: Uuid) {
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
         VALUES ($1, $2, $3)",
    )
    .bind(user_id)
    .bind(role_id)
    .bind(cooperative_id)
    .execute(pool)
    .await
    .expect("scoped assignment");
}

async fn membership_status(pool: &PgPool, coop: Uuid, user: Uuid, status: &str) {
    sqlx::query(
        "UPDATE cooperative_memberships SET status = $3 \
         WHERE cooperative_id = $1 AND user_id = $2",
    )
    .bind(coop)
    .bind(user)
    .bind(status)
    .execute(pool)
    .await
    .expect("membership status");
}

fn ctx_for(user_id: Uuid, cooperative_id: Uuid) -> TenantCtx {
    TenantCtx {
        auth: CurrentAuth {
            user_id,
            username: "test".into(),
            display_name: "Test".into(),
            session_id: Uuid::new_v4(),
            session_created_at: OffsetDateTime::now_utc(),
            session_expires_at: OffsetDateTime::now_utc(),
            csrf_token: "csrf".into(),
            active_cooperative_id: None,
        },
        cooperative_id,
        cooperative_name: "Test".into(),
    }
}

async fn perms(pool: &PgPool, user: Uuid, coop: Uuid) -> HashSet<String> {
    authz::effective_permissions_in_cooperative(pool, user, coop)
        .await
        .expect("scoped resolution")
}

/// Standard fixture: user is an active member of A (bootstrap) and B;
/// `a_perms` via role A, `b_perms` via role B.
async fn dual_membership_fixture(
    pool: &PgPool,
    username: &str,
    b_name: &str,
    a_perms: &[&str],
    b_perms: &[&str],
) -> (Uuid, Uuid, Uuid) {
    let user = create_user(pool, username).await;
    let (coop_a, _) = repo::bootstrap_initial_cooperative(pool, "A Kooperatifi")
        .await
        .expect("bootstrap A");
    let coop_b = repo::create_cooperative(pool, b_name, None)
        .await
        .expect("coop B")
        .id;
    repo::add_member(pool, b_name, user, None)
        .await
        .expect("B membership");
    if !a_perms.is_empty() {
        let role_a = scoped_role(pool, &format!("{username}-a"), coop_a, a_perms).await;
        assign(pool, user, role_a, coop_a).await;
    }
    if !b_perms.is_empty() {
        let role_b = scoped_role(pool, &format!("{username}-b"), coop_b, b_perms).await;
        assign(pool, user, role_b, coop_b).await;
    }
    (user, coop_a, coop_b)
}

#[tokio::test]
async fn admin_in_a_viewer_in_b_resolves_per_cooperative() {
    let Some(test) = setup().await else { return };
    let (user, coop_a, coop_b) = dual_membership_fixture(
        &test.pool,
        "admin_a_viewer_b",
        "B1 Kooperatifi",
        &["roles.manage", "roles.read", "users.manage"],
        &["roles.read"],
    )
    .await;

    let a = perms(&test.pool, user, coop_a).await;
    assert!(a.contains("roles.manage") && a.contains("users.manage"));
    let b = perms(&test.pool, user, coop_b).await;
    assert!(b.contains("roles.read"));
    assert!(
        !b.contains("roles.manage") && !b.contains("users.manage"),
        "A-admin rights must never leak into B"
    );

    // Reverse profile: B-admin, A-viewer.
    let (user2, coop_a, coop_b) = dual_membership_fixture(
        &test.pool,
        "viewer_a_admin_b",
        "B2 Kooperatifi",
        &["roles.read"],
        &["roles.manage", "users.manage"],
    )
    .await;
    let a2 = perms(&test.pool, user2, coop_a).await;
    assert!(a2.contains("roles.read") && !a2.contains("roles.manage"));
    let b2 = perms(&test.pool, user2, coop_b).await;
    assert!(b2.contains("roles.manage") && b2.contains("users.manage"));

    test.cleanup().await;
}

#[tokio::test]
async fn membership_status_and_scope_boundaries() {
    let Some(test) = setup().await else { return };

    // A-member / B-nonmember: resolution in B returns nothing.
    let user = create_user(&test.pool, "yalniz_a").await;
    let (coop_a, _) = repo::bootstrap_initial_cooperative(&test.pool, "A Kooperatifi")
        .await
        .expect("bootstrap");
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;
    let role_a = scoped_role(&test.pool, "a-yonetici", coop_a, &["roles.manage"]).await;
    assign(&test.pool, user, role_a, coop_a).await;
    assert!(perms(&test.pool, user, coop_a)
        .await
        .contains("roles.manage"));
    assert!(
        perms(&test.pool, user, coop_b).await.is_empty(),
        "nonmember resolves to empty set"
    );

    // Suspended membership → immediate zero permissions.
    membership_status(&test.pool, coop_a, user, "suspended").await;
    assert!(
        perms(&test.pool, user, coop_a).await.is_empty(),
        "suspended membership resolves nothing"
    );

    // Ended (revoked) membership → still nothing.
    membership_status(&test.pool, coop_a, user, "ended").await;
    assert!(perms(&test.pool, user, coop_a).await.is_empty());

    // Zero membership at all → nothing anywhere.
    let nobody = create_user(&test.pool, "uyesiz").await;
    assert!(perms(&test.pool, nobody, coop_a).await.is_empty());

    // Unknown / invalid cooperative scope → empty set, never an error or
    // a fallback to global permissions.
    assert!(perms(&test.pool, user, Uuid::new_v4()).await.is_empty());

    test.cleanup().await;
}

#[tokio::test]
async fn legacy_null_lane_is_excluded_from_scoped_resolution() {
    let Some(test) = setup().await else { return };

    // Legacy rows (NULL cooperative_id) keep the GLOBAL path working —
    // bootstrap compatibility (P1-K3) — but must not leak into scoped
    // resolution.
    let user = create_user(&test.pool, "legacy_kullanici").await;
    let (coop_a, _) = repo::bootstrap_initial_cooperative(&test.pool, "A Kooperatifi")
        .await
        .expect("bootstrap");
    let legacy_role: Uuid =
        sqlx::query_scalar("INSERT INTO roles (name) VALUES ('Eski Global Rol') RETURNING id")
            .fetch_one(&test.pool)
            .await
            .expect("legacy role");
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission_id) \
         SELECT $1, id FROM permissions WHERE key = 'roles.manage'",
    )
    .bind(legacy_role)
    .execute(&test.pool)
    .await
    .expect("legacy perms");
    sqlx::query("INSERT INTO user_role_assignments (user_id, role_id) VALUES ($1, $2)")
        .bind(user)
        .bind(legacy_role)
        .execute(&test.pool)
        .await
        .expect("legacy assignment");

    // Scoped view: NULL lane excluded.
    let scoped = perms(&test.pool, user, coop_a).await;
    assert!(
        scoped.is_empty(),
        "NULL-cooperative rows must not grant scoped permissions"
    );

    // Global view unchanged — P1-b does not break existing authorization
    // (legacy call sites migrate in P1-c).
    let global = authz::effective_permissions(&test.pool, user)
        .await
        .expect("global resolution");
    assert!(global.contains("roles.manage"), "global path preserved");

    test.cleanup().await;
}

#[tokio::test]
async fn resolution_is_fresh_per_request() {
    let Some(test) = setup().await else { return };

    let (user, coop_a, _) = dual_membership_fixture(
        &test.pool,
        "taze_uye",
        "B3 Kooperatifi",
        &["payments.read"],
        &[],
    )
    .await;

    assert!(perms(&test.pool, user, coop_a)
        .await
        .contains("payments.read"));

    // Permission removed mid-"session" (no caching — next resolution
    // sees it).
    sqlx::query(
        "DELETE FROM role_permissions WHERE role_id IN \
           (SELECT role_id FROM user_role_assignments WHERE user_id = $1)",
    )
    .bind(user)
    .execute(&test.pool)
    .await
    .expect("perm revoked");
    assert!(perms(&test.pool, user, coop_a).await.is_empty());

    // Role disabled → excluded even if assignment remains.
    let role_b = scoped_role(&test.pool, "taze-b-rolu", coop_a, &["shares.read"]).await;
    assign(&test.pool, user, role_b, coop_a).await;
    sqlx::query("UPDATE roles SET status = 'disabled' WHERE id = $1")
        .bind(role_b)
        .execute(&test.pool)
        .await
        .expect("role disabled");
    assert!(!perms(&test.pool, user, coop_a)
        .await
        .contains("shares.read"));

    test.cleanup().await;
}

#[tokio::test]
async fn concurrent_scopes_resolve_independently() {
    let Some(test) = setup().await else { return };

    let (user, coop_a, coop_b) = dual_membership_fixture(
        &test.pool,
        "eszamanli",
        "B4 Kooperatifi",
        &["payments.manage"],
        &["payments.read"],
    )
    .await;

    let (a, b) = tokio::join!(
        perms(&test.pool, user, coop_a),
        perms(&test.pool, user, coop_b)
    );
    assert!(a.contains("payments.manage"));
    assert!(b.contains("payments.read") && !b.contains("payments.manage"));

    test.cleanup().await;
}

#[tokio::test]
async fn require_in_cooperative_enforces_verified_context() {
    let Some(test) = setup().await else { return };

    let (user, coop_a, coop_b) = dual_membership_fixture(
        &test.pool,
        "zorlama",
        "B5 Kooperatifi",
        &["shares.manage"],
        &[],
    )
    .await;

    // Granted in A.
    authz::require_in_cooperative(&test.state, &ctx_for(user, coop_a), "shares.manage")
        .await
        .expect("A-scope permission granted");

    // Not granted permission in A → denied.
    let denied = authz::require_in_cooperative(&test.state, &ctx_for(user, coop_a), "roles.manage")
        .await
        .expect_err("ungranted permission must deny");
    assert!(matches!(denied, ApiError::PermissionDenied));

    // Member of B but no scoped rights there → denied.
    authz::require_in_cooperative(&test.state, &ctx_for(user, coop_b), "shares.manage")
        .await
        .expect_err("no cross-coop privilege");

    // Nonexistent cooperative → denied, never global fallback.
    authz::require_in_cooperative(&test.state, &ctx_for(user, Uuid::new_v4()), "shares.manage")
        .await
        .expect_err("invalid scope must deny");

    // Suspended membership → denied immediately.
    membership_status(&test.pool, coop_a, user, "suspended").await;
    authz::require_in_cooperative(&test.state, &ctx_for(user, coop_a), "shares.manage")
        .await
        .expect_err("suspended membership denies");

    test.cleanup().await;
}

#[tokio::test]
async fn scoped_role_summaries_follow_the_same_rules() {
    let Some(test) = setup().await else { return };

    let (user, coop_a, coop_b) = dual_membership_fixture(
        &test.pool,
        "ozet",
        "B6 Kooperatifi",
        &["roles.read"],
        &["shares.read"],
    )
    .await;

    let a_roles = authz::active_role_summaries_in_cooperative(&test.pool, user, coop_a)
        .await
        .expect("A summaries");
    assert_eq!(a_roles.len(), 1);
    assert_eq!(a_roles[0].1, "ozet-a");
    let b_roles = authz::active_role_summaries_in_cooperative(&test.pool, user, coop_b)
        .await
        .expect("B summaries");
    assert_eq!(b_roles.len(), 1);
    assert_eq!(b_roles[0].1, "ozet-b");

    test.cleanup().await;
}

// ------------------------------------------------------------------
// M1-P1-007 (P1-c2b) — cooperative-scoped administrator continuity:
// scoped path count, scoped advisory-lock keys, hardened bootstrap
// attribution. DB-level proofs backing the HTTP tests in
// rbac_cooperative.rs.
// ------------------------------------------------------------------

/// Write a NULL-cooperative role + NULL-cooperative assignment (the
/// legacy lane) carrying `permissions` for `user`.
async fn null_admin_grant(pool: &PgPool, user: Uuid, name: &str, permissions: &[&str]) -> Uuid {
    let role_id: Uuid = sqlx::query_scalar("INSERT INTO roles (name) VALUES ($1) RETURNING id")
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("null role");
    for key in permissions {
        sqlx::query(
            "INSERT INTO role_permissions (role_id, permission_id) \
             SELECT $1, id FROM permissions WHERE key = $2",
        )
        .bind(role_id)
        .bind(key)
        .execute(pool)
        .await
        .expect("perm");
    }
    sqlx::query("INSERT INTO user_role_assignments (user_id, role_id) VALUES ($1, $2)")
        .bind(user)
        .bind(role_id)
        .execute(pool)
        .await
        .expect("null assignment");
    role_id
}

/// Cooperative-scoped `administration_path_count` (P1-K5): counts only
/// paths EFFECTIVE in the queried cooperative — scoped role + scoped
/// assignment + active membership + active user, with NULL-attributed
/// legacy rows counting ONLY for the marker-verified bootstrap
/// cooperative.
#[tokio::test]
async fn scoped_administration_path_count_is_per_cooperative() {
    let Some(test) = setup().await else { return };

    let (coop_a, _) = repo::bootstrap_initial_cooperative(&test.pool, "A Kooperatifi")
        .await
        .expect("bootstrap");
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;

    // A-admin: scoped role + scoped assignment + active A membership.
    let a_admin = create_user(&test.pool, "sayim_a").await;
    repo::add_member(&test.pool, "A Kooperatifi", a_admin, None)
        .await
        .expect("A membership");
    let role_a = scoped_role(&test.pool, "sayim-a-admin", coop_a, &["roles.manage"]).await;
    assign(&test.pool, a_admin, role_a, coop_a).await;

    // B-admin: fully B-scoped path.
    let b_admin = create_user(&test.pool, "sayim_b").await;
    repo::add_member(&test.pool, "B Kooperatifi", b_admin, None)
        .await
        .expect("B membership");
    let role_b = scoped_role(&test.pool, "sayim-b-admin", coop_b, &["roles.manage"]).await;
    assign(&test.pool, b_admin, role_b, coop_b).await;

    // NULL-attributed legacy admin: bootstrap membership + NULL role +
    // NULL assignment — effective ONLY in the bootstrap scope.
    let null_admin = create_user(&test.pool, "sayim_null").await;
    repo::add_member(&test.pool, "A Kooperatifi", null_admin, None)
        .await
        .expect("A membership");
    null_admin_grant(
        &test.pool,
        null_admin,
        "sayim-null-admin",
        &["roles.manage"],
    )
    .await;

    let mut tx = test.pool.begin().await.expect("tx");
    let count_a = authz::administration_path_count(&mut tx, Some(coop_a))
        .await
        .expect("count A");
    assert_eq!(count_a, 2, "scoped A path + NULL-attributed A path");
    let count_b = authz::administration_path_count(&mut tx, Some(coop_b))
        .await
        .expect("count B");
    assert_eq!(
        count_b, 1,
        "B sees ONLY its own admin path — NULL never leaks"
    );
    tx.rollback().await.expect("rollback");

    // Suspending A's scoped admin drops A's count but never B's.
    membership_status(&test.pool, coop_a, a_admin, "suspended").await;
    let mut tx = test.pool.begin().await.expect("tx");
    assert_eq!(
        authz::administration_path_count(&mut tx, Some(coop_a))
            .await
            .expect("count A"),
        1,
        "suspended membership no longer counts as a path"
    );
    assert_eq!(
        authz::administration_path_count(&mut tx, Some(coop_b))
            .await
            .expect("count B"),
        1
    );
    tx.rollback().await.expect("rollback");

    // The legacy global arm (None) counts every path regardless of
    // scope — preserved for the never-bootstrapped lane.
    let mut tx = test.pool.begin().await.expect("tx");
    assert_eq!(
        authz::administration_path_count(&mut tx, None)
            .await
            .expect("global count"),
        3
    );
    tx.rollback().await.expect("rollback");

    test.cleanup().await;
}

/// Malicious/incorrectly supplied scope values: the hardened bootstrap
/// attribution (P1-c2b audit) can never assign NULL legacy rows to a
/// non-bootstrap cooperative — even for a genuine member of that
/// cooperative. `effective_permissions_in_bootstrap` re-verifies the
/// marker inside the query.
#[tokio::test]
async fn null_legacy_never_resolves_in_foreign_scope() {
    let Some(test) = setup().await else { return };

    let (coop_a, _) = repo::bootstrap_initial_cooperative(&test.pool, "A Kooperatifi")
        .await
        .expect("bootstrap");
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;

    // User is an ACTIVE member of B only, holding a NULL-attributed
    // admin grant (legacy lane). In B's scope this grants NOTHING.
    let user = create_user(&test.pool, "yabanci_scope").await;
    repo::add_member(&test.pool, "B Kooperatifi", user, None)
        .await
        .expect("B membership");
    null_admin_grant(&test.pool, user, "yabanci-null", &["roles.manage"]).await;

    let b_perms = authz::effective_permissions_in_bootstrap(&test.pool, user, coop_b)
        .await
        .expect("B resolution");
    assert!(
        b_perms.is_empty(),
        "NULL attribution must never apply to a non-bootstrap scope: {b_perms:?}"
    );

    // In the real bootstrap scope the same NULL rows grant — but only
    // through an active bootstrap membership, which this user lacks.
    let a_perms = authz::effective_permissions_in_bootstrap(&test.pool, user, coop_a)
        .await
        .expect("A resolution");
    assert!(
        a_perms.is_empty(),
        "no bootstrap membership → NULL grant resolves to nothing"
    );

    // Add the bootstrap membership: the NULL grant now resolves in the
    // bootstrap scope (verified legacy attribution) — still not in B.
    repo::add_member(&test.pool, "A Kooperatifi", user, None)
        .await
        .expect("A membership");
    let a_perms = authz::effective_permissions_in_bootstrap(&test.pool, user, coop_a)
        .await
        .expect("A resolution after membership");
    assert!(a_perms.contains("roles.manage"));
    let b_perms = authz::effective_permissions_in_bootstrap(&test.pool, user, coop_b)
        .await
        .expect("B resolution after A membership");
    assert!(b_perms.is_empty(), "B scope still sees nothing");

    test.cleanup().await;
}

/// Advisory-lock key strategy (P1-K5): `begin_guarded_transaction`
/// derives a per-cooperative key — same cooperative serializes,
/// different cooperatives never block each other, and the two-int
/// scoped form cannot collide with the legacy bigint key.
#[tokio::test]
async fn scoped_advisory_lock_isolates_per_cooperative() {
    let Some(test) = setup().await else { return };

    let coop_a = repo::bootstrap_initial_cooperative(&test.pool, "A Kooperatifi")
        .await
        .expect("bootstrap")
        .0;
    let coop_b = repo::create_cooperative(&test.pool, "B Kooperatifi", None)
        .await
        .expect("coop B")
        .id;

    // Transaction holds the A-scoped guard lock.
    let tx_a = authz::begin_guarded_transaction(&test.pool, Some(coop_a))
        .await
        .expect("A guard tx");

    let try_lock = |coop: Uuid| {
        let pool = test.pool.clone();
        async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT pg_try_advisory_xact_lock($1::int4, hashtext($2::text))",
            )
            .bind(authz::ADMIN_PATH_GUARD_LOCK_NAMESPACE)
            .bind(coop.to_string())
            .fetch_one(&pool)
            .await
            .expect("try lock")
        }
    };

    // Same cooperative → lock visibly held (serialization proven).
    assert!(
        !try_lock(coop_a).await,
        "same-cooperative mutation must serialize on the scoped key"
    );
    // Different cooperative → independent key, acquired at once.
    assert!(
        try_lock(coop_b).await,
        "another cooperative's mutations must never block on A's key"
    );

    // The legacy global key is a DIFFERENT lock space: a scoped lock
    // does not contend with the never-bootstrapped lane's bigint key,
    // and vice versa.
    let tx_global = authz::begin_guarded_transaction(&test.pool, None)
        .await
        .expect("global guard tx must not contend with scoped locks");

    // Release the guard transactions before cleanup — pool.close()
    // waits for checked-out connections to be returned.
    tx_global.rollback().await.expect("global rollback");
    tx_a.rollback().await.expect("A rollback");

    test.cleanup().await;
}

/// The guarded transaction composes: mutation + scoped count inside
/// ONE transaction, so the post-mutation administrator-continuity
/// check is evaluated atomically under the scoped lock.
#[tokio::test]
async fn guarded_tx_evaluates_post_mutation_count_atomically() {
    let Some(test) = setup().await else { return };

    let (coop_a, _) = repo::bootstrap_initial_cooperative(&test.pool, "A Kooperatifi")
        .await
        .expect("bootstrap");
    let admin = create_user(&test.pool, "tek_yol").await;
    repo::add_member(&test.pool, "A Kooperatifi", admin, None)
        .await
        .expect("A membership");
    let role = scoped_role(&test.pool, "tek-yol-admin", coop_a, &["roles.manage"]).await;
    assign(&test.pool, admin, role, coop_a).await;

    // Inside the guarded tx: delete the assignment, then the scoped
    // count observes ZERO and the caller would roll back.
    let mut tx = authz::begin_guarded_transaction(&test.pool, Some(coop_a))
        .await
        .expect("guard tx");
    sqlx::query("DELETE FROM user_role_assignments WHERE user_id = $1")
        .bind(admin)
        .execute(tx.as_mut())
        .await
        .expect("delete");
    let count = authz::administration_path_count(&mut tx, Some(coop_a))
        .await
        .expect("count");
    assert_eq!(count, 0, "post-mutation state must see zero paths");
    tx.rollback().await.expect("rollback");

    // Rolled back — the path survived outside the tx.
    let mut tx = test.pool.begin().await.expect("tx");
    assert_eq!(
        authz::administration_path_count(&mut tx, Some(coop_a))
            .await
            .expect("count"),
        1
    );
    tx.rollback().await.expect("rollback");

    test.cleanup().await;
}
