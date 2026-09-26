//! Deterministic, relationship-stratified qualitative sanity set.
//!
//! The objective is an integration sanity check, not a model bake-off: does the
//! MusicPack embedding path produce plausible similar-track recommendations on
//! this collection? Strata come from exact directory-derived artist/album
//! metadata, never from a judgement about musical quality.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::audio::{sha256_file, sha256_text};

pub const STRATUM_SAME_ALBUM: &str = "same_album";
pub const STRATUM_SAME_ARTIST: &str = "same_artist";
pub const STRATUM_DIFFERENT_ARTIST: &str = "different_artist";

const SELECTION_KEY_SALT: &str = "stratified-v1";
const ORDER_KEY_SALT: &str = "stratified-order";

#[derive(Debug, Clone)]
pub struct StratifiedRun {
    pub run_directory: PathBuf,
}

#[derive(Debug, Clone)]
pub struct StratifiedTargets {
    pub same_album_cases: usize,
    pub same_artist_cases: usize,
    pub different_artist_cases: usize,
    pub min_total_cases: usize,
    pub max_total_cases: usize,
}

impl Default for StratifiedTargets {
    fn default() -> Self {
        Self {
            same_album_cases: 5,
            same_artist_cases: 5,
            different_artist_cases: 12,
            min_total_cases: 20,
            max_total_cases: 24,
        }
    }
}

#[derive(Debug, Deserialize)]
struct EmbeddingsFile {
    metadata: Metadata,
    tracks: Vec<Track>,
}

#[derive(Debug, Clone, Deserialize)]
struct Metadata {
    model_name: String,
    model_sha256: String,
    patch_hop: usize,
    corpus_identity: String,
    corpus_root_sha256: String,
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
    score: f32,
}

#[derive(Debug)]
struct LoadedRun {
    directory: PathBuf,
    metadata: Metadata,
    tracks: Vec<Track>,
    neighbors: Vec<NeighborQuery>,
    by_path: BTreeMap<String, usize>,
    neighbors_per_query: usize,
}

/// One eligible (query, candidate) pair taken from the canonical run's
/// neighbour lists, with the metadata needed for stratification and ordering
/// resolved up front.
#[derive(Debug, Clone)]
struct Pair {
    query_index: usize,
    candidate_index: usize,
    rank: usize,
    score: f32,
    relation: String,
    query_sha256: String,
    candidate_sha256: String,
    query_artist: String,
    candidate_artist: String,
    selection_key: String,
}

impl Pair {
    fn order_key(&self) -> String {
        sha256_text(&format!(
            "{ORDER_KEY_SALT}\0{}\0{}",
            self.query_sha256, self.candidate_sha256
        ))
    }
}

#[derive(Debug, Clone)]
struct Case {
    review_id: String,
    stratum: String,
    reviewer_label: String,
    query_index: usize,
    candidate_index: usize,
    rank: usize,
    score: f32,
    relation: String,
    score_bucket: usize,
    score_bucket_population: usize,
    order_key: String,
}

/// Resolved outcome of stratum sizing, including the top-up decision.
#[derive(Debug, Clone, Copy)]
struct SelectionPlan {
    same_album_cases: usize,
    same_artist_cases: usize,
    discovery_buckets: usize,
    top_up_cases: usize,
    excluded_self_pairs: usize,
}

#[derive(Debug, Serialize)]
struct Manifest {
    case_count: usize,
    review_csv_columns: Vec<String>,
    totals: TotalsReport,
    strata: Vec<StratumReport>,
    source_run: SourceRunReport,
    source_files: Vec<SourceFileReport>,
    selection_algorithm: Vec<String>,
    confirmations: Confirmations,
    corpus_shape: CorpusShape,
}

#[derive(Debug, Serialize)]
struct TotalsReport {
    requested_cases: usize,
    selected_cases: usize,
    min_total_cases: usize,
    max_total_cases: usize,
    min_total_met: bool,
    top_up_applied: bool,
    top_up_cases: usize,
    top_up_stratum: String,
}

#[derive(Debug, Serialize)]
struct StratumReport {
    stratum: String,
    reviewer_label: String,
    eligible_pairs: usize,
    requested_cases: usize,
    selected_cases: usize,
    planned_score_buckets: usize,
    score_buckets: usize,
    bucket_population_min: Option<usize>,
    bucket_population_max: Option<usize>,
    eligible_score_min: Option<f64>,
    eligible_score_max: Option<f64>,
    selected_score_min: Option<f64>,
    selected_score_max: Option<f64>,
    selected_rank_min: Option<usize>,
    selected_rank_max: Option<usize>,
    distinct_query_tracks: usize,
    distinct_query_artists: usize,
    distinct_query_albums: usize,
    distinct_candidate_tracks: usize,
    distinct_candidate_artists: usize,
    top_up_applied: bool,
    note: Option<String>,
}

#[derive(Debug, Serialize)]
struct SourceRunReport {
    run_directory: String,
    model_name: String,
    model_sha256: String,
    patch_hop: usize,
    corpus_identity: String,
    corpus_root_sha256: String,
    tracks_in_run: usize,
    neighbours_per_query: usize,
    eligible_pairs_total: usize,
    excluded_self_pairs: usize,
}

#[derive(Debug, Serialize)]
struct SourceFileReport {
    file_name: String,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct Confirmations {
    selection_used_human_judgment: bool,
    selection_used_expected_musical_quality: bool,
    ratings_generated: bool,
    model_identity_hidden_from_reviewer: bool,
    patch_hop_hidden_from_reviewer: bool,
    score_hidden_from_reviewer: bool,
    original_rank_hidden_from_reviewer: bool,
    relation_hidden_from_reviewer: bool,
    source_sha256_hidden_from_reviewer: bool,
    stratum_column_omitted_from_review_csv: bool,
    strata_interleaved_in_presentation_order: bool,
    model_bake_off_performed: bool,
}

#[derive(Debug, Serialize)]
struct CorpusShape {
    tracks: usize,
    artist_album_groups: usize,
    distinct_artists: usize,
    artists_with_more_than_one_album: Vec<String>,
}

pub fn build_stratified_set(
    run: &StratifiedRun,
    targets: &StratifiedTargets,
    output_dir: &Path,
) -> Result<usize> {
    if targets.same_album_cases == 0
        || targets.different_artist_cases == 0
        || targets.min_total_cases == 0
        || targets.max_total_cases < targets.min_total_cases
    {
        return Err("invalid stratum targets".into());
    }
    let loaded = load_run(run)?;
    let (pools, excluded_self_pairs) = build_pools(&loaded)?;
    let plan = plan_selection(&pools, targets, excluded_self_pairs);

    let mut cases = Vec::new();
    cases.extend(select_stratum(
        &pools.same_album,
        STRATUM_SAME_ALBUM,
        "context",
        plan.same_album_cases,
    ));
    cases.extend(select_stratum(
        &pools.same_artist,
        STRATUM_SAME_ARTIST,
        "artist_consistency",
        plan.same_artist_cases,
    ));
    cases.extend(select_stratum(
        &pools.different_artist,
        STRATUM_DIFFERENT_ARTIST,
        "discovery",
        plan.discovery_buckets,
    ));
    if cases.is_empty() {
        return Err("no eligible review cases in the canonical run".into());
    }
    assert_no_repeated_pairs(&cases)?;

    // One interleaved presentation order: strata must not appear as blocks and
    // the original rank of a case must not be recoverable from row order.
    cases.sort_by(|left, right| {
        left.order_key.cmp(&right.order_key).then_with(|| {
            (left.query_index, left.candidate_index)
                .cmp(&(right.query_index, right.candidate_index))
        })
    });
    for (index, case) in cases.iter_mut().enumerate() {
        if case.query_index == case.candidate_index {
            return Err("a self pair reached the review set".into());
        }
        case.review_id = format!("R{:04}", index + 1);
    }
    let track_ids = assign_track_ids(&cases);

    fs::create_dir_all(output_dir)?;
    write_review_csv(output_dir, &cases, &track_ids)?;
    write_listening_list(output_dir, &loaded.tracks, &track_ids)?;
    write_mapping_csv(output_dir, &cases, &loaded, &track_ids)?;
    write_manifest(output_dir, &loaded, &cases, &pools, targets, plan)?;
    write_instructions(output_dir, cases.len())?;
    Ok(cases.len())
}

/// The identical (query, candidate) pair must never appear twice. Reusing a
/// query track across different cases is allowed and is tracked in the
/// manifest, but it should not happen inside a single stratum: the reviewer
/// would effectively be rating one recommendation more than once.
fn assert_no_repeated_pairs(cases: &[Case]) -> Result<()> {
    let mut pairs = BTreeSet::new();
    for case in cases {
        if !pairs.insert((case.query_index, case.candidate_index)) {
            return Err("the same query/candidate pair was selected twice".into());
        }
    }
    for stratum in [
        STRATUM_SAME_ALBUM,
        STRATUM_SAME_ARTIST,
        STRATUM_DIFFERENT_ARTIST,
    ] {
        let mut queries = BTreeSet::new();
        for case in cases.iter().filter(|case| case.stratum == stratum) {
            if !queries.insert(case.query_index) {
                return Err(format!(
                    "stratum {stratum} reuses one query track across several cases"
                )
                .into());
            }
        }
    }
    Ok(())
}

/// Honour the requested stratum sizes first; top up only the discovery stratum
/// (by adding buckets, so the spread stays even rather than appending a tail)
/// until the minimum total is reachable within the maximum.
fn plan_selection(
    pools: &Pools,
    targets: &StratifiedTargets,
    excluded_self_pairs: usize,
) -> SelectionPlan {
    let same_album_cases = targets.same_album_cases.min(pools.same_album.len());
    let same_artist_cases = targets.same_artist_cases.min(pools.same_artist.len());
    let baseline = same_album_cases + same_artist_cases + targets.different_artist_cases;
    let (discovery_buckets, top_up_cases) = if baseline < targets.min_total_cases {
        let shortfall = targets.min_total_cases - baseline;
        let head_room = targets
            .max_total_cases
            .saturating_sub(same_album_cases + same_artist_cases);
        (
            (targets.different_artist_cases + shortfall).min(head_room),
            shortfall,
        )
    } else {
        (targets.different_artist_cases, 0)
    };
    SelectionPlan {
        same_album_cases,
        same_artist_cases,
        discovery_buckets: discovery_buckets.min(pools.different_artist.len()),
        top_up_cases,
        excluded_self_pairs,
    }
}

fn load_run(run: &StratifiedRun) -> Result<LoadedRun> {
    let embeddings: EmbeddingsFile =
        serde_json::from_reader(File::open(run.run_directory.join("embeddings.json"))?)?;
    let neighbors: NeighborsFile =
        serde_json::from_reader(File::open(run.run_directory.join("neighbors.json"))?)?;
    let by_path = embeddings
        .tracks
        .iter()
        .enumerate()
        .map(|(index, track)| (track.path.clone(), index))
        .collect::<BTreeMap<_, _>>();
    let neighbors_per_query = neighbors
        .tracks
        .iter()
        .map(|entry| entry.neighbors.len())
        .max()
        .unwrap_or(0);
    Ok(LoadedRun {
        directory: run.run_directory.clone(),
        metadata: embeddings.metadata,
        tracks: embeddings.tracks,
        neighbors: neighbors.tracks,
        by_path,
        neighbors_per_query,
    })
}

#[derive(Debug, Default)]
struct Pools {
    same_album: Vec<Pair>,
    same_artist: Vec<Pair>,
    different_artist: Vec<Pair>,
}

/// Every (query, candidate) pair present in the run's neighbour lists becomes
/// eligible, classified only by exact directory-derived artist/album metadata.
fn build_pools(loaded: &LoadedRun) -> Result<(Pools, usize)> {
    let mut pools = Pools::default();
    let mut excluded_self_pairs = 0usize;
    for entry in &loaded.neighbors {
        let query = loaded
            .tracks
            .get(entry.index)
            .ok_or("query index out of range")?;
        for neighbor in &entry.neighbors {
            let candidate_index = *loaded
                .by_path
                .get(&neighbor.path)
                .ok_or("neighbour path is not part of the run corpus")?;
            if candidate_index == entry.index {
                excluded_self_pairs += 1;
                continue;
            }
            let candidate = &loaded.tracks[candidate_index];
            let relation = relation(
                &query.artist,
                &query.album,
                &candidate.artist,
                &candidate.album,
            );
            let pair = Pair {
                query_index: entry.index,
                candidate_index,
                rank: neighbor.rank,
                score: neighbor.score,
                relation: relation.clone(),
                query_sha256: query.source_sha256.clone(),
                candidate_sha256: candidate.source_sha256.clone(),
                query_artist: query.artist.clone(),
                candidate_artist: candidate.artist.clone(),
                selection_key: sha256_text(&format!(
                    "{SELECTION_KEY_SALT}\0{relation}\0{}\0{}",
                    query.source_sha256, candidate.source_sha256
                )),
            };
            match relation.as_str() {
                STRATUM_SAME_ALBUM => pools.same_album.push(pair),
                STRATUM_SAME_ARTIST => pools.same_artist.push(pair),
                _ => pools.different_artist.push(pair),
            }
        }
    }
    Ok((pools, excluded_self_pairs))
}

/// One case per contiguous score bucket, highest-score bucket first. The pick
/// inside a bucket is deliberately not the best-scoring one: it is the pair
/// whose query track, query artist, candidate track and candidate artist have
/// been used least so far, then the lowest precomputed selection key. Use
/// counts only spread identity across the sample; they are not quality signals.
fn select_stratum(
    pool: &[Pair],
    stratum: &str,
    reviewer_label: &str,
    bucket_count: usize,
) -> Vec<Case> {
    if pool.is_empty() || bucket_count == 0 {
        return Vec::new();
    }
    let mut ordered = pool.to_vec();
    ordered.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.query_sha256.cmp(&right.query_sha256))
            .then_with(|| left.candidate_sha256.cmp(&right.candidate_sha256))
    });
    let buckets = bucket_count.min(ordered.len());
    // Identity-use counters. Artist and track are counted separately so a
    // corpus where one artist holds several tracks still spreads out, and so a
    // single query track is not rated repeatedly within one stratum.
    let mut query_artist_uses: BTreeMap<String, usize> = BTreeMap::new();
    let mut candidate_artist_uses: BTreeMap<String, usize> = BTreeMap::new();
    let mut query_track_uses: BTreeMap<usize, usize> = BTreeMap::new();
    let mut candidate_track_uses: BTreeMap<usize, usize> = BTreeMap::new();
    let mut cases = Vec::with_capacity(buckets);
    for bucket in 0..buckets {
        let start = bucket * ordered.len() / buckets;
        let end = (bucket + 1) * ordered.len() / buckets;
        let mut slice = ordered[start..end].to_vec();
        // Owned key: the use-count maps are only read inside the comparator, so
        // the key cannot borrow from the pair being scored.
        let preference = |pair: &Pair| -> (usize, usize, usize, usize, String) {
            (
                query_track_uses
                    .get(&pair.query_index)
                    .copied()
                    .unwrap_or(0),
                query_artist_uses
                    .get(&pair.query_artist)
                    .copied()
                    .unwrap_or(0),
                candidate_track_uses
                    .get(&pair.candidate_index)
                    .copied()
                    .unwrap_or(0),
                candidate_artist_uses
                    .get(&pair.candidate_artist)
                    .copied()
                    .unwrap_or(0),
                pair.selection_key.clone(),
            )
        };
        slice.sort_by_key(preference);
        let Some(chosen) = slice.first() else {
            continue;
        };
        *query_artist_uses
            .entry(chosen.query_artist.clone())
            .or_default() += 1;
        *candidate_artist_uses
            .entry(chosen.candidate_artist.clone())
            .or_default() += 1;
        *query_track_uses.entry(chosen.query_index).or_default() += 1;
        *candidate_track_uses
            .entry(chosen.candidate_index)
            .or_default() += 1;
        cases.push(Case {
            review_id: String::new(),
            stratum: stratum.to_string(),
            reviewer_label: reviewer_label.to_string(),
            query_index: chosen.query_index,
            candidate_index: chosen.candidate_index,
            rank: chosen.rank,
            score: chosen.score,
            relation: chosen.relation.clone(),
            score_bucket: bucket,
            score_bucket_population: slice.len(),
            order_key: chosen.order_key(),
        });
    }
    cases
}

fn relation(query_artist: &str, query_album: &str, artist: &str, album: &str) -> String {
    if query_artist == artist && query_album == album {
        STRATUM_SAME_ALBUM.to_string()
    } else if query_artist == artist {
        STRATUM_SAME_ARTIST.to_string()
    } else {
        STRATUM_DIFFERENT_ARTIST.to_string()
    }
}

fn artists_with_multiple_albums(tracks: &[Track]) -> Vec<String> {
    let mut albums: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for track in tracks {
        albums
            .entry(track.artist.as_str())
            .or_default()
            .insert(track.album.as_str());
    }
    albums
        .into_iter()
        .filter(|(_, values)| values.len() > 1)
        .map(|(artist, _)| artist.to_string())
        .collect()
}

/// A single track-ID space for both roles, assigned in order of first
/// appearance, so no file can appear under two different identifiers (which
/// would itself leak a relationship).
fn assign_track_ids(cases: &[Case]) -> BTreeMap<usize, String> {
    let mut order: Vec<usize> = Vec::new();
    let mut seen: BTreeSet<usize> = BTreeSet::new();
    for case in cases {
        for index in [case.query_index, case.candidate_index] {
            if seen.insert(index) {
                order.push(index);
            }
        }
    }
    order
        .iter()
        .enumerate()
        .map(|(position, index)| (*index, format!("T{:03}", position + 1)))
        .collect()
}

fn write_review_csv(
    output_dir: &Path,
    cases: &[Case],
    track_ids: &BTreeMap<usize, String>,
) -> Result<()> {
    let mut file = File::create(output_dir.join("stratified-sanity-review.csv"))?;
    writeln!(
        file,
        "review_id,query_id,query_track,candidate_id,candidate_track,rating,note"
    )?;
    for case in cases {
        let query_id = track_ids
            .get(&case.query_index)
            .ok_or("query track id is missing")?;
        let candidate_id = track_ids
            .get(&case.candidate_index)
            .ok_or("candidate track id is missing")?;
        // The track columns repeat the identifier the reviewer resolves through
        // the listening list, so no file is ever published under two IDs.
        writeln!(
            file,
            "{},{query_id},{query_id},{candidate_id},{candidate_id},,",
            csv_field(&case.review_id)
        )?;
    }
    Ok(())
}

/// The reviewer has to play real files, so the listening list must resolve an
/// identifier to something playable. It deliberately does *not* resolve to the
/// library path: a path spells out artist and album, which reveals the very
/// relationship the strata are built from. Instead each track is exposed as an
/// opaque symlink inside `stratified-sanity-play/`, so the reviewer sees only
/// `T001.flac`-style names. The real paths stay in the private mapping.
fn write_listening_list(
    output_dir: &Path,
    tracks: &[Track],
    track_ids: &BTreeMap<usize, String>,
) -> Result<()> {
    let play_dir = output_dir.join("stratified-sanity-play");
    if play_dir.is_dir() {
        fs::remove_dir_all(&play_dir)?;
    }
    fs::create_dir_all(&play_dir)?;
    let mut file = File::create(output_dir.join("stratified-sanity-listening.csv"))?;
    writeln!(file, "track_id,audio_file")?;
    // Emitted in track-ID order so the reviewer reads T001..Tnnn in sequence.
    let mut ordered_ids = track_ids.iter().collect::<Vec<_>>();
    ordered_ids.sort_by_key(|(_, id)| (*id).clone());
    for (index, id) in ordered_ids {
        let track = tracks.get(*index).ok_or("track index out of range")?;
        let extension = Path::new(&track.path)
            .extension()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .unwrap_or("audio");
        let file_name = format!("{id}.{extension}");
        let link = play_dir.join(&file_name);
        // A stale link from an earlier run must not block regeneration.
        match fs::remove_file(&link) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        std::os::unix::fs::symlink(&track.path, &link)?;
        writeln!(file, "{},{}", csv_field(id), csv_field(&file_name))?;
    }
    Ok(())
}

fn write_mapping_csv(
    output_dir: &Path,
    cases: &[Case],
    loaded: &LoadedRun,
    track_ids: &BTreeMap<usize, String>,
) -> Result<()> {
    let mut file = File::create(output_dir.join("stratified-sanity-mapping.csv"))?;
    writeln!(
        file,
        "review_id,stratum,reviewer_label,query_id,candidate_id,model_variant,model_sha256,patch_hop,original_rank,cosine,relation,score_bucket,score_bucket_population,query_source_sha256,candidate_source_sha256,query_artist,query_album,query_title,candidate_artist,candidate_album,candidate_title,query_relative_path,candidate_relative_path"
    )?;
    for case in cases {
        let query_id = track_ids
            .get(&case.query_index)
            .ok_or("query track id is missing")?;
        let candidate_id = track_ids
            .get(&case.candidate_index)
            .ok_or("candidate track id is missing")?;
        let query = loaded
            .tracks
            .get(case.query_index)
            .ok_or("query track index out of range")?;
        let candidate = loaded
            .tracks
            .get(case.candidate_index)
            .ok_or("candidate track index out of range")?;
        let fields = [
            case.review_id.clone(),
            case.stratum.clone(),
            case.reviewer_label.clone(),
            query_id.clone(),
            candidate_id.clone(),
            loaded.metadata.model_name.clone(),
            loaded.metadata.model_sha256.clone(),
            loaded.metadata.patch_hop.to_string(),
            case.rank.to_string(),
            format!("{:.6}", case.score),
            case.relation.clone(),
            case.score_bucket.to_string(),
            case.score_bucket_population.to_string(),
            query.source_sha256.clone(),
            candidate.source_sha256.clone(),
            query.artist.clone(),
            query.album.clone(),
            query.title.clone(),
            candidate.artist.clone(),
            candidate.album.clone(),
            candidate.title.clone(),
            query.path.clone(),
            candidate.path.clone(),
        ];
        let escaped = fields
            .iter()
            .map(|value| csv_field(value))
            .collect::<Vec<_>>();
        writeln!(file, "{}", escaped.join(","))?;
    }
    Ok(())
}

fn write_manifest(
    output_dir: &Path,
    loaded: &LoadedRun,
    cases: &[Case],
    pools: &Pools,
    targets: &StratifiedTargets,
    plan: SelectionPlan,
) -> Result<()> {
    let artists_multi_album = artists_with_multiple_albums(&loaded.tracks);
    let same_artist_note = (pools.same_artist.is_empty()).then(|| {
        format!(
            "No eligible pair in this run shares an artist across different albums: the run's {} tracks cover {} artist/album groups with {} artist(s) holding more than one album. This stratum cannot be sampled from the canonical run; no case was invented or substituted.",
            loaded.tracks.len(),
            loaded
                .tracks
                .iter()
                .map(|track| (track.artist.as_str(), track.album.as_str()))
                .collect::<BTreeSet<_>>()
                .len(),
            artists_multi_album.len()
        )
    });

    let strata = vec![
        stratum_report(
            STRATUM_SAME_ALBUM,
            "context",
            &pools.same_album,
            cases,
            loaded,
            targets.same_album_cases,
            plan.same_album_cases,
            false,
            None,
        ),
        stratum_report(
            STRATUM_SAME_ARTIST,
            "artist_consistency",
            &pools.same_artist,
            cases,
            loaded,
            targets.same_artist_cases,
            plan.same_artist_cases,
            false,
            same_artist_note,
        ),
        stratum_report(
            STRATUM_DIFFERENT_ARTIST,
            "discovery",
            &pools.different_artist,
            cases,
            loaded,
            targets.different_artist_cases,
            plan.discovery_buckets,
            plan.top_up_cases > 0,
            None,
        ),
    ];

    let manifest = Manifest {
        case_count: cases.len(),
        review_csv_columns: [
            "review_id",
            "query_id",
            "query_track",
            "candidate_id",
            "candidate_track",
            "rating",
            "note",
        ]
        .iter()
        .map(|value| (*value).to_string())
        .collect(),
        totals: TotalsReport {
            requested_cases: targets.same_album_cases
                + targets.same_artist_cases
                + targets.different_artist_cases,
            selected_cases: cases.len(),
            min_total_cases: targets.min_total_cases,
            max_total_cases: targets.max_total_cases,
            min_total_met: cases.len() >= targets.min_total_cases,
            top_up_applied: plan.top_up_cases > 0,
            top_up_cases: plan.top_up_cases,
            top_up_stratum: if plan.top_up_cases > 0 {
                STRATUM_DIFFERENT_ARTIST.to_string()
            } else {
                String::new()
            },
        },
        strata,
        source_run: SourceRunReport {
            run_directory: loaded.directory.display().to_string(),
            model_name: loaded.metadata.model_name.clone(),
            model_sha256: loaded.metadata.model_sha256.clone(),
            patch_hop: loaded.metadata.patch_hop,
            corpus_identity: loaded.metadata.corpus_identity.clone(),
            corpus_root_sha256: loaded.metadata.corpus_root_sha256.clone(),
            tracks_in_run: loaded.tracks.len(),
            neighbours_per_query: loaded.neighbors_per_query,
            eligible_pairs_total: pools.same_album.len()
                + pools.same_artist.len()
                + pools.different_artist.len(),
            excluded_self_pairs: plan.excluded_self_pairs,
        },
        source_files: source_file_reports(loaded)?,
        selection_algorithm: selection_algorithm_steps(),
        confirmations: Confirmations {
            selection_used_human_judgment: false,
            selection_used_expected_musical_quality: false,
            ratings_generated: false,
            model_identity_hidden_from_reviewer: true,
            patch_hop_hidden_from_reviewer: true,
            score_hidden_from_reviewer: true,
            original_rank_hidden_from_reviewer: true,
            relation_hidden_from_reviewer: true,
            source_sha256_hidden_from_reviewer: true,
            stratum_column_omitted_from_review_csv: true,
            strata_interleaved_in_presentation_order: true,
            model_bake_off_performed: false,
        },
        corpus_shape: CorpusShape {
            tracks: loaded.tracks.len(),
            artist_album_groups: loaded
                .tracks
                .iter()
                .map(|track| (track.artist.as_str(), track.album.as_str()))
                .collect::<BTreeSet<_>>()
                .len(),
            distinct_artists: loaded
                .tracks
                .iter()
                .map(|track| track.artist.as_str())
                .collect::<BTreeSet<_>>()
                .len(),
            artists_with_more_than_one_album: artists_multi_album,
        },
    };
    let file = File::create(output_dir.join("stratified-sanity-manifest.json"))?;
    serde_json::to_writer_pretty(file, &manifest)?;
    Ok(())
}

fn source_file_reports(loaded: &LoadedRun) -> Result<Vec<SourceFileReport>> {
    let mut reports = Vec::new();
    for file_name in ["embeddings.json", "neighbors.json"] {
        let path = loaded.directory.join(file_name);
        reports.push(SourceFileReport {
            file_name: path.display().to_string(),
            sha256: sha256_file(&path)?,
        });
    }
    Ok(reports)
}

fn selection_algorithm_steps() -> Vec<String> {
    [
        "1. Read the canonical run's embeddings.json and neighbors.json. No model is loaded and no new inference is run.",
        "2. Build the eligible pool from every (query, candidate) pair present in the run's neighbour lists, dropping self-pairs. Classify each pair with exact directory-derived metadata only: same_album = same artist AND same album; same_artist = same artist with a different album; different_artist = different artist.",
        "3. Order each stratum's pool by cosine descending; ties broken by query source SHA-256 ascending, then candidate source SHA-256 ascending.",
        "4. Split the ordered pool into N contiguous, near-equal score buckets, where N is the number of cases wanted from that stratum; the highest-score bucket comes first.",
        "5. Take exactly one case from each bucket. Inside a bucket the pick is the pair with the fewest query-track uses so far, then the fewest query-artist uses, then the fewest candidate-track uses, then the fewest candidate-artist uses, and finally the lowest SHA-256 of 'stratified-v1\\0<stratum>\\0<query source SHA-256>\\0<candidate source SHA-256>'. The highest-scoring pair in a bucket is therefore never preferred, and identity use counts only spread the sample across the collection.",
        "6. Because every bucket yields exactly one case, the sample spans the whole available cosine range and the whole available rank range, including low-ranked results that are plausible false positives.",
        "7. If the strata fall below the minimum total, only the different_artist bucket count is raised by the shortfall, which re-partitions that pool into more, still even, buckets. The applied top-up is recorded per stratum.",
        "8. Present every case in one block ordered by SHA-256 of 'stratified-order\\0<query source SHA-256>\\0<candidate source SHA-256>', then assign R#### review ids and T### track ids. Strata are interleaved and original rank is not recoverable from row order.",
        "9. No rating, human judgement or expectation about musical quality takes part in any step. Cosine and rank are used only to spread the sample, never to prefer a case.",
    ]
    .iter()
    .map(|value| (*value).to_string())
    .collect()
}

#[allow(clippy::too_many_arguments)]
fn stratum_report(
    stratum: &str,
    reviewer_label: &str,
    pool: &[Pair],
    cases: &[Case],
    loaded: &LoadedRun,
    requested: usize,
    planned: usize,
    top_up_applied: bool,
    note: Option<String>,
) -> StratumReport {
    let selected = cases
        .iter()
        .filter(|case| case.stratum == stratum)
        .collect::<Vec<_>>();
    let track = |index: usize| -> &Track {
        loaded
            .tracks
            .get(index)
            .expect("case track index is inside the run corpus")
    };
    StratumReport {
        stratum: stratum.to_string(),
        reviewer_label: reviewer_label.to_string(),
        eligible_pairs: pool.len(),
        requested_cases: requested,
        selected_cases: selected.len(),
        planned_score_buckets: planned,
        score_buckets: selected.len(),
        bucket_population_min: selected
            .iter()
            .map(|case| case.score_bucket_population)
            .min(),
        bucket_population_max: selected
            .iter()
            .map(|case| case.score_bucket_population)
            .max(),
        eligible_score_min: score_extremes(pool.iter().map(|pair| pair.score)).0,
        eligible_score_max: score_extremes(pool.iter().map(|pair| pair.score)).1,
        selected_score_min: score_extremes(selected.iter().map(|case| case.score)).0,
        selected_score_max: score_extremes(selected.iter().map(|case| case.score)).1,
        selected_rank_min: selected.iter().map(|case| case.rank).min(),
        selected_rank_max: selected.iter().map(|case| case.rank).max(),
        distinct_query_tracks: selected
            .iter()
            .map(|case| case.query_index)
            .collect::<BTreeSet<_>>()
            .len(),
        distinct_query_artists: selected
            .iter()
            .map(|case| track(case.query_index).artist.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        distinct_query_albums: selected
            .iter()
            .map(|case| {
                (
                    track(case.query_index).artist.as_str(),
                    track(case.query_index).album.as_str(),
                )
            })
            .collect::<BTreeSet<_>>()
            .len(),
        distinct_candidate_tracks: selected
            .iter()
            .map(|case| case.candidate_index)
            .collect::<BTreeSet<_>>()
            .len(),
        distinct_candidate_artists: selected
            .iter()
            .map(|case| track(case.candidate_index).artist.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        top_up_applied,
        note,
    }
}

fn score_extremes(values: impl Iterator<Item = f32>) -> (Option<f64>, Option<f64>) {
    let mut minimum = f64::INFINITY;
    let mut maximum = f64::NEG_INFINITY;
    let mut any = false;
    for value in values {
        let value = f64::from(value);
        minimum = minimum.min(value);
        maximum = maximum.max(value);
        any = true;
    }
    if any {
        (Some(minimum), Some(maximum))
    } else {
        (None, None)
    }
}

fn write_instructions(output_dir: &Path, case_count: usize) -> Result<()> {
    let mut file = File::create(output_dir.join("stratified-sanity-review.md"))?;
    writeln!(file, "# Similar-track sanity check\n")?;
    writeln!(file, "Review cases: {case_count}\n")?;
    writeln!(file, "## What to do\n")?;
    writeln!(
        file,
        "1. Find `query_id` in `stratified-sanity-listening.csv`; it gives a file name such as `T001.flac` in `stratified-sanity-play/`. Play it.\n"
    )?;
    writeln!(
        file,
        "2. Find `candidate_id` in the same file and play that file the same way.\n"
    )?;
    writeln!(
        file,
        "3. Ask one question: would this candidate be a reasonable similar-track recommendation for the query?\n"
    )?;
    writeln!(
        file,
        "4. Record one rating in `stratified-sanity-review.csv`: `clearly_similar`, `somewhat_related` or `not_similar`.\n"
    )?;
    writeln!(file, "5. Optionally add a short note.\n")?;
    writeln!(file, "## Notes\n")?;
    writeln!(
        file,
        "`stratified-sanity-play/` holds symlinks named only by track ID, so playing a file does not reveal its artist, album, score or rank.\n"
    )?;
    writeln!(
        file,
        "Do not open `stratified-sanity-mapping.csv` or `stratified-sanity-manifest.json` before the review is finished: they hold the model, hop, score, rank and track relationships.\n"
    )?;
    writeln!(
        file,
        "Judge every case on its own. A recommendation only has to be musically reasonable, not perfect.\n"
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
