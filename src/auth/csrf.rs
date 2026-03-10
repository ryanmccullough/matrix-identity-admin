use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::Rng;
use subtle::ConstantTimeEq;

use crate::error::AppError;

/// Generate a new random CSRF token (URL-safe base64, 32 bytes of entropy).
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Validate that the submitted CSRF token matches the session's token.
/// Returns `AppError::Validation` on mismatch.
pub fn validate(session_token: &str, submitted_token: &str) -> Result<(), AppError> {
    // Constant-time comparison to prevent timing attacks.
    if !constant_time_eq(session_token.as_bytes(), submitted_token.as_bytes()) {
        return Err(AppError::Validation("Invalid CSRF token.".to_string()));
    }
    Ok(())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    a.ct_eq(b).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_passes_with_matching_tokens() {
        let token = "some-csrf-token";
        assert!(validate(token, token).is_ok());
    }

    #[test]
    fn validate_fails_with_different_tokens() {
        assert!(validate("correct-token", "wrong-token--").is_err());
    }

    #[test]
    fn validate_fails_with_different_lengths() {
        // Must not accidentally treat a prefix as equal.
        assert!(validate("abc", "abcd").is_err());
        assert!(validate("abcd", "abc").is_err());
    }

    #[test]
    fn generated_token_has_expected_length() {
        // 32 random bytes → URL-safe base64 without padding = 43 chars.
        let token = generate_token();
        assert_eq!(token.len(), 43, "unexpected token length: {token}");
    }

    #[test]
    fn generated_token_is_url_safe_base64() {
        let token = generate_token();
        assert!(
            token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "token contains non-URL-safe characters: {token}"
        );
    }

    #[test]
    fn generated_tokens_are_unique() {
        // Statistically impossible to collide with 256 bits of entropy.
        let t1 = generate_token();
        let t2 = generate_token();
        assert_ne!(t1, t2);
    }
}
