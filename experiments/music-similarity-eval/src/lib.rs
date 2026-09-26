pub mod audio;
pub mod comparison;
pub mod corpus;
pub mod eval;
pub mod mel;
pub mod model;
pub mod report;
pub mod review;
pub mod sanity;
pub mod stratified;
pub mod worksheet;

use std::error::Error;
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Clone)]
pub struct Config {
    pub model_path: PathBuf,
    pub model_sha256: String,
    pub model_variant: String,
    pub library: PathBuf,
    pub output: PathBuf,
    pub albums: usize,
    pub tracks_per_album: usize,
    pub top_k: usize,
    pub patch_hop: usize,
}

impl Config {
    pub fn from_args() -> Result<Option<Self>> {
        let mut args = std::env::args().skip(1);
        let mut model_path = None;
        let mut model_sha256 = None;
        let mut model_variant = "multi".to_string();
        let mut library = None;
        let mut output = PathBuf::from("out");
        let mut albums = 15usize;
        let mut tracks_per_album = 3usize;
        let mut top_k = 10usize;
        let mut patch_hop = 61usize;

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--model" => model_path = Some(PathBuf::from(next_value(&mut args, "--model")?)),
                "--model-sha256" => model_sha256 = Some(next_value(&mut args, "--model-sha256")?),
                "--model-variant" => model_variant = next_value(&mut args, "--model-variant")?,
                "--library" => library = Some(PathBuf::from(next_value(&mut args, "--library")?)),
                "--out" => output = PathBuf::from(next_value(&mut args, "--out")?),
                "--albums" => albums = parse_value(&mut args, "--albums")?,
                "--tracks-per-album" => {
                    tracks_per_album = parse_value(&mut args, "--tracks-per-album")?
                }
                "--top-k" => top_k = parse_value(&mut args, "--top-k")?,
                "--patch-hop" => patch_hop = parse_value(&mut args, "--patch-hop")?,
                "-h" | "--help" => {
                    print_help();
                    return Ok(None);
                }
                other => return Err(format!("unknown argument: {other}").into()),
            }
        }

        let model_path = model_path.ok_or("--model is required")?;
        let model_sha256 = model_sha256.ok_or("--model-sha256 is required")?;
        let library = library.ok_or("--library is required")?;
        if albums == 0 || tracks_per_album == 0 || top_k == 0 || patch_hop == 0 {
            return Err("albums, tracks-per-album, top-k, and patch-hop must be positive".into());
        }
        if model_sha256.len() != 64 || !model_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("--model-sha256 must be a 64-character hexadecimal SHA-256".into());
        }
        if model_variant != "multi" && model_variant != "release" {
            return Err("--model-variant must be multi or release".into());
        }

        Ok(Some(Self {
            model_path,
            model_sha256: model_sha256.to_ascii_lowercase(),
            model_variant,
            library,
            output,
            albums,
            tracks_per_album,
            top_k,
            patch_hop,
        }))
    }
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
    T::Err: Error + Send + Sync + 'static,
{
    let value = next_value(args, flag)?;
    value
        .parse::<T>()
        .map_err(|error| format!("invalid value for {flag}: {error}").into())
}

fn print_help() {
    println!(
        "music-similarity-eval\n\n\
         Required:\n\
         \x20 --model PATH             externally supplied ONNX model\n\
         \x20 --model-sha256 SHA256   expected model digest\n\
         \x20 --model-variant NAME     multi or release (default: multi)\n\
         \x20 --library PATH          local music library root\n\n\
         Optional:\n\
         \x20 --out PATH              output directory (default: out)\n\
         \x20 --albums N              number of albums to select (default: 15)\n\
         \x20 --tracks-per-album N    tracks per album (default: 3)\n\
         \x20 --top-k N                neighbours in the report (default: 10)\n\
         \x20 --patch-hop N           model patch hop (default: 61)\n\
         \x20 -h, --help              show this help"
    );
}
