//! Income/Expense domain model (STEP-010, docs/07): entry kinds,
//! category types, lifecycle and input validation.
//!
//! Every posted entry produces exactly ONE Account Movement of the
//! matching direction (`income` -> inflow, `expense` -> outflow). The
//! entry records WHY the movement happened; the movement remains the
//! authoritative money record (ADR-003).

use rust_decimal::Decimal;
use time::OffsetDateTime;

use crate::financial_accounts::model as account_model;
use crate::payments::model as payment_model;
use crate::shares::model as share_model;

pub const DESCRIPTION_MAX_LEN: usize = 500;
pub const COUNTERPARTY_MAX_LEN: usize = 200;
pub const REFERENCE_NO_MAX_LEN: usize = 120;

#[derive(Debug, PartialEq, Eq)]
pub enum IncomeExpenseError {
    InvalidCategoryType,
    InvalidName,
    InvalidDescription,
    InvalidCounterparty,
    InvalidReferenceNo,
    InvalidAmount,
    InvalidOccurredAt,
    InvalidIdempotencyKey,
    InvalidReason,
    InvalidEntryStatus,
}

/// Category kind — the DB binds category type to entry kind through a
/// composite FK, so a wrong-typed category is impossible (not merely
/// rejected) once written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CategoryType {
    Income,
    Expense,
}

impl CategoryType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Income => "income",
            Self::Expense => "expense",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "income" => Some(Self::Income),
            "expense" => Some(Self::Expense),
            _ => None,
        }
    }
}

/// Category lifecycle: `active <-> inactive`. An inactive category
/// keeps its history and remains interpretable on old entries — it
/// only refuses NEW postings (docs/19 historical integrity).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CategoryStatus {
    Active,
    Inactive,
}

impl CategoryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "inactive" => Some(Self::Inactive),
            _ => None,
        }
    }

    pub fn can_transition_to(self, target: Self) -> bool {
        self != target
    }
}

/// Entry lifecycle: `posted -> reversed`, terminal — same
/// status-based correction model as Payments and Transfers (docs/19).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryStatus {
    Posted,
    Reversed,
}

impl EntryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Posted => "posted",
            Self::Reversed => "reversed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "posted" => Some(Self::Posted),
            "reversed" => Some(Self::Reversed),
            _ => None,
        }
    }

    pub fn can_transition_to(self, target: Self) -> bool {
        matches!((self, target), (Self::Posted, Self::Reversed))
    }
}

/// Which entry table a command targets — the repo engine is shared;
/// the kind decides table, category type and movement direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Income,
    Expense,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Income => "income",
            Self::Expense => "expense",
        }
    }

    pub fn category_type(self) -> CategoryType {
        match self {
            Self::Income => CategoryType::Income,
            Self::Expense => CategoryType::Expense,
        }
    }

    /// The ONE movement direction this kind always produces.
    pub fn movement_direction(self) -> account_model::MovementDirection {
        match self {
            Self::Income => account_model::MovementDirection::Inflow,
            Self::Expense => account_model::MovementDirection::Outflow,
        }
    }

    /// Movement provenance tag (docs/07 §movements).
    pub fn movement_source(self) -> account_model::MovementSource {
        match self {
            Self::Income => account_model::MovementSource::Income,
            Self::Expense => account_model::MovementSource::Expense,
        }
    }
}

pub fn validate_name(raw: &str) -> Result<String, IncomeExpenseError> {
    account_model::validate_name(raw).map_err(|_| IncomeExpenseError::InvalidName)
}

/// Required description for entries (the business "why").
pub fn validate_entry_description(raw: &str) -> Result<String, IncomeExpenseError> {
    let description = raw.trim();
    let length = description.chars().count();
    if length == 0 || length > DESCRIPTION_MAX_LEN {
        return Err(IncomeExpenseError::InvalidDescription);
    }
    Ok(description.to_string())
}

/// Optional free-form category description.
pub fn validate_description(raw: Option<&str>) -> Result<Option<String>, IncomeExpenseError> {
    account_model::validate_description(raw).map_err(|_| IncomeExpenseError::InvalidDescription)
}

/// Optional descriptive counterparty label (income source / expense
/// payee). Plain text only — it never creates Person/Shareholder
/// semantics (docs/01; STEP-010 scope).
pub fn validate_counterparty(raw: Option<&str>) -> Result<Option<String>, IncomeExpenseError> {
    match raw {
        None => Ok(None),
        Some(value) => {
            let counterparty = value.trim();
            if counterparty.is_empty() {
                return Ok(None);
            }
            if counterparty.chars().count() > COUNTERPARTY_MAX_LEN {
                return Err(IncomeExpenseError::InvalidCounterparty);
            }
            Ok(Some(counterparty.to_string()))
        }
    }
}

/// Optional document/reference label ("Fatura No: ABC-123").
pub fn validate_reference_no(raw: Option<&str>) -> Result<Option<String>, IncomeExpenseError> {
    match raw {
        None => Ok(None),
        Some(value) => {
            let reference = value.trim();
            if reference.is_empty() {
                return Ok(None);
            }
            if reference.chars().count() > REFERENCE_NO_MAX_LEN {
                return Err(IncomeExpenseError::InvalidReferenceNo);
            }
            Ok(Some(reference.to_string()))
        }
    }
}

/// Strictly-positive exact amount (NUMERIC(19,2) contract, ADR-004).
pub fn validate_positive_amount(raw: &str) -> Result<Decimal, IncomeExpenseError> {
    let amount =
        share_model::validate_amount(raw).map_err(|_| IncomeExpenseError::InvalidAmount)?;
    if amount.is_zero() {
        return Err(IncomeExpenseError::InvalidAmount);
    }
    Ok(amount)
}

/// Backdating allowed; future business time rejected (same rule as
/// transfers).
pub fn validate_occurred_at(
    occurred_at: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<OffsetDateTime, IncomeExpenseError> {
    account_model::validate_occurred_at(occurred_at, now)
        .map_err(|_| IncomeExpenseError::InvalidOccurredAt)
}

pub fn validate_reversal_reason(raw: &str) -> Result<String, IncomeExpenseError> {
    payment_model::validate_reversal_reason(raw).map_err(|_| IncomeExpenseError::InvalidReason)
}

pub fn validate_idempotency_key(raw: &str) -> Result<String, IncomeExpenseError> {
    payment_model::validate_idempotency_key(raw)
        .map_err(|_| IncomeExpenseError::InvalidIdempotencyKey)
}

/// Canonical JSON fingerprint of a post-entry command (ADR-006):
/// same key + same payload replays; same key + different payload
/// conflicts. Covers every financially meaningful field.
pub struct EntryFingerprint {
    pub financial_account_id: uuid::Uuid,
    pub category_id: uuid::Uuid,
    pub amount: Decimal,
    /// The CLIENT-EXPRESSED timestamp only: `None` when the request
    /// omitted `occurredAt` and the server defaulted it. A retried
    /// request that again omits the field must produce the identical
    /// fingerprint — hashing a server-generated `now` would make every
    /// retry a false 409 (PILOT-FIX-001 / F4).
    pub occurred_at: Option<OffsetDateTime>,
    pub description: String,
    pub counterparty: Option<String>,
    pub reference_no: Option<String>,
}

pub fn entry_fingerprint(input: &EntryFingerprint) -> String {
    serde_json::json!({
        "financialAccountId": input.financial_account_id,
        "categoryId": input.category_id,
        "amount": payment_model::canonical_amount(input.amount),
        "occurredAt": input.occurred_at,
        "description": input.description,
        "counterparty": input.counterparty,
        "referenceNo": input.reference_no,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;
    use uuid::Uuid;

    #[test]
    fn entry_kind_maps_exactly_one_direction_and_source() {
        assert_eq!(EntryKind::Income.movement_direction().as_str(), "inflow");
        assert_eq!(EntryKind::Expense.movement_direction().as_str(), "outflow");
        assert_eq!(EntryKind::Income.movement_source().as_str(), "income");
        assert_eq!(EntryKind::Expense.movement_source().as_str(), "expense");
        assert_eq!(EntryKind::Income.category_type().as_str(), "income");
        assert_eq!(EntryKind::Expense.category_type().as_str(), "expense");
    }

    #[test]
    fn entry_status_is_posted_to_reversed_only() {
        assert!(EntryStatus::Posted.can_transition_to(EntryStatus::Reversed));
        assert!(!EntryStatus::Reversed.can_transition_to(EntryStatus::Posted));
        assert!(!EntryStatus::Posted.can_transition_to(EntryStatus::Posted));
    }

    #[test]
    fn descriptions_are_required_and_bounded() {
        assert_eq!(
            validate_entry_description("  Kira ödemesi  ").unwrap(),
            "Kira ödemesi"
        );
        assert_eq!(
            validate_entry_description("   "),
            Err(IncomeExpenseError::InvalidDescription)
        );
        assert_eq!(
            validate_entry_description(&"x".repeat(DESCRIPTION_MAX_LEN + 1)),
            Err(IncomeExpenseError::InvalidDescription)
        );
    }

    #[test]
    fn counterparty_and_reference_are_optional_bounded_text() {
        assert_eq!(validate_counterparty(None).unwrap(), None);
        assert_eq!(validate_counterparty(Some("   ")).unwrap(), None);
        assert_eq!(
            validate_counterparty(Some("  Emlak Ofisi  ")).unwrap(),
            Some("Emlak Ofisi".to_string())
        );
        assert_eq!(
            validate_counterparty(Some(&"x".repeat(COUNTERPARTY_MAX_LEN + 1))),
            Err(IncomeExpenseError::InvalidCounterparty)
        );
        assert_eq!(validate_reference_no(None).unwrap(), None);
        assert_eq!(
            validate_reference_no(Some("  ABC-123  ")).unwrap(),
            Some("ABC-123".to_string())
        );
        assert_eq!(
            validate_reference_no(Some(&"x".repeat(REFERENCE_NO_MAX_LEN + 1))),
            Err(IncomeExpenseError::InvalidReferenceNo)
        );
    }

    #[test]
    fn amounts_reject_zero_negative_and_bad_precision() {
        assert_eq!(
            validate_positive_amount("0.00"),
            Err(IncomeExpenseError::InvalidAmount)
        );
        assert_eq!(
            validate_positive_amount("-1.00"),
            Err(IncomeExpenseError::InvalidAmount)
        );
        assert_eq!(
            validate_positive_amount("10.001"),
            Err(IncomeExpenseError::InvalidAmount)
        );
        assert_eq!(
            validate_positive_amount("1250.50").unwrap(),
            Decimal::new(125_050, 2)
        );
    }

    #[test]
    fn occurred_at_rejects_future() {
        let now = OffsetDateTime::UNIX_EPOCH;
        assert!(validate_occurred_at(now, now).is_ok());
        assert_eq!(
            validate_occurred_at(now + Duration::seconds(1), now),
            Err(IncomeExpenseError::InvalidOccurredAt)
        );
    }

    #[test]
    fn fingerprint_covers_every_financial_field() {
        let base = EntryFingerprint {
            financial_account_id: Uuid::from_u128(1),
            category_id: Uuid::from_u128(2),
            amount: Decimal::new(100, 2),
            occurred_at: Some(OffsetDateTime::UNIX_EPOCH),
            description: "x".to_string(),
            counterparty: None,
            reference_no: None,
        };
        assert_eq!(entry_fingerprint(&base), entry_fingerprint(&base));
        let changed_amount = entry_fingerprint(&EntryFingerprint {
            amount: Decimal::new(200, 2),
            ..EntryFingerprint {
                financial_account_id: base.financial_account_id,
                category_id: base.category_id,
                amount: base.amount,
                occurred_at: base.occurred_at,
                description: base.description.clone(),
                counterparty: base.counterparty.clone(),
                reference_no: base.reference_no.clone(),
            }
        });
        assert_ne!(entry_fingerprint(&base), changed_amount);
    }
}
