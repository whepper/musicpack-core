//! # musicpack-core
//!
//! Canonical, reusable Rust implementation of the **MusicPack domain** and
//! the **`.mpack` v1** / **MPAK v1** package formats.
//!
//! The behavioural reference is the existing MusicPack application
//! repository (the C library `core/libmusicpack`, its CLI, and the
//! TypeScript package `web/player-core`), together with the normative
//! specifications:
//!
//! | Spec | Scope |
//! |------|-------|
//! | `specs/musicpack-v1.md` | manifest and directory bundle (logical model) |
//! | `specs/mpak-v1.md` | single-file `.mpak` physical container |
//! | `specs/musicpack-waveform-v1.md` | per-track waveform envelopes |
//! | `specs/musicpack-sonic-v1.md` | sonic analysis documents |
//! | `docs/musicpack-lyrics-v1.md` | lyrics content profile, per-track references, timing |
//!
//! This crate re-implements that behaviour; it does not redesign it.
//! Compatibility with the existing implementation is a core requirement and
//! is verified by differential testing against it (see `docs/architecture.md`).
//!
//! ## Module map
//!
//! | Module | Responsibility | Status |
//! |--------|----------------|--------|
//! | [`error`] | error taxonomy | complete |
//! | [`limits`] | untrusted-input resource budgets | complete |
//! | [`json`] | cJSON-compatible strict JSON (value, parser, canonical printer) | complete |
//! | [`format::path`] | canonical package-relative path rules | complete |
//! | [`format::checksum`] | SHA-256 declaration format + digest computation | complete |
//! | [`format::number`] | exact C `%.8g` / integer number formatting | complete |
//! | [`format::manifest`] | `.mpack` v1 manifest model, strict parser, canonical writer | complete |
//! | [`format::mpak`] | MPAK v1 container constants, framing, CRC-16 | constants + CRC-16; parser/writer in phase 6 |
//! | [`format::waveform`] | waveform envelope constants and quantization | kernel + limits; accumulator later |
//! | [`identity`] | collector identity: package fingerprint, group/release keys, MBID validity | complete (promoted from the server in R4.1) |
//! | [`authoring`] | authoring draft → canonical `.mpack` directory builder | complete (`unix`; R4.1) |
//! | [`lyrics`] | lyrics domain: strict LRC profile parser, model, active-line timing | complete (`docs/musicpack-lyrics-v1.md`) |
//! | [`storage`] | platform-independent package-object backend (directory adapter on unix) | trait + reference adapter |
//! | [`validation`] | `verify` semantics: report, budgets, checksums, containment | complete (sonic documents deferred) |
//! | [`audio`] | PCM contract, decode abstraction (native WAV + `claxon` FLAC), waveform accumulator, BS.1770-5 meter | complete (phases 8–9) |
//! | [`player`] | platform-independent player core (port of `web/player-core`): queue, engine seam, gain policy, snapshots, events, Sweet-Fades planner, orchestrator | complete (phase 10) |
//! | [`policy`] | representation selection and playback policy | planned |
//!
//! ## Non-goals
//!
//! This crate never contains: HTTP, UI, browser or Node.js APIs,
//! authentication, sessions, application database access, or deployment
//! infrastructure. Platform adapters (native server integration, a future
//! `musicpack-wasm` crate, a CLI) live outside the core.
//!
//! ## Trust model
//!
//! All `.mpack`/`.mpak` input is untrusted. Parsers must be bounds-checked,
//! overflow-checked, and bounded by the limits in [`limits`] with
//! predictable errors — never panics on malformed input.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod audio;
#[cfg(unix)]
pub mod authoring;
pub mod error;
pub mod format;
pub mod identity;
pub mod json;
pub mod limits;
pub mod lyrics;
pub mod player;
pub mod policy;
pub mod storage;
pub mod validation;

pub use error::{Error, Result};
