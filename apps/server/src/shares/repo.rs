//! Shares repository: list/detail queries with full owner identity
//! context (no N+1), and transactional aggregate operations — initial
//! allocation, owner change (transfer/sale) and lifecycle transitions.

use rust_decimal::Decimal;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use super::model::{AcquisitionType, ShareStatus};
use crate::parties::model as party_model;

/// Shareholder identity fragment carried on every share surface
/// (STEP-005 §7/§37): owner name + guardian + current family context.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ShareholderIdentityRow {
    pub shareholder_id: Uuid,
    pub first_name: String,
    pub last_name: String,
    pub guardian_first_name: Option<String>,
    pub guardian_last_name: Option<String>,
    pub family_sequence: Option<i64>,
    pub shareholder_status: String,
}

#[derive(Debug, sqlx::FromRow)]
pub struct ShareListRow {
    pub id: Uuid,
    pub share_number: i64,
    pub status: String,
    pub owner_shareholder_id: Option<Uuid>,
    pub owner_first_name: Option<String>,
    pub owner_last_name: Option<String>,
    pub owner_guardian_first_name: Option<String>,
    pub owner_guardian_last_name: Option<String>,
    pub owner_family_sequence: Option<i64>,
    pub owner_shareholder_status: Option<String>,
    pub acquisition_type: Option<String>,
    pub ownership_started_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub total_count: i64,
}

/// Current-owner identity joins, aliased per usage (list rows, event
/// from/to sides). `owner` prefix; joins the shareholder's CURRENT
/// family membership — historical family-as-of rendering is deferred
/// (§13), the temporal membership table already supports it.
const OWNER_JOINS: &str = "\
LEFT JOIN share_ownerships o ON o.share_id = sh.id AND o.ended_at IS NULL \
LEFT JOIN shareholders os ON os.id = o.shareholder_id \
LEFT JOIN persons op ON op.id = os.person_id \
LEFT JOIN persons ogp ON ogp.id = os.guardian_person_id \
LEFT JOIN shareholder_family_memberships om \
    ON om.shareholder_id = os.id AND om.ended_at IS NULL \
LEFT JOIN families ofm ON ofm.id = om.family_id ";

const SHARE_LIST_SELECT: &str = "SELECT sh.id, sh.share_number, sh.status, \
    os.id AS owner_shareholder_id, \
    op.first_name AS owner_first_name, op.last_name AS owner_last_name, \
    ogp.first_name AS owner_guardian_first_name, ogp.last_name AS owner_guardian_last_name, \
    ofm.sequence_number AS owner_family_sequence, \
    os.status AS owner_shareholder_status, \
    o.acquisition_type, o.started_at AS ownership_started_at, \
    sh.created_at, sh.updated_at, \
    count(*) OVER () AS total_count \
FROM shares sh ";

/// Share list: folded substring over owner name, guardian name, or an
/// exact share/family number when the term is a positive integer.
/// Stable ordering by share number (ADR-010).
pub async fn list_shares(
    pool: &PgPool,
    search: Option<&str>,
    page: i64,
    page_size: i64,
) -> Result<Vec<ShareListRow>, sqlx::Error> {
    let raw = search.map(str::trim).filter(|s| !s.is_empty());
    let folded: Option<String> = raw.map(|term| format!("%{}%", party_model::fold_search(term)));
    let number: Option<i64> = raw
        .and_then(|term| term.parse::<i64>().ok())
        .filter(|n| *n > 0);

    let sql = format!(
        "{SHARE_LIST_SELECT}{OWNER_JOINS} \
         WHERE ( \
            $1::text IS NULL \
            OR op.search_name LIKE $1 OR ogp.search_name LIKE $1 \
            OR ($2::bigint IS NOT NULL AND (sh.share_number = $2 OR ofm.sequence_number = $2)) \
         ) \
         ORDER BY sh.share_number \
         OFFSET $3 ROWS FETCH NEXT $4 ROWS ONLY"
    );
    sqlx::query_as::<_, ShareListRow>(&sql)
        .bind(folded)
        .bind(number)
        .bind((page - 1) * page_size)
        .bind(page_size)
        .fetch_all(pool)
        .await
}

/// Currently held shares of one shareholder (shareholder detail
/// "Hisseler" section; §66). Open intervals only; share status is
/// carried so voided/suspended shares remain distinguishable.
pub async fn list_shares_by_shareholder(
    pool: &PgPool,
    shareholder_id: Uuid,
) -> Result<Vec<ShareListRow>, sqlx::Error> {
    let sql = format!(
        "{SHARE_LIST_SELECT}{OWNER_JOINS} \
         WHERE os.id = $1 ORDER BY sh.share_number"
    );
    sqlx::query_as::<_, ShareListRow>(&sql)
        .bind(shareholder_id)
        .fetch_all(pool)
        .await
}

pub async fn find_share(pool: &PgPool, id: Uuid) -> Result<Option<ShareListRow>, sqlx::Error> {
    let sql = format!("{SHARE_LIST_SELECT}{OWNER_JOINS} WHERE sh.id = $1");
    let mut rows = sqlx::query_as::<_, ShareListRow>(&sql)
        .bind(id)
        .fetch_all(pool)
        .await?;
    Ok(rows.pop())
}

/// One side of a share event (from/to) with full canonical identity.
#[derive(Debug, sqlx::FromRow)]
pub struct ShareEventRow {
    pub id: Uuid,
    pub event_type: String,
    pub occurred_at: OffsetDateTime,
    pub acquisition_type: Option<String>,
    pub amount: Option<Decimal>,
    pub currency: String,
    pub status_from: Option<String>,
    pub status_to: Option<String>,
    pub reason: Option<String>,
    pub from_shareholder_id: Option<Uuid>,
    pub from_first_name: Option<String>,
    pub from_last_name: Option<String>,
    pub from_guardian_first_name: Option<String>,
    pub from_guardian_last_name: Option<String>,
    pub from_family_sequence: Option<i64>,
    pub to_shareholder_id: Option<Uuid>,
    pub to_first_name: Option<String>,
    pub to_last_name: Option<String>,
    pub to_guardian_first_name: Option<String>,
    pub to_guardian_last_name: Option<String>,
    pub to_family_sequence: Option<i64>,
}

/// Append-oriented business history (§53): chronological share events
/// with canonical identity on both sides. Bounded for safety (§77).
pub async fn share_event_history(
    pool: &PgPool,
    share_id: Uuid,
    limit: i64,
) -> Result<Vec<ShareEventRow>, sqlx::Error> {
    sqlx::query_as::<_, ShareEventRow>(
        "SELECT e.id, e.event_type, e.occurred_at, e.acquisition_type, e.amount, e.currency, \
            e.status_from, e.status_to, e.reason, \
            fs.id AS from_shareholder_id, \
            fp.first_name AS from_first_name, fp.last_name AS from_last_name, \
            fgp.first_name AS from_guardian_first_name, fgp.last_name AS from_guardian_last_name, \
            ff.sequence_number AS from_family_sequence, \
            ts.id AS to_shareholder_id, \
            tp.first_name AS to_first_name, tp.last_name AS to_last_name, \
            tgp.first_name AS to_guardian_first_name, tgp.last_name AS to_guardian_last_name, \
            tf.sequence_number AS to_family_sequence \
         FROM share_events e \
         LEFT JOIN shareholders fs ON fs.id = e.from_shareholder_id \
         LEFT JOIN persons fp ON fp.id = fs.person_id \
         LEFT JOIN persons fgp ON fgp.id = fs.guardian_person_id \
         LEFT JOIN shareholder_family_memberships fm \
             ON fm.shareholder_id = fs.id AND fm.ended_at IS NULL \
         LEFT JOIN families ff ON ff.id = fm.family_id \
         LEFT JOIN shareholders ts ON ts.id = e.to_shareholder_id \
         LEFT JOIN persons tp ON tp.id = ts.person_id \
         LEFT JOIN persons tgp ON tgp.id = ts.guardian_person_id \
         LEFT JOIN shareholder_family_memberships tm \
             ON tm.shareholder_id = ts.id AND tm.ended_at IS NULL \
         LEFT JOIN families tf ON tf.id = tm.family_id \
         WHERE e.share_id = $1 \
         ORDER BY e.occurred_at, e.created_at, e.id \
         LIMIT $2",
    )
    .bind(share_id)
    .bind(limit)
    .fetch_all(pool)
    .await
}

// ------------------------------------------------------------------
// Transactional commands
// ------------------------------------------------------------------

pub struct CreateShare {
    pub shareholder_id: Uuid,
    pub acquisition_type: AcquisitionType,
    pub fee: Option<Decimal>,
    pub effective_at: OffsetDateTime,
    pub reason: Option<String>,
}

#[derive(Debug)]
pub enum CreateShareError {
    ShareholderNotFound,
    ShareholderNotActive,
    InvalidAcquisitionType,
    /// Backdating the FIRST allocation into the future is meaningless;
    /// occurred_at must not be in the future.
    FutureEffectiveAt,
    Database(sqlx::Error),
}

/// Create Share + initial ownership + initial_acquisition event in ONE
/// transaction (§62): a Share never exists without a recorded initial
/// acquisition. The shareholder must exist and be `active` (§45).
pub async fn create_share(
    pool: &PgPool,
    actor: Uuid,
    command: CreateShare,
    now: OffsetDateTime,
) -> Result<(Uuid, i64), CreateShareError> {
    if !command.acquisition_type.is_initial() {
        return Err(CreateShareError::InvalidAcquisitionType);
    }
    if command.effective_at > now {
        return Err(CreateShareError::FutureEffectiveAt);
    }

    let mut tx = pool.begin().await.map_err(CreateShareError::Database)?;

    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM shareholders WHERE id = $1 FOR UPDATE")
            .bind(command.shareholder_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(CreateShareError::Database)?;
    let Some(status) = status else {
        return Err(CreateShareError::ShareholderNotFound);
    };
    if status != party_model::ShareholderStatus::Active.as_str() {
        return Err(CreateShareError::ShareholderNotActive);
    }

    let (share_id, share_number): (Uuid, i64) =
        sqlx::query_as("INSERT INTO shares (created_by) VALUES ($1) RETURNING id, share_number")
            .bind(actor)
            .fetch_one(tx.as_mut())
            .await
            .map_err(CreateShareError::Database)?;

    let event_id: Uuid = sqlx::query_scalar(
        "INSERT INTO share_events \
             (share_id, event_type, occurred_at, to_shareholder_id, acquisition_type, \
              amount, currency, reason, actor_user_id) \
         VALUES ($1, 'initial_acquisition', $2, $3, $4, $5, 'TRY', $6, $7) \
         RETURNING id",
    )
    .bind(share_id)
    .bind(command.effective_at)
    .bind(command.shareholder_id)
    .bind(command.acquisition_type.as_str())
    .bind(command.fee)
    .bind(&command.reason)
    .bind(actor)
    .fetch_one(tx.as_mut())
    .await
    .map_err(CreateShareError::Database)?;

    sqlx::query(
        "INSERT INTO share_ownerships \
             (share_id, shareholder_id, started_at, acquisition_type, source_event_id, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(share_id)
    .bind(command.shareholder_id)
    .bind(command.effective_at)
    .bind(command.acquisition_type.as_str())
    .bind(event_id)
    .bind(actor)
    .execute(tx.as_mut())
    .await
    .map_err(CreateShareError::Database)?;

    tx.commit().await.map_err(CreateShareError::Database)?;
    Ok((share_id, share_number))
}

#[derive(Debug)]
pub enum OwnershipChangeError {
    ShareNotFound,
    ShareNotActive,
    /// No active ownership interval exists to close (corrupt state or
    /// voided share) — treated as an invalid transition.
    NoCurrentOwner,
    SameOwner,
    ShareholderNotFound,
    ShareholderNotActive,
    /// Chronology guard (§47): a transfer cannot be dated before the
    /// current ownership interval began — that would rewrite history.
    InvalidEffectiveAt,
    FutureEffectiveAt,
    StaleState,
    Database(sqlx::Error),
}

/// Ownership-change command (transfer/sale share the same mechanics).
pub struct ChangeOwner {
    pub share_id: Uuid,
    pub to_shareholder_id: Uuid,
    pub kind: AcquisitionType,
    pub amount: Option<Decimal>,
    pub reason: Option<String>,
    pub effective_at: OffsetDateTime,
    pub expected_updated_at: OffsetDateTime,
    pub now: OffsetDateTime,
}

/// Transactional owner change (§56): transfer and sale share the same
/// mechanics — close current interval, open the new one, append the
/// business event — differing only in event/ownership type and amount.
/// The share row lock serializes concurrent mutations; the exclusion
/// constraint makes double-ownership impossible under any race.
pub async fn change_owner(
    pool: &PgPool,
    actor: Uuid,
    command: ChangeOwner,
) -> Result<(Uuid, i64), OwnershipChangeError> {
    // (returns: previous owner id, share_number)
    debug_assert!(matches!(
        command.kind,
        AcquisitionType::Transfer | AcquisitionType::Sale
    ));
    if command.effective_at > command.now {
        return Err(OwnershipChangeError::FutureEffectiveAt);
    }

    let mut tx = pool.begin().await.map_err(OwnershipChangeError::Database)?;

    let share: Option<(String, OffsetDateTime)> =
        sqlx::query_as("SELECT status, updated_at FROM shares WHERE id = $1 FOR UPDATE")
            .bind(command.share_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(OwnershipChangeError::Database)?;
    let Some((status, updated_at)) = share else {
        return Err(OwnershipChangeError::ShareNotFound);
    };
    if updated_at != command.expected_updated_at {
        return Err(OwnershipChangeError::StaleState);
    }
    if status != ShareStatus::Active.as_str() {
        return Err(OwnershipChangeError::ShareNotActive);
    }

    let current: Option<(Uuid, Uuid, OffsetDateTime)> = sqlx::query_as(
        "SELECT id, shareholder_id, started_at FROM share_ownerships \
         WHERE share_id = $1 AND ended_at IS NULL FOR UPDATE",
    )
    .bind(command.share_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(OwnershipChangeError::Database)?;
    let Some((ownership_id, from_shareholder_id, started_at)) = current else {
        return Err(OwnershipChangeError::NoCurrentOwner);
    };
    if from_shareholder_id == command.to_shareholder_id {
        return Err(OwnershipChangeError::SameOwner);
    }
    if command.effective_at < started_at {
        return Err(OwnershipChangeError::InvalidEffectiveAt);
    }

    // Target must be an active Shareholder (§45); row lock prevents a
    // concurrent void/status change racing the transfer.
    let target_status: Option<String> =
        sqlx::query_scalar("SELECT status FROM shareholders WHERE id = $1 FOR UPDATE")
            .bind(command.to_shareholder_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(OwnershipChangeError::Database)?;
    let Some(target_status) = target_status else {
        return Err(OwnershipChangeError::ShareholderNotFound);
    };
    if target_status != party_model::ShareholderStatus::Active.as_str() {
        return Err(OwnershipChangeError::ShareholderNotActive);
    }

    let share_number: i64 = sqlx::query_scalar("SELECT share_number FROM shares WHERE id = $1")
        .bind(command.share_id)
        .fetch_one(tx.as_mut())
        .await
        .map_err(OwnershipChangeError::Database)?;

    sqlx::query("UPDATE share_ownerships SET ended_at = $2 WHERE id = $1")
        .bind(ownership_id)
        .bind(command.effective_at)
        .execute(tx.as_mut())
        .await
        .map_err(OwnershipChangeError::Database)?;

    let event_type = command.kind.as_str();
    let event_id: Uuid = sqlx::query_scalar(
        "INSERT INTO share_events \
             (share_id, event_type, occurred_at, from_shareholder_id, to_shareholder_id, \
              amount, currency, reason, actor_user_id) \
         VALUES ($1, $2, $3, $4, $5, $6, 'TRY', $7, $8) \
         RETURNING id",
    )
    .bind(command.share_id)
    .bind(event_type)
    .bind(command.effective_at)
    .bind(from_shareholder_id)
    .bind(command.to_shareholder_id)
    .bind(command.amount)
    .bind(&command.reason)
    .bind(actor)
    .fetch_one(tx.as_mut())
    .await
    .map_err(OwnershipChangeError::Database)?;

    let insert = sqlx::query(
        "INSERT INTO share_ownerships \
             (share_id, shareholder_id, started_at, acquisition_type, source_event_id, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(command.share_id)
    .bind(command.to_shareholder_id)
    .bind(command.effective_at)
    .bind(event_type)
    .bind(event_id)
    .bind(actor)
    .execute(tx.as_mut())
    .await;

    match insert {
        Ok(_) => {}
        Err(sqlx::Error::Database(db_err))
            if db_err
                .constraint()
                .is_some_and(|c| c.contains("no_overlap")) =>
        {
            return Err(OwnershipChangeError::StaleState);
        }
        Err(other) => return Err(OwnershipChangeError::Database(other)),
    }

    sqlx::query("UPDATE shares SET updated_at = $2 WHERE id = $1")
        .bind(command.share_id)
        .bind(command.now)
        .execute(tx.as_mut())
        .await
        .map_err(OwnershipChangeError::Database)?;

    tx.commit().await.map_err(OwnershipChangeError::Database)?;
    Ok((from_shareholder_id, share_number))
}

#[derive(Debug)]
pub enum StatusChangeError {
    ShareNotFound,
    InvalidTransition,
    /// Voiding is only allowed for mistaken records that never went
    /// beyond their initial allocation (§30): no transfer/sale/etc.
    /// events may exist.
    HasHistory,
    Database(sqlx::Error),
}

/// Lifecycle transition in one transaction (docs/16 explicit-command
/// rule). Voiding additionally requires "no downstream history" and
/// closes the initial ownership interval so a voided share keeps no
/// current owner.
pub async fn change_status(
    pool: &PgPool,
    actor: Uuid,
    share_id: Uuid,
    target: ShareStatus,
    reason: Option<String>,
    now: OffsetDateTime,
) -> Result<i64, StatusChangeError> {
    let mut tx = pool.begin().await.map_err(StatusChangeError::Database)?;

    let share: Option<(String, i64)> =
        sqlx::query_as("SELECT status, share_number FROM shares WHERE id = $1 FOR UPDATE")
            .bind(share_id)
            .fetch_optional(tx.as_mut())
            .await
            .map_err(StatusChangeError::Database)?;
    let Some((status, share_number)) = share else {
        return Err(StatusChangeError::ShareNotFound);
    };
    let from_status = ShareStatus::parse(&status).ok_or(StatusChangeError::Database(
        sqlx::Error::Decode("unknown share status".into()),
    ))?;
    if !from_status.can_transition_to(target) {
        return Err(StatusChangeError::InvalidTransition);
    }

    if target == ShareStatus::Voided {
        let later_events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM share_events \
             WHERE share_id = $1 AND event_type <> 'initial_acquisition'",
        )
        .bind(share_id)
        .fetch_one(tx.as_mut())
        .await
        .map_err(StatusChangeError::Database)?;
        if later_events > 0 {
            return Err(StatusChangeError::HasHistory);
        }
        // Close the initial interval: a voided share keeps no owner.
        let owner: Option<Uuid> = sqlx::query_scalar(
            "UPDATE share_ownerships SET ended_at = $2 \
             WHERE share_id = $1 AND ended_at IS NULL RETURNING shareholder_id",
        )
        .bind(share_id)
        .bind(now)
        .fetch_optional(tx.as_mut())
        .await
        .map_err(StatusChangeError::Database)?;

        sqlx::query(
            "INSERT INTO share_events \
                 (share_id, event_type, occurred_at, from_shareholder_id, \
                  status_from, status_to, reason, actor_user_id) \
             VALUES ($1, 'voided', $2, $3, $4, 'voided', $5, $6)",
        )
        .bind(share_id)
        .bind(now)
        .bind(owner)
        .bind(from_status.as_str())
        .bind(&reason)
        .bind(actor)
        .execute(tx.as_mut())
        .await
        .map_err(StatusChangeError::Database)?;
    } else {
        sqlx::query(
            "INSERT INTO share_events \
                 (share_id, event_type, occurred_at, status_from, status_to, reason, actor_user_id) \
             VALUES ($1, 'status_change', $2, $3, $4, $5, $6)",
        )
        .bind(share_id)
        .bind(now)
        .bind(from_status.as_str())
        .bind(target.as_str())
        .bind(&reason)
        .bind(actor)
        .execute(tx.as_mut())
        .await
        .map_err(StatusChangeError::Database)?;
    }

    sqlx::query("UPDATE shares SET status = $2, updated_at = $3 WHERE id = $1")
        .bind(share_id)
        .bind(target.as_str())
        .bind(now)
        .execute(tx.as_mut())
        .await
        .map_err(StatusChangeError::Database)?;

    tx.commit().await.map_err(StatusChangeError::Database)?;
    Ok(share_number)
}
