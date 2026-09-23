//! The authoring pipeline: draft JSON → encode → waveform → `.mpack` →
//! optional `.mpak`.
//!
//! ```text
//! draft JSON ──parse──► Draft ──identify──► Draft ──validate──► Draft
//!                                                            │
//!                        ┌───────────────────────────────────┤
//!                        ▼                                   ▼
//!                 encode / copy audio              stage artwork/assets
//!                        │                                   │
//!                        └──────────► work tree ◄────────────┘
//!                                        │
//!                         AuthoringDraft + waveform payloads
//!                                        │
//!                    core::authoring::build_directory()  (sole builder)
//!                                        │
//!                                   core verify
//!                                        │
//!                                     .mpack ──pack──► .mpak
//! ```
//!
//! The pipeline owns orchestration only. Every package semantic — path
//! validation, hashing, manifest serialization, verification, identity — is
//! [`musicpack_core::authoring::build_directory`] and the shared core
//! verifier; the pipeline never writes `manifest.json` itself and never
//! duplicates a package rule.
//!
//! # Staging
//!
//! Encoded audio and generated waveform payloads need real files, so the
//! pipeline materializes a work tree (like the reference's encode staging),
//! then hands it to the core builder, which stages and publishes atomically.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use musicpack_core::authoring::{
    AuthoringDraft, BuildOptions, BuildOutcome, DraftAnalysis, DraftArtwork, DraftAsset, DraftDisc,
    DraftLyrics, DraftRepresentation, DraftTrack, DraftWaveform,
};
use musicpack_core::format::checksum;
use musicpack_core::format::manifest::Manifest;
use musicpack_core::json::{self, Value};
use musicpack_core::storage::directory;
use musicpack_core::validation::Report;

use crate::draft::{
    self, Draft, DraftTrack as JsonTrack, ValidationReport, json_array_mut, json_object_mut,
    json_remove, json_set,
};
use crate::error::{AuthorError, Result};
use crate::identify::{self, Confidence, MusicBrainzProvider};
use crate::{artwork, encode, waveform};

/// Pipeline options.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineOptions {
    /// Musepack quality (default 6.0, the Author default).
    pub quality: f32,
    /// Generate per-track waveform envelopes (default on).
    pub waveform: bool,
    /// Whether the core builder measures loudness/duration.
    pub loudness: musicpack_core::authoring::LoudnessMode,
    /// Optional `.mpak` output.
    pub mpak: Option<PathBuf>,
    /// Replace an existing output directory atomically.
    pub replace: bool,
}

impl Default for PipelineOptions {
    fn default() -> Self {
        Self {
            quality: encode::DEFAULT_QUALITY,
            waveform: true,
            loudness: musicpack_core::authoring::LoudnessMode::Measure,
            mpak: None,
            replace: false,
        }
    }
}

/// How the draft should be identified before building.
pub enum IdentifyRequest<'a> {
    /// Apply an offline MusicBrainz release document (`--mb-json`).
    Document {
        /// The release document bytes.
        doc: &'a [u8],
        /// An asserted release id (`--mbid`).
        asserted_mbid: Option<&'a str>,
    },
    /// Fetch and apply a release by id through a provider.
    ReleaseId {
        /// The transport provider.
        provider: &'a dyn MusicBrainzProvider,
        /// The release id.
        mbid: &'a str,
    },
}

/// A complete authoring request.
pub struct AuthorRequest<'a> {
    /// The draft JSON bytes.
    pub draft_json: &'a [u8],
    /// The `.mpack` directory to create (or replace).
    pub output: &'a Path,
    /// Pipeline options.
    pub options: PipelineOptions,
    /// Optional identification step applied before validation.
    pub identify: Option<IdentifyRequest<'a>>,
}

/// What the pipeline produced.
#[derive(Debug, Clone)]
pub struct AuthorOutcome {
    /// The built manifest.
    pub manifest: Manifest,
    /// SHA-256 of the canonical `manifest.json`.
    pub manifest_sha256: String,
    /// Package fingerprint.
    pub fingerprint: String,
    /// Release-group key.
    pub group_key: String,
    /// Release/edition key.
    pub release_key: String,
    /// Core verification report (always clean on success).
    pub report: Report,
    /// Draft validation verdict (warnings are informational).
    pub validation: ValidationReport,
    /// The published `.mpack` directory.
    pub output: PathBuf,
    /// The optional `.mpak` container.
    pub mpak: Option<PathBuf>,
}

/// A staged audio asset (package path + work-tree path).
#[derive(Debug, Clone)]
struct StagedAudio {
    package: String,
    work: String,
}

/// A staged waveform payload (package path + work-tree path).
#[derive(Debug, Clone)]
struct StagedWaveform {
    package: String,
    work: String,
}

/// A staged pass-through asset (package path + work-tree path).
#[derive(Debug, Clone)]
struct StagedAsset {
    package: String,
    work: String,
}

/// A staged artwork asset (role + package path + work-tree path).
#[derive(Debug, Clone)]
struct StagedArtwork {
    role: String,
    package: String,
    work: String,
}

/// A staged analysis-document asset.
#[derive(Debug, Clone)]
struct StagedAnalysis {
    kind: String,
    profile: Option<String>,
    package: String,
    work: String,
}

/// Parses and validates a draft, returning its verdict (no side effects).
pub fn validate_json(draft_json: &[u8]) -> Result<ValidationReport> {
    let parsed = draft::parse(draft_json)?;
    Ok(draft::validate(&parsed))
}

/// Runs the full pipeline.
pub fn run(request: &AuthorRequest<'_>) -> Result<AuthorOutcome> {
    let mut draft = draft::parse(request.draft_json)?;

    if let Some(identify) = &request.identify {
        apply_identification(&mut draft, identify)?;
    }

    let validation = draft::validate(&draft);
    if !validation.is_ok() {
        return Err(AuthorError::Validation {
            errors: validation.errors.clone(),
            warnings: validation.warnings.clone(),
        });
    }

    if request.output.exists() && !request.options.replace {
        return Err(AuthorError::Io {
            detail: format!(
                "output destination '{}' already exists",
                request.output.display()
            ),
        });
    }
    if let Some(mpak) = &request.options.mpak
        && mpak.exists()
    {
        return Err(AuthorError::Io {
            detail: format!("output '{}' already exists", mpak.display()),
        });
    }

    let works = WorkTree::create(request.output)?;
    let mut namer = UniqueNamer::default();

    // ---- audio: encode or pass through ----
    let mut audio: Vec<Vec<StagedAudio>> = Vec::new();
    let multi_disc = draft.media.len() > 1;
    for disc in &draft.media {
        let mut per_track = Vec::new();
        for track in &disc.tracks {
            let staged = stage_audio(
                &draft,
                disc.number,
                track,
                multi_disc,
                &works,
                &mut namer,
                request.options.quality,
            )?;
            per_track.push(staged);
        }
        audio.push(per_track);
    }

    // ---- waveform envelopes from the packaged audio ----
    let mut waveforms: Vec<Vec<Option<StagedWaveform>>> = Vec::new();
    if request.options.waveform {
        for (di, disc) in draft.media.iter().enumerate() {
            let mut per_track = Vec::new();
            for (ti, track) in disc.tracks.iter().enumerate() {
                let staged = &audio[di][ti];
                let (pkg_path, work_path) = waveform::payload_names(disc.number, track.number);
                let dest = works.path().join(&work_path);
                create_parent(&dest)?;
                waveform::generate_to(&works.path().join(&staged.work), &dest)?;
                per_track.push(Some(StagedWaveform {
                    package: pkg_path,
                    work: work_path,
                }));
            }
            waveforms.push(per_track);
        }
    } else {
        for disc in &draft.media {
            waveforms.push((0..disc.tracks.len()).map(|_| None).collect());
        }
    }

    // ---- artwork + document assets ----
    let artwork = stage_artwork(&draft, &works, &mut namer)?;
    let booklet = stage_plain(&draft, &draft.booklet, "booklet", &works, &mut namer)?;
    let root_lyrics = stage_plain(&draft, &draft.lyrics, "lyrics", &works, &mut namer)?;
    let extras = stage_plain(&draft, &draft.extras, "extras", &works, &mut namer)?;
    let analysis = stage_analysis(&draft, &works, &mut namer)?;

    // ---- assemble the core authoring draft ----
    let assembled = assemble(
        &draft,
        &audio,
        &waveforms,
        &artwork,
        &booklet,
        &root_lyrics,
        &extras,
        &analysis,
        &works,
        &mut namer,
    )?;

    // ---- build through the core (the sole package constructor) ----
    let build_options = BuildOptions {
        loudness: request.options.loudness,
    };
    let staging = request.output.with_file_name(format!(
        ".{}.new-{}",
        file_name(request.output)?,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&staging);
    let build: BuildOutcome = musicpack_core::authoring::build_directory(
        &assembled,
        works.path(),
        &staging,
        &build_options,
    )?;
    publish(&staging, request.output, request.options.replace)?;

    // ---- optional .mpak ----
    let mpak = match &request.options.mpak {
        None => None,
        Some(path) => {
            directory::pack_directory(request.output, path).map_err(|e| AuthorError::Pack {
                detail: e.to_string(),
            })?;
            Some(path.clone())
        }
    };

    Ok(AuthorOutcome {
        manifest: build.manifest,
        manifest_sha256: build.manifest_sha256,
        fingerprint: build.fingerprint,
        group_key: build.group_key,
        release_key: build.release_key,
        report: build.report,
        validation,
        output: request.output.to_path_buf(),
        mpak,
    })
}

// ---------------------------------------------------------------------
// split stages (the Author UI runs encode and waveform before the build)
// ---------------------------------------------------------------------

/// The **encode** stage (`encode-draft`): encodes/passes through every track
/// into `staging`, stages the authored assets alongside, and returns the
/// transformed draft JSON whose `sourceRoot` is `staging` and whose
/// `audioPath` values point at the encoded `.mpc` files.
///
/// The caller owns `staging` (created here if missing, never removed here).
/// Derived values stay derived: this rewrites only authored references.
pub fn encode_stage(draft_json: &[u8], staging: &Path, quality: f32) -> Result<String> {
    encode_stage_with(draft_json, staging, quality, &mut |_, _, _, _, _| true)
}

/// [`encode_stage`] with a per-track progress callback
/// `(done, total, disc, track, title) -> continue?`. Returning `false`
/// cancels between tracks ([`AuthorError::Cancelled`]).
pub fn encode_stage_with(
    draft_json: &[u8],
    staging: &Path,
    quality: f32,
    on_track: &mut dyn FnMut(usize, usize, i32, i32, &str) -> bool,
) -> Result<String> {
    let mut root = json::parse(draft_json).map_err(|e| AuthorError::Draft {
        detail: format!("malformed draft JSON: {e}"),
    })?;
    let draft = draft::parse(draft_json)?;
    fs::create_dir_all(staging).map_err(|e| AuthorError::Io {
        detail: format!("cannot create '{}': {e}", staging.display()),
    })?;
    let works = WorkTree::borrowed(staging);
    let mut namer = UniqueNamer::default();
    let multi = draft.media.len() > 1;
    let total: usize = draft.media.iter().map(|d| d.tracks.len()).sum();
    let mut done = 0usize;

    for (di, disc) in draft.media.iter().enumerate() {
        for (ti, track) in disc.tracks.iter().enumerate() {
            let staged = stage_audio(
                &draft,
                disc.number,
                track,
                multi,
                &works,
                &mut namer,
                quality,
            )?;
            patch_track(&mut root, di, ti, &staged.work)?;
            done += 1;
            if !on_track(done, total, disc.number, track.number, &track.title) {
                return Err(AuthorError::Cancelled);
            }
        }
    }

    let artwork = stage_artwork(&draft, &works, &mut namer)?;
    for (i, a) in artwork.iter().enumerate() {
        patch_artwork(&mut root, i, &a.work)?;
    }
    patch_assets(
        &mut root,
        "booklet",
        &stage_plain(&draft, &draft.booklet, "booklet", &works, &mut namer)?,
    );
    patch_assets(
        &mut root,
        "lyrics",
        &stage_plain(&draft, &draft.lyrics, "lyrics", &works, &mut namer)?,
    );
    patch_assets(
        &mut root,
        "extras",
        &stage_plain(&draft, &draft.extras, "extras", &works, &mut namer)?,
    );
    patch_track_lyrics(&draft, &mut root, &works, &mut namer)?;
    for (i, a) in stage_analysis(&draft, &works, &mut namer)?
        .iter()
        .enumerate()
    {
        patch_analysis(&mut root, i, &a.work)?;
    }

    // The staged tree is now the source root; the draft is no longer editing
    // the package it may have been opened from.
    json_set(
        &mut root,
        "sourceRoot",
        Value::String(staging.to_string_lossy().into_owned()),
    );
    json_remove(&mut root, "openedFrom");
    Ok(json::print_canonical(&root))
}

/// One waveform entry produced by the waveform stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaveformEntry {
    /// Disc number.
    pub disc: i32,
    /// Track number.
    pub track: i32,
    /// Bucket count.
    pub points: u64,
    /// Lowercase hex SHA-256 of the payload.
    pub sha256: String,
    /// Staging-relative payload path.
    pub path: String,
}

/// The **waveform** stage (`waveform-draft`): writes canonical v1 envelopes
/// into `staging` and returns the draft JSON with a `waveformAnalysis` preview
/// block. Returns the transformed draft plus the per-track entries.
///
/// The build re-derives the authoritative envelopes; this stage exists for
/// the UI preview and mirrors the reference command's output shape.
pub fn waveform_stage(draft_json: &[u8], staging: &Path) -> Result<(String, Vec<WaveformEntry>)> {
    waveform_stage_with(draft_json, staging, &mut |_, _, _| true)
}

/// [`waveform_stage`] with a per-track progress callback
/// `(done, total, entry) -> continue?`. Returning `false` cancels between
/// tracks ([`AuthorError::Cancelled`]).
pub fn waveform_stage_with(
    draft_json: &[u8],
    staging: &Path,
    on_track: &mut dyn FnMut(usize, usize, &WaveformEntry) -> bool,
) -> Result<(String, Vec<WaveformEntry>)> {
    let mut root = json::parse(draft_json).map_err(|e| AuthorError::Draft {
        detail: format!("malformed draft JSON: {e}"),
    })?;
    let draft = draft::parse(draft_json)?;
    fs::create_dir_all(staging).map_err(|e| AuthorError::Io {
        detail: format!("cannot create '{}': {e}", staging.display()),
    })?;

    let mut entries = Vec::new();
    let total: usize = draft.media.iter().map(|d| d.tracks.len()).sum();
    for disc in &draft.media {
        for track in &disc.tracks {
            let source =
                draft::resolve_source(&draft.source_root, &track.audio_path).ok_or_else(|| {
                    AuthorError::Io {
                        detail: format!("source audio not found: {}", track.audio_path),
                    }
                })?;
            let name = format!("{:02}-{:02}.wfm", disc.number, track.number);
            let dest = staging.join(&name);
            let points = waveform::generate_to(&source, &dest)?;
            let bytes = fs::read(&dest).map_err(|e| AuthorError::Io {
                detail: format!("cannot read '{}': {e}", dest.display()),
            })?;
            entries.push(WaveformEntry {
                disc: disc.number,
                track: track.number,
                points,
                sha256: checksum::sha256_hex(&bytes),
                path: name,
            });
            let entry = entries.last().expect("just pushed");
            if !on_track(entries.len(), total, entry) {
                return Err(AuthorError::Cancelled);
            }
        }
    }

    let tracks = Value::Array(
        entries
            .iter()
            .map(|e| {
                Value::Object(vec![
                    ("disc".into(), Value::Number(f64::from(e.disc))),
                    ("track".into(), Value::Number(f64::from(e.track))),
                    ("points".into(), Value::Number(e.points as f64)),
                    ("sha256".into(), Value::String(e.sha256.clone())),
                    ("path".into(), Value::String(e.path.clone())),
                ])
            })
            .collect(),
    );
    let block = Value::Object(vec![
        ("status".into(), Value::String("ready".into())),
        (
            "intervalMs".into(),
            Value::Number(f64::from(musicpack_core::format::waveform::INTERVAL_MS)),
        ),
        (
            "encoding".into(),
            Value::String(musicpack_core::format::waveform::ENCODING.into()),
        ),
        (
            "floorDb".into(),
            Value::Number(f64::from(musicpack_core::format::waveform::FLOOR_DB)),
        ),
        ("tracks".into(), tracks),
        (
            "tracksGenerated".into(),
            Value::Number(entries.len() as f64),
        ),
        ("tracksTotal".into(), Value::Number(total as f64)),
        (
            "sourceRoot".into(),
            Value::String(staging.to_string_lossy().into_owned()),
        ),
    ]);
    json_set(&mut root, "waveformAnalysis", block);
    Ok((json::print_canonical(&root), entries))
}

fn obj_member_mut<'a>(value: &'a mut Value, key: &str) -> Option<&'a mut Value> {
    json_object_mut(value)?
        .iter_mut()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
}

fn patch_track(root: &mut Value, di: usize, ti: usize, audio_path: &str) -> Result<()> {
    let disc = json_array_mut(obj_member_mut(root, "media").ok_or_else(bad_draft)?)
        .ok_or_else(bad_draft)?
        .get_mut(di)
        .ok_or_else(bad_draft)?;
    let track = json_array_mut(obj_member_mut(disc, "tracks").ok_or_else(bad_draft)?)
        .ok_or_else(bad_draft)?
        .get_mut(ti)
        .ok_or_else(bad_draft)?;
    json_set(track, "audioPath", Value::String(audio_path.to_string()));
    json_set(track, "codec", Value::String("musepack-sv8".to_string()));
    Ok(())
}

fn patch_artwork(root: &mut Value, index: usize, path: &str) -> Result<()> {
    let entry = json_array_mut(obj_member_mut(root, "artwork").ok_or_else(bad_draft)?)
        .ok_or_else(bad_draft)?
        .get_mut(index)
        .ok_or_else(bad_draft)?;
    json_set(entry, "path", Value::String(path.to_string()));
    json_remove(entry, "embedded");
    json_remove(entry, "sourceAudio");
    Ok(())
}

fn patch_assets(root: &mut Value, key: &str, staged: &[StagedAsset]) {
    let items: Vec<Value> = staged
        .iter()
        .map(|a| Value::Object(vec![("path".into(), Value::String(a.work.clone()))]))
        .collect();
    json_set(root, key, Value::Array(items));
}

fn patch_track_lyrics(
    draft: &Draft,
    root: &mut Value,
    works: &WorkTree,
    namer: &mut UniqueNamer,
) -> Result<()> {
    for (di, disc) in draft.media.iter().enumerate() {
        for (ti, track) in disc.tracks.iter().enumerate() {
            if track.lyrics.is_empty() {
                continue;
            }
            let mut items = Vec::with_capacity(track.lyrics.len());
            for lyric in &track.lyrics {
                let source =
                    draft::resolve_source(&draft.source_root, &lyric.path).ok_or_else(|| {
                        AuthorError::Io {
                            detail: format!("lyrics file not found: {}", lyric.path),
                        }
                    })?;
                let base = Path::new(&lyric.path)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| lyric.path.clone());
                let rel = namer.unique(format!("lyrics/{base}"));
                copy_into(works.path(), &rel, &source)?;
                items.push(Value::Object(vec![
                    ("path".into(), Value::String(rel)),
                    (
                        "lang".into(),
                        match &lyric.lang {
                            Some(l) => Value::String(l.clone()),
                            None => Value::Null,
                        },
                    ),
                ]));
            }
            let disc_v = json_array_mut(obj_member_mut(root, "media").ok_or_else(bad_draft)?)
                .ok_or_else(bad_draft)?
                .get_mut(di)
                .ok_or_else(bad_draft)?;
            let track_v = json_array_mut(obj_member_mut(disc_v, "tracks").ok_or_else(bad_draft)?)
                .ok_or_else(bad_draft)?
                .get_mut(ti)
                .ok_or_else(bad_draft)?;
            json_set(track_v, "lyrics", Value::Array(items));
        }
    }
    Ok(())
}

fn patch_analysis(root: &mut Value, index: usize, path: &str) -> Result<()> {
    let entry = json_array_mut(obj_member_mut(root, "analysis").ok_or_else(bad_draft)?)
        .ok_or_else(bad_draft)?
        .get_mut(index)
        .ok_or_else(bad_draft)?;
    json_set(entry, "path", Value::String(path.to_string()));
    Ok(())
}

fn bad_draft() -> AuthorError {
    AuthorError::Draft {
        detail: "draft structure changed during staging".into(),
    }
}

/// Runs identification and applies the result to the draft.
fn apply_identification(draft: &mut Draft, request: &IdentifyRequest<'_>) -> Result<Confidence> {
    match request {
        IdentifyRequest::Document { doc, asserted_mbid } => {
            let (confidence, _applied) = identify::identify_apply(draft, doc, *asserted_mbid)?;
            Ok(confidence)
        }
        IdentifyRequest::ReleaseId { provider, mbid } => {
            let (confidence, _applied) = identify::identify_mbid(*provider, draft, mbid)?;
            Ok(confidence)
        }
    }
}

// ---------------------------------------------------------------------
// staging helpers
// ---------------------------------------------------------------------

/// A work tree removed on drop.
struct WorkTree {
    path: PathBuf,
    armed: bool,
}

impl WorkTree {
    fn create(output: &Path) -> Result<Self> {
        let parent = output.parent().filter(|p| !p.as_os_str().is_empty());
        if let Some(parent) = parent {
            fs::create_dir_all(parent).map_err(|e| AuthorError::Io {
                detail: format!("cannot create '{}': {e}", parent.display()),
            })?;
        }
        let name = format!(
            ".{}.author-{}",
            output
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "package".into()),
            std::process::id()
        );
        let path = output.with_file_name(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).map_err(|e| AuthorError::Io {
            detail: format!("cannot create work directory '{}': {e}", path.display()),
        })?;
        Ok(Self { path, armed: true })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Borrows an existing directory (created and owned by the caller)
    /// without removing it on drop — used by the split encode/waveform
    /// stages, whose staging directory is managed by the host.
    fn borrowed(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            armed: false,
        }
    }
}

impl Drop for WorkTree {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// Deduplicates basenames like the reference `unique_target` (`-2`, `-3`, …).
#[derive(Default)]
struct UniqueNamer {
    used: std::collections::HashSet<String>,
}

impl UniqueNamer {
    fn unique(&mut self, path: String) -> String {
        if self.used.insert(path.clone()) {
            return path;
        }
        let (stem, ext) = split_extension(&path);
        let mut n = 2;
        loop {
            let candidate = if ext.is_empty() {
                format!("{stem}-{n}")
            } else {
                format!("{stem}-{n}.{ext}")
            };
            if self.used.insert(candidate.clone()) {
                return candidate;
            }
            n += 1;
        }
    }
}

fn split_extension(path: &str) -> (String, String) {
    match path.rfind('.') {
        Some(i) if i > path.rfind('/').map(|s| s + 1).unwrap_or(0) => {
            (path[..i].to_string(), path[i + 1..].to_string())
        }
        _ => (path.to_string(), String::new()),
    }
}

fn create_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AuthorError::Io {
            detail: format!("cannot create '{}': {e}", parent.display()),
        })?;
    }
    Ok(())
}

fn extension_of(path: &str) -> String {
    Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn sanitize_component(s: &str) -> String {
    s.replace(['/', '\\', ':'], "-")
}

fn audio_rel_path(multi: bool, disc: i32, track: i32, title: &str, ext: &str) -> String {
    let name = if multi {
        format!("{disc}-{:02} - {}{}", track, sanitize_component(title), ext)
    } else {
        format!("{:02} - {}{}", track, sanitize_component(title), ext)
    };
    format!("audio/{name}")
}

/// Encodes (or copies) one track into the work tree.
fn stage_audio(
    draft: &Draft,
    disc: i32,
    track: &JsonTrack,
    multi: bool,
    works: &WorkTree,
    namer: &mut UniqueNamer,
    quality: f32,
) -> Result<StagedAudio> {
    let source = draft::resolve_source(&draft.source_root, &track.audio_path).ok_or_else(|| {
        AuthorError::Encode {
            disc,
            track: track.number,
            detail: format!("source audio not found: {}", track.audio_path),
        }
    })?;
    let ext = extension_of(&track.audio_path);
    if ext == "mpc" {
        // Already-encoded sources pass through (the Author's "inspect an
        // MPC album" workflow); the extension is preserved.
        let rel = namer.unique(audio_rel_path(
            multi,
            disc,
            track.number,
            &track.title,
            ".mpc",
        ));
        let dest = works.path().join(&rel);
        create_parent(&dest)?;
        fs::copy(&source, &dest).map_err(|e| AuthorError::Encode {
            disc,
            track: track.number,
            detail: format!("cannot copy '{}': {e}", track.audio_path),
        })?;
        return Ok(StagedAudio {
            package: rel.clone(),
            work: rel,
        });
    }
    if !encode::is_encodable_extension(&track.audio_path) {
        return Err(AuthorError::Unsupported {
            detail: format!(
                "only FLAC and WAV sources are supported (got '{}')",
                track.audio_path
            ),
        });
    }
    let rel = namer.unique(audio_rel_path(
        multi,
        disc,
        track.number,
        &track.title,
        ".mpc",
    ));
    let dest = works.path().join(&rel);
    create_parent(&dest)?;
    encode::encode_to(&source, &dest, quality).map_err(|e| match e {
        AuthorError::Unsupported { detail } => AuthorError::Unsupported {
            detail: format!("disc {disc} track {}: {detail}", track.number),
        },
        other => other,
    })?;
    Ok(StagedAudio {
        package: rel.clone(),
        work: rel,
    })
}

/// Stages artwork: file-based entries are copied as before; embedded
/// entries are extracted from their `sourceAudio` file (FLAC `PICTURE`
/// blocks / the APEv2 front cover) through [`crate::artwork`], with the
/// original image bytes preserved and the extension taken from the
/// JPEG/PNG signature. Extraction is fail-closed — a draft entry that
/// promises embedded artwork must yield signature-valid bytes of the
/// entry's role at build time (the reference's `extract_embedded_image`
/// contract); the downstream builder receives plain files and never
/// learns where they came from.
fn stage_artwork(
    draft: &Draft,
    works: &WorkTree,
    namer: &mut UniqueNamer,
) -> Result<Vec<StagedArtwork>> {
    let mut out = Vec::new();
    for entry in &draft.artwork {
        let (rel, source) = if let Some(path) = &entry.path {
            let source =
                draft::resolve_source(&draft.source_root, path).ok_or_else(|| AuthorError::Io {
                    detail: format!("artwork file not found: {path}"),
                })?;
            let ext = extension_of(path);
            let name = if ext.is_empty() {
                entry.role.clone()
            } else {
                format!("{}.{ext}", entry.role)
            };
            (namer.unique(format!("artwork/{name}")), source)
        } else if entry.embedded || entry.source_audio.is_some() {
            let src = entry
                .source_audio
                .as_deref()
                .ok_or_else(|| AuthorError::Artwork {
                    detail: format!(
                        "embedded artwork entry for role '{}' has no sourceAudio",
                        entry.role
                    ),
                })?;
            let source =
                draft::resolve_source(&draft.source_root, src).ok_or_else(|| AuthorError::Io {
                    detail: format!("embedded artwork source not found: {src}"),
                })?;
            let image =
                artwork::extract_role(&source, &entry.role).map_err(|e| AuthorError::Artwork {
                    detail: format!(
                        "cannot extract embedded '{}' artwork from '{}': {e}",
                        entry.role, src
                    ),
                })?;
            let Some(image) = image else {
                return Err(AuthorError::Artwork {
                    detail: format!("no usable embedded '{}' artwork in '{}'", entry.role, src),
                });
            };
            let rel = namer.unique(format!("artwork/{}.{}", entry.role, image.format.ext()));
            let dest = works.path().join(&rel);
            create_parent(&dest)?;
            fs::write(&dest, &image.bytes).map_err(|e| AuthorError::Io {
                detail: format!("cannot write '{}': {e}", dest.display()),
            })?;
            out.push(StagedArtwork {
                role: entry.role.clone(),
                package: rel.clone(),
                work: rel,
            });
            continue;
        } else {
            return Err(AuthorError::Artwork {
                detail: format!(
                    "artwork entry for role '{}' has neither a path nor an embedded source",
                    entry.role
                ),
            });
        };
        copy_into(works.path(), &rel, &source)?;
        out.push(StagedArtwork {
            role: entry.role.clone(),
            package: rel.clone(),
            work: rel,
        });
    }
    Ok(out)
}

/// Stages a list of `{dir}/<basename>` document assets.
fn stage_plain(
    draft: &Draft,
    list: &[String],
    dir: &str,
    works: &WorkTree,
    namer: &mut UniqueNamer,
) -> Result<Vec<StagedAsset>> {
    let mut out = Vec::new();
    for path in list {
        let source =
            draft::resolve_source(&draft.source_root, path).ok_or_else(|| AuthorError::Io {
                detail: format!("{dir} file not found: {path}"),
            })?;
        let base = Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        let rel = namer.unique(format!("{dir}/{base}"));
        copy_into(works.path(), &rel, &source)?;
        out.push(StagedAsset {
            package: rel.clone(),
            work: rel,
        });
    }
    Ok(out)
}

/// Stages analysis-document references opaquely (sonic and future kinds),
/// preserving their canonical package paths.
fn stage_analysis(
    draft: &Draft,
    works: &WorkTree,
    namer: &mut UniqueNamer,
) -> Result<Vec<StagedAnalysis>> {
    let mut out = Vec::new();
    for a in &draft.analysis {
        let source =
            draft::resolve_source(&draft.source_root, &a.path).ok_or_else(|| AuthorError::Io {
                detail: format!("analysis file not found: {}", a.path),
            })?;
        let rel = namer.unique(a.path.clone());
        copy_into(works.path(), &rel, &source)?;
        out.push(StagedAnalysis {
            kind: a.kind.clone(),
            profile: a.profile.clone(),
            package: rel.clone(),
            work: rel,
        });
    }
    Ok(out)
}

fn copy_into(work: &Path, rel: &str, source: &Path) -> Result<()> {
    let dest = work.join(rel);
    create_parent(&dest)?;
    fs::copy(source, &dest).map_err(|e| AuthorError::Io {
        detail: format!("cannot copy '{}': {e}", source.display()),
    })?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn assemble(
    draft: &Draft,
    audio: &[Vec<StagedAudio>],
    waveforms: &[Vec<Option<StagedWaveform>>],
    artwork: &[StagedArtwork],
    booklet: &[StagedAsset],
    root_lyrics: &[StagedAsset],
    extras: &[StagedAsset],
    analysis: &[StagedAnalysis],
    works: &WorkTree,
    namer: &mut UniqueNamer,
) -> Result<AuthoringDraft> {
    let mut media: Vec<DraftDisc> = Vec::with_capacity(draft.media.len());
    for (di, disc) in draft.media.iter().enumerate() {
        let mut tracks: Vec<DraftTrack> = Vec::with_capacity(disc.tracks.len());
        for (ti, track) in disc.tracks.iter().enumerate() {
            let staged = &audio[di][ti];
            let waveform = waveforms[di][ti].as_ref().map(|w| DraftWaveform {
                path: w.package.clone(),
                source: w.work.clone(),
            });
            let mut lyrics: Vec<DraftLyrics> = Vec::new();
            for lyric in &track.lyrics {
                let source =
                    draft::resolve_source(&draft.source_root, &lyric.path).ok_or_else(|| {
                        AuthorError::Io {
                            detail: format!("lyrics file not found: {}", lyric.path),
                        }
                    })?;
                let base = Path::new(&lyric.path)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| lyric.path.clone());
                let rel = namer.unique(format!("lyrics/{base}"));
                copy_into(works.path(), &rel, &source)?;
                lyrics.push(DraftLyrics {
                    path: rel.clone(),
                    source: rel,
                    lang: lyric.lang.clone(),
                });
            }
            let mut representations: Vec<DraftRepresentation> = Vec::new();
            for representation in &track.representations {
                let source = draft::resolve_source(&draft.source_root, &representation.path)
                    .ok_or_else(|| AuthorError::Io {
                        detail: format!("representation file not found: {}", representation.path),
                    })?;
                let rel = representation.path.clone();
                copy_into(works.path(), &rel, &source)?;
                representations.push(DraftRepresentation {
                    path: rel.clone(),
                    source: rel,
                    label: representation.label.clone(),
                    codec: representation.codec.clone(),
                });
            }
            tracks.push(DraftTrack {
                number: track.number,
                title: track.title.clone(),
                artists: track.artists.clone(),
                identifiers: track.identifiers.clone(),
                source: track.source.clone(),
                source_audio: track.source_audio.clone(),
                audio_codec: Some("musepack-sv8".into()),
                audio: DraftAsset {
                    path: staged.package.clone(),
                    source: staged.work.clone(),
                },
                waveform,
                lyrics,
                representations,
            });
        }
        media.push(DraftDisc {
            number: disc.number,
            format: disc.format,
            title: disc.title.clone(),
            tracks,
        });
    }

    let artwork = artwork
        .iter()
        .map(|a| DraftArtwork {
            role: a.role.clone(),
            asset: DraftAsset {
                path: a.package.clone(),
                source: a.work.clone(),
            },
        })
        .collect();

    Ok(AuthoringDraft {
        album: draft.album.clone(),
        release: draft.release.clone(),
        identifiers: draft.identifiers.clone(),
        identity: draft.identity.clone(),
        source: draft.source.clone(),
        media,
        artwork,
        booklet: booklet
            .iter()
            .map(|a| DraftAsset {
                path: a.package.clone(),
                source: a.work.clone(),
            })
            .collect(),
        lyrics: root_lyrics
            .iter()
            .map(|a| DraftAsset {
                path: a.package.clone(),
                source: a.work.clone(),
            })
            .collect(),
        extras: extras
            .iter()
            .map(|a| DraftAsset {
                path: a.package.clone(),
                source: a.work.clone(),
            })
            .collect(),
        analysis: analysis
            .iter()
            .map(|a| DraftAnalysis {
                kind: a.kind.clone(),
                profile: a.profile.clone(),
                asset: DraftAsset {
                    path: a.package.clone(),
                    source: a.work.clone(),
                },
            })
            .collect(),
        provenance: None,
    })
}

/// Publishes a freshly built staging directory at `final_output`.
fn publish(staging: &Path, final_output: &Path, replace: bool) -> Result<()> {
    if !final_output.exists() {
        fs::rename(staging, final_output).map_err(|e| AuthorError::Io {
            detail: format!("cannot publish '{}': {e}", final_output.display()),
        })?;
        return Ok(());
    }
    if !replace {
        return Err(AuthorError::Io {
            detail: format!(
                "output destination '{}' already exists",
                final_output.display()
            ),
        });
    }
    let backup = final_output.with_file_name(format!(
        ".{}.old-{}",
        file_name(final_output)?,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&backup);
    fs::rename(final_output, &backup).map_err(|e| AuthorError::Io {
        detail: format!(
            "cannot begin replacement of '{}': {e}",
            final_output.display()
        ),
    })?;
    match fs::rename(staging, final_output) {
        Ok(()) => {
            let _ = fs::remove_dir_all(&backup);
            Ok(())
        }
        Err(e) => {
            // Roll back: restore the previous package.
            let _ = fs::rename(&backup, final_output);
            Err(AuthorError::Io {
                detail: format!("cannot install '{}': {e}", final_output.display()),
            })
        }
    }
}

fn file_name(path: &Path) -> Result<String> {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .ok_or_else(|| AuthorError::Io {
            detail: format!("'{}' has no file name", path.display()),
        })
}

fn _io(detail: impl Into<String>) -> AuthorError {
    AuthorError::Io {
        detail: detail.into(),
    }
}

/// Convenience: write `bytes` to `path` (used by tests and the CLI).
pub fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| _io(e.to_string()))?;
    }
    let mut f = fs::File::create(path).map_err(|e| _io(e.to_string()))?;
    f.write_all(bytes).map_err(|e| _io(e.to_string()))?;
    Ok(())
}
