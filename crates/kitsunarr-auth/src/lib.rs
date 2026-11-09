//! Credential primitives: Argon2id password hashing, opaque session tokens and API keys.
//!
//! Session tokens and API keys are random 256-bit / 128-bit values. Only a SHA-256 digest of
//! a session token is stored server side, so a leaked database does not expose live sessions.

use argon2::password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("password hashing failed: {0}")]
    Hash(String),
    #[error("stored password hash is malformed")]
    MalformedHash,
}

/// Argon2id with the OWASP-recommended baseline (19 MiB, 2 passes, 1 lane).
fn hasher() -> Argon2<'static> {
    let params = Params::new(19 * 1024, 2, 1, None).expect("static Argon2 parameters are valid");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Hashes a password into a PHC string (`$argon2id$v=19$m=19456,t=2,p=1$...`).
pub fn hash_password(password: &str) -> Result<String, AuthError> {
    hasher()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| AuthError::Hash(e.to_string()))
}

/// Verifies a password against a stored PHC string. Parameters are taken from the hash,
/// so hashes created with older parameters keep verifying.
pub fn verify_password(password: &str, phc: &str) -> Result<bool, AuthError> {
    let parsed = PasswordHash::new(phc).map_err(|_| AuthError::MalformedHash)?;
    Ok(hasher()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

/// A precomputed hash used to equalise timing when a username does not exist.
pub fn dummy_verify(password: &str) {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let phc = DUMMY.get_or_init(|| hash_password("kitsunarr-timing-equaliser").unwrap_or_default());
    let _ = verify_password(password, phc);
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    rand::fill(&mut buf[..]);
    buf
}

/// New opaque session token (43 URL-safe characters).
pub fn generate_session_token() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<32>())
}

/// New API key: 32 lowercase hex characters, the same shape as Sonarr keys so existing
/// tooling that validates key format keeps working.
pub fn generate_api_key() -> String {
    hex::encode(random_bytes::<16>())
}

/// SHA-256 digest (hex) of a session token, as stored in the database.
pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Constant-time string comparison for secrets.
pub fn secrets_equal(a: &str, b: &str) -> bool {
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify() {
        let phc = hash_password("correct horse battery staple").unwrap();
        assert!(phc.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{phc}");
        assert!(verify_password("correct horse battery staple", &phc).unwrap());
        assert!(!verify_password("wrong password", &phc).unwrap());
        assert!(matches!(
            verify_password("x", "not-a-hash"),
            Err(AuthError::MalformedHash)
        ));
    }

    #[test]
    fn hashes_are_salted() {
        assert_ne!(
            hash_password("same").unwrap(),
            hash_password("same").unwrap()
        );
    }

    #[test]
    fn tokens_and_keys() {
        let t1 = generate_session_token();
        let t2 = generate_session_token();
        assert_eq!(t1.len(), 43);
        assert_ne!(t1, t2);
        let key = generate_api_key();
        assert_eq!(key.len(), 32);
        assert!(
            key.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_eq!(hash_token("abc").len(), 64);
        assert_eq!(hash_token("abc"), hash_token("abc"));
    }

    #[test]
    fn constant_time_compare() {
        assert!(secrets_equal("abc", "abc"));
        assert!(!secrets_equal("abc", "abd"));
        assert!(!secrets_equal("abc", "abcd"));
    }
}
