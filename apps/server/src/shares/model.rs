//! Shares domain model (STEP-005): lifecycle transitions, acquisition
//! classification and agreed-value (money) validation per ADR-004.

use rust_decimal::Decimal;
use std::str::FromStr;

pub const REASON_MAX_LEN: usize = 500;
/// NUMERIC(19,2) upper bound: 19 significant digits — 17 integer +
/// 2 fraction → max 99,999,999,999,999,999.99 (mantissa 19 nines, scale 2).
const MAX_AMOUNT_MANTISSA: i128 = 9_999_999_999_999_999_999;

#[derive(Debug, PartialEq, Eq)]
pub enum ShareError {
    InvalidStatus,
    InvalidAcquisitionType,
    InvalidAmount,
    InvalidReason,
}

/// Share lifecycle (docs/16 candidate subset implemented in STEP-005):
/// `active` ↔ `suspended`; `active`/`suspended` → `voided` (terminal,
/// mistaken-record correction). `return_pending`/`closed` belong to the
/// future Share Return workflow and are intentionally absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareStatus {
    Active,
    Suspended,
    Voided,
}

impl ShareStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Voided => "voided",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "suspended" => Some(Self::Suspended),
            "voided" => Some(Self::Voided),
            _ => None,
        }
    }

    /// Explicit transition table (docs/16 rule for implementation).
    pub fn can_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (Self::Active, Self::Suspended)
                | (Self::Suspended, Self::Active)
                | (Self::Active, Self::Voided)
                | (Self::Suspended, Self::Voided)
        )
    }
}

/// How an ownership interval began. `Founder`/`LaterAcquisition` mark
/// the initial allocation (§14–§16); `Transfer`/`Sale` mark later owner
/// changes. Founder status is explicit data — never inferred from
/// shareholder age or sequence (§40/§41).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquisitionType {
    Founder,
    LaterAcquisition,
    Transfer,
    Sale,
}

impl AcquisitionType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Founder => "founder",
            Self::LaterAcquisition => "later_acquisition",
            Self::Transfer => "transfer",
            Self::Sale => "sale",
        }
    }

    /// Parses an initial-allocation classification (request input).
    pub fn parse_initial(value: &str) -> Option<Self> {
        match value {
            "founder" => Some(Self::Founder),
            "later_acquisition" => Some(Self::LaterAcquisition),
            _ => None,
        }
    }

    pub fn is_initial(self) -> bool {
        matches!(self, Self::Founder | Self::LaterAcquisition)
    }
}

/// Parse an API amount (decimal string) into an exact Decimal valid for
/// NUMERIC(19,2): non-negative, at most 2 fraction digits. `None`
/// (absent amount) is handled by the caller — NULL ≠ 0 (§17).
pub fn validate_amount(raw: &str) -> Result<Decimal, ShareError> {
    let trimmed = raw.trim();
    let parsed = Decimal::from_str(trimmed).map_err(|_| ShareError::InvalidAmount)?;
    if parsed.is_sign_negative()
        || parsed.scale() > 2
        || parsed.abs() > Decimal::from_i128_with_scale(MAX_AMOUNT_MANTISSA, 2)
    {
        return Err(ShareError::InvalidAmount);
    }
    // Normalize to scale 2 so "50" and "50.00" round-trip identically.
    // scale <= 2 is enforced above, so round_dp only pads — never rounds.
    Ok(parsed.round_dp(2))
}

/// Canonical scale-2 decimal string for API/audit surfaces (NULL vs 0
/// stays visible: "0" never appears where "0.00" was meant).
pub fn canonical_amount(amount: Decimal) -> String {
    let mut normalized = amount;
    normalized.rescale(2);
    normalized.to_string()
}

/// Validate an optional free-text reason/note.
pub fn validate_reason(raw: Option<&str>) -> Result<Option<String>, ShareError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(text) if text.chars().count() <= REASON_MAX_LEN => Ok(Some(text.to_string())),
        Some(_) => Err(ShareError::InvalidReason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_status_transitions_are_explicit() {
        use ShareStatus::*;
        assert!(Active.can_transition_to(Suspended));
        assert!(Suspended.can_transition_to(Active));
        assert!(Active.can_transition_to(Voided));
        assert!(Suspended.can_transition_to(Voided));
        assert!(!Voided.can_transition_to(Active));
        assert!(!Active.can_transition_to(Active));
        assert!(!Suspended.can_transition_to(Suspended));
    }

    #[test]
    fn acquisition_type_parsing_is_explicit() {
        assert_eq!(
            AcquisitionType::parse_initial("founder"),
            Some(AcquisitionType::Founder)
        );
        assert_eq!(
            AcquisitionType::parse_initial("later_acquisition"),
            Some(AcquisitionType::LaterAcquisition)
        );
        assert_eq!(AcquisitionType::parse_initial("sale"), None);
        assert_eq!(AcquisitionType::parse_initial(""), None);
    }

    #[test]
    fn amounts_are_exact_and_bounded() {
        assert_eq!(validate_amount("50000").unwrap(), Decimal::new(5000000, 2));
        assert_eq!(
            validate_amount("50000.25").unwrap(),
            Decimal::new(5000025, 2)
        );
        assert_eq!(validate_amount("0").unwrap(), Decimal::ZERO);
        assert!(validate_amount("-1").is_err());
        assert!(validate_amount("1.005").is_err());
        assert!(validate_amount("abc").is_err());
        assert!(validate_amount("999999999999999999.99").is_err());
        assert!(validate_amount("99999999999999999.99").is_ok());
    }

    #[test]
    fn reasons_trim_and_bound() {
        assert_eq!(validate_reason(None).unwrap(), None);
        assert_eq!(validate_reason(Some("   ")).unwrap(), None);
        assert_eq!(
            validate_reason(Some(" aile değişimi ")).unwrap(),
            Some("aile değişimi".to_string())
        );
        assert!(validate_reason(Some(&"x".repeat(REASON_MAX_LEN + 1))).is_err());
    }
}
