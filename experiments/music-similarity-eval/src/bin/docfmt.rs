//! Review tool for the similarity document v1 reference fixtures.
//!
//! ```text
//! cargo run --manifest-path experiments/music-similarity-eval/Cargo.toml \
//!   --bin docfmt -- dump fixtures/similarity-doc/minimal-ok.msim
//! cargo run --manifest-path experiments/music-similarity-eval/Cargo.toml \
//!   --bin docfmt -- emit  fixtures/similarity-doc
//! cargo run --manifest-path experiments/music-similarity-eval/Cargo.toml \
//!   --bin docfmt -- profiles
//! ```
//!
//! Nothing here runs a model, reads audio, or touches a library, a server or a
//! database. `emit` writes into a directory you name; it is never invoked by CI,
//! and the committed bytes — not this tool — are the contract.

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use music_similarity_eval::docfixtures::{self, MANIFEST_NAME};
use music_similarity_eval::docfmt;

fn usage() -> &'static str {
    "usage: docfmt <dump FILE | emit DIR | profiles>"
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        eprintln!("{}", usage());
        return ExitCode::FAILURE;
    };
    match command.as_str() {
        "dump" => {
            let Some(path) = args.next().map(PathBuf::from) else {
                eprintln!("{}", usage());
                return ExitCode::FAILURE;
            };
            match fs::read(&path) {
                Ok(bytes) => {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.display().to_string());
                    print!("{}", docfixtures::dump(&name, &bytes));
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("cannot read {}: {error}", path.display());
                    ExitCode::FAILURE
                }
            }
        }
        "emit" => {
            let Some(directory) = args.next().map(PathBuf::from) else {
                eprintln!("{}", usage());
                return ExitCode::FAILURE;
            };
            if let Err(error) = fs::create_dir_all(&directory) {
                eprintln!("cannot create {}: {error}", directory.display());
                return ExitCode::FAILURE;
            }
            // Remove stale fixtures so the directory always matches the set.
            if let Ok(entries) = fs::read_dir(&directory) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(docfixtures::FIXTURE_EXTENSION) || name == MANIFEST_NAME {
                        let _ = fs::remove_file(entry.path());
                    }
                }
            }
            for fixture in docfixtures::fixtures() {
                let path = directory.join(&fixture.name);
                if let Err(error) = fs::write(&path, &fixture.bytes) {
                    eprintln!("cannot write {}: {error}", path.display());
                    return ExitCode::FAILURE;
                }
                println!(
                    "{}  {} bytes  {}",
                    fixture.name,
                    fixture.bytes.len(),
                    docfmt::sha256_hex(&fixture.bytes)
                );
            }
            let manifest = directory.join(MANIFEST_NAME);
            if let Err(error) = fs::write(&manifest, docfixtures::manifest()) {
                eprintln!("cannot write {}: {error}", manifest.display());
                return ExitCode::FAILURE;
            }
            println!("{}", manifest.display());
            ExitCode::SUCCESS
        }
        "profiles" => {
            for (label, fields) in [
                ("musicpack-similarity-fixture-v1", docfixtures::profile_a()),
                (
                    "musicpack-similarity-fixture-f16-v1",
                    docfixtures::profile_b(),
                ),
            ] {
                let tlv = docfmt::profile_tlv(&fields);
                let fingerprint = docfmt::profile_fingerprint(&fields);
                println!("{label}");
                println!("  tlv bytes    {}", tlv.len());
                println!("  fingerprint  {}", docfmt::fingerprint_hex(&fingerprint));
                // Printed so a reviewer can diff the canonical encoding against
                // FORMAT_SPEC.md appendix A without writing any code.
                println!("  tlv hex      {}", docfmt::hex_encode(&tlv));
            }
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{}", usage());
            ExitCode::FAILURE
        }
    }
}
