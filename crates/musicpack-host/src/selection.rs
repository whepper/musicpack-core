//! Representation-aware source selection for a host.
//!
//! The pure rules live in [`musicpack_core::policy`]; this module is the host
//! mapping from that decision to a concrete source URL/size — the same seam
//! the reference's `itemForTrack()` provides. It performs no I/O and no
//! network access: it only labels already-known candidates.

use musicpack_core::policy::{self, AudioPreference, Playability, TrackAudio};

/// A source the host could play for a track.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceCandidate {
    /// Representation id. Ignored for the primary (set anything, usually 0).
    pub id: f64,
    /// Source URL (HTTP range, local file key, package member path, ...).
    pub url: String,
    /// Declared byte size, when known.
    pub byte_size: Option<u64>,
    /// Codec family hint.
    pub codec: Option<String>,
    /// MIME type hint.
    pub mime_type: Option<String>,
}

/// The host's playable sources for one track: the primary plus alternates, in
/// manifest order.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackSources {
    /// The track's primary audio.
    pub primary: SourceCandidate,
    /// Alternate representations, in manifest order.
    pub representations: Vec<SourceCandidate>,
}

/// The chosen source and its representation identity (`None` = primary).
#[derive(Debug, Clone, PartialEq)]
pub struct SelectedSource {
    /// The chosen representation id, or `None` for the primary.
    pub representation_id: Option<f64>,
    /// The chosen source URL.
    pub url: String,
    /// The chosen source's declared byte size.
    pub byte_size: Option<u64>,
    /// The chosen source's codec hint.
    pub codec: Option<String>,
    /// The chosen source's MIME type hint.
    pub mime_type: Option<String>,
}

impl SelectedSource {
    /// The representation-aware playback-item identity (`t{track}r{rep}` or
    /// `t{track}`), matching the reference's Phase 4 contract.
    pub fn item_id(&self, track_id: i64) -> String {
        policy::item_id(track_id, self.representation_id)
    }
}

impl TrackSources {
    /// The policy-shaped view (codec/mime/id only).
    pub fn policy_input(&self) -> TrackAudio {
        TrackAudio {
            codec: self.primary.codec.clone(),
            mime_type: self.primary.mime_type.clone(),
            representations: self
                .representations
                .iter()
                .map(|r| policy::RepresentationRef {
                    id: r.id,
                    codec: r.codec.clone(),
                    mime_type: r.mime_type.clone(),
                })
                .collect(),
        }
    }

    fn candidate(&self, index: usize) -> &SourceCandidate {
        &self.representations[index]
    }

    /// Selects the source with [`musicpack_core::policy::resolve_audio`].
    pub fn select<P: Playability + ?Sized>(
        &self,
        pref: Option<&AudioPreference>,
        can_play: &P,
    ) -> SelectedSource {
        let track = self.policy_input();
        let selected = policy::resolve_audio(&track, pref, can_play);
        match selected.representation {
            None => SelectedSource {
                representation_id: None,
                url: self.primary.url.clone(),
                byte_size: self.primary.byte_size,
                codec: self.primary.codec.clone(),
                mime_type: self.primary.mime_type.clone(),
            },
            Some(index) => {
                let chosen = self.candidate(index);
                SelectedSource {
                    representation_id: Some(chosen.id),
                    url: chosen.url.clone(),
                    byte_size: chosen.byte_size,
                    codec: chosen.codec.clone(),
                    mime_type: chosen.mime_type.clone(),
                }
            }
        }
    }
}

/// Convenience free function: select a track's source under a preference.
pub fn select_source<P: Playability + ?Sized>(
    sources: &TrackSources,
    pref: Option<&AudioPreference>,
    can_play: &P,
) -> SelectedSource {
    sources.select(pref, can_play)
}
