//! Share Return domain model (STEP-011, docs/10 + docs/16): return
//! lifecycle, entitlement types/statuses and input validation.
//!
//! Three layers stay separate:
//!   SHARE LIFECYCLE  — active -> return_pending -> closed
//!   ENTITLEMENT      — the cooperative's obligation (never cash)
//!   SETTLEMENT       — posted -> reversed, one outflow movement each
//!
//! Amounts are never computed here: Refundable Invested Amount and
//! Profit Right formulas are UNRESOLVED/POLICY_DRIVEN (docs/17), so the
//! operator supplies the cooperative-approved amount which is then
//! snapshotted on the entitlement row.

use rust_decimal::Decimal;
use time::OffsetDateTime;

use crate::payments::model as payment_model;
use crate::shares::model as share_model;

pub const DESCRIPTION_MAX_LEN: usize = 500;
pub const POLICY_REFERENCE_MAX_LEN: usize = 200;

#[derive(Debug, PartialEq, Eq)]
pub enum ShareReturnError {
    InvalidStatus,
    InvalidEntitlementType,
    InvalidEntitlementStatus,
    InvalidAmount,
    InvalidReason,
    InvalidDescription,
    InvalidPolicyReference,
    InvalidIdempotencyKey,
}

/// Return case lifecycle (docs/16 Share Return workflow, bounded to the
/// states a single-operator flow can own — the approval/policy engine
/// of roadmap Phase 7 does not exist yet):
/// `pending -> finalized` (rights crystallized, Share closed) or
/// `pending -> cancelled` (Share returns to `active`, ownership never
/// closed so nothing is "restored" — it was never ended).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnStatus {
    Pending,
    Finalized,
    Cancelled,
}

impl ReturnStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Finalized => "finalized",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "finalized" => Some(Self::Finalized),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// The two economically distinct rights a return may crystallize
/// (docs/10). They carry independent amounts, due dates and
/// policy provenance — never merged into one number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntitlementType {
    Principal,
    Profit,
}

impl EntitlementType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Principal => "principal",
            Self::Profit => "profit",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "principal" => Some(Self::Principal),
            "profit" => Some(Self::Profit),
            _ => None,
        }
    }
}

/// Entitlement lifecycle (docs/16 Receivable direction):
/// `open -> partially_settled -> settled | cancelled`. "Not yet due /
/// due / overdue" is DERIVED from due_date vs the cooperative-local
/// today — never a stored state (docs/16 rule + STEP-011 §50).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntitlementStatus {
    Open,
    PartiallySettled,
    Settled,
    Cancelled,
}

impl EntitlementStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::PartiallySettled => "partially_settled",
            Self::Settled => "settled",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "partially_settled" => Some(Self::PartiallySettled),
            "settled" => Some(Self::Settled),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// Settlement lifecycle: `posted -> reversed`, terminal — the same
/// status-based correction model as Income/Expense (docs/19).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementStatus {
    Posted,
    Reversed,
}

impl SettlementStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Posted => "posted",
            Self::Reversed => "reversed",
        }
    }
}

/// Non-negative entitlement amount — `None` stays `None` (an existing
/// but unquantified right is not a zero right, §16).
pub fn validate_entitlement_amount(raw: Option<&str>) -> Result<Option<Decimal>, ShareReturnError> {
    match raw {
        None => Ok(None),
        Some(text) => share_model::validate_amount(text)
            .map(Some)
            .map_err(|_| ShareReturnError::InvalidAmount),
    }
}

/// Strictly-positive settlement amount (NUMERIC(19,2), ADR-004).
pub fn validate_settlement_amount(raw: &str) -> Result<Decimal, ShareReturnError> {
    let amount = share_model::validate_amount(raw).map_err(|_| ShareReturnError::InvalidAmount)?;
    if amount.is_zero() {
        return Err(ShareReturnError::InvalidAmount);
    }
    Ok(amount)
}

pub fn validate_reason(raw: Option<&str>) -> Result<Option<String>, ShareReturnError> {
    share_model::validate_reason(raw).map_err(|_| ShareReturnError::InvalidReason)
}

/// Required non-empty reason (cancellation/reversal contract).
pub fn validate_required_reason(raw: &str) -> Result<String, ShareReturnError> {
    payment_model::validate_reversal_reason(raw).map_err(|_| ShareReturnError::InvalidReason)
}

/// Optional free-text explanation of how the right was determined
/// (inputs/notes — the "how calculated?" evidence).
pub fn validate_description(raw: Option<&str>) -> Result<Option<String>, ShareReturnError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(text) if text.chars().count() <= DESCRIPTION_MAX_LEN => Ok(Some(text.to_string())),
        Some(_) => Err(ShareReturnError::InvalidDescription),
    }
}

/// Optional governing-rule evidence (policy name / board decision ref).
pub fn validate_policy_reference(raw: Option<&str>) -> Result<Option<String>, ShareReturnError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(text) if text.chars().count() <= POLICY_REFERENCE_MAX_LEN => {
            Ok(Some(text.to_string()))
        }
        Some(_) => Err(ShareReturnError::InvalidPolicyReference),
    }
}

pub fn validate_idempotency_key(raw: &str) -> Result<String, ShareReturnError> {
    payment_model::validate_idempotency_key(raw)
        .map_err(|_| ShareReturnError::InvalidIdempotencyKey)
}

/// Canonical JSON fingerprint of an initiate command (ADR-006).
pub fn initiate_fingerprint(
    share_id: uuid::Uuid,
    effective_return_date: time::Date,
    reason: &Option<String>,
) -> String {
    serde_json::json!({
        "shareId": share_id,
        "effectiveReturnDate": effective_return_date,
        "reason": reason,
    })
    .to_string()
}

/// Canonical JSON fingerprint of a settle command (ADR-006).
/// `settled_at` is the CLIENT-EXPRESSED timestamp only — `None` when
/// the request omitted it and the server defaulted (PILOT-FIX-001 /
/// F4: retries of a defaulted request must replay, not false-409).
pub fn settlement_fingerprint(
    entitlement_id: uuid::Uuid,
    financial_account_id: uuid::Uuid,
    amount: Decimal,
    settled_at: Option<OffsetDateTime>,
) -> String {
    serde_json::json!({
        "entitlementId": entitlement_id,
        "financialAccountId": financial_account_id,
        "amount": payment_model::canonical_amount(amount),
        "settledAt": settled_at,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn return_status_is_explicit() {
        assert_eq!(ReturnStatus::parse("pending"), Some(ReturnStatus::Pending));
        assert_eq!(
            ReturnStatus::parse("finalized"),
            Some(ReturnStatus::Finalized)
        );
        assert_eq!(
            ReturnStatus::parse("cancelled"),
            Some(ReturnStatus::Cancelled)
        );
        assert_eq!(ReturnStatus::parse("closed"), None);
    }

    #[test]
    fn entitlement_types_are_explicit() {
        assert_eq!(
            EntitlementType::parse("principal"),
            Some(EntitlementType::Principal)
        );
        assert_eq!(
            EntitlementType::parse("profit"),
            Some(EntitlementType::Profit)
        );
        assert_eq!(EntitlementType::parse("expense"), None);
    }

    #[test]
    fn entitlement_amount_preserves_null_vs_zero() {
        assert_eq!(validate_entitlement_amount(None).unwrap(), None);
        assert_eq!(
            validate_entitlement_amount(Some("0.00")).unwrap(),
            Some(Decimal::ZERO)
        );
        assert!(validate_entitlement_amount(Some("-1")).is_err());
    }

    #[test]
    fn settlement_amount_rejects_zero_and_bad_precision() {
        assert_eq!(
            validate_settlement_amount("0.00"),
            Err(ShareReturnError::InvalidAmount)
        );
        assert_eq!(
            validate_settlement_amount("40000.50").unwrap(),
            Decimal::new(4_000_050, 2)
        );
    }

    #[test]
    fn fingerprints_cover_every_input() {
        let a = initiate_fingerprint(Uuid::from_u128(1), time::Date::MIN, &None);
        let b = initiate_fingerprint(Uuid::from_u128(1), time::Date::MIN, &Some("x".to_string()));
        assert_ne!(a, b);
        let s1 = settlement_fingerprint(
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            Decimal::new(100, 2),
            Some(OffsetDateTime::UNIX_EPOCH),
        );
        let s2 = settlement_fingerprint(
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            Decimal::new(200, 2),
            Some(OffsetDateTime::UNIX_EPOCH),
        );
        assert_ne!(s1, s2);
    }
}
