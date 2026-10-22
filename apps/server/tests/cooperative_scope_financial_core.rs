//! M1-P2-C3 Batch 2 — cooperative-scoped FINANCIAL CORE data access:
//! periods & assessment rules, assessments & share sources, payments &
//! allocations, financial accounts, account movements and transfers.
//!
//! Proves at the repository layer — the enforcement point — that every
//! read, aggregate, insert, update, lock and transactional command in
//! this batch is scoped to the verified cooperative context, that
//! cross-cooperative identifiers resolve to indistinguishable
//! not-found domain errors without partial writes, that the derived
//! balance model, allocation/reversal and idempotency invariants are
//! preserved, that polymorphic `account_movements.source_*` references
//! cannot cross the boundary, and that the `None` context (the proven
//! never-bootstrapped lane) sees only legacy NULL-cooperative rows.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable
//! database, same convention as the other integration suites).

use kooperatif_server::db as app_db;
use kooperatif_server::financial_accounts::model::{AccountStatus, AccountType};
use kooperatif_server::financial_accounts::repo as account_repo;
use kooperatif_server::parties::repo as party_repo;
use kooperatif_server::payments::repo as payment_repo;
use kooperatif_server::periods::model::AssessmentRuleType;
use kooperatif_server::periods::repo as period_repo;
use rust_decimal::Decimal;
use sqlx::PgPool;
use time::{Date, Month, OffsetDateTime};
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

async fn create_shareholder_at(
    pool: &PgPool,
    coop: Option<Uuid>,
    actor: Uuid,
    first: &str,
    last: &str,
    sequence: i64,
) -> party_repo::CreatedShareholder {
    party_repo::create_shareholder(
        pool,
        coop,
        actor,
        party_repo::CreateShareholder {
            person_first_name: Some(first.to_string()),
            person_last_name: Some(last.to_string()),
            existing_person_id: None,
            guardian_person_id: None,
            guardian_first_name: None,
            guardian_last_name: None,
            family: party_repo::FamilyRef::New(sequence),
            default_collection_account_id: None,
        },
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("shareholder aggregate")
}

async fn create_account_at(pool: &PgPool, coop: Option<Uuid>, actor: Uuid, name: &str) -> Uuid {
    account_repo::create_account(
        pool,
        coop,
        actor,
        account_repo::CreateAccount {
            name: name.to_string(),
            account_type: AccountType::Cash,
            description: None,
            bank_name: None,
            iban: None,
        },
    )
    .await
    .expect("account")
}

fn date(y: i32, m: u8, d: u8) -> Date {
    Date::from_calendar_date(y, Month::try_from(m).unwrap(), d).unwrap()
}

async fn create_period_at(
    pool: &PgPool,
    coop: Option<Uuid>,
    actor: Uuid,
    name: &str,
) -> (Uuid, i64) {
    period_repo::create_period(
        pool,
        coop,
        actor,
        period_repo::CreatePeriod {
            name: name.to_string(),
            collection_start_date: date(2025, 1, 1),
            due_date: date(2025, 3, 31),
            rule_type: AssessmentRuleType::PerShareholder,
            base_amount: Decimal::from(1000),
            assessment_effective_date: date(2025, 1, 15),
        },
    )
    .await
    .expect("period")
}

fn payment_command(
    destination_account_id: Uuid,
    amount: i64,
    key: &str,
    allocations: Vec<payment_repo::NewAllocation>,
) -> payment_repo::CreatePayment {
    payment_repo::CreatePayment {
        payer_person_id: None,
        payer_first_name: Some("Test".to_string()),
        payer_last_name: Some("Ödeyen".to_string()),
        amount: Decimal::from(amount),
        method: "cash".to_string(),
        received_at: OffsetDateTime::now_utc(),
        note: None,
        destination_account_id,
        idempotency_key: key.to_string(),
        fingerprint: format!("fp-{key}"),
        allocations,
    }
}

// ---------------------------------------------------------------------
// Financial accounts: reads, derived balances, writes, concurrency.
// ---------------------------------------------------------------------

#[tokio::test]
async fn accounts_are_scoped_and_carry_the_cooperative_id() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    let acc_a = create_account_at(&db.pool, Some(fx.coop_a), fx.user, "Kasa A").await;
    let acc_b = create_account_at(&db.pool, Some(fx.coop_b), fx.user, "Kasa B").await;

    // Lists & options: each cooperative sees only its own accounts.
    let list_a = account_repo::list_accounts(&db.pool, Some(fx.coop_a), None, None, None, 1, 50)
        .await
        .unwrap();
    assert_eq!(list_a.len(), 1);
    assert_eq!(list_a[0].id, acc_a);
    let opts_b = account_repo::list_active_account_options(&db.pool, Some(fx.coop_b))
        .await
        .unwrap();
    assert_eq!(opts_b.len(), 1);
    assert_eq!(opts_b[0].id, acc_b);

    // Detail: foreign id → None (indistinguishable).
    assert!(account_repo::get_account(&db.pool, Some(fx.coop_a), acc_b)
        .await
        .unwrap()
        .is_none());

    // Insert carried the cooperative.
    let coop: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM financial_accounts WHERE id = $1")
            .bind(acc_a)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(coop, Some(fx.coop_a));

    // Derived balances are scoped by currency.
    let totals_a = account_repo::total_balance_by_currency(&db.pool, Some(fx.coop_a))
        .await
        .unwrap();
    assert_eq!(totals_a.len(), 1);
    assert_eq!(totals_a[0].1, Decimal::ZERO);

    // Update + status change: foreign account → NotFound.
    let updated_b: OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM financial_accounts WHERE id = $1")
            .bind(acc_b)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let err = account_repo::update_account(
        &db.pool,
        Some(fx.coop_a),
        acc_b,
        "İzinsiz".to_string(),
        None,
        None,
        None,
        updated_b,
    )
    .await
    .expect_err("foreign account update must fail");
    assert!(matches!(err, account_repo::AccountCommandError::NotFound));

    let err = account_repo::change_account_status(
        &db.pool,
        Some(fx.coop_a),
        acc_b,
        AccountStatus::Inactive,
    )
    .await
    .expect_err("foreign status change must fail");
    assert!(matches!(err, account_repo::AccountCommandError::NotFound));

    // Coop-B untouched.
    let status_b: String =
        sqlx::query_scalar("SELECT status FROM financial_accounts WHERE id = $1")
            .bind(acc_b)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(status_b, "active");

    db.cleanup().await;
}

#[tokio::test]
async fn transfers_are_scoped_and_keep_balance_and_reversal_invariants() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    let src_a = create_account_at(&db.pool, Some(fx.coop_a), fx.user, "Kasa A1").await;
    let dst_a = create_account_at(&db.pool, Some(fx.coop_a), fx.user, "Kasa A2").await;
    let dst_b = create_account_at(&db.pool, Some(fx.coop_b), fx.user, "Kasa B").await;

    // Fund src_a with a payment inflow (also exercises the payment ->
    // movement chain). 1000 TRY in.
    payment_repo::create_payment(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        payment_command(src_a, 1000, "fund-a", Vec::new()),
    )
    .await
    .expect("funding payment");

    // Same-coop transfer: 250 A1 -> A2.
    let outcome = account_repo::post_transfer(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        account_repo::PostTransfer {
            source_account_id: src_a,
            destination_account_id: dst_a,
            amount: Decimal::from(250),
            occurred_at: OffsetDateTime::now_utc(),
            note: None,
            idempotency_key: "tr-a-1".to_string(),
            fingerprint: "fp-tr-a-1".to_string(),
        },
    )
    .await
    .expect("same-coop transfer");
    assert!(!outcome.replayed);

    // Both legs landed in coop-A; derived balances reflect them.
    let movements = account_repo::list_movements(&db.pool, Some(fx.coop_a), dst_a, 1, 50)
        .await
        .unwrap();
    assert_eq!(movements.len(), 1);
    let mv_coop: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM account_movements WHERE id = $1")
            .bind(movements[0].id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(mv_coop, Some(fx.coop_a));
    // Movement listing shows the SAME-coop transfer number label.
    assert_eq!(movements[0].source_number, Some(outcome.transfer_number));

    let src_row = account_repo::get_account(&db.pool, Some(fx.coop_a), src_a)
        .await
        .unwrap()
        .expect("src account");
    assert_eq!(src_row.balance, Decimal::from(750));
    let dst_row = account_repo::get_account(&db.pool, Some(fx.coop_a), dst_a)
        .await
        .unwrap()
        .expect("dst account");
    assert_eq!(dst_row.balance, Decimal::from(250));

    // Cross-coop destination: NotFound, nothing written.
    let err = account_repo::post_transfer(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        account_repo::PostTransfer {
            source_account_id: src_a,
            destination_account_id: dst_b,
            amount: Decimal::from(10),
            occurred_at: OffsetDateTime::now_utc(),
            note: None,
            idempotency_key: "tr-cross".to_string(),
            fingerprint: "fp-tr-cross".to_string(),
        },
    )
    .await
    .err()
    .expect("cross-coop destination must fail");
    assert!(matches!(err, account_repo::AccountCommandError::NotFound));
    let transfer_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM account_transfers WHERE cooperative_id = $1")
            .bind(fx.coop_a)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(transfer_count, 1, "no partial cross-coop transfer row");

    // Foreign transfer detail/reversal → NotFound, coop-B rows intact.
    assert!(
        account_repo::get_transfer(&db.pool, Some(fx.coop_b), outcome.transfer_id)
            .await
            .unwrap()
            .is_none()
    );
    let err = account_repo::reverse_transfer(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        outcome.transfer_id,
        "deneme".to_string(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect_err("foreign transfer reversal must fail");
    assert!(matches!(err, account_repo::AccountCommandError::NotFound));

    // Same-coop reversal: transfer + both legs reversed, balances back.
    let (replayed, _) = account_repo::reverse_transfer(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        outcome.transfer_id,
        "düzeltme".to_string(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("same-coop reversal");
    assert!(!replayed);
    let src_row = account_repo::get_account(&db.pool, Some(fx.coop_a), src_a)
        .await
        .unwrap()
        .expect("src account");
    assert_eq!(src_row.balance, Decimal::from(1000));
    let reversed_mv: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_movements WHERE account_id = $1 AND status = 'reversed'",
    )
    .bind(dst_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(reversed_mv, 1);

    db.cleanup().await;
}

#[tokio::test]
async fn concurrent_transfers_serialize_deterministically() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    let src_a = create_account_at(&db.pool, Some(fx.coop_a), fx.user, "Kasa A1").await;
    let dst_a = create_account_at(&db.pool, Some(fx.coop_a), fx.user, "Kasa A2").await;
    payment_repo::create_payment(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        payment_command(src_a, 1000, "fund-a-conc", Vec::new()),
    )
    .await
    .expect("funding payment");

    // Two concurrent transfers against the same account pair: the
    // ascending-id lock order serializes them — no deadlock, no lost
    // funds, both succeed.
    let pool1 = db.pool.clone();
    let pool2 = db.pool.clone();
    let (r1, r2) = tokio::join!(
        account_repo::post_transfer(
            &pool1,
            Some(fx.coop_a),
            fx.user,
            account_repo::PostTransfer {
                source_account_id: src_a,
                destination_account_id: dst_a,
                amount: Decimal::from(300),
                occurred_at: OffsetDateTime::now_utc(),
                note: None,
                idempotency_key: "conc-1".to_string(),
                fingerprint: "fp-conc-1".to_string(),
            },
        ),
        account_repo::post_transfer(
            &pool2,
            Some(fx.coop_a),
            fx.user,
            account_repo::PostTransfer {
                source_account_id: src_a,
                destination_account_id: dst_a,
                amount: Decimal::from(400),
                occurred_at: OffsetDateTime::now_utc(),
                note: None,
                idempotency_key: "conc-2".to_string(),
                fingerprint: "fp-conc-2".to_string(),
            },
        ),
    );
    r1.expect("first concurrent transfer");
    r2.expect("second concurrent transfer");

    let src_row = account_repo::get_account(&db.pool, Some(fx.coop_a), src_a)
        .await
        .unwrap()
        .expect("src account");
    assert_eq!(src_row.balance, Decimal::from(300), "1000 - 300 - 400");
    let dst_row = account_repo::get_account(&db.pool, Some(fx.coop_a), dst_a)
        .await
        .unwrap()
        .expect("dst account");
    assert_eq!(dst_row.balance, Decimal::from(700));

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// Periods, assessment rules, assessments and share sources.
// ---------------------------------------------------------------------

#[tokio::test]
async fn periods_and_assessments_are_scoped_end_to_end() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    let sh_a =
        create_shareholder_at(&db.pool, Some(fx.coop_a), fx.user, "Ayşe", "Yılmaz", 11).await;
    let _sh_b =
        create_shareholder_at(&db.pool, Some(fx.coop_b), fx.user, "Mehmet", "Demir", 22).await;

    let (period_a, _) = create_period_at(&db.pool, Some(fx.coop_a), fx.user, "2025-A").await;
    let (period_b, _) = create_period_at(&db.pool, Some(fx.coop_b), fx.user, "2025-B").await;

    // Period + rule carry the cooperative.
    for (table, col) in [("periods", "id"), ("assessment_rules", "period_id")] {
        let coop: Option<Uuid> = sqlx::query_scalar(&format!(
            "SELECT cooperative_id FROM {table} WHERE {col} = $1"
        ))
        .bind(period_a)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(coop, Some(fx.coop_a), "{table} must carry cooperative_id");
    }

    // List/detail scoped.
    let periods_a = period_repo::list_periods(&db.pool, Some(fx.coop_a), None, 1, 50)
        .await
        .unwrap();
    assert_eq!(periods_a.len(), 1);
    assert!(
        period_repo::find_period(&db.pool, Some(fx.coop_a), period_b)
            .await
            .unwrap()
            .is_none()
    );

    // Mutation commands on foreign periods → NotFound (scoped locks).
    for result in [
        period_repo::delete_period(&db.pool, Some(fx.coop_a), period_b).await,
        period_repo::close_period(
            &db.pool,
            Some(fx.coop_a),
            period_b,
            OffsetDateTime::now_utc(),
        )
        .await,
    ] {
        assert!(matches!(
            result.expect_err("foreign period command must fail"),
            period_repo::PeriodCommandError::NotFound
        ));
    }
    let updated_b: OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM periods WHERE id = $1")
            .bind(period_b)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let err = period_repo::update_period(
        &db.pool,
        Some(fx.coop_a),
        period_b,
        period_repo::UpdatePeriod {
            name: "İzinsiz".to_string(),
            collection_start_date: date(2025, 1, 1),
            due_date: date(2025, 3, 31),
            rule_type: AssessmentRuleType::PerShareholder,
            base_amount: Decimal::from(1),
            assessment_effective_date: date(2025, 1, 15),
            expected_updated_at: updated_b,
        },
        OffsetDateTime::now_utc(),
    )
    .await
    .expect_err("foreign period update must fail");
    assert!(matches!(err, period_repo::PeriodCommandError::NotFound));

    // Generation: only coop-A's shareholder population is eligible;
    // every produced row carries coop-A.
    let summary = period_repo::generate_assessments(&db.pool, Some(fx.coop_a), fx.user, period_a)
        .await
        .expect("generation");
    assert_eq!(summary.assessment_count, 1);
    let assessment_a: Uuid = sqlx::query_scalar("SELECT id FROM assessments WHERE period_id = $1")
        .bind(period_a)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let coop: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM assessments WHERE id = $1")
            .bind(assessment_a)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(coop, Some(fx.coop_a));

    // Assessment reads scoped; foreign ids invisible.
    let assessments_a =
        period_repo::list_assessments(&db.pool, Some(fx.coop_a), period_a, None, 1, 50)
            .await
            .unwrap();
    assert_eq!(assessments_a.len(), 1);
    assert!(
        period_repo::list_assessments(&db.pool, Some(fx.coop_a), period_b, None, 1, 50)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        period_repo::list_assessments_by_shareholder(
            &db.pool,
            Some(fx.coop_a),
            sh_a.shareholder_id
        )
        .await
        .unwrap()
        .len()
            == 1
    );

    // Preview of a foreign draft is NotFound.
    let err = period_repo::preview_assessments(&db.pool, Some(fx.coop_a), period_b)
        .await
        .err()
        .expect("foreign preview must fail");
    assert!(matches!(err, period_repo::PeriodCommandError::NotFound));

    // Coop-B generation untouched.
    let status_b: String = sqlx::query_scalar("SELECT status FROM periods WHERE id = $1")
        .bind(period_b)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(status_b, "draft");

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// Payments: creation, allocations, idempotency, reversals.
// ---------------------------------------------------------------------

#[tokio::test]
async fn payments_allocations_and_reversals_are_scoped() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    let sh_a =
        create_shareholder_at(&db.pool, Some(fx.coop_a), fx.user, "Ayşe", "Yılmaz", 33).await;
    let sh_b =
        create_shareholder_at(&db.pool, Some(fx.coop_b), fx.user, "Mehmet", "Demir", 44).await;
    let acc_a = create_account_at(&db.pool, Some(fx.coop_a), fx.user, "Kasa A").await;
    let acc_b = create_account_at(&db.pool, Some(fx.coop_b), fx.user, "Kasa B").await;
    let (period_a, _) = create_period_at(&db.pool, Some(fx.coop_a), fx.user, "2025-A").await;
    let (period_b, _) = create_period_at(&db.pool, Some(fx.coop_b), fx.user, "2025-B").await;
    period_repo::generate_assessments(&db.pool, Some(fx.coop_a), fx.user, period_a)
        .await
        .expect("generation A");
    period_repo::generate_assessments(&db.pool, Some(fx.coop_b), fx.user, period_b)
        .await
        .expect("generation B");
    let assessment_a: Uuid = sqlx::query_scalar("SELECT id FROM assessments WHERE period_id = $1")
        .bind(period_a)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let assessment_b: Uuid = sqlx::query_scalar("SELECT id FROM assessments WHERE period_id = $1")
        .bind(period_b)
        .fetch_one(&db.pool)
        .await
        .unwrap();

    // Same-coop payment: 600 allocated + 400 remainder into acc_a.
    let outcome = payment_repo::create_payment(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        payment_command(
            acc_a,
            1000,
            "pay-a-1",
            vec![payment_repo::NewAllocation {
                assessment_id: assessment_a,
                amount: Decimal::from(600),
            }],
        ),
    )
    .await
    .expect("payment");
    assert!(!outcome.replayed);
    assert_eq!(outcome.allocated_amount, Decimal::from(600));
    assert_eq!(outcome.unallocated_amount, Decimal::from(400));

    // Every produced row carries coop-A: payment, allocation, movement.
    for (table, id) in [
        ("payments", outcome.payment_id),
        (
            "payment_allocations",
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM payment_allocations WHERE payment_id = $1",
            )
            .bind(outcome.payment_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        ),
        (
            "account_movements",
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM account_movements WHERE source_type = 'payment' AND source_id = $1",
            )
            .bind(outcome.payment_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        ),
    ] {
        let coop: Option<Uuid> =
            sqlx::query_scalar(&format!("SELECT cooperative_id FROM {table} WHERE id = $1"))
                .bind(id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(coop, Some(fx.coop_a), "{table} must carry cooperative_id");
    }

    // Reads scoped: list/detail/allocations/assessment history.
    let list_a = payment_repo::list_payments(&db.pool, Some(fx.coop_a), None, 1, 50)
        .await
        .unwrap();
    assert_eq!(list_a.len(), 1);
    assert!(
        payment_repo::find_payment(&db.pool, Some(fx.coop_b), outcome.payment_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        payment_repo::payment_allocations(&db.pool, Some(fx.coop_b), outcome.payment_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        payment_repo::assessment_payments(&db.pool, Some(fx.coop_a), assessment_a)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        payment_repo::assessment_payments(&db.pool, Some(fx.coop_a), assessment_b)
            .await
            .unwrap()
            .is_empty()
    );

    // Indirect debtor search: coop-A search finds its payer/debtor
    // surface; coop-B sees nothing of coop-A's payments.
    let found = payment_repo::list_payments(&db.pool, Some(fx.coop_a), Some("Ödeyen"), 1, 50)
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    let foreign = payment_repo::list_payments(&db.pool, Some(fx.coop_b), Some("Ödeyen"), 1, 50)
        .await
        .unwrap();
    assert!(foreign.is_empty());

    // Shareholder/period summaries scoped.
    let open_a =
        payment_repo::shareholder_open_assessments(&db.pool, Some(fx.coop_a), sh_a.shareholder_id)
            .await
            .unwrap();
    assert_eq!(open_a.len(), 1);
    assert_eq!(open_a[0].remaining_amount, Decimal::from(400));
    let sum_b =
        payment_repo::shareholder_financial_summary(&db.pool, Some(fx.coop_a), sh_b.shareholder_id)
            .await
            .unwrap();
    assert_eq!(sum_b.assessment_count, 0, "foreign shareholder summary");
    let period_sum = payment_repo::period_financial_summary(&db.pool, Some(fx.coop_a), period_b)
        .await
        .unwrap();
    assert_eq!(period_sum.contributing_payment_count, 0);
    assert_eq!(period_sum.assessment_count, 0);

    // Cross-coop identifiers rejected BEFORE any side effect:
    // foreign destination account → AccountNotFoundOrInactive;
    // foreign assessment target → InactiveAssessment; foreign payer
    // person → PayerNotFound. Nothing partial may persist.
    let err = payment_repo::create_payment(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        payment_command(acc_b, 50, "pay-cross-acc", Vec::new()),
    )
    .await
    .err()
    .expect("foreign destination account must fail");
    assert!(matches!(
        err,
        payment_repo::PaymentCommandError::AccountNotFoundOrInactive
    ));

    let err = payment_repo::create_payment(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        payment_command(
            acc_a,
            50,
            "pay-cross-assessment",
            vec![payment_repo::NewAllocation {
                assessment_id: assessment_b,
                amount: Decimal::from(50),
            }],
        ),
    )
    .await
    .err()
    .expect("foreign assessment target must fail");
    assert!(matches!(
        err,
        payment_repo::PaymentCommandError::InactiveAssessment
    ));

    let mut payer_cmd = payment_command(acc_a, 50, "pay-cross-payer", Vec::new());
    payer_cmd.payer_person_id = Some(sh_b.person_id);
    payer_cmd.payer_first_name = None;
    payer_cmd.payer_last_name = None;
    let err = payment_repo::create_payment(&db.pool, Some(fx.coop_a), fx.user, payer_cmd)
        .await
        .err()
        .expect("foreign payer must fail");
    assert!(matches!(
        err,
        payment_repo::PaymentCommandError::PayerNotFound
    ));

    let payment_count_a: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payments WHERE cooperative_id = $1")
            .bind(fx.coop_a)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(payment_count_a, 1, "failed attempts wrote no payment");
    let person_count_b: i64 =
        sqlx::query_scalar("SELECT count(*) FROM persons WHERE cooperative_id = $1")
            .bind(fx.coop_b)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(person_count_b, 1, "failed attempts created no person");

    // add_allocations: foreign payment → NotFound; foreign target on
    // an in-scope payment → InactiveAssessment.
    let err = payment_repo::add_allocations(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        outcome.payment_id,
        vec![payment_repo::NewAllocation {
            assessment_id: assessment_b,
            amount: Decimal::from(10),
        }],
    )
    .await
    .err()
    .expect("foreign payment must fail");
    assert!(matches!(err, payment_repo::PaymentCommandError::NotFound));

    let err = payment_repo::add_allocations(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        outcome.payment_id,
        vec![payment_repo::NewAllocation {
            assessment_id: assessment_b,
            amount: Decimal::from(10),
        }],
    )
    .await
    .err()
    .expect("foreign allocation target must fail");
    assert!(matches!(
        err,
        payment_repo::PaymentCommandError::InactiveAssessment
    ));

    // reverse_allocation: foreign allocation id on in-scope payment →
    // NotFound.
    let err = payment_repo::reverse_allocation(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        outcome.payment_id,
        Uuid::new_v4(),
        "düzeltme".to_string(),
        OffsetDateTime::now_utc(),
    )
    .await
    .err()
    .expect("unknown allocation must fail");
    assert!(matches!(err, payment_repo::PaymentCommandError::NotFound));

    // reverse_payment: foreign scope → NotFound; same scope reverses
    // payment + allocation + movement atomically.
    let err = payment_repo::reverse_payment(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        outcome.payment_id,
        "deneme".to_string(),
        OffsetDateTime::now_utc(),
    )
    .await
    .err()
    .expect("foreign payment reversal must fail");
    assert!(matches!(err, payment_repo::PaymentCommandError::NotFound));

    let reversal = payment_repo::reverse_payment(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        outcome.payment_id,
        "yanlış kayıt".to_string(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("same-coop reversal");
    assert!(!reversal.replayed);
    assert_eq!(reversal.reversed_allocation_count, 1);

    // Movement flipped; derived account balance back to zero; the
    // assessment is owed again in full.
    let acc_row = account_repo::get_account(&db.pool, Some(fx.coop_a), acc_a)
        .await
        .unwrap()
        .expect("account");
    assert_eq!(acc_row.balance, Decimal::ZERO);
    let open_a =
        payment_repo::shareholder_open_assessments(&db.pool, Some(fx.coop_a), sh_a.shareholder_id)
            .await
            .unwrap();
    assert_eq!(open_a[0].remaining_amount, Decimal::from(1000));

    // Idempotent replay: same key + same fingerprint inside coop-A.
    let replay = payment_repo::create_payment(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        payment_command(
            acc_a,
            1000,
            "pay-a-1",
            vec![payment_repo::NewAllocation {
                assessment_id: assessment_a,
                amount: Decimal::from(600),
            }],
        ),
    )
    .await
    .expect("replay");
    assert!(replay.replayed);
    assert_eq!(replay.payment_id, outcome.payment_id);

    db.cleanup().await;
}

#[tokio::test]
async fn idempotency_keys_are_scoped_across_cooperatives() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    let acc_a = create_account_at(&db.pool, Some(fx.coop_a), fx.user, "Kasa A").await;
    let acc_b = create_account_at(&db.pool, Some(fx.coop_b), fx.user, "Kasa B").await;

    let outcome_b = payment_repo::create_payment(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        payment_command(acc_b, 100, "shared-key", Vec::new()),
    )
    .await
    .expect("coop-B payment");

    // Since 0025 the idempotency key is cooperative-scoped: coop-A
    // reusing B's key creates an independent coop-A row — never a
    // replay or disclosure of B's payment.
    let outcome_a = payment_repo::create_payment(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        payment_command(acc_a, 100, "shared-key", Vec::new()),
    )
    .await
    .expect("cross-coop same key is scoped, not a collision");
    assert!(!outcome_a.replayed);
    assert_ne!(outcome_a.payment_id, outcome_b.payment_id);

    // Coop-B's own replay still resolves normally.
    let replay = payment_repo::create_payment(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        payment_command(acc_b, 100, "shared-key", Vec::new()),
    )
    .await
    .expect("same-coop replay");
    assert!(replay.replayed);
    assert_eq!(replay.payment_id, outcome_b.payment_id);

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// Polymorphic account movement source references (explicit risk).
// ---------------------------------------------------------------------

#[tokio::test]
async fn polymorphic_movement_sources_never_cross_the_boundary() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    let acc_a = create_account_at(&db.pool, Some(fx.coop_a), fx.user, "Kasa A").await;
    let acc_b = create_account_at(&db.pool, Some(fx.coop_b), fx.user, "Kasa B").await;

    // A real coop-B payment (polymorphic source of a coop-B movement).
    let pay_b = payment_repo::create_payment(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        payment_command(acc_b, 250, "pay-b-1", Vec::new()),
    )
    .await
    .expect("coop-B payment");

    // Crafted anomaly: a coop-A movement whose polymorphic source_id
    // points at coop-B's payment (no FK exists — the documented C2
    // exception). The scoped source-label join must NOT resolve the
    // foreign payment number.
    sqlx::query(
        "INSERT INTO account_movements \
            (account_id, direction, amount, source_type, source_id, occurred_at, \
             status, cooperative_id, created_by) \
         VALUES ($1, 'inflow', 999, 'payment', $2, now(), 'active', $3, $4)",
    )
    .bind(acc_a)
    .bind(pay_b.payment_id)
    .bind(fx.coop_a)
    .bind(fx.user)
    .execute(&db.pool)
    .await
    .expect("crafted movement");

    let movements = account_repo::list_movements(&db.pool, Some(fx.coop_a), acc_a, 1, 50)
        .await
        .unwrap();
    let crafted = movements
        .iter()
        .find(|m| m.source_id == pay_b.payment_id)
        .expect("crafted movement is listed (it IS coop-A's row)");
    assert_eq!(
        crafted.source_number, None,
        "foreign source label must never resolve"
    );

    // Reversal targeting the foreign source id under coop-A scope
    // flips only coop-A movements; coop-B's payment movement stays
    // active (application-layer source ownership validation).
    let mut tx = db.pool.begin().await.unwrap();
    let flipped = account_repo::reverse_movements_of_source(
        &mut tx,
        Some(fx.coop_a),
        kooperatif_server::financial_accounts::model::MovementSource::Payment,
        pay_b.payment_id,
        OffsetDateTime::now_utc(),
        fx.user,
        "test",
    )
    .await
    .expect("scoped reversal");
    tx.commit().await.unwrap();
    assert_eq!(flipped, 1, "only the coop-A movement is flipped");

    let active_in_b: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_movements \
         WHERE source_type = 'payment' AND source_id = $1 \
           AND cooperative_id = $2 AND status = 'active'",
    )
    .bind(pay_b.payment_id)
    .bind(fx.coop_b)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(active_in_b, 1, "foreign movement untouched");

    // movement_of_payment under coop-A scope resolves only coop-A
    // movements of that source — the crafted row — never coop-B's
    // real payment movement (which lives on acc_b, amount 250).
    let resolved = account_repo::movement_of_payment(&db.pool, Some(fx.coop_a), pay_b.payment_id)
        .await
        .unwrap()
        .expect("the crafted coop-A movement");
    assert_eq!(resolved.account_id, acc_a);
    assert_eq!(resolved.amount, Decimal::from(999));
    assert_ne!(
        resolved.account_id, acc_b,
        "coop-B movement must not resolve"
    );

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// NULL legacy lane is closed (0025): `cooperative_id` is NOT NULL on
// all business tables, so `None` context cannot create rows and its
// reads see nothing.
// ---------------------------------------------------------------------

#[tokio::test]
async fn null_context_lane_is_closed_by_not_null() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    let acc_a = create_account_at(&db.pool, Some(fx.coop_a), fx.user, "Kasa A").await;

    // `None` context sees no rows at all — NULL-owned rows can no
    // longer be written.
    let accounts_null = account_repo::list_accounts(&db.pool, None, None, None, None, 1, 50)
        .await
        .unwrap();
    assert!(accounts_null.is_empty());
    assert!(account_repo::get_account(&db.pool, None, acc_a)
        .await
        .unwrap()
        .is_none());

    // Repo-layer writes with a NULL cooperative context are rejected
    // by the NOT NULL constraint.
    let err = account_repo::create_account(
        &db.pool,
        None,
        fx.user,
        account_repo::CreateAccount {
            name: "Eski Kasa".to_string(),
            account_type: AccountType::Cash,
            description: None,
            bank_name: None,
            iban: None,
        },
    )
    .await
    .expect_err("NULL-coop account must be rejected");
    assert!(matches!(
        err,
        account_repo::AccountCommandError::Database(_)
    ));
    let err = period_repo::create_period(
        &db.pool,
        None,
        fx.user,
        period_repo::CreatePeriod {
            name: "Eski Dönem".to_string(),
            collection_start_date: date(2025, 1, 1),
            due_date: date(2025, 3, 31),
            rule_type: AssessmentRuleType::PerShareholder,
            base_amount: Decimal::from(1000),
            assessment_effective_date: date(2025, 1, 15),
        },
    )
    .await
    .expect_err("NULL-coop period must be rejected");
    assert!(matches!(err, period_repo::PeriodCommandError::Database(_)));

    db.cleanup().await;
}
