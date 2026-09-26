use std::path::PathBuf;

use music_similarity_eval::Result;
use music_similarity_eval::comparison::compare_runs;

fn main() {
    if let Err(error) = run() {
        eprintln!("music-similarity-compare: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut a = None;
    let mut b = None;
    let mut output = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--a" => a = Some(PathBuf::from(next_value(&mut args, "--a")?)),
            "--b" => b = Some(PathBuf::from(next_value(&mut args, "--b")?)),
            "--out" => output = Some(PathBuf::from(next_value(&mut args, "--out")?)),
            "-h" | "--help" => {
                println!("music-similarity-compare --a DIR --b DIR --out DIR");
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    let a = a.ok_or("--a is required")?;
    let b = b.ok_or("--b is required")?;
    let output = output.ok_or("--out is required")?;
    let report = compare_runs(&a, &b, &output)?;
    println!(
        "compared {} tracks: hash_equal={}, top1_changed={}, overlap_mean={:.4}",
        report.track_count,
        report.hash_equal_count,
        report.top1_changed_count,
        report.top_k_overlap_fraction_mean
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
