//! Policy domain: representation selection and playback policy.
//!
//! **Status: implemented (migration phase 12).** See
//! [`representation`] for the resolver; the playback loudness policy lives in
//! [`crate::player::gain`].
//!
//! # Representation selection
//!
//! Reference: `docs/representation-selection-phase4.md` and
//! `web/src/lib/state/representation-selection.ts` in the existing
//! repository. Deliberately **outside** the player core in the reference
//! (player-core stays representation-blind); in Rust the same separation
//! applies — this module decides *which audio object a track plays*, the
//! player module consumes the decision.
//!
//! The persisted preference (`musicpack.audio-preference.v1`):
//!
//! ```text
//! { mode: "default" }                       primary only
//! { mode: "representation", id: number }    one explicit representation
//! { mode: "codec", codec: string }          exact codec family, case-insensitive
//! { mode: "lossless" }                      first playable lossless
//! ```
//!
//! Selection algorithm (candidates in **manifest order** — the only
//! tie-break; size never decides; playability is an injected predicate):
//!
//! 1. `default`/undefined → step 5;
//! 2. `representation` → first candidate with that id, if playable, else step 5;
//! 3. `codec` → first playable candidate of that codec family, else step 5;
//! 4. `lossless` → first playable candidate in the closed set
//!    `["flac", "wav", "aiff"]`, else step 5;
//! 5. rescue: primary with playable-or-absent codec; else first playable
//!    candidate; else primary regardless (the unsupported-format error
//!    surfaces at engine-open time).
//!
//! The resolver is pure and total; it never throws and never invents
//! representations. Changing the preference never rebuilds or restarts
//! already-playing items.
//!
//! # Playback loudness policy
//!
//! Reference: `web/player-core/src/gain.ts`. Modes `off | album | track`
//! (default `album`); gain `= −16 LUFS − measured LUFS`, capped by
//! `−1 dBTP − measured true peak`; user volume and normalization combine
//! linearly (`volume × 10^(dB/20)`). Measured values are never modified —
//! this is playback policy only.

pub mod representation;

pub use representation::{
    AcceptAll, AudioPreference, Candidate, LOSSLESS_CODECS, Playability, RepresentationRef,
    SelectedAudio, SourceRef, TrackAudio, item_id, resolve_audio,
};
