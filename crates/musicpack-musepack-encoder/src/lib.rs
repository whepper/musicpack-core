//! `musicpack-musepack-encoder`: the Rust Musepack SV8 encoder.
//!
//! This crate is the home of the safe-Rust replacement for the legacy C
//! Musepack encoder (`codec/libmpcenc`, `codec/libmpcpsy` and the encoding
//! logic in `codec/mpcenc/mpcenc.c` in the reference repository). The goal is
//! a native-Rust, WASM-capable, deterministic encoder that produces
//! Musepack SV8 streams compatible with the established reference output, so
//! that the C encoder can eventually be deleted.
//!
//! # Status
//!
//! The crate is the production `PCM → SV8` encoder
//! (`encoder::MusepackEncoder`) and reproduces the reference `mpcenc` stream
//! byte-for-byte across the full **SV8 quality surface**: any finite `f32`
//! quality clipped to `[0, 10]` at 44100/48000/37800/32000 Hz — J.2
//! fractional-quality parity. The44 integer configurations stay frozen as
//! C-oracle regression tables, every other finite quality is computed
//! deterministically from frozen ATH bases (`psy::computed`), and a sparse
//! fractional corpus (`tests/data/encoder/fractional_manifest.txt`,27 rows
//! incl. a tonal signal) is byte-matched against the scalar C reference by
//! the `encoder_fractional` test (plus the mono streams in
//! `matrix_manifest.txt` / `encoder_matrix`). Non-finite qualities are
//! rejected with `EncoderError::NonFiniteQuality` — an intentional
//! compatibility boundary, since C's `NaN` path is undefined behaviour. See
//! `PSYCHOACOUSTIC_CONTRACT.md` §10.
//!
//! The foundation primitives established in Phase 15B remain in place:
//!
//! * [`bitwriter`] — the deterministic, MSB-first bit writer every SV8 field
//!   is written through;
//! * [`differential`] — the pure comparison primitives the compatibility
//!   oracle is built on.
//!
//! See `README.md` for the SV8 primitive terminology, the frozen reference
//! environment, and the migration/removal strategy.
//!
//! # Licensing treatment (engineering boundary, not legal advice)
//!
//! The legacy C encoder is LGPL-2.1-or-later, and a faithful Rust
//! implementation derived from it is treated conservatively as
//! **LGPL-2.1-or-later** as well. This crate therefore carries that license
//! (`LICENSE`) and is isolated from the permissively-licensed core:
//!
//! * `musicpack-core` stays BSD-3-Clause and **must never depend on this
//!   crate**;
//! * this crate is `publish = false` while it is under construction;
//! * no legal conclusions are asserted here beyond the repository's chosen
//!   engineering treatment. Open licensing questions are tracked in the
//!   migration document, not decided in code.
//!
//! # Design rules
//!
//! * `#![forbid(unsafe_code)]`, no FFI, no bindgen, no C encoder at runtime.
//! * `std`-only and `wasm32-unknown-unknown`-clean.
//! * Explicit per-instance state; **no global mutable encoder state**.
//! * Deterministic: identical inputs produce identical bytes.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod bitwriter;
pub mod blocks;
pub mod coding;
pub mod differential;
pub mod encoder;
pub mod error;
pub mod filterbank;
pub mod psy;
pub mod sv8;
