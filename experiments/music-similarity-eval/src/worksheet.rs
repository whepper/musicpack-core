use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use serde::Deserialize;

use crate::Result;

#[derive(Debug, Deserialize)]
struct EmbeddingsFile {
    metadata: RunMetadata,
    tracks: Vec<EmbeddingTrack>,
}

#[derive(Debug, Deserialize)]
struct RunMetadata {
    model_name: String,
    model_sha256: String,
    patch_hop: usize,
    corpus_identity: String,
    embedding_dimensions: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct EmbeddingTrack {
    artist: String,
    album: String,
    title: String,
    path: String,
    duration_seconds: f64,
    source_sha256: String,
}

#[derive(Debug, Deserialize)]
struct NeighborsFile {
    tracks: Vec<NeighborQuery>,
}

#[derive(Debug, Deserialize)]
struct NeighborQuery {
    index: usize,
    track: NeighborTrack,
    neighbors: Vec<Neighbor>,
}

#[derive(Debug, Deserialize)]
struct NeighborTrack {
    path: String,
}

#[derive(Debug, Deserialize)]
struct Neighbor {
    rank: usize,
    #[serde(default)]
    track_index: Option<usize>,
    path: String,
    #[serde(default)]
    source_sha256: Option<String>,
    #[serde(default)]
    duration_seconds: Option<f64>,
    score: f32,
}

#[derive(Debug, Clone)]
struct TrackInfo {
    id: String,
    artist: String,
    album: String,
    title: String,
    path: String,
    duration_seconds: f64,
    source_sha256: String,
}

pub fn write_combined_worksheet(
    multi_dir: &Path,
    release_dir: &Path,
    output_csv: &Path,
) -> Result<usize> {
    let multi = load_run(multi_dir)?;
    let release = load_run(release_dir)?;
    if multi.metadata.corpus_identity != release.metadata.corpus_identity {
        return Err("multi and release runs do not share the same corpus identity".into());
    }
    if multi.tracks.len() != release.tracks.len() {
        return Err("multi and release runs have different track counts".into());
    }
    for (left, right) in multi.tracks.iter().zip(&release.tracks) {
        if left.source_sha256 != right.source_sha256 {
            return Err("multi and release track ordering differs".into());
        }
    }

    if let Some(parent) = output_csv.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = File::create(output_csv)?;
    writeln!(
        file,
        "model_variant,patch_hop,query_id,query_source_sha256,query_artist,query_album,query_title,query_duration_seconds,query_path,rank,neighbor_id,neighbor_source_sha256,neighbor_artist,neighbor_album,neighbor_title,neighbor_duration_seconds,neighbor_path,relation,cosine,rating,note"
    )?;

    let mut row_count = 0usize;
    for (label, run) in [("multi", &multi), ("release", &release)] {
        let by_path = run
            .tracks
            .iter()
            .enumerate()
            .map(|(index, track)| {
                (
                    track.path.clone(),
                    TrackInfo {
                        id: format!("T{:03}", index + 1),
                        artist: track.artist.clone(),
                        album: track.album.clone(),
                        title: track.title.clone(),
                        path: track.path.clone(),
                        duration_seconds: track.duration_seconds,
                        source_sha256: track.source_sha256.clone(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut queries = run.neighbors.tracks.iter().collect::<Vec<_>>();
        queries.sort_by_key(|query| query.index);
        for query in queries {
            let Some(query_info) = by_path.get(&query.track.path) else {
                return Err(format!(
                    "neighbor query path is absent from embeddings: {}",
                    query.track.path
                )
                .into());
            };
            for neighbor in &query.neighbors {
                let neighbor_info = neighbor
                    .track_index
                    .and_then(|index| run.tracks.get(index))
                    .and_then(|track| by_path.get(&track.path))
                    .cloned()
                    .or_else(|| by_path.get(&neighbor.path).cloned());
                let Some(neighbor_info) = neighbor_info else {
                    return Err(format!(
                        "neighbor path is absent from embeddings: {}",
                        neighbor.path
                    )
                    .into());
                };
                let source_sha256 = neighbor
                    .source_sha256
                    .clone()
                    .unwrap_or_else(|| neighbor_info.source_sha256.clone());
                let duration_seconds = neighbor
                    .duration_seconds
                    .unwrap_or(neighbor_info.duration_seconds);
                let relation = relation(
                    &query_info.artist,
                    &query_info.album,
                    &neighbor_info.artist,
                    &neighbor_info.album,
                );
                let fields = vec![
                    label.to_string(),
                    run.metadata.patch_hop.to_string(),
                    query_info.id.clone(),
                    source_field(&query_info.source_sha256),
                    field(&query_info.artist),
                    field(&query_info.album),
                    field(&query_info.title),
                    format!("{:.3}", query_info.duration_seconds),
                    field(&query_info.path),
                    neighbor.rank.to_string(),
                    neighbor_info.id.clone(),
                    source_field(&source_sha256),
                    field(&neighbor_info.artist),
                    field(&neighbor_info.album),
                    field(&neighbor_info.title),
                    format!("{duration_seconds:.3}"),
                    field(&neighbor_info.path),
                    relation.to_string(),
                    format!("{:.6}", neighbor.score),
                    String::new(),
                    String::new(),
                ];
                writeln!(file, "{}", fields.join(","))?;
                row_count += 1;
            }
        }
    }

    let instructions = output_csv.with_extension("md");
    write_instructions(&instructions, row_count, &multi.metadata, &release.metadata)?;
    Ok(row_count)
}

struct LoadedRun {
    metadata: RunMetadata,
    tracks: Vec<EmbeddingTrack>,
    neighbors: NeighborsFile,
}

fn load_run(dir: &Path) -> Result<LoadedRun> {
    let embeddings: EmbeddingsFile =
        serde_json::from_reader(File::open(dir.join("embeddings.json"))?)?;
    let neighbors: NeighborsFile =
        serde_json::from_reader(File::open(dir.join("neighbors.json"))?)?;
    Ok(LoadedRun {
        metadata: embeddings.metadata,
        tracks: embeddings.tracks,
        neighbors,
    })
}

fn relation(query_artist: &str, query_album: &str, artist: &str, album: &str) -> &'static str {
    if query_artist == artist && query_album == album {
        "same_album"
    } else if query_artist == artist {
        "same_artist"
    } else {
        "different_artist"
    }
}

fn source_field(value: &str) -> String {
    field(value)
}

fn field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn write_instructions(
    path: &Path,
    rows: usize,
    multi: &RunMetadata,
    release: &RunMetadata,
) -> Result<()> {
    let mut file = File::create(path)?;
    writeln!(file, "# Manual similarity review worksheet\n")?;
    writeln!(file, "Rows: {rows}\n")?;
    writeln!(
        file,
        "This worksheet is generated, not labeled. A human reviewer must fill the `rating` column.\n"
    )?;
    writeln!(file, "## Rating values\n")?;
    writeln!(
        file,
        "- `clearly_similar`\n- `similar`\n- `somewhat_related`\n- `not_similar`\n- `clearly_wrong`\n"
    )?;
    writeln!(
        file,
        "Use `note` for a short optional explanation. Do not infer labels from directory names or scores.\n"
    )?;
    writeln!(file, "## Review focus\n")?;
    writeln!(
        file,
        "- same artist and same album\n- different artists with similar style\n- same broad genre but clearly different sound\n- acoustic similarity without meaningful musical similarity\n- obvious false positives\n"
    )?;
    writeln!(file, "## Runs\n")?;
    writeln!(
        file,
        "- multi: `{}`, patch hop {}, {} dimensions, model SHA-256 `{}`",
        multi.model_name, multi.patch_hop, multi.embedding_dimensions, multi.model_sha256
    )?;
    writeln!(
        file,
        "- release: `{}`, patch hop {}, {} dimensions, model SHA-256 `{}`",
        release.model_name, release.patch_hop, release.embedding_dimensions, release.model_sha256
    )?;
    writeln!(
        file,
        "\nPaths and local metadata are present only in the external worksheet output; keep it outside Git.\n"
    )?;
    Ok(())
}
