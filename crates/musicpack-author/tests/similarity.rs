//! Similarity producer-boundary tests: the model-neutral Author side.
//!
//! A deterministic synthetic producer (obviously fake: fixed vectors from
//! track indices, a `test-synthetic-*` profile that names no model family)
//! drives the pipeline. No ML model, runtime, download, or audio analysis
//! beyond the pipeline's own staged-audio handling appears here.

use std::cell::Cell;
use std::path::{Path, PathBuf};

use musicpack_author::pipeline::{
    AuthorRequest, PipelineOptions, SimilaritySetup, run_with_similarity,
};
use musicpack_author::similarity::{
    CacheKey, CachedVector, MemSimilarityCache, ProducerInput, SimilarityCache, SimilarityProducer,
    SimilarityProfile, TrackSimilarity,
};
use musicpack_core::authoring::LoudnessMode;
use musicpack_core::format::manifest::ParsedManifest;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "musicpack-author-sim-{}-{name}-{n}",
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

/// Obviously synthetic producer: deterministic vectors from track indices,
/// scripted non-ok outcomes, no audio read, no model. Its profile names no
/// model family and must never be mistaken for a production profile.
struct SyntheticProducer {
    profile: SimilarityProfile,
    insufficient: Vec<(i32, i32)>,
    failed: Vec<(i32, i32)>,
    bad_vector: Vec<(i32, i32)>,
    calls: Cell<usize>,
}

impl SyntheticProducer {
    fn clean() -> Self {
        Self {
            profile: SimilarityProfile::new(
                "test-synthetic-similarity-v1".to_string(),
                [0x5a; 32],
                4,
            )
            .unwrap(),
            insufficient: Vec::new(),
            failed: Vec::new(),
            bad_vector: Vec::new(),
            calls: Cell::new(0),
        }
    }
}

impl SimilarityProducer for SyntheticProducer {
    fn profile(&self) -> &SimilarityProfile {
        &self.profile
    }

    fn analyze(&self, input: &ProducerInput<'_>) -> TrackSimilarity {
        self.calls.set(self.calls.get() + 1);
        let key = (input.disc, input.track);
        if self.insufficient.contains(&key) {
            return TrackSimilarity::InsufficientAudio;
        }
        if self.failed.contains(&key) {
            return TrackSimilarity::Failed;
        }
        if self.bad_vector.contains(&key) {
            // Contract violation on purpose: wrong length.
            return TrackSimilarity::Ok {
                vector: vec![1.0, 0.0],
            };
        }
        // Deterministic unit-ish vector from track identity.
        let (d, t) = (input.disc as f32, input.track as f32);
        let raw = [d, t, d + t, 1.0];
        let norm = raw.iter().map(|v| v * v).sum::<f32>().sqrt();
        TrackSimilarity::Ok {
            vector: raw.iter().map(|v| v / norm).collect(),
        }
    }
}

/// Two-track source tree + draft JSON (author draft shape).
fn two_track_album(temp: &TempDir) -> (PathBuf, Vec<u8>) {
    let root = temp.path().join("album");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::copy(fixture("flac-mono-44k.flac"), root.join("one.flac")).unwrap();
    std::fs::copy(fixture("flac16-44k.flac"), root.join("two.flac")).unwrap();
    let json = format!(
        r#"{{"schema":"musicpack-draft","version":1,"sourceRoot":{root},"album":{{"title":"Sim Album","artists":[{{"name":"Alice"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"One","audioPath":"one.flac"}},{{"track":2,"title":"Two","audioPath":"two.flac"}}]}}]}}"#,
        root = json_string(&root.to_string_lossy()),
    );
    (root, json.into_bytes())
}

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn manifest_of(dir: &Path) -> ParsedManifest {
    let bytes = std::fs::read(dir.join("manifest.json")).unwrap();
    ParsedManifest::parse(&bytes).unwrap()
}

/// Minimal structural read of an `.msim` file: magic, dims, encoding,
/// track count, and the (disc, track, status) table.
fn read_doc(bytes: &[u8]) -> (u16, u8, Vec<(u32, u32, u8)>) {
    assert!(bytes.len() >= 64);
    assert_eq!(&bytes[0..4], b"MSIM");
    let dims = u16::from_be_bytes([bytes[40], bytes[41]]);
    let encoding = bytes[42];
    let count = u32::from_be_bytes([bytes[44], bytes[45], bytes[46], bytes[47]]) as usize;
    let mut members = Vec::new();
    for i in 0..count {
        let base = 64 + i * 12;
        members.push((
            u32::from_be_bytes([
                bytes[base],
                bytes[base + 1],
                bytes[base + 2],
                bytes[base + 3],
            ]),
            u32::from_be_bytes([
                bytes[base + 4],
                bytes[base + 5],
                bytes[base + 6],
                bytes[base + 7],
            ]),
            bytes[base + 8],
        ));
    }
    (dims, encoding, members)
}

// ---------------------------------------------------------------------
// producer abstraction + package integration
// ---------------------------------------------------------------------

#[test]
fn disabled_similarity_leaves_the_package_unchanged() {
    let temp = TempDir::new("disabled");
    let (_root, draft) = two_track_album(&temp);
    let output = temp.path().join("Album.mpack");
    let producer = SyntheticProducer::clean();
    let outcome = run_with_similarity(
        &AuthorRequest {
            draft_json: &draft,
            output: &output,
            options: PipelineOptions {
                waveform: false,
                loudness: LoudnessMode::Omit,
                ..PipelineOptions::default()
            },
            identify: None,
        },
        SimilaritySetup {
            enabled: false,
            producer: Some(&producer),
            cache: None,
        },
        &mut |_| {},
    )
    .unwrap();
    assert!(outcome.report.is_ok());
    let manifest = manifest_of(&output);
    assert!(
        manifest.manifest().analysis.is_empty(),
        "disabled similarity must not add analysis entries"
    );
    assert_eq!(producer.calls.get(), 0, "disabled producer must not run");
}

#[test]
fn enabled_without_producer_is_a_configuration_error() {
    let temp = TempDir::new("noproducer");
    let (_root, draft) = two_track_album(&temp);
    let output = temp.path().join("Album.mpack");
    let err = run_with_similarity(
        &AuthorRequest {
            draft_json: &draft,
            output: &output,
            options: PipelineOptions {
                waveform: false,
                loudness: LoudnessMode::Omit,
                ..PipelineOptions::default()
            },
            identify: None,
        },
        SimilaritySetup {
            enabled: true,
            producer: None,
            cache: None,
        },
        &mut |_| {},
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("no producer configured"),
        "unexpected error: {err}"
    );
    assert!(
        !output.exists(),
        "a configuration error must not publish a package"
    );
}

#[test]
fn valid_producer_emits_one_valid_document() {
    let temp = TempDir::new("valid");
    let (_root, draft) = two_track_album(&temp);
    let output = temp.path().join("Album.mpack");
    let producer = SyntheticProducer::clean();
    let outcome = run_with_similarity(
        &AuthorRequest {
            draft_json: &draft,
            output: &output,
            options: PipelineOptions {
                waveform: false,
                loudness: LoudnessMode::Omit,
                ..PipelineOptions::default()
            },
            identify: None,
        },
        SimilaritySetup {
            enabled: true,
            producer: Some(&producer),
            cache: None,
        },
        &mut |_| {},
    )
    .unwrap();
    assert!(outcome.report.is_ok(), "{:?}", outcome.report.findings());
    assert_eq!(producer.calls.get(), 2);

    let manifest = manifest_of(&output);
    let entries = &manifest.manifest().analysis;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].kind, "similarity");
    assert_eq!(
        entries[0].profile.as_deref(),
        Some("test-synthetic-similarity-v1")
    );
    let bytes = std::fs::read(output.join(&entries[0].asset.path)).unwrap();
    assert_eq!(
        entries[0].asset.sha256,
        musicpack_core::format::checksum::sha256_hex(&bytes)
    );
    let (dims, encoding, members) = read_doc(&bytes);
    assert_eq!(dims, 4);
    assert_eq!(encoding, 1); // f32le, always in Slice 0
    assert_eq!(members, vec![(1, 1, 0), (1, 2, 0)]);
    // Declared layout is exact: header + table + two 4-D f32 vectors.
    assert_eq!(bytes.len(), 64 + 24 + 2 * 16);
}

#[test]
fn result_states_become_exact_statuses() {
    let temp = TempDir::new("states");
    let (_root, draft) = two_track_album(&temp);
    let output = temp.path().join("Album.mpack");
    let mut producer = SyntheticProducer::clean();
    producer.insufficient = vec![(1, 2)];
    let outcome = run_with_similarity(
        &AuthorRequest {
            draft_json: &draft,
            output: &output,
            options: PipelineOptions {
                waveform: false,
                loudness: LoudnessMode::Omit,
                ..PipelineOptions::default()
            },
            identify: None,
        },
        SimilaritySetup {
            enabled: true,
            producer: Some(&producer),
            cache: None,
        },
        &mut |_| {},
    )
    .unwrap();
    assert!(outcome.report.is_ok());
    let manifest = manifest_of(&output);
    let bytes = std::fs::read(output.join(&manifest.manifest().analysis[0].asset.path)).unwrap();
    let (_, _, members) = read_doc(&bytes);
    assert_eq!(members, vec![(1, 1, 0), (1, 2, 1)]);
    // The non-ok member occupies zero bytes.
    assert_eq!(bytes.len(), 64 + 24 + 16);
}

#[test]
fn malformed_producer_output_becomes_failed_not_package_data() {
    let temp = TempDir::new("badoutput");
    let (_root, draft) = two_track_album(&temp);
    let output = temp.path().join("Album.mpack");
    let mut producer = SyntheticProducer::clean();
    producer.bad_vector = vec![(1, 1), (1, 2)]; // wrong length for a 4-D profile
    let outcome = run_with_similarity(
        &AuthorRequest {
            draft_json: &draft,
            output: &output,
            options: PipelineOptions {
                waveform: false,
                loudness: LoudnessMode::Omit,
                ..PipelineOptions::default()
            },
            identify: None,
        },
        SimilaritySetup {
            enabled: true,
            producer: Some(&producer),
            cache: None,
        },
        &mut |_| {},
    )
    .unwrap();
    // The package is valid; both tracks honestly report failure.
    assert!(outcome.report.is_ok());
    let manifest = manifest_of(&output);
    let bytes = std::fs::read(output.join(&manifest.manifest().analysis[0].asset.path)).unwrap();
    let (_, _, members) = read_doc(&bytes);
    assert_eq!(members, vec![(1, 1, 3), (1, 2, 3)]);
    assert_eq!(bytes.len(), 64 + 24, "failed members carry zero bytes");
}

#[test]
fn analysis_is_deterministic_across_runs() {
    let build = |temp: &TempDir, name: &str| -> Vec<u8> {
        let (_root, draft) = two_track_album(temp);
        let output = temp.path().join(name);
        let producer = SyntheticProducer::clean();
        let outcome = run_with_similarity(
            &AuthorRequest {
                draft_json: &draft,
                output: &output,
                options: PipelineOptions {
                    waveform: false,
                    loudness: LoudnessMode::Omit,
                    ..PipelineOptions::default()
                },
                identify: None,
            },
            SimilaritySetup {
                enabled: true,
                producer: Some(&producer),
                cache: None,
            },
            &mut |_| {},
        )
        .unwrap();
        assert!(outcome.report.is_ok());
        let manifest = manifest_of(&output);
        std::fs::read(output.join(&manifest.manifest().analysis[0].asset.path)).unwrap()
    };
    let temp = TempDir::new("determinism");
    let first = build(&temp, "a.mpack");
    let second = build(&temp, "b.mpack");
    assert_eq!(first, second, "identical inputs must yield identical bytes");
}

// ---------------------------------------------------------------------
// cache behaviour
// ---------------------------------------------------------------------

#[test]
fn cache_hit_avoids_reanalysis() {
    let temp = TempDir::new("cachehit");
    let (_root, draft) = two_track_album(&temp);
    let mut cache = MemSimilarityCache::new();
    let run = |temp: &TempDir,
               name: &str,
               cache: &mut MemSimilarityCache,
               producer: &SyntheticProducer| {
        let output = temp.path().join(name);
        run_with_similarity(
            &AuthorRequest {
                draft_json: &draft,
                output: &output,
                options: PipelineOptions {
                    waveform: false,
                    loudness: LoudnessMode::Omit,
                    ..PipelineOptions::default()
                },
                identify: None,
            },
            SimilaritySetup {
                enabled: true,
                producer: Some(producer),
                cache: Some(cache),
            },
            &mut |_| {},
        )
        .unwrap()
    };
    let first = SyntheticProducer::clean();
    run(&temp, "a.mpack", &mut cache, &first);
    assert_eq!(first.calls.get(), 2);
    assert_eq!(cache.len(), 2);
    // Second run over identical bytes: every track hits, producer idles.
    let second = SyntheticProducer::clean();
    run(&temp, "b.mpack", &mut cache, &second);
    assert_eq!(second.calls.get(), 0, "cache must serve both tracks");
}

#[test]
fn cache_misses_on_profile_audio_or_invalidation() {
    let temp = TempDir::new("cachemiss");
    let (_root, draft) = two_track_album(&temp);
    let mut cache = MemSimilarityCache::new();
    let run = |temp: &TempDir,
               name: &str,
               cache: &mut MemSimilarityCache,
               producer: &SyntheticProducer| {
        let output = temp.path().join(name);
        run_with_similarity(
            &AuthorRequest {
                draft_json: &draft,
                output: &output,
                options: PipelineOptions {
                    waveform: false,
                    loudness: LoudnessMode::Omit,
                    ..PipelineOptions::default()
                },
                identify: None,
            },
            SimilaritySetup {
                enabled: true,
                producer: Some(producer),
                cache: Some(cache),
            },
            &mut |_| {},
        )
        .unwrap()
    };
    let base = SyntheticProducer::clean();
    run(&temp, "a.mpack", &mut cache, &base);
    assert_eq!(base.calls.get(), 2);

    // Same audio, different fingerprint: full miss (never keyed by id).
    let mut other_fp = SyntheticProducer::clean();
    other_fp.profile = musicpack_author::similarity::SimilarityProfile::new(
        "test-synthetic-similarity-v2".to_string(),
        [0x6b; 32],
        4,
    )
    .unwrap();
    run(&temp, "b.mpack", &mut cache, &other_fp);
    assert_eq!(other_fp.calls.get(), 2, "new fingerprint must miss");

    // Changed audio (bytes never analyzed before): track 1 misses
    // (1 call); track 2's bytes are unchanged, so it still hits.
    std::fs::copy(
        fixture("flac24-48k.flac"),
        temp.path().join("album/one.flac"),
    )
    .unwrap();
    let changed = SyntheticProducer::clean();
    run(&temp, "c.mpack", &mut cache, &changed);
    assert_eq!(changed.calls.get(), 1, "changed bytes must miss");

    // Invalidation drops the source's rows; clear drops everything.
    cache.clear();
    assert!(cache.is_empty());
    let cleared = SyntheticProducer::clean();
    run(&temp, "d.mpack", &mut cache, &cleared);
    assert_eq!(cleared.calls.get(), 2, "cleared cache must miss");
}

#[test]
fn cache_disabled_means_every_track_is_analyzed() {
    let temp = TempDir::new("nocache");
    let (_root, draft) = two_track_album(&temp);
    let output = temp.path().join("Album.mpack");
    let producer = SyntheticProducer::clean();
    run_with_similarity(
        &AuthorRequest {
            draft_json: &draft,
            output: &output,
            options: PipelineOptions {
                waveform: false,
                loudness: LoudnessMode::Omit,
                ..PipelineOptions::default()
            },
            identify: None,
        },
        SimilaritySetup {
            enabled: true,
            producer: Some(&producer),
            cache: None,
        },
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(producer.calls.get(), 2);
}

#[test]
fn invalid_cached_values_fail_closed() {
    // The public store API refuses garbage; a lookup can therefore never
    // return it. Tampering paths that bypass `store` do not exist: rows
    // are private and every read revalidates shape through the writer.
    let mut cache = MemSimilarityCache::new();
    let key = CacheKey {
        source_sha256: "aa".to_string(),
        fingerprint: [0x11; 32],
    };
    cache.store(
        &key,
        CachedVector {
            dimensions: 2,
            vector: vec![1.0],
        },
    );
    cache.store(
        &key,
        CachedVector {
            dimensions: 2,
            vector: vec![f32::NAN, 0.0],
        },
    );
    cache.store(
        &key,
        CachedVector {
            dimensions: 2,
            vector: vec![0.0, 0.0],
        },
    );
    assert_eq!(cache.lookup(&key), None);
    assert!(cache.is_empty(), "refused stores leave no rows");
}

#[test]
fn invalidate_source_drops_only_that_source() {
    let mut cache = MemSimilarityCache::new();
    let a = CacheKey {
        source_sha256: "aa".to_string(),
        fingerprint: [0x11; 32],
    };
    let b = CacheKey {
        source_sha256: "bb".to_string(),
        fingerprint: [0x11; 32],
    };
    for key in [&a, &b] {
        cache.store(
            key,
            CachedVector {
                dimensions: 1,
                vector: vec![1.0],
            },
        );
    }
    assert_eq!(cache.len(), 2);
    cache.invalidate_source("aa");
    assert_eq!(cache.lookup(&a), None);
    assert!(cache.lookup(&b).is_some());
}

// ---------------------------------------------------------------------
// split stage
// ---------------------------------------------------------------------

#[test]
fn split_stage_previews_and_cancels() {
    use musicpack_author::pipeline::similarity_stage_with;
    let temp = TempDir::new("split");
    let (_root, draft) = two_track_album(&temp);
    let staging = temp.path().join("staging");
    let producer = SyntheticProducer::clean();
    let (json, entries) = similarity_stage_with(
        &draft,
        &staging,
        SimilaritySetup {
            enabled: true,
            producer: Some(&producer),
            cache: None,
        },
        &mut |_, _, _, _, _| true,
    )
    .unwrap();
    assert_eq!(entries.len(), 2);
    assert!(staging.join("similarity.msim").is_file());
    assert!(json.contains("similarityAnalysis"), "{json}");
    assert!(json.contains("test-synthetic-similarity-v1"), "{json}");

    // Cancellation between tracks.
    let staging2 = temp.path().join("staging2");
    let err = similarity_stage_with(
        &draft,
        &staging2,
        SimilaritySetup {
            enabled: true,
            producer: Some(&producer),
            cache: None,
        },
        &mut |_, _, _, _, _| false,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("cancelled"),
        "unexpected error: {err}"
    );

    // Disabled: JSON unchanged, no entries, no file.
    let staging3 = temp.path().join("staging3");
    let (plain, empty) = similarity_stage_with(
        &draft,
        &staging3,
        SimilaritySetup {
            enabled: false,
            producer: Some(&producer),
            cache: None,
        },
        &mut |_, _, _, _, _| true,
    )
    .unwrap();
    assert!(empty.is_empty());
    assert!(!plain.contains("similarityAnalysis"));
    assert!(!staging3.join("similarity.msim").exists());
}
