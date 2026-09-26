//! Synthetic fixture mechanism for the experiment's selection/report tests.
//!
//! This module is compiled only under `cfg(test)`. It fabricates a minimal run
//! directory — `embeddings.json` plus `neighbors.json` — containing nothing but
//! invented metadata, opaque object paths, and synthetic vectors/scores. There
//! is no model weight, no audio, no network access, and no real collection data
//! anywhere in it.
//!
//! The corpus shape is chosen so the three relationship strata are all populated.
//! Real runs of this experiment used a collection with exactly one album per
//! artist, which made the `same_artist` stratum structurally empty; the fixture
//! deliberately gives one artist two albums so that code path is exercised.
//!
//! Object paths are deliberately opaque (`/synthetic/library/obj/NNNN.flac`)
//! rather than metadata-shaped, so that a leakage assertion against a generated
//! human-facing file tests the real contract instead of accidentally testing the
//! fixture.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;

/// Artists in the synthetic corpus.
pub const ARTISTS: usize = 6;
/// Tracks on each artist's main album.
pub const MAIN_TRACKS: usize = 3;
/// Tracks on each artist's second album (creates `same_artist` pairs).
pub const EXTRA_TRACKS: usize = 2;
/// Neighbours recorded per query.
pub const NEIGHBOURS: usize = 8;
/// Patch hop written into the run metadata.
pub const PATCH_HOP: usize = 61;
/// Fabricated model name; no real model is referenced.
pub const MODEL_NAME: &str = "Synthetic EffNet-like model";
/// Fabricated model digest (64 hex characters, not a real artefact).
pub const MODEL_SHA256: &str = "0000000000000000000000000000000000000000000000000000000000000000";
/// Opaque object root; never contains artist or album text.
pub const LIBRARY_ROOT: &str = "/synthetic/library";

/// A fabricated track record.
#[derive(Debug, Clone)]
pub struct FixtureTrack {
    pub artist: String,
    pub album: String,
    pub title: String,
    pub path: String,
    pub source_sha256: String,
}

/// One recorded neighbour, in the order written to `neighbors.json` (already
/// sorted by descending score, with `rank` equal to the 1-based position).
#[derive(Debug, Clone)]
pub struct NeighbourRecord {
    pub rank: usize,
    pub candidate: usize,
    pub score: f32,
}

/// The complete synthetic run, in memory, plus the directory it was written to.
#[derive(Debug, Clone)]
pub struct Fixture {
    pub tracks: Vec<FixtureTrack>,
    /// Indexed by query track index.
    pub neighbours: Vec<Vec<NeighbourRecord>>,
    pub directory: PathBuf,
}

impl Fixture {
    /// Recompute the eligible pair count per relation directly from the
    /// fabricated data. Deliberately written as a plain, independent loop so it
    /// does not share code with the selection logic it is used to check.
    pub fn recount_eligible(&self) -> BTreeMap<&'static str, usize> {
        let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
        for (query, records) in self.neighbours.iter().enumerate() {
            for record in records {
                let relation = relation_of(&self.tracks[query], &self.tracks[record.candidate]);
                *counts.entry(relation).or_default() += 1;
            }
        }
        counts
    }

    /// The rank the fixture actually recorded for `query -> candidate`, if the
    /// pair is present at all.
    pub fn recorded_rank(&self, query: usize, candidate: usize) -> Option<usize> {
        self.neighbours[query]
            .iter()
            .find(|record| record.candidate == candidate)
            .map(|record| record.rank)
    }

    pub fn source_index(&self) -> BTreeMap<&str, usize> {
        self.tracks
            .iter()
            .enumerate()
            .map(|(index, track)| (track.source_sha256.as_str(), index))
            .collect()
    }
}

/// The same relation rule the production code applies, restated independently.
pub fn relation_of(query: &FixtureTrack, candidate: &FixtureTrack) -> &'static str {
    if query.artist == candidate.artist && query.album == candidate.album {
        "same_album"
    } else if query.artist == candidate.artist {
        "same_artist"
    } else {
        "different_artist"
    }
}

/// A self-cleaning temporary directory. No `tempfile` dependency is added; the
/// name is made unique with a process id and a process-wide counter so the
/// parallel test runner cannot collide.
pub struct TempDir {
    pub path: PathBuf,
}

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "music-similarity-eval-test-{tag}-{}-{unique}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("fixture temp directory");
        TempDir { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Build the synthetic corpus and write a run directory into `directory`.
pub fn materialise(directory: &Path) -> Fixture {
    let tracks = build_tracks();
    let neighbours = build_neighbours(&tracks);
    write_run(directory, &tracks, &neighbours);
    Fixture {
        tracks,
        neighbours,
        directory: directory.to_path_buf(),
    }
}

fn build_tracks() -> Vec<FixtureTrack> {
    let mut tracks = Vec::new();
    for artist_index in 0..ARTISTS {
        for (album_label, count) in [("Main", MAIN_TRACKS), ("Extra", EXTRA_TRACKS)] {
            for track_in_album in 0..count {
                let global = tracks.len();
                tracks.push(FixtureTrack {
                    artist: format!("Synthetic Artist {artist_index:02}"),
                    album: format!("Synthetic Album {artist_index:02} {album_label}"),
                    title: format!("Track {track_in_album:02}"),
                    // Opaque: no artist or album text in the path.
                    path: format!("{LIBRARY_ROOT}/obj/{global:04}.flac"),
                    source_sha256: crate::audio::sha256_text(&format!(
                        "synthetic-source-{global:04}"
                    )),
                });
            }
        }
    }
    tracks
}

fn build_neighbours(tracks: &[FixtureTrack]) -> Vec<Vec<NeighbourRecord>> {
    let mut all: Vec<Vec<NeighbourRecord>> = Vec::with_capacity(tracks.len());
    for (query_index, query) in tracks.iter().enumerate() {
        let mut same_album = Vec::new();
        let mut same_artist = Vec::new();
        let mut other = Vec::new();
        for (candidate_index, candidate) in tracks.iter().enumerate() {
            if candidate_index == query_index {
                continue;
            }
            match relation_of(query, candidate) {
                "same_album" => same_album.push(candidate_index),
                "same_artist" => same_artist.push(candidate_index),
                _ => other.push(candidate_index),
            }
        }

        let mut scored: Vec<(usize, f32)> = Vec::new();
        // Contextual neighbours score highest, then same-artist, then a spread
        // of cross-artist candidates that deliberately covers a wide range.
        for candidate in &same_album {
            scored.push((*candidate, 0.93));
        }
        for candidate in &same_artist {
            scored.push((*candidate, 0.78));
        }
        for (slot, candidate) in other.iter().take(NEIGHBOURS).enumerate() {
            let spread = 0.70 - 0.09 * slot as f32 - 0.001 * (query_index % 7) as f32;
            scored.push((*candidate, spread));
        }
        scored.truncate(NEIGHBOURS);
        scored.sort_by(|left, right| right.1.total_cmp(&left.1));
        all.push(
            scored
                .into_iter()
                .enumerate()
                .map(|(position, (candidate, score))| NeighbourRecord {
                    rank: position + 1,
                    candidate,
                    score,
                })
                .collect(),
        );
    }
    all
}

fn write_run(directory: &Path, tracks: &[FixtureTrack], neighbours: &[Vec<NeighbourRecord>]) {
    fs::create_dir_all(directory).expect("run directory");
    let metadata = json!({
        "model_name": MODEL_NAME,
        "model_sha256": MODEL_SHA256,
        "patch_hop": PATCH_HOP,
        "corpus_identity": crate::audio::sha256_text("synthetic-corpus"),
        "corpus_root_sha256": crate::audio::sha256_text("synthetic-root"),
        "embedding_dimensions": 8,
        "similarity": "cosine",
    });
    let track_values: Vec<_> = tracks
        .iter()
        .map(|track| {
            json!({
                "artist": track.artist,
                "album": track.album,
                "title": track.title,
                "path": track.path,
                "source_sha256": track.source_sha256,
            })
        })
        .collect();
    let embeddings = json!({ "metadata": metadata, "tracks": track_values });
    fs::write(
        directory.join("embeddings.json"),
        serde_json::to_vec_pretty(&embeddings).expect("embeddings json"),
    )
    .expect("write embeddings.json");

    let query_values: Vec<_> = neighbours
        .iter()
        .enumerate()
        .map(|(query_index, records)| {
            let entries: Vec<_> = records
                .iter()
                .map(|record| {
                    json!({
                        "rank": record.rank,
                        "path": tracks[record.candidate].path,
                        "score": record.score,
                    })
                })
                .collect();
            json!({ "index": query_index, "neighbors": entries })
        })
        .collect();
    let neighbours_file = json!({ "tracks": query_values });
    fs::write(
        directory.join("neighbors.json"),
        serde_json::to_vec_pretty(&neighbours_file).expect("neighbors json"),
    )
    .expect("write neighbors.json");
}

/// Read a generated file as text.
pub fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| {
        panic!("could not read {}: {error}", path.display());
    })
}

/// Split one CSV line, honouring double-quoted fields.
pub fn split_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '"' if quoted && chars.peek() == Some(&'"') => {
                current.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut current)),
            other => current.push(other),
        }
    }
    fields.push(current);
    fields
}

/// Parse a CSV file into a header plus rows, using [`split_csv_line`].
pub fn parse_csv(path: &Path) -> (Vec<String>, Vec<Vec<String>>) {
    let text = read(path);
    let mut lines = text.lines().filter(|line| !line.is_empty());
    let header = split_csv_line(lines.next().unwrap_or_default());
    let rows = lines.map(split_csv_line).collect();
    (header, rows)
}

/// Index of a named column, panicking with a clear message if absent.
pub fn column(header: &[String], name: &str) -> usize {
    header
        .iter()
        .position(|field| field == name)
        .unwrap_or_else(|| panic!("missing column {name} in {header:?}"))
}
