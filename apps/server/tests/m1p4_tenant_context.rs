//! M1-P4-P1 — transaction-local tenant context + pool-leak guard.
//!
//! Proves the plumbing that P3 RLS policies will read:
//!   * `tenant_tx` stamps `app.cooperative_id` / `app.user_id` scoped
//!     to the transaction via `set_config(…, true)`;
//!   * the pool's `before_acquire` reset wipes any session-level
//!     residue so a connection can never carry a previous borrower's
//!     tenant;
//!   * concurrent transactions on different cooperatives stay
//!     isolated;
//!   * the restricted `kooperatif_app` role exists with hardened
//!     attributes (migration 0032) and can read/write business tables
//!     but not `_sqlx_migrations`.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL — see tests/m1p3_numbering.rs.

use kooperatif_server::db as app_db;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

fn test_database_url() -> String {
    let value = std::env::var("KOOPERATIF_TEST_DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "KOOPERATIF_TEST_DATABASE_URL is not set — required DB-gated integration tests cannot silently pass; point it at a disposable PostgreSQL database"
        )
    });
    if value.trim().is_empty() {
        panic!("KOOPERATIF_TEST_DATABASE_URL is empty");
    }
    value
}

struct TestDb {
    _claim: app_db::TestDbClaim,
    pool: PgPool,
    admin_url: String,
    database_name: String,
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
    let admin_url = url
        .rsplit_once('/')
        .map(|(base, _)| format!("{base}/postgres"))
        .expect("database url must have a db segment");
    let base_url = url
        .rsplit_once('/')
        .map(|(base, _)| base.to_string())
        .expect("database url must have a db segment");
    let admin = app_db::connect(&admin_url)
        .await
        .expect("admin connectivity");
    let _ = app_db::drop_stale_test_databases(&admin).await;
    let suffix = Uuid::new_v4().simple();
    let database_name = format!("kooperatif_test_{suffix}");
    let claim = app_db::create_test_database(&admin, &admin_url, &database_name).await;
    admin.close().await;

    let pool = app_db::connect(&format!("{base_url}/{database_name}"))
        .await
        .expect("test db connectivity");
    app_db::run_test_migrations(&pool)
        .await
        .expect("migrations");
    TestDb {
        _claim: claim,
        pool,
        admin_url,
        database_name,
    }
}

#[tokio::test]
async fn tenant_tx_stamps_cooperative_and_user_gucs() {
    let db = setup().await;
    let coop = Uuid::new_v4();
    let user = Uuid::new_v4();

    let mut tx = app_db::tenant_tx(&db.pool, Some(coop), Some(user))
        .await
        .expect("tenant tx");
    let (guc_coop, guc_user): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT current_setting('app.cooperative_id', true), \
                current_setting('app.user_id', true)",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(guc_coop.as_deref(), Some(coop.to_string().as_str()));
    assert_eq!(guc_user.as_deref(), Some(user.to_string().as_str()));
    tx.commit().await.unwrap();

    db.cleanup().await;
}

#[tokio::test]
async fn tenant_gucs_are_transaction_local() {
    let db = setup().await;
    let coop = Uuid::new_v4();

    let tx = app_db::tenant_tx(&db.pool, Some(coop), None)
        .await
        .expect("tenant tx");
    tx.commit().await.unwrap();

    // A fresh pooled checkout (may reuse the same physical connection)
    // must observe NO tenant context — SET LOCAL cannot leak past the
    // transaction boundary.
    let guc: Option<String> =
        sqlx::query_scalar("SELECT current_setting('app.cooperative_id', true)")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(
        guc.is_none() || guc.as_deref() == Some(""),
        "tenant GUC leaked past the transaction: {guc:?}"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn session_level_residue_is_wiped_on_checkout() {
    let db = setup().await;
    let coop = Uuid::new_v4();

    // Pathological session-level SET (what a buggy caller or hostile
    // injection would plant) must be cleared by before_acquire.
    {
        let mut conn = db.pool.acquire().await.expect("conn");
        sqlx::raw_sql(&format!(
            "SET app.cooperative_id = '{coop}'; SET app.user_id = '{coop}'"
        ))
        .execute(&mut *conn)
        .await
        .unwrap();
        let live: Option<String> =
            sqlx::query_scalar("SELECT current_setting('app.cooperative_id', true)")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(live.as_deref(), Some(coop.to_string().as_str()));
        // conn drops → returns to pool with the residue still set
    }

    for attempt in 0..8 {
        let mut conn = db.pool.acquire().await.expect("reacquire");
        let (gc, gu): (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT current_setting('app.cooperative_id', true), \
                    current_setting('app.user_id', true)",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert!(
            gc.is_none() || gc.as_deref() == Some(""),
            "attempt {attempt}: cooperative GUC leaked across checkout: {gc:?}"
        );
        assert!(
            gu.is_none() || gu.as_deref() == Some(""),
            "attempt {attempt}: user GUC leaked across checkout: {gu:?}"
        );
    }

    db.cleanup().await;
}

#[tokio::test]
async fn concurrent_tenant_transactions_are_isolated() {
    let db = setup().await;
    let coop_a = Uuid::new_v4();
    let coop_b = Uuid::new_v4();

    let mut tasks = Vec::new();
    for coop in [coop_a, coop_b] {
        let pool = db.pool.clone();
        tasks.push(tokio::spawn(async move {
            let mut tx = app_db::tenant_tx(&pool, Some(coop), None)
                .await
                .expect("tenant tx");
            // Hold the tx open across a scheduling boundary while a
            // concurrent tx on the OTHER cooperative runs.
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            let guc: Option<String> =
                sqlx::query_scalar("SELECT current_setting('app.cooperative_id', true)")
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
            tx.commit().await.unwrap();
            guc
        }));
    }

    let mut seen = Vec::new();
    for task in tasks {
        seen.push(task.await.unwrap());
    }
    let expected = [coop_a.to_string(), coop_b.to_string()];
    for guc in seen {
        assert!(
            expected.contains(&guc.clone().unwrap_or_default()),
            "each tx must see its own cooperative"
        );
    }

    db.cleanup().await;
}

#[tokio::test]
async fn app_role_exists_with_hardened_attributes() {
    let db = setup().await;

    let row: Option<(bool, bool, bool)> = sqlx::query_as(
        "SELECT rolsuper, rolcreaterole, rolcreatedb FROM pg_roles \
         WHERE rolname = 'kooperatif_app'",
    )
    .fetch_optional(&db.pool)
    .await
    .unwrap();
    let Some((sup, createrole, createdb)) = row else {
        panic!("kooperatif_app role missing after migration 0032");
    };
    let (repl, bypass, canlogin): (bool, bool, bool) = sqlx::query_as(
        "SELECT rolreplication, rolbypassrls, rolcanlogin FROM pg_roles \
         WHERE rolname = 'kooperatif_app'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(
        !sup && !createrole && !createdb && !repl && !bypass && canlogin,
        "kooperatif_app must be a hardened login role: \
         super={sup} createrole={createrole} createdb={createdb} repl={repl} bypassrls={bypass}"
    );

    // Grant surface: business reads + inserts allowed; R3 (0054)
    // removed table-level UPDATE/DELETE — lifecycle columns are
    // granted per-column and posted history is non-deletable.
    let can_read_insert: bool = sqlx::query_scalar(
        "SELECT has_table_privilege('kooperatif_app', 'payments', 'SELECT,INSERT')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(
        can_read_insert,
        "kooperatif_app must keep SELECT+INSERT on business tables"
    );
    for (verb, what) in [("UPDATE", "payments UPDATE"), ("DELETE", "payments DELETE")] {
        let held: bool = sqlx::query_scalar(&format!(
            "SELECT has_table_privilege('kooperatif_app', 'payments', '{verb}')"
        ))
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert!(!held, "{what} must not be a table-level grant (R3)");
    }
    // R3B (0055): lifecycle columns are no longer writable at all —
    // reversals run only through permission-checked SECURITY DEFINERs.
    let status_col: bool = sqlx::query_scalar(
        "SELECT has_column_privilege('kooperatif_app', 'payments', 'status', 'UPDATE')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(
        !status_col,
        "payments.status UPDATE must be denied — reversals go through app_payment_reverse_tx (R3C)"
    );
    let component_exec: bool = sqlx::query_scalar(
        "SELECT has_function_privilege('kooperatif_app', \
         'public.app_payment_reverse(uuid,timestamp with time zone,uuid,text,uuid)', 'EXECUTE')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(
        !component_exec,
        "component definer must NOT be executable — incomplete reversals are impossible (R3C)"
    );
    let package_exec: bool = sqlx::query_scalar(
        "SELECT has_function_privilege('kooperatif_app', \
         'public.app_payment_reverse_tx(uuid,timestamp with time zone,uuid,text,uuid)', 'EXECUTE')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(
        package_exec,
        "kooperatif_app must EXECUTE the atomic reversal package (R3C)"
    );
    let amount_col: bool = sqlx::query_scalar(
        "SELECT has_column_privilege('kooperatif_app', 'payments', 'amount', 'UPDATE')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(!amount_col, "payments.amount UPDATE must be denied (R3)");
    let ledger: bool = sqlx::query_scalar(
        "SELECT has_table_privilege('kooperatif_app', '_sqlx_migrations', 'SELECT,INSERT,UPDATE,DELETE')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(!ledger, "kooperatif_app must NOT touch _sqlx_migrations");

    // P3C: the membership resolver is no longer directly callable by
    // the runtime — with an unstamped connection it resolved ARBITRARY
    // (user, coop) pairs, an enumeration oracle. Internal definer
    // callers (app_stamp_cooperative, app_session_set_cooperative) are
    // unaffected because EXECUTE is not needed inside a SECURITY
    // DEFINER chain.
    let callable: bool = sqlx::query_scalar(
        "SELECT has_function_privilege('kooperatif_app', 'app_verify_membership(uuid,uuid)', 'EXECUTE')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(
        !callable,
        "app_verify_membership must NOT be executable by kooperatif_app"
    );

    // Owner lane still resolves; non-member pairs return no row.
    let fake_coop = Uuid::new_v4();
    let fake_user = Uuid::new_v4();
    let found: i64 = sqlx::query_scalar("SELECT count(*) FROM app_verify_membership($1, $2)")
        .bind(fake_user)
        .bind(fake_coop)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(found, 0, "non-member pair must resolve to no row");

    db.cleanup().await;
}

#[tokio::test]
async fn membership_shim_returns_minimal_facts_and_is_secdef() {
    let db = setup().await;

    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, display_name, password_hash) \
         VALUES ('m1p4_user', 'M1P4 User', 'x') RETURNING id",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let coop: Uuid = sqlx::query_scalar(
        "INSERT INTO cooperatives (name, status, is_bootstrap, business_enabled) \
         VALUES ('m1p4-coop', 'active', false, true) RETURNING id",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         VALUES ($1, $2, 'active')",
    )
    .bind(coop)
    .bind(user)
    .execute(&db.pool)
    .await
    .unwrap();

    let (status, name, coop_status, enabled): (String, String, String, bool) =
        sqlx::query_as("SELECT * FROM app_verify_membership($1, $2)")
            .bind(user)
            .bind(coop)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(status, "active");
    assert_eq!(name, "m1p4-coop");
    assert_eq!(coop_status, "active");
    assert!(enabled);

    // The shim must be a SECURITY DEFINER function with a pinned,
    // non-caller-controlled search_path — its whole point is running
    // membership reads as the owner before a tenant context exists.
    let (secdef, search_path): (bool, Option<Vec<String>>) = sqlx::query_as(
        "SELECT prosecdef, proconfig FROM pg_proc \
         WHERE proname = 'app_verify_membership' \
           AND pronamespace = 'public'::regnamespace",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(secdef, "app_verify_membership must be SECURITY DEFINER");
    let cfg = search_path.unwrap_or_default();
    assert!(
        cfg.iter().any(|c| c.contains("search_path")),
        "search_path must be pinned, got {cfg:?}"
    );

    // Bootstrap shim: empty before bootstrap, returns the bootstrap
    // cooperative once installation_state points at it.
    let empty: i64 = sqlx::query_scalar("SELECT count(*) FROM app_resolve_bootstrap()")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(empty, 0);

    let boot: Uuid = sqlx::query_scalar(
        "INSERT INTO cooperatives (name, status, is_bootstrap, business_enabled) \
         VALUES ('m1p4-boot', 'active', true, true) RETURNING id",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE installation_state SET bootstrap_cooperative_id = $1, \
         tenant_bootstrapped_at = now() WHERE singleton",
    )
    .bind(boot)
    .execute(&db.pool)
    .await
    .unwrap();
    let resolved: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM app_resolve_bootstrap()")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(resolved, Some(boot));

    db.cleanup().await;
}

#[tokio::test]
async fn migration_0032_is_idempotent_and_rls_stays_disabled() {
    let db = setup().await;

    // Version 32 must be recorded exactly once.
    let recorded: i64 =
        sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE version = 32 AND success")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(recorded, 1);

    // Re-running the full migrator must be a no-op success — no role
    // or privilege drift on repeat application.
    app_db::run_test_migrations(&db.pool)
        .await
        .expect("migration re-run must be idempotent");

    // P1/P2 ship plumbing only; P3A enables the bounded RLS pilot and
    // Batch 1 (0040) extends it to six leaf tables — ENABLE+FORCE on
    // exactly `periods`, `governance_bodies`, `income_entries`,
    // `expense_entries`, `payment_allocations`,
    // `shareholder_family_memberships`, `governance_votes`,
    // `investment_incomes`, plus batch 2 (`payments`, `assessments`,
    // `assessment_rules`, `assessment_share_sources`,
    // `shareholder_credits`, `credit_applications`), batch 3
    // (`investments`, `investment_fundings`, `investment_valuations`,
    // `investment_disposals`, `investment_disposal_proceeds`,
    // `governance_decisions`), batch 4 (`persons`, `families`,
    // `shareholders`, `shares`, `share_events`, `share_ownerships`),
    // batch 5 (`share_returns`, `share_return_entitlements`,
    // `share_return_settlements`, `social_aid_funds`,
    // `social_aid_donations`, `social_aid_disbursements`) and batch 6
    // (`financial_accounts`, `financial_categories`,
    // `account_movements`, `account_transfers`,
    // `governance_memberships`), S1 (`cooperative_number_counters`),
    // S3 (`security_events`), S4 (`cooperative_memberships`) and S5
    // (`roles`, `user_role_assignments`), nothing else — every table
    // carries exactly owner + app policies except the counters table
    // and security_events, which carry ONLY the owner lane by design
    // (the runtime role must never see counters or audit rows
    // directly; allocation and audit writes run through definer
    // boundaries). Migration post-checks enforce the same bound inside
    // the migration.
    let rls_tables: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relkind = 'r' \
           AND (c.relrowsecurity OR c.relforcerowsecurity) \
         ORDER BY c.relname",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        rls_tables,
        vec![
            "account_movements",
            "account_transfers",
            "assessment_rules",
            "assessment_share_sources",
            "assessments",
            "cooperative_memberships",
            "cooperative_number_counters",
            "credit_applications",
            "expense_entries",
            "families",
            "financial_accounts",
            "financial_categories",
            "governance_bodies",
            "governance_decisions",
            "governance_memberships",
            "governance_votes",
            "income_entries",
            "investment_disposal_proceeds",
            "investment_disposals",
            "investment_fundings",
            "investment_incomes",
            "investment_valuations",
            "investments",
            "payment_allocations",
            "payments",
            "periods",
            "persons",
            "roles",
            "security_events",
            "share_events",
            "share_ownerships",
            "share_return_entitlements",
            "share_return_settlements",
            "share_returns",
            "shareholder_credits",
            "shareholder_family_memberships",
            "shareholders",
            "shares",
            "social_aid_disbursements",
            "social_aid_donations",
            "social_aid_funds",
            "user_role_assignments",
            "users",
        ],
        "RLS must stay bounded to the pilot + batch-1..6 + S1/S3/S4/S5 + R2 users"
    );
    let policies: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pg_policies WHERE schemaname = 'public'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    // 37 business tables × 2 policies + counters/security_events
    // owner-only lanes + memberships/roles/assignments/users owner+app
    // = 84, plus R3 (0054) RESTRICTIVE guards: 18 terminal-transition
    // FOR UPDATE + 18 FOR INSERT live-status + draft-confined
    // periods/assessment_rules delete/update/insert = 40.
    assert_eq!(
        policies, 124,
        "owner + app policies per RLS table + R3 restrictive guards"
    );

    db.cleanup().await;
}

/// P2A — the runtime principal must hold ONLY the verified verb set.
/// Exercised under `SET ROLE kooperatif_app` (the role is intentionally
/// passwordless in local dev, so tests impersonate rather than
/// reconnect).
#[tokio::test]
async fn app_role_holds_only_the_minimal_grant_surface() {
    let db = setup().await;

    let coop: Uuid = sqlx::query_scalar(
        "INSERT INTO cooperatives (name, status, is_bootstrap, business_enabled) \
         VALUES ('p2a-grants', 'active', false, true) RETURNING id",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let mut conn = db.pool.acquire().await.unwrap();
    sqlx::raw_sql("SET ROLE kooperatif_app")
        .execute(&mut *conn)
        .await
        .unwrap();

    // --- allowed: business CRUD, allocator, and the exact system verbs ---
    // P3C: the allocator is now SECURITY DEFINER and demands a sealed
    // cooperative context when the REAL session_user is kooperatif_app.
    // Under `SET ROLE` the session principal is still the owner, so the
    // owner lane runs unsealed here — the sealed-path check is covered
    // by m1p4_rls_pilot (request_tx) and the denial below covers the
    // raw app lane.
    let allocated: i64 =
        sqlx::query_scalar("SELECT public.allocate_document_number($1, 'payments.payment_number')")
            .bind(coop)
            .fetch_one(&mut *conn)
            .await
            .expect("allocator must run on the owner/SET ROLE lane");
    assert_eq!(allocated, 1);

    // R2: user status mutations moved behind app_user_set_status —
    // no direct users UPDATE remains for the runtime role.
    // Non-credential SELECT columns stay readable (RBAC lookups); R2
    // narrowed the surface to the four columns the runtime uses.
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM (SELECT id, username, display_name, status \
                FROM users) t",
    )
    .fetch_one(&mut *conn)
    .await
    .expect("users non-credential SELECT must be allowed");
    // --- denied: provisioning/migration/identity lanes ---
    let denied_cases: &[(&str, &str)] = &[
        (
            "INSERT INTO users (username, display_name, password_hash) \
             VALUES ('x','x','x')",
            "users INSERT (create-user is CLI-only)",
        ),
        ("DELETE FROM users", "users DELETE"),
        (
            "UPDATE users SET status = 'disabled'",
            "users UPDATE(status) — R2: status changes go through \
             app_user_set_status",
        ),
        (
            "SELECT last_login_at FROM users LIMIT 1",
            "users.last_login_at SELECT (R2 column narrowing)",
        ),
        (
            "INSERT INTO cooperatives (name) VALUES ('x')",
            "cooperatives INSERT (provisioning lane)",
        ),
        ("UPDATE cooperatives SET name = 'x'", "cooperatives UPDATE"),
        (
            "INSERT INTO cooperative_memberships (cooperative_id, user_id) \
             VALUES (gen_random_uuid(), gen_random_uuid())",
            "cooperative_memberships INSERT (add-member lane)",
        ),
        (
            "UPDATE cooperative_memberships SET status = 'ended'",
            "cooperative_memberships UPDATE",
        ),
        (
            "UPDATE installation_state SET bootstrap_cooperative_id = NULL",
            "installation_state UPDATE (bootstrap lane)",
        ),
        ("DELETE FROM roles", "roles DELETE"),
        (
            "UPDATE permissions SET name = 'x'",
            "permissions UPDATE (read-only catalog)",
        ),
        (
            "INSERT INTO document_number_series (series_key) VALUES ('x.y')",
            "document_number_series INSERT (registry is migration-owned)",
        ),
        (
            "SELECT version FROM _sqlx_migrations LIMIT 1",
            "_sqlx_migrations even for SELECT",
        ),
        (
            "INSERT INTO security_events (event_type, metadata) \
             VALUES ('login_failed', '{}'::jsonb)",
            "security_events INSERT (writes go through S2B definers)",
        ),
        (
            "SELECT event_type FROM security_events LIMIT 1",
            "security_events SELECT (table fully closed in S3; the \
             never_bootstrapped probe runs inside a definer)",
        ),
        (
            "UPDATE security_events SET metadata = '{}'::jsonb",
            "security_events UPDATE (append-only writer)",
        ),
        (
            "DELETE FROM user_sessions",
            "user_sessions DELETE (sessions are revoked, not deleted)",
        ),
        // P3B: session table fully closed; session ops run through the
        // credential-bound definer API only.
        (
            "SELECT token_hash FROM user_sessions LIMIT 1",
            "user_sessions SELECT (hash enumeration vector)",
        ),
        (
            "INSERT INTO user_sessions (user_id, token_hash, csrf_token, \
             created_at, last_seen_at, expires_at, idle_expires_at) \
             VALUES (gen_random_uuid(), '\\x00', 'x', now(), now(), \
                     now() + interval '1 day', now() + interval '1 day')",
            "user_sessions INSERT (session fabrication vector)",
        ),
        (
            "UPDATE user_sessions SET revoked_at = now()",
            "user_sessions UPDATE (revocation goes through the API)",
        ),
        // Credential columns on users are column-invisible.
        (
            "SELECT password_hash FROM users LIMIT 1",
            "users.password_hash SELECT",
        ),
        (
            "SELECT login_verifier FROM users LIMIT 1",
            "users.login_verifier SELECT",
        ),
        (
            "UPDATE users SET password_hash = 'x'",
            "users.password_hash UPDATE",
        ),
        (
            "UPDATE users SET display_name = 'x'",
            "users.display_name UPDATE (non-RBAC column)",
        ),
        // P3C: numbering counters moved fully behind the definer
        // allocators — no direct reads or writes for the runtime role
        // (grant-level denial; S1/0046 adds FORCE RLS on top, proven in
        // the m1p4_rls_pilot s1_* tests).
        (
            "SELECT next_value FROM cooperative_number_counters LIMIT 1",
            "cooperative_number_counters SELECT",
        ),
        (
            "UPDATE cooperative_number_counters SET next_value = 1",
            "cooperative_number_counters UPDATE",
        ),
        (
            "INSERT INTO cooperative_number_counters \
             (cooperative_id, series_key, next_value) \
             VALUES (gen_random_uuid(), 'x.y', 1)",
            "cooperative_number_counters INSERT",
        ),
        // Seal material stays owner-only.
        ("SELECT value FROM app_secrets", "app_secrets SELECT"),
        (
            "SELECT public.app_seal_of(gen_random_uuid(), gen_random_uuid(), \
             gen_random_uuid(), 1)",
            "app_seal_of EXECUTE (internal mint primitive)",
        ),
    ];
    for (sql, what) in denied_cases {
        let err = sqlx::raw_sql(sql)
            .execute(&mut *conn)
            .await
            .expect_err(&format!("{what} must be denied for kooperatif_app"));
        let msg = err.to_string();
        assert!(
            msg.contains("permission denied"),
            "{what}: expected permission denied, got {msg}"
        );
    }

    sqlx::raw_sql("RESET ROLE")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    db.cleanup().await;
}

/// P2A — a checkout after SET ROLE must come back clean: the
/// before_acquire reset now clears role residue too.
#[tokio::test]
async fn session_level_set_role_is_wiped_on_checkout() {
    let db = setup().await;
    {
        let mut conn = db.pool.acquire().await.unwrap();
        sqlx::raw_sql("SET ROLE kooperatif_app")
            .execute(&mut *conn)
            .await
            .unwrap();
        let who: String = sqlx::query_scalar("SELECT current_user")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(who, "kooperatif_app");
    }
    for _ in 0..8 {
        let mut conn = db.pool.acquire().await.unwrap();
        let who: String = sqlx::query_scalar("SELECT current_user")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(who, "kooperatif", "SET ROLE residue leaked: {who}");
    }
    db.cleanup().await;
}

/// P2A — startup principal verification: the admin role is privileged,
/// kooperatif_app is not.
#[tokio::test]
async fn inspect_runtime_principal_flags_privileged_roles() {
    let db = setup().await;

    let admin = app_db::inspect_runtime_principal(&db.pool).await.unwrap();
    assert_eq!(admin.name, "kooperatif");
    assert!(
        admin.privileged,
        "schema owner/superuser must be flagged privileged"
    );

    let mut conn = db.pool.acquire().await.unwrap();
    sqlx::raw_sql("SET ROLE kooperatif_app")
        .execute(&mut *conn)
        .await
        .unwrap();
    // inspect_runtime_principal takes a pool; probe the same check on
    // the raw connection instead.
    let (rolsuper, bypass, owns): (bool, bool, bool) = sqlx::query_as(
        "SELECT r.rolsuper, r.rolbypassrls,
                EXISTS (SELECT 1 FROM pg_class c
                         JOIN pg_namespace n ON n.oid = c.relnamespace
                        WHERE n.nspname = 'public' AND c.relkind='r'
                          AND c.relowner = r.oid)
           FROM pg_roles r WHERE r.rolname = current_user",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert!(!rolsuper && !bypass && !owns);
    sqlx::raw_sql("RESET ROLE")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    db.cleanup().await;
}

/// P2A — once app.user_id is stamped, the membership shim may only
/// resolve THAT user (caller binding), while an unstamped connection
/// still serves the pre-context bootstrap lane.
#[tokio::test]
async fn membership_shim_binds_to_stamped_user() {
    let db = setup().await;

    let user_a: Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, display_name, password_hash) \
         VALUES ('p2a_a', 'A', 'x') RETURNING id",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let user_b: Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, display_name, password_hash) \
         VALUES ('p2a_b', 'B', 'x') RETURNING id",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let coop: Uuid = sqlx::query_scalar(
        "INSERT INTO cooperatives (name, status, is_bootstrap, business_enabled) \
         VALUES ('p2a-bind', 'active', false, true) RETURNING id",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         VALUES ($1, $2, 'active')",
    )
    .bind(coop)
    .bind(user_b)
    .execute(&db.pool)
    .await
    .unwrap();

    // Unstamped (bootstrap lane): real membership resolves.
    let found: i64 = sqlx::query_scalar("SELECT count(*) FROM app_verify_membership($1, $2)")
        .bind(user_b)
        .bind(coop)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(found, 1);

    // Stamped as user A: asking for user B's membership must resolve
    // nothing — the caller's claim cannot exceed their own identity.
    let mut tx = app_db::tenant_tx(&db.pool, Some(coop), Some(user_a))
        .await
        .unwrap();
    let denied: i64 = sqlx::query_scalar("SELECT count(*) FROM app_verify_membership($1, $2)")
        .bind(user_b)
        .bind(coop)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        denied, 0,
        "stamped caller A must not resolve user B's membership"
    );
    tx.commit().await.unwrap();

    db.cleanup().await;
}

// ── M1-P4-P2B — authenticated request-context propagation ─────────
//
// Proves the session-derived identity binding that replaces the
// pre-context shim lane for business handlers:
//   * `request_tx` derives `app.user_id` from the session row (the
//     token hash is the identity claim, never a caller-supplied
//     UUID) and stamps `app.cooperative_id` only after
//     `app_verify_membership` succeeds;
//   * unknown / revoked / expired tokens fail closed;
//   * a valid session cannot open a foreign cooperative;
//   * `request_user_tx` stamps only the user (no cooperative);
//   * `request_tx_repeatable` configures repeatable-read before any
//     context query.

/// Inserts a user, a cooperative and (optionally) an active
/// membership, then returns `(user_id, coop_id, member_id, member_coop)`
/// fixtures plus a live session token hash for `member_id`.
async fn request_ctx_fixture(db: &TestDb) -> (Uuid, Uuid, Vec<u8>, Uuid) {
    let user = Uuid::new_v4();
    let foreign_user = Uuid::new_v4();
    let coop = Uuid::new_v4();
    let foreign_coop = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash) \
         VALUES ($1,$2,$3,$4), ($5,$6,$7,$8)",
    )
    .bind(user)
    .bind(format!("rc-{}", &user.to_string()[..8]))
    .bind("Request Ctx")
    .bind("$argon2i$v=19$m=8,t=1,p=1$c29tZXNhbHQ$QzNw3jg9qR1M0eQz2uD+cw")
    .bind(foreign_user)
    .bind(format!("rf-{}", &foreign_user.to_string()[..8]))
    .bind("Request Foreign")
    .bind("$argon2i$v=19$m=8,t=1,p=1$c29tZXNhbHQ$QzNw3jg9qR1M0eQz2uD+cw")
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO cooperatives (id, name, status, is_bootstrap, business_enabled) \
         VALUES ($1,'RC Kooperatif','active',false,true), \
                ($2,'RC Foreign','active',false,true)",
    )
    .bind(coop)
    .bind(foreign_coop)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         VALUES ($1, $2, 'active')",
    )
    .bind(coop)
    .bind(user)
    .execute(&db.pool)
    .await
    .unwrap();
    let token_hash: Vec<u8> = vec![0x7au8; 32];
    // M1-P6 B4 (0071): the session's active cooperative is the only
    // stampable scope — the fixture session has switched into `coop`.
    sqlx::query(
        "INSERT INTO user_sessions \
           (user_id, token_hash, csrf_token, created_at, last_seen_at, expires_at, idle_expires_at, \
            active_cooperative_id) \
         VALUES ($1,$2,'csrf',now(),now(),now() + interval '1 day',now() + interval '1 day',$3)",
    )
    .bind(user)
    .bind(&token_hash)
    .bind(coop)
    .execute(&db.pool)
    .await
    .unwrap();
    (user, coop, token_hash, foreign_coop)
}

#[tokio::test]
async fn request_tx_derives_user_from_session_and_stamps_cooperative() {
    let db = setup().await;
    let (user, coop, token_hash, _foreign) = request_ctx_fixture(&db).await;

    let (mut tx, _ctx) = app_db::request_tx(&db.pool, &token_hash, coop, OffsetDateTime::now_utc())
        .await
        .expect("request tx");
    let (guc_user, guc_coop): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT current_setting('app.user_id', true), \
                current_setting('app.cooperative_id', true)",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(guc_user.as_deref(), Some(user.to_string().as_str()));
    assert_eq!(guc_coop.as_deref(), Some(coop.to_string().as_str()));
    tx.commit().await.unwrap();

    db.cleanup().await;
}

#[tokio::test]
async fn request_tx_rejects_unknown_token() {
    let db = setup().await;
    let (_user, coop, _hash, _foreign) = request_ctx_fixture(&db).await;

    let unknown: Vec<u8> = vec![0x42u8; 32];
    let err = app_db::request_tx(&db.pool, &unknown, coop, OffsetDateTime::now_utc())
        .await
        .expect_err("unknown token must fail closed");
    assert!(matches!(err, app_db::RequestContextError::Unauthenticated));

    db.cleanup().await;
}

#[tokio::test]
async fn request_tx_rejects_revoked_session() {
    let db = setup().await;
    let (_user, coop, token_hash, _foreign) = request_ctx_fixture(&db).await;
    sqlx::query("UPDATE user_sessions SET revoked_at = now() WHERE token_hash = $1")
        .bind(&token_hash)
        .execute(&db.pool)
        .await
        .unwrap();

    let err = app_db::request_tx(&db.pool, &token_hash, coop, OffsetDateTime::now_utc())
        .await
        .expect_err("revoked session must fail closed");
    assert!(matches!(err, app_db::RequestContextError::Unauthenticated));

    db.cleanup().await;
}

#[tokio::test]
async fn request_tx_rejects_session_that_is_expired_at_application_time() {
    let db = setup().await;
    let (_user, coop, token_hash, _foreign) = request_ctx_fixture(&db).await;

    // Application clock reads past the session expiry — the supplied
    // timestamp is authoritative, not the database clock.
    let future = OffsetDateTime::now_utc() + time::Duration::days(3);
    let err = app_db::request_tx(&db.pool, &token_hash, coop, future)
        .await
        .expect_err("expired-at-app-time session must fail closed");
    assert!(matches!(err, app_db::RequestContextError::Unauthenticated));

    db.cleanup().await;
}

#[tokio::test]
async fn request_tx_cannot_hop_to_a_foreign_cooperative() {
    let db = setup().await;
    let (_user, _coop, token_hash, foreign_coop) = request_ctx_fixture(&db).await;

    // Valid session, but the user holds no membership in the foreign
    // cooperative — the caller cannot switch scope by choosing an id.
    let err = app_db::request_tx(
        &db.pool,
        &token_hash,
        foreign_coop,
        OffsetDateTime::now_utc(),
    )
    .await
    .expect_err("foreign cooperative must be denied");
    assert!(matches!(err, app_db::RequestContextError::NotMember));

    db.cleanup().await;
}

#[tokio::test]
async fn request_user_tx_stamps_user_only() {
    let db = setup().await;
    let (user, _coop, token_hash, _foreign) = request_ctx_fixture(&db).await;

    let (mut tx, uid) = app_db::request_user_tx(&db.pool, &token_hash, OffsetDateTime::now_utc())
        .await
        .expect("user-lane tx");
    let (guc_user, guc_coop): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT current_setting('app.user_id', true), \
                current_setting('app.cooperative_id', true)",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(guc_user.as_deref(), Some(user.to_string().as_str()));
    assert_eq!(uid, user);
    assert!(
        guc_coop.as_deref().is_none_or(|v| v.is_empty()),
        "user lane must not stamp a cooperative: {guc_coop:?}"
    );
    tx.commit().await.unwrap();

    db.cleanup().await;
}

#[tokio::test]
async fn request_tx_repeatable_configures_isolation_before_context() {
    let db = setup().await;
    let (_user, coop, token_hash, _foreign) = request_ctx_fixture(&db).await;

    let (mut tx, _ctx) =
        app_db::request_tx_repeatable(&db.pool, &token_hash, coop, OffsetDateTime::now_utc())
            .await
            .expect("repeatable request tx");
    let level: String = sqlx::query_scalar("SHOW transaction_isolation")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(level, "repeatable read");
    tx.commit().await.unwrap();

    db.cleanup().await;
}

#[tokio::test]
async fn request_tx_gucs_do_not_leak_across_checkouts() {
    let db = setup().await;
    let (user, coop, token_hash, _foreign) = request_ctx_fixture(&db).await;

    let (tx, _ctx) = app_db::request_tx(&db.pool, &token_hash, coop, OffsetDateTime::now_utc())
        .await
        .expect("request tx");
    tx.commit().await.unwrap();

    // A fresh pool checkout must not observe the committed request
    // transaction's GUCs.
    let (guc_user, guc_coop): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT current_setting('app.user_id', true), \
                current_setting('app.cooperative_id', true)",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    for (name, v) in [("app.user_id", guc_user), ("app.cooperative_id", guc_coop)] {
        assert!(
            v.as_deref().is_none_or(|x| x.is_empty()),
            "{name} leaked across checkout: {v:?}"
        );
    }
    let _ = user;

    db.cleanup().await;
}
