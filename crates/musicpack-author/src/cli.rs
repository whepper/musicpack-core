//! Hand-rolled CLI for the authoring pipeline.
//!
//! Follows the repository's minimal CLI convention (positional commands,
//! hand-rolled flags, no argument-parsing dependency). It is a thin adapter
//! over [`crate::pipeline`]:
//!
//! ```text
//! musicpack-author validate <draft.json> [--json]
//! musicpack-author identify <draft.json> [--mb-json FILE] [--mbid UUID]
//!                                        [--mb-search-json FILE] [--json]
//! musicpack-author build    <draft.json> -o DIR [--mpak FILE] [--quality Q]
//!                                        [--no-waveform] [--no-loudness]
//!                                        [--replace] [--mb-json FILE]
//!                                        [--mbid UUID] [--json]
//! ```
//!
//! This is the R4.3 replacement surface, not the C command surface: the Tauri
//! host will call the library API (or this CLI) directly after the cutover.

use std::path::PathBuf;
use std::process::ExitCode;

use musicpack_core::authoring::LoudnessMode;
use musicpack_core::json::{Value, print_canonical};

use crate::error::AuthorError;
use crate::identify;
use crate::pipeline::{AuthorRequest, IdentifyRequest, PipelineOptions, run as run_pipeline};
use crate::{draft, pipeline};

/// A CLI-level failure: usage problems exit 2, stage failures exit 1.
pub enum CliError {
    /// Bad invocation.
    Usage(String),
    /// The command ran but failed.
    Failure(String),
}

impl From<AuthorError> for CliError {
    fn from(e: AuthorError) -> Self {
        CliError::Failure(e.to_string())
    }
}

/// Runs the CLI, returning the process exit code.
pub fn run(args: &[String]) -> Result<ExitCode, CliError> {
    let Some(command) = args.first() else {
        return Err(CliError::Usage(usage()));
    };
    let rest = &args[1..];
    match command.as_str() {
        "validate" => cmd_validate(rest),
        "identify" => cmd_identify(rest),
        "build" => cmd_build(rest),
        "inspect" => cmd_inspect(rest),
        "help" | "--help" | "-h" => Err(CliError::Usage(usage())),
        other => Err(CliError::Usage(format!(
            "unknown command '{other}'\n{}",
            usage()
        ))),
    }
}

fn usage() -> String {
    "usage: musicpack-author <validate|identify|build|inspect> <args...>\n\
     \x20 validate  <draft.json> [--json]\n\
     \x20 identify  <draft.json> [--mb-json FILE] [--mbid UUID] [--mb-search-json FILE] [--json]\n\
     \x20 build     <draft.json> -o DIR [--mpak FILE] [--quality Q] [--no-waveform]\n\
     \x20           [--no-loudness] [--replace] [--mb-json FILE] [--mbid UUID] [--json]\n\
     \x20 inspect   <album-dir|package-dir> [--json]"
        .into()
}

/// A parsed flag set.
struct Args {
    draft: String,
    json: bool,
    output: Option<PathBuf>,
    mpak: Option<PathBuf>,
    quality: f32,
    waveform: bool,
    loudness: LoudnessMode,
    replace: bool,
    mb_json: Option<PathBuf>,
    mb_search_json: Option<PathBuf>,
    mbid: Option<String>,
}

fn parse_args(rest: &[String], command: &str) -> Result<Args, CliError> {
    let mut draft: Option<String> = None;
    let mut args = Args {
        draft: String::new(),
        json: false,
        output: None,
        mpak: None,
        quality: crate::encode::DEFAULT_QUALITY,
        waveform: true,
        loudness: LoudnessMode::Measure,
        replace: false,
        mb_json: None,
        mb_search_json: None,
        mbid: None,
    };
    let mut i = 0;
    while i < rest.len() {
        let arg = &rest[i];
        match arg.as_str() {
            "--json" => args.json = true,
            "--no-waveform" => args.waveform = false,
            "--no-loudness" => args.loudness = LoudnessMode::Omit,
            "--replace" => args.replace = true,
            "-o" | "--out" => {
                i += 1;
                args.output = Some(PathBuf::from(take(rest, i, arg)?));
            }
            "--mpak" => {
                i += 1;
                args.mpak = Some(PathBuf::from(take(rest, i, arg)?));
            }
            "--quality" => {
                i += 1;
                let q = take(rest, i, arg)?;
                args.quality = q.parse().map_err(|_| {
                    CliError::Usage(format!("--quality must be a number, got '{q}'"))
                })?;
            }
            "--mb-json" => {
                i += 1;
                args.mb_json = Some(PathBuf::from(take(rest, i, arg)?));
            }
            "--mb-search-json" => {
                i += 1;
                args.mb_search_json = Some(PathBuf::from(take(rest, i, arg)?));
            }
            "--mbid" => {
                i += 1;
                args.mbid = Some(take(rest, i, arg)?);
            }
            other if other.starts_with('-') => {
                return Err(CliError::Usage(format!("unknown option '{other}'")));
            }
            other => {
                if draft.is_some() {
                    return Err(CliError::Usage(format!(
                        "{command} takes exactly one draft path"
                    )));
                }
                draft = Some(other.to_string());
            }
        }
        i += 1;
    }
    args.draft =
        draft.ok_or_else(|| CliError::Usage(format!("{command} requires a draft path")))?;
    Ok(args)
}

fn take(rest: &[String], i: usize, flag: &str) -> Result<String, CliError> {
    rest.get(i)
        .cloned()
        .ok_or_else(|| CliError::Usage(format!("{flag} requires a value")))
}

fn read(path: &str) -> Result<Vec<u8>, CliError> {
    std::fs::read(path).map_err(|e| CliError::Failure(format!("cannot read '{path}': {e}")))
}

fn print_value(json: bool, value: &Value, text: impl FnOnce()) {
    if json {
        println!("{}", print_canonical(value));
    } else {
        text();
    }
}

fn cmd_inspect(rest: &[String]) -> Result<ExitCode, CliError> {
    let args = parse_args(rest, "inspect")?;
    let draft = crate::inspect::open_to_draft(std::path::Path::new(&args.draft))?;
    println!("{draft}");
    Ok(ExitCode::SUCCESS)
}

fn cmd_validate(rest: &[String]) -> Result<ExitCode, CliError> {
    let args = parse_args(rest, "validate")?;
    let bytes = read(&args.draft)?;
    let report = pipeline::validate_json(&bytes)?;
    let errors = Value::Array(
        report
            .errors
            .iter()
            .map(|e| Value::String(e.clone()))
            .collect(),
    );
    let warnings = Value::Array(
        report
            .warnings
            .iter()
            .map(|w| Value::String(w.clone()))
            .collect(),
    );
    print_value(
        args.json,
        &Value::Object(vec![
            ("ok".into(), Value::Bool(report.is_ok())),
            ("errors".into(), errors),
            ("warnings".into(), warnings),
        ]),
        || {
            for e in &report.errors {
                println!("error: {e}");
            }
            for w in &report.warnings {
                println!("warning: {w}");
            }
            println!(
                "validate: {} error(s), {} warning(s)",
                report.errors.len(),
                report.warnings.len()
            );
        },
    );
    Ok(if report.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn cmd_identify(rest: &[String]) -> Result<ExitCode, CliError> {
    let args = parse_args(rest, "identify")?;
    let bytes = read(&args.draft)?;
    let mut parsed = draft::parse(&bytes)?;

    if let Some(path) = &args.mb_search_json {
        let doc = std::fs::read(path)
            .map_err(|e| CliError::Failure(format!("cannot read '{}': {e}", path.display())))?;
        let candidates = identify::identify_candidates(&parsed, &doc)?;
        let items: Vec<Value> = candidates
            .iter()
            .map(|c| {
                let opt = |v: &Option<String>| match v {
                    Some(s) => Value::String(s.clone()),
                    None => Value::Null,
                };
                Value::Object(vec![
                    ("releaseId".into(), opt(&c.release_id)),
                    ("releaseGroupId".into(), opt(&c.release_group_id)),
                    ("title".into(), opt(&c.title)),
                    ("artist".into(), opt(&c.artist)),
                    ("date".into(), opt(&c.date)),
                    ("country".into(), opt(&c.country)),
                    ("barcode".into(), opt(&c.barcode)),
                    (
                        "confidence".into(),
                        Value::String(c.confidence.as_str().into()),
                    ),
                ])
            })
            .collect();
        println!(
            "{}",
            print_canonical(&Value::Object(vec![(
                "candidates".into(),
                Value::Array(items)
            )]))
        );
        return Ok(ExitCode::SUCCESS);
    }

    let Some(path) = &args.mb_json else {
        return Err(CliError::Usage(
            "identify requires --mb-json or --mb-search-json".into(),
        ));
    };
    let doc = std::fs::read(path)
        .map_err(|e| CliError::Failure(format!("cannot read '{}': {e}", path.display())))?;
    let (confidence, applied) = identify::identify_apply(&mut parsed, &doc, args.mbid.as_deref())?;
    let value = Value::Object(vec![
        (
            "confidence".into(),
            Value::String(confidence.as_str().into()),
        ),
        ("applied".into(), Value::Bool(applied)),
        (
            "releaseId".into(),
            parsed
                .identifiers
                .as_ref()
                .and_then(|i| i.musicbrainz_release_id.clone())
                .map(Value::String)
                .unwrap_or(Value::Null),
        ),
        (
            "releaseGroupId".into(),
            parsed
                .identifiers
                .as_ref()
                .and_then(|i| i.musicbrainz_release_group_id.clone())
                .map(Value::String)
                .unwrap_or(Value::Null),
        ),
    ]);
    print_value(args.json, &value, || {
        println!("identify: {}", confidence.as_str());
        if !applied {
            println!("identify: no match applied");
        }
    });
    Ok(ExitCode::SUCCESS)
}

fn cmd_build(rest: &[String]) -> Result<ExitCode, CliError> {
    let args = parse_args(rest, "build")?;
    let output = args
        .output
        .clone()
        .ok_or_else(|| CliError::Usage("build requires -o <dir>".into()))?;
    let bytes = read(&args.draft)?;

    let doc = match &args.mb_json {
        None => None,
        Some(path) => Some(
            std::fs::read(path)
                .map_err(|e| CliError::Failure(format!("cannot read '{}': {e}", path.display())))?,
        ),
    };
    let identify = doc.as_ref().map(|doc| IdentifyRequest::Document {
        doc,
        asserted_mbid: args.mbid.as_deref(),
    });

    let options = PipelineOptions {
        quality: args.quality,
        waveform: args.waveform,
        loudness: args.loudness,
        mpak: args.mpak.clone(),
        replace: args.replace,
    };
    let request = AuthorRequest {
        draft_json: &bytes,
        output: &output,
        options,
        identify,
    };
    let outcome = run_pipeline(&request)?;

    let value = Value::Object(vec![
        ("ok".into(), Value::Bool(true)),
        (
            "output".into(),
            Value::String(outcome.output.display().to_string()),
        ),
        (
            "mpak".into(),
            match &outcome.mpak {
                Some(p) => Value::String(p.display().to_string()),
                None => Value::Null,
            },
        ),
        (
            "fingerprint".into(),
            Value::String(outcome.fingerprint.clone()),
        ),
        ("groupKey".into(), Value::String(outcome.group_key.clone())),
        (
            "releaseKey".into(),
            Value::String(outcome.release_key.clone()),
        ),
        (
            "errors".into(),
            Value::Number(outcome.report.errors() as f64),
        ),
        (
            "warnings".into(),
            Value::Number(outcome.report.warnings() as f64),
        ),
    ]);
    print_value(args.json, &value, || {
        println!(
            "built '{}' ({} error(s), {} warning(s))",
            outcome.output.display(),
            outcome.report.errors(),
            outcome.report.warnings()
        );
        if let Some(mpak) = &outcome.mpak {
            println!("packed '{}'", mpak.display());
        }
    });
    Ok(ExitCode::SUCCESS)
}
