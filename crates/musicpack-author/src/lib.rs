//! `musicpack-author`: the Rust authoring pipeline.
//!
//! ```text
//! draft JSON ─► validate ─► identify ─► encode ─► waveform ─► .mpack ─► .mpak
//!                                                              │
//!                                         musicpack-core::authoring::build_directory
//! ```
//!
//! This crate is the R4.2 replacement for the functional responsibilities of
//! the legacy C `musicpack` authoring CLI. It contains **orchestration
//! only**:
//!
//! * [`draft`] — the JSON draft surface and its validation;
//! * [`encode`] — supported source audio → Musepack SV8 via the isolated
//!   `musicpack-musepack-encoder` crate;
//! * [`waveform`] — canonical v1 envelope generation via the core kernel;
//! * [`identify`] — MusicBrainz matching/application behind a provider
//!   abstraction (transport stays in the host);
//! * [`pipeline`] — the end-to-end run that assembles the core
//!   [`AuthoringDraft`](musicpack_core::authoring::AuthoringDraft) and calls
//!   [`build_directory`](musicpack_core::authoring::build_directory).
//!
//! # Boundaries
//!
//! Package semantics stay in `musicpack-core`: path validation, hashing,
//! canonical manifest serialization, verification, identity and the MPAK
//! container. This crate never writes `manifest.json`, never re-implements a
//! verifier, never re-parses lyrics, and never spawns a subprocess. It has
//! no HTTP client, no FFmpeg and no `unsafe`.
//!
//! The legacy C `musicpack` CLI, `mpcenc` and `musicpack-sonic` are **not**
//! invoked by anything here. The old Author runtime remains available
//! unchanged; R4.3 performs the cutover.
//!
//! # Determinism and robustness
//!
//! Encoding is bit-exact to the reference for the encoder's frozen
//! `(quality, sample-rate)` corpus; waveform generation is the core kernel;
//! loudness is measured by the core builder. The pipeline is fail-closed:
//! invalid drafts, unsupported sources, encoder failures and malformed
//! MusicBrainz documents produce typed errors and leave no package behind.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod artwork;
pub mod cli;
pub mod draft;
pub mod encode;
pub mod error;
pub mod identify;
pub mod inspect;
pub mod pipeline;
pub mod scan;
pub mod waveform;

pub use draft::{Draft, ValidationReport};
pub use error::{AuthorError, Result};
pub use identify::{
    Confidence, MusicBrainzProvider, identify_apply_json, identify_candidates_json,
    identify_mbid_json,
};
pub use pipeline::{
    AuthorOutcome, AuthorRequest, BuildPhase, BuildProgress, IdentifyRequest, PipelineOptions,
    WaveformEntry, encode_stage, encode_stage_with, run, run_with, validate_json, waveform_stage,
    waveform_stage_with,
};
