//! Identification stage: MusicBrainz matching and draft application.
//!
//! Division of labour (preserved from the reference): **transport is the
//! host's concern, matching is this crate's**. A [`MusicBrainzProvider`]
//! supplies raw MusicBrainz JSON (the Tauri host keeps its `ureq`
//! transport; tests use a static provider); everything below — candidate
//! extraction, confidence scoring and draft application — is pure and
//! deterministic over that JSON.
//!
//! # Matching semantics (port of `musicpack_mb_match_confidence`)
//!
//! The reference scores a candidate against the draft with four signals and
//! a fixed ladder:
//!
//! | signal | meaning |
//! |---|---|
//! | release id | exact string equality with the draft's release id (or the asserted `--mbid`) |
//! | barcode | non-empty equality |
//! | ISRC hit | any candidate recording ISRC appears in the draft's track ISRCs |
//! | track count | candidate track count equals the draft track count (and > 0) |
//! | title | exact, case-sensitive equality of release title and album title |
//!
//! ```text
//! id equal            -> exact
//! barcode equal       -> confirmed
//! ISRC hit + counts   -> confirmed
//! ISRC hit or title   -> probable
//! otherwise           -> none
//! ```
//!
//! Artist and date do **not** participate (the reference does not use them).
//! This crate preserves that behavior exactly rather than inventing a new
//! algorithm.
//!
//! # Ambiguity
//!
//! A barcode search returns **all** candidates with their confidence; it
//! never auto-selects a "best" one. Applying is explicit: the caller either
//! asserts a release id (`--mbid`) or offline (`--mb-json`). A confidence of
//! `none` applies nothing.

use musicpack_core::json::Value;

use crate::draft::Draft;
use crate::error::{AuthorError, Result};

/// Match confidence (the reference's `exact|confirmed|probable|none`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// The release id matched.
    Exact,
    /// Barcode matched, or ISRCs matched with an equal track count.
    Confirmed,
    /// An ISRC or the album title matched.
    Probable,
    /// No signal matched.
    None,
}

impl Confidence {
    /// The manifest/JSON spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::Exact => "exact",
            Confidence::Confirmed => "confirmed",
            Confidence::Probable => "probable",
            Confidence::None => "none",
        }
    }
}

/// A MusicBrainz transport provider.
///
/// Implementations perform the HTTP request and return the raw response
/// body. They must not interpret or mutate it. The pipeline only calls this
/// trait for live identification; offline (`--mb-json`) identification
/// bypasses it entirely.
pub trait MusicBrainzProvider {
    /// Fetches a release document by MBID
    /// (`/release/<mbid>?inc=...&fmt=json`).
    fn fetch_release(&self, mbid: &str) -> std::result::Result<Vec<u8>, String>;
    /// Searches releases by barcode (`/release/?query=barcode:<b>&fmt=json`).
    fn search_barcode(&self, barcode: &str) -> std::result::Result<Vec<u8>, String>;
}

// ---------------------------------------------------------------------
// parsed MusicBrainz documents
// ---------------------------------------------------------------------

/// A parsed MusicBrainz release.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MbRelease {
    /// Release id.
    pub id: Option<String>,
    /// Release title.
    pub title: Option<String>,
    /// Release date.
    pub date: Option<String>,
    /// Country.
    pub country: Option<String>,
    /// Barcode.
    pub barcode: Option<String>,
    /// Release-group id.
    pub release_group_id: Option<String>,
    /// Release-group primary type.
    pub primary_type: Option<String>,
    /// Release-group first-release date.
    pub first_release_date: Option<String>,
    /// Artist names (`artist-credit[].name`).
    pub artists: Vec<String>,
    /// Artist ids (`artist-credit[].artist.id`).
    pub artist_ids: Vec<String>,
    /// Artist sort names (`artist-credit[].artist.sort-name`).
    pub artist_sort_names: Vec<String>,
    /// Label name.
    pub label: Option<String>,
    /// Catalogue number.
    pub catalogue_number: Option<String>,
    /// Tracks per medium.
    pub media: Vec<MbMedium>,
}

/// A MusicBrainz medium.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MbMedium {
    /// Position (disc number).
    pub position: i32,
    /// Format string.
    pub format: Option<String>,
    /// Tracks.
    pub tracks: Vec<MbTrack>,
}

/// A MusicBrainz track.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MbTrack {
    /// Track number within the medium.
    pub number: i32,
    /// Track title.
    pub title: Option<String>,
    /// MusicBrainz track id.
    pub id: Option<String>,
    /// Recording id.
    pub recording_id: Option<String>,
    /// Recording ISRCs.
    pub isrcs: Vec<String>,
}

/// A barcode-search candidate (never auto-selected).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MbCandidate {
    /// Release id.
    pub release_id: Option<String>,
    /// Release-group id.
    pub release_group_id: Option<String>,
    /// Title.
    pub title: Option<String>,
    /// First credited artist.
    pub artist: Option<String>,
    /// Date (release date, else first-release date).
    pub date: Option<String>,
    /// Country.
    pub country: Option<String>,
    /// Barcode.
    pub barcode: Option<String>,
    /// Confidence of this candidate.
    pub confidence: Confidence,
}

/// Parses a release document (a release object, or an envelope whose
/// `releases[]` carries the first release).
pub fn parse_release(doc: &[u8]) -> Result<MbRelease> {
    let value = musicpack_core::json::parse(doc).map_err(|e| AuthorError::Identification {
        detail: format!("MusicBrainz response is not valid JSON: {e}"),
    })?;
    let release = select_release(&value).ok_or_else(|| AuthorError::Identification {
        detail: "MusicBrainz response contains no release".into(),
    })?;
    Ok(release_from_value(release))
}

/// Picks the release object: a top-level document with an `id`, else the
/// first element of `releases[]` (the reference's `select_release`).
fn select_release(value: &Value) -> Option<&Value> {
    if value.get("id").and_then(string_of).is_some() {
        return Some(value);
    }
    value
        .get("releases")
        .and_then(|r| match r {
            Value::Array(items) => items.first(),
            _ => None,
        })
        .filter(|r| r.is_object())
}

fn string_of(v: &Value) -> Option<&str> {
    match v {
        Value::String(s) => Some(s),
        _ => None,
    }
}

fn int_of(v: &Value) -> Option<i32> {
    match v {
        Value::Number(n) if n.is_finite() && n.fract() == 0.0 => Some(*n as i32),
        _ => None,
    }
}

fn string_list(v: &Value) -> Vec<String> {
    match v {
        Value::Array(items) => items
            .iter()
            .filter_map(|i| string_of(i).map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

fn release_from_value(release: &Value) -> MbRelease {
    let rg = release.get("release-group");
    let mut artists = Vec::new();
    let mut artist_ids = Vec::new();
    let mut artist_sort_names = Vec::new();
    if let Some(Value::Array(credits)) = release.get("artist-credit") {
        for credit in credits {
            if let Some(name) = credit.get("name").and_then(string_of) {
                artists.push(name.to_string());
            }
            if let Some(artist) = credit.get("artist") {
                artist_ids.push(
                    artist
                        .get("id")
                        .and_then(string_of)
                        .unwrap_or_default()
                        .to_string(),
                );
                artist_sort_names.push(
                    artist
                        .get("sort-name")
                        .and_then(string_of)
                        .unwrap_or_default()
                        .to_string(),
                );
            }
        }
    }
    let (label, catalogue_number) = match release.get("label-info") {
        Some(Value::Array(items)) => match items.first() {
            Some(info) => (
                info.get("label")
                    .and_then(|l| l.get("name"))
                    .and_then(string_of)
                    .map(str::to_string),
                info.get("catalog-number")
                    .and_then(string_of)
                    .map(str::to_string),
            ),
            None => (None, None),
        },
        _ => (None, None),
    };
    let media = match release.get("media") {
        Some(Value::Array(items)) => items
            .iter()
            .map(medium_from_value)
            .collect::<Vec<MbMedium>>(),
        _ => Vec::new(),
    };
    MbRelease {
        id: release.get("id").and_then(string_of).map(str::to_string),
        title: release.get("title").and_then(string_of).map(str::to_string),
        date: release.get("date").and_then(string_of).map(str::to_string),
        country: release
            .get("country")
            .and_then(string_of)
            .map(str::to_string),
        barcode: release
            .get("barcode")
            .and_then(string_of)
            .map(str::to_string),
        release_group_id: rg
            .and_then(|v| v.get("id"))
            .and_then(string_of)
            .map(str::to_string),
        primary_type: rg
            .and_then(|v| v.get("primary-type"))
            .and_then(string_of)
            .map(str::to_string),
        first_release_date: rg
            .and_then(|v| v.get("first-release-date"))
            .and_then(string_of)
            .map(str::to_string),
        artists,
        artist_ids,
        artist_sort_names,
        label,
        catalogue_number,
        media,
    }
}

fn medium_from_value(value: &Value) -> MbMedium {
    let tracks = match value.get("tracks") {
        Some(Value::Array(items)) => items.iter().map(track_from_value).collect(),
        _ => Vec::new(),
    };
    MbMedium {
        position: value.get("position").and_then(int_of).unwrap_or(0),
        format: value.get("format").and_then(string_of).map(str::to_string),
        tracks,
    }
}

fn track_from_value(value: &Value) -> MbTrack {
    let recording = value.get("recording");
    MbTrack {
        number: value.get("number").and_then(int_of).unwrap_or(0),
        title: value.get("title").and_then(string_of).map(str::to_string),
        id: value.get("id").and_then(string_of).map(str::to_string),
        recording_id: recording
            .and_then(|r| r.get("id"))
            .and_then(string_of)
            .map(str::to_string),
        isrcs: recording
            .and_then(|r| r.get("isrcs"))
            .map(string_list)
            .unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------
// matching
// ---------------------------------------------------------------------

/// Every ISRC on the draft's tracks (the reference's match inputs).
fn draft_isrcs(draft: &Draft) -> Vec<String> {
    let mut out = Vec::new();
    for disc in &draft.media {
        for track in &disc.tracks {
            if let Some(id) = &track.identifiers
                && let Some(isrc) = &id.isrc
                && !isrc.is_empty()
            {
                out.push(isrc.clone());
            }
        }
    }
    out
}

fn draft_track_count(draft: &Draft) -> usize {
    draft.media.iter().map(|d| d.tracks.len()).sum()
}

/// Scores a parsed release against the draft (reference ladder).
///
/// `asserted_mbid` is the id the caller asserts (`--mbid`); it participates
/// exactly as if the draft carried it.
pub fn match_confidence(
    draft: &Draft,
    release: &MbRelease,
    asserted_mbid: Option<&str>,
) -> Confidence {
    let draft_id = asserted_mbid
        .map(str::to_string)
        .or_else(|| draft.identifiers.as_ref()?.musicbrainz_release_id.clone());
    if let (Some(a), Some(b)) = (draft_id.as_deref(), release.id.as_deref())
        && a == b
    {
        return Confidence::Exact;
    }

    let barcode_hit = draft
        .identifiers
        .as_ref()
        .and_then(|i| i.barcode.as_deref())
        .zip(release.barcode.as_deref())
        .is_some_and(|(a, b)| !b.is_empty() && a == b);
    if barcode_hit {
        return Confidence::Confirmed;
    }

    let candidate_isrcs: Vec<String> = release
        .media
        .iter()
        .flat_map(|m| m.tracks.iter())
        .flat_map(|t| t.isrcs.iter().cloned())
        .collect();
    let isrc_hit = draft_isrcs(draft)
        .iter()
        .any(|isrc| candidate_isrcs.iter().any(|c| c == isrc));
    let candidate_tracks = release.media.iter().map(|m| m.tracks.len()).sum::<usize>();
    let count_ok = candidate_tracks > 0 && candidate_tracks == draft_track_count(draft);
    let title_match = release
        .title
        .as_deref()
        .zip(Some(draft.album.title.as_str()))
        .is_some_and(|(a, b)| !a.is_empty() && a == b);

    if isrc_hit && count_ok {
        Confidence::Confirmed
    } else if isrc_hit || title_match {
        Confidence::Probable
    } else {
        Confidence::None
    }
}

// ---------------------------------------------------------------------
// application
// ---------------------------------------------------------------------

/// Applies a matched release to the draft, filling **empty** fields only
/// (the reference's first-wins `apply_release`). Sets the identity
/// source/confidence always.
pub fn apply_release(draft: &mut Draft, release: &MbRelease, confidence: Confidence) {
    let ids = draft.identifiers.get_or_insert_with(Default::default);
    if ids.musicbrainz_release_group_id.is_none() {
        ids.musicbrainz_release_group_id = release.release_group_id.clone();
    }
    if ids.musicbrainz_release_id.is_none() {
        ids.musicbrainz_release_id = release.id.clone();
    }
    if ids.barcode.is_none() {
        ids.barcode = release.barcode.clone();
    }
    if let Some(r) = release
        .release_type()
        .and_then(|t| musicpack_core::format::manifest::ReleaseType::parse(&t))
        && draft.album.release_type.is_none()
    {
        draft.album.release_type = Some(r);
    }
    if draft.album.title.is_empty()
        && let Some(title) = &release.title
    {
        draft.album.title = title.clone();
    }
    if draft.album.original_release_date.is_none() {
        draft.album.original_release_date = release.first_release_date.clone();
    }
    if draft.album.artists.is_empty() {
        for (i, name) in release.artists.iter().enumerate() {
            if name.is_empty() {
                continue;
            }
            draft
                .album
                .artists
                .push(musicpack_core::format::manifest::Artist {
                    name: name.clone(),
                    role: None,
                    musicbrainz_id: release.artist_ids.get(i).filter(|s| !s.is_empty()).cloned(),
                    sort_name: release
                        .artist_sort_names
                        .get(i)
                        .filter(|s| !s.is_empty())
                        .cloned(),
                });
        }
    }
    let rel = draft.release.get_or_insert_with(Default::default);
    if rel.release_date.is_none() {
        rel.release_date = release.date.clone();
    }
    if rel.country.is_none() {
        rel.country = release.country.clone();
    }
    if rel.label.is_none() {
        rel.label = release.label.clone();
    }
    if rel.catalogue_number.is_none() {
        rel.catalogue_number = release.catalogue_number.clone();
    }

    // Per-track identifiers, matched by (disc position, track number).
    for disc in &mut draft.media {
        let Some(medium) = release.media.iter().find(|m| m.position == disc.number) else {
            continue;
        };
        // Medium format hint (reference fills an empty disc format).
        if disc.format.is_none()
            && let Some(format) = &medium.format
            && let Some(mf) = musicpack_core::format::manifest::MediumFormat::parse(format)
        {
            disc.format = Some(mf);
        }
        for track in &mut disc.tracks {
            let Some(candidate) = medium.tracks.iter().find(|t| t.number == track.number) else {
                continue;
            };
            let ids = track.identifiers.get_or_insert_with(Default::default);
            if ids.isrc.is_none() {
                ids.isrc = candidate.isrcs.first().cloned();
            }
            if ids.musicbrainz_track_id.is_none() {
                ids.musicbrainz_track_id = candidate.id.clone();
            }
            if ids.musicbrainz_recording_id.is_none() {
                ids.musicbrainz_recording_id = candidate.recording_id.clone();
            }
            if track.title.is_empty()
                && let Some(title) = &candidate.title
            {
                track.title = title.clone();
            }
        }
    }

    draft.identity = Some(musicpack_core::format::manifest::Identity {
        source: Some(musicpack_core::format::manifest::IdentitySource::MusicBrainz),
        confidence: Some(match confidence {
            Confidence::Exact => musicpack_core::format::manifest::IdentityConfidence::Exact,
            Confidence::Confirmed => {
                musicpack_core::format::manifest::IdentityConfidence::Confirmed
            }
            Confidence::Probable => musicpack_core::format::manifest::IdentityConfidence::Probable,
            Confidence::None => musicpack_core::format::manifest::IdentityConfidence::None,
        }),
    });
}

impl MbRelease {
    /// The release-group primary type, lowercased and hyphenated
    /// (`"Album"` → `"album"`), when it is a recognized kind.
    fn release_type(&self) -> Option<String> {
        let primary = self.primary_type.as_deref()?;
        Some(primary.to_lowercase().replace(' ', "-"))
    }
}

// ---------------------------------------------------------------------
// candidates + high-level entry points
// ---------------------------------------------------------------------

/// Applies an offline/live release document (`--mb-json`).
///
/// Returns the confidence and whether anything was applied; `none` applies
/// nothing (the reference's behavior).
pub fn identify_apply(
    draft: &mut Draft,
    doc: &[u8],
    asserted_mbid: Option<&str>,
) -> Result<(Confidence, bool)> {
    let release = parse_release(doc)?;
    let confidence = match_confidence(draft, &release, asserted_mbid);
    if confidence == Confidence::None {
        return Ok((confidence, false));
    }
    apply_release(draft, &release, confidence);
    Ok((confidence, true))
}

/// Fetches a release through the provider, then applies it.
pub fn identify_mbid(
    provider: &dyn MusicBrainzProvider,
    draft: &mut Draft,
    mbid: &str,
) -> Result<(Confidence, bool)> {
    let doc = provider
        .fetch_release(mbid)
        .map_err(|e| AuthorError::Identification {
            detail: format!("cannot fetch release '{mbid}': {e}"),
        })?;
    identify_apply(draft, &doc, Some(mbid))
}

/// Extracts barcode-search candidates (never auto-selects).
pub fn identify_candidates(draft: &Draft, search_doc: &[u8]) -> Result<Vec<MbCandidate>> {
    let value =
        musicpack_core::json::parse(search_doc).map_err(|e| AuthorError::Identification {
            detail: format!("MusicBrainz search response is not valid JSON: {e}"),
        })?;
    let items = match value.get("releases") {
        Some(Value::Array(items)) => items,
        _ => {
            return Err(AuthorError::Identification {
                detail: "MusicBrainz search response has no \"releases\" array".into(),
            });
        }
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let release = release_from_value(item);
        let confidence = match_confidence(draft, &release, None);
        out.push(MbCandidate {
            release_id: release.id.clone(),
            release_group_id: release.release_group_id.clone(),
            title: release.title.clone(),
            artist: release.artists.first().cloned(),
            date: release.date.clone().or(release.first_release_date.clone()),
            country: release.country.clone(),
            barcode: release.barcode.clone(),
            confidence,
        });
    }
    Ok(out)
}

/// Fetches a barcode search through the provider, then extracts candidates.
pub fn search_barcode(
    provider: &dyn MusicBrainzProvider,
    draft: &Draft,
    barcode: &str,
) -> Result<Vec<MbCandidate>> {
    let doc = provider
        .search_barcode(barcode)
        .map_err(|e| AuthorError::Identification {
            detail: format!("cannot search barcode '{barcode}': {e}"),
        })?;
    identify_candidates(draft, &doc)
}

// ---------------------------------------------------------------------
// JSON-preserving entry points (the Author host mutates draft JSON)
// ---------------------------------------------------------------------

/// Applies an identification document to draft **JSON**, preserving every
/// unrelated field. Returns the transformed JSON, the confidence and whether
/// anything was applied.
pub fn identify_apply_json(
    draft_json: &[u8],
    doc: &[u8],
    asserted_mbid: Option<&str>,
) -> Result<(String, Confidence, bool)> {
    let mut root = musicpack_core::json::parse(draft_json).map_err(|e| AuthorError::Draft {
        detail: format!("malformed draft JSON: {e}"),
    })?;
    let mut draft = crate::draft::parse(draft_json)?;
    let (confidence, applied) = identify_apply(&mut draft, doc, asserted_mbid)?;
    if applied {
        crate::inspect::sync_metadata(&mut root, &draft);
    }
    Ok((
        musicpack_core::json::print_canonical(&root),
        confidence,
        applied,
    ))
}

/// Fetches a release through the provider and applies it to draft JSON.
pub fn identify_mbid_json(
    provider: &dyn MusicBrainzProvider,
    draft_json: &[u8],
    mbid: &str,
) -> Result<(String, Confidence, bool)> {
    let doc = provider
        .fetch_release(mbid)
        .map_err(|e| AuthorError::Identification {
            detail: format!("cannot fetch release '{mbid}': {e}"),
        })?;
    identify_apply_json(draft_json, &doc, Some(mbid))
}

/// Extracts barcode-search candidates from draft JSON (never applies).
pub fn identify_candidates_json(draft_json: &[u8], search_doc: &[u8]) -> Result<Vec<MbCandidate>> {
    let draft = crate::draft::parse(draft_json)?;
    identify_candidates(&draft, search_doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draft;

    fn draft_from(json: &str) -> Draft {
        draft::parse(json.as_bytes()).unwrap()
    }

    const RELEASE: &str = r#"{
      "id": "aaaaaaaa-0000-0000-0000-000000000001",
      "title": "Test Album",
      "date": "2020-01-01",
      "country": "GB",
      "barcode": "1234567890123",
      "release-group": {"id": "bbbbbbbb-0000-0000-0000-000000000002", "primary-type": "Album", "first-release-date": "2019-12-01"},
      "artist-credit": [{"name": "Test Artist", "artist": {"id": "cccccccc-0000-0000-0000-000000000003", "sort-name": "Artist, Test"}}],
      "label-info": [{"label": {"name": "Test Label"}, "catalog-number": "CAT-1"}],
      "media": [{"position": 1, "format": "CD", "tracks": [
        {"id": "dddddddd-0000-0000-0000-000000000004", "number": 1, "title": "One", "recording": {"id": "eeeeeeee-0000-0000-0000-000000000005", "isrcs": ["USABC1234567"]}}
      ]}]
    }"#;

    fn minimal_draft() -> &'static str {
        r#"{"schema":"musicpack-draft","version":1,"sourceRoot":"/tmp","album":{"title":"Test Album","artists":[{"name":"Test Artist"}]},"media":[{"disc":1,"tracks":[{"track":1,"title":"One","audioPath":"a.flac"}]}]}"#
    }

    #[test]
    fn exact_on_release_id() {
        let d = draft_from(minimal_draft());
        let r = parse_release(RELEASE.as_bytes()).unwrap();
        assert_eq!(
            match_confidence(&d, &r, Some("aaaaaaaa-0000-0000-0000-000000000001")),
            Confidence::Exact
        );
    }

    #[test]
    fn confirmed_on_barcode_then_isrc() {
        let mut d = draft_from(minimal_draft());
        d.identifiers = Some(musicpack_core::format::manifest::Identifiers {
            musicbrainz_release_group_id: None,
            musicbrainz_release_id: None,
            barcode: Some("1234567890123".into()),
        });
        let r = parse_release(RELEASE.as_bytes()).unwrap();
        assert_eq!(match_confidence(&d, &r, None), Confidence::Confirmed);

        // ISRC hit + equal count is also confirmed.
        let mut d = draft_from(minimal_draft());
        d.media[0].tracks[0].identifiers =
            Some(musicpack_core::format::manifest::TrackIdentifiers {
                isrc: Some("USABC1234567".into()),
                musicbrainz_track_id: None,
                musicbrainz_recording_id: None,
            });
        assert_eq!(match_confidence(&d, &r, None), Confidence::Confirmed);
    }

    #[test]
    fn probable_on_title_only_and_none_otherwise() {
        let r = parse_release(RELEASE.as_bytes()).unwrap();
        // Title matches -> probable.
        let quiet = draft_from(minimal_draft());
        assert_eq!(match_confidence(&quiet, &r, None), Confidence::Probable);

        // Neither -> none.
        let mut d = draft_from(minimal_draft());
        d.album.title = "Different".into();
        assert_eq!(match_confidence(&d, &r, None), Confidence::None);
    }

    #[test]
    fn apply_fills_empty_fields_only() {
        let mut d = draft_from(minimal_draft());
        let r = parse_release(RELEASE.as_bytes()).unwrap();
        apply_release(&mut d, &r, Confidence::Probable);
        let ids = d.identifiers.as_ref().unwrap();
        assert_eq!(
            ids.musicbrainz_release_id.as_deref(),
            Some("aaaaaaaa-0000-0000-0000-000000000001")
        );
        assert_eq!(
            ids.musicbrainz_release_group_id.as_deref(),
            Some("bbbbbbbb-0000-0000-0000-000000000002")
        );
        assert_eq!(ids.barcode.as_deref(), Some("1234567890123"));
        assert_eq!(
            d.album.release_type,
            Some(musicpack_core::format::manifest::ReleaseType::Album)
        );
        assert_eq!(
            d.release.as_ref().unwrap().label.as_deref(),
            Some("Test Label")
        );
        assert_eq!(
            d.media[0].tracks[0]
                .identifiers
                .as_ref()
                .unwrap()
                .isrc
                .as_deref(),
            Some("USABC1234567")
        );
        // Non-empty album title is preserved.
        assert_eq!(d.album.title, "Test Album");
    }

    #[test]
    fn candidates_are_never_auto_selected() {
        let d = draft_from(minimal_draft());
        let search = format!(
            r#"{{"releases":[{}, {{"id":"ffffffff-0000-0000-0000-000000000009","title":"Other","media":[]}}]}}"#,
            RELEASE
        );
        let candidates = identify_candidates(&d, search.as_bytes()).unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].confidence, Confidence::Probable);
        assert_eq!(candidates[1].confidence, Confidence::None);
        assert_eq!(candidates[1].title.as_deref(), Some("Other"));
    }
}
