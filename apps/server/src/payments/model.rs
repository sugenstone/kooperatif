//! Payments domain model (STEP-007): lifecycle, methods and input
//! validation. Money reuses the exact-decimal helpers (ADR-004:
//! NUMERIC(19,2), decimal-string API transport); a Payment amount and
//! every Allocation amount must be strictly positive.

use rust_decimal::Decimal;
use time::OffsetDateTime;

use crate::shares::model as share_model;

pub const NOTE_MAX_LEN: usize = 500;
pub const IDEMPOTENCY_KEY_MAX_LEN: usize = 120;

#[derive(Debug, PartialEq, Eq)]
pub enum PaymentError {
    InvalidAmount,
    InvalidMethod,
    InvalidNote,
    InvalidReceivedAt,
    InvalidIdempotencyKey,
    InvalidReason,
    InvalidPayer,
}

/// Payment lifecycle (docs/16 candidate subset implemented in
/// STEP-007): a Payment is created directly `posted` by the atomic
/// collection command — the Draft/Pending-Approval states are not
/// modeled because no approval policy is approved (POLICY_DRIVEN).
/// `posted -> reversed` is the only transition and is terminal; a
/// reversed Payment is immutable history (docs/19).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentStatus {
    Posted,
    Reversed,
}

impl PaymentStatus {
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

    /// Explicit transition table (docs/16): posted -> reversed only.
    /// Reversed never returns anywhere.
    pub fn can_transition_to(self, target: Self) -> bool {
        matches!((self, target), (Self::Posted, Self::Reversed))
    }
}

/// Allocation lifecycle: `active -> reversed`, terminal (docs/19).
/// Reversal never deletes the row — the original application of value
/// remains auditable forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocationStatus {
    Active,
    Reversed,
}

impl AllocationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Reversed => "reversed",
        }
    }
}

/// How the value was handed over (docs/06 §10). Recording metadata
/// only: HOW the money arrived — never WHERE it is held. The
/// destination Financial Account (STEP-008) is a separate required
/// field; a cash-method Payment may still land in a bank account and
/// vice versa. Ledger accounts remain a deferred domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentMethod {
    Cash,
    BankTransfer,
    Card,
    Other,
}

impl PaymentMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cash => "cash",
            Self::BankTransfer => "bank_transfer",
            Self::Card => "card",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "cash" => Some(Self::Cash),
            "bank_transfer" => Some(Self::BankTransfer),
            "card" => Some(Self::Card),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

/// Strictly-positive exact amount for Payments and Allocations:
/// NUMERIC(19,2) bounds from the shared helper plus the zero rejection
/// — a zero Payment records no received value and a zero Allocation
/// settles nothing (docs/15).
pub fn validate_positive_amount(raw: &str) -> Result<Decimal, PaymentError> {
    let amount = share_model::validate_amount(raw).map_err(|_| PaymentError::InvalidAmount)?;
    if amount.is_zero() {
        return Err(PaymentError::InvalidAmount);
    }
    Ok(amount)
}

/// Optional free-text note (bounded, trimmed).
pub fn validate_note(raw: Option<&str>) -> Result<Option<String>, PaymentError> {
    share_model::validate_reason(raw).map_err(|_| PaymentError::InvalidNote)
}

/// Required reversal reason — docs/19: every reversal records actor,
/// time AND a reason. An empty reason cannot satisfy that contract.
pub fn validate_reversal_reason(raw: &str) -> Result<String, PaymentError> {
    match share_model::validate_reason(Some(raw)).map_err(|_| PaymentError::InvalidReason)? {
        Some(reason) => Ok(reason),
        None => Err(PaymentError::InvalidReason),
    }
}

/// Client-supplied idempotency key (ADR-006): non-empty, bounded, no
/// control characters. Uniqueness is enforced durably by the database.
pub fn validate_idempotency_key(raw: &str) -> Result<String, PaymentError> {
    let trimmed = raw.trim();
    let length = trimmed.chars().count();
    if length == 0 || length > IDEMPOTENCY_KEY_MAX_LEN || trimmed.chars().any(char::is_control) {
        return Err(PaymentError::InvalidIdempotencyKey);
    }
    Ok(trimmed.to_string())
}

/// `received_at` may be backdated (operator records earlier physical
/// collection) but never future-dated — a Payment cannot record value
/// received at a time that has not happened yet.
pub fn validate_received_at(
    received_at: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<OffsetDateTime, PaymentError> {
    if received_at > now {
        return Err(PaymentError::InvalidReceivedAt);
    }
    Ok(received_at)
}

/// Canonical scale-2 decimal string (shared rendering rule).
pub fn canonical_amount(amount: Decimal) -> String {
    share_model::canonical_amount(amount)
}

/// Input bundle for the create-command fingerprint.
pub struct PaymentFingerprint<'a> {
    pub payer_person_id: Option<uuid::Uuid>,
    pub payer_first_name: Option<&'a str>,
    pub payer_last_name: Option<&'a str>,
    pub amount: Decimal,
    pub method: PaymentMethod,
    /// CLIENT-EXPRESSED timestamp only — `None` when the request omitted
    /// `receivedAt` (PILOT-FIX-001 / F4: server defaults must not enter
    /// the fingerprint or retries collide as false 409s).
    pub received_at: Option<OffsetDateTime>,
    pub note: Option<&'a str>,
    /// STEP-008: the destination Financial Account is part of the
    /// fingerprint — replaying the same key with a different account
    /// is a different command (409).
    pub destination_account_id: uuid::Uuid,
    pub allocations: &'a [(uuid::Uuid, Decimal)],
}

/// Canonical JSON fingerprint of the create command — serialized with
/// serde_json's deterministic (BTreeMap) key order. Stored next to the
/// idempotency key so a REPLAY with the identical payload returns the
/// existing Payment while the same key carrying a DIFFERENT payload is
/// a 409 conflict (ADR-006).
pub fn idempotency_fingerprint(input: &PaymentFingerprint<'_>) -> String {
    let mut sorted: Vec<&(uuid::Uuid, Decimal)> = input.allocations.iter().collect();
    sorted.sort_by_key(|(id, _)| *id);
    serde_json::json!({
        "payerPersonId": input.payer_person_id,
        "payerFirstName": input.payer_first_name,
        "payerLastName": input.payer_last_name,
        "amount": canonical_amount(input.amount),
        "method": input.method.as_str(),
        "receivedAt": input.received_at,
        "note": input.note,
        "destinationAccountId": input.destination_account_id,
        "allocations": sorted
            .iter()
            .map(|(id, amount)| serde_json::json!({
                "assessmentId": id,
                "amount": canonical_amount(*amount),
            }))
            .collect::<Vec<_>>(),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;
    use uuid::Uuid;

    #[test]
    fn payment_status_transitions_are_explicit() {
        assert!(PaymentStatus::Posted.can_transition_to(PaymentStatus::Reversed));
        assert!(!PaymentStatus::Reversed.can_transition_to(PaymentStatus::Posted));
        assert!(!PaymentStatus::Posted.can_transition_to(PaymentStatus::Posted));
        assert!(!PaymentStatus::Reversed.can_transition_to(PaymentStatus::Reversed));
    }

    #[test]
    fn methods_parse_explicitly() {
        assert_eq!(PaymentMethod::parse("cash"), Some(PaymentMethod::Cash));
        assert_eq!(
            PaymentMethod::parse("bank_transfer"),
            Some(PaymentMethod::BankTransfer)
        );
        assert_eq!(PaymentMethod::parse("card"), Some(PaymentMethod::Card));
        assert_eq!(PaymentMethod::parse("other"), Some(PaymentMethod::Other));
        assert_eq!(PaymentMethod::parse("havale"), None);
        assert_eq!(PaymentMethod::parse(""), None);
    }

    #[test]
    fn positive_amounts_reject_zero_and_keep_exact_bounds() {
        assert!(validate_positive_amount("0").is_err());
        assert!(validate_positive_amount("0.00").is_err());
        assert!(validate_positive_amount("-5").is_err());
        assert!(validate_positive_amount("1.005").is_err());
        assert_eq!(
            validate_positive_amount("0.01").unwrap(),
            Decimal::new(1, 2)
        );
        assert_eq!(
            validate_positive_amount("30000").unwrap(),
            Decimal::new(3000000, 2)
        );
        assert!(validate_positive_amount("99999999999999999.99").is_ok());
        assert!(validate_positive_amount("999999999999999999.99").is_err());
    }

    #[test]
    fn reversal_reason_is_required_and_bounded() {
        assert!(validate_reversal_reason("").is_err());
        assert!(validate_reversal_reason("   ").is_err());
        assert_eq!(
            validate_reversal_reason("  hatalı tahsilat ").unwrap(),
            "hatalı tahsilat".to_string()
        );
        assert!(validate_reversal_reason(&"x".repeat(NOTE_MAX_LEN + 1)).is_err());
    }

    #[test]
    fn idempotency_keys_are_bounded_and_clean() {
        assert!(validate_idempotency_key("").is_err());
        assert!(validate_idempotency_key("   ").is_err());
        assert!(validate_idempotency_key("abc-123").is_ok());
        assert!(validate_idempotency_key(&"x".repeat(IDEMPOTENCY_KEY_MAX_LEN + 1)).is_err());
        assert!(validate_idempotency_key("line\nbreak").is_err());
    }

    #[test]
    fn received_at_rejects_the_future() {
        let now = OffsetDateTime::now_utc();
        assert!(validate_received_at(now - Duration::days(30), now).is_ok());
        assert!(validate_received_at(now, now).is_ok());
        assert_eq!(
            validate_received_at(now + Duration::seconds(1), now),
            Err(PaymentError::InvalidReceivedAt)
        );
    }

    #[test]
    fn fingerprint_is_order_independent_for_allocations() {
        let a = Uuid::nil();
        let b = Uuid::from_u128(1);
        let now = OffsetDateTime::UNIX_EPOCH;
        let base = PaymentFingerprint {
            payer_person_id: None,
            payer_first_name: Some("Ali"),
            payer_last_name: Some("Yılmaz"),
            amount: Decimal::new(1000, 2),
            method: PaymentMethod::Cash,
            received_at: Some(now),
            note: None,
            destination_account_id: Uuid::from_u128(9),
            allocations: &[(a, Decimal::new(500, 2)), (b, Decimal::new(500, 2))],
        };
        let first = idempotency_fingerprint(&base);
        let second = idempotency_fingerprint(&PaymentFingerprint {
            allocations: &[(b, Decimal::new(500, 2)), (a, Decimal::new(500, 2))],
            ..base
        });
        assert_eq!(first, second);

        let different_amount = idempotency_fingerprint(&PaymentFingerprint {
            amount: Decimal::new(1001, 2),
            allocations: &[(a, Decimal::new(500, 2)), (b, Decimal::new(501, 2))],
            ..base
        });
        assert_ne!(first, different_amount);
    }
}
