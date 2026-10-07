//! Investments domain model (STEP-012): identity types, lifecycle and
//! input validation.
//!
//! Four concepts stay separate (docs/08):
//!   IDENTITY            `investments`
//!   ACQUISITION COST    `investment_fundings`    (posted cash legs)
//!   ESTIMATED VALUE     `investment_valuations` (informational, no cash)
//!   CASH RESULT         `investment_incomes` + `investment_disposal_proceeds`
//! No column ever collapses them into one number.

use rust_decimal::Decimal;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::payments::model as payment_model;
use crate::shares::model as share_model;

pub const NAME_MAX_LEN: usize = 140;
pub const DESCRIPTION_MAX_LEN: usize = 500;
pub const LOCATION_MAX_LEN: usize = 200;
pub const REFERENCE_MAX_LEN: usize = 120;
pub const COUNTERPARTY_MAX_LEN: usize = 140;
pub const NOTE_MAX_LEN: usize = 500;
pub const METHOD_MAX_LEN: usize = 140;
pub const SOURCE_MAX_LEN: usize = 140;

#[derive(Debug, PartialEq, Eq)]
pub enum InvestmentError {
    InvalidName,
    InvalidType,
    InvalidDescription,
    InvalidLocation,
    InvalidReference,
    InvalidCounterparty,
    InvalidNote,
    InvalidAmount,
    InvalidDate,
    InvalidOccurredAt,
    InvalidReason,
    InvalidIdempotencyKey,
    InvalidProceeds,
}

/// Type taxonomy is an open decision (docs/08 §open decisions) — the
/// minimal authoritative distinction is Real Estate vs Business
/// Holding (docs/02 canonical names).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvestmentType {
    RealEstate,
    Business,
}

impl InvestmentType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RealEstate => "real_estate",
            Self::Business => "business",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "real_estate" => Some(Self::RealEstate),
            "business" => Some(Self::Business),
            _ => None,
        }
    }
}

/// Investment lifecycle (docs/16: distinguish active ownership from
/// disposed/closed historical state):
///   active -> disposed   (via a posted Disposal — terminal in STEP-012)
///   active -> cancelled  (only while the investment carries no
///                         financial events)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvestmentStatus {
    Active,
    Disposed,
    Cancelled,
}

impl InvestmentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disposed => "disposed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "disposed" => Some(Self::Disposed),
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

/// Valuation lifecycle: recorded -> cancelled. Valuations are
/// informational history — they never produce a movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValuationStatus {
    Recorded,
    Cancelled,
}

impl ValuationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recorded => "recorded",
            Self::Cancelled => "cancelled",
        }
    }
}

fn validate_optional_bounded(
    raw: Option<&str>,
    max_len: usize,
    error: InvestmentError,
) -> Result<Option<String>, InvestmentError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(text) if text.chars().count() <= max_len => Ok(Some(text.to_string())),
        Some(_) => Err(error),
    }
}

pub fn validate_name(raw: &str) -> Result<String, InvestmentError> {
    let name = raw.trim();
    let length = name.chars().count();
    if length == 0 || length > NAME_MAX_LEN {
        return Err(InvestmentError::InvalidName);
    }
    Ok(name.to_string())
}

pub fn validate_description(raw: Option<&str>) -> Result<Option<String>, InvestmentError> {
    validate_optional_bounded(
        raw,
        DESCRIPTION_MAX_LEN,
        InvestmentError::InvalidDescription,
    )
}

pub fn validate_location(raw: Option<&str>) -> Result<Option<String>, InvestmentError> {
    validate_optional_bounded(raw, LOCATION_MAX_LEN, InvestmentError::InvalidLocation)
}

pub fn validate_reference(raw: Option<&str>) -> Result<Option<String>, InvestmentError> {
    validate_optional_bounded(raw, REFERENCE_MAX_LEN, InvestmentError::InvalidReference)
}

pub fn validate_counterparty(raw: Option<&str>) -> Result<Option<String>, InvestmentError> {
    validate_optional_bounded(
        raw,
        COUNTERPARTY_MAX_LEN,
        InvestmentError::InvalidCounterparty,
    )
}

pub fn validate_note(raw: Option<&str>) -> Result<Option<String>, InvestmentError> {
    validate_optional_bounded(raw, NOTE_MAX_LEN, InvestmentError::InvalidNote)
}

pub fn validate_method(raw: Option<&str>) -> Result<Option<String>, InvestmentError> {
    validate_optional_bounded(raw, METHOD_MAX_LEN, InvestmentError::InvalidAmount)
}

pub fn validate_source(raw: Option<&str>) -> Result<Option<String>, InvestmentError> {
    validate_optional_bounded(raw, SOURCE_MAX_LEN, InvestmentError::InvalidAmount)
}

/// Required description on cash-income events (same traceability
/// contract as STEP-010 entries).
pub fn validate_income_description(raw: &str) -> Result<String, InvestmentError> {
    let description = raw.trim();
    let length = description.chars().count();
    if length == 0 || length > DESCRIPTION_MAX_LEN {
        return Err(InvestmentError::InvalidDescription);
    }
    Ok(description.to_string())
}

/// Strictly-positive exact amount (NUMERIC(19,2), ADR-004) for funding
/// legs, valuations, income and proceeds.
pub fn validate_positive_amount(raw: &str) -> Result<Decimal, InvestmentError> {
    let amount = share_model::validate_amount(raw).map_err(|_| InvestmentError::InvalidAmount)?;
    if amount.is_zero() {
        return Err(InvestmentError::InvalidAmount);
    }
    Ok(amount)
}

/// Optional agreed consideration on a disposal — informational
/// metadata; when present it must be strictly positive.
pub fn validate_consideration(raw: Option<&str>) -> Result<Option<Decimal>, InvestmentError> {
    match raw {
        None => Ok(None),
        Some(text) => validate_positive_amount(text).map(Some),
    }
}

/// A business date (disposal / valuation / acquisition) may be
/// backdated but never future-dated.
pub fn validate_business_date(
    date: time::Date,
    today: time::Date,
) -> Result<time::Date, InvestmentError> {
    if date > today {
        return Err(InvestmentError::InvalidDate);
    }
    Ok(date)
}

/// `occurred_at` may be backdated but never future-dated (same rule as
/// every cash event).
pub fn validate_occurred_at(
    occurred_at: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<OffsetDateTime, InvestmentError> {
    if occurred_at > now {
        return Err(InvestmentError::InvalidOccurredAt);
    }
    Ok(occurred_at)
}

pub fn validate_reversal_reason(raw: &str) -> Result<String, InvestmentError> {
    payment_model::validate_reversal_reason(raw).map_err(|_| InvestmentError::InvalidReason)
}

pub fn validate_idempotency_key(raw: &str) -> Result<String, InvestmentError> {
    payment_model::validate_idempotency_key(raw).map_err(|_| InvestmentError::InvalidIdempotencyKey)
}

// ------------------------------------------------------------------
// ADR-006 fingerprints: same key + same payload replays the stored
// record; same key + different payload answers 409.
// ------------------------------------------------------------------

pub struct CreateInvestmentFingerprint {
    pub name: String,
    pub investment_type: InvestmentType,
    pub description: Option<String>,
    pub location: Option<String>,
    pub reference: Option<String>,
    pub counterparty_name: Option<String>,
    pub acquired_at: Option<time::Date>,
}

pub fn create_fingerprint(input: &CreateInvestmentFingerprint) -> String {
    serde_json::json!({
        "name": input.name,
        "investmentType": input.investment_type.as_str(),
        "description": input.description,
        "location": input.location,
        "reference": input.reference,
        "counterpartyName": input.counterparty_name,
        "acquiredAt": input.acquired_at,
    })
    .to_string()
}

pub fn funding_fingerprint(
    investment_id: Uuid,
    financial_account_id: Uuid,
    amount: Decimal,
    occurred_at: OffsetDateTime,
    reference: &Option<String>,
    note: &Option<String>,
) -> String {
    serde_json::json!({
        "investmentId": investment_id,
        "financialAccountId": financial_account_id,
        "amount": payment_model::canonical_amount(amount),
        "occurredAt": occurred_at,
        "reference": reference,
        "note": note,
    })
    .to_string()
}

pub fn valuation_fingerprint(
    investment_id: Uuid,
    valuation_date: time::Date,
    amount: Decimal,
    method: &Option<String>,
    source: &Option<String>,
    note: &Option<String>,
) -> String {
    serde_json::json!({
        "investmentId": investment_id,
        "valuationDate": valuation_date,
        "amount": payment_model::canonical_amount(amount),
        "method": method,
        "source": source,
        "note": note,
    })
    .to_string()
}

pub fn income_fingerprint(
    investment_id: Uuid,
    financial_account_id: Uuid,
    amount: Decimal,
    occurred_at: OffsetDateTime,
    description: &str,
    counterparty: &Option<String>,
    reference_no: &Option<String>,
) -> String {
    serde_json::json!({
        "investmentId": investment_id,
        "financialAccountId": financial_account_id,
        "amount": payment_model::canonical_amount(amount),
        "occurredAt": occurred_at,
        "description": description,
        "counterparty": counterparty,
        "referenceNo": reference_no,
    })
    .to_string()
}

/// Disposal fingerprint covers the event AND every proceeds leg so a
/// same-key replay with altered legs is a 409.
pub fn disposal_fingerprint(
    investment_id: Uuid,
    disposed_at: time::Date,
    consideration: &Option<Decimal>,
    counterparty_name: &Option<String>,
    reference: &Option<String>,
    note: &Option<String>,
    proceeds: &[(Uuid, Decimal, OffsetDateTime, Option<String>)],
) -> String {
    let legs: Vec<serde_json::Value> = proceeds
        .iter()
        .map(|(account, amount, occurred_at, leg_reference)| {
            serde_json::json!({
                "financialAccountId": account,
                "amount": payment_model::canonical_amount(*amount),
                "occurredAt": occurred_at,
                "reference": leg_reference,
            })
        })
        .collect();
    serde_json::json!({
        "investmentId": investment_id,
        "disposedAt": disposed_at,
        "considerationAmount": consideration.map(payment_model::canonical_amount),
        "counterpartyName": counterparty_name,
        "reference": reference,
        "note": note,
        "proceeds": legs,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn investment_type_parsing_is_explicit() {
        assert_eq!(
            InvestmentType::parse("real_estate"),
            Some(InvestmentType::RealEstate)
        );
        assert_eq!(
            InvestmentType::parse("business"),
            Some(InvestmentType::Business)
        );
        assert_eq!(InvestmentType::parse("gold"), None);
    }

    #[test]
    fn status_parsing_is_explicit() {
        assert_eq!(
            InvestmentStatus::parse("active"),
            Some(InvestmentStatus::Active)
        );
        assert_eq!(
            InvestmentStatus::parse("disposed"),
            Some(InvestmentStatus::Disposed)
        );
        assert_eq!(
            InvestmentStatus::parse("cancelled"),
            Some(InvestmentStatus::Cancelled)
        );
        assert_eq!(InvestmentStatus::parse("planned"), None);
    }

    #[test]
    fn positive_amount_rejects_zero() {
        assert_eq!(
            validate_positive_amount("0.00"),
            Err(InvestmentError::InvalidAmount)
        );
        assert_eq!(
            validate_positive_amount("-1.00"),
            Err(InvestmentError::InvalidAmount)
        );
        assert!(validate_positive_amount("2000000.00").is_ok());
    }

    #[test]
    fn consideration_is_optional_but_positive_when_present() {
        assert_eq!(validate_consideration(None), Ok(None));
        assert_eq!(
            validate_consideration(Some("0.00")),
            Err(InvestmentError::InvalidAmount)
        );
        assert_eq!(
            validate_consideration(Some("2500000.00")).unwrap(),
            Some(Decimal::new(250000000, 2))
        );
    }

    #[test]
    fn disposal_fingerprint_covers_legs() {
        let inv = Uuid::from_u128(1);
        let acc = Uuid::from_u128(2);
        let date = time::Date::MIN;
        let now = OffsetDateTime::UNIX_EPOCH;
        let legs = vec![(acc, Decimal::new(100, 0), now, None)];
        let a = disposal_fingerprint(inv, date, &None, &None, &None, &None, &legs);
        let legs_b = vec![(acc, Decimal::new(200, 0), now, None)];
        let b = disposal_fingerprint(inv, date, &None, &None, &None, &None, &legs_b);
        assert_ne!(a, b);
    }
}
