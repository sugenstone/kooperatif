//! Financial Accounts domain model (STEP-008): account types,
//! lifecycle, movement effect semantics and input validation.
//!
//! ONE balance engine: `cash` and `bank` differ only in type/metadata —
//! balances are always derived from `account_movements` (ADR-003).

use rust_decimal::Decimal;
use time::OffsetDateTime;

use crate::payments::model as payment_model;
use crate::shares::model as share_model;

pub const NAME_MAX_LEN: usize = 120;
pub const DESCRIPTION_MAX_LEN: usize = 500;
pub const IBAN_MAX_LEN: usize = 34;
pub const TRANSFER_NOTE_MAX_LEN: usize = 500;

#[derive(Debug, PartialEq, Eq)]
pub enum AccountError {
    InvalidName,
    InvalidType,
    InvalidDescription,
    InvalidBankName,
    InvalidIban,
    InvalidAmount,
    InvalidOccurredAt,
    InvalidNote,
    InvalidIdempotencyKey,
    InvalidReason,
    ImmutableField,
}

/// Account operational shapes sharing one balance engine (docs/07).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountType {
    Cash,
    Bank,
}

impl AccountType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cash => "cash",
            Self::Bank => "bank",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "cash" => Some(Self::Cash),
            "bank" => Some(Self::Bank),
            _ => None,
        }
    }
}

/// Account lifecycle (operator decision): `active <-> inactive`.
/// An inactive account keeps its full history and derived balance but
/// accepts no new postings (docs/16 candidate lifecycle subset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountStatus {
    Active,
    Inactive,
}

impl AccountStatus {
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

    /// Explicit transition table: active <-> inactive is the only
    /// command; no delete, no terminal state in STEP-008.
    pub fn can_transition_to(self, target: Self) -> bool {
        self != target
    }
}

/// Movement effect: direction + strictly positive amount — ONE
/// representation, never a signed-amount/direction mix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovementDirection {
    Inflow,
    Outflow,
}

impl MovementDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inflow => "inflow",
            Self::Outflow => "outflow",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "inflow" => Some(Self::Inflow),
            "outflow" => Some(Self::Outflow),
            _ => None,
        }
    }

    /// Signed contribution to the derived balance.
    pub fn effect(self, amount: Decimal) -> Decimal {
        match self {
            Self::Inflow => amount,
            Self::Outflow => -amount,
        }
    }
}

/// Structured movement provenance — the ONLY allowed movement sources
/// in STEP-008 (docs/07 §movements; future sources extend the enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovementSource {
    Payment,
    Transfer,
}

impl MovementSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Payment => "payment",
            Self::Transfer => "transfer",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "payment" => Some(Self::Payment),
            "transfer" => Some(Self::Transfer),
            _ => None,
        }
    }
}

/// Transfer lifecycle: `posted -> reversed`, terminal (docs/19).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferStatus {
    Posted,
    Reversed,
}

impl TransferStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Posted => "posted",
            Self::Reversed => "reversed",
        }
    }

    pub fn can_transition_to(self, target: Self) -> bool {
        matches!((self, target), (Self::Posted, Self::Reversed))
    }
}

pub fn validate_name(raw: &str) -> Result<String, AccountError> {
    let name = raw.trim();
    let length = name.chars().count();
    if length == 0 || length > NAME_MAX_LEN {
        return Err(AccountError::InvalidName);
    }
    Ok(name.to_string())
}

pub fn validate_description(raw: Option<&str>) -> Result<Option<String>, AccountError> {
    match raw {
        None => Ok(None),
        Some(value) => {
            let description = value.trim();
            if description.is_empty() {
                return Ok(None);
            }
            if description.chars().count() > DESCRIPTION_MAX_LEN {
                return Err(AccountError::InvalidDescription);
            }
            Ok(Some(description.to_string()))
        }
    }
}

/// Optional bank name — only meaningful for `bank` accounts; the
/// caller enforces the type pairing.
pub fn validate_bank_name(raw: Option<&str>) -> Result<Option<String>, AccountError> {
    match raw {
        None => Ok(None),
        Some(value) => {
            let bank_name = value.trim();
            if bank_name.is_empty() {
                return Ok(None);
            }
            if bank_name.chars().count() > NAME_MAX_LEN {
                return Err(AccountError::InvalidBankName);
            }
            Ok(Some(bank_name.to_string()))
        }
    }
}

/// IBAN stored as an optional label only — never authentication data.
/// Basic safety validation: bounded length, alphanumeric + spaces.
pub fn validate_iban(raw: Option<&str>) -> Result<Option<String>, AccountError> {
    match raw {
        None => Ok(None),
        Some(value) => {
            let compact: String = value.split_whitespace().collect();
            if compact.is_empty() {
                return Ok(None);
            }
            let length = compact.chars().count();
            if !(5..=IBAN_MAX_LEN).contains(&length)
                || !compact.chars().all(|c| c.is_ascii_alphanumeric())
            {
                return Err(AccountError::InvalidIban);
            }
            Ok(Some(compact.to_uppercase()))
        }
    }
}

/// Strictly-positive exact amount for transfers (same NUMERIC(19,2)
/// contract as every financial value).
pub fn validate_positive_amount(raw: &str) -> Result<Decimal, AccountError> {
    let amount = share_model::validate_amount(raw).map_err(|_| AccountError::InvalidAmount)?;
    if amount.is_zero() {
        return Err(AccountError::InvalidAmount);
    }
    Ok(amount)
}

/// `occurred_at` may be backdated (operator records the earlier
/// physical movement) but never future-dated.
pub fn validate_occurred_at(
    occurred_at: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<OffsetDateTime, AccountError> {
    if occurred_at > now {
        return Err(AccountError::InvalidOccurredAt);
    }
    Ok(occurred_at)
}

pub fn validate_note(raw: Option<&str>) -> Result<Option<String>, AccountError> {
    match raw {
        None => Ok(None),
        Some(value) => {
            let note = value.trim();
            if note.is_empty() {
                return Ok(None);
            }
            if note.chars().count() > TRANSFER_NOTE_MAX_LEN {
                return Err(AccountError::InvalidNote);
            }
            Ok(Some(note.to_string()))
        }
    }
}

pub fn validate_reversal_reason(raw: &str) -> Result<String, AccountError> {
    payment_model::validate_reversal_reason(raw).map_err(|_| AccountError::InvalidReason)
}

pub fn validate_idempotency_key(raw: &str) -> Result<String, AccountError> {
    payment_model::validate_idempotency_key(raw).map_err(|_| AccountError::InvalidIdempotencyKey)
}

/// Canonical JSON fingerprint of the transfer command (same ADR-006
/// pattern as Payments): replay identical → existing Transfer;
/// different payload under the same key → 409.
pub struct TransferFingerprint {
    pub source_account_id: uuid::Uuid,
    pub destination_account_id: uuid::Uuid,
    pub amount: Decimal,
    pub occurred_at: OffsetDateTime,
    pub note: Option<String>,
}

pub fn transfer_fingerprint(input: &TransferFingerprint) -> String {
    serde_json::json!({
        "sourceAccountId": input.source_account_id,
        "destinationAccountId": input.destination_account_id,
        "amount": payment_model::canonical_amount(input.amount),
        "occurredAt": input.occurred_at,
        "note": input.note,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;
    use uuid::Uuid;

    #[test]
    fn account_type_parsing_is_explicit() {
        assert_eq!(AccountType::parse("cash"), Some(AccountType::Cash));
        assert_eq!(AccountType::parse("bank"), Some(AccountType::Bank));
        assert_eq!(AccountType::parse("gold"), None);
        assert_eq!(AccountType::parse("wallet"), None);
    }

    #[test]
    fn account_status_transitions_are_explicit() {
        assert!(AccountStatus::Active.can_transition_to(AccountStatus::Inactive));
        assert!(AccountStatus::Inactive.can_transition_to(AccountStatus::Active));
        assert!(!AccountStatus::Active.can_transition_to(AccountStatus::Active));
    }

    #[test]
    fn movement_effect_is_exact_signed_contribution() {
        assert_eq!(
            MovementDirection::Inflow.effect(Decimal::new(1000, 2)),
            Decimal::new(1000, 2)
        );
        assert_eq!(
            MovementDirection::Outflow.effect(Decimal::new(1000, 2)),
            Decimal::new(-1000, 2)
        );
    }

    #[test]
    fn names_trim_and_bound() {
        assert_eq!(
            validate_name("  İstanbul TL Kasası  ").unwrap(),
            "İstanbul TL Kasası"
        );
        assert_eq!(validate_name(""), Err(AccountError::InvalidName));
        assert_eq!(
            validate_name(&"x".repeat(NAME_MAX_LEN + 1)),
            Err(AccountError::InvalidName)
        );
    }

    #[test]
    fn iban_validates_shape_safely() {
        assert_eq!(
            validate_iban(Some("TR33 0006 1005 1978 6457 8413 26")).unwrap(),
            Some("TR330006100519786457841326".to_string())
        );
        assert_eq!(
            validate_iban(Some("iban' OR 1=1 --")),
            Err(AccountError::InvalidIban)
        );
        assert_eq!(validate_iban(Some("AB")), Err(AccountError::InvalidIban));
    }

    #[test]
    fn amounts_reject_zero_and_keep_bounds() {
        assert_eq!(
            validate_positive_amount("0.00"),
            Err(AccountError::InvalidAmount)
        );
        assert_eq!(
            validate_positive_amount("-5.00"),
            Err(AccountError::InvalidAmount)
        );
        assert_eq!(
            validate_positive_amount("10.001"),
            Err(AccountError::InvalidAmount)
        );
        // NUMERIC(19,2) bound: 17 integer digits + 2 scale.
        assert!(validate_positive_amount("99999999999999999.99").is_ok());
        assert_eq!(
            validate_positive_amount("999999999999999999.99"),
            Err(AccountError::InvalidAmount)
        );
    }

    #[test]
    fn occurred_at_rejects_the_future() {
        let now = OffsetDateTime::UNIX_EPOCH;
        assert!(validate_occurred_at(now, now).is_ok());
        assert_eq!(
            validate_occurred_at(now + Duration::seconds(1), now),
            Err(AccountError::InvalidOccurredAt)
        );
    }

    #[test]
    fn transfer_fingerprint_covers_payload() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let now = OffsetDateTime::UNIX_EPOCH;
        let base = TransferFingerprint {
            source_account_id: a,
            destination_account_id: b,
            amount: Decimal::new(1000, 2),
            occurred_at: now,
            note: None,
        };
        assert_eq!(transfer_fingerprint(&base), transfer_fingerprint(&base));
        let different = TransferFingerprint {
            amount: Decimal::new(2000, 2),
            ..TransferFingerprint {
                source_account_id: a,
                destination_account_id: b,
                amount: Decimal::new(1000, 2),
                occurred_at: now,
                note: None,
            }
        };
        assert_ne!(
            transfer_fingerprint(&base),
            transfer_fingerprint(&different)
        );
    }
}
