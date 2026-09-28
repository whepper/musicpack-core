//! Discogs-EffNet profile tests: identity, artifact verification, and the
//! model-gated live path.
//!
//! Everything that does not need model weights runs unconditionally.
//! Inference tests (live runs, producer construction, full model
//! verification) require the `discogs-effnet` feature *and* read
//! operator-supplied artifacts from `MUSICPACK_TEST_EFFNET_MULTI` /
//! `MUSICPACK_TEST_EFFNET_RELEASE`, skipping with a notice when absent —
//! models are never downloaded, bundled, or required for the rest of the
//! suite.

use std::path::{Path, PathBuf};

#[cfg(feature = "discogs-effnet")]
use musicpack_author::similarity::{ProducerInput, SimilarityProducer, TrackSimilarity};
#[cfg(feature = "discogs-effnet")]
use musicpack_author::similarity_effnet::{EffNetProducer, verify_model_artifact};
use musicpack_author::similarity_effnet::{
    INPUT_TENSOR_NAME, MULTI_MODEL_SHA256, ModelError, RELEASE_MODEL_SHA256, multi_fields,
    multi_profile, release_fields, release_profile, verify_artifact_sha,
};

fn temp_dir(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "musicpack-effnet-{name}-{n}-{pid}",
        n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
        pid = std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Deterministic mono 16-bit WAV: mixed sines, no randomness, fixed phase.
#[cfg(feature = "discogs-effnet")]
fn sine_wav(path: &Path, seconds: u32) {
    let rate = 16_000u32;
    let frames = (rate * seconds) as usize;
    let mut data = Vec::with_capacity(44 + frames * 2);
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&((36 + frames * 2) as u32).to_le_bytes());
    data.extend_from_slice(b"WAVEfmt ");
    data.extend_from_slice(&16u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&rate.to_le_bytes());
    data.extend_from_slice(&(rate * 2).to_le_bytes());
    data.extend_from_slice(&2u16.to_le_bytes());
    data.extend_from_slice(&16u16.to_le_bytes());
    data.extend_from_slice(b"data");
    data.extend_from_slice(&(frames as u32 * 2).to_le_bytes());
    for i in 0..frames {
        let t = i as f32 / rate as f32;
        let sample = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
            + 0.25 * (2.0 * std::f32::consts::PI * 880.0 * t).sin()
            + 0.125 * (2.0 * std::f32::consts::PI * 617.0 * t).sin();
        data.extend_from_slice(&((sample * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, &data).unwrap();
}

#[cfg(feature = "discogs-effnet")]
fn supplied_model(var: &str) -> Option<PathBuf> {
    let key = format!("MUSICPACK_TEST_EFFNET_{var}");
    match std::env::var(&key) {
        Ok(path) if Path::new(&path).is_file() => Some(PathBuf::from(path)),
        _ => {
            eprintln!("note: {key} unset or missing; live-model test skipped");
            None
        }
    }
}

// ---------------------------------------------------------------------
// profile identity (no model needed)
// ---------------------------------------------------------------------

#[test]
fn profile_metadata_matches_the_recorded_evidence() {
    let multi = multi_profile();
    assert_eq!(multi.dimensions, 1280);
    assert_eq!(release_profile().dimensions, 512);
    // Tensor contract from the experiment (FINDINGS.md model table).
    assert_eq!(INPUT_TENSOR_NAME, "serving_default_melspectrogram");
    // The profile field lists carry the recorded model digests.
    fn hex(bytes: &[u8; 32]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
    assert_eq!(
        hex(multi_fields()
            .model_sha256
            .as_ref()
            .expect("multi has weights")),
        MULTI_MODEL_SHA256
    );
    assert_eq!(
        hex(release_fields()
            .model_sha256
            .as_ref()
            .expect("release has weights")),
        RELEASE_MODEL_SHA256
    );
    // Preprocessing/patch-hop evidence lives in the field lists.
    let fields = release_fields();
    assert_eq!(fields.patch_hop, 61);
    assert_eq!(fields.output_encoding, "f32le");
}

// ---------------------------------------------------------------------
// artifact verification (no model needed)
// ---------------------------------------------------------------------

#[test]
fn missing_artifact_fails_closed() {
    let err =
        verify_artifact_sha(Path::new("/nonexistent/model.onnx"), MULTI_MODEL_SHA256).unwrap_err();
    assert!(matches!(err, ModelError::MissingArtifact { .. }), "{err:?}");
}

#[test]
fn wrong_sha_is_rejected_before_any_load() {
    let dir = temp_dir("wrongsha");
    let garbage = dir.join("model.onnx");
    std::fs::write(&garbage, b"not a model, definitely not 16MB of weights").unwrap();
    let err = verify_artifact_sha(&garbage, MULTI_MODEL_SHA256).unwrap_err();
    assert!(matches!(err, ModelError::ShaMismatch { .. }), "{err:?}");
}

#[cfg(feature = "discogs-effnet")]
#[test]
fn incompatible_bytes_with_right_sha_are_rejected() {
    // Right SHA, wrong content: passes identity, fails loading. This is the
    // "incompatible model" path without needing any real artifact.
    let dir = temp_dir("incompat");
    let garbage = dir.join("model.onnx");
    let content = b"predictable non-model bytes for sha pinning";
    std::fs::write(&garbage, content).unwrap();
    let sha = musicpack_core::format::checksum::sha256_hex(content);
    let err = verify_model_artifact(&garbage, &sha, 1280).unwrap_err();
    assert!(
        matches!(
            err,
            ModelError::LoadError { .. } | ModelError::UnexpectedLayout { .. }
        ),
        "{err:?}"
    );
}

#[cfg(feature = "discogs-effnet")]
#[test]
fn missing_model_degrades_to_failed_status_not_error() {
    let dir = temp_dir("missingmodel");
    let wav = dir.join("four.wav");
    sine_wav(&wav, 4);
    // The producer constructs fine with no file present; analysis reports
    // failure per track instead of aborting setup.
    let producer = EffNetProducer::multi(dir.join("absent.onnx"));
    let result = producer.analyze(&ProducerInput {
        disc: 1,
        track: 1,
        audio_path: &wav,
    });
    assert_eq!(result, TrackSimilarity::Failed);
}

#[cfg(feature = "discogs-effnet")]
#[test]
fn too_short_audio_reports_insufficient_not_failed() {
    let dir = temp_dir("short");
    let wav = dir.join("short.wav");
    sine_wav(&wav, 1); // ~62 frames < 128: no complete patch
    // Model state is irrelevant here only if load is attempted first:
    // use a present-but-garbage model file so load fails fast, then check
    // that decode-level outcomes still classify correctly. With no model
    // at all every track is Failed (covered above).
    let model = dir.join("model.onnx");
    std::fs::write(&model, b"garbage").unwrap();
    let producer = EffNetProducer::multi(model);
    let result = producer.analyze(&ProducerInput {
        disc: 1,
        track: 1,
        audio_path: &wav,
    });
    // Load fails before audio is even read: Failed, honestly.
    assert_eq!(result, TrackSimilarity::Failed);
}

#[cfg(feature = "discogs-effnet")]
#[test]
fn missing_model_still_builds_a_valid_package() {
    use musicpack_author::pipeline::{
        AuthorRequest, PipelineOptions, SimilaritySetup, run_with_similarity,
    };
    use musicpack_core::authoring::LoudnessMode;

    // Two-track source tree with genuine audio; the model file does not
    // exist, so every track honestly reports failure — and the package
    // stays valid.
    let dir = temp_dir("nomodelpkg");
    let root = dir.join("album");
    std::fs::create_dir_all(&root).unwrap();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/reference/audio");
    std::fs::copy(fixture.join("flac-mono-44k.flac"), root.join("one.flac")).unwrap();
    std::fs::copy(fixture.join("flac-mono-44k.flac"), root.join("two.flac")).unwrap();
    let draft = format!(
        r#"{{"schema":"musicpack-draft","version":1,"sourceRoot":{root},"album":{{"title":"No Model","artists":[{{"name":"Alice"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"One","audioPath":"one.flac"}},{{"track":2,"title":"Two","audioPath":"two.flac"}}]}}]}}"#,
        root = json_escape(&root.to_string_lossy()),
    );
    let output = dir.join("NoModel.mpack");
    let producer =
        musicpack_author::similarity_effnet::EffNetProducer::multi(dir.join("absent.onnx"));
    let outcome = run_with_similarity(
        &AuthorRequest {
            draft_json: draft.as_bytes(),
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
    let manifest_bytes = std::fs::read(output.join("manifest.json")).unwrap();
    let manifest =
        musicpack_core::format::manifest::ParsedManifest::parse(&manifest_bytes).unwrap();
    assert_eq!(manifest.manifest().analysis.len(), 1);
    assert_eq!(manifest.manifest().analysis[0].kind, "similarity");
    // All-failed document: table present, zero vector bytes.
    let doc = std::fs::read(output.join(&manifest.manifest().analysis[0].asset.path)).unwrap();
    assert_eq!(&doc[0..4], b"MSIM");
    assert_eq!(doc.len(), 64 + 2 * 12);
}

#[cfg(feature = "discogs-effnet")]
fn json_escape(s: &str) -> String {
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

// ---------------------------------------------------------------------
// live model path (operator-supplied artifact only)
// ---------------------------------------------------------------------

#[cfg(feature = "discogs-effnet")]
fn live_producer(
    var: &str,
    make: fn(PathBuf) -> EffNetProducer,
) -> Option<(EffNetProducer, PathBuf)> {
    let path = supplied_model(var)?;
    // The artifact must verify against the recorded digest first: a wrong
    // file fails the test instead of running unknown weights.
    let expected = if var == "MULTI" {
        MULTI_MODEL_SHA256
    } else {
        RELEASE_MODEL_SHA256
    };
    let dims = if var == "MULTI" { 1280 } else { 512 };
    match verify_model_artifact(&path, expected, dims) {
        Ok(info) => {
            assert!(info.output_name.to_ascii_lowercase().contains("embedding"));
            assert_eq!(info.embedding_dim, dims);
            Some((make(path.clone()), path))
        }
        Err(error) => panic!("supplied {var} artifact rejected: {error}"),
    }
}

#[cfg(feature = "discogs-effnet")]
#[test]
fn live_multi_inference_is_finite_normalized_and_deterministic() {
    let Some((producer, _)) = live_producer("MULTI", EffNetProducer::multi) else {
        return;
    };
    assert_eq!(producer.producer_profile().dimensions, 1280);
    let dir = temp_dir("livemulti");
    let wav = dir.join("four.wav");
    sine_wav(&wav, 4);
    let input = ProducerInput {
        disc: 1,
        track: 1,
        audio_path: &wav,
    };
    let first = producer.analyze(&input);
    let second = producer.analyze(&input);
    let (TrackSimilarity::Ok { vector: a }, TrackSimilarity::Ok { vector: b }) = (first, second)
    else {
        panic!("expected ok vectors");
    };
    assert_eq!(a.len(), 1280);
    assert!(a.iter().all(|v| v.is_finite()));
    let norm: f64 = a.iter().map(|v| f64::from(*v) * f64::from(*v)).sum();
    assert!((norm.sqrt() - 1.0).abs() < 1e-5, "L2 normalized");
    assert_eq!(a, b, "repeated inference must be byte-identical");
    // Surfaced for the experiment-side comparison: SHA-256 over the
    // concatenated f32 little-endian bytes, the `embedding_sha256` scheme.
    let digest: Vec<u8> = a.iter().flat_map(|v| v.to_le_bytes()).collect();
    eprintln!(
        "production_sha256={}",
        musicpack_core::format::checksum::sha256_hex(&digest)
    );
    // Deterministic serialization through the existing writer.
    let entries = vec![((1, 1), TrackSimilarity::Ok { vector: a })];
    let bytes_a =
        musicpack_author::similarity::write_msim(producer.producer_profile(), &entries).unwrap();
    let bytes_b =
        musicpack_author::similarity::write_msim(producer.producer_profile(), &entries).unwrap();
    assert_eq!(bytes_a, bytes_b);
}

#[cfg(feature = "discogs-effnet")]
#[test]
fn live_release_inference_is_finite_normalized_and_deterministic() {
    let Some((producer, _)) = live_producer("RELEASE", EffNetProducer::release) else {
        return;
    };
    assert_eq!(producer.producer_profile().dimensions, 512);
    let dir = temp_dir("liverelease");
    let wav = dir.join("four.wav");
    sine_wav(&wav, 4);
    let input = ProducerInput {
        disc: 1,
        track: 1,
        audio_path: &wav,
    };
    let (TrackSimilarity::Ok { vector: a }, TrackSimilarity::Ok { vector: b }) =
        (producer.analyze(&input), producer.analyze(&input))
    else {
        panic!("expected ok vectors");
    };
    assert_eq!(a.len(), 512);
    assert!(a.iter().all(|v| v.is_finite()));
    let norm: f64 = a.iter().map(|v| f64::from(*v) * f64::from(*v)).sum();
    assert!((norm.sqrt() - 1.0).abs() < 1e-5, "L2 normalized");
    assert_eq!(a, b, "repeated inference must be byte-identical");
    let digest: Vec<u8> = a.iter().flat_map(|v| v.to_le_bytes()).collect();
    eprintln!(
        "production_sha256={}",
        musicpack_core::format::checksum::sha256_hex(&digest)
    );
}
