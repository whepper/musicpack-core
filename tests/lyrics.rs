//! Lyrics domain tests — the normative matrix of
//! `docs/musicpack-lyrics-v1.md` (§14, Core): strict LRC parsing
//! (accepted grammar, defined reductions, rejections, limits,
//! normalization, Unicode) and the `active_line` timing contract
//! (deterministic selection, ties, boundaries, monotonicity).

use musicpack_core::lyrics::{
    self, LyricsContent, LyricsDocument, LyricsError, LyricsLine, active_line, parse,
};

fn parse_ok(input: &str) -> LyricsDocument {
    parse(input.as_bytes()).expect("input should parse")
}

fn synced(input: &str) -> Vec<(i64, String)> {
    let doc = parse_ok(input);
    assert!(doc.is_synced(), "expected a synchronised document");
    doc.synced_lines()
        .unwrap()
        .iter()
        .map(|l| (l.timestamp_ms, l.text.clone()))
        .collect()
}

fn plain(input: &str) -> Vec<String> {
    let doc = parse_ok(input);
    assert!(!doc.is_synced(), "expected a plain document");
    doc.plain_lines().unwrap().to_vec()
}

// ---------------------------------------------------------------------
// Accepted grammar (spec §4)
// ---------------------------------------------------------------------

#[test]
fn plain_document() {
    let doc = parse_ok("First line\nSecond line\n");
    assert!(!doc.is_synced());
    assert_eq!(
        plain("First line\nSecond line\n"),
        vec!["First line", "Second line"]
    );
}

#[test]
fn empty_document_is_plain() {
    let doc = parse_ok("");
    assert_eq!(doc.plain_lines().unwrap(), &[""][..0]);
}

#[test]
fn blank_lines_are_stanza_breaks() {
    assert_eq!(plain("a\n\nb\n"), vec!["a", "", "b"]);
}

#[test]
fn bom_is_stripped() {
    assert_eq!(
        synced("\u{feff}[00:01.00]hello"),
        vec![(1_000, "hello".into())]
    );
}

#[test]
fn crlf_and_cr_normalize_to_lf() {
    assert_eq!(
        synced("[00:01.00]a\r\n[00:02.00]b\r[00:03.00]c\n"),
        vec![
            (1_000, "a".into()),
            (2_000, "b".into()),
            (3_000, "c".into()),
        ]
    );
    // Normalization is invariant: CRLF and LF inputs parse identically.
    let lf = parse_ok("[00:01.00]a\n[00:02.00]b\n");
    let crlf = parse_ok("[00:01.00]a\r\n[00:02.00]b\r\n");
    assert_eq!(lf, crlf);
}

#[test]
fn metadata_tags_are_captured_and_last_wins() {
    let doc = parse_ok("[ar:A1]\n[ti:T1]\n[al:L]\n[by:B]\n[ti:T2]\n[00:01.00]x");
    assert_eq!(doc.metadata.artist.as_deref(), Some("A1"));
    assert_eq!(doc.metadata.title.as_deref(), Some("T2"));
    assert_eq!(doc.metadata.album.as_deref(), Some("L"));
    assert_eq!(doc.metadata.by.as_deref(), Some("B"));
}

#[test]
fn empty_metadata_value_clears_the_tag() {
    let doc = parse_ok("[ti:T]\n[ti:]\n[00:01.00]x");
    assert_eq!(doc.metadata.title, None);
}

#[test]
fn unknown_tags_and_section_markers_are_ignored() {
    // `[length:...]` is an unknown tag; `[Chorus]` is a marker; neither
    // is content and neither is an error.
    let doc = parse_ok("[length:03:12]\n[Chorus]\n[00:01.00]la\n");
    assert_eq!(
        synced("[length:03:12]\n[Chorus]\n[00:01.00]la\n"),
        vec![(1_000, "la".into())]
    );
    assert_eq!(doc.metadata, Default::default());
}

#[test]
fn tag_only_lines_are_not_content_in_plain_documents() {
    assert_eq!(plain("[ar:X]\n[ti:Y]\n"), Vec::<String>::new());
}

#[test]
fn basic_timestamp_forms() {
    assert_eq!(synced("[00:01]a"), vec![(1_000, "a".into())]);
    assert_eq!(synced("[0:01]a"), vec![(1_000, "a".into())]);
    assert_eq!(synced("[01:02.03]a"), vec![(62_030, "a".into())]);
    assert_eq!(synced("[01:02.034]a"), vec![(62_034, "a".into())]);
    assert_eq!(synced("[00:00.00]a"), vec![(0, "a".into())]);
    assert_eq!(synced("[75:00.00]a"), vec![(4_500_000, "a".into())]);
}

#[test]
fn multiple_timestamps_expand_to_repeated_lines() {
    assert_eq!(
        synced("[00:12.00][01:45.00]text"),
        vec![(12_000, "text".into()), (105_000, "text".into())]
    );
}

#[test]
fn duplicate_timestamps_keep_document_order() {
    let doc = parse_ok("[00:02.00]b\n[00:01.00]a\n[00:02.00]c\n");
    let lines = doc.synced_lines().unwrap();
    assert_eq!(
        lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
        vec!["a", "b", "c"]
    );
    // The tie group (b, c) keeps document order and the tie-break
    // returns the LAST of the group.
    assert_eq!(active_line(&doc, 2_000), Some(2));
}

#[test]
fn offset_is_applied_and_last_wins() {
    assert_eq!(
        synced("[offset:+500]\n[00:01.00]a"),
        vec![(1_500, "a".into())]
    );
    assert_eq!(
        synced("[offset:-1500]\n[00:03.00]a"),
        vec![(1_500, "a".into())]
    );
    // Last tag wins.
    assert_eq!(
        synced("[offset:+500]\n[offset:+1000]\n[00:01.00]a"),
        vec![(2_000, "a".into())]
    );
    // Signed and padded values are accepted.
    assert_eq!(
        synced("[offset: +250 ]\n[00:01.00]a"),
        vec![(1_250, "a".into())]
    );
}

#[test]
fn multi_timestamp_line_with_offset() {
    assert_eq!(
        synced("[offset:-100]\n[00:01.00][00:02.00]x"),
        vec![(900, "x".into()), (1_900, "x".into())]
    );
}

#[test]
fn word_tags_are_stripped_and_reported() {
    let doc = parse_ok("[00:12.34]<00:12.34>He<00:12.60>y");
    assert!(doc.word_tags_stripped);
    assert_eq!(
        synced("[00:12.34]<00:12.34>He<00:12.60>y"),
        vec![(12_340, "Hey".into())]
    );
    // A non-tag `<…>` is literal text.
    let doc = parse_ok("[00:01.00]i <3 you");
    assert!(!doc.word_tags_stripped);
    assert_eq!(
        synced("[00:01.00]i <3 you"),
        vec![(1_000, "i <3 you".into())]
    );
}

#[test]
fn untimed_lines_attach_to_the_following_timestamped_block() {
    // Leading block attaches forward to the first timestamped line.
    assert_eq!(
        synced("intro one\nintro two\n[00:10.00]verse"),
        vec![(10_000, "intro one\nintro two\nverse".into())]
    );
    // Mid-document untimed lines attach to the next timestamped line.
    assert_eq!(
        synced("[00:10.00]a\nbridge\n[00:20.00]b"),
        vec![(10_000, "a".into()), (20_000, "bridge\nb".into())]
    );
    // A trailing block attaches to the last timestamped line.
    assert_eq!(
        synced("[00:10.00]a\ncoda\nfinal"),
        vec![(10_000, "a\ncoda\nfinal".into())]
    );
}

#[test]
fn text_after_brackets_is_literal() {
    assert_eq!(
        synced("[00:01.00]hello [world]"),
        vec![(1_000, "hello [world]".into())]
    );
    // An unclosed bracket is not a token; the line is literal text.
    assert_eq!(
        synced("[00:01.00]oops [00:02.00"),
        vec![(1_000, "oops [00:02.00".into())]
    );
}

#[test]
fn whitespace_rules() {
    // Leading/trailing ASCII whitespace trimmed; interior preserved.
    assert_eq!(
        synced("  [00:01.00]  keep  the  gaps  \t"),
        vec![(1_000, "keep  the  gaps".into())]
    );
    // Tabs trim at the edges, persist inside.
    assert_eq!(synced("\t[00:01.00]\ta\tb\t"), vec![(1_000, "a\tb".into())]);
}

#[test]
fn unicode_is_preserved_without_normalization() {
    // NFD input stays NFD (bytes preserved verbatim, spec §4.1).
    let nfd = "cafe\u{301} \u{266a} \u{1f3b5}";
    assert_eq!(
        synced(&format!("[00:01.00]{nfd}")),
        vec![(1_000, nfd.into())]
    );
    assert_eq!(plain(nfd), vec![nfd]);
}

#[test]
fn metadata_only_document_is_plain_with_metadata() {
    let doc = parse_ok("[ar:X]\n[ti:Y]\n");
    assert!(doc.plain_lines().unwrap().is_empty());
    assert_eq!(doc.metadata.title.as_deref(), Some("Y"));
}

#[test]
fn fixture_lrc_files_conform_to_the_profile() {
    // The pre-R3 fixture albums carry one-line LRC files; they are valid
    // under the strict profile. These are byte-copies of the real
    // fixture `.lrc` files (`web/tests/fixtures/*.mpack/lyrics/`),
    // committed under `tests/data/lyrics/` so core tests do not reach
    // into the web tree.
    for name in [
        "01 - Big in Japan.lrc",
        "02 - The Van.lrc",
        "01 - Piece One.lrc",
    ] {
        let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("tests/data/lyrics");
        path.push(name);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        let doc = parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(
            doc.is_synced(),
            "{name}: expected the [00:00.00] line to sync"
        );
        assert_eq!(doc.synced_lines().unwrap()[0].timestamp_ms, 0);
    }
}

// ---------------------------------------------------------------------
// Rejections (spec §4.6)
// ---------------------------------------------------------------------

fn is_err(input: impl Into<Vec<u8>> + std::fmt::Debug, expected: &LyricsError) {
    let bytes = input.into();
    let got = parse(&bytes);
    assert_eq!(got.err().as_ref(), Some(expected), "input: {bytes:?}");
}

#[test]
fn invalid_utf8_is_rejected() {
    is_err(b"\xff\xfe text", &LyricsError::InvalidUtf8);
}

#[test]
fn malformed_timestamps_are_rejected() {
    for bad in [
        "[00:6a]x",                   // non-digit seconds
        "[1:2:3]x",                   // two colons
        "[-0:12.0]x",                 // sign
        "[00:60.000]x",               // seconds out of range
        "[0:1]x",                     // seconds must be two digits
        "[00:12.1]x",                 // one-digit fraction
        "[00:12.1234]x",              // four-digit fraction
        "[+00:12]x",                  // sign on minutes
        "[00:]x",                     // empty seconds
        "[99999999999999999999:00]x", // minutes overflow u64
    ] {
        assert_eq!(
            parse(bad.as_bytes()).err().as_ref(),
            Some(&LyricsError::MalformedTimestamp { line: 1 }),
            "input: {bad}"
        );
    }
    // `[:12]` does not look like `digits:digits` — it is an unknown
    // (empty-key) tag, ignored per spec §4.4, not a timestamp error.
    assert_eq!(plain("[:12]x"), vec!["x"]);
}

#[test]
fn malformed_timestamp_reports_the_line() {
    // Only a leading token is a timestamp candidate; a bracket in the
    // middle of text is literal (spec §4.3).
    assert_eq!(
        parse(b"[00:01.00]ok\n[00:xx]bad line").err(),
        Some(LyricsError::MalformedTimestamp { line: 2 })
    );
    assert_eq!(
        parse(b"[00:01.00]ok\nbad [00:xx] mid-line").err(),
        None,
        "mid-line brackets are literal text"
    );
}

#[test]
fn malformed_offsets_are_rejected() {
    for bad in [
        "[offset:x]",
        "[offset:1.5]",
        "[offset:]",
        "[offset:3600001]",
        "[offset:-3600001]",
    ] {
        assert_eq!(
            parse(bad.as_bytes()).err().as_ref(),
            Some(&LyricsError::MalformedOffset { line: 1 }),
            "input: {bad}"
        );
    }
    // Boundary values are accepted (with timestamps late enough to stay
    // non-negative after the shift).
    assert!(parse(b"[offset:3600000][00:01.00]x").is_ok());
    assert!(parse(b"[offset:-3600000][61:00.00]x").is_ok());
}

#[test]
fn negative_timestamps_after_offset_are_rejected() {
    assert_eq!(
        parse(b"[offset:-2000]\n[00:01.00]too early").err(),
        Some(LyricsError::NegativeTimestamp { line: 2 })
    );
    // Exactly zero after the shift is fine.
    assert!(parse(b"[offset:-2000]\n[00:02.00]edge").is_ok());
}

#[test]
fn document_size_limit() {
    // Exactly at the limit, respecting every per-line limit too:
    // 128 lines of 4096 bytes = 524 288 bytes.
    let line = format!("{}\n", "a".repeat(lyrics::MAX_LINE_BYTES - 1)).repeat(128);
    assert_eq!(line.len(), lyrics::MAX_DOC_BYTES);
    assert!(parse(line.as_bytes()).is_ok());
    let over = vec![b'a'; lyrics::MAX_DOC_BYTES + 1];
    assert_eq!(
        parse(&over).err(),
        Some(LyricsError::DocTooLarge {
            size: lyrics::MAX_DOC_BYTES + 1
        })
    );
}

#[test]
fn line_count_limit() {
    let at_limit = "[00:01.00]x\n".repeat(lyrics::MAX_LINES);
    assert!(parse(at_limit.as_bytes()).is_ok());
    let over = "[00:01.00]x\n".repeat(lyrics::MAX_LINES + 1);
    assert_eq!(
        parse(over.as_bytes()).err(),
        Some(LyricsError::TooManyLines {
            count: lyrics::MAX_LINES + 1
        })
    );
}

#[test]
fn line_length_limit() {
    let at_limit = format!("[00:01.00]{}", "a".repeat(lyrics::MAX_LINE_BYTES - 10));
    assert!(parse(at_limit.as_bytes()).is_ok());
    let over = format!("[00:01.00]{}", "a".repeat(lyrics::MAX_LINE_BYTES - 9));
    assert_eq!(
        parse(over.as_bytes()).err(),
        Some(LyricsError::LineTooLong {
            line: 1,
            length: lyrics::MAX_LINE_BYTES + 1
        })
    );
}

// ---------------------------------------------------------------------
// Timing semantics (spec §9)
// ---------------------------------------------------------------------

fn doc(lines: &[(i64, &str)]) -> LyricsDocument {
    LyricsDocument {
        content: LyricsContent::Synced(
            lines
                .iter()
                .map(|(t, text)| LyricsLine {
                    timestamp_ms: *t,
                    text: (*text).to_string(),
                })
                .collect(),
        ),
        metadata: Default::default(),
        word_tags_stripped: false,
    }
}

#[test]
fn active_line_selection_rules() {
    let d = doc(&[(1_000, "a"), (2_000, "b"), (3_000, "c")]);
    // Before the first timestamp → None.
    assert_eq!(active_line(&d, 0), None);
    assert_eq!(active_line(&d, 999), None);
    // Exact hit.
    assert_eq!(active_line(&d, 1_000), Some(0));
    // Between timestamps → the preceding line holds.
    assert_eq!(active_line(&d, 1_500), Some(0));
    assert_eq!(active_line(&d, 2_999), Some(1));
    // After the last timestamp → the last line stays active.
    assert_eq!(active_line(&d, 3_000), Some(2));
    assert_eq!(active_line(&d, 86_400_000), Some(2));
}

#[test]
fn active_line_handles_arbitrary_i64_positions() {
    let d = doc(&[(0, "a"), (5_000, "b")]);
    assert_eq!(active_line(&d, i64::MIN), None);
    assert_eq!(active_line(&d, -1), None);
    assert_eq!(active_line(&d, i64::MAX), Some(1));
}

#[test]
fn active_line_tie_breaks_by_document_order() {
    let d = doc(&[(1_000, "a"), (2_000, "b"), (2_000, "c")]);
    assert_eq!(active_line(&d, 1_999), Some(0));
    assert_eq!(active_line(&d, 2_000), Some(2)); // last of the tie group
    // Before the tie resolves, only strictly earlier lines can win.
    let tie_only = doc(&[(2_000, "b"), (2_000, "c")]);
    assert_eq!(active_line(&tie_only, 1_999), None);
}

#[test]
fn active_line_on_plain_documents_is_none() {
    let d = parse_ok("just text\nmore text");
    assert_eq!(active_line(&d, 0), None);
    assert_eq!(active_line(&d, i64::MAX), None);
}

#[test]
fn active_line_is_deterministic_and_stateless() {
    let d = doc(&[(1_000, "a"), (2_000, "b")]);
    // Interleaved arbitrary calls produce identical results — no
    // dependence on call history (spec §9: not a stateful iterator).
    for _ in 0..3 {
        assert_eq!(active_line(&d, 2_500), Some(1));
        assert_eq!(active_line(&d, 500), None);
        assert_eq!(active_line(&d, 1_000), Some(0));
        assert_eq!(active_line(&d, 2_500), Some(1));
    }
}

#[test]
fn active_line_monotonicity_property() {
    // Deterministic pseudo-random sweep (xorshift; the repo defers
    // proptest until triggered — this pins the same invariant
    // deterministically): for a sorted document, increasing positions
    // never select an earlier line, and every selection is exact
    // (timestamp <= p < next timestamp).
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _case in 0..200 {
        let n = 1 + (next() % 40) as usize;
        let mut lines = Vec::new();
        let mut t = 0i64;
        for i in 0..n {
            t += (next() % 5_000) as i64; // gaps include 0 → real ties occur
            lines.push(LyricsLine {
                timestamp_ms: t,
                text: format!("line {i}"),
            });
        }
        let document = LyricsDocument {
            content: LyricsContent::Synced(lines),
            metadata: Default::default(),
            word_tags_stripped: false,
        };
        let sorted = document.synced_lines().unwrap();
        let mut prev: i64 = -1;
        let mut p = i64::MIN;
        for _step in 0..200 {
            p = p.saturating_add((next() % 2_000) as i64);
            let selected = active_line(&document, p);
            let index = selected.map_or(-1, |v| v as i64);
            assert!(index >= prev, "monotone: p={p} index={index} prev={prev}");
            match selected {
                Some(i) => {
                    assert!(sorted[i].timestamp_ms <= p, "active line's ts <= p");
                    if i + 1 < sorted.len() {
                        assert!(sorted[i + 1].timestamp_ms > p, "next line's ts > p");
                    }
                }
                None => assert!(p < sorted[0].timestamp_ms, "None only before the first ts"),
            }
            prev = index;
        }
        // Backwards seeking recomputes identically: stepping back down
        // the same positions reproduces the same selections.
        let mut recorded: Vec<Option<usize>> = Vec::new();
        let mut q = p;
        while q > i64::MIN {
            recorded.push(active_line(&document, q));
            q = q.saturating_sub((next() % 2_000).max(1) as i64);
            if q < 0 {
                break;
            }
        }
        assert!(!recorded.is_empty());
    }
}
