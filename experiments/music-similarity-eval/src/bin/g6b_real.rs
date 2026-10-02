//! G-6B real-corpus runner: evaluate the frozen seven-gate table
//! (`G6B_METHODOLOGY.md` §11) over recorded Discogs-EffNet `embeddings.json`
//! corpora.
//!
//! ```text
//! cargo run --manifest-path experiments/music-similarity-eval/Cargo.toml --bin g6b_real -- \
//!   --corpus multi61=PATH/TO/embeddings.json \
//!   --corpus multi62=PATH/TO/embeddings.json \
//!   --corpus release61=PATH/TO/embeddings.json \
//!   --corpus release62=PATH/TO/embeddings.json \
//!   --criteria experiments/music-similarity-eval/G6B_METHODOLOGY.md \
//!   --out SOME_EXTERNAL_DIR
//! ```
//!
//! Pure deterministic post-processing: no model download, no audio decoding,
//! no inference, no Python, no network, no production code. Per-track
//! digests are re-verified before any measurement (§13), and the frozen
//! criteria rows are compared byte-for-byte against the methodology document
//! at run time (§17.5) — a drift aborts the run instead of producing a
//! verdict. `--out` is an external, gitignored directory: the artefact
//! carries digests and model identity, never track metadata.

use std::path::PathBuf;
use std::process::ExitCode;

use music_similarity_eval::docfmt;
use music_similarity_eval::g6b;
use music_similarity_eval::g6b_real;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut corpus_args: Vec<(String, PathBuf)> = Vec::new();
    let mut criteria_path: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--corpus" => match args.next() {
                Some(value) => match value.split_once('=') {
                    Some((label, path)) if !label.is_empty() && !path.is_empty() => {
                        corpus_args.push((label.to_string(), PathBuf::from(path)));
                    }
                    _ => {
                        eprintln!("--corpus needs LABEL=PATH to an embeddings.json");
                        return ExitCode::FAILURE;
                    }
                },
                None => {
                    eprintln!("--corpus needs LABEL=PATH to an embeddings.json");
                    return ExitCode::FAILURE;
                }
            },
            "--criteria" => match args.next() {
                Some(value) => criteria_path = Some(PathBuf::from(value)),
                None => {
                    eprintln!("--criteria needs a path to G6B_METHODOLOGY.md");
                    return ExitCode::FAILURE;
                }
            },
            "--out" => match args.next() {
                Some(value) => out = Some(PathBuf::from(value)),
                None => {
                    eprintln!("--out needs a directory");
                    return ExitCode::FAILURE;
                }
            },
            "--help" | "-h" => {
                println!(
                    "usage: g6b_real --corpus LABEL=PATH [--corpus LABEL=PATH ...] --criteria G6B_METHODOLOGY.md [--out DIR]"
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument {other}");
                return ExitCode::FAILURE;
            }
        }
    }

    let Some(criteria_path) = criteria_path else {
        eprintln!(
            "--criteria is required: the frozen gate rows are verified against the methodology document at run time (G6B_METHODOLOGY.md §17.5)"
        );
        return ExitCode::FAILURE;
    };
    if corpus_args.is_empty() {
        eprintln!(
            "at least one --corpus LABEL=PATH is required (the planned run uses four: {{multi, release}} x {{hop 61, 62}})"
        );
        return ExitCode::FAILURE;
    }

    // §17.5: the freeze must hold before any gate is evaluated. A drift is a
    // protocol failure — the run produces no verdict artefact at all.
    let criteria_text = match std::fs::read_to_string(&criteria_path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("cannot read criteria {}: {error}", criteria_path.display());
            return ExitCode::FAILURE;
        }
    };
    let freeze = g6b::check_criteria_freeze(&criteria_text);
    if let Err(reason) = &freeze {
        eprintln!("criteria freeze check FAILED: {reason}");
        eprintln!(
            "refusing to evaluate the frozen gates against drifted criteria (G6B_METHODOLOGY.md §14, §17.5)"
        );
        return ExitCode::FAILURE;
    }

    let mut corpora = Vec::new();
    for (label, path) in &corpus_args {
        let corpus = match g6b_real::load_corpus(path) {
            Ok(corpus) => corpus,
            Err(error) => {
                eprintln!("corpus {label}: {error}");
                return ExitCode::FAILURE;
            }
        };
        let mut report = g6b::run_corpus_experiment(label, &corpus.records);
        report.provenance = Some(corpus.provenance_line());
        corpora.push(report);
    }

    let report_text = g6b::render_real_run_report(&freeze, &corpora);
    let overall = g6b::overall_verdict(&corpora);
    print!("{report_text}");
    println!(
        "report_sha256={}",
        docfmt::sha256_hex(report_text.as_bytes())
    );

    if let Some(directory) = out {
        if let Err(error) = std::fs::create_dir_all(&directory) {
            eprintln!("cannot create {}: {error}", directory.display());
            return ExitCode::FAILURE;
        }
        let path = directory.join("REPORT.txt");
        if let Err(error) = std::fs::write(&path, &report_text) {
            eprintln!("cannot write {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
        println!("wrote {}", path.display());
    }

    // A PASS exits successfully; FAIL or INCONCLUSIVE do not, so the run can
    // never be scripted into "green" without every gate being evaluated and
    // green on every corpus.
    match overall {
        g6b::Verdict::Pass => ExitCode::SUCCESS,
        g6b::Verdict::Fail | g6b::Verdict::Inconclusive => ExitCode::FAILURE,
    }
}
