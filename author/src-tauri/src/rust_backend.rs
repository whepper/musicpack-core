// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// RustBackend: the default Author runtime (R4.3).
//
// The application used to shell out to the frozen C `musicpack` CLI (and
// `mpcenc`/`musicpack-sonic`). This module replaces that with an in-process
// adapter over `musicpack-author`, so the normal authoring path spawns no
// subprocess and no legacy binary:
//
//     Tauri command -> RustBackend -> musicpack-author -> musicpack-core
//
// Package semantics stay in `musicpack-core`; orchestration (draft JSON,
// encoding, waveform, `build_directory`, `.mpak`) stays in
// `musicpack-author`. This module only translates the UI-level request and
// the typed Rust error into the shapes the frontend speaks.
//
// The legacy CLI path (`author_service.rs`) remains for `tauri dev` behind
// an explicit, non-default `MUSICPACK_AUTHOR_LEGACY=1` escape hatch.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use musicpack_author_pipeline as author;
use musicpack_core::storage::{directory, mpak};
use musicpack_core::validation::{Report, Severity};

use crate::author_service::BackendInfo;
use crate::{musicbrainz, track_lyrics};

/// Author API version spoken by this backend. Bumping it is a UI/host
/// contract change (see `docs/author-runtime.md` §API version).
pub const AUTHOR_API: u32 = 8;

/// MusicBrainz requests are paced to the service's ~1 req/s expectation.
const MB_MINIMUM_INTERVAL: Duration = Duration::from_secs(1);

/// A typed, UI-facing backend error: `{ code, message }`.
#[derive(Debug, Clone, Serialize)]
pub struct HostError {
    /// Stable machine-readable code (never prose).
    pub code: String,
    /// Human-readable detail (includes disc/track context where relevant).
    pub message: String,
}

impl HostError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    fn io(message: impl Into<String>) -> Self {
        Self::new("io_failed", message)
    }
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for HostError {}

impl From<author::AuthorError> for HostError {
    fn from(error: author::AuthorError) -> Self {
        use author::AuthorError as E;
        match error {
            E::Draft { detail } => HostError::new("invalid_draft", detail),
            E::Validation { errors, .. } => HostError::new("invalid_draft", errors.join("; ")),
            E::Encode {
                disc,
                track,
                detail,
            } => HostError::new(
                "encode_failed",
                format!("disc {disc} track {track}: {detail}"),
            ),
            E::Unsupported { detail } => HostError::new("unsupported", detail),
            E::Identification { detail } => HostError::new("musicbrainz_failed", detail),
            E::Build(error) => HostError::new("build_failed", error.to_string()),
            E::Pack { detail } => HostError::new("pack_failed", detail),
            E::Io { detail } => HostError::new("io_failed", detail),
            E::Cancelled => HostError::new("cancelled", "operation cancelled"),
            other => HostError::new("build_failed", other.to_string()),
        }
    }
}

impl From<musicpack_core::Error> for HostError {
    fn from(error: musicpack_core::Error) -> Self {
        HostError::new("build_failed", error.to_string())
    }
}

impl From<crate::author_service::AuthorError> for HostError {
    fn from(error: crate::author_service::AuthorError) -> Self {
        use crate::author_service::AuthorError as E;
        match error {
            E::CliNotFound(message) => HostError::new("backend_unavailable", message),
            E::Io(message) => HostError::new("io_failed", message),
            E::CliFailure { code, message } => {
                HostError::new(code.unwrap_or_else(|| "legacy_failed".into()), message)
            }
            E::Output(message) => HostError::new("io_failed", message),
            E::MusicBrainz(error) => HostError::new("musicbrainz_failed", error.to_string()),
            E::IncompatibleBackend {
                expected,
                found,
                version,
            } => HostError::new(
                "backend_incompatible",
                format!(
                    "incompatible authoring backend: {version} speaks author API {found}, \
                     MusicPack Author requires {expected}"
                ),
            ),
            E::Lyrics(message) => HostError::new("invalid_lyrics", message),
        }
    }
}

/// Removes an Author-owned encode staging directory, refusing anything that
/// is not one (defense in depth against a stray path from the frontend).
/// Shared by both backends (the Rust stages create the same prefix).
pub fn cleanup_staging(dir: &str) -> Result<(), HostError> {
    let d = PathBuf::from(dir);
    let temp = std::fs::canonicalize(std::env::temp_dir())
        .map_err(|e| HostError::io(format!("cannot resolve temporary directory: {e}")))?;
    let parent = d.parent().and_then(|p| std::fs::canonicalize(p).ok());
    let prefix = format!("musicpack-author-encode-{}", std::process::id());
    let name = d
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !name.starts_with(&prefix) || parent.as_deref() != Some(temp.as_path()) {
        return Err(HostError::new(
            "io_failed",
            format!("refusing to remove {dir}: not a MusicPack Author staging directory"),
        ));
    }
    std::fs::remove_dir_all(&d)
        .map_err(|e| HostError::io(format!("cannot remove staging directory: {e}")))?;
    Ok(())
}

/// Renders a core verification report as the CLI's `{ok, errors, warnings}`.
fn report_json(report: &Report) -> Value {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    for finding in report.findings() {
        match finding.severity {
            Severity::Error => errors.push(Value::String(finding.message.clone())),
            Severity::Warning => warnings.push(Value::String(finding.message.clone())),
        }
    }
    json!({ "ok": report.is_ok(), "errors": errors, "warnings": warnings })
}

/// Live MusicBrainz transport (`ureq`, already used by the host) behind the
/// pipeline's provider abstraction. Matching/application stays in
/// `musicpack-author`.
struct LiveMusicBrainz;

impl author::MusicBrainzProvider for LiveMusicBrainz {
    fn fetch_release(&self, mbid: &str) -> Result<Vec<u8>, String> {
        musicbrainz::fetch_release(mbid)
            .map(String::into_bytes)
            .map_err(|e| e.to_string())
    }
    fn search_barcode(&self, barcode: &str) -> Result<Vec<u8>, String> {
        musicbrainz::fetch_barcode_search(barcode)
            .map(String::into_bytes)
            .map_err(|e| e.to_string())
    }
}

/// The in-process Rust authoring backend.
pub struct RustBackend {
    version: String,
    last_mb_request: Option<Instant>,
}

impl RustBackend {
    pub fn new() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            last_mb_request: None,
        }
    }

    pub fn backend_info(&self) -> BackendInfo {
        BackendInfo {
            musicpack_version: self.version.clone(),
            author_api: AUTHOR_API,
            location: "rust".to_string(),
        }
    }

    fn pace_musicbrainz(&mut self) {
        if let Some(last) = self.last_mb_request {
            if let Some(remaining) = MB_MINIMUM_INTERVAL.checked_sub(last.elapsed()) {
                std::thread::sleep(remaining);
            }
        }
        self.last_mb_request = Some(Instant::now());
    }

    // ---- package operations -------------------------------------------

    /// Opens an existing `.mpack` directory as an authoring draft.
    pub fn inspect_album(&mut self, path: &str) -> Result<Value, HostError> {
        let draft = author::inspect::package_to_draft(Path::new(path))?;
        serde_json::from_str(&draft)
            .map_err(|e| HostError::new("inspect_failed", format!("invalid draft JSON: {e}")))
    }

    /// Structural validation plus the R3.5 lyric-content findings, merged
    /// into one verdict (the panel's single source of truth).
    pub fn validate_draft(&mut self, draft_json: &str) -> Result<Value, HostError> {
        let mut report = author::validate_json(draft_json.as_bytes())?;
        merge_lyric_findings(draft_json, &mut report);
        Ok(json!({
            "ok": report.errors.is_empty(),
            "errors": report.errors,
            "warnings": report.warnings,
        }))
    }

    pub fn identify_draft(
        &mut self,
        draft_json: &str,
        mbid: Option<&str>,
        barcode: Option<&str>,
        mb_json: Option<&str>,
    ) -> Result<Value, HostError> {
        if let Some(id) = mbid {
            self.pace_musicbrainz();
            let (draft, confidence, applied) =
                author::identify_mbid_json(&LiveMusicBrainz, draft_json.as_bytes(), id)?;
            return applied_result(&draft, confidence, applied);
        }
        if let Some(bc) = barcode {
            self.pace_musicbrainz();
            let doc = author::MusicBrainzProvider::search_barcode(&LiveMusicBrainz, bc)
                .map_err(|e| HostError::new("musicbrainz_failed", e))?;
            let candidates = author::identify_candidates_json(draft_json.as_bytes(), &doc)?;
            return Ok(json!({
                "kind": "candidates",
                "candidates": candidates.iter().map(candidate_json).collect::<Vec<_>>(),
            }));
        }
        if let Some(doc) = mb_json {
            let (draft, confidence, applied) =
                author::identify_apply_json(draft_json.as_bytes(), doc.as_bytes(), None)?;
            return applied_result(&draft, confidence, applied);
        }
        Err(HostError::new(
            "identify_failed",
            "no identification source: provide a MusicBrainz id, barcode or document",
        ))
    }

    /// Builds a `.mpack` directory. `replace` publishes atomically over an
    /// existing package (a failed build never destroys the previous one);
    /// `sync_tags` is accepted for UI compatibility and has no effect (the
    /// Rust path rebuilds deterministically — APEv2 sync is retired, see
    /// `docs/author-runtime.md`).
    pub fn create_package(
        &mut self,
        draft_json: &str,
        output_dir: &str,
        replace: bool,
        _sync_tags: bool,
    ) -> Result<Value, HostError> {
        stage_lyrics_validation(draft_json)?;
        let options = author::PipelineOptions {
            quality: author::encode::DEFAULT_QUALITY,
            waveform: waveform_enabled(draft_json),
            loudness: musicpack_core::authoring::LoudnessMode::Measure,
            mpak: None,
            replace,
        };
        let request = author::AuthorRequest {
            draft_json: draft_json.as_bytes(),
            output: Path::new(output_dir),
            options,
            identify: None,
        };
        let outcome = author::run(&request)?;
        Ok(json!({
            "ok": true,
            "outputPath": outcome.output.to_string_lossy(),
            "replaced": replace,
            "verify": { "errors": outcome.report.errors(), "warnings": outcome.report.warnings() },
        }))
    }

    /// Builds the draft and packs it into a single-file `.mpak`, removing
    /// the intermediate package directory.
    pub fn create_mpak(&mut self, draft_json: &str, output_mpak: &str) -> Result<Value, HostError> {
        stage_lyrics_validation(draft_json)?;
        if Path::new(output_mpak).exists() {
            return Err(HostError::new(
                "output_exists",
                format!("output '{output_mpak}' already exists"),
            ));
        }
        let staging = unique_staging_dir("mpak")?;
        let options = author::PipelineOptions {
            quality: author::encode::DEFAULT_QUALITY,
            waveform: waveform_enabled(draft_json),
            loudness: musicpack_core::authoring::LoudnessMode::Measure,
            mpak: Some(PathBuf::from(output_mpak)),
            replace: false,
        };
        let request = author::AuthorRequest {
            draft_json: draft_json.as_bytes(),
            output: &staging,
            options,
            identify: None,
        };
        let result = author::run(&request);
        let _ = std::fs::remove_dir_all(&staging);
        result?;
        Ok(json!({ "ok": true, "outputPath": output_mpak }))
    }

    /// Verifies a `.mpack` directory, then packs it into `.mpak`. The source
    /// is preserved.
    pub fn pack_package(&mut self, input_dir: &str, output_mpak: &str) -> Result<Value, HostError> {
        let report = verify_path(input_dir)?;
        if !report.is_ok() {
            let detail: Vec<String> = report
                .findings()
                .iter()
                .filter(|f| f.severity == Severity::Error)
                .map(|f| f.message.clone())
                .collect();
            return Err(HostError::new(
                "verification_failed",
                format!("source package failed verification: {}", detail.join("; ")),
            ));
        }
        if Path::new(output_mpak).exists() {
            return Err(HostError::new(
                "output_exists",
                format!("output '{output_mpak}' already exists"),
            ));
        }
        directory::pack_directory(Path::new(input_dir), Path::new(output_mpak))?;
        Ok(json!({ "ok": true, "outputPath": output_mpak }))
    }

    /// The `{ok, errors, warnings}` verdict for a `.mpack` directory or a
    /// `.mpak` container.
    pub fn verify_package(&mut self, path: &str) -> Result<Value, HostError> {
        Ok(report_json(&verify_path(path)?))
    }

    // ---- staged operations (encode / waveform) ------------------------

    /// Encodes every track into `staging` and returns the transformed draft
    /// JSON (whose `sourceRoot` is `staging`). Emits per-track progress via
    /// `on_track`.
    pub fn encode_stage(
        &mut self,
        draft_json: &str,
        staging: &Path,
        quality: f32,
        on_track: &mut dyn FnMut(usize, usize, i32, i32, &str) -> bool,
    ) -> Result<String, HostError> {
        author::encode_stage_with(draft_json.as_bytes(), staging, quality, on_track)
            .map_err(HostError::from)
    }

    /// Generates waveform envelopes into `staging` and returns the
    /// transformed draft JSON.
    pub fn waveform_stage(
        &mut self,
        draft_json: &str,
        staging: &Path,
        on_track: &mut dyn FnMut(usize, usize, &author::WaveformEntry) -> bool,
    ) -> Result<String, HostError> {
        author::waveform_stage_with(draft_json.as_bytes(), staging, on_track)
            .map(|(draft, _)| draft)
            .map_err(HostError::from)
    }

    /// Validates a single lyric file for the track editor (pure; no package
    /// semantics).
    pub fn lyrics_probe(&self, path: &str) -> Result<Value, HostError> {
        match track_lyrics::probe_file(Path::new(path)) {
            Ok(probe) => Ok(json!({
                "ok": true,
                "synced": probe.synced,
                "lines": probe.lines,
            })),
            Err(message) => Ok(json!({
                "ok": false,
                "error": { "code": "invalid_lyrics", "message": message },
            })),
        }
    }
}

impl Default for RustBackend {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------

fn applied_result(
    draft: &str,
    confidence: author::Confidence,
    applied: bool,
) -> Result<Value, HostError> {
    let draft: Value = serde_json::from_str(draft)
        .map_err(|e| HostError::new("identify_failed", format!("invalid draft JSON: {e}")))?;
    Ok(json!({
        "kind": "applied",
        "draft": draft,
        "confidence": confidence.as_str(),
        "applied": applied,
    }))
}

fn candidate_json(candidate: &author::identify::MbCandidate) -> Value {
    json!({
        "releaseId": candidate.release_id,
        "releaseGroupId": candidate.release_group_id,
        "title": candidate.title,
        "artist": candidate.artist,
        "date": candidate.date,
        "country": candidate.country,
        "barcode": candidate.barcode,
        "confidence": candidate.confidence.as_str(),
    })
}

/// `true` unless the draft explicitly disabled waveform generation.
fn waveform_enabled(draft_json: &str) -> bool {
    serde_json::from_str::<Value>(draft_json)
        .ok()
        .and_then(|v| {
            v.get("waveformAnalysis")
                .and_then(|w| w.get("status"))
                .and_then(|s| s.as_str())
                .map(|s| s != "disabled")
        })
        .unwrap_or(true)
}

/// Validates the draft's track-linked lyrics (R3.5 content rules) before a
/// build, so an invalid `.lrc` fails the operation instead of being packaged.
fn stage_lyrics_validation(draft_json: &str) -> Result<(), HostError> {
    let Ok(draft) = serde_json::from_str::<Value>(draft_json) else {
        return Ok(());
    };
    let Some(source_root) = draft
        .get("sourceRoot")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
    else {
        return Ok(());
    };
    let refs = track_lyrics::collect_from_draft(&draft);
    if refs.is_empty() {
        return Ok(());
    }
    track_lyrics::snapshot_refs(Path::new(source_root), &refs)
        .map(|_| ())
        .map_err(|message| HostError::new("invalid_lyrics", message))
}

fn merge_lyric_findings(draft_json: &str, report: &mut author::ValidationReport) {
    let Ok(draft) = serde_json::from_str::<Value>(draft_json) else {
        return;
    };
    let Some(source_root) = draft
        .get("sourceRoot")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
    else {
        return;
    };
    let refs = track_lyrics::collect_from_draft(&draft);
    if refs.is_empty() {
        return;
    }
    report
        .errors
        .extend(track_lyrics::check_refs(Path::new(source_root), &refs));
}

fn verify_path(path: &str) -> Result<Report, HostError> {
    let p = Path::new(path);
    if p.is_dir() {
        directory::verify_directory(p).map_err(HostError::from)
    } else {
        mpak::verify_mpak_file(p).map_err(HostError::from)
    }
}

fn unique_staging_dir(tag: &str) -> Result<PathBuf, HostError> {
    let base = std::env::temp_dir();
    for n in 0..1000u32 {
        let candidate = base.join(format!(
            "musicpack-author-{tag}-{}-{n}.mpack",
            std::process::id()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(HostError::io("cannot allocate a fresh staging directory"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waveform_is_enabled_by_default_and_honours_disabled() {
        assert!(waveform_enabled("{}"));
        assert!(waveform_enabled(
            r#"{"waveformAnalysis":{"status":"pending"}}"#
        ));
        assert!(!waveform_enabled(
            r#"{"waveformAnalysis":{"status":"disabled"}}"#
        ));
    }

    #[test]
    fn report_json_separates_errors_and_warnings() {
        let value = json!({});
        let _ = value;
        // A clean report.
        let clean = Report::default();
        let rendered = report_json(&clean);
        assert_eq!(rendered["ok"], json!(true));
        assert_eq!(rendered["errors"], json!([]));
    }

    #[test]
    fn host_errors_carry_stable_codes() {
        // "sample rate96000" is genuinely unsupported (non-SV8 rate);
        // fractional quality has been valid since encoder parity J.1/J.2.
        let e: HostError = author::AuthorError::Unsupported {
            detail: "unsupported sample rate96000 Hz".into(),
        }
        .into();
        assert_eq!(e.code, "unsupported");
        assert!(e.message.contains("unsupported sample rate96000 Hz"));

        let e: HostError = author::AuthorError::Encode {
            disc: 1,
            track: 2,
            detail: "cannot decode".into(),
        }
        .into();
        assert_eq!(e.code, "encode_failed");
        assert!(e.message.contains("disc 1 track 2"));
    }
}
