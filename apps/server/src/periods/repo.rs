//! Periods repository (STEP-006): list/detail queries with canonical
//! Shareholder identity (no N+1), and transactional commands — draft
//! create/update/delete, the atomic assessment-generation command and
//! the explicit close command.
//!
//! Snapshot discipline (§19/§35/§36): generation resolves eligibility
//! ONCE inside the transaction and persists amount + rule snapshot +
//! per-Share provenance. Nothing here recomputes obligations from
//! current state at read time.

use rust_decimal::Decimal;
use sqlx::PgPool;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use super::model::{AssessmentRuleType, PeriodStatus};
use crate::parties::model as party_model;
use crate::shares::repo::ShareholderIdentityRow;

// ------------------------------------------------------------------
// Read models
// ------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
pub struct PeriodRow {
    pub id: Uuid,
    pub period_number: i64,
    pub name: String,
    pub status: String,
    pub collection_start_date: Date,
    pub due_date: Date,
    pub rule_type: Option<String>,
    pub base_amount: Option<Decimal>,
    pub currency: Option<String>,
    pub assessment_effective_date: Option<Date>,
    pub assessment_count: i64,
    /// NULL before generation (no obligations exist yet) — the frontend
    /// must not render "0" as if a zero total were posted.
    pub total_assessment: Option<Decimal>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub total_count: i64,
}

const PERIOD_SELECT: &str = "SELECT p.id, p.period_number, p.name, p.status, \
    p.collection_start_date, p.due_date, \
    r.rule_type, r.base_amount, r.currency, r.assessment_effective_date, \
    COALESCE(agg.cnt, 0)::bigint AS assessment_count, agg.total AS total_assessment, \
    p.created_at, p.updated_at \
FROM periods p \
LEFT JOIN assessment_rules r ON r.period_id = p.id \
LEFT JOIN ( \
    SELECT period_id, count(*) AS cnt, sum(amount) AS total \
    FROM assessments GROUP BY period_id \
) agg ON agg.period_id = p.id ";

/// Period list: substring over the name or an exact period number when
/// the term is a positive integer. Stable ordering by business number
/// (ADR-010; small administrative dataset → offset pagination).
pub async fn list_periods(
    pool: &PgPool,
    search: Option<&str>,
    page: i64,
    page_size: i64,
) -> Result<Vec<PeriodRow>, sqlx::Error> {
    let raw = search.map(str::trim).filter(|s| !s.is_empty());
    let folded: Option<String> = raw.map(|term| format!("%{}%", party_model::fold_search(term)));
    let number: Option<i64> = raw
        .and_then(|term| term.parse::<i64>().ok())
        .filter(|n| *n > 0);

    let sql = format!(
        "SELECT *, count(*) OVER () AS total_count FROM ({PERIOD_SELECT}) base \
         WHERE ( \
            $1::text IS NULL \
            OR lower(base.name) LIKE $1 \
            OR ($2::bigint IS NOT NULL AND base.period_number = $2) \
         ) \
         ORDER BY base.period_number \
         OFFSET $3 ROWS FETCH NEXT $4 ROWS ONLY"
    );
    sqlx::query_as::<_, PeriodRow>(&sql)
        .bind(folded)
        .bind(number)
        .bind((page - 1) * page_size)
        .bind(page_size)
        .fetch_all(pool)
        .await
}

pub async fn find_period(pool: &PgPool, id: Uuid) -> Result<Option<PeriodRow>, sqlx::Error> {
    let sql =
        format!("SELECT *, 0::bigint AS total_count FROM ({PERIOD_SELECT} WHERE p.id = $1) base");
    sqlx::query_as::<_, PeriodRow>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// One row of an assessment list/preview: canonical debtor identity +
/// obligation amount + per-Share count (0 for per_shareholder).
#[derive(Debug, sqlx::FromRow)]
pub struct AssessmentRow {
    pub id: Uuid,
    pub period_id: Uuid,
    pub rule_type: String,
    pub base_amount: Decimal,
    pub amount: Decimal,
    pub currency: String,
    pub status: String,
    pub assessment_effective_date: Date,
    pub generated_at: OffsetDateTime,
    pub share_count: i64,
    #[sqlx(flatten)]
    pub shareholder: ShareholderIdentityRow,
    pub total_count: i64,
}

const ASSESSMENT_SELECT: &str = "SELECT a.id, a.period_id, a.rule_type, \
    a.base_amount, a.amount, a.currency, a.status, a.assessment_effective_date, \
    a.generated_at, \
    COALESCE(src.cnt, 0)::bigint AS share_count, \
    a.shareholder_id, \
    p.first_name, p.last_name, \
    gp.first_name AS guardian_first_name, gp.last_name AS guardian_last_name, \
    f.sequence_number AS family_sequence, \
    s.status AS shareholder_status \
FROM assessments a \
JOIN shareholders s ON s.id = a.shareholder_id \
JOIN persons p ON p.id = s.person_id \
LEFT JOIN persons gp ON gp.id = s.guardian_person_id \
LEFT JOIN shareholder_family_memberships m \
    ON m.shareholder_id = s.id AND m.ended_at IS NULL \
LEFT JOIN families f ON f.id = m.family_id \
LEFT JOIN ( \
    SELECT assessment_id, count(*) AS cnt \
    FROM assessment_share_sources GROUP BY assessment_id \
) src ON src.assessment_id = a.id ";

/// Assessment list for one Period (§61): canonical identity on every
/// row; search folds over shareholder name, guardian name or an exact
/// family number; stable ordering by folded name + assessment id.
pub async fn list_assessments(
    pool: &PgPool,
    period_id: Uuid,
    search: Option<&str>,
    page: i64,
    page_size: i64,
) -> Result<Vec<AssessmentRow>, sqlx::Error> {
    let raw = search.map(str::trim).filter(|s| !s.is_empty());
    let folded: Option<String> = raw.map(|term| format!("%{}%", party_model::fold_search(term)));
    let number: Option<i64> = raw
        .and_then(|term| term.parse::<i64>().ok())
        .filter(|n| *n > 0);

    let sql = format!(
        "SELECT *, count(*) OVER () AS total_count FROM ({ASSESSMENT_SELECT}) base \
         WHERE base.period_id = $1 AND ( \
            $2::text IS NULL \
            OR lower(base.first_name || ' ' || base.last_name) LIKE $2 \
            OR lower(coalesce(base.guardian_first_name, '') || ' ' || \
                     coalesce(base.guardian_last_name, '')) LIKE $2 \
            OR ($3::bigint IS NOT NULL AND base.family_sequence = $3) \
         ) \
         ORDER BY lower(base.last_name), lower(base.first_name), base.id \
         OFFSET $4 ROWS FETCH NEXT $5 ROWS ONLY"
    );
    sqlx::query_as::<_, AssessmentRow>(&sql)
        .bind(period_id)
        .bind(folded)
        .bind(number)
        .bind((page - 1) * page_size)
        .bind(page_size)
        .fetch_all(pool)
        .await
}

pub async fn find_assessment(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<AssessmentRow>, sqlx::Error> {
    let sql = format!(
        "SELECT *, 0::bigint AS total_count FROM ({ASSESSMENT_SELECT} WHERE a.id = $1) base"
    );
    sqlx::query_as::<_, AssessmentRow>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// One provenance line of a per_share assessment: which Share caused
/// how much of the obligation, pinned to the exact ownership interval
/// used at generation (§23).
#[derive(Debug, sqlx::FromRow)]
pub struct AssessmentSourceRow {
    pub share_id: Uuid,
    pub share_number: i64,
    pub amount_component: Decimal,
    pub ownership_started_at: OffsetDateTime,
}

pub async fn assessment_sources(
    pool: &PgPool,
    assessment_id: Uuid,
) -> Result<Vec<AssessmentSourceRow>, sqlx::Error> {
    sqlx::query_as::<_, AssessmentSourceRow>(
        "SELECT src.share_id, sh.share_number, src.amount_component, \
            o.started_at AS ownership_started_at \
         FROM assessment_share_sources src \
         JOIN shares sh ON sh.id = src.share_id \
         JOIN share_ownerships o ON o.id = src.ownership_id \
         WHERE src.assessment_id = $1 \
         ORDER BY sh.share_number",
    )
    .bind(assessment_id)
    .fetch_all(pool)
    .await
}

/// A Shareholder's assessment history across Periods (§62): one row per
/// obligation with Period context — never recomputed from current
/// ownership.
#[derive(Debug, sqlx::FromRow)]
pub struct ShareholderAssessmentRow {
    pub id: Uuid,
    pub period_id: Uuid,
    pub period_number: i64,
    pub period_name: String,
    pub due_date: Date,
    pub rule_type: String,
    pub amount: Decimal,
    pub currency: String,
    pub share_count: i64,
    pub generated_at: OffsetDateTime,
}

pub async fn list_assessments_by_shareholder(
    pool: &PgPool,
    shareholder_id: Uuid,
) -> Result<Vec<ShareholderAssessmentRow>, sqlx::Error> {
    sqlx::query_as::<_, ShareholderAssessmentRow>(
        "SELECT a.id, a.period_id, p.period_number, p.name AS period_name, \
            p.due_date, a.rule_type, a.amount, a.currency, a.generated_at, \
            COALESCE(src.cnt, 0)::bigint AS share_count \
         FROM assessments a \
         JOIN periods p ON p.id = a.period_id \
         LEFT JOIN ( \
             SELECT assessment_id, count(*) AS cnt \
             FROM assessment_share_sources GROUP BY assessment_id \
         ) src ON src.assessment_id = a.id \
         WHERE a.shareholder_id = $1 \
         ORDER BY p.period_number DESC, a.id",
    )
    .bind(shareholder_id)
    .fetch_all(pool)
    .await
}

// ------------------------------------------------------------------
// Eligibility resolution (preview + generation share one source)
// ------------------------------------------------------------------

/// One eligible (shareholder, share, ownership-interval) triple or —
/// for the per_shareholder rule — one eligible shareholder.
pub struct EligibleRow {
    pub shareholder: ShareholderIdentityRow,
    /// per_share only: the contributing share + its ownership interval.
    pub share_id: Option<Uuid>,
    pub ownership_id: Option<Uuid>,
}

/// Active Shareholders with canonical identity — the per_shareholder
/// eligible population (§25: active only; inactive/voided never
/// receive obligations).
async fn eligible_shareholders(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<Vec<ShareholderIdentityRow>, sqlx::Error> {
    sqlx::query_as::<_, ShareholderIdentityRow>(
        "SELECT s.id AS shareholder_id, p.first_name, p.last_name, \
            gp.first_name AS guardian_first_name, gp.last_name AS guardian_last_name, \
            f.sequence_number AS family_sequence, s.status AS shareholder_status \
         FROM shareholders s \
         JOIN persons p ON p.id = s.person_id \
         LEFT JOIN persons gp ON gp.id = s.guardian_person_id \
         LEFT JOIN shareholder_family_memberships m \
             ON m.shareholder_id = s.id AND m.ended_at IS NULL \
         LEFT JOIN families f ON f.id = m.family_id \
         WHERE s.status = 'active' \
         ORDER BY p.search_name, s.id",
    )
    .fetch_all(tx.as_mut())
    .await
}

/// Eligible (active) Shares owned at the assessment effective point by
/// active Shareholders (§25/§26): share status is evaluated at
/// generation time (share status has no temporal dimension) while
/// OWNERSHIP is resolved at the effective date — the interval that
/// covers the end of that business day in Europe/Istanbul, i.e.
/// `started_at < D+1 00:00 Istanbul AND (ended_at IS NULL OR
/// ended_at >= D+1 00:00 Istanbul)`.
async fn eligible_share_ownerships(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    effective_date: Date,
) -> Result<Vec<EligibleRow>, sqlx::Error> {
    #[derive(sqlx::FromRow)]
    struct Row {
        share_id: Uuid,
        ownership_id: Uuid,
        #[sqlx(flatten)]
        shareholder: ShareholderIdentityRow,
    }
    let rows = sqlx::query_as::<_, Row>(
        "SELECT so.share_id, so.id AS ownership_id, \
            s.id AS shareholder_id, p.first_name, p.last_name, \
            gp.first_name AS guardian_first_name, gp.last_name AS guardian_last_name, \
            f.sequence_number AS family_sequence, s.status AS shareholder_status \
         FROM share_ownerships so \
         JOIN shares sh ON sh.id = so.share_id AND sh.status = 'active' \
         JOIN shareholders s ON s.id = so.shareholder_id AND s.status = 'active' \
         JOIN persons p ON p.id = s.person_id \
         LEFT JOIN persons gp ON gp.id = s.guardian_person_id \
         LEFT JOIN shareholder_family_memberships m \
             ON m.shareholder_id = s.id AND m.ended_at IS NULL \
         LEFT JOIN families f ON f.id = m.family_id \
         WHERE so.started_at < (($1::date + 1)::timestamp AT TIME ZONE 'Europe/Istanbul') \
           AND (so.ended_at IS NULL \
                OR so.ended_at >= (($1::date + 1)::timestamp AT TIME ZONE 'Europe/Istanbul')) \
         ORDER BY p.search_name, s.id, sh.share_number",
    )
    .bind(effective_date)
    .fetch_all(tx.as_mut())
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| EligibleRow {
            shareholder: r.shareholder,
            share_id: Some(r.share_id),
            ownership_id: Some(r.ownership_id),
        })
        .collect())
}

/// Resolve the eligible population for a rule — used identically by
/// preview and finalization so what the operator saw is what is
/// generated.
async fn resolve_eligible(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule_type: AssessmentRuleType,
    effective_date: Date,
) -> Result<Vec<EligibleRow>, sqlx::Error> {
    match rule_type {
        AssessmentRuleType::PerShareholder => Ok(eligible_shareholders(tx)
            .await?
            .into_iter()
            .map(|shareholder| EligibleRow {
                shareholder,
                share_id: None,
                ownership_id: None,
            })
            .collect()),
        AssessmentRuleType::PerShare => eligible_share_ownerships(tx, effective_date).await,
    }
}

// ------------------------------------------------------------------
// Transactional commands
// ------------------------------------------------------------------

pub struct CreatePeriod {
    pub name: String,
    pub collection_start_date: Date,
    pub due_date: Date,
    pub rule_type: AssessmentRuleType,
    pub base_amount: Decimal,
    pub assessment_effective_date: Date,
}

#[derive(Debug)]
pub enum PeriodCommandError {
    NotFound,
    /// The period is not a draft — financial configuration and
    /// destructive operations are finalized-state protected (§37/§38).
    NotDraft,
    /// The period is not open (close command guard).
    NotOpen,
    /// Optimistic-concurrency precondition failed (§86).
    StaleState,
    /// Assessments already exist for this period (§32 double-generation
    /// protection) or the unique constraint fired — 409 conflict.
    AlreadyFinalized,
    /// Invariant: a period always has its rule row.
    MissingRule,
    Database(sqlx::Error),
}

/// Create Draft Period + its Assessment Rule in ONE transaction — a
/// Period never exists without its explicit rule (§13/§65). No
/// obligations are generated here.
pub async fn create_period(
    pool: &PgPool,
    actor: Uuid,
    command: CreatePeriod,
) -> Result<(Uuid, i64), PeriodCommandError> {
    let mut tx = pool.begin().await.map_err(PeriodCommandError::Database)?;

    let (period_id, period_number): (Uuid, i64) = sqlx::query_as(
        "INSERT INTO periods (name, collection_start_date, due_date, created_by) \
         VALUES ($1, $2, $3, $4) RETURNING id, period_number",
    )
    .bind(&command.name)
    .bind(command.collection_start_date)
    .bind(command.due_date)
    .bind(actor)
    .fetch_one(tx.as_mut())
    .await
    .map_err(PeriodCommandError::Database)?;

    sqlx::query(
        "INSERT INTO assessment_rules \
             (period_id, rule_type, base_amount, currency, assessment_effective_date) \
         VALUES ($1, $2, $3, 'TRY', $4)",
    )
    .bind(period_id)
    .bind(command.rule_type.as_str())
    .bind(command.base_amount)
    .bind(command.assessment_effective_date)
    .execute(tx.as_mut())
    .await
    .map_err(PeriodCommandError::Database)?;

    tx.commit().await.map_err(PeriodCommandError::Database)?;
    Ok((period_id, period_number))
}

pub struct UpdatePeriod {
    pub name: String,
    pub collection_start_date: Date,
    pub due_date: Date,
    pub rule_type: AssessmentRuleType,
    pub base_amount: Decimal,
    pub assessment_effective_date: Date,
    pub expected_updated_at: OffsetDateTime,
}

/// Update a DRAFT period's configuration + rule (§37/§66). The row lock
/// and expected_updated_at precondition serialize concurrent edits;
/// finalized periods reject updates entirely so generated obligations
/// can never be silently reinterpreted.
pub async fn update_period(
    pool: &PgPool,
    id: Uuid,
    command: UpdatePeriod,
    now: OffsetDateTime,
) -> Result<(), PeriodCommandError> {
    let mut tx = pool.begin().await.map_err(PeriodCommandError::Database)?;

    let row: Option<(String, OffsetDateTime)> =
        sqlx::query_as("SELECT status, updated_at FROM periods WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(PeriodCommandError::Database)?;
    let Some((status, updated_at)) = row else {
        return Err(PeriodCommandError::NotFound);
    };
    if updated_at != command.expected_updated_at {
        return Err(PeriodCommandError::StaleState);
    }
    if status != PeriodStatus::Draft.as_str() {
        return Err(PeriodCommandError::NotDraft);
    }

    sqlx::query(
        "UPDATE periods SET name = $2, collection_start_date = $3, due_date = $4, \
            updated_at = $5 WHERE id = $1",
    )
    .bind(id)
    .bind(&command.name)
    .bind(command.collection_start_date)
    .bind(command.due_date)
    .bind(now)
    .execute(tx.as_mut())
    .await
    .map_err(PeriodCommandError::Database)?;

    sqlx::query(
        "UPDATE assessment_rules SET rule_type = $2, base_amount = $3, \
            assessment_effective_date = $4, updated_at = $5 WHERE period_id = $1",
    )
    .bind(id)
    .bind(command.rule_type.as_str())
    .bind(command.base_amount)
    .bind(command.assessment_effective_date)
    .bind(now)
    .execute(tx.as_mut())
    .await
    .map_err(PeriodCommandError::Database)?;

    tx.commit().await.map_err(PeriodCommandError::Database)
}

/// Hard-delete is allowed ONLY for safe drafts (docs/02, docs/19 §9):
/// a draft carries no assessments by construction, so nothing durable
/// is lost. Open/closed periods are financial history and cannot be
/// deleted through the product surface (§38).
pub async fn delete_period(pool: &PgPool, id: Uuid) -> Result<i64, PeriodCommandError> {
    let mut tx = pool.begin().await.map_err(PeriodCommandError::Database)?;

    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT status, period_number FROM periods WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(PeriodCommandError::Database)?;
    let Some((status, period_number)) = row else {
        return Err(PeriodCommandError::NotFound);
    };
    if status != PeriodStatus::Draft.as_str() {
        return Err(PeriodCommandError::NotDraft);
    }

    sqlx::query("DELETE FROM assessment_rules WHERE period_id = $1")
        .bind(id)
        .execute(tx.as_mut())
        .await
        .map_err(PeriodCommandError::Database)?;
    sqlx::query("DELETE FROM periods WHERE id = $1")
        .bind(id)
        .execute(tx.as_mut())
        .await
        .map_err(PeriodCommandError::Database)?;

    tx.commit().await.map_err(PeriodCommandError::Database)?;
    Ok(period_number)
}

/// Explicit `open -> closed` transition (docs/16): locks ordinary
/// mutation without deleting or rewriting obligations.
pub async fn close_period(
    pool: &PgPool,
    id: Uuid,
    now: OffsetDateTime,
) -> Result<i64, PeriodCommandError> {
    let mut tx = pool.begin().await.map_err(PeriodCommandError::Database)?;

    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT status, period_number FROM periods WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(PeriodCommandError::Database)?;
    let Some((status, period_number)) = row else {
        return Err(PeriodCommandError::NotFound);
    };
    if status != PeriodStatus::Open.as_str() {
        return Err(PeriodCommandError::NotOpen);
    }

    sqlx::query("UPDATE periods SET status = 'closed', updated_at = $2 WHERE id = $1")
        .bind(id)
        .bind(now)
        .execute(tx.as_mut())
        .await
        .map_err(PeriodCommandError::Database)?;

    tx.commit().await.map_err(PeriodCommandError::Database)?;
    Ok(period_number)
}

// ------------------------------------------------------------------
// Preview + finalization
// ------------------------------------------------------------------

/// What the eligible population produces under the governing rule.
/// `rows` are the per-Shareholder preview lines (share_count = 0 for
/// per_shareholder); `sources` pairs each row index with its
/// contributing shares (empty for per_shareholder).
pub struct PreviewPlan {
    pub rule_type: AssessmentRuleType,
    pub base_amount: Decimal,
    pub effective_date: Date,
    /// (shareholder, share_count, amount) ordered stably.
    pub rows: Vec<(ShareholderIdentityRow, i64, Decimal)>,
    /// row index -> (share_id, ownership_id) contributing to that row.
    pub sources: Vec<(usize, Uuid, Uuid)>,
    pub eligible_share_count: i64,
    pub total_amount: Decimal,
}

fn plan_of(
    rule_type: AssessmentRuleType,
    base_amount: Decimal,
    effective_date: Date,
    eligible: Vec<EligibleRow>,
) -> PreviewPlan {
    match rule_type {
        AssessmentRuleType::PerShareholder => {
            let count = Decimal::from(eligible.len() as i64);
            let rows = eligible
                .into_iter()
                .map(|row| (row.shareholder, 0i64, base_amount))
                .collect();
            PreviewPlan {
                rule_type,
                base_amount,
                effective_date,
                rows,
                sources: Vec::new(),
                eligible_share_count: 0,
                total_amount: base_amount * count,
            }
        }
        AssessmentRuleType::PerShare => {
            // Group consecutive per-share rows by shareholder (the query
            // already orders by folded name then shareholder id).
            let mut rows: Vec<(ShareholderIdentityRow, i64, Decimal)> = Vec::new();
            let mut sources: Vec<(usize, Uuid, Uuid)> = Vec::new();
            let eligible_share_count = eligible.len() as i64;
            for row in eligible {
                let share_id = row.share_id.expect("per_share row");
                let ownership_id = row.ownership_id.expect("per_share row");
                let same = rows
                    .last()
                    .is_some_and(|(s, _, _)| s.shareholder_id == row.shareholder.shareholder_id);
                if same {
                    let last = rows.last_mut().expect("nonempty");
                    last.1 += 1;
                    last.2 += base_amount;
                } else {
                    rows.push((row.shareholder, 1, base_amount));
                }
                sources.push((rows.len() - 1, share_id, ownership_id));
            }
            let total_amount = base_amount * Decimal::from(eligible_share_count);
            PreviewPlan {
                rule_type,
                base_amount,
                effective_date,
                rows,
                sources,
                eligible_share_count,
                total_amount,
            }
        }
    }
}

async fn load_rule(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    period_id: Uuid,
) -> Result<(AssessmentRuleType, Decimal, Date), PeriodCommandError> {
    let row: Option<(String, Decimal, Date)> = sqlx::query_as(
        "SELECT rule_type, base_amount, assessment_effective_date \
         FROM assessment_rules WHERE period_id = $1",
    )
    .bind(period_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(PeriodCommandError::Database)?;
    let Some((rule_type, base_amount, effective_date)) = row else {
        return Err(PeriodCommandError::MissingRule);
    };
    let rule_type = AssessmentRuleType::parse(&rule_type).ok_or(PeriodCommandError::MissingRule)?;
    Ok((rule_type, base_amount, effective_date))
}

/// Read-only preview (§28/§29): resolves the eligible population
/// exactly as generation would, inside a transaction so the picture is
/// consistent, then ROLLS BACK — preview never persists obligations.
pub async fn preview_assessments(
    pool: &PgPool,
    period_id: Uuid,
) -> Result<PreviewPlan, PeriodCommandError> {
    let mut tx = pool.begin().await.map_err(PeriodCommandError::Database)?;

    let status: Option<String> = sqlx::query_scalar("SELECT status FROM periods WHERE id = $1")
        .bind(period_id)
        .fetch_optional(tx.as_mut())
        .await
        .map_err(PeriodCommandError::Database)?;
    let Some(status) = status else {
        return Err(PeriodCommandError::NotFound);
    };
    if status != PeriodStatus::Draft.as_str() {
        return Err(PeriodCommandError::NotDraft);
    }

    let (rule_type, base_amount, effective_date) = load_rule(&mut tx, period_id).await?;
    let eligible = resolve_eligible(&mut tx, rule_type, effective_date)
        .await
        .map_err(PeriodCommandError::Database)?;
    let plan = plan_of(rule_type, base_amount, effective_date, eligible);

    tx.rollback().await.map_err(PeriodCommandError::Database)?;
    Ok(plan)
}

/// Aggregate facts returned by a successful generation (audit payload).
pub struct GenerationSummary {
    pub assessment_count: i64,
    pub share_source_count: i64,
    pub total_amount: Decimal,
}

/// Finalization (§30–§33): the explicit, confirmed, atomic command.
/// The Period row lock serializes concurrent finalizations; status
/// `draft` + UNIQUE(period_id, shareholder_id) make a second durable
/// generation impossible (§84/§85). Any failure rolls the whole set
/// back — no half-generated Period (§31/§92).
pub async fn generate_assessments(
    pool: &PgPool,
    actor: Uuid,
    period_id: Uuid,
) -> Result<GenerationSummary, PeriodCommandError> {
    let mut tx = pool.begin().await.map_err(PeriodCommandError::Database)?;

    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM periods WHERE id = $1 FOR UPDATE")
            .bind(period_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(PeriodCommandError::Database)?;
    let Some(status) = status else {
        return Err(PeriodCommandError::NotFound);
    };
    if status != PeriodStatus::Draft.as_str() {
        return Err(PeriodCommandError::AlreadyFinalized);
    }

    let (rule_type, base_amount, effective_date) = load_rule(&mut tx, period_id).await?;
    let eligible = resolve_eligible(&mut tx, rule_type, effective_date)
        .await
        .map_err(PeriodCommandError::Database)?;
    let plan = plan_of(rule_type, base_amount, effective_date, eligible);

    let mut assessment_ids: Vec<Uuid> = Vec::with_capacity(plan.rows.len());
    for (shareholder, _share_count, amount) in &plan.rows {
        let inserted = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO assessments \
                 (period_id, shareholder_id, rule_type, base_amount, amount, currency, \
                  status, assessment_effective_date, generated_by) \
             VALUES ($1, $2, $3, $4, $5, 'TRY', 'active', $6, $7) \
             RETURNING id",
        )
        .bind(period_id)
        .bind(shareholder.shareholder_id)
        .bind(rule_type.as_str())
        .bind(base_amount)
        .bind(amount)
        .bind(effective_date)
        .bind(actor)
        .fetch_one(tx.as_mut())
        .await;
        match inserted {
            Ok(id) => assessment_ids.push(id),
            Err(sqlx::Error::Database(db_err))
                if db_err
                    .constraint()
                    .is_some_and(|c| c.contains("period_shareholder")) =>
            {
                return Err(PeriodCommandError::AlreadyFinalized);
            }
            Err(other) => return Err(PeriodCommandError::Database(other)),
        }
    }

    for (row_index, share_id, ownership_id) in &plan.sources {
        sqlx::query(
            "INSERT INTO assessment_share_sources \
                 (assessment_id, share_id, ownership_id, amount_component) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(assessment_ids[*row_index])
        .bind(share_id)
        .bind(ownership_id)
        .bind(base_amount)
        .execute(tx.as_mut())
        .await
        .map_err(PeriodCommandError::Database)?;
    }

    sqlx::query("UPDATE periods SET status = 'open', updated_at = now() WHERE id = $1")
        .bind(period_id)
        .execute(tx.as_mut())
        .await
        .map_err(PeriodCommandError::Database)?;

    let summary = GenerationSummary {
        assessment_count: assessment_ids.len() as i64,
        share_source_count: plan.sources.len() as i64,
        total_amount: plan.total_amount,
    };
    tx.commit().await.map_err(PeriodCommandError::Database)?;
    Ok(summary)
}
