//! SHA-256 declaration format and digest computation.
//!
//! Every manifest-referenced asset declares its digest as exactly 64
//! lowercase hexadecimal characters (`musicpack-v1.md` §3). Uppercase hex,
//! wrong length, and non-hex characters are parse-time errors.
//!
//! Digest computation is the production half of the `.mpack` integrity
//! model. It uses the RustCrypto [`sha2`] crate (see `Cargo.toml` and
//! `docs/architecture.md` §6 for the dependency rationale): the digest
//! bytes and lowercase-hex rendering match the reference C implementation
//! (`musicpack_sha256`, `checksum.c`) exactly, which is pinned by known
//! vectors and by a cross-check against the independent SHA-256 oracle in
//! the test support module.
//!
//! [`sha2`]: https://crates.io/crates/sha2

use sha2::{Digest, Sha256};

/// Length of a raw SHA-256 digest in bytes.
pub const DIGEST_LEN: usize = 32;

/// Length of the lowercase-hex digest rendering (`MUSICPACK_SHA256_HEX_SIZE`
/// minus the NUL terminator).
pub const HEX_LEN: usize = 64;

/// Returns `true` when `s` is exactly 64 lowercase hexadecimal characters,
/// the required textual form of every manifest `sha256` field.
pub fn is_valid_sha256_hex(s: &str) -> bool {
    s.len() == HEX_LEN && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Computes the SHA-256 digest of `data`.
pub fn sha256(data: &[u8]) -> [u8; DIGEST_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// Computes the SHA-256 digest of `data` as lowercase hex.
pub fn sha256_hex(data: &[u8]) -> String {
    to_hex(&sha256(data))
}

/// Streams `reader` through SHA-256 without buffering the whole object.
///
/// This is the file-hashing path: the verifier streams potentially large
/// assets instead of reading them into memory.
pub fn sha256_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<[u8; DIGEST_LEN]> {
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

/// Renders a raw digest as 64 lowercase hex characters.
pub fn to_hex(digest: &[u8; DIGEST_LEN]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(HEX_LEN);
    for &byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Parses 64 lowercase hex characters into a raw digest.
///
/// `None` when the text is not exactly 64 lowercase hex characters (the
/// manifest declaration form).
pub fn sha256_hex_to_bytes(hex: &str) -> Option<[u8; DIGEST_LEN]> {
    if !is_valid_sha256_hex(hex) {
        return None;
    }
    let nibble = |b: u8| -> u8 {
        match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            _ => unreachable!("validated hex"),
        }
    };
    let bytes = hex.as_bytes();
    let mut out = [0u8; DIGEST_LEN];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = (nibble(bytes[i * 2]) << 4) | nibble(bytes[i * 2 + 1]);
    }
    Some(out)
}

/// Compares two digest renderings for equality (the declared value is
/// already validated as 64 lowercase hex; the computed one is rendered by
/// [`to_hex`]).
pub fn hex_eq(a: &str, b: &str) -> bool {
    // Not constant-time: .mpack digests are not secrets. Length is checked
    // first so a maliciously long declared value cannot cause extra work.
    a.len() == b.len() && a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn accepts_lowercase_hex() {
        assert!(is_valid_sha256_hex(VALID));
    }

    #[test]
    fn rejects_wrong_forms() {
        assert!(!is_valid_sha256_hex("")); // empty
        assert!(!is_valid_sha256_hex("0".repeat(63).as_str())); // short
        assert!(!is_valid_sha256_hex("0".repeat(65).as_str())); // long
        assert!(!is_valid_sha256_hex(VALID.to_uppercase().as_str())); // uppercase
        assert!(!is_valid_sha256_hex(&format!("g{}", &VALID[1..]))); // non-hex
        assert!(!is_valid_sha256_hex(&format!("{VALID} "))); // trailing space
    }

    #[test]
    fn known_vectors() {
        // FIPS 180-4 / NIST examples.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // One million 'a' (streamed in chunks by the reader path below).
        let million_a = sha256_hex(&vec![b'a'; 1_000_000]);
        assert_eq!(
            million_a,
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn reader_and_slice_paths_agree() {
        for data in [
            b"".as_slice(),
            b"one",
            b"the quick brown fox jumps over the lazy dog",
        ] {
            let mut cursor = std::io::Cursor::new(data);
            assert_eq!(
                sha256_reader(&mut cursor).unwrap(),
                sha256(data),
                "reader and one-shot paths must agree"
            );
        }
        // Cross the internal buffer boundary.
        let big = vec![0x5a; 64 * 1024 + 7];
        let mut cursor = std::io::Cursor::new(&big);
        assert_eq!(sha256_reader(&mut cursor).unwrap(), sha256(&big));
    }

    #[test]
    fn hex_rendering() {
        assert_eq!(
            to_hex(&sha256(b"one")),
            "7692c3ad3540bb803c020b3aee66cd8887123234ea0c6e7143c0add73ff431ed"
        );
        assert!(hex_eq("abc", "abc"));
        assert!(!hex_eq("abc", "abd"));
        assert!(!hex_eq("abc", "abcd"));
    }
}
