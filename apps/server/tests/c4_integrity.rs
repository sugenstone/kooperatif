//! M1-P2-C4 — data-integrity validation and C5 readiness audit.
//!
//! Boots a disposable database, runs the real bootstrap path (which claims
//! the ten global `financial_categories` seeds and fails closed on any
//! ambiguous NULL ownership), then populates TWO cooperatives through the
//! production repository layer — payments, credits, transfers, investments,
//! social aid, governance, share returns, income/expense — and executes the
//! C4 verification battery:
//!
//!   C4-01  schema inventory (37 business tables carry cooperative_id)
//!   C4-02  NULL cooperative_id scan per business table
//!   C4-03  orphan cooperative references per business table
//!   C4-04  composite *_coop_fk presence + validation (66, all validated)
//!   C4-05  cross-cooperative relationship probes (dynamically generated
//!          from every composite FK: cross-coop + dangling)
//!   C4-06  polymorphic account_movements source ownership
//!   C4-07  duplicate identifier scan (numbers + idempotency keys)
//!   C4-08  C5 inventory: global UNIQUE indexes without cooperative_id
//!   C4-09  reverse binding: entry->movement cooperative consistency
//!   C4-10  bootstrap seed-category ownership
//!   C4-11  population coverage: every business table non-empty
//!
//! Output is machine-readable: `C4|<id>|<subject>|<PASS|FAIL|INFO>|<detail>`
//! lines plus a final `C4-REPORT|<json>` line; the JSON is also written to
//! implementation/M1-P2-C4-RESULT.json.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same convention as the other integration suites).

use kooperatif_server::credits::repo as credit_repo;
use kooperatif_server::db as app_db;
use kooperatif_server::financial_accounts::model::AccountType;
use kooperatif_server::financial_accounts::repo as account_repo;
use kooperatif_server::governance::model::{DecisionOutcome, VoteChoice};
use kooperatif_server::governance::repo as gov_repo;
use kooperatif_server::income_expense::model::{CategoryType, EntryKind};
use kooperatif_server::income_expense::repo as entry_repo;
use kooperatif_server::investments::model::InvestmentType;
use kooperatif_server::investments::repo as inv_repo;
use kooperatif_server::parties::repo as party_repo;
use kooperatif_server::payments::repo as payment_repo;
use kooperatif_server::periods::model::AssessmentRuleType;
use kooperatif_server::periods::repo as period_repo;
use kooperatif_server::share_returns::model::EntitlementType;
use kooperatif_server::share_returns::repo as return_repo;
use kooperatif_server::shares::model::AcquisitionType;
use kooperatif_server::shares::repo as share_repo;
use kooperatif_server::social_aid::repo as aid_repo;
use rust_decimal::Decimal;
use sqlx::PgPool;
use time::{Date, Month, OffsetDateTime};
use uuid::Uuid;

const BUSINESS_TABLES: [&str; 37] = [
    "persons",
    "families",
    "shareholders",
    "shareholder_family_memberships",
    "shares",
    "share_events",
    "share_ownerships",
    "periods",
    "assessment_rules",
    "assessments",
    "assessment_share_sources",
    "payments",
    "payment_allocations",
    "financial_accounts",
    "account_movements",
    "account_transfers",
    "shareholder_credits",
    "credit_applications",
    "financial_categories",
    "income_entries",
    "expense_entries",
    "share_returns",
    "share_return_entitlements",
    "share_return_settlements",
    "investments",
    "investment_fundings",
    "investment_valuations",
    "investment_incomes",
    "investment_disposals",
    "investment_disposal_proceeds",
    "social_aid_funds",
    "social_aid_donations",
    "social_aid_disbursements",
    "governance_bodies",
    "governance_memberships",
    "governance_decisions",
    "governance_votes",
];

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

/// Real bootstrap path for coop A (claims the seed categories + exercises
/// the P0 fail-closed guard); coop B is created directly as a second,
/// non-bootstrap cooperative.
async fn fixture(pool: &PgPool) -> Fixture {
    let user = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash) VALUES ($1,$2,$3,$4)",
    )
    .bind(user)
    .bind(format!("c4{}", &user.simple().to_string()[..8]))
    .bind("C4 Kullanici")
    .bind("not-a-real-hash")
    .execute(pool)
    .await
    .expect("fixture user");
    // Membership rows are written by bootstrap for existing users; the
    // fixture user is created first so the count includes them.
    let (coop_a, _memberships) =
        kooperatif_server::tenant::repo::bootstrap_initial_cooperative(pool, "C4 Kooperatif A")
            .await
            .expect("bootstrap cooperative A");
    let coop_b = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO cooperatives (id, name, status, is_bootstrap, business_enabled)
         VALUES ($1,'C4 Kooperatif B','active',false,true)",
    )
    .bind(coop_b)
    .execute(pool)
    .await
    .expect("fixture cooperative B");
    Fixture {
        coop_a,
        coop_b,
        user,
    }
}

fn date(y: i32, m: u8, d: u8) -> Date {
    Date::from_calendar_date(y, Month::try_from(m).unwrap(), d).unwrap()
}

fn key(tag: &str, n: &str) -> (String, String) {
    (format!("c4-{tag}-{n}"), format!("fp-c4-{tag}-{n}"))
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

async fn create_period_at(
    pool: &PgPool,
    coop: Option<Uuid>,
    actor: Uuid,
    name: &str,
    rule: AssessmentRuleType,
    base: i64,
    effective: Date,
) -> Uuid {
    period_repo::create_period(
        pool,
        coop,
        actor,
        period_repo::CreatePeriod {
            name: name.to_string(),
            collection_start_date: date(2026, 1, 1),
            due_date: date(2026, 3, 31),
            rule_type: rule,
            base_amount: Decimal::from(base),
            assessment_effective_date: effective,
        },
    )
    .await
    .expect("period")
    .0
}

/// Populate every business domain for one cooperative via the production
/// repository layer. `tag` keeps idempotency keys unique per cooperative.
async fn populate(pool: &PgPool, coop: Uuid, user: Uuid, tag: &str) {
    let now = OffsetDateTime::now_utc();
    let co = Some(coop);
    // families.sequence_number is a GLOBAL unique until C5 — derive a
    // distinct sequence per cooperative tag.
    let sequence = if tag == "a" { 10 } else { 20 };

    // parties: person + family + membership + shareholder
    let sh =
        create_shareholder_at(pool, co, user, &format!("{tag} Ortak"), "Soyad", sequence).await;

    // shares: share + event + ownership (effective 3 days ago so a
    // per-share assessment generated "today" sees the ownership)
    let share_started = now - time::Duration::days(3);
    let (share, _evt) = share_repo::create_share(
        pool,
        co,
        user,
        share_repo::CreateShare {
            shareholder_id: sh.shareholder_id,
            acquisition_type: AcquisitionType::Founder,
            fee: None,
            effective_at: share_started,
            reason: None,
        },
        now,
    )
    .await
    .expect("share");

    // accounts
    let acc = create_account_at(pool, co, user, &format!("Kasa {tag}")).await;
    let acc2 = create_account_at(pool, co, user, &format!("Banka {tag}")).await;

    // periods + assessments: per_shareholder and per_share (the latter
    // produces assessment_share_sources provenance rows)
    let p1 = create_period_at(
        pool,
        co,
        user,
        &format!("D-{tag}-1"),
        AssessmentRuleType::PerShareholder,
        1000,
        now.date(),
    )
    .await;
    period_repo::generate_assessments(pool, co, user, p1)
        .await
        .expect("generate p1");
    let p2 = create_period_at(
        pool,
        co,
        user,
        &format!("D-{tag}-2"),
        AssessmentRuleType::PerShare,
        500,
        now.date(),
    )
    .await;
    period_repo::generate_assessments(pool, co, user, p2)
        .await
        .expect("generate p2");
    let assessment_1: Uuid = sqlx::query_scalar("SELECT id FROM assessments WHERE period_id = $1")
        .bind(p1)
        .fetch_one(pool)
        .await
        .unwrap();
    // The manual-apply target: a DRAFT period whose assessment is
    // raw-inserted AFTER the assign below — generation never runs and
    // the assign-time sweep cannot see it (batch-3 fixture pattern).

    // payment 2000 -> 600 allocated; remainder funds the credit below
    let payment = payment_repo::create_payment(
        pool,
        co,
        user,
        payment_repo::CreatePayment {
            payer_person_id: None,
            payer_first_name: Some(format!("{tag} Odeyen")),
            payer_last_name: Some("Test".to_string()),
            amount: Decimal::from(2000),
            method: "cash".to_string(),
            received_at: now,
            note: None,
            destination_account_id: acc,
            idempotency_key: key(tag, "pay").0,
            fingerprint: key(tag, "pay").1,
            allocations: vec![payment_repo::NewAllocation {
                assessment_id: assessment_1,
                amount: Decimal::from(600),
            }],
        },
    )
    .await
    .expect("payment");

    // credit assigned from the payment remainder; auto-apply consumes the
    // open assessments FIFO (400 on assessment_1 + 500 on assessment_2),
    // leaving exactly 100 for the manual exact application below
    let credit = credit_repo::assign_credit(
        pool,
        co,
        user,
        credit_repo::AssignCredit {
            payment_id: payment.payment_id,
            shareholder_id: sh.shareholder_id,
            amount: Decimal::from(1000),
            note: None,
            idempotency_key: key(tag, "cr").0,
            fingerprint: key(tag, "cr").1,
        },
        now,
    )
    .await
    .expect("assign credit");

    // Manual-apply target: a DRAFT period whose assessment is
    // raw-inserted AFTER the assign — generation never runs and the
    // assign-time sweep cannot see it (batch-3 fixture pattern). The
    // sweep consumed 400 (assessment_1) + 500 (assessment_2), leaving
    // exactly 100 of the 1000 credit.
    let p3 = create_period_at(
        pool,
        co,
        user,
        &format!("D-{tag}-3"),
        AssessmentRuleType::PerShareholder,
        800,
        now.date(),
    )
    .await;
    let assessment_3 = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO assessments (id, period_id, shareholder_id, rule_type, base_amount, \
         amount, currency, status, assessment_effective_date, generated_by, cooperative_id) \
         VALUES ($1,$2,$3,'per_shareholder',800,800,'TRY','active',$4,$5,$6)",
    )
    .bind(assessment_3)
    .bind(p3)
    .bind(sh.shareholder_id)
    .bind(now.date())
    .bind(user)
    .bind(coop)
    .execute(pool)
    .await
    .expect("manual assessment");

    // manual exact application against the still-open p3 assessment
    credit_repo::apply_credit(
        pool,
        co,
        user,
        credit_repo::ApplyCredit {
            assessment_id: assessment_3,
            amount: Decimal::from(100),
            idempotency_key: key(tag, "apply").0,
            fingerprint: key(tag, "apply").1,
        },
    )
    .await
    .expect("manual apply");
    let _ = credit.credit_id;

    // transfer acc -> acc2
    account_repo::post_transfer(
        pool,
        co,
        user,
        account_repo::PostTransfer {
            source_account_id: acc,
            destination_account_id: acc2,
            amount: Decimal::from(200),
            occurred_at: now,
            note: None,
            idempotency_key: key(tag, "tr").0,
            fingerprint: key(tag, "tr").1,
        },
    )
    .await
    .expect("transfer");

    // categories + income + expense entries (income first: funds the
    // later settlement outflow on acc)
    let cat_in = entry_repo::create_category(
        pool,
        co,
        user,
        entry_repo::CreateCategory {
            category_type: CategoryType::Income,
            name: format!("Kira {tag}"),
            description: None,
        },
    )
    .await
    .expect("income category");
    let cat_out = entry_repo::create_category(
        pool,
        co,
        user,
        entry_repo::CreateCategory {
            category_type: CategoryType::Expense,
            name: format!("Elektrik {tag}"),
            description: None,
        },
    )
    .await
    .expect("expense category");
    entry_repo::post_entry(
        pool,
        co,
        user,
        EntryKind::Income,
        entry_repo::PostEntry {
            financial_account_id: acc,
            category_id: cat_in,
            amount: Decimal::from(3000),
            occurred_at: now,
            description: "kira".to_string(),
            counterparty: None,
            reference_no: None,
            idempotency_key: key(tag, "inc").0,
            fingerprint: key(tag, "inc").1,
        },
    )
    .await
    .expect("income entry");
    entry_repo::post_entry(
        pool,
        co,
        user,
        EntryKind::Expense,
        entry_repo::PostEntry {
            financial_account_id: acc,
            category_id: cat_out,
            amount: Decimal::from(150),
            occurred_at: now,
            description: "elektrik".to_string(),
            counterparty: None,
            reference_no: None,
            idempotency_key: key(tag, "exp").0,
            fingerprint: key(tag, "exp").1,
        },
    )
    .await
    .expect("expense entry");

    // share return: initiate -> finalize -> settle
    let initiated = return_repo::initiate_return(
        pool,
        co,
        user,
        return_repo::InitiateReturn {
            share_id: share,
            effective_return_date: now.date(),
            reason: Some("cikis".to_string()),
            idempotency_key: key(tag, "sr").0,
            fingerprint: key(tag, "sr").1,
        },
        now,
    )
    .await
    .expect("initiate return");
    let detail = return_repo::get_return(pool, co, initiated.return_id)
        .await
        .expect("return detail")
        .expect("return row");
    let finalized = return_repo::finalize_return(
        pool,
        co,
        user,
        initiated.return_id,
        detail.updated_at,
        &[return_repo::EntitlementSpec {
            entitlement_type: EntitlementType::Principal,
            amount: Some(Decimal::from(1200)),
            due_date: Some(date(2026, 12, 31)),
            policy_reference: None,
            description: None,
        }],
        now,
    )
    .await
    .expect("finalize return");
    return_repo::post_settlement(
        pool,
        co,
        user,
        return_repo::PostSettlement {
            entitlement_id: finalized.entitlement_ids[0],
            financial_account_id: acc,
            amount: Decimal::from(1200),
            settled_at: now,
            idempotency_key: key(tag, "set").0,
            fingerprint: key(tag, "set").1,
        },
    )
    .await
    .expect("settlement");

    // investments: create -> funding -> valuation -> income -> disposal
    let inv = inv_repo::create_investment(
        pool,
        co,
        user,
        inv_repo::CreateInvestment {
            name: format!("Arsa {tag}"),
            investment_type: InvestmentType::RealEstate,
            description: None,
            location: None,
            reference: None,
            counterparty_name: None,
            acquired_at: None,
            idempotency_key: key(tag, "inv").0,
            fingerprint: key(tag, "inv").1,
        },
    )
    .await
    .expect("investment")
    .investment_id;
    inv_repo::post_funding(
        pool,
        co,
        user,
        inv_repo::PostFunding {
            investment_id: inv,
            financial_account_id: acc,
            amount: Decimal::from(500),
            occurred_at: now,
            reference: None,
            note: None,
            idempotency_key: key(tag, "fund").0,
            fingerprint: key(tag, "fund").1,
        },
    )
    .await
    .expect("funding");
    inv_repo::record_valuation(
        pool,
        co,
        user,
        inv_repo::RecordValuation {
            investment_id: inv,
            valuation_date: now.date(),
            amount: Decimal::from(900),
            method: None,
            source: None,
            note: None,
            idempotency_key: key(tag, "val").0,
            fingerprint: key(tag, "val").1,
        },
    )
    .await
    .expect("valuation");
    inv_repo::post_income(
        pool,
        co,
        user,
        inv_repo::PostIncome {
            investment_id: inv,
            financial_account_id: acc,
            amount: Decimal::from(100),
            occurred_at: now,
            description: "kira geliri".to_string(),
            counterparty: None,
            reference_no: None,
            idempotency_key: key(tag, "ivi").0,
            fingerprint: key(tag, "ivi").1,
        },
    )
    .await
    .expect("investment income");
    inv_repo::dispose_investment(
        pool,
        co,
        user,
        inv_repo::DisposeInvestment {
            investment_id: inv,
            disposed_at: now.date(),
            consideration_amount: Some(Decimal::from(1200)),
            counterparty_name: None,
            reference: None,
            note: None,
            proceeds: vec![inv_repo::ProceedsLeg {
                financial_account_id: acc,
                amount: Decimal::from(1200),
                occurred_at: now,
                reference: None,
            }],
            idempotency_key: key(tag, "disp").0,
            fingerprint: key(tag, "disp").1,
        },
        now,
    )
    .await
    .expect("disposal");

    // social aid: fund -> donation -> disbursement (restricted money
    // stays on acc2, separate from the operational flows on acc)
    let fund = aid_repo::create_fund(
        pool,
        co,
        user,
        aid_repo::CreateFund {
            name: format!("Fon {tag}"),
            description: None,
            starts_on: None,
            ends_on: None,
            idempotency_key: key(tag, "fon").0,
            fingerprint: key(tag, "fon").1,
        },
    )
    .await
    .expect("fund")
    .fund_id;
    aid_repo::post_donation(
        pool,
        co,
        user,
        aid_repo::PostDonation {
            fund_id: fund,
            donor_person_id: Some(sh.person_id),
            donor_display_name: None,
            financial_account_id: acc2,
            amount: Decimal::from(400),
            occurred_at: now,
            reference: None,
            note: None,
            idempotency_key: key(tag, "don").0,
            fingerprint: key(tag, "don").1,
        },
    )
    .await
    .expect("donation");
    aid_repo::post_disbursement(
        pool,
        co,
        user,
        aid_repo::PostDisbursement {
            fund_id: fund,
            beneficiary_person_id: Some(sh.person_id),
            beneficiary_display_name: None,
            financial_account_id: acc2,
            amount: Decimal::from(100),
            occurred_at: now,
            reason: "destek".to_string(),
            reference: None,
            idempotency_key: key(tag, "disb").0,
            fingerprint: key(tag, "disb").1,
        },
    )
    .await
    .expect("disbursement");

    // governance: body -> membership -> decision -> open -> vote -> finalize
    let body = gov_repo::create_body(
        pool,
        co,
        user,
        gov_repo::CreateBody {
            name: format!("YK {tag}"),
            body_type: "board".to_string(),
            description: None,
            idempotency_key: key(tag, "body").0,
            fingerprint: key(tag, "body").1,
        },
    )
    .await
    .expect("body")
    .body_id;
    gov_repo::create_membership(
        pool,
        co,
        user,
        gov_repo::CreateMembership {
            body_id: body,
            person_id: sh.person_id,
            title: Some("baskan".to_string()),
            started_at: now - time::Duration::days(1),
            idempotency_key: key(tag, "mem").0,
            fingerprint: key(tag, "mem").1,
        },
    )
    .await
    .expect("membership");
    let decision = gov_repo::create_decision(
        pool,
        co,
        user,
        gov_repo::CreateDecision {
            body_id: body,
            title: format!("Karar {tag}"),
            decision_text: "metin".to_string(),
            decision_on: now.date(),
            effective_on: None,
            idempotency_key: key(tag, "dec").0,
            fingerprint: key(tag, "dec").1,
        },
    )
    .await
    .expect("decision");
    gov_repo::open_decision(pool, co, user, decision.decision_id)
        .await
        .expect("open decision");
    gov_repo::cast_vote(
        pool,
        co,
        user,
        gov_repo::CastVote {
            decision_id: decision.decision_id,
            person_id: sh.person_id,
            choice: VoteChoice::Approve,
            note: None,
            idempotency_key: key(tag, "vote").0,
            fingerprint: key(tag, "vote").1,
        },
    )
    .await
    .expect("vote");
    gov_repo::finalize_decision(
        pool,
        co,
        user,
        gov_repo::FinalizeDecision {
            decision_id: decision.decision_id,
            outcome: DecisionOutcome::Approved,
        },
    )
    .await
    .expect("finalize decision");
}

// ---------------------------------------------------------------------
// Check battery
// ---------------------------------------------------------------------

struct Check {
    id: &'static str,
    subject: String,
    passed: bool,
    detail: String,
}

impl Check {
    fn pass(id: &'static str, subject: impl Into<String>, detail: impl Into<String>) -> Self {
        Check {
            id,
            subject: subject.into(),
            passed: true,
            detail: detail.into(),
        }
    }
    fn fail(id: &'static str, subject: impl Into<String>, detail: impl Into<String>) -> Self {
        Check {
            id,
            subject: subject.into(),
            passed: false,
            detail: detail.into(),
        }
    }
    fn info(id: &'static str, subject: impl Into<String>, detail: impl Into<String>) -> Self {
        Check {
            id,
            subject: subject.into(),
            passed: true, // informational rows never fail the gate
            detail: format!("INFO:{}", detail.into()),
        }
    }
    fn emit(&self) {
        println!(
            "C4|{}|{}|{}|{}",
            self.id,
            self.subject,
            if self.passed { "PASS" } else { "FAIL" },
            self.detail
        );
    }
}

async fn scalar_i64(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(sql)
        .fetch_one(pool)
        .await
        .expect("scalar check query")
}

/// Run the full C4 battery against `pool`. Every check is read-only.
async fn run_checks(pool: &PgPool) -> Vec<Check> {
    let mut checks = Vec::new();

    // C4-01 schema inventory
    let schema_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_attribute a \
         JOIN pg_class c ON c.oid = a.attrelid AND c.relkind = 'r' \
         WHERE a.attname = 'cooperative_id' AND NOT a.attisdropped \
           AND c.relname = ANY($1)",
    )
    .bind(&BUSINESS_TABLES[..])
    .fetch_one(pool)
    .await
    .expect("schema inventory");
    checks.push(if schema_count == 37 {
        Check::pass("C4-01", "schema_inventory", format!("{schema_count}/37"))
    } else {
        Check::fail("C4-01", "schema_inventory", format!("{schema_count}/37"))
    });

    // C4-02 NULL ownership + C4-03 orphan ownership + C4-11 coverage
    for table in BUSINESS_TABLES {
        let total = scalar_i64(pool, &format!("SELECT count(*) FROM {table}")).await;
        let nulls = scalar_i64(
            pool,
            &format!("SELECT count(*) FROM {table} WHERE cooperative_id IS NULL"),
        )
        .await;
        let orphans = scalar_i64(
            pool,
            &format!(
                "SELECT count(*) FROM {table} x LEFT JOIN cooperatives c \
                 ON c.id = x.cooperative_id \
                 WHERE x.cooperative_id IS NOT NULL AND c.id IS NULL"
            ),
        )
        .await;
        checks.push(if nulls == 0 {
            Check::pass("C4-02", table, format!("total={total} nulls=0"))
        } else {
            Check::fail("C4-02", table, format!("total={total} nulls={nulls}"))
        });
        checks.push(if orphans == 0 {
            Check::pass("C4-03", table, "orphans=0")
        } else {
            Check::fail("C4-03", table, format!("orphans={orphans}"))
        });
        checks.push(if total > 0 {
            Check::pass("C4-11", table, format!("rows={total}"))
        } else {
            Check::fail("C4-11", table, "rows=0 (coverage vacuous)")
        });
    }

    // C4-04 composite cooperative FKs: 66 present, all validated
    let fk_total = scalar_i64(
        pool,
        "SELECT count(*) FROM pg_constraint con \
         WHERE con.contype = 'f' AND con.conname LIKE '%\\_coop\\_fk' ESCAPE '\\'",
    )
    .await;
    let fk_unvalidated = scalar_i64(
        pool,
        "SELECT count(*) FROM pg_constraint con \
         WHERE con.contype = 'f' AND con.conname LIKE '%\\_coop\\_fk' ESCAPE '\\' \
           AND NOT con.convalidated",
    )
    .await;
    checks.push(if fk_total == 66 {
        Check::pass("C4-04", "composite_fk_count", format!("{fk_total}/66"))
    } else {
        Check::fail("C4-04", "composite_fk_count", format!("{fk_total}/66"))
    });
    checks.push(if fk_unvalidated == 0 {
        Check::pass("C4-04", "composite_fk_validated", "not_valid=0")
    } else {
        Check::fail(
            "C4-04",
            "composite_fk_validated",
            format!("not_valid={fk_unvalidated}"),
        )
    });

    // C4-05 cross-cooperative + dangling probes, generated dynamically
    // from every composite FK (child col -> parent id).
    let fks: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT con.conname, cl.relname, pcl.relname, catt.attname, patt.attname \
         FROM pg_constraint con \
         JOIN pg_class cl ON cl.oid = con.conrelid \
         JOIN pg_class pcl ON pcl.oid = con.confrelid \
         JOIN pg_attribute catt ON catt.attrelid = con.conrelid \
           AND catt.attnum = ANY(con.conkey) AND catt.attname <> 'cooperative_id' \
         JOIN pg_attribute patt ON patt.attrelid = con.confrelid \
           AND patt.attnum = ANY(con.confkey) AND patt.attname <> 'cooperative_id' \
         WHERE con.contype = 'f' AND con.conname LIKE '%\\_coop\\_fk' ESCAPE '\\' \
         ORDER BY con.conname",
    )
    .fetch_all(pool)
    .await
    .expect("fk metadata");
    assert_eq!(fks.len(), 66, "composite FK metadata must enumerate 66");
    for (name, child, parent, ccol, pcol) in &fks {
        let cross = scalar_i64(
            pool,
            &format!(
                "SELECT count(*) FROM {child} c JOIN {parent} p \
                 ON p.{pcol} = c.{ccol} \
                 WHERE p.cooperative_id IS DISTINCT FROM c.cooperative_id"
            ),
        )
        .await;
        let dangling = scalar_i64(
            pool,
            &format!(
                "SELECT count(*) FROM {child} c LEFT JOIN {parent} p \
                 ON p.{pcol} = c.{ccol} \
                 WHERE c.{ccol} IS NOT NULL AND p.{pcol} IS NULL"
            ),
        )
        .await;
        checks.push(if cross == 0 && dangling == 0 {
            Check::pass("C4-05", name.clone(), "cross=0 dangling=0")
        } else {
            Check::fail(
                "C4-05",
                name.clone(),
                format!("cross={cross} dangling={dangling}"),
            )
        });
    }

    // C4-06 polymorphic movement sources: each source_type resolves to a
    // live same-cooperative row.
    for (src, table) in [
        ("payment", "payments"),
        ("transfer", "account_transfers"),
        ("income", "income_entries"),
        ("expense", "expense_entries"),
        ("share_return_settlement", "share_return_settlements"),
        ("investment_funding", "investment_fundings"),
        ("investment_income", "investment_incomes"),
        ("investment_disposal", "investment_disposal_proceeds"),
        ("social_aid_donation", "social_aid_donations"),
        ("social_aid_disbursement", "social_aid_disbursements"),
    ] {
        let (dangling, cross): (i64, i64) = sqlx::query_as(&format!(
            "SELECT count(*) FILTER (WHERE s.id IS NULL), \
                        count(*) FILTER (WHERE s.id IS NOT NULL \
                          AND s.cooperative_id IS DISTINCT FROM m.cooperative_id) \
                 FROM account_movements m LEFT JOIN {table} s ON s.id = m.source_id \
                 WHERE m.source_type = '{src}'"
        ))
        .fetch_one(pool)
        .await
        .expect("movement source probe");
        checks.push(if dangling == 0 && cross == 0 {
            Check::pass("C4-06", src, "dangling=0 cross=0")
        } else {
            Check::fail("C4-06", src, format!("dangling={dangling} cross={cross}"))
        });
    }

    // C4-07 duplicate identifiers (cooperative-scoped uniqueness —
    // adopted numbering series legitimately repeat across
    // cooperatives, M1-P3-B2).
    for (label, table, col) in [
        ("families.sequence_number", "families", "sequence_number"),
        ("periods.period_number", "periods", "period_number"),
        ("payments.payment_number", "payments", "payment_number"),
        ("payments.idempotency_key", "payments", "idempotency_key"),
        (
            "account_transfers.transfer_number",
            "account_transfers",
            "transfer_number",
        ),
        (
            "account_transfers.idempotency_key",
            "account_transfers",
            "idempotency_key",
        ),
        (
            "shareholder_credits.credit_number",
            "shareholder_credits",
            "credit_number",
        ),
        (
            "shareholder_credits.idempotency_key",
            "shareholder_credits",
            "idempotency_key",
        ),
        (
            "credit_applications.idempotency_key",
            "credit_applications",
            "idempotency_key",
        ),
        ("shares.share_number", "shares", "share_number"),
        (
            "income_entries.income_number",
            "income_entries",
            "income_number",
        ),
        (
            "income_entries.idempotency_key",
            "income_entries",
            "idempotency_key",
        ),
        (
            "expense_entries.expense_number",
            "expense_entries",
            "expense_number",
        ),
        (
            "expense_entries.idempotency_key",
            "expense_entries",
            "idempotency_key",
        ),
        (
            "share_returns.return_number",
            "share_returns",
            "return_number",
        ),
        (
            "share_returns.idempotency_key",
            "share_returns",
            "idempotency_key",
        ),
        (
            "share_return_entitlements.entitlement_number",
            "share_return_entitlements",
            "entitlement_number",
        ),
        (
            "share_return_settlements.settlement_number",
            "share_return_settlements",
            "settlement_number",
        ),
        (
            "share_return_settlements.idempotency_key",
            "share_return_settlements",
            "idempotency_key",
        ),
        (
            "investments.investment_number",
            "investments",
            "investment_number",
        ),
        (
            "investments.idempotency_key",
            "investments",
            "idempotency_key",
        ),
        (
            "investment_fundings.funding_number",
            "investment_fundings",
            "funding_number",
        ),
        (
            "investment_fundings.idempotency_key",
            "investment_fundings",
            "idempotency_key",
        ),
        (
            "investment_valuations.valuation_number",
            "investment_valuations",
            "valuation_number",
        ),
        (
            "investment_valuations.idempotency_key",
            "investment_valuations",
            "idempotency_key",
        ),
        (
            "investment_incomes.income_number",
            "investment_incomes",
            "income_number",
        ),
        (
            "investment_incomes.idempotency_key",
            "investment_incomes",
            "idempotency_key",
        ),
        (
            "investment_disposals.disposal_number",
            "investment_disposals",
            "disposal_number",
        ),
        (
            "investment_disposals.idempotency_key",
            "investment_disposals",
            "idempotency_key",
        ),
        (
            "social_aid_funds.fund_number",
            "social_aid_funds",
            "fund_number",
        ),
        (
            "social_aid_funds.idempotency_key",
            "social_aid_funds",
            "idempotency_key",
        ),
        (
            "social_aid_donations.donation_number",
            "social_aid_donations",
            "donation_number",
        ),
        (
            "social_aid_donations.idempotency_key",
            "social_aid_donations",
            "idempotency_key",
        ),
        (
            "social_aid_disbursements.disbursement_number",
            "social_aid_disbursements",
            "disbursement_number",
        ),
        (
            "social_aid_disbursements.idempotency_key",
            "social_aid_disbursements",
            "idempotency_key",
        ),
        (
            "governance_bodies.body_number",
            "governance_bodies",
            "body_number",
        ),
        (
            "governance_bodies.idempotency_key",
            "governance_bodies",
            "idempotency_key",
        ),
        (
            "governance_memberships.idempotency_key",
            "governance_memberships",
            "idempotency_key",
        ),
        (
            "governance_decisions.decision_number",
            "governance_decisions",
            "decision_number",
        ),
        (
            "governance_decisions.idempotency_key",
            "governance_decisions",
            "idempotency_key",
        ),
        (
            "governance_votes.idempotency_key",
            "governance_votes",
            "idempotency_key",
        ),
    ] {
        // NULLs are legitimate (auto-apply stamps the key on the first
        // row only); uniqueness is enforced within each cooperative.
        let dup = scalar_i64(
            pool,
            &format!(
                "SELECT count(*) FILTER (WHERE {col} IS NOT NULL) \
                 - count(DISTINCT (cooperative_id, {col})) \
                   FILTER (WHERE {col} IS NOT NULL) FROM {table}"
            ),
        )
        .await;
        checks.push(if dup == 0 {
            Check::pass("C4-07", label, "duplicates=0")
        } else {
            Check::fail("C4-07", label, format!("duplicates={dup}"))
        });
    }

    // C4-08 C5 replacement inventory: UNIQUE indexes on business tables
    // that do not yet include cooperative_id. Informational rows plus one
    // gate confirming none were rescoped/dropped early.
    let uniques: Vec<(String, String)> = sqlx::query_as(
        "SELECT idx.indexrelid::regclass::text, idx.indrelid::regclass::text \
         FROM pg_index idx \
         JOIN pg_class t ON t.oid = idx.indrelid \
         WHERE t.relkind = 'r' AND idx.indisunique \
           AND t.relname = ANY($1) \
           AND NOT EXISTS ( \
             SELECT 1 FROM pg_attribute a \
             WHERE a.attrelid = idx.indrelid AND a.attname = 'cooperative_id' \
               AND a.attnum = ANY(idx.indkey)) \
         ORDER BY 2, 1",
    )
    .bind(&BUSINESS_TABLES[..])
    .fetch_all(pool)
    .await
    .expect("unique index inventory");
    for (index, table) in &uniques {
        checks.push(Check::info(
            "C4-08",
            index.clone(),
            format!("table={table}"),
        ));
    }
    checks.push(if !uniques.is_empty() {
        Check::pass(
            "C4-08",
            "c5_inventory_intact",
            format!("{} global uniques preserved", uniques.len()),
        )
    } else {
        Check::fail("C4-08", "c5_inventory_intact", "no global uniques found")
    });

    // C4-09 reverse binding: rows that POINT at a movement must match its
    // cooperative (movement_id -> account_movements).
    for (label, table, col) in [
        (
            "income_entries.account_movement_id",
            "income_entries",
            "account_movement_id",
        ),
        (
            "expense_entries.account_movement_id",
            "expense_entries",
            "account_movement_id",
        ),
        (
            "share_return_settlements.account_movement_id",
            "share_return_settlements",
            "account_movement_id",
        ),
        (
            "investment_fundings.account_movement_id",
            "investment_fundings",
            "account_movement_id",
        ),
        (
            "investment_incomes.account_movement_id",
            "investment_incomes",
            "account_movement_id",
        ),
        (
            "investment_disposal_proceeds.account_movement_id",
            "investment_disposal_proceeds",
            "account_movement_id",
        ),
        (
            "social_aid_donations.account_movement_id",
            "social_aid_donations",
            "account_movement_id",
        ),
        (
            "social_aid_disbursements.account_movement_id",
            "social_aid_disbursements",
            "account_movement_id",
        ),
    ] {
        let violations = scalar_i64(
            pool,
            &format!(
                "SELECT count(*) FROM {table} e \
                 JOIN account_movements m ON m.id = e.{col} \
                 WHERE e.{col} IS NOT NULL \
                   AND m.cooperative_id IS DISTINCT FROM e.cooperative_id"
            ),
        )
        .await;
        checks.push(if violations == 0 {
            Check::pass("C4-09", label, "violations=0")
        } else {
            Check::fail("C4-09", label, format!("violations={violations}"))
        });
    }

    // C4-10 bootstrap seed ownership: a bootstrap cooperative exists, and
    // every global seed category (created_by IS NULL) is claimed.
    let (bootstrap_coops, unclaimed): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM cooperatives WHERE is_bootstrap), \
                (SELECT count(*) FROM financial_categories \
                  WHERE created_by IS NULL AND cooperative_id IS NULL)",
    )
    .fetch_one(pool)
    .await
    .expect("seed ownership probe");
    checks.push(if bootstrap_coops > 0 && unclaimed == 0 {
        Check::pass(
            "C4-10",
            "seed_categories_claimed",
            format!("bootstrap={bootstrap_coops} unclaimed=0"),
        )
    } else {
        Check::fail(
            "C4-10",
            "seed_categories_claimed",
            format!("bootstrap={bootstrap_coops} unclaimed={unclaimed}"),
        )
    });

    checks
}

async fn emit_report(checks: &[Check]) {
    for check in checks {
        check.emit();
    }
    let failed: Vec<String> = checks
        .iter()
        .filter(|c| !c.passed)
        .map(|c| format!("{}:{}", c.id, c.subject))
        .collect();
    let report = serde_json::json!({
        "suite": "M1-P2-C4",
        "checks": checks.len(),
        "passed": checks.iter().filter(|c| c.passed).count(),
        "failed": failed,
    });
    let json = serde_json::to_string_pretty(&report).expect("json");
    println!(
        "C4-REPORT|{}",
        serde_json::to_string(&report).expect("json")
    );
    if let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let path = std::path::Path::new(&dir)
            .join("..")
            .join("..")
            .join("implementation")
            .join("M1-P2-C4-RESULT.json");
        let _ = std::fs::write(&path, &json);
    }
}

// ---------------------------------------------------------------------

#[tokio::test]
async fn c4_populated_two_cooperative_database_passes_all_gates() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    populate(&db.pool, fx.coop_a, fx.user, "a").await;
    populate(&db.pool, fx.coop_b, fx.user, "b").await;

    let checks = run_checks(&db.pool).await;
    emit_report(&checks).await;

    let failures: Vec<&Check> = checks.iter().filter(|c| !c.passed).collect();
    assert!(
        failures.is_empty(),
        "C4 gate failures: {:?}",
        failures
            .iter()
            .map(|c| format!("{}:{}", c.id, c.subject))
            .collect::<Vec<_>>()
    );
    db.cleanup().await;
}

/// The battery must not be vacuous: a dangling polymorphic movement
/// source is detected. Since C5-3 made `cooperative_id` NOT NULL on all
/// business tables, the historical NULL-ownership injection is now
/// rejected by the schema itself — the test asserts that rejection and
/// keeps the dangling-source detection probe.
#[tokio::test]
async fn c4_checks_detect_injected_violations() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;
    let now = OffsetDateTime::now_utc();
    let acc = create_account_at(&db.pool, Some(fx.coop_a), fx.user, "Kasa v").await;

    // NULL ownership is physically impossible since C5-3 — prove the
    // schema wall holds instead of injecting a NULL row.
    let unowned = sqlx::query(
        "INSERT INTO persons (first_name, last_name, search_name) \
         VALUES ('Sahipsiz', 'Kayit', 'sahipsiz kayit')",
    )
    .execute(&db.pool)
    .await;
    assert!(
        unowned.is_err(),
        "NULL cooperative_id must be rejected by NOT NULL (C5-3)"
    );

    // Inject: a movement whose polymorphic source does not exist.
    sqlx::query(
        "INSERT INTO account_movements \
         (account_id, direction, amount, source_type, source_id, occurred_at, \
          created_by, cooperative_id) \
         VALUES ($1, 'inflow', 1, 'payment', $2, $3, $4, $5)",
    )
    .bind(acc)
    .bind(Uuid::new_v4())
    .bind(now)
    .bind(fx.user)
    .bind(fx.coop_a)
    .execute(&db.pool)
    .await
    .expect("inject dangling movement");

    let checks = run_checks(&db.pool).await;
    for check in &checks {
        check.emit();
    }
    assert!(
        checks
            .iter()
            .any(|c| c.id == "C4-02" && c.subject == "persons" && c.passed),
        "NULL-ownership scan stays green — the schema forbids the row"
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "C4-06" && c.subject == "payment" && !c.passed),
        "dangling movement source must be detected"
    );
    db.cleanup().await;
}
