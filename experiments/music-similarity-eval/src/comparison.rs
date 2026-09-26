use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::eval::cosine;

#[derive(Debug, Deserialize)]
struct EmbeddingsFile {
    metadata: Metadata,
    tracks: Vec<TrackEmbedding>,
}

#[derive(Debug, Deserialize)]
struct Metadata {
    model_name: String,
    model_sha256: String,
    patch_hop: usize,
    corpus_identity: String,
    embedding_dimensions: usize,
}

#[derive(Debug, Deserialize)]
struct TrackEmbedding {
    path: String,
    source_sha256: String,
    embedding_sha256: String,
    embedding: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct NeighborsFile {
    tracks: Vec<NeighborQuery>,
}

#[derive(Debug, Deserialize)]
struct NeighborQuery {
    index: usize,
    neighbors: Vec<Neighbor>,
}

#[derive(Debug, Deserialize)]
struct Neighbor {
    rank: usize,
    path: String,
    #[serde(default)]
    track_index: Option<usize>,
    score: f32,
}

#[derive(Debug)]
struct LoadedRun {
    metadata: Metadata,
    tracks: Vec<TrackEmbedding>,
    neighbors: Vec<NeighborQuery>,
    by_path: BTreeMap<String, usize>,
}

#[derive(Debug, Serialize)]
pub struct ComparisonReport {
    pub model_variant: String,
    pub model_sha256: String,
    pub patch_hop_a: usize,
    pub patch_hop_b: usize,
    pub corpus_identity: String,
    pub track_count: usize,
    pub hash_equal_count: usize,
    pub embedding_cosine_min: f32,
    pub embedding_cosine_mean: f32,
    pub embedding_cosine_max: f32,
    pub top_k_overlap_fraction_mean: f32,
    pub top_k_overlap_fraction_min: f32,
    pub top1_changed_count: usize,
    pub mean_abs_rank_delta: f32,
    pub max_abs_rank_delta: usize,
    pub mean_abs_score_delta: f32,
    pub tracks: Vec<TrackComparison>,
}

#[derive(Debug, Serialize)]
pub struct TrackComparison {
    pub id: String,
    pub source_sha256: String,
    pub hash_equal: bool,
    pub embedding_cosine: f32,
    pub top_k_overlap_count: usize,
    pub top_k_size: usize,
    pub top_k_overlap_fraction: f32,
    pub top1_changed: bool,
    pub mean_abs_rank_delta: Option<f32>,
    pub max_abs_rank_delta: usize,
    pub mean_abs_score_delta: Option<f32>,
}

pub fn compare_runs(a_dir: &Path, b_dir: &Path, output_dir: &Path) -> Result<ComparisonReport> {
    let a = load_run(a_dir)?;
    let b = load_run(b_dir)?;
    if a.metadata.model_sha256 != b.metadata.model_sha256 {
        return Err("comparison runs use different model artifacts".into());
    }
    if a.metadata.corpus_identity != b.metadata.corpus_identity {
        return Err("comparison runs use different corpora".into());
    }
    if a.metadata.model_name != b.metadata.model_name {
        return Err("comparison runs use different model variants".into());
    }
    if a.metadata.embedding_dimensions != b.metadata.embedding_dimensions {
        return Err("comparison runs use different embedding dimensions".into());
    }
    if a.tracks.len() != b.tracks.len() {
        return Err("comparison runs have different track counts".into());
    }

    let mut tracks = Vec::with_capacity(a.tracks.len());
    for (index, (left, right)) in a.tracks.iter().zip(&b.tracks).enumerate() {
        if left.source_sha256 != right.source_sha256 {
            return Err(format!("track ordering differs at index {index}").into());
        }
        let embedding_cosine = cosine(&left.embedding, &right.embedding);
        let hash_equal = left.embedding_sha256 == right.embedding_sha256;
        let left_neighbors = neighbor_map(&a, index)?;
        let right_neighbors = neighbor_map(&b, index)?;
        let common = left_neighbors
            .keys()
            .filter(|source| right_neighbors.contains_key(*source))
            .cloned()
            .collect::<BTreeSet<_>>();
        let top_k_size = left_neighbors.len().max(right_neighbors.len());
        let overlap_fraction = if top_k_size == 0 {
            f32::NAN
        } else {
            common.len() as f32 / top_k_size as f32
        };
        let top1_changed = left_neighbors.values().next() != right_neighbors.values().next();
        let rank_deltas = common
            .iter()
            .map(|source| (left_neighbors[source] as f64 - right_neighbors[source] as f64).abs())
            .collect::<Vec<_>>();
        let max_abs_rank_delta =
            rank_deltas.iter().copied().fold(0.0f64, f64::max).round() as usize;
        let score_deltas = common
            .iter()
            .map(|source| {
                (left_neighbors_score(&a, index, source.as_str())
                    - right_neighbors_score(&b, index, source.as_str()))
                .abs() as f64
            })
            .collect::<Vec<_>>();
        tracks.push(TrackComparison {
            id: format!("T{:03}", index + 1),
            source_sha256: left.source_sha256.clone(),
            hash_equal,
            embedding_cosine,
            top_k_overlap_count: common.len(),
            top_k_size,
            top_k_overlap_fraction: overlap_fraction,
            top1_changed,
            mean_abs_rank_delta: mean_f64(&rank_deltas).map(|value| value as f32),
            max_abs_rank_delta,
            mean_abs_score_delta: mean_f64(&score_deltas).map(|value| value as f32),
        });
    }

    let report = ComparisonReport {
        model_variant: a.metadata.model_name.clone(),
        model_sha256: a.metadata.model_sha256.clone(),
        patch_hop_a: a.metadata.patch_hop,
        patch_hop_b: b.metadata.patch_hop,
        corpus_identity: a.metadata.corpus_identity.clone(),
        track_count: tracks.len(),
        hash_equal_count: tracks.iter().filter(|track| track.hash_equal).count(),
        embedding_cosine_min: min_field(&tracks, |track| track.embedding_cosine),
        embedding_cosine_mean: mean_field(&tracks, |track| track.embedding_cosine),
        embedding_cosine_max: max_field(&tracks, |track| track.embedding_cosine),
        top_k_overlap_fraction_mean: mean_field(&tracks, |track| track.top_k_overlap_fraction),
        top_k_overlap_fraction_min: min_field(&tracks, |track| track.top_k_overlap_fraction),
        top1_changed_count: tracks.iter().filter(|track| track.top1_changed).count(),
        mean_abs_rank_delta: mean_optional(&tracks, |track| track.mean_abs_rank_delta),
        max_abs_rank_delta: tracks
            .iter()
            .map(|track| track.max_abs_rank_delta)
            .max()
            .unwrap_or(0),
        mean_abs_score_delta: mean_optional(&tracks, |track| track.mean_abs_score_delta),
        tracks,
    };
    fs::create_dir_all(output_dir)?;
    let json = File::create(output_dir.join("comparison.json"))?;
    serde_json::to_writer_pretty(json, &report)?;
    write_markdown(&output_dir.join("comparison.md"), &report)?;
    Ok(report)
}

fn load_run(dir: &Path) -> Result<LoadedRun> {
    let embeddings: EmbeddingsFile =
        serde_json::from_reader(File::open(dir.join("embeddings.json"))?)?;
    let neighbors: NeighborsFile =
        serde_json::from_reader(File::open(dir.join("neighbors.json"))?)?;
    let by_path = embeddings
        .tracks
        .iter()
        .enumerate()
        .map(|(index, track)| (track.path.clone(), index))
        .collect::<BTreeMap<_, _>>();
    Ok(LoadedRun {
        metadata: embeddings.metadata,
        tracks: embeddings.tracks,
        neighbors: neighbors.tracks,
        by_path,
    })
}

fn neighbor_map(run: &LoadedRun, query_index: usize) -> Result<BTreeMap<String, usize>> {
    let query = run
        .neighbors
        .iter()
        .find(|query| query.index == query_index)
        .ok_or_else(|| format!("missing neighbor query {query_index}"))?;
    query
        .neighbors
        .iter()
        .map(|neighbor| {
            let source = neighbor
                .track_index
                .and_then(|index| run.tracks.get(index))
                .or_else(|| {
                    run.by_path
                        .get(&neighbor.path)
                        .and_then(|index| run.tracks.get(*index))
                })
                .map(|track| track.source_sha256.clone())
                .unwrap_or_else(|| neighbor.path.clone());
            Ok((source, neighbor.rank))
        })
        .collect()
}

fn left_neighbors_score(run: &LoadedRun, query_index: usize, source: &str) -> f32 {
    score_for(run, query_index, source)
}

fn right_neighbors_score(run: &LoadedRun, query_index: usize, source: &str) -> f32 {
    score_for(run, query_index, source)
}

fn score_for(run: &LoadedRun, query_index: usize, source: &str) -> f32 {
    run.neighbors
        .iter()
        .find(|query| query.index == query_index)
        .and_then(|query| {
            query.neighbors.iter().find(|neighbor| {
                neighbor
                    .track_index
                    .and_then(|index| run.tracks.get(index))
                    .map(|track| track.source_sha256 == source)
                    .or_else(|| {
                        run.by_path
                            .get(&neighbor.path)
                            .and_then(|index| run.tracks.get(*index))
                            .map(|track| track.source_sha256 == source)
                    })
                    .unwrap_or(neighbor.path == source)
            })
        })
        .map(|neighbor| neighbor.score)
        .unwrap_or(f32::NAN)
}

fn mean_f64(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }
}

fn mean_field<T, F>(values: &[T], field: F) -> f32
where
    F: Fn(&T) -> f32,
{
    let values = values
        .iter()
        .map(field)
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    if values.is_empty() {
        f32::NAN
    } else {
        (values.iter().map(|value| f64::from(*value)).sum::<f64>() / values.len() as f64) as f32
    }
}

fn min_field<T, F>(values: &[T], field: F) -> f32
where
    F: Fn(&T) -> f32,
{
    values
        .iter()
        .map(field)
        .filter(|value| value.is_finite())
        .fold(f32::INFINITY, f32::min)
}

fn max_field<T, F>(values: &[T], field: F) -> f32
where
    F: Fn(&T) -> f32,
{
    values
        .iter()
        .map(field)
        .filter(|value| value.is_finite())
        .fold(f32::NEG_INFINITY, f32::max)
}

fn mean_optional<T, F>(values: &[T], field: F) -> f32
where
    F: Fn(&T) -> Option<f32>,
{
    let values = values
        .iter()
        .filter_map(field)
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    if values.is_empty() {
        f32::NAN
    } else {
        (values.iter().map(|value| f64::from(*value)).sum::<f64>() / values.len() as f64) as f32
    }
}

fn write_markdown(path: &Path, report: &ComparisonReport) -> Result<()> {
    let mut file = File::create(path)?;
    writeln!(file, "# Patch-hop comparison\n")?;
    writeln!(file, "- Model: `{}`", report.model_variant)?;
    writeln!(
        file,
        "- Patch hop: {} vs {}",
        report.patch_hop_a, report.patch_hop_b
    )?;
    writeln!(file, "- Tracks: {}", report.track_count)?;
    writeln!(
        file,
        "- Identical embedding hashes: {}/{}\n",
        report.hash_equal_count, report.track_count
    )?;
    writeln!(file, "## Embeddings\n")?;
    writeln!(
        file,
        "- Cosine min/mean/max: {:.6} / {:.6} / {:.6}",
        report.embedding_cosine_min, report.embedding_cosine_mean, report.embedding_cosine_max
    )?;
    writeln!(
        file,
        "- Mean absolute neighbour-score delta: {:.6}\n",
        report.mean_abs_score_delta
    )?;
    writeln!(file, "## Neighbour ordering\n")?;
    writeln!(
        file,
        "- Top-K overlap fraction mean/min: {:.4} / {:.4}",
        report.top_k_overlap_fraction_mean, report.top_k_overlap_fraction_min
    )?;
    writeln!(
        file,
        "- Queries with changed top-1: {}",
        report.top1_changed_count
    )?;
    writeln!(
        file,
        "- Mean absolute common-neighbour rank delta: {:.4}\n",
        report.mean_abs_rank_delta
    )?;
    writeln!(
        file,
        "No acceptance threshold is imposed; these are diagnostics, not a quality verdict.\n"
    )?;
    Ok(())
}
