//! M1-P3-B1 — cooperative-local numbering allocator tests.
//!
//! Proves the foundational allocator (migration 0026 + `numbering`
//! module) on disposable databases: independent per-cooperative
//! counters, concurrency-safe allocation, transactional rollback,
//! reservation floors for imports/corrections, registry enforcement,
//! and that the manual `families.sequence_number` lane is untouched.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable
//! database, same convention as the other integration suites).

use std::collections::HashSet;

use kooperatif_server::db as app_db;
use kooperatif_server::numbering::{self, NumberingSeries};
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

async fn insert_cooperative(pool: &PgPool, name: &str, bootstrap: bool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO cooperatives (id, name, status, is_bootstrap, business_enabled) \
         VALUES ($1, $2, 'active', $3, true) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(name)
    .bind(bootstrap)
    .fetch_one(pool)
    .await
    .expect("cooperative fixture")
}

#[tokio::test]
async fn two_cooperatives_start_at_number_one() {
    let db = setup().await;
    let coop_a = insert_cooperative(&db.pool, "Numaralama A", true).await;
    let coop_b = insert_cooperative(&db.pool, "Numaralama B", false).await;

    // Each cooperative's counter is fully independent — both start at 1.
    assert_eq!(
        numbering::allocate(&db.pool, coop_a, NumberingSeries::PaymentNumber)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        numbering::allocate(&db.pool, coop_b, NumberingSeries::PaymentNumber)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        numbering::allocate(&db.pool, coop_a, NumberingSeries::PaymentNumber)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        numbering::allocate(&db.pool, coop_b, NumberingSeries::PaymentNumber)
            .await
            .unwrap(),
        2
    );

    // A different series in the same cooperative has its own counter.
    assert_eq!(
        numbering::allocate(&db.pool, coop_a, NumberingSeries::ShareNumber)
            .await
            .unwrap(),
        1
    );

    db.cleanup().await;
}

#[tokio::test]
async fn concurrent_allocations_never_duplicate() {
    let db = setup().await;
    let coop = insert_cooperative(&db.pool, "Numaralama", true).await;

    // 16 racing transactions in ONE cooperative — every allocated
    // number must be distinct (counter row serialization).
    const RACERS: usize = 16;
    let mut tasks = Vec::new();
    for _ in 0..RACERS {
        let pool = db.pool.clone();
        tasks.push(tokio::spawn(async move {
            let mut tx = pool.begin().await.expect("tx");
            let n = numbering::allocate(&mut *tx, coop, NumberingSeries::ShareholderCreditNumber)
                .await
                .expect("allocate");
            tx.commit().await.expect("commit");
            n
        }));
    }
    let mut numbers = HashSet::new();
    for task in tasks {
        numbers.insert(task.await.expect("racer"));
    }
    assert_eq!(
        numbers.len(),
        RACERS,
        "every concurrent allocation must receive a distinct number"
    );
    assert!(
        numbers.iter().all(|n| (1..=RACERS as i64).contains(n)),
        "numbers must form the contiguous range 1..=16: {numbers:?}"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn rollback_returns_the_number_to_the_pool() {
    let db = setup().await;
    let coop = insert_cooperative(&db.pool, "Numaralama", true).await;

    // A transaction that allocates then rolls back leaves no consumed
    // number — the next attempt reissues the same value (gapless).
    let mut tx = db.pool.begin().await.unwrap();
    let abandoned = numbering::allocate(&mut *tx, coop, NumberingSeries::ShareReturnNumber)
        .await
        .unwrap();
    assert_eq!(abandoned, 1);
    tx.rollback().await.unwrap();

    assert_eq!(
        numbering::allocate(&db.pool, coop, NumberingSeries::ShareReturnNumber)
            .await
            .unwrap(),
        1,
        "rolled-back allocation must be reissued"
    );
    assert_eq!(
        numbering::allocate(&db.pool, coop, NumberingSeries::ShareReturnNumber)
            .await
            .unwrap(),
        2
    );

    db.cleanup().await;
}

#[tokio::test]
async fn reserve_raises_floor_and_allocate_continues() {
    let db = setup().await;
    let coop = insert_cooperative(&db.pool, "Numaralama", true).await;

    // Import/correction path: number 41 arrived out-of-band — reserve
    // it so the allocator can never reissue it.
    let floor = numbering::reserve(&db.pool, coop, NumberingSeries::PeriodNumber, 41)
        .await
        .unwrap();
    assert_eq!(floor, 42);
    assert_eq!(
        numbering::allocate(&db.pool, coop, NumberingSeries::PeriodNumber)
            .await
            .unwrap(),
        42
    );

    // Reserving a value BELOW the current floor never lowers it.
    let floor = numbering::reserve(&db.pool, coop, NumberingSeries::PeriodNumber, 7)
        .await
        .unwrap();
    assert_eq!(floor, 43);
    assert_eq!(
        numbering::allocate(&db.pool, coop, NumberingSeries::PeriodNumber)
            .await
            .unwrap(),
        43
    );

    db.cleanup().await;
}

#[tokio::test]
async fn unknown_series_is_rejected() {
    let db = setup().await;
    let coop = insert_cooperative(&db.pool, "Numaralama", true).await;

    // The registry FK guards the allocator against fat-fingered series
    // names — anything outside the 20 keys fails closed.
    let err = sqlx::query_scalar::<_, i64>("SELECT allocate_document_number($1, $2)")
        .bind(coop)
        .bind("bogus.series")
        .fetch_one(&db.pool)
        .await
        .expect_err("unknown series must be rejected");
    assert!(matches!(err, sqlx::Error::Database(ref d) if d.is_foreign_key_violation()));

    // The manual family lane is not a series.
    let err = sqlx::query_scalar::<_, i64>("SELECT allocate_document_number($1, $2)")
        .bind(coop)
        .bind("families.sequence_number")
        .fetch_one(&db.pool)
        .await
        .expect_err("manual lane must not be an allocator series");
    assert!(matches!(err, sqlx::Error::Database(ref d) if d.is_foreign_key_violation()));

    db.cleanup().await;
}

#[tokio::test]
async fn manual_family_sequence_stays_untouched() {
    let db = setup().await;
    let coop = insert_cooperative(&db.pool, "Numaralama", true).await;

    // Operator-supplied sequence numbers insert and stay scoped-unique
    // without any allocator involvement.
    sqlx::query(
        "INSERT INTO families (id, sequence_number, cooperative_id) \
         VALUES ($1, 7, $2)",
    )
    .bind(Uuid::new_v4())
    .bind(coop)
    .execute(&db.pool)
    .await
    .expect("manual sequence insert");
    let dup = sqlx::query(
        "INSERT INTO families (id, sequence_number, cooperative_id) \
         VALUES ($1, 7, $2)",
    )
    .bind(Uuid::new_v4())
    .bind(coop)
    .execute(&db.pool)
    .await;
    assert!(
        dup.is_err(),
        "same-coop duplicate sequence must still be rejected"
    );
    let coop_b = insert_cooperative(&db.pool, "Numaralama B", false).await;
    sqlx::query(
        "INSERT INTO families (id, sequence_number, cooperative_id) \
         VALUES ($1, 7, $2)",
    )
    .bind(Uuid::new_v4())
    .bind(coop_b)
    .execute(&db.pool)
    .await
    .expect("cross-coop same sequence is independent");

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// M1-P3-B2 — adoption batch: payments.payment_number and
// account_transfers.transfer_number through the real repository paths.
// account_movements carries no number column (B2 discovery — reported,
// not improvised). families.sequence_number stays manual.
// ---------------------------------------------------------------------

use kooperatif_server::financial_accounts::model::AccountType;
use kooperatif_server::financial_accounts::repo as account_repo;
use kooperatif_server::payments::repo as payment_repo;
use rust_decimal::Decimal;
use time::OffsetDateTime;

async fn fixture_user(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash) VALUES ($1,$2,$3,$4)",
    )
    .bind(id)
    .bind(format!("u{}", &id.simple().to_string()[..8]))
    .bind("Fixture")
    .bind("not-a-hash")
    .execute(pool)
    .await
    .expect("fixture user");
    id
}

async fn fixture_person(pool: &PgPool, coop: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO persons (first_name, last_name, search_name, cooperative_id) \
         VALUES ('B2', 'Ödeyen', 'b2 odeyen', $1) RETURNING id",
    )
    .bind(coop)
    .fetch_one(pool)
    .await
    .expect("fixture person")
}

async fn make_account(pool: &PgPool, coop: Uuid, actor: Uuid, name: &str) -> Uuid {
    account_repo::create_account(
        pool,
        Some(coop),
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

fn payment_cmd(
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

#[tokio::test]
async fn adopted_series_number_independently_per_cooperative() {
    let db = setup().await;
    let coop_a = insert_cooperative(&db.pool, "Adopt A", true).await;
    let coop_b = insert_cooperative(&db.pool, "Adopt B", false).await;
    let user = fixture_user(&db.pool).await;
    let acc_a = make_account(&db.pool, coop_a, user, "Kasa A").await;
    let acc_a2 = make_account(&db.pool, coop_a, user, "Banka A").await;
    let acc_b = make_account(&db.pool, coop_b, user, "Kasa B").await;
    let acc_b2 = make_account(&db.pool, coop_b, user, "Banka B").await;

    // Repository INSERTs allocate inside the business transaction —
    // each cooperative starts at 1 for BOTH adopted series.
    let pay_a = payment_repo::create_payment(
        &db.pool,
        Some(coop_a),
        user,
        payment_cmd(acc_a, 500, "pay-a1", Vec::new()),
    )
    .await
    .expect("coop-A payment");
    assert_eq!(pay_a.payment_number, 1);
    let pay_b = payment_repo::create_payment(
        &db.pool,
        Some(coop_b),
        user,
        payment_cmd(acc_b, 500, "pay-b1", Vec::new()),
    )
    .await
    .expect("coop-B payment");
    assert_eq!(pay_b.payment_number, 1);
    let pay_a2 = payment_repo::create_payment(
        &db.pool,
        Some(coop_a),
        user,
        payment_cmd(acc_a, 500, "pay-a2", Vec::new()),
    )
    .await
    .expect("coop-A second payment");
    assert_eq!(pay_a2.payment_number, 2);

    let tr_a = account_repo::post_transfer(
        &db.pool,
        Some(coop_a),
        user,
        account_repo::PostTransfer {
            source_account_id: acc_a,
            destination_account_id: acc_a2,
            amount: Decimal::from(100),
            occurred_at: OffsetDateTime::now_utc(),
            note: None,
            idempotency_key: "tr-a1".to_string(),
            fingerprint: "fp-tr-a1".to_string(),
        },
    )
    .await
    .expect("coop-A transfer");
    assert_eq!(tr_a.transfer_number, 1);
    let tr_b = account_repo::post_transfer(
        &db.pool,
        Some(coop_b),
        user,
        account_repo::PostTransfer {
            source_account_id: acc_b,
            destination_account_id: acc_b2,
            amount: Decimal::from(100),
            occurred_at: OffsetDateTime::now_utc(),
            note: None,
            idempotency_key: "tr-b1".to_string(),
            fingerprint: "fp-tr-b1".to_string(),
        },
    )
    .await
    .expect("coop-B transfer");
    assert_eq!(tr_b.transfer_number, 1);

    db.cleanup().await;
}

#[tokio::test]
async fn failed_payment_rolls_back_allocated_number() {
    let db = setup().await;
    let coop = insert_cooperative(&db.pool, "Adopt", true).await;
    let user = fixture_user(&db.pool).await;
    let acc = make_account(&db.pool, coop, user, "Kasa").await;

    let first = payment_repo::create_payment(
        &db.pool,
        Some(coop),
        user,
        payment_cmd(acc, 100, "ok-1", Vec::new()),
    )
    .await
    .expect("first payment");
    assert_eq!(first.payment_number, 1);

    // Allocation to a nonexistent assessment → FK failure inside the
    // SAME transaction that allocated the number → everything rolls
    // back, including the counter.
    let err = payment_repo::create_payment(
        &db.pool,
        Some(coop),
        user,
        payment_cmd(
            acc,
            50,
            "bad-alloc",
            vec![payment_repo::NewAllocation {
                assessment_id: Uuid::new_v4(),
                amount: Decimal::from(50),
            }],
        ),
    )
    .await;
    assert!(
        matches!(
            err,
            Err(payment_repo::PaymentCommandError::InactiveAssessment)
        ),
        "dangling allocation must fail"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM payments")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1,
        "failed payment left no row"
    );

    let second = payment_repo::create_payment(
        &db.pool,
        Some(coop),
        user,
        payment_cmd(acc, 100, "ok-2", Vec::new()),
    )
    .await
    .expect("second payment");
    assert_eq!(
        second.payment_number, 2,
        "rolled-back allocation must be reissued — no ghost gap"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn idempotent_replay_never_writes_a_second_document() {
    let db = setup().await;
    let coop = insert_cooperative(&db.pool, "Adopt", true).await;
    let user = fixture_user(&db.pool).await;
    let acc = make_account(&db.pool, coop, user, "Kasa").await;

    let first = payment_repo::create_payment(
        &db.pool,
        Some(coop),
        user,
        payment_cmd(acc, 100, "idem-1", Vec::new()),
    )
    .await
    .expect("first payment");
    let replay = payment_repo::create_payment(
        &db.pool,
        Some(coop),
        user,
        payment_cmd(acc, 100, "idem-1", Vec::new()),
    )
    .await
    .expect("replay must succeed as replay");
    assert!(replay.replayed);
    assert_eq!(replay.payment_id, first.payment_id);
    assert_eq!(replay.payment_number, first.payment_number);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM payments")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1,
        "replay never writes a second document"
    );

    // Different payload under the same key still conflicts.
    let mut conflicting = payment_cmd(acc, 999, "idem-1", Vec::new());
    conflicting.fingerprint = "fp-DIFFERENT".to_string();
    let err = payment_repo::create_payment(&db.pool, Some(coop), user, conflicting).await;
    assert!(
        matches!(
            err,
            Err(payment_repo::PaymentCommandError::IdempotencyConflict)
        ),
        "fingerprint mismatch must conflict"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn legacy_identity_writes_are_fail_closed() {
    let db = setup().await;
    let coop = insert_cooperative(&db.pool, "Adopt", true).await;
    let user = fixture_user(&db.pool).await;
    let person = fixture_person(&db.pool, coop).await;
    let acc = make_account(&db.pool, coop, user, "Kasa").await;

    for (table, column) in [
        ("payments", "payment_number"),
        ("account_transfers", "transfer_number"),
        ("periods", "period_number"),
        ("shares", "share_number"),
        ("shareholder_credits", "credit_number"),
        ("income_entries", "income_number"),
        ("expense_entries", "expense_number"),
        ("share_returns", "return_number"),
        ("share_return_entitlements", "entitlement_number"),
        ("share_return_settlements", "settlement_number"),
        ("investments", "investment_number"),
        ("investment_fundings", "funding_number"),
        ("investment_valuations", "valuation_number"),
        ("investment_incomes", "income_number"),
        ("investment_disposals", "disposal_number"),
        ("social_aid_funds", "fund_number"),
        ("social_aid_donations", "donation_number"),
        ("social_aid_disbursements", "disbursement_number"),
        ("governance_bodies", "body_number"),
        ("governance_decisions", "decision_number"),
    ] {
        let identity: String = sqlx::query_scalar(
            "SELECT is_identity FROM information_schema.columns \
             WHERE table_name = $1 AND column_name = $2",
        )
        .bind(table)
        .bind(column)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(identity, "NO", "{table}.{column} must not be IDENTITY");
    }

    // 0058: the number is trigger-assigned on every lane — an INSERT
    // that omits `payment_number` succeeds and receives the next
    // counter value; mixed/legacy numbering stays impossible because
    // the restricted lane may never supply the value itself.
    let assigned: i64 = sqlx::query_scalar(
        "INSERT INTO payments (id, payer_person_id, destination_account_id, amount, method, \
             received_at, status, idempotency_key, idempotency_fingerprint, created_by, \
             cooperative_id) \
         VALUES ($1, $2, $3, 10, 'cash', now(), 'posted', 'x', 'fp', $4, $5) \
         RETURNING payment_number",
    )
    .bind(Uuid::new_v4())
    .bind(person)
    .bind(acc)
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .expect("number-less insert receives a server-assigned number");
    assert!(assigned > 0, "counter-assigned number, never NULL");

    db.cleanup().await;
}

#[tokio::test]
async fn seeding_continues_from_existing_cooperative_max() {
    let db = setup().await;
    let coop_a = insert_cooperative(&db.pool, "Adopt A", true).await;
    let coop_b = insert_cooperative(&db.pool, "Adopt B", false).await;
    let user = fixture_user(&db.pool).await;
    let person_a = fixture_person(&db.pool, coop_a).await;
    let person_b = fixture_person(&db.pool, coop_b).await;
    let acc_a = make_account(&db.pool, coop_a, user, "Kasa A").await;
    let acc_b = make_account(&db.pool, coop_b, user, "Kasa B").await;

    // Pre-adoption rows carry explicit numbers (what IDENTITY would
    // have produced) — simulating a populated table at migration time.
    for (coop, acc, payer, number) in [
        (coop_a, acc_a, person_a, 7_i64),
        (coop_a, acc_a, person_a, 8),
        (coop_b, acc_b, person_b, 3),
    ] {
        sqlx::query(
            "INSERT INTO payments (id, payer_person_id, destination_account_id, amount, method, received_at, \
                 status, idempotency_key, idempotency_fingerprint, created_by, cooperative_id, \
                 payment_number) \
             VALUES ($1, $7, $2, 10, 'cash', now(), 'posted', $3, 'fp', $4, $5, $6)",
        )
        .bind(Uuid::new_v4())
        .bind(acc)
        .bind(format!("seed-{coop}-{number}"))
        .bind(user)
        .bind(coop)
        .bind(number)
        .bind(payer)
        .execute(&db.pool)
        .await
        .expect("seeded legacy row");
    }

    // The exact seed statement from migration 0027.
    sqlx::query(
        "INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value) \
         SELECT cooperative_id, 'payments.payment_number', MAX(payment_number) + 1 \
           FROM payments GROUP BY cooperative_id \
         ON CONFLICT (cooperative_id, series_key) DO NOTHING",
    )
    .execute(&db.pool)
    .await
    .expect("seed");

    // The allocator continues per cooperative — A at 9, B at 4 —
    // never renumbering, never colliding with existing documents.
    assert_eq!(
        numbering::allocate(&db.pool, coop_a, NumberingSeries::PaymentNumber)
            .await
            .unwrap(),
        9
    );
    assert_eq!(
        numbering::allocate(&db.pool, coop_b, NumberingSeries::PaymentNumber)
            .await
            .unwrap(),
        4
    );
    let stored: Vec<i64> = sqlx::query_scalar(
        "SELECT payment_number FROM payments WHERE cooperative_id = $1 ORDER BY payment_number",
    )
    .bind(coop_a)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(stored, vec![7, 8], "historical numbers unchanged");

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// M1-P3-B3 — adoption batch: periods.period_number, shares.share_number
// and shareholder_credits.credit_number through the real repository
// paths. credit_applications carries no number column (B3 discovery —
// reported, not improvised). families.sequence_number stays manual.
// ---------------------------------------------------------------------

use kooperatif_server::credits::repo as credit_repo;
use kooperatif_server::periods::model::AssessmentRuleType;
use kooperatif_server::periods::repo as period_repo;
use kooperatif_server::shares::model::AcquisitionType;
use kooperatif_server::shares::repo as share_repo;
use time::{Date, Month};

fn b3_date(year: i32, month: u8, day: u8) -> Date {
    Date::from_calendar_date(year, Month::try_from(month).unwrap(), day).unwrap()
}

async fn fixture_shareholder(pool: &PgPool, coop: Uuid) -> Uuid {
    let person = fixture_person(pool, coop).await;
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

async fn make_period(pool: &PgPool, coop: Uuid, actor: Uuid, name: &str) -> (Uuid, i64) {
    period_repo::create_period(
        pool,
        Some(coop),
        actor,
        period_repo::CreatePeriod {
            name: name.to_string(),
            collection_start_date: b3_date(2026, 1, 1),
            due_date: b3_date(2026, 3, 31),
            rule_type: AssessmentRuleType::PerShareholder,
            base_amount: Decimal::from(100),
            assessment_effective_date: b3_date(2026, 1, 1),
        },
    )
    .await
    .expect("period")
}

async fn make_share(pool: &PgPool, coop: Uuid, actor: Uuid, shareholder: Uuid) -> (Uuid, i64) {
    make_share_at(pool, coop, actor, shareholder, OffsetDateTime::now_utc()).await
}

async fn make_share_at(
    pool: &PgPool,
    coop: Uuid,
    actor: Uuid,
    shareholder: Uuid,
    effective_at: OffsetDateTime,
) -> (Uuid, i64) {
    share_repo::create_share(
        pool,
        Some(coop),
        actor,
        share_repo::CreateShare {
            shareholder_id: shareholder,
            acquisition_type: AcquisitionType::Founder,
            fee: None,
            effective_at,
            reason: None,
        },
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("share")
}

async fn next_counter(pool: &PgPool, coop: Uuid, series: &str) -> Option<i64> {
    sqlx::query_scalar(
        "SELECT next_value FROM cooperative_number_counters \
         WHERE cooperative_id = $1 AND series_key = $2",
    )
    .bind(coop)
    .bind(series)
    .fetch_optional(pool)
    .await
    .expect("counter read")
}

#[tokio::test]
async fn b3_adopted_series_number_independently_per_cooperative() {
    let db = setup().await;
    let coop_a = insert_cooperative(&db.pool, "B3 A", true).await;
    let coop_b = insert_cooperative(&db.pool, "B3 B", false).await;
    let user = fixture_user(&db.pool).await;

    for coop in [coop_a, coop_b] {
        let shareholder = fixture_shareholder(&db.pool, coop).await;
        let account = make_account(&db.pool, coop, user, "Kasa").await;

        let (_, period_one) = make_period(&db.pool, coop, user, "Dönem 1").await;
        let (_, period_two) = make_period(&db.pool, coop, user, "Dönem 2").await;
        assert_eq!((period_one, period_two), (1, 2), "periods start at 1");

        let (_, share_one) = make_share(&db.pool, coop, user, shareholder).await;
        let (_, share_two) = make_share(&db.pool, coop, user, shareholder).await;
        assert_eq!((share_one, share_two), (1, 2), "shares start at 1");

        let payment = payment_repo::create_payment(
            &db.pool,
            Some(coop),
            user,
            payment_cmd(account, 300, &format!("b3-src-{coop}"), Vec::new()),
        )
        .await
        .expect("payment");
        let credit = credit_repo::assign_credit(
            &db.pool,
            Some(coop),
            user,
            credit_repo::AssignCredit {
                payment_id: payment.payment_id,
                shareholder_id: shareholder,
                amount: Decimal::from(50),
                note: None,
                idempotency_key: format!("b3-credit-{coop}"),
                fingerprint: "fp".to_string(),
            },
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("credit");
        assert!(!credit.replayed);
        assert_eq!(credit.credit_number, 1, "credits start at 1");
    }

    // Same coop-local values in both cooperatives — scoped uniqueness
    // accepts them, global reuse is impossible to distinguish.
    for (table, col) in [
        ("periods", "period_number"),
        ("shares", "share_number"),
        ("shareholder_credits", "credit_number"),
    ] {
        let dup = sqlx::query_scalar::<_, i64>(&format!(
            "SELECT count(*) - count(DISTINCT (cooperative_id, {col})) FROM {table}"
        ))
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(
            dup, 0,
            "{table}.{col} must stay unique inside a cooperative"
        );
    }

    db.cleanup().await;
}

#[tokio::test]
async fn b3_successful_replay_does_not_advance_the_counter() {
    let db = setup().await;
    let coop = insert_cooperative(&db.pool, "B3 Replay", true).await;
    let user = fixture_user(&db.pool).await;
    let shareholder = fixture_shareholder(&db.pool, coop).await;
    let account = make_account(&db.pool, coop, user, "Kasa").await;

    let payment = payment_repo::create_payment(
        &db.pool,
        Some(coop),
        user,
        payment_cmd(account, 500, "b3-replay-src", Vec::new()),
    )
    .await
    .expect("payment");

    let credit_cmd = || credit_repo::AssignCredit {
        payment_id: payment.payment_id,
        shareholder_id: shareholder,
        amount: Decimal::from(100),
        note: None,
        idempotency_key: "b3-credit-replay".to_string(),
        fingerprint: "fp-credit".to_string(),
    };
    let first = credit_repo::assign_credit(
        &db.pool,
        Some(coop),
        user,
        credit_cmd(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("first assignment");
    let after_first = next_counter(&db.pool, coop, "shareholder_credits.credit_number")
        .await
        .expect("counter row exists after first allocation");

    // Sequential replay — same key + fingerprint, resolved by the early
    // short-circuit BEFORE the INSERT, so the allocator never runs.
    let replay = credit_repo::assign_credit(
        &db.pool,
        Some(coop),
        user,
        credit_cmd(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("replay");
    assert!(replay.replayed);
    assert_eq!(replay.credit_number, first.credit_number);
    assert_eq!(
        next_counter(&db.pool, coop, "shareholder_credits.credit_number").await,
        Some(after_first),
        "a successful replay must not consume a number"
    );

    // Concurrent identical requests through the ON CONFLICT lane: the
    // loser's speculative INSERT already evaluated the allocator inside
    // its transaction, which then rolls back — the counter must advance
    // by EXACTLY one for exactly one stored document.
    let payments_before = next_counter(&db.pool, coop, "payments.payment_number")
        .await
        .unwrap_or(1);
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let pool = db.pool.clone();
        tasks.push(tokio::spawn(async move {
            payment_repo::create_payment(
                &pool,
                Some(coop),
                user,
                payment_cmd(account, 10, "b3-race", Vec::new()),
            )
            .await
        }));
    }
    let mut ok = 0;
    for task in tasks {
        if task.await.unwrap().is_ok() {
            ok += 1;
        }
    }
    assert!(ok >= 1, "at least the winner must succeed");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM payments WHERE idempotency_key = 'b3-race'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1,
        "exactly one document for one key"
    );
    assert_eq!(
        next_counter(&db.pool, coop, "payments.payment_number").await,
        Some(payments_before + 1),
        "concurrent replays consumed exactly one number"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn b3_stale_counter_rejected_and_consistent_counter_kept() {
    let db = setup().await;
    let coop = insert_cooperative(&db.pool, "B3 Stale", true).await;
    let user = fixture_user(&db.pool).await;
    let _ = make_share(
        &db.pool,
        coop,
        user,
        fixture_shareholder(&db.pool, coop).await,
    )
    .await;
    // Counter now sits at 2 (share_number 1 issued). A stale counter at
    // or below the live maximum must be rejected by the migration's
    // own check SQL.
    let stale_check = sqlx::query(
        "DO $$ \
         BEGIN \
             IF EXISTS ( \
                 SELECT 1 FROM cooperative_number_counters c \
                   JOIN shares s ON s.cooperative_id = c.cooperative_id \
                  WHERE c.series_key = 'shares.share_number' \
                    AND c.next_value <= s.share_number) THEN \
                 RAISE EXCEPTION 'stale cooperative counter for shares.share_number'; \
             END IF; \
         END $$",
    )
    .execute(&db.pool)
    .await;
    assert!(
        stale_check.is_ok(),
        "consistent counter (next=2 > max=1) must pass the stale check"
    );

    sqlx::query(
        "UPDATE cooperative_number_counters SET next_value = 1 \
         WHERE cooperative_id = $1 AND series_key = 'shares.share_number'",
    )
    .bind(coop)
    .execute(&db.pool)
    .await
    .expect("forced stale counter");
    let stale_check = sqlx::query(
        "DO $$ \
         BEGIN \
             IF EXISTS ( \
                 SELECT 1 FROM cooperative_number_counters c \
                   JOIN shares s ON s.cooperative_id = c.cooperative_id \
                  WHERE c.series_key = 'shares.share_number' \
                    AND c.next_value <= s.share_number) THEN \
                 RAISE EXCEPTION 'stale cooperative counter for shares.share_number'; \
             END IF; \
         END $$",
    )
    .execute(&db.pool)
    .await;
    assert!(
        stale_check.is_err(),
        "counter at/below the live maximum must be rejected before IDENTITY removal"
    );

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// M1-P3-B4 — adoption batch: income_entries.income_number,
// expense_entries.expense_number, share_returns.return_number,
// share_return_entitlements.entitlement_number,
// share_return_settlements.settlement_number through the real
// repository paths. share_returns.share_number is a denormalized copy
// of the share's own number, NOT a generated series — untouched.
// ---------------------------------------------------------------------

use kooperatif_server::income_expense::model::{CategoryType, EntryKind};
use kooperatif_server::income_expense::repo as entry_repo;
use kooperatif_server::share_returns::model::EntitlementType;
use kooperatif_server::share_returns::repo as return_repo;

async fn make_category(
    pool: &PgPool,
    coop: Uuid,
    actor: Uuid,
    kind: CategoryType,
    name: &str,
) -> Uuid {
    entry_repo::create_category(
        pool,
        Some(coop),
        actor,
        entry_repo::CreateCategory {
            category_type: kind,
            name: name.to_string(),
            description: None,
        },
    )
    .await
    .expect("category")
}

fn entry_cmd(account: Uuid, category: Uuid, amount: i64, key: &str) -> entry_repo::PostEntry {
    entry_repo::PostEntry {
        financial_account_id: account,
        category_id: category,
        amount: Decimal::from(amount),
        occurred_at: OffsetDateTime::now_utc(),
        description: "b4".to_string(),
        counterparty: None,
        reference_no: None,
        idempotency_key: key.to_string(),
        fingerprint: format!("fp-{key}"),
    }
}

#[tokio::test]
async fn b4_adopted_series_number_independently_per_cooperative() {
    let db = setup().await;
    let coop_a = insert_cooperative(&db.pool, "B4 A", true).await;
    let coop_b = insert_cooperative(&db.pool, "B4 B", false).await;
    let user = fixture_user(&db.pool).await;

    for (coop, tag) in [(coop_a, "a"), (coop_b, "b")] {
        let shareholder = fixture_shareholder(&db.pool, coop).await;
        let account = make_account(&db.pool, coop, user, "Kasa").await;
        let cat_in = make_category(&db.pool, coop, user, CategoryType::Income, "Kira").await;
        let cat_out = make_category(&db.pool, coop, user, CategoryType::Expense, "Elektrik").await;

        // Fund the account through a real payment so the expense and
        // settlement outflows are legal.
        payment_repo::create_payment(
            &db.pool,
            Some(coop),
            user,
            payment_cmd(account, 5000, &format!("b4-fund-{tag}"), Vec::new()),
        )
        .await
        .expect("funding payment");

        let income_one = entry_repo::post_entry(
            &db.pool,
            Some(coop),
            user,
            EntryKind::Income,
            entry_cmd(account, cat_in, 3000, &format!("b4-inc1-{tag}")),
        )
        .await
        .expect("income 1");
        let income_two = entry_repo::post_entry(
            &db.pool,
            Some(coop),
            user,
            EntryKind::Income,
            entry_cmd(account, cat_in, 100, &format!("b4-inc2-{tag}")),
        )
        .await
        .expect("income 2");
        assert_eq!(
            (income_one.entry_number, income_two.entry_number),
            (1, 2),
            "income entries start at 1 per cooperative"
        );

        let expense = entry_repo::post_entry(
            &db.pool,
            Some(coop),
            user,
            EntryKind::Expense,
            entry_cmd(account, cat_out, 150, &format!("b4-exp-{tag}")),
        )
        .await
        .expect("expense");
        assert_eq!(expense.entry_number, 1, "expense series is independent");

        // share return chain: initiate -> finalize (entitlement) ->
        // settle — three series on one lifecycle. Ownership must start
        // before today's Istanbul day boundary for the effective date.
        let (share_id, _) = make_share_at(
            &db.pool,
            coop,
            user,
            shareholder,
            OffsetDateTime::now_utc() - time::Duration::days(2),
        )
        .await;
        let now = OffsetDateTime::now_utc();
        let initiated = return_repo::initiate_return(
            &db.pool,
            Some(coop),
            user,
            return_repo::InitiateReturn {
                share_id,
                effective_return_date: now.date(),
                reason: None,
                idempotency_key: format!("b4-sr-{tag}"),
                fingerprint: format!("fp-sr-{tag}"),
            },
            now,
        )
        .await
        .expect("initiate return");
        assert_eq!(initiated.return_number, 1, "returns start at 1");

        let detail = return_repo::get_return(&db.pool, Some(coop), initiated.return_id)
            .await
            .expect("detail")
            .expect("row");
        let finalized = return_repo::finalize_return(
            &db.pool,
            Some(coop),
            user,
            initiated.return_id,
            detail.updated_at,
            &[return_repo::EntitlementSpec {
                entitlement_type: EntitlementType::Principal,
                amount: Some(Decimal::from(1200)),
                due_date: Some(b3_date(2026, 12, 31)),
                policy_reference: None,
                description: None,
            }],
            now,
        )
        .await
        .expect("finalize");
        let settlement = return_repo::post_settlement(
            &db.pool,
            Some(coop),
            user,
            return_repo::PostSettlement {
                entitlement_id: finalized.entitlement_ids[0],
                financial_account_id: account,
                amount: Decimal::from(1200),
                settled_at: now,
                idempotency_key: format!("b4-set-{tag}"),
                fingerprint: format!("fp-set-{tag}"),
            },
        )
        .await
        .expect("settlement");
        assert_eq!(settlement.settlement_number, 1, "settlements start at 1");

        let entitlement_number: i64 = sqlx::query_scalar(
            "SELECT entitlement_number FROM share_return_entitlements WHERE id = $1",
        )
        .bind(finalized.entitlement_ids[0])
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(entitlement_number, 1, "entitlements start at 1");
    }

    // Cross-cooperative duplicates of the same business number are the
    // intended scoped semantics; inside one coop they stay unique.
    for (table, col) in [
        ("income_entries", "income_number"),
        ("expense_entries", "expense_number"),
        ("share_returns", "return_number"),
        ("share_return_entitlements", "entitlement_number"),
        ("share_return_settlements", "settlement_number"),
    ] {
        let dup = sqlx::query_scalar::<_, i64>(&format!(
            "SELECT count(*) - count(DISTINCT (cooperative_id, {col})) FROM {table}"
        ))
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(
            dup, 0,
            "{table}.{col} must stay unique inside a cooperative"
        );
    }

    db.cleanup().await;
}

#[tokio::test]
async fn b4_entry_replay_and_failure_leave_the_counter_untouched() {
    let db = setup().await;
    let coop = insert_cooperative(&db.pool, "B4 Replay", true).await;
    let user = fixture_user(&db.pool).await;
    let account = make_account(&db.pool, coop, user, "Kasa").await;
    let cat_in = make_category(&db.pool, coop, user, CategoryType::Income, "Kira").await;
    let cat_out = make_category(&db.pool, coop, user, CategoryType::Expense, "Gider").await;

    payment_repo::create_payment(
        &db.pool,
        Some(coop),
        user,
        payment_cmd(account, 1000, "b4-fund", Vec::new()),
    )
    .await
    .expect("funding payment");

    let first = entry_repo::post_entry(
        &db.pool,
        Some(coop),
        user,
        EntryKind::Income,
        entry_cmd(account, cat_in, 500, "b4-inc-replay"),
    )
    .await
    .expect("income");
    let after_first = next_counter(&db.pool, coop, "income_entries.income_number")
        .await
        .expect("counter row exists after first allocation");

    // Successful replay returns the same document number and the
    // counter does not move.
    let replay = entry_repo::post_entry(
        &db.pool,
        Some(coop),
        user,
        EntryKind::Income,
        entry_cmd(account, cat_in, 500, "b4-inc-replay"),
    )
    .await
    .expect("replay");
    assert!(replay.replayed);
    assert_eq!(replay.entry_number, first.entry_number);
    assert_eq!(
        next_counter(&db.pool, coop, "income_entries.income_number").await,
        Some(after_first),
        "replay must not consume a number"
    );

    // A rejected command (expense exceeding the free balance after the
    // reservation rules) must not leave a counter gap either — the
    // allocator runs inside the rolled-back transaction.
    let expense_before = next_counter(&db.pool, coop, "expense_entries.expense_number").await;
    let failed = entry_repo::post_entry(
        &db.pool,
        Some(coop),
        user,
        EntryKind::Expense,
        entry_cmd(account, cat_out, 9_999_999, "b4-exp-fail"),
    )
    .await;
    assert!(failed.is_err(), "unfundable expense must fail");
    assert_eq!(
        next_counter(&db.pool, coop, "expense_entries.expense_number").await,
        expense_before,
        "a failed posting leaves no counter residue"
    );

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// M1-P3-B5 — adoption batch: the five investment series.
// investment_disposal_proceeds carries no number column (a leg of its
// disposal) — not a series.
// ---------------------------------------------------------------------

use kooperatif_server::investments::model::InvestmentType;
use kooperatif_server::investments::repo as inv_repo;

fn investment_cmd(key: &str) -> inv_repo::CreateInvestment {
    inv_repo::CreateInvestment {
        name: format!("Arsa {key}"),
        investment_type: InvestmentType::RealEstate,
        description: None,
        location: None,
        reference: None,
        counterparty_name: None,
        acquired_at: Some(b3_date(2025, 6, 1)),
        idempotency_key: key.to_string(),
        fingerprint: format!("fp-{key}"),
    }
}

#[tokio::test]
async fn b5_adopted_series_number_independently_per_cooperative() {
    let db = setup().await;
    let coop_a = insert_cooperative(&db.pool, "B5 A", true).await;
    let coop_b = insert_cooperative(&db.pool, "B5 B", false).await;
    let user = fixture_user(&db.pool).await;

    for (coop, tag) in [(coop_a, "a"), (coop_b, "b")] {
        let account = make_account(&db.pool, coop, user, "Kasa").await;
        payment_repo::create_payment(
            &db.pool,
            Some(coop),
            user,
            payment_cmd(account, 9000, &format!("b5-fund-{tag}"), Vec::new()),
        )
        .await
        .expect("funding payment");

        let investment = inv_repo::create_investment(
            &db.pool,
            Some(coop),
            user,
            investment_cmd(&format!("b5-inv-{tag}")),
        )
        .await
        .expect("investment");
        assert_eq!(
            investment.investment_number, 1,
            "investments start at 1 per cooperative"
        );

        let funding = inv_repo::post_funding(
            &db.pool,
            Some(coop),
            user,
            inv_repo::PostFunding {
                investment_id: investment.investment_id,
                financial_account_id: account,
                amount: Decimal::from(4000),
                occurred_at: OffsetDateTime::now_utc(),
                reference: None,
                note: None,
                idempotency_key: format!("b5-fnd-{tag}"),
                fingerprint: format!("fp-fnd-{tag}"),
            },
        )
        .await
        .expect("funding");
        assert_eq!(funding.event_number, 1, "fundings start at 1");

        let valuation = inv_repo::record_valuation(
            &db.pool,
            Some(coop),
            user,
            inv_repo::RecordValuation {
                investment_id: investment.investment_id,
                valuation_date: b3_date(2026, 6, 30),
                amount: Decimal::from(5000),
                method: None,
                source: None,
                note: None,
                idempotency_key: format!("b5-val-{tag}"),
                fingerprint: format!("fp-val-{tag}"),
            },
        )
        .await
        .expect("valuation");
        assert_eq!(valuation.event_number, 1, "valuations start at 1");

        let income = inv_repo::post_income(
            &db.pool,
            Some(coop),
            user,
            inv_repo::PostIncome {
                investment_id: investment.investment_id,
                financial_account_id: account,
                amount: Decimal::from(300),
                occurred_at: OffsetDateTime::now_utc(),
                description: "kira".to_string(),
                counterparty: None,
                reference_no: None,
                idempotency_key: format!("b5-ini-{tag}"),
                fingerprint: format!("fp-ini-{tag}"),
            },
        )
        .await
        .expect("investment income");
        assert_eq!(income.event_number, 1, "investment incomes start at 1");

        let disposal = inv_repo::dispose_investment(
            &db.pool,
            Some(coop),
            user,
            inv_repo::DisposeInvestment {
                investment_id: investment.investment_id,
                disposed_at: b3_date(2026, 9, 1),
                consideration_amount: Some(Decimal::from(5200)),
                counterparty_name: None,
                reference: None,
                note: None,
                proceeds: vec![inv_repo::ProceedsLeg {
                    financial_account_id: account,
                    amount: Decimal::from(5200),
                    occurred_at: OffsetDateTime::now_utc(),
                    reference: None,
                }],
                idempotency_key: format!("b5-dsp-{tag}"),
                fingerprint: format!("fp-dsp-{tag}"),
            },
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("disposal");
        assert_eq!(disposal.event_number, 1, "disposals start at 1");
    }

    for (table, col) in [
        ("investments", "investment_number"),
        ("investment_fundings", "funding_number"),
        ("investment_valuations", "valuation_number"),
        ("investment_incomes", "income_number"),
        ("investment_disposals", "disposal_number"),
    ] {
        let dup = sqlx::query_scalar::<_, i64>(&format!(
            "SELECT count(*) - count(DISTINCT (cooperative_id, {col})) FROM {table}"
        ))
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(
            dup, 0,
            "{table}.{col} must stay unique inside a cooperative"
        );
    }

    db.cleanup().await;
}

#[tokio::test]
async fn b5_replay_and_concurrent_race_do_not_consume_numbers() {
    let db = setup().await;
    let coop = insert_cooperative(&db.pool, "B5 Replay", true).await;
    let user = fixture_user(&db.pool).await;

    // Sequential replay — the early short-circuit returns before the
    // INSERT, so the allocator never runs.
    let first = inv_repo::create_investment(&db.pool, Some(coop), user, investment_cmd("b5-r1"))
        .await
        .expect("first");
    let after_first = next_counter(&db.pool, coop, "investments.investment_number")
        .await
        .expect("counter exists");
    let replay = inv_repo::create_investment(&db.pool, Some(coop), user, investment_cmd("b5-r1"))
        .await
        .expect("replay");
    assert!(replay.replayed);
    assert_eq!(replay.investment_number, first.investment_number);
    assert_eq!(
        next_counter(&db.pool, coop, "investments.investment_number").await,
        Some(after_first),
        "replay must not consume a number"
    );

    // Concurrent identical requests: create_investment now wraps its
    // INSERT in a transaction, so the loser's speculative allocation
    // rolls back with it — exactly one number consumed for one row.
    let before = next_counter(&db.pool, coop, "investments.investment_number")
        .await
        .unwrap_or(1);
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let pool = db.pool.clone();
        tasks.push(tokio::spawn(async move {
            inv_repo::create_investment(&pool, Some(coop), user, investment_cmd("b5-race")).await
        }));
    }
    for task in tasks {
        task.await.unwrap().expect("winner or replay");
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM investments WHERE idempotency_key = 'b5-race'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1,
        "exactly one document for one key"
    );
    assert_eq!(
        next_counter(&db.pool, coop, "investments.investment_number").await,
        Some(before + 1),
        "concurrent replays consumed exactly one number"
    );

    db.cleanup().await;
}

// ==================================================================
// M1-P3-B6 — final five series: social_aid_funds/donations/
// disbursements, governance_bodies/decisions
// ==================================================================

use kooperatif_server::governance::repo as gov_repo;
use kooperatif_server::social_aid::repo as aid_repo;

fn fund_cmd(key: &str, name: &str) -> aid_repo::CreateFund {
    aid_repo::CreateFund {
        name: name.into(),
        description: None,
        starts_on: None,
        ends_on: None,
        idempotency_key: key.into(),
        fingerprint: format!("fp-{key}"),
    }
}

fn donation_cmd(fund: Uuid, account: Uuid, amount: i64, key: &str) -> aid_repo::PostDonation {
    aid_repo::PostDonation {
        fund_id: fund,
        donor_person_id: None,
        donor_display_name: Some("Bagisci".into()),
        financial_account_id: account,
        amount: Decimal::from(amount),
        occurred_at: OffsetDateTime::now_utc(),
        reference: None,
        note: None,
        idempotency_key: key.into(),
        fingerprint: format!("fp-{key}"),
    }
}

fn disbursement_cmd(
    fund: Uuid,
    account: Uuid,
    amount: i64,
    key: &str,
) -> aid_repo::PostDisbursement {
    aid_repo::PostDisbursement {
        fund_id: fund,
        beneficiary_person_id: None,
        beneficiary_display_name: Some("Yararlanan".into()),
        financial_account_id: account,
        amount: Decimal::from(amount),
        occurred_at: OffsetDateTime::now_utc(),
        reason: "gida".into(),
        reference: None,
        idempotency_key: key.into(),
        fingerprint: format!("fp-{key}"),
    }
}

fn body_cmd(key: &str) -> gov_repo::CreateBody {
    gov_repo::CreateBody {
        name: "Yonetim".into(),
        body_type: "board".into(),
        description: None,
        idempotency_key: key.into(),
        fingerprint: format!("fp-{key}"),
    }
}

fn decision_cmd(body: Uuid, key: &str) -> gov_repo::CreateDecision {
    gov_repo::CreateDecision {
        body_id: body,
        title: "Karar".into(),
        decision_text: "Metin".into(),
        decision_on: b3_date(2025, 1, 15),
        effective_on: None,
        idempotency_key: key.into(),
        fingerprint: format!("fp-{key}"),
    }
}

#[tokio::test]
async fn b6_adopted_series_number_independently_per_cooperative() {
    let db = setup().await;
    let coop_a = insert_cooperative(&db.pool, "B6 A", true).await;
    let coop_b = insert_cooperative(&db.pool, "B6 B", false).await;
    let user = fixture_user(&db.pool).await;
    let acc_a = make_account(&db.pool, coop_a, user, "Kasa A").await;
    let acc_b = make_account(&db.pool, coop_b, user, "Kasa B").await;

    let mut seen: HashSet<(&'static str, Uuid, i64)> = HashSet::new();

    // social_aid_funds
    for (coop, name) in [(coop_a, "Fon A"), (coop_b, "Fon B")] {
        let out = aid_repo::create_fund(&db.pool, Some(coop), user, fund_cmd("b6-f", name))
            .await
            .expect("create fund");
        assert!(!out.replayed);
        assert_eq!(out.fund_number, 1, "{name} must start at 1");
        assert!(seen.insert(("fund", coop, out.fund_number)));
    }

    // donations + disbursements (restricted availability funded first)
    for (coop, acc) in [(coop_a, acc_a), (coop_b, acc_b)] {
        let fund = aid_repo::create_fund(&db.pool, Some(coop), user, fund_cmd("b6-fx", "F"))
            .await
            .unwrap();
        let don = aid_repo::post_donation(
            &db.pool,
            Some(coop),
            user,
            donation_cmd(fund.fund_id, acc, 100, "b6-d"),
        )
        .await
        .expect("donation");
        assert_eq!(don.event_number, 1, "donation must start at 1");
        assert!(seen.insert(("donation", coop, don.event_number)));

        let dis = aid_repo::post_disbursement(
            &db.pool,
            Some(coop),
            user,
            disbursement_cmd(fund.fund_id, acc, 40, "b6-x"),
        )
        .await
        .expect("disbursement");
        assert_eq!(dis.event_number, 1, "disbursement must start at 1");
        assert!(seen.insert(("disbursement", coop, dis.event_number)));
    }

    // governance bodies + decisions
    for coop in [coop_a, coop_b] {
        let body = gov_repo::create_body(&db.pool, Some(coop), user, body_cmd("b6-b"))
            .await
            .expect("create body");
        assert_eq!(body.body_number, 1, "body must start at 1");
        assert!(seen.insert(("body", coop, body.body_number)));

        let dec = gov_repo::create_decision(
            &db.pool,
            Some(coop),
            user,
            decision_cmd(body.body_id, "b6-k"),
        )
        .await
        .expect("create decision");
        assert_eq!(dec.decision_number, 1, "decision must start at 1");
        assert!(seen.insert(("decision", coop, dec.decision_number)));
    }

    db.cleanup().await;
}

#[tokio::test]
async fn b6_replay_race_and_failure_leave_counters_untouched() {
    let db = setup().await;
    let coop = insert_cooperative(&db.pool, "B6 Replay", true).await;
    let user = fixture_user(&db.pool).await;
    let acc = make_account(&db.pool, coop, user, "Kasa").await;

    // Sequential replay — early short-circuit before the INSERT.
    let first = aid_repo::create_fund(&db.pool, Some(coop), user, fund_cmd("b6-r1", "Fon"))
        .await
        .expect("first");
    let after_first = next_counter(&db.pool, coop, "social_aid_funds.fund_number")
        .await
        .expect("counter exists");
    let replay = aid_repo::create_fund(&db.pool, Some(coop), user, fund_cmd("b6-r1", "Fon"))
        .await
        .expect("replay");
    assert!(replay.replayed);
    assert_eq!(replay.fund_number, first.fund_number);
    assert_eq!(
        next_counter(&db.pool, coop, "social_aid_funds.fund_number").await,
        Some(after_first),
        "replay must not consume a number"
    );

    // Concurrent identical requests: create_body now wraps its INSERT
    // in a transaction, so the loser's speculative allocation rolls
    // back with it — exactly one number consumed for one row.
    let before = next_counter(&db.pool, coop, "governance_bodies.body_number")
        .await
        .unwrap_or(1);
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let pool = db.pool.clone();
        tasks.push(tokio::spawn(async move {
            gov_repo::create_body(&pool, Some(coop), user, body_cmd("b6-race")).await
        }));
    }
    for task in tasks {
        task.await.unwrap().expect("winner or replay");
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM governance_bodies WHERE idempotency_key = 'b6-race'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1,
        "exactly one document for one key"
    );
    assert_eq!(
        next_counter(&db.pool, coop, "governance_bodies.body_number").await,
        Some(before + 1),
        "concurrent replays consumed exactly one number"
    );

    // Failed financial operation: a disbursement beyond restricted
    // availability is rejected BEFORE the INSERT — the counter must
    // not move and no partial movement may survive.
    let fund = aid_repo::create_fund(&db.pool, Some(coop), user, fund_cmd("b6-fz", "F"))
        .await
        .unwrap();
    aid_repo::post_donation(
        &db.pool,
        Some(coop),
        user,
        donation_cmd(fund.fund_id, acc, 50, "b6-dz"),
    )
    .await
    .unwrap();
    let before_dis = next_counter(
        &db.pool,
        coop,
        "social_aid_disbursements.disbursement_number",
    )
    .await
    .unwrap_or(1);
    let failed = aid_repo::post_disbursement(
        &db.pool,
        Some(coop),
        user,
        disbursement_cmd(fund.fund_id, acc, 999, "b6-xz"),
    )
    .await;
    assert!(failed.is_err(), "over-availability disbursement must fail");
    assert_eq!(
        next_counter(
            &db.pool,
            coop,
            "social_aid_disbursements.disbursement_number"
        )
        .await
        .unwrap_or(1),
        before_dis,
        "rejected disbursement must not consume a number"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM social_aid_disbursements WHERE idempotency_key = 'b6-xz'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0,
        "no partial disbursement"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM account_movements WHERE source_type = 'social_aid_disbursement'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0,
        "no partial cash movement"
    );

    db.cleanup().await;
}
