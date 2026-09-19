//! Resource budgets for untrusted input.
//!
//! Byte-for-byte ports of the policy limits the C reference enforces before
//! and during parsing. These are security boundaries, not tuning knobs: a
//! hostile manifest must never be able to amplify a small input into a
//! large allocation or an unbounded traversal.
//!
//! | Constant | C source |
//! |----------|----------|
//! | [`MANIFEST_MAX_BYTES`], [`JSON_MAX_DEPTH`] | `musicpack-v1.md` §8 (16 MiB manifest bound, JSON nesting ≤ 100) |
//! | [`MAX_REFERENCED_ASSETS`], [`MAX_FILE_BYTES`], [`MAX_TOTAL_BYTES`], per-array budgets | `manifest.h` |
//! | [`PATH_MAX_BYTES`] | `path.h` (`MUSICPACK_PATH_MAX`) |
//!
//! The values are compile-time constants in the C library; they stay
//! constants here. Making them configurable would be a compatibility-policy
//! change and must not happen silently.

/// Maximum accepted size of the `manifest.json` document (16 MiB).
///
/// Also the MPAK v1 bound on a `MANF` block payload (`mpak-v1.md` §4).
pub const MANIFEST_MAX_BYTES: usize = 16 * 1024 * 1024;

/// Maximum JSON nesting depth accepted in a manifest.
///
/// **Discrepancy, documented rather than resolved:** `specs/musicpack-v1.md`
/// §8 and the reference README state a limit of 100, but the vendored cJSON
/// the reference actually parses with enforces `CJSON_NESTING_LIMIT` =
/// 1000. The implementation is the behavioural authority, so this constant
/// is 1000; see the discrepancy log in `docs/architecture.md`.
pub const JSON_MAX_DEPTH: usize = 1000;

/// Maximum total number of referenced assets across all manifest arrays.
pub const MAX_REFERENCED_ASSETS: usize = 4096;

/// Maximum size of a single referenced file (8 GiB).
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Maximum aggregate referenced bytes per package (64 GiB).
pub const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024 * 1024;

/// Maximum number of bytes in a package-relative path.
///
/// Note: the C reference checks `strlen` (bytes), not Unicode scalar count;
/// a multi-byte UTF-8 path therefore has a *smaller* character budget than
/// an ASCII one. This module matches the C behaviour.
pub const PATH_MAX_BYTES: usize = 4096;

/// Maximum number of `media[]` entries (discs).
pub const MAX_DISCS: usize = 32;

/// Maximum number of tracks within one disc.
pub const MAX_TRACKS_PER_DISC: usize = 512;

/// Maximum number of artist credits in one credit list.
pub const MAX_ARTISTS_PER_CREDIT: usize = 64;

/// Maximum number of `genres[]` entries.
pub const MAX_GENRES: usize = 64;

/// Maximum number of `artwork[]` entries.
pub const MAX_ARTWORK: usize = 32;

/// Maximum number of `booklet[]` entries.
pub const MAX_BOOKLET: usize = 32;

/// Maximum number of `lyrics[]` entries.
pub const MAX_LYRICS: usize = 512;

/// Maximum number of `extras[]` entries.
pub const MAX_EXTRAS: usize = 256;

/// Maximum number of `analysis[]` entries.
pub const MAX_ANALYSIS: usize = 32;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgets_match_the_c_reference() {
        // Pin the values to the C headers/specs so an accidental edit fails
        // a test instead of silently changing security behaviour.
        assert_eq!(MANIFEST_MAX_BYTES, 16 * 1024 * 1024);
        // 1000 = cJSON's CJSON_NESTING_LIMIT (the spec's "100" claim is
        // contradicted by the reference implementation; see the
        // discrepancy log in docs/architecture.md).
        assert_eq!(JSON_MAX_DEPTH, 1000);
        assert_eq!(MAX_REFERENCED_ASSETS, 4096);
        assert_eq!(MAX_FILE_BYTES, 8 * 1024 * 1024 * 1024);
        assert_eq!(MAX_TOTAL_BYTES, 64 * 1024 * 1024 * 1024);
        assert_eq!(PATH_MAX_BYTES, 4096);
        assert_eq!(MAX_DISCS, 32);
        assert_eq!(MAX_TRACKS_PER_DISC, 512);
        assert_eq!(MAX_ARTISTS_PER_CREDIT, 64);
        assert_eq!(MAX_GENRES, 64);
        assert_eq!(MAX_ARTWORK, 32);
        assert_eq!(MAX_BOOKLET, 32);
        assert_eq!(MAX_LYRICS, 512);
        assert_eq!(MAX_EXTRAS, 256);
        assert_eq!(MAX_ANALYSIS, 32);
    }
}
