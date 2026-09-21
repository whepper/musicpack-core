//! The authoring draft: the **authored input** for the package builder.
//!
//! A draft describes what the author decided (metadata, structure, which
//! source files belong in the package and where they go). It deliberately
//! does **not** carry anything the package contract derives:
//!
//! | Derived by the builder | Why it is not in the draft |
//! |---|---|
//! | every asset `sha256` | computed from the bytes being packaged |
//! | waveform `points` | implied by the envelope payload length |
//! | track `duration` and `loudness` | measured from the primary audio |
//! | album `loudness` | measured as one concatenated program |
//! | package fingerprint / group / release keys | [`crate::identity`] over the built manifest |
//! | verification status | the shared core verifier's output |
//!
//! Only the metadata types the manifest already defines ([`Album`],
//! [`Release`], [`Artist`], …) are reused directly; the draft adds just the
//! source-file references the manifest cannot express. There is no second
//! lyrics model: a draft lyric reference becomes a manifest
//! [`LyricsRef`](crate::format::manifest::LyricsRef) via
//! [`crate::format::checksum`] and the existing layout rules.
//!
//! Source paths are interpreted relative to the builder's `source_root`
//! and must stay inside it (absolute paths and `..` escapes are rejected).
//! Package paths are canonical package-relative paths validated by
//! [`crate::format::path`].

use crate::format::manifest::{
    Album, Identifiers, Identity, Provenance, Release, Source, SourceAudio, TrackIdentifiers,
    TrackSource,
};

/// A source file materialized into the package at a canonical path.
///
/// `path` is the package-relative destination the manifest will reference;
/// `source` is the file to copy, relative to the builder's source root.
///
/// The builder copies the bytes verbatim and derives the `sha256` — the
/// caller never supplies a hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftAsset {
    /// Canonical package-relative destination path.
    pub path: String,
    /// Source file, relative to the builder's source root.
    pub source: String,
}

impl DraftAsset {
    /// Convenience constructor.
    pub fn new(path: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            source: source.into(),
        }
    }
}

/// A per-track lyrics reference (the manifest's [`LyricsRef`](crate::format::manifest::LyricsRef) minus the derived hash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftLyrics {
    /// Canonical package-relative destination path.
    pub path: String,
    /// Source `.lrc` file, relative to the builder's source root.
    pub source: String,
    /// Optional language tag (BCP-47 recommended, free-form, non-empty,
    /// no control characters — `docs/musicpack-lyrics-v1.md` §5).
    pub lang: Option<String>,
}

impl DraftLyrics {
    /// Convenience constructor.
    pub fn new(path: impl Into<String>, source: impl Into<String>, lang: Option<&str>) -> Self {
        Self {
            path: path.into(),
            source: source.into(),
            lang: lang.map(str::to_string),
        }
    }
}

/// An alternate audio representation (the manifest's
/// [`Representation`](crate::format::manifest::Representation) minus the derived hash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftRepresentation {
    /// Canonical package-relative destination path.
    pub path: String,
    /// Source audio file, relative to the builder's source root.
    pub source: String,
    /// Optional display label.
    pub label: Option<String>,
    /// Optional codec hint.
    pub codec: Option<String>,
}

/// A role-tagged artwork entry (the manifest's
/// [`Artwork`](crate::format::manifest::Artwork) minus the derived hash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftArtwork {
    /// Required role (`"front"`, `"back"`, …).
    pub role: String,
    /// The referenced image.
    pub asset: DraftAsset,
}

/// A waveform envelope payload to place in the package (the manifest's
/// [`WaveformRef`](crate::format::manifest::WaveformRef) minus the derived
/// hash and point count).
///
/// The builder materializes the payload and derives `points` from its
/// length; it never synthesizes the envelope (waveform generation is the
/// authoring pipeline's job, R4.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftWaveform {
    /// Canonical package-relative destination path
    /// (conventionally `analysis/waveform/<disc:02>-<track:02>.wfm`).
    pub path: String,
    /// Source envelope payload, relative to the builder's source root.
    pub source: String,
}

/// An analysis document reference (the manifest's
/// [`Analysis`](crate::format::manifest::Analysis) minus the derived hash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftAnalysis {
    /// Required analysis kind (`"sonic"` is the v1 type).
    pub kind: String,
    /// Required when `kind == "sonic"`; the sonic profile id.
    pub profile: Option<String>,
    /// The referenced document.
    pub asset: DraftAsset,
}

/// A track: authored metadata plus its audio and optional assets.
#[derive(Debug, Clone, PartialEq)]
pub struct DraftTrack {
    /// Track number (≥ 1, unique within the disc).
    pub number: i32,
    /// Required title.
    pub title: String,
    /// Optional per-track credit override (empty = absent).
    pub artists: Vec<crate::format::manifest::Artist>,
    /// Optional per-track identifiers.
    pub identifiers: Option<TrackIdentifiers>,
    /// Optional per-track source.
    pub source: Option<TrackSource>,
    /// Optional pre-encoding source reference.
    pub source_audio: Option<SourceAudio>,
    /// Optional encoded-audio codec hint.
    pub audio_codec: Option<String>,
    /// The primary audio object (source + destination).
    pub audio: DraftAsset,
    /// Optional waveform envelope payload.
    pub waveform: Option<DraftWaveform>,
    /// Optional per-track lyrics references.
    pub lyrics: Vec<DraftLyrics>,
    /// Optional alternate representations.
    pub representations: Vec<DraftRepresentation>,
}

/// A disc/medium: authored metadata plus its tracks.
#[derive(Debug, Clone, PartialEq)]
pub struct DraftDisc {
    /// Disc number (≥ 1, unique across the package).
    pub number: i32,
    /// Optional medium format.
    pub format: Option<crate::format::manifest::MediumFormat>,
    /// Optional medium title.
    pub title: Option<String>,
    /// Required non-empty track list.
    pub tracks: Vec<DraftTrack>,
}

/// The complete authoring draft for one release.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthoringDraft {
    /// Release-group level metadata.
    pub album: Album,
    /// Optional specific release/edition.
    pub release: Option<Release>,
    /// Optional release-level identifiers.
    pub identifiers: Option<Identifiers>,
    /// Optional identity match metadata.
    pub identity: Option<Identity>,
    /// Optional audio source metadata.
    pub source: Option<Source>,
    /// Required non-empty disc list.
    pub media: Vec<DraftDisc>,
    /// Optional artwork entries.
    pub artwork: Vec<DraftArtwork>,
    /// Optional booklet documents.
    pub booklet: Vec<DraftAsset>,
    /// Optional root-level lyrics documents.
    pub lyrics: Vec<DraftAsset>,
    /// Optional extras.
    pub extras: Vec<DraftAsset>,
    /// Optional analysis document references.
    pub analysis: Vec<DraftAnalysis>,
    /// Optional provenance.
    pub provenance: Option<Provenance>,
}
