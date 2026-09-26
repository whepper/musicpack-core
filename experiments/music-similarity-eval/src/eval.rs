use std::collections::BTreeMap;

use crate::Result;
use crate::corpus::TrackSpec;

#[derive(Debug, Clone)]
pub struct TrackEmbedding {
    pub spec: TrackSpec,
    pub duration_seconds: f64,
    pub source_sha256: String,
    pub frame_count: usize,
    pub patch_count: usize,
    pub embedding_sha256: String,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct Neighbor {
    pub index: usize,
    pub score: f32,
}

#[derive(Debug, Clone)]
pub struct AlbumEmbedding {
    pub artist: String,
    pub album: String,
    pub track_count: usize,
    pub vector: Vec<f32>,
}

pub fn pool_mean_norm(window_embeddings: &[Vec<f32>], embedding_dim: usize) -> Result<Vec<f32>> {
    if window_embeddings.is_empty() {
        return Err("cannot pool an empty window set".into());
    }
    let mut sum = vec![0.0f64; embedding_dim];
    for embedding in window_embeddings {
        if embedding.len() != embedding_dim {
            return Err(format!(
                "window embedding has {} values; expected {embedding_dim}",
                embedding.len()
            )
            .into());
        }
        let norm = f64::from(norm(embedding));
        if norm == 0.0 || !norm.is_finite() {
            return Err("model returned a zero or non-finite embedding".into());
        }
        for (index, value) in embedding.iter().enumerate() {
            sum[index] += f64::from(*value) / norm;
        }
    }
    let mean = sum
        .into_iter()
        .map(|value| (value / window_embeddings.len() as f64) as f32)
        .collect::<Vec<_>>();
    let mean_norm = norm(&mean);
    if mean_norm == 0.0 || !mean_norm.is_finite() {
        return Err("pooled embedding is zero or non-finite".into());
    }
    Ok(mean
        .into_iter()
        .map(|value| (value / mean_norm) as f32)
        .collect())
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return f32::NAN;
    }
    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;
    for (left, right) in a.iter().zip(b) {
        dot += f64::from(*left) * f64::from(*right);
        norm_a += f64::from(*left) * f64::from(*left);
        norm_b += f64::from(*right) * f64::from(*right);
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return f32::NAN;
    }
    (dot / (norm_a.sqrt() * norm_b.sqrt())) as f32
}

pub fn nearest(records: &[TrackEmbedding], query: usize, top_k: usize) -> Vec<Neighbor> {
    let mut neighbors = records
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != query)
        .map(|(index, record)| Neighbor {
            index,
            score: cosine(&records[query].vector, &record.vector),
        })
        .filter(|neighbor| neighbor.score.is_finite())
        .collect::<Vec<_>>();
    neighbors.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.index.cmp(&right.index))
    });
    neighbors.truncate(top_k);
    neighbors
}

pub fn aggregate_albums(records: &[TrackEmbedding]) -> Vec<AlbumEmbedding> {
    let Some(embedding_dim) = records.first().map(|record| record.vector.len()) else {
        return Vec::new();
    };
    if records
        .iter()
        .any(|record| record.vector.len() != embedding_dim)
    {
        return Vec::new();
    }
    let mut groups: BTreeMap<(String, String), Vec<&TrackEmbedding>> = BTreeMap::new();
    for record in records {
        groups
            .entry((record.spec.artist.clone(), record.spec.album.clone()))
            .or_default()
            .push(record);
    }
    groups
        .into_iter()
        .filter_map(|((artist, album), tracks)| {
            let mut sum = vec![0.0f64; embedding_dim];
            for track in &tracks {
                for (index, value) in track.vector.iter().enumerate() {
                    sum[index] += f64::from(*value);
                }
            }
            let mean = sum
                .into_iter()
                .map(|value| (value / tracks.len() as f64) as f32)
                .collect::<Vec<_>>();
            let length = norm(&mean);
            if length == 0.0 || !length.is_finite() {
                return None;
            }
            Some(AlbumEmbedding {
                artist,
                album,
                track_count: tracks.len(),
                vector: mean
                    .into_iter()
                    .map(|value| (value / length) as f32)
                    .collect(),
            })
        })
        .collect()
}

pub fn album_nearest(albums: &[AlbumEmbedding], query: usize, top_k: usize) -> Vec<Neighbor> {
    let mut neighbors = albums
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != query)
        .map(|(index, album)| Neighbor {
            index,
            score: cosine(&albums[query].vector, &album.vector),
        })
        .filter(|neighbor| neighbor.score.is_finite())
        .collect::<Vec<_>>();
    neighbors.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.index.cmp(&right.index))
    });
    neighbors.truncate(top_k);
    neighbors
}

fn norm(values: &[f32]) -> f32 {
    let mut sum = 0.0f64;
    for value in values {
        sum += f64::from(*value) * f64::from(*value);
    }
    sum.sqrt() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str, vector: Vec<f32>) -> TrackEmbedding {
        TrackEmbedding {
            spec: TrackSpec {
                path: name.into(),
                artist: "artist".into(),
                album: "album".into(),
                title: name.into(),
            },
            duration_seconds: 1.0,
            source_sha256: String::new(),
            frame_count: 1,
            patch_count: 1,
            embedding_sha256: String::new(),
            vector,
        }
    }

    #[test]
    fn pooling_accepts_model_specific_dimension() {
        let pooled = pool_mean_norm(&[vec![3.0, 4.0], vec![3.0, 4.0]], 2).unwrap();
        assert_eq!(pooled.len(), 2);
        assert!((pooled[0] - 0.6).abs() < 1.0e-6);
        assert!((pooled[1] - 0.8).abs() < 1.0e-6);
    }

    #[test]
    fn cosine_ignores_scale() {
        let a = [1.0, 2.0, 3.0];
        let b = [2.0, 4.0, 6.0];
        assert!((cosine(&a, &b) - 1.0).abs() < 1.0e-6);
    }

    #[test]
    fn nearest_excludes_query_and_sorts_descending() {
        let records = vec![
            record("a", vec![1.0, 0.0]),
            record("b", vec![0.9, 0.1]),
            record("c", vec![0.0, 1.0]),
        ];
        let neighbors = nearest(&records, 0, 2);
        assert_eq!(neighbors[0].index, 1);
        assert_eq!(neighbors[1].index, 2);
    }
}
