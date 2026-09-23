//! The authoring **draft JSON** surface and its validation.
//!
//! The schema mirrors the legacy C draft (`schema: "musicpack-draft"`,
//! `version: 1`) so the same documents flow through either implementation,
//! with the R3.5 per-track `lyrics[]` extension already used by the Author.
//!
//! # Authored vs derived
//!
//! Only authored input is represented here. The draft may *carry* derived
//! artifacts produced by an earlier legacy stage (a `waveformAnalysis`
//! block, per-track `duration`/`sampleRate` display hints, `sonicAnalysis`),
//! but the Rust pipeline never trusts them: it re-encodes, re-generates
//! waveforms, re-measures loudness/duration, and derives every hash,
//! identity key and verification result. This mirrors the brief's rule that
//! callers must not supply hashes, fingerprints, keys, measurements, points
//! or verification state — supplying them has no effect on the output.
//!
//! Source paths (`audioPath`, artwork/booklet/lyrics/extras paths) are
//! relative to [`Draft::source_root`] and are validated as package-relative
//! paths, exactly like the reference `draft_validate`.

use std::path::{Path, PathBuf};

use musicpack_core::format::manifest::{
    Album, Artist, Identifiers, Identity, IdentityConfidence, IdentitySource, MediumFormat,
    Release, ReleaseType, Source, SourceAudio, TrackIdentifiers, TrackSource,
};
use musicpack_core::format::path;
use musicpack_core::json::Value;

use crate::error::{AuthorError, Result};

// ---------------------------------------------------------------------
// model
// ---------------------------------------------------------------------

/// The parsed authoring draft.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    /// Source root every relative path is anchored to.
    pub source_root: PathBuf,
    /// Release-group metadata.
    pub album: Album,
    /// Optional release/edition metadata.
    pub release: Option<Release>,
    /// Optional release-level identifiers.
    pub identifiers: Option<Identifiers>,
    /// Optional identity match metadata.
    pub identity: Option<Identity>,
    /// Optional audio source metadata.
    pub source: Option<Source>,
    /// Discs/tracks.
    pub media: Vec<DraftDisc>,
    /// Artwork entries (file-based or embedded).
    pub artwork: Vec<DraftArtworkEntry>,
    /// Booklet source paths.
    pub booklet: Vec<String>,
    /// Root-level lyrics source paths.
    pub lyrics: Vec<String>,
    /// Extras source paths.
    pub extras: Vec<String>,
    /// Analysis-document references preserved opaquely (kind/profile/path).
    pub analysis: Vec<DraftAnalysisRef>,
}

/// An analysis-document reference as authored in the draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftAnalysisRef {
    /// Analysis kind (`"sonic"` is v1).
    pub kind: String,
    /// Profile id (required for sonic).
    pub profile: Option<String>,
    /// Source path (relative to the source root).
    pub path: String,
}

/// A draft disc.
#[derive(Debug, Clone, PartialEq)]
pub struct DraftDisc {
    /// Disc number (≥ 1).
    pub number: i32,
    /// Optional medium format.
    pub format: Option<MediumFormat>,
    /// Optional medium title.
    pub title: Option<String>,
    /// Tracks.
    pub tracks: Vec<DraftTrack>,
}

/// A draft track.
#[derive(Debug, Clone, PartialEq)]
pub struct DraftTrack {
    /// Track number (≥ 1, unique within the disc).
    pub number: i32,
    /// Title.
    pub title: String,
    /// Optional per-track credits.
    pub artists: Vec<Artist>,
    /// Optional per-track identifiers.
    pub identifiers: Option<TrackIdentifiers>,
    /// Optional per-track source.
    pub source: Option<TrackSource>,
    /// Optional pre-encoding source reference.
    pub source_audio: Option<SourceAudio>,
    /// Source audio path (relative to the source root).
    pub audio_path: String,
    /// Optional encoded-codec hint (`"musepack-sv8"` after a legacy encode).
    pub codec: Option<String>,
    /// Optional alternate representations.
    pub representations: Vec<DraftRepresentation>,
    /// Optional per-track lyric references (R3.5 extension).
    pub lyrics: Vec<DraftLyricRef>,
}

/// A draft representation reference.
#[derive(Debug, Clone, PartialEq)]
pub struct DraftRepresentation {
    /// Source path (relative to the source root).
    pub path: String,
    /// Optional display label.
    pub label: Option<String>,
    /// Optional codec hint.
    pub codec: Option<String>,
}

/// A draft per-track lyric reference.
#[derive(Debug, Clone, PartialEq)]
pub struct DraftLyricRef {
    /// Source `.lrc` path (relative to the source root).
    pub path: String,
    /// Optional language tag.
    pub lang: Option<String>,
}

/// A draft artwork entry: either a file path or an embedded picture source.
#[derive(Debug, Clone, PartialEq)]
pub struct DraftArtworkEntry {
    /// Required role.
    pub role: String,
    /// File-based artwork source path.
    pub path: Option<String>,
    /// `true` when the entry is embedded (`embedded: "true"`).
    pub embedded: bool,
    /// The audio file to extract the embedded picture from.
    pub source_audio: Option<String>,
    /// Optional embedded MIME hint.
    pub mime: Option<String>,
}

// ---------------------------------------------------------------------
// JSON helpers
// ---------------------------------------------------------------------

fn draft_err(detail: impl Into<String>) -> AuthorError {
    AuthorError::Draft {
        detail: detail.into(),
    }
}

fn as_object<'a>(v: &'a Value, what: &str) -> Result<&'a [(String, Value)]> {
    v.as_object()
        .ok_or_else(|| draft_err(format!("\"{what}\" must be an object")))
}

fn as_array<'a>(v: &'a Value, what: &str) -> Result<&'a Vec<Value>> {
    match v {
        Value::Array(a) => Ok(a),
        _ => Err(draft_err(format!("\"{what}\" must be an array"))),
    }
}

fn get<'a>(obj: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    obj.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn opt_string(obj: &[(String, Value)], key: &str) -> Result<Option<String>> {
    match get(obj, key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(draft_err(format!("\"{key}\" must be a string"))),
    }
}

fn req_string(obj: &[(String, Value)], key: &str) -> Result<String> {
    match get(obj, key) {
        Some(Value::String(s)) if !s.is_empty() => Ok(s.clone()),
        _ => Err(draft_err(format!(
            "\"{key}\" is required and must be a non-empty string"
        ))),
    }
}

fn opt_int(obj: &[(String, Value)], key: &str) -> Result<Option<i32>> {
    match get(obj, key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(v)) if v.is_finite() && v.fract() == 0.0 => Ok(Some(*v as i32)),
        Some(_) => Err(draft_err(format!("\"{key}\" must be an integer"))),
    }
}

fn opt_boolish(obj: &[(String, Value)], key: &str) -> Result<bool> {
    match get(obj, key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(Value::String(s)) => Ok(s == "true" || s == "1"),
        Some(_) => Err(draft_err(format!("\"{key}\" must be a boolean"))),
    }
}

fn string_array(obj: &[(String, Value)], key: &str) -> Result<Vec<String>> {
    match get(obj, key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::String(s) => out.push(s.clone()),
                    _ => return Err(draft_err(format!("\"{key}\" entries must be strings"))),
                }
            }
            Ok(out)
        }
        Some(_) => Err(draft_err(format!("\"{key}\" must be an array"))),
    }
}

/// Reads an asset array (`booklet`/`lyrics`/`extras`). The reference and the
/// Author UI emit objects (`[{"path": "notes.pdf"}]`, `draft.c`'s
/// `parse_asset_array`); a bare string is also accepted for convenience.
fn asset_paths(obj: &[(String, Value)], key: &str) -> Result<Vec<String>> {
    match get(obj, key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::String(s) => out.push(s.clone()),
                    Value::Object(members) => match get(members, "path") {
                        Some(Value::String(s)) => out.push(s.clone()),
                        _ => {
                            return Err(draft_err(format!(
                                "\"{key}\" entries must be objects with a string \"path\""
                            )));
                        }
                    },
                    _ => {
                        return Err(draft_err(format!(
                            "\"{key}\" entries must be objects with a \"path\""
                        )));
                    }
                }
            }
            Ok(out)
        }
        Some(_) => Err(draft_err(format!("\"{key}\" must be an array"))),
    }
}

fn artists(obj: &[(String, Value)], key: &str) -> Result<Vec<Artist>> {
    let Some(v) = get(obj, key) else {
        return Ok(Vec::new());
    };
    let items = as_array(v, key)?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let a = as_object(item, "artist")?;
        out.push(Artist {
            name: req_string(a, "name")?,
            role: opt_string(a, "role")?,
            musicbrainz_id: opt_string(a, "musicbrainzId")?,
            sort_name: opt_string(a, "sortName")?,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------
// parsing
// ---------------------------------------------------------------------

/// Parses a draft JSON document.
pub fn parse(bytes: &[u8]) -> Result<Draft> {
    let value = musicpack_core::json::parse(bytes)
        .map_err(|e| draft_err(format!("malformed draft JSON: {e}")))?;
    from_value(&value)
}

/// Maps a parsed JSON value to a [`Draft`].
pub fn from_value(root: &Value) -> Result<Draft> {
    let obj = as_object(root, "draft")?;

    let source_root_text = opt_string(obj, "sourceRoot")?.unwrap_or_default();
    let source_root = resolve_root(&source_root_text);

    // album
    let album_obj = as_object(
        get(obj, "album").ok_or_else(|| draft_err("\"album\" is required"))?,
        "album",
    )?;
    let album = Album {
        title: opt_string(album_obj, "title")?.unwrap_or_default(),
        artists: artists(album_obj, "artists")?,
        release_type: parse_release_type(opt_string(album_obj, "releaseType")?.as_deref())?,
        original_release_date: opt_string(album_obj, "originalReleaseDate")?,
        genres: string_array(album_obj, "genres")?,
    };

    let release = match get(obj, "release") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let o = as_object(v, "release")?;
            let r = Release {
                release_date: opt_string(o, "releaseDate")?,
                edition: opt_string(o, "edition")?,
                country: opt_string(o, "country")?,
                label: opt_string(o, "label")?,
                catalogue_number: opt_string(o, "catalogueNumber")?,
                notes: opt_string(o, "notes")?,
            };
            if r.is_present() { Some(r) } else { None }
        }
    };

    let identifiers = match get(obj, "identifiers") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let o = as_object(v, "identifiers")?;
            let i = Identifiers {
                musicbrainz_release_group_id: opt_string(o, "musicbrainzReleaseGroupId")?,
                musicbrainz_release_id: opt_string(o, "musicbrainzReleaseId")?,
                barcode: opt_string(o, "barcode")?,
            };
            if i.is_present() { Some(i) } else { None }
        }
    };

    let identity = match get(obj, "identity") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let o = as_object(v, "identity")?;
            let source = match opt_string(o, "source")? {
                None => None,
                Some(s) => Some(
                    IdentitySource::parse(&s)
                        .ok_or_else(|| draft_err(format!("unknown identity source \"{s}\"")))?,
                ),
            };
            let confidence = match opt_string(o, "confidence")? {
                None => None,
                Some(s) => Some(
                    IdentityConfidence::parse(&s)
                        .ok_or_else(|| draft_err(format!("unknown identity confidence \"{s}\"")))?,
                ),
            };
            let i = Identity { source, confidence };
            if i.is_present() { Some(i) } else { None }
        }
    };

    let source = match get(obj, "source") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let o = as_object(v, "source")?;
            let s = Source {
                kind: opt_string(o, "type")?,
                store: opt_string(o, "store")?,
                id: opt_string(o, "sourceId")?,
            };
            if s.is_present() { Some(s) } else { None }
        }
    };

    let media = match get(obj, "media") {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => {
            let items = as_array(v, "media")?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(parse_disc(item)?);
            }
            out
        }
    };

    let artwork = match get(obj, "artwork") {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => {
            let items = as_array(v, "artwork")?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(parse_artwork(item)?);
            }
            out
        }
    };

    let analysis = match get(obj, "analysis") {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => {
            let items = as_array(v, "analysis")?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let a = as_object(item, "analysis entry")?;
                out.push(DraftAnalysisRef {
                    kind: req_string(a, "kind")?,
                    profile: opt_string(a, "profile")?,
                    path: req_string(a, "path")?,
                });
            }
            out
        }
    };

    Ok(Draft {
        source_root,
        album,
        release,
        identifiers,
        identity,
        source,
        media,
        artwork,
        booklet: asset_paths(obj, "booklet")?,
        lyrics: asset_paths(obj, "lyrics")?,
        extras: asset_paths(obj, "extras")?,
        analysis,
    })
}

fn parse_disc(v: &Value) -> Result<DraftDisc> {
    let o = as_object(v, "media entry")?;
    let number = opt_int(o, "disc")?.unwrap_or(0);
    let format = match opt_string(o, "format")?.as_deref() {
        None => None,
        Some(s) => Some(
            MediumFormat::parse(s)
                .ok_or_else(|| draft_err(format!("unknown medium format \"{s}\"")))?,
        ),
    };
    let tracks = match get(o, "tracks") {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => {
            let items = as_array(v, "tracks")?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(parse_track(item)?);
            }
            out
        }
    };
    Ok(DraftDisc {
        number,
        format,
        title: opt_string(o, "title")?,
        tracks,
    })
}

fn parse_track(v: &Value) -> Result<DraftTrack> {
    let o = as_object(v, "track")?;
    let identifiers = match get(o, "identifiers") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let io = as_object(v, "track identifiers")?;
            let i = TrackIdentifiers {
                isrc: opt_string(io, "isrc")?,
                musicbrainz_track_id: opt_string(io, "musicbrainzTrackId")?,
                musicbrainz_recording_id: opt_string(io, "musicbrainzRecordingId")?,
            };
            if i.is_present() { Some(i) } else { None }
        }
    };
    let source = match get(o, "source") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let so = as_object(v, "track source")?;
            let s = TrackSource {
                store: opt_string(so, "store")?,
                track_id: opt_string(so, "trackId")?,
            };
            if s.is_present() { Some(s) } else { None }
        }
    };
    let source_audio = match get(o, "sourceAudio") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let sa = as_object(v, "track sourceAudio")?;
            let s = SourceAudio {
                codec: opt_string(sa, "codec")?,
                md5: opt_string(sa, "md5")?,
            };
            if s.is_present() { Some(s) } else { None }
        }
    };
    let representations = match get(o, "representations") {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => {
            let items = as_array(v, "representations")?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let r = as_object(item, "representation")?;
                out.push(DraftRepresentation {
                    path: opt_string(r, "path")?.unwrap_or_default(),
                    label: opt_string(r, "label")?,
                    codec: opt_string(r, "codec")?,
                });
            }
            out
        }
    };
    let lyrics = match get(o, "lyrics") {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => {
            let items = as_array(v, "lyrics")?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let l = as_object(item, "lyrics entry")?;
                out.push(DraftLyricRef {
                    path: opt_string(l, "path")?.unwrap_or_default(),
                    lang: opt_string(l, "lang")?,
                });
            }
            out
        }
    };
    Ok(DraftTrack {
        number: opt_int(o, "track")?.unwrap_or(0),
        title: opt_string(o, "title")?.unwrap_or_default(),
        artists: artists(o, "artists")?,
        identifiers,
        source,
        source_audio,
        audio_path: opt_string(o, "audioPath")?.unwrap_or_default(),
        codec: opt_string(o, "codec")?,
        representations,
        lyrics,
    })
}

fn parse_artwork(v: &Value) -> Result<DraftArtworkEntry> {
    let o = as_object(v, "artwork entry")?;
    Ok(DraftArtworkEntry {
        role: opt_string(o, "role")?.unwrap_or_default(),
        path: opt_string(o, "path")?,
        embedded: opt_boolish(o, "embedded")?,
        source_audio: opt_string(o, "sourceAudio")?,
        mime: opt_string(o, "mime")?,
    })
}

fn parse_release_type(s: Option<&str>) -> Result<Option<ReleaseType>> {
    match s {
        None => Ok(None),
        Some(s) => ReleaseType::parse(s)
            .map(Some)
            .ok_or_else(|| draft_err(format!("unknown release type \"{s}\""))),
    }
}

/// Resolves a draft `sourceRoot` (relative roots are anchored at the
/// process working directory, like the reference CLI).
fn resolve_root(text: &str) -> PathBuf {
    let p = Path::new(text);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    }
}

// ---------------------------------------------------------------------
// validation
// ---------------------------------------------------------------------

/// The `validate-draft` verdict.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationReport {
    /// Fatal problems.
    pub errors: Vec<String>,
    /// Non-fatal notes.
    pub warnings: Vec<String>,
}

impl ValidationReport {
    /// `true` when there are no errors.
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Validates a parsed draft (port of the reference `draft_validate`
/// structural rules). Source-file existence is checked; the authoritative
/// gate remains the package builder, which re-validates the assembled
/// manifest.
pub fn validate(draft: &Draft) -> ValidationReport {
    let mut report = ValidationReport::default();

    if draft.source_root.as_os_str().is_empty() {
        report.errors.push("missing source root".into());
    }
    if draft.album.title.is_empty() {
        report.errors.push("missing required album title".into());
    }
    if draft.album.artists.is_empty() {
        report.errors.push("no artist".into());
    }
    if draft.media.is_empty() {
        report.errors.push("no media".into());
    }

    let mut referenced: Vec<String> = Vec::new();
    for disc in &draft.media {
        if disc.number < 1 {
            report
                .errors
                .push(format!("invalid disc number {}", disc.number));
        }
        if disc.tracks.is_empty() {
            report
                .errors
                .push(format!("disc {} has no tracks", disc.number));
        }
        for track in &disc.tracks {
            if track.number < 1 {
                report
                    .errors
                    .push(format!("invalid track number on disc {}", disc.number));
            }
            if track.title.is_empty() {
                report.errors.push(format!(
                    "track {} on disc {} has no title",
                    track.number, disc.number
                ));
            }
            if track.audio_path.is_empty() {
                report.errors.push(format!(
                    "track {} on disc {} has no audio file",
                    track.number, disc.number
                ));
                continue;
            }
            if let Err(e) = path::validate(&track.audio_path) {
                report
                    .errors
                    .push(format!("invalid path '{}': {e}", track.audio_path));
                continue;
            }
            referenced.push(track.audio_path.clone());
            if !source_exists(draft, &track.audio_path) {
                report
                    .errors
                    .push(format!("audio file not found: {}", track.audio_path));
            }
            // Encodability hints (warnings only; the encode stage probes).
            let ext = extension(&track.audio_path);
            if ext != "mpc" && ext != "flac" && ext != "wav" {
                report.warnings.push(format!(
                    "track {} on disc {} ({}) cannot be encoded to Musepack; only FLAC and WAV sources are supported",
                    track.number, disc.number, ext
                ));
            }
            for lyric in &track.lyrics {
                if let Err(e) = path::validate(&lyric.path) {
                    report
                        .errors
                        .push(format!("invalid lyrics path '{}': {e}", lyric.path));
                }
            }
            for representation in &track.representations {
                if let Err(e) = path::validate(&representation.path) {
                    report.errors.push(format!(
                        "invalid representation path '{}': {e}",
                        representation.path
                    ));
                }
            }
        }
    }

    for entry in &draft.artwork {
        if let Some(p) = &entry.path {
            if let Err(e) = path::validate(p) {
                report
                    .errors
                    .push(format!("invalid artwork path '{p}': {e}"));
            } else if !source_exists(draft, p) {
                report.errors.push(format!("artwork file not found: {p}"));
            }
        } else if entry.embedded || entry.source_audio.is_some() {
            match &entry.source_audio {
                None => report.errors.push(format!(
                    "embedded artwork for role '{}' has no sourceAudio",
                    entry.role
                )),
                Some(src) if !source_exists(draft, src) => report
                    .errors
                    .push(format!("embedded artwork source not found: {src}")),
                Some(_) => {}
            }
        } else {
            report
                .errors
                .push(format!("invalid artwork source for role '{}'", entry.role));
        }
    }

    for (what, list) in [
        ("booklet", &draft.booklet),
        ("lyrics", &draft.lyrics),
        ("extras", &draft.extras),
    ] {
        for p in list {
            if let Err(e) = path::validate(p) {
                report
                    .errors
                    .push(format!("invalid {what} path '{p}': {e}"));
            } else if !source_exists(draft, p) {
                report.errors.push(format!("{what} file not found: {p}"));
            }
        }
    }

    for analysis in &draft.analysis {
        if let Err(e) = path::validate(&analysis.path) {
            report
                .errors
                .push(format!("invalid analysis path '{}': {e}", analysis.path));
        } else if !source_exists(draft, &analysis.path) {
            report
                .errors
                .push(format!("analysis file not found: {}", analysis.path));
        }
    }

    if draft.artwork.is_empty() {
        report.warnings.push("no artwork".into());
    }
    if draft
        .release
        .as_ref()
        .and_then(|r| r.catalogue_number.as_ref())
        .is_none()
    {
        report.warnings.push("missing catalogue number".into());
    }
    let exact = draft
        .identity
        .as_ref()
        .and_then(|i| i.confidence)
        .map(|c| c == IdentityConfidence::Exact)
        .unwrap_or(false);
    if draft
        .identifiers
        .as_ref()
        .and_then(|i| i.musicbrainz_release_id.as_ref())
        .is_none()
        && !exact
    {
        report.warnings.push("missing release identity".into());
    }

    report
}

fn source_exists(draft: &Draft, rel: &str) -> bool {
    resolve_source(&draft.source_root, rel).is_some_and(|p| p.is_file())
}

/// Resolves a draft-relative path under the source root with containment
/// (`..`/absolute rejected). Returns `None` when the path is unsafe or the
/// source root itself is unusable.
pub fn resolve_source(root: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || Path::new(rel).is_absolute() {
        return None;
    }
    if path::validate(rel).is_err() {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let joined = root.join(rel);
    let canonical = joined.canonicalize().ok()?;
    if !canonical.starts_with(&root) {
        return None;
    }
    Some(canonical)
}

fn extension(path: &str) -> String {
    Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// The draft's file extension for a track's source audio, lowercased.
pub fn audio_extension(audio_path: &str) -> String {
    extension(audio_path)
}

// ---------------------------------------------------------------------
// in-place JSON draft surgery (stage rewriting)
// ---------------------------------------------------------------------

/// Sets an object member in place, appending it when absent.
pub(crate) fn json_set(object: &mut Value, key: &str, value: Value) {
    if let Value::Object(members) = object {
        if let Some(entry) = members.iter_mut().find(|(k, _)| k == key) {
            entry.1 = value;
        } else {
            members.push((key.to_string(), value));
        }
    }
}

/// Removes an object member (no-op when absent / not an object).
pub(crate) fn json_remove(object: &mut Value, key: &str) {
    if let Value::Object(members) = object {
        members.retain(|(k, _)| k != key);
    }
}

/// Mutable access to an object's member list.
pub(crate) fn json_object_mut(value: &mut Value) -> Option<&mut Vec<(String, Value)>> {
    match value {
        Value::Object(members) => Some(members),
        _ => None,
    }
}

/// Mutable access to an array's items.
pub(crate) fn json_array_mut(value: &mut Value) -> Option<&mut Vec<Value>> {
    match value {
        Value::Array(items) => Some(items),
        _ => None,
    }
}
