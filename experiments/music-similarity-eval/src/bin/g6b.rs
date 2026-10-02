//! G-6B runner: emit the deterministic instrument-check report, and optionally
//! write it to a directory for two-run comparison.
//!
//! ```text
//! cargo run --manifest-path experiments/music-similarity-eval/Cargo.toml \
//!   --bin g6b -- --out experiments/music-similarity-eval/fixtures/g6b
//! ```
//!
//! Nothing here downloads a model, decodes audio, runs inference, reads a music
//! library or touches a database. Every input is generated deterministically in
//! `g6b.rs` from the unchanged `g6` surrogate corpora. The artefact this emits
//! is an **instrument check**, not criteria evidence: see `G6B_METHODOLOGY.md`
//! §14 for the same-data prohibition.

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use music_similarity_eval::g6b;

/// Name of the committed instrument-check report inside the output directory.
const REPORT_NAME: &str = "REPORT.txt";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut out: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => match args.next() {
                Some(value) => out = Some(PathBuf::from(value)),
                None => {
                    eprintln!("--out needs a directory");
                    return ExitCode::FAILURE;
                }
            },
            "--help" | "-h" => {
                println!("usage: g6b [--out DIR]");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument {other}");
                return ExitCode::FAILURE;
            }
        }
    }

    let report = g6b::render_report();
    // Printed in full so the run is inspectable without writing anything.
    print!("{report}");
    println!(
        "report_sha256={}",
        music_similarity_eval::docfmt::sha256_hex(report.as_bytes())
    );

    if let Some(directory) = out {
        if let Err(error) = fs::create_dir_all(&directory) {
            eprintln!("cannot create {}: {error}", directory.display());
            return ExitCode::FAILURE;
        }
        let path = directory.join(REPORT_NAME);
        if let Err(error) = fs::write(&path, &report) {
            eprintln!("cannot write {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
        println!("wrote {}", path.display());
    }
    ExitCode::SUCCESS
}
