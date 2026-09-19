//! Fuzz target: the MPAK container scanner (normal and recovery modes).
//!
//! Properties under test:
//!
//! - no panic on arbitrary bytes;
//! - no infinite scanning (every iteration advances);
//! - no out-of-bounds reads (`ByteSource` bounds-checks every read);
//! - no allocation sized from an untrusted declared length before the
//!   CRC/bounds checks (MANF/INDX/TAIL payloads are bounded, member
//!   preambles are ≤ 4096 bytes);
//! - CRC-before-length and checked arithmetic hold.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let source = musicpack_core::format::mpak::MemorySource::new(data.to_vec());
    let _ = musicpack_core::format::mpak::scan(&source, false);
    let _ = musicpack_core::format::mpak::scan(&source, true);
});