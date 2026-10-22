//! M1-P2-C3 Batch 4 — cooperative-scoped data access for investments
//! (funding/valuation/income/disposal), social aid (funds, donations,
//! disbursements), governance (bodies, memberships, decisions, votes)
//! and the reports aggregate projections.
//!
//! Proves at the repository layer — the enforcement point — that every
//! read, lock, insert, update and transactional command in this batch
//! is bound to the verified cooperative context, that cross-cooperative
//! identifiers resolve to not-found without disclosure and with ZERO
//! partial writes, that new rows carry the correct cooperative_id, that
//! report aggregates never leak foreign-cooperative totals, and that
//! the `None` legacy lane stays isolated.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same convention as the other integration suites).

use kooperatif_server::db as app_db;
use kooperatif_server::financial_accounts::model::AccountType;
use kooperatif_server::financial_accounts::repo as account_repo;
use kooperatif_server::governance::model::{DecisionOutcome, VoteChoice};
use kooperatif_server::governance::repo as gov_repo;
use kooperatif_server::investments::model::InvestmentType;
use kooperatif_server::investments::repo as inv_repo;
use kooperatif_server::reports::repo as report_repo;
use kooperatif_server::social_aid::repo as aid_repo;
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

/// Funds an account directly through a transfer-less income movement —
/// social-aid donation is the inflow path for restricted money, but the
/// account needs physical cash for outflows too; the canonical funding
/// path in repo-level tests is a payment-free direct movement.
async fn fund_account_at(
    pool: &PgPool,
    coop: Option<Uuid>,
    actor: Uuid,
    account_id: Uuid,
    amount: i64,
) {
    let movement_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO account_movements \
            (id, account_id, direction, amount, source_type, source_id, \
             occurred_at, status, created_by, cooperative_id) \
         VALUES ($1,$2,'inflow',$3,'transfer',$4, now(), 'active', $5, $6)",
    )
    .bind(movement_id)
    .bind(account_id)
    .bind(Decimal::new(amount, 2))
    .bind(movement_id)
    .bind(actor)
    .bind(coop)
    .execute(pool)
    .await
    .expect("fund movement");
}

async fn create_person_at(pool: &PgPool, coop: Option<Uuid>, first: &str, last: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO persons (first_name, last_name, search_name, cooperative_id) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(first)
    .bind(last)
    .bind(format!("{first} {last}").to_lowercase())
    .bind(coop)
    .fetch_one(pool)
    .await
    .expect("person")
}

fn key(tag: &str) -> (String, String) {
    (
        format!("b4-{tag}-{}", Uuid::new_v4().simple()),
        "fp".to_string(),
    )
}

fn date(y: i32, m: u8, d: u8) -> Date {
    Date::from_calendar_date(y, Month::try_from(m).unwrap(), d).unwrap()
}

async fn make_fund(pool: &PgPool, coop: Option<Uuid>, actor: Uuid, name: &str) -> Uuid {
    let (key, fp) = key("fund");
    aid_repo::create_fund(
        pool,
        coop,
        actor,
        aid_repo::CreateFund {
            name: name.to_string(),
            description: None,
            starts_on: None,
            ends_on: None,
            idempotency_key: key,
            fingerprint: fp,
        },
    )
    .await
    .expect("fund")
    .fund_id
}

async fn make_body(pool: &PgPool, coop: Option<Uuid>, actor: Uuid, name: &str) -> Uuid {
    let (key, fp) = key("body");
    gov_repo::create_body(
        pool,
        coop,
        actor,
        gov_repo::CreateBody {
            name: name.to_string(),
            body_type: "board".to_string(),
            description: None,
            idempotency_key: key,
            fingerprint: fp,
        },
    )
    .await
    .expect("body")
    .body_id
}

async fn make_investment(pool: &PgPool, coop: Option<Uuid>, actor: Uuid, name: &str) -> Uuid {
    let (key, fp) = key("inv");
    inv_repo::create_investment(
        pool,
        coop,
        actor,
        inv_repo::CreateInvestment {
            name: name.to_string(),
            investment_type: InvestmentType::RealEstate,
            description: None,
            location: None,
            reference: None,
            counterparty_name: None,
            acquired_at: None,
            idempotency_key: key,
            fingerprint: fp,
        },
    )
    .await
    .expect("investment")
    .investment_id
}

async fn count_where(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(sql)
        .fetch_one(pool)
        .await
        .expect("count")
}

// ------------------------------------------------------------------
// Social aid — funds, donations, disbursements.
// ------------------------------------------------------------------

#[tokio::test]
async fn social_aid_fund_reads_and_writes_are_cooperative_scoped() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;
    let coop_a = Some(fx.coop_a);
    let coop_b = Some(fx.coop_b);

    let fund_a = make_fund(&db.pool, coop_a, fx.user, "Fon A").await;
    let fund_b = make_fund(&db.pool, coop_b, fx.user, "Fon B").await;

    // New rows carry the owning cooperative.
    let bound_a: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM social_aid_funds WHERE id = $1")
            .bind(fund_a)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(bound_a, coop_a);
    let bound_b: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM social_aid_funds WHERE id = $1")
            .bind(fund_b)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(bound_b, coop_b);

    // List isolation.
    let filter = aid_repo::FundFilter {
        status: None,
        search: None,
        page: 1,
        page_size: 50,
    };
    let rows_a = aid_repo::list_funds(&db.pool, coop_a, &filter)
        .await
        .unwrap();
    assert_eq!(rows_a.len(), 1);
    assert_eq!(rows_a[0].id, fund_a);
    let rows_b = aid_repo::list_funds(&db.pool, coop_b, &filter)
        .await
        .unwrap();
    assert_eq!(rows_b.len(), 1);
    assert_eq!(rows_b[0].id, fund_b);
    let rows_none = aid_repo::list_funds(&db.pool, None, &filter).await.unwrap();
    assert!(rows_none.is_empty(), "legacy lane sees no scoped rows");

    // Detail isolation: foreign fund resolves to absent, not forbidden.
    assert!(aid_repo::get_fund_detail(&db.pool, coop_b, fund_a)
        .await
        .unwrap()
        .is_none());
    assert!(aid_repo::get_fund_detail(&db.pool, coop_a, fund_a)
        .await
        .unwrap()
        .is_some());

    // Close/cancel of a foreign fund is a not-found with zero writes.
    let err = aid_repo::close_fund(&db.pool, coop_b, fx.user, fund_a, OffsetDateTime::now_utc())
        .await
        .expect_err("foreign close must fail");
    assert!(matches!(err, aid_repo::SocialAidError::NotFound));
    let err = aid_repo::cancel_fund(
        &db.pool,
        coop_b,
        fx.user,
        fund_a,
        "x".into(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect_err("foreign cancel must fail");
    assert!(matches!(err, aid_repo::SocialAidError::NotFound));
    let status: String = sqlx::query_scalar("SELECT status FROM social_aid_funds WHERE id = $1")
        .bind(fund_a)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(status, "active", "failed foreign ops leave no writes");

    db.cleanup().await;
}

#[tokio::test]
async fn donation_and_disbursement_are_cooperative_scoped() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;
    let coop_a = Some(fx.coop_a);
    let coop_b = Some(fx.coop_b);

    let fund_a = make_fund(&db.pool, coop_a, fx.user, "Fon A").await;
    let fund_b = make_fund(&db.pool, coop_b, fx.user, "Fon B").await;
    let account_a = create_account_at(&db.pool, coop_a, fx.user, "A-Kasa").await;
    let account_b = create_account_at(&db.pool, coop_b, fx.user, "B-Kasa").await;
    let person_a = create_person_at(&db.pool, coop_a, "Bagisci", "A").await;
    fund_account_at(&db.pool, coop_a, fx.user, account_a, 100_000).await;

    // A donation on coop A posts and binds the cooperative on both the
    // donation row and its account movement.
    let (k, f) = key("dn");
    let donation = aid_repo::post_donation(
        &db.pool,
        coop_a,
        fx.user,
        aid_repo::PostDonation {
            fund_id: fund_a,
            donor_person_id: Some(person_a),
            donor_display_name: None,
            financial_account_id: account_a,
            amount: Decimal::new(40_000, 2),
            occurred_at: OffsetDateTime::now_utc(),
            reference: None,
            note: None,
            idempotency_key: k,
            fingerprint: f,
        },
    )
    .await
    .expect("donation");
    let bound: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM social_aid_donations WHERE id = $1")
            .bind(donation.event_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(bound, coop_a);
    let movement_coop: Option<Uuid> = sqlx::query_scalar(
        "SELECT m.cooperative_id FROM account_movements m \
         JOIN social_aid_donations d ON d.account_movement_id = m.id \
         WHERE d.id = $1",
    )
    .bind(donation.event_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(movement_coop, coop_a);

    // Donation to a foreign fund is not-found with zero writes.
    let (k, f) = key("dn-foreign");
    let before = count_where(&db.pool, "SELECT count(*) FROM social_aid_donations").await;
    let moves_before = count_where(&db.pool, "SELECT count(*) FROM account_movements").await;
    let err = aid_repo::post_donation(
        &db.pool,
        coop_a,
        fx.user,
        aid_repo::PostDonation {
            fund_id: fund_b,
            donor_person_id: None,
            donor_display_name: Some("Yabanci".into()),
            financial_account_id: account_a,
            amount: Decimal::new(1_000, 2),
            occurred_at: OffsetDateTime::now_utc(),
            reference: None,
            note: None,
            idempotency_key: k,
            fingerprint: f,
        },
    )
    .await
    .err()
    .expect("donation to foreign fund must fail");
    assert!(matches!(err, aid_repo::SocialAidError::NotFound));
    assert_eq!(
        count_where(&db.pool, "SELECT count(*) FROM social_aid_donations").await,
        before,
        "foreign donation leaves zero partial writes"
    );
    assert_eq!(
        count_where(&db.pool, "SELECT count(*) FROM account_movements").await,
        moves_before,
        "foreign donation leaves no movement"
    );

    // Donation with a foreign account is likewise rejected.
    let (k, f) = key("dn-foreign-acct");
    let err = aid_repo::post_donation(
        &db.pool,
        coop_a,
        fx.user,
        aid_repo::PostDonation {
            fund_id: fund_a,
            donor_person_id: None,
            donor_display_name: Some("Yabanci".into()),
            financial_account_id: account_b,
            amount: Decimal::new(1_000, 2),
            occurred_at: OffsetDateTime::now_utc(),
            reference: None,
            note: None,
            idempotency_key: k,
            fingerprint: f,
        },
    )
    .await
    .err()
    .expect("donation on foreign account must fail");
    assert!(matches!(err, aid_repo::SocialAidError::NotFound));

    // Donation list/detail isolation.
    let (rows, _) = aid_repo::list_donations(
        &db.pool,
        coop_a,
        &aid_repo::DonationFilter {
            fund_id: None,
            page: 1,
            page_size: 50,
        },
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    let (rows_b, _) = aid_repo::list_donations(
        &db.pool,
        coop_b,
        &aid_repo::DonationFilter {
            fund_id: None,
            page: 1,
            page_size: 50,
        },
    )
    .await
    .unwrap();
    assert!(rows_b.is_empty());
    assert!(aid_repo::get_donation(&db.pool, coop_b, donation.event_id)
        .await
        .unwrap()
        .is_none());

    // A disbursement posts, reverses and stays inside the cooperative.
    let (k, f) = key("db");
    let disb = aid_repo::post_disbursement(
        &db.pool,
        coop_a,
        fx.user,
        aid_repo::PostDisbursement {
            fund_id: fund_a,
            beneficiary_person_id: Some(person_a),
            beneficiary_display_name: None,
            financial_account_id: account_a,
            amount: Decimal::new(10_000, 2),
            occurred_at: OffsetDateTime::now_utc(),
            reason: "destek".into(),
            reference: None,
            idempotency_key: k,
            fingerprint: f,
        },
    )
    .await
    .expect("disbursement");
    let (replayed, _) = aid_repo::reverse_disbursement(
        &db.pool,
        coop_a,
        fx.user,
        disb.event_id,
        "duzeltme".into(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("reverse disbursement");
    assert!(!replayed);

    // Reversing a foreign donation is not-found.
    let err = aid_repo::reverse_donation(
        &db.pool,
        coop_b,
        fx.user,
        donation.event_id,
        "x".into(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect_err("foreign donation reversal must fail");
    assert!(matches!(err, aid_repo::SocialAidError::NotFound));
    let st: String = sqlx::query_scalar("SELECT status FROM social_aid_donations WHERE id = $1")
        .bind(donation.event_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(st, "posted");

    db.cleanup().await;
}

// ------------------------------------------------------------------
// Governance — bodies, memberships, decisions, votes.
// ------------------------------------------------------------------

#[tokio::test]
async fn governance_lifecycle_is_cooperative_scoped() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;
    let coop_a = Some(fx.coop_a);
    let coop_b = Some(fx.coop_b);

    let body_a = make_body(&db.pool, coop_a, fx.user, "Kurul A").await;
    let body_b = make_body(&db.pool, coop_b, fx.user, "Kurul B").await;
    let bound: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM governance_bodies WHERE id = $1")
            .bind(body_a)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(bound, coop_a);

    // List/detail isolation.
    let rows_a = gov_repo::list_bodies(
        &db.pool,
        coop_a,
        &gov_repo::BodyFilter {
            status: None,
            search: None,
            page: 1,
            page_size: 50,
        },
    )
    .await
    .unwrap();
    assert_eq!(rows_a.len(), 1);
    assert_eq!(rows_a[0].id, body_a);
    assert!(gov_repo::get_body_detail(&db.pool, coop_b, body_a)
        .await
        .unwrap()
        .is_none());

    // Membership binds the cooperative and requires a same-coop person.
    let person_a = create_person_at(&db.pool, coop_a, "Uye", "A").await;
    let person_b = create_person_at(&db.pool, coop_b, "Uye", "B").await;
    let (k, f) = key("mem-foreign");
    let err = gov_repo::create_membership(
        &db.pool,
        coop_a,
        fx.user,
        gov_repo::CreateMembership {
            body_id: body_a,
            person_id: person_b,
            title: None,
            started_at: OffsetDateTime::now_utc() - time::Duration::days(1),
            idempotency_key: k,
            fingerprint: f,
        },
    )
    .await
    .err()
    .expect("foreign person membership must fail");
    assert!(matches!(err, gov_repo::GovernanceRepoError::NotFound));
    let (k, f) = key("mem");
    let membership = gov_repo::create_membership(
        &db.pool,
        coop_a,
        fx.user,
        gov_repo::CreateMembership {
            body_id: body_a,
            person_id: person_a,
            title: None,
            started_at: OffsetDateTime::now_utc() - time::Duration::days(1),
            idempotency_key: k,
            fingerprint: f,
        },
    )
    .await
    .expect("membership");
    let bound: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM governance_memberships WHERE id = $1")
            .bind(membership.membership_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(bound, coop_a);

    // Decision on a foreign body is not-found with zero writes.
    let (k, f) = key("dec-foreign");
    let err = gov_repo::create_decision(
        &db.pool,
        coop_a,
        fx.user,
        gov_repo::CreateDecision {
            body_id: body_b,
            title: "Karar".into(),
            decision_text: "metin".into(),
            decision_on: date(2026, 1, 10),
            effective_on: None,
            idempotency_key: k,
            fingerprint: f,
        },
    )
    .await
    .err()
    .expect("decision on foreign body must fail");
    assert!(matches!(err, gov_repo::GovernanceRepoError::NotFound));
    assert_eq!(
        count_where(&db.pool, "SELECT count(*) FROM governance_decisions").await,
        0,
        "foreign decision leaves zero writes"
    );

    // Full decision lifecycle in coop A: create -> open -> vote ->
    // finalize; every row carries the cooperative.
    let (k, f) = key("dec");
    let decision = gov_repo::create_decision(
        &db.pool,
        coop_a,
        fx.user,
        gov_repo::CreateDecision {
            body_id: body_a,
            title: "Karar A".into(),
            decision_text: "metin".into(),
            decision_on: date(2026, 1, 10),
            effective_on: None,
            idempotency_key: k,
            fingerprint: f,
        },
    )
    .await
    .expect("decision");
    let bound: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM governance_decisions WHERE id = $1")
            .bind(decision.decision_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(bound, coop_a);
    gov_repo::open_decision(&db.pool, coop_a, fx.user, decision.decision_id)
        .await
        .expect("open");
    let (k, f) = key("vote");
    let vote = gov_repo::cast_vote(
        &db.pool,
        coop_a,
        fx.user,
        gov_repo::CastVote {
            decision_id: decision.decision_id,
            person_id: person_a,
            choice: VoteChoice::Approve,
            note: None,
            idempotency_key: k,
            fingerprint: f,
        },
    )
    .await
    .expect("vote");
    let bound: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM governance_votes WHERE id = $1")
            .bind(vote.vote_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(bound, coop_a);
    gov_repo::finalize_decision(
        &db.pool,
        coop_a,
        fx.user,
        gov_repo::FinalizeDecision {
            decision_id: decision.decision_id,
            outcome: DecisionOutcome::Approved,
        },
    )
    .await
    .expect("finalize");

    // Foreign-coop commands against the decision are not-found.
    let err = gov_repo::open_decision(&db.pool, coop_b, fx.user, decision.decision_id)
        .await
        .expect_err("foreign open must fail");
    assert!(matches!(err, gov_repo::GovernanceRepoError::NotFound));
    let (k, f) = key("vote-foreign");
    let err = gov_repo::cast_vote(
        &db.pool,
        coop_b,
        fx.user,
        gov_repo::CastVote {
            decision_id: decision.decision_id,
            person_id: person_b,
            choice: VoteChoice::Reject,
            note: None,
            idempotency_key: k,
            fingerprint: f,
        },
    )
    .await
    .err()
    .expect("foreign vote must fail");
    assert!(matches!(err, gov_repo::GovernanceRepoError::NotFound));
    assert_eq!(
        count_where(&db.pool, "SELECT count(*) FROM governance_votes").await,
        1,
        "foreign vote leaves zero writes"
    );

    // Decision list/detail isolation.
    let rows_a = gov_repo::list_decisions(
        &db.pool,
        coop_a,
        &gov_repo::DecisionFilter {
            body_id: None,
            status: None,
            search: None,
            page: 1,
            page_size: 50,
        },
    )
    .await
    .unwrap();
    assert_eq!(rows_a.len(), 1);
    assert!(
        gov_repo::get_decision_detail(&db.pool, coop_b, decision.decision_id)
            .await
            .unwrap()
            .is_none()
    );

    db.cleanup().await;
}

// ------------------------------------------------------------------
// Investments.
// ------------------------------------------------------------------

#[tokio::test]
async fn investment_flow_is_cooperative_scoped() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;
    let coop_a = Some(fx.coop_a);
    let coop_b = Some(fx.coop_b);

    let inv_a = make_investment(&db.pool, coop_a, fx.user, "Arsa A").await;
    let inv_b = make_investment(&db.pool, coop_b, fx.user, "Arsa B").await;
    let bound: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM investments WHERE id = $1")
            .bind(inv_a)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(bound, coop_a);

    let rows_a = inv_repo::list_investments(
        &db.pool,
        coop_a,
        &inv_repo::InvestmentFilter {
            status: None,
            investment_type: None,
            search: None,
            page: 1,
            page_size: 50,
        },
    )
    .await
    .unwrap();
    assert_eq!(rows_a.len(), 1);
    assert_eq!(rows_a[0].id, inv_a);
    assert!(inv_repo::get_investment_detail(&db.pool, coop_b, inv_a)
        .await
        .unwrap()
        .is_none());

    // Funding a foreign investment is not-found with zero writes.
    let account_a = create_account_at(&db.pool, coop_a, fx.user, "A-Kasa").await;
    fund_account_at(&db.pool, coop_a, fx.user, account_a, 100_000).await;
    let (k, f) = key("fnd-foreign");
    let before = count_where(&db.pool, "SELECT count(*) FROM investment_fundings").await;
    let err = inv_repo::post_funding(
        &db.pool,
        coop_a,
        fx.user,
        inv_repo::PostFunding {
            investment_id: inv_b,
            financial_account_id: account_a,
            amount: Decimal::new(50_000, 2),
            occurred_at: OffsetDateTime::now_utc(),
            reference: None,
            note: None,
            idempotency_key: k,
            fingerprint: f,
        },
    )
    .await
    .err()
    .expect("funding foreign investment must fail");
    assert!(matches!(err, inv_repo::InvestmentCommandError::NotFound));
    assert_eq!(
        count_where(&db.pool, "SELECT count(*) FROM investment_fundings").await,
        before,
        "foreign funding leaves zero partial writes"
    );

    // Funding in coop A posts and binds the cooperative on the funding
    // row and its movement.
    let (k, f) = key("fnd");
    let funding = inv_repo::post_funding(
        &db.pool,
        coop_a,
        fx.user,
        inv_repo::PostFunding {
            investment_id: inv_a,
            financial_account_id: account_a,
            amount: Decimal::new(50_000, 2),
            occurred_at: OffsetDateTime::now_utc(),
            reference: None,
            note: None,
            idempotency_key: k,
            fingerprint: f,
        },
    )
    .await
    .expect("funding");
    let bound: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM investment_fundings WHERE id = $1")
            .bind(funding.event_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(bound, coop_a);

    db.cleanup().await;
}

// ------------------------------------------------------------------
// Reports — aggregate projections never cross the cooperative line.
// ------------------------------------------------------------------

#[tokio::test]
async fn reports_never_leak_foreign_cooperative_totals() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;
    let coop_a = Some(fx.coop_a);
    let coop_b = Some(fx.coop_b);

    // Build data in BOTH cooperatives: an account + a fund + donation
    // each, an investment each, a body each.
    let account_a = create_account_at(&db.pool, coop_a, fx.user, "A-Kasa").await;
    let account_b = create_account_at(&db.pool, coop_b, fx.user, "B-Kasa").await;
    let fund_a = make_fund(&db.pool, coop_a, fx.user, "Fon A").await;
    let fund_b = make_fund(&db.pool, coop_b, fx.user, "Fon B").await;
    make_investment(&db.pool, coop_a, fx.user, "Arsa A").await;
    make_investment(&db.pool, coop_b, fx.user, "Arsa B").await;
    make_body(&db.pool, coop_a, fx.user, "Kurul A").await;
    make_body(&db.pool, coop_b, fx.user, "Kurul B").await;

    for (fund, account, coop) in [(fund_a, account_a, coop_a), (fund_b, account_b, coop_b)] {
        let (k, f) = key("dn-r");
        aid_repo::post_donation(
            &db.pool,
            coop,
            fx.user,
            aid_repo::PostDonation {
                fund_id: fund,
                donor_person_id: None,
                donor_display_name: Some("Bagisci".into()),
                financial_account_id: account,
                amount: Decimal::new(10_000, 2),
                occurred_at: OffsetDateTime::now_utc(),
                reference: None,
                note: None,
                idempotency_key: k,
                fingerprint: f,
            },
        )
        .await
        .expect("donation");
    }

    // Overview: each cooperative sees exactly its own totals.
    let ov_a = report_repo::overview(&db.pool, coop_a).await.unwrap();
    let ov_b = report_repo::overview(&db.pool, coop_b).await.unwrap();
    let ov_none = report_repo::overview(&db.pool, None).await.unwrap();
    assert_eq!(ov_a.social_aid_donations_total, Decimal::new(10_000, 2));
    assert_eq!(ov_b.social_aid_donations_total, Decimal::new(10_000, 2));
    assert_eq!(
        ov_none.social_aid_donations_total,
        Decimal::ZERO,
        "legacy lane never aggregates scoped rows"
    );
    assert_eq!(ov_a.investment_active_count, 1);
    assert_eq!(ov_a.active_body_count, 1);
    assert_eq!(ov_none.investment_active_count, 0);
    assert_eq!(ov_none.active_body_count, 0);

    // Scoped fund report shows only own funds.
    let rows_a = report_repo::social_aid_report(&db.pool, coop_a, 1, 50)
        .await
        .unwrap();
    assert_eq!(rows_a.len(), 1);
    assert_eq!(rows_a[0].fund_id, fund_a);
    let rows_none = report_repo::social_aid_report(&db.pool, None, 1, 50)
        .await
        .unwrap();
    assert!(rows_none.is_empty());

    // Investments report isolation.
    let rows_a = report_repo::investments_report(&db.pool, coop_a, 1, 50)
        .await
        .unwrap();
    assert_eq!(rows_a.len(), 1);

    // Accounts report isolation — each coop sees its own account only.
    let rows_a = report_repo::accounts_report(&db.pool, coop_a, None, 1, 50)
        .await
        .unwrap();
    assert_eq!(rows_a.len(), 1);
    assert_eq!(rows_a[0].id, account_a);

    // Movements summary never counts foreign movements.
    let summary_a = report_repo::movements_summary(
        &db.pool,
        coop_a,
        &report_repo::MovementFilter {
            account_id: None,
            source_type: None,
            direction: None,
            status: None,
            date_from: None,
            date_to: None,
        },
    )
    .await
    .unwrap();
    let summary_b = report_repo::movements_summary(
        &db.pool,
        coop_b,
        &report_repo::MovementFilter {
            account_id: None,
            source_type: None,
            direction: None,
            status: None,
            date_from: None,
            date_to: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(summary_a.movement_count, 1);
    assert_eq!(summary_b.movement_count, 1);

    db.cleanup().await;
}

// ------------------------------------------------------------------
// Legacy NULL lane — the un-scoped cooperative context sees only NULL
// rows; scoped rows are invisible to it.
// ------------------------------------------------------------------

#[tokio::test]
async fn legacy_null_lane_only_sees_null_rows() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;
    let coop_a = Some(fx.coop_a);

    // NULL-cooperative rows can no longer be written (0025): the
    // legacy lane is closed by NOT NULL on every business table.
    let legacy_fund = Uuid::new_v4();
    let rejected = sqlx::query(
        "INSERT INTO social_aid_funds (id, name, status, idempotency_key, \
            idempotency_fingerprint, created_by, cooperative_id) \
         VALUES ($1, 'Eski Fon', 'active', $2, 'fp', $3, NULL)",
    )
    .bind(legacy_fund)
    .bind(format!("legacy-{}", Uuid::new_v4().simple()))
    .bind(fx.user)
    .execute(&db.pool)
    .await;
    assert!(
        rejected.is_err(),
        "NULL cooperative_id fund must be rejected"
    );
    let scoped_fund = make_fund(&db.pool, coop_a, fx.user, "Yeni Fon").await;

    let filter = aid_repo::FundFilter {
        status: None,
        search: None,
        page: 1,
        page_size: 50,
    };
    let none_rows = aid_repo::list_funds(&db.pool, None, &filter).await.unwrap();
    assert!(none_rows.is_empty());
    let a_rows = aid_repo::list_funds(&db.pool, coop_a, &filter)
        .await
        .unwrap();
    assert_eq!(a_rows.len(), 1);
    assert_eq!(a_rows[0].id, scoped_fund);

    // The NULL lane cannot reach scoped resources.
    assert!(aid_repo::get_fund_detail(&db.pool, None, scoped_fund)
        .await
        .unwrap()
        .is_none());

    db.cleanup().await;
}
