use std::path::PathBuf;

use music_similarity_eval::Result;
use music_similarity_eval::review::{RunInput, build_review_set};

fn main() {
    if let Err(error) = run() {
        eprintln!("music-similarity-review-set: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut multi61 = None;
    let mut release61 = None;
    let mut multi62 = None;
    let mut release62 = None;
    let mut output = None;
    let mut queries = 15usize;
    let mut neighbors = 5usize;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--multi61" => multi61 = Some(PathBuf::from(next_value(&mut args, "--multi61")?)),
            "--release61" => release61 = Some(PathBuf::from(next_value(&mut args, "--release61")?)),
            "--multi62" => multi62 = Some(PathBuf::from(next_value(&mut args, "--multi62")?)),
            "--release62" => release62 = Some(PathBuf::from(next_value(&mut args, "--release62")?)),
            "--out" => output = Some(PathBuf::from(next_value(&mut args, "--out")?)),
            "--queries" => queries = next_value(&mut args, "--queries")?.parse()?,
            "--neighbors" => neighbors = next_value(&mut args, "--neighbors")?.parse()?,
            "-h" | "--help" => {
                println!(
                    "music-similarity-review-set --multi61 DIR --release61 DIR --multi62 DIR --release62 DIR --out DIR [--queries N] [--neighbors N]"
                );
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    let runs = vec![
        RunInput {
            label: "multi-hop61".to_string(),
            model_variant: "multi".to_string(),
            patch_hop: 61,
            directory: multi61.ok_or("--multi61 is required")?,
        },
        RunInput {
            label: "release-hop61".to_string(),
            model_variant: "release".to_string(),
            patch_hop: 61,
            directory: release61.ok_or("--release61 is required")?,
        },
        RunInput {
            label: "multi-hop62".to_string(),
            model_variant: "multi".to_string(),
            patch_hop: 62,
            directory: multi62.ok_or("--multi62 is required")?,
        },
        RunInput {
            label: "release-hop62".to_string(),
            model_variant: "release".to_string(),
            patch_hop: 62,
            directory: release62.ok_or("--release62 is required")?,
        },
    ];
    let output = output.ok_or("--out is required")?;
    let rows = build_review_set(&runs, queries, neighbors, &output)?;
    println!(
        "wrote {rows} blind review rows for {queries} queries to {}",
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
