//! Parties domain model: validation and Turkish-aware search folding.

/// Maximum name length per part (data minimization; names stay human).
pub const NAME_MAX_LEN: usize = 100;
pub const FAMILY_SEQUENCE_MAX: i64 = 999_999;

#[derive(Debug, PartialEq, Eq)]
pub enum PartyError {
    InvalidName,
    InvalidSequence,
}

/// Turkish-aware fold for SEARCH ONLY.
///
/// Requirements (STEP-004 §23): search must be case-insensitive and
/// correct with Turkish letters, while STORED display values keep the
/// original characters. Unicode default lowercasing is insufficient:
/// `'İ'` (dotted capital I) lowercases to `"i" + U+0307` and `'ı'`
/// (dotless i) does not fold to ASCII `i`, so plain `lower()` searches
/// for "ismail"/"isik" would miss stored "İsmail"/"Işık".
///
/// Fold rules: `İ`, `I`, `ı` → `i`; every other char → Unicode
/// lowercase; combining diacritics (U+0300..U+036F) are stripped as a
/// safety net. Deterministic and dependency-free.
pub fn fold_search(input: &str) -> String {
    let mut folded = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            'İ' | 'I' | 'ı' => folded.push('i'),
            '\u{0300}'..='\u{036F}' => {}
            other => folded.extend(other.to_lowercase()),
        }
    }
    folded
}

/// Validate + trim a person name part. Non-empty after trim, at most
/// NAME_MAX_LEN chars, no control characters.
pub fn validate_name(raw: &str) -> Result<String, PartyError> {
    let trimmed = raw.trim();
    let length = trimmed.chars().count();
    if length == 0 || length > NAME_MAX_LEN || trimmed.chars().any(char::is_control) {
        return Err(PartyError::InvalidName);
    }
    Ok(trimmed.to_string())
}

pub fn validate_family_sequence(raw: i64) -> Result<i64, PartyError> {
    if (1..=FAMILY_SEQUENCE_MAX).contains(&raw) {
        Ok(raw)
    } else {
        Err(PartyError::InvalidSequence)
    }
}

/// Shareholder lifecycle (docs/16: implement only supported states).
/// `active` ↔ `inactive`; `voided` marks a mistaken, unused record
/// (terminal; no hard delete of durable business records).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareholderStatus {
    Active,
    Inactive,
    Voided,
}

impl ShareholderStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
            Self::Voided => "voided",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "inactive" => Some(Self::Inactive),
            "voided" => Some(Self::Voided),
            _ => None,
        }
    }

    /// Allowed transitions (docs/16: explicit transitions only).
    /// active ↔ inactive; active/inactive → voided (terminal).
    pub fn can_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (Self::Active, Self::Inactive)
                | (Self::Inactive, Self::Active)
                | (Self::Active, Self::Voided)
                | (Self::Inactive, Self::Voided)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_handles_turkish_dotless_and_dotted_i() {
        assert_eq!(fold_search("İsmail"), "ismail");
        assert_eq!(fold_search("Işık"), "işik");
        assert_eq!(fold_search("ışık"), "işik");
        assert_eq!(fold_search("IŞIK"), "işik");
        // Distinct Turkish letters stay distinct (Ş ≠ S): precision
        // matters — Şahin and Sahin are different names.
        assert_eq!(fold_search("Şahin"), "şahin");
        // Both 'I' and 'ı' fold to 'i': stored and query forms always
        // land on the same value (Yılmaz/YILMAZ/YıLMAZ all match).
        assert_eq!(fold_search("ÇAĞRI"), "çağri");
        assert_eq!(fold_search("Öztürk"), "öztürk");
        assert_eq!(fold_search("GÜL"), "gül");
        assert_eq!(fold_search("Hasan YILMAZ"), "hasan yilmaz");
        assert_eq!(fold_search("Hasan Yılmaz"), "hasan yilmaz");
    }

    #[test]
    fn fold_is_idempotent_and_ascii_neutral() {
        for input in ["Mehmet Yılmaz", "AHMET", "ayşe"] {
            let once = fold_search(input);
            assert_eq!(once, fold_search(&once), "idempotent for {input}");
        }
        assert_eq!(fold_search("Ali"), "ali");
    }

    #[test]
    fn names_must_be_nonempty_and_bounded() {
        assert!(validate_name("  Mehmet ").is_ok());
        assert_eq!(validate_name("  Mehmet ").unwrap(), "Mehmet");
        assert_eq!(validate_name(""), Err(PartyError::InvalidName));
        assert_eq!(validate_name("   "), Err(PartyError::InvalidName));
        assert_eq!(
            validate_name(&"a".repeat(NAME_MAX_LEN + 1)),
            Err(PartyError::InvalidName)
        );
    }

    #[test]
    fn family_sequence_bounds() {
        assert!(validate_family_sequence(1).is_ok());
        assert!(validate_family_sequence(125).is_ok());
        assert_eq!(
            validate_family_sequence(0),
            Err(PartyError::InvalidSequence)
        );
        assert_eq!(
            validate_family_sequence(FAMILY_SEQUENCE_MAX + 1),
            Err(PartyError::InvalidSequence)
        );
    }

    #[test]
    fn shareholder_status_transitions_are_explicit() {
        use ShareholderStatus::*;
        assert!(Active.can_transition_to(Inactive));
        assert!(Inactive.can_transition_to(Active));
        assert!(Active.can_transition_to(Voided));
        assert!(!Voided.can_transition_to(Active));
        assert!(!Active.can_transition_to(Active));
    }
}
