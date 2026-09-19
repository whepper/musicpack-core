//! The `.mpack` format layer: everything defined by the normative v1
//! specifications, independent of any storage backend.
//!
//! The logical model (`format::manifest`) is storage-independent; a package
//! is either a directory bundle (`musicpack-v1.md` §2) or a single-file MPAK
//! v1 container (`mpak-v1.md`) carrying exactly the same manifest bytes.
//!
//! Current contents are foundational ports: the canonical path rules, the
//! SHA-256 declaration format, the manifest's closed enumerations and value
//! bounds, the MPAK framing constants plus CRC-16, and the waveform
//! quantization kernel. The manifest parser, MPAK parser/writer, and
//! package model land in later migration phases (see `docs/architecture.md`).

pub mod checksum;
pub mod manifest;
pub mod mpak;
pub mod number;
pub mod path;
pub mod waveform;
