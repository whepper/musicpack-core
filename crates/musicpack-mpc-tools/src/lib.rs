//! `musicpack-mpc-tools`: Rust SV8 container tools.
//!
//! This crate ports the *container-surgery* side of the legacy C Musepack
//! tools (`codec/mpccut`, and the container half of `codec/mpc2sv8`) onto the
//! shared Rust SV8 primitives. It performs no psychoacoustic encoding and
//! owns no codec state: cutting and reframing reuse
//! [`musicpack_musepack_encoder`] block writers and the
//! [`musicpack_core`] stream-info parser.
//!
//! # Licensing treatment (engineering boundary, not legal advice)
//!
//! The ported control flow is derived from LGPL-2.1-or-later C sources, so
//! this crate is LGPL-2.1-or-later like the encoder crate it builds on.
//! `publish = false` while under construction.
//!
//! # Design rules
//!
//! * `#![forbid(unsafe_code)]`, no FFI, no bindgen, no subprocess calls.
//! * `std`-only and `wasm32-unknown-unknown`-clean.
//! * Byte-preserving: unmodified blocks are copied verbatim, never
//!   decoded/re-encoded.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod cut;
