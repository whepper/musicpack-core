use std::path::PathBuf;

use music_similarity_eval::Result;
use music_similarity_eval::worksheet::write_combined_worksheet;

fn main() {
    if let Err(error) = run() {
        eprintln!("music-similarity-worksheet: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut multi = None;
    let mut release = None;
    let mut output = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--multi" => multi = Some(PathBuf::from(next_value(&mut args, "--multi")?)),
            "--release" => release = Some(PathBuf::from(next_value(&mut args, "--release")?)),
            "--out" => output = Some(PathBuf::from(next_value(&mut args, "--out")?)),
            "-h" | "--help" => {
                println!("music-similarity-worksheet --multi DIR --release DIR --out FILE");
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    let multi = multi.ok_or("--multi is required")?;
    let release = release.ok_or("--release is required")?;
    let output = output.ok_or("--out is required")?;
    let rows = write_combined_worksheet(&multi, &release, &output)?;
    println!(
        "wrote {rows} blank-label review rows to {}",
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
