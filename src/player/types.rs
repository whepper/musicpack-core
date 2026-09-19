//! Platform-independent playback types.
//!
//! Port of `web/player-core/src/types.ts` (BSD-3-Clause). JSON-representable
//! plain data only: no handles, no closures, no audio buffers.

use crate::json::Value;

/// Where an engine obtains the bytes of one item.
///
/// `HttpRange` is the demand-driven range source, `Stream` covers
/// element/native playback, and `LocalFile` addresses an offline-held asset.
/// Engines resolve each kind at their own boundary; the core never does.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SourceKind {
    /// `"http-range"`.
    HttpRange,
    /// `"stream"`.
    Stream,
    /// `"local-file"`.
    LocalFile,
    /// Any other host-defined kind, preserved verbatim.
    Other(String),
}

impl SourceKind {
    /// The wire string.
    pub fn as_str(&self) -> &str {
        match self {
            SourceKind::HttpRange => "http-range",
            SourceKind::Stream => "stream",
            SourceKind::LocalFile => "local-file",
            SourceKind::Other(s) => s,
        }
    }

    /// Parses a wire string (any string is accepted, unknown → `Other`).
    pub fn parse(s: &str) -> Self {
        match s {
            "http-range" => SourceKind::HttpRange,
            "stream" => SourceKind::Stream,
            "local-file" => SourceKind::LocalFile,
            other => SourceKind::Other(other.to_string()),
        }
    }
}

/// A source descriptor (`PlaybackSource`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackSource {
    /// Source kind.
    pub kind: SourceKind,
    /// URL / stable key.
    pub url: String,
    /// Optional byte size hint.
    pub byte_size: Option<u64>,
}

/// A platform-independent queue entry (`PlaybackItem`).
///
/// All TypeScript fields are represented. Host-specific passthrough fields
/// (e.g. the web's nested `track` object, `releaseId`) are preserved in
/// [`PlaybackItem::extra`] so the player stays representation-blind and the
/// session snapshot stays shape-compatible.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackItem {
    /// Stable per queue entry; the identity commands/events use.
    pub id: String,
    /// Server track-row identity (key of the exact-length cache).
    pub track_id: i64,
    /// Where the engine gets the bytes.
    pub source: PlaybackSource,
    /// Manifest duration hint in seconds; exact decoded length overrides it.
    pub duration_hint_seconds: Option<f64>,
    /// Display title.
    pub title: String,
    /// Display artist.
    pub artist: String,
    /// Display album title.
    pub album_title: String,
    /// Optional edition.
    pub edition: Option<String>,
    /// Optional artwork URL.
    pub artwork_url: Option<String>,
    /// BS.1770 track loudness.
    pub loudness: Option<TrackLoudness>,
    /// BS.1770 album loudness.
    pub album_loudness: Option<AlbumLoudness>,
    /// Codec hint.
    pub codec: Option<String>,
    /// MIME type hint.
    pub mime_type: Option<String>,
    /// Host passthrough fields, in document order.
    pub extra: Vec<(String, Value)>,
}

/// Track loudness (`{ lufs, truePeakDb }`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackLoudness {
    /// Integrated loudness in LUFS.
    pub lufs: f64,
    /// True peak in dBTP.
    pub true_peak_db: f64,
}

/// Album loudness (`{ albumLufs, albumTruePeakDb }`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlbumLoudness {
    /// Album integrated loudness in LUFS.
    pub album_lufs: f64,
    /// Album true peak in dBTP.
    pub album_true_peak_db: f64,
}

/// Engine stream facts (`StreamInfo`), in the engine's OUTPUT timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamInfo {
    /// Output sample rate (Hz).
    pub rate: u32,
    /// Channel count.
    pub channels: u32,
    /// Codec stream version (0 when unknown).
    pub version: u32,
    /// Exact decoded length in output-rate samples.
    pub length_samples: u64,
}

/// Engine taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EngineKind {
    /// The Musepack pipeline (range/stream/local).
    Musepack,
    /// A browser/native element-backed codec.
    Native,
}

/// Queue repeat policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatMode {
    /// Stop at the end.
    Off,
    /// Reload the current item (resolved by the player, not the queue).
    One,
    /// Wrap to the start.
    All,
}

impl RepeatMode {
    /// Wire string.
    pub fn as_str(&self) -> &'static str {
        match self {
            RepeatMode::Off => "off",
            RepeatMode::One => "one",
            RepeatMode::All => "all",
        }
    }

    /// Parses a wire string; invalid values → `Off`.
    pub fn parse(s: &str) -> Self {
        match s {
            "one" => RepeatMode::One,
            "all" => RepeatMode::All,
            _ => RepeatMode::Off,
        }
    }
}

/// Playback normalization mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalizationMode {
    /// No normalization.
    Off,
    /// Use album loudness.
    Album,
    /// Use track loudness.
    Track,
}

impl NormalizationMode {
    /// Wire string.
    pub fn as_str(&self) -> &'static str {
        match self {
            NormalizationMode::Off => "off",
            NormalizationMode::Album => "album",
            NormalizationMode::Track => "track",
        }
    }

    /// Parses a wire string; invalid values → `Album`.
    pub fn parse(s: &str) -> Self {
        match s {
            "off" => NormalizationMode::Off,
            "track" => NormalizationMode::Track,
            _ => NormalizationMode::Album,
        }
    }
}

/// Queue-entry identity: two entries describe the same playable item iff
/// both the queue-entry id and the track id match.
pub fn same_item_identity(a: &PlaybackItem, b: &PlaybackItem) -> bool {
    a.id == b.id && a.track_id == b.track_id
}

/// Identity helper: stable per-queue-entry key.
pub fn item_key(item: &PlaybackItem) -> String {
    format!("{}@{}", item.id, item.track_id)
}

const KNOWN_KEYS: &[&str] = &[
    "id",
    "trackId",
    "source",
    "durationHintSeconds",
    "title",
    "artist",
    "albumTitle",
    "edition",
    "artworkUrl",
    "loudness",
    "albumLoudness",
    "codec",
    "mimeType",
];

fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    match v.get(key) {
        Some(Value::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

fn num_at(v: &Value, key: &str) -> Option<f64> {
    match v.get(key) {
        Some(Value::Number(n)) if n.is_finite() => Some(*n),
        _ => None,
    }
}

impl PlaybackItem {
    /// Parses a JSON object into a typed item, or `None` when it does not
    /// satisfy the restore-eligibility contract (mirrors `isRestorableItem`):
    /// non-empty string `id`, `source.kind` string, non-empty `source.url`
    /// string, numeric `track.id`, and truthy `track.audio.url`.
    pub fn from_value(v: &Value) -> Option<PlaybackItem> {
        let id = str_at(v, "id")?;
        if id.is_empty() {
            return None;
        }
        let source = v.get("source")?;
        let kind = str_at(source, "kind")?;
        let url = str_at(source, "url")?;
        if url.is_empty() {
            return None;
        }
        let track = v.get("track")?;
        let track_id = num_at(track, "id")?;
        let audio_url = track.get("audio").and_then(|audio| str_at(audio, "url"))?;
        if audio_url.is_empty() {
            return None;
        }
        let track_id = if track_id.is_finite() && track_id.fract() == 0.0 {
            track_id as i64
        } else {
            return None;
        };

        let source = PlaybackSource {
            kind: SourceKind::parse(kind),
            url: url.to_string(),
            byte_size: num_at(source, "byteSize")
                .filter(|n| n.is_finite() && *n >= 0.0)
                .map(|n| n as u64),
        };
        let loudness = parse_track_loudness(v.get("loudness"));
        let album_loudness = parse_album_loudness(v.get("albumLoudness"));

        let mut extra: Vec<(String, Value)> = Vec::new();
        if let Value::Object(members) = v {
            for (k, val) in members {
                if !KNOWN_KEYS.contains(&k.as_str()) {
                    extra.push((k.clone(), val.clone()));
                }
            }
        }

        Some(PlaybackItem {
            id: id.to_string(),
            track_id,
            source,
            duration_hint_seconds: num_at(v, "durationHintSeconds"),
            title: str_at(v, "title").unwrap_or_default().to_string(),
            artist: str_at(v, "artist").unwrap_or_default().to_string(),
            album_title: str_at(v, "albumTitle").unwrap_or_default().to_string(),
            edition: str_at(v, "edition").map(str::to_string),
            artwork_url: str_at(v, "artworkUrl").map(str::to_string),
            loudness,
            album_loudness,
            codec: str_at(v, "codec").map(str::to_string),
            mime_type: str_at(v, "mimeType").map(str::to_string),
            extra,
        })
    }

    /// Serializes the item back to its JSON object shape.
    ///
    /// A nested `track` object is synthesized when the host did not supply
    /// one, so snapshot restore eligibility holds for Rust-native hosts.
    pub fn to_value(&self) -> Value {
        let mut members: Vec<(String, Value)> = Vec::new();
        members.push(("id".into(), Value::String(self.id.clone())));
        members.push(("trackId".into(), Value::Number(self.track_id as f64)));
        let mut source = vec![
            (
                "kind".into(),
                Value::String(self.source.kind.as_str().to_string()),
            ),
            ("url".into(), Value::String(self.source.url.clone())),
        ];
        if let Some(n) = self.source.byte_size {
            source.push(("byteSize".into(), Value::Number(n as f64)));
        }
        members.push(("source".into(), Value::Object(source)));
        if let Some(h) = self.duration_hint_seconds {
            members.push(("durationHintSeconds".into(), Value::Number(h)));
        }
        members.push(("title".into(), Value::String(self.title.clone())));
        members.push(("artist".into(), Value::String(self.artist.clone())));
        members.push(("albumTitle".into(), Value::String(self.album_title.clone())));
        if let Some(e) = &self.edition {
            members.push(("edition".into(), Value::String(e.clone())));
        }
        if let Some(a) = &self.artwork_url {
            members.push(("artworkUrl".into(), Value::String(a.clone())));
        }
        if let Some(l) = &self.loudness {
            members.push((
                "loudness".into(),
                Value::Object(vec![
                    ("lufs".into(), Value::Number(l.lufs)),
                    ("truePeakDb".into(), Value::Number(l.true_peak_db)),
                ]),
            ));
        }
        if let Some(l) = &self.album_loudness {
            members.push((
                "albumLoudness".into(),
                Value::Object(vec![
                    ("albumLufs".into(), Value::Number(l.album_lufs)),
                    (
                        "albumTruePeakDb".into(),
                        Value::Number(l.album_true_peak_db),
                    ),
                ]),
            ));
        }
        if let Some(c) = &self.codec {
            members.push(("codec".into(), Value::String(c.clone())));
        }
        if let Some(m) = &self.mime_type {
            members.push(("mimeType".into(), Value::String(m.clone())));
        }
        let mut has_track = false;
        for (k, v) in &self.extra {
            if k == "track" {
                has_track = true;
            }
            // Never let a passthrough duplicate a known core key.
            if KNOWN_KEYS.contains(&k.as_str()) {
                continue;
            }
            members.push((k.clone(), v.clone()));
        }
        if !has_track {
            members.push((
                "track".into(),
                Value::Object(vec![
                    ("id".into(), Value::Number(self.track_id as f64)),
                    (
                        "audio".into(),
                        Value::Object(vec![("url".into(), Value::String(self.source.url.clone()))]),
                    ),
                ]),
            ));
        }
        Value::Object(members)
    }
}

fn parse_track_loudness(v: Option<&Value>) -> Option<TrackLoudness> {
    let v = v?;
    let lufs = num_at(v, "lufs")?;
    let true_peak_db = num_at(v, "truePeakDb")?;
    Some(TrackLoudness { lufs, true_peak_db })
}

fn parse_album_loudness(v: Option<&Value>) -> Option<AlbumLoudness> {
    let v = v?;
    let album_lufs = num_at(v, "albumLufs")?;
    let album_true_peak_db = num_at(v, "albumTruePeakDb")?;
    Some(AlbumLoudness {
        album_lufs,
        album_true_peak_db,
    })
}
