//! Representation-selection policy (phase 12).
//!
//! A deterministic, total, I/O-free port of the Phase 4 TypeScript resolver
//! (`web/app/src/lib/state/representation-selection.ts`, BSD-3-Clause), which
//! remains the behavioural oracle:
//!
//! ```text
//! resolveAudio(track, pref, canPlay) -> { representation: RepresentationRef | null }
//! ```
//!
//! Candidates are the track's representations in **manifest order** — the only
//! tie-break; size never decides. Playability is an injected predicate so the
//! host owns browser/offline capability probing and this module stays pure.
//!
//! Selection:
//!
//! 1. `None`/`Default` → step 5;
//! 2. `Representation { id }` → first candidate with that id, if playable,
//!    else step 5;
//! 3. `Codec { codec }` → first playable candidate of that codec family
//!    (case-insensitive), else step 5;
//! 4. `Lossless` → first playable candidate in [`LOSSLESS_CODECS`], else step 5;
//! 5. rescue: primary with playable-or-absent codec; else the first playable
//!    candidate; else the primary regardless (the unsupported-format error
//!    surfaces later at engine open). The resolver never throws and never
//!    invents representations.
//!
//! Representation identity is a `f64` to mirror the TypeScript `number` (which
//! the API assigns and compares by exact equality). Ids are expected to be
//! integral; [`item_id`] formats them as the reference does.

use crate::json::Value;

/// Codecs classified as lossless by the Phase 4 policy (closed set).
pub const LOSSLESS_CODECS: [&str; 3] = ["flac", "wav", "aiff"];

/// The single active audio preference (`musicpack.audio-preference.v1`).
#[derive(Debug, Clone, PartialEq)]
pub enum AudioPreference {
    /// Primary audio only (pre-Phase-4 behaviour).
    Default,
    /// One explicit representation row id.
    Representation {
        /// The representation id (exact `number` equality, as in the API).
        id: f64,
    },
    /// First playable representation of an exact codec family.
    Codec {
        /// The codec family string (compared case-insensitively).
        codec: String,
    },
    /// First playable lossless representation, otherwise normal fallback.
    Lossless,
}

impl AudioPreference {
    /// Validates an unknown JSON value into a preference; `None` when unusable
    /// (malformed preferences fall back to the primary, mirroring the
    /// reference's `parseAudioPreference`).
    pub fn from_value(raw: &Value) -> Option<Self> {
        if !raw.is_object() {
            return None;
        }
        let mode = match raw.get("mode") {
            Some(Value::String(s)) => s.as_str(),
            _ => return None,
        };
        match mode {
            "default" => Some(AudioPreference::Default),
            "representation" => match raw.get("id") {
                Some(Value::Number(id)) if id.is_finite() => {
                    Some(AudioPreference::Representation { id: *id })
                }
                _ => None,
            },
            "codec" => match raw.get("codec") {
                Some(Value::String(codec)) if !codec.trim().is_empty() => {
                    Some(AudioPreference::Codec {
                        codec: codec.clone(),
                    })
                }
                _ => None,
            },
            "lossless" => Some(AudioPreference::Lossless),
            _ => None,
        }
    }

    /// Parses a preference from JSON text; `None` when the text is not JSON or
    /// does not describe a valid preference.
    pub fn from_json_str(raw: &str) -> Option<Self> {
        crate::json::parse(raw.as_bytes())
            .ok()
            .and_then(|v| Self::from_value(&v))
    }

    /// Serializes the preference to the persisted JSON value shape.
    pub fn to_value(&self) -> Value {
        match self {
            AudioPreference::Default => {
                Value::Object(vec![("mode".into(), Value::String("default".into()))])
            }
            AudioPreference::Representation { id } => Value::Object(vec![
                ("mode".into(), Value::String("representation".into())),
                ("id".into(), Value::Number(*id)),
            ]),
            AudioPreference::Codec { codec } => Value::Object(vec![
                ("mode".into(), Value::String("codec".into())),
                ("codec".into(), Value::String(codec.clone())),
            ]),
            AudioPreference::Lossless => {
                Value::Object(vec![("mode".into(), Value::String("lossless".into()))])
            }
        }
    }
}

/// A candidate alternate representation (policy input).
#[derive(Debug, Clone, PartialEq)]
pub struct RepresentationRef {
    /// Stable representation id (exact-equality identity).
    pub id: f64,
    /// Codec family hint (`"flac"`, ...).
    pub codec: Option<String>,
    /// MIME type hint (used by browser-native playability probes).
    pub mime_type: Option<String>,
}

/// The playability-relevant view of a track (policy input).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TrackAudio {
    /// Primary audio codec family.
    pub codec: Option<String>,
    /// Primary audio MIME type.
    pub mime_type: Option<String>,
    /// Alternate representations, in manifest order.
    pub representations: Vec<RepresentationRef>,
}

/// Which audio object a playability probe is judging.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SourceRef {
    /// The track's primary audio.
    Primary,
    /// An alternate representation row.
    Representation {
        /// The representation id (exact `number` identity).
        id: f64,
    },
}

/// The argument a [`Playability`] predicate receives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate<'a> {
    /// Codec family hint.
    pub codec: Option<&'a str>,
    /// MIME type hint.
    pub mime_type: Option<&'a str>,
    /// Which object is being judged.
    pub source: SourceRef,
}

/// Injected playability (browser capability, offline availability, ...).
pub trait Playability {
    /// `true` when the candidate can be played in this environment.
    fn can_play(&self, candidate: &Candidate<'_>) -> bool;
}

impl<F> Playability for F
where
    F: for<'a> Fn(&Candidate<'a>) -> bool,
{
    fn can_play(&self, candidate: &Candidate<'_>) -> bool {
        self(candidate)
    }
}

/// Accepts every candidate (used when no availability information exists).
#[derive(Debug, Clone, Copy, Default)]
pub struct AcceptAll;

impl Playability for AcceptAll {
    fn can_play(&self, _candidate: &Candidate<'_>) -> bool {
        true
    }
}

/// The resolver result: `representation` is an index into
/// [`TrackAudio::representations`], or `None` for the primary audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedAudio {
    /// Chosen representation index, or `None` for the primary.
    pub representation: Option<usize>,
}

impl SelectedAudio {
    /// `true` when the primary audio was selected.
    pub fn is_primary(&self) -> bool {
        self.representation.is_none()
    }

    /// The chosen representation's id, or `None` for the primary.
    pub fn representation_id(&self, track: &TrackAudio) -> Option<f64> {
        self.representation.map(|i| track.representations[i].id)
    }
}

fn lower(value: Option<&str>) -> String {
    value.unwrap_or("").to_lowercase()
}

fn is_lossless(codec: Option<&str>) -> bool {
    LOSSLESS_CODECS.contains(&lower(codec).as_str())
}

fn candidate_for(representation: &RepresentationRef) -> Candidate<'_> {
    Candidate {
        codec: representation.codec.as_deref(),
        mime_type: representation.mime_type.as_deref(),
        source: SourceRef::Representation {
            id: representation.id,
        },
    }
}

fn primary_candidate(track: &TrackAudio) -> Candidate<'_> {
    Candidate {
        codec: track.codec.as_deref(),
        mime_type: track.mime_type.as_deref(),
        source: SourceRef::Primary,
    }
}

/// Deterministic selection + fallback; never fails and never invents
/// representations (see the module docs for the exact rule order).
pub fn resolve_audio<P: Playability + ?Sized>(
    track: &TrackAudio,
    pref: Option<&AudioPreference>,
    can_play: &P,
) -> SelectedAudio {
    let candidates = &track.representations;
    let primary_playable = can_play.can_play(&primary_candidate(track));

    let mut chosen: Option<usize> = None;
    match pref {
        None | Some(AudioPreference::Default) => {}
        Some(AudioPreference::Representation { id }) => {
            if let Some(index) = candidates.iter().position(|r| r.id == *id) {
                if can_play.can_play(&candidate_for(&candidates[index])) {
                    chosen = Some(index);
                }
            }
        }
        Some(AudioPreference::Codec { codec }) => {
            let wanted = codec.to_lowercase();
            chosen = candidates
                .iter()
                .enumerate()
                .filter(|(_, r)| lower(r.codec.as_deref()) == wanted)
                .find(|(_, r)| can_play.can_play(&candidate_for(r)))
                .map(|(index, _)| index);
        }
        Some(AudioPreference::Lossless) => {
            chosen = candidates
                .iter()
                .enumerate()
                .filter(|(_, r)| is_lossless(r.codec.as_deref()))
                .find(|(_, r)| can_play.can_play(&candidate_for(r)))
                .map(|(index, _)| index);
        }
    }

    if chosen.is_none() && !primary_playable && !candidates.is_empty() {
        // Rescue: the primary is unplayable but a playable alternate exists.
        chosen = candidates
            .iter()
            .enumerate()
            .find(|(_, r)| can_play.can_play(&candidate_for(r)))
            .map(|(index, _)| index);
    }

    SelectedAudio {
        representation: chosen,
    }
}

/// Representation-aware playback-item identity: `t{track}r{rep}` for an
/// alternate, `t{track}` for the primary (the reference's Phase 4 contract).
pub fn item_id(track_id: i64, representation_id: Option<f64>) -> String {
    match representation_id {
        Some(id) => format!("t{track_id}r{id}"),
        None => format!("t{track_id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rep(id: f64, codec: &str, mime: &str) -> RepresentationRef {
        RepresentationRef {
            id,
            codec: Some(codec.into()),
            mime_type: Some(mime.into()),
        }
    }

    fn track(reps: Vec<RepresentationRef>) -> TrackAudio {
        TrackAudio {
            codec: Some("musepack-sv8".into()),
            mime_type: Some("audio/musepack".into()),
            representations: reps,
        }
    }

    fn only(codes: &[&str]) -> impl Playability + 'static {
        let codes: Vec<String> = codes.iter().map(|s| s.to_lowercase()).collect();
        move |c: &Candidate<'_>| {
            let codec = c.codec.unwrap_or("").to_lowercase();
            codes.contains(&codec)
        }
    }

    fn none(_: &Candidate<'_>) -> bool {
        false
    }

    #[test]
    fn undefined_and_default_preferences_pick_primary() {
        let t = track(vec![rep(77.0, "flac", "audio/flac")]);
        assert!(resolve_audio(&t, None, &AcceptAll).is_primary());
        assert!(resolve_audio(&t, Some(&AudioPreference::Default), &AcceptAll).is_primary());
    }

    #[test]
    fn tracks_without_representations_always_resolve_primary() {
        let plain = track(vec![]);
        assert!(resolve_audio(&plain, Some(&AudioPreference::Lossless), &AcceptAll).is_primary());
        assert!(
            resolve_audio(
                &plain,
                Some(&AudioPreference::Codec {
                    codec: "flac".into()
                }),
                &AcceptAll
            )
            .is_primary()
        );
        assert!(resolve_audio(&plain, None, &none).is_primary());
    }

    #[test]
    fn explicit_representation_is_chosen_when_playable() {
        let t = track(vec![
            rep(76.0, "flac", "audio/flac"),
            rep(77.0, "wav", "audio/wav"),
        ]);
        let selected = resolve_audio(
            &t,
            Some(&AudioPreference::Representation { id: 77.0 }),
            &AcceptAll,
        );
        assert_eq!(selected.representation, Some(1));
    }

    #[test]
    fn explicit_representation_absent_or_unplayable_falls_back() {
        let t = track(vec![rep(77.0, "flac", "audio/flac")]);
        assert!(
            resolve_audio(
                &t,
                Some(&AudioPreference::Representation { id: 99.0 }),
                &AcceptAll
            )
            .is_primary()
        );
        assert!(
            resolve_audio(
                &t,
                Some(&AudioPreference::Representation { id: 77.0 }),
                &only(&["musepack-sv8"])
            )
            .is_primary()
        );
    }

    #[test]
    fn codec_preference_is_case_insensitive_and_first_in_order() {
        let t = track(vec![
            rep(75.0, "mp3", "audio/mpeg"),
            rep(76.0, "FLAC", "audio/flac"),
            rep(77.0, "flac", "audio/flac"),
        ]);
        let selected = resolve_audio(
            &t,
            Some(&AudioPreference::Codec {
                codec: "flac".into(),
            }),
            &AcceptAll,
        );
        assert_eq!(selected.representation, Some(1));
    }

    #[test]
    fn codec_preference_without_match_falls_back() {
        let t = track(vec![rep(75.0, "mp3", "audio/mpeg")]);
        assert!(
            resolve_audio(
                &t,
                Some(&AudioPreference::Codec {
                    codec: "ogg".into()
                }),
                &AcceptAll
            )
            .is_primary()
        );
    }

    #[test]
    fn codec_preference_skips_unplayable_matches() {
        let t = track(vec![
            rep(75.0, "flac", "audio/x-broken"),
            rep(76.0, "flac", "audio/flac"),
        ]);
        let selected = resolve_audio(
            &t,
            Some(&AudioPreference::Codec {
                codec: "flac".into(),
            }),
            &|c: &Candidate<'_>| c.mime_type != Some("audio/x-broken"),
        );
        assert_eq!(selected.representation, Some(1));
    }

    #[test]
    fn lossless_picks_the_first_lossless_in_manifest_order() {
        let t = track(vec![
            rep(73.0, "mp3", "audio/mpeg"),
            rep(74.0, "aiff", "audio/aiff"),
            rep(75.0, "flac", "audio/flac"),
        ]);
        let selected = resolve_audio(&t, Some(&AudioPreference::Lossless), &AcceptAll);
        assert_eq!(selected.representation, Some(1));
    }

    #[test]
    fn lossless_ignores_lossy_candidates() {
        let t = track(vec![
            rep(73.0, "mp3", "audio/mpeg"),
            rep(74.0, "opus", "audio/opus"),
        ]);
        assert!(resolve_audio(&t, Some(&AudioPreference::Lossless), &AcceptAll).is_primary());
    }

    #[test]
    fn lossless_set_is_the_agreed_closed_classification() {
        let mut set = LOSSLESS_CODECS.to_vec();
        set.sort_unstable();
        assert_eq!(set, ["aiff", "flac", "wav"]);
    }

    #[test]
    fn unplayable_primary_with_playable_alternate_rescues() {
        let t = track(vec![rep(77.0, "flac", "audio/flac")]);
        let selected = resolve_audio(&t, None, &only(&["musepack-x", "flac"]));
        assert_eq!(selected.representation, Some(0));
    }

    #[test]
    fn playable_primary_is_never_displaced() {
        let t = track(vec![rep(77.0, "flac", "audio/flac")]);
        assert!(resolve_audio(&t, None, &only(&["musepack-sv8"])).is_primary());
    }

    #[test]
    fn nothing_playable_returns_primary() {
        let t = track(vec![rep(77.0, "flac", "audio/flac")]);
        assert!(resolve_audio(&t, Some(&AudioPreference::Lossless), &none).is_primary());
    }

    #[test]
    fn rescue_walks_manifest_order_past_unplayable_alternates() {
        let t = track(vec![
            rep(76.0, "wav", "audio/wav"),
            rep(77.0, "flac", "audio/flac"),
        ]);
        let selected = resolve_audio(&t, None, &only(&["flac"]));
        assert_eq!(selected.representation_id(&t), Some(77.0));
    }

    #[test]
    fn resolution_is_deterministic_and_inputs_are_not_mutated() {
        let t = track(vec![
            rep(76.0, "wav", "audio/wav"),
            rep(77.0, "flac", "audio/flac"),
        ]);
        let before = t.clone();
        let prefs = [
            None,
            Some(AudioPreference::Default),
            Some(AudioPreference::Lossless),
            Some(AudioPreference::Codec {
                codec: "FLAC".into(),
            }),
            Some(AudioPreference::Representation { id: 77.0 }),
        ];
        for pref in &prefs {
            let a = resolve_audio(&t, pref.as_ref(), &AcceptAll);
            let b = resolve_audio(&t, pref.as_ref(), &AcceptAll);
            assert_eq!(a, b);
        }
        assert_eq!(t, before);
    }

    #[test]
    fn parse_round_trips_every_valid_mode() {
        let cases = [
            r#"{"mode":"default"}"#,
            r#"{"mode":"lossless"}"#,
            r#"{"mode":"representation","id":7}"#,
            r#"{"mode":"codec","codec":"flac"}"#,
        ];
        let expected = [
            AudioPreference::Default,
            AudioPreference::Lossless,
            AudioPreference::Representation { id: 7.0 },
            AudioPreference::Codec {
                codec: "flac".into(),
            },
        ];
        for (raw, want) in cases.iter().zip(expected) {
            let parsed = AudioPreference::from_json_str(raw).expect(raw);
            assert_eq!(parsed, want);
            assert_eq!(
                parsed.to_value(),
                AudioPreference::from_json_str(raw).unwrap().to_value()
            );
        }
    }

    #[test]
    fn parse_rejects_junk() {
        assert_eq!(AudioPreference::from_json_str("null"), None);
        assert_eq!(AudioPreference::from_json_str("\"x\""), None);
        assert_eq!(AudioPreference::from_json_str("42"), None);
        assert_eq!(AudioPreference::from_json_str("{}"), None);
        assert_eq!(AudioPreference::from_json_str(r#"{"mode":"shiny"}"#), None);
        assert_eq!(
            AudioPreference::from_json_str(r#"{"mode":"representation","id":"77"}"#),
            None
        );
        assert_eq!(
            AudioPreference::from_json_str(r#"{"mode":"codec","codec":""}"#),
            None
        );
        assert_eq!(
            AudioPreference::from_json_str(r#"{"mode":"codec","codec":"   "}"#),
            None
        );
    }

    #[test]
    fn item_id_is_representation_aware() {
        assert_eq!(item_id(14, None), "t14");
        assert_eq!(item_id(14, Some(77.0)), "t14r77");
    }
}
