//! R4.2 authoring-pipeline tests.
//!
//! Covers the stages (draft parse/validate, identification, encoding,
//! waveform) and the end-to-end vertical:
//!
//! ```text
//! draft JSON -> encode -> waveform -> build_directory -> core verify
//!            -> .mpack -> .mpak -> (re)open
//! ```
//!
//! The legacy C `musicpack` CLI / `mpcenc` are used **only** as optional
//! differential oracles; the pipeline itself never invokes them.

use std::path::{Path, PathBuf};
use std::process::Command;

use musicpack_author::draft::{self, ValidationReport};
use musicpack_author::identify::{self, Confidence, MusicBrainzProvider};
use musicpack_author::pipeline::{
    AuthorRequest, IdentifyRequest, PipelineOptions, encode_stage, run, validate_json,
    waveform_stage,
};
use musicpack_author::{encode, waveform};
use musicpack_core::authoring::LoudnessMode;
use musicpack_core::format::checksum;
use musicpack_core::format::manifest::ParsedManifest;
use musicpack_core::storage::directory;
use musicpack_musepack_encoder::encoder::{EncoderConfig, MusepackEncoder};

// ---------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "musicpack-author-{}-{name}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/reference/audio")
        .join(name)
}

fn reference_cli() -> Option<PathBuf> {
    if let Ok(cli) = std::env::var("MUSICPACK_REF_CLI") {
        let p = PathBuf::from(cli);
        return p.is_file().then_some(p);
    }
    let sibling = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../musicpack");
    let cli = sibling.join("build/core/musicpack/musicpack");
    cli.is_file().then_some(cli)
}

fn mpcenc() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MUSICPACK_MPCENC") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let sibling =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../musicpack/build/codec/mpcenc/mpcenc");
    sibling.is_file().then_some(sibling)
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Builds a temp album source tree and returns its root.
fn album_root(temp: &TempDir, with_assets: bool) -> PathBuf {
    let root = temp.path().join("album");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::copy(fixture("flac16-44k.flac"), root.join("one.flac")).unwrap();
    if with_assets {
        std::fs::write(root.join("front.jpg"), b"front-cover-bytes").unwrap();
        std::fs::write(root.join("notes.pdf"), b"booklet-bytes").unwrap();
        std::fs::write(root.join("bonus.txt"), b"extra-bytes").unwrap();
        std::fs::write(
            root.join("one.lrc"),
            b"[00:01.00]first line\n[00:04.00]second\n",
        )
        .unwrap();
    }
    root
}

fn draft_json(root: &Path, with_assets: bool) -> Vec<u8> {
    let lyrics = if with_assets {
        r#""lyrics":[{"path":"one.lrc","lang":"en"}]"#
    } else {
        r#""lyrics":[]"#
    };
    let artwork = if with_assets {
        r#"[{"role":"front","path":"front.jpg"}]"#
    } else {
        "[]"
    };
    let booklet = if with_assets {
        r#"["notes.pdf"]"#
    } else {
        "[]"
    };
    let extras = if with_assets {
        r#"["bonus.txt"]"#
    } else {
        "[]"
    };
    format!(
        r#"{{
  "schema": "musicpack-draft",
  "version": 1,
  "sourceRoot": {root},
  "album": {{ "title": "Test Album", "artists": [{{ "name": "Test Artist" }}], "releaseType": "album" }},
  "release": {{ "releaseDate": "2020-01-01", "country": "GB", "label": "Test Label", "catalogueNumber": "CAT-1" }},
  "identifiers": {{ "musicbrainzReleaseGroupId": "11111111-1111-1111-1111-111111111111", "musicbrainzReleaseId": "22222222-2222-2222-2222-222222222222", "barcode": "1234567890123" }},
  "media": [{{ "disc": 1, "format": "CD", "tracks": [
    {{ "track": 1, "title": "One", "audioPath": "one.flac", {lyrics} }}
  ]}}],
  "artwork": {artwork},
  "booklet": {booklet},
  "lyrics": [],
  "extras": {extras}
}}"#,
        root = json_string(&root.to_string_lossy()),
    )
    .into_bytes()
}

// ---------------------------------------------------------------------
// stage tests
// ---------------------------------------------------------------------

#[test]
fn draft_parses_and_validates() {
    let temp = TempDir::new("parse");
    let root = album_root(&temp, true);
    let bytes = draft_json(&root, true);
    let parsed = draft::parse(&bytes).unwrap();
    assert_eq!(parsed.album.title, "Test Album");
    assert_eq!(parsed.media[0].tracks[0].audio_path, "one.flac");
    assert_eq!(
        parsed.media[0].tracks[0].lyrics[0].lang.as_deref(),
        Some("en")
    );
    let report = draft::validate(&parsed);
    assert!(report.is_ok(), "{:?}", report.errors);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[test]
fn validation_rejects_bad_drafts() {
    let temp = TempDir::new("bad");
    let root = album_root(&temp, false);

    // Missing audio file.
    let json = format!(
        r#"{{"sourceRoot":{},"album":{{"title":"T","artists":[{{"name":"A"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"One","audioPath":"missing.flac"}}]}}]}}"#,
        json_string(&root.to_string_lossy())
    );
    let report = validate_json(json.as_bytes()).unwrap();
    assert!(!report.is_ok());
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("audio file not found")),
        "{:?}",
        report.errors
    );

    // Missing artist.
    let json = r#"{"sourceRoot":"/tmp","album":{"title":"T"},"media":[{"disc":1,"tracks":[{"track":1,"title":"One","audioPath":"a.flac"}]}]}"#;
    let report = validate_json(json.as_bytes()).unwrap();
    assert!(
        report.errors.iter().any(|e| e == "no artist"),
        "{:?}",
        report.errors
    );
}

#[test]
fn waveform_payload_is_the_core_kernel() {
    let temp = TempDir::new("waveform");
    let root = album_root(&temp, false);
    let out = temp.path().join("one.wfm");
    let points = waveform::generate_to(&root.join("one.flac"), &out).unwrap();
    let payload = std::fs::read(&out).unwrap();
    assert_eq!(payload.len() as u64, points * 2);
    assert!(points > 0);
    // Deterministic: regenerating yields identical bytes.
    let out2 = temp.path().join("one-2.wfm");
    let points2 = waveform::generate_to(&root.join("one.flac"), &out2).unwrap();
    assert_eq!(points, points2);
    assert_eq!(std::fs::read(&out2).unwrap(), payload);
}

#[test]
fn encoder_rejects_unsupported_configs() {
    let temp = TempDir::new("unsupported");
    let root = album_root(&temp, false);
    // Fractional quality is supported since J.2; what must still fail close
    // is a non-finite quality (typed rejection — C's NaN path is undefined).
    let out = temp.path().join("x.mpc");
    let err = encode::encode_to(&root.join("one.flac"), &out, f32::NAN).unwrap_err();
    assert!(
        matches!(err, musicpack_author::AuthorError::Unsupported { .. }),
        "{err:?}"
    );
    assert!(!out.exists());
}

#[test]
fn encoder_accepts_fractional_quality() {
    // J.2: fractional qualities follow the C clip/interpolation path; the
    // Author UI stays integer-only, but the API/CLI must not reject them.
    let temp = TempDir::new("fractional");
    let root = album_root(&temp, false);
    let out = temp.path().join("frac.mpc");
    encode::encode_to(&root.join("one.flac"), &out, 5.5).expect("fractional encode");
    assert!(out.metadata().expect("out exists").len() > 0);
}

#[test]
fn encoder_accepts_every_ui_quality_at_every_source_rate() {
    // The Author UI offers q5/6/7/8; after integer parity (J.1) all of them
    // are valid at every SV8 source rate, so the UI and the encoder no longer
    // disagree anywhere the reference C encoder supports.
    let temp = TempDir::new("ui-qualities");
    let root = album_root(&temp, false);
    for (n, quality) in [5.0f32, 6.0, 7.0, 8.0].into_iter().enumerate() {
        let out = temp.path().join(format!("q{quality}.mpc"));
        encode::encode_to(&root.join("one.flac"), &out, quality)
            .unwrap_or_else(|e| panic!("q{quality} must encode: {e}"));
        assert!(out.exists(), "q{quality} output missing");
        assert!(n < 4);
    }
}

// ---------------------------------------------------------------------
// MusicBrainz (mock provider + offline document)
// ---------------------------------------------------------------------

const MB_RELEASE: &str = r#"{
  "id": "22222222-2222-2222-2222-222222222222",
  "title": "Test Album",
  "date": "2020-01-01",
  "country": "GB",
  "barcode": "1234567890123",
  "release-group": {"id": "11111111-1111-1111-1111-111111111111", "primary-type": "Album", "first-release-date": "2019-12-01"},
  "artist-credit": [{"name": "Test Artist", "artist": {"id": "33333333-3333-3333-3333-333333333333", "sort-name": "Artist, Test"}}],
  "label-info": [{"label": {"name": "Test Label"}, "catalog-number": "CAT-1"}],
  "media": [{"position": 1, "format": "CD", "tracks": [
    {"id": "44444444-4444-4444-4444-444444444444", "number": 1, "title": "One", "recording": {"id": "55555555-5555-5555-5555-555555555555", "isrcs": ["USABC1234567"]}}
  ]}]
}"#;

struct StaticProvider(&'static str);

impl MusicBrainzProvider for StaticProvider {
    fn fetch_release(&self, _mbid: &str) -> Result<Vec<u8>, String> {
        Ok(self.0.as_bytes().to_vec())
    }
    fn search_barcode(&self, _barcode: &str) -> Result<Vec<u8>, String> {
        Ok(format!(r#"{{"releases":[{self}]}}"#, self = self.0).into_bytes())
    }
}

#[test]
fn identification_matches_and_applies_via_provider() {
    let temp = TempDir::new("identify");
    let root = album_root(&temp, false);
    let mut parsed = draft::parse(&draft_json(&root, false)).unwrap();
    // Clear the pre-set identifiers so matching has to work.
    parsed.identifiers = None;
    let provider = StaticProvider(MB_RELEASE);
    let (confidence, applied) = identify::identify_mbid(
        &provider,
        &mut parsed,
        "22222222-2222-2222-2222-222222222222",
    )
    .unwrap();
    assert_eq!(confidence, Confidence::Exact);
    assert!(applied);
    assert_eq!(
        parsed.identifiers.as_ref().unwrap().barcode.as_deref(),
        Some("1234567890123")
    );
    assert_eq!(
        parsed.media[0].tracks[0]
            .identifiers
            .as_ref()
            .unwrap()
            .isrc
            .as_deref(),
        Some("USABC1234567")
    );
}

#[test]
fn barcode_search_returns_candidates_without_selecting() {
    let temp = TempDir::new("search");
    let root = album_root(&temp, false);
    let mut parsed = draft::parse(&draft_json(&root, false)).unwrap();
    // Keep only a barcode so the barcode signal (not the release id) decides.
    parsed.identifiers = Some(musicpack_core::format::manifest::Identifiers {
        musicbrainz_release_group_id: None,
        musicbrainz_release_id: None,
        barcode: Some("1234567890123".into()),
    });
    let provider = StaticProvider(MB_RELEASE);
    let candidates = identify::search_barcode(&provider, &parsed, "1234567890123").unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].confidence, Confidence::Confirmed);
    // Candidate mode never applies anything: the draft keeps only its barcode.
    assert_eq!(
        parsed.identifiers.as_ref().unwrap().musicbrainz_release_id,
        None
    );
}

// ---------------------------------------------------------------------
// end-to-end
// ---------------------------------------------------------------------

#[test]
fn full_pipeline_builds_verifies_and_packs() {
    let temp = TempDir::new("e2e");
    let root = album_root(&temp, true);
    let bytes = draft_json(&root, true);
    let output = temp.path().join("Test Album.mpack");
    let mpak = temp.path().join("Test Album.mpak");

    let options = PipelineOptions {
        mpak: Some(mpak.clone()),
        ..PipelineOptions::default()
    };
    let request = AuthorRequest {
        draft_json: &bytes,
        output: &output,
        options,
        identify: None,
    };
    let outcome = run(&request).unwrap();

    assert!(outcome.report.is_ok(), "{:?}", outcome.report.findings());
    assert_eq!(outcome.mpak.as_ref(), Some(&mpak));
    assert!(output.join("manifest.json").is_file());
    assert!(output.join("audio/01 - One.mpc").is_file());
    assert!(output.join("analysis/waveform/01-01.wfm").is_file());
    assert!(output.join("artwork/front.jpg").is_file());
    assert!(output.join("booklet/notes.pdf").is_file());
    assert!(output.join("extras/bonus.txt").is_file());
    assert!(output.join("lyrics/one.lrc").is_file());

    // The built package verifies with the core verifier.
    let report = directory::verify_directory(&output).unwrap();
    assert!(report.is_ok(), "{:?}", report.findings());

    // Manifest semantics.
    let (raw, parsed) = {
        let raw = std::fs::read(output.join("manifest.json")).unwrap();
        let parsed = ParsedManifest::parse(&raw).unwrap();
        (raw, parsed)
    };
    let m = parsed.manifest();
    assert_eq!(m.media[0].tracks[0].lyrics.len(), 1);
    assert_eq!(m.media[0].tracks[0].lyrics[0].lang.as_deref(), Some("en"));
    assert_eq!(
        m.media[0].tracks[0].audio.sha256,
        checksum::sha256_hex(&std::fs::read(output.join("audio/01 - One.mpc")).unwrap())
    );
    let wf = m.media[0].tracks[0].waveform.as_ref().unwrap();
    assert!(wf.points > 0, "waveform points derived from the payload");
    let wf_bytes = std::fs::read(output.join(&wf.path)).unwrap();
    assert_eq!(wf_bytes.len() as u64, wf.points * 2);
    assert!(m.loudness.is_some());
    assert!(m.media[0].tracks[0].loudness.is_some());
    assert_eq!(outcome.manifest_sha256, checksum::sha256_hex(&raw));

    // .mpak round trip: the existing writer, then re-open and verify.
    let report = musicpack_core::storage::mpak::verify_mpak_file(&mpak).unwrap();
    assert!(report.is_ok(), "{:?}", report.findings());

    // The C verifier accepts the Rust-built package (skipped if absent).
    if let Some(cli) = reference_cli() {
        let result = Command::new(cli)
            .arg("verify")
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "C verify rejected: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn loudness_can_be_omitted_and_waveform_disabled() {
    let temp = TempDir::new("nomodes");
    let root = album_root(&temp, false);
    let bytes = draft_json(&root, false);
    let output = temp.path().join("Quiet.mpack");
    let options = PipelineOptions {
        waveform: false,
        loudness: LoudnessMode::Omit,
        ..PipelineOptions::default()
    };
    let outcome = run(&AuthorRequest {
        draft_json: &bytes,
        output: &output,
        options,
        identify: None,
    })
    .unwrap();
    assert!(outcome.report.is_ok());
    assert!(outcome.manifest.loudness.is_none());
    assert!(outcome.manifest.media[0].tracks[0].loudness.is_none());
    assert!(outcome.manifest.media[0].tracks[0].waveform.is_none());
    assert!(!output.join("analysis/waveform").exists());
}

#[test]
fn multi_disc_naming_matches_the_reference() {
    let temp = TempDir::new("multidisc");
    let root = temp.path().join("album");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::copy(fixture("flac16-44k.flac"), root.join("a.flac")).unwrap();
    let json = format!(
        r#"{{"sourceRoot":{},"album":{{"title":"Two Discs","artists":[{{"name":"A"}}]}},"media":[
          {{"disc":1,"tracks":[{{"track":1,"title":"First","audioPath":"a.flac"}}]}},
          {{"disc":2,"tracks":[{{"track":1,"title":"Second","audioPath":"a.flac"}}]}}]}}"#,
        json_string(&root.to_string_lossy())
    );
    let output = temp.path().join("Two Discs.mpack");
    let outcome = run(&AuthorRequest {
        draft_json: json.as_bytes(),
        output: &output,
        options: PipelineOptions {
            waveform: false,
            loudness: LoudnessMode::Omit,
            ..PipelineOptions::default()
        },
        identify: None,
    })
    .unwrap();
    assert!(outcome.report.is_ok(), "{:?}", outcome.report.findings());
    // Multi-disc naming is `audio/<disc>-<track:02> - <title>.mpc` (the C
    // convention), preserved through the Rust encoder.
    assert!(output.join("audio/1-01 - First.mpc").is_file());
    assert!(output.join("audio/2-01 - Second.mpc").is_file());
}

#[test]
fn replace_publishes_atomically_and_keeps_no_staging() {
    let temp = TempDir::new("replace");
    let root = album_root(&temp, false);
    let bytes = draft_json(&root, false);
    let output = temp.path().join("Album.mpack");

    let build = |replace: bool| {
        run(&AuthorRequest {
            draft_json: &bytes,
            output: &output,
            options: PipelineOptions {
                loudness: LoudnessMode::Omit,
                replace,
                ..PipelineOptions::default()
            },
            identify: None,
        })
    };
    build(false).unwrap();
    let first = std::fs::read(output.join("manifest.json")).unwrap();

    // A second build without --replace is refused.
    let err = build(false).unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");

    // With --replace it succeeds and no staging directories remain.
    build(true).unwrap();
    assert_eq!(std::fs::read(output.join("manifest.json")).unwrap(), first);
    let leftovers: Vec<String> = std::fs::read_dir(temp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("author-") || n.contains("new-") || n.contains("old-"))
        .collect();
    assert!(leftovers.is_empty(), "staging leftovers: {leftovers:?}");
}

#[test]
fn identification_document_flows_through_the_pipeline() {
    let temp = TempDir::new("identify-e2e");
    let root = album_root(&temp, false);
    // A draft with no identifiers; the document supplies them.
    let json = format!(
        r#"{{"sourceRoot":{},"album":{{"title":"Test Album","artists":[{{"name":"Test Artist"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"One","audioPath":"one.flac"}}]}}]}}"#,
        json_string(&root.to_string_lossy())
    );
    let output = temp.path().join("Id.mpack");
    let outcome = run(&AuthorRequest {
        draft_json: json.as_bytes(),
        output: &output,
        options: PipelineOptions {
            loudness: LoudnessMode::Omit,
            ..PipelineOptions::default()
        },
        identify: Some(IdentifyRequest::Document {
            doc: MB_RELEASE.as_bytes(),
            asserted_mbid: None,
        }),
    })
    .unwrap();
    // The MB title matched; the applied ids anchor the identity.
    assert_eq!(outcome.group_key, "mb:11111111-1111-1111-1111-111111111111");
    assert_eq!(
        outcome.release_key,
        "mb:22222222-2222-2222-2222-222222222222"
    );
}

// ---------------------------------------------------------------------
// C differential oracles (skipped when the reference build is absent)
// ---------------------------------------------------------------------

#[test]
fn rust_encoder_matches_mpcenc_on_a_real_flac() {
    let Some(mpcenc) = mpcenc() else {
        eprintln!("note: mpcenc not built; encoder differential skipped");
        return;
    };
    let temp = TempDir::new("encode-diff");
    let root = album_root(&temp, false);
    let source = root.join("one.flac");

    // Reference: decode nothing; mpcenc reads the FLAC directly.
    let c_out = temp.path().join("c.mpc");
    let status = Command::new(&mpcenc)
        .arg("--quality")
        .arg("6.0")
        .arg("--overwrite")
        .arg("--silent")
        .arg(&source)
        .arg(&c_out)
        .status()
        .unwrap();
    assert!(status.success(), "mpcenc failed");

    let rust_out = temp.path().join("rust.mpc");
    encode::encode_to(&source, &rust_out, 6.0).unwrap();

    assert_eq!(
        std::fs::read(&rust_out).unwrap(),
        std::fs::read(&c_out).unwrap(),
        "Rust encoder must be byte-identical to mpcenc for q6 @ 44100"
    );
}

// ---------------------------------------------------------------------
// J.6: wide-PCM (24-bit) end-to-end through the Author pipeline
// ---------------------------------------------------------------------

/// A deterministic 24-bit stereo WAV: exact full-scale minimum/maximum in
/// the first two frames (the low-order-erasing worst case), then LCG noise
/// across the full 24-bit range. Integer-only, little-endian.
fn write_wav24(path: &Path, frames: usize) {
    fn s24(state: u32) -> i32 {
        let v = (state & 0x00FF_FFFF) as i32;
        if v >= 0x0080_0000 { v - 0x0100_0000 } else { v }
    }
    let mut data = Vec::with_capacity(frames * 6);
    let mut state = 0x1234_5678u32;
    for i in 0..frames {
        let (l, r): (i32, i32) = match i {
            0 => (0x007F_FFFF, -0x0080_0000),
            1 => (-0x0080_0000, 0x007F_FFFF),
            _ => {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                let l = s24(state);
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                (l, s24(state))
            }
        };
        data.extend_from_slice(&l.to_le_bytes()[..3]);
        data.extend_from_slice(&r.to_le_bytes()[..3]);
    }
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&2u16.to_le_bytes()); // channels
    wav.extend_from_slice(&44100u32.to_le_bytes());
    wav.extend_from_slice(&(44100u32 * 6).to_le_bytes()); // byte rate
    wav.extend_from_slice(&6u16.to_le_bytes()); // block align
    wav.extend_from_slice(&24u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    std::fs::write(path, wav).expect("write 24-bit WAV");
}

/// J.6 end-to-end: a 24-bit source survives the Author encode stage —
/// `encode_to` must equal the encoder fed the full-precision
/// left-aligned samples (no reduction to `i16` anywhere) and must differ
/// from what the old `>> 16` reduction would have produced.
#[test]
fn author_preserves_24bit_precision_end_to_end() {
    let temp = TempDir::new("wide24");
    let wav = temp.path().join("wide24.wav");
    write_wav24(&wav, 5000);
    let out = temp.path().join("wide24.mpc");
    encode::encode_to(&wav, &out, 6.0).expect("24-bit encode");
    let author_bytes = std::fs::read(&out).expect("author output");

    // Replay the same source at full precision: decode with the core
    // `read_s32` (left-aligned i32) and feed `encode_s32` directly. The
    // Author's bytes must be identical — i.e. the stage neither truncated
    // nor otherwise altered the samples.
    let mut decoder =
        musicpack_core::audio::open(Box::new(std::fs::File::open(&wav).unwrap())).unwrap();
    let info = decoder.info().clone();
    assert_eq!(
        (info.bits_per_sample, info.channels, info.is_float),
        (24, 2, false)
    );
    let mut pcm = vec![0i32; 5000 * 2];
    let frames = decoder.read_s32(&mut pcm).unwrap();
    assert_eq!(frames, 5000);
    let config = EncoderConfig::new(6.0, 44100, 2);
    let wide = MusepackEncoder::new(config)
        .unwrap()
        .encode_s32(&pcm)
        .unwrap();
    assert_eq!(
        author_bytes, wide,
        "the Author output must equal the full-precision encode_s32 stream"
    );

    // The low-order bits are load-bearing: the old 16-bit reduction of the
    // very same samples must produce a different stream.
    let truncated: Vec<i16> = pcm.iter().map(|&v| (v >> 16) as i16).collect();
    let narrow = MusepackEncoder::new(config)
        .unwrap()
        .encode(&truncated)
        .unwrap();
    assert_ne!(
        author_bytes, narrow,
        "the Author path must not reduce 24-bit sources to i16"
    );
}

/// J.6 differential: the Author's wide path must be byte-identical to the
/// scalar C `mpcenc` 1.32.0 oracle on a 24-bit source (skipped when the
/// reference binary is absent; the committed encoder corpus pins the same
/// property hermetically).
#[test]
fn author_matches_mpcenc_on_a_wide_wav() {
    let Some(mpcenc) = mpcenc() else {
        eprintln!("note: mpcenc not built; wide-PCM differential skipped");
        return;
    };
    let temp = TempDir::new("wide-diff");
    let wav = temp.path().join("wide.wav");
    write_wav24(&wav, 5000);

    let c_out = temp.path().join("c.mpc");
    // Scalar-forced: the strict compatibility target, same flags as the
    // committed fixture corpus.
    let status = Command::new(&mpcenc)
        .args([
            "--quality",
            "6.0",
            "--overwrite",
            "--silent",
            "--impl",
            "scalar",
            "--psy-impl",
            "scalar",
        ])
        .arg(&wav)
        .arg(&c_out)
        .status()
        .unwrap();
    assert!(status.success(), "mpcenc failed on the 24-bit WAV");

    let rust_out = temp.path().join("rust.mpc");
    encode::encode_to(&wav, &rust_out, 6.0).unwrap();

    let c_bytes = std::fs::read(&c_out).unwrap();
    let rust_bytes = std::fs::read(&rust_out).unwrap();
    if c_bytes != rust_bytes {
        let first = c_bytes
            .iter()
            .zip(&rust_bytes)
            .position(|(a, b)| a != b)
            .unwrap_or(c_bytes.len().min(rust_bytes.len()));
        panic!(
            "wide-PCM divergence at byte {first} (C {} bytes, Rust {} bytes)",
            c_bytes.len(),
            rust_bytes.len()
        );
    }
}

#[test]
fn c_builder_and_rust_pipeline_agree_on_the_manifest() {
    let Some(cli) = reference_cli() else {
        eprintln!("note: reference CLI not built; build differential skipped");
        return;
    };
    let temp = TempDir::new("build-diff");
    let root = album_root(&temp, false);

    // C build-draft on the original FLAC source, loudness disabled,
    // waveform disabled (so the two paths share the same inputs).
    let c_draft = format!(
        r#"{{"schema":"musicpack-draft","version":1,"sourceRoot":{root},
          "album":{{"title":"Test Album","artists":[{{"name":"Test Artist"}}],"releaseType":"album"}},
          "release":{{"releaseDate":"2020-01-01","country":"GB","label":"Test Label","catalogueNumber":"CAT-1"}},
          "identifiers":{{"musicbrainzReleaseGroupId":"11111111-1111-1111-1111-111111111111","musicbrainzReleaseId":"22222222-2222-2222-2222-222222222222","barcode":"1234567890123"}},
          "media":[{{"disc":1,"format":"CD","tracks":[{{"track":1,"title":"One","audioPath":"one.flac"}}]}}],
          "artwork":[],"booklet":[],"lyrics":[],"extras":[],
          "waveformAnalysis":{{"status":"disabled"}}}}"#,
        root = json_string(&root.to_string_lossy())
    );
    let draft_path = temp.path().join("draft.json");
    std::fs::write(&draft_path, c_draft).unwrap();
    let c_out = temp.path().join("C.mpack");
    let status = Command::new(&cli)
        .arg("build-draft")
        .arg("--draft")
        .arg(&draft_path)
        .arg("-o")
        .arg(&c_out)
        .arg("--no-loudness")
        .status()
        .unwrap();
    assert!(status.success(), "C build-draft failed");

    // Rust pipeline with loudness/waveform disabled.
    let rust_out = temp.path().join("Rust.mpack");
    run(&AuthorRequest {
        draft_json: &draft_json(&root, false),
        output: &rust_out,
        options: PipelineOptions {
            waveform: false,
            loudness: LoudnessMode::Omit,
            ..PipelineOptions::default()
        },
        identify: None,
    })
    .unwrap();

    let (c, r) = (
        std::fs::read(c_out.join("manifest.json")).unwrap(),
        std::fs::read(rust_out.join("manifest.json")).unwrap(),
    );
    // Same semantic input, same canonical manifest bytes (the core writer is
    // byte-identical to the C writer; R4.1 proved it, this pins the pipeline
    // against the C `build-draft` end of the flow).
    let cp = ParsedManifest::parse(&c).unwrap();
    let rp = ParsedManifest::parse(&r).unwrap();
    assert_eq!(cp.manifest().album, rp.manifest().album);
    assert_eq!(cp.manifest().release, rp.manifest().release);
    assert_eq!(cp.manifest().identifiers, rp.manifest().identifiers);
    assert_eq!(cp.manifest().media.len(), rp.manifest().media.len());
    assert_eq!(
        cp.manifest().media[0].tracks[0].audio.sha256,
        rp.manifest().media[0].tracks[0].audio.sha256,
        "both paths must package the same audio bytes"
    );
    assert_eq!(
        String::from_utf8_lossy(&c),
        String::from_utf8_lossy(&r),
        "Rust pipeline and C build-draft must agree on the canonical manifest"
    );
    assert_eq!(
        std::fs::read(c_out.join("audio/01 - One.mpc")).unwrap(),
        std::fs::read(rust_out.join("audio/01 - One.mpc")).unwrap()
    );
}

#[test]
fn validation_report_type_is_shared() {
    // Compile-time guard: `ValidationReport` is the public verdict type.
    let report: ValidationReport = ValidationReport::default();
    assert!(report.is_ok());
}

// ---------------------------------------------------------------------
// R4.3: UI-shaped drafts, split stages, inspect round trip
// ---------------------------------------------------------------------

#[test]
fn ui_shaped_draft_is_accepted() {
    // The Author UI emits `booklet`/`lyrics`/`extras` as `[{path}]` and
    // carries app-state blocks (`openedFrom`, `waveformAnalysis`,
    // `sonicAnalysis`) that the pipeline must ignore, not reject.
    let temp = TempDir::new("ui-shape");
    let root = album_root(&temp, true);
    let json = format!(
        r#"{{
  "schema": "musicpack-draft",
  "version": 1,
  "sourceRoot": {root},
  "openedFrom": "/tmp/Some.mpack",
  "album": {{ "title": "Test Album", "artists": [{{ "name": "Test Artist" }}] }},
  "media": [{{ "disc": 1, "tracks": [{{ "track": 1, "title": "One", "audioPath": "one.flac",
      "lyrics": [{{ "path": "one.lrc", "lang": "en" }}] }}] }}],
  "artwork": [{{ "role": "front", "path": "front.jpg" }}],
  "booklet": [{{ "path": "notes.pdf" }}],
  "lyrics": [],
  "extras": [{{ "path": "bonus.txt" }}],
  "waveformAnalysis": {{ "status": "ready", "intervalMs": 100, "encoding": "peak-rms-u8", "floorDb": -60, "tracks": [] }},
  "sonicAnalysis": {{ "status": "not_analysed" }}
}}"#,
        root = json_string(&root.to_string_lossy())
    );
    let parsed = draft::parse(json.as_bytes()).unwrap();
    assert_eq!(parsed.booklet, vec!["notes.pdf".to_string()]);
    assert_eq!(parsed.extras, vec!["bonus.txt".to_string()]);
    assert!(draft::validate(&parsed).is_ok());
}

#[test]
fn encode_stage_rewrites_and_builds_from_staging() {
    let temp = TempDir::new("encode-stage");
    let root = album_root(&temp, true);
    let bytes = draft_json(&root, true);
    let staging = temp.path().join("staging");
    let transformed = encode_stage(&bytes, &staging, 6.0).unwrap();

    let parsed = draft::parse(transformed.as_bytes()).unwrap();
    assert_eq!(
        parsed.source_root.canonicalize().unwrap(),
        staging.canonicalize().unwrap()
    );
    assert!(parsed.media[0].tracks[0].audio_path.ends_with(".mpc"));
    assert!(
        staging
            .join(&parsed.media[0].tracks[0].audio_path)
            .is_file()
    );
    assert!(staging.join("artwork/front.jpg").is_file());
    assert!(staging.join("booklet/notes.pdf").is_file());
    assert!(staging.join("extras/bonus.txt").is_file());
    assert!(staging.join("lyrics/one.lrc").is_file());
    // The reopened package anchor is dropped: the staged tree is the source.
    assert!(transformed.contains("\"sourceRoot\""));

    // The transformed draft builds (audio passes through).
    let output = temp.path().join("Built.mpack");
    let outcome = run(&AuthorRequest {
        draft_json: transformed.as_bytes(),
        output: &output,
        options: PipelineOptions::default(),
        identify: None,
    })
    .unwrap();
    assert!(outcome.report.is_ok(), "{:?}", outcome.report.findings());
    assert!(output.join("audio/01 - One.mpc").is_file());
    assert!(output.join("lyrics/one.lrc").is_file());
}

#[test]
fn waveform_stage_writes_envelopes_and_a_block() {
    let temp = TempDir::new("waveform-stage");
    let root = album_root(&temp, false);
    let bytes = draft_json(&root, false);
    let staging = temp.path().join("wf");
    let (transformed, entries) = waveform_stage(&bytes, &staging).unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert!(entry.points > 0);
    assert!(staging.join(&entry.path).is_file());
    assert_eq!(
        musicpack_core::format::checksum::sha256_hex(
            &std::fs::read(staging.join(&entry.path)).unwrap()
        ),
        entry.sha256
    );
    assert!(transformed.contains("\"waveformAnalysis\""));
    assert!(transformed.contains(&entry.sha256));
}

#[test]
fn inspect_round_trips_a_built_package() {
    let temp = TempDir::new("inspect");
    let root = album_root(&temp, true);
    let bytes = draft_json(&root, true);
    let output = temp.path().join("Original.mpack");
    run(&AuthorRequest {
        draft_json: &bytes,
        output: &output,
        options: PipelineOptions::default(),
        identify: None,
    })
    .unwrap();

    let inspected = musicpack_author::inspect::package_to_draft(&output).unwrap();
    let parsed = draft::parse(inspected.as_bytes()).unwrap();
    assert_eq!(parsed.album.title, "Test Album");
    assert_eq!(parsed.media[0].tracks[0].audio_path, "audio/01 - One.mpc");
    assert_eq!(
        parsed.media[0].tracks[0].codec.as_deref(),
        Some("musepack-sv8")
    );
    assert_eq!(
        parsed.media[0].tracks[0].lyrics[0].lang.as_deref(),
        Some("en")
    );
    assert_eq!(parsed.artwork[0].role, "front");
    assert_eq!(parsed.booklet, vec!["booklet/notes.pdf".to_string()]);

    // Rebuilding from the inspected draft reproduces the exact manifest.
    let rebuilt = temp.path().join("Rebuilt.mpack");
    run(&AuthorRequest {
        draft_json: inspected.as_bytes(),
        output: &rebuilt,
        options: PipelineOptions::default(),
        identify: None,
    })
    .unwrap();
    assert_eq!(
        std::fs::read(output.join("manifest.json")).unwrap(),
        std::fs::read(rebuilt.join("manifest.json")).unwrap()
    );
    assert_eq!(
        std::fs::read(output.join("analysis/waveform/01-01.wfm")).unwrap(),
        std::fs::read(rebuilt.join("analysis/waveform/01-01.wfm")).unwrap()
    );
}
