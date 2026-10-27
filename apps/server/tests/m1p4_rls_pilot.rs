//! M1-P4-P3A/P3B — sealed-RLS pilot on `periods` + `governance_bodies`.
//!
//! These tests connect as the REAL restricted principal
//! `kooperatif_app` (not `SET ROLE`) and prove:
//!
//!   * missing / invalid `app.cooperative_id` fails closed;
//!   * cross-cooperative reads return zero rows, writes are rejected
//!     (`WITH CHECK`), and `cooperative_id` reassignment is denied;
//!   * arbitrary SQL as the runtime role cannot forge or replay the
//!     sealed context (`app_ctx_ok` recomputes an HMAC over
//!     user|session|coop|txid under an owner-only secret);
//!   * `user_sessions` and credential columns are fully isolated behind
//!     the credential-bound SECURITY DEFINER session API;
//!   * rollback + pool reuse never leak the tenant context;
//!   * `kooperatif_app` is not owner/superuser/bypassrls — it cannot
//!     bypass the policies;
//!   * the owner lane (migrations, CLI, fixtures) is unaffected;
//!   * the verified `request_tx` workflow still works as the app role.
//!
//! Requires KOOPERATIF_TEST_DATABASE_URL and a local `kooperatif_app`
//! password (see .env.example — never committed).

use kooperatif_server::db as app_db;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

const APP_ROLE: &str = "kooperatif_app";
/// Restricted-principal password: `KOOPERATIF_APP_TEST_PASSWORD`
/// overrides for CI; otherwise the documented local dev password.
fn app_password() -> String {
    app_db::test_app_role_password()
}

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

/// Rewrites an admin connection URL to the restricted runtime
/// principal (same host/port/database, app credentials).
fn app_url(admin_url: &str) -> String {
    let (scheme, rest) = admin_url.split_once("://").expect("url scheme");
    let at = rest.find('@').expect("userinfo in url");
    format!("{scheme}://{APP_ROLE}:{}{}", app_password(), &rest[at..])
}

struct TestDb {
    _claim: app_db::TestDbClaim,
    pool: PgPool,
    admin_url: String,
    database_name: String,
}

impl TestDb {
    /// A pool connected as the REAL restricted runtime principal.
    async fn app_pool(&self) -> PgPool {
        let base = format!("{}/{}", self.admin_url_base(), self.database_name);
        app_db::connect(&app_url(&base))
            .await
            .expect("kooperatif_app connectivity")
    }

    fn admin_url_base(&self) -> String {
        self.admin_url
            .rsplit_once('/')
            .map(|(base, _)| base.to_string())
            .expect("admin url base")
    }

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

/// Two cooperatives, a member+session each, one pilot row per coop.
/// `user_c` is a second member of coop A — the legitimate target of
/// the privileged session-revocation path.
struct Pilot {
    coop_a: Uuid,
    coop_b: Uuid,
    user_a: Uuid,
    user_c: Uuid,
    token_a: Vec<u8>,
    period_a: Uuid,
    period_b: Uuid,
    body_a: Uuid,
    body_b: Uuid,
}

async fn pilot_fixture(db: &TestDb) -> Pilot {
    let p = Pilot {
        coop_a: Uuid::new_v4(),
        coop_b: Uuid::new_v4(),
        user_a: Uuid::new_v4(),
        user_c: Uuid::new_v4(),
        token_a: vec![0x51u8; 32],
        period_a: Uuid::new_v4(),
        period_b: Uuid::new_v4(),
        body_a: Uuid::new_v4(),
        body_b: Uuid::new_v4(),
    };
    let user_b = Uuid::new_v4();
    let token_b: Vec<u8> = vec![0x52u8; 32];
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, login_verifier) \
         VALUES ($1,$2,'Pilot A','$argon2i$v=19$m=8,t=1,p=1$c29tZXNhbHQ$QzNw3jg9qR1M0eQz2uD+cw', \
                 crypt('pilot-sifre', gen_salt('bf'))), \
                ($3,$4,'Pilot B','$argon2i$v=19$m=8,t=1,p=1$c29tZXNhbHQ$QzNw3jg9qR1M0eQz2uD+cw', \
                 crypt('pilot-sifre', gen_salt('bf'))), \
                ($5,$6,'Pilot C','$argon2i$v=19$m=8,t=1,p=1$c29tZXNhbHQ$QzNw3jg9qR1M0eQz2uD+cw', \
                 crypt('pilot-sifre', gen_salt('bf')))",
    )
    .bind(p.user_a)
    .bind(format!("pa-{}", &p.user_a.to_string()[..8]))
    .bind(user_b)
    .bind(format!("pb-{}", &user_b.to_string()[..8]))
    .bind(p.user_c)
    .bind(format!("pc-{}", &p.user_c.to_string()[..8]))
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO cooperatives (id, name, status, is_bootstrap, business_enabled) \
         VALUES ($1,'Pilot A','active',false,true), ($2,'Pilot B','active',false,true)",
    )
    .bind(p.coop_a)
    .bind(p.coop_b)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         VALUES ($1,$2,'active'), ($3,$4,'active'), ($1,$5,'active')",
    )
    .bind(p.coop_a)
    .bind(p.user_a)
    .bind(p.coop_b)
    .bind(user_b)
    .bind(p.user_c)
    .execute(&db.pool)
    .await
    .unwrap();
    // M1-P6 B4 (0071): each session has switched into its own
    // cooperative — the session's active cooperative is the only
    // stampable scope.
    sqlx::query(
        "INSERT INTO user_sessions \
           (user_id, token_hash, csrf_token, created_at, last_seen_at, expires_at, idle_expires_at, \
            active_cooperative_id) \
         VALUES ($1,$2,'csrf',now(),now(),now() + interval '1 day',now() + interval '1 day',$7), \
                ($3,$4,'csrf',now(),now(),now() + interval '1 day',now() + interval '1 day',$8), \
                ($5,$6,'csrf',now(),now(),now() + interval '1 day',now() + interval '1 day',$7)",
    )
    .bind(p.user_a)
    .bind(&p.token_a)
    .bind(user_b)
    .bind(&token_b)
    .bind(p.user_c)
    .bind(vec![0x53u8; 32])
    .bind(p.coop_a)
    .bind(p.coop_b)
    .execute(&db.pool)
    .await
    .unwrap();
    // user_a holds users.manage in coop A (admin-revocation lane).
    let role_a = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO roles (id, name, cooperative_id, status) \
         VALUES ($1, 'pilot-admin', $2, 'active')",
    )
    .bind(role_a)
    .bind(p.coop_a)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission_id) \
         SELECT $1, id FROM permissions WHERE key = 'users.manage'",
    )
    .bind(role_a)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
         VALUES ($1, $2, $3)",
    )
    .bind(p.user_a)
    .bind(role_a)
    .bind(p.coop_a)
    .execute(&db.pool)
    .await
    .unwrap();

    // Pilot rows, inserted through the owner lane (unaffected by RLS).
    sqlx::query(
        "INSERT INTO periods (id, name, collection_start_date, due_date, cooperative_id, period_number) \
         VALUES ($1,'PA','2026-01-01','2026-01-31',$2,1), \
                ($3,'PB','2026-01-01','2026-01-31',$4,1)",
    )
    .bind(p.period_a)
    .bind(p.coop_a)
    .bind(p.period_b)
    .bind(p.coop_b)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO governance_bodies \
           (id, name, body_type, status, created_by, cooperative_id, body_number, \
            idempotency_key, idempotency_fingerprint) \
         VALUES ($1,'GA','board','active',$2,$3,1,'ga-key','ga-fp'), \
                ($4,'GB','board','active',$5,$6,1,'gb-key','gb-fp')",
    )
    .bind(p.body_a)
    .bind(p.user_a)
    .bind(p.coop_a)
    .bind(p.body_b)
    .bind(user_b)
    .bind(p.coop_b)
    .execute(&db.pool)
    .await
    .unwrap();
    p
}

/// Batch-1 (migration 0040) tables under RLS — all cooperative-owned
/// leaf tables: no other table holds an inbound FK into them, so
/// enabling FORCE RLS cannot break referential checks elsewhere.
const BATCH1_TABLES: [&str; 6] = [
    "income_entries",
    "expense_entries",
    "payment_allocations",
    "shareholder_family_memberships",
    "governance_votes",
    "investment_incomes",
];

/// Row ids of the one batch-1 row seeded per cooperative.
#[derive(Default)]
struct BatchRows {
    income_entry: Uuid,
    expense_entry: Uuid,
    payment_allocation: Uuid,
    family_membership: Uuid,
    governance_vote: Uuid,
    investment_income: Uuid,
}

/// Seed the minimum parent chain plus one row in every batch-1 table
/// for `coop`, all through the owner lane (RLS-unaffected). Returns
/// the batch row ids.
async fn batch_rows(db: &TestDb, coop: Uuid, user: Uuid, tag: &str) -> BatchRows {
    let person: Uuid = sqlx::query_scalar(
        "INSERT INTO persons (first_name, last_name, search_name, cooperative_id) \
         VALUES ($2,'T','t',$1) RETURNING id",
    )
    .bind(coop)
    .bind(format!("P{tag}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let family: Uuid = sqlx::query_scalar(
        "INSERT INTO families (sequence_number, cooperative_id) \
         VALUES ((SELECT COALESCE(max(sequence_number),0)+1 FROM families), $1) RETURNING id",
    )
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let shareholder: Uuid = sqlx::query_scalar(
        "INSERT INTO shareholders (person_id, cooperative_id) VALUES ($1,$2) RETURNING id",
    )
    .bind(person)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let account: Uuid = sqlx::query_scalar(
        "INSERT INTO financial_accounts (name, account_type, created_by, cooperative_id) \
         VALUES ($2,'cash',$3,$1) RETURNING id",
    )
    .bind(coop)
    .bind(format!("Kasa {tag}"))
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let category_income: Uuid = sqlx::query_scalar(
        "INSERT INTO financial_categories (category_type, name, created_by, cooperative_id) \
         VALUES ('income',$2,$3,$1) RETURNING id",
    )
    .bind(coop)
    .bind(format!("Gelir {tag}"))
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let category_expense: Uuid = sqlx::query_scalar(
        "INSERT INTO financial_categories (category_type, name, created_by, cooperative_id) \
         VALUES ('expense',$2,$3,$1) RETURNING id",
    )
    .bind(coop)
    .bind(format!("Gider {tag}"))
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    async fn movement(db: &TestDb, account: Uuid, user: Uuid, coop: Uuid, src: Uuid) -> Uuid {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO account_movements \
             (account_id, direction, amount, source_type, source_id, occurred_at, created_by, cooperative_id) \
             VALUES ($1,'inflow',10,'income',$2,now(),$3,$4) RETURNING id",
        )
        .bind(account)
        .bind(src)
        .bind(user)
        .bind(coop)
        .fetch_one(&db.pool)
        .await
        .unwrap()
    }
    let period: Uuid = sqlx::query_scalar(
        "INSERT INTO periods (name, collection_start_date, due_date, cooperative_id, period_number) \
         VALUES ($2,'2026-01-01','2026-01-31',$1,99) RETURNING id",
    )
    .bind(coop)
    .bind(format!("BP{tag}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let assessment: Uuid = sqlx::query_scalar(
        "INSERT INTO assessments \
         (period_id, shareholder_id, rule_type, base_amount, amount, currency, \
          assessment_effective_date, generated_by, cooperative_id) \
         VALUES ($1,$2,'per_shareholder',10,10,'TRY','2026-01-01',$3,$4) RETURNING id",
    )
    .bind(period)
    .bind(shareholder)
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let payment: Uuid = sqlx::query_scalar(
        "INSERT INTO payments \
         (payer_person_id, amount, method, received_at, idempotency_key, \
          idempotency_fingerprint, created_by, cooperative_id, payment_number) \
         VALUES ($1,10,'cash',now(),$2,$3,$4,$5, \
                 public.allocate_document_number($5,'payments.payment_number')) RETURNING id",
    )
    .bind(person)
    .bind(format!("pay-{tag}"))
    .bind(format!("payfp-{tag}"))
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let investment: Uuid = sqlx::query_scalar(
        "INSERT INTO investments \
         (name, investment_type, idempotency_key, idempotency_fingerprint, created_by, cooperative_id, \
          investment_number) \
         VALUES ($2,'real_estate',$3,$4,$5,$1, \
                 public.allocate_document_number($1,'investments.investment_number')) RETURNING id",
    )
    .bind(coop)
    .bind(format!("Inv {tag}"))
    .bind(format!("inv-{tag}"))
    .bind(format!("invfp-{tag}"))
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let body: Uuid = sqlx::query_scalar(
        "INSERT INTO governance_bodies \
         (name, body_type, status, created_by, cooperative_id, body_number, \
          idempotency_key, idempotency_fingerprint) \
         VALUES ($2,'board','active',$3,$1,50,$4,$5) RETURNING id",
    )
    .bind(coop)
    .bind(format!("BB{tag}"))
    .bind(user)
    .bind(format!("gb-{tag}"))
    .bind(format!("gbfp-{tag}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let membership: Uuid = sqlx::query_scalar(
        "INSERT INTO governance_memberships \
         (body_id, person_id, started_at, idempotency_key, idempotency_fingerprint, \
          created_by, cooperative_id) \
         VALUES ($1,$2,now(),$3,$4,$5,$6) RETURNING id",
    )
    .bind(body)
    .bind(person)
    .bind(format!("gm-{tag}"))
    .bind(format!("gmfp-{tag}"))
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let decision: Uuid = sqlx::query_scalar(
        "INSERT INTO governance_decisions \
         (body_id, title, decision_text, decision_on, idempotency_key, \
          idempotency_fingerprint, created_by, cooperative_id, decision_number) \
         VALUES ($1,'T','Metin','2026-01-01',$2,$3,$4,$5, \
                 public.allocate_document_number($5,'governance_decisions.decision_number')) RETURNING id",
    )
    .bind(body)
    .bind(format!("gd-{tag}"))
    .bind(format!("gdfp-{tag}"))
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let mov_income = movement(db, account, user, coop, Uuid::new_v4()).await;
    let mov_expense = movement(db, account, user, coop, Uuid::new_v4()).await;
    let mov_inv = movement(db, account, user, coop, Uuid::new_v4()).await;

    let income_entry = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO income_entries \
         (id, financial_account_id, category_id, amount, occurred_at, description, \
          account_movement_id, idempotency_key, idempotency_fingerprint, created_by, cooperative_id, \
          income_number) \
         VALUES (gen_random_uuid(),$1,$2,10,now(),'I',$3,$4,$5,$6,$7, \
                 public.allocate_document_number($7,'income_entries.income_number')) RETURNING id",
    )
    .bind(account)
    .bind(category_income)
    .bind(mov_income)
    .bind(format!("ie-{tag}"))
    .bind(format!("iefp-{tag}"))
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let expense_entry = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO expense_entries \
         (id, financial_account_id, category_id, amount, occurred_at, description, \
          account_movement_id, idempotency_key, idempotency_fingerprint, created_by, cooperative_id, \
          expense_number) \
         VALUES (gen_random_uuid(),$1,$2,10,now(),'E',$3,$4,$5,$6,$7, \
                 public.allocate_document_number($7,'expense_entries.expense_number')) RETURNING id",
    )
    .bind(account)
    .bind(category_expense)
    .bind(mov_expense)
    .bind(format!("ee-{tag}"))
    .bind(format!("eefp-{tag}"))
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let payment_allocation = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO payment_allocations \
         (payment_id, assessment_id, amount, created_by, cooperative_id) \
         VALUES ($1,$2,10,$3,$4) RETURNING id",
    )
    .bind(payment)
    .bind(assessment)
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let family_membership = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO shareholder_family_memberships \
         (shareholder_id, family_id, created_by, cooperative_id) \
         VALUES ($1,$2,$3,$4) RETURNING id",
    )
    .bind(shareholder)
    .bind(family)
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let governance_vote = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO governance_votes \
         (decision_id, membership_id, person_id, choice, recorded_by, \
          idempotency_key, idempotency_fingerprint, cooperative_id) \
         VALUES ($1,$2,$3,'approve',$4,$5,$6,$7) RETURNING id",
    )
    .bind(decision)
    .bind(membership)
    .bind(person)
    .bind(user)
    .bind(format!("gv-{tag}"))
    .bind(format!("gvfp-{tag}"))
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let investment_income = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO investment_incomes \
         (investment_id, financial_account_id, amount, occurred_at, description, \
          account_movement_id, idempotency_key, idempotency_fingerprint, created_by, cooperative_id, \
          income_number) \
         VALUES ($1,$2,10,now(),'II',$3,$4,$5,$6,$7, \
                 public.allocate_document_number($7,'investment_incomes.income_number')) RETURNING id",
    )
    .bind(investment)
    .bind(account)
    .bind(mov_inv)
    .bind(format!("ii-{tag}"))
    .bind(format!("iifp-{tag}"))
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    BatchRows {
        income_entry,
        expense_entry,
        payment_allocation,
        family_membership,
        governance_vote,
        investment_income,
    }
}

/// (table, id) pairs of the seeded batch rows — used by the
/// table-parameterized matrix tests.
fn batch_row_ids(rows: &BatchRows) -> Vec<(&'static str, Uuid)> {
    vec![
        ("income_entries", rows.income_entry),
        ("expense_entries", rows.expense_entry),
        ("payment_allocations", rows.payment_allocation),
        ("shareholder_family_memberships", rows.family_membership),
        ("governance_votes", rows.governance_vote),
        ("investment_incomes", rows.investment_income),
    ]
}

async fn pool_conn(pool: &PgPool) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    pool.acquire().await.unwrap()
}

/// Stamp a raw cooperative GUC on an app-role connection — a FORGERY
/// attempt since P3B: without the owner-secret-backed seal chain this
/// must see nothing.
async fn stamp(pool: &PgPool, coop: Uuid) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    let mut conn = pool.acquire().await.unwrap();
    // Begin a tx so the stamp matches request_tx semantics (tx-local).
    sqlx::raw_sql("BEGIN").execute(&mut *conn).await.unwrap();
    sqlx::query("SELECT set_config('app.cooperative_id', $1::text, true)")
        .bind(coop.to_string())
        .execute(&mut *conn)
        .await
        .unwrap();
    conn
}

#[tokio::test]
async fn missing_context_fails_closed() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    // No GUC at all: the runtime sees nothing and cannot write.
    let seen: i64 = sqlx::query_scalar("SELECT count(*) FROM periods")
        .fetch_one(&app)
        .await
        .unwrap();
    assert_eq!(seen, 0, "missing context must fail closed on SELECT");
    let seen_bodies: i64 = sqlx::query_scalar("SELECT count(*) FROM governance_bodies")
        .fetch_one(&app)
        .await
        .unwrap();
    assert_eq!(seen_bodies, 0);

    let insert = sqlx::query(
        "INSERT INTO periods (name, collection_start_date, due_date, cooperative_id, period_number) \
         VALUES ('X','2026-01-01','2026-01-31',$1,9)",
    )
    .bind(p.coop_a)
    .execute(&app)
    .await;
    assert!(
        insert.is_err(),
        "missing context must fail closed on INSERT (WITH CHECK)"
    );

    app.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn invalid_context_fails_closed() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    // A uuid that simply does not exist as a cooperative.
    let mut conn = stamp(&app, Uuid::new_v4()).await;
    let seen: i64 = sqlx::query_scalar("SELECT count(*) FROM periods")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(seen, 0, "unknown cooperative context sees nothing");
    let seen_b: i64 = sqlx::query_scalar("SELECT count(*) FROM governance_bodies")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(seen_b, 0);
    sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await.unwrap();
    drop(conn);
    let _ = p;

    app.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn cross_cooperative_reads_return_no_rows() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    // Legitimate context only through the verified, sealed request_tx.
    let (mut conn, _ctx) =
        app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
            .await
            .expect("sealed request tx");

    let a_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM periods WHERE id = $1")
        .bind(p.period_a)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let b_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM periods WHERE id = $1")
        .bind(p.period_b)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(a_rows, 1);
    assert_eq!(b_rows, 0, "coop A context must not see coop B rows");

    let b_body: i64 = sqlx::query_scalar("SELECT count(*) FROM governance_bodies WHERE id = $1")
        .bind(p.body_b)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(b_body, 0);
    conn.rollback().await.unwrap();

    app.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn cross_cooperative_writes_and_reassignment_rejected() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");

    // INSERT with foreign coop -> WITH CHECK rejects.
    let ins = sqlx::query(
        "INSERT INTO periods (name, collection_start_date, due_date, cooperative_id, period_number) \
         VALUES ('X','2026-01-01','2026-01-31',$1,9)",
    )
    .bind(p.coop_b)
    .execute(&mut *tx)
    .await;
    assert!(ins.is_err(), "cross-coop INSERT must be rejected");
    tx.rollback().await.unwrap();

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    // UPDATE foreign row: invisible under USING -> 0 rows affected.
    let upd = sqlx::query("UPDATE periods SET name = 'H' WHERE id = $1")
        .bind(p.period_b)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(upd.rows_affected(), 0, "foreign UPDATE must hit no rows");
    tx.rollback().await.unwrap();

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    // cooperative_id reassignment of own row -> WITH CHECK rejects.
    let reassign = sqlx::query("UPDATE periods SET cooperative_id = $1 WHERE id = $2")
        .bind(p.coop_b)
        .bind(p.period_a)
        .execute(&mut *tx)
        .await;
    assert!(reassign.is_err(), "cooperative_id reassignment must fail");
    tx.rollback().await.unwrap();

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    // R3 (0054): DELETE is revoked on governance_bodies entirely —
    // the foreign-row probe now dies at the grant layer (42501).
    let del = sqlx::query("DELETE FROM governance_bodies WHERE id = $1")
        .bind(p.body_b)
        .execute(&mut *tx)
        .await;
    assert!(
        matches!(del, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "foreign DELETE must be permission-denied"
    );
    tx.rollback().await.unwrap();

    app.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn request_tx_workflow_still_works_as_app_role() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    // R4: direct periods INSERT is permission-gated — grant the
    // sealed principal the module permission the guard requires.
    r3_grant_perms(&db, p.coop_a, p.user_a, &["periods.manage"]).await;
    let app = db.app_pool().await;

    // The REAL verified path: session-derived context on the app
    // principal sees exactly its own cooperative.
    let (mut tx, ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("request tx");
    assert_eq!(ctx.cooperative_id, p.coop_a);
    let seen: i64 = sqlx::query_scalar("SELECT count(*) FROM periods")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(seen, 1, "request_tx must see exactly its own rows");

    sqlx::query(
        "INSERT INTO periods (name, collection_start_date, due_date, cooperative_id, period_number) \
         VALUES ('P2','2026-02-01','2026-02-28',$1,2)",
    )
    .bind(p.coop_a)
    .execute(&mut *tx)
    .await
    .expect("in-scope insert");
    // R3 (0054): governance_bodies.name left the UPDATE surface (the
    // only write is the active->closed transition) — exercise a column
    // the runtime still legitimately writes: a draft period's name.
    sqlx::query("UPDATE periods SET name = 'PA2' WHERE id = $1")
        .bind(p.period_a)
        .execute(&mut *tx)
        .await
        .expect("in-scope update");
    tx.commit().await.unwrap();

    // Foreign coop hop through the verified path is refused.
    let err = app_db::request_tx(&app, &p.token_a, p.coop_b, OffsetDateTime::now_utc())
        .await
        .expect_err("membership-verified lane cannot hop cooperatives");
    assert!(matches!(err, app_db::RequestContextError::NotMember));

    app.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn rollback_and_connection_reuse_do_not_leak_context() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    let (tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .unwrap();
    drop(tx); // rollback

    // Next checkout of the same pool must start clean.
    let (u, c): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT current_setting('app.user_id', true), \
                current_setting('app.cooperative_id', true)",
    )
    .fetch_one(&app)
    .await
    .unwrap();
    assert!(u.as_deref().is_none_or(|v| v.is_empty()));
    assert!(c.as_deref().is_none_or(|v| v.is_empty()));
    let seen: i64 = sqlx::query_scalar("SELECT count(*) FROM periods")
        .fetch_one(&app)
        .await
        .unwrap();
    assert_eq!(seen, 0, "post-rollback checkout must see nothing");

    app.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn runtime_role_cannot_bypass_rls_and_pilot_is_bounded() {
    let db = setup().await;
    let _p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    // Catalog: the runtime principal holds no bypass attributes and
    // owns no public tables.
    let (superuser, bypassrls, owns): (bool, bool, bool) = sqlx::query_as(
        "SELECT rolsuper, rolbypassrls, \
                EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n \
                        ON n.oid = c.relnamespace \
                        WHERE n.nspname = 'public' AND c.relkind = 'r' \
                          AND c.relowner = 'kooperatif_app'::regrole) \
         FROM pg_roles WHERE rolname = current_user",
    )
    .fetch_one(&app)
    .await
    .unwrap();
    assert!(!superuser && !bypassrls && !owns);

    // Rollout bound: exactly the pilot + batch-1..6 tables carry
    // ENABLE+FORCE row security.
    let pilot: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname FROM pg_class c JOIN pg_namespace n \
         ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relrowsecurity ORDER BY 1",
    )
    .fetch_all(&app)
    .await
    .unwrap();
    assert_eq!(
        pilot,
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
        ]
    );

    app.close().await;
    db.cleanup().await;
}

/// P3B hard-boundary test: arbitrary SQL executed AS `kooperatif_app`
/// can no longer mint a foreign context by setting GUCs — the pilot
/// policies additionally require `app_ctx_ok()`, which recomputes an
/// HMAC seal bound to (user, session, cooperative, transaction id)
/// under an owner-only secret the runtime role cannot read.
#[tokio::test]
async fn forged_guc_context_is_rejected() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    // Direct GUC manipulation no longer mints a usable context.
    let mut conn = stamp(&app, p.coop_b).await;
    let foreign: i64 = sqlx::query_scalar("SELECT count(*) FROM periods WHERE id = $1")
        .bind(p.period_b)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(foreign, 0, "forged cooperative GUC must not see rows");
    sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await.unwrap();
    drop(conn);

    // Forging EVERY context GUC including a fabricated seal still
    // fails: the recomputation uses the owner-only secret.
    let mut conn = pool_conn(&app).await;
    sqlx::raw_sql("BEGIN").execute(&mut *conn).await.unwrap();
    for (key, value) in [
        ("app.user_id", p.user_a.to_string()),
        ("app.session_id", Uuid::new_v4().to_string()),
        ("app.cooperative_id", p.coop_b.to_string()),
        ("app.ctx_txid", "1".to_string()),
        ("app.user_seal", "f".repeat(64)),
        ("app.ctx_seal", "f".repeat(64)),
    ] {
        sqlx::query("SELECT set_config($1, $2, true)")
            .bind(key)
            .bind(value)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let foreign: i64 = sqlx::query_scalar("SELECT count(*) FROM periods WHERE id = $1")
        .bind(p.period_b)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(foreign, 0, "fully forged GUC set must see nothing");
    sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await.unwrap();
    drop(conn);

    app.close().await;
    db.cleanup().await;
}

/// A captured seal cannot be replayed: it binds the minting
/// transaction id (txid_current), which differs in any later tx —
/// even on the same pooled connection.
#[tokio::test]
async fn seal_replay_across_transactions_is_rejected() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    // Legitimate request tx — capture every stamped GUC value.
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    let gucs: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT key, value FROM unnest(\
             ARRAY['app.user_id','app.session_id','app.cooperative_id',\
                   'app.ctx_txid','app.user_seal','app.ctx_seal']) AS key,\
             LATERAL (SELECT current_setting(key, true) AS value) v",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    let stolen: std::collections::HashMap<String, String> = gucs
        .into_iter()
        .map(|(k, v)| (k, v.unwrap_or_default()))
        .collect();
    assert_eq!(stolen["app.ctx_seal"].len(), 64, "seal must be stamped");
    tx.commit().await.unwrap();

    // Replay the ENTIRE stolen context on a fresh connection — the txid
    // component differs, so the recomputed seal never matches.
    let mut conn = pool_conn(&app).await;
    sqlx::raw_sql("BEGIN").execute(&mut *conn).await.unwrap();
    for (key, value) in &stolen {
        sqlx::query("SELECT set_config($1, $2, true)")
            .bind(key)
            .bind(value)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let seen: i64 = sqlx::query_scalar("SELECT count(*) FROM periods WHERE id = $1")
        .bind(p.period_a)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(seen, 0, "a replayed seal must fail — txid mismatch");
    sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await.unwrap();
    drop(conn);

    app.close().await;
    db.cleanup().await;
}

/// Forged user/session GUCs cannot drive `app_stamp_cooperative`: the
/// user-seal recomputation fails before membership is even consulted.
#[tokio::test]
async fn forged_user_context_cannot_stamp_cooperative() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    let mut conn = pool_conn(&app).await;
    sqlx::raw_sql("BEGIN").execute(&mut *conn).await.unwrap();
    for (key, value) in [
        ("app.user_id", p.user_a.to_string()),
        ("app.session_id", Uuid::new_v4().to_string()),
    ] {
        sqlx::query("SELECT set_config($1, $2, true)")
            .bind(key)
            .bind(value)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM public.app_stamp_cooperative($1)")
        .bind(p.coop_a)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(rows, 0, "stamp must refuse a forged user context");
    sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await.unwrap();
    drop(conn);

    app.close().await;
    db.cleanup().await;
}

/// Session credential isolation: the runtime role cannot enumerate
/// `user_sessions`, cannot read credential columns on `users`, cannot
/// read `app_secrets`, and cannot EXECUTE the seal helper.
#[tokio::test]
async fn session_credentials_and_seal_secret_are_isolated() {
    let db = setup().await;
    let _p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    let sessions = sqlx::query("SELECT token_hash FROM user_sessions LIMIT 1")
        .fetch_optional(&app)
        .await;
    assert!(sessions.is_err(), "user_sessions must be unreadable");
    let sessions2 = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM user_sessions")
        .fetch_one(&app)
        .await;
    assert!(sessions2.is_err(), "no session-table access at all");

    let creds = sqlx::query("SELECT password_hash FROM users LIMIT 1")
        .fetch_optional(&app)
        .await;
    assert!(creds.is_err(), "password_hash must be column-invisible");
    let verifier = sqlx::query("SELECT login_verifier FROM users LIMIT 1")
        .fetch_optional(&app)
        .await;
    assert!(verifier.is_err(), "login_verifier must be column-invisible");

    let secrets = sqlx::query("SELECT value FROM app_secrets")
        .fetch_optional(&app)
        .await;
    assert!(secrets.is_err(), "app_secrets must stay owner-only");

    let seal_call =
        sqlx::query_scalar::<_, Option<String>>("SELECT public.app_seal_of($1,$2,$3,1)")
            .bind(Uuid::new_v4())
            .bind(Uuid::new_v4())
            .bind(Uuid::new_v4())
            .fetch_one(&app)
            .await;
    assert!(seal_call.is_err(), "app_seal_of must not be callable");

    // The granted session API remains available but never exposes the
    // hash: resolve returns a bounded projection.
    let resolved: Vec<(Option<String>, Option<Uuid>)> =
        sqlx::query_as("SELECT rejection, user_id FROM public.app_session_resolve($1, now())")
            .bind(vec![0x51u8; 32])
            .fetch_all(&app)
            .await
            .unwrap();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].0.as_deref(), None, "valid session resolves");

    app.close().await;
    db.cleanup().await;
}

/// Credential-bound login through `app_session_open`: a session can
/// only be created with the actual password — the runtime role cannot
/// fabricate sessions for a user it does not control.
#[tokio::test]
async fn session_open_requires_real_credentials() {
    let db = setup().await;
    let _p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    // The fixture user is username 'pa-<id8>'; reconstruct through the
    // owner lane — R2 closed unsealed users reads entirely (RLS), the
    // login definer itself needs no directory access.
    let username: String = sqlx::query_scalar("SELECT username FROM users LIMIT 1")
        .fetch_one(&db.pool)
        .await
        .unwrap();

    let wrong: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.app_session_open(\
             $1, 'yanlis-sifre', '\\x00'::bytea, 'c', now(), 3600, 600, NULL)",
    )
    .bind(&username)
    .fetch_one(&app)
    .await
    .unwrap();
    assert_eq!(wrong, 0, "wrong password must not open a session");

    let ok: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.app_session_open(\
             $1, 'pilot-sifre', '\\x00'::bytea, 'c', now(), 3600, 600, NULL)",
    )
    .bind(&username)
    .fetch_one(&app)
    .await
    .unwrap();
    assert_eq!(ok, 1, "correct password opens the session");

    let unknown: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.app_session_open(\
             'yok-kullanici', 'pilot-sifre', '\\x00'::bytea, 'c', now(), 3600, 600, NULL)",
    )
    .fetch_one(&app)
    .await
    .unwrap();
    assert_eq!(unknown, 0, "unknown user opens nothing");

    app.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn owner_lane_is_unaffected() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let _ = &p;

    // The owner principal (migrations, CLI, test fixtures) sees every
    // cooperative's rows under FORCE via the owner policy.
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM periods")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(total, 2);
    let total_b: i64 = sqlx::query_scalar("SELECT count(*) FROM governance_bodies")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(total_b, 2);

    db.cleanup().await;
}

/// P3C §3 — same-transaction isolation: a captured/legitimate context
/// cannot be transferred, overwritten or re-minted inside the SAME
/// transaction to reach foreign cooperative data.
#[tokio::test]
async fn same_transaction_context_tampering_is_rejected() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");

    // Overwrite the cooperative GUC mid-transaction → the seal no
    // longer matches the stamped cooperative: NOTHING is readable,
    // including the originally-authorized rows.
    sqlx::query("SELECT set_config('app.cooperative_id', $1::text, true)")
        .bind(p.coop_b.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let seen: i64 = sqlx::query_scalar("SELECT count(*) FROM periods")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(seen, 0, "coop overwrite in the same tx must fail closed");

    // Overwrite the seal itself → still nothing.
    sqlx::query("SELECT set_config('app.ctx_seal', 'ff00ff00', true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.cooperative_id', $1::text, true)")
        .bind(p.coop_a.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let seen: i64 = sqlx::query_scalar("SELECT count(*) FROM periods")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(seen, 0, "seal overwrite must fail closed");

    // Overwrite the user identity mid-transaction → user seal mismatch
    // poisons the whole context.
    sqlx::query("SELECT set_config('app.user_id', $1::text, true)")
        .bind(p.user_c.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let seen: i64 = sqlx::query_scalar("SELECT count(*) FROM periods")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(seen, 0, "user overwrite must fail closed");
    tx.rollback().await.unwrap();

    // A fresh sealed tx, then a re-stamp attempt for a foreign coop:
    // refused BEFORE membership runs (the user is not a member of B),
    // and the original context is left intact.
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM public.app_stamp_cooperative($1)")
        .bind(p.coop_b)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(rows, 0, "re-stamp for a non-member coop must refuse");
    let own: i64 = sqlx::query_scalar("SELECT count(*) FROM periods WHERE id = $1")
        .bind(p.period_a)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(own, 1, "legitimate context survives a refused re-stamp");
    tx.rollback().await.unwrap();

    app.close().await;
    db.cleanup().await;
}

/// P3C §1 — `app_admin_revoke_user_sessions` now requires a sealed
/// administrative context: raw invocation, forged context and
/// insufficient permission all raise insufficient_privilege; the
/// legitimate sealed users.manage path still revokes.
#[tokio::test]
async fn admin_revoke_requires_sealed_administrative_context() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    // Raw arbitrary SQL — no stamped context at all.
    let raw = sqlx::query_scalar::<_, i64>("SELECT public.app_admin_revoke_user_sessions($1, $2)")
        .bind(p.user_c)
        .bind(now)
        .fetch_one(&app)
        .await;
    assert!(raw.is_err(), "unauthenticated admin revoke must fail");

    // Forged GUCs (no valid seal) → refused.
    let mut conn = pool_conn(&app).await;
    sqlx::raw_sql("BEGIN").execute(&mut *conn).await.unwrap();
    sqlx::query("SELECT set_config('app.user_id', $1::text, true)")
        .bind(p.user_a.to_string())
        .execute(&mut *conn)
        .await
        .unwrap();
    let forged =
        sqlx::query_scalar::<_, i64>("SELECT public.app_admin_revoke_user_sessions($1, $2)")
            .bind(p.user_c)
            .bind(now)
            .fetch_one(&mut *conn)
            .await;
    assert!(forged.is_err(), "forged-context admin revoke must fail");
    sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await.unwrap();
    drop(conn);

    // Sealed request tx WITHOUT users.manage: user_c has membership
    // but no role → refused.
    let (mut tx, _ctx) = app_db::request_tx(&app, &[0x53u8; 32], p.coop_a, now)
        .await
        .expect("member but non-admin sealed tx");
    let denied =
        sqlx::query_scalar::<_, i64>("SELECT public.app_admin_revoke_user_sessions($1, $2)")
            .bind(p.user_a)
            .bind(now)
            .fetch_one(&mut *tx)
            .await;
    assert!(denied.is_err(), "non-admin sealed context must be denied");
    tx.rollback().await.unwrap();

    // Sealed users.manage context, but target outside the scope:
    // user_b is only a member of coop B → refused (and the raising
    // call aborts the tx, so the legitimate path uses a fresh one).
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("admin sealed tx");
    let oob = sqlx::query_scalar::<_, i64>("SELECT public.app_admin_revoke_user_sessions($1, $2)")
        .bind(sqlx::types::Uuid::nil())
        .bind(now)
        .fetch_one(&mut *tx)
        .await;
    assert!(oob.is_err(), "out-of-scope target must be denied");
    tx.rollback().await.unwrap();

    // Legitimate: admin of coop A revokes coop-A member's sessions.
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("admin sealed tx");
    let revoked: i64 = sqlx::query_scalar("SELECT public.app_admin_revoke_user_sessions($1, $2)")
        .bind(p.user_c)
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .expect("sealed admin revoke");
    assert_eq!(revoked, 1, "admin revoke must hit the member's session");
    tx.commit().await.unwrap();

    app.close().await;
    db.cleanup().await;
}

/// P3C §2 — the login definer enforces a failure window under the app
/// clock, serialized by a per-username advisory lock: after the
/// verdict cap, further attempts (even correct credentials) return the
/// uniform empty set, throttle rows are audited but excluded from the
/// counter, and the window expires to restore access.
#[tokio::test]
async fn login_throttle_blocks_excessive_attempts_and_recovers() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();
    let username = format!("pa-{}", &p.user_a.to_string()[..8]);

    for i in 0..10 {
        let rows: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM public.app_session_open(\
                 $1, 'yanlis', '\\x00'::bytea, 'c', $2, 3600, 600, NULL)",
        )
        .bind(&username)
        .bind(now)
        .fetch_one(&app)
        .await
        .unwrap();
        assert_eq!(rows, 0, "attempt {i} must be a uniform failure");
    }

    // 11th attempt — even the CORRECT password is refused inside the
    // window (consistent with the app-side limiter semantics).
    let blocked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.app_session_open(\
             $1, 'pilot-sifre', '\\x00'::bytea, 'c', $2, 3600, 600, NULL)",
    )
    .bind(&username)
    .bind(now)
    .fetch_one(&app)
    .await
    .unwrap();
    assert_eq!(blocked, 0, "throttled window must deny even valid creds");

    // Throttled attempts are audited but excluded from the counter —
    // a flood cannot extend its own lockout beyond the window.
    let throttled: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events \
         WHERE event_type = 'login_failed' \
           AND metadata ->> 'reason' = 'throttled' \
           AND metadata ->> 'username' = $1",
    )
    .bind(&username)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(throttled, 1);

    // Window expiry restores access (no permanent lockout).
    let recovered: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.app_session_open(\
             $1, 'pilot-sifre', '\\x00'::bytea, 'c', $2, 3600, 600, NULL)",
    )
    .bind(&username)
    .bind(now + time::Duration::seconds(301))
    .fetch_one(&app)
    .await
    .unwrap();
    assert_eq!(recovered, 1, "window expiry must restore access");

    app.close().await;
    db.cleanup().await;
}

/// P3C §3 — full granted-function abuse surface: no function granted
/// to `kooperatif_app` may mint, transfer or act on an unauthorized
/// context, and the numbering allocators (now SECURITY DEFINER)
/// require the sealed cooperative to match the requested scope.
#[tokio::test]
async fn granted_function_surface_cannot_act_without_authorization() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    // Session API with a foreign/garbage token hash → uniform refusal,
    // never a hash echo or another user's data. `app_session_resolve`
    // answers with a rejection discriminator, all fields NULL.
    let resolved: Option<String> = sqlx::query_scalar(
        "SELECT rejection FROM public.app_session_resolve('\\xDEAD'::bytea, $1)",
    )
    .bind(now)
    .fetch_one(&app)
    .await
    .unwrap();
    assert_eq!(resolved.as_deref(), Some("unknown"));
    let listed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.app_session_list('\\xDEAD'::bytea, $1)")
            .bind(now)
            .fetch_one(&app)
            .await
            .unwrap();
    assert_eq!(listed, 0);
    let revoked: String =
        sqlx::query_scalar("SELECT public.app_session_revoke('\\xDEAD'::bytea, $1, $2, 'x')")
            .bind(Uuid::new_v4())
            .bind(now)
            .fetch_one(&app)
            .await
            .unwrap();
    assert_eq!(revoked, "not_found");
    let switched: bool =
        sqlx::query_scalar("SELECT public.app_session_set_cooperative('\\xDEAD'::bytea, $1)")
            .bind(p.coop_b)
            .fetch_one(&app)
            .await
            .unwrap();
    assert!(!switched);

    // The membership resolver is no longer directly executable.
    let direct =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM public.app_verify_membership($1, $2)")
            .bind(p.user_a)
            .bind(p.coop_a)
            .fetch_one(&app)
            .await;
    assert!(direct.is_err(), "verify_membership must not be executable");

    // Numbering allocators without a sealed context → denied.
    let alloc = sqlx::query_scalar::<_, i64>(
        "SELECT public.allocate_document_number($1, 'payments.payment_number')",
    )
    .bind(p.coop_a)
    .fetch_one(&app)
    .await;
    assert!(alloc.is_err(), "unsealed allocation must be denied");
    let reserve = sqlx::query_scalar::<_, i64>(
        "SELECT public.reserve_document_number($1, 'payments.payment_number', 9)",
    )
    .bind(p.coop_a)
    .fetch_one(&app)
    .await;
    assert!(reserve.is_err(), "unsealed reservation must be denied");

    // Inside a legitimately sealed tx for coop A, allocating for coop A
    // works — but allocating for coop B is refused (scope mismatch).
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("sealed request tx");
    let own: i64 =
        sqlx::query_scalar("SELECT public.allocate_document_number($1, 'payments.payment_number')")
            .bind(p.coop_a)
            .fetch_one(&mut *tx)
            .await
            .expect("sealed same-scope allocation");
    assert!(own >= 1);
    let foreign = sqlx::query_scalar::<_, i64>(
        "SELECT public.allocate_document_number($1, 'payments.payment_number')",
    )
    .bind(p.coop_b)
    .fetch_one(&mut *tx)
    .await;
    assert!(foreign.is_err(), "foreign-scope allocation must be denied");
    tx.rollback().await.unwrap();

    // Direct counter-table manipulation is closed at the grant level —
    // and, since S1 (0046), additionally by FORCE RLS (proven in the
    // s1_* tests by temporarily granting the privileges owner-side).
    let counter = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM cooperative_number_counters")
        .fetch_one(&app)
        .await;
    assert!(counter.is_err(), "counter table must not be readable");

    app.close().await;
    db.cleanup().await;
}

/// P3C §2 — concurrent direct-SQL login attempts on one username are
/// serialized by the advisory lock: every attempt is counted, the
/// verdict cap still holds, and no session opens.
#[tokio::test]
async fn concurrent_login_attempts_stay_under_the_cap() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();
    let username = format!("pa-{}", &p.user_a.to_string()[..8]);

    let mut handles = Vec::new();
    for _ in 0..15 {
        let pool = app.clone();
        let uname = username.clone();
        handles.push(tokio::spawn(async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM public.app_session_open(                     $1, 'yanlis', '\\x00'::bytea, 'c', $2, 3600, 600, NULL)",
            )
            .bind(uname)
            .bind(now)
            .fetch_one(&pool)
            .await
            .unwrap()
        }));
    }
    for h in handles {
        assert_eq!(h.await.unwrap(), 0, "every attempt must fail");
    }

    // After the parallel flood the verdict cap is exhausted — a correct
    // password inside the window is still refused.
    let blocked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.app_session_open(             $1, 'pilot-sifre', '\\x00'::bytea, 'c', $2, 3600, 600, NULL)",
    )
    .bind(&username)
    .bind(now)
    .fetch_one(&app)
    .await
    .unwrap();
    assert_eq!(blocked, 0);

    // Exactly 10 verdict failures + 5 throttled rows — serialized
    // counting under the advisory lock, no double-count.
    let verdicts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events          WHERE event_type = 'login_failed'            AND metadata ->> 'username' = $1            AND metadata ->> 'reason' = 'invalid_credentials'",
    )
    .bind(&username)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let throttled: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events          WHERE event_type = 'login_failed'            AND metadata ->> 'username' = $1            AND metadata ->> 'reason' = 'throttled'",
    )
    .bind(&username)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(verdicts, 10, "exactly the verdict cap must be counted");
    // 5 flood overflows + the blocked correct-password attempt above.
    assert_eq!(throttled, 6, "the overflow must be audited as throttled");

    app.close().await;
    db.cleanup().await;
}

// ---------------------------------------------------------------------
// M1-P4 Batch 1 (migration 0040) — sealed-RLS fleet rollout on six
// cooperative-owned leaf tables:
//   income_entries, expense_entries, payment_allocations,
//   shareholder_family_memberships, governance_votes, investment_incomes
// ---------------------------------------------------------------------

/// Batch 1: every selected table fails closed when no sealed context
/// exists — zero rows on SELECT, WITH CHECK rejection on INSERT.
#[tokio::test]
async fn batch1_missing_context_fails_closed() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let _rows_a = batch_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    for table in BATCH1_TABLES {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&app)
            .await
            .unwrap();
        assert_eq!(seen, 0, "{table}: unauthenticated SELECT must see nothing");
    }
    // A direct INSERT must hit WITH CHECK (row-level security error),
    // not a foreign-key or not-null complaint.
    let err = sqlx::query(
        "INSERT INTO shareholder_family_memberships \
         (shareholder_id, family_id, created_by, cooperative_id) \
         VALUES (gen_random_uuid(), gen_random_uuid(), $1, $2)",
    )
    .bind(p.user_a)
    .bind(p.coop_a)
    .execute(&app)
    .await
    .expect_err("unauthenticated INSERT must be denied");
    let msg = err.to_string();
    assert!(
        msg.contains("row-level security")
            || msg.contains("row_security")
            || msg.contains("insufficient_privilege"),
        "INSERT must fail on the RLS WITH CHECK, got: {msg}"
    );
    app.close().await;
    db.cleanup().await;
}

/// Batch 1: a raw `set_config` stamp (forge attempt — no seal chain)
/// must still see zero rows on every batch table.
#[tokio::test]
async fn batch1_forged_guc_context_sees_nothing() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    let mut conn = stamp(&app, p.coop_a).await;
    for (table, row) in batch_row_ids(&rows_a) {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(row)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(
            seen, 0,
            "{table}: unsealed GUC stamp must not satisfy the policy"
        );
    }
    sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await.unwrap();
    drop(conn);
    app.close().await;
    db.cleanup().await;
}

/// Batch 1: the sealed request transaction sees exactly its own
/// cooperative's rows on every batch table — never the foreign coop's.
#[tokio::test]
async fn batch1_cross_cooperative_reads_are_isolated() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch_rows(&db, p.coop_a, p.user_a, "a").await;
    let rows_b = batch_rows(&db, p.coop_b, p.user_c, "b").await;
    let app = db.app_pool().await;

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    for ((table, a_row), (_, b_row)) in batch_row_ids(&rows_a)
        .into_iter()
        .zip(batch_row_ids(&rows_b))
    {
        let own: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(a_row)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(own, 1, "{table}: own row must be visible");
        let foreign: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
                .bind(b_row)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(foreign, 0, "{table}: foreign row must be invisible");
    }
    tx.rollback().await.unwrap();
    app.close().await;
    db.cleanup().await;
}

/// Batch 1: cross-coop writes are rejected — foreign INSERT hits
/// WITH CHECK, foreign UPDATE/DELETE hit zero rows, and
/// cooperative_id reassignment on an own row is denied.
#[tokio::test]
async fn batch1_cross_cooperative_writes_and_reassignment_rejected() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch_rows(&db, p.coop_a, p.user_a, "a").await;
    let rows_b = batch_rows(&db, p.coop_b, p.user_c, "b").await;
    let app = db.app_pool().await;

    // Foreign UPDATE/DELETE must affect zero rows (USING hides them).
    for ((table, _), (_, b_row)) in batch_row_ids(&rows_a)
        .into_iter()
        .zip(batch_row_ids(&rows_b))
    {
        // R3 (0054): cooperative_id left the runtime UPDATE surface and
        // DELETE was revoked on financial/history tables — the writes
        // now die at the grant layer (42501) before row visibility is
        // even consulted. A privilege error aborts the tx, so each
        // probe runs in its own sealed request tx.
        let upd = {
            let (mut tx, _c) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!(
                "UPDATE {table} SET cooperative_id = cooperative_id WHERE id = $1"
            ))
            .bind(b_row)
            .execute(&mut *tx)
            .await
        };
        assert!(
            matches!(upd, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{table}: UPDATE must be denied at the grant layer"
        );
        let del = {
            let (mut tx, _c) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!("DELETE FROM {table} WHERE id = $1"))
                .bind(b_row)
                .execute(&mut *tx)
                .await
        };
        match del {
            Ok(_) => panic!("{table}: DELETE must be denied — grant revoked in 0054"),
            Err(sqlx::Error::Database(ref e)) => assert_eq!(
                e.code().as_deref(),
                Some("42501"),
                "{table}: DELETE must be permission-denied"
            ),
            Err(e) => panic!("{table}: unexpected DELETE error {e}"),
        }
        let reassign = {
            let (mut tx, _c) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!(
                "UPDATE {table} SET cooperative_id = $1 WHERE id = $2"
            ))
            .bind(p.coop_b)
            .bind(a_row_of(table, &rows_a))
            .execute(&mut *tx)
            .await
        };
        assert!(
            matches!(reassign, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{table}: reassignment must fail"
        );
    }
    app.close().await;
    db.cleanup().await;
}

fn a_row_of(table: &str, rows: &BatchRows) -> Uuid {
    batch_row_ids(rows)
        .into_iter()
        .find(|(t, _)| *t == table)
        .map(|(_, id)| id)
        .unwrap()
}

/// Batch 1: a legitimate sealed-context workflow still works —
/// SELECT own row + INSERT own row + UPDATE own row, all committed.
#[tokio::test]
async fn batch1_legitimate_workflow_still_works() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    r3_grant_perms(&db, p.coop_a, p.user_a, &["shareholders.manage"]).await;
    let rows_a = batch_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    for (table, row) in batch_row_ids(&rows_a) {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(row)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(seen, 1, "{table}: own row must be visible");
        // Touch update on the own row must pass WITH CHECK.
        // R3 (0054): the attribution column left the runtime UPDATE
        // surface — even in-scope, a cooperative_id write is now a
        // grant-layer error. Probe in its own tx (a privilege error
        // aborts the transaction).
        let touch = {
            let (mut t2, _c2) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!(
                "UPDATE {table} SET cooperative_id = cooperative_id WHERE id = $1"
            ))
            .bind(row)
            .execute(&mut *t2)
            .await
        };
        assert!(
            matches!(touch, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{table}: attribution column must not be writable"
        );
    }
    // A fresh own-scope INSERT into a batch table (family membership
    // history row needs a new shareholder to satisfy no-overlap).
    let person: Uuid = sqlx::query_scalar(
        "INSERT INTO persons (first_name, last_name, search_name, cooperative_id) \
         VALUES ('W','T','wt',$1) RETURNING id",
    )
    .bind(p.coop_a)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let sh: Uuid = sqlx::query_scalar(
        "INSERT INTO shareholders (person_id, cooperative_id) VALUES ($1,$2) RETURNING id",
    )
    .bind(person)
    .bind(p.coop_a)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO shareholder_family_memberships \
         (shareholder_id, family_id, created_by, cooperative_id) \
         SELECT $1, id, $2, $3 FROM families WHERE cooperative_id = $3 LIMIT 1",
    )
    .bind(sh)
    .bind(p.user_a)
    .bind(p.coop_a)
    .execute(&mut *tx)
    .await
    .expect("in-scope batch INSERT must succeed");
    tx.commit().await.unwrap();

    // The committed own rows remain visible on a new sealed tx.
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM shareholder_family_memberships WHERE cooperative_id = $1",
    )
    .bind(p.coop_a)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(count, 2);
    tx.rollback().await.unwrap();
    app.close().await;
    db.cleanup().await;
}

/// Batch 1: rollback and pooled-connection reuse never leak the
/// sealed context into a later checkout.
#[tokio::test]
async fn batch1_rollback_and_reuse_do_not_leak() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    let (tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .unwrap();
    drop(tx); // rollback

    for (table, row) in batch_row_ids(&rows_a) {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(row)
            .fetch_one(&app)
            .await
            .unwrap();
        assert_eq!(seen, 0, "{table}: post-rollback checkout must see nothing");
    }
    app.close().await;
    db.cleanup().await;
}

// ---------------------------------------------------------------------
// M1-P4 Batch 2 (migration 0041) — sealed-RLS rollout on the
// receivables/collection cluster:
//   payments, assessments, assessment_rules, assessment_share_sources,
//   shareholder_credits, credit_applications
// ---------------------------------------------------------------------

/// Batch-2 (migration 0041) tables under RLS — the receivables /
/// collection cluster. `periods` (pilot) and `payment_allocations`
/// (batch 1) were already forced; this closes the chain end-to-end.
const BATCH2_TABLES: [&str; 6] = [
    "payments",
    "assessments",
    "assessment_rules",
    "assessment_share_sources",
    "shareholder_credits",
    "credit_applications",
];

/// Row ids of the one batch-2 row seeded per cooperative.
struct Batch2Rows {
    payment: Uuid,
    assessment: Uuid,
    rule: Uuid,
    share_source: Uuid,
    credit: Uuid,
    credit_application: Uuid,
}

/// Seed the minimum parent chain plus one row in every batch-2 table
/// for `coop`, all through the owner lane (RLS-unaffected).
async fn batch2_rows(db: &TestDb, coop: Uuid, user: Uuid, tag: &str) -> Batch2Rows {
    let person: Uuid = sqlx::query_scalar(
        "INSERT INTO persons (first_name, last_name, search_name, cooperative_id) \
         VALUES ($2,'T','t',$1) RETURNING id",
    )
    .bind(coop)
    .bind(format!("P2{tag}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let shareholder: Uuid = sqlx::query_scalar(
        "INSERT INTO shareholders (person_id, cooperative_id) VALUES ($1,$2) RETURNING id",
    )
    .bind(person)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let share: Uuid = sqlx::query_scalar(
        "INSERT INTO shares (cooperative_id, share_number, created_by) \
         VALUES ($1, public.allocate_document_number($1,'shares.share_number'), $2) RETURNING id",
    )
    .bind(coop)
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let share_event: Uuid = sqlx::query_scalar(
        "INSERT INTO share_events \
         (share_id, event_type, occurred_at, to_shareholder_id, acquisition_type, \
          actor_user_id, cooperative_id) \
         VALUES ($1,'initial_acquisition',now(),$2,'founder',$3,$4) RETURNING id",
    )
    .bind(share)
    .bind(shareholder)
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let ownership: Uuid = sqlx::query_scalar(
        "INSERT INTO share_ownerships \
         (share_id, shareholder_id, started_at, acquisition_type, source_event_id, \
          created_by, cooperative_id) \
         VALUES ($1,$2,now(),'founder',$3,$4,$5) RETURNING id",
    )
    .bind(share)
    .bind(shareholder)
    .bind(share_event)
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let period: Uuid = sqlx::query_scalar(
        "INSERT INTO periods (name, collection_start_date, due_date, cooperative_id, period_number) \
         VALUES ($2,'2026-03-01','2026-03-31',$1,77) RETURNING id",
    )
    .bind(coop)
    .bind(format!("B2P{tag}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let rule: Uuid = sqlx::query_scalar(
        "INSERT INTO assessment_rules \
         (period_id, rule_type, base_amount, assessment_effective_date, cooperative_id) \
         VALUES ($1,'per_share',10,'2026-03-01',$2) RETURNING id",
    )
    .bind(period)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let assessment: Uuid = sqlx::query_scalar(
        "INSERT INTO assessments \
         (period_id, shareholder_id, rule_type, base_amount, amount, currency, \
          assessment_effective_date, generated_by, cooperative_id) \
         VALUES ($1,$2,'per_share',10,10,'TRY','2026-03-01',$3,$4) RETURNING id",
    )
    .bind(period)
    .bind(shareholder)
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let share_source: Uuid = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO assessment_share_sources \
         (assessment_id, share_id, ownership_id, amount_component, cooperative_id) \
         VALUES ($1,$2,$3,10,$4) RETURNING assessment_id",
    )
    .bind(assessment)
    .bind(share)
    .bind(ownership)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let payment: Uuid = sqlx::query_scalar(
        "INSERT INTO payments \
         (payer_person_id, amount, method, received_at, idempotency_key, \
          idempotency_fingerprint, created_by, cooperative_id, payment_number) \
         VALUES ($1,20,'cash',now(),$2,$3,$4,$5, \
                 public.allocate_document_number($5,'payments.payment_number')) RETURNING id",
    )
    .bind(person)
    .bind(format!("b2pay-{tag}"))
    .bind(format!("b2payfp-{tag}"))
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let credit: Uuid = sqlx::query_scalar(
        "INSERT INTO shareholder_credits \
         (source_payment_id, shareholder_id, amount, idempotency_key, \
          idempotency_fingerprint, created_by, cooperative_id, credit_number) \
         VALUES ($1,$2,5,$3,$4,$5,$6, \
                 public.allocate_document_number($6,'shareholder_credits.credit_number')) RETURNING id",
    )
    .bind(payment)
    .bind(shareholder)
    .bind(format!("b2cr-{tag}"))
    .bind(format!("b2crfp-{tag}"))
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let credit_application: Uuid = sqlx::query_scalar(
        "INSERT INTO credit_applications \
         (credit_id, assessment_id, amount, mode, idempotency_key, \
          idempotency_fingerprint, created_by, cooperative_id) \
         VALUES ($1,$2,5,'manual',$3,$4,$5,$6) RETURNING id",
    )
    .bind(credit)
    .bind(assessment)
    .bind(format!("b2ca-{tag}"))
    .bind(format!("b2cafp-{tag}"))
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    Batch2Rows {
        payment,
        assessment,
        rule,
        share_source,
        credit,
        credit_application,
    }
}

/// (table, id) pairs of the seeded batch-2 rows. `assessment_share_sources`
/// has a composite PK — identified by assessment_id.
fn batch2_row_ids(rows: &Batch2Rows) -> Vec<(&'static str, Uuid)> {
    vec![
        ("payments", rows.payment),
        ("assessments", rows.assessment),
        ("assessment_rules", rows.rule),
        ("assessment_share_sources", rows.share_source),
        ("shareholder_credits", rows.credit),
        ("credit_applications", rows.credit_application),
    ]
}

/// Batch 2: every selected table fails closed when no sealed context
/// exists — zero rows on SELECT, WITH CHECK rejection on INSERT.
#[tokio::test]
async fn batch2_missing_context_fails_closed() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let _rows_a = batch2_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    for table in BATCH2_TABLES {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&app)
            .await
            .unwrap();
        assert_eq!(seen, 0, "{table}: unauthenticated SELECT must see nothing");
    }
    let err = sqlx::query(
        "INSERT INTO assessment_rules \
         (period_id, rule_type, base_amount, assessment_effective_date, cooperative_id) \
         VALUES (gen_random_uuid(),'per_shareholder',10,'2026-01-01',$1)",
    )
    .bind(p.coop_a)
    .execute(&app)
    .await
    .expect_err("unauthenticated INSERT must be denied");
    let msg = err.to_string();
    assert!(
        msg.contains("row-level security")
            || msg.contains("row_security")
            || msg.contains("insufficient_privilege"),
        "INSERT must fail on the RLS WITH CHECK, got: {msg}"
    );
    app.close().await;
    db.cleanup().await;
}

/// Batch 2: a raw `set_config` stamp (forge attempt — no seal chain)
/// must still see zero rows on every batch table.
#[tokio::test]
async fn batch2_forged_guc_context_sees_nothing() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch2_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    let mut conn = stamp(&app, p.coop_a).await;
    for (table, row) in batch2_row_ids(&rows_a) {
        let id_col = if table == "assessment_share_sources" {
            "assessment_id"
        } else {
            "id"
        };
        let seen: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE {id_col} = $1"))
                .bind(row)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(
            seen, 0,
            "{table}: unsealed GUC stamp must not satisfy the policy"
        );
    }
    sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await.unwrap();
    drop(conn);
    app.close().await;
    db.cleanup().await;
}

/// Batch 2: the sealed request transaction sees exactly its own
/// cooperative's rows on every batch table — never the foreign coop's.
#[tokio::test]
async fn batch2_cross_cooperative_reads_are_isolated() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch2_rows(&db, p.coop_a, p.user_a, "a").await;
    let rows_b = batch2_rows(&db, p.coop_b, p.user_c, "b").await;
    let app = db.app_pool().await;

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    for ((table, a_row), (_, b_row)) in batch2_row_ids(&rows_a)
        .into_iter()
        .zip(batch2_row_ids(&rows_b))
    {
        let id_col = if table == "assessment_share_sources" {
            "assessment_id"
        } else {
            "id"
        };
        let own: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE {id_col} = $1"))
                .bind(a_row)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(own, 1, "{table}: own row must be visible");
        let foreign: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE {id_col} = $1"))
                .bind(b_row)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(foreign, 0, "{table}: foreign row must be invisible");
    }
    tx.rollback().await.unwrap();
    app.close().await;
    db.cleanup().await;
}

/// Batch 2: cross-coop writes are rejected — foreign UPDATE/DELETE hit
/// zero rows, and cooperative_id reassignment on an own row is denied.
#[tokio::test]
async fn batch2_cross_cooperative_writes_and_reassignment_rejected() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch2_rows(&db, p.coop_a, p.user_a, "a").await;
    let rows_b = batch2_rows(&db, p.coop_b, p.user_c, "b").await;
    let app = db.app_pool().await;

    for ((table, a_row), (_, b_row)) in batch2_row_ids(&rows_a)
        .into_iter()
        .zip(batch2_row_ids(&rows_b))
    {
        let id_col = if table == "assessment_share_sources" {
            "assessment_id"
        } else {
            "id"
        };
        // R3 (0054): cooperative_id left the runtime UPDATE surface and
        // DELETE was revoked on financial/history tables — the writes
        // now die at the grant layer (42501) before row visibility is
        // even consulted. A privilege error aborts the tx, so each
        // probe runs in its own sealed request tx.
        let upd = {
            let (mut tx, _c) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!(
                "UPDATE {table} SET cooperative_id = cooperative_id WHERE {id_col} = $1"
            ))
            .bind(b_row)
            .execute(&mut *tx)
            .await
        };
        assert!(
            matches!(upd, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{table}: UPDATE must be denied at the grant layer"
        );
        let del = {
            let (mut tx, _c) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!("DELETE FROM {table} WHERE {id_col} = $1"))
                .bind(b_row)
                .execute(&mut *tx)
                .await
        };
        match del {
            // assessment_rules keeps DELETE (draft cleanup) but the
            // foreign row stays invisible under USING -> 0 rows.
            Ok(res) if table == "assessment_rules" => {
                assert_eq!(res.rows_affected(), 0, "{table}: foreign DELETE")
            }
            Ok(_) => panic!("{table}: DELETE must be denied — grant revoked in 0054"),
            Err(sqlx::Error::Database(ref e)) => {
                assert_eq!(
                    e.code().as_deref(),
                    Some("42501"),
                    "{table}: DELETE must be permission-denied"
                );
            }
            Err(e) => panic!("{table}: unexpected DELETE error {e}"),
        }
        let reassign = {
            let (mut tx, _c) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!(
                "UPDATE {table} SET cooperative_id = $1 WHERE {id_col} = $2"
            ))
            .bind(p.coop_b)
            .bind(a_row)
            .execute(&mut *tx)
            .await
        };
        assert!(
            matches!(reassign, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{table}: reassignment must fail"
        );
    }
    app.close().await;
    db.cleanup().await;
}

/// Batch 2: a legitimate sealed-context workflow still works —
/// own-row visibility, in-scope touch updates and an in-scope INSERT.
#[tokio::test]
async fn batch2_legitimate_workflow_still_works() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    r3_grant_perms(&db, p.coop_a, p.user_a, &["payments.manage"]).await;
    let rows_a = batch2_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    for (table, row) in batch2_row_ids(&rows_a) {
        let id_col = if table == "assessment_share_sources" {
            "assessment_id"
        } else {
            "id"
        };
        let seen: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE {id_col} = $1"))
                .bind(row)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(seen, 1, "{table}: own row must be visible");
        // R3 (0054): the attribution column left the runtime UPDATE
        // surface — even in-scope, a cooperative_id write is now a
        // grant-layer error. Probe in its own tx (a privilege error
        // aborts the transaction).
        let touch = {
            let (mut t2, _c2) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!(
                "UPDATE {table} SET cooperative_id = cooperative_id WHERE {id_col} = $1"
            ))
            .bind(row)
            .execute(&mut *t2)
            .await
        };
        assert!(
            matches!(touch, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{table}: attribution column must not be writable"
        );
    }
    // Fresh in-scope INSERT into a batch-2 table. 0058 binds the
    // payment at commit to exactly one matching movement leg, and
    // the document number is trigger-assigned — the legitimate
    // sealed package is payment + its cash leg together.
    let account: Uuid = sqlx::query_scalar(
        "INSERT INTO financial_accounts (name, account_type, created_by, cooperative_id) \
         VALUES ('B2Kasa','cash',$1,$2) RETURNING id",
    )
    .bind(p.user_a)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let person: Uuid = sqlx::query_scalar(
        "INSERT INTO persons (first_name, last_name, search_name, cooperative_id) \
         VALUES ('W2','T','w2t',$1) RETURNING id",
    )
    .bind(p.coop_a)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let payment_id: Uuid = sqlx::query_scalar(
        "INSERT INTO payments \
         (payer_person_id, amount, method, received_at, idempotency_key, \
          idempotency_fingerprint, created_by, cooperative_id, \
          destination_account_id) \
         VALUES ($1,10,'cash',now(),'b2-ins','b2-insfp',$2,$3,$4) RETURNING id",
    )
    .bind(person)
    .bind(p.user_a)
    .bind(p.coop_a)
    .bind(account)
    .fetch_one(&mut *tx)
    .await
    .expect("in-scope batch-2 INSERT must succeed");
    sqlx::query(
        "INSERT INTO account_movements \
         (account_id, direction, amount, source_type, source_id, occurred_at, \
          created_by, cooperative_id) \
         VALUES ($1,'inflow',10,'payment',$2,now(),$3,$4)",
    )
    .bind(account)
    .bind(payment_id)
    .bind(p.user_a)
    .bind(p.coop_a)
    .execute(&mut *tx)
    .await
    .expect("the payment's cash leg inserts in the same tx");
    tx.commit().await.unwrap();

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM payments WHERE cooperative_id = $1")
        .bind(p.coop_a)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 2);
    tx.rollback().await.unwrap();
    app.close().await;
    db.cleanup().await;
}

/// Batch 2: rollback and pooled-connection reuse never leak the
/// sealed context into a later checkout.
#[tokio::test]
async fn batch2_rollback_and_reuse_do_not_leak() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch2_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    let (tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .unwrap();
    drop(tx); // rollback

    for (table, row) in batch2_row_ids(&rows_a) {
        let id_col = if table == "assessment_share_sources" {
            "assessment_id"
        } else {
            "id"
        };
        let seen: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE {id_col} = $1"))
                .bind(row)
                .fetch_one(&app)
                .await
                .unwrap();
        assert_eq!(seen, 0, "{table}: post-rollback checkout must see nothing");
    }
    app.close().await;
    db.cleanup().await;
}

// ---------------------------------------------------------------------
// M1-P4 Batch 3 (migration 0042) — sealed-RLS rollout on the
// investment lifecycle cluster plus the governance decision record:
//   investments, investment_fundings, investment_valuations,
//   investment_disposals, investment_disposal_proceeds,
//   governance_decisions
// ---------------------------------------------------------------------

/// Batch-3 (migration 0042) tables under RLS. `investment_incomes`
/// and `governance_votes` (batch 1) were already forced; this closes
/// the investment and decision chains end-to-end.
const BATCH3_TABLES: [&str; 6] = [
    "investments",
    "investment_fundings",
    "investment_valuations",
    "investment_disposals",
    "investment_disposal_proceeds",
    "governance_decisions",
];

/// Row ids of the one batch-3 row seeded per cooperative.
struct Batch3Rows {
    investment: Uuid,
    funding: Uuid,
    valuation: Uuid,
    disposal: Uuid,
    proceed: Uuid,
    decision: Uuid,
}

/// Seed the minimum parent chain plus one row in every batch-3 table
/// for `coop`, all through the owner lane (RLS-unaffected).
async fn batch3_rows(db: &TestDb, coop: Uuid, user: Uuid, tag: &str) -> Batch3Rows {
    let account: Uuid = sqlx::query_scalar(
        "INSERT INTO financial_accounts (name, account_type, created_by, cooperative_id) \
         VALUES ($2,'cash',$3,$1) RETURNING id",
    )
    .bind(coop)
    .bind(format!("B3Kasa{tag}"))
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let body: Uuid = sqlx::query_scalar(
        "INSERT INTO governance_bodies \
         (name, body_type, status, created_by, cooperative_id, body_number, \
          idempotency_key, idempotency_fingerprint) \
         VALUES ($2,'board','active',$3,$1,50,$4,$5) RETURNING id",
    )
    .bind(coop)
    .bind(format!("B3Body{tag}"))
    .bind(user)
    .bind(format!("b3body-{tag}"))
    .bind(format!("b3bodyfp-{tag}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let investment: Uuid = sqlx::query_scalar(
        "INSERT INTO investments \
         (name, investment_type, idempotency_key, idempotency_fingerprint, \
          created_by, cooperative_id, investment_number) \
         VALUES ($2,'real_estate',$3,$4,$5,$1, \
                 public.allocate_document_number($1,'investments.investment_number')) RETURNING id",
    )
    .bind(coop)
    .bind(format!("B3Inv{tag}"))
    .bind(format!("b3inv-{tag}"))
    .bind(format!("b3invfp-{tag}"))
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    // Funding: the account_movement row must exist first — source_id
    // is polymorphic (no FK), so it can point at the funding id.
    let funding: Uuid = Uuid::new_v4();
    let funding_movement: Uuid = sqlx::query_scalar(
        "INSERT INTO account_movements \
         (account_id, direction, amount, source_type, source_id, occurred_at, \
          created_by, cooperative_id) \
         VALUES ($1,'outflow',50,'investment_funding',$2,now(),$3,$4) RETURNING id",
    )
    .bind(account)
    .bind(funding)
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO investment_fundings \
         (id, investment_id, financial_account_id, amount, occurred_at, \
          account_movement_id, idempotency_key, idempotency_fingerprint, \
          created_by, cooperative_id, funding_number) \
         VALUES ($1,$2,$3,50,now(),$4,$5,$6,$7,$8, \
                 public.allocate_document_number($8,'investment_fundings.funding_number'))",
    )
    .bind(funding)
    .bind(investment)
    .bind(account)
    .bind(funding_movement)
    .bind(format!("b3fnd-{tag}"))
    .bind(format!("b3fndfp-{tag}"))
    .bind(user)
    .bind(coop)
    .execute(&db.pool)
    .await
    .unwrap();
    let valuation: Uuid = sqlx::query_scalar(
        "INSERT INTO investment_valuations \
         (investment_id, valuation_date, amount, idempotency_key, \
          idempotency_fingerprint, created_by, cooperative_id, valuation_number) \
         VALUES ($1,'2026-03-01',60,$2,$3,$4,$5, \
                 public.allocate_document_number($5,'investment_valuations.valuation_number')) RETURNING id",
    )
    .bind(investment)
    .bind(format!("b3val-{tag}"))
    .bind(format!("b3valfp-{tag}"))
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let disposal: Uuid = sqlx::query_scalar(
        "INSERT INTO investment_disposals \
         (investment_id, disposed_at, idempotency_key, idempotency_fingerprint, \
          created_by, cooperative_id, disposal_number) \
         VALUES ($1,now(),$2,$3,$4,$5, \
                 public.allocate_document_number($5,'investment_disposals.disposal_number')) RETURNING id",
    )
    .bind(investment)
    .bind(format!("b3dsp-{tag}"))
    .bind(format!("b3dspfp-{tag}"))
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let proceed_movement: Uuid = sqlx::query_scalar(
        "INSERT INTO account_movements \
         (account_id, direction, amount, source_type, source_id, occurred_at, \
          created_by, cooperative_id) \
         VALUES ($1,'inflow',70,'investment_disposal',$2,now(),$3,$4) RETURNING id",
    )
    .bind(account)
    .bind(disposal)
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let proceed: Uuid = sqlx::query_scalar(
        "INSERT INTO investment_disposal_proceeds \
         (disposal_id, financial_account_id, amount, occurred_at, \
          account_movement_id, cooperative_id) \
         VALUES ($1,$2,70,now(),$3,$4) RETURNING id",
    )
    .bind(disposal)
    .bind(account)
    .bind(proceed_movement)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let decision: Uuid = sqlx::query_scalar(
        "INSERT INTO governance_decisions \
         (body_id, title, decision_text, decision_on, idempotency_key, \
          idempotency_fingerprint, created_by, cooperative_id, decision_number) \
         VALUES ($1,$2,'t','2026-03-01',$3,$4,$5,$6, \
                 public.allocate_document_number($6,'governance_decisions.decision_number')) RETURNING id",
    )
    .bind(body)
    .bind(format!("B3Dec{tag}"))
    .bind(format!("b3dec-{tag}"))
    .bind(format!("b3decfp-{tag}"))
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    Batch3Rows {
        investment,
        funding,
        valuation,
        disposal,
        proceed,
        decision,
    }
}

/// (table, id) pairs of the seeded batch-3 rows.
fn batch3_row_ids(rows: &Batch3Rows) -> Vec<(&'static str, Uuid)> {
    vec![
        ("investments", rows.investment),
        ("investment_fundings", rows.funding),
        ("investment_valuations", rows.valuation),
        ("investment_disposals", rows.disposal),
        ("investment_disposal_proceeds", rows.proceed),
        ("governance_decisions", rows.decision),
    ]
}

/// Batch 3: every selected table fails closed when no sealed context
/// exists — zero rows on SELECT, WITH CHECK rejection on INSERT.
#[tokio::test]
async fn batch3_missing_context_fails_closed() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let _rows_a = batch3_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    for table in BATCH3_TABLES {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&app)
            .await
            .unwrap();
        assert_eq!(seen, 0, "{table}: unauthenticated SELECT must see nothing");
    }
    let err = sqlx::query(
        "INSERT INTO investments \
         (name, investment_type, idempotency_key, idempotency_fingerprint, \
          created_by, cooperative_id, investment_number) \
         VALUES ('x','real_estate','b3-x','b3-xfp',$1,$2,999)",
    )
    .bind(p.user_a)
    .bind(p.coop_a)
    .execute(&app)
    .await
    .expect_err("unauthenticated INSERT must be denied");
    let msg = err.to_string();
    assert!(
        msg.contains("row-level security")
            || msg.contains("row_security")
            || msg.contains("insufficient_privilege"),
        "INSERT must fail on the RLS WITH CHECK, got: {msg}"
    );
    app.close().await;
    db.cleanup().await;
}

/// Batch 3: a raw `set_config` stamp (forge attempt — no seal chain)
/// must still see zero rows on every batch table.
#[tokio::test]
async fn batch3_forged_guc_context_sees_nothing() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch3_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    let mut conn = stamp(&app, p.coop_a).await;
    for (table, row) in batch3_row_ids(&rows_a) {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(row)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(
            seen, 0,
            "{table}: unsealed GUC stamp must not satisfy the policy"
        );
    }
    sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await.unwrap();
    drop(conn);
    app.close().await;
    db.cleanup().await;
}

/// Batch 3: the sealed request transaction sees exactly its own
/// cooperative's rows on every batch table — never the foreign coop's.
#[tokio::test]
async fn batch3_cross_cooperative_reads_are_isolated() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch3_rows(&db, p.coop_a, p.user_a, "a").await;
    let rows_b = batch3_rows(&db, p.coop_b, p.user_c, "b").await;
    let app = db.app_pool().await;

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    for ((table, a_row), (_, b_row)) in batch3_row_ids(&rows_a)
        .into_iter()
        .zip(batch3_row_ids(&rows_b))
    {
        let own: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(a_row)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(own, 1, "{table}: own row must be visible");
        let foreign: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
                .bind(b_row)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(foreign, 0, "{table}: foreign row must be invisible");
    }
    tx.rollback().await.unwrap();
    app.close().await;
    db.cleanup().await;
}

/// Batch 3: cross-coop writes are rejected — foreign UPDATE/DELETE hit
/// zero rows, and cooperative_id reassignment on an own row is denied.
#[tokio::test]
async fn batch3_cross_cooperative_writes_and_reassignment_rejected() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch3_rows(&db, p.coop_a, p.user_a, "a").await;
    let rows_b = batch3_rows(&db, p.coop_b, p.user_c, "b").await;
    let app = db.app_pool().await;

    for ((table, a_row), (_, b_row)) in batch3_row_ids(&rows_a)
        .into_iter()
        .zip(batch3_row_ids(&rows_b))
    {
        let (tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
            .await
            .expect("sealed request tx");
        // R3 (0054): cooperative_id left the runtime UPDATE surface and
        // DELETE was revoked on financial/history tables — the writes
        // now die at the grant layer (42501) before row visibility is
        // even consulted. A privilege error aborts the tx, so each
        // probe runs in its own sealed request tx.
        drop(tx);
        let upd = {
            let (mut tx, _c) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!(
                "UPDATE {table} SET cooperative_id = cooperative_id WHERE id = $1"
            ))
            .bind(b_row)
            .execute(&mut *tx)
            .await
        };
        assert!(
            matches!(upd, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{table}: UPDATE must be denied at the grant layer"
        );
        let del = {
            let (mut tx, _c) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!("DELETE FROM {table} WHERE id = $1"))
                .bind(b_row)
                .execute(&mut *tx)
                .await
        };
        match del {
            // assessment_rules keeps DELETE (draft cleanup) but the
            // foreign row stays invisible under USING -> 0 rows.
            Ok(res) if table == "assessment_rules" => {
                assert_eq!(res.rows_affected(), 0, "{table}: foreign DELETE")
            }
            Ok(_) => panic!("{table}: DELETE must be denied — grant revoked in 0054"),
            Err(sqlx::Error::Database(ref e)) => {
                assert_eq!(
                    e.code().as_deref(),
                    Some("42501"),
                    "{table}: DELETE must be permission-denied"
                );
            }
            Err(e) => panic!("{table}: unexpected DELETE error {e}"),
        }
        let reassign = {
            let (mut tx, _c) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!(
                "UPDATE {table} SET cooperative_id = $1 WHERE id = $2"
            ))
            .bind(p.coop_b)
            .bind(a_row)
            .execute(&mut *tx)
            .await
        };
        assert!(
            matches!(reassign, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{table}: reassignment must fail"
        );
    }
    app.close().await;
    db.cleanup().await;
}

/// Batch 3: a legitimate sealed-context workflow still works —
/// own-row visibility, in-scope touch updates and an in-scope INSERT.
#[tokio::test]
async fn batch3_legitimate_workflow_still_works() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    r3_grant_perms(&db, p.coop_a, p.user_a, &["investments.manage"]).await;
    let rows_a = batch3_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    for (table, row) in batch3_row_ids(&rows_a) {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(row)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(seen, 1, "{table}: own row must be visible");
        // R3 (0054): the attribution column left the runtime UPDATE
        // surface — even in-scope, a cooperative_id write is now a
        // grant-layer error. Probe in its own tx (a privilege error
        // aborts the transaction).
        let touch = {
            let (mut t2, _c2) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!(
                "UPDATE {table} SET cooperative_id = cooperative_id WHERE id = $1"
            ))
            .bind(row)
            .execute(&mut *t2)
            .await
        };
        assert!(
            matches!(touch, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{table}: attribution column must not be writable"
        );
    }
    // Fresh in-scope INSERT into a batch-3 table.
    sqlx::query(
        "INSERT INTO investments \
         (name, investment_type, idempotency_key, idempotency_fingerprint, \
          created_by, cooperative_id, investment_number) \
         VALUES ('B3Ins','business','b3-ins','b3-insfp',$1,$2, \
                 public.allocate_document_number($2,'investments.investment_number'))",
    )
    .bind(p.user_a)
    .bind(p.coop_a)
    .execute(&mut *tx)
    .await
    .expect("in-scope batch-3 INSERT must succeed");
    tx.commit().await.unwrap();

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM investments WHERE cooperative_id = $1")
            .bind(p.coop_a)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(count, 2);
    tx.rollback().await.unwrap();
    app.close().await;
    db.cleanup().await;
}

/// Batch 3: rollback and pooled-connection reuse never leak the
/// sealed context into a later checkout.
#[tokio::test]
async fn batch3_rollback_and_reuse_do_not_leak() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch3_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    let (tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .unwrap();
    drop(tx); // rollback

    for (table, row) in batch3_row_ids(&rows_a) {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(row)
            .fetch_one(&app)
            .await
            .unwrap();
        assert_eq!(seen, 0, "{table}: post-rollback checkout must see nothing");
    }
    app.close().await;
    db.cleanup().await;
}

// ---------------------------------------------------------------------
// M1-P4 Batch 4 (migration 0043) — sealed-RLS rollout on the identity
// and share-ownership cluster:
//   persons, families, shareholders, shares, share_events,
//   share_ownerships
// ---------------------------------------------------------------------

/// Batch-4 (migration 0043) tables under RLS — the identity and
/// share-ownership cluster. Their referrers are already forced
/// (payments, assessments, shareholder_credits,
/// shareholder_family_memberships, governance_votes,
/// assessment_share_sources) or inside this set.
const BATCH4_TABLES: [&str; 6] = [
    "persons",
    "families",
    "shareholders",
    "shares",
    "share_events",
    "share_ownerships",
];

/// Row ids of the one batch-4 row seeded per cooperative.
struct Batch4Rows {
    person: Uuid,
    family: Uuid,
    shareholder: Uuid,
    share: Uuid,
    share_event: Uuid,
    ownership: Uuid,
}

/// Seed one row in every batch-4 table for `coop`, all through the
/// owner lane (RLS-unaffected).
async fn batch4_rows(db: &TestDb, coop: Uuid, user: Uuid, tag: &str) -> Batch4Rows {
    let person: Uuid = sqlx::query_scalar(
        "INSERT INTO persons (first_name, last_name, search_name, cooperative_id) \
         VALUES ($2,'T','t',$1) RETURNING id",
    )
    .bind(coop)
    .bind(format!("P4{tag}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let family: Uuid = sqlx::query_scalar(
        "INSERT INTO families (sequence_number, cooperative_id) \
         VALUES ($2,$1) RETURNING id",
    )
    .bind(coop)
    .bind(70_i64)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let shareholder: Uuid = sqlx::query_scalar(
        "INSERT INTO shareholders (person_id, cooperative_id) VALUES ($1,$2) RETURNING id",
    )
    .bind(person)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let share: Uuid = sqlx::query_scalar(
        "INSERT INTO shares (cooperative_id, share_number, created_by) \
         VALUES ($1, public.allocate_document_number($1,'shares.share_number'), $2) RETURNING id",
    )
    .bind(coop)
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let share_event: Uuid = sqlx::query_scalar(
        "INSERT INTO share_events \
         (share_id, event_type, occurred_at, to_shareholder_id, acquisition_type, \
          actor_user_id, cooperative_id) \
         VALUES ($1,'initial_acquisition',now(),$2,'founder',$3,$4) RETURNING id",
    )
    .bind(share)
    .bind(shareholder)
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let ownership: Uuid = sqlx::query_scalar(
        "INSERT INTO share_ownerships \
         (share_id, shareholder_id, started_at, acquisition_type, source_event_id, \
          created_by, cooperative_id) \
         VALUES ($1,$2,now(),'founder',$3,$4,$5) RETURNING id",
    )
    .bind(share)
    .bind(shareholder)
    .bind(share_event)
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    Batch4Rows {
        person,
        family,
        shareholder,
        share,
        share_event,
        ownership,
    }
}

/// (table, id) pairs of the seeded batch-4 rows.
fn batch4_row_ids(rows: &Batch4Rows) -> Vec<(&'static str, Uuid)> {
    vec![
        ("persons", rows.person),
        ("families", rows.family),
        ("shareholders", rows.shareholder),
        ("shares", rows.share),
        ("share_events", rows.share_event),
        ("share_ownerships", rows.ownership),
    ]
}

/// Batch 4: every selected table fails closed when no sealed context
/// exists — zero rows on SELECT, WITH CHECK rejection on INSERT.
#[tokio::test]
async fn batch4_missing_context_fails_closed() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let _rows_a = batch4_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    for table in BATCH4_TABLES {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&app)
            .await
            .unwrap();
        assert_eq!(seen, 0, "{table}: unauthenticated SELECT must see nothing");
    }
    let err = sqlx::query(
        "INSERT INTO persons (first_name, last_name, search_name, cooperative_id) \
         VALUES ('x','t','t',$1)",
    )
    .bind(p.coop_a)
    .execute(&app)
    .await
    .expect_err("unauthenticated INSERT must be denied");
    let msg = err.to_string();
    assert!(
        msg.contains("row-level security")
            || msg.contains("row_security")
            || msg.contains("insufficient_privilege"),
        "INSERT must fail on the RLS WITH CHECK, got: {msg}"
    );
    app.close().await;
    db.cleanup().await;
}

/// Batch 4: a raw `set_config` stamp (forge attempt — no seal chain)
/// must still see zero rows on every batch table.
#[tokio::test]
async fn batch4_forged_guc_context_sees_nothing() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch4_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    let mut conn = stamp(&app, p.coop_a).await;
    for (table, row) in batch4_row_ids(&rows_a) {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(row)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(
            seen, 0,
            "{table}: unsealed GUC stamp must not satisfy the policy"
        );
    }
    sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await.unwrap();
    drop(conn);
    app.close().await;
    db.cleanup().await;
}

/// Batch 4: the sealed request transaction sees exactly its own
/// cooperative's rows on every batch table — never the foreign coop's.
#[tokio::test]
async fn batch4_cross_cooperative_reads_are_isolated() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch4_rows(&db, p.coop_a, p.user_a, "a").await;
    let rows_b = batch4_rows(&db, p.coop_b, p.user_c, "b").await;
    let app = db.app_pool().await;

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    for ((table, a_row), (_, b_row)) in batch4_row_ids(&rows_a)
        .into_iter()
        .zip(batch4_row_ids(&rows_b))
    {
        let own: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(a_row)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(own, 1, "{table}: own row must be visible");
        let foreign: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
                .bind(b_row)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(foreign, 0, "{table}: foreign row must be invisible");
    }
    tx.rollback().await.unwrap();
    app.close().await;
    db.cleanup().await;
}

/// Batch 4: cross-coop writes are rejected — foreign UPDATE/DELETE hit
/// zero rows, and cooperative_id reassignment on an own row is denied.
/// Composite FK integrity still holds inside the sealed scope.
#[tokio::test]
async fn batch4_cross_cooperative_writes_and_reassignment_rejected() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    r3_grant_perms(&db, p.coop_a, p.user_a, &["shares.manage"]).await;
    let rows_a = batch4_rows(&db, p.coop_a, p.user_a, "a").await;
    let rows_b = batch4_rows(&db, p.coop_b, p.user_c, "b").await;
    let app = db.app_pool().await;

    for ((table, a_row), (_, b_row)) in batch4_row_ids(&rows_a)
        .into_iter()
        .zip(batch4_row_ids(&rows_b))
    {
        let (tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
            .await
            .expect("sealed request tx");
        // R3 (0054): cooperative_id left the runtime UPDATE surface and
        // DELETE was revoked on financial/history tables — the writes
        // now die at the grant layer (42501) before row visibility is
        // even consulted. A privilege error aborts the tx, so each
        // probe runs in its own sealed request tx.
        drop(tx);
        let upd = {
            let (mut tx, _c) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!(
                "UPDATE {table} SET cooperative_id = cooperative_id WHERE id = $1"
            ))
            .bind(b_row)
            .execute(&mut *tx)
            .await
        };
        assert!(
            matches!(upd, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{table}: UPDATE must be denied at the grant layer"
        );
        let del = {
            let (mut tx, _c) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!("DELETE FROM {table} WHERE id = $1"))
                .bind(b_row)
                .execute(&mut *tx)
                .await
        };
        match del {
            // assessment_rules keeps DELETE (draft cleanup) but the
            // foreign row stays invisible under USING -> 0 rows.
            Ok(res) if table == "assessment_rules" => {
                assert_eq!(res.rows_affected(), 0, "{table}: foreign DELETE")
            }
            Ok(_) => panic!("{table}: DELETE must be denied — grant revoked in 0054"),
            Err(sqlx::Error::Database(ref e)) => {
                assert_eq!(
                    e.code().as_deref(),
                    Some("42501"),
                    "{table}: DELETE must be permission-denied"
                );
            }
            Err(e) => panic!("{table}: unexpected DELETE error {e}"),
        }
        let reassign = {
            let (mut tx, _c) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!(
                "UPDATE {table} SET cooperative_id = $1 WHERE id = $2"
            ))
            .bind(p.coop_b)
            .bind(a_row)
            .execute(&mut *tx)
            .await
        };
        assert!(
            matches!(reassign, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{table}: reassignment must fail"
        );
    }

    // Composite-FK integrity inside the sealed scope: a share event
    // referencing coop B's share under coop A's stamp must still be
    // rejected by the composite FK even though the parent row is now
    // invisible to the app role (FK enforcement is not RLS-filtered).
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    let err = sqlx::query(
        "INSERT INTO share_events \
         (share_id, event_type, occurred_at, to_shareholder_id, acquisition_type, \
          actor_user_id, cooperative_id) \
         VALUES ($1,'initial_acquisition',now(),$2,'founder',$3,$4)",
    )
    .bind(rows_b.share)
    .bind(rows_a.shareholder)
    .bind(p.user_a)
    .bind(p.coop_a)
    .execute(&mut *tx)
    .await
    .expect_err("cross-coop composite FK must still be enforced");
    assert!(
        err.to_string().contains("_coop_fk") || err.to_string().contains("foreign key"),
        "expected composite FK violation, got: {err}"
    );
    tx.rollback().await.unwrap();
    app.close().await;
    db.cleanup().await;
}

/// Batch 4: a legitimate sealed-context workflow still works —
/// own-row visibility, in-scope touch updates, an in-scope INSERT and
/// a parent→child join.
#[tokio::test]
async fn batch4_legitimate_workflow_still_works() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    r3_grant_perms(
        &db,
        p.coop_a,
        p.user_a,
        &[
            "shareholders.manage",
            "shares.manage",
            "financial_accounts.manage",
        ],
    )
    .await;
    let rows_a = batch4_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    for (table, row) in batch4_row_ids(&rows_a) {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(row)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(seen, 1, "{table}: own row must be visible");
        // R3 (0054): the attribution column left the runtime UPDATE
        // surface — even in-scope, a cooperative_id write is now a
        // grant-layer error. Probe in its own tx (a privilege error
        // aborts the transaction).
        let touch = {
            let (mut t2, _c2) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!(
                "UPDATE {table} SET cooperative_id = cooperative_id WHERE id = $1"
            ))
            .bind(row)
            .execute(&mut *t2)
            .await
        };
        assert!(
            matches!(touch, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{table}: attribution column must not be writable"
        );
    }
    // Join across RLS'd parent/child pairs resolves inside scope.
    let joined: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM share_ownerships o JOIN shares s ON s.id = o.share_id \
         WHERE o.id = $1 AND s.cooperative_id = $2",
    )
    .bind(rows_a.ownership)
    .bind(p.coop_a)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(joined, 1, "ownership→share join must resolve in scope");
    // Fresh in-scope INSERT into a batch-4 table.
    sqlx::query(
        "INSERT INTO persons (first_name, last_name, search_name, cooperative_id) \
         VALUES ('W4','T','w4t',$1)",
    )
    .bind(p.coop_a)
    .execute(&mut *tx)
    .await
    .expect("in-scope batch-4 INSERT must succeed");
    tx.commit().await.unwrap();

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM persons WHERE cooperative_id = $1")
        .bind(p.coop_a)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 2);
    tx.rollback().await.unwrap();
    app.close().await;
    db.cleanup().await;
}

/// Batch 4: rollback and pooled-connection reuse never leak the
/// sealed context into a later checkout.
#[tokio::test]
async fn batch4_rollback_and_reuse_do_not_leak() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch4_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    let (tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .unwrap();
    drop(tx); // rollback

    for (table, row) in batch4_row_ids(&rows_a) {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(row)
            .fetch_one(&app)
            .await
            .unwrap();
        assert_eq!(seen, 0, "{table}: post-rollback checkout must see nothing");
    }
    app.close().await;
    db.cleanup().await;
}

// ---------------------------------------------------------------------
// M1-P4 Batch 5 (migration 0044) — sealed-RLS rollout on the
// share-return and social-aid chains:
//   share_returns, share_return_entitlements, share_return_settlements,
//   social_aid_funds, social_aid_donations, social_aid_disbursements
// ---------------------------------------------------------------------

/// Batch-5 (migration 0044) tables under RLS — the share-return and
/// social-aid chains, both fully closed (inbound referrers are in-set
/// only).
const BATCH5_TABLES: [&str; 6] = [
    "share_returns",
    "share_return_entitlements",
    "share_return_settlements",
    "social_aid_funds",
    "social_aid_donations",
    "social_aid_disbursements",
];

/// Row ids of the one batch-5 row seeded per cooperative.
struct Batch5Rows {
    share_return: Uuid,
    entitlement: Uuid,
    settlement: Uuid,
    fund: Uuid,
    donation: Uuid,
    disbursement: Uuid,
}

/// Seed one row in every batch-5 table for `coop`, all through the
/// owner lane (RLS-unaffected).
async fn batch5_rows(db: &TestDb, coop: Uuid, user: Uuid, tag: &str) -> Batch5Rows {
    // Parents: person → shareholder → share (+ ownership event), and a
    // cash account for the financial legs.
    let person: Uuid = sqlx::query_scalar(
        "INSERT INTO persons (first_name, last_name, search_name, cooperative_id) \
         VALUES ($2,'T','t',$1) RETURNING id",
    )
    .bind(coop)
    .bind(format!("P5{tag}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let shareholder: Uuid = sqlx::query_scalar(
        "INSERT INTO shareholders (person_id, cooperative_id) VALUES ($1,$2) RETURNING id",
    )
    .bind(person)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let share: Uuid = sqlx::query_scalar(
        "INSERT INTO shares (cooperative_id, share_number, created_by) \
         VALUES ($1, public.allocate_document_number($1,'shares.share_number'), $2) RETURNING id",
    )
    .bind(coop)
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let account: Uuid = sqlx::query_scalar(
        "INSERT INTO financial_accounts (name, account_type, created_by, cooperative_id) \
         VALUES ($2,'cash',$3,$1) RETURNING id",
    )
    .bind(coop)
    .bind(format!("B5Kasa{tag}"))
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    // Share-return chain: return → entitlement → settlement.
    let share_return: Uuid = sqlx::query_scalar(
        "INSERT INTO share_returns \
         (share_id, shareholder_id, owner_display_name, share_number, \
          ownership_started_at, requested_at, effective_return_date, \
          idempotency_key, idempotency_fingerprint, created_by, cooperative_id, \
          return_number) \
         VALUES ($1,$2,$3,1,now(),now(),'2026-04-01',$4,$5,$6,$7, \
                 public.allocate_document_number($7,'share_returns.return_number')) RETURNING id",
    )
    .bind(share)
    .bind(shareholder)
    .bind(format!("B5Owner{tag}"))
    .bind(format!("b5sr-{tag}"))
    .bind(format!("b5srfp-{tag}"))
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let entitlement: Uuid = sqlx::query_scalar(
        "INSERT INTO share_return_entitlements \
         (share_return_id, entitlement_type, beneficiary_shareholder_id, \
          recognized_at, created_by, cooperative_id, entitlement_number, amount, \
          determined_at) \
         VALUES ($1,'principal',$2,now(),$3,$4, \
                 public.allocate_document_number($4,'share_return_entitlements.entitlement_number'), \
                 30, now()) RETURNING id",
    )
    .bind(share_return)
    .bind(shareholder)
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    // Production convention: the settlement movement's source_id is the
    // SETTLEMENT id — pre-generate it so the pair links correctly.
    let settlement_id = Uuid::new_v4();
    let settlement_movement: Uuid = sqlx::query_scalar(
        "INSERT INTO account_movements \
         (account_id, direction, amount, source_type, source_id, occurred_at, \
          created_by, cooperative_id) \
         VALUES ($1,'outflow',30,'share_return_settlement',$2,now(),$3,$4) RETURNING id",
    )
    .bind(account)
    .bind(settlement_id)
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let settlement: Uuid = sqlx::query_scalar(
        "INSERT INTO share_return_settlements \
         (id, entitlement_id, financial_account_id, amount, settled_at, \
          account_movement_id, idempotency_key, idempotency_fingerprint, \
          created_by, cooperative_id, settlement_number) \
         VALUES ($8,$1,$2,30,now(),$3,$4,$5,$6,$7, \
                 public.allocate_document_number($7,'share_return_settlements.settlement_number')) RETURNING id",
    )
    .bind(entitlement)
    .bind(account)
    .bind(settlement_movement)
    .bind(format!("b5st-{tag}"))
    .bind(format!("b5stfp-{tag}"))
    .bind(user)
    .bind(coop)
    .bind(settlement_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    // Social-aid chain: fund → donation + disbursement.
    let fund: Uuid = sqlx::query_scalar(
        "INSERT INTO social_aid_funds \
         (name, idempotency_key, idempotency_fingerprint, created_by, \
          cooperative_id, fund_number) \
         VALUES ($2,$3,$4,$5,$1, \
                 public.allocate_document_number($1,'social_aid_funds.fund_number')) RETURNING id",
    )
    .bind(coop)
    .bind(format!("B5Fon{tag}"))
    .bind(format!("b5sf-{tag}"))
    .bind(format!("b5sffp-{tag}"))
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    // Movement legs reference their source ROW id (donation/
    // disbursement), matching production writes.
    let donation_id = Uuid::new_v4();
    let donation_movement: Uuid = sqlx::query_scalar(
        "INSERT INTO account_movements \
         (account_id, direction, amount, source_type, source_id, occurred_at, \
          created_by, cooperative_id) \
         VALUES ($1,'inflow',40,'social_aid_donation',$2,now(),$3,$4) RETURNING id",
    )
    .bind(account)
    .bind(donation_id)
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let donation: Uuid = sqlx::query_scalar(
        "INSERT INTO social_aid_donations \
         (id, fund_id, financial_account_id, amount, occurred_at, \
          account_movement_id, idempotency_key, idempotency_fingerprint, \
          created_by, cooperative_id, donation_number, donor_display_name) \
         VALUES ($8,$1,$2,40,now(),$3,$4,$5,$6,$7, \
                 public.allocate_document_number($7,'social_aid_donations.donation_number'), \
                 'B5Donor') RETURNING id",
    )
    .bind(fund)
    .bind(account)
    .bind(donation_movement)
    .bind(format!("b5dn-{tag}"))
    .bind(format!("b5dnfp-{tag}"))
    .bind(user)
    .bind(coop)
    .bind(donation_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let disbursement_id = Uuid::new_v4();
    let disbursement_movement: Uuid = sqlx::query_scalar(
        "INSERT INTO account_movements \
         (account_id, direction, amount, source_type, source_id, occurred_at, \
          created_by, cooperative_id) \
         VALUES ($1,'outflow',15,'social_aid_disbursement',$2,now(),$3,$4) RETURNING id",
    )
    .bind(account)
    .bind(disbursement_id)
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let disbursement: Uuid = sqlx::query_scalar(
        "INSERT INTO social_aid_disbursements \
         (id, fund_id, financial_account_id, amount, occurred_at, reason, \
          account_movement_id, idempotency_key, idempotency_fingerprint, \
          created_by, cooperative_id, disbursement_number, beneficiary_display_name) \
         VALUES ($8,$1,$2,15,now(),'b5-reason',$3,$4,$5,$6,$7, \
                 public.allocate_document_number($7,'social_aid_disbursements.disbursement_number'), \
                 'B5Ben') RETURNING id",
    )
    .bind(fund)
    .bind(account)
    .bind(disbursement_movement)
    .bind(format!("b5db-{tag}"))
    .bind(format!("b5dbfp-{tag}"))
    .bind(user)
    .bind(coop)
    .bind(disbursement_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    Batch5Rows {
        share_return,
        entitlement,
        settlement,
        fund,
        donation,
        disbursement,
    }
}

/// (table, id) pairs of the seeded batch-5 rows.
fn batch5_row_ids(rows: &Batch5Rows) -> Vec<(&'static str, Uuid)> {
    vec![
        ("share_returns", rows.share_return),
        ("share_return_entitlements", rows.entitlement),
        ("share_return_settlements", rows.settlement),
        ("social_aid_funds", rows.fund),
        ("social_aid_donations", rows.donation),
        ("social_aid_disbursements", rows.disbursement),
    ]
}

/// Batch 5: every selected table fails closed when no sealed context
/// exists — zero rows on SELECT, WITH CHECK rejection on INSERT.
#[tokio::test]
async fn batch5_missing_context_fails_closed() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let _rows_a = batch5_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    for table in BATCH5_TABLES {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&app)
            .await
            .unwrap();
        assert_eq!(seen, 0, "{table}: unauthenticated SELECT must see nothing");
    }
    let err = sqlx::query(
        "INSERT INTO social_aid_funds \
         (name, idempotency_key, idempotency_fingerprint, created_by, \
          cooperative_id, fund_number) \
         VALUES ('x','b5-x','b5-xfp',$1,$2,999)",
    )
    .bind(p.user_a)
    .bind(p.coop_a)
    .execute(&app)
    .await
    .expect_err("unauthenticated INSERT must be denied");
    let msg = err.to_string();
    assert!(
        msg.contains("row-level security")
            || msg.contains("row_security")
            || msg.contains("insufficient_privilege"),
        "INSERT must fail on the RLS WITH CHECK, got: {msg}"
    );
    app.close().await;
    db.cleanup().await;
}

/// Batch 5: a raw `set_config` stamp (forge attempt — no seal chain)
/// must still see zero rows on every batch table.
#[tokio::test]
async fn batch5_forged_guc_context_sees_nothing() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch5_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    let mut conn = stamp(&app, p.coop_a).await;
    for (table, row) in batch5_row_ids(&rows_a) {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(row)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(
            seen, 0,
            "{table}: unsealed GUC stamp must not satisfy the policy"
        );
    }
    sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await.unwrap();
    drop(conn);
    app.close().await;
    db.cleanup().await;
}

/// Batch 5: the sealed request transaction sees exactly its own
/// cooperative's rows on every batch table — never the foreign coop's.
#[tokio::test]
async fn batch5_cross_cooperative_reads_are_isolated() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch5_rows(&db, p.coop_a, p.user_a, "a").await;
    let rows_b = batch5_rows(&db, p.coop_b, p.user_c, "b").await;
    let app = db.app_pool().await;

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    for ((table, a_row), (_, b_row)) in batch5_row_ids(&rows_a)
        .into_iter()
        .zip(batch5_row_ids(&rows_b))
    {
        let own: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(a_row)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(own, 1, "{table}: own row must be visible");
        let foreign: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
                .bind(b_row)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(foreign, 0, "{table}: foreign row must be invisible");
    }
    tx.rollback().await.unwrap();
    app.close().await;
    db.cleanup().await;
}

/// Batch 5: cross-coop writes are rejected — foreign UPDATE/DELETE hit
/// zero rows, and cooperative_id reassignment on an own row is denied.
#[tokio::test]
async fn batch5_cross_cooperative_writes_and_reassignment_rejected() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch5_rows(&db, p.coop_a, p.user_a, "a").await;
    let rows_b = batch5_rows(&db, p.coop_b, p.user_c, "b").await;
    let app = db.app_pool().await;

    for ((table, a_row), (_, b_row)) in batch5_row_ids(&rows_a)
        .into_iter()
        .zip(batch5_row_ids(&rows_b))
    {
        let (tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
            .await
            .expect("sealed request tx");
        // R3 (0054): cooperative_id left the runtime UPDATE surface and
        // DELETE was revoked on financial/history tables — the writes
        // now die at the grant layer (42501) before row visibility is
        // even consulted. A privilege error aborts the tx, so each
        // probe runs in its own sealed request tx.
        drop(tx);
        let upd = {
            let (mut tx, _c) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!(
                "UPDATE {table} SET cooperative_id = cooperative_id WHERE id = $1"
            ))
            .bind(b_row)
            .execute(&mut *tx)
            .await
        };
        assert!(
            matches!(upd, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{table}: UPDATE must be denied at the grant layer"
        );
        let del = {
            let (mut tx, _c) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!("DELETE FROM {table} WHERE id = $1"))
                .bind(b_row)
                .execute(&mut *tx)
                .await
        };
        match del {
            // assessment_rules keeps DELETE (draft cleanup) but the
            // foreign row stays invisible under USING -> 0 rows.
            Ok(res) if table == "assessment_rules" => {
                assert_eq!(res.rows_affected(), 0, "{table}: foreign DELETE")
            }
            Ok(_) => panic!("{table}: DELETE must be denied — grant revoked in 0054"),
            Err(sqlx::Error::Database(ref e)) => {
                assert_eq!(
                    e.code().as_deref(),
                    Some("42501"),
                    "{table}: DELETE must be permission-denied"
                );
            }
            Err(e) => panic!("{table}: unexpected DELETE error {e}"),
        }
        let reassign = {
            let (mut tx, _c) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!(
                "UPDATE {table} SET cooperative_id = $1 WHERE id = $2"
            ))
            .bind(p.coop_b)
            .bind(a_row)
            .execute(&mut *tx)
            .await
        };
        assert!(
            matches!(reassign, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{table}: reassignment must fail"
        );
    }
    app.close().await;
    db.cleanup().await;
}

/// Batch 5: a legitimate sealed-context workflow still works —
/// own-row visibility, in-scope touch updates, a chain join
/// (entitlement→return→share) and an in-scope INSERT.
#[tokio::test]
async fn batch5_legitimate_workflow_still_works() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    r3_grant_perms(
        &db,
        p.coop_a,
        p.user_a,
        &["social_aid.manage", "financial_accounts.manage"],
    )
    .await;
    let rows_a = batch5_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    for (table, row) in batch5_row_ids(&rows_a) {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(row)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(seen, 1, "{table}: own row must be visible");
        // R3 (0054): the attribution column left the runtime UPDATE
        // surface — even in-scope, a cooperative_id write is now a
        // grant-layer error. Probe in its own tx (a privilege error
        // aborts the transaction).
        let touch = {
            let (mut t2, _c2) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!(
                "UPDATE {table} SET cooperative_id = cooperative_id WHERE id = $1"
            ))
            .bind(row)
            .execute(&mut *t2)
            .await
        };
        assert!(
            matches!(touch, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{table}: attribution column must not be writable"
        );
    }
    // Chain join: entitlement → share_return → share, all in-scope.
    let joined: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM share_return_entitlements e \
         JOIN share_returns r ON r.id = e.share_return_id \
         JOIN shares s ON s.id = r.share_id \
         WHERE e.id = $1 AND s.cooperative_id = $2",
    )
    .bind(rows_a.entitlement)
    .bind(p.coop_a)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(joined, 1, "entitlement→return→share join must resolve");
    // Fresh in-scope INSERT into a batch-5 table.
    sqlx::query(
        "INSERT INTO social_aid_funds \
         (name, idempotency_key, idempotency_fingerprint, created_by, \
          cooperative_id, fund_number) \
         VALUES ('B5Ins','b5-ins','b5-insfp',$1,$2, \
                 public.allocate_document_number($2,'social_aid_funds.fund_number'))",
    )
    .bind(p.user_a)
    .bind(p.coop_a)
    .execute(&mut *tx)
    .await
    .expect("in-scope batch-5 INSERT must succeed");
    tx.commit().await.unwrap();

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM social_aid_funds WHERE cooperative_id = $1")
            .bind(p.coop_a)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(count, 2);
    tx.rollback().await.unwrap();
    app.close().await;
    db.cleanup().await;
}

/// Batch 5: rollback and pooled-connection reuse never leak the
/// sealed context into a later checkout.
#[tokio::test]
async fn batch5_rollback_and_reuse_do_not_leak() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch5_rows(&db, p.coop_a, p.user_a, "a").await;
    let app = db.app_pool().await;

    let (tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .unwrap();
    drop(tx); // rollback

    for (table, row) in batch5_row_ids(&rows_a) {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(row)
            .fetch_one(&app)
            .await
            .unwrap();
        assert_eq!(seen, 0, "{table}: post-rollback checkout must see nothing");
    }
    app.close().await;
    db.cleanup().await;
}

// ---------------------------------------------------------------------
// M1-P4 Batch 6 (migration 0045) — sealed-RLS rollout on the financial
// core plus the last independent leaf:
//   financial_accounts, financial_categories, account_movements,
//   account_transfers, governance_memberships
// ---------------------------------------------------------------------

/// Batch-6 (migration 0045) tables under RLS — the financial core
/// (every referrer is already forced or in-set) and the independent
/// governance-membership leaf.
const BATCH6_TABLES: [&str; 5] = [
    "financial_accounts",
    "financial_categories",
    "account_movements",
    "account_transfers",
    "governance_memberships",
];

/// Row ids of the batch-6 rows seeded per cooperative (one movement is
/// seeded standalone; the transfer carries its own pair implicitly).
struct Batch6Rows {
    account: Uuid,
    category: Uuid,
    movement: Uuid,
    transfer: Uuid,
    membership: Uuid,
}

/// Seed one row in every batch-6 table for `coop`, all through the
/// owner lane (RLS-unaffected). `body` is an existing governance body
/// of the same cooperative (from the pilot fixture).
async fn batch6_rows(db: &TestDb, coop: Uuid, user: Uuid, body: Uuid, tag: &str) -> Batch6Rows {
    let account: Uuid = sqlx::query_scalar(
        "INSERT INTO financial_accounts (name, account_type, created_by, cooperative_id) \
         VALUES ($2,'cash',$3,$1) RETURNING id",
    )
    .bind(coop)
    .bind(format!("B6Kasa{tag}"))
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let category: Uuid = sqlx::query_scalar(
        "INSERT INTO financial_categories (category_type, name, cooperative_id) \
         VALUES ('income',$2,$1) RETURNING id",
    )
    .bind(coop)
    .bind(format!("B6Kat{tag}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let movement: Uuid = sqlx::query_scalar(
        "INSERT INTO account_movements \
         (account_id, direction, amount, source_type, source_id, occurred_at, \
          created_by, cooperative_id) \
         VALUES ($1,'inflow',50,'income',$2,now(),$3,$4) RETURNING id",
    )
    .bind(account)
    .bind(Uuid::new_v4())
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let dest_account: Uuid = sqlx::query_scalar(
        "INSERT INTO financial_accounts (name, account_type, created_by, cooperative_id) \
         VALUES ($2,'bank',$3,$1) RETURNING id",
    )
    .bind(coop)
    .bind(format!("B6Banka{tag}"))
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let transfer: Uuid = sqlx::query_scalar(
        "INSERT INTO account_transfers \
         (transfer_number, source_account_id, destination_account_id, amount, \
          occurred_at, idempotency_key, idempotency_fingerprint, created_by, \
          cooperative_id) \
         VALUES (public.allocate_document_number($4,'account_transfers.transfer_number'), \
                 $1,$2,25,now(),$5,$6,$3,$4) RETURNING id",
    )
    .bind(account)
    .bind(dest_account)
    .bind(user)
    .bind(coop)
    .bind(format!("b6tr-{tag}"))
    .bind(format!("b6trfp-{tag}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    // Production always posts BOTH movement legs for a transfer — the
    // reversal package's destination-balance check depends on them.
    for (acct, dir) in [(account, "outflow"), (dest_account, "inflow")] {
        sqlx::query(
            "INSERT INTO account_movements \
             (account_id, direction, amount, source_type, source_id, occurred_at, \
              created_by, cooperative_id) \
             VALUES ($1,$2::text,25,'transfer',$3,now(),$4,$5)",
        )
        .bind(acct)
        .bind(dir)
        .bind(transfer)
        .bind(user)
        .bind(coop)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    let person: Uuid = sqlx::query_scalar(
        "INSERT INTO persons (first_name, last_name, search_name, cooperative_id) \
         VALUES ($2,'T','t',$1) RETURNING id",
    )
    .bind(coop)
    .bind(format!("P6{tag}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let membership: Uuid = sqlx::query_scalar(
        "INSERT INTO governance_memberships \
         (body_id, person_id, started_at, idempotency_key, \
          idempotency_fingerprint, created_by, cooperative_id) \
         VALUES ($1,$2,now(),$3,$4,$5,$6) RETURNING id",
    )
    .bind(body)
    .bind(person)
    .bind(format!("b6gm-{tag}"))
    .bind(format!("b6gmfp-{tag}"))
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    Batch6Rows {
        account,
        category,
        movement,
        transfer,
        membership,
    }
}

/// (table, id) pairs of the seeded batch-6 rows.
fn batch6_row_ids(rows: &Batch6Rows) -> Vec<(&'static str, Uuid)> {
    vec![
        ("financial_accounts", rows.account),
        ("financial_categories", rows.category),
        ("account_movements", rows.movement),
        ("account_transfers", rows.transfer),
        ("governance_memberships", rows.membership),
    ]
}

/// Batch 6: every selected table fails closed when no sealed context
/// exists — zero rows on SELECT, WITH CHECK rejection on INSERT.
#[tokio::test]
async fn batch6_missing_context_fails_closed() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let _rows_a = batch6_rows(&db, p.coop_a, p.user_a, p.body_a, "a").await;
    let app = db.app_pool().await;

    for table in BATCH6_TABLES {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&app)
            .await
            .unwrap();
        assert_eq!(seen, 0, "{table}: unauthenticated SELECT must see nothing");
    }
    let err = sqlx::query(
        "INSERT INTO financial_accounts (name, account_type, created_by, cooperative_id) \
         VALUES ('x','cash',$1,$2)",
    )
    .bind(p.user_a)
    .bind(p.coop_a)
    .execute(&app)
    .await
    .expect_err("unauthenticated INSERT must be denied");
    let msg = err.to_string();
    assert!(
        msg.contains("row-level security")
            || msg.contains("row_security")
            || msg.contains("insufficient_privilege"),
        "INSERT must fail on the RLS WITH CHECK, got: {msg}"
    );
    app.close().await;
    db.cleanup().await;
}

/// Batch 6: a raw `set_config` stamp (forge attempt — no seal chain)
/// must still see zero rows on every batch table.
#[tokio::test]
async fn batch6_forged_guc_context_sees_nothing() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch6_rows(&db, p.coop_a, p.user_a, p.body_a, "a").await;
    let app = db.app_pool().await;

    let mut conn = stamp(&app, p.coop_a).await;
    for (table, row) in batch6_row_ids(&rows_a) {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(row)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(
            seen, 0,
            "{table}: unsealed GUC stamp must not satisfy the policy"
        );
    }
    sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await.unwrap();
    drop(conn);
    app.close().await;
    db.cleanup().await;
}

/// Batch 6: the sealed request transaction sees exactly its own
/// cooperative's rows on every batch table — never the foreign coop's.
#[tokio::test]
async fn batch6_cross_cooperative_reads_are_isolated() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch6_rows(&db, p.coop_a, p.user_a, p.body_a, "a").await;
    let rows_b = batch6_rows(&db, p.coop_b, p.user_c, p.body_b, "b").await;
    let app = db.app_pool().await;

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    for ((table, a_row), (_, b_row)) in batch6_row_ids(&rows_a)
        .into_iter()
        .zip(batch6_row_ids(&rows_b))
    {
        let own: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(a_row)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(own, 1, "{table}: own row must be visible");
        let foreign: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
                .bind(b_row)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(foreign, 0, "{table}: foreign row must be invisible");
    }
    tx.rollback().await.unwrap();
    app.close().await;
    db.cleanup().await;
}

/// Batch 6: cross-coop writes are rejected — foreign UPDATE/DELETE hit
/// zero rows, cooperative_id reassignment on an own row is denied, and
/// a cross-coop account_movements insert fails the composite FK.
#[tokio::test]
async fn batch6_cross_cooperative_writes_and_reassignment_rejected() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch6_rows(&db, p.coop_a, p.user_a, p.body_a, "a").await;
    let rows_b = batch6_rows(&db, p.coop_b, p.user_c, p.body_b, "b").await;
    let app = db.app_pool().await;

    for ((table, a_row), (_, b_row)) in batch6_row_ids(&rows_a)
        .into_iter()
        .zip(batch6_row_ids(&rows_b))
    {
        let (tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
            .await
            .expect("sealed request tx");
        // R3 (0054): cooperative_id left the runtime UPDATE surface and
        // DELETE was revoked on financial/history tables — the writes
        // now die at the grant layer (42501) before row visibility is
        // even consulted. A privilege error aborts the tx, so each
        // probe runs in its own sealed request tx.
        drop(tx);
        let upd = {
            let (mut tx, _c) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!(
                "UPDATE {table} SET cooperative_id = cooperative_id WHERE id = $1"
            ))
            .bind(b_row)
            .execute(&mut *tx)
            .await
        };
        assert!(
            matches!(upd, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{table}: UPDATE must be denied at the grant layer"
        );
        let del = {
            let (mut tx, _c) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!("DELETE FROM {table} WHERE id = $1"))
                .bind(b_row)
                .execute(&mut *tx)
                .await
        };
        match del {
            // assessment_rules keeps DELETE (draft cleanup) but the
            // foreign row stays invisible under USING -> 0 rows.
            Ok(res) if table == "assessment_rules" => {
                assert_eq!(res.rows_affected(), 0, "{table}: foreign DELETE")
            }
            Ok(_) => panic!("{table}: DELETE must be denied — grant revoked in 0054"),
            Err(sqlx::Error::Database(ref e)) => {
                assert_eq!(
                    e.code().as_deref(),
                    Some("42501"),
                    "{table}: DELETE must be permission-denied"
                );
            }
            Err(e) => panic!("{table}: unexpected DELETE error {e}"),
        }
        let reassign = {
            let (mut tx, _c) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!(
                "UPDATE {table} SET cooperative_id = $1 WHERE id = $2"
            ))
            .bind(p.coop_b)
            .bind(a_row)
            .execute(&mut *tx)
            .await
        };
        assert!(
            matches!(reassign, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{table}: reassignment must fail"
        );
    }

    // Composite-FK proof: an own-coop movement cannot point at a
    // foreign account — RLS hides the parent, but the composite
    // (cooperative_id, account_id) key rejects independently.
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    let fk = sqlx::query(
        "INSERT INTO account_movements \
         (account_id, direction, amount, source_type, source_id, occurred_at, \
          created_by, cooperative_id) \
         VALUES ($1,'inflow',10,'income',$2,now(),$3,$4)",
    )
    .bind(rows_b.account)
    .bind(Uuid::new_v4())
    .bind(p.user_a)
    .bind(p.coop_a)
    .execute(&mut *tx)
    .await;
    assert!(
        fk.is_err(),
        "cross-coop movement→account must fail the composite FK"
    );
    tx.rollback().await.unwrap();
    app.close().await;
    db.cleanup().await;
}

/// Batch 6: legitimate sealed-context workflows still work — own-row
/// visibility, in-scope touch updates, a transfer chain join and an
/// in-scope movement INSERT against an own account.
#[tokio::test]
async fn batch6_legitimate_workflow_still_works() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    r3_grant_perms(
        &db,
        p.coop_a,
        p.user_a,
        &["financial_accounts.manage", "income_expense.manage"],
    )
    .await;
    let rows_a = batch6_rows(&db, p.coop_a, p.user_a, p.body_a, "a").await;
    let app = db.app_pool().await;

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    for (table, row) in batch6_row_ids(&rows_a) {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(row)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(seen, 1, "{table}: own row must be visible");
        // R3 (0054): the attribution column left the runtime UPDATE
        // surface — even in-scope, a cooperative_id write is now a
        // grant-layer error. Probe in its own tx (a privilege error
        // aborts the transaction).
        let touch = {
            let (mut t2, _c2) =
                app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                    .await
                    .expect("sealed request tx");
            sqlx::query(&format!(
                "UPDATE {table} SET cooperative_id = cooperative_id WHERE id = $1"
            ))
            .bind(row)
            .execute(&mut *t2)
            .await
        };
        assert!(
            matches!(touch, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{table}: attribution column must not be writable"
        );
    }
    // Ledger join: movement → account resolves inside the sealed scope.
    let joined: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_movements m \
         JOIN financial_accounts a ON a.id = m.account_id \
         WHERE m.id = $1 AND a.cooperative_id = $2",
    )
    .bind(rows_a.movement)
    .bind(p.coop_a)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(joined, 1, "movement→account join must resolve");
    // Fresh in-scope movement against the own account — R4 requires a
    // resolvable same-cooperative source, so post the expense entry
    // with its account_movement_id backlink in the same transaction.
    let expense_category: Uuid = sqlx::query_scalar(
        "INSERT INTO financial_categories (category_type, name, cooperative_id) \
         VALUES ('expense',$2,$1) RETURNING id",
    )
    .bind(p.coop_a)
    .bind(format!("B6Gider{}", &p.coop_a.to_string()[..6]))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let expense_id = Uuid::new_v4();
    let movement: Uuid = sqlx::query_scalar(
        "INSERT INTO account_movements \
         (account_id, direction, amount, source_type, source_id, occurred_at, \
          created_by, cooperative_id) \
         VALUES ($1,'outflow',5,'expense',$2,now(),$3,$4) RETURNING id",
    )
    .bind(rows_a.account)
    .bind(expense_id)
    .bind(p.user_a)
    .bind(p.coop_a)
    .fetch_one(&mut *tx)
    .await
    .expect("in-scope movement INSERT must succeed");
    sqlx::query(
        "INSERT INTO expense_entries \
         (id, financial_account_id, category_id, amount, occurred_at, \
          description, account_movement_id, idempotency_key, \
          idempotency_fingerprint, created_by, cooperative_id, \
          expense_number) \
         VALUES ($1,$2,$3,5,now(),'b6 gider',$4,'b6exp','b6expfp',$5,$6, \
                 public.allocate_document_number($6,'expense_entries.expense_number'))",
    )
    .bind(expense_id)
    .bind(rows_a.account)
    .bind(expense_category)
    .bind(movement)
    .bind(p.user_a)
    .bind(p.coop_a)
    .execute(&mut *tx)
    .await
    .expect("linked expense entry");
    tx.commit().await.unwrap();

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM account_movements WHERE cooperative_id = $1")
            .bind(p.coop_a)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    // Seeded loose movement + two transfer legs + the one just inserted.
    assert_eq!(count, 4);
    tx.rollback().await.unwrap();
    app.close().await;
    db.cleanup().await;
}

/// Batch 6: rollback and pooled-connection reuse never leak the
/// sealed context into a later checkout.
#[tokio::test]
async fn batch6_rollback_and_reuse_do_not_leak() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch6_rows(&db, p.coop_a, p.user_a, p.body_a, "a").await;
    let app = db.app_pool().await;

    let (tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .unwrap();
    drop(tx); // rollback

    for (table, row) in batch6_row_ids(&rows_a) {
        let seen: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE id = $1"))
            .bind(row)
            .fetch_one(&app)
            .await
            .unwrap();
        assert_eq!(seen, 0, "{table}: post-rollback checkout must see nothing");
    }
    app.close().await;
    db.cleanup().await;
}

// ---------------------------------------------------------------------
// M1-P4 S1 (migration 0046) — FORCE RLS on cooperative_number_counters.
// The table carries ONLY the owner lane: kooperatif_app holds no
// direct grants AND sees zero rows — all numbering flows through the
// P3C SECURITY DEFINER allocators.
// ---------------------------------------------------------------------

/// S1: the RLS layer itself is proven — temporarily GRANT the runtime
/// role full privileges on the counter table (owner lane), then show
/// kooperatif_app still cannot see or touch a single row, sealed or
/// not (there is no app policy — under FORCE RLS that is a full deny).
/// The privileges are revoked again through the owner lane.
#[tokio::test]
async fn s1_counters_invisible_to_app_even_in_sealed_tx() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    // Seed a counter row via the owner lane — the series key must
    // exist in document_number_series (FK), registered owner-side.
    sqlx::query("INSERT INTO document_number_series (series_key) VALUES ('s1.probe')")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value) \
         VALUES ($1, 's1.probe', 7)",
    )
    .bind(p.coop_a)
    .execute(&db.pool)
    .await
    .unwrap();
    // Grant-level proof of the RLS layer: hand the runtime role every
    // privilege — FORCE RLS must still deny everything.
    sqlx::query(
        "GRANT SELECT, INSERT, UPDATE, DELETE \
         ON cooperative_number_counters TO kooperatif_app",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let app = db.app_pool().await;

    // Bare pool: SELECT sees nothing; UPDATE/DELETE hit nothing;
    // INSERT is rejected by row-level security.
    let seen: i64 = sqlx::query_scalar("SELECT count(*) FROM cooperative_number_counters")
        .fetch_one(&app)
        .await
        .unwrap();
    assert_eq!(seen, 0, "RLS must hide counters even WITH grants");
    let upd = sqlx::query("UPDATE cooperative_number_counters SET next_value = 99")
        .execute(&app)
        .await
        .unwrap();
    assert_eq!(upd.rows_affected(), 0, "RLS must block UPDATE");
    let del = sqlx::query("DELETE FROM cooperative_number_counters")
        .execute(&app)
        .await
        .unwrap();
    assert_eq!(del.rows_affected(), 0, "RLS must block DELETE");
    let ins = sqlx::query(
        "INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value) \
         VALUES ($1, 's1.probe', 1)",
    )
    .bind(p.coop_a)
    .execute(&app)
    .await;
    assert!(ins.is_err(), "counter INSERT must be denied by RLS");

    // Sealed cooperative tx: still zero visibility — no app lane exists.
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    let sealed_seen: i64 = sqlx::query_scalar("SELECT count(*) FROM cooperative_number_counters")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(sealed_seen, 0, "sealed tx must not see counters either");
    tx.rollback().await.unwrap();
    app.close().await;

    // Restore the intended grant surface (the disposable DB is dropped
    // anyway, but the invariant is re-asserted for readability).
    sqlx::query(
        "REVOKE SELECT, INSERT, UPDATE, DELETE \
         ON cooperative_number_counters FROM kooperatif_app",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    db.cleanup().await;
}

/// S1: the legitimate allocation path still works end-to-end inside a
/// sealed tx — own-coop allocate/reserve succeed, foreign-coop calls
/// are refused by the allocator's own seal gate (not by RLS).
#[tokio::test]
async fn s1_sealed_allocation_and_reservation_work() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    sqlx::query("INSERT INTO document_number_series (series_key) VALUES ('s1.series')")
        .execute(&db.pool)
        .await
        .unwrap();
    let app = db.app_pool().await;

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    let n1: i64 = sqlx::query_scalar("SELECT public.allocate_document_number($1, 's1.series')")
        .bind(p.coop_a)
        .fetch_one(&mut *tx)
        .await
        .expect("own-coop allocation must succeed");
    assert_eq!(n1, 1, "fresh series must start at 1");
    let floor: i64 =
        sqlx::query_scalar("SELECT public.reserve_document_number($1, 's1.series', 50)")
            .bind(p.coop_a)
            .fetch_one(&mut *tx)
            .await
            .expect("own-coop reservation must succeed");
    assert_eq!(floor, 51, "reservation must raise the floor past 50");
    let foreign_alloc =
        sqlx::query_scalar::<_, i64>("SELECT public.allocate_document_number($1, 's1.series')")
            .bind(p.coop_b)
            .fetch_one(&mut *tx)
            .await;
    assert!(foreign_alloc.is_err(), "foreign allocation must be denied");
    tx.rollback().await.unwrap(); // context error poisoned the tx; discard

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    let foreign_reserve =
        sqlx::query_scalar::<_, i64>("SELECT public.reserve_document_number($1, 's1.series', 9)")
            .bind(p.coop_b)
            .fetch_one(&mut *tx)
            .await;
    assert!(
        foreign_reserve.is_err(),
        "foreign reservation must be denied"
    );
    tx.rollback().await.unwrap();
    app.close().await;
    db.cleanup().await;
}

/// S1: forged GUCs cannot reach the counters — a raw set_config stamp
/// still fails the allocator's sealed-context gate.
#[tokio::test]
async fn s1_forged_guc_cannot_allocate() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    sqlx::query("INSERT INTO document_number_series (series_key) VALUES ('s1.forged')")
        .execute(&db.pool)
        .await
        .unwrap();
    let app = db.app_pool().await;

    let mut conn = stamp(&app, p.coop_a).await;
    let forged =
        sqlx::query_scalar::<_, i64>("SELECT public.allocate_document_number($1, 's1.forged')")
            .bind(p.coop_a)
            .fetch_one(&mut *conn)
            .await;
    assert!(
        forged.is_err(),
        "allocator must reject an unsealed GUC stamp"
    );
    sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await.unwrap();
    drop(conn);
    app.close().await;
    db.cleanup().await;
}

/// S1: rollback does not consume a number, and concurrent sealed
/// allocations stay unique and monotonic.
#[tokio::test]
async fn s1_rollback_and_concurrency_preserve_numbering() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    sqlx::query("INSERT INTO document_number_series (series_key) VALUES ('s1.rb'), ('s1.cc')")
        .execute(&db.pool)
        .await
        .unwrap();
    let app = db.app_pool().await;

    // Rollback path: allocate then roll back — the next allocation
    // must reuse the same number (no silent gap).
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    let rolled: i64 = sqlx::query_scalar("SELECT public.allocate_document_number($1, 's1.rb')")
        .bind(p.coop_a)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx");
    let after: i64 = sqlx::query_scalar("SELECT public.allocate_document_number($1, 's1.rb')")
        .bind(p.coop_a)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(after, rolled, "rolled-back number must be reused");
    tx.commit().await.unwrap();

    // Concurrency path: two sealed txs race the same series; the row
    // lock serializes them so each commits a distinct value.
    let (mut tx1, _c1) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx 1");
    let v1: i64 = sqlx::query_scalar("SELECT public.allocate_document_number($1, 's1.cc')")
        .bind(p.coop_a)
        .fetch_one(&mut *tx1)
        .await
        .unwrap();
    let app2 = app.clone();
    let token = p.token_a.clone();
    let coop = p.coop_a;
    let second = tokio::spawn(async move {
        let (mut tx2, _c2) = app_db::request_tx(&app2, &token, coop, OffsetDateTime::now_utc())
            .await
            .expect("sealed request tx 2");
        let v2: i64 = sqlx::query_scalar("SELECT public.allocate_document_number($1, 's1.cc')")
            .bind(coop)
            .fetch_one(&mut *tx2)
            .await
            .unwrap();
        tx2.commit().await.unwrap();
        v2
    });
    tx1.commit().await.unwrap();
    let v2 = second.await.unwrap();
    assert_ne!(v1, v2, "concurrent allocations must be unique");
    assert_eq!(v2 - v1, 1, "concurrent allocations must be monotonic");

    app.close().await;
    db.cleanup().await;
}

/// S1: the owner lane keeps direct administrative access working.
#[tokio::test]
async fn s1_owner_lane_operations_unaffected() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    sqlx::query("INSERT INTO document_number_series (series_key) VALUES ('s1.owner')")
        .execute(&db.pool)
        .await
        .unwrap();

    let id: (Uuid, String) = sqlx::query_as(
        "INSERT INTO cooperative_number_counters (cooperative_id, series_key, next_value) \
         VALUES ($1, 's1.owner', 3) RETURNING cooperative_id, series_key",
    )
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let seen: i64 = sqlx::query_scalar(
        "SELECT next_value FROM cooperative_number_counters \
         WHERE cooperative_id = $1 AND series_key = 's1.owner'",
    )
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(seen, 3);
    let upd = sqlx::query(
        "UPDATE cooperative_number_counters SET next_value = 4 \
         WHERE cooperative_id = $1 AND series_key = 's1.owner'",
    )
    .bind(p.coop_a)
    .execute(&db.pool)
    .await
    .unwrap();
    assert_eq!(upd.rows_affected(), 1);
    let del = sqlx::query(
        "DELETE FROM cooperative_number_counters \
         WHERE cooperative_id = $1 AND series_key = $2",
    )
    .bind(id.0)
    .bind(&id.1)
    .execute(&db.pool)
    .await
    .unwrap();
    assert_eq!(del.rows_affected(), 1);

    db.cleanup().await;
}

// ─────────────────────────────────────────────────────────────────
// M1-P4-S2A (migration 0047) — credential-bound authorization readers.
//
// The pre-context membership/permission readers moved behind
// `app_session_*` SECURITY DEFINER functions that derive the principal
// from the session token hash inside the database. These tests run as
// the REAL `kooperatif_app` role and prove:
//   * an unknown / expired / revoked credential resolves to nobody —
//     every reader returns empty (fail closed, no oracle);
//   * the derived identity can never be widened by parameters —
//     `app_session_membership` answers only for the session owner, so a
//     foreign cooperative or a non-member pair yields zero rows;
//   * scoped permission resolution still enforces the active-membership
//     join and the bootstrap-only NULL-attribution arm — a caller cannot
//     escalate by pointing at another cooperative;
//   * the legacy global lane re-verifies `never_bootstrapped` inside the
//     database — on a tenanted database it returns empty;
//   * the readers stamp no `app.*` GUC — pool reuse carries nothing;
//   * internal helpers (`app_session_owner`, `app_session_legacy_lane_ok`)
//     stay unexecutable by the runtime role.
// ─────────────────────────────────────────────────────────────────

/// Garbage / revoked / expired credentials resolve to nobody: every
/// S2A reader returns empty and never reveals whether a user or
/// cooperative exists.
#[tokio::test]
async fn s2a_readers_fail_closed_without_a_valid_session() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();
    let garbage: Vec<u8> = vec![0xEE; 32];

    // Owner lane seeds a revoked and an expired session for user_a.
    sqlx::query(
        "INSERT INTO user_sessions \
           (user_id, token_hash, csrf_token, created_at, last_seen_at, \
            expires_at, idle_expires_at, revoked_at) \
         VALUES ($1, $2, 'csrf', now(), now(), \
                 now() + interval '1 day', now() + interval '1 day', now()), \
                ($1, $3, 'csrf', now(), now(), \
                 now() - interval '1 second', now() + interval '1 day', NULL)",
    )
    .bind(p.user_a)
    .bind(vec![0x71u8; 32])
    .bind(vec![0x72u8; 32])
    .execute(&db.pool)
    .await
    .unwrap();

    for (label, hash) in [
        ("garbage", garbage.clone()),
        ("revoked", vec![0x71u8; 32]),
        ("expired", vec![0x72u8; 32]),
    ] {
        let memberships: i64 =
            sqlx::query_scalar("SELECT count(*) FROM public.app_session_memberships($1, $2)")
                .bind(&hash)
                .bind(now)
                .fetch_one(&app)
                .await
                .unwrap();
        assert_eq!(memberships, 0, "{label}: memberships must be empty");

        let membership: i64 = sqlx::query_scalar(
            "SELECT count(*) \
             FROM public.app_session_membership($1, $2, $3)",
        )
        .bind(&hash)
        .bind(p.coop_a)
        .bind(now)
        .fetch_one(&app)
        .await
        .unwrap();
        assert_eq!(membership, 0, "{label}: membership probe must be empty");

        let business_ok: bool =
            sqlx::query_scalar("SELECT public.app_session_business_membership_ok($1, $2)")
                .bind(&hash)
                .bind(now)
                .fetch_one(&app)
                .await
                .unwrap();
        assert!(!business_ok, "{label}: business gate must deny");

        let perms: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM public.app_session_permissions_scoped($1, $2, $3)",
        )
        .bind(&hash)
        .bind(p.coop_a)
        .bind(now)
        .fetch_one(&app)
        .await
        .unwrap();
        assert_eq!(perms, 0, "{label}: scoped permissions must be empty");

        let roles: i64 = sqlx::query_scalar(
            "SELECT count(*) \
             FROM public.app_session_role_summaries_scoped($1, $2, $3)",
        )
        .bind(&hash)
        .bind(p.coop_a)
        .bind(now)
        .fetch_one(&app)
        .await
        .unwrap();
        assert_eq!(roles, 0, "{label}: scoped role summaries must be empty");
    }

    db.cleanup().await;
}

/// The principal is derived from the credential — never from a
/// parameter: the owner's own memberships come back, foreign
/// cooperatives and non-member pairs are indistinguishable from
/// nonexistence.
#[tokio::test]
async fn s2a_readers_are_bound_to_the_session_owner() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    // user_a's own list: exactly coop A.
    let mine: Vec<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM public.app_session_memberships($1, $2)")
            .bind(&p.token_a)
            .bind(now)
            .fetch_all(&app)
            .await
            .unwrap();
    assert_eq!(mine, vec![p.coop_a], "owner sees exactly own memberships");

    // Membership probe: own coop answers; foreign coop is invisible.
    let own: Option<(String, String, String, bool)> = sqlx::query_as(
        "SELECT membership_status, cooperative_name, cooperative_status, \
                business_enabled \
         FROM public.app_session_membership($1, $2, $3)",
    )
    .bind(&p.token_a)
    .bind(p.coop_a)
    .bind(now)
    .fetch_optional(&app)
    .await
    .unwrap();
    let own = own.expect("own membership must resolve");
    assert_eq!(own.0, "active");
    assert_eq!(own.1, "Pilot A");
    assert_eq!(own.2, "active");
    assert!(own.3);

    let foreign: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.app_session_membership($1, $2, $3)")
            .bind(&p.token_a)
            .bind(p.coop_b)
            .bind(now)
            .fetch_one(&app)
            .await
            .unwrap();
    assert_eq!(foreign, 0, "foreign cooperative must be invisible");

    // The coop-B member's credential sees coop B — and nothing of A.
    // (token_b is user_b's session hash: 0x52 * 32.)
    let theirs: Vec<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM public.app_session_memberships($1, $2)")
            .bind(vec![0x52u8; 32])
            .bind(now)
            .fetch_all(&app)
            .await
            .unwrap();
    assert_eq!(theirs, vec![p.coop_b]);

    // user_c (member, no roles) — scoped permission set is empty even
    // though the cooperative itself resolves.
    let c_perms: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.app_session_permissions_scoped($1, $2, $3)",
    )
    .bind(vec![0x53u8; 32])
    .bind(p.coop_a)
    .bind(now)
    .fetch_one(&app)
    .await
    .unwrap();
    assert_eq!(c_perms, 0, "member without roles gets no permissions");

    db.cleanup().await;
}

/// Scoped permission resolution cannot be escalated: the derived
/// identity's own grants come back, the foreign cooperative yields an
/// empty set, and the legacy global lane is closed on a tenanted
/// database even for an otherwise-privileged session.
#[tokio::test]
async fn s2a_permissions_cannot_be_escalated() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    // Own scope: exactly the assigned grant (users.manage from the
    // fixture's pilot-admin role).
    let own: Vec<String> =
        sqlx::query_scalar("SELECT public.app_session_permissions_scoped($1, $2, $3)")
            .bind(&p.token_a)
            .bind(p.coop_a)
            .bind(now)
            .fetch_all(&app)
            .await
            .unwrap();
    assert_eq!(own, vec!["users.manage".to_string()]);

    // Foreign scope: user_a is not a member of coop B — empty.
    let foreign: Vec<String> =
        sqlx::query_scalar("SELECT public.app_session_permissions_scoped($1, $2, $3)")
            .bind(&p.token_a)
            .bind(p.coop_b)
            .bind(now)
            .fetch_all(&app)
            .await
            .unwrap();
    assert!(foreign.is_empty(), "foreign scope must resolve empty");

    // Equivalence: the definer's scoped answer equals the raw owner-lane
    // resolution for the same (user, scope) — semantics unchanged, only
    // the caller-binding moved.
    let owner_side: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT p.key \
         FROM user_role_assignments ura \
         JOIN roles r ON r.id = ura.role_id AND r.status = 'active' \
         JOIN cooperative_memberships m \
           ON m.cooperative_id = $2 AND m.user_id = ura.user_id \
          AND m.status = 'active' \
         JOIN role_permissions rp ON rp.role_id = r.id \
         JOIN permissions p ON p.id = rp.permission_id \
         WHERE ura.user_id = $1 AND ura.cooperative_id = $2",
    )
    .bind(p.user_a)
    .bind(p.coop_a)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        own, owner_side,
        "definer answer must equal owner-lane resolution"
    );

    // Role summaries carry the same binding.
    let summaries: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT role_id, role_name \
         FROM public.app_session_role_summaries_scoped($1, $2, $3)",
    )
    .bind(&p.token_a)
    .bind(p.coop_a)
    .bind(now)
    .fetch_all(&app)
    .await
    .unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].1, "pilot-admin");

    // Legacy global lane: the fixture is tenanted, so the
    // never-bootstrapped predicate is false and the definer returns
    // empty — even though the same user holds scoped grants.
    let legacy: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.app_session_permissions_legacy($1, $2)")
            .bind(&p.token_a)
            .bind(now)
            .fetch_one(&app)
            .await
            .unwrap();
    assert_eq!(legacy, 0, "legacy lane must be closed on a tenanted db");

    let legacy_roles: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.app_session_role_summaries_legacy($1, $2)")
            .bind(&p.token_a)
            .bind(now)
            .fetch_one(&app)
            .await
            .unwrap();
    assert_eq!(legacy_roles, 0);

    db.cleanup().await;
}

/// The readers stamp nothing: a pooled `kooperatif_app` connection
/// carries no `app.*` GUC after the calls, and a subsequently checked
/// out connection cannot reuse any residue.
#[tokio::test]
async fn s2a_readers_leave_no_context_residue() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    let mut conn = pool_conn(&app).await;
    let _: Vec<Uuid> =
        sqlx::query_scalar("SELECT cooperative_id FROM public.app_session_memberships($1, $2)")
            .bind(&p.token_a)
            .bind(now)
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    let _: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.app_session_permissions_scoped($1, $2, $3)",
    )
    .bind(&p.token_a)
    .bind(p.coop_a)
    .bind(now)
    .fetch_one(&mut *conn)
    .await
    .unwrap();

    for guc in [
        "app.user_id",
        "app.session_id",
        "app.cooperative_id",
        "app.ctx_txid",
        "app.user_seal",
        "app.ctx_seal",
    ] {
        let value: Option<String> = sqlx::query_scalar("SELECT current_setting($1, true)")
            .bind(guc)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert!(
            value.is_none() || value.as_deref() == Some(""),
            "{guc} must not be stamped by S2A readers"
        );
    }
    drop(conn);

    // A fresh checkout sees the same empty context — nothing leaked.
    let mut reused = pool_conn(&app).await;
    let leaked: Option<String> = sqlx::query_scalar("SELECT current_setting('app.user_id', true)")
        .fetch_one(&mut *reused)
        .await
        .unwrap();
    assert!(leaked.is_none() || leaked.as_deref() == Some(""));

    db.cleanup().await;
}

/// Internal helpers stay unreachable from the runtime role; the
/// request-facing readers keep their exact grant surface.
#[tokio::test]
async fn s2a_function_surface_is_bounded() {
    let db = setup().await;
    let app = db.app_pool().await;

    // Internal helpers stay unreachable — except
    // app_session_legacy_lane_ok(), which S3 promotes to the
    // never_bootstrapped probe (zero-arg boolean, no privilege
    // increase over the direct SELECT it replaced).
    let allowed: bool = sqlx::query_scalar(
        "SELECT has_function_privilege('kooperatif_app', \
         'public.app_session_owner(bytea,timestamptz)', 'EXECUTE')",
    )
    .fetch_one(&app)
    .await
    .unwrap();
    assert!(!allowed, "app_session_owner must stay internal");

    let probe: bool = sqlx::query_scalar(
        "SELECT has_function_privilege('kooperatif_app', \
         'public.app_session_legacy_lane_ok()', 'EXECUTE')",
    )
    .fetch_one(&app)
    .await
    .unwrap();
    assert!(probe, "S3: the never_bootstrapped probe must be reachable");

    for reader in [
        "app_session_membership(bytea,uuid,timestamptz)",
        "app_session_memberships(bytea,timestamptz)",
        "app_session_business_membership_ok(bytea,timestamptz)",
        "app_session_permissions_scoped(bytea,uuid,timestamptz)",
        "app_session_role_summaries_scoped(bytea,uuid,timestamptz)",
        "app_session_permissions_legacy(bytea,timestamptz)",
        "app_session_role_summaries_legacy(bytea,timestamptz)",
    ] {
        let allowed: bool =
            sqlx::query_scalar("SELECT has_function_privilege('kooperatif_app', $1, 'EXECUTE')")
                .bind(format!("public.{reader}"))
                .fetch_one(&app)
                .await
                .unwrap();
        assert!(allowed, "{reader} must be executable by kooperatif_app");

        // And every S2A function is hardened: definer + pinned path.
        let hardened: bool = sqlx::query_scalar(
            "SELECT p.prosecdef \
                AND p.proconfig::text[] @> ARRAY['search_path=public, pg_catalog'] \
               FROM pg_proc p \
              WHERE p.oid = $1::regprocedure",
        )
        .bind(format!("public.{reader}"))
        .fetch_one(&app)
        .await
        .unwrap();
        assert!(
            hardened,
            "{reader} must be SECURITY DEFINER with a pinned search_path"
        );
    }

    db.cleanup().await;
}

// ────────────────────────────────────────────────────────────────────
// S2B — sealed security-event WRITE boundary (migration 0048)
// ────────────────────────────────────────────────────────────────────

/// The mandatory lane derives ALL attribution from the sealed context:
/// the sealed user/session/cooperative land on the row while the caller
/// can only choose event_type + metadata. Written inside the request
/// transaction it rolls back with the protected operation.
#[tokio::test]
async fn s2b_ctx_audit_writes_sealed_attribution_and_rolls_back() {
    let db = setup().await;
    let _p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    let (mut tx, _ctx) =
        app_db::request_tx(&app, &_p.token_a, _p.coop_a, OffsetDateTime::now_utc())
            .await
            .expect("sealed tx");

    sqlx::query("SELECT public.app_ctx_audit('shareholder_family_changed', $1)")
        .bind(serde_json::json!({"probe": "s2b"}))
        .execute(&mut *tx)
        .await
        .expect("sealed audit write");

    // Uncommitted: invisible to a second connection.
    let uncommitted: i64 =
        sqlx::query_scalar("SELECT count(*) FROM security_events WHERE metadata->>'probe' = 's2b'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(uncommitted, 0, "audit row must be part of the open tx");

    tx.rollback().await.unwrap();
    let after_rollback: i64 =
        sqlx::query_scalar("SELECT count(*) FROM security_events WHERE metadata->>'probe' = 's2b'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(after_rollback, 0, "rollback must erase the audit row");

    // And on a committed tx the row carries ONLY seal-derived identity.
    let (mut tx, _ctx) =
        app_db::request_tx(&app, &_p.token_a, _p.coop_a, OffsetDateTime::now_utc())
            .await
            .expect("sealed tx");
    sqlx::query("SELECT public.app_ctx_audit('shareholder_family_changed', $1)")
        .bind(serde_json::json!({"probe": "s2b-commit"}))
        .execute(&mut *tx)
        .await
        .expect("sealed audit write");
    tx.commit().await.unwrap();

    let row: (Uuid, Uuid, Option<Uuid>) = sqlx::query_as(
        "SELECT cooperative_id, user_id, session_id FROM security_events           WHERE metadata->>'probe' = 's2b-commit'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(row.0, _p.coop_a, "cooperative comes from the seal");
    assert_eq!(row.1, _p.user_a, "actor comes from the seal");
    assert!(row.2.is_some(), "session id derives from the seal");

    db.cleanup().await;
}

/// No seal, no write: bare pool calls, forged GUCs and stale-tx seals
/// all fail closed through the definer's app_ctx_ok() re-verification.
#[tokio::test]
async fn s2b_ctx_audit_fails_closed_without_a_seal() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    // Bare connection: nothing stamped.
    let bare = sqlx::query("SELECT public.app_ctx_audit('role_created', '{}'::jsonb)")
        .execute(&app)
        .await;
    assert!(bare.is_err(), "unsealed write must fail");

    // Forged GUC set: caller mints user/coop/txid/seal claims by hand.
    let mut conn = app.acquire().await.unwrap();
    sqlx::query("BEGIN").execute(&mut *conn).await.unwrap();
    for (k, v) in [
        ("app.user_id", p.user_a.to_string()),
        ("app.session_id", Uuid::new_v4().to_string()),
        ("app.cooperative_id", p.coop_a.to_string()),
        ("app.ctx_txid", "1".to_string()),
        ("app.ctx_seal", "deadbeef".to_string()),
        ("app.user_seal", "deadbeef".to_string()),
    ] {
        sqlx::query(&format!("SELECT set_config('{k}', '{v}', true)"))
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let forged = sqlx::query("SELECT public.app_ctx_audit('role_created', '{}'::jsonb)")
        .execute(&mut *conn)
        .await;
    assert!(forged.is_err(), "forged seal must fail");
    sqlx::query("ROLLBACK").execute(&mut *conn).await.unwrap();

    // A rolled-back sealed tx must leave no reusable seal on the
    // connection when it returns to the pool.
    {
        let (mut tx, _ctx) =
            app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                .await
                .expect("sealed tx");
        let ok: bool = sqlx::query_scalar("SELECT public.app_ctx_ok()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert!(ok, "seal must be live inside its own tx");
        tx.rollback().await.unwrap();
    }
    let mut conn2 = app.acquire().await.unwrap();
    let residue: Option<String> =
        sqlx::query_scalar("SELECT current_setting('app.ctx_seal', true)")
            .fetch_one(&mut *conn2)
            .await
            .unwrap();
    assert!(
        residue.is_none() || residue.as_deref() == Some(""),
        "pooled connection must not carry a stale seal"
    );
    let stale_write = sqlx::query("SELECT public.app_ctx_audit('role_created', '{}'::jsonb)")
        .execute(&mut *conn2)
        .await;
    assert!(stale_write.is_err(), "reused connection must stay closed");

    db.cleanup().await;
}

/// The session-bound scoped lane: lockout_prevented writes carry the
/// derived owner + verified membership scope. A session can never
/// attribute an event to a cooperative it does not actively belong to,
/// and a NULL cooperative is only valid on the never-bootstrapped lane
/// (which this fixture has already left — two cooperatives exist).
#[tokio::test]
async fn s2b_scoped_session_writer_enforces_membership_scope() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    // Legitimate: user_a is an active member of coop A.
    sqlx::query("SELECT public.app_session_audit_scoped($1, $2, 'lockout_prevented', $3, $4)")
        .bind(&p.token_a)
        .bind(p.coop_a)
        .bind(serde_json::json!({"operation": "role_disable"}))
        .bind(now)
        .execute(&app)
        .await
        .expect("in-scope lockout event");

    let row: (Uuid, Uuid) = sqlx::query_as(
        "SELECT cooperative_id, user_id FROM security_events \
          WHERE event_type = 'lockout_prevented'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(row.0, p.coop_a);
    assert_eq!(row.1, p.user_a, "actor is the session owner, not a claim");

    // Cross-cooperative injection: user_a's token cannot attribute to B.
    let foreign = sqlx::query(
        "SELECT public.app_session_audit_scoped($1, $2, 'lockout_prevented', '{}'::jsonb, $3)",
    )
    .bind(&p.token_a)
    .bind(p.coop_b)
    .bind(now)
    .execute(&app)
    .await;
    assert!(foreign.is_err(), "foreign scope must be rejected");

    // NULL cooperative outside the never-bootstrapped lane is refused.
    let null_scope = sqlx::query(
        "SELECT public.app_session_audit_scoped($1, NULL, 'lockout_prevented', '{}'::jsonb, $2)",
    )
    .bind(&p.token_a)
    .bind(now)
    .execute(&app)
    .await;
    assert!(null_scope.is_err(), "NULL scope post-bootstrap must fail");

    // Other event types are outside this lane.
    let wrong_type = sqlx::query(
        "SELECT public.app_session_audit_scoped($1, $2, 'payment_posted', '{}'::jsonb, $3)",
    )
    .bind(&p.token_a)
    .bind(p.coop_a)
    .bind(now)
    .execute(&app)
    .await;
    assert!(wrong_type.is_err(), "lane is whitelisted");

    // A revoked session resolves nothing.
    sqlx::query("UPDATE user_sessions SET revoked_at = now() WHERE token_hash = $1")
        .bind(&p.token_a)
        .execute(&db.pool)
        .await
        .unwrap();
    let dead = sqlx::query(
        "SELECT public.app_session_audit_scoped($1, $2, 'lockout_prevented', '{}'::jsonb, $3)",
    )
    .bind(&p.token_a)
    .bind(p.coop_a)
    .bind(now)
    .execute(&app)
    .await;
    assert!(dead.is_err(), "revoked session cannot write audit");

    db.cleanup().await;
}

/// Platform lane: login_succeeded is session-bound (actor + session id
/// derived from the hash, cooperative forced NULL) and whitelisted.
#[tokio::test]
async fn s2b_platform_lane_is_session_bound_and_whitelisted() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    sqlx::query("SELECT public.app_session_audit_platform($1, 'login_succeeded', $2, $3)")
        .bind(&p.token_a)
        .bind(serde_json::json!({"username": "pa-test"}))
        .bind(now)
        .execute(&app)
        .await
        .expect("platform event");

    let row: (Option<Uuid>, Uuid, Uuid) = sqlx::query_as(
        "SELECT cooperative_id, user_id, session_id FROM security_events \
          WHERE event_type = 'login_succeeded' AND metadata->>'username' = 'pa-test'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(row.0.is_none(), "platform lane is never coop-attributed");
    assert_eq!(row.1, p.user_a);
    // session_id is the resolved session row, not a caller claim.

    let forged = sqlx::query(
        "SELECT public.app_session_audit_platform($1, 'login_succeeded', '{}'::jsonb, $2)",
    )
    .bind(vec![0xEEu8; 32])
    .bind(now)
    .execute(&app)
    .await;
    assert!(forged.is_err(), "unknown token must fail");

    let wrong = sqlx::query(
        "SELECT public.app_session_audit_platform($1, 'role_created', '{}'::jsonb, $2)",
    )
    .bind(&p.token_a)
    .bind(now)
    .execute(&app)
    .await;
    assert!(wrong.is_err(), "non-whitelisted event must fail");

    db.cleanup().await;
}

/// The unauthenticated lane writes exactly one fixed event shape —
/// login_failed with NULL actor/session/cooperative — regardless of
/// what the caller supplies.
#[tokio::test]
async fn s2b_login_failed_lane_cannot_impersonate() {
    let db = setup().await;
    let _p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    sqlx::query("SELECT public.app_audit_login_failed($1, $2)")
        .bind(serde_json::json!({
            "username": "saldirgan",
            "reason": "invalid_credentials",
            "user_id": Uuid::new_v4().to_string(),
            "cooperative_id": Uuid::new_v4().to_string()
        }))
        .bind(OffsetDateTime::now_utc())
        .execute(&app)
        .await
        .expect("unauthenticated login_failed lane");

    let row: (String, Option<Uuid>, Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT event_type, cooperative_id, user_id, session_id \
           FROM security_events WHERE metadata->>'username' = 'saldirgan'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(row.0, "login_failed");
    assert!(
        row.1.is_none() && row.2.is_none() && row.3.is_none(),
        "metadata-supplied ids must never reach the row"
    );

    db.cleanup().await;
}

/// Revocation + audit are one atomic definer call: a revoked session
/// still leaves its session_revoked row, and 'logout' maps to the
/// logout event with empty metadata (former call-site shapes).
#[tokio::test]
async fn s2b_revocation_writes_its_event_atomically() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    // user_c has a session (token 0x53..) — user_a revokes own token.
    // The session id is resolved on the owner lane: kooperatif_app has
    // no SELECT on user_sessions (P3B isolation).
    let session_a: Uuid = sqlx::query_scalar("SELECT id FROM user_sessions WHERE token_hash = $1")
        .bind(&p.token_a)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let outcome: String =
        sqlx::query_scalar("SELECT public.app_session_revoke($1, $2, $3, 'user_revoked')")
            .bind(&p.token_a)
            .bind(session_a)
            .bind(now)
            .fetch_one(&app)
            .await
            .unwrap();
    assert_eq!(outcome, "revoked");

    let ev: (String, Uuid, Option<Uuid>) = sqlx::query_as(
        "SELECT event_type, user_id, cooperative_id FROM security_events \
          WHERE event_type = 'session_revoked' ORDER BY occurred_at DESC LIMIT 1",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(ev.0, "session_revoked");
    assert_eq!(ev.1, p.user_a);
    assert!(ev.2.is_none());

    // The revoked token can no longer write anything.
    let after = sqlx::query(
        "SELECT public.app_session_audit_platform($1, 'login_succeeded', '{}'::jsonb, $2)",
    )
    .bind(&p.token_a)
    .bind(now)
    .execute(&app)
    .await;
    assert!(after.is_err(), "revoked session must fail closed");

    db.cleanup().await;
}

/// Cooperative switching records `cooperative_context_switched`
/// atomically inside `app_session_set_cooperative` — attribution is the
/// session owner and the verified membership scope.
#[tokio::test]
async fn s2b_context_switch_is_audited_atomically() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    let ok: bool = sqlx::query_scalar("SELECT public.app_session_set_cooperative($1, $2)")
        .bind(&p.token_a)
        .bind(p.coop_a)
        .fetch_one(&app)
        .await
        .unwrap();
    assert!(ok);

    let ev: (Uuid, Uuid) = sqlx::query_as(
        "SELECT cooperative_id, user_id FROM security_events \
          WHERE event_type = 'cooperative_context_switched'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(ev, (p.coop_a, p.user_a));

    // A foreign cooperative refuses — and leaves no event.
    let denied: bool = sqlx::query_scalar("SELECT public.app_session_set_cooperative($1, $2)")
        .bind(&p.token_a)
        .bind(p.coop_b)
        .fetch_one(&app)
        .await
        .unwrap();
    assert!(!denied, "non-member cooperative must be refused");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events \
          WHERE event_type = 'cooperative_context_switched' AND cooperative_id = $1",
    )
    .bind(p.coop_b)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(n, 0, "refused switch must leave no audit row");

    db.cleanup().await;
}

/// The direct-INSERT surface is closed: kooperatif_app keeps SELECT
/// (never_bootstrapped probe) but can no longer write the table —
/// every runtime write flows through a hardened definer.
#[tokio::test]
async fn s2b_direct_insert_is_denied_to_the_runtime_role() {
    let db = setup().await;
    let _p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    let direct = sqlx::query(
        "INSERT INTO security_events (event_type, user_id, metadata) \
         VALUES ('login_succeeded', $1, '{}'::jsonb)",
    )
    .bind(Uuid::new_v4())
    .execute(&app)
    .await;
    assert!(direct.is_err(), "direct INSERT must be denied");

    // S3 (FORCE RLS) removes the last direct grant: the table is fully
    // closed to the runtime role — reads and writes alike go through
    // the definer boundary.
    let can_select: bool = sqlx::query_scalar(
        "SELECT has_table_privilege('kooperatif_app', 'public.security_events', 'SELECT')",
    )
    .fetch_one(&app)
    .await
    .unwrap();
    assert!(!can_select, "direct SELECT must be revoked in S3");

    db.cleanup().await;
}

/// The user-seal lane (request_user_tx, no cooperative): a sealed
/// user-only context may write platform/NULL-attributed events — that
/// is the legitimate never-bootstrapped surface. But a hand-forged
/// app.cooperative_id GUC must NOT downgrade to a NULL-attributed
/// write: a stamped cooperative requires its ctx_seal.
#[tokio::test]
async fn s2b_user_seal_lane_writes_null_attribution_and_rejects_forged_coop() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    // User-seal only (no app_stamp_cooperative call).
    let mut tx = app.begin().await.unwrap();
    let uid: Option<Uuid> = sqlx::query_scalar("SELECT public.app_open_request_context($1, $2)")
        .bind(&p.token_a)
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(uid, Some(p.user_a));

    sqlx::query("SELECT public.app_ctx_audit('shareholder_family_changed', $1)")
        .bind(serde_json::json!({"probe": "s2b-null"}))
        .execute(&mut *tx)
        .await
        .expect("user-sealed NULL-attributed write");
    tx.commit().await.unwrap();

    let row: (Option<Uuid>, Uuid) = sqlx::query_as(
        "SELECT cooperative_id, user_id FROM security_events \
          WHERE event_type = 'shareholder_family_changed' AND metadata->>'probe' = 's2b-null'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(row, (None, p.user_a));

    // A forged cooperative GUC on a user-seal-only tx is rejected —
    // it may NOT silently degrade to NULL attribution.
    let mut tx2 = app.begin().await.unwrap();
    sqlx::query_scalar::<_, Option<Uuid>>("SELECT public.app_open_request_context($1, $2)")
        .bind(&p.token_a)
        .bind(now)
        .fetch_one(&mut *tx2)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.cooperative_id', $1::text, true)")
        .bind(p.coop_a)
        .execute(&mut *tx2)
        .await
        .unwrap();
    let forged = sqlx::query("SELECT public.app_ctx_audit('role_created', '{}'::jsonb)")
        .execute(&mut *tx2)
        .await;
    assert!(forged.is_err(), "forged cooperative GUC must be rejected");
    tx2.rollback().await.unwrap();

    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events \
          WHERE event_type = 'role_created' AND metadata = '{}'::jsonb \
            AND occurred_at > $1",
    )
    .bind(now - time::Duration::minutes(1))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(n, 0, "forged write must leave no row");

    db.cleanup().await;
}

// ── S3: FORCE RLS on security_events ──────────────────────────────

/// kooperatif_app holds NO direct privilege on security_events: even
/// SELECT is gone — the table is fully closed outside the definer
/// boundary.
#[tokio::test]
async fn s3_security_events_fully_closed_to_runtime_role() {
    let db = setup().await;
    let _p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    for sql in [
        "SELECT event_type FROM security_events",
        "INSERT INTO security_events (event_type, metadata) VALUES ('x','{}')",
        "UPDATE security_events SET metadata = '{}'::jsonb",
        "DELETE FROM security_events",
    ] {
        let res = sqlx::query(sql).execute(&app).await;
        assert!(res.is_err(), "direct statement must be denied: {sql}");
    }

    db.cleanup().await;
}

/// RLS is enforced INDEPENDENTLY of grants: with a temporary owner-lane
/// GRANT the runtime role still sees zero rows and cannot write —
/// there is deliberately no kooperatif_app policy. Grants are revoked
/// again inside the disposable fixture.
#[tokio::test]
async fn s3_rls_independently_enforced_under_temporary_grants() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    // Seed one legitimate event via the boundary.
    sqlx::query("SELECT public.app_audit_login_failed($1, $2)")
        .bind(serde_json::json!({"probe": "s3"}))
        .bind(OffsetDateTime::now_utc())
        .execute(&app)
        .await
        .unwrap();

    sqlx::query("GRANT SELECT, INSERT, UPDATE, DELETE ON security_events TO kooperatif_app")
        .execute(&db.pool)
        .await
        .unwrap();

    let seen: i64 =
        sqlx::query_scalar("SELECT count(*) FROM security_events WHERE metadata->>'probe' = 's3'")
            .fetch_one(&app)
            .await
            .unwrap();
    assert_eq!(seen, 0, "RLS invisibility despite granted SELECT");

    // INSERT hits the WITH CHECK wall; UPDATE/DELETE see zero rows
    // (invisible rows are a silent no-op under RLS — same nuance as S1).
    let ins = sqlx::query(
        "INSERT INTO security_events (event_type, metadata) \
         VALUES ('role_created','{}'::jsonb)",
    )
    .execute(&app)
    .await;
    assert!(ins.is_err(), "INSERT must hit the RLS wall");
    for sql in [
        "UPDATE security_events SET metadata = '{}'::jsonb",
        "DELETE FROM security_events",
    ] {
        let res = sqlx::query(sql).execute(&app).await.unwrap();
        assert_eq!(res.rows_affected(), 0, "invisible rows: {sql}");
    }

    // Even inside a sealed cooperative tx the app role sees nothing —
    // no app policy exists at all.
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed tx");
    let sealed_seen: i64 = sqlx::query_scalar("SELECT count(*) FROM security_events")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(sealed_seen, 0, "no app policy — sealed tx sees nothing");

    sqlx::query("REVOKE SELECT, INSERT, UPDATE, DELETE ON security_events FROM kooperatif_app")
        .execute(&db.pool)
        .await
        .unwrap();
    tx.rollback().await.unwrap();

    db.cleanup().await;
}

/// The write boundary still works under FORCE RLS: definer bodies run
/// as the owner and pass the owner lane. Sealed ctx writes stay
/// atomic, session-bound and anonymous writers still mint rows, and
/// the never_bootstrapped probe (now a definer call) still answers.
#[tokio::test]
async fn s3_definer_writers_and_probe_survive_force_rls() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    // Sealed ctx write — mandatory lane.
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("sealed tx");
    sqlx::query("SELECT public.app_ctx_audit('assessments_generated', $1)")
        .bind(serde_json::json!({"probe": "s3-sealed"}))
        .execute(&mut *tx)
        .await
        .expect("sealed write under FORCE RLS");
    tx.rollback().await.unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events WHERE metadata->>'probe' = 's3-sealed'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(n, 0, "rolled-back sealed write leaves no row");

    // Session-bound platform + anonymous lanes.
    sqlx::query("SELECT public.app_session_audit_platform($1, 'login_succeeded', $2, $3)")
        .bind(&p.token_a)
        .bind(serde_json::json!({"probe": "s3-platform"}))
        .bind(now)
        .execute(&app)
        .await
        .expect("platform write");
    sqlx::query("SELECT public.app_audit_login_failed($1, $2)")
        .bind(serde_json::json!({"probe": "s3-anon"}))
        .bind(now)
        .execute(&app)
        .await
        .expect("anonymous write");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events \
          WHERE metadata->>'probe' IN ('s3-platform','s3-anon')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(n, 2);

    // never_bootstrapped probe on a TENANTED db answers false through
    // the definer — the audit-evidence arm is not RLS-blinded.
    let never: bool = sqlx::query_scalar("SELECT public.app_session_legacy_lane_ok()")
        .fetch_one(&app)
        .await
        .unwrap();
    assert!(
        !never,
        "tenanted database must report not-never-bootstrapped"
    );

    db.cleanup().await;
}

// ── S4: FORCE RLS on cooperative_memberships ──────────────────────

/// Sealed-coop reads see own rows only; NULL-attributed and foreign
/// rows are invisible; a bare (unsealed) connection sees nothing.
#[tokio::test]
async fn s4_membership_reads_are_sealed_scope_only() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    // Unsealed pool: zero rows.
    let bare: i64 = sqlx::query_scalar("SELECT count(*) FROM cooperative_memberships")
        .fetch_one(&app)
        .await
        .unwrap();
    assert_eq!(bare, 0, "unsealed connection must see nothing");

    // Sealed to coop A: exactly the A memberships (user_a + user_c).
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed tx");
    let own: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cooperative_memberships WHERE cooperative_id = $1",
    )
    .bind(p.coop_a)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(own, 2);
    let all: i64 = sqlx::query_scalar("SELECT count(*) FROM cooperative_memberships")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(all, 2, "foreign coop rows invisible inside sealed tx");
    let nulls: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cooperative_memberships WHERE cooperative_id IS NULL",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(nulls, 0, "NULL-attributed rows have no app lane");
    tx.rollback().await.unwrap();

    db.cleanup().await;
}

/// Independent RLS proof: with a temporary owner-lane GRANT of write
/// verbs, INSERT/UPDATE/DELETE still cannot cross the sealed scope or
/// mint NULL rows. Grants restored within the disposable fixture.
#[tokio::test]
async fn s4_rls_independently_enforced_under_temporary_grants() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    sqlx::query("GRANT INSERT, UPDATE, DELETE ON cooperative_memberships TO kooperatif_app")
        .execute(&db.pool)
        .await
        .unwrap();

    // In-scope INSERT allowed by the app policy (real user row for
    // the FK — created on the owner lane).
    let new_user: Uuid = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, login_verifier) \
         VALUES ($1, $2, 'S4 Temp', 'x', 'x')",
    )
    .bind(new_user)
    .bind(format!("s4-{}", &new_user.to_string()[..8]))
    .execute(&db.pool)
    .await
    .unwrap();
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
        .await
        .expect("sealed tx");
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         VALUES ($1, $2, 'suspended')",
    )
    .bind(p.coop_a)
    .bind(new_user)
    .execute(&mut *tx)
    .await
    .expect("in-scope insert under sealed context");

    // UPDATE/DELETE touch only sealed-scope rows (foreign coop no-op).
    let upd = sqlx::query(
        "UPDATE cooperative_memberships SET status = 'suspended' WHERE cooperative_id = $1",
    )
    .bind(p.coop_b)
    .execute(&mut *tx)
    .await
    .unwrap();
    assert_eq!(upd.rows_affected(), 0, "cross-coop update sees nothing");
    let del = sqlx::query("DELETE FROM cooperative_memberships WHERE cooperative_id = $1")
        .bind(p.coop_b)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(del.rows_affected(), 0, "cross-coop delete sees nothing");
    tx.rollback().await.unwrap();

    // Cross-coop and NULL attribution rejected by WITH CHECK — each in
    // its own sealed tx: a rejected INSERT aborts the transaction.
    for (label, sql, coop) in [
        (
            "cross-coop insert",
            "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
          VALUES ($1, $2, 'active')",
            Some(p.coop_b),
        ),
        (
            "NULL insert",
            "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
          VALUES (NULL, $2, 'active')",
            None,
        ),
    ] {
        let (mut rtx, _) =
            app_db::request_tx(&app, &p.token_a, p.coop_a, OffsetDateTime::now_utc())
                .await
                .expect("sealed tx");
        let res = match coop {
            Some(c) => {
                sqlx::query(sql)
                    .bind(c)
                    .bind(new_user)
                    .execute(&mut *rtx)
                    .await
            }
            None => sqlx::query(sql).bind(new_user).execute(&mut *rtx).await,
        };
        assert!(res.is_err(), "{label} must hit WITH CHECK");
        rtx.rollback().await.unwrap();
    }

    sqlx::query("REVOKE INSERT, UPDATE, DELETE ON cooperative_memberships FROM kooperatif_app")
        .execute(&db.pool)
        .await
        .unwrap();

    db.cleanup().await;
}

/// The legitimate membership-dependent paths survive FORCE RLS:
/// request_tx membership stamping, cooperative switching, scoped
/// authorization reads and the administration-path count all resolve
/// as before.
#[tokio::test]
async fn s4_legitimate_membership_paths_still_work() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    // Sealed tx = proof that app_stamp_cooperative's definer membership
    // check passed under FORCE RLS.
    let (tx, ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("sealed tx — stamp_cooperative resolved membership");
    assert_eq!(ctx.user_id, p.user_a);
    tx.rollback().await.unwrap();

    // Cooperative switch (definer membership check + atomic audit).
    let ok: bool = sqlx::query_scalar("SELECT public.app_session_set_cooperative($1, $2)")
        .bind(&p.token_a)
        .bind(p.coop_a)
        .fetch_one(&app)
        .await
        .unwrap();
    assert!(ok);

    // Session-bound membership reader (S2A) still answers.
    let m_status: Option<String> = sqlx::query_scalar(
        "SELECT membership_status FROM public.app_session_membership($1, $2, $3)",
    )
    .bind(&p.token_a)
    .bind(p.coop_a)
    .bind(now)
    .fetch_one(&app)
    .await
    .unwrap();
    assert_eq!(m_status.as_deref(), Some("active"));

    // A foreign token context cannot stamp coop A: user_b's token is
    // coop-B-bound; a fabricated seal-less stamp attempt fails closed.
    let denied = app_db::request_tx(&app, &[0x52u8; 32], p.coop_a, now).await;
    assert!(denied.is_err(), "non-member token cannot stamp coop A");

    db.cleanup().await;
}

// ────────────────────────────────────────────────────────────────────
// S5 — FORCE RLS on roles + user_role_assignments (migration 0051)
// ────────────────────────────────────────────────────────────────────

/// Unsealed `kooperatif_app` connections see neither table; direct
/// writes fail or no-op. Owner-side fixture rows exist but are
/// unreachable without a seal.
#[tokio::test]
async fn s5_unsealed_access_is_denied_and_invisible() {
    let db = setup().await;
    let _p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    let n_roles: i64 = sqlx::query_scalar("SELECT count(*) FROM roles")
        .fetch_one(&app)
        .await
        .unwrap();
    assert_eq!(n_roles, 0, "unsealed SELECT on roles must see nothing");
    let n_ura: i64 = sqlx::query_scalar("SELECT count(*) FROM user_role_assignments")
        .fetch_one(&app)
        .await
        .unwrap();
    assert_eq!(
        n_ura, 0,
        "unsealed SELECT on user_role_assignments must see nothing"
    );

    let ins = sqlx::query(
        "INSERT INTO roles (name, cooperative_id, status) VALUES ('x', NULL, 'active')",
    )
    .execute(&app)
    .await;
    assert!(ins.is_err(), "unsealed INSERT must be denied");

    // R1: write grants revoked — unsealed UPDATE/DELETE are hard
    // privilege errors, not just invisible rows.
    let upd = sqlx::query("UPDATE roles SET status = 'disabled'")
        .execute(&app)
        .await;
    assert!(upd.is_err(), "unsealed UPDATE must be denied (R1 grant)");
    let del = sqlx::query("DELETE FROM user_role_assignments")
        .execute(&app)
        .await;
    assert!(del.is_err(), "unsealed DELETE must be denied (R1 grant)");

    db.cleanup().await;
}

/// A sealed cooperative context sees only its own cooperative's rows:
/// foreign roles/assignments are invisible to reads, updates and
/// deletes, and cannot be inserted (WITH CHECK). NULL-attributed rows
/// are unreachable from a non-bootstrap scope.
#[tokio::test]
async fn s5_sealed_scope_isolates_roles_and_assignments() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    // Owner lane: a role + assignment in coop B, and a NULL-attributed
    // legacy role — all invisible to coop A's sealed context.
    let role_b = Uuid::new_v4();
    let role_null = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO roles (id, name, cooperative_id, status) \
         VALUES ($1,'b-role',$2,'active'), ($3,'legacy-null',NULL,'active')",
    )
    .bind(role_b)
    .bind(p.coop_b)
    .bind(role_null)
    .execute(&db.pool)
    .await
    .unwrap();

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("sealed tx");

    let visible: Vec<String> = sqlx::query_scalar("SELECT name FROM roles ORDER BY name")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        visible,
        vec!["pilot-admin".to_string()],
        "sealed coop-A context must see only its own role"
    );

    // R1: direct writes are denied by grants even inside the sealed
    // scope — a sealed context alone no longer authorizes RBAC
    // mutations. Each attempt runs in its own sealed tx (a rejected
    // statement aborts the transaction).
    for (label, sql) in [
        (
            "cross-coop role update",
            "UPDATE roles SET status = 'disabled' WHERE id = $1",
        ),
        (
            "cross-coop role insert",
            "INSERT INTO roles (name, cooperative_id, status) \
             VALUES ('evil', $1, 'active')",
        ),
        (
            "cross-coop assignment insert",
            "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
             VALUES ($2, $3, $1)",
        ),
        (
            "NULL-attributed assignment insert",
            "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
             VALUES ($2, $3, NULL)",
        ),
        (
            "in-scope role insert (no direct writes at all)",
            "INSERT INTO roles (name, cooperative_id, status) \
             VALUES ('a-clerk', $4, 'active')",
        ),
        (
            "in-scope self-assignment insert",
            "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
             VALUES ($2, $3, $4)",
        ),
        (
            "in-scope role update",
            "UPDATE roles SET description = 'x' WHERE id = $3",
        ),
        (
            "in-scope assignment delete",
            "DELETE FROM user_role_assignments WHERE cooperative_id = $4",
        ),
    ] {
        let (mut rtx, _) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
            .await
            .expect("fresh sealed tx");
        let res = sqlx::query(sql)
            .bind(p.coop_b)
            .bind(p.user_a)
            .bind(role_b)
            .bind(p.coop_a)
            .execute(&mut *rtx)
            .await;
        assert!(
            res.is_err(),
            "{label} must be denied — R1 revoked direct write grants"
        );
        rtx.rollback().await.unwrap();
    }

    // Reads stay scoped: the sealed context sees only its own
    // assignments.
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("sealed tx");
    let cnt: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_role_assignments WHERE cooperative_id = $1")
            .bind(p.coop_a)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(cnt, 1, "sealed scope sees its own assignments");
    tx.rollback().await.unwrap();

    db.cleanup().await;
}

/// Forged GUCs satisfy neither seal: arbitrary `set_config` calls on an
/// app connection cannot conjure row visibility.
#[tokio::test]
async fn s5_forged_gucs_reveal_nothing() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    let mut tx = app.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.user_id', $1::text, true)")
        .bind(p.user_a)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.session_id', $1::text, true)")
        .bind(Uuid::new_v4())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.cooperative_id', $1::text, true)")
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.ctx_txid', txid_current()::text, true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.ctx_seal', 'deadbeef', true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.user_seal', 'deadbeef', true)")
        .execute(&mut *tx)
        .await
        .unwrap();

    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM roles")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, 0, "forged seals must satisfy neither policy arm");
    let n2: i64 = sqlx::query_scalar("SELECT count(*) FROM user_role_assignments")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n2, 0);
    tx.rollback().await.unwrap();

    db.cleanup().await;
}

/// The bootstrap cooperative keeps the legacy NULL lane: NULL-attributed
/// roles/assignments are visible and manageable ONLY from the
/// marker-verified bootstrap scope — never from a plain cooperative.
#[tokio::test]
async fn s5_bootstrap_scope_inherits_null_rows_only() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    // Owner lane: mark coop A as the bootstrap cooperative and add
    // NULL-attributed legacy rows.
    sqlx::query("UPDATE cooperatives SET is_bootstrap = true WHERE id = $1")
        .bind(p.coop_a)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE installation_state SET bootstrap_cooperative_id = $1, \
         tenant_bootstrapped_at = now()",
    )
    .bind(p.coop_a)
    .execute(&db.pool)
    .await
    .unwrap();
    let legacy_role = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO roles (id, name, cooperative_id, status) \
         VALUES ($1,'legacy-admin',NULL,'active')",
    )
    .bind(legacy_role)
    .execute(&db.pool)
    .await
    .unwrap();

    // Bootstrap scope sees own + NULL rows.
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("sealed bootstrap tx");
    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM roles ORDER BY name")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        names,
        vec![
            "Sistem Yöneticisi".to_string(),
            "legacy-admin".to_string(),
            "pilot-admin".to_string(),
            "platform-lifecycle".to_string()
        ],
        "bootstrap scope must see own + NULL legacy rows"
    );
    tx.rollback().await.unwrap();

    // A non-bootstrap scope can NEVER inherit NULL rows: coop B sees
    // neither coop-A rows nor the NULL legacy row.
    let user_b = Uuid::new_v4();
    let token_b = vec![0x5Bu8; 32];
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, login_verifier) \
         VALUES ($1,$2,'B','x','x')",
    )
    .bind(user_b)
    .bind(format!("b5-{}", &user_b.to_string()[..8]))
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         VALUES ($1,$2,'active')",
    )
    .bind(p.coop_b)
    .bind(user_b)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_sessions \
           (user_id, token_hash, csrf_token, created_at, last_seen_at, expires_at, idle_expires_at, \
            active_cooperative_id) \
         VALUES ($1,$2,'csrf',now(),now(),now() + interval '1 day',now() + interval '1 day',$3)",
    )
    .bind(user_b)
    .bind(&token_b)
    .bind(p.coop_b)
    .execute(&db.pool)
    .await
    .unwrap();
    let (mut txb, _ctx) = app_db::request_tx(&app, &token_b, p.coop_b, now)
        .await
        .expect("sealed coop-B tx");
    let nb: i64 = sqlx::query_scalar("SELECT count(*) FROM roles")
        .fetch_one(&mut *txb)
        .await
        .unwrap();
    assert_eq!(
        nb, 0,
        "non-bootstrap scope must never see NULL or foreign rows"
    );
    // And cannot write a NULL-attributed row either.
    let ins = sqlx::query(
        "INSERT INTO roles (name, cooperative_id, status) VALUES ('x', NULL, 'active')",
    )
    .execute(&mut *txb)
    .await;
    assert!(ins.is_err(), "non-bootstrap scope cannot mint NULL rows");
    txb.rollback().await.unwrap();

    db.cleanup().await;
}

/// The never-bootstrapped lane: with zero cooperatives, a user-sealed
/// transaction (request_user_tx — no cooperative stamped) manages NULL
/// rows. The moment a cooperative exists the lane closes: NULL rows
/// become invisible and unwritable to the user-only seal.
#[tokio::test]
async fn s5_never_bootstrapped_lane_opens_then_closes() {
    let db = setup().await; // fresh DB — zero cooperatives
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    let user = Uuid::new_v4();
    let token = vec![0x5Cu8; 32];
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, login_verifier) \
         VALUES ($1,$2,'Legacy','x','x')",
    )
    .bind(user)
    .bind(format!("lg-{}", &user.to_string()[..8]))
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_sessions \
           (user_id, token_hash, csrf_token, created_at, last_seen_at, expires_at, idle_expires_at) \
         VALUES ($1,$2,'csrf',now(),now(),now() + interval '1 day',now() + interval '1 day')",
    )
    .bind(user)
    .bind(&token)
    .execute(&db.pool)
    .await
    .unwrap();
    let role_null = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO roles (id, name, cooperative_id, status) \
         VALUES ($1,'seed-admin',NULL,'active')",
    )
    .bind(role_null)
    .execute(&db.pool)
    .await
    .unwrap();

    // Pre-tenancy: user-sealed tx still READS NULL rows (the
    // migration-seeded 'Sistem Yöneticisi' is NULL-attributed too).
    // R1: but the user seal alone does NOT authorize writes — direct
    // DML is grant-denied, and the boundary definers additionally
    // require the NULL-attributed administrative permission.
    let (mut tx, uid) = app_db::request_user_tx(&app, &token, now)
        .await
        .expect("user-sealed tx");
    assert_eq!(uid, user);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM roles WHERE name = 'seed-admin'")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, 1, "never-bootstrapped lane must see NULL rows");

    let upd = sqlx::query("UPDATE roles SET description = 'managed' WHERE id = $1")
        .bind(role_null)
        .execute(&mut *tx)
        .await;
    assert!(upd.is_err(), "R1: user seal alone must not write NULL rows");
    tx.rollback().await.unwrap();

    // The user holds no NULL-arm permission yet — even the boundary
    // definer refuses.
    let (mut tx, _uid) = app_db::request_user_tx(&app, &token, now)
        .await
        .expect("user-sealed tx");
    let denied =
        sqlx::query_scalar::<_, Option<Uuid>>("SELECT public.app_role_update($1, NULL, 'managed')")
            .bind(role_null)
            .fetch_one(&mut *tx)
            .await;
    assert!(
        matches!(denied, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "R1: definer must require NULL-arm roles.manage"
    );
    tx.rollback().await.unwrap();

    // Legitimate pre-tenancy administration: owner assigns the seed
    // admin role; the boundary definer then works for the user seal.
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission_id) \
         SELECT $1, id FROM permissions WHERE key = 'roles.manage'",
    )
    .bind(role_null)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
         VALUES ($1, $2, NULL)",
    )
    .bind(user)
    .bind(role_null)
    .execute(&db.pool)
    .await
    .unwrap();
    let (mut tx, _uid) = app_db::request_user_tx(&app, &token, now)
        .await
        .expect("user-sealed tx");
    let updated: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_role_update($1, NULL, 'managed')")
            .bind(role_null)
            .fetch_one(&mut *tx)
            .await
            .expect("authorized pre-tenancy definer call");
    assert_eq!(updated, Some(role_null), "NULL row manageable via definer");
    tx.rollback().await.unwrap();

    // Bootstrap happens: a cooperative now exists — the lane closes.
    sqlx::query(
        "INSERT INTO cooperatives (id, name, status, is_bootstrap, business_enabled) \
         VALUES ($1,'C','active',true,true)",
    )
    .bind(Uuid::new_v4())
    .execute(&db.pool)
    .await
    .unwrap();

    let (mut tx, _uid) = app_db::request_user_tx(&app, &token, now)
        .await
        .expect("user-sealed tx");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM roles")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, 0, "post-tenancy user seal must not reach NULL rows");
    let n2: i64 = sqlx::query_scalar("SELECT count(*) FROM user_role_assignments")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n2, 0);
    let ins = sqlx::query(
        "INSERT INTO roles (name, cooperative_id, status) VALUES ('x', NULL, 'active')",
    )
    .execute(&mut *tx)
    .await;
    assert!(
        ins.is_err(),
        "NULL scope is not a write bypass post-tenancy"
    );
    tx.rollback().await.unwrap();

    db.cleanup().await;
}

/// Independent RLS proof: grant the missing verbs temporarily on the
/// owner lane; sealed-context writes still cannot cross scope or mint
/// NULL attribution. Grants restored inside the disposable fixture.
#[tokio::test]
async fn s5_rls_independently_enforced_under_temporary_grants() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    sqlx::query("GRANT DELETE ON roles TO kooperatif_app")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("GRANT UPDATE ON user_role_assignments TO kooperatif_app")
        .execute(&db.pool)
        .await
        .unwrap();

    let role_b = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO roles (id, name, cooperative_id, status) \
         VALUES ($1,'b-target',$2,'active')",
    )
    .bind(role_b)
    .bind(p.coop_b)
    .execute(&db.pool)
    .await
    .unwrap();

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("sealed tx");

    // Granted-but-foreign verbs still affect zero rows.
    let del = sqlx::query("DELETE FROM roles WHERE id = $1")
        .bind(role_b)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(del.rows_affected(), 0, "cross-coop delete sees nothing");
    let upd =
        sqlx::query("UPDATE user_role_assignments SET user_id = $1 WHERE cooperative_id = $2")
            .bind(p.user_c)
            .bind(p.coop_b)
            .execute(&mut *tx)
            .await
            .unwrap();
    assert_eq!(upd.rows_affected(), 0, "cross-coop update sees nothing");
    tx.rollback().await.unwrap();

    // Cooperative-id reassignment of an in-scope row is denied.
    let (mut rtx, _) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("fresh sealed tx");
    let reassign = sqlx::query("UPDATE roles SET cooperative_id = $1 WHERE cooperative_id = $2")
        .bind(p.coop_b)
        .bind(p.coop_a)
        .execute(&mut *rtx)
        .await;
    assert!(
        reassign.is_err(),
        "cooperative reassignment must hit WITH CHECK"
    );
    rtx.rollback().await.unwrap();

    sqlx::query("REVOKE DELETE ON roles FROM kooperatif_app")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("REVOKE UPDATE ON user_role_assignments FROM kooperatif_app")
        .execute(&db.pool)
        .await
        .unwrap();

    db.cleanup().await;
}

// ────────────────────────────────────────────────────────────────────
// R1 — RBAC database boundary (migration 0052, closure-audit F1+F2)
// Invariant: a valid session / sealed cooperative context alone does
// NOT authorize RBAC mutations. Every write goes through a definer
// that re-verifies the administrative permission inside the database.
// ────────────────────────────────────────────────────────────────────

/// Owner-lane principal factory: user + session + active membership +
/// a fresh active role carrying `permissions` + assignment in `coop`.
/// M1-P6 B4 test helper: owner-lane stand-in for the audited switch
/// (`POST /api/auth/cooperative`) — moves a session's authoritative
/// scope, the only cooperative 0071 lets it stamp.
async fn pin_session(db: &TestDb, token: &[u8], coop: Uuid) {
    sqlx::query("UPDATE user_sessions SET active_cooperative_id = $2 WHERE token_hash = $1")
        .bind(token)
        .bind(coop)
        .execute(&db.pool)
        .await
        .unwrap();
}

async fn r1_principal(db: &TestDb, tag: &str, coop: Uuid, permissions: &[&str]) -> (Uuid, Vec<u8>) {
    let user = Uuid::new_v4();
    let token: Vec<u8> = {
        let mut v = tag.as_bytes().to_vec();
        v.resize(32, 0x42);
        v
    };
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, login_verifier) \
         VALUES ($1,$2,'R1','x','x')",
    )
    .bind(user)
    .bind(format!("r1-{tag}-{}", &user.to_string()[..6]))
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         VALUES ($1,$2,'active')",
    )
    .bind(coop)
    .bind(user)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_sessions \
           (user_id, token_hash, csrf_token, created_at, last_seen_at, expires_at, idle_expires_at, \
            active_cooperative_id) \
         VALUES ($1,$2,'csrf',now(),now(),now() + interval '1 day',now() + interval '1 day',$3)",
    )
    .bind(user)
    .bind(&token)
    .bind(coop)
    .execute(&db.pool)
    .await
    .unwrap();
    if !permissions.is_empty() {
        let role = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO roles (id, name, cooperative_id, status) \
             VALUES ($1, $2, $3, 'active')",
        )
        .bind(role)
        .bind(format!("r1-role-{tag}"))
        .bind(coop)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO role_permissions (role_id, permission_id) \
             SELECT $1, id FROM permissions WHERE key = ANY($2)",
        )
        .bind(role)
        .bind(permissions)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
             VALUES ($1, $2, $3)",
        )
        .bind(user)
        .bind(role)
        .bind(coop)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    (user, token)
}

/// F1 reproduction: role_permissions writes are unreachable — no
/// session, seal or permission exists at the SQL layer. (Before 0052
/// this INSERT/DELETE succeeded for ANY app connection.)
#[tokio::test]
async fn r1_role_permissions_direct_write_denied() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    let perm_id: Uuid = sqlx::query_scalar("SELECT id FROM permissions LIMIT 1")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let role_any: Uuid = sqlx::query_scalar("SELECT id FROM roles LIMIT 1")
        .fetch_one(&db.pool)
        .await
        .unwrap();

    // Unsealed: every write verb is a hard privilege error.
    for (label, sql) in [
        (
            "unsealed rp insert",
            "INSERT INTO role_permissions (role_id, permission_id) VALUES ($1, $2)",
        ),
        (
            "unsealed rp delete",
            "DELETE FROM role_permissions WHERE role_id = $1",
        ),
        (
            "unsealed rp update",
            "UPDATE role_permissions SET permission_id = $2 WHERE role_id = $1",
        ),
    ] {
        let res = sqlx::query(sql)
            .bind(role_any)
            .bind(perm_id)
            .execute(&app)
            .await;
        assert!(
            matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{label} must be 42501"
        );
    }

    // Sealed member context (no admin permission): still denied.
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("sealed tx");
    let res = sqlx::query("INSERT INTO role_permissions (role_id, permission_id) VALUES ($1, $2)")
        .bind(role_any)
        .bind(perm_id)
        .execute(&mut *tx)
        .await;
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "sealed rp insert must be 42501"
    );
    tx.rollback().await.unwrap();

    db.cleanup().await;
}

/// F2 reproduction: a sealed member (no administrative permission)
/// cannot self-assign a role, create roles, change statuses, touch
/// role_permissions or rewrite assignments — not by direct SQL and
/// not through the boundary definers.
#[tokio::test]
async fn r1_sealed_member_has_no_rbac_mutation_path() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    // user_c: plain active member of coop A, no roles at all.
    let token_c = vec![0x53u8; 32];
    let admin_role: Uuid = sqlx::query_scalar("SELECT id FROM roles WHERE name = 'pilot-admin'")
        .fetch_one(&db.pool)
        .await
        .unwrap();

    // Direct self-assignment — the F2 exploit — is a privilege error.
    let (mut tx, _ctx) = app_db::request_tx(&app, &token_c, p.coop_a, now)
        .await
        .expect("sealed member tx");
    let res = sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
         VALUES ($1, $2, $3)",
    )
    .bind(p.user_c)
    .bind(admin_role)
    .bind(p.coop_a)
    .execute(&mut *tx)
    .await;
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "F2: sealed member self-assignment must be denied at the grant layer"
    );
    tx.rollback().await.unwrap();

    // Every boundary definer re-verifies the permission inside the
    // database and refuses a member without the right.
    let denied_role_create = {
        let (mut rtx, _) = app_db::request_tx(&app, &token_c, p.coop_a, now)
            .await
            .expect("fresh member tx");
        let res = sqlx::query_scalar::<_, Option<Uuid>>("SELECT public.app_role_create('x','')")
            .fetch_optional(&mut *rtx)
            .await;
        rtx.rollback().await.unwrap();
        res
    };
    assert!(
        matches!(denied_role_create,
            Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "app_role_create must raise insufficient_privilege for a member"
    );

    let denied_status = {
        let (mut rtx, _) = app_db::request_tx(&app, &token_c, p.coop_a, now)
            .await
            .expect("fresh member tx");
        let res = sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT public.app_role_set_status($1, 'disabled')",
        )
        .bind(admin_role)
        .fetch_optional(&mut *rtx)
        .await;
        rtx.rollback().await.unwrap();
        res
    };
    assert!(
        matches!(denied_status,
            Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "app_role_set_status must raise insufficient_privilege for a member"
    );

    let denied_perms = {
        let (mut rtx, _) = app_db::request_tx(&app, &token_c, p.coop_a, now)
            .await
            .expect("fresh member tx");
        let res = sqlx::query("SELECT public.app_role_permissions_set($1, $2)")
            .bind(admin_role)
            .bind(vec!["roles.manage".to_string()])
            .execute(&mut *rtx)
            .await;
        rtx.rollback().await.unwrap();
        res
    };
    assert!(
        matches!(denied_perms,
            Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "app_role_permissions_set must raise insufficient_privilege"
    );

    let denied_assign = {
        let (mut rtx, _) = app_db::request_tx(&app, &token_c, p.coop_a, now)
            .await
            .expect("fresh member tx");
        let res = sqlx::query("SELECT public.app_user_roles_set($1, $2)")
            .bind(p.user_c)
            .bind(vec![admin_role])
            .execute(&mut *rtx)
            .await;
        rtx.rollback().await.unwrap();
        res
    };
    assert!(
        matches!(denied_assign,
            Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "app_user_roles_set must raise insufficient_privilege"
    );

    db.cleanup().await;
}

/// Legitimate administration: a verified roles.manage+users.manage
/// holder performs the full CRUD surface through the definers.
#[tokio::test]
async fn r1_authorized_admin_mutations_work_through_definers() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();
    let (_admin, token_adm) = r1_principal(
        &db,
        "adm",
        p.coop_a,
        &["roles.manage", "users.manage", "periods.read"],
    )
    .await;

    let (mut tx, _ctx) = app_db::request_tx(&app, &token_adm, p.coop_a, now)
        .await
        .expect("sealed admin tx");

    let new_role: Uuid =
        sqlx::query_scalar("SELECT public.app_role_create('r1-clerk', 'clerk role')")
            .fetch_one(&mut *tx)
            .await
            .expect("admin role create");
    // Attribution is forced to the sealed cooperative — never caller input.
    let owner: Option<Uuid> = sqlx::query_scalar("SELECT cooperative_id FROM roles WHERE id = $1")
        .bind(new_role)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        owner,
        Some(p.coop_a),
        "role must be attributed to sealed scope"
    );

    let updated: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_role_update($1, 'r1-clerk2', NULL)")
            .bind(new_role)
            .fetch_one(&mut *tx)
            .await
            .expect("admin role update");
    assert_eq!(updated, Some(new_role));

    sqlx::query("SELECT public.app_role_permissions_set($1, $2)")
        .bind(new_role)
        .bind(vec!["periods.read".to_string()])
        .execute(&mut *tx)
        .await
        .expect("admin permission set");

    sqlx::query("SELECT public.app_user_roles_set($1, $2)")
        .bind(p.user_c)
        .bind(vec![new_role])
        .execute(&mut *tx)
        .await
        .expect("admin assignment set");

    // Rollback: nothing persists (statement-level atomicity).
    tx.rollback().await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM roles WHERE id = $1")
        .bind(new_role)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "rolled-back definer writes leave nothing");

    db.cleanup().await;
}

/// Cross-cooperative: an admin sealed into coop A cannot touch coop
/// B's roles, permission sets or members — through direct SQL or the
/// definers.
#[tokio::test]
async fn r1_cross_cooperative_rbac_mutation_denied() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();
    let (_admin, token_adm) =
        r1_principal(&db, "adm", p.coop_a, &["roles.manage", "users.manage"]).await;
    let (user_b, _tok_b) = r1_principal(&db, "memb", p.coop_b, &[]).await;

    let role_b = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO roles (id, name, cooperative_id, status) \
         VALUES ($1,'b-role',$2,'active')",
    )
    .bind(role_b)
    .bind(p.coop_b)
    .execute(&db.pool)
    .await
    .unwrap();

    // Role update on a foreign role → NULL (outside verified scope).
    let (mut tx, _ctx) = app_db::request_tx(&app, &token_adm, p.coop_a, now)
        .await
        .expect("sealed admin tx");
    let updated: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_role_update($1, 'hijack', NULL)")
            .bind(role_b)
            .fetch_one(&mut *tx)
            .await
            .expect("definer call");
    assert_eq!(updated, None, "foreign role must be outside scope");
    tx.rollback().await.unwrap();

    // Permission graft onto a foreign role → insufficient_privilege.
    let (mut tx, _ctx) = app_db::request_tx(&app, &token_adm, p.coop_a, now)
        .await
        .expect("sealed admin tx");
    let res = sqlx::query("SELECT public.app_role_permissions_set($1, $2)")
        .bind(role_b)
        .bind(vec!["roles.manage".to_string()])
        .execute(&mut *tx)
        .await;
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "foreign role_permissions graft denied"
    );
    tx.rollback().await.unwrap();

    // Status change on a foreign role → NULL.
    let (mut tx, _ctx) = app_db::request_tx(&app, &token_adm, p.coop_a, now)
        .await
        .expect("sealed admin tx");
    let updated: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_role_set_status($1, 'disabled')")
            .bind(role_b)
            .fetch_one(&mut *tx)
            .await
            .expect("definer call");
    assert_eq!(updated, None, "foreign role status change sees nothing");
    tx.rollback().await.unwrap();

    // Assignment change on a foreign member → insufficient_privilege.
    let (mut tx, _ctx) = app_db::request_tx(&app, &token_adm, p.coop_a, now)
        .await
        .expect("sealed admin tx");
    let res = sqlx::query("SELECT public.app_user_roles_set($1, $2)")
        .bind(user_b)
        .bind(Vec::<Uuid>::new())
        .execute(&mut *tx)
        .await;
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "foreign member assignment denied"
    );
    tx.rollback().await.unwrap();

    db.cleanup().await;
}

/// Platform/NULL-attributed roles cannot be mutated from a
/// non-bootstrap cooperative — even by its administrator.
#[tokio::test]
async fn r1_platform_role_immutable_outside_bootstrap_scope() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();
    let (_admin, token_adm) = r1_principal(&db, "adm", p.coop_a, &["roles.manage"]).await;

    let null_role: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE cooperative_id IS NULL LIMIT 1")
            .fetch_one(&db.pool)
            .await
            .expect("seed NULL role exists");

    let (mut tx, _ctx) = app_db::request_tx(&app, &token_adm, p.coop_a, now)
        .await
        .expect("sealed admin tx");
    let updated: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_role_update($1, 'hijack', NULL)")
            .bind(null_role)
            .fetch_one(&mut *tx)
            .await
            .expect("definer call");
    assert_eq!(updated, None, "NULL role out of scope for non-bootstrap");
    let res = sqlx::query("SELECT public.app_role_permissions_set($1, $2)")
        .bind(null_role)
        .bind(vec!["roles.manage".to_string()])
        .execute(&mut *tx)
        .await;
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "NULL role permission graft denied"
    );
    tx.rollback().await.unwrap();

    db.cleanup().await;
}

/// Last-administrator protection lives inside the database boundary:
/// stripping the only admin role's permission or disabling it raises
/// 'lockout_prevented' and leaves the tables untouched.
#[tokio::test]
async fn r1_lockout_protection_inside_boundary() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    // `admin` holds the ONLY roles.manage path in coop A (pilot-admin
    // carries users.manage only).
    let (admin, token_adm) = r1_principal(&db, "adm", p.coop_a, &["roles.manage"]).await;
    let adm_role: Uuid =
        sqlx::query_scalar("SELECT role_id FROM user_role_assignments WHERE user_id = $1")
            .bind(admin)
            .fetch_one(&db.pool)
            .await
            .unwrap();

    // 1. Strip roles.manage from the last admin role → lockout,
    //    role_permissions untouched.
    let (mut tx, _ctx) = app_db::request_tx(&app, &token_adm, p.coop_a, now)
        .await
        .expect("sealed admin tx");
    let res = sqlx::query("SELECT public.app_role_permissions_set($1, $2)")
        .bind(adm_role)
        // An empty set still carries no unowned keys, so the delegation
        // guard passes and the strip reaches the lockout check.
        .bind(Vec::<String>::new())
        .execute(&mut *tx)
        .await;
    let locked = matches!(res, Err(sqlx::Error::Database(ref e))
        if e.code().as_deref() == Some("23514") && e.message().contains("lockout_prevented"));
    assert!(locked, "stripping the last admin path must lockout");
    tx.rollback().await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM role_permissions WHERE role_id = $1")
        .bind(adm_role)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(n, 1, "lockout must leave role_permissions untouched");

    // 2. Disable the last admin role → lockout, status untouched.
    let (mut tx, _ctx) = app_db::request_tx(&app, &token_adm, p.coop_a, now)
        .await
        .expect("sealed admin tx");
    let res = sqlx::query("SELECT public.app_role_set_status($1, 'disabled')")
        .bind(adm_role)
        .execute(&mut *tx)
        .await;
    let locked = matches!(res, Err(sqlx::Error::Database(ref e))
        if e.code().as_deref() == Some("23514") && e.message().contains("lockout_prevented"));
    assert!(locked, "disabling the last admin role must lockout");
    tx.rollback().await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM roles WHERE id = $1")
        .bind(adm_role)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(status, "active", "lockout must leave the role untouched");

    // 3. Removing the last admin's assignments → lockout (requires
    //    users.manage — a second principal carries both rights, but
    //    `admin` remains the sole roles.manage path).
    let (_admin2, token_adm2) =
        r1_principal(&db, "adm2", p.coop_a, &["roles.manage", "users.manage"]).await;
    let (mut tx, _ctx) = app_db::request_tx(&app, &token_adm2, p.coop_a, now)
        .await
        .expect("sealed admin tx");
    let res = sqlx::query("SELECT public.app_user_roles_set($1, $2)")
        .bind(admin)
        .bind(Vec::<Uuid>::new())
        .execute(&mut *tx)
        .await;
    assert!(
        res.is_ok(),
        "removing one of two roles.manage holders is allowed"
    );
    // Now `admin` lost its path; adm2 is the sole remaining one —
    // removing adm2's assignments must lockout.
    let res = sqlx::query("SELECT public.app_user_roles_set($1, $2)")
        .bind(_admin2)
        .bind(Vec::<Uuid>::new())
        .execute(&mut *tx)
        .await;
    let locked = matches!(res, Err(sqlx::Error::Database(ref e))
        if e.code().as_deref() == Some("23514") && e.message().contains("lockout_prevented"));
    assert!(locked, "removing the last admin's roles must lockout");
    tx.rollback().await.unwrap();

    db.cleanup().await;
}

/// Forged GUCs cannot invoke the boundary definers: hand-set
/// app.user_id/app.cooperative_id/seals on a raw connection all fail
/// the seal verification inside the definer. Unsealed invocation too.
#[tokio::test]
async fn r1_forged_guc_cannot_invoke_mutators() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    let mut tx = app.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.user_id', $1::text, true)")
        .bind(p.user_a)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.cooperative_id', $1::text, true)")
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.ctx_txid', txid_current()::text, true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.ctx_seal', 'deadbeef', true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.user_seal', 'deadbeef', true)")
        .execute(&mut *tx)
        .await
        .unwrap();

    let res = sqlx::query_scalar::<_, Option<Uuid>>("SELECT public.app_role_create('x','')")
        .fetch_optional(&mut *tx)
        .await;
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "forged seals must not invoke the boundary"
    );
    tx.rollback().await.unwrap();

    let res = sqlx::query_scalar::<_, Option<Uuid>>("SELECT public.app_role_create('x','')")
        .fetch_optional(&app)
        .await;
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "unsealed invocation must be denied"
    );

    db.cleanup().await;
}

// ────────────────────────────────────────────────────────────────────
// R2 — users table privilege boundary (migration 0053, closure-audit
// F3; M1 — migration 0060 — separates identity from membership).
// Invariants:
//   * direct users writes are denied to kooperatif_app in EVERY lane
//     (sealed or not, admin or not — the grant is gone);
//   * global users.status mutates only through the platform lane
//     (app_platform_user_set_status, NULL-scope users.manage);
//     cooperative users.manage can only drive
//     app_membership_set_status on its own cooperative's rows;
//   * sealed SELECT sees members of the verified cooperative only —
//     users have no cooperative_id, scope derives through
//     cooperative_memberships;
//   * credential columns stay column-invisible.
// ────────────────────────────────────────────────────────────────────

/// F3 reproduction part 1: every direct write verb on `users` is a
/// hard privilege error — unsealed AND inside a sealed transaction
/// (the audit showed `UPDATE users SET status='disabled'` succeeded
/// on ANY user before 0053).
#[tokio::test]
async fn r2_users_direct_write_denied() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    for (label, sql) in [
        (
            "unsealed status update",
            "UPDATE users SET status = 'disabled', updated_at = now() WHERE id = $1",
        ),
        (
            "unsealed display update",
            "UPDATE users SET display_name = 'x' WHERE id = $1",
        ),
        (
            "unsealed insert",
            "INSERT INTO users (username) VALUES ('x')",
        ),
        ("unsealed delete", "DELETE FROM users WHERE id = $1"),
    ] {
        let res = sqlx::query(sql).bind(p.user_c).execute(&app).await;
        assert!(
            matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{label} must be 42501"
        );
    }

    // Sealed member context: still denied at the grant layer.
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("sealed tx");
    let res = sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(p.user_c)
        .execute(&mut *tx)
        .await;
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "sealed direct UPDATE must be 42501"
    );
    tx.rollback().await.unwrap();

    // Even the users.manage admin lane has no direct write grant —
    // the definer is the only mutation path.
    let res = sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(p.user_c)
        .execute(&app)
        .await;
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "admin lane direct UPDATE must be 42501"
    );

    db.cleanup().await;
}

/// F3 reproduction part 2: sealed SELECT sees only members of the
/// verified cooperative — cross-cooperative enumeration is closed at
/// the row layer (previously every app connection enumerated all
/// users unconditionally).
#[tokio::test]
async fn r2_users_enumeration_is_scope_bound() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    // Unsealed: the policy arms need a verified seal — zero rows.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&app)
        .await
        .unwrap();
    assert_eq!(n, 0, "unsealed users read must be empty");

    // Sealed coop A: exactly coop A's members (user_a, user_c) —
    // the coop-B member and membership-less users are invisible.
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("sealed coop A tx");
    let mut ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM users ORDER BY id")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    ids.sort();
    let mut expected = vec![p.user_a, p.user_c];
    expected.sort();
    assert_eq!(ids, expected, "coop A must see its members only");
    tx.rollback().await.unwrap();

    // Sealed coop B (member token 0x52…52): sees only its own member.
    let (mut tx, _ctx) = app_db::request_tx(&app, &[0x52u8; 32], p.coop_b, now)
        .await
        .expect("sealed coop B tx");
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM users")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(ids.len(), 1, "coop B must see exactly its own member");
    assert!(
        !ids.contains(&p.user_a) && !ids.contains(&p.user_c),
        "coop A members must not leak into coop B"
    );
    tx.rollback().await.unwrap();

    // A suspended member stays VISIBLE (administrable) — visibility is
    // membership-row based, not status based.
    sqlx::query(
        "UPDATE cooperative_memberships SET status = 'suspended' \
         WHERE cooperative_id = $1 AND user_id = $2",
    )
    .bind(p.coop_a)
    .bind(p.user_c)
    .execute(&db.pool)
    .await
    .unwrap();
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("sealed coop A tx");
    let still_visible: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE id = $1")
        .bind(p.user_c)
        .fetch_optional(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        still_visible,
        Some(p.user_c),
        "suspended members must remain administrable"
    );
    tx.rollback().await.unwrap();

    // Credential columns remain column-invisible even for members.
    for col in [
        "password_hash",
        "login_verifier",
        "last_login_at",
        "created_at",
    ] {
        let res = sqlx::query(&format!("SELECT {col} FROM users LIMIT 1"))
            .execute(&app)
            .await;
        assert!(
            matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "users.{col} must stay outside the runtime surface"
        );
    }

    db.cleanup().await;
}

/// M1 boundary definers: member without users.manage → 42501; a
/// verified admin suspends/reactivates a coop MEMBERSHIP (never
/// users.status); a foreign-coop-only user is out of scope (NULL, not
/// an error); idempotent no-ops return NULL; and the same verified
/// admin CANNOT reach global users.status — that lane is platform-only.
#[tokio::test]
async fn m1_membership_set_status_definer_paths() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    // member (no users.manage): denied inside the database.
    let (mut tx, _ctx) = app_db::request_tx(&app, &[0x53u8; 32], p.coop_a, now)
        .await
        .expect("sealed member tx");
    let res = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT public.app_membership_set_status($1, 'suspended')",
    )
    .bind(p.user_c)
    .fetch_optional(&mut *tx)
    .await;
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "member must not pass the definer's users.manage check"
    );
    tx.rollback().await.unwrap();

    // user_a holds users.manage in coop A: suspend + reactivate a member.
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("sealed admin tx");
    let updated: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_membership_set_status($1, 'suspended')")
            .bind(p.user_c)
            .fetch_one(&mut *tx)
            .await
            .expect("authorized suspend");
    assert_eq!(updated, Some(p.user_c));
    // Idempotent: already in the target state → NULL.
    let again: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_membership_set_status($1, 'suspended')")
            .bind(p.user_c)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(again, None, "no-op membership change must be idempotent");
    let reactivated: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_membership_set_status($1, 'active')")
            .bind(p.user_c)
            .fetch_one(&mut *tx)
            .await
            .expect("authorized reactivation");
    assert_eq!(reactivated, Some(p.user_c));
    // users.status was never touched by the membership lifecycle.
    let global_status: String = sqlx::query_scalar("SELECT status FROM users WHERE id = $1")
        .bind(p.user_c)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        global_status, "active",
        "membership ops never touch users.status"
    );

    // M1: the global identity write is closed to cooperative-scoped
    // callers — verified users.manage is NOT platform authority.
    let res =
        sqlx::query_scalar::<_, Option<Uuid>>("SELECT public.app_user_set_status($1, 'disabled')")
            .bind(p.user_c)
            .fetch_optional(&mut *tx)
            .await;
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "cooperative users.manage must not reach global users.status"
    );
    tx.rollback().await.unwrap();

    // Nor may a cooperative-scoped admin invoke the platform lane:
    // app_platform_user_set_status rejects a cooperative seal (fresh
    // tx — the denial above aborted the previous one).
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("sealed admin tx");
    let res = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT public.app_platform_user_set_status($1, 'disabled')",
    )
    .bind(p.user_c)
    .fetch_optional(&mut *tx)
    .await;
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "cooperative seal must not satisfy the platform lane — got {res:?}"
    );
    tx.rollback().await.unwrap();

    // Cross-cooperative target: user_b is a member of coop B only —
    // invisible to a coop-A administrator (NULL, not an error).
    let user_b: Uuid =
        sqlx::query_scalar("SELECT user_id FROM cooperative_memberships WHERE cooperative_id = $1")
            .bind(p.coop_b)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("sealed admin tx");
    let out_of_scope: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_membership_set_status($1, 'suspended')")
            .bind(user_b)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(
        out_of_scope, None,
        "a member of another cooperative must be outside the verified scope"
    );
    tx.rollback().await.unwrap();
    let foreign: String = sqlx::query_scalar(
        "SELECT status FROM cooperative_memberships WHERE cooperative_id = $1 AND user_id = $2",
    )
    .bind(p.coop_b)
    .bind(user_b)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(foreign, "active", "cross-coop membership must be untouched");

    db.cleanup().await;
}

/// M1 core invariant: a membership change in cooperative A must not
/// affect cooperative B. A shared user's membership is suspended in B
/// by B's own administrator; the A membership and the global identity
/// stay untouched — and the suspended scope stops resolving for the
/// user's existing session immediately.
#[tokio::test]
async fn m1_membership_suspend_is_cooperative_scoped() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    // user_c additionally joins coop B — now shared.
    sqlx::query(
        "INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
         VALUES ($1, $2, 'active')",
    )
    .bind(p.coop_b)
    .bind(p.user_c)
    .execute(&db.pool)
    .await
    .unwrap();

    // Coop-B administrator: users.manage scoped to B only.
    let (_b_admin, token_b_admin) = r1_principal(&db, "badm", p.coop_b, &["users.manage"]).await;

    // Baseline: the shared user resolves BOTH cooperatives — one at a
    // time, through the session scope (B4: pin = audited switch).
    pin_session(&db, &[0x53u8; 32], p.coop_a).await;
    app_db::request_tx(&app, &[0x53u8; 32], p.coop_a, now)
        .await
        .expect("user_c resolves coop A")
        .0
        .rollback()
        .await
        .unwrap();
    pin_session(&db, &[0x53u8; 32], p.coop_b).await;
    app_db::request_tx(&app, &[0x53u8; 32], p.coop_b, now)
        .await
        .expect("user_c resolves coop B")
        .0
        .rollback()
        .await
        .unwrap();

    // B's admin suspends ONLY the B membership.
    let (mut tx, _ctx) = app_db::request_tx(&app, &token_b_admin, p.coop_b, now)
        .await
        .expect("sealed coop B admin tx");
    let updated: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_membership_set_status($1, 'suspended')")
            .bind(p.user_c)
            .fetch_one(&mut *tx)
            .await
            .expect("coop-B admin may suspend a member of B");
    assert_eq!(updated, Some(p.user_c));
    tx.commit().await.unwrap();

    // B membership suspended; A membership + global identity untouched.
    let status_b: String = sqlx::query_scalar(
        "SELECT status FROM cooperative_memberships WHERE cooperative_id = $1 AND user_id = $2",
    )
    .bind(p.coop_b)
    .bind(p.user_c)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let status_a: String = sqlx::query_scalar(
        "SELECT status FROM cooperative_memberships WHERE cooperative_id = $1 AND user_id = $2",
    )
    .bind(p.coop_a)
    .bind(p.user_c)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let user_status: String = sqlx::query_scalar("SELECT status FROM users WHERE id = $1")
        .bind(p.user_c)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(status_b, "suspended");
    assert_eq!(status_a, "active", "cooperative A must be unaffected");
    assert_eq!(user_status, "active", "global identity must be unaffected");

    // The EXISTING session loses coop B immediately (per-request
    // membership re-resolution) while coop A still works.
    pin_session(&db, &[0x53u8; 32], p.coop_b).await;
    let denied_b = app_db::request_tx(&app, &[0x53u8; 32], p.coop_b, now).await;
    assert!(
        denied_b.is_err(),
        "suspended membership must stop resolving"
    );
    pin_session(&db, &[0x53u8; 32], p.coop_a).await;
    app_db::request_tx(&app, &[0x53u8; 32], p.coop_a, now)
        .await
        .expect("coop A must remain reachable for the same session")
        .0
        .rollback()
        .await
        .unwrap();

    // Reactivation in B is independent and restores B only.
    let (mut tx, _ctx) = app_db::request_tx(&app, &token_b_admin, p.coop_b, now)
        .await
        .expect("sealed coop B admin tx");
    let restored: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_membership_set_status($1, 'active')")
            .bind(p.user_c)
            .fetch_one(&mut *tx)
            .await
            .expect("reactivation");
    assert_eq!(restored, Some(p.user_c));
    tx.commit().await.unwrap();
    pin_session(&db, &[0x53u8; 32], p.coop_b).await;
    app_db::request_tx(&app, &[0x53u8; 32], p.coop_b, now)
        .await
        .expect("coop B resolves again after reactivation")
        .0
        .rollback()
        .await
        .unwrap();

    db.cleanup().await;
}

/// Last-administrator protection for the MEMBERSHIP lane: suspending
/// the membership that carries the scope's sole roles.manage path
/// raises 'lockout_prevented' (23514) and rolls back atomically.
#[tokio::test]
async fn m1_membership_lockout_inside_boundary() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    // `admin` is the ONLY roles.manage path in coop A.
    let (admin, _t) = r1_principal(&db, "r2adm", p.coop_a, &["roles.manage"]).await;

    // user_a (users.manage) tries to suspend that last admin path.
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("sealed admin tx");
    let res = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT public.app_membership_set_status($1, 'suspended')",
    )
    .bind(admin)
    .fetch_optional(&mut *tx)
    .await;
    let locked = matches!(res, Err(sqlx::Error::Database(ref e))
        if e.code().as_deref() == Some("23514") && e.message().contains("lockout_prevented"));
    assert!(locked, "suspending the last admin path must lockout");
    tx.rollback().await.unwrap();

    let status: String = sqlx::query_scalar(
        "SELECT status FROM cooperative_memberships WHERE user_id = $1 AND cooperative_id = $2",
    )
    .bind(admin)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        status, "active",
        "lockout must leave the membership untouched"
    );

    // A second roles.manage holder re-opens the lane: with two paths
    // the same suspension succeeds.
    let (_admin2, _t2) = r1_principal(&db, "r2adm2", p.coop_a, &["roles.manage"]).await;
    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("sealed admin tx");
    let updated: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_membership_set_status($1, 'suspended')")
            .bind(admin)
            .fetch_one(&mut *tx)
            .await
            .expect("suspend must pass once a second admin path exists");
    assert_eq!(updated, Some(admin));
    tx.rollback().await.unwrap();

    db.cleanup().await;
}

/// Ending a membership preserves the historical row (never deleted)
/// and stamps ended_at; the ended membership stops resolving but stays
/// attributable for audit.
#[tokio::test]
async fn m1_membership_end_preserves_history() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    let (mut tx, _ctx) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("sealed admin tx");
    let updated: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_membership_set_status($1, 'ended')")
            .bind(p.user_c)
            .fetch_one(&mut *tx)
            .await
            .expect("authorized end");
    assert_eq!(updated, Some(p.user_c));
    tx.commit().await.unwrap();

    let (status, ended_at): (String, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT status, ended_at FROM cooperative_memberships \
         WHERE user_id = $1 AND cooperative_id = $2",
    )
    .bind(p.user_c)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(status, "ended");
    assert!(ended_at.is_some(), "ended must stamp ended_at");

    // Ended membership no longer resolves a sealed context.
    let denied = app_db::request_tx(&app, &[0x53u8; 32], p.coop_a, now).await;
    assert!(denied.is_err(), "ended membership must stop resolving");

    // Audit: the transition trigger minted cooperative_membership_ended
    // attributed to the acting cooperative.
    let event: Option<String> = sqlx::query_scalar(
        "SELECT event_type FROM security_events \
         WHERE event_type = 'cooperative_membership_ended' \
           AND cooperative_id = $1 AND metadata->>'target_user_id' = $2",
    )
    .bind(p.coop_a)
    .bind(p.user_c.to_string())
    .fetch_optional(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        event.as_deref(),
        Some("cooperative_membership_ended"),
        "membership end must be audited with cooperative attribution"
    );

    db.cleanup().await;
}

/// Concurrent membership mutations: two simultaneous suspends of the
/// same membership row serialize on the row lock — exactly one wins,
/// the loser observes an idempotent NULL, and the audit mints once.
#[tokio::test]
async fn m1_concurrent_membership_suspend_serializes() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    let (tx1, _ctx1) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("admin tx 1");
    let (tx2, _ctx2) = app_db::request_tx(&app, &p.token_a, p.coop_a, now)
        .await
        .expect("admin tx 2");

    // Commit inside the future — the lock holder must release before
    // the blocked sibling can proceed.
    let q = |t: sqlx::Transaction<'static, sqlx::Postgres>| async move {
        let mut t = t;
        let res = sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT public.app_membership_set_status($1, 'suspended')",
        )
        .bind(p.user_c)
        .fetch_one(&mut *t)
        .await;
        if res.is_ok() {
            t.commit().await.unwrap();
        } else {
            t.rollback().await.unwrap();
        }
        res
    };
    let (r1, r2) = tokio::join!(q(tx1), q(tx2));

    let wins = [&r1, &r2]
        .iter()
        .filter(|r| matches!(r, Ok(Some(id)) if *id == p.user_c))
        .count();
    let idempotent = [&r1, &r2].iter().filter(|r| matches!(r, Ok(None))).count();
    assert_eq!(
        wins + idempotent,
        2,
        "no concurrent call may error: {r1:?} {r2:?}"
    );
    assert_eq!(
        wins, 1,
        "exactly one concurrent suspend wins: {r1:?} {r2:?}"
    );

    let status: String = sqlx::query_scalar(
        "SELECT status FROM cooperative_memberships WHERE user_id = $1 AND cooperative_id = $2",
    )
    .bind(p.user_c)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(status, "suspended");
    let minted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events \
         WHERE event_type = 'cooperative_membership_suspended' \
           AND metadata->>'target_user_id' = $1",
    )
    .bind(p.user_c.to_string())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(minted, 1, "the transition must mint exactly once");

    db.cleanup().await;
}

/// Platform lane: only NULL-scope (platform) users.manage may mutate
/// global users.status; global disable blocks session resolution, and
/// global reactivation never reopens a suspended membership.
#[tokio::test]
async fn m1_global_status_is_platform_lane() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    // user_a is a COOPERATIVE admin (users.manage in coop A) but holds
    // no NULL-scope grant: the platform lane rejects them even on a
    // user-sealed (no-cooperative) context.
    let (mut tx, _uid) = app_db::request_user_tx(&app, &p.token_a, now)
        .await
        .expect("user-sealed tx");
    let res = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT public.app_platform_user_set_status($1, 'disabled')",
    )
    .bind(p.user_c)
    .fetch_optional(&mut *tx)
    .await;
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "cooperative users.manage must not satisfy platform authority"
    );
    tx.rollback().await.unwrap();

    // A platform administrator: NULL-scope users.manage holder.
    let platform_admin = Uuid::new_v4();
    let platform_token: Vec<u8> = vec![0x5cu8; 32];
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, login_verifier) \
         VALUES ($1,$2,'Platform Admin','x','x')",
    )
    .bind(platform_admin)
    .bind(format!("m1padm-{}", &platform_admin.to_string()[..6]))
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_sessions \
           (user_id, token_hash, csrf_token, created_at, last_seen_at, expires_at, idle_expires_at) \
         VALUES ($1,$2,'csrf',now(),now(),now() + interval '1 day',now() + interval '1 day')",
    )
    .bind(platform_admin)
    .bind(&platform_token)
    .execute(&db.pool)
    .await
    .unwrap();
    let platform_role = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO roles (id, name, cooperative_id, status) \
         VALUES ($1, 'm1-platform-admin', NULL, 'active')",
    )
    .bind(platform_role)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission_id) \
         SELECT $1, id FROM permissions WHERE key IN ('users.manage', 'roles.manage')",
    )
    .bind(platform_role)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
         VALUES ($1, $2, NULL)",
    )
    .bind(platform_admin)
    .bind(platform_role)
    .execute(&db.pool)
    .await
    .unwrap();

    // Global disable on the platform lane: id returned, the global
    // identity flips, sessions revoked, user_disabled minted.
    let (mut tx, _uid) = app_db::request_user_tx(&app, &platform_token, now)
        .await
        .expect("platform-sealed tx");
    let updated: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_platform_user_set_status($1, 'disabled')")
            .bind(p.user_c)
            .fetch_one(&mut *tx)
            .await
            .expect("platform admin may disable a global user");
    assert_eq!(updated, Some(p.user_c));
    sqlx::query("SELECT public.app_admin_revoke_user_sessions($1, $2)")
        .bind(p.user_c)
        .bind(now)
        .execute(&mut *tx)
        .await
        .expect("platform session revocation");
    tx.commit().await.unwrap();

    let user_status: String = sqlx::query_scalar("SELECT status FROM users WHERE id = $1")
        .bind(p.user_c)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(user_status, "disabled");

    // Global disable blocks ALL cooperative access: the target's
    // session resolves no cooperative.
    let denied = app_db::request_tx(&app, &[0x53u8; 32], p.coop_a, now).await;
    assert!(
        denied.is_err(),
        "globally disabled user must resolve nothing"
    );

    // Global reactivation does NOT reactivate suspended memberships.
    sqlx::query(
        "UPDATE cooperative_memberships SET status = 'suspended' \
         WHERE user_id = $1 AND cooperative_id = $2",
    )
    .bind(p.user_c)
    .bind(p.coop_a)
    .execute(&db.pool)
    .await
    .unwrap();
    let (mut tx, _uid) = app_db::request_user_tx(&app, &platform_token, now)
        .await
        .expect("platform-sealed tx");
    let enabled: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_platform_user_set_status($1, 'active')")
            .bind(p.user_c)
            .fetch_one(&mut *tx)
            .await
            .expect("platform re-enable");
    assert_eq!(enabled, Some(p.user_c));
    tx.commit().await.unwrap();
    let m_status: String = sqlx::query_scalar(
        "SELECT status FROM cooperative_memberships WHERE user_id = $1 AND cooperative_id = $2",
    )
    .bind(p.user_c)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        m_status, "suspended",
        "global reactivation must not reactivate a suspended membership"
    );
    let denied = app_db::request_tx(&app, &[0x53u8; 32], p.coop_a, now).await;
    assert!(
        denied.is_err(),
        "reactivated identity with suspended membership must resolve nothing"
    );

    db.cleanup().await;
}

/// Forged contexts cannot invoke the mutators: hand-set GUCs and
/// unsealed calls both fail inside the definers — and RLS still hides
/// every user row on that forged connection.
#[tokio::test]
async fn r2_forged_guc_cannot_set_user_status() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;

    for sql in [
        "SELECT public.app_membership_set_status($1, 'suspended')",
        "SELECT public.app_user_set_status($1, 'disabled')",
        "SELECT public.app_platform_user_set_status($1, 'disabled')",
    ] {
        // Each probe needs its own tx: a denial aborts the transaction.
        let mut tx = app.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.user_id', $1::text, true)")
            .bind(p.user_a)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('app.cooperative_id', $1::text, true)")
            .bind(p.coop_a)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('app.ctx_txid', txid_current()::text, true)")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('app.ctx_seal', 'deadbeef', true)")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('app.user_seal', 'deadbeef', true)")
            .execute(&mut *tx)
            .await
            .unwrap();
        let res = sqlx::query_scalar::<_, Option<Uuid>>(sql)
            .bind(p.user_c)
            .fetch_optional(&mut *tx)
            .await;
        assert!(
            matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "forged seals must not invoke {sql}"
        );
        tx.rollback().await.unwrap();
    }

    // The forged connection never saw a single user row — RLS needs
    // the seal, not the GUC values.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&app)
        .await
        .unwrap();
    assert_eq!(n, 0, "forged GUCs must reveal no user rows");

    let res = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT public.app_membership_set_status($1, 'suspended')",
    )
    .bind(p.user_c)
    .fetch_optional(&app)
    .await;
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "unsealed invocation must be denied"
    );

    db.cleanup().await;
}

/// Never-bootstrapped lane: pre-tenancy a user-sealed tx carries the
/// NULL arm — users.manage via NULL-attributed legacy roles authorizes
/// app_user_set_status; the first cooperative row closes the lane.
#[tokio::test]
async fn r2_never_bootstrapped_lane_opens_then_closes() {
    let db = setup().await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();

    // Pre-tenancy fixture: admin user + target user, NULL-attributed
    // users.manage role and assignment — owner lane writes.
    let admin = Uuid::new_v4();
    let target = Uuid::new_v4();
    let token: Vec<u8> = vec![0x5au8; 32];
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, login_verifier) \
         VALUES ($1,$2,'R2','x','x'), ($3,$4,'R2T','x','x')",
    )
    .bind(admin)
    .bind(format!("r2-pre-{}", &admin.to_string()[..6]))
    .bind(target)
    .bind(format!("r2-tgt-{}", &target.to_string()[..6]))
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_sessions \
           (user_id, token_hash, csrf_token, created_at, last_seen_at, expires_at, idle_expires_at) \
         VALUES ($1,$2,'csrf',now(),now(),now() + interval '1 day',now() + interval '1 day')",
    )
    .bind(admin)
    .bind(&token)
    .execute(&db.pool)
    .await
    .unwrap();
    let role = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO roles (id, name, cooperative_id, status) \
         VALUES ($1, 'r2-legacy-admin', NULL, 'active')",
    )
    .bind(role)
    .execute(&db.pool)
    .await
    .unwrap();
    // The admin carries roles.manage too — disabling `target` must not
    // trip the last-administrator guard (target is no admin path).
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission_id) \
         SELECT $1, id FROM permissions WHERE key IN ('users.manage', 'roles.manage')",
    )
    .bind(role)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
         VALUES ($1, $2, NULL)",
    )
    .bind(admin)
    .bind(role)
    .execute(&db.pool)
    .await
    .unwrap();

    // Pre-tenancy: user seal alone sees every user (NULL arm) and the
    // definer manages them.
    let (mut tx, _uid) = app_db::request_user_tx(&app, &token, now)
        .await
        .expect("user-sealed tx");
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(visible, 2, "pre-tenancy NULL arm must see all users");
    let updated: Option<Uuid> =
        sqlx::query_scalar("SELECT public.app_user_set_status($1, 'disabled')")
            .bind(target)
            .fetch_one(&mut *tx)
            .await
            .expect("pre-tenancy NULL-arm manage");
    assert_eq!(updated, Some(target));
    tx.rollback().await.unwrap();

    // First cooperative appears → the same user seal instantly loses
    // both visibility and the mutation lane.
    sqlx::query(
        "INSERT INTO cooperatives (id, name, status, is_bootstrap, business_enabled) \
         VALUES (gen_random_uuid(), 'r2-first', 'active', false, true)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let (mut tx, _uid) = app_db::request_user_tx(&app, &token, now)
        .await
        .expect("user-sealed tx after tenancy");
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(visible, 0, "post-tenancy user seal must see nothing");
    let res =
        sqlx::query_scalar::<_, Option<Uuid>>("SELECT public.app_user_set_status($1, 'active')")
            .bind(target)
            .fetch_optional(&mut *tx)
            .await;
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "post-tenancy NULL lane must close"
    );
    tx.rollback().await.unwrap();

    db.cleanup().await;
}

// ────────────────────────────────────────────────────────────────────
// R3 — financial history & grant-surface hardening (migration 0054,
// closure-audit F4). Invariant: a sealed cooperative context must not
// be able to destructively delete or rewrite posted financial history.
// DELETE survives only for draft periods / their rules; UPDATE is
// confined to the exact columns each repository writes, and on
// terminal-lifecycle tables the only permitted write is the single
// forward transition — frozen afterwards.
// ────────────────────────────────────────────────────────────────────

/// A sealed member tx for the R3 probes — plain member, no
/// administrative permission: the grant/RLS layer is what is being
/// tested, not the HTTP permission model.
async fn r3_sealed(
    app: &PgPool,
    token: &[u8],
    coop: Uuid,
) -> sqlx::Transaction<'static, sqlx::Postgres> {
    app_db::request_tx(app, token, coop, OffsetDateTime::now_utc())
        .await
        .expect("sealed request tx")
        .0
}

/// One statement on its own sealed tx — grant-layer errors (42501)
/// abort the transaction, so every denied probe needs a fresh context.
async fn r3_probe_id(
    app: &PgPool,
    token: &[u8],
    coop: Uuid,
    sql: &str,
    id: Uuid,
) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    let mut tx = r3_sealed(app, token, coop).await;
    let res = sqlx::query(sql).bind(id).execute(&mut *tx).await;
    tx.rollback().await.ok();
    res
}

fn r3_is_42501(res: &Result<sqlx::postgres::PgQueryResult, sqlx::Error>) -> bool {
    matches!(res, Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some("42501"))
}

/// Grant `user` a role holding every listed permission inside `coop`
/// (owner lane — the legitimate authorized-actor lane for R3B probes).
async fn r3_grant_perms(db: &TestDb, coop: Uuid, user: Uuid, keys: &[&str]) {
    let role = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO roles (id, name, cooperative_id, status) \
         VALUES ($1, $2, $3, 'active')",
    )
    .bind(role)
    .bind(format!("r3b-{}", &role.to_string()[..8]))
    .bind(coop)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission_id) \
         SELECT $1, id FROM permissions WHERE key = ANY($2)",
    )
    .bind(role)
    .bind(keys)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
         VALUES ($1, $2, $3)",
    )
    .bind(user)
    .bind(role)
    .bind(coop)
    .execute(&db.pool)
    .await
    .unwrap();
}

/// Seed an investment + funding + valuation for `coop` on the owner
/// lane (batch3_rows also seeds a governance body with a fixed
/// body_number, which collides with batch_rows — this helper avoids it).
async fn r3_investment_rows(
    db: &TestDb,
    coop: Uuid,
    user: Uuid,
    account: Uuid,
    tag: &str,
) -> (Uuid, Uuid, Uuid) {
    let investment: Uuid = sqlx::query_scalar(
        "INSERT INTO investments \
         (name, investment_type, idempotency_key, idempotency_fingerprint, \
          created_by, cooperative_id, investment_number) \
         VALUES ($2,'real_estate',$3,$4,$5,$1, \
                 public.allocate_document_number($1,'investments.investment_number')) RETURNING id",
    )
    .bind(coop)
    .bind(format!("R3Inv{tag}"))
    .bind(format!("r3inv-{tag}"))
    .bind(format!("r3invfp-{tag}"))
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let funding: Uuid = Uuid::new_v4();
    let funding_movement: Uuid = sqlx::query_scalar(
        "INSERT INTO account_movements \
         (account_id, direction, amount, source_type, source_id, occurred_at, \
          created_by, cooperative_id) \
         VALUES ($1,'outflow',50,'investment_funding',$2,now(),$3,$4) RETURNING id",
    )
    .bind(account)
    .bind(funding)
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO investment_fundings \
         (id, investment_id, financial_account_id, amount, occurred_at, \
          account_movement_id, idempotency_key, idempotency_fingerprint, \
          created_by, cooperative_id, funding_number) \
         VALUES ($1,$2,$3,50,now(),$4,$5,$6,$7,$8, \
                 public.allocate_document_number($8,'investment_fundings.funding_number'))",
    )
    .bind(funding)
    .bind(investment)
    .bind(account)
    .bind(funding_movement)
    .bind(format!("r3fnd-{tag}"))
    .bind(format!("r3fndfp-{tag}"))
    .bind(user)
    .bind(coop)
    .execute(&db.pool)
    .await
    .unwrap();
    let valuation: Uuid = sqlx::query_scalar(
        "INSERT INTO investment_valuations \
         (investment_id, valuation_date, amount, idempotency_key, \
          idempotency_fingerprint, created_by, cooperative_id, valuation_number) \
         VALUES ($1,'2026-03-01',60,$2,$3,$4,$5, \
                 public.allocate_document_number($5,'investment_valuations.valuation_number')) RETURNING id",
    )
    .bind(investment)
    .bind(format!("r3val-{tag}"))
    .bind(format!("r3valfp-{tag}"))
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    (investment, funding, valuation)
}

/// F4 reproduction, now closed: a sealed member context cannot delete
/// or rewrite posted financial rows — every historical column left
/// the UPDATE surface and DELETE is gone.
#[tokio::test]
async fn r3_posted_financial_history_is_immutable() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch_rows(&db, p.coop_a, p.user_a, "r3a").await;
    let rows2 = batch2_rows(&db, p.coop_a, p.user_a, "r3b").await;
    let rows4 = batch4_rows(&db, p.coop_a, p.user_a, "r3c").await;
    let rows6 = batch6_rows(&db, p.coop_a, p.user_a, p.body_a, "r3d").await;
    let app = db.app_pool().await;
    let token_c = vec![0x53u8; 32];

    // Historical columns are off the UPDATE surface.
    for (label, sql, id) in [
        (
            "payments.amount",
            "UPDATE payments SET amount = 0 WHERE id = $1",
            rows2.payment,
        ),
        (
            "payments.payer",
            "UPDATE payments SET payer_person_id = NULL WHERE id = $1",
            rows2.payment,
        ),
        (
            "payments.cooperative_id",
            "UPDATE payments SET cooperative_id = NULL WHERE id = $1",
            rows2.payment,
        ),
        (
            "account_movements.amount",
            "UPDATE account_movements SET amount = 1 WHERE id = $1",
            rows6.movement,
        ),
        (
            "account_movements.account",
            "UPDATE account_movements SET account_id = NULL WHERE id = $1",
            rows6.movement,
        ),
        (
            "income_entries.amount",
            "UPDATE income_entries SET amount = 0 WHERE id = $1",
            rows_a.income_entry,
        ),
        (
            "shareholder_credits.amount",
            "UPDATE shareholder_credits SET amount = 0 WHERE id = $1",
            rows2.credit,
        ),
        (
            "persons.cooperative_id",
            "UPDATE persons SET cooperative_id = NULL WHERE id = $1",
            rows4.person,
        ),
        (
            "assessments.status (no UPDATE grant at all)",
            "UPDATE assessments SET status = 'voided' WHERE id = $1",
            rows2.assessment,
        ),
        (
            "share_events.event_type (append-only)",
            "UPDATE share_events SET event_type = 'voided' WHERE id = $1",
            rows4.share_event,
        ),
    ] {
        let res = r3_probe_id(&app, &token_c, p.coop_a, sql, id).await;
        assert!(r3_is_42501(&res), "{label}: UPDATE must be 42501");
    }

    // Posted history is non-deletable — every table except the two
    // draft-cleanup lanes lost DELETE.
    for (table, id) in [
        ("payments", rows2.payment),
        ("account_movements", rows6.movement),
        ("account_transfers", rows6.transfer),
        ("income_entries", rows_a.income_entry),
        ("expense_entries", rows_a.expense_entry),
        ("payment_allocations", rows_a.payment_allocation),
        ("assessments", rows2.assessment),
        ("shareholder_credits", rows2.credit),
        ("credit_applications", rows2.credit_application),
        ("persons", rows4.person),
        ("shares", rows4.share),
        ("share_events", rows4.share_event),
    ] {
        let res = r3_probe_id(
            &app,
            &token_c,
            p.coop_a,
            &format!("DELETE FROM {table} WHERE id = $1"),
            id,
        )
        .await;
        assert!(r3_is_42501(&res), "{table}: DELETE must be 42501");
    }

    // Owner lane confirms nothing moved.
    let (amount, status): (rust_decimal::Decimal, String) =
        sqlx::query_as("SELECT amount, status FROM payments WHERE id = $1")
            .bind(rows2.payment)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(status, "posted");
    assert_eq!(amount, rust_decimal::Decimal::new(20, 0));

    app.close().await;
    db.cleanup().await;
}

/// The only UPDATE the runtime can perform on a lifecycle table is the
/// single forward terminal transition — afterwards the row is frozen,
/// a second identical write hits zero rows, and the transition cannot
/// be walked back.
#[tokio::test]
async fn r3_terminal_transitions_are_forward_only() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_a = batch_rows(&db, p.coop_a, p.user_a, "t1a").await;
    let rows2 = batch2_rows(&db, p.coop_a, p.user_a, "t1b").await;
    let rows5 = batch5_rows(&db, p.coop_a, p.user_a, "t1d").await;
    let rows6 = batch6_rows(&db, p.coop_a, p.user_a, p.body_a, "t1e").await;
    let (_investment, funding, valuation) =
        r3_investment_rows(&db, p.coop_a, p.user_a, rows6.account, "t1c").await;
    let app = db.app_pool().await;
    let token_c = vec![0x53u8; 32];

    // R3B: lifecycle writes run through permission-checked definers.
    // user_c becomes the authorized actor for the legitimate path.
    r3_grant_perms(
        &db,
        p.coop_a,
        p.user_c,
        &[
            "payments.manage",
            "financial_accounts.manage",
            "income_expense.manage",
            "credits.manage",
            "investments.manage",
            "share_returns.manage",
            "social_aid.manage",
            "governance.manage",
        ],
    )
    .await;

    let return_share: Uuid = sqlx::query_scalar("SELECT share_id FROM share_returns WHERE id = $1")
        .bind(rows5.share_return)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    // The mark-pending package requires the 'return_requested' event —
    // seed it (owner lane) as the create flow would have.
    sqlx::query(
        "INSERT INTO share_events \
         (share_id, event_type, occurred_at, actor_user_id, cooperative_id) \
         VALUES ($1,'return_requested',now(),$2,$3)",
    )
    .bind(return_share)
    .bind(p.user_a)
    .bind(p.coop_a)
    .execute(&db.pool)
    .await
    .unwrap();
    // A bare investment for the cancel package — the one above carries
    // a funding row and is (correctly) un-cancellable forever.
    let bare_investment: Uuid = sqlx::query_scalar(
        "INSERT INTO investments \
         (name, investment_type, idempotency_key, idempotency_fingerprint, \
          created_by, cooperative_id, investment_number) \
         VALUES ('R3Bare','real_estate','r3bare','r3barefp',$1,$2, \
                 public.allocate_document_number($2,'investments.investment_number')) RETURNING id",
    )
    .bind(p.user_a)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    // Movements reverse via their SOURCE package now — link the seeded
    // loose movement to the income entry it must travel with.
    sqlx::query("UPDATE account_movements SET source_id = $1 WHERE id = $2")
        .bind(rows_a.income_entry)
        .bind(rows6.movement)
        .execute(&db.pool)
        .await
        .unwrap();

    // (table, row id checked afterwards, legitimate package call,
    // value bound as $2, extra binds appended after $3=coop).
    // Convention: $1 = actor, $3 = cooperative scope arg (p_coop),
    // $4+ = extras. Ordering is load-bearing: the credit application
    // releases the credit BEFORE the payment package runs; the
    // disbursement returns restricted money BEFORE the donation
    // package re-checks the fund's restricted availability.
    let forward: &[(&str, Uuid, &str, Uuid, &[Uuid])] = &[
        (
            "credit_applications",
            rows2.credit_application,
            "SELECT public.app_credit_application_reverse($2, now(), $1, 'r3', $3)",
            rows2.credit_application,
            &[],
        ),
        (
            "payments",
            rows2.payment,
            "SELECT public.app_payment_reverse_tx($2, now(), $1, 'r3', $3)",
            rows2.payment,
            &[],
        ),
        (
            "payment_allocations",
            rows_a.payment_allocation,
            "SELECT public.app_payment_allocation_reverse($2, now(), $1, 'r3', $3)",
            rows_a.payment_allocation,
            &[],
        ),
        (
            "account_movements",
            rows6.movement,
            "SELECT public.app_income_entry_reverse_tx($2, now(), $1, 'r3', $3)",
            rows_a.income_entry,
            &[],
        ),
        (
            "account_transfers",
            rows6.transfer,
            "SELECT public.app_transfer_reverse_tx($2, now(), $1, 'r3', $3)",
            rows6.transfer,
            &[],
        ),
        (
            "income_entries",
            rows_a.income_entry,
            "SELECT public.app_income_entry_reverse_tx($2, now(), $1, 'r3', $3)",
            rows_a.income_entry,
            &[],
        ),
        (
            "expense_entries",
            rows_a.expense_entry,
            "SELECT public.app_expense_entry_reverse_tx($2, now(), $1, 'r3', $3)",
            rows_a.expense_entry,
            &[],
        ),
        (
            "shareholder_credits",
            rows2.credit,
            "SELECT public.app_credit_reverse_tx($2, now(), $1, 'r3', $3)",
            rows2.credit,
            &[],
        ),
        (
            "investment_fundings",
            funding,
            "SELECT public.app_investment_funding_reverse_tx($2, now(), $1, 'r3', $3)",
            funding,
            &[],
        ),
        (
            "investment_incomes",
            rows_a.investment_income,
            "SELECT public.app_investment_income_reverse_tx($2, now(), $1, 'r3', $3)",
            rows_a.investment_income,
            &[],
        ),
        (
            "share_return_settlements",
            rows5.settlement,
            "SELECT public.app_share_return_settlement_reverse_tx($2, now(), $1, 'r3', $3)",
            rows5.settlement,
            &[],
        ),
        (
            "social_aid_disbursements",
            rows5.disbursement,
            "SELECT public.app_social_aid_disbursement_reverse_tx($2, now(), $1, 'r3', $3)",
            rows5.disbursement,
            &[],
        ),
        (
            "social_aid_donations",
            rows5.donation,
            "SELECT public.app_social_aid_donation_reverse_tx($2, now(), $1, 'r3', $3)",
            rows5.donation,
            &[],
        ),
        (
            "investment_valuations",
            valuation,
            "SELECT public.app_investment_valuation_cancel($2, now(), $1, 'r3', $3)",
            valuation,
            &[],
        ),
        (
            "investments",
            bare_investment,
            "SELECT public.app_investment_cancel_tx($2, now(), $1, 'r3', $3)",
            bare_investment,
            &[],
        ),
        (
            "share_returns",
            rows5.share_return,
            "SELECT public.app_share_return_cancel_tx($2, now(), $1, 'r3', $3)",
            rows5.share_return,
            &[],
        ),
        (
            "social_aid_funds",
            rows5.fund,
            "SELECT public.app_social_aid_fund_close_tx($2, now(), $1, $3)",
            rows5.fund,
            &[],
        ),
        (
            "governance_bodies",
            p.body_a,
            "SELECT public.app_governance_body_close($2, $1, $3)",
            p.body_a,
            &[],
        ),
    ];

    // All legitimate transitions land in ONE sealed tx, executed by an
    // actor with the verified module permissions.
    let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
    // The share must be marked return_pending (event-backed) before
    // the return case can cancel — part of the same atomic operation
    // ordering the route layer performs.
    sqlx::query("SELECT public.app_share_return_mark_pending_tx($1, now(), $2)")
        .bind(return_share)
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await
        .expect("mark the share return_pending");
    for (table, _check, sql, bind_id, extra) in forward {
        let mut q = sqlx::query(sql)
            .bind(p.user_c)
            .bind(*bind_id)
            .bind(p.coop_a);
        for x in *extra {
            q = q.bind(*x);
        }
        q.execute(&mut *tx)
            .await
            .unwrap_or_else(|e| panic!("{table}: forward transition failed: {e}"));
    }
    tx.commit().await.unwrap();

    // Frozen afterwards — the runtime role no longer holds any UPDATE
    // grant on lifecycle columns, so a direct probe is always 42501;
    // the definer precondition additionally makes a second transition
    // a no-op / precondition failure.
    for (table, row, _, _, _) in forward {
        let res = r3_probe_id(
            &app,
            &token_c,
            p.coop_a,
            &format!("UPDATE {table} SET status = status WHERE id = $1"),
            *row,
        )
        .await;
        match res {
            Ok(done) => assert_eq!(
                done.rows_affected(),
                0,
                "{table}: terminal row must be frozen"
            ),
            Err(sqlx::Error::Database(e)) => assert_eq!(
                e.code().as_deref(),
                Some("42501"),
                "{table}: terminal row must be frozen"
            ),
            Err(e) => panic!("{table}: unexpected error {e}"),
        }
    }

    // Reversal without attribution violates the row CHECK (23514).
    // Since R3B the app role holds no UPDATE on payments at all, so the
    // CHECK is exercised on the owner lane — it is a constraint, not a
    // policy, and binds every lane.
    let pay2: Uuid = sqlx::query_scalar(
        "INSERT INTO payments \
         (payer_person_id, amount, method, received_at, idempotency_key, \
          idempotency_fingerprint, created_by, cooperative_id, payment_number) \
         VALUES ((SELECT id FROM persons WHERE cooperative_id = $1 LIMIT 1), \
                 10,'cash',now(),'r3-t1f','r3-t1ffp',$2,$1, \
                 public.allocate_document_number($1,'payments.payment_number')) \
         RETURNING id",
    )
    .bind(p.coop_a)
    .bind(p.user_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let res = sqlx::query("UPDATE payments SET status='reversed' WHERE id = $1")
        .bind(pay2)
        .execute(&db.pool)
        .await;
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("23514")),
        "reversal without attribution must hit the CHECK"
    );

    app.close().await;
    db.cleanup().await;
}

/// Rows cannot be born in a terminal state — the runtime cannot mint
/// pre-reversed / pre-closed / pre-cancelled history.
#[tokio::test]
async fn r3_terminal_rows_cannot_be_inserted() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    r3_grant_perms(
        &db,
        p.coop_a,
        p.user_c,
        &["payments.manage", "investments.manage"],
    )
    .await;
    let rows4 = batch4_rows(&db, p.coop_a, p.user_a, "ins").await;
    let app = db.app_pool().await;
    let token_c = vec![0x53u8; 32];

    // Pre-reversed payment insert -> birth-pin (42501) or RLS WITH
    // CHECK violation — either lane-authorization denial is valid.
    let res = {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        sqlx::query(
            "INSERT INTO payments \
             (payer_person_id, amount, method, received_at, idempotency_key, \
              idempotency_fingerprint, created_by, cooperative_id, \
              status, reversed_at, reversed_by, reversal_reason) \
             VALUES ($1,10,'cash',now(),'r3-dead','r3-deadfp',$2,$3, \
                     'reversed', now(), $2, 'forged')",
        )
        .bind(rows4.person)
        .bind(p.user_c)
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await
    };
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "pre-reversed insert must be rejected"
    );

    // Pre-disposed investment insert -> rejected as well.
    let res = {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        sqlx::query(
            "INSERT INTO investments \
             (name, investment_type, idempotency_key, idempotency_fingerprint, \
              created_by, cooperative_id, investment_number, status, \
              disposed_at, disposed_by) \
             VALUES ('dead','business','r3-d','r3-dfp',$1,$2, \
                     public.allocate_document_number($2,'investments.investment_number'), \
                     'disposed', now(), $1)",
        )
        .bind(p.user_c)
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await
    };
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "pre-disposed investment insert must be rejected"
    );

    // A legitimate live insert still lands — the document number is
    // now server-assigned by `payments_number_pin` (0058), so the
    // column is omitted entirely.
    let res = {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        sqlx::query(
            "INSERT INTO payments \
             (payer_person_id, amount, method, received_at, idempotency_key, \
              idempotency_fingerprint, created_by, cooperative_id) \
             VALUES ($1,10,'cash',now(),'r3-live','r3-livefp',$2,$3)",
        )
        .bind(rows4.person)
        .bind(p.user_c)
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await
    };
    assert!(res.is_ok(), "live payment insert must succeed: {res:?}");

    app.close().await;
    db.cleanup().await;
}

/// The only runtime DELETEs left are draft-confined: a draft period
/// and its rules can be removed; once a period opens both become
/// immutable to the runtime role.
#[tokio::test]
async fn r3_deletion_is_draft_confined() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let token_c = vec![0x53u8; 32];

    // Owner lane: one draft period + rule, one open period + rule.
    let draft_period: Uuid = sqlx::query_scalar(
        "INSERT INTO periods (name, collection_start_date, due_date, cooperative_id, period_number, status) \
         VALUES ('R3D','2026-05-01','2026-05-31',$1,88,'draft') RETURNING id",
    )
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let draft_rule: Uuid = sqlx::query_scalar(
        "INSERT INTO assessment_rules \
         (period_id, rule_type, base_amount, assessment_effective_date, cooperative_id) \
         VALUES ($1,'per_share',10,'2026-05-01',$2) RETURNING id",
    )
    .bind(draft_period)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let open_period: Uuid = sqlx::query_scalar(
        "INSERT INTO periods (name, collection_start_date, due_date, cooperative_id, period_number, status) \
         VALUES ('R3O','2026-06-01','2026-06-30',$1,89,'open') RETURNING id",
    )
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let open_rule: Uuid = sqlx::query_scalar(
        "INSERT INTO assessment_rules \
         (period_id, rule_type, base_amount, assessment_effective_date, cooperative_id) \
         VALUES ($1,'per_share',10,'2026-06-01',$2) RETURNING id",
    )
    .bind(open_period)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    // Draft rule edit + delete succeed; open-parent rule is frozen.
    let res = r3_probe_id(
        &app,
        &token_c,
        p.coop_a,
        "UPDATE assessment_rules SET base_amount = 20 WHERE id = $1",
        draft_rule,
    )
    .await;
    assert_eq!(
        res.expect("draft rule update").rows_affected(),
        1,
        "draft rule must remain editable"
    );
    let res = r3_probe_id(
        &app,
        &token_c,
        p.coop_a,
        "UPDATE assessment_rules SET base_amount = 20 WHERE id = $1",
        open_rule,
    )
    .await;
    assert_eq!(
        res.expect("open rule update").rows_affected(),
        0,
        "rule of an open period must be immutable"
    );
    // The real draft-cleanup deletes the rule then the period inside
    // ONE committed tx (the probe helper rolls back, which would leave
    // the FK reference dangling).
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let res = sqlx::query("DELETE FROM assessment_rules WHERE id = $1")
            .bind(draft_rule)
            .execute(&mut *tx)
            .await
            .expect("draft rule delete");
        assert_eq!(res.rows_affected(), 1, "draft rule must be deletable");
        let res = sqlx::query("DELETE FROM periods WHERE id = $1")
            .bind(draft_period)
            .execute(&mut *tx)
            .await
            .expect("draft period delete");
        assert_eq!(res.rows_affected(), 1, "draft period must be deletable");
        tx.commit().await.unwrap();
    }
    let res = r3_probe_id(
        &app,
        &token_c,
        p.coop_a,
        "DELETE FROM assessment_rules WHERE id = $1",
        open_rule,
    )
    .await;
    assert_eq!(
        res.expect("open rule delete").rows_affected(),
        0,
        "open-period rule must not be deletable"
    );

    let res = r3_probe_id(
        &app,
        &token_c,
        p.coop_a,
        "DELETE FROM periods WHERE id = $1",
        open_period,
    )
    .await;
    assert_eq!(
        res.expect("open period delete").rows_affected(),
        0,
        "open period must not be deletable"
    );

    // Rule INSERT grafted onto an open period is rejected too.
    let res = {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        sqlx::query(
            "INSERT INTO assessment_rules \
             (period_id, rule_type, base_amount, assessment_effective_date, cooperative_id) \
             VALUES ($1,'per_share',1,'2026-06-01',$2)",
        )
        .bind(open_period)
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await
    };
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "rule graft onto an open period must be rejected"
    );

    app.close().await;
    db.cleanup().await;
}

/// Mutable operational configuration keeps working in scope —
/// account/category/person/shareholder/share edits land — while the
/// attribution columns stay unwritable.
#[tokio::test]
async fn r3_operational_edits_still_work_attribution_frozen() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows3 = batch3_rows(&db, p.coop_a, p.user_a, "op3").await;
    let rows4 = batch4_rows(&db, p.coop_a, p.user_a, "op4").await;
    let rows6 = batch6_rows(&db, p.coop_a, p.user_a, p.body_a, "op6").await;
    let app = db.app_pool().await;
    let token_c = vec![0x53u8; 32];

    // R3B: the lifecycle portion runs through definers; grant the
    // module permissions to the sealed actor.
    r3_grant_perms(
        &db,
        p.coop_a,
        p.user_c,
        &["shareholders.manage", "shares.manage", "governance.manage"],
    )
    .await;

    let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
    for (label, sql, id) in [
        (
            "financial_accounts.name",
            "UPDATE financial_accounts SET name = 'Yeni' WHERE id = $1",
            rows6.account,
        ),
        (
            "financial_categories.description",
            "UPDATE financial_categories SET description = 'd' WHERE id = $1",
            rows6.category,
        ),
        (
            "persons.first_name",
            "UPDATE persons SET first_name = 'Y' WHERE id = $1",
            rows4.person,
        ),
        (
            "governance_decisions.title",
            "UPDATE governance_decisions SET title = 'Yeni' WHERE id = $1",
            rows3.decision,
        ),
    ] {
        let res = sqlx::query(sql)
            .bind(id)
            .execute(&mut *tx)
            .await
            .unwrap_or_else(|e| panic!("{label}: legitimate edit failed: {e}"));
        assert_eq!(res.rows_affected(), 1, "{label}");
    }
    // Lifecycle writes are package-definer-only now. Ownership
    // intervals have no standalone close — they end inside the
    // transfer/finalize packages only.
    for (label, sql, id) in [
        (
            "shareholders.status",
            "SELECT public.app_shareholder_set_status_tx($1, 'inactive', 'r3', NULL, $2)",
            rows4.shareholder,
        ),
        (
            "shares.status",
            "SELECT public.app_share_set_status_tx($1, 'suspended', 'r3', now(), NULL, $2)",
            rows4.share,
        ),
    ] {
        sqlx::query(sql)
            .bind(id)
            .bind(p.coop_a)
            .execute(&mut *tx)
            .await
            .unwrap_or_else(|e| panic!("{label}: definer transition failed: {e}"));
    }
    sqlx::query("SELECT public.app_governance_membership_end($1, now(), $2, 'r', $3)")
        .bind(rows6.membership)
        .bind(p.user_c)
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await
        .expect("membership end must succeed");
    tx.commit().await.unwrap();

    app.close().await;
    db.cleanup().await;
}

/// A sealed coop-A context cannot mutate coop-B financial rows:
/// historical-column writes die at the grant layer and lifecycle
/// writes see zero rows.
#[tokio::test]
async fn r3_cross_cooperative_financial_mutation_denied() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows_b = batch2_rows(&db, p.coop_b, p.user_c, "xb").await;
    let app = db.app_pool().await;
    let token_c = vec![0x53u8; 32];

    let res = r3_probe_id(
        &app,
        &token_c,
        p.coop_a,
        "UPDATE payments SET amount = 1 WHERE id = $1",
        rows_b.payment,
    )
    .await;
    assert!(r3_is_42501(&res), "cross-coop amount write must be 42501");

    let res = r3_probe_id(
        &app,
        &token_c,
        p.coop_a,
        "DELETE FROM payments WHERE id = $1",
        rows_b.payment,
    )
    .await;
    assert!(r3_is_42501(&res), "cross-coop delete must be 42501");

    // Lifecycle write on a foreign row: direct UPDATE has no grant at
    // all (42501); the definer path sees zero rows because the stamped
    // scope does not match the row's cooperative.
    let res = r3_probe_id(
        &app,
        &token_c,
        p.coop_a,
        "UPDATE payments SET status='reversed', reversed_at=now(), \
         reversed_by=gen_random_uuid(), reversal_reason='x', updated_at=now() WHERE id = $1",
        rows_b.payment,
    )
    .await;
    assert!(r3_is_42501(&res), "direct lifecycle UPDATE must be 42501");

    let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
    // user_c has no payments.manage, so the definer must deny outright.
    let res = sqlx::query("SELECT public.app_payment_reverse($1, now(), $2, 'x', $3)")
        .bind(rows_b.payment)
        .bind(p.user_c)
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await;
    assert!(
        matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
        "definer must deny a member without payments.manage"
    );

    let (status,): (String,) = sqlx::query_as("SELECT status FROM payments WHERE id = $1")
        .bind(rows_b.payment)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(status, "posted", "foreign payment must be untouched");

    app.close().await;
    db.cleanup().await;
}

/// R3B adversarial suite — a sealed cooperative session ALONE must not
/// authorize financial lifecycle transitions. Every sensitive writer
/// re-verifies the module permission inside the database boundary via
/// narrow SECURITY DEFINER functions (0055); direct lifecycle UPDATE
/// grants are gone.
#[tokio::test]
async fn r3b_lifecycle_transitions_require_verified_permission() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows2 = batch2_rows(&db, p.coop_a, p.user_a, "r3ba").await;
    let rows_b = batch2_rows(&db, p.coop_b, p.user_c, "r3bb").await;
    let rows5 = batch5_rows(&db, p.coop_a, p.user_a, "r3bc").await;
    let draft_period: Uuid = sqlx::query_scalar(
        "INSERT INTO periods (name, collection_start_date, due_date, \
         cooperative_id, period_number, status) \
         VALUES ('R3BD','2026-03-01','2026-03-31',$1,910,'draft') RETURNING id",
    )
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    // The period-open package requires at least one generated
    // assessment — seed one so the legitimate open path flips.
    let shareholder: Uuid =
        sqlx::query_scalar("SELECT shareholder_id FROM assessments WHERE id = $1")
            .bind(rows2.assessment)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO assessments \
         (period_id, shareholder_id, rule_type, base_amount, amount, currency, \
          assessment_effective_date, generated_by, cooperative_id) \
         VALUES ($1,$2,'per_share',10,10,'TRY','2026-03-01',$3,$4)",
    )
    .bind(draft_period)
    .bind(shareholder)
    .bind(p.user_a)
    .bind(p.coop_a)
    .execute(&db.pool)
    .await
    .unwrap();
    // A clean payment with no credit entanglement for the authorized
    // reversal flow — the batch-2 fixture's payment has an ACTIVE
    // credit application and is (correctly) un-reversible.
    let payer: Uuid = sqlx::query_scalar("SELECT payer_person_id FROM payments WHERE id = $1")
        .bind(rows2.payment)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let pay_clean: Uuid = sqlx::query_scalar(
        "INSERT INTO payments \
         (payer_person_id, amount, method, received_at, idempotency_key, \
          idempotency_fingerprint, created_by, cooperative_id, payment_number) \
         VALUES ($1,20,'cash',now(),'r3b-clean','r3b-cleanfp',$2,$3, \
                 public.allocate_document_number($3,'payments.payment_number')) RETURNING id",
    )
    .bind(payer)
    .bind(p.user_a)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let app = db.app_pool().await;
    let token_c = vec![0x53u8; 32];

    // (1) Forged/absent context: an unsealed kooperatif_app connection
    // cannot drive a definer at all.
    let mut conn = app.acquire().await.unwrap();
    let res = sqlx::query("SELECT public.app_payment_reverse($1, now(), $2, 'x', $3)")
        .bind(rows2.payment)
        .bind(p.user_c)
        .bind(p.coop_a)
        .execute(&mut *conn)
        .await;
    drop(conn);
    assert!(res.is_err(), "unsealed connection must be refused");

    // (2) Sealed member WITHOUT the module permission — every
    // sensitive transition denies with insufficient_privilege (42501).
    // (label, sql, binds) — $-positions vary by signature.
    for (label, sql, binds) in [
        (
            "payments.reverse",
            "SELECT public.app_payment_reverse($1, now(), $2, 'x', $3)",
            vec![rows2.payment, p.user_c, p.coop_a],
        ),
        (
            "movements.reverse(payment)",
            "SELECT public.app_movements_reverse_of_source('payment', $1, now(), $2, 'x', $3)",
            vec![rows2.payment, p.user_c, p.coop_a],
        ),
        (
            "transfer.reverse",
            "SELECT public.app_transfer_reverse($1, now(), $2, 'x', $3)",
            vec![rows_b.payment, p.user_c, p.coop_a],
        ),
        (
            "entitlement.cancel",
            "SELECT public.app_share_return_entitlement_cancel($1, now(), $2, 'x', $3)",
            vec![rows5.entitlement, p.user_c, p.coop_a],
        ),
        (
            "entitlement.determine",
            "SELECT public.app_share_return_entitlement_determine($1, 5, NULL, NULL, NULL, now(), $2)",
            vec![rows5.entitlement, p.coop_a],
        ),
        (
            "period.open",
            "SELECT public.app_period_open($1, $2)",
            vec![draft_period, p.coop_a],
        ),
        (
            "share_return.settlement.reverse",
            "SELECT public.app_share_return_settlement_reverse($1, now(), $2, 'x', $3)",
            vec![rows5.settlement, p.user_c, p.coop_a],
        ),
    ] {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let mut q = sqlx::query(sql);
        for b in &binds {
            q = q.bind(*b);
        }
        let res = q.execute(&mut *tx).await;
        tx.rollback().await.ok();
        assert!(
            matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "{label}: sealed session without permission must be denied"
        );
    }

    // (3) Sealed member without permission — direct lifecycle UPDATE is
    // gone entirely (double boundary).
    let res = r3_probe_id(
        &app,
        &token_c,
        p.coop_a,
        "UPDATE payments SET status='reversed', reversed_at=now(), \
         reversed_by=gen_random_uuid(), reversal_reason='x' WHERE id=$1",
        rows2.payment,
    )
    .await;
    assert!(r3_is_42501(&res), "direct lifecycle UPDATE must be denied");
    let res = r3_probe_id(
        &app,
        &token_c,
        p.coop_a,
        "UPDATE share_return_entitlements SET amount = 0 WHERE id=$1",
        rows5.entitlement,
    )
    .await;
    assert!(r3_is_42501(&res), "entitlement write must be denied");
    let res = r3_probe_id(
        &app,
        &token_c,
        p.coop_a,
        "UPDATE periods SET status='closed' WHERE id=$1",
        draft_period,
    )
    .await;
    assert!(r3_is_42501(&res), "period lifecycle write must be denied");

    // (4) Authorized actor — verified permissions make the legitimate
    // transitions succeed.
    r3_grant_perms(
        &db,
        p.coop_a,
        p.user_c,
        &["payments.manage", "periods.manage", "share_returns.manage"],
    )
    .await;
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        sqlx::query(
            "SELECT public.app_payment_reverse_tx($1, now(), $2, 'authorized reversal', $3)",
        )
        .bind(pay_clean)
        .bind(p.user_c)
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await
        .expect("authorized payment reversal");
        sqlx::query("SELECT public.app_period_open_tx($1, $2)")
            .bind(draft_period)
            .bind(p.coop_a)
            .execute(&mut *tx)
            .await
            .expect("authorized period open");
        tx.commit().await.unwrap();
    }
    let (status, by): (String, Uuid) =
        sqlx::query_as("SELECT status, reversed_by FROM payments WHERE id = $1")
            .bind(pay_clean)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(status, "reversed");
    assert_eq!(by, p.user_c, "reversal must carry the verified actor");

    // (5) Replay: reversing an already-reversed payment through the
    // package is an explicit idempotent replay, not a double reversal.
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let (replayed,): (bool,) = sqlx::query_as(
            "SELECT replayed FROM public.app_payment_reverse_tx($1, now(), $2, 'double', $3)",
        )
        .bind(pay_clean)
        .bind(p.user_c)
        .bind(p.coop_a)
        .fetch_one(&mut *tx)
        .await
        .expect("re-reverse replays, it does not error");
        assert!(replayed, "second reversal must report replayed=true");
        tx.commit().await.unwrap();
    }
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payments WHERE id = $1 AND status = 'reversed'")
            .bind(pay_clean)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(n, 1);

    // (6) Cross-cooperative: the authorized coop-A actor's package
    // call against a coop-B row raises not_found — the verified scope
    // (coop A) does not match, and the caller-supplied p_coop is
    // ignored under the seal.
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let res = sqlx::query("SELECT public.app_payment_reverse_tx($1, now(), $2, 'xcoop', $3)")
            .bind(rows_b.payment)
            .bind(p.user_c)
            .bind(p.coop_b)
            .execute(&mut *tx)
            .await;
        tx.rollback().await.ok();
        assert!(
            matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("P0002")),
            "foreign row must be invisible to the package"
        );
    }
    let (status,): (String,) = sqlx::query_as("SELECT status FROM payments WHERE id = $1")
        .bind(rows_b.payment)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(status, "posted", "foreign payment must be untouched");

    app.close().await;
    db.cleanup().await;
}

/// R3C adversarial suite — an authorized `kooperatif_app` caller must
/// be unable to run an INCOMPLETE reversal: the lifecycle flip and its
/// compensating movement/credit/allocation effects live in one atomic
/// package definer (0056), and the component definers are no longer
/// executable by the runtime role.
#[tokio::test]
async fn r3c_incomplete_reversal_package_is_impossible() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let rows2 = batch2_rows(&db, p.coop_a, p.user_a, "r3ca").await;
    let rows_b = batch2_rows(&db, p.coop_b, p.user_c, "r3cb").await;
    let rows5 = batch5_rows(&db, p.coop_a, p.user_a, "r3cc").await;

    // ---- fixture (owner lane): a cash account and three payments.
    // pay_full — fully entangled: inflow movement + active allocation
    //            + active shareholder credit.
    // pay_blocked — same shape but its credit is CONSUMED by an active
    //            credit application: reversal must be rejected.
    let payer: Uuid = sqlx::query_scalar("SELECT payer_person_id FROM payments WHERE id = $1")
        .bind(rows2.payment)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let shareholder: Uuid =
        sqlx::query_scalar("SELECT shareholder_id FROM assessments WHERE id = $1")
            .bind(rows2.assessment)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let account: Uuid = sqlx::query_scalar(
        "INSERT INTO financial_accounts (name, account_type, created_by, cooperative_id) \
         VALUES ('R3CKasa','cash',$1,$2) RETURNING id",
    )
    .bind(p.user_a)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    #[allow(clippy::too_many_arguments)]
    async fn entangled_payment(
        db: &TestDb,
        coop: Uuid,
        user: Uuid,
        payer: Uuid,
        shareholder: Uuid,
        account: Uuid,
        assessment: Uuid,
        tag: &str,
        consumed: bool,
    ) -> Uuid {
        let payment: Uuid = sqlx::query_scalar(
            "INSERT INTO payments \
             (payer_person_id, amount, method, received_at, destination_account_id, \
              idempotency_key, idempotency_fingerprint, created_by, cooperative_id, \
              payment_number) \
             VALUES ($1,60,'cash',now(),$2,$3,$4,$5,$6, \
                     public.allocate_document_number($6,'payments.payment_number')) RETURNING id",
        )
        .bind(payer)
        .bind(account)
        .bind(format!("r3c-pay-{tag}"))
        .bind(format!("r3c-payfp-{tag}"))
        .bind(user)
        .bind(coop)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO account_movements \
             (account_id, direction, amount, source_type, source_id, occurred_at, \
              created_by, cooperative_id) \
             VALUES ($1,'inflow',60,'payment',$2,now(),$3,$4)",
        )
        .bind(account)
        .bind(payment)
        .bind(user)
        .bind(coop)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO payment_allocations \
             (payment_id, assessment_id, amount, created_by, cooperative_id) \
             VALUES ($1,$2,10,$3,$4)",
        )
        .bind(payment)
        .bind(assessment)
        .bind(user)
        .bind(coop)
        .execute(&db.pool)
        .await
        .unwrap();
        let credit: Uuid = sqlx::query_scalar(
            "INSERT INTO shareholder_credits \
             (source_payment_id, shareholder_id, amount, idempotency_key, \
              idempotency_fingerprint, created_by, cooperative_id, credit_number) \
             VALUES ($1,$2,15,$3,$4,$5,$6, \
                     public.allocate_document_number($6,'shareholder_credits.credit_number')) RETURNING id",
        )
        .bind(payment)
        .bind(shareholder)
        .bind(format!("r3c-cr-{tag}"))
        .bind(format!("r3c-crfp-{tag}"))
        .bind(user)
        .bind(coop)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        if consumed {
            sqlx::query(
                "INSERT INTO credit_applications \
                 (credit_id, assessment_id, amount, mode, idempotency_key, \
                  idempotency_fingerprint, created_by, cooperative_id) \
                 VALUES ($1,$2,5,'manual',$3,$4,$5,$6)",
            )
            .bind(credit)
            .bind(assessment)
            .bind(format!("r3c-ca-{tag}"))
            .bind(format!("r3c-cafp-{tag}"))
            .bind(user)
            .bind(coop)
            .execute(&db.pool)
            .await
            .unwrap();
        }
        payment
    }
    let pay_full = entangled_payment(
        &db,
        p.coop_a,
        p.user_a,
        payer,
        shareholder,
        account,
        rows2.assessment,
        "full",
        false,
    )
    .await;
    let pay_blocked = entangled_payment(
        &db,
        p.coop_a,
        p.user_a,
        payer,
        shareholder,
        account,
        rows2.assessment,
        "blocked",
        true,
    )
    .await;

    let app = db.app_pool().await;
    let token_c = vec![0x53u8; 32];

    // (1) Forged/absent context: an unsealed kooperatif_app connection
    // cannot drive the package at all.
    let mut conn = app.acquire().await.unwrap();
    let res = sqlx::query("SELECT public.app_payment_reverse_tx($1, now(), $2, 'x', $3)")
        .bind(pay_full)
        .bind(p.user_c)
        .bind(p.coop_a)
        .execute(&mut *conn)
        .await;
    drop(conn);
    assert!(res.is_err(), "unsealed connection must be refused");

    // (2) Sealed member WITHOUT payments.manage — the package itself
    // re-verifies the permission inside the database.
    let res = r3_probe_id(
        &app,
        &token_c,
        p.coop_a,
        "SELECT public.app_payment_reverse_tx($1, now(), gen_random_uuid(), 'x', NULL)",
        pay_full,
    )
    .await;
    assert!(
        r3_is_42501(&res),
        "package must deny a member without payments.manage"
    );

    // Grant the real permission, then prove the COMPONENT definers are
    // no longer invocable even by a fully authorized actor — EXECUTE
    // is gone, not merely the business-permission check.
    r3_grant_perms(
        &db,
        p.coop_a,
        p.user_c,
        &["payments.manage", "share_returns.manage"],
    )
    .await;
    for (label, sql, id) in [
        (
            "payment.reverse(component)",
            "SELECT public.app_payment_reverse($1, now(), $1, 'x', $1)",
            pay_full,
        ),
        (
            "movements.reverse(component)",
            "SELECT public.app_movements_reverse_of_source('payment', $1, now(), $1, 'x', $1)",
            pay_full,
        ),
        (
            "settlement.reverse(component)",
            "SELECT public.app_share_return_settlement_reverse($1, now(), $1, 'x', $1)",
            rows5.settlement,
        ),
    ] {
        let res = r3_probe_id(&app, &token_c, p.coop_a, sql, id).await;
        assert!(
            r3_is_42501(&res),
            "{label}: component definer must not be executable by the runtime role"
        );
    }

    // (3) Authorized complete reversal — ONE package call flips the
    // payment, the account movement, the allocation AND the credit.
    // p_by is deliberately fabricated (user_a): the seal wins.
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let (replayed, n_alloc): (bool, i64) = sqlx::query_as(
            "SELECT replayed, reversed_allocations \
             FROM public.app_payment_reverse_tx($1, now(), $2, 'r3c', $3)",
        )
        .bind(pay_full)
        .bind(p.user_a)
        .bind(p.coop_a)
        .fetch_one(&mut *tx)
        .await
        .expect("authorized package reversal");
        assert!(!replayed);
        assert_eq!(
            n_alloc, 1,
            "the active allocation must be reversed inside the package"
        );
        tx.commit().await.unwrap();
    }
    let (pay_status, rev_by): (String, Uuid) =
        sqlx::query_as("SELECT status, reversed_by FROM payments WHERE id = $1")
            .bind(pay_full)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(pay_status, "reversed");
    assert_eq!(
        rev_by, p.user_c,
        "attribution comes from the seal, not the caller"
    );
    let (mv_active, alloc_active, credit_active): (i64, i64, i64) = sqlx::query_as(
        "SELECT \
           (SELECT count(*) FROM account_movements \
             WHERE source_type='payment' AND source_id=$1 AND status='active'), \
           (SELECT count(*) FROM payment_allocations \
             WHERE payment_id=$1 AND status='active'), \
           (SELECT count(*) FROM shareholder_credits \
             WHERE source_payment_id=$1 AND status='active')",
    )
    .bind(pay_full)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        (mv_active, alloc_active, credit_active),
        (0, 0, 0),
        "every compensating effect must reverse atomically — no partial package"
    );

    // (4) Compensation failure rolls the WHOLE package back: the
    // consumed credit blocks the reversal; payment, movement and
    // credit stay untouched.
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let res =
            sqlx::query("SELECT public.app_payment_reverse_tx($1, now(), $2, 'r3c-blocked', $3)")
                .bind(pay_blocked)
                .bind(p.user_c)
                .bind(p.coop_a)
                .execute(&mut *tx)
                .await;
        tx.rollback().await.ok();
        assert!(
            matches!(res, Err(sqlx::Error::Database(ref e))
                if e.code().as_deref() == Some("P0001")
                   && e.message().contains("credit_effect_blocked")),
            "active credit application must block the reversal package"
        );
    }
    let (pay_status, mv_active, credit_active): (String, i64, i64) = sqlx::query_as(
        "SELECT \
           (SELECT status FROM payments WHERE id=$1), \
           (SELECT count(*) FROM account_movements \
             WHERE source_type='payment' AND source_id=$1 AND status='active'), \
           (SELECT count(*) FROM shareholder_credits \
             WHERE source_payment_id=$1 AND status='active')",
    )
    .bind(pay_blocked)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        (pay_status.as_str(), mv_active, credit_active),
        ("posted", 1, 1),
        "a rejected package must leave every effect untouched"
    );

    // (5) Concurrent reversals serialize on the payment row lock: two
    // sealed transactions racing the SAME fresh payment — exactly one
    // performs the reversal, the loser blocks on FOR UPDATE, then sees
    // the terminal state and replays. Never a double post.
    let pay_race = entangled_payment(
        &db,
        p.coop_a,
        p.user_a,
        payer,
        shareholder,
        account,
        rows2.assessment,
        "race",
        false,
    )
    .await;
    {
        let sql =
            "SELECT replayed FROM public.app_payment_reverse_tx($1, now(), $2, 'r3c-race', $3)";
        let app_race = app.clone();
        let token_race = token_c.clone();
        let handle = tokio::spawn(async move {
            let mut tx = r3_sealed(&app_race, &token_race, p.coop_a).await;
            let row: (bool,) = sqlx::query_as(sql)
                .bind(pay_race)
                .bind(p.user_c)
                .bind(p.coop_a)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            row.0
        });
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let ra: (bool,) = sqlx::query_as(sql)
            .bind(pay_race)
            .bind(p.user_c)
            .bind(p.coop_a)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let rb = handle.await.unwrap();
        assert_ne!(
            ra.0, rb,
            "exactly one racer reverses; the loser replays — no double post"
        );
    }
    let mv_reversed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_movements \
         WHERE source_type='payment' AND source_id=$1 AND status='reversed'",
    )
    .bind(pay_race)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(mv_reversed, 1, "the movement is reversed exactly once");

    // (6) Cross-cooperative: the coop-A actor cannot reach a coop-B
    // payment — the package raises not_found under the verified scope.
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let res = sqlx::query("SELECT public.app_payment_reverse_tx($1, now(), $2, 'xcoop', $3)")
            .bind(rows_b.payment)
            .bind(p.user_c)
            .bind(p.coop_b)
            .execute(&mut *tx)
            .await;
        tx.rollback().await.ok();
        assert!(
            matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("P0002")),
            "cross-cooperative payment must be invisible to the package"
        );
    }

    // (7) Settlement reversal package — the same contract on the
    // share-return lane: settlement + movement reverse atomically.
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let (replayed,): (bool,) = sqlx::query_as(
            "SELECT replayed FROM public.app_share_return_settlement_reverse_tx($1, now(), $2, 'r3c', $3)",
        )
        .bind(rows5.settlement)
        .bind(p.user_c)
        .bind(p.coop_a)
        .fetch_one(&mut *tx)
        .await
        .expect("authorized settlement reversal package");
        assert!(!replayed);
        tx.commit().await.unwrap();
    }
    let (st_status, st_mv): (String, i64) = sqlx::query_as(
        "SELECT \
           (SELECT status FROM share_return_settlements WHERE id=$1), \
           (SELECT count(*) FROM account_movements m \
             JOIN share_return_settlements s ON s.account_movement_id = m.id \
             WHERE s.id=$1 AND m.status='reversed')",
    )
    .bind(rows5.settlement)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(st_status, "reversed");
    assert_eq!(
        st_mv, 1,
        "the settlement movement must reverse with its row"
    );

    app.close().await;
    db.cleanup().await;
}

// ═══════════════════════════════════════════════════════════════════
// M1-P4-R4 — audit integrity + residual security closure.
//
//   (1) app_ctx_audit no longer mints arbitrary event types — the
//       whitelist holds only the two route-owned aggregates;
//   (2) every trigger-owned business mutation mints its audit row
//       INSIDE the sealed transaction — rollback erases both;
//   (3) attribution is seal-derived, never caller-supplied;
//   (4) the F6 enumeration tables are closed to the runtime role and
//       the verified definer readers still answer;
//   (5) the F8 INSERT write-guard denies a sealed member without the
//       module permission, even though the *_insert_live RLS policy
//       would pass;
//   (6) the F7 deferred source-integrity check rejects dangling and
//       cross-cooperative account_movements at COMMIT, while a
//       well-formed same-cooperative source still posts.
// ═══════════════════════════════════════════════════════════════════

/// Owner-lane payment + movement + allocation fixture for R4 probes.
async fn r4_payment_rows(db: &TestDb, coop: Uuid, user: Uuid, account: Uuid, tag: &str) -> Uuid {
    let person: Uuid = sqlx::query_scalar(
        "INSERT INTO persons (first_name, last_name, search_name, cooperative_id) \
         VALUES ($2,'T','t',$1) RETURNING id",
    )
    .bind(coop)
    .bind(format!("R4P{tag}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let payment: Uuid = sqlx::query_scalar(
        "INSERT INTO payments \
         (payer_person_id, amount, method, received_at, idempotency_key, \
          idempotency_fingerprint, created_by, cooperative_id, payment_number, \
          destination_account_id) \
         VALUES ($1,100,'cash',now(),$2,$3,$4,$5, \
                 public.allocate_document_number($5,'payments.payment_number'), $6) \
         RETURNING id",
    )
    .bind(person)
    .bind(format!("r4pay-{tag}"))
    .bind(format!("r4payfp-{tag}"))
    .bind(user)
    .bind(coop)
    .bind(account)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let movement: Uuid = sqlx::query_scalar(
        "INSERT INTO account_movements \
         (account_id, direction, amount, source_type, source_id, occurred_at, \
          created_by, cooperative_id) \
         VALUES ($1,'inflow',100,'payment',$2,now(),$3,$4) RETURNING id",
    )
    .bind(account)
    .bind(payment)
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let shareholder: Uuid = sqlx::query_scalar(
        "INSERT INTO shareholders (person_id, cooperative_id) \
         VALUES ($1,$2) RETURNING id",
    )
    .bind(person)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let period: Uuid = sqlx::query_scalar(
        "INSERT INTO periods (name, collection_start_date, due_date, cooperative_id, period_number) \
         VALUES ($2,'2026-04-01','2026-04-30',$1,$3) RETURNING id",
    )
    .bind(coop)
    .bind(format!("R4Per{tag}"))
    .bind(200 + tag.bytes().map(|b| b as i32).sum::<i32>())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let rule: Uuid = sqlx::query_scalar(
        "INSERT INTO assessment_rules \
         (period_id, rule_type, base_amount, assessment_effective_date, cooperative_id) \
         VALUES ($1,'per_share',10,'2026-04-01',$2) RETURNING id",
    )
    .bind(period)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let assessment: Uuid = sqlx::query_scalar(
        "INSERT INTO assessments \
         (period_id, shareholder_id, rule_type, base_amount, amount, currency, \
          assessment_effective_date, generated_by, cooperative_id) \
         VALUES ($1,$2,'per_share',10,10,'TRY','2026-04-01',$3,$4) RETURNING id",
    )
    .bind(period)
    .bind(shareholder)
    .bind(user)
    .bind(coop)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let _ = rule;
    sqlx::query(
        "INSERT INTO payment_allocations \
         (payment_id, assessment_id, amount, status, created_by, cooperative_id) \
         VALUES ($1, $2, 40, 'active', $3, $4)",
    )
    .bind(payment)
    .bind(assessment)
    .bind(user)
    .bind(coop)
    .execute(&db.pool)
    .await
    .unwrap();
    let _ = movement;
    payment
}

#[tokio::test]
async fn r4_audit_integrity_and_residual_closure() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let app = db.app_pool().await;
    let now = OffsetDateTime::now_utc();
    let token_c: Vec<u8> = vec![0x53u8; 32];

    // ── (1) F5: sealed callers cannot mint privileged event types ──
    for forged in ["payment_posted", "role_created", "user_disabled"] {
        let res = sqlx::query("SELECT public.app_ctx_audit($1, '{}'::jsonb)")
            .bind(forged)
            .execute(&mut *r3_sealed(&app, &token_c, p.coop_a).await)
            .await;
        assert!(r3_is_42501(&res), "sealed forge of {forged} must be denied");
    }
    // …while the route-owned aggregate stays mintable and carries
    // seal-derived attribution only.
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        sqlx::query("SELECT public.app_ctx_audit('assessments_generated', $1)")
            .bind(serde_json::json!({"probe": "r4-ctx"}))
            .execute(&mut *tx)
            .await
            .expect("whitelisted type must mint");
        tx.commit().await.unwrap();
    }
    let attr: (Uuid, Uuid, Uuid) = sqlx::query_as(
        "SELECT cooperative_id, user_id, session_id FROM security_events \
          WHERE event_type = 'assessments_generated' \
            AND metadata->>'probe' = 'r4-ctx'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(attr.0, p.coop_a);
    assert_eq!(attr.1, p.user_c, "actor must be the sealed user");

    // ── (2) package-internal mandatory audit, seal-derived ──
    let account_a: Uuid = sqlx::query_scalar(
        "INSERT INTO financial_accounts (name, account_type, created_by, cooperative_id) \
         VALUES ('R4Kasa','cash',$1,$2) RETURNING id",
    )
    .bind(p.user_a)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let payment_a = r4_payment_rows(&db, p.coop_a, p.user_a, account_a, "a").await;
    r3_grant_perms(
        &db,
        p.coop_a,
        p.user_c,
        &[
            "payments.manage",
            "shareholders.manage",
            "financial_accounts.manage",
        ],
    )
    .await;

    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        sqlx::query("SELECT public.app_payment_reverse_tx($1, $2, $3, 'r4 reverse', $4)")
            .bind(payment_a)
            .bind(now)
            .bind(p.user_a) // forged actor — must be overwritten by the seal
            .bind(p.coop_a)
            .execute(&mut *tx)
            .await
            .expect("package call by authorized member");
        tx.commit().await.unwrap();
    }
    let rev: (i64, Uuid, Uuid) = sqlx::query_as(
        "SELECT count(*) OVER (), cooperative_id, user_id FROM security_events \
          WHERE event_type = 'payment_reversed' \
            AND metadata->>'payment_id' = $1::text LIMIT 1",
    )
    .bind(payment_a.to_string())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(rev.0, 1, "exactly one payment_reversed — no duplicate");
    assert_eq!(rev.1, p.coop_a, "cooperative attribution from the seal");
    assert_eq!(rev.2, p.user_c, "actor attribution from the seal");
    let alloc_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events \
          WHERE event_type = 'payment_allocation_reversed' \
            AND metadata->>'payment_id' = $1::text",
    )
    .bind(payment_a.to_string())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(alloc_events, 1, "the component reversal minted its own row");

    // ── (3) audit is atomic with the mutation ──
    // A sealed tx that mutates then fails must leave neither the
    // business change nor the audit row.
    let sh_a: Uuid = sqlx::query_scalar(
        "INSERT INTO persons (first_name, last_name, search_name, cooperative_id) \
         VALUES ('R4S','T','t',$1) RETURNING id",
    )
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let shareholder_a: Uuid = sqlx::query_scalar(
        "INSERT INTO shareholders (person_id, status, cooperative_id) \
         VALUES ($1,'active',$2) RETURNING id",
    )
    .bind(sh_a)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        sqlx::query("SELECT public.app_shareholder_set_status_tx($1,'inactive','r4',$2,$3)")
            .bind(shareholder_a)
            .bind(p.user_c)
            .bind(p.coop_a)
            .execute(&mut *tx)
            .await
            .expect("status mutation");
        // Force the transaction to fail AFTER the minted event.
        let doomed = sqlx::query("SELECT 1/0").execute(&mut *tx).await;
        assert!(doomed.is_err());
        tx.rollback().await.ok();
    }
    let leaked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events \
          WHERE event_type = 'shareholder_status_changed' \
            AND metadata->>'shareholder_id' = $1::text",
    )
    .bind(shareholder_a.to_string())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(leaked, 0, "rolled-back mutation leaves no audit row");
    let status: String = sqlx::query_scalar("SELECT status FROM shareholders WHERE id = $1")
        .bind(shareholder_a)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(status, "active", "the mutation itself rolled back");

    // ── (4) F6: enumeration surface closed, readers still answer ──
    for table in [
        "cooperatives",
        "installation_state",
        "permissions",
        "role_permissions",
    ] {
        let res = sqlx::query(&format!("SELECT count(*) FROM {table}"))
            .execute(&app)
            .await;
        assert!(r3_is_42501(&res), "{table} must be unreadable by app role");
    }
    let catalog: i64 = sqlx::query_scalar("SELECT count(*) FROM public.app_permission_catalog()")
        .fetch_one(&app)
        .await
        .expect("catalog reader must answer");
    assert!(catalog > 0);
    sqlx::query("SELECT cooperative_id FROM public.app_resolve_bootstrap()")
        .fetch_optional(&app)
        .await
        .expect("bootstrap resolution must still answer");

    // ── (5) F8: INSERT write-guard on the runtime lane ──
    // user_c holds payments/shareholders/financial_accounts.manage but
    // NOT periods.manage: a sealed INSERT into periods must fail even
    // though the live-status RLS policy would have allowed it.
    {
        let res = sqlx::query(
            "INSERT INTO periods (name, status, collection_start_date, due_date,               cooperative_id, period_number)              VALUES ('forged','draft','2026-01-01','2026-01-31',$1,99)",
        )
        .bind(p.coop_a)
        .execute(&mut *r3_sealed(&app, &token_c, p.coop_a).await)
        .await;
        assert!(r3_is_42501(&res), "unauthorized INSERT must be denied");
    }
    // …and the same actor CAN insert a row its permissions cover.
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        sqlx::query(
            "INSERT INTO financial_accounts (name, account_type, created_by, cooperative_id)              VALUES ('R4Allowed','cash',$1,$2)",
        )
        .bind(p.user_c)
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await
        .expect("authorized insert must pass the guard");
        tx.commit().await.unwrap();
    }

    // ── (6) F7: deferred source-integrity on account_movements ──
    // Cross-cooperative source: the movement claims coop_a but its
    // source_id is coop_b's payment → rejected at COMMIT.
    let payment_b = {
        let account_b: Uuid = sqlx::query_scalar(
            "INSERT INTO financial_accounts (name, account_type, created_by, cooperative_id)              VALUES ('R4KasaB','cash',$1,$2) RETURNING id",
        )
        .bind(p.user_c)
        .bind(p.coop_b)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        r4_payment_rows(&db, p.coop_b, p.user_c, account_b, "b").await
    };
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        sqlx::query(
            "INSERT INTO account_movements              (account_id, direction, amount, source_type, source_id, occurred_at,               created_by, cooperative_id)              VALUES ($1,'inflow',100,'payment',$2,now(),$3,$4)",
        )
        .bind(account_a)
        .bind(payment_b)
        .bind(p.user_c)
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await
        .expect("the row itself inserts — the check is deferred");
        let committed = tx.commit().await;
        assert!(committed.is_err(), "cross-coop source must fail at commit");
    }
    // Dangling source: no such payment anywhere → rejected at COMMIT.
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        sqlx::query(
            "INSERT INTO account_movements              (account_id, direction, amount, source_type, source_id, occurred_at,               created_by, cooperative_id)              VALUES ($1,'inflow',100,'payment',$2,now(),$3,$4)",
        )
        .bind(account_a)
        .bind(Uuid::new_v4())
        .bind(p.user_c)
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await
        .expect("insert is deferred-checked");
        assert!(tx.commit().await.is_err(), "dangling source must fail");
    }
    // Legitimate same-cooperative source → commits, and created_by is
    // pinned to the sealed actor (a caller-supplied creator is ignored).
    {
        // A payment that declares account_a as its destination but has
        // no movement yet — the sealed insert supplies that leg.
        let payer: Uuid = sqlx::query_scalar(
            "INSERT INTO persons (first_name, last_name, search_name, cooperative_id) \
             VALUES ('R4OK','T','t',$1) RETURNING id",
        )
        .bind(p.coop_a)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let pay_ok: Uuid = sqlx::query_scalar(
            "INSERT INTO payments \
             (payer_person_id, amount, method, received_at, idempotency_key, \
              idempotency_fingerprint, created_by, cooperative_id, payment_number, \
              destination_account_id) \
             VALUES ($1,100,'cash',now(),'r4ok','r4okfp',$2,$3, \
                     public.allocate_document_number($3,'payments.payment_number'), $4) \
             RETURNING id",
        )
        .bind(payer)
        .bind(p.user_a)
        .bind(p.coop_a)
        .bind(account_a)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let movement_id: Uuid = sqlx::query_scalar(
            "INSERT INTO account_movements              (account_id, direction, amount, source_type, source_id, occurred_at,               created_by, cooperative_id)              VALUES ($1,'inflow',100,'payment',$2,now(),$3,$4) RETURNING id",
        )
        .bind(account_a)
        .bind(pay_ok)
        .bind(p.user_a) // forged creator — overwritten by the pre-trigger
        .bind(p.coop_a)
        .fetch_one(&mut *tx)
        .await
        .expect("valid same-coop source must pass");
        tx.commit().await.unwrap();
        let created_by: Uuid =
            sqlx::query_scalar("SELECT created_by FROM account_movements WHERE id = $1")
                .bind(movement_id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(created_by, p.user_c, "created_by is seal-pinned");
    }

    app.close().await;
    db.cleanup().await;
}

// ======================================================================
// M1-P4-B1 — payment creation integrity (migration 0058)
//
// The independent closure audit proved a sealed `payments.manage`
// actor could fabricate a `posted` Payment + its account movement
// through raw SQL — cash on the books with none of the command's
// atomic semantics (document numbering, actor pinning, pair binding,
// disposition bounds). 0058 binds those invariants at the database
// boundary on the restricted lane:
//
//   * `created_by` is seal-pinned; rows are born in their single
//     legitimate status; reversal fields are cleared at birth;
//   * `payment_number`/`credit_number` are server-assigned — a
//     caller-supplied value is a 42501;
//   * a posted Payment requires EXACTLY ONE matching active inflow
//     movement on its destination account, same transaction
//     (INITIALLY DEFERRED constraint trigger);
//   * allocations + source credits can never exceed the Payment;
//     settlements can never exceed the Assessment; applications can
//     never exceed the Credit nor settle another shareholder's debt.
//
// These probes run as the REAL `kooperatif_app` principal inside
// sealed request transactions — plus the legitimate repository
// command path driving the same sealed lane end to end.
// ======================================================================

struct B1Rows {
    account_a: Uuid,
    account_b: Uuid,
    account_foreign: Uuid,
    payer: Uuid,
    shareholder_a: Uuid,
    shareholder_b: Uuid,
    period: Uuid,
}

async fn b1_rows(db: &TestDb, p: &Pilot) -> B1Rows {
    let account_a: Uuid = sqlx::query_scalar(
        "INSERT INTO financial_accounts (name, account_type, created_by, cooperative_id) \
         VALUES ('B1KasaA','cash',$1,$2) RETURNING id",
    )
    .bind(p.user_c)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let account_b: Uuid = sqlx::query_scalar(
        "INSERT INTO financial_accounts (name, account_type, created_by, cooperative_id) \
         VALUES ('B1KasaB','cash',$1,$2) RETURNING id",
    )
    .bind(p.user_c)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let account_foreign: Uuid = sqlx::query_scalar(
        "INSERT INTO financial_accounts (name, account_type, created_by, cooperative_id) \
         VALUES ('B1KasaForeign','cash',$1,$2) RETURNING id",
    )
    .bind(p.user_c)
    .bind(p.coop_b)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let payer: Uuid = sqlx::query_scalar(
        "INSERT INTO persons (first_name, last_name, search_name, cooperative_id) \
         VALUES ('B1','Payer','payer',$1) RETURNING id",
    )
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let mut shareholders = Vec::new();
    for (first, last) in [("B1ShA", "a"), ("B1ShB", "b")] {
        let person: Uuid = sqlx::query_scalar(
            "INSERT INTO persons (first_name, last_name, search_name, cooperative_id) \
             VALUES ($1,'T',$2,$3) RETURNING id",
        )
        .bind(first)
        .bind(last)
        .bind(p.coop_a)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let sh: Uuid = sqlx::query_scalar(
            "INSERT INTO shareholders (person_id, status, cooperative_id) \
             VALUES ($1,'active',$2) RETURNING id",
        )
        .bind(person)
        .bind(p.coop_a)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        shareholders.push(sh);
    }
    let period: Uuid = sqlx::query_scalar(
        "INSERT INTO periods (name, collection_start_date, due_date, cooperative_id, period_number) \
         VALUES ('B1Per','2026-05-01','2026-05-31',$1,900) RETURNING id",
    )
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    B1Rows {
        account_a,
        account_b,
        account_foreign,
        payer,
        shareholder_a: shareholders[0],
        shareholder_b: shareholders[1],
        period,
    }
}

async fn b1_assessment(
    db: &TestDb,
    p: &Pilot,
    rows: &B1Rows,
    shareholder: Uuid,
    amount: i64,
) -> Uuid {
    // (period, shareholder) is unique — every obligation needs its
    // own period row.
    let period: Uuid = sqlx::query_scalar(
        "INSERT INTO periods (name, collection_start_date, due_date, cooperative_id, period_number) \
         VALUES ($2,'2026-05-01','2026-05-31',$1, \
                 (SELECT COALESCE(max(period_number),0)+1 FROM periods \
                   WHERE cooperative_id = $1)) RETURNING id",
    )
    .bind(p.coop_a)
    .bind(format!("B1Per-{}", &Uuid::new_v4().simple().to_string()[..8]))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let _ = rows.period;
    sqlx::query_scalar(
        "INSERT INTO assessments \
         (period_id, shareholder_id, rule_type, base_amount, amount, currency, \
          assessment_effective_date, generated_by, cooperative_id) \
         VALUES ($1,$2,'per_share',$3,$3,'TRY','2026-05-01',$4,$5) RETURNING id",
    )
    .bind(period)
    .bind(shareholder)
    .bind(amount)
    .bind(p.user_c)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

/// Raw payment insert inside a sealed tx — the number column is
/// omitted so `payments_number_pin` assigns it. Returns the id.
async fn b1_raw_payment(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payer: Uuid,
    user: Uuid,
    coop: Uuid,
    account: Uuid,
    amount: i64,
    key: &str,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO payments \
         (payer_person_id, amount, method, received_at, idempotency_key, \
          idempotency_fingerprint, created_by, cooperative_id, \
          destination_account_id) \
         VALUES ($1,$5,'cash',now(),$6,$7,$2,$3,$4) RETURNING id",
    )
    .bind(payer)
    .bind(user)
    .bind(coop)
    .bind(account)
    .bind(amount)
    .bind(key)
    .bind(format!("{key}fp"))
    .fetch_one(&mut **tx)
    .await
    .expect("raw payment row inserts")
}

async fn b1_raw_movement(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account: Uuid,
    payment: Uuid,
    user: Uuid,
    coop: Uuid,
    amount: i64,
) {
    sqlx::query(
        "INSERT INTO account_movements \
         (account_id, direction, amount, source_type, source_id, occurred_at, \
          created_by, cooperative_id) \
         VALUES ($1,'inflow',$5,'payment',$2,now(),$3,$4)",
    )
    .bind(account)
    .bind(payment)
    .bind(user)
    .bind(coop)
    .bind(amount)
    .execute(&mut **tx)
    .await
    .expect("raw movement row inserts");
}

fn b1_err_contains(err: &sqlx::Error, marker: &str) -> bool {
    format!("{err:?}").contains(marker)
}

#[tokio::test]
async fn b1_payment_creation_integrity() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    r3_grant_perms(
        &db,
        p.coop_a,
        p.user_c,
        &["payments.manage", "credits.manage"],
    )
    .await;
    let rows = b1_rows(&db, &p).await;
    let app = db.app_pool().await;
    let token_c: Vec<u8> = vec![0x53u8; 32];
    let now = OffsetDateTime::now_utc();

    // ── (1) caller-supplied document numbers are refused ──
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let res = sqlx::query(
            "INSERT INTO payments \
             (payer_person_id, amount, method, received_at, idempotency_key, \
              idempotency_fingerprint, created_by, cooperative_id, \
              payment_number, destination_account_id) \
             VALUES ($1,10,'cash',now(),'b1-num','b1-numfp',$2,$3,77,$4)",
        )
        .bind(rows.payer)
        .bind(p.user_c)
        .bind(p.coop_a)
        .bind(rows.account_a)
        .execute(&mut *tx)
        .await;
        assert!(
            r3_is_42501(&res),
            "caller payment_number must be refused: {res:?}"
        );
        tx.rollback().await.ok();
    }
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let res = sqlx::query(
            "INSERT INTO shareholder_credits \
             (source_payment_id, shareholder_id, amount, credit_number, \
              created_by, cooperative_id) \
             VALUES ($1,$2,10,77,$3,$4)",
        )
        .bind(Uuid::new_v4())
        .bind(rows.shareholder_a)
        .bind(p.user_c)
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await;
        assert!(
            r3_is_42501(&res),
            "caller credit_number must be refused: {res:?}"
        );
        tx.rollback().await.ok();
    }

    // ── (2) rows are born in their legitimate status only ──
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let res = sqlx::query(
            "INSERT INTO payments \
             (payer_person_id, amount, method, received_at, idempotency_key, \
              idempotency_fingerprint, created_by, cooperative_id, status, \
              reversed_at, reversed_by, reversal_reason, destination_account_id) \
             VALUES ($1,10,'cash',now(),'b1-dead','b1-deadfp',$2,$3,'reversed', \
                     now(), $2, 'born dead', $4)",
        )
        .bind(rows.payer)
        .bind(p.user_c)
        .bind(p.coop_a)
        .bind(rows.account_a)
        .execute(&mut *tx)
        .await;
        assert!(
            r3_is_42501(&res),
            "born-reversed payment must be refused: {res:?}"
        );
        tx.rollback().await.ok();
    }

    // ── (3) forged attribution is pinned to the sealed actor ──
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let payment = b1_raw_payment(
            &mut tx,
            rows.payer,
            p.user_a,
            p.coop_a,
            rows.account_a,
            10,
            "b1-pin",
        )
        .await;
        b1_raw_movement(&mut tx, rows.account_a, payment, p.user_a, p.coop_a, 10).await;
        tx.commit().await.expect("well-formed pair commits");
        let (pay_by, mov_by): (Uuid, Uuid) = sqlx::query_as(
            "SELECT p.created_by, m.created_by FROM payments p \
             JOIN account_movements m ON m.source_type='payment' AND m.source_id=p.id \
             WHERE p.id = $1",
        )
        .bind(payment)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(pay_by, p.user_c, "payment creator is seal-pinned");
        assert_eq!(mov_by, p.user_c, "movement creator is seal-pinned");
    }

    // ── (4) the B1 exploit: a posted payment without its movement ──
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let payment = b1_raw_payment(
            &mut tx,
            rows.payer,
            p.user_c,
            p.coop_a,
            rows.account_a,
            5000,
            "b1-solo",
        )
        .await;
        let err = tx
            .commit()
            .await
            .expect_err("an unpaired posted payment must fail at commit");
        assert!(
            b1_err_contains(&err, "payment_movement_pair"),
            "expected pair-binding error, got {err:?}"
        );
        let exists: i64 = sqlx::query_scalar("SELECT count(*) FROM payments WHERE id = $1")
            .bind(payment)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(exists, 0, "the aborted payment left no row");
    }

    // ── (5) the leg must match the destination account and amount ──
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let payment = b1_raw_payment(
            &mut tx,
            rows.payer,
            p.user_c,
            p.coop_a,
            rows.account_a,
            100,
            "b1-xacct",
        )
        .await;
        // The leg lands on the WRONG coop_a account — the deferred
        // source check of 0057 (destination mismatch) plus the pair
        // counter of 0058 both refuse at commit.
        b1_raw_movement(&mut tx, rows.account_b, payment, p.user_c, p.coop_a, 100).await;
        let err = tx
            .commit()
            .await
            .expect_err("mismatched destination must fail at commit");
        assert!(
            b1_err_contains(&err, "kooperatif:"),
            "expected domain rejection, got {err:?}"
        );
    }
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let payment = b1_raw_payment(
            &mut tx,
            rows.payer,
            p.user_c,
            p.coop_a,
            rows.account_a,
            100,
            "b1-xamt",
        )
        .await;
        b1_raw_movement(&mut tx, rows.account_a, payment, p.user_c, p.coop_a, 55).await;
        let err = tx
            .commit()
            .await
            .expect_err("mismatched amount must fail at commit");
        assert!(b1_err_contains(&err, "kooperatif:"), "got {err:?}");
    }
    // Cross-cooperative leg — the composite FK on
    // (cooperative_id, account_id) refuses at INSERT itself.
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let payment = b1_raw_payment(
            &mut tx,
            rows.payer,
            p.user_c,
            p.coop_a,
            rows.account_a,
            100,
            "b1-xcoop",
        )
        .await;
        let res = sqlx::query(
            "INSERT INTO account_movements \
             (account_id, direction, amount, source_type, source_id, occurred_at, \
              created_by, cooperative_id) \
             VALUES ($1,'inflow',100,'payment',$2,now(),$3,$4)",
        )
        .bind(rows.account_foreign)
        .bind(payment)
        .bind(p.user_c)
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await;
        assert!(res.is_err(), "cross-coop movement leg must fail: {res:?}");
        tx.rollback().await.ok();
    }

    // ── (6) a well-formed raw package still commits — the boundary
    //        is invariant-based: allocation + matching movement in the
    //        same transaction is exactly what the command produces.
    let asm_full = b1_assessment(&db, &p, &rows, rows.shareholder_a, 10).await;
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let payment = b1_raw_payment(
            &mut tx,
            rows.payer,
            p.user_c,
            p.coop_a,
            rows.account_a,
            10,
            "b1-ok",
        )
        .await;
        sqlx::query(
            "INSERT INTO payment_allocations \
             (payment_id, assessment_id, amount, created_by, cooperative_id) \
             VALUES ($1,$2,10,$3,$4)",
        )
        .bind(payment)
        .bind(asm_full)
        .bind(p.user_c)
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await
        .expect("allocation inserts");
        b1_raw_movement(&mut tx, rows.account_a, payment, p.user_c, p.coop_a, 10).await;
        tx.commit().await.expect("a well-formed package commits");
    }

    // ── (7) legitimate repository command path on the sealed lane ──
    // Zero allocations: the whole amount stays unallocated on the
    // Payment — a supported state, never auto-attributed.
    let unapplied = {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let outcome = kooperatif_server::payments::repo::create_payment(
            &mut tx,
            Some(p.coop_a),
            p.user_c,
            kooperatif_server::payments::repo::CreatePayment {
                payer_person_id: Some(rows.payer),
                payer_first_name: None,
                payer_last_name: None,
                amount: rust_decimal::Decimal::new(10000, 2),
                method: "cash".into(),
                received_at: now,
                note: None,
                destination_account_id: rows.account_a,
                idempotency_key: "b1-repo-unapplied".into(),
                fingerprint: "fp-unapplied".into(),
                allocations: vec![],
            },
        )
        .await
        .expect("unapplied payment via repo on sealed lane");
        tx.commit().await.expect("sealed package commits");
        outcome
    };
    assert!(!unapplied.replayed);
    assert!(unapplied.payment_number > 0, "server-assigned number");
    assert_eq!(
        unapplied.unallocated_amount,
        rust_decimal::Decimal::new(10000, 2),
        "zero allocations — the whole amount awaits disposition"
    );

    // Partial allocation — the remainder stays on the Payment, and a
    // caller-selected physical cashbox (account_b) is honoured.
    let asm_partial = b1_assessment(&db, &p, &rows, rows.shareholder_a, 50).await;
    let partial = {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let outcome = kooperatif_server::payments::repo::create_payment(
            &mut tx,
            Some(p.coop_a),
            p.user_c,
            kooperatif_server::payments::repo::CreatePayment {
                payer_person_id: Some(rows.payer),
                payer_first_name: None,
                payer_last_name: None,
                amount: rust_decimal::Decimal::new(10000, 2),
                method: "bank_transfer".into(),
                received_at: now,
                note: None,
                destination_account_id: rows.account_b,
                idempotency_key: "b1-repo-partial".into(),
                fingerprint: "fp-partial".into(),
                allocations: vec![kooperatif_server::payments::repo::NewAllocation {
                    assessment_id: asm_partial,
                    amount: rust_decimal::Decimal::new(3000, 2),
                }],
            },
        )
        .await
        .expect("partial allocation via repo");
        tx.commit().await.expect("sealed package commits");
        outcome
    };
    assert_eq!(
        partial.allocated_amount,
        rust_decimal::Decimal::new(3000, 2)
    );
    assert_eq!(
        partial.unallocated_amount,
        rust_decimal::Decimal::new(7000, 2)
    );

    // Family bulk collection — one payer settling obligations of TWO
    // different shareholders in a single command (docs/06).
    let asm_family_a = b1_assessment(&db, &p, &rows, rows.shareholder_a, 10).await;
    let asm_family_b = b1_assessment(&db, &p, &rows, rows.shareholder_b, 10).await;
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let outcome = kooperatif_server::payments::repo::create_payment(
            &mut tx,
            Some(p.coop_a),
            p.user_c,
            kooperatif_server::payments::repo::CreatePayment {
                payer_person_id: Some(rows.payer),
                payer_first_name: None,
                payer_last_name: None,
                amount: rust_decimal::Decimal::new(2000, 2),
                method: "cash".into(),
                received_at: now,
                note: None,
                destination_account_id: rows.account_a,
                idempotency_key: "b1-repo-family".into(),
                fingerprint: "fp-family".into(),
                allocations: vec![
                    kooperatif_server::payments::repo::NewAllocation {
                        assessment_id: asm_family_a,
                        amount: rust_decimal::Decimal::new(1000, 2),
                    },
                    kooperatif_server::payments::repo::NewAllocation {
                        assessment_id: asm_family_b,
                        amount: rust_decimal::Decimal::new(1000, 2),
                    },
                ],
            },
        )
        .await
        .expect("family bulk payment via repo");
        tx.commit().await.expect("sealed package commits");
        assert_eq!(outcome.allocation_count, 2);
        assert_eq!(outcome.unallocated_amount, rust_decimal::Decimal::ZERO);
    }

    // Overpayment remainder -> shareholder credit via the real
    // command: the credit gets a server-assigned number and the FIFO
    // sweep auto-applies it to the beneficiary's open assessments.
    let credit_outcome = {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let outcome = kooperatif_server::credits::repo::assign_credit(
            &mut tx,
            Some(p.coop_a),
            p.user_c,
            kooperatif_server::credits::repo::AssignCredit {
                payment_id: unapplied.payment_id,
                shareholder_id: rows.shareholder_a,
                amount: rust_decimal::Decimal::new(4000, 2),
                note: None,
                idempotency_key: "b1-credit-1".into(),
                fingerprint: "fp-credit-1".into(),
            },
            now,
        )
        .await
        .expect("credit assignment via repo on sealed lane");
        tx.commit().await.expect("credit package commits");
        outcome
    };
    assert!(!credit_outcome.replayed);
    assert!(credit_outcome.credit_number > 0, "server-assigned number");
    assert!(credit_outcome.applied_amount > rust_decimal::Decimal::ZERO);

    // ── (8) disposition bounds — raw SQL can never over-dispose ──
    // Payment remainder is 100 - 40(credit) = 60; allocating 70 must
    // fail the deferred disposition check.
    let asm_over = b1_assessment(&db, &p, &rows, rows.shareholder_b, 100).await;
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        sqlx::query(
            "INSERT INTO payment_allocations \
             (payment_id, assessment_id, amount, created_by, cooperative_id) \
             VALUES ($1,$2,70,$3,$4)",
        )
        .bind(unapplied.payment_id)
        .bind(asm_over)
        .bind(p.user_c)
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await
        .expect("row inserts — the bound check is deferred");
        let err = tx
            .commit()
            .await
            .expect_err("over-disposed allocation must fail at commit");
        assert!(
            b1_err_contains(&err, "payment_over_disposed"),
            "expected over-disposed rejection, got {err:?}"
        );
    }
    // Settling more than the assessment owes is refused even when the
    // payment could fund it.
    let asm_small = b1_assessment(&db, &p, &rows, rows.shareholder_b, 10).await;
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        sqlx::query(
            "INSERT INTO payment_allocations \
             (payment_id, assessment_id, amount, created_by, cooperative_id) \
             VALUES ($1,$2,20,$3,$4)",
        )
        .bind(unapplied.payment_id)
        .bind(asm_small)
        .bind(p.user_c)
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await
        .expect("row inserts — deferred");
        let err = tx
            .commit()
            .await
            .expect_err("over-settled assessment must fail at commit");
        assert!(
            b1_err_contains(&err, "assessment_over_settled"),
            "expected over-settled rejection, got {err:?}"
        );
    }
    // A raw credit exceeding the remainder is refused.
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        sqlx::query(
            "INSERT INTO shareholder_credits \
             (source_payment_id, shareholder_id, amount, created_by, cooperative_id, \
              idempotency_key, idempotency_fingerprint) \
             VALUES ($1,$2,61,$3,$4,'b1-overcredit','b1-overcreditfp')",
        )
        .bind(unapplied.payment_id)
        .bind(rows.shareholder_a)
        .bind(p.user_c)
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await
        .expect("row inserts — deferred");
        let err = tx
            .commit()
            .await
            .expect_err("over-funded credit must fail at commit");
        assert!(
            b1_err_contains(&err, "payment_over_disposed"),
            "expected over-disposed rejection, got {err:?}"
        );
    }
    // A credit settles only ITS OWN shareholder's debt.
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        sqlx::query(
            "INSERT INTO credit_applications \
             (credit_id, assessment_id, amount, mode, created_by, cooperative_id) \
             VALUES ($1,$2,5,'manual',$3,$4)",
        )
        .bind(credit_outcome.credit_id)
        .bind(asm_family_b) // shareholder_b's assessment
        .bind(p.user_c)
        .bind(p.coop_a)
        .execute(&mut *tx)
        .await
        .expect("row inserts — deferred");
        let err = tx
            .commit()
            .await
            .expect_err("foreign-debt application must fail at commit");
        assert!(
            b1_err_contains(&err, "application_owner_mismatch"),
            "expected owner-mismatch rejection, got {err:?}"
        );
    }
    // Applications can never exceed the credit that funds them.
    {
        let credit_amount: rust_decimal::Decimal =
            sqlx::query_scalar("SELECT amount FROM shareholder_credits WHERE id = $1")
                .bind(credit_outcome.credit_id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        let applied: rust_decimal::Decimal = sqlx::query_scalar(
            "SELECT COALESCE(sum(amount),0) FROM credit_applications \
             WHERE credit_id = $1 AND status = 'active'",
        )
        .bind(credit_outcome.credit_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let asm_extra = b1_assessment(&db, &p, &rows, rows.shareholder_a, 500).await;
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        sqlx::query(
            "INSERT INTO credit_applications \
             (credit_id, assessment_id, amount, mode, created_by, cooperative_id) \
             VALUES ($1,$2,$5::numeric,'manual',$3,$4)",
        )
        .bind(credit_outcome.credit_id)
        .bind(asm_extra)
        .bind(p.user_c)
        .bind(p.coop_a)
        .bind((credit_amount - applied + rust_decimal::Decimal::ONE).to_string())
        .execute(&mut *tx)
        .await
        .expect("row inserts — deferred");
        let err = tx
            .commit()
            .await
            .expect_err("over-applied credit must fail at commit");
        assert!(
            b1_err_contains(&err, "credit_over_applied"),
            "expected over-applied rejection, got {err:?}"
        );
    }

    // ── (9) idempotency — replay identical, conflict on divergence ──
    let replay = {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let outcome = kooperatif_server::payments::repo::create_payment(
            &mut tx,
            Some(p.coop_a),
            p.user_c,
            kooperatif_server::payments::repo::CreatePayment {
                payer_person_id: Some(rows.payer),
                payer_first_name: None,
                payer_last_name: None,
                amount: rust_decimal::Decimal::new(10000, 2),
                method: "cash".into(),
                received_at: now,
                note: None,
                destination_account_id: rows.account_a,
                idempotency_key: "b1-repo-unapplied".into(),
                fingerprint: "fp-unapplied".into(),
                allocations: vec![],
            },
        )
        .await
        .expect("identical replay must succeed");
        tx.commit().await.expect("replay tx commits");
        outcome
    };
    assert!(replay.replayed, "same key + fingerprint replays");
    assert_eq!(replay.payment_id, unapplied.payment_id);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payments WHERE idempotency_key = 'b1-repo-unapplied'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "no double posting on replay");
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let res = kooperatif_server::payments::repo::create_payment(
            &mut tx,
            Some(p.coop_a),
            p.user_c,
            kooperatif_server::payments::repo::CreatePayment {
                payer_person_id: Some(rows.payer),
                payer_first_name: None,
                payer_last_name: None,
                amount: rust_decimal::Decimal::new(9900, 2),
                method: "cash".into(),
                received_at: now,
                note: None,
                destination_account_id: rows.account_a,
                idempotency_key: "b1-repo-unapplied".into(),
                fingerprint: "fp-DIVERGENT".into(),
                allocations: vec![],
            },
        )
        .await;
        assert!(
            matches!(
                res,
                Err(kooperatif_server::payments::repo::PaymentCommandError::IdempotencyConflict)
            ),
            "divergent fingerprint must conflict: {:?}",
            res.err()
        );
        tx.rollback().await.ok();
    }

    // ── (10) concurrent same-key submissions — exactly one posts ──
    {
        let race_cmd = || kooperatif_server::payments::repo::CreatePayment {
            payer_person_id: Some(rows.payer),
            payer_first_name: None,
            payer_last_name: None,
            amount: rust_decimal::Decimal::new(500, 2),
            method: "cash".into(),
            received_at: now,
            note: None,
            destination_account_id: rows.account_a,
            idempotency_key: "b1-race".into(),
            fingerprint: "fp-race".into(),
            allocations: vec![],
        };
        let (r1, r2) = tokio::join!(
            async {
                let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
                let out = kooperatif_server::payments::repo::create_payment(
                    &mut tx,
                    Some(p.coop_a),
                    p.user_c,
                    race_cmd(),
                )
                .await;
                let committed = tx.commit().await.is_ok();
                (out.ok(), committed)
            },
            async {
                let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
                let out = kooperatif_server::payments::repo::create_payment(
                    &mut tx,
                    Some(p.coop_a),
                    p.user_c,
                    race_cmd(),
                )
                .await;
                let committed = tx.commit().await.is_ok();
                (out.ok(), committed)
            }
        );
        let posted: i64 =
            sqlx::query_scalar("SELECT count(*) FROM payments WHERE idempotency_key = 'b1-race'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(posted, 1, "concurrent writers never double-post");
        let movements: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM account_movements m \
             JOIN payments p ON p.id = m.source_id \
             WHERE m.source_type = 'payment' AND p.idempotency_key = 'b1-race'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(movements, 1, "exactly one cash leg posted");
        let succeeded = [r1, r2]
            .iter()
            .filter(|(outcome, committed)| outcome.is_some() && *committed)
            .count();
        assert!(succeeded >= 1, "at least one writer completes");
    }

    // ── (11) cross-cooperative references stay indistinguishable ──
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let res = kooperatif_server::payments::repo::create_payment(
            &mut tx,
            Some(p.coop_a),
            p.user_c,
            kooperatif_server::payments::repo::CreatePayment {
                payer_person_id: Some(rows.payer),
                payer_first_name: None,
                payer_last_name: None,
                amount: rust_decimal::Decimal::new(500, 2),
                method: "cash".into(),
                received_at: now,
                note: None,
                destination_account_id: rows.account_foreign,
                idempotency_key: "b1-foreign-acct".into(),
                fingerprint: "fp-foreign".into(),
                allocations: vec![],
            },
        )
        .await;
        assert!(
            matches!(
                res,
                Err(
                    kooperatif_server::payments::repo::PaymentCommandError::AccountNotFoundOrInactive
                )
            ),
            "foreign destination is indistinguishable: {:?}",
            res.err()
        );
        tx.rollback().await.ok();
    }

    // ── (12) audit atomicity — minted in-tx, never surviving its
    //         rollback.
    let payment_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events \
         WHERE event_type = 'payment_posted' \
           AND cooperative_id = $1 AND user_id = $2 \
           AND metadata->>'payment_id' = $3::text",
    )
    .bind(p.coop_a)
    .bind(p.user_c)
    .bind(unapplied.payment_id.to_string())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(payment_events, 1, "the payment minted exactly one event");
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let payment = b1_raw_payment(
            &mut tx,
            rows.payer,
            p.user_c,
            p.coop_a,
            rows.account_a,
            10,
            "b1-doomed",
        )
        .await;
        b1_raw_movement(&mut tx, rows.account_a, payment, p.user_c, p.coop_a, 10).await;
        let doomed = sqlx::query("SELECT 1/0").execute(&mut *tx).await;
        assert!(doomed.is_err());
        tx.rollback().await.ok();
        let leaked: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM security_events \
             WHERE metadata->>'payment_id' = $1::text",
        )
        .bind(payment.to_string())
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(leaked, 0, "rolled-back package leaves no audit row");
    }

    // ── (13) reconciliation sweep — every posted payment carries
    //         exactly one matching cash leg and a non-negative
    //         unallocated remainder; no assessment is over-settled.
    let violations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payments p \
         WHERE p.cooperative_id = $1 AND p.status = 'posted' AND ( \
           (SELECT count(*) FROM account_movements m \
             WHERE m.source_type='payment' AND m.source_id=p.id \
               AND m.status='active' AND m.direction='inflow' \
               AND m.account_id=p.destination_account_id \
               AND m.amount=p.amount) <> 1 \
           OR (SELECT COALESCE(sum(a.amount),0) FROM payment_allocations a \
                WHERE a.payment_id=p.id AND a.status='active') \
            + (SELECT COALESCE(sum(c.amount),0) FROM shareholder_credits c \
                WHERE c.source_payment_id=p.id AND c.status='active') \
              > p.amount \
         )",
    )
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        violations, 0,
        "payment/movement/disposition invariants hold"
    );
    let over_settled: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM assessments a \
         WHERE a.cooperative_id = $1 AND ( \
           (SELECT COALESCE(sum(pa.amount),0) FROM payment_allocations pa \
             WHERE pa.assessment_id=a.id AND pa.status='active') \
         + (SELECT COALESCE(sum(ca.amount),0) FROM credit_applications ca \
             WHERE ca.assessment_id=a.id AND ca.status='active') \
         ) > a.amount",
    )
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(over_settled, 0, "no assessment is over-settled");

    app.close().await;
    db.cleanup().await;
}

// ------------------------------------------------------------------
// M1-P4-B6 — Restricted-lane lock remediation (0059): the thirteen
// FOR UPDATE sites on grant-less tables now go through scope-pinned
// SECURITY DEFINER helpers; the 0058 helpers are pinned to the
// verified sealed scope so a forged p_coop cannot lock another
// cooperative's rows.
// ------------------------------------------------------------------

#[tokio::test]
async fn b6_restricted_lane_locks() {
    let db = setup().await;
    let p = pilot_fixture(&db).await;
    let token_c: Vec<u8> = vec![0x53u8; 32];
    r3_grant_perms(
        &db,
        p.coop_a,
        p.user_c,
        &[
            "governance.manage",
            "investments.manage",
            "families.manage",
            "share_returns.manage",
            "social_aid.manage",
            "payments.manage",
        ],
    )
    .await;
    let app = db.app_pool().await;

    // ── Owner-lane fixtures (coop A) ──
    let person: Uuid = sqlx::query_scalar(
        "INSERT INTO persons (first_name, last_name, search_name, cooperative_id) \
         VALUES ('B6','T','b6t',$1) RETURNING id",
    )
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let shareholder: Uuid = sqlx::query_scalar(
        "INSERT INTO shareholders (person_id, cooperative_id) \
         VALUES ($1,$2) RETURNING id",
    )
    .bind(person)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let family: Uuid = sqlx::query_scalar(
        "INSERT INTO families (sequence_number, cooperative_id) \
         VALUES ((SELECT COALESCE(max(sequence_number),0)+1 FROM families), $1) \
         RETURNING id",
    )
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO shareholder_family_memberships \
         (shareholder_id, family_id, started_at, created_by, cooperative_id) \
         VALUES ($1,$2,'2025-01-01',$3,$4)",
    )
    .bind(shareholder)
    .bind(family)
    .bind(p.user_c)
    .bind(p.coop_a)
    .execute(&db.pool)
    .await
    .unwrap();
    let share: Uuid = sqlx::query_scalar(
        "INSERT INTO shares (cooperative_id, share_number, created_by) \
         VALUES ($1, public.allocate_document_number($1,'shares.share_number'), $2) \
         RETURNING id",
    )
    .bind(p.coop_a)
    .bind(p.user_c)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let share_event: Uuid = sqlx::query_scalar(
        "INSERT INTO share_events \
         (share_id, event_type, occurred_at, to_shareholder_id, acquisition_type, \
          actor_user_id, cooperative_id) \
         VALUES ($1,'initial_acquisition','2025-01-01',$2,'founder',$3,$4) RETURNING id",
    )
    .bind(share)
    .bind(shareholder)
    .bind(p.user_c)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO share_ownerships \
         (share_id, shareholder_id, started_at, acquisition_type, source_event_id, \
          created_by, cooperative_id) \
         VALUES ($1,$2,'2025-01-01','founder',$3,$4,$5)",
    )
    .bind(share)
    .bind(shareholder)
    .bind(share_event)
    .bind(p.user_c)
    .bind(p.coop_a)
    .execute(&db.pool)
    .await
    .unwrap();
    // Governance membership on the fixture body — ends through the
    // remediated repo path below.
    let gmembership: Uuid = sqlx::query_scalar(
        "INSERT INTO governance_memberships \
         (body_id, person_id, started_at, idempotency_key, idempotency_fingerprint, \
          created_by, cooperative_id) \
         VALUES ($1,$2,'2025-01-01','b6gm','b6gmfp',$3,$4) RETURNING id",
    )
    .bind(p.body_a)
    .bind(person)
    .bind(p.user_c)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let account: Uuid = sqlx::query_scalar(
        "INSERT INTO financial_accounts (name, account_type, created_by, cooperative_id) \
         VALUES ('B6Kasa','cash',$1,$2) RETURNING id",
    )
    .bind(p.user_c)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let (investment, _funding, valuation) =
        r3_investment_rows(&db, p.coop_a, p.user_c, account, "b6").await;
    let rows5 = batch5_rows(&db, p.coop_a, p.user_c, "b6").await;

    // An income leg so the investment funding can draw on balance.
    let income_id = Uuid::new_v4();
    let income_movement: Uuid = sqlx::query_scalar(
        "INSERT INTO account_movements \
         (account_id, direction, amount, source_type, source_id, occurred_at, \
          created_by, cooperative_id) \
         VALUES ($1,'inflow',1000,'income',$2,now(),$3,$4) RETURNING id",
    )
    .bind(account)
    .bind(income_id)
    .bind(p.user_c)
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let category_income: Uuid = sqlx::query_scalar(
        "INSERT INTO financial_categories \
         (category_type, name, cooperative_id) \
         VALUES ('income','B6Gelir',$1) RETURNING id",
    )
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO income_entries \
         (id, financial_account_id, category_id, amount, occurred_at, description, \
          account_movement_id, idempotency_key, idempotency_fingerprint, \
          created_by, cooperative_id, income_number) \
         VALUES ($1,$2,$3,1000,now(),'B6 kaynak',$4,'b6inc','b6incfp',$5,$6, \
                 public.allocate_document_number($6,'income_entries.income_number'))",
    )
    .bind(income_id)
    .bind(account)
    .bind(category_income)
    .bind(income_movement)
    .bind(p.user_c)
    .bind(p.coop_a)
    .execute(&db.pool)
    .await
    .unwrap();

    // ── (1) every new helper resolves its row inside the sealed scope ──
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        for (label, sql, id, expected) in [
            (
                "governance_body",
                "SELECT count(*) FROM public.app_lock_governance_body($1,$2)",
                p.body_a,
                1i64,
            ),
            (
                "governance_membership",
                "SELECT count(*) FROM public.app_lock_governance_membership($1,$2)",
                gmembership,
                1,
            ),
            (
                "investment",
                "SELECT count(*) FROM public.app_lock_investment($1,$2)",
                investment,
                1,
            ),
            (
                "investment_valuation",
                "SELECT count(*) FROM public.app_lock_investment_valuation($1,$2)",
                valuation,
                1,
            ),
            (
                "share_return",
                "SELECT count(*) FROM public.app_lock_share_return($1,$2)",
                rows5.share_return,
                1,
            ),
            (
                "share_return_entitlement",
                "SELECT count(*) FROM public.app_lock_share_return_entitlement($1,$2)",
                rows5.entitlement,
                1,
            ),
            // batch5's return is draft → the finalized-only helper
            // legitimately sees zero rows (join filter, not a lock).
            (
                "finalized_entitlement",
                "SELECT count(*) FROM public.app_lock_finalized_entitlement($1,$2)",
                rows5.entitlement,
                0,
            ),
            (
                "social_aid_fund",
                "SELECT count(*) FROM public.app_lock_social_aid_fund($1,$2)",
                rows5.fund,
                1,
            ),
        ] {
            let n: i64 = sqlx::query_scalar(sql)
                .bind(id)
                .bind(p.coop_a)
                .fetch_one(&mut *tx)
                .await
                .unwrap_or_else(|e| panic!("{label} lock failed: {e}"));
            assert_eq!(n, expected, "{label}: in-scope row must lock");
        }
        let n: i64 =
            sqlx::query_scalar("SELECT count(*) FROM public.app_lock_governance_voter($1,$2,$3)")
                .bind(p.body_a)
                .bind(person)
                .bind(p.coop_a)
                .fetch_one(&mut *tx)
                .await
                .expect("voter lock");
        assert_eq!(n, 1, "active membership row locks by voter key");
        let n: i64 =
            sqlx::query_scalar("SELECT count(*) FROM public.app_lock_share_ownership($1,$2)")
                .bind(share)
                .bind(p.coop_a)
                .fetch_one(&mut *tx)
                .await
                .expect("ownership lock");
        assert_eq!(n, 1, "live ownership locks by share");
        let n: i64 =
            sqlx::query_scalar("SELECT count(*) FROM public.app_lock_family_membership($1,$2)")
                .bind(shareholder)
                .bind(p.coop_a)
                .fetch_one(&mut *tx)
                .await
                .expect("family membership lock");
        assert_eq!(n, 1, "active family membership locks");
        tx.rollback().await.unwrap();
    }

    // ── (2) helpers are pinned to the VERIFIED scope ──
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        // A foreign body id resolves as absent — even when the caller
        // passes the OTHER cooperative as p_coop.
        let n: i64 =
            sqlx::query_scalar("SELECT count(*) FROM public.app_lock_governance_body($1,$2)")
                .bind(p.body_b)
                .bind(p.coop_b)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(n, 0, "cross-coop row is unreachable");
        // A forged p_coop argument is ignored entirely: the sealed
        // scope governs, so the own row still resolves.
        let n: i64 =
            sqlx::query_scalar("SELECT count(*) FROM public.app_lock_governance_body($1,$2)")
                .bind(p.body_a)
                .bind(p.coop_b)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(n, 1, "p_coop cannot redirect the verified scope");
        // Same pin on the 0058 helpers: a foreign coop parameter can
        // no longer reach foreign rows.
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM public.app_lock_payment($1,$2)")
            .bind(Uuid::new_v4())
            .bind(p.coop_b)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(n, 0, "forged coop cannot lock foreign payments");
        tx.rollback().await.unwrap();
    }

    // ── (3) permission gate: no business grant → 42501 ──
    {
        let mut tx = r3_sealed(&app, &p.token_a, p.coop_a).await;
        let res = sqlx::query("SELECT status FROM public.app_lock_governance_body($1,$2)")
            .bind(p.body_a)
            .bind(p.coop_a)
            .execute(&mut *tx)
            .await;
        assert!(
            matches!(res, Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("42501")),
            "governance.manage is required — users.manage alone must fail"
        );
        tx.rollback().await.ok();
    }

    // ── (4) the raw lock path stays closed ──
    for table in [
        "social_aid_funds",
        "share_ownerships",
        "governance_memberships",
    ] {
        let res = r3_probe_id(
            &app,
            &token_c,
            p.coop_a,
            &format!("SELECT id FROM {table} WHERE id = $1 FOR UPDATE"),
            Uuid::new_v4(),
        )
        .await;
        assert!(r3_is_42501(&res), "{table}: raw FOR UPDATE stays denied");
    }

    // ── (5) repo command paths on the real restricted lane ──
    // Each was a confirmed 42501 before 0059 (lock on a grant-less
    // table). All now run inside the sealed request transaction.
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        kooperatif_server::governance::repo::end_membership(
            &mut tx,
            Some(p.coop_a),
            p.user_c,
            gmembership,
            OffsetDateTime::now_utc(),
            "b6 end",
        )
        .await
        .expect("end_membership on the restricted lane");
        tx.commit().await.expect("commit");
    }
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let outcome = kooperatif_server::share_returns::repo::initiate_return(
            &mut tx,
            Some(p.coop_a),
            p.user_c,
            kooperatif_server::share_returns::repo::InitiateReturn {
                share_id: share,
                effective_return_date: OffsetDateTime::now_utc().date(),
                reason: Some("b6".to_string()),
                idempotency_key: "b6-init".to_string(),
                fingerprint: "b6-initfp".to_string(),
            },
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("initiate_return on the restricted lane");
        assert!(!outcome.replayed);
        tx.commit().await.expect("commit");
    }
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        kooperatif_server::social_aid::repo::post_donation(
            &mut tx,
            Some(p.coop_a),
            p.user_c,
            kooperatif_server::social_aid::repo::PostDonation {
                fund_id: rows5.fund,
                donor_person_id: None,
                donor_display_name: Some("B6 Hayırsever".to_string()),
                financial_account_id: account,
                amount: rust_decimal::Decimal::new(2500, 2),
                occurred_at: OffsetDateTime::now_utc(),
                reference: None,
                note: None,
                idempotency_key: "b6-don".to_string(),
                fingerprint: "b6-donfp".to_string(),
            },
        )
        .await
        .expect("post_donation on the restricted lane");
        tx.commit().await.expect("commit");
    }
    {
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        kooperatif_server::investments::repo::post_funding(
            &mut tx,
            Some(p.coop_a),
            p.user_c,
            kooperatif_server::investments::repo::PostFunding {
                investment_id: investment,
                financial_account_id: account,
                amount: rust_decimal::Decimal::new(1000, 2),
                occurred_at: OffsetDateTime::now_utc(),
                reference: None,
                note: None,
                idempotency_key: "b6-fnd".to_string(),
                fingerprint: "b6-fndfp".to_string(),
            },
        )
        .await
        .expect("post_funding on the restricted lane");
        tx.commit().await.expect("commit");
    }
    {
        let new_family: Uuid = sqlx::query_scalar(
            "INSERT INTO families (sequence_number, cooperative_id) \
             VALUES ((SELECT COALESCE(max(sequence_number),0)+1 FROM families), $1) \
             RETURNING id",
        )
        .bind(p.coop_a)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let updated_at: OffsetDateTime =
            sqlx::query_scalar("SELECT updated_at FROM shareholders WHERE id = $1")
                .bind(shareholder)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        let mut tx = r3_sealed(&app, &token_c, p.coop_a).await;
        let (_, _, new_seq) = kooperatif_server::parties::repo::change_family(
            &mut tx,
            Some(p.coop_a),
            p.user_c,
            shareholder,
            kooperatif_server::parties::repo::FamilyChangeTarget::Existing(new_family),
            Some("b6 move"),
            updated_at,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("change_family on the restricted lane");
        assert!(new_seq > 0);
        tx.commit().await.expect("commit");
    }

    // ── (6) audit atomicity is intact: the mutation and its event are
    // one transaction ──
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events \
         WHERE cooperative_id = $1 AND event_type IN \
           ('governance_membership_ended','share_return_requested',\
            'social_aid_donation_posted','investment_funding_posted')",
    )
    .bind(p.coop_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(events, 4, "each mutation minted exactly one event");

    app.close().await;
    db.cleanup().await;
}
