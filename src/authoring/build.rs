//! The package-directory builder: an [`AuthoringDraft`] plus source files
//! become a verified `.mpack` directory.
//!
//! ```text
//! AuthoringDraft ── validate ──► resolve sources ──► staging dir
//!                                                       │
//!                        copy + hash every asset ◄──────┘
//!                                 │
//!                        measure loudness/duration (optional)
//!                                 │
//!                        canonical manifest.json
//!                                 │
//!                        core verification ──► atomic rename ──► .mpack
//! ```
//!
//! # What the builder owns
//!
//! - **Path safety and uniqueness.** Every package-relative path is
//!   validated by [`crate::format::path`] and checked for package-wide
//!   uniqueness *before* anything is written — the same rule the manifest
//!   parser enforces.
//! - **Derivation.** Hashes, waveform point counts, duration, loudness and
//!   the manifest bytes (canonical writer) all come from the bytes being
//!   packaged, never from the caller.
//! - **Determinism.** Output is a pure function of the draft and the source
//!   bytes: no timestamps, no random ids, no enumeration order.
//! - **Atomicity.** Construction happens in a sibling staging directory and
//!   is published with a single rename; a failure removes the staging tree,
//!   so a partially built `.mpack` is never observable at the destination.
//! - **Verification.** The finished directory is checked by the existing
//!   [`crate::validation::verify`] through the directory backend; a package
//!   that would not verify is never published.
//!
//! # Bounded scope (R4.1)
//!
//! The builder does **not** encode audio, synthesize waveform envelopes,
//! match MusicBrainz, or speak the authoring JSON protocol — those belong to
//! the R4.2 authoring pipeline. MusicPack noise here is deliberately the
//! package contract only.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::audio::loudness::STANDARD as LOUDNESS_STANDARD;
use crate::audio::{self, LoudnessMeter};
use crate::error::Error;
use crate::format::checksum;
use crate::format::manifest::{
    AlbumLoudness, Analysis, Artwork, Asset, Disc, LyricsRef, Manifest, Representation, Track,
    WaveformRef,
};
use crate::format::path;
use crate::format::waveform::MAX_PAYLOAD_BYTES;
use crate::limits::{
    MAX_ANALYSIS, MAX_ARTWORK, MAX_BOOKLET, MAX_DISCS, MAX_EXTRAS, MAX_LYRICS,
    MAX_REFERENCED_ASSETS, MAX_TRACKS_PER_DISC,
};
use crate::storage::directory::{self, MANIFEST_NAME};
use crate::validation::{Report, verify};

use super::draft::{AuthoringDraft, DraftAsset, DraftLyrics, DraftRepresentation, DraftWaveform};

/// Whether the builder measures loudness from the primary audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoudnessMode {
    /// Measure per-track loudness, duration and the concatenated-program
    /// album loudness (the reference `build-draft` default).
    Measure,
    /// Leave loudness and duration absent (the reference `--no-loudness`).
    Omit,
}

/// Builder options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildOptions {
    /// Loudness/duration handling.
    pub loudness: LoudnessMode,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            loudness: LoudnessMode::Measure,
        }
    }
}

impl BuildOptions {
    /// Options that skip loudness measurement (no audio decode).
    pub fn omitting_loudness() -> Self {
        Self {
            loudness: LoudnessMode::Omit,
        }
    }
}

/// Everything the caller needs about the package that was built.
#[derive(Debug, Clone)]
pub struct BuildOutcome {
    /// The manifest written into the package.
    pub manifest: Manifest,
    /// SHA-256 of the canonical `manifest.json` bytes.
    pub manifest_sha256: String,
    /// [`crate::identity::package_fingerprint`] of the built manifest.
    pub fingerprint: String,
    /// [`crate::identity::group_key`] of the built manifest.
    pub group_key: String,
    /// [`crate::identity::release_key`] of the built manifest.
    pub release_key: String,
    /// The core verification report (always `is_ok()` on success).
    pub report: Report,
    /// The published package directory.
    pub output: PathBuf,
}

fn invalid(detail: impl Into<String>) -> Error {
    Error::Invalid {
        detail: detail.into(),
    }
}

/// Builds a `.mpack` directory from `draft`, reading source files relative
/// to `source_root` and publishing the verified package at `output`.
///
/// The destination must not already exist; on any failure the staging tree
/// is removed and `output` is left untouched.
pub fn build_directory(
    draft: &AuthoringDraft,
    source_root: &Path,
    output: &Path,
    options: &BuildOptions,
) -> Result<BuildOutcome, Error> {
    validate_draft(draft)?;

    if fs::symlink_metadata(output).is_ok() {
        return Err(invalid(format!(
            "output destination '{}' already exists",
            output.display()
        )));
    }
    let root = source_root.canonicalize().map_err(|e| Error::Io {
        detail: format!(
            "cannot resolve source root '{}': {e}",
            source_root.display()
        ),
    })?;

    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|e| Error::Io {
            detail: format!("cannot create '{}': {e}", parent.display()),
        })?;
    }

    let staging = staging_path(output);
    if fs::symlink_metadata(&staging).is_ok() {
        return Err(invalid(format!(
            "staging directory '{}' already exists",
            staging.display()
        )));
    }
    fs::create_dir(&staging).map_err(|e| Error::Io {
        detail: format!("cannot create staging '{}': {e}", staging.display()),
    })?;
    let mut guard = StagingGuard::new(staging.clone());

    // Materialize every asset, deriving hashes and sizes.
    let mut hashes: HashMap<String, (String, u64)> = HashMap::new();
    for disc in &draft.media {
        for track in &disc.tracks {
            materialize(&staging, &root, &track.audio, &mut hashes)?;
            if let Some(waveform) = &track.waveform {
                materialize(&staging, &root, &waveform_asset(waveform), &mut hashes)?;
            }
            for lyrics in &track.lyrics {
                materialize(&staging, &root, &lyrics_asset(lyrics), &mut hashes)?;
            }
            for representation in &track.representations {
                materialize(
                    &staging,
                    &root,
                    &representation_asset(representation),
                    &mut hashes,
                )?;
            }
        }
    }
    for artwork in &draft.artwork {
        materialize(&staging, &root, &artwork.asset, &mut hashes)?;
    }
    for asset in draft
        .booklet
        .iter()
        .chain(&draft.lyrics)
        .chain(&draft.extras)
    {
        materialize(&staging, &root, asset, &mut hashes)?;
    }
    for analysis in &draft.analysis {
        materialize(&staging, &root, &analysis.asset, &mut hashes)?;
    }

    // Optional loudness/duration measurement, over the staged audio.
    let (measurements, album_loudness) = match options.loudness {
        LoudnessMode::Measure => measure(&staging, draft)?,
        LoudnessMode::Omit => (HashMap::new(), None),
    };

    // Assemble, validate and write the canonical manifest.
    let manifest = assemble(draft, &hashes, &measurements, album_loudness)?;
    let canonical = manifest.write_canonical()?;
    fs::write(staging.join(MANIFEST_NAME), canonical.as_bytes()).map_err(|e| Error::Io {
        detail: format!("cannot write '{MANIFEST_NAME}': {e}"),
    })?;

    // The existing verifier is authoritative for final validity.
    let report = verify_staged(&staging)?;
    if !report.is_ok() {
        let mut findings: Vec<String> = report
            .findings()
            .iter()
            .map(|f| f.message.clone())
            .collect();
        findings.truncate(5);
        return Err(invalid(format!(
            "built package failed verification ({} error(s)): {}",
            report.errors(),
            findings.join("; ")
        )));
    }

    // Publish atomically, then disarm the cleanup guard.
    fs::rename(&staging, output).map_err(|e| Error::Io {
        detail: format!("cannot publish '{}': {e}", output.display()),
    })?;
    guard.disarm();

    let manifest_sha256 = checksum::sha256_hex(canonical.as_bytes());
    Ok(BuildOutcome {
        fingerprint: crate::identity::package_fingerprint(&manifest)?,
        group_key: crate::identity::group_key(&manifest),
        release_key: crate::identity::release_key(&manifest),
        manifest,
        manifest_sha256,
        report,
        output: output.to_path_buf(),
    })
}

// ---------------------------------------------------------------------
// staging lifecycle
// ---------------------------------------------------------------------

/// Removes the staging tree on drop unless the build published it.
struct StagingGuard {
    path: PathBuf,
    armed: bool,
}

impl StagingGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// The sibling staging directory used before the atomic rename.
fn staging_path(output: &Path) -> PathBuf {
    let mut name = std::ffi::OsString::from(".");
    name.push(output.file_name().unwrap_or_else(|| "package".as_ref()));
    name.push(format!(".build-{}", std::process::id()));
    output.with_file_name(name)
}

// ---------------------------------------------------------------------
// validation
// ---------------------------------------------------------------------

/// Pre-flight validation: everything that can be decided without touching
/// the filesystem or the source files.
fn validate_draft(draft: &AuthoringDraft) -> Result<(), Error> {
    if draft.album.title.is_empty() {
        return Err(invalid("album title must not be empty"));
    }
    if draft.album.artists.is_empty() {
        return Err(invalid("album artists must not be empty"));
    }
    if draft.media.is_empty() {
        return Err(invalid("media must be a non-empty array"));
    }
    if draft.media.len() > MAX_DISCS {
        return Err(invalid(format!(
            "media has {} entries; exceeds the limit of {MAX_DISCS}",
            draft.media.len()
        )));
    }
    if draft.artwork.len() > MAX_ARTWORK {
        return Err(invalid(format!(
            "artwork has {} entries; exceeds the limit of {MAX_ARTWORK}",
            draft.artwork.len()
        )));
    }
    for (what, count, max) in [
        ("booklet", draft.booklet.len(), MAX_BOOKLET),
        ("lyrics", draft.lyrics.len(), MAX_LYRICS),
        ("extras", draft.extras.len(), MAX_EXTRAS),
        ("analysis", draft.analysis.len(), MAX_ANALYSIS),
    ] {
        if count > max {
            return Err(invalid(format!(
                "{what} has {count} entries; exceeds the limit of {max}"
            )));
        }
    }

    let mut paths: Vec<&str> = Vec::new();
    for (di, disc) in draft.media.iter().enumerate() {
        if disc.number < 1 {
            return Err(invalid(format!(
                "disc number {} must be an integer >= 1",
                disc.number
            )));
        }
        if draft.media[di + 1..]
            .iter()
            .any(|o| o.number == disc.number)
        {
            return Err(invalid(format!("duplicate disc number {}", disc.number)));
        }
        if disc.tracks.is_empty() {
            return Err(invalid(format!(
                "disc {} must have a non-empty track list",
                disc.number
            )));
        }
        if disc.tracks.len() > MAX_TRACKS_PER_DISC {
            return Err(invalid(format!(
                "disc {} has {} tracks; exceeds the limit of {MAX_TRACKS_PER_DISC}",
                disc.number,
                disc.tracks.len()
            )));
        }
        for (ti, track) in disc.tracks.iter().enumerate() {
            if track.number < 1 {
                return Err(invalid(format!(
                    "disc {} track number {} must be an integer >= 1",
                    disc.number, track.number
                )));
            }
            if track.title.is_empty() {
                return Err(invalid(format!(
                    "disc {} track {}: title must not be empty",
                    disc.number, track.number
                )));
            }
            if disc.tracks[ti + 1..]
                .iter()
                .any(|o| o.number == track.number)
            {
                return Err(invalid(format!(
                    "duplicate track number {} on disc {}",
                    track.number, disc.number
                )));
            }
            if track.lyrics.len() > MAX_LYRICS {
                return Err(invalid(format!(
                    "disc {} track {}: lyrics has {} entries; exceeds the limit of {MAX_LYRICS}",
                    disc.number,
                    track.number,
                    track.lyrics.len()
                )));
            }
            for lyrics in &track.lyrics {
                if let Some(lang) = &lyrics.lang
                    && (lang.is_empty() || lang.chars().any(char::is_control))
                {
                    return Err(invalid(format!(
                        "disc {} track {}: lyrics \"lang\" must be a non-empty string without control characters",
                        disc.number, track.number
                    )));
                }
            }
            paths.push(&track.audio.path);
            if let Some(waveform) = &track.waveform {
                paths.push(&waveform.path);
            }
            for lyrics in &track.lyrics {
                paths.push(&lyrics.path);
            }
            for representation in &track.representations {
                paths.push(&representation.path);
            }
        }
    }
    for artwork in &draft.artwork {
        paths.push(&artwork.asset.path);
    }
    for asset in draft
        .booklet
        .iter()
        .chain(&draft.lyrics)
        .chain(&draft.extras)
    {
        paths.push(&asset.path);
    }
    for analysis in &draft.analysis {
        paths.push(&analysis.asset.path);
    }

    if paths.len() > MAX_REFERENCED_ASSETS {
        return Err(invalid(format!(
            "referenced assets ({}); exceeds the limit of {MAX_REFERENCED_ASSETS}",
            paths.len()
        )));
    }
    let mut seen: HashSet<&str> = HashSet::with_capacity(paths.len());
    for p in &paths {
        path::validate(p)?;
        if !seen.insert(p) {
            return Err(invalid(format!("duplicate asset path \"{p}\"")));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------
// source resolution + materialization
// ---------------------------------------------------------------------

/// Resolves a draft source reference under the canonical source root,
/// rejecting absolute paths, `..` escapes and symlink escapes.
fn resolve_source(root: &Path, rel: &str) -> Result<PathBuf, Error> {
    if rel.is_empty() {
        return Err(invalid("source path must not be empty"));
    }
    let rel_path = Path::new(rel);
    // `has_root` (not just `is_absolute`): on Windows a path like
    // `/etc/hosts` is rooted but not "absolute", yet joining it would still
    // discard the source root — it must never be treated as relative.
    if rel_path.has_root() {
        return Err(invalid(format!(
            "source '{rel}' must be relative to the album directory"
        )));
    }
    let mut depth: i32 = 0;
    for component in rel_path.components() {
        match component {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err(invalid(format!(
                        "source '{rel}' must stay inside the album directory"
                    )));
                }
            }
            Component::Normal(_) => depth += 1,
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
    let joined = root.join(rel_path);
    let canonical = joined.canonicalize().map_err(|_| Error::Missing {
        path: rel.to_string(),
    })?;
    if !canonical.starts_with(root) {
        return Err(invalid(format!(
            "source '{rel}' must stay inside the album directory"
        )));
    }
    let meta = fs::metadata(&canonical).map_err(|_| Error::Missing {
        path: rel.to_string(),
    })?;
    if !meta.is_file() {
        return Err(invalid(format!("source '{rel}' is not a regular file")));
    }
    Ok(canonical)
}

/// Copies one asset into the staging tree and records its derived hash.
fn materialize(
    staging: &Path,
    root: &Path,
    asset: &DraftAsset,
    hashes: &mut HashMap<String, (String, u64)>,
) -> Result<(), Error> {
    let source = resolve_source(root, &asset.source)?;
    let dest = staging.join(&asset.path);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::Io {
            detail: format!("cannot create '{}': {e}", parent.display()),
        })?;
    }
    let (sha256, len) = copy_and_hash(&source, &dest)?;
    hashes.insert(asset.path.clone(), (sha256, len));
    Ok(())
}

fn copy_and_hash(source: &Path, dest: &Path) -> Result<(String, u64), Error> {
    let mut input = File::open(source).map_err(|e| Error::Io {
        detail: format!("cannot read '{}': {e}", source.display()),
    })?;
    let mut output = File::create(dest).map_err(|e| Error::Io {
        detail: format!("cannot write '{}': {e}", dest.display()),
    })?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = input.read(&mut buf).map_err(|e| Error::Io {
            detail: e.to_string(),
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        output.write_all(&buf[..n]).map_err(|e| Error::Io {
            detail: e.to_string(),
        })?;
        total = total
            .checked_add(n as u64)
            .ok_or_else(|| invalid("asset size overflow"))?;
    }
    let digest: [u8; 32] = hasher.finalize().into();
    Ok((checksum::to_hex(&digest), total))
}

fn waveform_asset(waveform: &DraftWaveform) -> DraftAsset {
    DraftAsset {
        path: waveform.path.clone(),
        source: waveform.source.clone(),
    }
}

fn lyrics_asset(lyrics: &DraftLyrics) -> DraftAsset {
    DraftAsset {
        path: lyrics.path.clone(),
        source: lyrics.source.clone(),
    }
}

fn representation_asset(representation: &DraftRepresentation) -> DraftAsset {
    DraftAsset {
        path: representation.path.clone(),
        source: representation.source.clone(),
    }
}

// ---------------------------------------------------------------------
// loudness + duration
// ---------------------------------------------------------------------

/// Per-track measured facts (loudness and duration).
struct Measurement {
    loudness: crate::format::manifest::Loudness,
    duration: f64,
}

/// Decodes every primary audio track in manifest order and measures it,
/// feeding one meter per track and a single album meter.
fn measure(
    staging: &Path,
    draft: &AuthoringDraft,
) -> Result<(HashMap<String, Measurement>, Option<AlbumLoudness>), Error> {
    let mut per_track: HashMap<String, Measurement> = HashMap::new();
    let mut album: Option<LoudnessMeter> = None;
    let mut album_rate: u32 = 0;
    let mut album_channels: u8 = 0;

    for disc in &draft.media {
        for track in &disc.tracks {
            let path = staging.join(&track.audio.path);
            let file = File::open(&path).map_err(|e| Error::Io {
                detail: format!("cannot read staged audio '{}': {e}", path.display()),
            })?;
            let mut decoder = audio::open(Box::new(file))?;
            let info = decoder.info().clone();
            let channels = info.channels;
            let rate = info.sample_rate;

            if channels == 0 || channels > 2 {
                return Err(invalid(format!(
                    "disc {} track {}: loudness metering supports 1-2 channels ({channels} found); build with loudness omitted",
                    disc.number, track.number
                )));
            }
            match &album {
                None => {
                    album = Some(LoudnessMeter::new(channels, rate)?);
                    album_channels = channels;
                    album_rate = rate;
                }
                Some(_) if channels != album_channels || rate != album_rate => {
                    return Err(invalid(format!(
                        "disc {} track {}: album loudness requires a uniform channel count and sample rate across tracks ({} ch / {} Hz vs {} ch / {} Hz)",
                        disc.number, track.number, channels, rate, album_channels, album_rate
                    )));
                }
                Some(_) => {}
            }
            let mut track_meter = LoudnessMeter::new(channels, rate)?;
            let album_meter = album.as_mut().expect("album meter created above");

            let mut buffer = vec![0f32; 8192 * channels as usize];
            let mut frames: u64 = 0;
            loop {
                let read = decoder.read_f32(&mut buffer)?;
                if read == 0 {
                    break;
                }
                let samples = read * channels as usize;
                track_meter.process(&buffer[..samples])?;
                album_meter.process(&buffer[..samples])?;
                frames += read as u64;
            }

            let measured = track_meter.result();
            if !crate::format::manifest::is_valid_loudness_value(measured.lufs)
                || !crate::format::manifest::is_valid_loudness_value(measured.true_peak_db_tp)
            {
                return Err(invalid(format!(
                    "disc {} track {}: measured loudness is outside the manifest range [-70, 6] (LUFS {}, dBTP {})",
                    disc.number, track.number, measured.lufs, measured.true_peak_db_tp
                )));
            }
            per_track.insert(
                track.audio.path.clone(),
                Measurement {
                    loudness: crate::format::manifest::Loudness {
                        lufs: measured.lufs,
                        true_peak_db: measured.true_peak_db_tp,
                    },
                    duration: frames as f64 / rate as f64,
                },
            );
        }
    }

    let album_loudness = album.map(|meter| {
        let measured = meter.result();
        AlbumLoudness {
            algorithm: Some(LOUDNESS_STANDARD.to_string()),
            lufs: measured.lufs,
            true_peak_db: measured.true_peak_db_tp,
        }
    });
    Ok((per_track, album_loudness))
}

// ---------------------------------------------------------------------
// manifest assembly
// ---------------------------------------------------------------------

fn asset(hashes: &HashMap<String, (String, u64)>, path: &str) -> Result<Asset, Error> {
    let (sha256, _) = hashes
        .get(path)
        .ok_or_else(|| invalid(format!("internal: asset '{path}' was not materialized")))?;
    Ok(Asset {
        path: path.to_string(),
        sha256: sha256.clone(),
    })
}

fn assemble(
    draft: &AuthoringDraft,
    hashes: &HashMap<String, (String, u64)>,
    measurements: &HashMap<String, Measurement>,
    album_loudness: Option<AlbumLoudness>,
) -> Result<Manifest, Error> {
    let mut media: Vec<Disc> = Vec::with_capacity(draft.media.len());
    for disc in &draft.media {
        let mut tracks: Vec<Track> = Vec::with_capacity(disc.tracks.len());
        for track in &disc.tracks {
            let measured = measurements.get(&track.audio.path);
            let audio = asset(hashes, &track.audio.path)?;
            let waveform = match &track.waveform {
                None => None,
                Some(waveform) => {
                    let (sha256, len) = hashes.get(&waveform.path).ok_or_else(|| {
                        invalid(format!(
                            "internal: waveform '{}' was not materialized",
                            waveform.path
                        ))
                    })?;
                    if len % 2 != 0 {
                        return Err(invalid(format!(
                            "waveform '{}' must have an even payload length ({len} bytes)",
                            waveform.path
                        )));
                    }
                    if *len > MAX_PAYLOAD_BYTES {
                        return Err(invalid(format!(
                            "waveform '{}' exceeds the {MAX_PAYLOAD_BYTES}-byte payload limit",
                            waveform.path
                        )));
                    }
                    Some(WaveformRef {
                        path: waveform.path.clone(),
                        sha256: sha256.clone(),
                        points: len / 2,
                    })
                }
            };
            let mut lyrics: Vec<LyricsRef> = Vec::with_capacity(track.lyrics.len());
            for lyric in &track.lyrics {
                let (sha256, _) = hashes.get(&lyric.path).ok_or_else(|| {
                    invalid(format!(
                        "internal: lyrics '{}' was not materialized",
                        lyric.path
                    ))
                })?;
                lyrics.push(LyricsRef {
                    path: lyric.path.clone(),
                    sha256: sha256.clone(),
                    lang: lyric.lang.clone(),
                });
            }
            let mut representations: Vec<Representation> =
                Vec::with_capacity(track.representations.len());
            for representation in &track.representations {
                let (sha256, _) = hashes.get(&representation.path).ok_or_else(|| {
                    invalid(format!(
                        "internal: representation '{}' was not materialized",
                        representation.path
                    ))
                })?;
                representations.push(Representation {
                    path: representation.path.clone(),
                    sha256: sha256.clone(),
                    label: representation.label.clone(),
                    codec: representation.codec.clone(),
                });
            }
            tracks.push(Track {
                number: track.number,
                title: track.title.clone(),
                artists: track.artists.clone(),
                identifiers: track.identifiers.clone(),
                source: track.source.clone(),
                source_audio: track.source_audio.clone(),
                duration: measured.map(|m| m.duration),
                loudness: measured.map(|m| m.loudness),
                audio,
                audio_codec: track.audio_codec.clone(),
                waveform,
                lyrics,
                representations,
            });
        }
        media.push(Disc {
            number: disc.number,
            format: disc.format,
            title: disc.title.clone(),
            tracks,
        });
    }

    let mut artwork: Vec<Artwork> = Vec::with_capacity(draft.artwork.len());
    for entry in &draft.artwork {
        artwork.push(Artwork {
            role: entry.role.clone(),
            asset: asset(hashes, &entry.asset.path)?,
        });
    }
    let mut booklet: Vec<Asset> = Vec::with_capacity(draft.booklet.len());
    for entry in &draft.booklet {
        booklet.push(asset(hashes, &entry.path)?);
    }
    let mut lyrics: Vec<Asset> = Vec::with_capacity(draft.lyrics.len());
    for entry in &draft.lyrics {
        lyrics.push(asset(hashes, &entry.path)?);
    }
    let mut extras: Vec<Asset> = Vec::with_capacity(draft.extras.len());
    for entry in &draft.extras {
        extras.push(asset(hashes, &entry.path)?);
    }
    let mut analysis: Vec<Analysis> = Vec::with_capacity(draft.analysis.len());
    for entry in &draft.analysis {
        analysis.push(Analysis {
            kind: entry.kind.clone(),
            profile: entry.profile.clone(),
            asset: asset(hashes, &entry.asset.path)?,
        });
    }

    Ok(Manifest {
        album: draft.album.clone(),
        release: draft.release.clone(),
        identifiers: draft.identifiers.clone(),
        identity: draft.identity.clone(),
        source: draft.source.clone(),
        media,
        artwork,
        booklet,
        lyrics,
        extras,
        analysis,
        loudness: album_loudness,
        provenance: draft.provenance.clone(),
    })
}

// ---------------------------------------------------------------------
// verification of the staged package
// ---------------------------------------------------------------------

fn verify_staged(staging: &Path) -> Result<Report, Error> {
    let backend = directory::DirectoryBackend::open(staging)?;
    let parsed = crate::format::manifest::ParsedManifest::parse(backend.manifest_bytes())?;
    Ok(verify(parsed.manifest(), &backend))
}
