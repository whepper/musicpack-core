use std::path::Path;
use std::sync::Arc;

use rten::{Dimension, Model, RunOptions, ThreadPool, ValueView};

use crate::Result;
use crate::mel::{MEL_BANDS, PATCH_SIZE};

pub const BATCH_SIZE: usize = 64;
pub const DEFAULT_EMBEDDING_DIM: usize = 1280;
const INPUT_NAME: &str = "serving_default_melspectrogram";

pub struct ModelRunner {
    model: Model,
    input_id: rten::NodeId,
    output_id: rten::NodeId,
    embedding_dim: usize,
    thread_pool: Arc<ThreadPool>,
}

impl ModelRunner {
    pub fn load(path: &Path) -> Result<Self> {
        let model = Model::load_file(path)?;
        let input_id = model.node_id(INPUT_NAME).or_else(|_| {
            model
                .input_ids()
                .first()
                .copied()
                .ok_or("model has no input")
        })?;
        let (output_id, embedding_dim) = select_embedding_output(&model)?;
        let thread_pool = Arc::new(ThreadPool::with_num_threads(1));
        Ok(Self {
            model,
            input_id,
            output_id,
            embedding_dim,
            thread_pool,
        })
    }

    pub fn input_name(&self) -> String {
        self.model
            .node_info(self.input_id)
            .and_then(|info| info.name().map(str::to_string))
            .unwrap_or_else(|| "unknown".to_string())
    }

    pub fn input_shape(&self) -> String {
        self.model
            .input_shape(0)
            .map(|shape| format!("{shape:?}"))
            .unwrap_or_else(|| "unknown".to_string())
    }

    pub fn output_shape(&self) -> String {
        self.model
            .node_info(self.output_id)
            .map(|info| format!("{:?}", info.shape()))
            .unwrap_or_else(|| "unknown".to_string())
    }

    pub fn embedding_dim(&self) -> usize {
        self.embedding_dim
    }

    pub fn output_name(&self) -> String {
        self.model
            .node_info(self.output_id)
            .and_then(|info| info.name().map(str::to_string))
            .unwrap_or_else(|| "unknown".to_string())
    }

    pub fn run_patches(&self, patches: &[Vec<f32>]) -> Result<Vec<Vec<f32>>> {
        if patches.is_empty() {
            return Ok(Vec::new());
        }
        let mut results = Vec::with_capacity(patches.len());
        for batch in patches.chunks(BATCH_SIZE) {
            let mut data = vec![0.0f32; BATCH_SIZE * PATCH_SIZE * MEL_BANDS];
            for (batch_index, patch) in batch.iter().enumerate() {
                if patch.len() != PATCH_SIZE * MEL_BANDS {
                    return Err(format!(
                        "patch {} has {} values; expected {}",
                        batch_index,
                        patch.len(),
                        PATCH_SIZE * MEL_BANDS
                    )
                    .into());
                }
                let start = batch_index * PATCH_SIZE * MEL_BANDS;
                data[start..start + patch.len()].copy_from_slice(patch);
            }

            let input = ValueView::from_shape([BATCH_SIZE, PATCH_SIZE, MEL_BANDS], &data)?;
            let options = RunOptions::default().with_thread_pool(Some(self.thread_pool.clone()));
            let [output] = self.model.run_n(
                vec![(self.input_id, input.into())],
                [self.output_id],
                Some(options),
            )?;
            let (shape, values) = output.into_shape_vec::<f32, 2>()?;
            if shape[0] != BATCH_SIZE || shape[1] != self.embedding_dim {
                return Err(format!(
                    "unexpected embedding output shape: {shape:?}; expected [{BATCH_SIZE}, {}]",
                    self.embedding_dim
                )
                .into());
            }
            results.extend(
                values
                    .chunks_exact(self.embedding_dim)
                    .take(batch.len())
                    .map(|chunk| chunk.to_vec()),
            );
        }
        Ok(results)
    }
}

fn select_embedding_output(model: &Model) -> Result<(rten::NodeId, usize)> {
    let candidates = model
        .output_ids()
        .iter()
        .copied()
        .filter_map(|id| {
            let dimension = output_dimension(model, id)?;
            Some((id, dimension))
        })
        .collect::<Vec<_>>();
    let selected = candidates
        .iter()
        .copied()
        .find(|(id, _)| {
            model
                .node_info(*id)
                .and_then(|info| info.name())
                .is_some_and(|name| name.to_ascii_lowercase().contains("embedding"))
        })
        .or_else(|| {
            candidates
                .iter()
                .copied()
                .find(|(_, dimension)| *dimension == DEFAULT_EMBEDDING_DIM)
        })
        .or_else(|| {
            candidates
                .iter()
                .copied()
                .max_by_key(|(_, dimension)| *dimension)
        })
        .ok_or("model has no 2-D embedding output")?;
    Ok(selected)
}

fn output_dimension(model: &Model, id: rten::NodeId) -> Option<usize> {
    let shape = model.node_info(id)?.shape()?;
    if shape.len() != 2 {
        return None;
    }
    match &shape[1] {
        Dimension::Fixed(size) => Some(*size),
        Dimension::Symbolic(_) => None,
    }
}
