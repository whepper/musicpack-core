//! Package validation: the `verify` semantics.
//!
//! Port of `musicpack_package_verify` (`core/libmusicpack/src/package.c`).
//! Verification is expressed as a [`Report`] of ordered [`Finding`]s — each
//! an error (the package is untrustworthy) or a warning (noted, non-fatal).
//! The traversal order is the reference's, so reports are deterministic.
//!
//! # What verify checks
//!
//! For every referenced asset, in the reference's traversal order (each
//! disc's tracks: primary audio, waveform, representations, per-track
//! lyrics; then artwork, booklet, lyrics, extras, analysis):
//!
//! > Per-track lyric references are the one Rust-defined group
//! > (`docs/musicpack-lyrics-v1.md` §6.3, an additive field the
//! > reference has no group for); they slot after the track's
//! > representations so per-track assets stay grouped. Every other
//! > group keeps the reference's exact order.
//!
//! 1. containment/type via the storage backend — an escaped path is
//!    `unsafe path`, anything else unopenable is `missing file`;
//! 2. the per-file size budget (8 GiB) — `exceeds …-byte file limit`;
//! 3. the aggregate byte budget (64 GiB), with same-pass object
//!    deduplication — `aggregate referenced bytes exceed …-byte limit`;
//! 4. SHA-256 against the declaration — `checksum mismatch`.
//!
//! Waveform references additionally require `payload size == points × 2`,
//! respect the per-track payload limit and are structurally validated; a
//! `points`/duration divergence beyond two buckets is a warning.
//!
//! Finally, regular files present in the storage that no manifest entry
//! references produce `unreferenced file` warnings (storage meta files such
//! as `manifest.json` are excluded).
//!
//! # Not yet covered
//!
//! Sonic analysis documents (`analysis[]` entries of type `sonic`) have
//! spec-level validation (`musicpack-sonic-v1.md`) that is deferred to a
//! later phase; the reference parses and validates them here. This is
//! recorded in `docs/architecture.md` and is not exercised by the
//! conformance corpus.
//!
//! # Errors vs warnings
//!
//! Errors (values mirror the reference vocabulary):
//!
//! - `manifest: mutable manifest fails validation`
//! - `<kind>: unsafe path '<path>'`
//! - `<kind>: missing file '<path>'`
//! - `<kind>: '<path>' exceeds <n>-byte file limit`
//! - `<kind>: aggregate referenced bytes exceed <n>-byte limit`
//! - `<kind>: cannot hash '<path>'`
//! - `<kind>: checksum mismatch '<path>'`
//! - `waveform: '<path>' payload size inconsistent with points (<a> vs <b>)`
//! - `waveform: '<path>' exceeds per-track payload limit`
//! - `waveform: cannot read '<path>'`
//! - `waveform: '<path>' payload validation failed`
//!
//! Warnings:
//!
//! - `unreferenced file '<path>'`
//! - `waveform: '<path>' points=<n> differs from duration=<g> (<d> buckets)`
//!
//! `kind` is one of `track`, `representation`, `artwork`, `booklet`,
//! `lyrics`, `extras`, `analysis` (and `waveform` for waveform assets).

use std::collections::HashSet;
use std::io::Read;

use crate::format::checksum;
use crate::format::manifest::{Manifest, Track};
use crate::format::number::format_g_precision;
use crate::format::waveform::MAX_PAYLOAD_BYTES;
use crate::limits::{MAX_FILE_BYTES, MAX_TOTAL_BYTES};
use crate::storage::{BackendError, ObjectId, PackageBackend};

/// Severity of a verification finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// The package is malformed or untrustworthy; verification fails.
    Error,
    /// Noted but non-fatal (e.g. unreferenced files).
    Warning,
}

/// One verification finding, with the reference implementation's message
/// wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Error or warning.
    pub severity: Severity,
    /// The reference-compatible message.
    pub message: String,
}

/// The result of a verification pass.
///
/// Findings are in the reference's deterministic traversal order.
/// [`Report::is_ok`] is `true` when there are no errors (warnings are
/// allowed), matching `musicpack_package_verify` returning `MUSICPACK_OK`.
///
/// Consumers planned for later phases: the server's verified-only
/// visibility rule, MPAK verification, and CLI output (which prints
/// `error: <message>` / `warning: <message>` and exits non-zero when
/// `!is_ok()`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    findings: Vec<Finding>,
}

impl Report {
    /// Number of errors.
    pub fn errors(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .count()
    }

    /// Number of warnings.
    pub fn warnings(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Warning)
            .count()
    }

    /// All findings in traversal order.
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// `true` when there are no errors.
    pub fn is_ok(&self) -> bool {
        self.errors() == 0
    }

    /// Records an error.
    pub fn push_error(&mut self, message: impl Into<String>) {
        self.findings.push(Finding {
            severity: Severity::Error,
            message: message.into(),
        });
    }

    /// Records a warning.
    pub fn push_warning(&mut self, message: impl Into<String>) {
        self.findings.push(Finding {
            severity: Severity::Warning,
            message: message.into(),
        });
    }
}

/// Same-pass verification budgets (aggregate bytes + object dedup).
#[derive(Default)]
struct Budget {
    total_bytes: u64,
    seen: HashSet<ObjectId>,
}

impl Budget {
    /// Reference `inode_seen`: records `id`; `true` when already known
    /// (the object's content was already hashed this pass).
    fn is_seen(&mut self, id: ObjectId) -> bool {
        !self.seen.insert(id)
    }
}

/// Verifies a manifest against a storage backend.
///
/// Returns a [`Report`]; `report.is_ok()` corresponds to the reference's
/// `MUSICPACK_OK` return.
pub fn verify(manifest: &Manifest, backend: &dyn PackageBackend) -> Report {
    let mut report = Report::default();

    // Port of the reference's `manifest_shape_safe` + write-validation
    // gate: a mutable manifest that no longer serializes cannot be
    // verified.
    if manifest.validate().is_err() {
        report.push_error("manifest: mutable manifest fails validation");
        return report;
    }

    let mut budget = Budget::default();
    for disc in &manifest.media {
        for track in &disc.tracks {
            verify_one(
                backend,
                &track.audio.path,
                &track.audio.sha256,
                "track",
                &mut report,
                Some(&mut budget),
            );
            verify_waveform(backend, track, &mut report);
            for representation in &track.representations {
                verify_one(
                    backend,
                    &representation.path,
                    &representation.sha256,
                    "representation",
                    &mut report,
                    Some(&mut budget),
                );
            }
            // Per-track lyrics references (docs/musicpack-lyrics-v1.md
            // §6.3): verified like any budgeted asset. Traversal order is
            // Rust-defined (the reference has no such group); per-track
            // assets stay grouped. Only manifests carrying the additive
            // field have entries here, so reference-authored reports are
            // byte-identical to before.
            for lyrics in &track.lyrics {
                verify_one(
                    backend,
                    &lyrics.path,
                    &lyrics.sha256,
                    "lyrics",
                    &mut report,
                    Some(&mut budget),
                );
            }
        }
    }
    for artwork in &manifest.artwork {
        verify_one(
            backend,
            &artwork.asset.path,
            &artwork.asset.sha256,
            "artwork",
            &mut report,
            Some(&mut budget),
        );
    }
    verify_assets(
        backend,
        &manifest.booklet,
        "booklet",
        &mut report,
        &mut budget,
    );
    verify_assets(
        backend,
        &manifest.lyrics,
        "lyrics",
        &mut report,
        &mut budget,
    );
    verify_assets(
        backend,
        &manifest.extras,
        "extras",
        &mut report,
        &mut budget,
    );
    for analysis in &manifest.analysis {
        verify_one(
            backend,
            &analysis.asset.path,
            &analysis.asset.sha256,
            "analysis",
            &mut report,
            Some(&mut budget),
        );
    }

    // Sonic document validation is deferred to a later phase (module docs).

    verify_extra_files(manifest, backend, &mut report);

    // Backend-specific container consistency (reference: the MPAK
    // `musicpack_mpak_verify_extra` call site). The directory backend has
    // no container layer and keeps the default no-op.
    backend.verify_extra(manifest, &mut report);

    report
}

impl crate::storage::VerificationSink for Report {
    fn error(&mut self, message: &str) {
        self.push_error(message);
    }

    fn warning(&mut self, message: &str) {
        self.push_warning(message);
    }
}

fn verify_assets(
    backend: &dyn PackageBackend,
    assets: &[crate::format::manifest::Asset],
    kind: &str,
    report: &mut Report,
    budget: &mut Budget,
) {
    for asset in assets {
        verify_one(
            backend,
            &asset.path,
            &asset.sha256,
            kind,
            report,
            Some(budget),
        );
    }
}

/// Port of `verify_assets` for one referenced asset.
fn verify_one(
    backend: &dyn PackageBackend,
    path: &str,
    declared_sha256: &str,
    kind: &str,
    report: &mut Report,
    budget: Option<&mut Budget>,
) {
    let len = match backend.open_asset(path) {
        Err(BackendError::UnsafePath) => {
            report.push_error(format!("{kind}: unsafe path '{path}'"));
            return;
        }
        Err(_) => {
            report.push_error(format!("{kind}: missing file '{path}'"));
            return;
        }
        Ok(opened) => opened.len,
    };

    if len > MAX_FILE_BYTES {
        report.push_error(format!(
            "{kind}: '{path}' exceeds {MAX_FILE_BYTES}-byte file limit"
        ));
        return;
    }

    if let Some(budget) = budget {
        if let Some(id) = backend.object_id(path) {
            if budget.is_seen(id) {
                // Same underlying object already hashed this pass.
                return;
            }
        }
        budget.total_bytes = budget.total_bytes.saturating_add(len);
        if budget.total_bytes > MAX_TOTAL_BYTES {
            report.push_error(format!(
                "{kind}: aggregate referenced bytes exceed {MAX_TOTAL_BYTES}-byte limit"
            ));
            return;
        }
    }

    // Re-open for hashing: the reference queries the size and hashes in
    // separate opens, so an object replaced between them is observed.
    let digest = match backend.open_asset(path) {
        Err(_) => {
            report.push_error(format!("{kind}: cannot hash '{path}'"));
            return;
        }
        Ok(mut opened) => match checksum::sha256_reader(&mut opened.reader) {
            Err(_) => {
                report.push_error(format!("{kind}: cannot hash '{path}'"));
                return;
            }
            Ok(digest) => digest,
        },
    };
    if !checksum::hex_eq(&checksum::to_hex(&digest), declared_sha256) {
        report.push_error(format!("{kind}: checksum mismatch '{path}'"));
    }
}

/// Port of `verify_waveform_track`.
fn verify_waveform(backend: &dyn PackageBackend, track: &Track, report: &mut Report) {
    let Some(waveform) = &track.waveform else {
        return;
    };
    let path = waveform.path.as_str();

    // Generic asset rules (no budget: the reference passes budget = 0 for
    // waveform assets).
    verify_one(backend, path, &waveform.sha256, "waveform", report, None);

    let Ok(len) = size_of(backend, path) else {
        return; // already reported
    };
    let expected = waveform.points.saturating_mul(2);
    if len != expected {
        report.push_error(format!(
            "waveform: '{path}' payload size inconsistent with points ({len} vs {expected})"
        ));
        return;
    }
    if len > MAX_PAYLOAD_BYTES {
        report.push_error(format!(
            "waveform: '{path}' exceeds per-track payload limit"
        ));
        return;
    }

    // Read + structural validation. The v1 payload is `peak-rms-u8`: every
    // byte value is valid, so the substantive checks are the size (above)
    // and the digest (already verified); the read mirrors the reference.
    match backend.open_asset(path) {
        Err(_) => {
            report.push_error(format!("waveform: cannot read '{path}'"));
            return;
        }
        Ok(mut opened) => {
            let mut payload = Vec::with_capacity(len as usize);
            if opened.reader.read_to_end(&mut payload).is_err() || payload.len() as u64 != expected
            {
                report.push_error(format!("waveform: cannot read '{path}'"));
                return;
            }
        }
    }

    // Duration cross-check is a warning, not an error.
    if let Some(duration) = track.duration {
        let expected_points = (duration * 10.0 + 0.5) as i64;
        let actual_points = waveform.points as i64;
        let diff = (expected_points - actual_points).abs();
        if diff > 2 {
            report.push_warning(format!(
                "waveform: '{path}' points={} differs from duration={} ({} buckets)",
                waveform.points,
                format_g_precision(duration, 6),
                diff
            ));
        }
    }
}

fn size_of(backend: &dyn PackageBackend, path: &str) -> Result<u64, BackendError> {
    backend.open_asset(path).map(|opened| opened.len)
}

/// Port of `verify_extra_files`: regular files no manifest entry references
/// produce warnings. Storage meta files (e.g. `manifest.json`) are
/// excluded.
///
/// The reference walks the directory in `readdir` order; that order is
/// filesystem-dependent and not part of the contract, so the file list is
/// sorted for determinism.
fn verify_extra_files(manifest: &Manifest, backend: &dyn PackageBackend, report: &mut Report) {
    let referenced: HashSet<&str> = manifest.referenced_paths().into_iter().collect();
    for file in backend.list_files() {
        if backend.meta_files().contains(&file.as_str()) {
            continue;
        }
        if referenced.contains(file.as_str()) {
            continue;
        }
        report.push_warning(format!("unreferenced file '{file}'"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::manifest::ParsedManifest;

    /// An in-memory backend for exercising the orchestration without a
    /// filesystem.
    #[derive(Default)]
    struct MemoryBackend {
        files: std::collections::HashMap<String, Vec<u8>>,
        unsafe_paths: HashSet<String>,
    }

    impl MemoryBackend {
        fn with(mut self, path: &str, bytes: &[u8]) -> Self {
            self.files.insert(path.to_string(), bytes.to_vec());
            self
        }
    }

    impl PackageBackend for MemoryBackend {
        fn open_asset(&self, path: &str) -> Result<crate::storage::OpenedAsset, BackendError> {
            if self.unsafe_paths.contains(path) {
                return Err(BackendError::UnsafePath);
            }
            match self.files.get(path) {
                None => Err(BackendError::Missing),
                Some(bytes) => Ok(crate::storage::OpenedAsset {
                    len: bytes.len() as u64,
                    reader: Box::new(std::io::Cursor::new(bytes.clone())),
                }),
            }
        }

        fn object_id(&self, path: &str) -> Option<ObjectId> {
            self.files.get(path).map(|bytes| ObjectId {
                device: 1,
                inode: bytes.len() as u64,
            })
        }

        fn list_files(&self) -> Vec<String> {
            let mut files: Vec<String> = self.files.keys().cloned().collect();
            files.sort();
            files
        }
    }

    fn manifest_for(path: &str, digest: &str) -> Manifest {
        let json = format!(
            r#"{{"format":"musicpack","version":1,"album":{{"title":"T","artists":[{{"name":"A"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"One","audio":{{"path":"{path}","sha256":"{digest}"}}}}]}}]}}"#
        );
        ParsedManifest::parse(json.as_bytes())
            .expect("parses")
            .into_manifest()
    }

    #[test]
    fn valid_package_passes() {
        let digest = checksum::sha256_hex(b"one");
        let manifest = manifest_for("audio/01.bin", &digest);
        let backend = MemoryBackend::default().with("audio/01.bin", b"one");
        let report = verify(&manifest, &backend);
        assert!(report.is_ok(), "{:?}", report.findings());
        assert_eq!(report.errors(), 0);
        assert_eq!(report.warnings(), 0);
    }

    #[test]
    fn checksum_mismatch_and_missing() {
        let digest = checksum::sha256_hex(b"one");
        let manifest = manifest_for("audio/01.bin", &digest);

        let wrong = MemoryBackend::default().with("audio/01.bin", b"two");
        let report = verify(&manifest, &wrong);
        assert_eq!(report.errors(), 1);
        assert!(report.findings()[0].message.contains("checksum mismatch"));
        assert!(report.findings()[0].message.starts_with("track: "));

        let absent = MemoryBackend::default();
        let report = verify(&manifest, &absent);
        assert_eq!(report.errors(), 1);
        assert!(report.findings()[0].message.contains("missing file"));
    }

    #[test]
    fn unsafe_path_is_its_own_category() {
        let digest = checksum::sha256_hex(b"one");
        let manifest = manifest_for("audio/01.bin", &digest);
        let mut backend = MemoryBackend::default().with("audio/01.bin", b"one");
        backend.unsafe_paths.insert("audio/01.bin".to_string());
        let report = verify(&manifest, &backend);
        assert_eq!(report.errors(), 1);
        assert!(report.findings()[0].message.contains("unsafe path"));
    }

    #[test]
    fn unreferenced_files_warn() {
        let digest = checksum::sha256_hex(b"one");
        let manifest = manifest_for("audio/01.bin", &digest);
        let mut backend = MemoryBackend::default().with("audio/01.bin", b"one");
        backend
            .files
            .insert("extras/notes.txt".to_string(), b"x".to_vec());
        backend
            .files
            .insert("manifest.json".to_string(), b"{}".to_vec());
        // The memory backend declares no meta files, so manifest.json is a
        // warning here; the directory backend excludes it.
        let report = verify(&manifest, &backend);
        assert!(report.is_ok());
        assert_eq!(report.warnings(), 2);
        assert!(
            report
                .findings()
                .iter()
                .all(|f| f.severity == Severity::Warning)
        );
    }

    #[test]
    fn deterministic_order_multiple_failures() {
        let json = r#"{"format":"musicpack","version":1,"album":{"title":"T","artists":[{"name":"A"}]},"media":[{"disc":1,"tracks":[
            {"track":1,"title":"One","audio":{"path":"audio/01.bin","sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}},
            {"track":2,"title":"Two","audio":{"path":"audio/02.bin","sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}}
        ]},{"disc":2,"tracks":[{"track":1,"title":"Three","audio":{"path":"audio/03.bin","sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}}]}],
        "artwork":[{"role":"front","path":"artwork/front.jpg","sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}]}"#;
        let manifest = ParsedManifest::parse(json.as_bytes())
            .expect("parses")
            .into_manifest();
        let backend = MemoryBackend::default();
        let report = verify(&manifest, &backend);
        assert_eq!(report.errors(), 4);
        let kinds: Vec<&str> = report
            .findings()
            .iter()
            .map(|f| f.message.split(':').next().unwrap())
            .collect();
        // Disc 1 tracks first, then disc 2, then artwork: reference order.
        assert_eq!(kinds, vec!["track", "track", "track", "artwork"]);
    }

    #[test]
    fn waveform_payload_and_duration() {
        // 10 points → 20 bytes.
        let payload = vec![0u8; 20];
        let digest = checksum::sha256_hex(&payload);
        let make_json = |duration: &str| {
            format!(
                r#"{{"format":"musicpack","version":1,"album":{{"title":"T","artists":[{{"name":"A"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"One","duration":{duration},"audio":{{"path":"audio/01.bin","sha256":"{audio}"}},"waveform":{{"version":1,"path":"analysis/waveform/01-01.wfm","sha256":"{digest}","intervalMs":100,"encoding":"peak-rms-u8","floorDb":-60,"points":10}}}}]}}]}}"#,
                audio = checksum::sha256_hex(b"one"),
            )
        };
        let parse = |json: &str| {
            ParsedManifest::parse(json.as_bytes())
                .expect("parses")
                .into_manifest()
        };
        let good = MemoryBackend::default()
            .with("audio/01.bin", b"one")
            .with("analysis/waveform/01-01.wfm", &payload);

        // duration 1.0 s expects 10 points: consistent, no warnings.
        let report = verify(&parse(&make_json("1.0")), &good);
        assert!(report.is_ok(), "{:?}", report.findings());
        assert_eq!(report.warnings(), 0);

        // duration 9.0 s expects ~90 points vs declared 10 → warning only.
        let report = verify(&parse(&make_json("9.0")), &good);
        assert!(report.is_ok(), "{:?}", report.findings());
        assert_eq!(report.warnings(), 1);
        assert!(
            report.findings()[0]
                .message
                .contains("differs from duration")
        );

        // Payload size inconsistent with points.
        let bad = MemoryBackend::default()
            .with("audio/01.bin", b"one")
            .with("analysis/waveform/01-01.wfm", b"short");
        let report = verify(&parse(&make_json("1.0")), &bad);
        assert!(!report.is_ok());
        assert!(
            report
                .findings()
                .iter()
                .any(|f| f.message.contains("payload size inconsistent"))
        );
    }

    #[test]
    fn per_file_size_limit() {
        let digest = "0".repeat(64);
        let manifest = manifest_for("audio/big.bin", &digest);
        struct Huge;
        impl PackageBackend for Huge {
            fn open_asset(&self, _path: &str) -> Result<crate::storage::OpenedAsset, BackendError> {
                Ok(crate::storage::OpenedAsset {
                    len: MAX_FILE_BYTES + 1,
                    reader: Box::new(std::io::empty()),
                })
            }
            fn object_id(&self, _path: &str) -> Option<ObjectId> {
                None
            }
        }
        let report = verify(&manifest, &Huge);
        assert_eq!(report.errors(), 1);
        assert!(report.findings()[0].message.contains("exceeds"));
        assert!(report.findings()[0].message.contains("file limit"));
    }

    #[test]
    fn aggregate_byte_budget() {
        // Nine 8 GiB assets = 72 GiB > the 64 GiB aggregate budget. Each
        // file passes the per-file limit; the ninth trips the aggregate.
        // A fake backend keeps this instant (no 64 GiB of I/O).
        struct ExactlyEightGib;
        impl PackageBackend for ExactlyEightGib {
            fn open_asset(&self, _path: &str) -> Result<crate::storage::OpenedAsset, BackendError> {
                Ok(crate::storage::OpenedAsset {
                    len: 8 * 1024 * 1024 * 1024,
                    reader: Box::new(std::io::empty()),
                })
            }
            fn object_id(&self, _path: &str) -> Option<ObjectId> {
                None // no dedup: every path counts
            }
        }
        // The fake backend streams no bytes, so the declared digest must
        // be the empty-stream digest for hashing to succeed and leave the
        // budget as the only failure.
        let empty = checksum::sha256_hex(b"");
        let tracks: Vec<String> = (1..=9)
            .map(|i| {
                format!(
                    r#"{{"track":{i},"title":"t","audio":{{"path":"audio/{i:02}.bin","sha256":"{empty}"}}}}"#
                )
            })
            .collect();
        let json = format!(
            r#"{{"format":"musicpack","version":1,"album":{{"title":"T","artists":[{{"name":"A"}}]}},"media":[{{"disc":1,"tracks":[{}]}}]}}"#,
            tracks.join(",")
        );
        let manifest = ParsedManifest::parse(json.as_bytes())
            .expect("parses")
            .into_manifest();
        let report = verify(&manifest, &ExactlyEightGib);
        assert_eq!(report.errors(), 1);
        assert!(
            report.findings()[0]
                .message
                .contains("aggregate referenced bytes exceed"),
            "{:?}",
            report.findings()
        );
        // Eight exactly fit.
        let tracks8: Vec<String> = tracks.iter().take(8).cloned().collect();
        let json8 = format!(
            r#"{{"format":"musicpack","version":1,"album":{{"title":"T","artists":[{{"name":"A"}}]}},"media":[{{"disc":1,"tracks":[{}]}}]}}"#,
            tracks8.join(",")
        );
        let manifest8 = ParsedManifest::parse(json8.as_bytes())
            .expect("parses")
            .into_manifest();
        assert!(verify(&manifest8, &ExactlyEightGib).is_ok());
    }

    #[test]
    fn referenced_paths_lists_every_asset_group() {
        let json = r#"{"format":"musicpack","version":1,"album":{"title":"T","artists":[{"name":"A"}]},"media":[{"disc":1,"tracks":[{"track":1,"title":"One","audio":{"path":"audio/01.bin","sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"},"waveform":{"version":1,"path":"analysis/waveform/01-01.wfm","sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","intervalMs":100,"encoding":"peak-rms-u8","floorDb":-60,"points":0},"representations":[{"path":"audio/01.flac","sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}]}]}],"artwork":[{"role":"front","path":"artwork/f.jpg","sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}],"booklet":[{"path":"booklet/b.pdf","sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}],"lyrics":[{"path":"lyrics/1.lrc","sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}],"extras":[{"path":"extras/n.txt","sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}],"analysis":[{"type":"future","path":"analysis/x.json","sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}]}"#;
        let manifest = ParsedManifest::parse(json.as_bytes())
            .expect("parses")
            .into_manifest();
        assert_eq!(
            manifest.referenced_paths(),
            vec![
                "audio/01.bin",
                "analysis/waveform/01-01.wfm",
                "audio/01.flac",
                "artwork/f.jpg",
                "booklet/b.pdf",
                "lyrics/1.lrc",
                "extras/n.txt",
                "analysis/x.json",
            ]
        );
    }
}
