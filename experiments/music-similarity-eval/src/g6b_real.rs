//! Real-corpus ingestion for the G-6B run: the recorded Discogs-EffNet
//! `embeddings.json` files written by the original spike (`src/report.rs`).
//!
//! **Pure post-processing input.** Loading a corpus verifies its identity
//! (`G6B_METHODOLOGY.md` §13: "the per-track `embedding_sha256` values must be
//! re-verified against the vectors before measurement, so the corpus identity
//! is established, not assumed") and nothing else. No model, no audio, no
//! inference, no network.
//!
//! The recorded `albums[]` block is deliberately **not** deserialized: album
//! aggregates must be recomputed independently in each numerical world from
//! that world's own stored vectors (`G6B_METHODOLOGY.md` §18 R-3), and the
//! recorded block is the recording pipeline's f32 aggregate. Reading it would
//! recreate the mixed-regime defect G-6B exists to fix, one level up.
//!
//! Provenance lines carry model identity and digests only — never track
//! metadata, never paths — so they can be embedded in the verdict artefact.

use std::path::Path;

use serde::Deserialize;

use crate::corpus::TrackSpec;
use crate::docfmt;
use crate::eval::TrackEmbedding;
use crate::g6::f32_le_bytes;

#[derive(Debug, Deserialize)]
struct RawCorpus {
    metadata: RawMetadata,
    tracks: Vec<RawTrack>,
}

/// The metadata fields the run's provenance record requires
/// (`G6B_METHODOLOGY.md` §13). Unknown fields are ignored; missing ones are
/// a load error, so a schema drift fails loudly instead of silently.
#[derive(Debug, Deserialize)]
pub struct RawMetadata {
    pub model_name: String,
    pub model_sha256: String,
    pub patch_hop: usize,
    pub embedding_dimensions: usize,
    pub corpus_identity: String,
    pub corpus_root_sha256: String,
    pub runtime: String,
}

#[derive(Debug, Deserialize)]
struct RawTrack {
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

/// One verified recorded corpus: the metadata identity plus the track records
/// ready for the G-6B instrument.
#[derive(Debug)]
pub struct RecordedCorpus {
    pub metadata: RawMetadata,
    pub records: Vec<TrackEmbedding>,
}

impl RecordedCorpus {
    /// A provenance line safe for the verdict artefact: model identity and
    /// digests only — no track metadata, no filesystem path.
    pub fn provenance_line(&self) -> String {
        format!(
            "model={} model_sha256={} patch_hop={} dimensions={} corpus_identity={} corpus_root_sha256={} runtime={}",
            self.metadata.model_name,
            self.metadata.model_sha256,
            self.metadata.patch_hop,
            self.metadata.embedding_dimensions,
            self.metadata.corpus_identity,
            self.metadata.corpus_root_sha256,
            self.metadata.runtime,
        )
    }
}

/// Parse and identity-verify one recorded corpus from its JSON text.
pub fn parse_corpus(json: &str) -> Result<RecordedCorpus, String> {
    let raw: RawCorpus = serde_json::from_str(json)
        .map_err(|error| format!("cannot parse embeddings.json: {error}"))?;
    let declared_dimensions = raw.metadata.embedding_dimensions;
    let mut records = Vec::with_capacity(raw.tracks.len());
    for (index, track) in raw.tracks.iter().enumerate() {
        if track.embedding.len() != declared_dimensions {
            return Err(format!(
                "track {index} carries {} components; metadata declares {declared_dimensions}",
                track.embedding.len()
            ));
        }
        // §13: the corpus identity is established, not assumed — every
        // vector is re-digested and compared before any measurement runs.
        let recomputed = docfmt::sha256_hex(&f32_le_bytes(&track.embedding));
        if recomputed != track.embedding_sha256 {
            return Err(format!(
                "track {index} embedding_sha256 mismatch: recorded {}, recomputed {recomputed}; corpus identity is not established",
                track.embedding_sha256
            ));
        }
        records.push(TrackEmbedding {
            spec: TrackSpec {
                path: track.path.clone().into(),
                artist: track.artist.clone(),
                album: track.album.clone(),
                title: track.title.clone(),
            },
            duration_seconds: track.duration_seconds,
            source_sha256: track.source_sha256.clone(),
            frame_count: track.frame_count,
            patch_count: track.patch_count,
            embedding_sha256: track.embedding_sha256.clone(),
            vector: track.embedding.clone(),
        });
    }
    if records.is_empty() {
        return Err("the corpus carries no tracks".to_string());
    }
    Ok(RecordedCorpus {
        metadata: raw.metadata,
        records,
    })
}

/// Load one recorded corpus from disk.
pub fn load_corpus(path: &Path) -> Result<RecordedCorpus, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    parse_corpus(&text)
}
