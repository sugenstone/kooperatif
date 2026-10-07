//! Governance domain model (STEP-014, docs/09): governing bodies,
//! temporal memberships, decisions and votes.
//!
//! Four concepts stay separate (docs/09, docs/18):
//!   BODY        `governance_bodies`       (organ identity — configurable type)
//!   MEMBERSHIP  `governance_memberships`  (temporal Person -> Body link)
//!   DECISION    `governance_decisions`    (formal evidence; content freezes
//!                                          when voting opens)
//!   VOTE        `governance_votes`        (one recorded row per member;
//!                                          immutable once cast)
//!
//! Governance is EVIDENCE, not money: no command in this module ever
//! writes account_movements or touches a financial aggregate.
//!
//! Deliberately NOT implemented (docs/09 open decisions — never
//! invented): quorum, majority/pass arithmetic, weighted or share-based
//! voting, proxy, secret ballot, approval thresholds, decision-to-
//! transaction gating, Policy/PolicyVersion engine. The finalized
//! outcome is OPERATOR-RECORDED (Model B) alongside a frozen vote
//! tally snapshot.

use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::payments::model as payment_model;

pub const NAME_MAX_LEN: usize = 140;
pub const BODY_TYPE_MAX_LEN: usize = 80;
pub const DESCRIPTION_MAX_LEN: usize = 500;
pub const TITLE_MAX_LEN: usize = 140;
pub const MEMBERSHIP_TITLE_MAX_LEN: usize = 80;
pub const DECISION_TEXT_MAX_LEN: usize = 4000;
pub const REASON_MAX_LEN: usize = 500;
pub const NOTE_MAX_LEN: usize = 500;

#[derive(Debug, PartialEq, Eq)]
pub enum GovernanceError {
    InvalidName,
    InvalidBodyType,
    InvalidDescription,
    InvalidTitle,
    InvalidDecisionText,
    InvalidMembershipTitle,
    InvalidReason,
    InvalidNote,
    InvalidStartedAt,
    InvalidEndedAt,
    InvalidWindow,
    InvalidIdempotencyKey,
}

/// Body lifecycle: active -> closed. A body closes only while it has
/// no draft/open decisions and no active memberships; closed bodies
/// keep every historical decision and membership (no reopen in
/// STEP-014 — reopening is undefined policy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyStatus {
    Active,
    Closed,
}

impl BodyStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Closed => "closed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }
}

/// Decision lifecycle (docs/16 candidate, narrowed to the transitions
/// the specs actually define):
///   draft -> open -> approved | rejected
///   draft -> cancelled
/// `approved`/`rejected` are the operator-recorded formal outcomes —
/// the system performs NO quorum/majority arithmetic (docs/09 open
/// decisions). `effective_on` is a business date that never flips
/// status by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionStatus {
    Draft,
    Open,
    Approved,
    Rejected,
    Cancelled,
}

impl DecisionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Open => "open",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "draft" => Some(Self::Draft),
            "open" => Some(Self::Open),
            "approved" => Some(Self::Approved),
            "rejected" => Some(Self::Rejected),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// The operator-recorded formal outcome at finalization (Model B).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionOutcome {
    Approved,
    Rejected,
}

impl DecisionOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "approved" => Some(Self::Approved),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }
}

/// Vote choices: approve / reject / abstain — abstention is the only
/// non-binary choice docs/09 lists. Weighted and secret voting remain
/// undefined and are not modeled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoteChoice {
    Approve,
    Reject,
    Abstain,
}

impl VoteChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
            Self::Abstain => "abstain",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "approve" => Some(Self::Approve),
            "reject" => Some(Self::Reject),
            "abstain" => Some(Self::Abstain),
            _ => None,
        }
    }
}

fn validate_required_bounded(
    raw: &str,
    max_len: usize,
    error: GovernanceError,
) -> Result<String, GovernanceError> {
    let value = raw.trim();
    let length = value.chars().count();
    if length == 0 || length > max_len {
        return Err(error);
    }
    Ok(value.to_string())
}

fn validate_optional_bounded(
    raw: Option<&str>,
    max_len: usize,
    error: GovernanceError,
) -> Result<Option<String>, GovernanceError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(text) if text.chars().count() <= max_len => Ok(Some(text.to_string())),
        Some(_) => Err(error),
    }
}

pub fn validate_name(raw: &str) -> Result<String, GovernanceError> {
    validate_required_bounded(raw, NAME_MAX_LEN, GovernanceError::InvalidName)
}

/// Bounded free-text body classification — docs/09 leaves the legal
/// organ catalog configurable, so no fixed enum is invented.
pub fn validate_body_type(raw: &str) -> Result<String, GovernanceError> {
    validate_required_bounded(raw, BODY_TYPE_MAX_LEN, GovernanceError::InvalidBodyType)
}

pub fn validate_description(raw: Option<&str>) -> Result<Option<String>, GovernanceError> {
    validate_optional_bounded(
        raw,
        DESCRIPTION_MAX_LEN,
        GovernanceError::InvalidDescription,
    )
}

/// Decision subject line (docs/09 "subject").
pub fn validate_title(raw: &str) -> Result<String, GovernanceError> {
    validate_required_bounded(raw, TITLE_MAX_LEN, GovernanceError::InvalidTitle)
}

/// Resolution text — the material content voters decide on.
pub fn validate_decision_text(raw: &str) -> Result<String, GovernanceError> {
    validate_required_bounded(
        raw,
        DECISION_TEXT_MAX_LEN,
        GovernanceError::InvalidDecisionText,
    )
}

/// Optional bounded seat label — carries NO powers.
pub fn validate_membership_title(raw: Option<&str>) -> Result<Option<String>, GovernanceError> {
    validate_optional_bounded(
        raw,
        MEMBERSHIP_TITLE_MAX_LEN,
        GovernanceError::InvalidMembershipTitle,
    )
}

pub fn validate_reason(raw: &str) -> Result<String, GovernanceError> {
    validate_required_bounded(raw, REASON_MAX_LEN, GovernanceError::InvalidReason)
}

pub fn validate_optional_reason(raw: Option<&str>) -> Result<Option<String>, GovernanceError> {
    validate_optional_bounded(raw, REASON_MAX_LEN, GovernanceError::InvalidReason)
}

pub fn validate_note(raw: Option<&str>) -> Result<Option<String>, GovernanceError> {
    validate_optional_bounded(raw, NOTE_MAX_LEN, GovernanceError::InvalidNote)
}

/// Membership/vote-adjacent timestamps may be backdated but never
/// future-dated (same rule as every recorded event).
pub fn validate_not_future(
    at: OffsetDateTime,
    now: OffsetDateTime,
    error: GovernanceError,
) -> Result<OffsetDateTime, GovernanceError> {
    if at > now {
        return Err(error);
    }
    Ok(at)
}

/// Optional effective-date pair is not a window rule — `effective_on`
/// may precede or follow `decision_on` (Decision Date != Effective
/// Date is the invariant, not an ordering).
pub fn validate_idempotency_key(raw: &str) -> Result<String, GovernanceError> {
    payment_model::validate_idempotency_key(raw).map_err(|_| GovernanceError::InvalidIdempotencyKey)
}

// ------------------------------------------------------------------
// ADR-006 fingerprints: same key + same payload replays the stored
// record; same key + different payload answers 409.
// ------------------------------------------------------------------

pub fn create_body_fingerprint(
    name: &str,
    body_type: &str,
    description: &Option<String>,
) -> String {
    serde_json::json!({
        "name": name,
        "bodyType": body_type,
        "description": description,
    })
    .to_string()
}

pub fn create_membership_fingerprint(
    body_id: Uuid,
    person_id: Uuid,
    title: &Option<String>,
    started_at: OffsetDateTime,
) -> String {
    serde_json::json!({
        "bodyId": body_id,
        "personId": person_id,
        "title": title,
        "startedAt": started_at,
    })
    .to_string()
}

pub fn end_membership_fingerprint(
    membership_id: Uuid,
    ended_at: OffsetDateTime,
    reason: &str,
) -> String {
    serde_json::json!({
        "membershipId": membership_id,
        "endedAt": ended_at,
        "reason": reason,
    })
    .to_string()
}

pub fn create_decision_fingerprint(
    body_id: Uuid,
    title: &str,
    decision_text: &str,
    decision_on: Date,
    effective_on: Option<Date>,
) -> String {
    serde_json::json!({
        "bodyId": body_id,
        "title": title,
        "decisionText": decision_text,
        "decisionOn": decision_on,
        "effectiveOn": effective_on,
    })
    .to_string()
}

pub fn cast_vote_fingerprint(
    decision_id: Uuid,
    person_id: Uuid,
    choice: VoteChoice,
    note: &Option<String>,
) -> String {
    serde_json::json!({
        "decisionId": decision_id,
        "personId": person_id,
        "choice": choice.as_str(),
        "note": note,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    #[test]
    fn decision_status_parsing_is_explicit() {
        assert_eq!(DecisionStatus::parse("draft"), Some(DecisionStatus::Draft));
        assert_eq!(DecisionStatus::parse("open"), Some(DecisionStatus::Open));
        assert_eq!(
            DecisionStatus::parse("approved"),
            Some(DecisionStatus::Approved)
        );
        assert_eq!(
            DecisionStatus::parse("rejected"),
            Some(DecisionStatus::Rejected)
        );
        assert_eq!(
            DecisionStatus::parse("cancelled"),
            Some(DecisionStatus::Cancelled)
        );
        assert_eq!(DecisionStatus::parse("effective"), None);
        assert_eq!(DecisionStatus::parse("reopened"), None);
    }

    #[test]
    fn outcome_is_only_approved_or_rejected() {
        assert_eq!(
            DecisionOutcome::parse("approved"),
            Some(DecisionOutcome::Approved)
        );
        assert_eq!(
            DecisionOutcome::parse("rejected"),
            Some(DecisionOutcome::Rejected)
        );
        // No invented tie/pass states.
        assert_eq!(DecisionOutcome::parse("tie"), None);
        assert_eq!(DecisionOutcome::parse("quorum_failed"), None);
    }

    #[test]
    fn vote_choice_parsing_is_explicit() {
        assert_eq!(VoteChoice::parse("approve"), Some(VoteChoice::Approve));
        assert_eq!(VoteChoice::parse("reject"), Some(VoteChoice::Reject));
        assert_eq!(VoteChoice::parse("abstain"), Some(VoteChoice::Abstain));
        assert_eq!(VoteChoice::parse("weighted:2"), None);
    }

    #[test]
    fn required_text_is_bounded() {
        assert_eq!(validate_name("  "), Err(GovernanceError::InvalidName));
        assert_eq!(
            validate_name(&"x".repeat(NAME_MAX_LEN + 1)),
            Err(GovernanceError::InvalidName)
        );
        assert_eq!(
            validate_name("  Yönetim Kurulu  ").unwrap(),
            "Yönetim Kurulu"
        );
    }

    #[test]
    fn not_future_rejects_ahead_of_now() {
        let now = OffsetDateTime::UNIX_EPOCH;
        assert!(validate_not_future(now, now, GovernanceError::InvalidStartedAt).is_ok());
        assert_eq!(
            validate_not_future(
                now + Duration::seconds(1),
                now,
                GovernanceError::InvalidStartedAt
            ),
            Err(GovernanceError::InvalidStartedAt)
        );
    }

    #[test]
    fn vote_fingerprint_covers_payload() {
        let decision = Uuid::from_u128(1);
        let person = Uuid::from_u128(2);
        let a = cast_vote_fingerprint(decision, person, VoteChoice::Approve, &None);
        let b = cast_vote_fingerprint(decision, person, VoteChoice::Reject, &None);
        assert_ne!(a, b);
        assert_eq!(
            a,
            cast_vote_fingerprint(decision, person, VoteChoice::Approve, &None)
        );
    }
}
