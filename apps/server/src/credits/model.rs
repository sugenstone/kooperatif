//! STEP-009 domain model — Shareholder Credit (Excess Payment) &
//! Credit Applications.
//!
//! Canonical terminology (docs/01 glossary, docs/03, docs/06):
//!   * Shareholder Credit = "Excess Payment Balance" — received value
//!     attributable to exactly ONE Shareholder but not (yet) allocated
//!     to an obligation. Turkish UI: "Fazla Ödeme".
//!   * Credit Application = "Prior Excess Applied" — a settlement
//!     effect on an Assessment funded by held credit. NOT a Payment.
//!
//! Invariants enforced here and in the repo layer:
//!   * A credit is backed ONLY by a posted Payment's unassigned
//!     remainder — never created from nothing.
//!   * Credit creation/application move NO Financial Account money.
//!   * Balances are DERIVED: available = Σ active credits − Σ active
//!     applications; there is no mutable balance column anywhere.
//!   * Reversal is status-based with actor/time/reason (docs/19).

use rust_decimal::Decimal;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::shares::model as share_model;

/// Where a credit application's value is consumed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ApplicationMode {
    /// Created by the auto-offset engine (assessment generation or
    /// credit assignment sweeping the beneficiary's open debts).
    Automatic,
    /// Created by the explicit operator apply-credit command.
    Manual,
}

impl ApplicationMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Manual => "manual",
        }
    }
}

pub const STATUS_ACTIVE: &str = "active";
pub const STATUS_REVERSED: &str = "reversed";

#[derive(Debug)]
pub enum CreditError {
    InvalidAmount,
    InvalidReason,
    InvalidNote,
    InvalidIdempotencyKey,
}

/// Strictly-positive exact credit amount — identical bounds to every
/// other financial amount (NUMERIC(19,2), ≤2 fraction digits, > 0).
pub fn validate_amount(raw: &str) -> Result<Decimal, CreditError> {
    let amount = share_model::validate_amount(raw).map_err(|_| CreditError::InvalidAmount)?;
    if amount.is_zero() {
        return Err(CreditError::InvalidAmount);
    }
    Ok(amount)
}

/// Required reversal reason — docs/19: actor + time + reason, always.
pub fn validate_reversal_reason(raw: &str) -> Result<String, CreditError> {
    match share_model::validate_reason(Some(raw)).map_err(|_| CreditError::InvalidReason)? {
        Some(reason) => Ok(reason),
        None => Err(CreditError::InvalidReason),
    }
}

/// Optional free-text note on a credit assignment.
pub fn validate_note(raw: Option<&str>) -> Result<Option<String>, CreditError> {
    share_model::validate_reason(raw).map_err(|_| CreditError::InvalidNote)
}

/// Client-supplied idempotency key (ADR-006) — same policy as
/// Payments; uniqueness is enforced durably by the database.
pub fn validate_idempotency_key(raw: &str) -> Result<String, CreditError> {
    let trimmed = raw.trim();
    let length = trimmed.chars().count();
    if length == 0 || length > 120 || trimmed.chars().any(char::is_control) {
        return Err(CreditError::InvalidIdempotencyKey);
    }
    Ok(trimmed.to_string())
}

/// Deterministic fingerprint of a credit-assignment payload — a reused
/// key carrying a DIFFERENT payload is a hard 409 (ADR-006).
pub fn assignment_fingerprint(
    payment_id: Uuid,
    shareholder_id: Uuid,
    amount: Decimal,
    note: Option<&str>,
) -> String {
    format!(
        "credit-assign|{payment_id}|{shareholder_id}|{amount}|{}",
        note.unwrap_or("")
    )
}

/// Fingerprint for a manual credit application command.
pub fn application_fingerprint(assessment_id: Uuid, amount: Decimal) -> String {
    format!("credit-apply|{assessment_id}|{amount}")
}

/// One row of the derived credit ledger for UI/API output.
pub struct CreditRow {
    pub id: Uuid,
    pub credit_number: i64,
    pub source_payment_id: Uuid,
    pub payment_number: i64,
    pub shareholder_id: Uuid,
    pub shareholder_first_name: String,
    pub shareholder_last_name: String,
    pub amount: Decimal,
    pub applied_amount: Decimal,
    pub available_amount: Decimal,
    pub status: String,
    pub note: Option<String>,
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
    pub created_at: OffsetDateTime,
    pub created_by_name: Option<String>,
}

/// One row of application history (either direction of inspection).
pub struct ApplicationRow {
    pub id: Uuid,
    pub credit_id: Uuid,
    pub credit_number: i64,
    pub assessment_id: Uuid,
    pub period_name: String,
    pub amount: Decimal,
    pub mode: String,
    pub status: String,
    pub reversed_at: Option<OffsetDateTime>,
    pub reversal_reason: Option<String>,
    pub created_at: OffsetDateTime,
    pub created_by_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn dec(raw: &str) -> Decimal {
        Decimal::from_str(raw).unwrap()
    }

    #[test]
    fn amounts_reject_zero_negative_and_third_decimal() {
        assert_eq!(validate_amount("100.00").unwrap(), dec("100.00"));
        assert_eq!(validate_amount("0.01").unwrap(), dec("0.01"));
        assert!(validate_amount("0").is_err());
        assert!(validate_amount("-5.00").is_err());
        assert!(validate_amount("10.001").is_err());
        assert!(validate_amount("abc").is_err());
        assert!(validate_amount("99999999999999999.99").is_ok());
        assert!(validate_amount("999999999999999999.99").is_err());
    }

    #[test]
    fn reasons_are_required_trimmed_and_bounded() {
        assert_eq!(
            validate_reversal_reason("  düzeltme  ").unwrap(),
            "düzeltme"
        );
        assert!(validate_reversal_reason("").is_err());
        assert!(validate_reversal_reason("   ").is_err());
        assert!(validate_reversal_reason(&"x".repeat(501)).is_err());
        assert!(validate_reversal_reason(&"x".repeat(500)).is_ok());
    }

    #[test]
    fn fingerprints_cover_identity_and_amount() {
        let a = assignment_fingerprint(Uuid::nil(), Uuid::from_u128(1), dec("100.00"), Some("n"));
        let b = assignment_fingerprint(Uuid::nil(), Uuid::from_u128(1), dec("100.00"), Some("n"));
        let c = assignment_fingerprint(Uuid::nil(), Uuid::from_u128(2), dec("100.00"), Some("n"));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(
            application_fingerprint(Uuid::nil(), dec("50.00")),
            application_fingerprint(Uuid::nil(), dec("50.00"))
        );
    }

    #[test]
    fn idempotency_keys_are_bounded_and_clean() {
        assert!(validate_idempotency_key("key-123_ok").is_ok());
        assert!(validate_idempotency_key("").is_err());
        assert!(validate_idempotency_key(&"k".repeat(121)).is_err());
    }
}
