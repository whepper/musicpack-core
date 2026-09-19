//! `.mpack` v1 manifest constants, closed enumerations, model, parser, and
//! canonical writer.
//!
//! Scaffold for the manifest model (`musicpack-v1.md` §3, machine-readable
//! schema `specs/musicpack-v1.schema.json`). The normative authority for
//! behaviour is the C implementation (`core/libmusicpack/src/manifest.c`);
//! the JSON dialect, field semantics, and canonical output are ports of it,
//! not schema-derived re-implementations.
//!
//! What lives here:
//!
//! - the format identity (`format`/`version` fields);
//! - the four **closed enumerations** (unknown values are parse errors,
//!   `other` is the designed escape hatch — never inferred, never widened);
//! - the loudness value bounds enforced by the C parser;
//! - the typed manifest model ([`Manifest`]);
//! - the strict parser ([`ParsedManifest::parse`]) and the canonical
//!   writer ([`ParsedManifest::write_canonical`]).
//!
//! Reference implementation: `core/libmusicpack/src/manifest.c`.

mod parse;
mod write;

pub use parse::ParsedManifest;
pub use write::format_number;

use crate::error::Error;

/// The manifest `format` field: literal `"musicpack"`.
pub const FORMAT_ID: &str = "musicpack";

/// The manifest `version` field: `1`. Unknown majors are rejected cleanly.
pub const SCHEMA_VERSION: u64 = 1;

/// Album release type: a **closed enumeration**
/// (`musicpack-v1.md` §3 "Release type"). Never inferred from track count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReleaseType {
    /// A long-play album.
    Album,
    /// An extended play.
    Ep,
    /// A single.
    Single,
    /// A maxi-single.
    MaxiSingle,
    /// A compilation.
    Compilation,
    /// A soundtrack.
    Soundtrack,
    /// A live album.
    LiveAlbum,
    /// A remix album.
    RemixAlbum,
    /// A box set.
    BoxSet,
    /// Anything else — the escape hatch.
    Other,
}

impl ReleaseType {
    /// The manifest string for this value.
    pub fn as_str(self) -> &'static str {
        match self {
            ReleaseType::Album => "album",
            ReleaseType::Ep => "ep",
            ReleaseType::Single => "single",
            ReleaseType::MaxiSingle => "maxi-single",
            ReleaseType::Compilation => "compilation",
            ReleaseType::Soundtrack => "soundtrack",
            ReleaseType::LiveAlbum => "live-album",
            ReleaseType::RemixAlbum => "remix-album",
            ReleaseType::BoxSet => "box-set",
            ReleaseType::Other => "other",
        }
    }

    /// Parses a manifest value. `None` means the value is outside the
    /// closed enumeration and the manifest must be rejected (mirrors the
    /// `bad-release-type` conformance case).
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "album" => ReleaseType::Album,
            "ep" => ReleaseType::Ep,
            "single" => ReleaseType::Single,
            "maxi-single" => ReleaseType::MaxiSingle,
            "compilation" => ReleaseType::Compilation,
            "soundtrack" => ReleaseType::Soundtrack,
            "live-album" => ReleaseType::LiveAlbum,
            "remix-album" => ReleaseType::RemixAlbum,
            "box-set" => ReleaseType::BoxSet,
            "other" => ReleaseType::Other,
            _ => return None,
        })
    }
}

/// Medium format: a **closed enumeration** (`musicpack-v1.md` §3 "Medium
/// format"). Which *medium* an entry is; deliberately separate from
/// `release.edition`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediumFormat {
    /// Compact disc.
    Cd,
    /// Super Audio CD.
    Sacd,
    /// Vinyl record.
    Vinyl,
    /// Cassette tape.
    Cassette,
    /// Digital medium.
    Digital,
    /// Blu-ray Audio disc.
    BluRayAudio,
    /// DVD-Audio disc.
    DvdAudio,
    /// Anything else.
    Other,
}

impl MediumFormat {
    /// The manifest string for this value.
    pub fn as_str(self) -> &'static str {
        match self {
            MediumFormat::Cd => "CD",
            MediumFormat::Sacd => "SACD",
            MediumFormat::Vinyl => "Vinyl",
            MediumFormat::Cassette => "Cassette",
            MediumFormat::Digital => "Digital",
            MediumFormat::BluRayAudio => "Blu-ray Audio",
            MediumFormat::DvdAudio => "DVD-Audio",
            MediumFormat::Other => "Other",
        }
    }

    /// Parses a manifest value; `None` means outside the closed enumeration
    /// (mirrors the `bad-medium-format` conformance case).
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "CD" => MediumFormat::Cd,
            "SACD" => MediumFormat::Sacd,
            "Vinyl" => MediumFormat::Vinyl,
            "Cassette" => MediumFormat::Cassette,
            "Digital" => MediumFormat::Digital,
            "Blu-ray Audio" => MediumFormat::BluRayAudio,
            "DVD-Audio" => MediumFormat::DvdAudio,
            "Other" => MediumFormat::Other,
            _ => return None,
        })
    }
}

/// How the manifest's durable identifiers were matched
/// (`musicpack-v1.md` §4 `identity.source`): a closed enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdentitySource {
    /// Matched against MusicBrainz.
    MusicBrainz,
    /// Matched against a store's catalogue.
    Store,
    /// Matched locally.
    Local,
}

impl IdentitySource {
    /// The manifest string for this value.
    pub fn as_str(self) -> &'static str {
        match self {
            IdentitySource::MusicBrainz => "musicbrainz",
            IdentitySource::Store => "store",
            IdentitySource::Local => "local",
        }
    }

    /// Parses a manifest value; `None` means outside the closed enumeration
    /// (mirrors the `bad-identity-enum` conformance case).
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "musicbrainz" => IdentitySource::MusicBrainz,
            "store" => IdentitySource::Store,
            "local" => IdentitySource::Local,
            _ => return None,
        })
    }
}

/// How confidently the identifiers were matched
/// (`musicpack-v1.md` §4 `identity.confidence`): a closed enumeration.
/// Fuzzy matches are recorded honestly; they are never promoted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdentityConfidence {
    /// Exact match.
    Exact,
    /// Confirmed match.
    Confirmed,
    /// Probable match.
    Probable,
    /// No match.
    None,
}

impl IdentityConfidence {
    /// The manifest string for this value.
    pub fn as_str(self) -> &'static str {
        match self {
            IdentityConfidence::Exact => "exact",
            IdentityConfidence::Confirmed => "confirmed",
            IdentityConfidence::Probable => "probable",
            IdentityConfidence::None => "none",
        }
    }

    /// Parses a manifest value; `None` means outside the closed enumeration.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "exact" => IdentityConfidence::Exact,
            "confirmed" => IdentityConfidence::Confirmed,
            "probable" => IdentityConfidence::Probable,
            "none" => IdentityConfidence::None,
            _ => return None,
        })
    }
}

/// Inclusive lower bound for a valid LUFS or dBTP loudness value
/// (`loudness.c`: `musicpack_loudness_validate_lufs` /
/// `musicpack_loudness_validate_true_peak`).
pub const LOUDNESS_MIN_DB: f64 = -70.0;

/// Inclusive upper bound for a valid LUFS or dBTP loudness value.
pub const LOUDNESS_MAX_DB: f64 = 6.0;

/// Returns `true` when `v` is a finite value inside the accepted loudness
/// range. Non-finite values are never accepted — JSON `1e999`-style
/// overflows must fail validation, not silently become infinity.
pub fn is_valid_loudness_value(v: f64) -> bool {
    v.is_finite() && (LOUDNESS_MIN_DB..=LOUDNESS_MAX_DB).contains(&v)
}

/// Parses a loudness value the way the C parser does: finite and in range.
pub fn parse_loudness_value(v: f64) -> Result<f64, Error> {
    if is_valid_loudness_value(v) {
        Ok(v)
    } else {
        Err(Error::Invalid {
            detail: format!(
                "loudness value {v} out of range [{LOUDNESS_MIN_DB}, {LOUDNESS_MAX_DB}]"
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_enums_round_trip() {
        let release_types = [
            ReleaseType::Album,
            ReleaseType::Ep,
            ReleaseType::Single,
            ReleaseType::MaxiSingle,
            ReleaseType::Compilation,
            ReleaseType::Soundtrack,
            ReleaseType::LiveAlbum,
            ReleaseType::RemixAlbum,
            ReleaseType::BoxSet,
            ReleaseType::Other,
        ];
        assert_eq!(release_types.len(), 10);
        for v in release_types {
            assert_eq!(ReleaseType::parse(v.as_str()), Some(v));
        }
        assert_eq!(ReleaseType::parse("mixtape"), None); // conformance: bad-release-type

        let media = [
            MediumFormat::Cd,
            MediumFormat::Sacd,
            MediumFormat::Vinyl,
            MediumFormat::Cassette,
            MediumFormat::Digital,
            MediumFormat::BluRayAudio,
            MediumFormat::DvdAudio,
            MediumFormat::Other,
        ];
        assert_eq!(media.len(), 8);
        for v in media {
            assert_eq!(MediumFormat::parse(v.as_str()), Some(v));
        }
        assert_eq!(MediumFormat::parse("DAT"), None); // conformance: bad-medium-format

        for v in [
            IdentitySource::MusicBrainz,
            IdentitySource::Store,
            IdentitySource::Local,
        ] {
            assert_eq!(IdentitySource::parse(v.as_str()), Some(v));
        }
        assert_eq!(IdentitySource::parse("bogus"), None); // conformance: bad-identity-enum

        for v in [
            IdentityConfidence::Exact,
            IdentityConfidence::Confirmed,
            IdentityConfidence::Probable,
            IdentityConfidence::None,
        ] {
            assert_eq!(IdentityConfidence::parse(v.as_str()), Some(v));
        }
        assert_eq!(IdentityConfidence::parse("maybe"), None);
    }

    #[test]
    fn format_identity_is_frozen() {
        assert_eq!(FORMAT_ID, "musicpack");
        assert_eq!(SCHEMA_VERSION, 1);
    }

    #[test]
    fn loudness_bounds_match_the_c_reference() {
        assert!(is_valid_loudness_value(-70.0));
        assert!(is_valid_loudness_value(6.0));
        assert!(is_valid_loudness_value(-16.0));
        assert!(!is_valid_loudness_value(-70.1));
        assert!(!is_valid_loudness_value(6.1));
        assert!(!is_valid_loudness_value(-9999.0)); // conformance: bad-loudness
        assert!(!is_valid_loudness_value(f64::NAN));
        assert!(!is_valid_loudness_value(f64::INFINITY));
        assert!(!is_valid_loudness_value(f64::NEG_INFINITY));
        assert!(parse_loudness_value(-7.19).is_ok());
        assert!(parse_loudness_value(f64::INFINITY).is_err());
    }
}

// ---------------------------------------------------------------------
// The manifest model
// ---------------------------------------------------------------------

impl Manifest {
    /// Validates the model's shape (re-parses its canonical tree); used by
    /// the canonical writer and by verification.
    pub fn validate(&self) -> Result<(), Error> {
        write::validate_for_write(self)
    }

    /// Every referenced asset path, in the reference's grouping order:
    /// per-track audio, waveform and representations; then artwork,
    /// booklet, lyrics, extras and analysis.
    ///
    /// Used for the unreferenced-file check and by future package
    /// consumers; paths are package-unique (enforced at parse time).
    pub fn referenced_paths(&self) -> Vec<&str> {
        let mut paths = Vec::new();
        for disc in &self.media {
            for track in &disc.tracks {
                paths.push(track.audio.path.as_str());
                if let Some(waveform) = &track.waveform {
                    paths.push(waveform.path.as_str());
                }
                for representation in &track.representations {
                    paths.push(representation.path.as_str());
                }
            }
        }
        for artwork in &self.artwork {
            paths.push(artwork.asset.path.as_str());
        }
        for asset in &self.booklet {
            paths.push(asset.path.as_str());
        }
        for asset in &self.lyrics {
            paths.push(asset.path.as_str());
        }
        for asset in &self.extras {
            paths.push(asset.path.as_str());
        }
        for analysis in &self.analysis {
            paths.push(analysis.asset.path.as_str());
        }
        paths
    }
}

/// A referenced object in the package: package-relative path + required
/// lowercase SHA-256 hex.
#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    /// Canonical package-relative path.
    pub path: String,
    /// 64 lowercase hex characters.
    pub sha256: String,
}

/// A named credit (`album.artists[]` / `media[].tracks[].artists[]`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Artist {
    /// Required credited name.
    pub name: String,
    /// Optional role (`"main"`, `"featuring"`, ...).
    pub role: Option<String>,
    /// Optional MusicBrainz artist id (identity hint only).
    pub musicbrainz_id: Option<String>,
    /// Optional sort name.
    pub sort_name: Option<String>,
}

/// The release-group level: what album this package belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct Album {
    /// Required album (release group) title.
    pub title: String,
    /// Required, non-empty credit list.
    pub artists: Vec<Artist>,
    /// Optional closed-enum release type.
    pub release_type: Option<ReleaseType>,
    /// Optional ISO-8601 first-release date of the release group.
    pub original_release_date: Option<String>,
    /// Optional genre list (absent and empty are equivalent on write).
    pub genres: Vec<String>,
}

/// The specific release/edition this package represents.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Release {
    /// Optional ISO-8601 release date.
    pub release_date: Option<String>,
    /// Optional edition name.
    pub edition: Option<String>,
    /// Optional country (ISO 3166-1 alpha-2 recommended).
    pub country: Option<String>,
    /// Optional label.
    pub label: Option<String>,
    /// Optional catalogue number.
    pub catalogue_number: Option<String>,
    /// Optional edition/release notes.
    pub notes: Option<String>,
}

impl Release {
    /// `true` when any field is set (the object is only emitted then).
    pub fn is_present(&self) -> bool {
        self.release_date.is_some()
            || self.edition.is_some()
            || self.country.is_some()
            || self.label.is_some()
            || self.catalogue_number.is_some()
            || self.notes.is_some()
    }
}

/// Durable release-level identifiers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Identifiers {
    /// Release-group identity.
    pub musicbrainz_release_group_id: Option<String>,
    /// Specific-release identity.
    pub musicbrainz_release_id: Option<String>,
    /// Barcode.
    pub barcode: Option<String>,
}

impl Identifiers {
    /// `true` when any field is set (the object is only emitted then).
    pub fn is_present(&self) -> bool {
        self.musicbrainz_release_group_id.is_some()
            || self.musicbrainz_release_id.is_some()
            || self.barcode.is_some()
    }
}

/// How the identifiers were matched (`identity`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Identity {
    /// Optional closed-enum source.
    pub source: Option<IdentitySource>,
    /// Optional closed-enum confidence.
    pub confidence: Option<IdentityConfidence>,
}

impl Identity {
    /// `true` when any field is set (the object is only emitted then).
    pub fn is_present(&self) -> bool {
        self.source.is_some() || self.confidence.is_some()
    }
}

/// Where the audio came from (`source`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Source {
    /// Optional free-text type (`cd-rip`, `digital-download`, ...).
    pub kind: Option<String>,
    /// Optional store name.
    pub store: Option<String>,
    /// Optional provider release id.
    pub id: Option<String>,
}

impl Source {
    /// `true` when any field is set (the object is only emitted then).
    pub fn is_present(&self) -> bool {
        self.kind.is_some() || self.store.is_some() || self.id.is_some()
    }
}

/// A disc / medium.
#[derive(Debug, Clone, PartialEq)]
pub struct Disc {
    /// Required disc number (≥ 1, unique across the package).
    pub number: i32,
    /// Optional closed-enum medium format.
    pub format: Option<MediumFormat>,
    /// Optional medium title.
    pub title: Option<String>,
    /// Required, non-empty track list.
    pub tracks: Vec<Track>,
}

/// Per-track identifier block.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TrackIdentifiers {
    /// ISRC.
    pub isrc: Option<String>,
    /// MusicBrainz track id.
    pub musicbrainz_track_id: Option<String>,
    /// MusicBrainz recording id.
    pub musicbrainz_recording_id: Option<String>,
}

impl TrackIdentifiers {
    /// `true` when any field is set (the object is only emitted then).
    pub fn is_present(&self) -> bool {
        self.isrc.is_some()
            || self.musicbrainz_track_id.is_some()
            || self.musicbrainz_recording_id.is_some()
    }
}

/// Per-track source block (where this track's audio came from).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TrackSource {
    /// Optional store name.
    pub store: Option<String>,
    /// Optional provider track id.
    pub track_id: Option<String>,
}

impl TrackSource {
    /// `true` when any field is set (the object is only emitted then).
    pub fn is_present(&self) -> bool {
        self.store.is_some() || self.track_id.is_some()
    }
}

/// Per-track pre-encoding source reference (`sourceAudio`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceAudio {
    /// Optional pre-encoding codec (`flac`, ...).
    pub codec: Option<String>,
    /// Optional pre-encoding source hash.
    pub md5: Option<String>,
}

impl SourceAudio {
    /// `true` when any field is set (the object is only emitted then).
    pub fn is_present(&self) -> bool {
        self.codec.is_some() || self.md5.is_some()
    }
}

/// Measured BS.1770 loudness for a track. Gain is derived, never stored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    /// Integrated loudness, dB LUFS (validated range).
    pub lufs: f64,
    /// True peak, dBTP (validated range).
    pub true_peak_db: f64,
}

/// Album-level loudness (measured as one concatenated program).
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumLoudness {
    /// Optional BS.1770 revision string (canonical: "ITU-R BS.1770-5").
    pub algorithm: Option<String>,
    /// Integrated album loudness, dB LUFS.
    pub lufs: f64,
    /// Album true peak (max across tracks), dBTP.
    pub true_peak_db: f64,
}

/// Per-track waveform envelope reference (`specs/musicpack-waveform-v1.md`).
///
/// The v1 closed enums (`version=1`, `intervalMs=100`,
/// `encoding="peak-rms-u8"`, `floorDb=-60`) are frozen and therefore not
/// represented as fields; only the variable parts are.
#[derive(Debug, Clone, PartialEq)]
pub struct WaveformRef {
    /// Canonical package-relative path (unique across the package).
    pub path: String,
    /// 64 lowercase hex characters.
    pub sha256: String,
    /// Bucket count, including the final partial bucket (≤ 864000).
    pub points: u64,
}

/// An alternate audio representation for a track (`representations[]`).
#[derive(Debug, Clone, PartialEq)]
pub struct Representation {
    /// Canonical package-relative path (unique across the package).
    pub path: String,
    /// 64 lowercase hex characters.
    pub sha256: String,
    /// Optional display label (`"FLAC 24/96"`).
    pub label: Option<String>,
    /// Optional codec hint (`"flac"`, ...).
    pub codec: Option<String>,
}

/// A referenced analysis document (`analysis[]`).
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    /// Required analysis kind (`"sonic"` is the v1 type; unknown types are
    /// forward-compatible and structurally validated only).
    pub kind: String,
    /// Required when `kind == "sonic"`; the sonic profile id.
    pub profile: Option<String>,
    /// The referenced document.
    pub asset: Asset,
}

/// Provenance of the package itself (`provenance`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Provenance {
    /// Optional authoring tool name.
    pub tool: Option<String>,
    /// Optional tool version.
    pub tool_version: Option<String>,
}

impl Provenance {
    /// `true` when any field is set (the object is only emitted then).
    pub fn is_present(&self) -> bool {
        self.tool.is_some() || self.tool_version.is_some()
    }
}

/// A track on a disc.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    /// Required track number (≥ 1, unique within its disc).
    pub number: i32,
    /// Required title.
    pub title: String,
    /// Optional per-track credit override (empty = absent).
    pub artists: Vec<Artist>,
    /// Optional per-track identifiers.
    pub identifiers: Option<TrackIdentifiers>,
    /// Optional per-track source.
    pub source: Option<TrackSource>,
    /// Optional pre-encoding source reference.
    pub source_audio: Option<SourceAudio>,
    /// Optional duration in seconds (derived, not canonical; > 0).
    pub duration: Option<f64>,
    /// Optional measured loudness (both values or none).
    pub loudness: Option<Loudness>,
    /// The primary audio object.
    pub audio: Asset,
    /// Optional encoded-audio codec hint.
    pub audio_codec: Option<String>,
    /// Optional waveform envelope reference.
    pub waveform: Option<WaveformRef>,
    /// Optional alternate representations (empty = absent).
    pub representations: Vec<Representation>,
}

/// The parsed `.mpack` v1 manifest (storage-independent logical model).
///
/// Field semantics mirror `musicpack_manifest` in the C reference; the
/// `Option`/`is_present` conventions preserve the C writer's behaviour
/// exactly: an object is emitted only when at least one of its fields is
/// set, and absent and empty arrays are equivalent on write.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// Required release-group level album.
    pub album: Album,
    /// Optional specific release/edition.
    pub release: Option<Release>,
    /// Optional release-level identifiers.
    pub identifiers: Option<Identifiers>,
    /// Optional identity (how ids were matched).
    pub identity: Option<Identity>,
    /// Optional source (where the audio came from).
    pub source: Option<Source>,
    /// Required, non-empty disc list.
    pub media: Vec<Disc>,
    /// Optional artwork entries (role-tagged).
    pub artwork: Vec<Artwork>,
    /// Optional booklet documents.
    pub booklet: Vec<Asset>,
    /// Optional lyrics documents.
    pub lyrics: Vec<Asset>,
    /// Optional extras (opaque data).
    pub extras: Vec<Asset>,
    /// Optional analysis document references.
    pub analysis: Vec<Analysis>,
    /// Optional album loudness (present only with both measured values).
    pub loudness: Option<AlbumLoudness>,
    /// Optional provenance.
    pub provenance: Option<Provenance>,
}

/// Artwork with a role tag (`"front"`, `"back"`, ...).
#[derive(Debug, Clone, PartialEq)]
pub struct Artwork {
    /// Required role.
    pub role: String,
    /// The referenced image.
    pub asset: Asset,
}
