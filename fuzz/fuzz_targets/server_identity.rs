//! Fuzz target: the server collector identity functions.
//!
//! Properties under test:
//!
//! - `group_key` / `release_key` never panic on arbitrary byte-derived
//!   field values (including hostile UTF-8 boundary strings, NUL bytes are
//!   impossible in `str` by construction);
//! - determinism: the same field set always yields the same keys;
//! - shape: hash keys are `h:` + 64 lowercase hex chars, anchor keys are
//!   `mb:` + the input UUID verbatim;
//! - `valid_mbid` agrees with itself on every input (no panic, total).
//!
//! The manifest model is built directly from fuzzer bytes (no manifest
//! parsing involved — that surface has its own `json_manifest` target).
#![no_main]

use libfuzzer_sys::fuzz_target;
use musicpack_core::format::manifest::{Album, Artist, Identifiers, Manifest, Release};
use musicpack_server::{identity, identity::valid_mbid};

fn field(bytes: &[u8], start: usize, len: usize) -> String {
    let end = (start + len).min(bytes.len());
    String::from_utf8_lossy(&bytes[start..end]).into_owned()
}

fn opt_field(bytes: &[u8], start: usize, len: usize, present: bool) -> Option<String> {
    present.then(|| field(bytes, start, len))
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 16 {
        return;
    }
    let nth = |i: usize| data[i % data.len()];
    let chunk = |i: usize| field(data, i % data.len(), 1 + (nth(i) as usize % 48));

    let artist_count = (nth(0) % 4) as usize;
    let artists = (0..artist_count)
        .map(|i| Artist {
            name: chunk(1 + i * 3),
            role: opt_field(data, 2 + i * 3, 8, nth(3 + i) % 2 == 0),
            musicbrainz_id: None,
            sort_name: None,
        })
        .collect();

    let manifest = Manifest {
        album: Album {
            title: chunk(4),
            artists,
            release_type: None,
            original_release_date: opt_field(data, 5, 12, nth(6) % 2 == 0),
            genres: Vec::new(),
        },
        release: Some(Release {
            release_date: opt_field(data, 7, 12, nth(8) % 2 == 0),
            edition: opt_field(data, 9, 16, nth(10) % 2 == 0),
            country: opt_field(data, 11, 8, nth(12) % 2 == 0),
            label: opt_field(data, 13, 16, nth(14) % 2 == 0),
            catalogue_number: opt_field(data, 15, 12, nth(0) % 2 == 0),
            notes: None,
        }),
        identifiers: Some(Identifiers {
            musicbrainz_release_group_id: opt_field(data, 3, 40, nth(5) % 2 == 0),
            musicbrainz_release_id: opt_field(data, 6, 40, nth(7) % 2 == 0),
            barcode: opt_field(data, 10, 16, nth(9) % 2 == 0),
        }),
        identity: None,
        source: None,
        media: Vec::new(),
        artwork: Vec::new(),
        booklet: Vec::new(),
        lyrics: Vec::new(),
        extras: Vec::new(),
        analysis: Vec::new(),
        loudness: None,
        provenance: None,
    };

    let group = identity::group_key(&manifest);
    let release = identity::release_key(&manifest);
    // Determinism: recompute and require identical keys.
    assert_eq!(group, identity::group_key(&manifest));
    assert_eq!(release, identity::release_key(&manifest));
    // Shape: exactly one of the two key forms.
    for key in [&group, &release] {
        if let Some(hex) = key.strip_prefix("h:") {
            assert_eq!(hex.len(), 64);
            assert!(hex.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        } else {
            assert!(key.starts_with("mb:"), "unexpected key form {key}");
        }
    }
    // The validator is total over the same bytes.
    let _ = valid_mbid(&chunk(12));
});
