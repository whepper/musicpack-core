use std::path::PathBuf;

use music_similarity_eval::Result;
use music_similarity_eval::stratified::{StratifiedRun, StratifiedTargets, build_stratified_set};

fn main() {
    if let Err(error) = run() {
        eprintln!("music-similarity-stratified-set: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut run_directory = None;
    let mut output = None;
    let mut targets = StratifiedTargets::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--run" => run_directory = Some(PathBuf::from(next_value(&mut args, "--run")?)),
            "--library" => {
                let _ = next_value(&mut args, "--library")?;
                return Err(
                    "--library is not accepted: this tool exposes audio through opaque symlinks, so it never needs the library root"
                        .into(),
                );
            }
            "--out" => output = Some(PathBuf::from(next_value(&mut args, "--out")?)),
            "--same-album-cases" => {
                targets.same_album_cases = parse_value(&mut args, "--same-album-cases")?
            }
            "--same-artist-cases" => {
                targets.same_artist_cases = parse_value(&mut args, "--same-artist-cases")?
            }
            "--different-artist-cases" => {
                targets.different_artist_cases = parse_value(&mut args, "--different-artist-cases")?
            }
            "--min-cases" => targets.min_total_cases = parse_value(&mut args, "--min-cases")?,
            "--max-cases" => targets.max_total_cases = parse_value(&mut args, "--max-cases")?,
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    let run_directory = run_directory.ok_or("--run is required")?;
    let output = output.ok_or("--out is required")?;
    let cases = build_stratified_set(&StratifiedRun { run_directory }, &targets, &output)?;
    println!(
        "wrote {cases} stratified review cases to {}",
        output.display()
    );
    Ok(())
}

fn print_help() {
    println!(
        "music-similarity-stratified-set\n\n\
         Required:\n\
         \x20 --run DIR               existing canonical run directory\n\
         \x20 --out DIR               output directory\n\n\
         Optional (stratum targets, defaults in brackets):\n\
         \x20 --same-album-cases N         [5]\n\
         \x20 --same-artist-cases N        [5]\n\
         \x20 --different-artist-cases N   [12]\n\
         \x20 --min-cases N                [20] minimum total, topped up in discovery\n\
         \x20 --max-cases N                [24] maximum total\n\n\
         Reads existing run output only; runs no inference and loads no model."
    );
}

fn next_value<I>(args: &mut I, flag: &str) -> Result<String>
where
    I: Iterator<Item = String>,
{
    args.next()
        .ok_or_else(|| format!("{flag} requires a value").into())
}

fn parse_value<T>(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let value = next_value(args, flag)?;
    value
        .parse::<T>()
        .map_err(|error| format!("invalid value for {flag}: {error}").into())
}
