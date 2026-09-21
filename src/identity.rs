//! Collector identity — the port of the reference's `identity.c`.
//!
//! Filesystem paths are NOT the conceptual identity of a release. Three
//! levels are computed from the canonical manifest (nothing is added to
//! `.mpack` v1):
//!
//! - [`package_fingerprint`]: SHA-256 of the canonical manifest
//!   serialization (via the core's canonical writer, never reimplemented
//!   here). Any content change yields a new fingerprint; a moved package
//!   keeps its fingerprint.
//! - [`group_key`]: the MusicBrainz release-group id (`mb:<uuid>`) when the
//!   manifest carries a canonical one, else `h:` + SHA-256 over the
//!   canonical TLV encoding of the stable group fields (title, original
//!   date, type, artists sorted by (name, role)).
//! - [`release_key`]: the MusicBrainz release id (`mb:<uuid>`) when present,
//!   else `h:` + SHA-256 over the edition-distinguishing fields (edition,
//!   date, country, label, catalogue number, barcode).
//!
//! Byte-level rules preserved from the reference:
//!
//! - TLV field = 1-byte tag + 4-byte big-endian byte length + raw bytes.
//!   Absent fields (`None`, and empty MBIDs) are not emitted;
//!   present-but-empty values (`Some("")`) emit a zero length — they differ
//!   from absent fields.
//! - Artist sorting is byte-wise on (name, role with missing role as `""`)
//!   and stable, so a reordered artist list keeps its key.
//! - MBID validity is the canonical 8-4-4-4-12 hex-with-hyphens shape
//!   (upper- and lower-case hex accepted, like the reference's `isxdigit`).
//!
//! These are pure functions over the [`Manifest`] model; no I/O, no
//! database. They were computed server-side until R4.1 and are promoted
//! here so the authoring package builder can produce the same identity
//! without duplicating the algorithm (`docs/adr/0010-core-package-builder.md`).
//! Golden vectors generated from the C server live in
//! `crates/musicpack-server/tests/data/identity/` and continue to pin this
//! code through the server's re-export.

use crate::format::checksum;
use crate::format::manifest::Manifest;

/// Tag bytes of the canonical TLV encoding (`identity.c`).
const TAG_TITLE: u8 = 1;
const TAG_DATE: u8 = 2;
const TAG_TYPE: u8 = 3;
const TAG_ARTIST_NAME: u8 = 4;
const TAG_ARTIST_ROLE: u8 = 5;
const TAG_EDITION: u8 = 6;
const TAG_RELEASE_DATE: u8 = 7;
const TAG_COUNTRY: u8 = 8;
const TAG_LABEL: u8 = 9;
const TAG_CATALOGUE: u8 = 10;
const TAG_BARCODE: u8 = 11;

/// `true` when `s` is a canonical MusicBrainz UUID: 36 chars, 8-4-4-4-12
/// hex digits with hyphens (`mp_identity_valid_mbid`).
pub fn valid_mbid(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (i, &b) in bytes.iter().enumerate() {
        let is_dash = i == 8 || i == 13 || i == 18 || i == 23;
        if is_dash {
            if b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

/// `true` when an optional MBID is present, non-empty and canonical.
/// The reference treats a missing *or empty* id as "no anchor".
fn anchor(id: Option<&str>) -> Option<&str> {
    id.filter(|s| !s.is_empty()).filter(|s| valid_mbid(s))
}

/// Appends one TLV field. `None` emits nothing (absent); `Some("")` emits
/// the tag with a zero length (present-but-empty).
fn append_field(buf: &mut Vec<u8>, tag: u8, value: Option<&str>) {
    let Some(value) = value else { return };
    let bytes = value.as_bytes();
    buf.push(tag);
    buf.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(bytes);
}

/// SHA-256 of the canonical manifest serialization, lowercase hex
/// (`mp_identity_package_fingerprint` over `musicpack_manifest_write`).
pub fn package_fingerprint(manifest: &Manifest) -> Result<String, crate::Error> {
    let canonical = manifest.write_canonical()?;
    Ok(checksum::sha256_hex(canonical.as_bytes()))
}

/// SHA-256 of raw `manifest.json` bytes, lowercase hex — the cheap change
/// detector (`mp_identity_manifest_hash`).
pub fn manifest_hash(bytes: &[u8]) -> String {
    checksum::sha256_hex(bytes)
}

/// Stable release-group key (`mp_identity_group_key`).
pub fn group_key(manifest: &Manifest) -> String {
    if let Some(id) = manifest
        .identifiers
        .as_ref()
        .and_then(|i| anchor(i.musicbrainz_release_group_id.as_deref()))
    {
        return format!("mb:{id}");
    }
    let mut buf = Vec::new();
    append_field(&mut buf, TAG_TITLE, Some(&manifest.album.title));
    append_field(
        &mut buf,
        TAG_DATE,
        manifest.album.original_release_date.as_deref(),
    );
    append_field(
        &mut buf,
        TAG_TYPE,
        manifest.album.release_type.map(|t| t.as_str()),
    );
    // Stable insertion order for equal (name, role) pairs: `sort_by` is a
    // stable sort and the comparator coincides with the reference's
    // `artist_cmp`, so reordered artist lists hash identically.
    let mut artists: Vec<&crate::format::manifest::Artist> =
        manifest.album.artists.iter().collect();
    artists.sort_by(|a, b| {
        a.name.as_bytes().cmp(b.name.as_bytes()).then_with(|| {
            a.role
                .as_deref()
                .unwrap_or("")
                .as_bytes()
                .cmp(b.role.as_deref().unwrap_or("").as_bytes())
        })
    });
    for artist in artists {
        append_field(&mut buf, TAG_ARTIST_NAME, Some(&artist.name));
        append_field(&mut buf, TAG_ARTIST_ROLE, artist.role.as_deref());
    }
    format!("h:{}", checksum::sha256_hex(&buf))
}

/// Stable release/edition key within a group (`mp_identity_release_key`).
pub fn release_key(manifest: &Manifest) -> String {
    if let Some(id) = manifest
        .identifiers
        .as_ref()
        .and_then(|i| anchor(i.musicbrainz_release_id.as_deref()))
    {
        return format!("mb:{id}");
    }
    let mut buf = Vec::new();
    let release = manifest.release.as_ref();
    append_field(
        &mut buf,
        TAG_EDITION,
        release.and_then(|r| r.edition.as_deref()),
    );
    append_field(
        &mut buf,
        TAG_RELEASE_DATE,
        release.and_then(|r| r.release_date.as_deref()),
    );
    append_field(
        &mut buf,
        TAG_COUNTRY,
        release.and_then(|r| r.country.as_deref()),
    );
    append_field(
        &mut buf,
        TAG_LABEL,
        release.and_then(|r| r.label.as_deref()),
    );
    append_field(
        &mut buf,
        TAG_CATALOGUE,
        release.and_then(|r| r.catalogue_number.as_deref()),
    );
    append_field(
        &mut buf,
        TAG_BARCODE,
        manifest
            .identifiers
            .as_ref()
            .and_then(|i| i.barcode.as_deref()),
    );
    format!("h:{}", checksum::sha256_hex(&buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mbid_shape_follows_the_reference() {
        assert!(valid_mbid("12345678-1234-1234-1234-1234567890ab"));
        // Uppercase hex is accepted (the C uses isxdigit).
        assert!(valid_mbid("12345678-1234-1234-1234-1234567890AB"));
        assert!(!valid_mbid(""));
        assert!(!valid_mbid("12345678-1234-1234-1234-1234567890a")); // short
        assert!(!valid_mbid("12345678-1234-1234-1234-1234567890abc")); // long
        assert!(!valid_mbid("123456781234123412341234567890ab")); // no dashes
        assert!(!valid_mbid("12345678_1234_1234_1234_1234567890ab")); // wrong sep
        assert!(!valid_mbid("12345678-1234-1234-1234-1234567890ag")); // non-hex
        // Dashes in the wrong positions.
        assert!(!valid_mbid("-2345678-1234-1234-1234-1234567890ab"));
    }

    #[test]
    fn empty_mbid_is_no_anchor() {
        assert_eq!(anchor(None), None);
        assert_eq!(anchor(Some("")), None);
        assert_eq!(anchor(Some("not-a-uuid")), None);
        assert_eq!(
            anchor(Some("12345678-1234-1234-1234-1234567890ab")),
            Some("12345678-1234-1234-1234-1234567890ab")
        );
    }

    #[test]
    fn empty_string_differs_from_absent_in_the_tlv() {
        let mut absent = Vec::new();
        append_field(&mut absent, TAG_EDITION, None);
        let mut empty = Vec::new();
        append_field(&mut empty, TAG_EDITION, Some(""));
        assert!(absent.is_empty());
        assert_eq!(empty, vec![TAG_EDITION, 0, 0, 0, 0]);
    }

    #[test]
    fn tlv_layout_is_tag_be32_bytes() {
        let mut buf = Vec::new();
        append_field(&mut buf, TAG_TITLE, Some("AB"));
        assert_eq!(buf, vec![TAG_TITLE, 0, 0, 0, 2, b'A', b'B']);
    }
}
