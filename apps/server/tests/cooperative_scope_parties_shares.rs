//! M1-P2-C3 — cooperative-scoped data access, first module batch:
//! persons, families, shareholders, shares and ownership relationships.
//!
//! Proves at the repository layer — the enforcement point — that every
//! read, lookup, insert, update and transactional operation in this
//! batch is scoped to the verified cooperative context, that
//! cross-cooperative identifiers resolve to not-found without
//! disclosure, that new rows carry the correct cooperative_id, and
//! that the `None` context (reachable only in the proven
//! never-bootstrapped lane) sees only legacy NULL-cooperative rows.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL (per-test disposable database,
//! same convention as the other integration suites).

use kooperatif_server::db as app_db;
use kooperatif_server::parties::repo as party_repo;
use kooperatif_server::shares::model::{AcquisitionType, ShareStatus};
use kooperatif_server::shares::repo as share_repo;
use sqlx::PgPool;
use time::OffsetDateTime;
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

/// Create a full same-cooperative shareholder aggregate through the
/// repository (new person + new family + membership).
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

async fn create_share_at(
    pool: &PgPool,
    coop: Option<Uuid>,
    actor: Uuid,
    shareholder_id: Uuid,
) -> (Uuid, i64) {
    share_repo::create_share(
        pool,
        coop,
        actor,
        share_repo::CreateShare {
            shareholder_id,
            acquisition_type: AcquisitionType::Founder,
            fee: None,
            effective_at: OffsetDateTime::now_utc(),
            reason: None,
        },
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("share aggregate")
}

// ---------------------------------------------------------------------
// Read isolation: lists, details, search, aggregates, indirect lookups.
// ---------------------------------------------------------------------

#[tokio::test]
async fn reads_are_scoped_to_the_verified_cooperative() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    let sh_a =
        create_shareholder_at(&db.pool, Some(fx.coop_a), fx.user, "Ayşe", "Yılmaz", 11).await;
    let sh_b =
        create_shareholder_at(&db.pool, Some(fx.coop_b), fx.user, "Mehmet", "Demir", 22).await;
    let (share_a, _) =
        create_share_at(&db.pool, Some(fx.coop_a), fx.user, sh_a.shareholder_id).await;
    let (share_b, _) =
        create_share_at(&db.pool, Some(fx.coop_b), fx.user, sh_b.shareholder_id).await;

    // Shareholder list: each cooperative sees exactly its own rows.
    let rows_a = party_repo::list_shareholders(&db.pool, Some(fx.coop_a), None, None, 1, 50)
        .await
        .unwrap();
    assert_eq!(rows_a.len(), 1);
    assert_eq!(rows_a[0].id, sh_a.shareholder_id);
    let rows_b = party_repo::list_shareholders(&db.pool, Some(fx.coop_b), None, None, 1, 50)
        .await
        .unwrap();
    assert_eq!(rows_b.len(), 1);
    assert_eq!(rows_b[0].id, sh_b.shareholder_id);

    // Detail: a foreign shareholder id resolves to None — same as a
    // missing id, no cross-cooperative disclosure.
    assert!(
        party_repo::find_shareholder(&db.pool, Some(fx.coop_a), sh_b.shareholder_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        party_repo::find_shareholder(&db.pool, Some(fx.coop_a), sh_a.shareholder_id)
            .await
            .unwrap()
            .is_some()
    );

    // Person search is scoped (guardian/person picker).
    let persons_a = party_repo::search_persons(&db.pool, Some(fx.coop_a), "Mehmet", 20)
        .await
        .unwrap();
    assert!(persons_a.is_empty());
    let persons_b = party_repo::search_persons(&db.pool, Some(fx.coop_b), "Mehmet", 20)
        .await
        .unwrap();
    assert_eq!(persons_b.len(), 1);

    // Family list/detail scoped (list includes the aggregate member
    // count path — it counts only in-scope members).
    let fams_a = party_repo::list_families(&db.pool, Some(fx.coop_a), None, 1, 50)
        .await
        .unwrap();
    assert_eq!(fams_a.len(), 1);
    assert_eq!(fams_a[0].id, sh_a.family_id);
    assert!(
        party_repo::find_family(&db.pool, Some(fx.coop_a), sh_b.family_id)
            .await
            .unwrap()
            .is_none()
    );

    // Indirect lookups: membership history and family member lists.
    assert!(
        party_repo::membership_history(&db.pool, Some(fx.coop_a), sh_b.shareholder_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        party_repo::list_shareholders_by_family(&db.pool, Some(fx.coop_a), sh_b.family_id)
            .await
            .unwrap()
            .is_empty()
    );

    // Duplicate-candidate probing stays inside the cooperative.
    let dup_a = party_repo::duplicate_candidates(&db.pool, Some(fx.coop_a), "Mehmet", "Demir")
        .await
        .unwrap();
    assert!(dup_a.is_empty());

    // Share domain: list, detail, event history, ownership listing.
    let shares_a = share_repo::list_shares(&db.pool, Some(fx.coop_a), None, 1, 50)
        .await
        .unwrap();
    assert_eq!(shares_a.len(), 1);
    assert_eq!(shares_a[0].id, share_a);
    assert!(share_repo::find_share(&db.pool, Some(fx.coop_a), share_b)
        .await
        .unwrap()
        .is_none());
    assert!(
        share_repo::share_event_history(&db.pool, Some(fx.coop_a), share_b, 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        share_repo::list_shares_by_shareholder(&db.pool, Some(fx.coop_a), sh_b.shareholder_id)
            .await
            .unwrap()
            .is_empty()
    );

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// Inserts: every row of the aggregate carries the verified scope.
// ---------------------------------------------------------------------

#[tokio::test]
async fn inserts_bind_the_verified_cooperative_id() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    let sh = create_shareholder_at(&db.pool, Some(fx.coop_a), fx.user, "Ayşe", "Yılmaz", 33).await;
    let (share_id, _) =
        create_share_at(&db.pool, Some(fx.coop_a), fx.user, sh.shareholder_id).await;

    let assert_coop = |table: &str, id: Uuid| {
        let pool = db.pool.clone();
        let coop = fx.coop_a;
        let table = table.to_string();
        async move {
            let row_coop: Option<Uuid> =
                sqlx::query_scalar(&format!("SELECT cooperative_id FROM {table} WHERE id = $1"))
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .expect("row must exist");
            assert_eq!(row_coop, Some(coop), "{table} must carry cooperative_id");
        }
    };
    assert_coop("persons", sh.person_id).await;
    assert_coop("families", sh.family_id).await;
    assert_coop("shareholders", sh.shareholder_id).await;
    assert_coop("shares", share_id).await;

    let membership_coop: Option<Uuid> = sqlx::query_scalar(
        "SELECT cooperative_id FROM shareholder_family_memberships WHERE shareholder_id = $1",
    )
    .bind(sh.shareholder_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(membership_coop, Some(fx.coop_a));

    let event_coop: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM share_events WHERE share_id = $1")
            .bind(share_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(event_coop, Some(fx.coop_a));
    let ownership_coop: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM share_ownerships WHERE share_id = $1")
            .bind(share_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(ownership_coop, Some(fx.coop_a));

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// Writes & transactions: cross-cooperative identifiers fail closed.
// ---------------------------------------------------------------------

#[tokio::test]
async fn mutations_reject_cross_cooperative_identifiers() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    let sh_a =
        create_shareholder_at(&db.pool, Some(fx.coop_a), fx.user, "Ayşe", "Yılmaz", 44).await;
    let sh_b =
        create_shareholder_at(&db.pool, Some(fx.coop_b), fx.user, "Mehmet", "Demir", 55).await;
    let (share_a, _) =
        create_share_at(&db.pool, Some(fx.coop_a), fx.user, sh_a.shareholder_id).await;
    let (share_b, _) =
        create_share_at(&db.pool, Some(fx.coop_b), fx.user, sh_b.shareholder_id).await;

    // A coop-A caller cannot reference coop-B's person/family inside
    // the shareholder aggregate — not-found variants, no disclosure.
    let err = party_repo::create_shareholder(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        party_repo::CreateShareholder {
            person_first_name: None,
            person_last_name: None,
            existing_person_id: Some(sh_b.person_id),
            guardian_person_id: None,
            guardian_first_name: None,
            guardian_last_name: None,
            family: party_repo::FamilyRef::New(66),
            default_collection_account_id: None,
        },
        OffsetDateTime::now_utc(),
    )
    .await
    .err()
    .expect("cross-coop person reference must fail");
    assert!(matches!(
        err,
        party_repo::CreateShareholderError::PersonNotFound
    ));

    let err = party_repo::create_shareholder(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        party_repo::CreateShareholder {
            person_first_name: Some("Zeynep".into()),
            person_last_name: Some("Kaya".into()),
            existing_person_id: None,
            guardian_person_id: None,
            guardian_first_name: None,
            guardian_last_name: None,
            family: party_repo::FamilyRef::Existing(sh_b.family_id),
            default_collection_account_id: None,
        },
        OffsetDateTime::now_utc(),
    )
    .await
    .err()
    .expect("cross-coop family reference must fail");
    assert!(matches!(
        err,
        party_repo::CreateShareholderError::FamilyNotFound
    ));

    // Failed aggregate attempts rolled back: coop A must still have
    // exactly one person/family.
    let person_count_a: i64 =
        sqlx::query_scalar("SELECT count(*) FROM persons WHERE cooperative_id = $1")
            .bind(fx.coop_a)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(person_count_a, 1, "rollback must leave no partial rows");

    // create_share: a foreign shareholder id → ShareholderNotFound.
    let err = share_repo::create_share(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        share_repo::CreateShare {
            shareholder_id: sh_b.shareholder_id,
            acquisition_type: AcquisitionType::Founder,
            fee: None,
            effective_at: OffsetDateTime::now_utc(),
            reason: None,
        },
        OffsetDateTime::now_utc(),
    )
    .await
    .expect_err("cross-coop shareholder must fail");
    assert!(matches!(
        err,
        share_repo::CreateShareError::ShareholderNotFound
    ));

    // change_owner: foreign share → ShareNotFound; foreign transfer
    // target → ShareholderNotFound (inside the same transaction).
    let updated_a: OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM shares WHERE id = $1")
            .bind(share_a)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let updated_b: OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM shares WHERE id = $1")
            .bind(share_b)
            .fetch_one(&db.pool)
            .await
            .unwrap();

    let err = share_repo::change_owner(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        share_repo::ChangeOwner {
            share_id: share_b,
            to_shareholder_id: sh_b.shareholder_id,
            kind: AcquisitionType::Transfer,
            amount: None,
            reason: None,
            effective_at: OffsetDateTime::now_utc(),
            expected_updated_at: updated_b,
            now: OffsetDateTime::now_utc(),
        },
    )
    .await
    .expect_err("foreign share must be not-found");
    assert!(matches!(
        err,
        share_repo::OwnershipChangeError::ShareNotFound
    ));

    let err = share_repo::change_owner(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        share_repo::ChangeOwner {
            share_id: share_a,
            to_shareholder_id: sh_b.shareholder_id,
            kind: AcquisitionType::Sale,
            amount: None,
            reason: None,
            effective_at: OffsetDateTime::now_utc(),
            expected_updated_at: updated_a,
            now: OffsetDateTime::now_utc(),
        },
    )
    .await
    .expect_err("foreign transfer target must fail");
    assert!(matches!(
        err,
        share_repo::OwnershipChangeError::ShareholderNotFound
    ));

    // change_status on a foreign share → ShareNotFound (scoped lock).
    let err = share_repo::change_status(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        share_b,
        ShareStatus::Suspended,
        None,
        OffsetDateTime::now_utc(),
    )
    .await
    .expect_err("foreign share status change must fail");
    assert!(matches!(err, share_repo::StatusChangeError::ShareNotFound));

    // change_family: foreign shareholder → ShareholderNotFound;
    // foreign destination family → FamilyNotFound.
    let sh_a_updated_at: OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM shareholders WHERE id = $1")
            .bind(sh_a.shareholder_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let sh_b_updated_at: OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM shareholders WHERE id = $1")
            .bind(sh_b.shareholder_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();

    let err = party_repo::change_family(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        sh_b.shareholder_id,
        party_repo::FamilyChangeTarget::New(77),
        None,
        sh_b_updated_at,
        OffsetDateTime::now_utc(),
    )
    .await
    .expect_err("foreign shareholder must fail");
    assert!(matches!(
        err,
        party_repo::FamilyChangeError::ShareholderNotFound
    ));

    let err = party_repo::change_family(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        sh_a.shareholder_id,
        party_repo::FamilyChangeTarget::Existing(sh_b.family_id),
        None,
        sh_a_updated_at,
        OffsetDateTime::now_utc(),
    )
    .await
    .expect_err("foreign family must fail");
    assert!(matches!(err, party_repo::FamilyChangeError::FamilyNotFound));

    // Coop-B state untouched by every rejected attempt.
    assert_eq!(
        share_repo::list_shares(&db.pool, Some(fx.coop_b), None, 1, 50)
            .await
            .unwrap()
            .len(),
        1
    );
    let coop_b_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM share_events WHERE cooperative_id = $1 AND event_type <> 'initial_acquisition'",
    )
    .bind(fx.coop_b)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(coop_b_events, 0, "rejected operations wrote nothing");

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// Same-cooperative happy paths still work end-to-end.
// ---------------------------------------------------------------------

#[tokio::test]
async fn same_cooperative_operations_remain_functional() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    let sh_a =
        create_shareholder_at(&db.pool, Some(fx.coop_a), fx.user, "Ayşe", "Yılmaz", 88).await;
    let sh_a2 =
        create_shareholder_at(&db.pool, Some(fx.coop_a), fx.user, "Fatma", "Şahin", 99).await;
    let (share_a, _) =
        create_share_at(&db.pool, Some(fx.coop_a), fx.user, sh_a.shareholder_id).await;

    // Same-coop transfer: ownership closes for A1, opens for A2.
    let updated: OffsetDateTime = sqlx::query_scalar("SELECT updated_at FROM shares WHERE id = $1")
        .bind(share_a)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let (from, _) = share_repo::change_owner(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        share_repo::ChangeOwner {
            share_id: share_a,
            to_shareholder_id: sh_a2.shareholder_id,
            kind: AcquisitionType::Transfer,
            amount: None,
            reason: None,
            effective_at: OffsetDateTime::now_utc(),
            expected_updated_at: updated,
            now: OffsetDateTime::now_utc(),
        },
    )
    .await
    .expect("same-coop transfer");
    assert_eq!(from, sh_a.shareholder_id);

    let owners =
        share_repo::list_shares_by_shareholder(&db.pool, Some(fx.coop_a), sh_a2.shareholder_id)
            .await
            .unwrap();
    assert_eq!(owners.len(), 1);
    let events = share_repo::share_event_history(&db.pool, Some(fx.coop_a), share_a, 20)
        .await
        .unwrap();
    assert_eq!(events.len(), 2); // initial_acquisition + transfer

    // Same-coop family transfer.
    let sh_a2_updated_at: OffsetDateTime =
        sqlx::query_scalar("SELECT updated_at FROM shareholders WHERE id = $1")
            .bind(sh_a2.shareholder_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    party_repo::change_family(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        sh_a2.shareholder_id,
        party_repo::FamilyChangeTarget::Existing(sh_a.family_id),
        None,
        sh_a2_updated_at,
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("same-coop family change");
    let members =
        party_repo::list_shareholders_by_family(&db.pool, Some(fx.coop_a), sh_a.family_id)
            .await
            .unwrap();
    assert_eq!(members.len(), 2);

    // Share lifecycle: void a fresh same-coop share.
    let (share_v, _) =
        create_share_at(&db.pool, Some(fx.coop_a), fx.user, sh_a.shareholder_id).await;
    share_repo::change_status(
        &db.pool,
        Some(fx.coop_a),
        fx.user,
        share_v,
        ShareStatus::Voided,
        None,
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("same-coop void");

    db.cleanup().await;
}

// ---------------------------------------------------------------------
// NULL scope is closed (0025): cooperative_id is NOT NULL on every
// business table — the legacy NULL-cooperative lane can neither be
// written to nor read rows from.
// ---------------------------------------------------------------------

#[tokio::test]
async fn null_scope_is_closed_by_not_null() {
    let db = setup().await;
    let fx = fixture(&db.pool).await;

    let modern =
        create_shareholder_at(&db.pool, Some(fx.coop_a), fx.user, "Yeni", "Kayıt", 222).await;

    // NULL scope sees nothing — NULL-owned rows cannot exist.
    let rows = party_repo::list_shareholders(&db.pool, None, None, None, 1, 50)
        .await
        .unwrap();
    assert!(rows.is_empty());
    assert!(
        party_repo::find_shareholder(&db.pool, None, modern.shareholder_id)
            .await
            .unwrap()
            .is_none(),
        "NULL scope must NOT read cooperative-owned rows"
    );

    // Repo-layer NULL-coop writes are rejected by NOT NULL.
    let err = party_repo::create_shareholder(
        &db.pool,
        None,
        fx.user,
        party_repo::CreateShareholder {
            person_first_name: Some("Eski".to_string()),
            person_last_name: Some("Kayıt".to_string()),
            existing_person_id: None,
            guardian_person_id: None,
            guardian_first_name: None,
            guardian_last_name: None,
            family: party_repo::FamilyRef::New(111),
            default_collection_account_id: None,
        },
        OffsetDateTime::now_utc(),
    )
    .await;
    assert!(
        matches!(err, Err(party_repo::CreateShareholderError::Database(_))),
        "NULL-coop shareholder must be rejected"
    );

    // Scoped share writes carry the cooperative.
    let (share, _) =
        create_share_at(&db.pool, Some(fx.coop_a), fx.user, modern.shareholder_id).await;
    let share_coop: Option<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM shares WHERE id = $1")
            .bind(share)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(share_coop, Some(fx.coop_a));

    db.cleanup().await;
}
