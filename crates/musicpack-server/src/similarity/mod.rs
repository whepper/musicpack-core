//! Model-neutral music similarity indexing and exact search (Slice 0).
//!
//! This module implements the server side of ADR 0017 §5.7 over similarity
//! documents in the signed-off `.msim` v1 format (FORMAT_SPEC.md): a
//! profile-partitioned index of supplied f32 vectors, exact cosine queries,
//! and deterministic ordering. It contains no model, no runtime, no inference
//! and no ANN — the server indexes documents Author produced, exactly as
//! ADR 0017 D-5 requires.
//!
//! Boundaries, restated because they are easy to erode:
//!
//! - Vectors from different `profile_fingerprint` values are never compared,
//!   indexed together, or converted (ADR 0017 D-4/§8). The fingerprint is the
//!   partition key; the `profile_id` display string is never a key.
//! - Only `f32le` documents are indexed. `f16le` is assigned by the format
//!   but G-6 closed as KEEP F32LE, so it is refused for query use.
//! - Retrieval is exact cosine with no floors, no thresholds and no scores
//!   beyond the cosine itself. Ties break deterministically by ascending
//!   track row id — an implementation-defined rule (ADR 0017 pins no
//!   tie-break key; a content-defined key remains architecture decision D-4,
//!   still open).
//! - Similarity data is derived analysis, never musical identity: nothing
//!   here touches `group_key`, `release_key` or package identity.

pub mod reader;

pub use reader::{
    DocError, Document, ENCODING_F16LE, ENCODING_F32LE, MAX_DIMENSIONS, MAX_TRACK_COUNT, Member,
    STATUS_FAILED, STATUS_INSUFFICIENT_AUDIO, STATUS_OK, STATUS_UNSUPPORTED, hex_lower,
    status_name,
};

/// Registry encoding values the server implements for retrieval.
/// `f16le` is recognised by the reader but refused here (KEEP F32LE).
pub const IMPLEMENTED_ENCODING: u8 = ENCODING_F32LE;

/// Returns the `f32le` name for a registry encoding value, for API and log
/// surfaces. Unknown values are rendered numerically, never guessed.
pub fn encoding_name(encoding: u8) -> String {
    match encoding {
        ENCODING_F32LE => "f32le".to_string(),
        ENCODING_F16LE => "f16le".to_string(),
        other => format!("encoding({other})"),
    }
}

/// One ranked neighbour: the track row id and its exact cosine score.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredTrack {
    pub track_id: i64,
    pub score: f64,
}

/// Exact cosine similarity of two finite, non-zero f32 vectors.
///
/// Scalar loop in element order with `f64` accumulation: no SIMD lane
/// dependence, no reassociation, identical input bytes always produce
/// identical output bytes. (For L2-normalized vectors this equals the dot
/// product; the full cosine is computed anyway so unnormalized input cannot
/// silently change meaning.)
pub fn cosine(a: &[f32], b: &[f32]) -> Option<f64> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (f64::from(*x), f64::from(*y));
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return None;
    }
    Some(dot / (norm_a.sqrt() * norm_b.sqrt()))
}

/// Ranks candidates against a seed vector: exact cosine descending, ties
/// broken by ascending track id (deterministic and stable: row ids survive
/// rescans in place). Candidates that do not produce a finite score are
/// dropped rather than ordered arbitrarily. The seed itself is excluded by
/// the caller (SQL `track_id != ?`), never by score comparison.
pub fn rank(seed: &[f32], candidates: &[(i64, Vec<f32>)]) -> Vec<ScoredTrack> {
    let mut scored: Vec<ScoredTrack> = candidates
        .iter()
        .filter_map(|(id, vector)| {
            cosine(seed, vector).map(|score| ScoredTrack {
                track_id: *id,
                score,
            })
        })
        .collect();
    scored.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.track_id.cmp(&right.track_id))
    });
    scored
}

/// Decodes a stored `f32le` BLOB to floats. `None` on length mismatch (a
/// corrupt row fails closed at read time rather than decoding garbage).
pub fn decode_blob(bytes: &[u8]) -> Option<Vec<f32>> {
    if bytes.len() % 4 != 0 || bytes.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        out.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Some(out)
}

/// Encodes floats as a stored `f32le` BLOB.
pub fn encode_blob(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_matches_hand_computed_values() {
        let ortho = cosine(&[1.0, 0.0], &[0.0, 1.0]).unwrap();
        assert!((ortho - 0.0).abs() < 1e-12);
        let same = cosine(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]).unwrap();
        assert!((same - 1.0).abs() < 1e-12);
        // 45 degrees in 2-D.
        let half = cosine(&[1.0, 0.0], &[1.0, 1.0]).unwrap();
        assert!((half - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        // Scale invariance: the profile normalises, the query must not care.
        let scaled = cosine(&[1.0, 0.0], &[5.0, 5.0]).unwrap();
        assert!((scaled - half).abs() < 1e-12);
    }

    #[test]
    fn cosine_rejects_degenerate_inputs() {
        assert_eq!(cosine(&[], &[]), None);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), None);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), None);
        assert_eq!(cosine(&[f32::NAN, 0.0], &[1.0, 0.0]), None);
        assert_eq!(cosine(&[f32::INFINITY], &[1.0]), None);
    }

    #[test]
    fn rank_orders_by_score_with_stable_id_tiebreak() {
        let seed = vec![1.0, 0.0];
        let candidates = vec![
            (7, vec![0.0, 1.0]), // score 0
            (3, vec![1.0, 1.0]), // score ~0.707
            (5, vec![1.0, 1.0]), // score ~0.707, higher id loses the tie
            (9, vec![1.0, 0.0]), // score 1
        ];
        let ranked = rank(&seed, &candidates);
        assert_eq!(
            ranked.iter().map(|s| s.track_id).collect::<Vec<_>>(),
            vec![9, 3, 5, 7]
        );
        assert!((ranked[0].score - 1.0).abs() < 1e-12);
        assert!((ranked[1].score - ranked[2].score).abs() == 0.0);
    }

    #[test]
    fn rank_is_deterministic_across_input_orders() {
        let seed = vec![0.6, 0.8];
        let mut candidates = vec![
            (11, vec![0.0, 1.0]),
            (4, vec![1.0, 0.0]),
            (8, vec![0.6, 0.8]),
        ];
        let first = rank(&seed, &candidates);
        candidates.reverse();
        let second = rank(&seed, &candidates);
        assert_eq!(first, second);
    }

    #[test]
    fn blob_round_trip_is_exact() {
        let values = vec![1.0f32, -0.5, 0.0, 3.25];
        let blob = encode_blob(&values);
        assert_eq!(blob.len(), 16);
        assert_eq!(decode_blob(&blob).as_deref(), Some(values.as_slice()));
        assert_eq!(decode_blob(&[0u8; 3]), None);
        assert_eq!(decode_blob(&[]), None);
    }
}
