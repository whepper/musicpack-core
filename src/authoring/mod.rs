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
//! Platform scope mirrors the reference directory adapter: the builder is
//! portable std-only code and compiles on every target, including wasm32
//! (filesystem operations fail closed there at runtime, exactly like the
//! rest of the core). On unix it applies the reference's POSIX hardening;
//! elsewhere it applies the reference's own relaxed checks — see
//! [`crate::storage::directory`]. Decision record:
//! `docs/adr/0015-windows-directory-adapter.md`.

mod build;
mod draft;

pub use build::{
    BuildOptions, BuildOutcome, BuildProgress, BuildStage, LoudnessMode, build_directory,
    build_directory_with,
};
pub use draft::{
    AuthoringDraft, DraftAnalysis, DraftArtwork, DraftAsset, DraftDisc, DraftLyrics,
    DraftRepresentation, DraftTrack, DraftWaveform,
};
