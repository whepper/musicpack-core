//! The Musepack SV8 psychoacoustic model (Phase 15F).
//!
//! A faithful, scalar, source-derived migration of the reference
//! `codec/libmpcpsy` model. It consumes a [`PcmFrame`] (the same raw PCM the
//! analysis filterbank consumes) and returns [`Smr`] signal-to-mask ratios,
//! long-partition transient flags and the ANS masking thresholds that the
//! already-verified downstream coding stages consume.
//!
//! # Compatibility contract
//!
//! * **Inputs:** 1600 PCM samples per channel plus the `(quality, sample rate)`
//!   configuration. `M`/`S` are `(L+R)/2`, `(L-R)/2` exactly as the reference
//!   computes them.
//! * **Outputs:** `SMR.L/R/M/S`, `TransientL/R` (short partitions), `Transient`
//!   (via [`transienten_calc`]), `MS_Flag` (via [`ms_lr_entscheidung`]) and
//!   `ANSspec.L/R/M/S`.
//! * **State:** all FFT history, temporal-masking integrators, transient and
//!   pre-echo state, loudness tracking, CVD voice lines and ANS thresholds live
//!   in [`PsychoacousticModel`] and persist across frames. There is no global
//!   mutable state; independent instances are isolated.
//! * **Numerics:** the frozen tables and every `f32`/`f64` boundary follow the
//!   pinned reference build. See `tests/data/psy/README.md`.
//!
//! The psychoacoustic model is **not** yet wired into a production encoder;
//! this module exists to lock bit-exact compatibility first.

// The module is a faithful source-derived migration; its float literals are
// copied verbatim from the reference and intentionally carry more digits than
// an `f32` needs.
#![allow(clippy::excessive_precision)]

mod cvd;
mod cvd_tables;
mod fft;
mod frozen;
mod math;
mod model;
mod profile;
mod tables;

pub use model::{
    ANALYSE_BUFFER, ModelOutput, PcmFrame, PsychoacousticModel, Smr, ms_lr_entscheidung, raise_smr,
    transienten_calc,
};
pub use profile::PsyParams;
