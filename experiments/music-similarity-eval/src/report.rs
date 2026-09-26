use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use serde::Serialize;

use crate::Result;
use crate::eval::{TrackEmbedding, aggregate_albums, album_nearest, nearest};

#[derive(Debug, Clone, Serialize)]
pub struct ReportMetadata {
    pub model_name: String,
    pub model_version: String,
    pub model_source: String,
    pub model_file: String,
    pub model_size_bytes: u64,
    pub model_sha256: String,
    pub model_input_name: String,
    pub model_input_shape: String,
    pub model_output_name: String,
    pub model_output_shape: String,
    pub model_license: String,
    pub corpus_root_sha256: String,
    pub corpus_identity: String,
    pub corpus_selection: String,
    pub discovered_files: usize,
    pub unsupported_audio_files: usize,
    pub runtime: String,
    pub runtime_license: String,
    pub rust_version: String,
    pub sample_rate: u32,
    pub decoder: String,
    pub downmix: String,
    pub resampler: String,
    pub window: String,
    pub mel_filterbank: String,
    pub log_scaling: String,
    pub frame_policy: String,
    pub batch_policy: String,
    pub embedding_dimensions: usize,
    pub frame_size: usize,
    pub frame_hop: usize,
    pub mel_bands: usize,
    pub patch_size: usize,
    pub patch_hop: usize,
    pub pooling: String,
    pub album_aggregation: String,
    pub normalization: String,
    pub similarity: String,
    pub elapsed_seconds: f64,
}

#[derive(Serialize)]
struct TrackOutput {
    artist: String,
    album: String,
    title: String,
    path: String,
    duration_seconds: f64,
    source_sha256: String,
    frame_count: usize,
    patch_count: usize,
    embedding_sha256: String,
    embedding: Vec<f32>,
}

#[derive(Serialize)]
struct AlbumOutput {
    artist: String,
    album: String,
    track_count: usize,
    embedding: Vec<f32>,
}

#[derive(Serialize)]
struct NeighborOutput {
    rank: usize,
    track_index: usize,
    artist: String,
    album: String,
    title: String,
    path: String,
    source_sha256: String,
    duration_seconds: f64,
    score: f32,
}

#[derive(Serialize)]
struct TrackReport {
    index: usize,
    track: TrackOutput,
    neighbors: Vec<NeighborOutput>,
}

#[derive(Serialize)]
struct AlbumReport {
    index: usize,
    album: AlbumOutput,
    neighbors: Vec<NeighborOutput>,
}

pub fn write_reports(
    output: &Path,
    metadata: &ReportMetadata,
    records: &[TrackEmbedding],
    top_k: usize,
) -> Result<()> {
    fs::create_dir_all(output)?;
    let albums = aggregate_albums(records);

    let track_outputs = records.iter().map(track_output).collect::<Vec<_>>();
    let album_outputs = albums.iter().map(album_output).collect::<Vec<_>>();
    write_json(
        &output.join("embeddings.json"),
        &serde_json::json!({
            "metadata": metadata,
            "tracks": track_outputs,
            "albums": album_outputs,
        }),
    )?;

    let track_reports = records
        .iter()
        .enumerate()
        .map(|(index, record)| {
            let neighbors = nearest(records, index, top_k)
                .into_iter()
                .enumerate()
                .map(|(rank, neighbor)| NeighborOutput {
                    rank: rank + 1,
                    track_index: neighbor.index,
                    artist: records[neighbor.index].spec.artist.clone(),
                    album: records[neighbor.index].spec.album.clone(),
                    title: records[neighbor.index].spec.title.clone(),
                    path: records[neighbor.index].spec.path.display().to_string(),
                    source_sha256: records[neighbor.index].source_sha256.clone(),
                    duration_seconds: records[neighbor.index].duration_seconds,
                    score: neighbor.score,
                })
                .collect();
            TrackReport {
                index,
                track: track_output(record),
                neighbors,
            }
        })
        .collect::<Vec<_>>();
    let album_reports = albums
        .iter()
        .enumerate()
        .map(|(index, album)| {
            let neighbors = album_nearest(&albums, index, top_k)
                .into_iter()
                .enumerate()
                .map(|(rank, neighbor)| NeighborOutput {
                    rank: rank + 1,
                    track_index: neighbor.index,
                    artist: albums[neighbor.index].artist.clone(),
                    album: albums[neighbor.index].album.clone(),
                    title: String::new(),
                    path: String::new(),
                    source_sha256: String::new(),
                    duration_seconds: 0.0,
                    score: neighbor.score,
                })
                .collect();
            AlbumReport {
                index,
                album: album_output(album),
                neighbors,
            }
        })
        .collect::<Vec<_>>();
    write_json(
        &output.join("neighbors.json"),
        &serde_json::json!({
            "tracks": track_reports,
            "albums": album_reports,
        }),
    )?;
    write_manual_csv(&output.join("manual-review.csv"), metadata, &track_reports)?;
    write_markdown(
        &output.join("report.md"),
        metadata,
        records,
        &albums,
        &track_reports,
        &album_reports,
    )?;
    Ok(())
}

fn track_output(record: &TrackEmbedding) -> TrackOutput {
    TrackOutput {
        artist: record.spec.artist.clone(),
        album: record.spec.album.clone(),
        title: record.spec.title.clone(),
        path: record.spec.path.display().to_string(),
        duration_seconds: record.duration_seconds,
        source_sha256: record.source_sha256.clone(),
        frame_count: record.frame_count,
        patch_count: record.patch_count,
        embedding_sha256: record.embedding_sha256.clone(),
        embedding: record.vector.clone(),
    }
}

fn album_output(album: &crate::eval::AlbumEmbedding) -> AlbumOutput {
    AlbumOutput {
        artist: album.artist.clone(),
        album: album.album.clone(),
        track_count: album.track_count,
        embedding: album.vector.clone(),
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let file = File::create(path)?;
    serde_json::to_writer_pretty(file, value)?;
    Ok(())
}

fn write_manual_csv(path: &Path, metadata: &ReportMetadata, reports: &[TrackReport]) -> Result<()> {
    let mut file = File::create(path)?;
    writeln!(
        file,
        "model_variant,patch_hop,query_id,query_source_sha256,query_artist,query_album,query_title,query_duration_seconds,rank,neighbor_id,neighbor_source_sha256,neighbor_artist,neighbor_album,neighbor_title,neighbor_duration_seconds,relation,cosine,rating,note"
    )?;
    for report in reports {
        let query_id = track_id(report.index);
        for neighbor in &report.neighbors {
            let neighbor_id = track_id(neighbor.track_index);
            let relation = relation(
                &report.track.artist,
                &report.track.album,
                &neighbor.artist,
                &neighbor.album,
            );
            let fields = vec![
                csv_field(&metadata.model_name),
                metadata.patch_hop.to_string(),
                query_id.clone(),
                csv_field(&report.track.source_sha256),
                csv_field(&report.track.artist),
                csv_field(&report.track.album),
                csv_field(&report.track.title),
                format!("{:.3}", report.track.duration_seconds),
                neighbor.rank.to_string(),
                neighbor_id,
                csv_field(&neighbor.source_sha256),
                csv_field(&neighbor.artist),
                csv_field(&neighbor.album),
                csv_field(&neighbor.title),
                format!("{:.3}", neighbor.duration_seconds),
                relation.to_string(),
                format!("{:.6}", neighbor.score),
                String::new(),
                String::new(),
            ];
            writeln!(file, "{}", fields.join(","))?;
        }
    }
    Ok(())
}

fn track_id(index: usize) -> String {
    format!("T{:03}", index + 1)
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

fn write_markdown(
    path: &Path,
    metadata: &ReportMetadata,
    records: &[TrackEmbedding],
    albums: &[crate::eval::AlbumEmbedding],
    track_reports: &[TrackReport],
    album_reports: &[AlbumReport],
) -> Result<()> {
    let mut file = File::create(path)?;
    let report_k = top_k_placeholder(track_reports);
    let same_album = group_hit_rate(records, report_k, |record| {
        format!("{}\u{0}{}", record.spec.artist, record.spec.album)
    });
    let same_album_at_1 = group_hit_rate(records, 1, |record| {
        format!("{}\u{0}{}", record.spec.artist, record.spec.album)
    });
    let same_album_at_5 = group_hit_rate(records, 5, |record| {
        format!("{}\u{0}{}", record.spec.artist, record.spec.album)
    });
    let same_artist = group_hit_rate(records, report_k, |record| record.spec.artist.clone());
    let same_artist_at_1 = group_hit_rate(records, 1, |record| record.spec.artist.clone());
    let same_artist_at_5 = group_hit_rate(records, 5, |record| record.spec.artist.clone());
    writeln!(file, "# Music Similarity Evaluation Report\n")?;
    writeln!(file, "## 1. Model\n")?;
    writeln!(file, "- Name: `{}`", metadata.model_name)?;
    writeln!(file, "- Version: {}", metadata.model_version)?;
    writeln!(file, "- Source: {}", metadata.model_source)?;
    writeln!(file, "- File: `{}`", metadata.model_file)?;
    writeln!(file, "- Size: {} bytes", metadata.model_size_bytes)?;
    writeln!(file, "- SHA-256: `{}`", metadata.model_sha256)?;
    writeln!(file, "- Input node: `{}`", metadata.model_input_name)?;
    writeln!(file, "- Input shape: `{}`", metadata.model_input_shape)?;
    writeln!(
        file,
        "- Embedding output node: `{}`",
        metadata.model_output_name
    )?;
    writeln!(file, "- Output shape: `{}`", metadata.model_output_shape)?;
    writeln!(file, "- License: {}\n", metadata.model_license)?;
    writeln!(file, "## 2. Runtime\n")?;
    writeln!(file, "- Runtime: `{}`", metadata.runtime)?;
    writeln!(file, "- Runtime license: {}", metadata.runtime_license)?;
    writeln!(file, "- Rust: {}", metadata.rust_version)?;
    writeln!(file, "- Elapsed: {:.3}s\n", metadata.elapsed_seconds)?;
    writeln!(file, "## 3. Preprocessing\n")?;
    writeln!(file, "- Decoder: {}", metadata.decoder)?;
    writeln!(file, "- Downmix: {}", metadata.downmix)?;
    writeln!(file, "- Resampler: {}", metadata.resampler)?;
    writeln!(
        file,
        "- {} Hz mono; frame {} / hop {}; {} Slaney mel bands; patch {} / hop {}",
        metadata.sample_rate,
        metadata.frame_size,
        metadata.frame_hop,
        metadata.mel_bands,
        metadata.patch_size,
        metadata.patch_hop
    )?;
    writeln!(file, "- Window: {}", metadata.window)?;
    writeln!(file, "- Mel filterbank: {}", metadata.mel_filterbank)?;
    writeln!(file, "- Log scaling: {}", metadata.log_scaling)?;
    writeln!(file, "- Frame policy: {}", metadata.frame_policy)?;
    writeln!(file, "- Batch policy: {}", metadata.batch_policy)?;
    writeln!(
        file,
        "- Window pooling: {}; album aggregation: {}; normalization: {}; similarity: {}\n",
        metadata.pooling, metadata.album_aggregation, metadata.normalization, metadata.similarity
    )?;
    writeln!(file, "## 4. Corpus\n")?;
    writeln!(
        file,
        "- Root identity SHA-256: `{}`",
        metadata.corpus_root_sha256
    )?;
    writeln!(
        file,
        "- Selected-content identity SHA-256: `{}`",
        metadata.corpus_identity
    )?;
    writeln!(file, "- Selection policy: {}", metadata.corpus_selection)?;
    writeln!(
        file,
        "- Supported files discovered: {}",
        metadata.discovered_files
    )?;
    writeln!(
        file,
        "- Unsupported audio files skipped: {}",
        metadata.unsupported_audio_files
    )?;
    writeln!(file, "- Tracks embedded: {}", records.len())?;
    writeln!(file, "- Albums aggregated: {}", albums.len())?;
    writeln!(
        file,
        "- Labels: directory-derived album/artist groups; no human labels supplied.\n"
    )?;
    writeln!(file, "## 5. Embedding characteristics\n")?;
    writeln!(
        file,
        "- {} dimensions, finite unit-normalized vectors; each external track record includes a SHA-256 over canonical little-endian f32 values.\n",
        metadata.embedding_dimensions
    )?;
    writeln!(file, "## 6. Track nearest-neighbour examples\n")?;
    for report in track_reports.iter().take(5) {
        writeln!(
            file,
            "### {} — {} — {}\n",
            report.track.artist, report.track.album, report.track.title
        )?;
        for neighbor in &report.neighbors {
            writeln!(
                file,
                "{}. {} — {} — {} ({:.5})",
                neighbor.rank, neighbor.artist, neighbor.album, neighbor.title, neighbor.score
            )?;
        }
        writeln!(file)?;
    }
    writeln!(file, "## 7. Album nearest-neighbour examples\n")?;
    for report in album_reports.iter().take(5) {
        writeln!(
            file,
            "### {} — {}\n",
            report.album.artist, report.album.album
        )?;
        for neighbor in &report.neighbors {
            writeln!(
                file,
                "{}. {} — {} ({:.5})",
                neighbor.rank, neighbor.artist, neighbor.album, neighbor.score
            )?;
        }
        writeln!(file)?;
    }
    writeln!(file, "## 8. Quantitative evaluation\n")?;
    writeln!(
        file,
        "- Directory same-album hit rate @1/@5/@{}: {:.4} / {:.4} / {:.4}",
        report_k, same_album_at_1, same_album_at_5, same_album
    )?;
    writeln!(
        file,
        "- Directory same-artist hit rate @1/@5/@{}: {:.4} / {:.4} / {:.4}",
        report_k, same_artist_at_1, same_artist_at_5, same_artist
    )?;
    writeln!(
        file,
        "- These are directory-derived proxy metrics, not human relevance labels.\n"
    )?;
    writeln!(
        file,
        "- Human precision/recall: not calculated; no reliable labels were supplied.\n"
    )?;
    writeln!(file, "## 9. Manual observations\n")?;
    writeln!(
        file,
        "- See `manual-review.csv`; label columns are intentionally blank.\n"
    )?;
    writeln!(file, "## 10. Known limitations\n")?;
    writeln!(
        file,
        "- The corpus is a small local subset, not a population-level benchmark."
    )?;
    writeln!(
        file,
        "- Directory grouping is a proxy label, not a human similarity judgement."
    )?;
    writeln!(
        file,
        "- The Rust mel/resampling implementation is an experiment port, not a production compatibility claim."
    )?;
    writeln!(
        file,
        "- RTen/threaded/SIMD numeric differences may exist across platforms."
    )?;
    writeln!(
        file,
        "- The model is non-commercial-licensed and is not a production dependency."
    )?;
    writeln!(
        file,
        "- The multi and release ONNX artifacts expose different embedding dimensions; vectors must never be compared across variants.\n"
    )?;
    writeln!(file, "## 11. Licensing findings\n")?;
    writeln!(
        file,
        "- Essentia: AGPL-oriented; Discogs-EffNet weights: CC BY-NC-SA 4.0."
    )?;
    writeln!(
        file,
        "- The model was supplied externally and is not redistributed by this repository."
    )?;
    writeln!(
        file,
        "- Confirm any use beyond this isolated non-commercial evaluation with the appropriate licensor.\n"
    )?;
    writeln!(file, "## 12. Recommendation for the next experiment step\n")?;
    writeln!(
        file,
        "- Manually inspect the CSV and add a small, explicit expected/acceptable/wrong label set."
    )?;
    writeln!(
        file,
        "- Review the measured `multi` and `release` outputs separately; they have different embedding dimensions and are not cross-comparable."
    )?;
    writeln!(
        file,
        "- Run a patch-hop 61/62 sensitivity check and a cross-codec check before choosing a product direction."
    )?;
    writeln!(
        file,
        "- Do not change `.mpack`, Server, Author, Web, or Player based on this run alone."
    )?;
    Ok(())
}

fn top_k_placeholder(reports: &[TrackReport]) -> usize {
    reports.first().map_or(10, |report| report.neighbors.len())
}

fn group_hit_rate<F>(records: &[TrackEmbedding], top_k: usize, group: F) -> f32
where
    F: Fn(&TrackEmbedding) -> String,
{
    let mut hits = 0usize;
    let mut total = 0usize;
    for (index, record) in records.iter().enumerate() {
        for neighbor in nearest(records, index, top_k) {
            total += 1;
            if group(record) == group(&records[neighbor.index]) {
                hits += 1;
            }
        }
    }
    if total == 0 {
        f32::NAN
    } else {
        hits as f32 / total as f32
    }
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}
