//! Authoring-side package construction (R4.1).
//!
//! `musicpack-core` already owned the package *reader* (manifest parser,
//! canonical writer, verifier, MPAK container, identity). R4.1 adds the
//! missing writable sibling: a deterministic, verified **`.mpack`
//! directory builder** driven by an explicit [`AuthoringDraft`].
//!
//! ```text
//! AuthoringDraft (authored input)
//!       + source files
//!       ↓ build_directory
//! canonical manifest + verified .mpack directory
//! ```
//!
//! The builder is intentionally a *library primitive*, not an application:
//! it contains no UI, no persistence, no network access, no subprocess and
//! no encoding/orchestration. Those are the R4.2 authoring pipeline's job
//! (`docs/r3.7-r4-readiness.md` §13, `docs/adr/0010-core-package-builder.md`).
//!
//! Platform scope mirrors the reference directory adapter: the module is
//! `unix`-only (the `.mpak` writer remains portable). This keeps the core's
//! wasm32 targets clean, exactly like [`crate::storage::directory`].

mod build;
mod draft;

pub use build::{BuildOptions, BuildOutcome, LoudnessMode, build_directory};
pub use draft::{
    AuthoringDraft, DraftAnalysis, DraftArtwork, DraftAsset, DraftDisc, DraftLyrics,
    DraftRepresentation, DraftTrack, DraftWaveform,
};
