use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::audio::sha256_text;

#[derive(Debug, Clone)]
pub struct SanityRun {
    pub directory: PathBuf,
    pub library_root: PathBuf,
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
    metadata: Metadata,
    tracks: Vec<Track>,
    neighbors: Vec<NeighborQuery>,
    by_path: BTreeMap<String, usize>,
}

#[derive(Debug, Clone)]
struct QuerySelection {
    id: String,
    index: usize,
    source_sha256: String,
}

#[derive(Debug, Clone)]
struct Row {
    review_id: String,
    query_id: String,
    query_track: String,
    candidate_id: String,
    candidate_track: String,
    query: QuerySelection,
    query_metadata: Track,
    candidate: Track,
    original_rank: usize,
    cosine: f32,
    relation: String,
    shuffle_key: String,
}

#[derive(Debug, Serialize)]
struct Manifest {
    query_count: usize,
    candidate_count: usize,
    neighbor_count_per_query: usize,
    source_model: String,
    source_model_sha256: String,
    source_corpus_identity: String,
    source_patch_hop: usize,
    distinct_query_artists: usize,
    distinct_query_albums: usize,
    distinct_candidate_artists: usize,
    different_artist_rows: usize,
    selection_method: String,
}

#[derive(Debug, Serialize)]
struct MappingRow {
    review_id: String,
    query_id: String,
    candidate_id: String,
    model_variant: String,
    model_sha256: String,
    patch_hop: usize,
    original_rank: usize,
    cosine: f32,
    relation: String,
    query_source_sha256: String,
    candidate_source_sha256: String,
    query_artist: String,
    query_album: String,
    query_title: String,
    query_path: String,
    candidate_artist: String,
    candidate_album: String,
    candidate_title: String,
    candidate_path: String,
}

pub fn build_sanity_set(
    run: &SanityRun,
    query_count: usize,
    neighbor_count: usize,
    output_dir: &Path,
) -> Result<usize> {
    if query_count == 0 || neighbor_count == 0 {
        return Err("query and neighbour counts must be positive".into());
    }
    let loaded = load_run(run)?;
    let queries = select_queries(&loaded, query_count)?;
    let mut rows = Vec::with_capacity(queries.len() * neighbor_count);
    for query in &queries {
        let query_metadata = loaded
            .tracks
            .get(query.index)
            .ok_or("selected query index out of range")?
            .clone();
        let neighbors = loaded
            .neighbors
            .iter()
            .find(|entry| entry.index == query.index)
            .ok_or("selected query has no neighbour list")?;
        if neighbors.neighbors.len() < neighbor_count {
            return Err(format!(
                "query {} has fewer than {neighbor_count} neighbours",
                query.index
            )
            .into());
        }
        for neighbor in neighbors.neighbors.iter().take(neighbor_count) {
            let candidate_index = neighbor
                .track_index
                .or_else(|| loaded.by_path.get(&neighbor.path).copied())
                .ok_or("cannot identify neighbour track")?;
            let candidate = loaded
                .tracks
                .get(candidate_index)
                .ok_or("neighbour index out of range")?
                .clone();
            let relation = relation(
                &query_metadata.artist,
                &query_metadata.album,
                &candidate.artist,
                &candidate.album,
            );
            let shuffle_key = sha256_text(&format!(
                "sanity-blind\0{}\0{}",
                query.source_sha256, candidate.source_sha256
            ));
            rows.push(Row {
                review_id: String::new(),
                query_id: query.id.clone(),
                query_track: format!(
                    "{} | {}",
                    query.id,
                    relative_path(&run.library_root, &query_metadata.path)
                ),
                candidate_id: String::new(),
                candidate_track: String::new(),
                query: query.clone(),
                query_metadata: query_metadata.clone(),
                candidate,
                original_rank: neighbor.rank,
                cosine: neighbor.score,
                relation,
                shuffle_key,
            });
        }
    }
    rows.sort_by(|left, right| {
        left.query_id
            .cmp(&right.query_id)
            .then_with(|| left.shuffle_key.cmp(&right.shuffle_key))
    });
    for (index, row) in rows.iter_mut().enumerate() {
        row.review_id = format!("R{:04}", index + 1);
        row.candidate_id = format!("C{:04}", index + 1);
        row.candidate_track = format!(
            "{} | {}",
            row.candidate_id,
            relative_path(&run.library_root, &row.candidate.path)
        );
    }

    fs::create_dir_all(output_dir)?;
    write_review_csv(&output_dir.join("sanity-review.csv"), &rows)?;
    write_mapping_csv(
        &output_dir.join("sanity-mapping.csv"),
        &rows,
        &loaded.metadata,
    )?;
    write_manifest(
        output_dir,
        &loaded,
        &rows,
        query_count,
        neighbor_count,
        &run.library_root,
    )?;
    write_instructions(output_dir, query_count, rows.len())?;
    Ok(rows.len())
}

fn load_run(run: &SanityRun) -> Result<LoadedRun> {
    let embeddings: EmbeddingsFile =
        serde_json::from_reader(File::open(run.directory.join("embeddings.json"))?)?;
    let neighbors: NeighborsFile =
        serde_json::from_reader(File::open(run.directory.join("neighbors.json"))?)?;
    let by_path = embeddings
        .tracks
        .iter()
        .enumerate()
        .map(|(index, track)| (track.path.clone(), index))
        .collect();
    Ok(LoadedRun {
        metadata: embeddings.metadata,
        tracks: embeddings.tracks,
        neighbors: neighbors.tracks,
        by_path,
    })
}

fn select_queries(run: &LoadedRun, query_count: usize) -> Result<Vec<QuerySelection>> {
    if query_count > run.tracks.len() {
        return Err("query count exceeds available tracks".into());
    }
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, track) in run.tracks.iter().enumerate() {
        groups
            .entry(format!("{}\0{}", track.artist, track.album))
            .or_default()
            .push(index);
    }
    let mut group_keys = groups.keys().cloned().collect::<Vec<_>>();
    group_keys.sort_by_key(|key| sha256_text(&format!("sanity-group\0{key}")));
    let mut selected = Vec::new();
    for key in group_keys.iter().take(query_count) {
        let index = groups[key]
            .iter()
            .copied()
            .min_by_key(|index| &run.tracks[*index].source_sha256)
            .ok_or("empty track group")?;
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
        .map(|(index, track_index)| QuerySelection {
            id: format!("Q{:03}", index + 1),
            index: track_index,
            source_sha256: run.tracks[track_index].source_sha256.clone(),
        })
        .collect())
}

fn relative_path(root: &Path, path: &str) -> String {
    Path::new(path)
        .strip_prefix(root)
        .unwrap_or_else(|_| Path::new(path))
        .display()
        .to_string()
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

fn write_review_csv(path: &Path, rows: &[Row]) -> Result<()> {
    let mut file = File::create(path)?;
    writeln!(
        file,
        "review_id,query_id,query_track,candidate_id,candidate_track,rating,note"
    )?;
    for row in rows {
        let fields = [
            row.review_id.clone(),
            row.query_id.clone(),
            field(&row.query_track),
            row.candidate_id.clone(),
            field(&row.candidate_track),
            String::new(),
            String::new(),
        ];
        writeln!(file, "{}", fields.join(","))?;
    }
    Ok(())
}

fn write_mapping_csv(path: &Path, rows: &[Row], metadata: &Metadata) -> Result<()> {
    let mut file = File::create(path)?;
    writeln!(
        file,
        "review_id,query_id,candidate_id,model_variant,model_sha256,patch_hop,original_rank,cosine,relation,query_source_sha256,candidate_source_sha256,query_artist,query_album,query_title,query_path,candidate_artist,candidate_album,candidate_title,candidate_path"
    )?;
    for row in rows {
        let mapping = MappingRow {
            review_id: row.review_id.clone(),
            query_id: row.query_id.clone(),
            candidate_id: row.candidate_id.clone(),
            model_variant: metadata.model_name.clone(),
            model_sha256: metadata.model_sha256.clone(),
            patch_hop: metadata.patch_hop,
            original_rank: row.original_rank,
            cosine: row.cosine,
            relation: row.relation.clone(),
            query_source_sha256: row.query.source_sha256.clone(),
            candidate_source_sha256: row.candidate.source_sha256.clone(),
            query_artist: row.query_metadata.artist.clone(),
            query_album: row.query_metadata.album.clone(),
            query_title: row.query_metadata.title.clone(),
            query_path: row.query_metadata.path.clone(),
            candidate_artist: row.candidate.artist.clone(),
            candidate_album: row.candidate.album.clone(),
            candidate_title: row.candidate.title.clone(),
            candidate_path: row.candidate.path.clone(),
        };
        let fields = [
            mapping.review_id,
            mapping.query_id,
            mapping.candidate_id,
            field(&mapping.model_variant),
            mapping.model_sha256,
            mapping.patch_hop.to_string(),
            mapping.original_rank.to_string(),
            format!("{:.6}", mapping.cosine),
            mapping.relation,
            mapping.query_source_sha256,
            mapping.candidate_source_sha256,
            field(&mapping.query_artist),
            field(&mapping.query_album),
            field(&mapping.query_title),
            field(&mapping.query_path),
            field(&mapping.candidate_artist),
            field(&mapping.candidate_album),
            field(&mapping.candidate_title),
            field(&mapping.candidate_path),
        ];
        writeln!(file, "{}", fields.join(","))?;
    }
    Ok(())
}

fn write_manifest(
    output_dir: &Path,
    run: &LoadedRun,
    rows: &[Row],
    query_count: usize,
    neighbor_count: usize,
    library_root: &Path,
) -> Result<()> {
    let query_artists = rows
        .iter()
        .map(|row| row.query_metadata.artist.clone())
        .collect::<BTreeSet<_>>();
    let query_albums = rows
        .iter()
        .map(|row| {
            format!(
                "{}\0{}",
                row.query_metadata.artist, row.query_metadata.album
            )
        })
        .collect::<BTreeSet<_>>();
    let candidate_artists = rows
        .iter()
        .map(|row| row.candidate.artist.clone())
        .collect::<BTreeSet<_>>();
    let different_artist_rows = rows
        .iter()
        .filter(|row| row.relation == "different_artist")
        .count();
    let manifest = Manifest {
        query_count,
        candidate_count: rows.len(),
        neighbor_count_per_query: neighbor_count,
        source_model: run.metadata.model_name.clone(),
        source_model_sha256: run.metadata.model_sha256.clone(),
        source_corpus_identity: run.metadata.corpus_identity.clone(),
        source_patch_hop: run.metadata.patch_hop,
        distinct_query_artists: query_artists.len(),
        distinct_query_albums: query_albums.len(),
        distinct_candidate_artists: candidate_artists.len(),
        different_artist_rows,
        selection_method: format!(
            "Select up to {query_count} album groups by SHA-256 of a NUL-separated directory-derived artist/album key; choose the lowest source SHA-256 track in each group; fill any remaining query slots by source SHA-256. Use the pre-declared canonical run's first {neighbor_count} neighbours, shuffle their presentation order by a source-hash key, and assign anonymous IDs. No score, rank, relation, model comparison, or rating is used for query or candidate selection. Library root: {}.",
            library_root.display()
        ),
    };
    let file = File::create(output_dir.join("sanity-manifest.json"))?;
    serde_json::to_writer_pretty(file, &manifest)?;
    Ok(())
}

fn write_instructions(output_dir: &Path, query_count: usize, candidate_count: usize) -> Result<()> {
    let mut file = File::create(output_dir.join("sanity-review.md"))?;
    writeln!(file, "# Music similarity sanity check\n")?;
    writeln!(
        file,
        "Queries: {query_count}; candidate recommendations: {candidate_count}\n"
    )?;
    writeln!(file, "## What to do\n")?;
    writeln!(file, "1. Play or listen to the query track.\n")?;
    writeln!(file, "2. Play or listen to the candidate track.\n")?;
    writeln!(
        file,
        "3. Decide whether the candidate would make sense as a similar-track recommendation.\n"
    )?;
    writeln!(
        file,
        "4. Record one rating: `clearly_similar`, `somewhat_related`, or `not_similar`.\n"
    )?;
    writeln!(file, "5. Optionally add a short note.\n")?;
    writeln!(
        file,
        "This is a qualitative sanity check, not a model comparison. Do not open the private mapping until the review is complete.\n"
    )?;
    Ok(())
}

fn field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}
