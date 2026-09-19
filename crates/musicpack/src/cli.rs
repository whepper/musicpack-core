//! Argument parsing and command dispatch.
//!
//! Deliberately tiny: the reference CLI's commands take positional
//! arguments and a couple of flags, so hand-rolled parsing keeps the
//! production dependency graph empty.

use std::process::ExitCode;

/// A CLI-level error: usage problems exit 2, failures exit 1 (matching the
/// reference tool's `usage_error`/error convention).
pub enum CliError {
    /// Bad invocation.
    Usage(String),
    /// The command ran but failed.
    Failure(String),
}

/// Parses arguments and runs the requested command.
pub fn run(args: &[String]) -> Result<ExitCode, CliError> {
    let Some(command) = args.first() else {
        return Err(CliError::Usage("missing command".into()));
    };
    let rest = &args[1..];
    match command.as_str() {
        "info" => parse_info(rest),
        "verify" => parse_verify(rest),
        "pack" => parse_pack(rest),
        "unpack" => parse_unpack(rest),
        "help" | "--help" | "-h" => Err(CliError::Usage("help requested".into())),
        other => Err(CliError::Usage(format!("unknown command '{other}'"))),
    }
}

fn one_path(rest: &[String], command: &str) -> Result<String, CliError> {
    let positional: Vec<&String> = rest.iter().filter(|a| !a.starts_with('-')).collect();
    match positional.as_slice() {
        [path] => Ok((*path).clone()),
        [] => Err(CliError::Usage(format!(
            "{command} requires a package path"
        ))),
        _ => Err(CliError::Usage(format!(
            "{command} takes exactly one package path"
        ))),
    }
}

fn parse_info(rest: &[String]) -> Result<ExitCode, CliError> {
    let path = one_path(rest, "info")?;
    super::commands::info(&path)
}

fn parse_verify(rest: &[String]) -> Result<ExitCode, CliError> {
    let quiet = rest.iter().any(|a| a == "-q" || a == "--quiet");
    let json = rest.iter().any(|a| a == "--json");
    let path = one_path(rest, "verify")?;
    super::commands::verify(&path, quiet, json)
}

fn parse_pack(rest: &[String]) -> Result<ExitCode, CliError> {
    match rest {
        [dir, out] => super::commands::pack(dir, out),
        _ => Err(CliError::Usage(
            "pack requires <package-dir> <output.mpak>".into(),
        )),
    }
}

fn parse_unpack(rest: &[String]) -> Result<ExitCode, CliError> {
    match rest {
        [file, dir] => super::commands::unpack(file, dir),
        [file] => {
            // Reference convenience: derive the output directory by
            // stripping a `.mpak` suffix.
            let derived = file
                .strip_suffix(".mpak")
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("{file}.unpacked"));
            super::commands::unpack(file, &derived)
        }
        _ => Err(CliError::Usage(
            "unpack requires <input.mpak> [directory]".into(),
        )),
    }
}
