//! Fresh-album source discovery: album directory → authoring draft.
//!
//! This is the discovery boundary of the Author workflow — *what files
//! exist, what their tags say, in what order* — and nothing else. It
//! produces the same `musicpack-draft` JSON surface the rest of the
//! pipeline already consumes; package building, validation, encoding,
//! identity and verification are reused untouched (no second authoring
//! pipeline, no package logic here).
//!
//! ```text
//! Album/{01 - Track.flac, cover.jpg, …}
//!      │  scan::source_to_draft
//!      ▼
//! MusicPack draft ──► existing pipeline (validate → encode → waveform → build)
//! ```
//!
//! # Discovery rules (conventional album directories)
//!
//! * Files **at the root** and files **directly inside a disc-named
//!   subdirectory** (`CD1`, `cd2`, `Disc-3`, `disc_4`, …) are scanned;
//!   anything in other subdirectories (or deeper) is ignored — the
//!   reference's "ignore files under non-disc subdirectories" rule,
//!   tightened to one level (MusicPack targets conventional layouts).
//! * Audio extensions (case-insensitive): `.flac`, `.wav`, `.mpc`.
//!   `.mpc` is ordinary input and passes through the pipeline untouched.
//!   Formats the Rust pipeline cannot build (e.g. `.ogg`) are *not*
//!   discovered — a discovered-but-unbuildable draft would only fail
//!   later.
//! * Assets: one deterministic `front` cover from
//!   `cover|front|folder` × `jpg|jpeg|png` (root before disc dirs, then
//!   name order, then extension order), `booklet.pdf`, `*.lrc` sidecars,
//!   and `*.txt`/`*.md` extras.
//! * Tags: FLAC → Vorbis comments, `.mpc` → trailing APEv2 text items,
//!   WAV → none (the reference scan reads none either). Tag reads are
//!   **best-effort**: a tag failure leaves the file untagged rather than
//!   dropping it (filename inference still applies). Keys are matched
//!   case-insensitively.
//! * Numbering: `TRACKNUMBER` (APEv2 `Track`) wins, else the filename's
//!   leading digits, else the track is unnumbered. Disc comes from the
//!   disc-directory name (which wins over the tag, as in the reference),
//!   else `DISCNUMBER` on root files, else `1`. A disc with any
//!   unnumbered **or duplicate** track is renumbered `1..n` in sorted
//!   order — the reference `import` behaviour. (The reference `inspect`
//!   preserved duplicates so its GUI could surface them; the Rust draft
//!   validator has no duplicate check — the builder fails closed — so
//!   discovery normalises them instead of deferring a late failure.)
//! * Order is deterministic: disc, then track number (unnumbered last),
//!   then relative path.
//! * Album metadata is a first-wins union across the tracks in final
//!   order (the reference's tag union); album title falls back to the
//!   directory name, album artists fall back to the tracks' `ARTIST`
//!   union (the reference had no fallback and such albums failed
//!   validation with `no artist`; MusicPack discovers a valid draft
//!   instead — credits stay user-editable).
//!
//! # What discovery deliberately does *not* do
//!
//! * No stream probing: per-track `codec`/`sampleRate`/`duration` are
//!   derived facts the pipeline re-derives at build; sample-rate and
//!   channel support is enforced fail-closed by the encode stage.
//! * No embedded-artwork extraction (FLAC `PICTURE`, APEv2 cover art) —
//!   external cover files only; embedded extraction is a separate slice
//!   (`docs/author-pipeline.md` §9 lists it as fail-closed today).
//! * No resampling, downmixing, transcoding, hashing or MusicBrainz
//!   lookup (identify stays an explicit user action).
//! * No `waveformAnalysis`/`identity`/`openedFrom` blocks: absent means
//!   "new draft"; the runtime defaults waveforms on.

use std::fs;
use std::path::{Path, PathBuf};

use musicpack_core::audio::{flac, musepack};
use musicpack_core::format::manifest::ReleaseType;
use musicpack_core::json::{self, Value};

use crate::error::{AuthorError, Result};
use crate::inspect::{object, object_all, s};

/// Audio extensions the Rust pipeline can build, lower-case.
const AUDIO_EXTS: [&str; 3] = ["flac", "wav", "mpc"];
/// Cover file stems, in priority order.
const COVER_NAMES: [&str; 3] = ["cover", "front", "folder"];
/// Cover extensions, in priority order.
const COVER_EXTS: [&str; 3] = ["jpg", "jpeg", "png"];
/// Extra-document extensions.
const EXTRAS_EXTS: [&str; 2] = ["txt", "md"];

/// One discovered file, with its path relative to the source root.
struct Found {
    /// `/'`-separated path relative to the source root.
    rel: String,
    /// Absolute path.
    abs: PathBuf,
    /// The parent directory relative path (`""` for the root).
    parent: String,
    /// The file name.
    name: String,
    /// The disc number this file's directory names (`None` at the root).
    disc_dir: Option<i32>,
}

/// One scanned audio track with its tags (keys upper-cased).
struct ScannedTrack {
    /// Relative path (the draft's `audioPath`).
    rel: String,
    /// Parent directory relative path (lyric sidecar matching).
    parent: String,
    /// Original-case file stem (lyric sidecar matching).
    stem: String,
    /// Disc number (`1..`).
    disc: i32,
    /// Track number; `0` = unnumbered before the per-disc fill.
    number: i32,
    /// Track title (may be empty when neither tag nor filename yield one).
    title: String,
    /// Matched `.lrc` sidecars (same directory + stem), relative paths.
    lyrics: Vec<String>,
    /// Upper-cased, trimmed `(key, value)` tags in file order.
    tags: Vec<(String, String)>,
}

/// A discovered non-audio asset.
enum Asset {
    /// A candidate front cover.
    Cover,
    /// `booklet.pdf`.
    Booklet,
    /// An `.lrc` sidecar.
    Lyrics,
    /// A `.txt`/`.md` extra.
    Extras,
}

/// Disc number from a subdirectory name: `cd`/`disc` (case-insensitive)
/// + optional `-`/`_`/space separators + digits (`CD1`, `Disc-2`, …).
fn disc_from_dirname(name: &str) -> Option<i32> {
    let lower = name.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("cd")
        .or_else(|| lower.strip_prefix("disc"))?;
    let rest = rest.trim_start_matches(['-', '_', ' ']);
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let n: i32 = digits.parse().ok()?;
    (n >= 1).then_some(n)
}

/// The audio extension of `name` (lower-cased), `None` when the name has
/// no real stem-extension split or the extension is not buildable audio.
fn audio_ext(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let (stem, ext) = lower.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() {
        return None;
    }
    AUDIO_EXTS.contains(&ext).then(|| ext.to_string())
}

/// Classification of a file name (case-insensitive), `None` = ignored.
fn asset_kind(name: &str) -> Option<Asset> {
    let lower = name.to_ascii_lowercase();
    if lower == "booklet.pdf" {
        return Some(Asset::Booklet);
    }
    let (stem, ext) = lower.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() {
        return None;
    }
    if COVER_NAMES.contains(&stem) && COVER_EXTS.contains(&ext) {
        return Some(Asset::Cover);
    }
    if ext == "lrc" {
        return Some(Asset::Lyrics);
    }
    if EXTRAS_EXTS.contains(&ext) {
        return Some(Asset::Extras);
    }
    None
}

/// Cover priority: root before disc dirs, then name, then extension.
fn cover_rank(found: &Found) -> (u8, usize, usize, &str) {
    let lower = found.name.to_ascii_lowercase();
    let (stem, ext) = lower.rsplit_once('.').unwrap_or((&lower, ""));
    (
        u8::from(!found.parent.is_empty()), // root (0) beats disc dirs (1)
        COVER_NAMES.iter().position(|n| *n == stem).unwrap_or(9),
        COVER_EXTS.iter().position(|e| *e == ext).unwrap_or(9),
        &found.rel,
    )
}

/// Walks `root`: root-level files plus files directly inside disc-named
/// subdirectories, deterministically ordered (root files first, then
/// subdirectories by name, each sorted by name). Non-disc subdirectories
/// and deeper nesting are ignored; symlinks are skipped (the reference
/// classifies with `lstat`).
fn walk(root: &Path) -> Result<Vec<Found>> {
    let io = |e: std::io::Error| AuthorError::Io {
        detail: format!("cannot read '{}': {e}", root.display()),
    };
    let mut root_entries: Vec<fs::DirEntry> = fs::read_dir(root).map_err(io)?.flatten().collect();
    root_entries.sort_by_key(|e| e.file_name());

    let mut out = Vec::new();

    // Root-level files first.
    for entry in &root_entries {
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_file() {
            let name = entry.file_name().to_string_lossy().into_owned();
            out.push(Found {
                rel: name.clone(),
                abs: entry.path(),
                parent: String::new(),
                name,
                disc_dir: None,
            });
        }
    }
    // Then disc directories, each sorted by name.
    for entry in &root_entries {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        let Some(disc) = disc_from_dirname(&dir_name) else {
            continue; // non-disc subdirectory: ignored entirely
        };
        let mut children: Vec<fs::DirEntry> =
            fs::read_dir(entry.path()).map_err(io)?.flatten().collect();
        children.sort_by_key(|e| e.file_name());
        for child in &children {
            let Ok(cft) = child.file_type() else { continue };
            if !cft.is_file() {
                continue;
            }
            let name = child.file_name().to_string_lossy().into_owned();
            out.push(Found {
                rel: format!("{dir_name}/{name}"),
                abs: child.path(),
                parent: dir_name.clone(),
                name,
                disc_dir: Some(disc),
            });
        }
    }
    Ok(out)
}

/// Reads a file's tags, upper-cased and trimmed; best-effort (a failure
/// yields no tags rather than dropping the file). WAV has no tag block
/// the reference scan reads either.
fn read_tags(abs: &Path, ext: &str) -> Vec<(String, String)> {
    let raw: Option<Vec<(String, String)>> = match ext {
        "flac" => fs::File::open(abs)
            .ok()
            .and_then(|f| flac::read_vorbis_comments(Box::new(f)).ok()),
        "mpc" => fs::File::open(abs)
            .ok()
            .and_then(|mut f| musepack::apev2::read_tags(&mut f).ok()),
        _ => None,
    };
    raw.unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (k.to_ascii_uppercase(), v.trim().to_string()))
        .collect()
}

/// The first non-empty value among `keys`, preferring earlier keys and
/// earlier tracks (first-wins, like the reference tag union).
fn first_tag<'a>(tracks: &[&'a ScannedTrack], keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        for track in tracks {
            if let Some((_, v)) = track.tags.iter().find(|(k, _)| k == key) {
                if !v.is_empty() {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// Distinct non-empty values among `keys` across tracks, first-wins.
fn union_tags(tracks: &[&ScannedTrack], keys: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for track in tracks {
        for (k, v) in &track.tags {
            if keys.contains(&k.as_str()) && !v.is_empty() && !out.iter().any(|x| x == v) {
                out.push(v.clone());
            }
        }
    }
    out
}

/// The leading run of ASCII digits as a track/disc number (`≥ 1`), so
/// `"3/12"` → `3` and `"0"` → `None`.
fn leading_number(s: &str) -> Option<i32> {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    let n: i32 = digits.parse().ok()?;
    (n >= 1).then_some(n)
}

/// Filename stem → title: strips a leading track number and the
/// separators after it (`"01 - Title"` → `"Title"`). When stripping would
/// leave nothing (`"01.flac"`, `"1984.flac"`), the full stem is kept so
/// discovery yields a *valid* (user-editable) title instead of the
/// reference's empty-title validation error — an intentional divergence.
fn title_from_stem(stem: &str) -> String {
    let stripped = stem
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .trim_start_matches([' ', '-', '_', '.']);
    if stripped.is_empty() {
        stem.to_string()
    } else {
        stripped.to_string()
    }
}

/// The album artists: the `ALBUMARTIST` union first, falling back to the
/// tracks' `ARTIST` union (see the module docs).
fn album_artists(tracks: &[&ScannedTrack]) -> Vec<String> {
    let album_artist = union_tags(tracks, &["ALBUMARTIST", "ALBUM ARTIST", "ALBUM_ARTIST"]);
    if !album_artist.is_empty() {
        return album_artist;
    }
    union_tags(tracks, &["ARTIST"])
}

/// One track's credits: `ARTIST` values (role `main`) then `COMPOSER`
/// values (role `composer`) — the MVP tag-mapping surface.
fn track_artists(track: &ScannedTrack) -> Vec<Value> {
    let one = [track];
    let mut out: Vec<Value> = union_tags(&one, &["ARTIST"])
        .into_iter()
        .map(|name| object_all(vec![("name", s(&name)), ("role", s("main"))]))
        .collect();
    out.extend(
        union_tags(&one, &["COMPOSER"])
            .into_iter()
            .map(|name| object_all(vec![("name", s(&name)), ("role", s("composer"))])),
    );
    out
}

/// One track's identifiers: ISRC plus the MusicBrainz recording/track
/// ids (the fields the identify ladder and the manifest carry).
fn track_identifiers(track: &ScannedTrack) -> Option<Value> {
    let one = [track];
    let value = object(vec![
        ("isrc", first_tag(&one, &["ISRC"]).map(s)),
        (
            "musicbrainzRecordingId",
            first_tag(&one, &["MUSICBRAINZ_RECORDINGID"]).map(s),
        ),
        (
            "musicbrainzTrackId",
            first_tag(&one, &["MUSICBRAINZ_TRACKID"]).map(s),
        ),
    ]);
    (!value.as_object().is_some_and(|m| m.is_empty())).then_some(value)
}

/// One track's draft JSON.
fn track_value(track: &ScannedTrack) -> Value {
    let artists = track_artists(track);
    let lyrics = Value::Array(
        track
            .lyrics
            .iter()
            .map(|p| object_all(vec![("path", s(p))]))
            .collect(),
    );
    object(vec![
        ("track", Some(Value::Number(track.number as f64))),
        ("title", Some(s(&track.title))),
        (
            "artists",
            (!artists.is_empty()).then_some(Value::Array(artists)),
        ),
        ("identifiers", track_identifiers(track)),
        ("audioPath", Some(s(&track.rel))),
        ("lyrics", (!track.lyrics.is_empty()).then_some(lyrics)),
    ])
}

/// `true` when an object has no members.
fn members_empty(value: &Value) -> bool {
    value.as_object().is_some_and(|m| m.is_empty())
}

/// Builds the draft JSON value for a scanned album.
fn draft_from_tracks(root: &Path, tracks: &[ScannedTrack], found: &[Found]) -> Value {
    let root_str = root.to_string_lossy().into_owned();
    let dir_name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let refs: Vec<&ScannedTrack> = tracks.iter().collect();

    // ---- album -------------------------------------------------------
    let album_title = first_tag(&refs, &["ALBUM"]).unwrap_or(&dir_name);
    let artists = album_artists(&refs);
    let release_type = first_tag(&refs, &["RELEASETYPE", "MUSICBRAINZ_ALBUMTYPE"])
        .and_then(|v| ReleaseType::parse(&v.to_ascii_lowercase()));
    let original_release_date =
        first_tag(&refs, &["ORIGINALDATE", "ORIGINAL YEAR"]).map(str::to_string);
    let genres = union_tags(&refs, &["GENRE"]);

    let album = object(vec![
        ("title", Some(s(album_title))),
        (
            "artists",
            Some(Value::Array(
                artists
                    .iter()
                    .map(|name| object_all(vec![("name", s(name)), ("role", s("main"))]))
                    .collect(),
            )),
        ),
        ("releaseType", release_type.map(|t| s(t.as_str()))),
        (
            "originalReleaseDate",
            original_release_date.as_deref().map(s),
        ),
        (
            "genres",
            (!genres.is_empty()).then(|| Value::Array(genres.iter().map(|g| s(g)).collect())),
        ),
    ]);

    // ---- release / identifiers (only when a tag is present) ----------
    let release = object(vec![
        ("releaseDate", first_tag(&refs, &["DATE", "YEAR"]).map(s)),
        (
            "country",
            first_tag(&refs, &["MUSICBRAINZ_ALBUMCOUNTRY", "RELEASECOUNTRY"]).map(s),
        ),
        (
            "label",
            first_tag(&refs, &["PUBLISHER", "LABEL", "ORGANIZATION"]).map(s),
        ),
        (
            "catalogueNumber",
            first_tag(&refs, &["CATALOGNUMBER", "CATALOGUENUMBER", "CATALOG #"]).map(s),
        ),
    ]);
    let release = (!members_empty(&release)).then_some(release);

    let identifiers = object(vec![
        (
            "musicbrainzReleaseGroupId",
            first_tag(&refs, &["MUSICBRAINZ_RELEASEGROUPID"]).map(s),
        ),
        (
            "musicbrainzReleaseId",
            first_tag(&refs, &["MUSICBRAINZ_RELEASEID", "MUSICBRAINZ_ALBUMID"]).map(s),
        ),
        ("barcode", first_tag(&refs, &["BARCODE"]).map(s)),
    ]);
    let identifiers = (!members_empty(&identifiers)).then_some(identifiers);

    // ---- media (tracks are sorted, so disc runs are contiguous) ------
    let mut disc_runs: Vec<i32> = tracks.iter().map(|t| t.disc).collect();
    disc_runs.dedup();
    let media = Value::Array(
        disc_runs
            .into_iter()
            .map(|disc| {
                let members: Vec<&ScannedTrack> =
                    tracks.iter().filter(|t| t.disc == disc).collect();
                let tracks_value = Value::Array(members.iter().map(|t| track_value(t)).collect());
                object_all(vec![
                    ("disc", Value::Number(disc as f64)),
                    ("tracks", tracks_value),
                ])
            })
            .collect(),
    );

    // ---- assets ------------------------------------------------------
    let cover = found
        .iter()
        .filter(|f| matches!(asset_kind(&f.name), Some(Asset::Cover)))
        .min_by(|a, b| cover_rank(a).cmp(&cover_rank(b)));
    let artwork = Value::Array(
        cover
            .map(|f| object_all(vec![("role", s("front")), ("path", s(&f.rel))]))
            .into_iter()
            .collect(),
    );

    let paths_of = |kind: Asset| -> Vec<&Found> {
        found
            .iter()
            .filter(|f| {
                matches!(
                    (asset_kind(&f.name), &kind),
                    (Some(Asset::Booklet), Asset::Booklet)
                        | (Some(Asset::Lyrics), Asset::Lyrics)
                        | (Some(Asset::Extras), Asset::Extras)
                        | (Some(Asset::Cover), Asset::Cover)
                )
            })
            .collect()
    };
    let asset_array = |items: Vec<&Found>| {
        Value::Array(
            items
                .iter()
                .map(|f| object_all(vec![("path", s(&f.rel))]))
                .collect(),
        )
    };

    let booklet = asset_array(paths_of(Asset::Booklet));
    // Root-level lyrics: sidecars no track stem-matched.
    let matched: Vec<&String> = tracks.iter().flat_map(|t| &t.lyrics).collect();
    let root_lyrics: Vec<&Found> = paths_of(Asset::Lyrics)
        .into_iter()
        .filter(|f| !matched.contains(&&f.rel))
        .collect();
    let lyrics = Value::Array(
        root_lyrics
            .iter()
            .map(|f| object_all(vec![("path", s(&f.rel))]))
            .collect(),
    );
    let extras = asset_array(paths_of(Asset::Extras));

    object(vec![
        ("schema", Some(s("musicpack-draft"))),
        ("version", Some(Value::Number(1.0))),
        ("sourceRoot", Some(s(&root_str))),
        ("album", Some(album)),
        ("release", release),
        ("identifiers", identifiers),
        ("media", Some(media)),
        ("artwork", Some(artwork)),
        ("booklet", Some(booklet)),
        ("lyrics", Some(lyrics)),
        ("extras", Some(extras)),
    ])
}

/// Sort key: disc, then number (unnumbered last), then relative path.
fn sort_key(track: &ScannedTrack) -> (i32, i32, &str) {
    let number = if track.number <= 0 {
        i32::MAX
    } else {
        track.number
    };
    (track.disc, number, &track.rel)
}

/// Scans a fresh album source directory and returns authoring draft JSON.
///
/// Fails closed with [`AuthorError::Io`] when `dir` cannot be resolved,
/// is not a directory, or contains no buildable audio files (the
/// reference's `no audio files found` gate).
pub fn source_to_draft(dir: &Path) -> Result<String> {
    let root = dir.canonicalize().map_err(|e| AuthorError::Io {
        detail: format!("cannot resolve '{}': {e}", dir.display()),
    })?;
    if !root.is_dir() {
        return Err(AuthorError::Io {
            detail: format!("'{}' is not a directory", root.display()),
        });
    }

    let found = walk(&root)?;
    let audio: Vec<&Found> = found
        .iter()
        .filter(|f| audio_ext(&f.name).is_some())
        .collect();
    if audio.is_empty() {
        return Err(AuthorError::Io {
            detail: format!("no audio files found under '{}'", root.display()),
        });
    }

    // ---- tags + number/title/disc derivation -------------------------
    let mut tracks: Vec<ScannedTrack> = audio
        .iter()
        .map(|f| {
            let ext = audio_ext(&f.name).expect("filtered above");
            let tags = read_tags(&f.abs, &ext);
            let stem = Path::new(&f.name)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let number = tags
                .iter()
                .find(|(k, _)| k == "TRACKNUMBER" || k == "TRACK")
                .and_then(|(_, v)| leading_number(v))
                .or_else(|| leading_number(&stem))
                .unwrap_or(0);
            let title = tags
                .iter()
                .find(|(k, _)| k == "TITLE")
                .map(|(_, v)| v.clone())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| title_from_stem(&stem));
            let disc = match f.disc_dir {
                Some(d) => d, // a disc directory wins over the tag
                None => tags
                    .iter()
                    .find(|(k, _)| k == "DISCNUMBER" || k == "DISC")
                    .and_then(|(_, v)| leading_number(v))
                    .unwrap_or(1),
            };
            ScannedTrack {
                rel: f.rel.clone(),
                parent: f.parent.clone(),
                stem,
                disc,
                number,
                title,
                lyrics: Vec::new(),
                tags,
            }
        })
        .collect();

    // ---- deterministic order, then per-disc numbering ----------------
    tracks.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
    let mut start = 0;
    while start < tracks.len() {
        let disc = tracks[start].disc;
        let mut end = start;
        while end < tracks.len() && tracks[end].disc == disc {
            end += 1;
        }
        let group = &mut tracks[start..end];
        let unnumbered = group.iter().any(|t| t.number <= 0);
        let mut numbers: Vec<i32> = group.iter().map(|t| t.number).collect();
        numbers.sort_unstable();
        let duplicated = numbers.windows(2).any(|w| w[0] == w[1]);
        if unnumbered || duplicated {
            for (idx, track) in group.iter_mut().enumerate() {
                track.number = idx as i32 + 1;
            }
        }
        start = end;
    }

    // ---- lyric sidecars: same directory + stem -----------------------
    let sidecars: Vec<(String, String, String)> = found
        .iter()
        .filter(|f| matches!(asset_kind(&f.name), Some(Asset::Lyrics)))
        .filter_map(|f| {
            let stem = Path::new(&f.name)
                .file_stem()?
                .to_string_lossy()
                .into_owned();
            Some((f.parent.clone(), stem, f.rel.clone()))
        })
        .collect();
    for (parent, stem, rel) in sidecars {
        if let Some(track) = tracks
            .iter_mut()
            .find(|t| t.parent == parent && t.stem == stem)
        {
            track.lyrics.push(rel);
        }
    }

    let value = draft_from_tracks(&root, &tracks, &found);
    Ok(json::print_canonical(&value))
}
