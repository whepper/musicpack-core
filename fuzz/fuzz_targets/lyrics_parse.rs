//! Fuzz target: the strict lyrics parser (`musicpack_core::lyrics`).
//!
//! Properties (docs/musicpack-lyrics-v1.md §14): arbitrary bytes never
//! panic, never allocate beyond the declared limits, and either parse to
//! a document whose internal state is valid (sorted, non-negative
//! timestamps; stable document-order ties) or fail with a typed error;
//! `active_line` is total and monotone over the parsed document; the
//! document is `Clone`/`PartialEq`-stable (no interior mutability).

#![no_main]

use libfuzzer_sys::fuzz_target;
use musicpack_core::lyrics::{self, LyricsContent};

fuzz_target!(|data: &[u8]| {
    match lyrics::parse(data) {
        Ok(doc) => {
            // Document invariants (parser output is always normalized).
            if let LyricsContent::Synced(lines) = &doc.content {
                assert!(!lines.is_empty(), "synced documents have >= 1 line");
                let mut prev = i64::MIN;
                for line in lines {
                    assert!(line.timestamp_ms >= prev, "timestamps are sorted");
                    assert!(line.timestamp_ms >= 0, "timestamps are non-negative");
                    prev = line.timestamp_ms;
                }
            }
            // active_line is total, deterministic and monotone in the
            // position: probe a sorted spread of fixed and input-derived
            // positions.
            let mut probes: Vec<i64> = vec![
                i64::MIN,
                -1,
                0,
                1,
                999,
                1_000,
                60_000,
                3_600_000,
                i64::MAX,
                i64::from(data.first().copied().unwrap_or(0)),
                data.iter().fold(0i64, |a, b| a.wrapping_add(*b as i64)),
            ];
            probes.sort_unstable();
            let mut prev_index: i64 = -1; // None = -1
            for p in probes {
                let selected = lyrics::active_line(&doc, p);
                let index = selected.map_or(-1, |v| v as i64);
                assert!(index >= -1, "active_line result is in range");
                if let LyricsContent::Synced(lines) = &doc.content {
                    assert!(index < lines.len() as i64, "active_line result is in range");
                    if let Some(i) = selected {
                        assert!(
                            lines[i].timestamp_ms <= p,
                            "the active line's timestamp is <= the position"
                        );
                    }
                } else {
                    assert_eq!(index, -1, "plain documents never select a line");
                }
                assert!(
                    index >= prev_index,
                    "monotone in the position (None = -1)"
                );
                prev_index = index;
                assert_eq!(
                    lyrics::active_line(&doc, p),
                    selected,
                    "active_line is deterministic for (document, position)"
                );
            }
            // Clone/eq round trip is stable.
            let clone = doc.clone();
            assert_eq!(clone, doc);
            let _ = lyrics::parse(&data[..data.len() / 2]);
        }
        Err(e) => {
            // A rejection is a valid fuzz result; the error renders.
            let _ = e.to_string();
        }
    }
});
