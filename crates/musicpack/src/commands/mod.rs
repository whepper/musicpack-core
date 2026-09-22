//! Command implementations.
//!
//! Every command delegates semantics to `musicpack-core`: this module only
//! opens packages, presents results, and (for `unpack`) applies the
//! application-level extraction policy.

mod unpack;

use std::process::ExitCode;

use musicpack_core::Error;
use musicpack_core::json::{Value, print_canonical};
use musicpack_core::validation::{Report, Severity};

use crate::cli::CliError;
use crate::package::Package;

pub use unpack::unpack;

fn failure(message: String) -> CliError {
    CliError::Failure(message)
}

/// `musicpack info <package>`: manifest/container summary plus the
/// reference's `Integrity:` line, exiting non-zero exactly when
/// verification has errors (the reference behaves this way).
pub fn info(path: &str) -> Result<ExitCode, CliError> {
    let package = Package::open(path).map_err(|e| failure(open_message(path, &e)))?;
    let parsed = package
        .parsed()
        .map_err(|e| failure(open_message(path, &e)))?;
    let m = parsed.manifest();

    println!("Package: {path} ({})", package.kind());
    println!("Album: {}", m.album.title);
    for artist in &m.album.artists {
        match &artist.role {
            Some(role) => println!("Artist: {} ({role})", artist.name),
            None => println!("Artist: {}", artist.name),
        }
    }
    if let Some(release_type) = m.album.release_type {
        println!("Type: {}", release_type.as_str());
    }
    if let Some(release) = &m.release {
        if let Some(edition) = &release.edition {
            println!("Edition: {edition}");
        }
        if let Some(date) = &release.release_date {
            println!("Release date: {date}");
        }
        if let Some(country) = &release.country {
            println!("Country: {country}");
        }
        if let Some(label) = &release.label {
            println!("Label: {label}");
        }
        if let Some(catalogue) = &release.catalogue_number {
            println!("Catalogue number: {catalogue}");
        }
    }
    if let Some(date) = &m.album.original_release_date {
        println!("Original release: {date}");
    }

    let tracks: usize = m.media.iter().map(|disc| disc.tracks.len()).sum();
    let waveforms = m
        .media
        .iter()
        .flat_map(|disc| &disc.tracks)
        .filter(|track| track.waveform.is_some())
        .count();
    println!("Media: {} disc(s), {tracks} track(s)", m.media.len());
    if waveforms > 0 {
        println!("Waveform: {waveforms}/{tracks} tracks have envelopes");
    } else {
        println!("Waveform: none");
    }

    let report = musicpack_core::validation::verify(m, package.backend());
    report_print_if_interesting(&report);
    println!(
        "Integrity: {} ({} errors, {} warnings)",
        if report.is_ok() { "OK" } else { "FAILED" },
        report.errors(),
        report.warnings()
    );
    Ok(exit_for(&report))
}

/// `musicpack verify <package>`: the shared verifier's report.
pub fn verify(path: &str, quiet: bool, json: bool) -> Result<ExitCode, CliError> {
    let package = Package::open(path).map_err(|e| failure(open_message(path, &e)))?;
    let report = package
        .verify()
        .map_err(|e| failure(open_message(path, &e)))?;

    if json {
        println!("{}", report_json(&report));
    } else if !quiet {
        print_findings(&report);
        println!(
            "verify: {} error(s), {} warning(s)",
            report.errors(),
            report.warnings()
        );
    }
    Ok(exit_for(&report))
}

/// `musicpack pack <package-dir> <output.mpak>`: the core's directory
/// packer, staged and verified before publication (mirroring the reference
/// CLI's staging discipline).
pub fn pack(dir: &str, out: &str) -> Result<ExitCode, CliError> {
    let out_path = std::path::Path::new(out);
    if out_path.exists() {
        return Err(failure(format!("pack: output '{out}' already exists")));
    }
    let staging = staging_path(out_path);
    let result = (|| -> Result<(), Error> {
        musicpack_core::storage::directory::pack_directory(std::path::Path::new(dir), &staging)?;
        // The packed container must verify before it is published.
        let report = musicpack_core::storage::mpak::verify_mpak_file(&staging)?;
        if !report.is_ok() {
            return Err(Error::Invalid {
                detail: format!(
                    "packed container failed verification ({} error(s))",
                    report.errors()
                ),
            });
        }
        std::fs::rename(&staging, out_path).map_err(|e| Error::Io {
            detail: e.to_string(),
        })?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            println!("packed '{out}'");
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&staging);
            Err(failure(format!("pack: cannot pack '{dir}': {e}")))
        }
    }
}

fn staging_path(out: &std::path::Path) -> std::path::PathBuf {
    let mut staging = out.as_os_str().to_os_string();
    staging.push(format!(".staging-{}", std::process::id()));
    std::path::PathBuf::from(staging)
}

/// Parses a package and returns the shared verification report (used by
/// `unpack`'s post-extraction re-check).
pub(crate) fn verify_directory_report(dir: &std::path::Path) -> Option<Report> {
    musicpack_core::storage::directory::verify_directory(dir).ok()
}

fn open_message(path: &str, error: &Error) -> String {
    format!("cannot open package '{path}': {error}")
}

fn exit_for(report: &Report) -> ExitCode {
    if report.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn print_findings(report: &Report) {
    for finding in report.findings() {
        let label = match finding.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        println!("{label}: {}", finding.message);
    }
}

/// `info` prints findings only when there are any (the reference's human
/// `info` shows just the integrity line for clean packages).
fn report_print_if_interesting(report: &Report) {
    if !report.is_ok() {
        print_findings(report);
    }
}

/// The reference's machine-readable verify shape:
/// `{ "ok": bool, "errors": [...], "warnings": [...] }`.
fn report_json(report: &Report) -> String {
    let strings = |severity: Severity| -> Value {
        Value::Array(
            report
                .findings()
                .iter()
                .filter(|f| f.severity == severity)
                .map(|f| Value::String(f.message.clone()))
                .collect(),
        )
    };
    let root = vec![
        ("ok".to_string(), Value::Bool(report.is_ok())),
        ("errors".to_string(), strings(Severity::Error)),
        ("warnings".to_string(), strings(Severity::Warning)),
    ];
    print_canonical(&Value::Object(root))
}
