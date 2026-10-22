//! Governance repository (STEP-014, docs/09).
//!
//! Transaction discipline: every mutation is command-shaped and
//! idempotent (ADR-006). Canonical lock prefix for vote/finalization
//! commands:
//!   BEGIN -> idempotent replay check -> lock the DECISION row
//!   FOR UPDATE -> lock the MEMBERSHIP row (vote only) -> validate ->
//!   insert -> COMMIT.
//! The decision row lock serializes votes, open and finalize so a
//! finalized decision's vote set is deterministic and a vote can never
//! slip into a finalized decision. `end_membership` locks the
//! membership row, so a cast racing a membership end resolves
//! consistently against committed state.
//!
//! Governance is EVIDENCE, not money: no statement here writes
//! `account_movements` or any financial table.
//!
//! Undefined legal policy is never invented (docs/09 open decisions):
//! no quorum/majority computation, no vote weighting, no automatic
//! approval gating. Finalization records the formally decided outcome
//! plus a frozen electorate/tally snapshot.

use sqlx::Acquire;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::governance::model::{DecisionOutcome, VoteChoice};

#[derive(Debug)]
pub enum GovernanceRepoError {
    NotFound,
    /// Command rejected: wrong lifecycle state for the transition.
    InvalidState,
    /// Body close rejected: draft/open decisions or active memberships
    /// still exist.
    BodyBusy,
    /// Membership/vote command rejected: the body is not `active`.
    InactiveBody,
    /// Add-membership rejected: the person already holds an ACTIVE
    /// membership in this body (one active seat per person/body).
    ActiveMembershipExists,
    /// Vote rejected: no active membership for this person in the
    /// decision's body at cast time.
    NotEligible,
    /// Vote rejected: a vote row already exists for (decision, person).
    AlreadyVoted,
    /// Idempotency key reused with a different payload.
    IdempotencyConflict,
    Database(sqlx::Error),
}

// ------------------------------------------------------------------
// Read models
// ------------------------------------------------------------------

pub struct BodyFilter {
    pub status: Option<String>,
    pub search: Option<String>,
    pub page: i64,
    pub page_size: i64,
}

#[derive(Debug, sqlx::FromRow)]
pub struct BodyListRow {
    pub id: Uuid,
    pub body_number: i64,
    pub name: String,
    pub body_type: String,
    pub status: String,
    /// Active memberships (ended_at IS NULL) — derived, never stored.
    pub active_members: i64,
    pub created_at: OffsetDateTime,
    pub total_count: i64,
}

/// Server-side list (ADR-010): status/search filters, stable
/// `body_number DESC` ordering.
pub async fn list_bodies(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    filter: &BodyFilter,
) -> Result<Vec<BodyListRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    sqlx::query_as::<_, BodyListRow>(
        "SELECT b.id, b.body_number, b.name, b.body_type, b.status, \
                (SELECT count(*) FROM governance_memberships m \
                 WHERE m.body_id = b.id AND m.ended_at IS NULL) \
                    AS active_members, \
                b.created_at, \
                count(*) OVER () AS total_count \
         FROM governance_bodies b \
         WHERE b.cooperative_id IS NOT DISTINCT FROM $5 \
           AND ($1::text IS NULL OR b.status = $1) \
           AND ($2::text IS NULL OR b.name ILIKE '%' || $2 || '%' \
                OR b.body_type ILIKE '%' || $2 || '%' \
                OR b.body_number::text = $2) \
         ORDER BY b.body_number DESC \
         LIMIT $3 OFFSET $4",
    )
    .bind(&filter.status)
    .bind(&filter.search)
    .bind(filter.page_size)
    .bind((filter.page - 1) * filter.page_size)
    .bind(cooperative_id)
    .fetch_all(&mut *conn)
    .await
}

#[derive(Debug, sqlx::FromRow)]
pub struct BodyRow {
    pub id: Uuid,
    pub body_number: i64,
    pub name: String,
    pub body_type: String,
    pub description: Option<String>,
    pub status: String,
    pub closed_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

const BODY_SELECT: &str = "SELECT id, body_number, name, body_type, description, status, \
            closed_at, created_at, updated_at \
     FROM governance_bodies";

#[derive(Debug, sqlx::FromRow)]
pub struct MembershipRow {
    pub id: Uuid,
    pub body_id: Uuid,
    pub person_id: Uuid,
    pub person_name: String,
    pub title: Option<String>,
    pub started_at: OffsetDateTime,
    pub ended_at: Option<OffsetDateTime>,
    pub ended_by: Option<Uuid>,
    pub end_reason: Option<String>,
    pub created_at: OffsetDateTime,
}

const MEMBERSHIP_SELECT: &str = "SELECT m.id, m.body_id, m.person_id, \
            p.first_name || ' ' || p.last_name AS person_name, \
            m.title, m.started_at, m.ended_at, m.ended_by, m.end_reason, \
            m.created_at \
     FROM governance_memberships m \
     JOIN persons p ON p.id = m.person_id";

pub struct BodyDetail {
    pub body: BodyRow,
    /// Full membership timeline — current AND historical, newest first.
    pub memberships: Vec<MembershipRow>,
}

pub async fn get_body_detail(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    id: Uuid,
) -> Result<Option<BodyDetail>, sqlx::Error> {
    let mut conn = db.conn().await?;
    let body: Option<BodyRow> = sqlx::query_as(&format!(
        "{BODY_SELECT} WHERE id = $1 AND cooperative_id IS NOT DISTINCT FROM $2"
    ))
    .bind(id)
    .bind(cooperative_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(body) = body else {
        return Ok(None);
    };
    let memberships = sqlx::query_as::<_, MembershipRow>(&format!(
        "{MEMBERSHIP_SELECT} WHERE m.body_id = $1 \
         ORDER BY (m.ended_at IS NULL) DESC, m.started_at DESC, m.id"
    ))
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(Some(BodyDetail { body, memberships }))
}

pub struct DecisionFilter {
    pub body_id: Option<Uuid>,
    pub status: Option<String>,
    pub search: Option<String>,
    pub page: i64,
    pub page_size: i64,
}

#[derive(Debug, sqlx::FromRow)]
pub struct DecisionListRow {
    pub id: Uuid,
    pub decision_number: i64,
    pub body_id: Uuid,
    pub body_name: String,
    pub title: String,
    pub status: String,
    pub decision_on: Date,
    pub effective_on: Option<Date>,
    pub vote_count: i64,
    pub created_at: OffsetDateTime,
    pub total_count: i64,
}

/// Server-side list (ADR-010): body/status/search filters, stable
/// `decision_number DESC` ordering.
pub async fn list_decisions(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    filter: &DecisionFilter,
) -> Result<Vec<DecisionListRow>, sqlx::Error> {
    let mut conn = db.conn().await?;
    sqlx::query_as::<_, DecisionListRow>(
        "SELECT d.id, d.decision_number, d.body_id, b.name AS body_name, \
                d.title, d.status, d.decision_on, d.effective_on, \
                (SELECT count(*) FROM governance_votes v \
                 WHERE v.decision_id = d.id) AS vote_count, \
                d.created_at, \
                count(*) OVER () AS total_count \
         FROM governance_decisions d \
         JOIN governance_bodies b ON b.id = d.body_id \
         WHERE d.cooperative_id IS NOT DISTINCT FROM $6 \
           AND ($1::uuid IS NULL OR d.body_id = $1) \
           AND ($2::text IS NULL OR d.status = $2) \
           AND ($3::text IS NULL OR d.title ILIKE '%' || $3 || '%' \
                OR d.decision_number::text = $3) \
         ORDER BY d.decision_number DESC \
         LIMIT $4 OFFSET $5",
    )
    .bind(filter.body_id)
    .bind(&filter.status)
    .bind(&filter.search)
    .bind(filter.page_size)
    .bind((filter.page - 1) * filter.page_size)
    .bind(cooperative_id)
    .fetch_all(&mut *conn)
    .await
}

#[derive(Debug, sqlx::FromRow)]
pub struct DecisionRow {
    pub id: Uuid,
    pub decision_number: i64,
    pub body_id: Uuid,
    pub body_name: String,
    pub title: String,
    pub decision_text: String,
    pub decision_on: Date,
    pub effective_on: Option<Date>,
    pub status: String,
    pub opened_at: Option<OffsetDateTime>,
    pub finalized_at: Option<OffsetDateTime>,
    pub eligible_count: Option<i32>,
    pub approve_count: Option<i32>,
    pub reject_count: Option<i32>,
    pub abstain_count: Option<i32>,
    pub cancelled_at: Option<OffsetDateTime>,
    pub cancellation_reason: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

const DECISION_SELECT: &str = "SELECT d.id, d.decision_number, d.body_id, b.name AS body_name, \
            d.title, d.decision_text, d.decision_on, d.effective_on, \
            d.status, d.opened_at, d.finalized_at, \
            d.eligible_count, d.approve_count, d.reject_count, \
            d.abstain_count, d.cancelled_at, d.cancellation_reason, \
            d.created_at, d.updated_at \
     FROM governance_decisions d \
     JOIN governance_bodies b ON b.id = d.body_id";

#[derive(Debug, sqlx::FromRow)]
pub struct VoteRow {
    pub id: Uuid,
    pub decision_id: Uuid,
    pub membership_id: Uuid,
    pub person_id: Uuid,
    pub person_name: String,
    pub membership_title: Option<String>,
    pub choice: String,
    pub cast_at: OffsetDateTime,
    pub note: Option<String>,
    pub recorded_by: Uuid,
    pub created_at: OffsetDateTime,
}

const VOTE_SELECT: &str = "SELECT v.id, v.decision_id, v.membership_id, v.person_id, \
            p.first_name || ' ' || p.last_name AS person_name, \
            m.title AS membership_title, \
            v.choice, v.cast_at, v.note, v.recorded_by, v.created_at \
     FROM governance_votes v \
     JOIN persons p ON p.id = v.person_id \
     JOIN governance_memberships m ON m.id = v.membership_id";

pub struct DecisionDetail {
    pub decision: DecisionRow,
    /// Recorded votes, cast order.
    pub votes: Vec<VoteRow>,
    /// Currently-active memberships of the body — the electorate
    /// context for an open decision (historical evidence for a
    /// finalized one; the frozen tally snapshot stays authoritative).
    pub eligible_members: Vec<MembershipRow>,
}

pub async fn get_decision_detail(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    id: Uuid,
) -> Result<Option<DecisionDetail>, sqlx::Error> {
    let mut conn = db.conn().await?;
    let decision: Option<DecisionRow> = sqlx::query_as(&format!(
        "{DECISION_SELECT} WHERE d.id = $1 \
         AND d.cooperative_id IS NOT DISTINCT FROM $2"
    ))
    .bind(id)
    .bind(cooperative_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(decision) = decision else {
        return Ok(None);
    };
    let votes = sqlx::query_as::<_, VoteRow>(&format!(
        "{VOTE_SELECT} WHERE v.decision_id = $1 ORDER BY v.cast_at, v.id"
    ))
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    let eligible = sqlx::query_as::<_, MembershipRow>(&format!(
        "{MEMBERSHIP_SELECT} \
         WHERE m.body_id = $1 AND m.ended_at IS NULL \
         ORDER BY m.started_at, m.id"
    ))
    .bind(decision.body_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(Some(DecisionDetail {
        decision,
        votes,
        eligible_members: eligible,
    }))
}

// ------------------------------------------------------------------
// Body commands — identity metadata only, ZERO money.
// ------------------------------------------------------------------

pub struct CreateBody {
    pub name: String,
    pub body_type: String,
    pub description: Option<String>,
    pub idempotency_key: String,
    pub fingerprint: String,
}

pub struct BodyOutcome {
    pub body_id: Uuid,
    pub body_number: i64,
    pub replayed: bool,
}

pub async fn create_body(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    command: CreateBody,
) -> Result<BodyOutcome, GovernanceRepoError> {
    let mut tx = db.begin().await.map_err(GovernanceRepoError::Database)?;
    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, body_number, idempotency_fingerprint \
         FROM governance_bodies WHERE idempotency_key = $1 \
           AND cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(&command.idempotency_key)
    .bind(cooperative_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(GovernanceRepoError::Database)?;
    if let Some((id, number, fingerprint)) = existing {
        if fingerprint != command.fingerprint {
            return Err(GovernanceRepoError::IdempotencyConflict);
        }
        return Ok(BodyOutcome {
            body_id: id,
            body_number: number,
            replayed: true,
        });
    }

    // M1-P3-B6: the allocator runs inside the INSERT's own transaction
    // — a concurrent idempotency loser rolls its allocation back with
    // the tx, so a replay never consumes a document number.
    let inserted: Option<(Uuid, i64)> = sqlx::query_as(
        "INSERT INTO governance_bodies \
            (name, body_type, description, status, \
             idempotency_key, idempotency_fingerprint, created_by, cooperative_id, \
             body_number) \
         VALUES ($1, $2, $3, 'active', $4, $5, $6, $7, \
             allocate_document_number($7, 'governance_bodies.body_number')) \
         ON CONFLICT (cooperative_id, idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING \
         RETURNING id, body_number",
    )
    .bind(&command.name)
    .bind(&command.body_type)
    .bind(&command.description)
    .bind(&command.idempotency_key)
    .bind(&command.fingerprint)
    .bind(actor)
    .bind(cooperative_id)
    .fetch_optional(tx.as_mut())
    .await
    .or_else(|error| {
        if crate::db::is_idempotency_unique_violation(&error) {
            Ok(None)
        } else {
            Err(error)
        }
    })
    .map_err(GovernanceRepoError::Database)?;

    let Some((id, number)) = inserted else {
        let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
            "SELECT id, body_number, idempotency_fingerprint \
             FROM governance_bodies WHERE idempotency_key = $1 \
               AND cooperative_id IS NOT DISTINCT FROM $2",
        )
        .bind(&command.idempotency_key)
        .bind(cooperative_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(GovernanceRepoError::Database)?;
        tx.rollback().await.map_err(GovernanceRepoError::Database)?;
        let Some((id, number, fingerprint)) = existing else {
            return Err(GovernanceRepoError::NotFound);
        };
        if fingerprint != command.fingerprint {
            return Err(GovernanceRepoError::IdempotencyConflict);
        }
        return Ok(BodyOutcome {
            body_id: id,
            body_number: number,
            replayed: true,
        });
    };

    tx.commit().await.map_err(GovernanceRepoError::Database)?;
    Ok(BodyOutcome {
        body_id: id,
        body_number: number,
        replayed: false,
    })
}

/// Body close (active -> closed): allowed only while the body has no
/// draft/open decisions and no active memberships — a body must not be
/// closed leaving stranded open ballots or seats that pretend to be
/// electable. Historical decisions/memberships are untouched.
pub async fn close_body(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    body_id: Uuid,
) -> Result<(), GovernanceRepoError> {
    let mut tx = db.begin().await.map_err(GovernanceRepoError::Database)?;
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM public.app_lock_governance_body($1, $2)")
            .bind(body_id)
            .bind(cooperative_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(GovernanceRepoError::Database)?;
    let Some(status) = status else {
        return Err(GovernanceRepoError::NotFound);
    };
    if status != "active" {
        return Err(GovernanceRepoError::InvalidState);
    }

    let busy: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
            SELECT 1 FROM governance_decisions \
            WHERE body_id = $1 AND status IN ('draft','open') \
         ) OR EXISTS ( \
            SELECT 1 FROM governance_memberships \
            WHERE body_id = $1 AND ended_at IS NULL)",
    )
    .bind(body_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(GovernanceRepoError::Database)?;
    if busy {
        return Err(GovernanceRepoError::BodyBusy);
    }

    sqlx::query("SELECT public.app_governance_body_close($1, $2, $3)")
        .bind(body_id)
        .bind(actor)
        .bind(cooperative_id)
        .execute(&mut *tx)
        .await
        .map_err(GovernanceRepoError::Database)?;
    tx.commit().await.map_err(GovernanceRepoError::Database)
}

// ------------------------------------------------------------------
// Membership commands — temporal Person -> Body link.
// ------------------------------------------------------------------

pub struct CreateMembership {
    pub body_id: Uuid,
    pub person_id: Uuid,
    pub title: Option<String>,
    pub started_at: OffsetDateTime,
    pub idempotency_key: String,
    pub fingerprint: String,
}

pub struct MembershipOutcome {
    pub membership_id: Uuid,
    pub replayed: bool,
}

/// Starts a membership. One ACTIVE membership per (body, person) is
/// enforced by the partial unique index — a concurrent second seat
/// answers ActiveMembershipExists, never a duplicate row.
pub async fn create_membership(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    command: CreateMembership,
) -> Result<MembershipOutcome, GovernanceRepoError> {
    let mut tx = db.begin().await.map_err(GovernanceRepoError::Database)?;
    let existing: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, idempotency_fingerprint \
         FROM governance_memberships WHERE idempotency_key = $1 \
           AND cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(&command.idempotency_key)
    .bind(cooperative_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(GovernanceRepoError::Database)?;
    if let Some((id, fingerprint)) = existing {
        if fingerprint != command.fingerprint {
            return Err(GovernanceRepoError::IdempotencyConflict);
        }
        return Ok(MembershipOutcome {
            membership_id: id,
            replayed: true,
        });
    }

    // Lock the body first: membership creation serializes against body
    // close and keeps the body-active check consistent.
    let body_status: Option<String> =
        sqlx::query_scalar("SELECT status FROM public.app_lock_governance_body($1, $2)")
            .bind(command.body_id)
            .bind(cooperative_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(GovernanceRepoError::Database)?;
    let Some(body_status) = body_status else {
        return Err(GovernanceRepoError::NotFound);
    };
    if body_status != "active" {
        return Err(GovernanceRepoError::InactiveBody);
    }

    let person_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM persons WHERE id = $1 \
         AND cooperative_id IS NOT DISTINCT FROM $2)",
    )
    .bind(command.person_id)
    .bind(cooperative_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(GovernanceRepoError::Database)?;
    if !person_exists {
        return Err(GovernanceRepoError::NotFound);
    }

    // Savepoint: a unique-violation error must not abort the outer tx —
    // the idempotency winner re-read below runs on the same connection.
    let inserted: Option<Uuid> = {
        let mut sp = tx.begin().await.map_err(GovernanceRepoError::Database)?;
        match sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO governance_memberships \
                (body_id, person_id, title, started_at, \
                 idempotency_key, idempotency_fingerprint, created_by, cooperative_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             RETURNING id",
        )
        .bind(command.body_id)
        .bind(command.person_id)
        .bind(&command.title)
        .bind(command.started_at)
        .bind(&command.idempotency_key)
        .bind(&command.fingerprint)
        .bind(actor)
        .bind(cooperative_id)
        .fetch_one(&mut *sp)
        .await
        {
            Ok(id) => {
                sp.commit().await.map_err(GovernanceRepoError::Database)?;
                Some(id)
            }
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
                sp.rollback().await.ok();
                let key = db.constraint().unwrap_or_default().to_string();
                if key.contains("idempotency") {
                    None // concurrent replay — fall through to replay read
                } else {
                    return Err(GovernanceRepoError::ActiveMembershipExists);
                }
            }
            Err(error) => return Err(GovernanceRepoError::Database(error)),
        }
    };

    let Some(membership_id) = inserted else {
        let existing: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT id, idempotency_fingerprint \
             FROM governance_memberships WHERE idempotency_key = $1 \
               AND cooperative_id IS NOT DISTINCT FROM $2",
        )
        .bind(&command.idempotency_key)
        .bind(cooperative_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(GovernanceRepoError::Database)?;
        tx.commit().await.map_err(GovernanceRepoError::Database)?;
        let Some((id, fingerprint)) = existing else {
            return Err(GovernanceRepoError::NotFound);
        };
        if fingerprint != command.fingerprint {
            return Err(GovernanceRepoError::IdempotencyConflict);
        }
        return Ok(MembershipOutcome {
            membership_id: id,
            replayed: true,
        });
    };

    tx.commit().await.map_err(GovernanceRepoError::Database)?;
    Ok(MembershipOutcome {
        membership_id,
        replayed: false,
    })
}

/// Ends a membership term (active -> ended). The membership row lock
/// serializes against a vote cast racing this end — the voter's
/// eligibility check under the decision lock always sees committed
/// membership state. History is preserved: only end markers are set.
pub async fn end_membership(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    membership_id: Uuid,
    ended_at: OffsetDateTime,
    reason: &str,
) -> Result<(), GovernanceRepoError> {
    let mut tx = db.begin().await.map_err(GovernanceRepoError::Database)?;
    let row: Option<(OffsetDateTime, Option<OffsetDateTime>)> = sqlx::query_as(
        "SELECT started_at, ended_at \
         FROM public.app_lock_governance_membership($1, $2)",
    )
    .bind(membership_id)
    .bind(cooperative_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(GovernanceRepoError::Database)?;
    let Some((started_at, existing_end)) = row else {
        return Err(GovernanceRepoError::NotFound);
    };
    if existing_end.is_some() || ended_at <= started_at {
        return Err(GovernanceRepoError::InvalidState);
    }
    sqlx::query("SELECT public.app_governance_membership_end($1, $2, $3, $4, $5)")
        .bind(membership_id)
        .bind(ended_at)
        .bind(actor)
        .bind(reason)
        .bind(cooperative_id)
        .execute(&mut *tx)
        .await
        .map_err(GovernanceRepoError::Database)?;
    tx.commit().await.map_err(GovernanceRepoError::Database)
}

// ------------------------------------------------------------------
// Decision commands — lifecycle draft -> open -> approved|rejected,
// draft -> cancelled. ZERO money (docs/09: evidence only).
// ------------------------------------------------------------------

pub struct CreateDecision {
    pub body_id: Uuid,
    pub title: String,
    pub decision_text: String,
    pub decision_on: Date,
    pub effective_on: Option<Date>,
    pub idempotency_key: String,
    pub fingerprint: String,
}

pub struct DecisionOutcomeRow {
    pub decision_id: Uuid,
    pub decision_number: i64,
    pub replayed: bool,
}

pub async fn create_decision(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    command: CreateDecision,
) -> Result<DecisionOutcomeRow, GovernanceRepoError> {
    let mut tx = db.begin().await.map_err(GovernanceRepoError::Database)?;
    let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, decision_number, idempotency_fingerprint \
         FROM governance_decisions WHERE idempotency_key = $1 \
           AND cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(&command.idempotency_key)
    .bind(cooperative_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(GovernanceRepoError::Database)?;
    if let Some((id, number, fingerprint)) = existing {
        if fingerprint != command.fingerprint {
            return Err(GovernanceRepoError::IdempotencyConflict);
        }
        return Ok(DecisionOutcomeRow {
            decision_id: id,
            decision_number: number,
            replayed: true,
        });
    }

    let body_status: Option<String> =
        sqlx::query_scalar("SELECT status FROM public.app_lock_governance_body($1, $2)")
            .bind(command.body_id)
            .bind(cooperative_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(GovernanceRepoError::Database)?;
    let Some(body_status) = body_status else {
        return Err(GovernanceRepoError::NotFound);
    };
    if body_status != "active" {
        return Err(GovernanceRepoError::InactiveBody);
    }

    // Savepoint: a unique-violation error must not abort the outer tx —
    // the idempotency winner re-read below runs on the same connection.
    let inserted: Option<(Uuid, i64)> = {
        let mut sp = tx.begin().await.map_err(GovernanceRepoError::Database)?;
        match sqlx::query_as::<_, (Uuid, i64)>(
            "INSERT INTO governance_decisions \
                (body_id, title, decision_text, decision_on, effective_on, \
                 status, idempotency_key, idempotency_fingerprint, created_by, cooperative_id, \
                 decision_number) \
             VALUES ($1, $2, $3, $4, $5, 'draft', $6, $7, $8, $9, \
                 allocate_document_number($9, 'governance_decisions.decision_number')) \
             RETURNING id, decision_number",
        )
        .bind(command.body_id)
        .bind(&command.title)
        .bind(&command.decision_text)
        .bind(command.decision_on)
        .bind(command.effective_on)
        .bind(&command.idempotency_key)
        .bind(&command.fingerprint)
        .bind(actor)
        .bind(cooperative_id)
        .fetch_optional(&mut *sp)
        .await
        {
            Ok(row) => {
                sp.commit().await.map_err(GovernanceRepoError::Database)?;
                row
            }
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
                sp.rollback().await.ok();
                None
            }
            Err(error) => return Err(GovernanceRepoError::Database(error)),
        }
    };

    let Some((id, number)) = inserted else {
        let existing: Option<(Uuid, i64, String)> = sqlx::query_as(
            "SELECT id, decision_number, idempotency_fingerprint \
             FROM governance_decisions WHERE idempotency_key = $1 \
               AND cooperative_id IS NOT DISTINCT FROM $2",
        )
        .bind(&command.idempotency_key)
        .bind(cooperative_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(GovernanceRepoError::Database)?;
        tx.rollback().await.ok();
        let Some((id, number, fingerprint)) = existing else {
            return Err(GovernanceRepoError::NotFound);
        };
        if fingerprint != command.fingerprint {
            return Err(GovernanceRepoError::IdempotencyConflict);
        }
        return Ok(DecisionOutcomeRow {
            decision_id: id,
            decision_number: number,
            replayed: true,
        });
    };

    tx.commit().await.map_err(GovernanceRepoError::Database)?;
    Ok(DecisionOutcomeRow {
        decision_id: id,
        decision_number: number,
        replayed: false,
    })
}

/// Draft edit: material content (title, text, dates) may change ONLY
/// while status = 'draft'. Once voting opens the text is frozen so the
/// recorded electorate and history always describe what was voted on.
pub async fn update_decision(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    decision_id: Uuid,
    title: &str,
    decision_text: &str,
    decision_on: Date,
    effective_on: Option<Date>,
) -> Result<(), GovernanceRepoError> {
    let mut tx = db.begin().await.map_err(GovernanceRepoError::Database)?;
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM governance_decisions WHERE id = $1 \
         AND cooperative_id IS NOT DISTINCT FROM $2 FOR UPDATE",
    )
    .bind(decision_id)
    .bind(cooperative_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(GovernanceRepoError::Database)?;
    let Some(status) = status else {
        return Err(GovernanceRepoError::NotFound);
    };
    if status != "draft" {
        return Err(GovernanceRepoError::InvalidState);
    }
    sqlx::query(
        "UPDATE governance_decisions \
         SET title = $2, decision_text = $3, decision_on = $4, \
             effective_on = $5, updated_at = now() \
         WHERE id = $1 AND cooperative_id IS NOT DISTINCT FROM $6",
    )
    .bind(decision_id)
    .bind(title)
    .bind(decision_text)
    .bind(decision_on)
    .bind(effective_on)
    .bind(cooperative_id)
    .execute(&mut *tx)
    .await
    .map_err(GovernanceRepoError::Database)?;
    tx.commit().await.map_err(GovernanceRepoError::Database)
}

/// draft -> open: voting begins. Content freezes from this point.
pub async fn open_decision(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    decision_id: Uuid,
) -> Result<(), GovernanceRepoError> {
    let mut tx = db.begin().await.map_err(GovernanceRepoError::Database)?;
    // R3C: the draft + body-active preconditions live inside the
    // definer — a standalone call can no longer open a decision on a
    // closed body.
    sqlx::query("SELECT public.app_governance_decision_open_tx($1, $2, $3)")
        .bind(decision_id)
        .bind(actor)
        .bind(cooperative_id)
        .execute(&mut *tx)
        .await
        .map_err(|error| match crate::db::package_error_token(&error) {
            Some("not_found") => GovernanceRepoError::NotFound,
            Some("invalid_state") => GovernanceRepoError::InvalidState,
            Some("inactive_body") => GovernanceRepoError::InactiveBody,
            _ => GovernanceRepoError::Database(error),
        })?;
    tx.commit().await.map_err(GovernanceRepoError::Database)
}

/// draft -> cancelled: drafts may be withdrawn with a recorded reason;
/// history is preserved, never deleted.
pub async fn cancel_decision(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    decision_id: Uuid,
    reason: &str,
) -> Result<(), GovernanceRepoError> {
    let mut tx = db.begin().await.map_err(GovernanceRepoError::Database)?;
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM governance_decisions WHERE id = $1 \
         AND cooperative_id IS NOT DISTINCT FROM $2 FOR UPDATE",
    )
    .bind(decision_id)
    .bind(cooperative_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(GovernanceRepoError::Database)?;
    let Some(status) = status else {
        return Err(GovernanceRepoError::NotFound);
    };
    if status != "draft" {
        return Err(GovernanceRepoError::InvalidState);
    }
    sqlx::query("SELECT public.app_governance_decision_cancel($1, $2, $3, $4)")
        .bind(decision_id)
        .bind(actor)
        .bind(reason)
        .bind(cooperative_id)
        .execute(&mut *tx)
        .await
        .map_err(GovernanceRepoError::Database)?;
    tx.commit().await.map_err(GovernanceRepoError::Database)
}

// ------------------------------------------------------------------
// Vote command — one recorded row per (decision, person), immutable.
// ------------------------------------------------------------------

pub struct CastVote {
    pub decision_id: Uuid,
    pub person_id: Uuid,
    pub choice: VoteChoice,
    pub note: Option<String>,
    pub idempotency_key: String,
    pub fingerprint: String,
}

pub struct VoteOutcome {
    pub vote_id: Uuid,
    pub replayed: bool,
}

/// Records a vote. Eligibility is server-side authoritative: the
/// person must hold an ACTIVE membership (started_at <= now,
/// ended_at IS NULL) in the decision's body at cast time. The decision
/// row lock serializes votes against open/finalize; the membership row
/// lock serializes against a concurrent membership end; the unique
/// (decision, person) index makes a double vote impossible.
pub async fn cast_vote(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    command: CastVote,
) -> Result<VoteOutcome, GovernanceRepoError> {
    let mut tx = db.begin().await.map_err(GovernanceRepoError::Database)?;
    let existing: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, idempotency_fingerprint \
         FROM governance_votes WHERE idempotency_key = $1 \
           AND cooperative_id IS NOT DISTINCT FROM $2",
    )
    .bind(&command.idempotency_key)
    .bind(cooperative_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(GovernanceRepoError::Database)?;
    if let Some((id, fingerprint)) = existing {
        if fingerprint != command.fingerprint {
            return Err(GovernanceRepoError::IdempotencyConflict);
        }
        return Ok(VoteOutcome {
            vote_id: id,
            replayed: true,
        });
    }

    // Canonical lock prefix: decision row first, membership row second.
    let decision: Option<(String, Uuid)> = sqlx::query_as(
        "SELECT status, body_id FROM governance_decisions \
         WHERE id = $1 AND cooperative_id IS NOT DISTINCT FROM $2 FOR UPDATE",
    )
    .bind(command.decision_id)
    .bind(cooperative_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(GovernanceRepoError::Database)?;
    let Some((status, body_id)) = decision else {
        return Err(GovernanceRepoError::NotFound);
    };
    if status != "open" {
        return Err(GovernanceRepoError::InvalidState);
    }

    let membership: Option<(Uuid, OffsetDateTime)> = sqlx::query_as(
        "SELECT id, started_at \
         FROM public.app_lock_governance_voter($1, $2, $3)",
    )
    .bind(body_id)
    .bind(command.person_id)
    .bind(cooperative_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(GovernanceRepoError::Database)?;
    let Some((membership_id, started_at)) = membership else {
        return Err(GovernanceRepoError::NotEligible);
    };
    if started_at > OffsetDateTime::now_utc() {
        return Err(GovernanceRepoError::NotEligible);
    }

    // Savepoint: a unique-violation error must not abort the outer tx —
    // the idempotency winner re-read below runs on the same connection.
    let inserted: Option<Uuid> = {
        let mut sp = tx.begin().await.map_err(GovernanceRepoError::Database)?;
        match sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO governance_votes \
                (decision_id, membership_id, person_id, choice, note, \
                 recorded_by, idempotency_key, idempotency_fingerprint, cooperative_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             RETURNING id",
        )
        .bind(command.decision_id)
        .bind(membership_id)
        .bind(command.person_id)
        .bind(command.choice.as_str())
        .bind(&command.note)
        .bind(actor)
        .bind(&command.idempotency_key)
        .bind(&command.fingerprint)
        .bind(cooperative_id)
        .fetch_one(&mut *sp)
        .await
        {
            Ok(id) => {
                sp.commit().await.map_err(GovernanceRepoError::Database)?;
                Some(id)
            }
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
                sp.rollback().await.ok();
                // Several unique indexes may report the loser of a
                // concurrent write (scoped idempotency vs one_per_member)
                // — the reported constraint depends on index order, so the
                // idempotency key is always resolved first below and
                // one_per_member only claims violations with no key match.
                None
            }
            Err(error) => return Err(GovernanceRepoError::Database(error)),
        }
    };

    let Some(vote_id) = inserted else {
        let existing: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT id, idempotency_fingerprint \
             FROM governance_votes WHERE idempotency_key = $1 \
               AND cooperative_id IS NOT DISTINCT FROM $2",
        )
        .bind(&command.idempotency_key)
        .bind(cooperative_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(GovernanceRepoError::Database)?;
        tx.rollback().await.ok();
        if let Some((id, fingerprint)) = existing {
            if fingerprint != command.fingerprint {
                return Err(GovernanceRepoError::IdempotencyConflict);
            }
            return Ok(VoteOutcome {
                vote_id: id,
                replayed: true,
            });
        }
        // No row under this key: the violation was one_per_member —
        // the member already cast a different vote on this decision.
        return Err(GovernanceRepoError::AlreadyVoted);
    };

    tx.commit().await.map_err(GovernanceRepoError::Database)?;
    Ok(VoteOutcome {
        vote_id,
        replayed: false,
    })
}

// ------------------------------------------------------------------
// Finalization — operator-recorded outcome + frozen tally snapshot.
// ------------------------------------------------------------------

pub struct FinalizeDecision {
    pub decision_id: Uuid,
    pub outcome: DecisionOutcome,
}

/// open -> approved|rejected. The outcome is the FORMALLY RECORDED
/// result supplied by the authorized operator — the system computes no
/// quorum/majority arithmetic (docs/09 open decisions). Under the
/// decision row lock it freezes a deterministic evidence snapshot:
/// current electorate size plus the cast-vote tally. Natural
/// idempotency follows the project convention for state transitions:
/// a replayed finalize finds status != 'open' and answers
/// InvalidState (no duplicate finalization side effects).
pub async fn finalize_decision(
    mut db: impl crate::db::DbSource,
    cooperative_id: Option<Uuid>,
    actor: Uuid,
    command: FinalizeDecision,
) -> Result<(), GovernanceRepoError> {
    let mut tx = db.begin().await.map_err(GovernanceRepoError::Database)?;
    // R3C: the evidence snapshot (electorate size + vote tally) is
    // computed INSIDE the definer under the decision row lock — the
    // caller can no longer supply a fabricated tally. A replayed
    // finalize finds status != 'open' and answers InvalidState.
    sqlx::query("SELECT public.app_governance_decision_finalize_tx($1, $2, $3, $4)")
        .bind(command.decision_id)
        .bind(command.outcome.as_str())
        .bind(actor)
        .bind(cooperative_id)
        .execute(&mut *tx)
        .await
        .map_err(|error| match crate::db::package_error_token(&error) {
            Some("not_found") => GovernanceRepoError::NotFound,
            Some("invalid_state") => GovernanceRepoError::InvalidState,
            _ => GovernanceRepoError::Database(error),
        })?;
    tx.commit().await.map_err(GovernanceRepoError::Database)
}
