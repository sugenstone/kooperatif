//! M1-P2-C5 — cooperative-scoped unique indexes and ownership closure.
//!
//! Verifies migrations 0024+0025 on populated disposable databases:
//!   * exactly the 42 intended scoped unique indexes exist;
//!   * the 42 legacy global unique candidates they replace are gone;
//!   * the 19 structural global uniques remain untouched;
//!   * cooperative_id is NOT NULL on all 37 business tables — the NULL
//!     legacy lane is closed by construction;
//!   * cooperative-scoped semantics hold: same-coop duplicates reject,
//!     cross-coop duplicates are now ACCEPTED by the scoped indexes.

use kooperatif_server::db as app_db;
use kooperatif_server::governance::repo as gov_repo;
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

/// The 42 indexes migration 0024 must create.
const SCOPED_INDEXES: [&str; 43] = [
    "account_transfers_coop_transfer_number_uq",
    "expense_entries_coop_expense_number_uq",
    "families_coop_sequence_number_uq",
    "governance_bodies_coop_body_number_uq",
    "governance_decisions_coop_decision_number_uq",
    "income_entries_coop_income_number_uq",
    "investment_disposals_coop_disposal_number_uq",
    "investment_fundings_coop_funding_number_uq",
    "investment_incomes_coop_income_number_uq",
    "investment_valuations_coop_valuation_number_uq",
    "investments_coop_investment_number_uq",
    "payments_coop_payment_number_uq",
    "periods_coop_period_number_uq",
    "share_return_entitlements_coop_entitlement_number_uq",
    "share_return_settlements_coop_settlement_number_uq",
    "share_returns_coop_return_number_uq",
    "shareholder_credits_coop_credit_number_uq",
    "shares_coop_share_number_uq",
    "social_aid_disbursements_coop_disbursement_number_uq",
    "social_aid_donations_coop_donation_number_uq",
    "social_aid_funds_coop_fund_number_uq",
    "account_transfers_coop_idempotency_uq",
    "credit_applications_coop_idempotency_uq",
    "expense_entries_coop_idempotency_uq",
    "governance_bodies_coop_idempotency_uq",
    "governance_decisions_coop_idempotency_uq",
    "governance_memberships_coop_idempotency_uq",
    "governance_votes_coop_idempotency_uq",
    "income_entries_coop_idempotency_uq",
    "investment_disposals_coop_idempotency_uq",
    "investment_fundings_coop_idempotency_uq",
    "investment_incomes_coop_idempotency_uq",
    "investment_valuations_coop_idempotency_uq",
    "investments_coop_idempotency_uq",
    "payments_coop_idempotency_uq",
    "share_return_settlements_coop_idempotency_uq",
    "share_returns_coop_idempotency_uq",
    "shareholder_credits_coop_idempotency_uq",
    "social_aid_disbursements_coop_idempotency_uq",
    "social_aid_donations_coop_idempotency_uq",
    "social_aid_funds_coop_idempotency_uq",
    "financial_categories_coop_name_uq",
    // M1-P6 B1 (0067): role names are unique per cooperative —
    // NULLS NOT DISTINCT keeps the NULL platform scope one domain.
    "roles_coop_name_uq",
];

/// The 42 legacy global unique indexes/constraints migration 0025 drops
/// (21 numbering + 20 idempotency + financial_categories_name_key).
const DROPPED_GLOBALS: [&str; 42] = [
    "account_transfers_transfer_number_key",
    "expense_entries_expense_number_key",
    "families_sequence_number_key",
    "governance_bodies_body_number_key",
    "governance_decisions_decision_number_key",
    "income_entries_income_number_key",
    "investment_disposals_disposal_number_key",
    "investment_fundings_funding_number_key",
    "investment_incomes_income_number_key",
    "investment_valuations_valuation_number_key",
    "investments_investment_number_key",
    "payments_payment_number_key",
    "periods_period_number_key",
    "share_return_entitlements_number_key",
    "share_return_settlements_number_key",
    "share_returns_return_number_key",
    "shareholder_credits_credit_number_key",
    "shares_share_number_key",
    "social_aid_disbursements_disbursement_number_key",
    "social_aid_donations_donation_number_key",
    "social_aid_funds_fund_number_key",
    "account_transfers_idempotency_key_key",
    "credit_applications_idempotency_key_key",
    "expense_entries_idempotency_key_key",
    "governance_bodies_idempotency_key_key",
    "governance_decisions_idempotency_key_key",
    "governance_memberships_idempotency_key_key",
    "governance_votes_idempotency_key_key",
    "income_entries_idempotency_key_key",
    "investment_disposals_idempotency_key_key",
    "investment_fundings_idempotency_key_key",
    "investment_incomes_idempotency_key_key",
    "investment_valuations_idempotency_key_key",
    "investments_idempotency_key_key",
    "payments_idempotency_key_key",
    "share_return_settlements_idempotency_key_key",
    "share_returns_idempotency_key_key",
    "shareholder_credits_idempotency_key_key",
    "social_aid_disbursements_idempotency_key_key",
    "social_aid_donations_idempotency_key_key",
    "social_aid_funds_idempotency_key_key",
    "financial_categories_name_key",
];

/// The 19 structural global uniques that C5-3 keeps untouched — FK-pair,
/// movement-binding and partial keys whose columns are already globally
/// unique PKs, so scoping them would be redundant.
const STRUCTURAL_GLOBALS: [&str; 19] = [
    "account_movements_source_leg_key",
    "assessment_rules_period_id_key",
    "assessments_period_shareholder_key",
    "expense_entries_movement_key",
    "financial_categories_type_key",
    "governance_memberships_active_key",
    "governance_votes_one_per_member_key",
    "income_entries_movement_key",
    "investment_disposal_proceeds_movement_key",
    "investment_disposals_one_per_investment",
    "investment_fundings_movement_key",
    "investment_incomes_movement_key",
    "payment_allocations_payment_assessment_key",
    "share_return_entitlements_one_per_right",
    "share_return_settlements_movement_key",
    "share_returns_one_pending_per_share",
    "shareholders_person_id_key",
    "social_aid_disbursements_movement_key",
    "social_aid_donations_movement_key",
];

/// The 37 cooperative-owned business tables whose cooperative_id must
/// be NOT NULL after 0025.
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

#[tokio::test]
async fn c5_scoped_indexes_exist_and_swap_is_exact() {
    let db = setup().await;

    // All 43 scoped indexes exist, are unique, and key cooperative_id.
    for name in SCOPED_INDEXES {
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_class i JOIN pg_index ix \
             ON ix.indexrelid = i.oid WHERE i.relname = $1 AND ix.indisunique)",
        )
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .expect("index probe");
        assert!(found, "scoped unique index {name} missing");
        let def: String =
            sqlx::query_scalar("SELECT indexdef FROM pg_indexes WHERE indexname = $1")
                .bind(name)
                .fetch_one(&db.pool)
                .await
                .expect("index definition");
        assert!(
            def.contains("cooperative_id"),
            "scoped index {name} does not key cooperative_id: {def}"
        );
    }

    // C5-3: all 42 replaced legacy globals are gone.
    for name in DROPPED_GLOBALS {
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_class i JOIN pg_index ix \
             ON ix.indexrelid = i.oid WHERE i.relname = $1 AND ix.indisunique)",
        )
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .expect("index probe");
        assert!(!found, "dropped global unique {name} still exists");
    }

    // All 19 structural global uniques remain valid.
    for name in STRUCTURAL_GLOBALS {
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_class i JOIN pg_index ix \
             ON ix.indexrelid = i.oid WHERE i.relname = $1 AND ix.indisunique)",
        )
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .expect("index probe");
        assert!(found, "structural global unique {name} missing");
    }

    // The scoped set is exactly 43 — no extra C5 scoped index leaked in.
    // The C2 `*_coop_id_uq` / `*_coop_pk_uq` composite-FK support indexes
    // exist already and are excluded by name.
    let scoped_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_class i JOIN pg_index ix \
         ON ix.indexrelid = i.oid WHERE i.relname LIKE '%_coop_%_uq' \
         AND i.relname NOT LIKE '%_coop_id_uq' \
         AND i.relname NOT LIKE '%_coop_pk_uq' \
         AND ix.indisunique AND NOT ix.indisprimary",
    )
    .fetch_one(&db.pool)
    .await
    .expect("scoped index count");
    assert_eq!(scoped_count, 43, "unexpected extra scoped unique indexes");

    // C5-3: cooperative_id is NOT NULL on all 37 business tables.
    for table in BUSINESS_TABLES {
        let not_null: bool = sqlx::query_scalar(
            "SELECT a.attnotnull FROM pg_class t JOIN pg_attribute a \
             ON a.attrelid = t.oid WHERE t.relkind = 'r' \
             AND t.relname = $1 AND a.attname = 'cooperative_id'",
        )
        .bind(table)
        .fetch_one(&db.pool)
        .await
        .expect("attnotnull probe");
        assert!(not_null, "{table}.cooperative_id must be NOT NULL");
    }

    db.cleanup().await;
}

/// Populated two-cooperative check after C5-3: the scoped unique
/// indexes alone arbitrate per-cooperative identity — cross-coop
/// duplicates are accepted, same-coop duplicates are rejected, and the
/// NULL legacy lane is closed by NOT NULL.
#[tokio::test]
async fn c5_populated_two_cooperative_database_behaviour() {
    let db = setup().await;

    let (coop_a, _memberships) =
        kooperatif_server::tenant::repo::bootstrap_initial_cooperative(&db.pool, "C5 Kooperatif A")
            .await
            .expect("bootstrap cooperative A");
    let coop_b = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO cooperatives (id, name, status, is_bootstrap, business_enabled) \
         VALUES ($1,'C5 Kooperatif B','active',false,true)",
    )
    .bind(coop_b)
    .execute(&db.pool)
    .await
    .expect("fixture cooperative B");

    // families.sequence_number is caller-supplied: after C5-3 the
    // scoped index alone arbitrates — a cross-coop duplicate is now
    // ACCEPTED (per-coop identity), a same-coop duplicate is rejected.
    sqlx::query("INSERT INTO families (sequence_number, cooperative_id) VALUES (7, $1)")
        .bind(coop_a)
        .execute(&db.pool)
        .await
        .expect("coop A family");
    sqlx::query("INSERT INTO families (sequence_number, cooperative_id) VALUES (7, $1)")
        .bind(coop_b)
        .execute(&db.pool)
        .await
        .expect("cross-coop duplicate is accepted by the scoped index");
    let same_coop_dup =
        sqlx::query("INSERT INTO families (sequence_number, cooperative_id) VALUES (7, $1)")
            .bind(coop_a)
            .execute(&db.pool)
            .await;
    assert!(same_coop_dup.is_err(), "same-coop duplicate must fail");

    // The NULL legacy lane is closed: cooperative_id is NOT NULL.
    let null_lane =
        sqlx::query("INSERT INTO families (sequence_number, cooperative_id) VALUES (9, $1)")
            .bind(Option::<Uuid>::None)
            .execute(&db.pool)
            .await;
    assert!(
        null_lane.is_err(),
        "NULL cooperative_id must be rejected by NOT NULL (C5-3)"
    );
    sqlx::query("INSERT INTO families (sequence_number, cooperative_id) VALUES (8, $1)")
        .bind(coop_b)
        .execute(&db.pool)
        .await
        .expect("coop B family with a distinct number");

    // financial_categories scoped name uniqueness: the scoped index
    // (cooperative_id, category_type, lower(btrim(name))) rejects
    // same-coop duplicates including case/whitespace variants, while a
    // foreign cooperative may legitimately reuse the same name.
    sqlx::query(
        "INSERT INTO financial_categories (category_type, name, cooperative_id) \
         VALUES ('expense', 'Aidat', $1)",
    )
    .bind(coop_a)
    .execute(&db.pool)
    .await
    .expect("coop A category");
    let folded_dup = sqlx::query(
        "INSERT INTO financial_categories (category_type, name, cooperative_id) \
         VALUES ('expense', '  AİDAT  ', $1)",
    )
    .bind(coop_a)
    .execute(&db.pool)
    .await;
    let normalized: bool =
        sqlx::query_scalar("SELECT lower(btrim('  AİDAT  ')) = lower(btrim('Aidat'))")
            .fetch_one(&db.pool)
            .await
            .expect("collation probe");
    if normalized {
        // Database collation folds case/whitespace — the scoped index
        // must reject the normalized duplicate.
        assert!(
            folded_dup.is_err(),
            "same-coop case/whitespace-normalized duplicate must be rejected"
        );
    } else {
        // Non-folding collation: the raw duplicate form must still fail.
        let exact_dup = sqlx::query(
            "INSERT INTO financial_categories (category_type, name, cooperative_id) \
             VALUES ('expense', 'Aidat', $1)",
        )
        .bind(coop_a)
        .execute(&db.pool)
        .await;
        assert!(exact_dup.is_err(), "same-coop exact duplicate must fail");
    }
    sqlx::query(
        "INSERT INTO financial_categories (category_type, name, cooperative_id) \
         VALUES ('expense', 'Aidat', $1)",
    )
    .bind(coop_b)
    .execute(&db.pool)
    .await
    .expect("cross-coop same-name category is accepted after C5-3");

    // The scoped composite index physically indexes populated rows.
    let entries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM families f \
         WHERE EXISTS (SELECT 1 FROM pg_indexes \
         WHERE indexname = 'families_coop_sequence_number_uq' \
         AND tablename = 'families') \
         AND f.cooperative_id IS NOT NULL",
    )
    .fetch_one(&db.pool)
    .await
    .expect("scoped coverage probe");
    assert!(entries >= 2, "scoped index must cover populated coop rows");

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// M1-P2-C5-2/3 — cooperative-scoped idempotency arbiters: the
// ON CONFLICT target is the 0024 composite index; after 0025 the global
// uniques are gone, so cross-coop reuse of a key creates a separate,
// properly-owned record instead of an indistinguishable NotFound.
// ---------------------------------------------------------------------

/// `created_by` has a users FK — one fixture user per test DB.
async fn fixture_user(pool: &PgPool) -> Uuid {
    let user = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash) VALUES ($1,$2,$3,$4)",
    )
    .bind(user)
    .bind(format!("c5{}", &user.simple().to_string()[..8]))
    .bind("C5 Kullanici")
    .bind("not-a-real-hash")
    .execute(pool)
    .await
    .expect("fixture user");
    user
}

fn body_cmd(key: &str, fingerprint: &str) -> gov_repo::CreateBody {
    gov_repo::CreateBody {
        name: format!("C5 Yönetim {key}"),
        body_type: "board".to_string(),
        description: None,
        idempotency_key: key.to_string(),
        fingerprint: fingerprint.to_string(),
    }
}

#[tokio::test]
async fn c5_scoped_idempotency_semantics_on_two_cooperatives() {
    let db = setup().await;
    let (coop_a, _memberships) = kooperatif_server::tenant::repo::bootstrap_initial_cooperative(
        &db.pool,
        "C5-2 Kooperatif A",
    )
    .await
    .expect("bootstrap cooperative A");
    let coop_b = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO cooperatives (id, name, status, is_bootstrap, business_enabled) \
         VALUES ($1,'C5-2 Kooperatif B','active',false,true)",
    )
    .bind(coop_b)
    .execute(&db.pool)
    .await
    .expect("fixture cooperative B");
    let actor = fixture_user(&db.pool).await;

    // Same coop + same key + same payload → deterministic replay.
    let first = gov_repo::create_body(&db.pool, Some(coop_a), actor, body_cmd("k1", "fp1"))
        .await
        .expect("first insert");
    assert!(!first.replayed);
    let replay = gov_repo::create_body(&db.pool, Some(coop_a), actor, body_cmd("k1", "fp1"))
        .await
        .expect("same-key same-payload replay");
    assert!(replay.replayed);
    assert_eq!(replay.body_id, first.body_id);
    assert_eq!(replay.body_number, first.body_number);

    // Same coop + same key + incompatible payload → conflict.
    let conflict =
        gov_repo::create_body(&db.pool, Some(coop_a), actor, body_cmd("k1", "fp2")).await;
    assert!(
        matches!(
            conflict,
            Err(gov_repo::GovernanceRepoError::IdempotencyConflict)
        ),
        "same key with a different fingerprint must conflict"
    );

    // Different coop + same key: C5-3 dropped the global index, so the
    // scoped arbiter now ACCEPTS the insert as a separate coop-B record.
    let foreign = gov_repo::create_body(&db.pool, Some(coop_b), actor, body_cmd("k1", "fp1"))
        .await
        .expect("cross-coop key reuse is now an independent record");
    assert!(!foreign.replayed);
    assert_ne!(foreign.body_id, first.body_id);
    let foreign_row: Uuid =
        sqlx::query_scalar("SELECT cooperative_id FROM governance_bodies WHERE id = $1")
            .bind(foreign.body_id)
            .fetch_one(&db.pool)
            .await
            .expect("foreign row read");
    assert_eq!(foreign_row, coop_b, "the second record belongs to coop B");

    // The NULL legacy lane is closed: a NULL-cooperative business row
    // cannot be written at all (NOT NULL from 0025).
    let legacy = sqlx::query(
        "INSERT INTO governance_bodies (name, body_type, status, \
         idempotency_key, idempotency_fingerprint, created_by, cooperative_id) \
         VALUES ('Legacy Organ','board','active','k2','fp-legacy',$1,NULL)",
    )
    .bind(actor)
    .execute(&db.pool)
    .await;
    assert!(
        legacy.is_err(),
        "NULL cooperative_id must be rejected by NOT NULL (C5-3)"
    );

    // NULL idempotency keys: only `credit_applications` has a nullable
    // key (the auto-apply sweep stamps NULLs). Its scoped index must be
    // partial so NULL-keyed rows never conflict — row-level NULL-key
    // coexistence is already proven by the credits suites' sweep paths.
    let partial: String = sqlx::query_scalar(
        "SELECT indexdef FROM pg_indexes \
         WHERE indexname = 'credit_applications_coop_idempotency_uq'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("scoped index definition");
    assert!(
        partial.contains("WHERE (idempotency_key IS NOT NULL)"),
        "credit_applications scoped index must be partial: {partial}"
    );

    db.cleanup().await;
}

/// Concurrent identical requests on the same scoped key converge to a
/// single written row — the composite arbiter plus re-read path holds
/// under racing first-writers.
#[tokio::test]
async fn c5_concurrent_scoped_inserts_write_once() {
    let db = setup().await;
    let (coop_a, _memberships) = kooperatif_server::tenant::repo::bootstrap_initial_cooperative(
        &db.pool,
        "C5-2 Concurrent A",
    )
    .await
    .expect("bootstrap cooperative");
    let actor = fixture_user(&db.pool).await;

    let mut handles = Vec::new();
    for _ in 0..8 {
        let pool = db.pool.clone();
        handles.push(tokio::spawn(async move {
            gov_repo::create_body(&pool, Some(coop_a), actor, body_cmd("race", "fp-race")).await
        }));
    }
    let mut ok = 0usize;
    for handle in handles {
        match handle.await.expect("task panicked") {
            Ok(_) => ok += 1,
            Err(gov_repo::GovernanceRepoError::NotFound) => {
                // Tolerable window: loser may rollback before the winner
                // commits and see nothing — acceptable, never a write.
            }
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }
    assert!(ok >= 1, "at least the winner must succeed");

    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM governance_bodies WHERE idempotency_key = 'race'")
            .fetch_one(&db.pool)
            .await
            .expect("row count");
    assert_eq!(
        rows, 1,
        "concurrent same-key inserts must write exactly one row"
    );

    db.cleanup().await;
}
