//! Periods domain model (STEP-006): lifecycle transitions, rule types
//! and input validation. Money uses the STEP-005 exact-decimal helpers
//! (ADR-004: NUMERIC(19,2), decimal-string API transport).

use time::Date;

use crate::shares::model as share_model;

pub const NAME_MAX_LEN: usize = 120;

#[derive(Debug, PartialEq, Eq)]
pub enum PeriodError {
    InvalidName,
    InvalidRuleType,
    InvalidAmount,
    /// due_date < collection_start_date (§11).
    InvalidDates,
}

/// Period lifecycle (docs/16 candidate `Draft -> Open -> Closed`):
/// `draft` is editable and carries no obligations; `open` is reached
/// only through the transactional assessment-generation command; the
/// `closed` command locks ordinary mutation. Closed is terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeriodStatus {
    Draft,
    Open,
    Closed,
}

impl PeriodStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "draft" => Some(Self::Draft),
            "open" => Some(Self::Open),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }

    /// Explicit transition table (docs/16). `draft -> open` happens only
    /// inside the generate-assessments command, not as a status edit.
    pub fn can_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (Self::Draft, Self::Open) | (Self::Open, Self::Closed)
        )
    }
}

/// The two distinct obligation rules (§13–§16): one amount per eligible
/// Shareholder vs one amount component per eligible Share. The rule
/// type is durable financial context — an amount without it is
/// meaningless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssessmentRuleType {
    PerShareholder,
    PerShare,
}

impl AssessmentRuleType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PerShareholder => "per_shareholder",
            Self::PerShare => "per_share",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "per_shareholder" => Some(Self::PerShareholder),
            "per_share" => Some(Self::PerShare),
            _ => None,
        }
    }
}

/// Validate + trim a period name: non-empty, bounded, no control chars.
pub fn validate_name(raw: &str) -> Result<String, PeriodError> {
    let trimmed = raw.trim();
    let length = trimmed.chars().count();
    if length == 0 || length > NAME_MAX_LEN || trimmed.chars().any(char::is_control) {
        return Err(PeriodError::InvalidName);
    }
    Ok(trimmed.to_string())
}

/// Validate the business-date pair (§11: due date must not precede the
/// collection start).
pub fn validate_dates(collection_start_date: Date, due_date: Date) -> Result<(), PeriodError> {
    if due_date < collection_start_date {
        return Err(PeriodError::InvalidDates);
    }
    Ok(())
}

/// Validate a rule base amount: exact NUMERIC(19,2) semantics reused
/// from the shares domain, PLUS rejection of zero — a zero-valued rule
/// creates meaningless obligations (§41).
pub fn validate_base_amount(raw: &str) -> Result<rust_decimal::Decimal, PeriodError> {
    let amount = share_model::validate_amount(raw).map_err(|_| PeriodError::InvalidAmount)?;
    if amount.is_zero() {
        return Err(PeriodError::InvalidAmount);
    }
    Ok(amount)
}

/// Canonical scale-2 decimal string (re-export of the shares helper so
/// API/audit surfaces render identically across domains).
pub fn canonical_amount(amount: rust_decimal::Decimal) -> String {
    share_model::canonical_amount(amount)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u8, d: u8) -> Date {
        Date::from_calendar_date(y, m.try_into().unwrap(), d).unwrap()
    }

    #[test]
    fn period_status_transitions_are_explicit() {
        use PeriodStatus::*;
        assert!(Draft.can_transition_to(Open));
        assert!(Open.can_transition_to(Closed));
        assert!(!Closed.can_transition_to(Open));
        assert!(!Open.can_transition_to(Draft));
        assert!(!Draft.can_transition_to(Closed));
        assert!(!Closed.can_transition_to(Draft));
    }

    #[test]
    fn rule_type_parsing_is_explicit() {
        assert_eq!(
            AssessmentRuleType::parse("per_shareholder"),
            Some(AssessmentRuleType::PerShareholder)
        );
        assert_eq!(
            AssessmentRuleType::parse("per_share"),
            Some(AssessmentRuleType::PerShare)
        );
        assert_eq!(AssessmentRuleType::parse("per_family"), None);
    }

    #[test]
    fn dates_enforce_due_not_before_start() {
        assert!(validate_dates(date(2026, 10, 1), date(2026, 10, 10)).is_ok());
        // Same start/due date is allowed.
        assert!(validate_dates(date(2026, 10, 1), date(2026, 10, 1)).is_ok());
        assert_eq!(
            validate_dates(date(2026, 10, 10), date(2026, 10, 1)),
            Err(PeriodError::InvalidDates)
        );
    }

    #[test]
    fn base_amount_rejects_zero_and_keeps_exact_bounds() {
        use rust_decimal::Decimal;
        assert!(validate_base_amount("0").is_err());
        assert!(validate_base_amount("0.00").is_err());
        assert!(validate_base_amount("-5").is_err());
        assert!(validate_base_amount("1.005").is_err());
        assert_eq!(
            validate_base_amount("10000").unwrap(),
            Decimal::new(1000000, 2)
        );
        assert_eq!(validate_base_amount("0.01").unwrap(), Decimal::new(1, 2));
        assert!(validate_base_amount("99999999999999999.99").is_ok());
        assert!(validate_base_amount("999999999999999999.99").is_err());
    }

    #[test]
    fn names_trim_and_bound() {
        assert_eq!(
            validate_name("  2026 Ekim Dönemi ").unwrap(),
            "2026 Ekim Dönemi"
        );
        assert!(validate_name("   ").is_err());
        assert!(validate_name(&"x".repeat(NAME_MAX_LEN + 1)).is_err());
    }
}
