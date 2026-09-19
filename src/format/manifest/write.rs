//! Canonical manifest serialization — port of `build_tree` /
//! `musicpack_json_print` / `copy_package_extensions`
//! (`core/libmusicpack/src/manifest.c`).
//!
//! Compatibility-critical properties:
//!
//! - **Fixed key order** (the canonical order used by every tool in the
//!   ecosystem; credits: `musicbrainzId, name, role, sortName`).
//! - **Omission of absent optionals** — never `null`; a known object is
//!   emitted only when at least one of its fields is set.
//! - **Numbers** through the C-compatible printer: integral values within
//!   ±9.2e18 as plain integers, everything else `%.8g`.
//! - **2-space indentation**, `": "` separator, empty containers inline,
//!   one trailing newline.
//! - **Unknown root fields are appended last**, in original order, when
//!   writing through [`ParsedManifest::write_canonical`] (port of
//!   `copy_package_extensions`, which copies every root member whose key
//!   is not one of the fifteen known fields).
//! - A standalone [`Manifest::write_canonical`] (no original tree) drops
//!   unknown fields — the exact behaviour of the C
//!   `musicpack_manifest_write`.
//! - Before printing, the built tree is re-parsed through the same
//!   semantic parser (port of `validate_for_write`), so a corrupted or
//!   invalid edit cannot be serialized.
//!
//! Byte-identity with the reference writer is covered by
//! `tests/manifest_write.rs` (committed reference manifests) and the
//! numeric-format vector table.

use crate::error::Error;
use crate::format::manifest::{
    FORMAT_ID, Manifest, ParsedManifest, SCHEMA_VERSION, parse::manifest_from_tree,
};
use crate::format::number::format_json_number;
use crate::format::waveform::{ENCODING, FLOOR_DB, INTERVAL_MS, VERSION as WAVEFORM_VERSION};
use crate::json::{Value, print_canonical};

/// Root-level field names known to the manifest model (`is_package_field`).
const KNOWN_ROOT_FIELDS: [&str; 15] = [
    "format",
    "version",
    "album",
    "release",
    "identifiers",
    "identity",
    "source",
    "media",
    "artwork",
    "booklet",
    "lyrics",
    "extras",
    "analysis",
    "loudness",
    "provenance",
];

/// A JSON object under construction preserving insertion order.
type Obj = Vec<(String, Value)>;

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

fn n(v: f64) -> Value {
    Value::Number(v)
}

fn put(obj: &mut Obj, key: &str, value: Value) {
    obj.push((key.to_string(), value));
}

fn put_opt(obj: &mut Obj, key: &str, value: &Option<String>) {
    if let Some(v) = value {
        put(obj, key, s(v));
    }
}

fn artists_to_json(artists: &[crate::format::manifest::Artist]) -> Value {
    Value::Array(
        artists
            .iter()
            .map(|a| {
                let mut o: Obj = Vec::new();
                put_opt(&mut o, "musicbrainzId", &a.musicbrainz_id);
                put(&mut o, "name", s(&a.name));
                put_opt(&mut o, "role", &a.role);
                put_opt(&mut o, "sortName", &a.sort_name);
                Value::Object(o)
            })
            .collect(),
    )
}

fn asset_to_json(asset: &crate::format::manifest::Asset) -> Value {
    let mut o: Obj = Vec::new();
    put(&mut o, "path", s(&asset.path));
    put(&mut o, "sha256", s(&asset.sha256));
    Value::Object(o)
}

/// Port of `build_tree`: the known fields in canonical order.
pub(crate) fn build_tree(m: &Manifest) -> Value {
    let mut root: Obj = Vec::new();

    put(&mut root, "format", s(FORMAT_ID));
    put(&mut root, "version", n(SCHEMA_VERSION as f64));

    let mut album: Obj = Vec::new();
    put(&mut album, "title", s(&m.album.title));
    put(&mut album, "artists", artists_to_json(&m.album.artists));
    if let Some(rt) = &m.album.release_type {
        put(&mut album, "releaseType", s(rt.as_str()));
    }
    put_opt(
        &mut album,
        "originalReleaseDate",
        &m.album.original_release_date,
    );
    if !m.album.genres.is_empty() {
        put(
            &mut album,
            "genres",
            Value::Array(m.album.genres.iter().map(|g| s(g)).collect()),
        );
    }
    put(&mut root, "album", Value::Object(album));

    if let Some(release) = &m.release {
        let mut o: Obj = Vec::new();
        put_opt(&mut o, "releaseDate", &release.release_date);
        put_opt(&mut o, "edition", &release.edition);
        put_opt(&mut o, "country", &release.country);
        put_opt(&mut o, "label", &release.label);
        put_opt(&mut o, "catalogueNumber", &release.catalogue_number);
        put_opt(&mut o, "notes", &release.notes);
        put(&mut root, "release", Value::Object(o));
    }

    if let Some(ids) = &m.identifiers {
        let mut o: Obj = Vec::new();
        put_opt(
            &mut o,
            "musicbrainzReleaseGroupId",
            &ids.musicbrainz_release_group_id,
        );
        put_opt(&mut o, "musicbrainzReleaseId", &ids.musicbrainz_release_id);
        put_opt(&mut o, "barcode", &ids.barcode);
        put(&mut root, "identifiers", Value::Object(o));
    }

    if let Some(identity) = &m.identity {
        let mut o: Obj = Vec::new();
        if let Some(src) = &identity.source {
            put(&mut o, "source", s(src.as_str()));
        }
        if let Some(c) = &identity.confidence {
            put(&mut o, "confidence", s(c.as_str()));
        }
        put(&mut root, "identity", Value::Object(o));
    }

    if let Some(source) = &m.source {
        let mut o: Obj = Vec::new();
        put_opt(&mut o, "type", &source.kind);
        put_opt(&mut o, "store", &source.store);
        put_opt(&mut o, "sourceId", &source.id);
        put(&mut root, "source", Value::Object(o));
    }

    let media: Vec<Value> = m
        .media
        .iter()
        .map(|disc| {
            let mut d: Obj = Vec::new();
            put(&mut d, "disc", n(disc.number as f64));
            if let Some(f) = &disc.format {
                put(&mut d, "format", s(f.as_str()));
            }
            put_opt(&mut d, "title", &disc.title);
            let tracks: Vec<Value> = disc
                .tracks
                .iter()
                .map(|t| {
                    let mut to: Obj = Vec::new();
                    put(&mut to, "track", n(t.number as f64));
                    put(&mut to, "title", s(&t.title));
                    if !t.artists.is_empty() {
                        put(&mut to, "artists", artists_to_json(&t.artists));
                    }
                    if let Some(ids) = &t.identifiers {
                        let mut io: Obj = Vec::new();
                        put_opt(&mut io, "isrc", &ids.isrc);
                        put_opt(&mut io, "musicbrainzTrackId", &ids.musicbrainz_track_id);
                        put_opt(
                            &mut io,
                            "musicbrainzRecordingId",
                            &ids.musicbrainz_recording_id,
                        );
                        put(&mut to, "identifiers", Value::Object(io));
                    }
                    if let Some(src) = &t.source {
                        let mut so: Obj = Vec::new();
                        put_opt(&mut so, "store", &src.store);
                        put_opt(&mut so, "trackId", &src.track_id);
                        put(&mut to, "source", Value::Object(so));
                    }
                    if let Some(sa) = &t.source_audio {
                        let mut ao: Obj = Vec::new();
                        put_opt(&mut ao, "codec", &sa.codec);
                        put_opt(&mut ao, "md5", &sa.md5);
                        put(&mut to, "sourceAudio", Value::Object(ao));
                    }
                    if let Some(dur) = t.duration {
                        put(&mut to, "duration", n(dur));
                    }
                    if let Some(loudness) = &t.loudness {
                        let mut lo: Obj = Vec::new();
                        put(&mut lo, "trackLUFS", n(loudness.lufs));
                        put(&mut lo, "truePeakDbTP", n(loudness.true_peak_db));
                        put(&mut to, "loudness", Value::Object(lo));
                    }
                    let mut audio: Obj = Vec::new();
                    put(&mut audio, "path", s(&t.audio.path));
                    put(&mut audio, "sha256", s(&t.audio.sha256));
                    if let Some(codec) = &t.audio_codec {
                        put(&mut audio, "codec", s(codec));
                    }
                    put(&mut to, "audio", Value::Object(audio));
                    if let Some(wf) = &t.waveform {
                        let mut wo: Obj = Vec::new();
                        put(&mut wo, "version", n(WAVEFORM_VERSION as f64));
                        put(&mut wo, "path", s(&wf.path));
                        put(&mut wo, "sha256", s(&wf.sha256));
                        put(&mut wo, "intervalMs", n(INTERVAL_MS as f64));
                        put(&mut wo, "encoding", s(ENCODING));
                        put(&mut wo, "floorDb", n(FLOOR_DB as f64));
                        put(&mut wo, "points", n(wf.points as f64));
                        put(&mut to, "waveform", Value::Object(wo));
                    }
                    if !t.representations.is_empty() {
                        let reps: Vec<Value> = t
                            .representations
                            .iter()
                            .map(|r| {
                                let mut ro: Obj = Vec::new();
                                put(&mut ro, "path", s(&r.path));
                                put(&mut ro, "sha256", s(&r.sha256));
                                put_opt(&mut ro, "label", &r.label);
                                put_opt(&mut ro, "codec", &r.codec);
                                Value::Object(ro)
                            })
                            .collect();
                        put(&mut to, "representations", Value::Array(reps));
                    }
                    Value::Object(to)
                })
                .collect();
            put(&mut d, "tracks", Value::Array(tracks));
            Value::Object(d)
        })
        .collect();
    put(&mut root, "media", Value::Array(media));

    if !m.artwork.is_empty() {
        let artwork: Vec<Value> = m
            .artwork
            .iter()
            .map(|w| {
                let mut wo: Obj = Vec::new();
                put(&mut wo, "role", s(&w.role));
                put(&mut wo, "path", s(&w.asset.path));
                put(&mut wo, "sha256", s(&w.asset.sha256));
                Value::Object(wo)
            })
            .collect();
        put(&mut root, "artwork", Value::Array(artwork));
    }

    for (key, assets) in [
        ("booklet", &m.booklet),
        ("lyrics", &m.lyrics),
        ("extras", &m.extras),
    ] {
        if !assets.is_empty() {
            put(
                &mut root,
                key,
                Value::Array(assets.iter().map(asset_to_json).collect()),
            );
        }
    }

    if !m.analysis.is_empty() {
        let analysis: Vec<Value> = m
            .analysis
            .iter()
            .map(|a| {
                let mut ao: Obj = Vec::new();
                put(&mut ao, "type", s(&a.kind));
                put_opt(&mut ao, "profile", &a.profile);
                put(&mut ao, "path", s(&a.asset.path));
                put(&mut ao, "sha256", s(&a.asset.sha256));
                Value::Object(ao)
            })
            .collect();
        put(&mut root, "analysis", Value::Array(analysis));
    }

    if let Some(loudness) = &m.loudness {
        let mut lo: Obj = Vec::new();
        put_opt(&mut lo, "algorithm", &loudness.algorithm);
        put(&mut lo, "albumLUFS", n(loudness.lufs));
        put(&mut lo, "albumTruePeakDbTP", n(loudness.true_peak_db));
        put(&mut root, "loudness", Value::Object(lo));
    }

    if let Some(provenance) = &m.provenance {
        let mut po: Obj = Vec::new();
        put_opt(&mut po, "tool", &provenance.tool);
        put_opt(&mut po, "toolVersion", &provenance.tool_version);
        put(&mut root, "provenance", Value::Object(po));
    }

    Value::Object(root)
}

/// Port of `validate_for_write`: the built tree must re-parse to a valid
/// manifest before it may be serialized.
pub(super) fn validate_for_write(m: &Manifest) -> Result<(), Error> {
    let tree = build_tree(m);
    manifest_from_tree(&tree).map(|_| ())
}

impl Manifest {
    /// Serializes to canonical JSON (no unknown-field preservation —
    /// the behaviour of the C `musicpack_manifest_write`).
    pub fn write_canonical(&self) -> Result<String, Error> {
        validate_for_write(self)?;
        Ok(print_canonical(&build_tree(self)))
    }
}

impl ParsedManifest {
    /// Serializes to canonical JSON, preserving unknown **root-level**
    /// fields from the original document (port of
    /// `musicpack_manifest_write_with_original`).
    pub fn write_canonical(&self) -> Result<String, Error> {
        validate_for_write(&self.manifest)?;
        let mut tree = build_tree(&self.manifest);
        copy_package_extensions(&mut tree, &self.original)?;
        Ok(print_canonical(&tree))
    }
}

/// Port of `copy_package_extensions`: append every unknown root field of
/// `original` to `tree`, in original order.
fn copy_package_extensions(tree: &mut Value, original: &Value) -> Result<(), Error> {
    let Value::Object(members) = original else {
        return Ok(());
    };
    let Value::Object(root) = tree else {
        return Err(Error::Invalid {
            detail: "internal: manifest tree is not an object".into(),
        });
    };
    for (key, value) in members {
        if KNOWN_ROOT_FIELDS.contains(&key.as_str()) {
            continue;
        }
        root.push((key.clone(), value.clone()));
    }
    Ok(())
}

/// Formats a number the way the canonical writer does (re-exported for
/// tests and tooling).
pub fn format_number(v: f64) -> String {
    format_json_number(v)
}
