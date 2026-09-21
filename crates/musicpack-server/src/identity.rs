//! Collector identity — promoted to `musicpack_core::identity` in R4.1.
//!
//! The algorithm (package fingerprint, group/release keys, MBID validity,
//! manifest hash) is pure over the core manifest model and is now shared
//! with the authoring package builder, so it lives in `musicpack-core`
//! (`docs/adr/0010-core-package-builder.md`). This module re-exports it so
//! the server's existing call sites and golden-vector test are unchanged.
//! The C-generated vectors in `tests/data/identity/` still pin the moved
//! implementation.

pub use musicpack_core::identity::*;
