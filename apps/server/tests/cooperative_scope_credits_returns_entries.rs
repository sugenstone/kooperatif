//! M1-P2-C3 — cooperative-scoped data access, batch 3: shareholder
//! credits and credit applications, share returns/entitlements/
//! settlements, income/expense entries and financial categories.
//!
//! Proves at the repository layer — the enforcement point — that every
//! read, lock, aggregate, insert, update and transactional command in
//! this batch is bound to the verified cooperative context, that
//! cross-cooperative identifiers resolve to not-found without
//! disclosure and with ZERO partial writes, that new rows carry the
//! correct cooperative_id, and that the credit FIFO / manual-exact /
//! automatic application / reversal / settlement invariants survive
//! scoping. Also proves the `None` legacy lane stays isolated.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same convention as the other integration suites).

use kooperatif_server::credits::repo as credit_repo;
use kooperatif_server::db as app_db;
use kooperatif_server::financial_accounts::model::AccountType;
use kooperatif_server::financial_accounts::repo as account_repo;
use kooperatif_server::income_expense::model::{CategoryStatus, CategoryType, EntryKind};
use kooperatif_server::income_expense::repo as entry_repo;
use kooperatif_server::parties::repo as party_repo;
use kooperatif_server::payments::repo as payment_repo;
use kooperatif_server::periods::model::AssessmentRuleType;
use kooperatif_server::periods::repo as period_repo;
use kooperatif_server::share_returns::model::EntitlementType;
use kooperatif_server::share_returns::repo as return_repo;
use kooperatif_server::shares::model::AcquisitionType;
use kooperatif_server::shares::repo as share_repo;
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
    amount: i64,
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
            base_amount: Decimal::from(amount),
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

async fn coop_of(pool: &PgPool, table: &str, id: Uuid) -> Option<Uuid> {
    sqlx::query_scalar(&format!("SELECT cooperative_id FROM {table} WHERE id = $1"))
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn count_where(pool: &PgPool, table: &str, clause: &str, id: Uuid) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE {clause} = $1"))
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------
// Credits: assign/apply/list/summary/remainder/reversal — all scoped.
// ---------------------------------------------------------------------

#[tokio::test]
async fn credits_lifecycle_is_scoped() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;
    let now = OffsetDateTime::now_utc();

    let sh_a =
        create_shareholder_at(&db.pool, Some(fx.coop_a), fx.user, "Ayşe", "Yılmaz", 11).await;
    let sh_b =
        create_shareholder_at(&db.pool, Some(fx.coop_b), fx.user, "Mehmet", "Demir", 22).await;
    let acc_a = create_account_at(&db.pool, Some(fx.coop_a), fx.user, "Kasa A").await;
    let (period_a, _) = create_period_at(&db.pool, Some(fx.coop_a), fx.user, "2025-A", 1000).await;
    period_repo::generate_assessments(&db.pool, Some(fx.coop_a), fx.user, period_a)
        .await
        .expect("generation A");
    let assessment_a: Uuid = sqlx::query_scalar(
        "SELECT id FROM assessments WHERE period_id = $1 AND shareholder_id = $2",
    )
    .bind(period_a)
    .bind(sh_a.shareholder_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    // 1500 in, 600 allocated -> 900 unassigned remainder.
    let payment = payment_repo::create_payment(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        payment_command(
            acc_a,
            1500,
            "pay-a-1",
            vec![payment_repo::NewAllocation {
                assessment_id: assessment_a,
                amount: Decimal::from(600),
            }],
        ),
    )
    .await
    .expect("payment");

    // Assign 500 of the remainder as a credit for sh_a — auto-sweep
    // then consumes it against the remaining 400 of the assessment.
    let assigned = credit_repo::assign_credit(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        credit_repo::AssignCredit {
            payment_id: payment.payment_id,
            shareholder_id: sh_a.shareholder_id,
            amount: Decimal::from(500),
            note: None,
            idempotency_key: "cr-a-1".to_string(),
            fingerprint: "fp-cr-a-1".to_string(),
        },
        now,
    )
    .await
    .expect("assign credit");
    assert_eq!(assigned.applied_amount, Decimal::from(400));
    assert_eq!(assigned.application_count, 1);
    assert_eq!(
        coop_of(&db.pool, "shareholder_credits", assigned.credit_id).await,
        Some(fx.coop_a)
    );
    let application_a: Uuid =
        sqlx::query_scalar("SELECT id FROM credit_applications WHERE credit_id = $1")
            .bind(assigned.credit_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        coop_of(&db.pool, "credit_applications", application_a).await,
        Some(fx.coop_a),
        "application inherits its credit's cooperative"
    );

    // Read isolation.
    assert_eq!(
        credit_repo::list_shareholder_credits(&db.pool, Some(fx.coop_a), sh_a.shareholder_id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        credit_repo::list_shareholder_credits(&db.pool, Some(fx.coop_b), sh_a.shareholder_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        credit_repo::list_payment_credits(&db.pool, Some(fx.coop_b), payment.payment_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        credit_repo::list_shareholder_applications(&db.pool, Some(fx.coop_b), sh_a.shareholder_id)
            .await
            .unwrap()
            .len(),
        0
    );
    assert!(
        credit_repo::list_assessment_applications(&db.pool, Some(fx.coop_b), assessment_a)
            .await
            .unwrap()
            .is_empty()
    );
    let summary_b =
        credit_repo::shareholder_credit_summary(&db.pool, Some(fx.coop_b), sh_a.shareholder_id)
            .await
            .unwrap();
    assert_eq!(summary_b.credit_count, 0);
    assert_eq!(summary_b.available, Decimal::ZERO);
    // The remainder query is scope-bound to the payment's cooperative:
    // under coop B the payment does not exist.
    assert!(credit_repo::payment_unassigned_remainder(
        &db.pool,
        Some(fx.coop_b),
        payment.payment_id
    )
    .await
    .unwrap()
    .is_none());
    assert_eq!(
        credit_repo::payment_unassigned_remainder(&db.pool, Some(fx.coop_a), payment.payment_id)
            .await
            .unwrap()
            .unwrap(),
        Decimal::from(400)
    );

    // Cross-coop command attempts — indistinguishable + zero writes.
    let apps_before = count_where(
        &db.pool,
        "credit_applications",
        "credit_id",
        assigned.credit_id,
    )
    .await;
    let err = credit_repo::assign_credit(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        credit_repo::AssignCredit {
            payment_id: payment.payment_id,
            shareholder_id: sh_b.shareholder_id,
            amount: Decimal::from(100),
            note: None,
            idempotency_key: "cr-b-steal".to_string(),
            fingerprint: "fp-cr-b-steal".to_string(),
        },
        now,
    )
    .await
    .err()
    .expect("cross-coop assign must fail");
    assert!(matches!(err, credit_repo::CreditCommandError::NotFound));
    assert_eq!(
        count_where(
            &db.pool,
            "shareholder_credits",
            "source_payment_id",
            payment.payment_id
        )
        .await,
        1,
        "no credit may be created cross-coop"
    );

    // A foreign beneficiary id fails indistinguishably.
    let err = credit_repo::assign_credit(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        credit_repo::AssignCredit {
            payment_id: payment.payment_id,
            shareholder_id: sh_b.shareholder_id,
            amount: Decimal::from(100),
            note: None,
            idempotency_key: "cr-a-foreign-sh".to_string(),
            fingerprint: "fp-cr-a-foreign-sh".to_string(),
        },
        now,
    )
    .await
    .err()
    .expect("foreign beneficiary must fail");
    assert!(matches!(
        err,
        credit_repo::CreditCommandError::BeneficiaryInvalid
    ));

    // Cross-coop reversals are refused without touching the rows.
    let err = credit_repo::reverse_application(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        application_a,
        "no",
        now,
    )
    .await
    .expect_err("cross-coop application reversal must fail");
    assert!(matches!(err, credit_repo::CreditCommandError::NotFound));
    let err = credit_repo::reverse_credit(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        assigned.credit_id,
        "no",
        now,
    )
    .await
    .expect_err("cross-coop credit reversal must fail");
    assert!(matches!(err, credit_repo::CreditCommandError::NotFound));
    let status: String = sqlx::query_scalar("SELECT status FROM credit_applications WHERE id = $1")
        .bind(application_a)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(status, "active");

    // Same-coop: the credit funds an active application -> reversal
    // rejected as Consumed; reversing the application first, then the
    // credit, succeeds.
    let err = credit_repo::reverse_credit(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        assigned.credit_id,
        "r",
        now,
    )
    .await
    .expect_err("consumed credit reversal must fail");
    assert!(matches!(err, credit_repo::CreditCommandError::Consumed));
    credit_repo::reverse_application(&db.pool, Some(fx.coop_a), fx.user, application_a, "r", now)
        .await
        .expect("same-coop application reversal");
    credit_repo::reverse_credit(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        assigned.credit_id,
        "r",
        now,
    )
    .await
    .expect("same-coop credit reversal");
    assert_eq!(
        count_where(
            &db.pool,
            "credit_applications",
            "credit_id",
            assigned.credit_id
        )
        .await,
        apps_before,
        "reversal preserves history — nothing deleted"
    );

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// FIFO order + exact manual apply + automatic application invariants.
// ---------------------------------------------------------------------

#[tokio::test]
async fn credit_fifo_and_exact_manual_apply_hold_under_scope() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;
    let now = OffsetDateTime::now_utc();

    let sh = create_shareholder_at(&db.pool, Some(fx.coop_a), fx.user, "Ali", "Kaya", 55).await;
    let acc = create_account_at(&db.pool, Some(fx.coop_a), fx.user, "Kasa").await;
    let payment = payment_repo::create_payment(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        payment_command(acc, 2000, "pay-fifo", vec![]),
    )
    .await
    .expect("payment");

    // Two credits against a shareholder with NO open assessments —
    // no sweep consumption yet.
    let c1 = credit_repo::assign_credit(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        credit_repo::AssignCredit {
            payment_id: payment.payment_id,
            shareholder_id: sh.shareholder_id,
            amount: Decimal::from(800),
            note: None,
            idempotency_key: "fifo-c1".to_string(),
            fingerprint: "fp-fifo-c1".to_string(),
        },
        now,
    )
    .await
    .expect("credit 1");
    let c2 = credit_repo::assign_credit(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        credit_repo::AssignCredit {
            payment_id: payment.payment_id,
            shareholder_id: sh.shareholder_id,
            amount: Decimal::from(700),
            note: None,
            idempotency_key: "fifo-c2".to_string(),
            fingerprint: "fp-fifo-c2".to_string(),
        },
        now,
    )
    .await
    .expect("credit 2");
    assert_eq!(c1.applied_amount, Decimal::ZERO);
    assert_eq!(c2.applied_amount, Decimal::ZERO);

    // Automatic application at generation: assessment 1000 -> c1 fully
    // consumed first (FIFO), then 200 from c2.
    let (period, _) = create_period_at(&db.pool, Some(fx.coop_a), fx.user, "auto", 1000).await;
    period_repo::generate_assessments(&db.pool, Some(fx.coop_a), fx.user, period)
        .await
        .expect("generation");
    let applied: Vec<(Uuid, Decimal)> = sqlx::query_as(
        "SELECT ap.credit_id, ap.amount FROM credit_applications ap \
         JOIN shareholder_credits c ON c.id = ap.credit_id \
         WHERE ap.status = 'active' ORDER BY c.credit_number",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        applied,
        vec![
            (c1.credit_id, Decimal::from(800)),
            (c2.credit_id, Decimal::from(200))
        ],
        "automatic application must consume FIFO by credit_number"
    );

    // Manual exact apply on a raw-inserted open assessment in a SECOND
    // period (generation never ran there, so no auto-apply): 400 of
    // the remaining 500 -> entirely from c2 (c1 exhausted), exactly
    // 400.
    let (period2, _) = create_period_at(&db.pool, Some(fx.coop_a), fx.user, "manual", 1000).await;
    let manual_assessment = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO assessments (id, period_id, shareholder_id, rule_type, base_amount, \
         amount, currency, status, assessment_effective_date, generated_by, cooperative_id) \
         VALUES ($1,$2,$3,'per_shareholder',1000,1000,'TRY','active',$4,$5,$6)",
    )
    .bind(manual_assessment)
    .bind(period2)
    .bind(sh.shareholder_id)
    .bind(date(2025, 2, 1))
    .bind(fx.user)
    .bind(fx.coop_a)
    .execute(&db.pool)
    .await
    .expect("manual assessment");

    // Cross-coop apply is indistinguishable not-found, zero writes.
    let err = credit_repo::apply_credit(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        credit_repo::ApplyCredit {
            assessment_id: manual_assessment,
            amount: Decimal::from(100),
            idempotency_key: "m-cross".to_string(),
            fingerprint: "fp-m-cross".to_string(),
        },
    )
    .await
    .err()
    .expect("cross-coop apply must fail");
    assert!(matches!(err, credit_repo::CreditCommandError::NotFound));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM credit_applications WHERE assessment_id = $1"
        )
        .bind(manual_assessment)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0,
        "failed cross-coop apply leaves zero partial writes"
    );

    let outcome = credit_repo::apply_credit(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        credit_repo::ApplyCredit {
            assessment_id: manual_assessment,
            amount: Decimal::from(400),
            idempotency_key: "m-1".to_string(),
            fingerprint: "fp-m-1".to_string(),
        },
    )
    .await
    .expect("manual apply");
    assert_eq!(outcome.applied_amount, Decimal::from(400));
    assert_eq!(outcome.assessment_remaining, Decimal::from(600));
    // Exactly 400, all from c2 (the only credit with availability).
    let on_manual: Vec<(Uuid, Decimal)> = sqlx::query_as(
        "SELECT credit_id, amount FROM credit_applications \
         WHERE assessment_id = $1 AND status = 'active'",
    )
    .bind(manual_assessment)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(on_manual, vec![(c2.credit_id, Decimal::from(400))]);

    // Over-applying the remaining availability (100 left on c2) fails.
    let err = credit_repo::apply_credit(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        credit_repo::ApplyCredit {
            assessment_id: manual_assessment,
            amount: Decimal::from(200),
            idempotency_key: "m-2".to_string(),
            fingerprint: "fp-m-2".to_string(),
        },
    )
    .await
    .err()
    .expect("insufficient credit must fail");
    assert!(matches!(
        err,
        credit_repo::CreditCommandError::InsufficientCredit
    ));

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// Share returns: lifecycle + settlement, scoped at every stage.
// ---------------------------------------------------------------------

#[tokio::test]
async fn share_return_lifecycle_and_settlement_are_scoped() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;
    let now = OffsetDateTime::now_utc();

    let sh_a =
        create_shareholder_at(&db.pool, Some(fx.coop_a), fx.user, "Ayşe", "Yılmaz", 11).await;
    let acc_a = create_account_at(&db.pool, Some(fx.coop_a), fx.user, "Kasa A").await;
    let acc_b = create_account_at(&db.pool, Some(fx.coop_b), fx.user, "Kasa B").await;
    // The share's ownership interval must already cover today's
    // Istanbul midnight — the return cutoff — so it starts two days
    // back (a same-day share can never be returned the same day).
    let share_started = now - time::Duration::days(2);
    let (share_a, _) = share_repo::create_share(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        share_repo::CreateShare {
            shareholder_id: sh_a.shareholder_id,
            acquisition_type: AcquisitionType::Founder,
            fee: None,
            effective_at: share_started,
            reason: None,
        },
        now,
    )
    .await
    .expect("share A");

    // Fund coop A's account so the settlement outflow can be posted.
    payment_repo::create_payment(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        payment_command(acc_a, 5000, "fund-a", vec![]),
    )
    .await
    .expect("funding");

    // Cross-coop initiation on coop A's share: indistinguishable + the
    // share stays active with zero case rows.
    let err = return_repo::initiate_return(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        return_repo::InitiateReturn {
            share_id: share_a,
            effective_return_date: now.date(),
            reason: Some("x".to_string()),
            idempotency_key: "sr-cross".to_string(),
            fingerprint: "fp-sr-cross".to_string(),
        },
        now,
    )
    .await
    .err()
    .expect("cross-coop initiate must fail");
    assert!(matches!(
        err,
        return_repo::ShareReturnCommandError::NotFound
    ));
    assert_eq!(
        count_where(&db.pool, "share_returns", "share_id", share_a).await,
        0
    );
    let share_status: String = sqlx::query_scalar("SELECT status FROM shares WHERE id = $1")
        .bind(share_a)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(share_status, "active");

    // Same-coop initiate -> finalize (determined principal right).
    let initiated = return_repo::initiate_return(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        return_repo::InitiateReturn {
            share_id: share_a,
            effective_return_date: now.date(),
            reason: Some("exit".to_string()),
            idempotency_key: "sr-a-1".to_string(),
            fingerprint: "fp-sr-a-1".to_string(),
        },
        now,
    )
    .await
    .expect("initiate");
    assert_eq!(
        coop_of(&db.pool, "share_returns", initiated.return_id).await,
        Some(fx.coop_a)
    );
    assert!(
        return_repo::get_return(&db.pool, Some(fx.coop_b), initiated.return_id)
            .await
            .unwrap()
            .is_none(),
        "foreign cooperative must not resolve the return"
    );

    let detail = return_repo::get_return(&db.pool, Some(fx.coop_a), initiated.return_id)
        .await
        .unwrap()
        .unwrap();
    // Cross-coop finalize fails indistinguishably; nothing written.
    let err = return_repo::finalize_return(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        initiated.return_id,
        detail.updated_at,
        &[return_repo::EntitlementSpec {
            entitlement_type: EntitlementType::Principal,
            amount: Some(Decimal::from(1200)),
            due_date: None,
            policy_reference: None,
            description: None,
        }],
        now,
    )
    .await
    .err()
    .expect("cross-coop finalize must fail");
    assert!(matches!(
        err,
        return_repo::ShareReturnCommandError::NotFound
    ));
    assert_eq!(
        count_where(
            &db.pool,
            "share_return_entitlements",
            "share_return_id",
            initiated.return_id
        )
        .await,
        0
    );

    let finalized = return_repo::finalize_return(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        initiated.return_id,
        detail.updated_at,
        &[return_repo::EntitlementSpec {
            entitlement_type: EntitlementType::Principal,
            amount: Some(Decimal::from(1200)),
            due_date: Some(date(2025, 12, 31)),
            policy_reference: None,
            description: None,
        }],
        now,
    )
    .await
    .expect("finalize");
    let entitlement_a = finalized.entitlement_ids[0];
    assert_eq!(
        coop_of(&db.pool, "share_return_entitlements", entitlement_a).await,
        Some(fx.coop_a)
    );
    assert!(
        return_repo::get_entitlement(&db.pool, Some(fx.coop_b), entitlement_a)
            .await
            .unwrap()
            .is_none()
    );
    assert!(return_repo::list_entitlements(
        &db.pool,
        Some(fx.coop_b),
        None,
        None,
        None,
        None,
        1,
        50
    )
    .await
    .unwrap()
    .is_empty());

    // Settlement with a FOREIGN account fails; zero movements.
    let err = return_repo::post_settlement(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        return_repo::PostSettlement {
            entitlement_id: entitlement_a,
            financial_account_id: acc_b,
            amount: Decimal::from(1200),
            settled_at: now,
            idempotency_key: "set-acc-b".to_string(),
            fingerprint: "fp-set-acc-b".to_string(),
        },
    )
    .await
    .err()
    .expect("foreign account settlement must fail");
    assert!(matches!(
        err,
        return_repo::ShareReturnCommandError::NotFound
    ));
    // Settlement under the foreign context fails the same way.
    let err = return_repo::post_settlement(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        return_repo::PostSettlement {
            entitlement_id: entitlement_a,
            financial_account_id: acc_b,
            amount: Decimal::from(1200),
            settled_at: now,
            idempotency_key: "set-ctx-b".to_string(),
            fingerprint: "fp-set-ctx-b".to_string(),
        },
    )
    .await
    .err()
    .expect("cross-coop settlement must fail");
    assert!(matches!(
        err,
        return_repo::ShareReturnCommandError::NotFound
    ));
    assert_eq!(
        count_where(
            &db.pool,
            "share_return_settlements",
            "entitlement_id",
            entitlement_a
        )
        .await,
        0,
        "failed settlements leave zero partial writes"
    );
    assert_eq!(
        count_where(&db.pool, "account_movements", "account_id", acc_b).await,
        0
    );

    // Same-coop settlement: exactly one outflow movement + one
    // settlement row, both in coop A, bound 1:1.
    let settled = return_repo::post_settlement(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        return_repo::PostSettlement {
            entitlement_id: entitlement_a,
            financial_account_id: acc_a,
            amount: Decimal::from(1200),
            settled_at: now,
            idempotency_key: "set-a-1".to_string(),
            fingerprint: "fp-set-a-1".to_string(),
        },
    )
    .await
    .expect("settlement");
    assert_eq!(
        coop_of(&db.pool, "share_return_settlements", settled.settlement_id).await,
        Some(fx.coop_a)
    );
    let (movement_id, movement_coop): (Uuid, Option<Uuid>) = sqlx::query_as(
        "SELECT id, cooperative_id FROM account_movements \
         WHERE source_type = 'share_return_settlement' AND source_id = $1",
    )
    .bind(settled.settlement_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(movement_coop, Some(fx.coop_a));
    let bound: Option<Uuid> = sqlx::query_scalar(
        "SELECT account_movement_id FROM share_return_settlements WHERE id = $1",
    )
    .bind(settled.settlement_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        bound,
        Some(movement_id),
        "settlement binds 1:1 to its movement"
    );
    let entitlement_status: String =
        sqlx::query_scalar("SELECT status FROM share_return_entitlements WHERE id = $1")
            .bind(entitlement_a)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(entitlement_status, "settled");

    // Cross-coop settlement reversal: indistinguishable, rows intact.
    let err = return_repo::reverse_settlement(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        settled.settlement_id,
        "no".to_string(),
        now,
    )
    .await
    .expect_err("cross-coop settlement reversal must fail");
    assert!(matches!(
        err,
        return_repo::ShareReturnCommandError::NotFound
    ));
    let movement_status: String =
        sqlx::query_scalar("SELECT status FROM account_movements WHERE id = $1")
            .bind(movement_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(movement_status, "active");

    // Same-coop reversal: status-based on both rows; entitlement
    // recomputes back to open.
    return_repo::reverse_settlement(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        settled.settlement_id,
        "correction".to_string(),
        now,
    )
    .await
    .expect("same-coop reversal");
    let settlement_status: String =
        sqlx::query_scalar("SELECT status FROM share_return_settlements WHERE id = $1")
            .bind(settled.settlement_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(settlement_status, "reversed");
    let movement_status: String =
        sqlx::query_scalar("SELECT status FROM account_movements WHERE id = $1")
            .bind(movement_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(movement_status, "reversed");
    let entitlement_status: String =
        sqlx::query_scalar("SELECT status FROM share_return_entitlements WHERE id = $1")
            .bind(entitlement_a)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(entitlement_status, "open");

    // Settlement listing is scope-bound too.
    assert!(return_repo::list_settlements_of_return(
        &db.pool,
        Some(fx.coop_b),
        initiated.return_id
    )
    .await
    .unwrap()
    .is_empty());
    assert!(
        return_repo::get_settlement(&db.pool, Some(fx.coop_b), settled.settlement_id)
            .await
            .unwrap()
            .is_none()
    );

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// Income/expense entries + financial categories, scoped end to end.
// ---------------------------------------------------------------------

#[tokio::test]
async fn income_expense_categories_and_entries_are_scoped() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;
    let now = OffsetDateTime::now_utc();

    let acc_a = create_account_at(&db.pool, Some(fx.coop_a), fx.user, "Kasa A").await;
    let acc_b = create_account_at(&db.pool, Some(fx.coop_b), fx.user, "Kasa B").await;

    // Categories are cooperative-owned: created in A, invisible to B.
    let income_cat_a = entry_repo::create_category(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        entry_repo::CreateCategory {
            category_type: CategoryType::Income,
            name: "Kira Geliri A".to_string(),
            description: None,
        },
    )
    .await
    .expect("income category A");
    let expense_cat_a = entry_repo::create_category(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        entry_repo::CreateCategory {
            category_type: CategoryType::Expense,
            name: "Elektrik A".to_string(),
            description: None,
        },
    )
    .await
    .expect("expense category A");
    let expense_cat_b = entry_repo::create_category(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        entry_repo::CreateCategory {
            category_type: CategoryType::Expense,
            name: "Kira B".to_string(),
            description: None,
        },
    )
    .await
    .expect("expense category B");
    assert_eq!(
        coop_of(&db.pool, "financial_categories", income_cat_a).await,
        Some(fx.coop_a)
    );
    assert!(
        entry_repo::get_category(&db.pool, Some(fx.coop_b), income_cat_a)
            .await
            .unwrap()
            .is_none()
    );
    assert!(entry_repo::list_categories(
        &db.pool,
        Some(fx.coop_b),
        None,
        None,
        Some("Kira Geliri"),
        1,
        50
    )
    .await
    .unwrap()
    .is_empty());
    assert!(
        entry_repo::list_category_options(&db.pool, Some(fx.coop_b), CategoryType::Expense)
            .await
            .unwrap()
            .iter()
            .all(|o| o.id != expense_cat_a)
    );

    // Cross-coop category mutations are indistinguishable.
    let row_a = entry_repo::get_category(&db.pool, Some(fx.coop_a), income_cat_a)
        .await
        .unwrap()
        .unwrap();
    let err = entry_repo::update_category(
        &db.pool,
        Some(fx.coop_b),
        income_cat_a,
        "hijack".to_string(),
        None,
        row_a.updated_at,
    )
    .await
    .expect_err("cross-coop update must fail");
    assert!(matches!(
        err,
        entry_repo::IncomeExpenseCommandError::NotFound
    ));
    let err = entry_repo::change_category_status(
        &db.pool,
        Some(fx.coop_b),
        income_cat_a,
        CategoryStatus::Inactive,
    )
    .await
    .expect_err("cross-coop status change must fail");
    assert!(matches!(
        err,
        entry_repo::IncomeExpenseCommandError::NotFound
    ));

    // Post an income into coop A — entry + movement carry coop A.
    let income = entry_repo::post_entry(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        EntryKind::Income,
        entry_repo::PostEntry {
            financial_account_id: acc_a,
            category_id: income_cat_a,
            amount: Decimal::from(3000),
            occurred_at: now,
            description: "ocak kirası".to_string(),
            counterparty: None,
            reference_no: None,
            idempotency_key: "inc-a-1".to_string(),
            fingerprint: "fp-inc-a-1".to_string(),
        },
    )
    .await
    .expect("income entry");
    assert_eq!(
        coop_of(&db.pool, "income_entries", income.entry_id).await,
        Some(fx.coop_a)
    );
    let movement_coop: Option<Uuid> = sqlx::query_scalar(
        "SELECT cooperative_id FROM account_movements \
         WHERE source_type = 'income' AND source_id = $1",
    )
    .bind(income.entry_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(movement_coop, Some(fx.coop_a));

    // Read isolation on entries.
    let filter = entry_repo::EntryFilter {
        date_from: None,
        date_to: None,
        financial_account_id: None,
        category_id: None,
        status: None,
        search: None,
    };
    assert_eq!(
        entry_repo::list_entries(&db.pool, Some(fx.coop_a), EntryKind::Income, &filter, 1, 50)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        entry_repo::list_entries(&db.pool, Some(fx.coop_b), EntryKind::Income, &filter, 1, 50)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(entry_repo::get_entry(
        &db.pool,
        Some(fx.coop_b),
        EntryKind::Income,
        income.entry_id
    )
    .await
    .unwrap()
    .is_none());
    let summary_b = entry_repo::operational_summary(&db.pool, Some(fx.coop_b), &filter)
        .await
        .unwrap();
    assert_eq!(summary_b.income_total, Decimal::ZERO);

    // Cross-coop posting attempts — all indistinguishable, zero writes.
    let err = entry_repo::post_entry(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        EntryKind::Income,
        entry_repo::PostEntry {
            financial_account_id: acc_a,
            category_id: income_cat_a,
            amount: Decimal::from(10),
            occurred_at: now,
            description: "x".to_string(),
            counterparty: None,
            reference_no: None,
            idempotency_key: "inc-cross-acc".to_string(),
            fingerprint: "fp-inc-cross-acc".to_string(),
        },
    )
    .await
    .err()
    .expect("foreign account posting must fail");
    assert!(matches!(
        err,
        entry_repo::IncomeExpenseCommandError::NotFound
    ));
    let err = entry_repo::post_entry(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        EntryKind::Expense,
        entry_repo::PostEntry {
            financial_account_id: acc_b,
            category_id: expense_cat_a,
            amount: Decimal::from(10),
            occurred_at: now,
            description: "x".to_string(),
            counterparty: None,
            reference_no: None,
            idempotency_key: "exp-cross-cat".to_string(),
            fingerprint: "fp-exp-cross-cat".to_string(),
        },
    )
    .await
    .err()
    .expect("foreign category posting must fail");
    assert!(matches!(
        err,
        entry_repo::IncomeExpenseCommandError::CategoryInvalid
    ));
    // Wrong-kind category is still rejected (kind/type invariant).
    let err = entry_repo::post_entry(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        EntryKind::Income,
        entry_repo::PostEntry {
            financial_account_id: acc_b,
            category_id: expense_cat_b,
            amount: Decimal::from(10),
            occurred_at: now,
            description: "x".to_string(),
            counterparty: None,
            reference_no: None,
            idempotency_key: "inc-wrong-kind".to_string(),
            fingerprint: "fp-inc-wrong-kind".to_string(),
        },
    )
    .await
    .err()
    .expect("wrong-kind category must fail");
    assert!(matches!(
        err,
        entry_repo::IncomeExpenseCommandError::CategoryInvalid
    ));
    assert_eq!(
        count_where(&db.pool, "expense_entries", "category_id", expense_cat_a).await,
        0,
        "failed cross-coop postings write nothing"
    );

    // Cross-coop reversal of the income entry is indistinguishable.
    let err = entry_repo::reverse_entry(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        EntryKind::Income,
        income.entry_id,
        "no".to_string(),
        now,
    )
    .await
    .expect_err("cross-coop reversal must fail");
    assert!(matches!(
        err,
        entry_repo::IncomeExpenseCommandError::NotFound
    ));
    let entry_status: String =
        sqlx::query_scalar("SELECT status FROM income_entries WHERE id = $1")
            .bind(income.entry_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(entry_status, "posted");

    // Same-coop expense funded by the account works and reverses.
    let expense_cat_a2 = expense_cat_a;
    let expense = entry_repo::post_entry(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        EntryKind::Expense,
        entry_repo::PostEntry {
            financial_account_id: acc_a,
            category_id: expense_cat_a2,
            amount: Decimal::from(500),
            occurred_at: now,
            description: "fatura".to_string(),
            counterparty: None,
            reference_no: None,
            idempotency_key: "exp-a-1".to_string(),
            fingerprint: "fp-exp-a-1".to_string(),
        },
    )
    .await
    .expect("expense entry");
    entry_repo::reverse_entry(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        EntryKind::Expense,
        expense.entry_id,
        "yanlış kayıt".to_string(),
        now,
    )
    .await
    .expect("same-coop reversal");

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// Idempotency: global keys, scoped visibility — collision stays
// indistinguishable across cooperatives.
// ---------------------------------------------------------------------

#[tokio::test]
async fn idempotency_collisions_stay_indistinguishable() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;
    let now = OffsetDateTime::now_utc();

    let acc_a = create_account_at(&db.pool, Some(fx.coop_a), fx.user, "Kasa A").await;
    let acc_b = create_account_at(&db.pool, Some(fx.coop_b), fx.user, "Kasa B").await;
    let income_cat_a = entry_repo::create_category(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        entry_repo::CreateCategory {
            category_type: CategoryType::Income,
            name: "Gelir A".to_string(),
            description: None,
        },
    )
    .await
    .expect("category A");
    let income_cat_b = entry_repo::create_category(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        entry_repo::CreateCategory {
            category_type: CategoryType::Income,
            name: "Gelir B".to_string(),
            description: None,
        },
    )
    .await
    .expect("category B");

    entry_repo::post_entry(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        EntryKind::Income,
        entry_repo::PostEntry {
            financial_account_id: acc_a,
            category_id: income_cat_a,
            amount: Decimal::from(100),
            occurred_at: now,
            description: "a".to_string(),
            counterparty: None,
            reference_no: None,
            idempotency_key: "shared-key".to_string(),
            fingerprint: "fp-a".to_string(),
        },
    )
    .await
    .expect("coop A entry");

    // Coop B reusing A's key: since 0025 removed the global UNIQUE,
    // the scoped index owns idempotency — B's insert is a separate,
    // coop-B-owned row. B never sees or replays A's record.
    entry_repo::post_entry(
        &db.pool,
        Some(fx.coop_b),
        fx.user,
        EntryKind::Income,
        entry_repo::PostEntry {
            financial_account_id: acc_b,
            category_id: income_cat_b,
            amount: Decimal::from(100),
            occurred_at: now,
            description: "b".to_string(),
            counterparty: None,
            reference_no: None,
            idempotency_key: "shared-key".to_string(),
            fingerprint: "fp-a".to_string(),
        },
    )
    .await
    .expect("cross-coop same key is scoped, not a collision");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM income_entries")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        2,
        "A and B each own an independent row under the shared key"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM income_entries WHERE cooperative_id = $1"
        )
        .bind(fx.coop_b)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1,
        "B's row belongs to coop B"
    );

    // Same-coop replay with identical payload replays; different
    // payload conflicts.
    let replay = entry_repo::post_entry(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        EntryKind::Income,
        entry_repo::PostEntry {
            financial_account_id: acc_a,
            category_id: income_cat_a,
            amount: Decimal::from(100),
            occurred_at: now,
            description: "a".to_string(),
            counterparty: None,
            reference_no: None,
            idempotency_key: "shared-key".to_string(),
            fingerprint: "fp-a".to_string(),
        },
    )
    .await
    .expect("same-coop replay");
    assert!(replay.replayed);
    let err = entry_repo::post_entry(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        EntryKind::Income,
        entry_repo::PostEntry {
            financial_account_id: acc_a,
            category_id: income_cat_a,
            amount: Decimal::from(999),
            occurred_at: now,
            description: "different".to_string(),
            counterparty: None,
            reference_no: None,
            idempotency_key: "shared-key".to_string(),
            fingerprint: "fp-DIFFERENT".to_string(),
        },
    )
    .await
    .err()
    .expect("same-coop payload conflict must fail");
    assert!(matches!(
        err,
        entry_repo::IncomeExpenseCommandError::IdempotencyConflict
    ));

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// NULL legacy lane is closed (0025): cooperative_id is NOT NULL on
// all business tables, so NULL-ownership writes are rejected at the
// schema level regardless of which repository writes them.
// ---------------------------------------------------------------------

#[tokio::test]
async fn null_lane_is_closed_by_not_null() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    // Repo-layer writes with a NULL cooperative context are refused by
    // the NOT NULL constraint — the historical legacy lane cannot
    // produce new rows.
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

    let err = entry_repo::create_category(
        &db.pool,
        None,
        fx.user,
        entry_repo::CreateCategory {
            category_type: CategoryType::Income,
            name: "Eski Gelir".to_string(),
            description: None,
        },
    )
    .await
    .expect_err("NULL-coop category must be rejected");
    assert!(matches!(
        err,
        entry_repo::IncomeExpenseCommandError::Database(_)
    ));

    // Scoped writes still stamp the verified cooperative.
    let acc_a = create_account_at(&db.pool, Some(fx.coop_a), fx.user, "Kasa A").await;
    let payment = payment_repo::create_payment(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        payment_command(acc_a, 500, "pay-a", vec![]),
    )
    .await
    .expect("scoped payment");
    let credit = credit_repo::assign_credit(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        credit_repo::AssignCredit {
            payment_id: payment.payment_id,
            shareholder_id: create_shareholder_at(
                &db.pool,
                Some(fx.coop_a),
                fx.user,
                "Ortak",
                "A",
                1,
            )
            .await
            .shareholder_id,
            amount: Decimal::from(500),
            note: None,
            idempotency_key: "cr-a".to_string(),
            fingerprint: "fp-cr-a".to_string(),
        },
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("scoped credit");
    assert_eq!(
        coop_of(&db.pool, "shareholder_credits", credit.credit_id).await,
        Some(fx.coop_a),
        "scoped writes carry the cooperative"
    );

    db.cleanup().await;
}
