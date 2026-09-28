//! Parties repository: SQL for persons/shareholders/families with
//! server-side search, offset pagination (small/stable administrative
//! registry per ADR-010; collection screens later move hot lists to
//! cursors), and transactional aggregate operations.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use super::model;

#[derive(Debug, sqlx::FromRow)]
pub struct ShareholderListRow {
    pub id: Uuid,
    pub person_id: Uuid,
    pub first_name: String,
    pub last_name: String,
    pub guardian_person_id: Option<Uuid>,
    pub guardian_first_name: Option<String>,
    pub guardian_last_name: Option<String>,
    pub family_id: Option<Uuid>,
    pub family_sequence: Option<i64>,
    pub membership_started_at: Option<OffsetDateTime>,
    pub status: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub total_count: i64,
}

const SHAREHOLDER_LIST_SELECT: &str = "SELECT s.id, s.person_id, \
    p.first_name, p.last_name, s.guardian_person_id, \
    gp.first_name AS guardian_first_name, gp.last_name AS guardian_last_name, \
    m.family_id, f.sequence_number AS family_sequence, \
    m.started_at AS membership_started_at, s.status, s.created_at, s.updated_at, \
    count(*) OVER () AS total_count \
FROM shareholders s \
JOIN persons p ON p.id = s.person_id \
LEFT JOIN persons gp ON gp.id = s.guardian_person_id \
LEFT JOIN shareholder_family_memberships m \
    ON m.shareholder_id = s.id AND m.ended_at IS NULL \
LEFT JOIN families f ON f.id = m.family_id ";

const SHAREHOLDER_LIST_ORDER: &str =
    " ORDER BY f.sequence_number NULLS LAST, p.search_name, p.last_name, p.first_name, s.id ";

/// Search term semantics: folded substring match against the
/// shareholder's person name, the guardian person name, or an exact
/// family sequence number when the term is a positive integer.
pub async fn list_shareholders(
    pool: &PgPool,
    search: Option<&str>,
    page: i64,
    page_size: i64,
) -> Result<Vec<ShareholderListRow>, sqlx::Error> {
    let mut conditions: Vec<String> = Vec::new();
    let mut folded: Option<String> = None;
    let mut sequence: Option<i64> = None;
    if let Some(raw) = search.map(str::trim).filter(|s| !s.is_empty()) {
        folded = Some(format!("%{}%", model::fold_search(raw)));
        conditions.push("(p.search_name LIKE $1 OR gp.search_name LIKE $1)".to_string());
        if let Ok(number) = raw.parse::<i64>() {
            sequence = Some(number);
            conditions.push("f.sequence_number = $2".to_string());
            // The term may match the name OR the sequence — wrap as OR.
            let last = conditions.len() - 2;
            conditions[last] = format!("(({} OR {}))", conditions[last], conditions[last + 1]);
            conditions.truncate(last + 1);
        }
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    // sqlx cannot bind a dynamically-shaped query: build with a fixed
    // parameter layout ($1 folded, $2 sequence, $3/$4 paging) and pass
    // NULLs for unused dimensions. `LIKE NULL` is NULL → false, and
    // `= NULL` is NULL → false, so unused conditions are inert.
    let sql = format!(
        "{SHAREHOLDER_LIST_SELECT}{where_clause}{SHAREHOLDER_LIST_ORDER} \
         OFFSET $3 ROWS FETCH NEXT $4 ROWS ONLY"
    );
    let rows = sqlx::query_as::<_, ShareholderListRow>(&sql)
        .bind(folded)
        .bind(sequence)
        .bind((page - 1) * page_size)
        .bind(page_size)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// Current members of a family with full identity context (one query).
pub async fn list_shareholders_by_family(
    pool: &PgPool,
    family_id: Uuid,
) -> Result<Vec<ShareholderListRow>, sqlx::Error> {
    let sql = format!(
        "{SHAREHOLDER_LIST_SELECT}          WHERE m.family_id = $1 AND m.ended_at IS NULL{SHAREHOLDER_LIST_ORDER}"
    );
    sqlx::query_as::<_, ShareholderListRow>(&sql)
        .bind(family_id)
        .fetch_all(pool)
        .await
}

pub async fn find_shareholder(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<ShareholderListRow>, sqlx::Error> {
    // Same joins as the list select for a consistent row shape.
    let sql = format!("{SHAREHOLDER_LIST_SELECT} WHERE s.id = $1");
    let mut rows = sqlx::query_as::<_, ShareholderListRow>(&sql)
        .bind(id)
        .fetch_all(pool)
        .await?;
    Ok(rows.pop())
}

#[derive(Debug, sqlx::FromRow)]
pub struct MembershipHistoryRow {
    pub family_id: Uuid,
    pub sequence_number: i64,
    pub started_at: OffsetDateTime,
    pub ended_at: Option<OffsetDateTime>,
    pub reason: Option<String>,
}

pub async fn membership_history(
    pool: &PgPool,
    shareholder_id: Uuid,
) -> Result<Vec<MembershipHistoryRow>, sqlx::Error> {
    sqlx::query_as::<_, MembershipHistoryRow>(
        "SELECT m.family_id, f.sequence_number, m.started_at, m.ended_at, m.reason \
         FROM shareholder_family_memberships m \
         JOIN families f ON f.id = m.family_id \
         WHERE m.shareholder_id = $1 \
         ORDER BY m.started_at, m.id",
    )
    .bind(shareholder_id)
    .fetch_all(pool)
    .await
}

#[derive(Debug, sqlx::FromRow)]
pub struct PersonSearchRow {
    pub id: Uuid,
    pub first_name: String,
    pub last_name: String,
    pub shareholder_id: Option<Uuid>,
    pub shareholder_status: Option<String>,
}

/// Person lookup for guardian/person selection: same-name persons are
/// returned together with shareholder context so the operator can
/// distinguish them. No generic person CRUD module is exposed.
pub async fn search_persons(
    pool: &PgPool,
    search: &str,
    limit: i64,
) -> Result<Vec<PersonSearchRow>, sqlx::Error> {
    let folded = format!("%{}%", model::fold_search(search.trim()));
    sqlx::query_as::<_, PersonSearchRow>(
        "SELECT p.id, p.first_name, p.last_name, s.id AS shareholder_id, s.status AS shareholder_status \
         FROM persons p \
         LEFT JOIN shareholders s ON s.person_id = p.id \
         WHERE p.search_name LIKE $1 \
         ORDER BY p.search_name, p.last_name, p.first_name, p.id \
         LIMIT $2",
    )
    .bind(folded)
    .bind(limit)
    .fetch_all(pool)
    .await
}

#[derive(Debug, sqlx::FromRow)]
pub struct FamilyListRow {
    pub id: Uuid,
    pub sequence_number: i64,
    pub member_count: i64,
    pub total_count: i64,
}

/// Family search: sequence-number prefix or member/guardian name
/// substring (folded).
pub async fn list_families(
    pool: &PgPool,
    search: Option<&str>,
    page: i64,
    page_size: i64,
) -> Result<Vec<FamilyListRow>, sqlx::Error> {
    let raw = search.map(str::trim).filter(|s| !s.is_empty());
    let folded: Option<String> = raw.map(|term| format!("%{}%", model::fold_search(term)));
    let sequence_prefix: Option<String> = raw
        .filter(|term| term.chars().all(|c| c.is_ascii_digit()) && !term.is_empty())
        .map(|term| format!("{term}%"));

    let rows = sqlx::query_as::<_, FamilyListRow>(
        "SELECT f.id, f.sequence_number, \
            count(m.id) FILTER (WHERE m.ended_at IS NULL AND s.status <> 'voided') AS member_count, \
            count(*) OVER () AS total_count \
         FROM families f \
         LEFT JOIN shareholder_family_memberships m ON m.family_id = f.id \
         LEFT JOIN shareholders s ON s.id = m.shareholder_id \
         WHERE ( \
            $1::text IS NULL OR f.sequence_number::text LIKE $1 \
            OR EXISTS ( \
                SELECT 1 FROM shareholder_family_memberships m2 \
                JOIN shareholders s2 ON s2.id = m2.shareholder_id \
                JOIN persons p2 ON p2.id = s2.person_id \
                LEFT JOIN persons g2 ON g2.id = s2.guardian_person_id \
                WHERE m2.family_id = f.id AND m2.ended_at IS NULL \
                  AND (p2.search_name LIKE $2 OR g2.search_name LIKE $2) \
            ) \
         ) \
         GROUP BY f.id, f.sequence_number \
         ORDER BY f.sequence_number \
         OFFSET $3 ROWS FETCH NEXT $4 ROWS ONLY",
    )
    .bind(sequence_prefix)
    .bind(folded)
    .bind((page - 1) * page_size)
    .bind(page_size)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn find_family(pool: &PgPool, id: Uuid) -> Result<Option<FamilyListRow>, sqlx::Error> {
    let mut rows = sqlx::query_as::<_, FamilyListRow>(
        "SELECT f.id, f.sequence_number, \
            count(m.id) FILTER (WHERE m.ended_at IS NULL AND s.status <> 'voided') AS member_count, \
            1::bigint AS total_count \
         FROM families f \
         LEFT JOIN shareholder_family_memberships m ON m.family_id = f.id \
         LEFT JOIN shareholders s ON s.id = m.shareholder_id \
         WHERE f.id = $1 \
         GROUP BY f.id, f.sequence_number",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(rows.pop())
}

pub enum FamilyRef {
    Existing(Uuid),
    New(i64),
}

/// Everything `create_shareholder` needs, resolved transactionally.
pub struct CreateShareholder {
    pub person_first_name: Option<String>,
    pub person_last_name: Option<String>,
    pub existing_person_id: Option<Uuid>,
    pub guardian_person_id: Option<Uuid>,
    pub guardian_first_name: Option<String>,
    pub guardian_last_name: Option<String>,
    pub family: FamilyRef,
}

pub struct CreatedShareholder {
    pub shareholder_id: Uuid,
    pub person_id: Uuid,
    pub guardian_person_id: Option<Uuid>,
    pub family_id: Uuid,
    pub created_family: bool,
    pub created_person: bool,
    pub created_guardian_person: bool,
}

#[derive(Debug)]
pub enum CreateShareholderError {
    DuplicateSequence,
    PersonAlreadyShareholder,
    PersonNotFound,
    GuardianNotFound,
    FamilyNotFound,
    Database(sqlx::Error),
}

/// Create the whole aggregate in ONE transaction (STEP-004 §47):
/// optional new person + optional new guardian person + family
/// (existing or new) + shareholder + initial membership. Any failure
/// (e.g. duplicate family sequence) rolls back everything.
pub async fn create_shareholder(
    pool: &PgPool,
    actor: Uuid,
    command: CreateShareholder,
    now: OffsetDateTime,
) -> Result<CreatedShareholder, CreateShareholderError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(CreateShareholderError::Database)?;

    // 1. Shareholder person: existing or new.
    let (person_id, created_person) = match command.existing_person_id {
        Some(id) => {
            let exists: Option<Uuid> = sqlx::query_scalar("SELECT id FROM persons WHERE id = $1")
                .bind(id)
                .fetch_optional(tx.as_mut())
                .await
                .map_err(CreateShareholderError::Database)?;
            (exists.ok_or(CreateShareholderError::PersonNotFound)?, false)
        }
        None => {
            let first = command
                .person_first_name
                .ok_or(CreateShareholderError::PersonNotFound)?;
            let last = command
                .person_last_name
                .ok_or(CreateShareholderError::PersonNotFound)?;
            let search = model::fold_search(&format!("{first} {last}"));
            let id: Uuid = sqlx::query_scalar(
                "INSERT INTO persons (first_name, last_name, search_name) \
                 VALUES ($1, $2, $3) RETURNING id",
            )
            .bind(&first)
            .bind(&last)
            .bind(&search)
            .fetch_one(tx.as_mut())
            .await
            .map_err(CreateShareholderError::Database)?;
            (id, true)
        }
    };

    // One Person → at most one Shareholder (§46).
    let already: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM shareholders WHERE person_id = $1")
            .bind(person_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(CreateShareholderError::Database)?;
    if already.is_some() {
        return Err(CreateShareholderError::PersonAlreadyShareholder);
    }

    // 2. Guardian person: existing reference or new person (never a
    //    shareholder by itself).
    let guardian_person_id = if let Some(id) = command.guardian_person_id {
        let exists: Option<Uuid> = sqlx::query_scalar("SELECT id FROM persons WHERE id = $1")
            .bind(id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(CreateShareholderError::Database)?;
        Some(exists.ok_or(CreateShareholderError::GuardianNotFound)?)
    } else if let (Some(first), Some(last)) =
        (&command.guardian_first_name, &command.guardian_last_name)
    {
        let search = model::fold_search(&format!("{first} {last}"));
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO persons (first_name, last_name, search_name) \
             VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(first)
        .bind(last)
        .bind(&search)
        .fetch_one(tx.as_mut())
        .await
        .map_err(CreateShareholderError::Database)?;
        Some(id)
    } else {
        None
    };
    let created_guardian_person =
        guardian_person_id.is_some() && command.guardian_person_id.is_none();

    // 3. Family: existing or new (duplicate sequence → transactional 409).
    let (family_id, created_family) = match command.family {
        FamilyRef::Existing(id) => {
            let exists: Option<Uuid> = sqlx::query_scalar("SELECT id FROM families WHERE id = $1")
                .bind(id)
                .fetch_optional(tx.as_mut())
                .await
                .map_err(CreateShareholderError::Database)?;
            (exists.ok_or(CreateShareholderError::FamilyNotFound)?, false)
        }
        FamilyRef::New(sequence) => {
            let id: Result<Uuid, sqlx::Error> = sqlx::query_scalar(
                "INSERT INTO families (sequence_number) VALUES ($1) RETURNING id",
            )
            .bind(sequence)
            .fetch_one(tx.as_mut())
            .await;
            match id {
                Ok(id) => (id, true),
                Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
                    return Err(CreateShareholderError::DuplicateSequence);
                }
                Err(other) => return Err(CreateShareholderError::Database(other)),
            }
        }
    };

    // 4. Shareholder + initial membership.
    let shareholder_id: Uuid = sqlx::query_scalar(
        "INSERT INTO shareholders (person_id, guardian_person_id) \
         VALUES ($1, $2) RETURNING id",
    )
    .bind(person_id)
    .bind(guardian_person_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(CreateShareholderError::Database)?;

    sqlx::query(
        "INSERT INTO shareholder_family_memberships \
             (shareholder_id, family_id, started_at, created_by) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(shareholder_id)
    .bind(family_id)
    .bind(now)
    .bind(actor)
    .execute(tx.as_mut())
    .await
    .map_err(CreateShareholderError::Database)?;

    tx.commit()
        .await
        .map_err(CreateShareholderError::Database)?;

    Ok(CreatedShareholder {
        shareholder_id,
        person_id,
        guardian_person_id,
        family_id,
        created_family,
        created_person,
        created_guardian_person,
    })
}

/// A pre-resolved family target for family changes.
pub enum FamilyChangeTarget {
    Existing(Uuid),
    New(i64),
}

#[derive(Debug)]
pub enum FamilyChangeError {
    ShareholderNotFound,
    FamilyNotFound,
    DuplicateSequence,
    /// Optimistic-concurrency precondition failed (lost-update guard).
    StaleState,
    Database(sqlx::Error),
}

/// Transactional family transfer (STEP-004 amendment): close the
/// current membership, open the new one (creating the destination
/// family if requested) — one transaction, audited by the caller after
/// commit. The exclusion constraint guarantees no shareholder ever has
/// two active or overlapping memberships, under any concurrency
/// (concurrent transfers serialize on the insert and one aborts).
pub async fn change_family(
    pool: &PgPool,
    actor: Uuid,
    shareholder_id: Uuid,
    target: FamilyChangeTarget,
    reason: Option<&str>,
    expected_updated_at: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<(Uuid, Option<i64>, i64), FamilyChangeError> {
    // (returns: new_family_id, old_family_sequence, new_family_sequence)
    let mut tx = pool.begin().await.map_err(FamilyChangeError::Database)?;

    // Lock the shareholder row against concurrent edits (lost-update
    // guard combined with the updated_at precondition).
    let current: Option<(Uuid, Option<Uuid>, OffsetDateTime)> = sqlx::query_as(
        "SELECT s.id, s.guardian_person_id, s.updated_at \
         FROM shareholders s WHERE s.id = $1 FOR UPDATE",
    )
    .bind(shareholder_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(FamilyChangeError::Database)?;
    let Some((_, _, updated_at)) = current else {
        return Err(FamilyChangeError::ShareholderNotFound);
    };
    if updated_at != expected_updated_at {
        return Err(FamilyChangeError::StaleState);
    }

    // Resolve destination family.
    let (new_family_id, new_sequence) = match target {
        FamilyChangeTarget::Existing(id) => {
            let row: Option<(Uuid, i64)> =
                sqlx::query_as("SELECT id, sequence_number FROM families WHERE id = $1")
                    .bind(id)
                    .fetch_optional(tx.as_mut())
                    .await
                    .map_err(FamilyChangeError::Database)?;
            row.ok_or(FamilyChangeError::FamilyNotFound)?
        }
        FamilyChangeTarget::New(sequence) => {
            let id: Result<Uuid, sqlx::Error> = sqlx::query_scalar(
                "INSERT INTO families (sequence_number) VALUES ($1) RETURNING id",
            )
            .bind(sequence)
            .fetch_one(tx.as_mut())
            .await;
            match id {
                Ok(id) => (id, sequence),
                Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
                    return Err(FamilyChangeError::DuplicateSequence);
                }
                Err(other) => return Err(FamilyChangeError::Database(other)),
            }
        }
    };

    // Current membership (may be None: shareholder without a family,
    // e.g. after historical migration).
    let current_membership: Option<(Uuid, i64)> = sqlx::query_as(
        "SELECT m.family_id, f.sequence_number \
         FROM shareholder_family_memberships m \
         JOIN families f ON f.id = m.family_id \
         WHERE m.shareholder_id = $1 AND m.ended_at IS NULL \
         FOR UPDATE OF m",
    )
    .bind(shareholder_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(FamilyChangeError::Database)?;

    let old_sequence = current_membership.as_ref().map(|(_, seq)| *seq);

    // Close current + open new within the same transaction.
    if let Some((old_family_id, _)) = current_membership {
        sqlx::query(
            "UPDATE shareholder_family_memberships \
             SET ended_at = $3, reason = COALESCE($4, reason) \
             WHERE shareholder_id = $1 AND family_id = $2 AND ended_at IS NULL",
        )
        .bind(shareholder_id)
        .bind(old_family_id)
        .bind(now)
        .bind(reason)
        .execute(tx.as_mut())
        .await
        .map_err(FamilyChangeError::Database)?;
    }

    let insert = sqlx::query(
        "INSERT INTO shareholder_family_memberships \
             (shareholder_id, family_id, started_at, reason, created_by) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(shareholder_id)
    .bind(new_family_id)
    .bind(now)
    .bind(reason)
    .bind(actor)
    .execute(tx.as_mut())
    .await;

    match insert {
        Ok(_) => {}
        // The exclusion constraint fires if a concurrent transfer
        // already opened a membership covering `now`.
        Err(sqlx::Error::Database(db_err))
            if db_err
                .constraint()
                .is_some_and(|c| c.contains("no_overlap")) =>
        {
            return Err(FamilyChangeError::StaleState);
        }
        Err(other) => return Err(FamilyChangeError::Database(other)),
    }

    sqlx::query("UPDATE shareholders SET updated_at = $2 WHERE id = $1")
        .bind(shareholder_id)
        .bind(now)
        .execute(tx.as_mut())
        .await
        .map_err(FamilyChangeError::Database)?;

    tx.commit().await.map_err(FamilyChangeError::Database)?;

    Ok((new_family_id, old_sequence, new_sequence))
}

/// Duplicate-candidates for the create flow: same-name shareholders
/// with identity context (§29). Never blocks; the operator confirms.
pub async fn duplicate_candidates(
    pool: &PgPool,
    first_name: &str,
    last_name: &str,
) -> Result<Vec<ShareholderListRow>, sqlx::Error> {
    let folded = format!(
        "%{}%",
        model::fold_search(&format!("{} {}", first_name.trim(), last_name.trim()))
    );
    let sql = format!(
        "{SHAREHOLDER_LIST_SELECT}          WHERE p.search_name LIKE $1          {SHAREHOLDER_LIST_ORDER} LIMIT 20"
    );
    sqlx::query_as::<_, ShareholderListRow>(&sql)
        .bind(folded)
        .fetch_all(pool)
        .await
}
