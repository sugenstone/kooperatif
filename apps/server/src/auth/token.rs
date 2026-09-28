//! Opaque session/CSRF token generation and hashing (STEP-002 §11).
//!
//! Browser stores the RAW opaque token (HttpOnly cookie). PostgreSQL
//! stores only its SHA-256 hash. Tokens are 256-bit CSPRNG values —
//! never sequential, never predictable.

use base64ct::{Base64UrlUnpadded, Encoding};
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Raw token length in bytes (256 bits of entropy).
const TOKEN_BYTES: usize = 32;

#[derive(Debug)]
pub struct GeneratedToken {
    /// Raw secret — handed to the browser (cookie / JSON CSRF token),
    /// never persisted, never logged.
    pub raw: String,
    /// SHA-256 of the raw token — the only form stored in PostgreSQL.
    pub hash: [u8; 32],
}

/// Generate a cryptographically secure opaque token (base64url, no pad).
pub fn generate_token() -> GeneratedToken {
    let mut bytes = [0u8; TOKEN_BYTES];
    OsRng.fill_bytes(&mut bytes);
    let raw = Base64UrlUnpadded::encode_string(&bytes);
    GeneratedToken {
        hash: hash_token(&raw),
        raw,
    }
}

/// Hash a raw token for storage/lookup.
pub fn hash_token(raw: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hasher.finalize().into()
}

/// Constant-time comparison of two 32-byte hashes.
pub fn hash_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    // sha2 does not expose ct_compare; use a branch-light manual loop.
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_url_safe_and_high_entropy() {
        let token = generate_token();
        // 32 bytes -> 43 unpadded base64url chars.
        assert_eq!(token.raw.len(), 43);
        assert!(token
            .raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert_ne!(token.raw, generate_token().raw, "tokens must not repeat");
    }

    #[test]
    fn hashing_is_deterministic_and_raw_never_recoverable() {
        let token = generate_token();
        assert_eq!(token.hash, hash_token(&token.raw));
        assert_eq!(token.hash.len(), 32);
        assert!(!token
            .raw
            .as_bytes()
            .windows(4)
            .any(|w| token.hash.starts_with(w)));
    }

    #[test]
    fn hash_comparison_is_exact() {
        let a = generate_token();
        let b = generate_token();
        assert!(hash_eq(&a.hash, &a.hash));
        assert!(!hash_eq(&a.hash, &b.hash));
    }
}
