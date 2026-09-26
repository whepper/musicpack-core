use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use music_similarity_eval::Result;
use music_similarity_eval::audio;
use music_similarity_eval::corpus;
use music_similarity_eval::eval::{TrackEmbedding, cosine, nearest, pool_mean_norm};
use music_similarity_eval::mel::MelFrontend;
use music_similarity_eval::model::ModelRunner;
use serde::Serialize;

const DEFAULT_PAIR_LIMIT: usize = 5;

#[derive(Debug)]
struct Args {
    model: PathBuf,
    model_sha256: String,
    model_variant: String,
    library: PathBuf,
    output: PathBuf,
    pair_limit: usize,
    patch_hop: usize,
    top_k: usize,
    list_only: bool,
}

#[derive(Debug, Clone)]
struct Analyzed {
    path: PathBuf,
    source_sha256: String,
    duration_seconds: f64,
    frame_count: usize,
    patch_count: usize,
    embedding_sha256: String,
    vector: Vec<f32>,
}

#[derive(Debug, Serialize)]
struct PairReport {
    pair_id: String,
    flac_path: String,
    mpc_path: String,
    flac_source_sha256: String,
    mpc_source_sha256: String,
    flac_duration_seconds: f64,
    mpc_duration_seconds: f64,
    duration_delta_seconds: f64,
    flac_embedding_sha256: String,
    mpc_embedding_sha256: String,
    embedding_hash_equal: bool,
    embedding_cosine: f32,
    flac_counterpart_rank: Option<usize>,
    mpc_counterpart_rank: Option<usize>,
}

#[derive(Debug, Serialize)]
struct Report {
    model_variant: String,
    model_sha256: String,
    patch_hop: usize,
    candidate_pair_count: usize,
    evaluated_pair_count: usize,
    skipped_pair_count: usize,
    pairs: Vec<PairReport>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("music-similarity-cross-codec: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = parse_args()?;
    let (candidate_count, pairs) = corpus::find_cross_codec_pairs(&args.library, args.pair_limit)?;
    println!("cross-codec candidate pairs: {candidate_count}");
    if args.list_only {
        return Ok(());
    }
    if pairs.is_empty() {
        return write_report(&args, candidate_count, 0, 0, Vec::new());
    }

    let actual_hash = audio::sha256_file(&args.model)?;
    if actual_hash != args.model_sha256 {
        return Err(format!(
            "model SHA-256 mismatch: expected {}, got {}",
            args.model_sha256, actual_hash
        )
        .into());
    }
    let frontend = MelFrontend::new(args.patch_hop)?;
    let model = ModelRunner::load(&args.model)?;
    let mut analyzed = Vec::new();
    let mut skipped = 0usize;
    for pair in &pairs {
        let flac = analyze(&frontend, &model, &pair.flac)?;
        let mpc = analyze(&frontend, &model, &pair.mpc)?;
        if let (Some(flac), Some(mpc)) = (flac, mpc) {
            analyzed.push((pair.clone(), flac, mpc));
        } else {
            skipped += 1;
        }
    }
    let records = analyzed
        .iter()
        .flat_map(|(_, flac, mpc)| [record(flac), record(mpc)])
        .collect::<Vec<_>>();
    let mut pair_reports = Vec::new();
    for (pair_index, (pair, flac, mpc)) in analyzed.iter().enumerate() {
        let flac_record_index = pair_index * 2;
        let mpc_record_index = flac_record_index + 1;
        let flac_neighbors = nearest(&records, flac_record_index, args.top_k);
        let mpc_neighbors = nearest(&records, mpc_record_index, args.top_k);
        pair_reports.push(PairReport {
            pair_id: format!("P{:02}", pair_index + 1),
            flac_path: pair.flac.display().to_string(),
            mpc_path: pair.mpc.display().to_string(),
            flac_source_sha256: flac.source_sha256.clone(),
            mpc_source_sha256: mpc.source_sha256.clone(),
            flac_duration_seconds: flac.duration_seconds,
            mpc_duration_seconds: mpc.duration_seconds,
            duration_delta_seconds: (flac.duration_seconds - mpc.duration_seconds).abs(),
            flac_embedding_sha256: flac.embedding_sha256.clone(),
            mpc_embedding_sha256: mpc.embedding_sha256.clone(),
            embedding_hash_equal: flac.embedding_sha256 == mpc.embedding_sha256,
            embedding_cosine: cosine(&flac.vector, &mpc.vector),
            flac_counterpart_rank: flac_neighbors
                .iter()
                .position(|neighbor| neighbor.index == mpc_record_index)
                .map(|rank| rank + 1),
            mpc_counterpart_rank: mpc_neighbors
                .iter()
                .position(|neighbor| neighbor.index == flac_record_index)
                .map(|rank| rank + 1),
        });
    }
    write_report(
        &args,
        candidate_count,
        pair_reports.len(),
        skipped,
        pair_reports,
    )
}

fn analyze(frontend: &MelFrontend, model: &ModelRunner, path: &Path) -> Result<Option<Analyzed>> {
    let decoded = audio::decode_and_prepare(path)?;
    let output = frontend.process(&decoded.samples);
    if output.patches.is_empty() {
        return Ok(None);
    }
    let windows = model.run_patches(&output.patches)?;
    let vector = pool_mean_norm(&windows, model.embedding_dim())?;
    Ok(Some(Analyzed {
        path: path.to_path_buf(),
        source_sha256: decoded.source_sha256,
        duration_seconds: decoded.duration_seconds,
        frame_count: output.frame_count,
        patch_count: output.patches.len(),
        embedding_sha256: audio::sha256_f32(&vector),
        vector,
    }))
}

fn record(analyzed: &Analyzed) -> TrackEmbedding {
    TrackEmbedding {
        spec: corpus::TrackSpec {
            path: analyzed.path.clone(),
            artist: String::new(),
            album: String::new(),
            title: String::new(),
        },
        duration_seconds: analyzed.duration_seconds,
        source_sha256: analyzed.source_sha256.clone(),
        frame_count: analyzed.frame_count,
        patch_count: analyzed.patch_count,
        embedding_sha256: analyzed.embedding_sha256.clone(),
        vector: analyzed.vector.clone(),
    }
}

fn write_report(
    args: &Args,
    candidate_count: usize,
    evaluated_pair_count: usize,
    skipped_pair_count: usize,
    pairs: Vec<PairReport>,
) -> Result<()> {
    fs::create_dir_all(&args.output)?;
    let report = Report {
        model_variant: args.model_variant.clone(),
        model_sha256: args.model_sha256.clone(),
        patch_hop: args.patch_hop,
        candidate_pair_count: candidate_count,
        evaluated_pair_count,
        skipped_pair_count,
        pairs,
    };
    let json = File::create(args.output.join("cross-codec.json"))?;
    serde_json::to_writer_pretty(json, &report)?;
    let mut markdown = File::create(args.output.join("cross-codec.md"))?;
    writeln!(markdown, "# Cross-codec stability\n")?;
    writeln!(markdown, "- Model: `{}`", report.model_variant)?;
    writeln!(markdown, "- Patch hop: {}", report.patch_hop)?;
    writeln!(
        markdown,
        "- Selection: first N candidates in deterministic path order"
    )?;
    writeln!(
        markdown,
        "- Candidate artist/album/stem FLAC/Musepack pairs: {}",
        report.candidate_pair_count
    )?;
    writeln!(
        markdown,
        "- Evaluated pairs: {}",
        report.evaluated_pair_count
    )?;
    writeln!(markdown, "- Skipped pairs: {}\n", report.skipped_pair_count)?;
    if report.pairs.is_empty() {
        writeln!(
            markdown,
            "No candidate pairs were available; no cross-codec conclusion is drawn.\n"
        )?;
        return Ok(());
    }
    let cosines = report
        .pairs
        .iter()
        .map(|pair| pair.embedding_cosine)
        .collect::<Vec<_>>();
    let mean = cosines.iter().map(|value| f64::from(*value)).sum::<f64>() / cosines.len() as f64;
    let min = cosines.iter().copied().fold(f32::INFINITY, f32::min);
    let max = cosines.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    writeln!(markdown, "## Embedding agreement\n")?;
    writeln!(
        markdown,
        "- Cosine min/mean/max: {:.6} / {:.6} / {:.6}",
        min, mean, max
    )?;
    writeln!(
        markdown,
        "- Identical embedding hashes: {}/{}\n",
        report
            .pairs
            .iter()
            .filter(|pair| pair.embedding_hash_equal)
            .count(),
        report.pairs.len()
    )?;
    writeln!(markdown, "## Pair diagnostics\n")?;
    for pair in &report.pairs {
        writeln!(
            markdown,
            "- {}: duration delta {:.3}s, cosine {:.6}, counterpart ranks FLAC/MPC {}/{}",
            pair.pair_id,
            pair.duration_delta_seconds,
            pair.embedding_cosine,
            rank_display(pair.flac_counterpart_rank),
            rank_display(pair.mpc_counterpart_rank)
        )?;
    }
    writeln!(
        markdown,
        "\nThese are practical stability diagnostics, not a quality verdict or an assertion of identical source masters.\n"
    )?;
    Ok(())
}

fn rank_display(rank: Option<usize>) -> String {
    rank.map_or_else(|| "outside-top-k".to_string(), |value| value.to_string())
}

fn parse_args() -> Result<Args> {
    let mut model = None;
    let mut model_sha256 = None;
    let mut model_variant = "multi".to_string();
    let mut library = None;
    let mut output = PathBuf::from("out");
    let mut pair_limit = DEFAULT_PAIR_LIMIT;
    let mut patch_hop = 61usize;
    let mut top_k = 5usize;
    let mut list_only = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--model" => model = Some(PathBuf::from(next_value(&mut args, "--model")?)),
            "--model-sha256" => model_sha256 = Some(next_value(&mut args, "--model-sha256")?),
            "--model-variant" => model_variant = next_value(&mut args, "--model-variant")?,
            "--library" => library = Some(PathBuf::from(next_value(&mut args, "--library")?)),
            "--out" => output = PathBuf::from(next_value(&mut args, "--out")?),
            "--pairs" => pair_limit = next_value(&mut args, "--pairs")?.parse()?,
            "--patch-hop" => patch_hop = next_value(&mut args, "--patch-hop")?.parse()?,
            "--top-k" => top_k = next_value(&mut args, "--top-k")?.parse()?,
            "--list-only" => list_only = true,
            "-h" | "--help" => {
                println!(
                    "music-similarity-cross-codec --library DIR [--model PATH --model-sha256 SHA --model-variant NAME] --out DIR [--pairs N] [--patch-hop N] [--top-k N] [--list-only]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    let library = library.ok_or("--library is required")?;
    if list_only {
        return Ok(Args {
            model: PathBuf::new(),
            model_sha256: String::new(),
            model_variant,
            library,
            output,
            pair_limit,
            patch_hop,
            top_k,
            list_only,
        });
    }
    Ok(Args {
        model: model.ok_or("--model is required")?,
        model_sha256: model_sha256.ok_or("--model-sha256 is required")?,
        model_variant,
        library,
        output,
        pair_limit,
        patch_hop,
        top_k,
        list_only,
    })
}

fn next_value<I>(args: &mut I, flag: &str) -> Result<String>
where
    I: Iterator<Item = String>,
{
    args.next()
        .ok_or_else(|| format!("{flag} requires a value").into())
}
