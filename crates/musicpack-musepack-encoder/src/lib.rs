//! `musicpack-musepack-encoder`: the Rust Musepack SV8 encoder.
//!
//! This crate is the home of the safe-Rust replacement for the legacy C
//! Musepack encoder (`codec/libmpcenc`, `codec/libmpcpsy` and the encoding
//! logic in `codec/mpcenc/mpcenc.c` in the reference repository). The goal is
//! a native-Rust, WASM-capable, deterministic encoder that produces
//! Musepack SV8 streams compatible with the established reference output, so
//! that the C encoder can eventually be deleted.
//!
//! # Status (Phase 15B)
//!
//! This phase establishes the **foundation only**:
//!
//! * [`bitwriter`] — the deterministic, MSB-first bit writer every SV8 field
//!   is written through;
//! * [`differential`] — the pure comparison primitives the compatibility
//!   oracle is built on.
//!
//! There is deliberately **no** psychoacoustic model, analysis filterbank,
//! quantisation, allocation, Huffman coding or frame/stream encoding here yet.
//! Those land in later phases with explicit numerical-compatibility testing.
//! See `README.md` for the SV8 primitive terminology they will target, the
//! frozen reference environment, and the migration/removal strategy.
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
