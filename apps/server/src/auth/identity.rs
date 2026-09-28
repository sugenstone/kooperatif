//! Username normalization/validation and password policy + Argon2id
//! hashing (ADR-002 §Password handling).

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

/// Login identifier rules (STEP-002 §5).
///
/// Normalization: trim → ASCII lowercase. The normalized form is the
/// canonical login identifier and is what the database uniqueness
/// constraint protects. NOT email-based: operator usernames are
/// independent of email.
///
/// Validation (after normalization): 3..=64 chars; each char is an ASCII
/// letter/digit or one of `.`, `_`, `-`. ASCII-only is a deliberate
/// decision: Turkish dotted/dotless i (`İ`/`ı`) make Unicode
/// case-conversion non-roundtripping, which would silently split
/// identities; Turkish names belong in `display_name` (free-form),
/// not in the login handle.
pub const USERNAME_MIN_LEN: usize = 3;
pub const USERNAME_MAX_LEN: usize = 64;

#[derive(Debug, PartialEq, Eq)]
pub enum UsernameError {
    InvalidLength,
    InvalidCharacters,
}

impl UsernameError {
    pub fn message_key(&self) -> &'static str {
        match self {
            Self::InvalidLength => "auth.username.error.length",
            Self::InvalidCharacters => "auth.username.error.characters",
        }
    }
}

pub fn normalize_username(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
}

pub fn validate_normalized_username(username: &str) -> Result<(), UsernameError> {
    let length = username.chars().count();
    if !(USERNAME_MIN_LEN..=USERNAME_MAX_LEN).contains(&length) {
        return Err(UsernameError::InvalidLength);
    }
    let valid = username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if valid {
        Ok(())
    } else {
        Err(UsernameError::InvalidCharacters)
    }
}

/// Normalize + validate in one step.
pub fn normalize_and_validate_username(raw: &str) -> Result<String, UsernameError> {
    let normalized = normalize_username(raw);
    validate_normalized_username(&normalized)?;
    Ok(normalized)
}

/// Password policy (STEP-002 §9): minimum-length oriented, no arbitrary
/// composition rules, long passphrases supported, no silent truncation.
pub const PASSWORD_MIN_LEN: usize = 12;
pub const PASSWORD_MAX_LEN: usize = 512;

#[derive(Debug, PartialEq, Eq)]
pub enum PasswordPolicyError {
    TooShort,
    TooLong,
}

impl PasswordPolicyError {
    pub fn message_key(&self) -> &'static str {
        match self {
            Self::TooShort => "auth.password.error.tooShort",
            Self::TooLong => "auth.password.error.tooLong",
        }
    }
}

pub fn validate_password_policy(password: &str) -> Result<(), PasswordPolicyError> {
    let length = password.chars().count();
    if length < PASSWORD_MIN_LEN {
        Err(PasswordPolicyError::TooShort)
    } else if length > PASSWORD_MAX_LEN {
        Err(PasswordPolicyError::TooLong)
    } else {
        Ok(())
    }
}

/// Hash a password with Argon2id using the runtime's validated
/// parameters. Output is a PHC string (self-contained: algorithm,
/// version, parameters, salt, hash).
pub fn hash_password(
    argon2: &Argon2<'_>,
    password: &str,
) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut rand::rngs::OsRng);
    let hash = argon2.hash_password(password.as_bytes(), &salt)?;
    Ok(hash.to_string())
}

/// Constant-time-ish verification through the Argon2 crate.
pub fn verify_password(
    argon2: &Argon2<'_>,
    password: &str,
    phc_hash: &str,
) -> Result<bool, argon2::password_hash::Error> {
    let parsed = PasswordHash::new(phc_hash)?;
    Ok(argon2.verify_password(password.as_bytes(), &parsed).is_ok())
}

/// Equalize verification timing for unknown usernames (account-enumeration
/// defense, STEP-002 §17/§53): perform a real Argon2 verification against
/// a dummy hash generated once per process with the SAME parameters as
/// real logins. The dummy's preimage is irrelevant — it is never a valid
/// credential.
static DUMMY_HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn dummy_verify(argon2: &Argon2<'_>, password: &str) {
    let dummy = DUMMY_HASH.get_or_init(|| {
        // Generating costs one Argon2 pass — comparable to verification —
        // and happens at most once per process.
        hash_password(argon2, password).expect("dummy hash generation")
    });
    let _ = verify_password(argon2, password, dummy);
}

#[cfg(test)]
mod tests {
    use super::*;
    use argon2::Params;

    fn fast_argon2() -> Argon2<'static> {
        Argon2::new(
            argon2::Algorithm::Argon2id,
            argon2::Version::V0x13,
            Params::new(8192, 1, 1, None).expect("test parameters"),
        )
    }

    #[test]
    fn usernames_normalize_to_canonical_lowercase_form() {
        assert_eq!(normalize_username("  Ali.Yilmaz "), "ali.yilmaz");
        assert_eq!(
            normalize_and_validate_username("Kadir_UNLU-01").as_deref(),
            Ok("kadir_unlu-01")
        );
    }

    #[test]
    fn username_length_bounds_are_enforced() {
        assert_eq!(
            normalize_and_validate_username("ab"),
            Err(UsernameError::InvalidLength)
        );
        let too_long = "a".repeat(USERNAME_MAX_LEN + 1);
        assert_eq!(
            normalize_and_validate_username(&too_long),
            Err(UsernameError::InvalidLength)
        );
    }

    #[test]
    fn username_character_whitelist_is_enforced() {
        assert_eq!(
            normalize_and_validate_username("ad soyad"),
            Err(UsernameError::InvalidCharacters)
        );
        assert_eq!(
            normalize_and_validate_username("ad@site"),
            Err(UsernameError::InvalidCharacters)
        );
        // Non-ASCII letters are rejected: Turkish İ/ı case conversion is
        // not round-tripping, so handles stay ASCII; display names carry
        // the Turkish text.
        assert_eq!(
            normalize_and_validate_username("ayşe_yılmaz"),
            Err(UsernameError::InvalidCharacters)
        );
    }

    #[test]
    fn username_normalization_roundtrips_exactly() {
        let handle = "kullanici.tl-01";
        assert_eq!(
            normalize_username(&handle.to_uppercase()),
            handle,
            "case changes must never fork identities"
        );
    }

    #[test]
    fn password_policy_is_length_oriented() {
        assert_eq!(
            validate_password_policy("kısa"),
            Err(PasswordPolicyError::TooShort)
        );
        assert!(validate_password_policy("uzun bir parola ifadesi").is_ok());
        let too_long = "x".repeat(PASSWORD_MAX_LEN + 1);
        assert_eq!(
            validate_password_policy(&too_long),
            Err(PasswordPolicyError::TooLong)
        );
    }

    #[test]
    fn password_hash_roundtrip_and_format() {
        let argon2 = fast_argon2();
        let hash = hash_password(&argon2, "dogru parola 123").expect("hash succeeds");
        assert!(
            hash.starts_with("$argon2id$"),
            "must be Argon2id PHC: {hash}"
        );
        assert!(hash.contains("m=8192"), "parameters embedded");
        assert!(verify_password(&argon2, "dogru parola 123", &hash).expect("verify"));
        assert!(!verify_password(&argon2, "yanlis parola!", &hash).expect("verify"));
    }

    #[test]
    fn dummy_verify_performs_real_argon2_work() {
        let argon2 = fast_argon2();
        // Must not panic and must equalize timing for unknown users.
        dummy_verify(&argon2, "herhangi bir parola");
        dummy_verify(&argon2, "baska bir parola");
    }
}
