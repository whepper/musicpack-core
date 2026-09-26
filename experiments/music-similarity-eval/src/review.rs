use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::audio::sha256_text;

#[derive(Debug, Clone)]
pub struct RunInput {
    pub label: String,
    pub model_variant: String,
    pub patch_hop: usize,
    pub directory: std::path::PathBuf,
}

#[derive(Debug, Deserialize)]
struct EmbeddingsFile {
    metadata: Metadata,
    tracks: Vec<Track>,
}

#[derive(Debug, Deserialize)]
struct Metadata {
    model_name: String,
    model_sha256: String,
    patch_hop: usize,
    corpus_identity: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Track {
    artist: String,
    album: String,
    title: String,
    path: String,
    source_sha256: String,
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
    input: RunInput,
    metadata: Metadata,
    tracks: Vec<Track>,
    neighbors: Vec<NeighborQuery>,
    by_path: BTreeMap<String, usize>,
}

#[derive(Debug, Clone)]
struct SelectedQuery {
    id: String,
    index: usize,
    source_sha256: String,
}

#[derive(Debug, Clone)]
struct ReviewRow {
    review_id: String,
    query_id: String,
    candidate_id: String,
    query: SelectedQuery,
    query_artist: String,
    query_album: String,
    query_title: String,
    query_path: String,
    candidate: Track,
    run: RunInput,
    rank: usize,
    score: f32,
    relation: String,
    shuffle_key: String,
}

#[derive(Debug, Serialize)]
struct Manifest {
    query_count: usize,
    neighbors_per_query_per_condition: usize,
    condition_count: usize,
    row_count: usize,
    distinct_query_artists: usize,
    distinct_candidate_artists: usize,
    relation_counts: BTreeMap<String, usize>,
    condition_relation_counts: BTreeMap<String, BTreeMap<String, usize>>,
    selection_method: String,
    conditions: Vec<ManifestCondition>,
}

#[derive(Debug, Serialize)]
struct ManifestCondition {
    label: String,
    model_variant: String,
    patch_hop: usize,
    model_sha256: String,
}

#[derive(Debug, Serialize)]
struct MappingRow {
    review_id: String,
    query_id: String,
    candidate_id: String,
    query_source_sha256: String,
    candidate_source_sha256: String,
    model_variant: String,
    patch_hop: usize,
    rank: usize,
    cosine: f32,
    relation: String,
    query_artist: String,
    query_album: String,
    query_title: String,
    query_path: String,
    candidate_artist: String,
    candidate_album: String,
    candidate_title: String,
    candidate_path: String,
}

pub fn build_review_set(
    runs: &[RunInput],
    query_count: usize,
    neighbors_per_query: usize,
    output_dir: &Path,
) -> Result<usize> {
    if runs.is_empty() || query_count == 0 || neighbors_per_query == 0 {
        return Err("at least one run and positive sample sizes are required".into());
    }
    let loaded = runs
        .iter()
        .cloned()
        .map(load_run)
        .collect::<Result<Vec<_>>>()?;
    validate_runs(&loaded)?;
    let selected = select_queries(&loaded[0], query_count)?;
    let mut rows = Vec::new();
    for query in &selected {
        for run in &loaded {
            let query_neighbors = run
                .neighbors
                .iter()
                .find(|entry| entry.index == query.index)
                .ok_or_else(|| format!("missing neighbours for query {}", query.index))?;
            if query_neighbors.neighbors.len() < neighbors_per_query {
                return Err(format!(
                    "query {} has fewer than {} neighbours in {}",
                    query.index, neighbors_per_query, run.input.label
                )
                .into());
            }
            for neighbor in query_neighbors.neighbors.iter().take(neighbors_per_query) {
                let candidate_index = neighbor
                    .track_index
                    .or_else(|| run.by_path.get(&neighbor.path).copied())
                    .ok_or_else(|| format!("cannot identify neighbour {}", neighbor.path))?;
                let candidate = run
                    .tracks
                    .get(candidate_index)
                    .ok_or_else(|| format!("neighbour index out of range: {candidate_index}"))?
                    .clone();
                let query_track = &loaded[0].tracks[query.index];
                let relation = relation(
                    &query_track.artist,
                    &query_track.album,
                    &candidate.artist,
                    &candidate.album,
                );
                let shuffle_key = sha256_text(&format!(
                    "blind-review\0{}\0{}\0{}",
                    query.source_sha256, candidate.source_sha256, run.input.label
                ));
                rows.push(ReviewRow {
                    review_id: String::new(),
                    query_id: query.id.clone(),
                    candidate_id: String::new(),
                    query: query.clone(),
                    query_artist: query_track.artist.clone(),
                    query_album: query_track.album.clone(),
                    query_title: query_track.title.clone(),
                    query_path: query_track.path.clone(),
                    candidate,
                    run: run.input.clone(),
                    rank: neighbor.rank,
                    score: neighbor.score,
                    relation,
                    shuffle_key,
                });
            }
        }
    }

    // Group by anonymous query, then deterministically shuffle within each
    // query. The shuffle key never exposes the condition in the blind sheet.
    rows.sort_by(|left, right| {
        left.query_id
            .cmp(&right.query_id)
            .then_with(|| left.shuffle_key.cmp(&right.shuffle_key))
    });
    for (index, row) in rows.iter_mut().enumerate() {
        row.review_id = format!("R{:04}", index + 1);
        row.candidate_id = format!("C{:04}", index + 1);
    }

    fs::create_dir_all(output_dir)?;
    write_blind_csv(&output_dir.join("review-set.csv"), &rows)?;
    write_mapping_csv(&output_dir.join("review-set-mapping.csv"), &rows)?;
    write_manifest(
        output_dir,
        runs,
        &loaded,
        query_count,
        neighbors_per_query,
        &rows,
    )?;
    write_instructions(
        output_dir,
        query_count,
        neighbors_per_query,
        runs,
        rows.len(),
    )?;
    Ok(rows.len())
}

fn load_run(input: RunInput) -> Result<LoadedRun> {
    let embeddings: EmbeddingsFile =
        serde_json::from_reader(File::open(input.directory.join("embeddings.json"))?)?;
    let neighbors: NeighborsFile =
        serde_json::from_reader(File::open(input.directory.join("neighbors.json"))?)?;
    let by_path = embeddings
        .tracks
        .iter()
        .enumerate()
        .map(|(index, track)| (track.path.clone(), index))
        .collect();
    Ok(LoadedRun {
        input,
        metadata: embeddings.metadata,
        tracks: embeddings.tracks,
        neighbors: neighbors.tracks,
        by_path,
    })
}

fn validate_runs(runs: &[LoadedRun]) -> Result<()> {
    let first = &runs[0];
    for run in runs {
        if run.metadata.patch_hop != run.input.patch_hop {
            return Err(format!(
                "run {} patch hop does not match its metadata",
                run.input.label
            )
            .into());
        }
        if run.metadata.corpus_identity != first.metadata.corpus_identity {
            return Err(format!("run {} uses a different corpus identity", run.input.label).into());
        }
        if run.tracks.len() != first.tracks.len() {
            return Err(format!("run {} has a different track count", run.input.label).into());
        }
        for (index, (left, right)) in first.tracks.iter().zip(&run.tracks).enumerate() {
            if left.source_sha256 != right.source_sha256 {
                return Err(format!(
                    "run {} track order differs at index {index}",
                    run.input.label
                )
                .into());
            }
        }
    }
    Ok(())
}

fn select_queries(run: &LoadedRun, query_count: usize) -> Result<Vec<SelectedQuery>> {
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, track) in run.tracks.iter().enumerate() {
        let key = format!("{}\0{}", track.artist, track.album);
        groups.entry(key).or_default().push(index);
    }
    if query_count > run.tracks.len() {
        return Err("query count exceeds available tracks".into());
    }
    let mut group_keys = groups.keys().cloned().collect::<Vec<_>>();
    group_keys.sort_by_key(|key| sha256_text(&format!("album-group\0{key}")));
    let mut selected = Vec::new();
    for key in group_keys.iter().take(query_count) {
        let index = groups[key]
            .iter()
            .copied()
            .min_by_key(|index| &run.tracks[*index].source_sha256)
            .ok_or("empty album group")?;
        selected.push(index);
    }
    if selected.len() < query_count {
        let used = selected.iter().copied().collect::<BTreeSet<_>>();
        let mut remaining = (0..run.tracks.len())
            .filter(|index| !used.contains(index))
            .collect::<Vec<_>>();
        remaining.sort_by_key(|index| &run.tracks[*index].source_sha256);
        selected.extend(remaining.into_iter().take(query_count - selected.len()));
    }
    selected.sort_by_key(|index| &run.tracks[*index].source_sha256);
    Ok(selected
        .into_iter()
        .enumerate()
        .map(|(index, track_index)| SelectedQuery {
            id: format!("Q{:02}", index + 1),
            index: track_index,
            source_sha256: run.tracks[track_index].source_sha256.clone(),
        })
        .collect())
}

fn relation(query_artist: &str, query_album: &str, artist: &str, album: &str) -> String {
    if query_artist == artist && query_album == album {
        "same_album".to_string()
    } else if query_artist == artist {
        "same_artist".to_string()
    } else {
        "different_artist".to_string()
    }
}

fn write_blind_csv(path: &Path, rows: &[ReviewRow]) -> Result<()> {
    let mut file = File::create(path)?;
    writeln!(file, "review_id,query_id,candidate_id,rating,note")?;
    for row in rows {
        let fields = [
            row.review_id.clone(),
            row.query_id.clone(),
            row.candidate_id.clone(),
            String::new(),
            String::new(),
        ];
        writeln!(file, "{}", fields.join(","))?;
    }
    Ok(())
}

fn write_mapping_csv(path: &Path, rows: &[ReviewRow]) -> Result<()> {
    let mut file = File::create(path)?;
    writeln!(
        file,
        "review_id,query_id,candidate_id,query_source_sha256,candidate_source_sha256,model_variant,patch_hop,rank,cosine,relation,query_artist,query_album,query_title,query_path,candidate_artist,candidate_album,candidate_title,candidate_path"
    )?;
    for row in rows {
        let mapping = MappingRow {
            review_id: row.review_id.clone(),
            query_id: row.query_id.clone(),
            candidate_id: row.candidate_id.clone(),
            query_source_sha256: row.query.source_sha256.clone(),
            candidate_source_sha256: row.candidate.source_sha256.clone(),
            model_variant: row.run.model_variant.clone(),
            patch_hop: row.run.patch_hop,
            rank: row.rank,
            cosine: row.score,
            relation: row.relation.clone(),
            query_artist: row.query_artist.clone(),
            query_album: row.query_album.clone(),
            query_title: row.query_title.clone(),
            query_path: row.query_path.clone(),
            candidate_artist: row.candidate.artist.clone(),
            candidate_album: row.candidate.album.clone(),
            candidate_title: row.candidate.title.clone(),
            candidate_path: row.candidate.path.clone(),
        };
        let fields = vec![
            mapping.review_id,
            mapping.query_id,
            mapping.candidate_id,
            mapping.query_source_sha256,
            mapping.candidate_source_sha256,
            mapping.model_variant,
            mapping.patch_hop.to_string(),
            mapping.rank.to_string(),
            format!("{:.6}", mapping.cosine),
            mapping.relation,
            csv_field(&mapping.query_artist),
            csv_field(&mapping.query_album),
            csv_field(&mapping.query_title),
            csv_field(&mapping.query_path),
            csv_field(&mapping.candidate_artist),
            csv_field(&mapping.candidate_album),
            csv_field(&mapping.candidate_title),
            csv_field(&mapping.candidate_path),
        ];
        writeln!(file, "{}", fields.join(","))?;
    }
    Ok(())
}

fn write_manifest(
    output_dir: &Path,
    runs: &[RunInput],
    loaded: &[LoadedRun],
    query_count: usize,
    neighbors_per_query: usize,
    rows: &[ReviewRow],
) -> Result<()> {
    let conditions = loaded
        .iter()
        .map(|run| ManifestCondition {
            label: run.input.label.clone(),
            model_variant: run.metadata.model_name.clone(),
            patch_hop: run.metadata.patch_hop,
            model_sha256: run.metadata.model_sha256.clone(),
        })
        .collect::<Vec<_>>();
    let mut relation_counts = BTreeMap::new();
    let mut condition_relation_counts: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut query_artists = BTreeSet::new();
    let mut candidate_artists = BTreeSet::new();
    for row in rows {
        *relation_counts.entry(row.relation.clone()).or_default() += 1;
        *condition_relation_counts
            .entry(row.run.label.clone())
            .or_default()
            .entry(row.relation.clone())
            .or_default() += 1;
        query_artists.insert(row.query_artist.clone());
        candidate_artists.insert(row.candidate.artist.clone());
    }
    let manifest = Manifest {
        query_count,
        neighbors_per_query_per_condition: neighbors_per_query,
        condition_count: runs.len(),
        row_count: rows.len(),
        distinct_query_artists: query_artists.len(),
        distinct_candidate_artists: candidate_artists.len(),
        relation_counts,
        condition_relation_counts,
        selection_method: format!(
            "Select {query_count} album groups by SHA-256 of a directory-derived artist/album key; choose the lowest source SHA-256 track in each group; if there are fewer groups, fill remaining slots by source SHA-256; assign anonymous query IDs by source hash. No neighbour score, relation, proxy metric, or rating is used. Take the first {neighbors_per_query} neighbours from each supplied run condition, then deterministically shuffle within each query using a hash key."
        ),
        conditions,
    };
    let file = File::create(output_dir.join("review-set-manifest.json"))?;
    serde_json::to_writer_pretty(file, &manifest)?;
    Ok(())
}

fn write_instructions(
    output_dir: &Path,
    query_count: usize,
    neighbors_per_query: usize,
    runs: &[RunInput],
    row_count: usize,
) -> Result<()> {
    let mut file = File::create(output_dir.join("review-set.md"))?;
    writeln!(file, "# Blind music-similarity review set\n")?;
    writeln!(file, "Rows: {row_count}\n")?;
    writeln!(
        file,
        "Queries: {query_count}; neighbours per query per condition: {neighbors_per_query}; conditions: {}\n",
        runs.len()
    )?;
    writeln!(file, "## Procedure\n")?;
    writeln!(
        file,
        "1. Review `review-set.csv` without opening the mapping file.\n"
    )?;
    writeln!(
        file,
        "2. For each `review_id`, enter one rating: `clearly_similar`, `similar`, `somewhat_related`, `not_similar`, or `clearly_wrong`.\n"
    )?;
    writeln!(
        file,
        "3. Add an optional short note when the reason is useful.\n"
    )?;
    writeln!(
        file,
        "4. Return the completed sheet; keep `review-set-mapping.csv` private until analysis.\n"
    )?;
    writeln!(
        file,
        "The primary sheet contains no model, hop, path, artist, album, score, or relation information. The mapping retains those fields for later analysis.\n"
    )?;
    Ok(())
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}
