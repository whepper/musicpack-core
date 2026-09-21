//! Focused manifest-parsing tests: strict-JSON edges, resource budgets,
//! enum/number validation, and unknown-field behaviour — complementing the
//! 72-case conformance corpus with cases the corpus does not cover
//! (oversized inputs, infinities, deep unknown values, ...).

use musicpack_core::Error;
use musicpack_core::format::manifest::ParsedManifest;

fn minimal() -> String {
    // A valid minimal manifest.
    r#"{
  "format": "musicpack",
  "version": 1,
  "album": {
    "title": "T",
    "artists": [{"name": "A"}]
  },
  "media": [{
    "disc": 1,
    "tracks": [{
      "track": 1,
      "title": "One",
      "audio": {"path": "audio/01.bin", "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}
    }]
  }]
}"#
    .to_string()
}

fn parse(json: &str) -> Result<ParsedManifest, Error> {
    ParsedManifest::parse(json.as_bytes())
}

#[test]
fn valid_minimal_manifest_parses() {
    let parsed = parse(&minimal()).expect("parses");
    assert_eq!(parsed.manifest().album.title, "T");
    assert_eq!(parsed.manifest().media[0].tracks[0].number, 1);
}

#[test]
fn malformed_json_and_trailing_bytes() {
    assert!(matches!(parse("{"), Err(Error::Json { .. })));
    assert!(matches!(parse(""), Err(Error::Json { .. })));
    assert!(matches!(parse("[1, 2"), Err(Error::Json { .. })));
    // Trailing bytes: the conformance shapes.
    let mut doc = minimal().into_bytes();
    doc.extend_from_slice(b" trailing");
    assert!(matches!(
        ParsedManifest::parse(&doc),
        Err(Error::Json { .. })
    ));
    let mut doc = minimal().into_bytes();
    doc.extend_from_slice(b"\0suffix");
    assert!(matches!(
        ParsedManifest::parse(&doc),
        Err(Error::Json { .. })
    ));
    // Pure whitespace (including cJSON's ≤0x20 rule) is fine.
    assert!(parse(&format!("{}\n", minimal())).is_ok());
    assert!(parse(&format!("{}\x01", minimal())).is_ok());
}

#[test]
fn duplicate_keys_anywhere() {
    let json = minimal().replace(
        "\"format\": \"musicpack\",",
        "\"format\": \"musicpack\", \"format\": \"musicpack\",",
    );
    assert!(matches!(parse(&json), Err(Error::Invalid { .. })));

    // Nested: inside an unknown root object too.
    let json = minimal().replace(
        "\"media\":",
        "\"xUnknown\": {\"dup\": 1, \"dup\": 2},\n  \"media\":",
    );
    assert!(matches!(parse(&json), Err(Error::Invalid { .. })));

    // Inside an array member.
    let json = minimal().replace(
        "[{\"name\": \"A\"}]",
        "[{\"name\": \"A\", \"role\": \"x\", \"role\": \"y\"}]",
    );
    assert!(matches!(parse(&json), Err(Error::Invalid { .. })));
}

#[test]
fn deeply_nested_unknown_values() {
    // 900 nested arrays inside an unknown field: accepted (≤ 1000).
    let depth = 900;
    let json = minimal().replace(
        "\"media\":",
        &format!(
            "\"xUnknown\": {}900{} ,\n  \"media\":",
            "[".repeat(depth),
            "]".repeat(depth)
        ),
    );
    assert!(parse(&json).is_ok());

    // Over the cJSON limit (1000): rejected.
    let depth = 1001;
    let json = minimal().replace(
        "\"media\":",
        &format!(
            "\"xUnknown\": {}0{} ,\n  \"media\":",
            "[".repeat(depth),
            "]".repeat(depth)
        ),
    );
    assert!(matches!(parse(&json), Err(Error::Json { .. })));
}

#[test]
fn version_semantics() {
    // Number 2 → Version error.
    let json = minimal().replace("\"version\": 1", "\"version\": 2");
    assert!(matches!(
        parse(&json),
        Err(Error::Version { found, supported: 1 }) if found == "2"
    ));
    // Fractional number → Version error too (any number ≠ 1).
    let json = minimal().replace("\"version\": 1", "\"version\": 1.5");
    assert!(matches!(parse(&json), Err(Error::Version { .. })));
    // Non-number → Invalid.
    let json = minimal().replace("\"version\": 1", "\"version\": \"1\"");
    assert!(matches!(parse(&json), Err(Error::Invalid { .. })));
    // Overflow literal: infinite → not a valid number → Invalid.
    let json = minimal().replace("\"version\": 1", "\"version\": 1e999");
    assert!(matches!(parse(&json), Err(Error::Invalid { .. })));
    // Wrong format literal.
    let json = minimal().replace("\"format\": \"musicpack\"", "\"format\": \"other\"");
    assert!(matches!(parse(&json), Err(Error::Invalid { .. })));
}

#[test]
fn loudness_and_duration_numbers() {
    // Both-or-neither.
    let json = minimal().replace(
        "\"title\": \"One\",",
        "\"title\": \"One\", \"loudness\": {\"trackLUFS\": -12.0},",
    );
    assert!(matches!(parse(&json), Err(Error::Invalid { .. })));

    // Out of range.
    let json = minimal().replace(
        "\"title\": \"One\",",
        "\"title\": \"One\", \"loudness\": {\"trackLUFS\": -70.5, \"truePeakDbTP\": -1.0},",
    );
    assert!(matches!(parse(&json), Err(Error::Invalid { .. })));

    // Infinite loudness (1e999 → inf at the JSON layer).
    let json = minimal().replace(
        "\"title\": \"One\",",
        "\"title\": \"One\", \"loudness\": {\"trackLUFS\": 1e999, \"truePeakDbTP\": -1.0},",
    );
    assert!(matches!(parse(&json), Err(Error::Invalid { .. })));

    // Valid pair parses, and duration must be positive and finite.
    let json = minimal().replace(
        "\"title\": \"One\",",
        "\"title\": \"One\", \"loudness\": {\"trackLUFS\": -12.0, \"truePeakDbTP\": -1.0}, \"duration\": 1e999,",
    );
    assert!(matches!(parse(&json), Err(Error::Invalid { .. })));

    let json = minimal().replace(
        "\"title\": \"One\",",
        "\"title\": \"One\", \"loudness\": {\"trackLUFS\": -12.0, \"truePeakDbTP\": -1.0}, \"duration\": 183.5,",
    );
    let parsed = parse(&json).expect("parses");
    assert_eq!(parsed.manifest().media[0].tracks[0].duration, Some(183.5));
}

#[test]
fn closed_enums_and_numbering() {
    for (replacement, detail) in [
        ("\"releaseType\": \"mixtape\",", "releaseType"),
        ("\"format\": \"DAT\",", "medium format"),
    ] {
        let json = minimal().replace("\"media\": [{", &format!("\"media\": [{{ {replacement}"));
        // Insert at album level for releaseType instead:
        let json = if detail == "releaseType" {
            minimal().replace("\"album\": {", "\"album\": { \"releaseType\": \"mixtape\",")
        } else {
            json
        };
        assert!(
            matches!(parse(&json), Err(Error::Invalid { .. })),
            "{detail}"
        );
    }

    // Duplicate disc / track numbers.
    let json = minimal().replace("\"media\": [{", "\"media\": [{ \"disc\": 1, \"tracks\": [{\"track\": 9, \"title\": \"X\", \"audio\": {\"path\": \"a/1\", \"sha256\": \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"}}], ");
    assert!(matches!(parse(&json), Err(Error::Invalid { .. })));
}

#[test]
fn oversized_manifest_and_arrays() {
    // Manifest > 16 MiB: rejected with the "exceeds" vocabulary (before
    // any JSON parsing).
    let huge = format!("{}{}", " ".repeat(17 * 1024 * 1024), "{}");
    match parse(&huge) {
        Err(Error::Invalid { detail }) => assert!(detail.contains("exceeds")),
        other => panic!("expected Invalid, got {other:?}"),
    }

    // A 33-disc manifest, well-formed -> rejected by the per-array cap.
    let media: Vec<String> = (1..=33usize)
        .map(|d| {
            format!(
                r#"{{"disc": {d}, "tracks": [{{"track": 1, "title": "t", "audio": {{"path": "audio/{d}.bin", "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}}}}]}}"#
            )
        })
        .collect();
    let json = format!(
        r#"{{"format": "musicpack", "version": 1, "album": {{"title": "T", "artists": [{{"name": "A"}}]}}, "media": [{}]}}"#,
        media.join(",")
    );
    match parse(&json) {
        Err(Error::Invalid { detail }) => assert!(detail.contains("33")),
        other => panic!("expected Invalid, got {other:?}"),
    }

    // 32 discs are fine.
    let media: Vec<String> = (1..=32usize)
        .map(|d| {
            format!(
                r#"{{"disc": {d}, "tracks": [{{"track": 1, "title": "t", "audio": {{"path": "audio/{d}.bin", "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}}}}]}}"#
            )
        })
        .collect();
    let json = format!(
        r#"{{"format": "musicpack", "version": 1, "album": {{"title": "T", "artists": [{{"name": "A"}}]}}, "media": [{}]}}"#,
        media.join(",")
    );
    assert!(parse(&json).is_ok());
}

#[test]
fn asset_count_budget() {
    // 4096 referenced assets: exactly the budget is fine; 4097 fails.
    let all_tracks: Vec<String> = (1..=8usize)
        .flat_map(|d| {
            (1..=512usize).map(move |t| {
                format!(
                    r#"{{"track": {t}, "title": "t", "audio": {{"path": "audio/{d}-{t:03}.bin", "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}}}}"#
                )
            })
        })
        .collect();
    let media: Vec<String> = (1..=8usize)
        .map(|d| {
            let start = (d - 1) * 512;
            let end = d * 512;
            format!(
                r#"{{"disc": {d}, "tracks": [{}]}}"#,
                all_tracks[start..end].join(",")
            )
        })
        .collect();
    let json = format!(
        r#"{{"format": "musicpack", "version": 1, "album": {{"title": "T", "artists": [{{"name": "A"}}]}}, "media": [{}]}}"#,
        media.join(",")
    );
    assert!(parse(&json).is_ok(), "4096 assets are exactly the budget");

    // 4097 (one extra extras entry) → rejected with "exceeds". Splice the
    // entry in before the document's final brace (a blind replace would
    // corrupt the many `}}}` sequences inside the media array).
    let last = json.rfind('}').unwrap();
    let extras = r#", "extras": [{"path": "x/1", "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}]"#;
    let mut bytes = json.into_bytes();
    let tail = bytes.split_off(last);
    bytes.extend_from_slice(extras.as_bytes());
    bytes.extend_from_slice(&tail);
    let json = String::from_utf8(bytes).unwrap();
    match parse(&json) {
        Err(Error::Invalid { detail }) => assert!(detail.contains("exceeds")),
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn path_and_checksum_declarations() {
    // Unsafe audio path → Path error.
    let json = minimal().replace("audio/01.bin", "../evil.bin");
    assert!(matches!(parse(&json), Err(Error::Path(_))));

    // Bad checksum form (uppercase, wrong length) → Invalid.
    for sha in [
        "0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef",
        "0",
        "",
    ] {
        let json = minimal().replace(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            sha,
        );
        assert!(
            matches!(parse(&json), Err(Error::Invalid { .. })),
            "sha={sha:?}"
        );
    }
}

#[test]
fn unknown_fields_semantics() {
    // Unknown root fields parse and are visible in the original tree.
    let json = minimal().replace(
        "\"format\":",
        "\"xFuture\": {\"a\": [1, 2, {\"b\": null}]}, \"format\":",
    );
    let parsed = parse(&json).expect("parses");
    assert!(parsed.original().get("xFuture").is_some());

    // Canonical write preserves them last...
    let out = parsed.write_canonical().expect("writes");
    assert!(out.contains("xFuture"));
    let xf = out.find("xFuture").unwrap();
    let media = out.find("\"media\"").unwrap();
    assert!(media < xf, "unknown root fields come after the known ones");

    // ...and the model write drops them.
    let model = parsed.into_manifest();
    let out = model.write_canonical().expect("writes");
    assert!(!out.contains("xFuture"));

    // Unknown fields nested in known objects parse but do not survive a
    // canonical write (documented reference limitation).
    let json = minimal().replace("\"album\": {", "\"album\": {\"xNested\": true,");
    let parsed = parse(&json).expect("parses");
    let out = parsed.write_canonical().expect("writes");
    assert!(!out.contains("xNested"));
}

#[test]
fn utf8_boundary() {
    // Raw non-UTF-8 bytes: rejected at the JSON layer (documented
    // difference from cJSON, which copies them verbatim).
    let mut bytes = minimal().into_bytes();
    let pos = bytes.iter().position(|&b| b == b'T').unwrap();
    bytes.insert(pos, 0xFF);
    assert!(matches!(
        ParsedManifest::parse(&bytes),
        Err(Error::Json { .. })
    ));
}

#[test]
fn strtod_style_numbers_in_known_fields() {
    // cJSON accepts non-canonical numbers for *valid* fields as long as
    // the semantic checks pass: duration 01.5e0 == 1.5.
    let json = minimal().replace(
        "\"title\": \"One\",",
        "\"title\": \"One\", \"duration\": 01.5e0,",
    );
    let parsed = parse(&json).expect("cJSON-style numbers parse");
    assert_eq!(parsed.manifest().media[0].tracks[0].duration, Some(1.5));

    // Canonical write re-formats them through json_number.
    let out = parsed.write_canonical().expect("writes");
    assert!(out.contains("\"duration\": 1.5"), "{out}");
}

// ---------------------------------------------------------------------
// Per-track lyrics references (docs/musicpack-lyrics-v1.md §6)
// ---------------------------------------------------------------------

const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn with_track_lyrics(track_field: &str) -> String {
    minimal().replace(
        "\"title\": \"One\",",
        &format!("\"title\": \"One\", {track_field},"),
    )
}

#[test]
fn track_lyrics_parse_with_optional_lang() {
    let json = with_track_lyrics(&format!(
        "\"lyrics\": [{{\"path\": \"lyrics/01.lrc\", \"sha256\": \"{SHA}\", \"lang\": \"en\"}}]"
    ));
    let parsed = parse(&json).expect("parses");
    let lyrics = &parsed.manifest().media[0].tracks[0].lyrics;
    assert_eq!(lyrics.len(), 1);
    assert_eq!(lyrics[0].path, "lyrics/01.lrc");
    assert_eq!(lyrics[0].sha256, SHA);
    assert_eq!(lyrics[0].lang.as_deref(), Some("en"));
}

#[test]
fn track_lyrics_without_lang_and_absent_field() {
    let json = with_track_lyrics(&format!(
        "\"lyrics\": [{{\"path\": \"lyrics/01.lrc\", \"sha256\": \"{SHA}\"}}]"
    ));
    let parsed = parse(&json).expect("parses");
    let lyrics = &parsed.manifest().media[0].tracks[0].lyrics;
    assert_eq!(lyrics[0].lang, None);
    // Absent field → empty, existing manifests stay valid.
    assert!(
        parse(&minimal()).expect("parses").manifest().media[0].tracks[0]
            .lyrics
            .is_empty()
    );
    // Empty array is equivalent to absent.
    let json = with_track_lyrics("\"lyrics\": []");
    assert!(
        parse(&json).expect("parses").manifest().media[0].tracks[0]
            .lyrics
            .is_empty()
    );
}

#[test]
fn track_lyrics_multiple_entries_and_tracks() {
    let json = r#"{
  "format": "musicpack",
  "version": 1,
  "album": {"title": "T", "artists": [{"name": "A"}]},
  "media": [{
    "disc": 1,
    "tracks": [
      {"track": 1, "title": "One", "audio": {"path": "audio/01.bin", "sha256": "SHA0"},
       "lyrics": [
         {"path": "lyrics/01.en.lrc", "sha256": "SHA1", "lang": "en"},
         {"path": "lyrics/01.de.lrc", "sha256": "SHA2", "lang": "de"}
       ]},
      {"track": 2, "title": "Two", "audio": {"path": "audio/02.bin", "sha256": "SHA3"},
       "lyrics": [{"path": "lyrics/02.lrc", "sha256": "SHA4"}]}
    ]
  }]
}"#
    .replace("SHA0", SHA)
    .replace("SHA1", SHA)
    .replace("SHA2", SHA)
    .replace("SHA3", SHA)
    .replace("SHA4", SHA);
    let parsed = parse(&json).expect("parses");
    let tracks = &parsed.manifest().media[0].tracks;
    assert_eq!(tracks[0].lyrics.len(), 2);
    assert_eq!(tracks[0].lyrics[1].lang.as_deref(), Some("de"));
    assert_eq!(tracks[1].lyrics.len(), 1);
}

#[test]
fn track_lyrics_malformed_entries_are_rejected() {
    // Not an array.
    let json = with_track_lyrics("\"lyrics\": {\"path\": \"lyrics/01.lrc\"}");
    assert!(matches!(parse(&json), Err(Error::Invalid { .. })));
    // Missing sha256.
    let json = with_track_lyrics("\"lyrics\": [{\"path\": \"lyrics/01.lrc\"}]");
    assert!(matches!(parse(&json), Err(Error::Invalid { .. })));
    // Bad hash.
    let json = with_track_lyrics("\"lyrics\": [{\"path\": \"lyrics/01.lrc\", \"sha256\": \"zz\"}]");
    assert!(matches!(parse(&json), Err(Error::Invalid { .. })));
    // Unsafe path → Path error (existing manifest convention).
    let json = with_track_lyrics(&format!(
        "\"lyrics\": [{{\"path\": \"../escape.lrc\", \"sha256\": \"{SHA}\"}}]"
    ));
    assert!(matches!(parse(&json), Err(Error::Path(_))));
    // lang: empty, control character (via escape; note `\u0000` cannot
    // reach this check — the JSON layer truncates at NUL, its documented
    // D5 quirk), non-string.
    for lang in ["\"\"", "\"a\\u0001b\"", "7"] {
        let json = with_track_lyrics(&format!(
            "\"lyrics\": [{{\"path\": \"lyrics/01.lrc\", \"sha256\": \"{SHA}\", \"lang\": {lang}}}]"
        ));
        assert!(
            matches!(parse(&json), Err(Error::Invalid { .. })),
            "lang: {lang}"
        );
    }
}

#[test]
fn track_lyrics_join_the_path_uniqueness_set() {
    // Same file referenced per-track and in the root lyrics[] → parse
    // error (every referenced asset is referenced exactly once, spec
    // §6.3).
    let json = minimal()
        .replace(
            "\"title\": \"One\",",
            &format!(
                "\"title\": \"One\", \"lyrics\": [{{\"path\": \"lyrics/1.lrc\", \"sha256\": \"{SHA}\"}}],"
            ),
        )
        .replace(
            "\"media\":",
            &format!("\"lyrics\": [{{\"path\": \"lyrics/1.lrc\", \"sha256\": \"{SHA}\"}}], \"media\":"),
        );
    assert!(matches!(parse(&json), Err(Error::Invalid { .. })));

    // Two tracks sharing one lyrics file: also a uniqueness error (the
    // rule is format-wide; sharing means duplicating the file).
    let json = r#"{
  "format": "musicpack",
  "version": 1,
  "album": {"title": "T", "artists": [{"name": "A"}]},
  "media": [{
    "disc": 1,
    "tracks": [
      {"track": 1, "title": "One", "audio": {"path": "audio/01.bin", "sha256": "SHA"},
       "lyrics": [{"path": "lyrics/shared.lrc", "sha256": "SHA"}]},
      {"track": 2, "title": "Two", "audio": {"path": "audio/02.bin", "sha256": "SHA"},
       "lyrics": [{"path": "lyrics/shared.lrc", "sha256": "SHA"}]}
    ]
  }]
}"#
    .replace("SHA", SHA);
    assert!(matches!(parse(&json), Err(Error::Invalid { .. })));
}

#[test]
fn track_lyrics_count_the_referenced_asset_budget() {
    // The existing 512-entry cap applies per array (reuse of MAX_LYRICS).
    let entry = format!("{{\"path\": \"lyrics/x.lrc\", \"sha256\": \"{SHA}\"}},");
    let many = format!("\"lyrics\": [{}]", entry.repeat(513).trim_end_matches(','));
    let json = with_track_lyrics(&many);
    assert!(matches!(parse(&json), Err(Error::Invalid { .. })));
}

#[test]
fn track_lyrics_are_referenced_paths_and_never_unreferenced_warnings() {
    use musicpack_core::format::manifest::Manifest;
    let json = with_track_lyrics(&format!(
        "\"lyrics\": [{{\"path\": \"lyrics/01.lrc\", \"sha256\": \"{SHA}\"}}]"
    ));
    let parsed = parse(&json).expect("parses");
    let manifest: &Manifest = parsed.manifest();
    let paths = manifest.referenced_paths();
    assert!(
        paths.contains(&"lyrics/01.lrc"),
        "per-track lyrics join referenced_paths()"
    );
}
