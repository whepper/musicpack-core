//! Strict manifest parsing — port of `musicpack_manifest_parse` /
//! `musicpack_manifest_parse_tree` (`core/libmusicpack/src/manifest.c`).
//!
//! Pipeline (order matters — it determines which error a doubly-bad
//! manifest produces, which the conformance corpus pins):
//!
//! 1. length bound (16 MiB) — `Invalid`;
//! 2. JSON parse (cJSON-compatible dialect, `require_null_terminated`) —
//!    `Json`;
//! 3. root must be an object, duplicate keys rejected at any depth —
//!    `Invalid`;
//! 4. `format` literal, then `version` (a non-1 *number* is `Version`; a
//!    non-number version is `Invalid`);
//! 5. semantic parse of every section in the C's order;
//! 6. path uniqueness + the 4096 referenced-asset budget.
//!
//! Every budget violation and structural rejection is `Invalid`, matching
//! the C status codes.

use crate::error::Error;
use crate::format::checksum::is_valid_sha256_hex;
use crate::format::manifest::{
    Album, AlbumLoudness, Analysis, Artwork, Asset, Disc, FORMAT_ID, Identifiers, Identity,
    IdentityConfidence, IdentitySource, Loudness, Manifest, MediumFormat, Provenance, Release,
    ReleaseType, Representation, SCHEMA_VERSION, Source, SourceAudio, Track, TrackIdentifiers,
    TrackSource, WaveformRef,
};
use crate::format::path;
use crate::format::waveform::{
    ENCODING, FLOOR_DB, INTERVAL_MS, MAX_POINTS as WAVEFORM_MAX_POINTS, VERSION as WAVEFORM_VERSION,
};
use crate::json::{self, Value};
use crate::limits;

use super::{Artist, is_valid_loudness_value};

/// Maximum manifest input size (16 MiB), enforced before parsing.
const MANIFEST_MAX_BYTES: usize = 16 * 1024 * 1024;

/// A parsed manifest together with the original parse tree.
///
/// The original tree is retained because the canonical writer must
/// re-emit unknown *root-level* fields (preservation rule,
/// `musicpack-v1.md` §7): `write_canonical` copies every root member whose
/// key is not one of the fifteen known fields, in original order, after
/// the known fields. Unknown fields nested inside known objects are **not**
/// preserved (the C reference has the same limitation and documents it).
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedManifest {
    pub(super) manifest: Manifest,
    pub(super) original: Value,
}

impl ParsedManifest {
    /// Parses manifest bytes (the exact contents of `manifest.json`).
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MANIFEST_MAX_BYTES {
            return Err(Error::Invalid {
                detail: format!("manifest size exceeds {} bytes", MANIFEST_MAX_BYTES),
            });
        }
        let root = json::parse(bytes)?;
        let manifest = manifest_from_tree(&root)?;
        Ok(Self {
            manifest,
            original: root,
        })
    }

    /// The typed manifest model.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Mutable access to the model (unknown root fields are preserved on
    /// write regardless of model edits).
    pub fn manifest_mut(&mut self) -> &mut Manifest {
        &mut self.manifest
    }

    /// Consumes the parsed manifest, returning the model without the
    /// original tree (unknown root fields will no longer be preserved on
    /// write).
    pub fn into_manifest(self) -> Manifest {
        self.manifest
    }

    /// The original root object (including unknown fields).
    pub fn original(&self) -> &Value {
        &self.original
    }
}

/// Parses the typed model from a JSON tree, mirroring
/// `musicpack_manifest_parse_tree`.
///
/// Public within the crate: the writer uses it as its post-build
/// validation pass (the C `validate_for_write` round-trip).
pub(crate) fn manifest_from_tree(root: &Value) -> Result<Manifest, Error> {
    if !root.is_object() || root.has_duplicate_keys() {
        return Err(Error::Invalid {
            detail: "manifest is not a JSON object or has duplicate keys".into(),
        });
    }

    // format + version
    let format_value = root.get("format");
    match format_value {
        Some(Value::String(s)) if s == FORMAT_ID => {}
        _ => {
            return Err(Error::Invalid {
                detail: format!("format must be \"{FORMAT_ID}\""),
            });
        }
    }
    match root.get("version") {
        Some(Value::Number(v)) if v.is_finite() && *v == SCHEMA_VERSION as f64 => {}
        Some(Value::Number(v)) if v.is_finite() => {
            return Err(Error::Version {
                found: crate::format::number::format_json_number(*v),
                supported: SCHEMA_VERSION,
            });
        }
        _ => {
            return Err(Error::Invalid {
                detail: "version must be the number 1".into(),
            });
        }
    }

    // album
    let album_value = require_object(root.get("album"), "album")?;
    let title = require_string(album_value, "title")?;
    let artists = parse_artists(album_value.get("artists"))?;
    let release_type = match album_value.get("releaseType") {
        None => None,
        Some(v) => {
            let s = opt_string_value(v, "releaseType")?;
            match ReleaseType::parse(&s) {
                Some(t) => Some(t),
                None => {
                    return Err(Error::Invalid {
                        detail: format!("unknown releaseType \"{s}\""),
                    });
                }
            }
        }
    };
    let original_release_date = opt_string(album_value, "originalReleaseDate")?;
    let genres = parse_string_list(album_value.get("genres"), "genres", limits::MAX_GENRES)?;

    let album = Album {
        title,
        artists,
        release_type,
        original_release_date,
        genres,
    };

    // release
    let release = match root.get("release") {
        None => None,
        Some(v) => {
            let o = require_object(Some(v), "release")?;
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

    // identifiers
    let identifiers = match root.get("identifiers") {
        None => None,
        Some(v) => {
            let o = require_object(Some(v), "identifiers")?;
            let i = Identifiers {
                musicbrainz_release_group_id: opt_string(o, "musicbrainzReleaseGroupId")?,
                musicbrainz_release_id: opt_string(o, "musicbrainzReleaseId")?,
                barcode: opt_string(o, "barcode")?,
            };
            if i.is_present() { Some(i) } else { None }
        }
    };

    // identity
    let identity = match root.get("identity") {
        None => None,
        Some(v) => {
            let o = require_object(Some(v), "identity")?;
            let source = match o.get("source") {
                None => None,
                Some(s) => {
                    let text = opt_string_value(s, "identity.source")?;
                    match IdentitySource::parse(&text) {
                        Some(v) => Some(v),
                        None => {
                            return Err(Error::Invalid {
                                detail: format!("unknown identity source \"{text}\""),
                            });
                        }
                    }
                }
            };
            let confidence = match o.get("confidence") {
                None => None,
                Some(s) => {
                    let text = opt_string_value(s, "identity.confidence")?;
                    match IdentityConfidence::parse(&text) {
                        Some(v) => Some(v),
                        None => {
                            return Err(Error::Invalid {
                                detail: format!("unknown identity confidence \"{text}\""),
                            });
                        }
                    }
                }
            };
            let i = Identity { source, confidence };
            if i.is_present() { Some(i) } else { None }
        }
    };

    // source
    let source = match root.get("source") {
        None => None,
        Some(v) => {
            let o = require_object(Some(v), "source")?;
            let s = Source {
                kind: opt_string(o, "type")?,
                store: opt_string(o, "store")?,
                id: opt_string(o, "sourceId")?,
            };
            if s.is_present() { Some(s) } else { None }
        }
    };

    // media
    let media_value = root
        .get("media")
        .ok_or_else(|| invalid("media must be a non-empty array"))?;
    let media_array = require_array(Some(media_value), "media")?;
    if media_array.is_empty() {
        return Err(invalid("media must be a non-empty array"));
    }
    if media_array.len() > limits::MAX_DISCS {
        return Err(invalid(&format!(
            "media has {} entries; exceeds the limit of {}",
            media_array.len(),
            limits::MAX_DISCS
        )));
    }
    let mut media = Vec::with_capacity(media_array.len());
    for item in media_array {
        media.push(parse_disc(item)?);
    }

    // unique disc / track numbering
    for (di, disc) in media.iter().enumerate() {
        for (ti, track) in disc.tracks.iter().enumerate() {
            for other in &disc.tracks[ti + 1..] {
                if track.number == other.number {
                    return Err(invalid(&format!(
                        "duplicate track number {} on disc {}",
                        track.number, disc.number
                    )));
                }
            }
            let _ = track;
        }
        for other in &media[di + 1..] {
            if disc.number == other.number {
                return Err(invalid(&format!("duplicate disc number {}", disc.number)));
            }
        }
    }

    // artwork
    let artwork = match root.get("artwork") {
        None => Vec::new(),
        Some(v) => {
            let arr = require_array(Some(v), "artwork")?;
            if arr.len() > limits::MAX_ARTWORK {
                return Err(invalid(&format!(
                    "artwork has {} entries; exceeds the limit of {}",
                    arr.len(),
                    limits::MAX_ARTWORK
                )));
            }
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                let o = require_object(Some(item), "artwork entry")?;
                let role = require_string(o, "role")?;
                let asset = parse_asset(o)?;
                out.push(Artwork { role, asset });
            }
            out
        }
    };

    // booklet / lyrics / extras
    let booklet = parse_asset_array(root.get("booklet"), "booklet", limits::MAX_BOOKLET)?;
    let lyrics = parse_asset_array(root.get("lyrics"), "lyrics", limits::MAX_LYRICS)?;
    let extras = parse_asset_array(root.get("extras"), "extras", limits::MAX_EXTRAS)?;

    // analysis
    let analysis = match root.get("analysis") {
        None => Vec::new(),
        Some(v) => {
            let arr = require_array(Some(v), "analysis")?;
            if arr.len() > limits::MAX_ANALYSIS {
                return Err(invalid(&format!(
                    "analysis has {} entries; exceeds the limit of {}",
                    arr.len(),
                    limits::MAX_ANALYSIS
                )));
            }
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                let o = require_object(Some(item), "analysis entry")?;
                let kind = require_string(o, "type")?;
                let profile = opt_string(o, "profile")?;
                let path_text = require_string(o, "path")?;
                validate_path(&path_text)?;
                let sha256 = require_string(o, "sha256")?;
                if !is_valid_sha256_hex(&sha256) {
                    return Err(invalid(&format!(
                        "analysis sha256 for \"{path_text}\" is not 64 lowercase hex characters"
                    )));
                }
                if kind == "sonic" && profile.is_none() {
                    return Err(invalid("sonic analysis entries require a profile"));
                }
                out.push(Analysis {
                    kind,
                    profile,
                    asset: Asset {
                        path: path_text,
                        sha256,
                    },
                });
            }
            out
        }
    };

    // album loudness
    let loudness = match root.get("loudness") {
        None => None,
        Some(v) => {
            let o = require_object(Some(v), "loudness")?;
            let algorithm = opt_string(o, "algorithm")?;
            let lufs_present = o.get("albumLUFS");
            let peak_present = o.get("albumTruePeakDbTP");
            let (lufs, peak) = match (lufs_present, peak_present) {
                (Some(a), Some(b)) => (Some(a), Some(b)),
                (None, None) => (None, None),
                _ => {
                    return Err(invalid(
                        "album loudness requires both albumLUFS and albumTruePeakDbTP",
                    ));
                }
            };
            match (lufs, peak) {
                (None, None) => {
                    // Algorithm-only loudness objects parse but are not
                    // recorded (the C writer drops them).
                    None
                }
                (Some(Value::Number(a)), Some(Value::Number(b)))
                    if is_valid_loudness_value(*a) && is_valid_loudness_value(*b) =>
                {
                    Some(AlbumLoudness {
                        algorithm,
                        lufs: *a,
                        true_peak_db: *b,
                    })
                }
                _ => {
                    return Err(invalid(
                        "album loudness values must be finite numbers within [-70, 6]",
                    ));
                }
            }
        }
    };

    // provenance
    let provenance = match root.get("provenance") {
        None => None,
        Some(v) => {
            let o = require_object(Some(v), "provenance")?;
            let p = Provenance {
                tool: opt_string(o, "tool")?,
                tool_version: opt_string(o, "toolVersion")?,
            };
            if p.is_present() { Some(p) } else { None }
        }
    };

    let manifest = Manifest {
        album,
        release,
        identifiers,
        identity,
        source,
        media,
        artwork,
        booklet,
        lyrics,
        extras,
        analysis,
        loudness,
        provenance,
    };

    check_dup_paths(&manifest)?;

    Ok(manifest)
}

// ---------------------------------------------------------------------
// field helpers (ports of the C get_* helpers)
// ---------------------------------------------------------------------

fn invalid(detail: &str) -> Error {
    Error::Invalid {
        detail: detail.to_string(),
    }
}

fn require_object<'a>(v: Option<&'a Value>, what: &str) -> Result<&'a Value, Error> {
    match v {
        Some(v) if v.is_object() => Ok(v),
        _ => Err(invalid(&format!("\"{what}\" must be an object"))),
    }
}

fn require_array<'a>(v: Option<&'a Value>, what: &str) -> Result<&'a Vec<Value>, Error> {
    match v {
        Some(Value::Array(a)) => Ok(a),
        _ => Err(invalid(&format!("\"{what}\" must be an array"))),
    }
}

/// Required non-empty string (`get_req_string`).
fn require_string(obj: &Value, key: &str) -> Result<String, Error> {
    match obj.get(key) {
        Some(Value::String(s)) if !s.is_empty() => Ok(s.clone()),
        _ => Err(invalid(&format!(
            "\"{key}\" is required and must be a non-empty string"
        ))),
    }
}

/// Optional string (`get_opt_string`): present must be a string.
fn opt_string(obj: &Value, key: &str) -> Result<Option<String>, Error> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(invalid(&format!("\"{key}\" must be a string"))),
    }
}

/// Type-checked fetch used where the C reads an optional string from a
/// value it has already established exists (enum members).
fn opt_string_value(v: &Value, what: &str) -> Result<String, Error> {
    match v {
        Value::String(s) => Ok(s.clone()),
        _ => Err(invalid(&format!("\"{what}\" must be a string"))),
    }
}

/// Required positive integer (`get_req_int`): number, finite, integral,
/// 1..=i32::MAX.
fn require_int(obj: &Value, key: &str) -> Result<i32, Error> {
    match obj.get(key) {
        Some(Value::Number(v))
            if v.is_finite() && *v >= 1.0 && *v <= i32::MAX as f64 && v.fract() == 0.0 =>
        {
            Ok(*v as i32)
        }
        _ => Err(invalid(&format!(
            "\"{key}\" is required and must be an integer >= 1"
        ))),
    }
}

/// Optional double (`get_opt_double` without a validator).
fn opt_positive_double(obj: &Value, key: &str) -> Result<Option<f64>, Error> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::Number(v)) if v.is_finite() => {
            if *v <= 0.0 {
                return Err(invalid(&format!("\"{key}\" must be > 0")));
            }
            Ok(Some(*v))
        }
        Some(_) => Err(invalid(&format!("\"{key}\" must be a number"))),
    }
}

/// Required non-empty artist list (`parse_artists`): present arrays must be
/// non-empty and within the credit budget.
fn parse_artists(v: Option<&Value>) -> Result<Vec<Artist>, Error> {
    let arr = match v {
        None => return Ok(Vec::new()),
        Some(Value::Array(a)) => a,
        Some(_) => return Err(invalid("\"artists\" must be an array")),
    };
    if arr.is_empty() {
        return Err(invalid("\"artists\" must not be empty"));
    }
    if arr.len() > limits::MAX_ARTISTS_PER_CREDIT {
        return Err(invalid(&format!(
            "\"artists\" has {} entries; exceeds the limit of {}",
            arr.len(),
            limits::MAX_ARTISTS_PER_CREDIT
        )));
    }
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let name = require_string(item, "name")?;
        out.push(Artist {
            name,
            role: opt_string(item, "role")?,
            musicbrainz_id: opt_string(item, "musicbrainzId")?,
            sort_name: opt_string(item, "sortName")?,
        });
    }
    Ok(out)
}

/// Optional string-array (`genres`).
fn parse_string_list(v: Option<&Value>, what: &str, max: usize) -> Result<Vec<String>, Error> {
    let arr = match v {
        None => return Ok(Vec::new()),
        Some(Value::Array(a)) => a,
        Some(_) => return Err(invalid(&format!("\"{what}\" must be an array"))),
    };
    if arr.len() > max {
        return Err(invalid(&format!(
            "\"{what}\" has {} entries; exceeds the limit of {max}",
            arr.len()
        )));
    }
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        match item {
            Value::String(s) => out.push(s.clone()),
            _ => return Err(invalid(&format!("\"{what}\" entries must be strings"))),
        }
    }
    Ok(out)
}

/// Required asset object (`parse_asset`): path (validated) + sha256.
fn parse_asset(obj: &Value) -> Result<Asset, Error> {
    let path_text = require_string(obj, "path")?;
    validate_path(&path_text)?;
    let sha256 = require_string(obj, "sha256")?;
    if !is_valid_sha256_hex(&sha256) {
        return Err(invalid(&format!(
            "sha256 for \"{path_text}\" is not 64 lowercase hex characters"
        )));
    }
    Ok(Asset {
        path: path_text,
        sha256,
    })
}

fn validate_path(p: &str) -> Result<(), Error> {
    path::validate(p)?;
    Ok(())
}

/// Optional asset array (`PARSE_ASSET_ARRAY` macro).
fn parse_asset_array(v: Option<&Value>, what: &str, max: usize) -> Result<Vec<Asset>, Error> {
    let arr = match v {
        None => return Ok(Vec::new()),
        Some(Value::Array(a)) => a,
        Some(_) => return Err(invalid(&format!("\"{what}\" must be an array"))),
    };
    if arr.len() > max {
        return Err(invalid(&format!(
            "\"{what}\" has {} entries; exceeds the limit of {max}",
            arr.len()
        )));
    }
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let o = require_object(Some(item), what)?;
        out.push(parse_asset(o)?);
    }
    Ok(out)
}

fn parse_disc(item: &Value) -> Result<Disc, Error> {
    let o = require_object(Some(item), "media entry")?;
    let number = require_int(o, "disc")?;
    let format = match o.get("format") {
        None => None,
        Some(v) => {
            let s = opt_string_value(v, "media[].format")?;
            match MediumFormat::parse(&s) {
                Some(f) => Some(f),
                None => return Err(invalid(&format!("unknown medium format \"{s}\""))),
            }
        }
    };
    let title = opt_string(o, "title")?;

    let tracks_value = o
        .get("tracks")
        .ok_or_else(|| invalid("\"tracks\" must be a non-empty array"))?;
    let tracks_array = require_array(Some(tracks_value), "tracks")?;
    if tracks_array.is_empty() {
        return Err(invalid("\"tracks\" must be a non-empty array"));
    }
    if tracks_array.len() > limits::MAX_TRACKS_PER_DISC {
        return Err(invalid(&format!(
            "\"tracks\" has {} entries; exceeds the limit of {}",
            tracks_array.len(),
            limits::MAX_TRACKS_PER_DISC
        )));
    }
    let mut tracks = Vec::with_capacity(tracks_array.len());
    for track_item in tracks_array {
        tracks.push(parse_track(track_item)?);
    }

    Ok(Disc {
        number,
        format,
        title,
        tracks,
    })
}

fn parse_track(item: &Value) -> Result<Track, Error> {
    let o = require_object(Some(item), "track")?;
    let number = require_int(o, "track")?;
    let title = require_string(o, "title")?;
    let artists = parse_artists(o.get("artists"))?;

    let identifiers = match o.get("identifiers") {
        None => None,
        Some(v) => {
            let io = require_object(Some(v), "track identifiers")?;
            let i = TrackIdentifiers {
                isrc: opt_string(io, "isrc")?,
                musicbrainz_track_id: opt_string(io, "musicbrainzTrackId")?,
                musicbrainz_recording_id: opt_string(io, "musicbrainzRecordingId")?,
            };
            if i.is_present() { Some(i) } else { None }
        }
    };

    let source = match o.get("source") {
        None => None,
        Some(v) => {
            let so = require_object(Some(v), "track source")?;
            let s = TrackSource {
                store: opt_string(so, "store")?,
                track_id: opt_string(so, "trackId")?,
            };
            if s.is_present() { Some(s) } else { None }
        }
    };

    let source_audio = match o.get("sourceAudio") {
        None => None,
        Some(v) => {
            let sa = require_object(Some(v), "track sourceAudio")?;
            let s = SourceAudio {
                codec: opt_string(sa, "codec")?,
                md5: opt_string(sa, "md5")?,
            };
            if s.is_present() { Some(s) } else { None }
        }
    };

    let duration = opt_positive_double(o, "duration")?;

    let loudness = match o.get("loudness") {
        None => None,
        Some(v) => {
            let lo = require_object(Some(v), "track loudness")?;
            let lufs = lo.get("trackLUFS");
            let peak = lo.get("truePeakDbTP");
            match (lufs, peak) {
                (Some(Value::Number(a)), Some(Value::Number(b)))
                    if is_valid_loudness_value(*a) && is_valid_loudness_value(*b) =>
                {
                    Some(Loudness {
                        lufs: *a,
                        true_peak_db: *b,
                    })
                }
                _ => {
                    return Err(invalid(
                        "track loudness requires finite trackLUFS and truePeakDbTP within [-70, 6]",
                    ));
                }
            }
        }
    };

    let audio_value = o
        .get("audio")
        .ok_or_else(|| invalid("\"audio\" is required"))?;
    let audio = parse_asset(audio_value)?;
    let audio_codec = opt_string(audio_value, "codec")?;

    let waveform = match o.get("waveform") {
        None => None,
        Some(v) => {
            let w = require_object(Some(v), "waveform")?;
            let version = waveform_int(w, "version")?;
            let wpath = require_string(w, "path")?;
            validate_path(&wpath)?;
            let sha256 = require_string(w, "sha256")?;
            if !is_valid_sha256_hex(&sha256) {
                return Err(invalid(
                    "waveform sha256 is not 64 lowercase hex characters",
                ));
            }
            let interval = waveform_int(w, "intervalMs")?;
            let encoding = require_string(w, "encoding")?;
            if encoding != ENCODING {
                return Err(invalid(&format!(
                    "waveform encoding must be \"{ENCODING}\""
                )));
            }
            let floor = waveform_int(w, "floorDb")?;
            let points = match w.get("points") {
                Some(Value::Number(v))
                    if v.is_finite()
                        && v.fract() == 0.0
                        && *v >= 0.0
                        && *v <= WAVEFORM_MAX_POINTS as f64 =>
                {
                    *v as u64
                }
                _ => {
                    return Err(invalid(&format!(
                        "waveform points must be an integer within [0, {WAVEFORM_MAX_POINTS}]"
                    )));
                }
            };
            if version != WAVEFORM_VERSION as f64
                || interval != INTERVAL_MS as f64
                || floor != FLOOR_DB as f64
            {
                return Err(invalid(
                    "waveform closed enums violated (version=1, intervalMs=100, floorDb=-60)",
                ));
            }
            Some(WaveformRef {
                path: wpath,
                sha256,
                points,
            })
        }
    };

    let representations = match o.get("representations") {
        None => Vec::new(),
        Some(v) => {
            let arr = require_array(Some(v), "representations")?;
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                let ro = require_object(Some(item), "representation")?;
                let asset = parse_asset(ro)?;
                out.push(Representation {
                    path: asset.path,
                    sha256: asset.sha256,
                    label: opt_string(ro, "label")?,
                    codec: opt_string(ro, "codec")?,
                });
            }
            out
        }
    };

    Ok(Track {
        number,
        title,
        artists,
        identifiers,
        source,
        source_audio,
        duration,
        loudness,
        audio,
        audio_codec,
        waveform,
        representations,
    })
}

/// Waveform closed-enum fields: required finite integral numbers
/// (`parse_waveform` reads them with hard required-number checks).
fn waveform_int(w: &Value, key: &str) -> Result<f64, Error> {
    match w.get(key) {
        Some(Value::Number(v)) if v.is_finite() && v.fract() == 0.0 => Ok(*v),
        _ => Err(invalid(&format!(
            "waveform \"{key}\" is required and must be an integer"
        ))),
    }
}

/// Port of `check_dup_paths`: the total referenced-asset budget (≤ 4096)
/// and package-wide path uniqueness.
fn check_dup_paths(m: &Manifest) -> Result<(), Error> {
    let mut paths: Vec<&str> = Vec::new();
    for disc in &m.media {
        for track in &disc.tracks {
            paths.push(&track.audio.path);
            if let Some(w) = &track.waveform {
                paths.push(&w.path);
            }
            for r in &track.representations {
                paths.push(&r.path);
            }
        }
    }
    for a in &m.artwork {
        paths.push(&a.asset.path);
    }
    for a in &m.booklet {
        paths.push(&a.path);
    }
    for a in &m.lyrics {
        paths.push(&a.path);
    }
    for a in &m.extras {
        paths.push(&a.path);
    }
    for a in &m.analysis {
        paths.push(&a.asset.path);
    }

    if paths.len() > limits::MAX_REFERENCED_ASSETS {
        return Err(invalid(&format!(
            "referenced assets ({}); exceeds the limit of {}",
            paths.len(),
            limits::MAX_REFERENCED_ASSETS
        )));
    }

    let mut seen = std::collections::HashSet::with_capacity(paths.len());
    for p in paths {
        if !seen.insert(p) {
            return Err(invalid(&format!("duplicate asset path \"{p}\"")));
        }
    }
    Ok(())
}
