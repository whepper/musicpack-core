//! The `musicpack-server` library crate — stage 1 of the server migration.
//!
//! The behaviour lives here so the compatibility tests can exercise the
//! store and token domain directly; `src/main.rs` is a thin binary over it.
//!
//! Stage 1 contains the CLI/configuration skeleton ([`cli`], [`config`],
//! [`logging`]) and the byte-compatible SQLite persistence layer
//! ([`store`]) with API-token persistence ([`tokens`]). The scanner,
//! ingestion pipeline, HTTP API and byte serving are later stages of
//! `docs/server-migration.md`; nothing here fakes them.
//!
//! Boundary: depends on `musicpack-core`; nothing depends on this crate;
//! native-only (never a WASM target); `#![forbid(unsafe_code)]` at this
//! boundary — the bundled-SQLite C/`unsafe` of `rusqlite` stays inside the
//! dependency (see `docs/server-migration.md` D-S2/O-S1).

#![forbid(unsafe_code)]

pub mod cli;
pub mod config;
pub mod discover;
pub mod error;
pub mod http;
pub mod identity;
pub mod ingest;
pub mod jobs;
pub mod logging;
pub mod media;
pub mod pathsafe;
pub mod probe;
pub mod shutdown;
pub mod source;
pub mod store;
pub mod tokens;
