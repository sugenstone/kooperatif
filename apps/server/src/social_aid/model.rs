//! Social Aid domain model (STEP-013): Fund identity, donation and aid
//! disbursement lifecycles, input validation, ADR-006 fingerprints.
//!
//! Three concepts stay separate (docs/11):
//!   FUND          `social_aid_funds`          (purpose/restriction identity — no balance)
//!   DONATION      `social_aid_donations`      (inflow + restricted credit)
//!   DISBURSEMENT  `social_aid_disbursements`  (outflow to a beneficiary)
//! Restricted availability is ALWAYS derived: sum(posted donations) -
//! sum(posted disbursements) per (fund, financial_account) — never a
//! stored number.

use rust_decimal::Decimal;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::payments::model as payment_model;
use crate::shares::model as share_model;

pub const NAME_MAX_LEN: usize = 140;
pub const DESCRIPTION_MAX_LEN: usize = 500;
pub const REFERENCE_MAX_LEN: usize = 120;
pub const NOTE_MAX_LEN: usize = 500;
pub const IDENTITY_NAME_MAX_LEN: usize = 140;
pub const REASON_MAX_LEN: usize = 500;

#[derive(Debug, PartialEq, Eq)]
pub enum SocialAidError {
    InvalidName,
    InvalidDescription,
    InvalidReference,
    InvalidNote,
    InvalidIdentityName,
    InvalidReason,
    InvalidAmount,
    InvalidOccurredAt,
    InvalidWindow,
    InvalidIdempotencyKey,
}

/// Fund lifecycle (docs/16 candidate states, constrained by docs/11):
///   active -> closed     (only while EVERY (fund, account) restricted
///                         availability is zero — restricted money must
///                         never become ownerless)
///   active -> cancelled  (only while the fund carries no financial
///                         events at all)
/// Terminal: no reopen transition exists in STEP-013 — reopening a
/// closed fund is a governance/policy question deferred with the
/// reclassification rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FundStatus {
    Active,
    Closed,
    Cancelled,
}

impl FundStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Closed => "closed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "closed" => Some(Self::Closed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// Posted financial event lifecycle (docs/19): posted -> reversed,
/// original row preserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostedStatus {
    Posted,
    Reversed,
}

impl PostedStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Posted => "posted",
            Self::Reversed => "reversed",
        }
    }
}

fn validate_optional_bounded(
    raw: Option<&str>,
    max_len: usize,
    error: SocialAidError,
) -> Result<Option<String>, SocialAidError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(text) if text.chars().count() <= max_len => Ok(Some(text.to_string())),
        Some(_) => Err(error),
    }
}

pub fn validate_name(raw: &str) -> Result<String, SocialAidError> {
    let name = raw.trim();
    let length = name.chars().count();
    if length == 0 || length > NAME_MAX_LEN {
        return Err(SocialAidError::InvalidName);
    }
    Ok(name.to_string())
}

pub fn validate_description(raw: Option<&str>) -> Result<Option<String>, SocialAidError> {
    validate_optional_bounded(raw, DESCRIPTION_MAX_LEN, SocialAidError::InvalidDescription)
}

pub fn validate_reference(raw: Option<&str>) -> Result<Option<String>, SocialAidError> {
    validate_optional_bounded(raw, REFERENCE_MAX_LEN, SocialAidError::InvalidReference)
}

pub fn validate_note(raw: Option<&str>) -> Result<Option<String>, SocialAidError> {
    validate_optional_bounded(raw, NOTE_MAX_LEN, SocialAidError::InvalidNote)
}

/// Bounded display name for an external person or organization
/// supporter/beneficiary without a `persons` link (docs/02 controlled
/// free-text fallback). Anonymous recording is an open decision in
/// docs/11 and is NOT implemented — at least one identity is required
/// (enforced in routes + DB CHECK).
pub fn validate_identity_name(raw: Option<&str>) -> Result<Option<String>, SocialAidError> {
    validate_optional_bounded(
        raw,
        IDENTITY_NAME_MAX_LEN,
        SocialAidError::InvalidIdentityName,
    )
}

/// Required reason on an aid disbursement — answers why the aid was
/// granted (the approval/decision pipeline itself is deferred; docs/11
/// only requires a request/decision reference "where required").
pub fn validate_reason(raw: &str) -> Result<String, SocialAidError> {
    let reason = raw.trim();
    let length = reason.chars().count();
    if length == 0 || length > REASON_MAX_LEN {
        return Err(SocialAidError::InvalidReason);
    }
    Ok(reason.to_string())
}

/// Strictly-positive exact amount (NUMERIC(19,2), ADR-004).
pub fn validate_positive_amount(raw: &str) -> Result<Decimal, SocialAidError> {
    let amount = share_model::validate_amount(raw).map_err(|_| SocialAidError::InvalidAmount)?;
    if amount.is_zero() {
        return Err(SocialAidError::InvalidAmount);
    }
    Ok(amount)
}

/// `occurred_at` may be backdated but never future-dated (same rule as
/// every cash event).
pub fn validate_occurred_at(
    occurred_at: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<OffsetDateTime, SocialAidError> {
    if occurred_at > now {
        return Err(SocialAidError::InvalidOccurredAt);
    }
    Ok(occurred_at)
}

/// Optional program window: if both bounds exist, ends_on >= starts_on.
pub fn validate_window(
    starts_on: Option<time::Date>,
    ends_on: Option<time::Date>,
) -> Result<(Option<time::Date>, Option<time::Date>), SocialAidError> {
    if let (Some(start), Some(end)) = (starts_on, ends_on) {
        if end < start {
            return Err(SocialAidError::InvalidWindow);
        }
    }
    Ok((starts_on, ends_on))
}

pub fn validate_reversal_reason(raw: &str) -> Result<String, SocialAidError> {
    payment_model::validate_reversal_reason(raw).map_err(|_| SocialAidError::InvalidReason)
}

pub fn validate_idempotency_key(raw: &str) -> Result<String, SocialAidError> {
    payment_model::validate_idempotency_key(raw).map_err(|_| SocialAidError::InvalidIdempotencyKey)
}

// ------------------------------------------------------------------
// ADR-006 fingerprints: same key + same payload replays the stored
// record; same key + different payload answers 409.
// ------------------------------------------------------------------

pub fn create_fund_fingerprint(
    name: &str,
    description: &Option<String>,
    starts_on: Option<time::Date>,
    ends_on: Option<time::Date>,
) -> String {
    serde_json::json!({
        "name": name,
        "description": description,
        "startsOn": starts_on,
        "endsOn": ends_on,
    })
    .to_string()
}

pub struct DonationFingerprint {
    pub fund_id: Uuid,
    pub donor_person_id: Option<Uuid>,
    pub donor_display_name: Option<String>,
    pub financial_account_id: Uuid,
    pub amount: Decimal,
    pub occurred_at: OffsetDateTime,
    pub reference: Option<String>,
    pub note: Option<String>,
}

pub fn donation_fingerprint(input: &DonationFingerprint) -> String {
    serde_json::json!({
        "fundId": input.fund_id,
        "donorPersonId": input.donor_person_id,
        "donorDisplayName": input.donor_display_name,
        "financialAccountId": input.financial_account_id,
        "amount": payment_model::canonical_amount(input.amount),
        "occurredAt": input.occurred_at,
        "reference": input.reference,
        "note": input.note,
    })
    .to_string()
}

pub struct DisbursementFingerprint {
    pub fund_id: Uuid,
    pub beneficiary_person_id: Option<Uuid>,
    pub beneficiary_display_name: Option<String>,
    pub financial_account_id: Uuid,
    pub amount: Decimal,
    pub occurred_at: OffsetDateTime,
    pub reason: String,
    pub reference: Option<String>,
}

pub fn disbursement_fingerprint(input: &DisbursementFingerprint) -> String {
    serde_json::json!({
        "fundId": input.fund_id,
        "beneficiaryPersonId": input.beneficiary_person_id,
        "beneficiaryDisplayName": input.beneficiary_display_name,
        "financialAccountId": input.financial_account_id,
        "amount": payment_model::canonical_amount(input.amount),
        "occurredAt": input.occurred_at,
        "reason": input.reason,
        "reference": input.reference,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    #[test]
    fn fund_status_parsing_is_explicit() {
        assert_eq!(FundStatus::parse("active"), Some(FundStatus::Active));
        assert_eq!(FundStatus::parse("closed"), Some(FundStatus::Closed));
        assert_eq!(FundStatus::parse("cancelled"), Some(FundStatus::Cancelled));
        assert_eq!(FundStatus::parse("reopened"), None);
    }

    #[test]
    fn identity_name_is_bounded_optional() {
        assert_eq!(validate_identity_name(None), Ok(None));
        assert_eq!(validate_identity_name(Some("  ")), Ok(None));
        assert_eq!(
            validate_identity_name(Some("  Hayırsever Ltd.  ")).unwrap(),
            Some("Hayırsever Ltd.".to_string())
        );
        assert_eq!(
            validate_identity_name(Some(&"x".repeat(IDENTITY_NAME_MAX_LEN + 1))),
            Err(SocialAidError::InvalidIdentityName)
        );
    }

    #[test]
    fn reason_is_required_and_bounded() {
        assert_eq!(validate_reason("   "), Err(SocialAidError::InvalidReason));
        assert_eq!(
            validate_reason(&"x".repeat(REASON_MAX_LEN + 1)),
            Err(SocialAidError::InvalidReason)
        );
        assert_eq!(
            validate_reason("  Eğitim bursu ödemesi  ").unwrap(),
            "Eğitim bursu ödemesi"
        );
    }

    #[test]
    fn positive_amount_rejects_zero() {
        assert_eq!(
            validate_positive_amount("0.00"),
            Err(SocialAidError::InvalidAmount)
        );
        assert_eq!(
            validate_positive_amount("-10.00"),
            Err(SocialAidError::InvalidAmount)
        );
        assert!(validate_positive_amount("1500.50").is_ok());
    }

    #[test]
    fn occurred_at_rejects_the_future() {
        let now = OffsetDateTime::UNIX_EPOCH;
        assert!(validate_occurred_at(now, now).is_ok());
        assert_eq!(
            validate_occurred_at(now + Duration::seconds(1), now),
            Err(SocialAidError::InvalidOccurredAt)
        );
    }

    #[test]
    fn window_rejects_inverted_bounds() {
        let a = time::Date::from_calendar_date(2025, time::Month::January, 1).unwrap();
        let b = time::Date::from_calendar_date(2024, time::Month::January, 1).unwrap();
        assert_eq!(
            validate_window(Some(a), Some(b)),
            Err(SocialAidError::InvalidWindow)
        );
        assert!(validate_window(Some(b), Some(a)).is_ok());
        assert!(validate_window(None, Some(a)).is_ok());
    }

    #[test]
    fn donation_fingerprint_covers_payload() {
        let fund = Uuid::from_u128(1);
        let account = Uuid::from_u128(2);
        let now = OffsetDateTime::UNIX_EPOCH;
        let base = DonationFingerprint {
            fund_id: fund,
            donor_person_id: None,
            donor_display_name: None,
            financial_account_id: account,
            amount: Decimal::new(100, 0),
            occurred_at: now,
            reference: None,
            note: None,
        };
        let different = DonationFingerprint {
            amount: Decimal::new(200, 0),
            ..DonationFingerprint {
                fund_id: fund,
                donor_person_id: None,
                donor_display_name: None,
                financial_account_id: account,
                amount: Decimal::new(100, 0),
                occurred_at: now,
                reference: None,
                note: None,
            }
        };
        assert_ne!(
            donation_fingerprint(&base),
            donation_fingerprint(&different)
        );
        assert_eq!(donation_fingerprint(&base), donation_fingerprint(&base));
    }
}
