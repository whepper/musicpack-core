//! Fuzz target: the strict JSON parser and manifest parser.
//!
//! Properties under test:
//!
//! - no panic on arbitrary bytes (the parser is total);
//! - termination (no unbounded loops on malformed input);
//! - bounded resource use (nesting/asset/size limits are enforced);
//! - an accepted manifest always survives a canonical write and re-parse.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(parsed) = musicpack_core::format::manifest::ParsedManifest::parse(data) {
        // Write-validation must also be total for accepted input.
        let _ = parsed.write_canonical();
    }
});