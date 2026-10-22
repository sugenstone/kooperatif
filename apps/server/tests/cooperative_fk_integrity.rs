//! M1-P2-C2 — cooperative-consistent composite foreign keys.
//!
//! Proves that the 66 `*_coop_fk` constraints added by migrations 0022
//! (NOT VALID) and 0023 (VALIDATE) accept same-cooperative references
//! and reject cross-cooperative references at the database layer.
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
    app_db::run_test_migrations(&pool)
        .await
        .expect("migrations applied");
    TestDb {
        _claim: claim,
        pool,
        database_name,
        admin_url,
    }
}

/// Two cooperatives plus a fixture user (created_by is a global FK).
struct Fixture {
    coop_a: Uuid,
    coop_b: Uuid,
    user: Uuid,
}

async fn fixture(pool: &PgPool) -> Fixture {
    let user = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash) VALUES ($1,$2,$3,$4)",
    )
    .bind(user)
    .bind(format!("fx{}", &user.simple().to_string()[..8]))
    .bind("Fixture User")
    .bind("not-a-real-hash")
    .execute(pool)
    .await
    .expect("fixture user");
    let coop_a = Uuid::new_v4();
    let coop_b = Uuid::new_v4();
    for (id, name, bootstrap) in [
        (coop_a, "Test Kooperatif A", true),
        (coop_b, "Test Kooperatif B", false),
    ] {
        sqlx::query(
            "INSERT INTO cooperatives (id, name, status, is_bootstrap, business_enabled)
             VALUES ($1,$2,'active',$3,true)",
        )
        .bind(id)
        .bind(name)
        .bind(bootstrap)
        .execute(pool)
        .await
        .expect("fixture cooperative");
    }
    Fixture {
        coop_a,
        coop_b,
        user,
    }
}

async fn insert_person(pool: &PgPool, coop: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO persons (id, first_name, last_name, search_name, cooperative_id)
         VALUES ($1,'Fixture','Person','fixture person',$2)",
    )
    .bind(id)
    .bind(coop)
    .execute(pool)
    .await
    .expect("fixture person");
    id
}

async fn insert_account(pool: &PgPool, coop: Uuid, user: Uuid, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO financial_accounts (id, name, account_type, created_by, cooperative_id)
         VALUES ($1,$2,'cash',$3,$4)",
    )
    .bind(id)
    .bind(name)
    .bind(user)
    .bind(coop)
    .execute(pool)
    .await
    .expect("fixture account");
    id
}

async fn insert_period(pool: &PgPool, coop: Uuid, number: i32) -> Uuid {
    let id = Uuid::new_v4();
    // period_number comes from the cooperative counter (M1-P3-B3).
    sqlx::query(
        "INSERT INTO periods (id, name, collection_start_date, due_date, cooperative_id, period_number)
         VALUES ($1,$2,'2026-01-01','2026-01-31',$3, allocate_document_number($3,'periods.period_number'))",
    )
    .bind(id)
    .bind(format!("Dönem {number}"))
    .bind(coop)
    .execute(pool)
    .await
    .expect("fixture period");
    id
}

async fn insert_shareholder(pool: &PgPool, coop: Option<Uuid>, person: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO shareholders (id, person_id, cooperative_id) VALUES ($1,$2,$3)")
        .bind(id)
        .bind(person)
        .bind(coop)
        .execute(pool)
        .await
        .expect("fixture shareholder");
    id
}

async fn insert_share(pool: &PgPool, coop: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    // share_number comes from the cooperative counter (M1-P3-B3).
    sqlx::query(
        "INSERT INTO shares (id, cooperative_id, share_number) \
         VALUES ($1,$2, allocate_document_number($2,'shares.share_number'))",
    )
    .bind(id)
    .bind(coop)
    .execute(pool)
    .await
    .expect("fixture share");
    id
}

async fn insert_movement(pool: &PgPool, coop: Option<Uuid>, account: Uuid, user: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO account_movements (id, account_id, direction, amount, source_type, source_id, occurred_at, created_by, cooperative_id)
         VALUES ($1,$2,'inflow',1,'payment',$3,'2026-01-15 10:00:00+00',$4,$5)",
    )
    .bind(id)
    .bind(account)
    .bind(Uuid::new_v4())
    .bind(user)
    .bind(coop)
    .execute(pool)
    .await
    .expect("fixture movement");
    id
}

fn coop_fk_violation(error: &sqlx::Error, constraint: &str) -> bool {
    match error {
        sqlx::Error::Database(db) => {
            db.code().as_deref() == Some("23503") && db.message().contains(constraint)
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------
// Inventory: exactly 66 composite coop FKs, all validated by 0023.
// ---------------------------------------------------------------------

#[tokio::test]
async fn inventory_is_sixty_six_validated_composite_fks() {
    let db = setup().await;

    let (total, validated): (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE c.convalidated)
         FROM pg_constraint c
         JOIN pg_class r ON r.oid = c.conrelid
         JOIN pg_namespace n ON n.oid = r.relnamespace
         WHERE c.contype = 'f' AND n.nspname = 'public'
           AND c.conname LIKE '%\\_coop\\_fk' ESCAPE '\\'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("inventory query");

    assert_eq!(total, 66, "expected exactly 66 composite coop FKs");
    assert_eq!(validated, 66, "every composite coop FK must be validated");

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// Same-cooperative acceptance.
// ---------------------------------------------------------------------

#[tokio::test]
async fn same_cooperative_references_are_accepted() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    // persons → shareholders
    let person_a = insert_person(&db.pool, Some(fx.coop_a)).await;
    insert_shareholder(&db.pool, Some(fx.coop_a), person_a).await;

    // financial_accounts → account_movements
    let account_a = insert_account(&db.pool, fx.coop_a, fx.user, "Kasa A").await;
    insert_movement(&db.pool, Some(fx.coop_a), account_a, fx.user).await;

    // periods → assessment_rules
    let period_a = insert_period(&db.pool, fx.coop_a, 1).await;
    sqlx::query(
        "INSERT INTO assessment_rules (id, period_id, rule_type, base_amount, assessment_effective_date, cooperative_id)
         VALUES ($1,$2,'per_shareholder',100,'2026-01-05',$3)",
    )
    .bind(Uuid::new_v4())
    .bind(period_a)
    .bind(fx.coop_a)
    .execute(&db.pool)
    .await
    .expect("same-coop assessment rule");

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// Cross-cooperative rejection — the core C2 guarantee.
// ---------------------------------------------------------------------

#[tokio::test]
async fn cross_cooperative_references_are_rejected() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    // shareholder in coop A pointing at a person owned by coop B
    let person_b = insert_person(&db.pool, Some(fx.coop_b)).await;
    let err =
        sqlx::query("INSERT INTO shareholders (id, person_id, cooperative_id) VALUES ($1,$2,$3)")
            .bind(Uuid::new_v4())
            .bind(person_b)
            .bind(fx.coop_a)
            .execute(&db.pool)
            .await
            .expect_err("cross-coop shareholder must fail");
    assert!(
        coop_fk_violation(&err, "shareholders_person_id_coop_fk"),
        "expected shareholders_person_id_coop_fk violation, got: {err}"
    );

    // movement in coop A pointing at an account owned by coop B
    let account_b = insert_account(&db.pool, fx.coop_b, fx.user, "Kasa B").await;
    let err = sqlx::query(
        "INSERT INTO account_movements (id, account_id, direction, amount, source_type, source_id, occurred_at, created_by, cooperative_id)
         VALUES ($1,$2,'inflow',1,'payment',$3,'2026-01-15 10:00:00+00',$4,$5)",
    )
    .bind(Uuid::new_v4())
    .bind(account_b)
    .bind(Uuid::new_v4())
    .bind(fx.user)
    .bind(fx.coop_a)
    .execute(&db.pool)
    .await
    .expect_err("cross-coop movement must fail");
    assert!(
        coop_fk_violation(&err, "account_movements_account_id_coop_fk"),
        "expected account_movements_account_id_coop_fk violation, got: {err}"
    );

    // rule in coop A pointing at a period owned by coop B
    let period_b = insert_period(&db.pool, fx.coop_b, 2).await;
    let err = sqlx::query(
        "INSERT INTO assessment_rules (id, period_id, rule_type, base_amount, assessment_effective_date, cooperative_id)
         VALUES ($1,$2,'per_share',50,'2026-01-05',$3)",
    )
    .bind(Uuid::new_v4())
    .bind(period_b)
    .bind(fx.coop_a)
    .execute(&db.pool)
    .await
    .expect_err("cross-coop rule must fail");
    assert!(
        coop_fk_violation(&err, "assessment_rules_period_id_coop_fk"),
        "expected assessment_rules_period_id_coop_fk violation, got: {err}"
    );

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// Deep chain: payment_allocations → payments AND assessments.
// ---------------------------------------------------------------------

#[tokio::test]
async fn deep_chain_cross_cooperative_is_rejected() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    let person_a = insert_person(&db.pool, Some(fx.coop_a)).await;
    let shareholder_a = insert_shareholder(&db.pool, Some(fx.coop_a), person_a).await;
    let period_a = insert_period(&db.pool, fx.coop_a, 3).await;
    let account_a = insert_account(&db.pool, fx.coop_a, fx.user, "Kasa A").await;

    let assessment_a = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO assessments (id, period_id, shareholder_id, rule_type, base_amount, amount, currency, assessment_effective_date, generated_by, cooperative_id)
         VALUES ($1,$2,$3,'per_shareholder',100,100,'TRY','2026-01-05',$4,$5)",
    )
    .bind(assessment_a)
    .bind(period_a)
    .bind(shareholder_a)
    .bind(fx.user)
    .bind(fx.coop_a)
    .execute(&db.pool)
    .await
    .expect("fixture assessment");

    let payment_a = Uuid::new_v4();
    // payment_number comes from the cooperative counter (M1-P3-B2).
    sqlx::query(
        "INSERT INTO payments (id, payer_person_id, amount, method, received_at, idempotency_key, idempotency_fingerprint, destination_account_id, created_by, cooperative_id, payment_number)
         VALUES ($1,$2,100,'cash','2026-01-15 10:00:00+00',$3,$4,$5,$6,$7, allocate_document_number($7,'payments.payment_number'))",
    )
    .bind(payment_a)
    .bind(person_a)
    .bind(format!("key-{}", payment_a.simple()))
    .bind(format!("fp-{}", payment_a.simple()))
    .bind(account_a)
    .bind(fx.user)
    .bind(fx.coop_a)
    .execute(&db.pool)
    .await
    .expect("fixture payment");

    // same-coop allocation accepted
    sqlx::query(
        "INSERT INTO payment_allocations (id, payment_id, assessment_id, amount, created_by, cooperative_id)
         VALUES ($1,$2,$3,50,$4,$5)",
    )
    .bind(Uuid::new_v4())
    .bind(payment_a)
    .bind(assessment_a)
    .bind(fx.user)
    .bind(fx.coop_a)
    .execute(&db.pool)
    .await
    .expect("same-coop allocation");

    // allocation in coop B referencing a coop-A payment → rejected.
    // A fresh coop-A assessment (new period — (period_id, shareholder_id)
    // is globally unique) avoids tripping the (payment_id, assessment_id)
    // unique before the FK fires.
    let period_a2 = insert_period(&db.pool, fx.coop_a, 4).await;
    let assessment_a2 = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO assessments (id, period_id, shareholder_id, rule_type, base_amount, amount, currency, assessment_effective_date, generated_by, cooperative_id)
         VALUES ($1,$2,$3,'per_share',100,100,'TRY','2026-01-05',$4,$5)",
    )
    .bind(assessment_a2)
    .bind(period_a2)
    .bind(shareholder_a)
    .bind(fx.user)
    .bind(fx.coop_a)
    .execute(&db.pool)
    .await
    .expect("second fixture assessment");

    let err = sqlx::query(
        "INSERT INTO payment_allocations (id, payment_id, assessment_id, amount, created_by, cooperative_id)
         VALUES ($1,$2,$3,50,$4,$5)",
    )
    .bind(Uuid::new_v4())
    .bind(payment_a)
    .bind(assessment_a2)
    .bind(fx.user)
    .bind(fx.coop_b)
    .execute(&db.pool)
    .await
    .expect_err("cross-coop allocation must fail");
    assert!(
        err.to_string().contains("_coop_fk"),
        "expected a composite coop FK violation, got: {err}"
    );

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// NULL semantics after C5-3: cooperative_id is NOT NULL on all business
// tables, so the historical "NULL owner passes every FK trivially"
// loophole is closed at the schema level. NULL *FK columns* still pass
// by MATCH SIMPLE, and the polymorphic source_id exception remains
// unconstrained by design.
// ---------------------------------------------------------------------

#[tokio::test]
async fn null_cooperative_id_and_null_fk_columns_pass_by_design() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    // NULL cooperative_id child → rejected by NOT NULL (0025). The
    // composite-FK NULL-hole that made C3/C5 mandatory is now closed.
    let person_a = insert_person(&db.pool, Some(fx.coop_a)).await;
    let null_owner =
        sqlx::query("INSERT INTO shareholders (id, person_id, cooperative_id) VALUES ($1,$2,NULL)")
            .bind(Uuid::new_v4())
            .bind(person_a)
            .execute(&db.pool)
            .await;
    assert!(
        null_owner.is_err(),
        "NULL cooperative_id must be rejected by NOT NULL"
    );

    // NULL fk column → accepted regardless of cooperative.
    // shareholders.guardian_person_id is nullable; person_id is
    // globally unique so a second person is required.
    let person_a2 = insert_person(&db.pool, Some(fx.coop_a)).await;
    sqlx::query(
        "INSERT INTO shareholders (id, person_id, guardian_person_id, cooperative_id)
         VALUES ($1,$2,NULL,$3)",
    )
    .bind(Uuid::new_v4())
    .bind(person_a2)
    .bind(fx.coop_a)
    .execute(&db.pool)
    .await
    .expect("NULL guardian must pass");

    // Polymorphic reference (documented C2 exception): account_movements
    // (source_type, source_id) has no FK by design — a dangling
    // source_id is accepted. Coop-consistency for polymorphic
    // references stays an application-layer responsibility.
    let account_a = insert_account(&db.pool, fx.coop_a, fx.user, "Kasa A").await;
    sqlx::query(
        "INSERT INTO account_movements (id, account_id, direction, amount, source_type, source_id, occurred_at, created_by, cooperative_id)
         VALUES ($1,$2,'inflow',1,'payment',$3,'2026-01-15 10:00:00+00',$4,$5)",
    )
    .bind(Uuid::new_v4())
    .bind(account_a)
    .bind(Uuid::new_v4()) // dangling polymorphic reference — no FK
    .bind(fx.user)
    .bind(fx.coop_a)
    .execute(&db.pool)
    .await
    .expect("polymorphic source_id is intentionally unconstrained");

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// shares → share_events → share_ownerships chain (share domain).
// ---------------------------------------------------------------------

#[tokio::test]
async fn share_domain_chain_enforces_cooperative() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    let person_a = insert_person(&db.pool, Some(fx.coop_a)).await;
    let shareholder_a = insert_shareholder(&db.pool, Some(fx.coop_a), person_a).await;
    let share_a = insert_share(&db.pool, fx.coop_a).await;
    let share_b = insert_share(&db.pool, fx.coop_b).await;

    let event_a = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO share_events (id, share_id, event_type, occurred_at, to_shareholder_id, acquisition_type, actor_user_id, cooperative_id)
         VALUES ($1,$2,'initial_acquisition','2026-01-10 10:00:00+00',$3,'founder',$4,$5)",
    )
    .bind(event_a)
    .bind(share_a)
    .bind(shareholder_a)
    .bind(fx.user)
    .bind(fx.coop_a)
    .execute(&db.pool)
    .await
    .expect("fixture share event");

    // ownership in coop B referencing coop-A share/event → rejected
    let err = sqlx::query(
        "INSERT INTO share_ownerships (id, share_id, shareholder_id, started_at, acquisition_type, source_event_id, created_by, cooperative_id)
         VALUES ($1,$2,$3,'2026-01-10 10:00:00+00','founder',$4,$5,$6)",
    )
    .bind(Uuid::new_v4())
    .bind(share_b)
    .bind(shareholder_a)
    .bind(event_a)
    .bind(fx.user)
    .bind(fx.coop_b)
    .execute(&db.pool)
    .await
    .expect_err("cross-coop ownership must fail");
    assert!(
        err.to_string().contains("_coop_fk"),
        "expected a composite coop FK violation, got: {err}"
    );

    // same-coop ownership accepted
    sqlx::query(
        "INSERT INTO share_ownerships (id, share_id, shareholder_id, started_at, acquisition_type, source_event_id, created_by, cooperative_id)
         VALUES ($1,$2,$3,'2026-01-10 10:00:00+00','founder',$4,$5,$6)",
    )
    .bind(Uuid::new_v4())
    .bind(share_a)
    .bind(shareholder_a)
    .bind(event_a)
    .bind(fx.user)
    .bind(fx.coop_a)
    .execute(&db.pool)
    .await
    .expect("same-coop ownership");

    db.cleanup().await;
}
