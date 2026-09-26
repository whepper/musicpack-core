use std::path::PathBuf;

use music_similarity_eval::Result;
use music_similarity_eval::sanity::{SanityRun, build_sanity_set};

fn main() {
    if let Err(error) = run() {
        eprintln!("music-similarity-sanity-set: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut run_directory = None;
    let mut library = None;
    let mut output = None;
    let mut queries = 20usize;
    let mut neighbors = 5usize;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--run" => run_directory = Some(PathBuf::from(next_value(&mut args, "--run")?)),
            "--library" => library = Some(PathBuf::from(next_value(&mut args, "--library")?)),
            "--out" => output = Some(PathBuf::from(next_value(&mut args, "--out")?)),
            "--queries" => queries = next_value(&mut args, "--queries")?.parse()?,
            "--neighbors" => neighbors = next_value(&mut args, "--neighbors")?.parse()?,
            "-h" | "--help" => {
                println!(
                    "music-similarity-sanity-set --run DIR --library DIR --out DIR [--queries N] [--neighbors N]"
                );
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    let run_directory = run_directory.ok_or("--run is required")?;
    let library = library.ok_or("--library is required")?;
    let output = output.ok_or("--out is required")?;
    let rows = build_sanity_set(
        &SanityRun {
            directory: run_directory,
            library_root: library,
        },
        queries,
        neighbors,
        &output,
    )?;
    println!(
        "wrote {rows} candidate recommendations for {queries} queries to {}",
        output.display()
    );
    Ok(())
}

fn next_value<I>(args: &mut I, flag: &str) -> Result<String>
where
    I: Iterator<Item = String>,
{
    args.next()
        .ok_or_else(|| format!("{flag} requires a value").into())
}
