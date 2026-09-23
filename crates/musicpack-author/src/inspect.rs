//! `inspect`: source → authoring draft JSON.
//!
//! [`open_to_draft`] dispatches exactly like the reference `inspect`
//! command: a directory carrying a regular `manifest.json` is an existing
//! `.mpack` package (see [`package_to_draft`]); anything else is a fresh
//! album source directory handled by [`crate::scan::source_to_draft`].
//!
//! The authoring counterpart of `build_directory`. It reads a package
//! directory through the core [`DirectoryBackend`] (the same backend the
//! verifier uses), maps the parsed manifest onto the draft JSON surface and
//! anchors every source path at the package directory, so an
//! inspect → edit → rebuild round trip is source-compatible (audio is
//! already encoded and passes through).
//!
//! # Derived values are not emitted as authored input
//!
//! The manifest carries derived facts (hashes, waveform points, duration,
//! loudness). The draft surface exposes the *authored* subset plus, for
//! preview purposes only, a `waveformAnalysis` block and per-track `duration`
//! that the Rust pipeline re-derives at build. No value emitted here can
//! cause a hash, point count, measurement or identity to be trusted.
//!
//! Only package *directories* are inspected. (The Author UI opens
//! directories; `.mpak` containers are a distribution format.)

use std::path::Path;

use musicpack_core::format::manifest::{
    Album, Artist, Disc, Identifiers, Identity, Manifest, Release, Source, Track,
};
use musicpack_core::format::waveform;
use musicpack_core::json::Value;
use musicpack_core::storage::directory::DirectoryBackend;

use crate::error::{AuthorError, Result};

pub(crate) fn s(value: &str) -> Value {
    Value::String(value.to_string())
}

fn opt(value: &Option<String>) -> Option<Value> {
    value.as_deref().map(s)
}

/// Builds an object from `(key, value)` pairs, skipping `None`.
pub(crate) fn object(members: Vec<(&str, Option<Value>)>) -> Value {
    Value::Object(
        members
            .into_iter()
            .filter_map(|(k, v)| v.map(|v| (k.to_string(), v)))
            .collect(),
    )
}

/// Builds an object from `(key, value)` pairs (all present).
pub(crate) fn object_all(members: Vec<(&str, Value)>) -> Value {
    Value::Object(
        members
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

fn artists(artists: &[Artist]) -> Value {
    Value::Array(
        artists
            .iter()
            .map(|a| {
                object(vec![
                    ("name", Some(s(&a.name))),
                    ("role", opt(&a.role)),
                    ("musicbrainzId", opt(&a.musicbrainz_id)),
                    ("sortName", opt(&a.sort_name)),
                ])
            })
            .collect(),
    )
}

fn album(album: &Album) -> Value {
    object(vec![
        ("title", Some(s(&album.title))),
        ("artists", Some(artists(&album.artists))),
        ("releaseType", album.release_type.map(|t| s(t.as_str()))),
        ("originalReleaseDate", opt(&album.original_release_date)),
        (
            "genres",
            (!album.genres.is_empty())
                .then(|| Value::Array(album.genres.iter().map(|g| s(g)).collect())),
        ),
    ])
}

fn release(release: &Release) -> Value {
    object(vec![
        ("releaseDate", opt(&release.release_date)),
        ("edition", opt(&release.edition)),
        ("country", opt(&release.country)),
        ("label", opt(&release.label)),
        ("catalogueNumber", opt(&release.catalogue_number)),
        ("notes", opt(&release.notes)),
    ])
}

fn identifiers(ids: &Identifiers) -> Value {
    object(vec![
        (
            "musicbrainzReleaseGroupId",
            opt(&ids.musicbrainz_release_group_id),
        ),
        ("musicbrainzReleaseId", opt(&ids.musicbrainz_release_id)),
        ("barcode", opt(&ids.barcode)),
    ])
}

fn identity(identity: &Identity) -> Value {
    object(vec![
        ("source", identity.source.map(|v| s(v.as_str()))),
        ("confidence", identity.confidence.map(|v| s(v.as_str()))),
    ])
}

fn source(source: &Source) -> Value {
    object(vec![
        ("type", opt(&source.kind)),
        ("store", opt(&source.store)),
        ("sourceId", opt(&source.id)),
    ])
}

fn track(track: &Track, waveform_points: &NotGenerated) -> Value {
    let _ = waveform_points;
    let mut members: Vec<(&str, Option<Value>)> = vec![
        ("track", Some(Value::Number(f64::from(track.number)))),
        ("title", Some(s(&track.title))),
        (
            "artists",
            (!track.artists.is_empty()).then(|| artists(&track.artists)),
        ),
        (
            "identifiers",
            track.identifiers.as_ref().map(|i| {
                object(vec![
                    ("isrc", opt(&i.isrc)),
                    ("musicbrainzTrackId", opt(&i.musicbrainz_track_id)),
                    ("musicbrainzRecordingId", opt(&i.musicbrainz_recording_id)),
                ])
            }),
        ),
        (
            "source",
            track.source.as_ref().map(|src| {
                object(vec![
                    ("store", opt(&src.store)),
                    ("trackId", opt(&src.track_id)),
                ])
            }),
        ),
        (
            "sourceAudio",
            track
                .source_audio
                .as_ref()
                .map(|sa| object(vec![("codec", opt(&sa.codec)), ("md5", opt(&sa.md5))])),
        ),
        ("duration", track.duration.map(Value::Number)),
        ("codec", opt(&track.audio_codec)),
        ("audioPath", Some(s(&track.audio.path))),
    ];
    if !track.lyrics.is_empty() {
        members.push((
            "lyrics",
            Some(Value::Array(
                track
                    .lyrics
                    .iter()
                    .map(|l| object(vec![("path", Some(s(&l.path))), ("lang", opt(&l.lang))]))
                    .collect(),
            )),
        ));
    }
    if !track.representations.is_empty() {
        members.push((
            "representations",
            Some(Value::Array(
                track
                    .representations
                    .iter()
                    .map(|r| {
                        object(vec![
                            ("path", Some(s(&r.path))),
                            ("label", opt(&r.label)),
                            ("codec", opt(&r.codec)),
                        ])
                    })
                    .collect(),
            )),
        ));
    }
    object(members)
}

/// Marker type kept to document that waveform point counts are omitted from
/// the authored surface (the build derives them).
struct NotGenerated;

fn disc(disc: &Disc) -> Value {
    object(vec![
        ("disc", Some(Value::Number(f64::from(disc.number)))),
        ("format", disc.format.map(|f| s(f.as_str()))),
        ("title", opt(&disc.title)),
        (
            "tracks",
            Some(Value::Array(
                disc.tracks
                    .iter()
                    .map(|t| track(t, &NotGenerated))
                    .collect(),
            )),
        ),
    ])
}

fn asset_paths(assets: &[musicpack_core::format::manifest::Asset]) -> Value {
    Value::Array(
        assets
            .iter()
            .map(|a| object_all(vec![("path", s(&a.path))]))
            .collect(),
    )
}

/// The preview `waveformAnalysis` block built from manifest references.
///
/// Only `path` and a display `points` count are surfaced as app state; the
/// authoritative hash/point derivation happens at build. `None` when no track
/// carries a waveform.
fn waveform_analysis(manifest: &Manifest, source_root: &str) -> Option<Value> {
    let mut tracks = Vec::new();
    let mut total = 0usize;
    for disc in &manifest.media {
        for track in &disc.tracks {
            total += 1;
            if let Some(wf) = &track.waveform {
                tracks.push(object_all(vec![
                    ("disc", Value::Number(f64::from(disc.number))),
                    ("track", Value::Number(f64::from(track.number))),
                    ("points", Value::Number(wf.points as f64)),
                    ("sha256", s(&wf.sha256)),
                    ("path", s(&wf.path)),
                ]));
            }
        }
    }
    (!tracks.is_empty()).then(|| {
        object_all(vec![
            ("status", s("ready")),
            ("intervalMs", Value::Number(waveform::INTERVAL_MS as f64)),
            ("encoding", s(waveform::ENCODING)),
            ("floorDb", Value::Number(waveform::FLOOR_DB as f64)),
            ("tracks", Value::Array(tracks.clone())),
            ("tracksGenerated", Value::Number(tracks.len() as f64)),
            ("tracksTotal", Value::Number(total as f64)),
            ("sourceRoot", s(source_root)),
        ])
    })
}

/// The analysis-document passthrough (`analysis[]`), preserved opaquely so a
/// rebuild does not silently drop sonic (or future) analysis references.
fn analysis(manifest: &Manifest) -> Option<Value> {
    (!manifest.analysis.is_empty()).then(|| {
        Value::Array(
            manifest
                .analysis
                .iter()
                .map(|a| {
                    object(vec![
                        ("kind", Some(s(&a.kind))),
                        ("profile", opt(&a.profile)),
                        ("path", Some(s(&a.asset.path))),
                    ])
                })
                .collect(),
        )
    })
}

/// Maps a manifest onto the draft JSON value, anchored at `source_root`.
pub fn draft_value(manifest: &Manifest, source_root: &str) -> Value {
    let mut members: Vec<(&str, Option<Value>)> = vec![
        ("schema", Some(s("musicpack-draft"))),
        ("version", Some(Value::Number(1.0))),
        ("sourceRoot", Some(s(source_root))),
        ("openedFrom", Some(s(source_root))),
        ("album", Some(album(&manifest.album))),
        ("release", manifest.release.as_ref().map(release)),
        (
            "identifiers",
            manifest.identifiers.as_ref().map(identifiers),
        ),
        ("identity", manifest.identity.as_ref().map(identity)),
        ("source", manifest.source.as_ref().map(source)),
        (
            "media",
            Some(Value::Array(manifest.media.iter().map(disc).collect())),
        ),
        (
            "artwork",
            Some(Value::Array(
                manifest
                    .artwork
                    .iter()
                    .map(|a| object_all(vec![("role", s(&a.role)), ("path", s(&a.asset.path))]))
                    .collect(),
            )),
        ),
        ("booklet", Some(asset_paths(&manifest.booklet))),
        ("lyrics", Some(asset_paths(&manifest.lyrics))),
        ("extras", Some(asset_paths(&manifest.extras))),
        ("analysis", analysis(manifest)),
        ("waveformAnalysis", waveform_analysis(manifest, source_root)),
    ];
    members.retain(|(_, v)| v.is_some());
    Value::Object(
        members
            .into_iter()
            .filter_map(|(k, v)| v.map(|v| (k.to_string(), v)))
            .collect(),
    )
}

/// Writes the authored metadata subset of `draft` back into an existing draft
/// JSON value, preserving every unrelated field (app state, `openedFrom`,
/// `waveformAnalysis`, …). Used after identification mutates the typed draft.
pub fn sync_metadata(root: &mut Value, draft: &crate::draft::Draft) {
    use crate::draft::{json_array_mut, json_remove, json_set};

    json_set(root, "album", album(&draft.album));
    match &draft.release {
        Some(r) => json_set(root, "release", release(r)),
        None => json_remove(root, "release"),
    }
    match &draft.identifiers {
        Some(i) => json_set(root, "identifiers", identifiers(i)),
        None => json_remove(root, "identifiers"),
    }
    match &draft.identity {
        Some(i) => json_set(root, "identity", identity(i)),
        None => json_remove(root, "identity"),
    }
    match &draft.source {
        Some(s) => json_set(root, "source", source(s)),
        None => json_remove(root, "source"),
    }

    // Per-disc format/title and per-track title/identifiers.
    let some_media = obj_member_mut(root, "media").and_then(json_array_mut);
    let Some(media) = some_media else {
        return;
    };
    for (di, disc) in draft.media.iter().enumerate() {
        let Some(disc_v) = media.get_mut(di) else {
            continue;
        };
        if let Some(f) = disc.format {
            json_set(disc_v, "format", Value::String(f.as_str().to_string()));
        }
        if let Some(t) = &disc.title {
            json_set(disc_v, "title", Value::String(t.clone()));
        }
        let Some(tracks) = obj_member_mut(disc_v, "tracks").and_then(json_array_mut) else {
            continue;
        };
        for (ti, track) in disc.tracks.iter().enumerate() {
            let Some(track_v) = tracks.get_mut(ti) else {
                continue;
            };
            if !track.title.is_empty() {
                json_set(track_v, "title", Value::String(track.title.clone()));
            }
            if let Some(i) = &track.identifiers {
                json_set(
                    track_v,
                    "identifiers",
                    object(vec![
                        ("isrc", opt(&i.isrc)),
                        ("musicbrainzTrackId", opt(&i.musicbrainz_track_id)),
                        ("musicbrainzRecordingId", opt(&i.musicbrainz_recording_id)),
                    ]),
                );
            }
        }
    }
}

fn obj_member_mut<'a>(value: &'a mut Value, key: &str) -> Option<&'a mut Value> {
    match value {
        Value::Object(members) => members.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
        _ => None,
    }
}

/// Inspects a package directory and returns authoring draft JSON.
pub fn package_to_draft(package: &Path) -> Result<String> {
    let root = package.canonicalize().map_err(|e| AuthorError::Io {
        detail: format!("cannot resolve '{}': {e}", package.display()),
    })?;
    if !root.is_dir() {
        return Err(AuthorError::Io {
            detail: format!(
                "'{}' is not a package directory (use the package directory, not a .mpak)",
                package.display()
            ),
        });
    }
    let backend = DirectoryBackend::open(&root).map_err(|e| AuthorError::Io {
        detail: format!("cannot open '{}': {e}", root.display()),
    })?;
    let parsed = musicpack_core::format::manifest::ParsedManifest::parse(backend.manifest_bytes())
        .map_err(|e| AuthorError::Draft {
            detail: format!("'{}' is not a valid package manifest: {e}", root.display()),
        })?;
    let root_str = root.to_string_lossy().into_owned();
    Ok(musicpack_core::json::print_canonical(&draft_value(
        parsed.manifest(),
        &root_str,
    )))
}

/// Opens `path` as an authoring draft source: an existing `.mpack`
/// package directory when it carries a regular `manifest.json`, otherwise
/// a fresh album source directory discovered by
/// [`crate::scan::source_to_draft`]. This mirrors the reference `inspect`
/// dispatch (a stat on `manifest.json` picks the branch), so the Author
/// UI's single "open album or package" entry point works for both.
pub fn open_to_draft(path: &Path) -> Result<String> {
    if path.join("manifest.json").is_file() {
        package_to_draft(path)
    } else {
        crate::scan::source_to_draft(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicpack_core::format::manifest::Manifest;
    use musicpack_core::json;

    fn minimal_manifest() -> Manifest {
        Manifest {
            album: Album {
                title: "Album".into(),
                artists: vec![Artist {
                    name: "Artist".into(),
                    ..Default::default()
                }],
                release_type: None,
                original_release_date: None,
                genres: Vec::new(),
            },
            release: None,
            identifiers: None,
            identity: None,
            source: None,
            media: vec![Disc {
                number: 1,
                format: None,
                title: None,
                tracks: vec![Track {
                    number: 1,
                    title: "One".into(),
                    artists: Vec::new(),
                    identifiers: None,
                    source: None,
                    source_audio: None,
                    duration: None,
                    loudness: None,
                    audio: musicpack_core::format::manifest::Asset {
                        path: "audio/01 - One.mpc".into(),
                        sha256: "0".repeat(64),
                    },
                    audio_codec: Some("musepack-sv8".into()),
                    waveform: None,
                    lyrics: Vec::new(),
                    representations: Vec::new(),
                }],
            }],
            artwork: Vec::new(),
            booklet: Vec::new(),
            lyrics: Vec::new(),
            extras: Vec::new(),
            analysis: Vec::new(),
            loudness: None,
            provenance: None,
        }
    }

    #[test]
    fn draft_value_round_trips_through_the_parser() {
        let manifest = minimal_manifest();
        let value = draft_value(&manifest, "/tmp/Album.mpack");
        let bytes = json::print_canonical(&value).into_bytes();
        let parsed = crate::draft::parse(&bytes).unwrap();
        assert_eq!(parsed.album.title, "Album");
        assert_eq!(parsed.media[0].tracks[0].audio_path, "audio/01 - One.mpc");
        assert_eq!(
            parsed.media[0].tracks[0].codec.as_deref(),
            Some("musepack-sv8")
        );
    }

    #[test]
    fn booklet_objects_are_accepted() {
        // The reference/UI emit `booklet: [{path}]`, which the R4.2 parser
        // originally rejected (strings only).
        let json = br#"{"sourceRoot":"/tmp","album":{"title":"T","artists":[{"name":"A"}]},
            "media":[{"disc":1,"tracks":[{"track":1,"title":"One","audioPath":"a.mpc"}]}],
            "booklet":[{"path":"notes.pdf"}],"lyrics":[{"path":"a.lrc"}],"extras":[{"path":"x.txt"}]}"#;
        let parsed = crate::draft::parse(json).unwrap();
        assert_eq!(parsed.booklet, vec!["notes.pdf".to_string()]);
        assert_eq!(parsed.lyrics, vec!["a.lrc".to_string()]);
        assert_eq!(parsed.extras, vec!["x.txt".to_string()]);
    }
}
