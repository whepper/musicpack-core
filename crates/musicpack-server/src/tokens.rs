//! API bearer tokens — the port of the reference's `tokens.c` domain half.
//!
//! Semantics preserved exactly:
//!
//! - 256 bits from the operating system's secure random source, encoded
//!   base64url without padding (32 bytes → 43 chars), prefixed `mpk_`;
//! - only the lowercase-hex SHA-256 of the full secret is persisted (via
//!   [`musicpack_core::format::checksum`], the same primitive the reference
//!   links against);
//! - the plaintext is returned exactly once by the CLI and never logged or
//!   stored.
//!
//! Sessions (token → cookie exchange) are a later stage; nothing here
//! touches them.

use musicpack_core::format::checksum;

use crate::error::ServerError;

/// The `mpk_` prefix of every bearer token.
const PREFIX: &str = "mpk_";

/// base64url alphabet (RFC 4648 §5), as in the reference's `BASE64URL`.
const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Maximum accepted display name length, matching `MP_TOKEN_NAME_MAX`
/// (bytes, like the reference's `strlen` check).
pub const TOKEN_NAME_MAX: usize = 256;

/// Generates a fresh opaque token secret.
pub fn generate_secret() -> Result<String, ServerError> {
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw)
        .map_err(|e| ServerError::Store(format!("cannot generate token secret: {e}")))?;
    Ok(format!("{PREFIX}{}", encode_43(&raw)))
}

/// Generates a fresh session secret: the same 256-bit base64url encoding
/// without the `mpk_` prefix (the C `mp_session_secret_generate` emits the
/// 43 characters bare).
pub fn generate_session_secret() -> Result<String, ServerError> {
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw)
        .map_err(|e| ServerError::Store(format!("cannot generate session secret: {e}")))?;
    Ok(encode_43(&raw))
}

/// Encodes 32 raw bytes as unpadded base64url (43 chars).
///
/// Split out from the generators so the encoding is testable without
/// consuming entropy.
fn encode_43(raw: &[u8; 32]) -> String {
    let mut out = String::with_capacity(43);
    let mut chunk = |block: &[u8]| {
        let v = match block.len() {
            3 => ((block[0] as u32) << 16) | ((block[1] as u32) << 8) | block[2] as u32,
            2 => ((block[0] as u32) << 16) | ((block[1] as u32) << 8),
            1 => (block[0] as u32) << 16,
            _ => unreachable!("chunks are 1..=3 bytes"),
        };
        out.push(BASE64URL[(v >> 18) as usize & 63] as char);
        out.push(BASE64URL[(v >> 12) as usize & 63] as char);
        if block.len() > 1 {
            out.push(BASE64URL[(v >> 6) as usize & 63] as char);
        }
        if block.len() > 2 {
            out.push(BASE64URL[v as usize & 63] as char);
        }
    };
    for group in raw.chunks(3) {
        chunk(group);
    }
    out
}

/// Hashes a secret for storage: lowercase-hex SHA-256 of its UTF-8 bytes
/// (`mp_token_hash`).
pub fn hash_secret(secret: &str) -> String {
    checksum::sha256_hex(secret.as_bytes())
}

/// Validates a display name the way `mp_token_create` does: non-empty and
/// shorter than [`TOKEN_NAME_MAX`] bytes.
pub fn validate_name(name: &str) -> Result<(), ServerError> {
    if name.is_empty() {
        return Err(ServerError::Store("token name must not be empty".into()));
    }
    if name.len() >= TOKEN_NAME_MAX {
        return Err(ServerError::Store(format!(
            "token name is too long ({} bytes; the limit is {})",
            name.len(),
            TOKEN_NAME_MAX
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_matches_base64url_without_padding() {
        // The reference's hand-rolled encoder: 32 bytes in, exactly 43
        // base64url characters out (`mpk_` prefixed for tokens, bare for
        // sessions).
        let body = encode_43(&[0u8; 32]);
        let secret = format!("{PREFIX}{body}");
        assert_eq!(secret.len(), PREFIX.len() + 43);
        assert!(secret.starts_with(PREFIX));
        assert!(
            body.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "body must be base64url: {body}"
        );
        assert!(!body.contains('='), "no padding");
        // Independent oracle (Python `base64.urlsafe_b64encode` of 32 zero
        // bytes, padding stripped): exactly 43 'A' characters.
        assert_eq!(body, "A".repeat(43));

        // 0xFF 0xFF 0xFF → value 63 four times → "____" (base64url uses
        // '_' where standard base64 uses '/'); 0x00 0x00 0x00 → "AAAA".
        let mut ones = [0u8; 32];
        ones[0..3].copy_from_slice(&[0xff, 0xff, 0xff]);
        assert!(encode_43(&ones).starts_with("____"));
        let mut zeros = [0u8; 32];
        zeros[0..3].copy_from_slice(&[0, 0, 0]);
        assert!(encode_43(&zeros).starts_with("AAAA"));
    }

    #[test]
    fn session_secrets_are_bare_base64url() {
        let secret = generate_session_secret().unwrap();
        assert_eq!(secret.len(), 43);
        assert!(!secret.starts_with(PREFIX));
        assert!(
            secret
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "session secret must be base64url: {secret}"
        );
    }

    #[test]
    fn generated_secrets_are_unique_and_well_formed() {
        let a = generate_secret().unwrap();
        let b = generate_secret().unwrap();
        assert_ne!(a, b, "256-bit secrets never collide in practice");
        assert_eq!(a.len(), PREFIX.len() + 43);
        assert!(a.starts_with(PREFIX));
    }

    #[test]
    fn hashing_uses_the_core_sha256_hex_form() {
        // Same value the C stores: musicpack_sha256 over the secret bytes.
        assert_eq!(hash_secret("mpk_test"), checksum::sha256_hex(b"mpk_test"));
        assert_eq!(hash_secret("mpk_test").len(), 64);
        assert!(
            hash_secret("mpk_test")
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
    }

    #[test]
    fn name_validation_follows_the_reference_bounds() {
        assert!(validate_name("Web").is_ok());
        assert!(validate_name("").is_err(), "empty rejected");
        let long = "x".repeat(TOKEN_NAME_MAX); // exactly the limit: rejected
        assert!(validate_name(&long).is_err());
        assert!(validate_name(&"x".repeat(TOKEN_NAME_MAX - 1)).is_ok());
    }
}
