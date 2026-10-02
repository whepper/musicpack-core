//! Sonic52 Slice 4B: tiny pure-Rust 52-D reference network.
//!
//! Plumbing validation for the research-v1 `[187, 96]` patch
//! representation — **not a model, not trained, not Plex**:
//!
//! ```text
//! [187, 96] mel patch (Slice 1 frontend output)
//!   → Tensor3 [1, 187, 96]          (explicit, tested conversion)
//!   → Conv2D (research placeholder) (linear: NO hidden activation;
//!                                      the reported chain states none
//!                                      here, so none is invented)
//!   → Flatten (channel-major)
//!   → Dense(200) + ReLU             (ReLU is S1-reported for this layer;
//!                                      see FORENSICS.md, not invented)
//!   → Dense(52, linear)
//!   → sigmoid
//!   → [52] f32 in [0, 1]
//! ```
//!
//! Correct terminology: "52-D reference network inspired by the
//! publicly reported structural characteristics." Never "the Plex
//! model", never a reproduction, never trained weights. The weights
//! below are deterministic **reference weights** from a transparent
//! integer formula — reproducible and debuggable, musically meaningless.
//!
//! Implementation policy: pure safe Rust, no SIMD, no external crates,
//! explicit loops with fixed evaluation order, `f32` arithmetic
//! throughout (the eventual representation is `f32`; cross-platform
//! last-ulp identity is not claimed). Auditability beats performance:
//! nothing here is optimized.

#![forbid(unsafe_code)]

use std::fmt;

use crate::experiment::{apply_aggregation, AggError, Aggregation};
use crate::frontend::{Sonic52MelPatch, MEL_BANDS, PATCH_FRAMES};
use crate::{EmbeddingError, Sonic52Embedding, Sonic52Profile};

// ---------------------------------------------------------------------
// research convolution configuration (explicit placeholder)
// ---------------------------------------------------------------------

/// Padding modes. Only `Valid` is implemented: the experiment does not
/// require any other mode, so no other mode exists here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Padding {
    /// No padding: `out = in - kernel + 1` per spatial axis.
    Valid,
}

/// Explicit research convolution configuration. Every parameter is
/// visible; nothing hides inside magic constants.
///
/// The values in [`RESEARCH_CONV_PLACEHOLDER`] are a **research
/// placeholder**, not lineage and not Plex facts: the forensic evidence
/// establishes no kernel/channel configuration, and the lineage
/// multi-branch musically-motivated kernels are deliberately NOT copied
/// (copying them would imply a lineage claim this track must not make).
/// A 3×3 stride-1 valid convolution with 4 output channels is the
/// smallest auditable choice that exercises real Conv2D indexing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResearchConvConfig {
    /// Input channels (1: single mel patch plane).
    pub input_channels: usize,
    /// Input height in frames (187: mel frames).
    pub input_height: usize,
    /// Input width in bands (96: mel bands).
    pub input_width: usize,
    /// Output channels (placeholder: 4).
    pub output_channels: usize,
    /// Kernel height (placeholder: 3).
    pub kernel_height: usize,
    /// Kernel width (placeholder: 3).
    pub kernel_width: usize,
    /// Vertical stride (placeholder: 1).
    pub stride_height: usize,
    /// Horizontal stride (placeholder: 1).
    pub stride_width: usize,
    /// Padding (only valid convolution exists here).
    pub padding: Padding,
}

/// The research placeholder convolution: 3×3, stride 1, valid, 1→4
/// channels. Output: 185 × 94 × 4 = 69,560 features.
pub const RESEARCH_CONV_PLACEHOLDER: ResearchConvConfig = ResearchConvConfig {
    input_channels: 1,
    input_height: PATCH_FRAMES,
    input_width: MEL_BANDS,
    output_channels: 4,
    kernel_height: 3,
    kernel_width: 3,
    stride_height: 1,
    stride_width: 1,
    padding: Padding::Valid,
};

impl ResearchConvConfig {
    /// Output height under valid convolution.
    pub fn output_height(&self) -> usize {
        self.input_height - self.kernel_height + 1
    }

    /// Output width under valid convolution.
    pub fn output_width(&self) -> usize {
        self.input_width - self.kernel_width + 1
    }

    /// Flattened Conv2D output length (dense-layer input width).
    pub fn output_features(&self) -> usize {
        self.output_channels * self.output_height() * self.output_width()
    }
}

// ---------------------------------------------------------------------
// explicit tensor layout: [channels, height, width], row-major
// ---------------------------------------------------------------------

/// Rank-3 tensor with explicit channel-major row-major storage:
///
/// ```text
/// index(c, h, w) = ((c * height) + h) * width + w
/// ```
///
/// For a single-channel mel patch this is `[1, 187, 96]` holding the
/// frontend's `[187, 96]` values in row-major order with no transpose
/// (proven by [`mel_patch_to_tensor`] test, not by inspection).
#[derive(Debug, Clone, PartialEq)]
pub struct Tensor3 {
    /// Channel count.
    pub channels: usize,
    /// Height (mel frames for patch input).
    pub height: usize,
    /// Width (mel bands for patch input).
    pub width: usize,
    /// `channels * height * width` values in the layout above.
    pub data: Vec<f32>,
}

/// What is wrong with a tensor or a network input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkError {
    /// Buffer length does not match the declared shape.
    ShapeMismatch {
        /// Declared shape as (channels, height, width).
        expected: (usize, usize, usize),
        /// Offered buffer length.
        found: usize,
    },
    /// Tensor dimensions do not match the convolution configuration.
    DimMismatch {
        /// Required (channels, height, width).
        expected: (usize, usize, usize),
        /// Offered (channels, height, width).
        found: (usize, usize, usize),
    },
    /// A non-finite input element (rejected, never propagated).
    NonFiniteInput,
}

impl fmt::Display for NetworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetworkError::ShapeMismatch { expected, found } => write!(
                f,
                "tensor buffer has {found} elements for shape {expected:?}"
            ),
            NetworkError::DimMismatch { expected, found } => write!(
                f,
                "tensor has dimensions {found:?}, convolution requires {expected:?}"
            ),
            NetworkError::NonFiniteInput => write!(f, "non-finite network input"),
        }
    }
}

impl std::error::Error for NetworkError {}

impl Tensor3 {
    /// Builds a tensor, rejecting buffers of the wrong length. Never
    /// resizes, crops, transposes, or pads.
    pub fn new(
        channels: usize,
        height: usize,
        width: usize,
        data: Vec<f32>,
    ) -> Result<Self, NetworkError> {
        if data.len() != channels * height * width {
            return Err(NetworkError::ShapeMismatch {
                expected: (channels, height, width),
                found: data.len(),
            });
        }
        Ok(Self {
            channels,
            height,
            width,
            data,
        })
    }

    /// Element access in the documented layout.
    pub fn get(&self, channel: usize, row: usize, col: usize) -> f32 {
        self.data[(channel * self.height + row) * self.width + col]
    }
}

/// Converts one H0 mel patch to the network input tensor `[1, 187, 96]`
/// with row-major order preserved and no transpose. Rejects non-finite
/// patch content.
pub fn mel_patch_to_tensor(patch: &Sonic52MelPatch) -> Result<Tensor3, NetworkError> {
    let mut data = Vec::with_capacity(PATCH_FRAMES * MEL_BANDS);
    for frame in patch.frames() {
        for value in frame {
            if !value.is_finite() {
                return Err(NetworkError::NonFiniteInput);
            }
            data.push(*value);
        }
    }
    Tensor3::new(1, PATCH_FRAMES, MEL_BANDS, data)
}

// ---------------------------------------------------------------------
// Conv2D: valid, biased, linear, fixed loop order
// ---------------------------------------------------------------------

/// Flat weight index: `((oc * in_ch + ic) * kh + kh) * kw + kw`.
fn conv_weight_index(
    output_channel: usize,
    input_channel: usize,
    kernel_row: usize,
    kernel_col: usize,
    config: &ResearchConvConfig,
) -> usize {
    ((output_channel * config.input_channels + input_channel) * config.kernel_height + kernel_row)
        * config.kernel_width
        + kernel_col
}

/// Valid biased convolution, no activation: the reported structural
/// chain states no operation between Conv2D and Flatten, so none is
/// implemented (adding ReLU here would invent architecture).
/// Accumulation is `f32` in fixed loop order
/// (oc → oh → ow → ic → kh → kw).
pub fn conv2d_valid(
    input: &Tensor3,
    config: &ResearchConvConfig,
    weights: &[f32],
    bias: &[f32],
) -> Result<Tensor3, NetworkError> {
    let expected = (
        config.input_channels,
        config.input_height,
        config.input_width,
    );
    let found = (input.channels, input.height, input.width);
    if found != expected {
        return Err(NetworkError::DimMismatch { expected, found });
    }
    let out_height = config.output_height();
    let out_width = config.output_width();
    let mut data = vec![0.0f32; config.output_channels * out_height * out_width];
    for oc in 0..config.output_channels {
        for oh in 0..out_height {
            for ow in 0..out_width {
                let mut sum = bias[oc];
                for ic in 0..config.input_channels {
                    for kh in 0..config.kernel_height {
                        for kw in 0..config.kernel_width {
                            let weight = weights[conv_weight_index(oc, ic, kh, kw, config)];
                            sum += weight
                                * input.get(
                                    ic,
                                    oh * config.stride_height + kh,
                                    ow * config.stride_width + kw,
                                );
                        }
                    }
                }
                data[(oc * out_height + oh) * out_width + ow] = sum;
            }
        }
    }
    Tensor3::new(config.output_channels, out_height, out_width, data)
}

// ---------------------------------------------------------------------
// flatten: channel-major (storage order), pinned by test
// ---------------------------------------------------------------------

/// Flattens in channel-major order (outermost channel, then row, then
/// column) — exactly the storage order, so flattening never depends on
/// incidental memory layout; the loops below are the definition. A
/// 2-channel sequential tensor makes any other ordering fail the test.
pub fn flatten_channel_major(tensor: &Tensor3) -> Vec<f32> {
    let mut out = Vec::with_capacity(tensor.data.len());
    for channel in 0..tensor.channels {
        for row in 0..tensor.height {
            for col in 0..tensor.width {
                out.push(tensor.get(channel, row, col));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------
// dense layer: explicit weights, bias, fixed order, f32
// ---------------------------------------------------------------------

/// Generic dense layer: `out[j] = bias[j] + Σ_i weights[j][i] * x[i]`
/// with row-major weights (`weights[j * input_dim + i]`), `f32`
/// accumulation in fixed `j → i` order, no normalization, no
/// activation. (Rectification, where the reported chain includes it,
/// is applied explicitly by the caller — see [`relu_in_place`].)
#[derive(Debug, Clone, PartialEq)]
pub struct Dense {
    /// Input width.
    pub input_dim: usize,
    /// Output width.
    pub output_dim: usize,
    /// `output_dim * input_dim` weights, row-major.
    pub weights: Vec<f32>,
    /// `output_dim` biases.
    pub bias: Vec<f32>,
}

/// Dense construction failures (independent of network topology).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenseError {
    /// Weight count is not `input_dim * output_dim`.
    BadWeights {
        /// Expected count.
        expected: usize,
        /// Offered count.
        found: usize,
    },
    /// Bias count is not `output_dim`.
    BadBias {
        /// Expected count.
        expected: usize,
        /// Offered count.
        found: usize,
    },
    /// Input length is not `input_dim`. Never resized or padded.
    DimMismatch {
        /// Expected length.
        expected: usize,
        /// Offered length.
        found: usize,
    },
}

impl fmt::Display for DenseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DenseError::BadWeights { expected, found } => {
                write!(
                    f,
                    "dense weights have {found} elements, expected {expected}"
                )
            }
            DenseError::BadBias { expected, found } => {
                write!(f, "dense bias has {found} elements, expected {expected}")
            }
            DenseError::DimMismatch { expected, found } => {
                write!(f, "dense input has {found} elements, expected {expected}")
            }
        }
    }
}

impl std::error::Error for DenseError {}

impl Dense {
    /// Builds a dense layer, rejecting wrongly sized weights/bias.
    pub fn new(
        input_dim: usize,
        output_dim: usize,
        weights: Vec<f32>,
        bias: Vec<f32>,
    ) -> Result<Self, DenseError> {
        if weights.len() != input_dim * output_dim {
            return Err(DenseError::BadWeights {
                expected: input_dim * output_dim,
                found: weights.len(),
            });
        }
        if bias.len() != output_dim {
            return Err(DenseError::BadBias {
                expected: output_dim,
                found: bias.len(),
            });
        }
        Ok(Self {
            input_dim,
            output_dim,
            weights,
            bias,
        })
    }

    /// Forward evaluation in fixed order.
    pub fn forward(&self, input: &[f32]) -> Result<Vec<f32>, DenseError> {
        if input.len() != self.input_dim {
            return Err(DenseError::DimMismatch {
                expected: self.input_dim,
                found: input.len(),
            });
        }
        let mut out = vec![0.0f32; self.output_dim];
        for (j, slot) in out.iter_mut().enumerate() {
            let mut sum = self.bias[j];
            for (i, value) in input.iter().enumerate() {
                sum += self.weights[j * self.input_dim + i] * value;
            }
            *slot = sum;
        }
        Ok(out)
    }
}

/// Rectified linear unit, applied element-wise. Called out explicitly
/// wherever the reported chain includes it (after Dense(200) only);
/// never hidden inside [`Dense::forward`].
pub fn relu_in_place(values: &mut [f32]) {
    for value in values {
        if *value < 0.0 {
            *value = 0.0;
        }
    }
}

// ---------------------------------------------------------------------
// sigmoid: 1 / (1 + exp(-x)), finite on extremes
// ---------------------------------------------------------------------

/// Logistic sigmoid. `exp` overflow is harmless by construction:
/// large positive `x` gives `exp(-x) = 0` → 1; large negative `x`
/// gives `exp(-x) = +inf` → `1/inf = 0`. Both are finite and in range;
/// NaN input is the caller's contract violation (rejected upstream).
pub fn sigmoid_scalar(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Element-wise sigmoid.
pub fn sigmoid_slice(values: &[f32]) -> Vec<f32> {
    values.iter().map(|v| sigmoid_scalar(*v)).collect()
}

// ---------------------------------------------------------------------
// deterministic reference weights (transparent integer formula)
// ---------------------------------------------------------------------

/// Reference weight in (−0.1, 0.1) from pure integer arithmetic
/// (wrapping multiplies with distinct odd constants, modulo 1000).
/// Same `(seed, i, j)` always yields the same value; no RNG, no hidden
/// state, no platform libm involvement. **Reference weights are not
/// trained, not meaningful, and must never be described as model
/// weights.**
pub fn reference_weight(seed: u64, i: usize, j: usize) -> f32 {
    let n = (i as u64)
        .wrapping_mul(0x9E37_79B1)
        .wrapping_add((j as u64).wrapping_mul(0x85EB_CA6B))
        .wrapping_add(seed)
        .wrapping_add(0xC2B2_AE35)
        % 1000;
    (n as f32 / 1000.0 - 0.5) * 0.2
}

/// Reference bias in (−0.05, 0.05), same discipline, distinct constants.
pub fn reference_bias(seed: u64, j: usize) -> f32 {
    let n = (j as u64)
        .wrapping_mul(0x27D4_EB2F)
        .wrapping_add(seed)
        .wrapping_add(0x1656_25E3)
        % 1000;
    (n as f32 / 1000.0 - 0.5) * 0.1
}

/// Builds one flat row-major weight matrix from the reference formula.
pub fn reference_weights(seed: u64, rows: usize, cols: usize) -> Vec<f32> {
    let mut weights = Vec::with_capacity(rows * cols);
    for i in 0..rows {
        for j in 0..cols {
            weights.push(reference_weight(seed, i, j));
        }
    }
    weights
}

/// Builds one bias vector from the reference formula.
pub fn reference_biases(seed: u64, len: usize) -> Vec<f32> {
    (0..len).map(|j| reference_bias(seed, j)).collect()
}

// ---------------------------------------------------------------------
// the 52-D reference network
// ---------------------------------------------------------------------

/// Research seed identifying one reference-weight instantiation (part of
/// reproducibility reporting, not a model version).
pub const REFERENCE_SEED: u64 = 0x5EED_5252;

/// Width of the hidden dense layer (the reported 200).
pub const DENSE_HIDDEN: usize = 200;

/// Width of the output layer (the reported 52).
pub const DENSE_OUTPUT: usize = 52;

/// A 52-D reference network instance: placeholder convolution plus two
/// dense layers with formula-generated reference weights.
///
/// Construction is `reference_network(seed)`; there is no weight
/// loading, no file format, no training state.
#[derive(Debug, Clone, PartialEq)]
pub struct Sonic52ReferenceNetwork {
    /// Convolution configuration (always the research placeholder).
    pub conv: ResearchConvConfig,
    /// Flat convolution weights (see [`conv_weight_index`]).
    pub conv_weights: Vec<f32>,
    /// Convolution biases (one per output channel).
    pub conv_bias: Vec<f32>,
    /// Hidden dense layer (conv features → 200).
    pub dense_hidden: Dense,
    /// Output dense layer (200 → 52, linear; sigmoid applied after).
    pub dense_output: Dense,
}

/// Network-level failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardError {
    /// Tensor/cousin shape problem (see [`NetworkError`]).
    Network(NetworkError),
    /// Dense problem (see [`DenseError`]).
    Dense(DenseError),
    /// A non-finite value reached a stage input.
    NonFinite,
}

impl fmt::Display for ForwardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ForwardError::Network(error) => write!(f, "network shape: {error}"),
            ForwardError::Dense(error) => write!(f, "network dense: {error}"),
            ForwardError::NonFinite => write!(f, "non-finite value in network"),
        }
    }
}

impl std::error::Error for ForwardError {}

impl From<NetworkError> for ForwardError {
    fn from(error: NetworkError) -> Self {
        ForwardError::Network(error)
    }
}

impl From<DenseError> for ForwardError {
    fn from(error: DenseError) -> Self {
        ForwardError::Dense(error)
    }
}

/// Builds the reference network for one seed. Infallible by
/// construction: every dimension derives from the placeholder config,
/// and every weight comes from the reference formula.
pub fn reference_network(seed: u64) -> Sonic52ReferenceNetwork {
    let conv = RESEARCH_CONV_PLACEHOLDER;
    let features = conv.output_features();
    let conv_weights = reference_weights(
        seed.wrapping_add(0xC0FFEE),
        conv.output_channels,
        conv.input_channels * conv.kernel_height * conv.kernel_width,
    );
    let conv_bias = reference_biases(seed.wrapping_add(0xB1A5), conv.output_channels);
    let dense_hidden = Dense::new(
        features,
        DENSE_HIDDEN,
        reference_weights(seed.wrapping_add(0xD200), DENSE_HIDDEN, features),
        reference_biases(seed.wrapping_add(0xD201), DENSE_HIDDEN),
    )
    .expect("reference dense dimensions are consistent");
    let dense_output = Dense::new(
        DENSE_HIDDEN,
        DENSE_OUTPUT,
        reference_weights(seed.wrapping_add(0xD052), DENSE_OUTPUT, DENSE_HIDDEN),
        reference_biases(seed.wrapping_add(0xD053), DENSE_OUTPUT),
    )
    .expect("reference dense dimensions are consistent");
    Sonic52ReferenceNetwork {
        conv,
        conv_weights,
        conv_bias,
        dense_hidden,
        dense_output,
    }
}

fn check_finite(values: &[f32]) -> Result<(), ForwardError> {
    if values.iter().any(|v| !v.is_finite()) {
        return Err(ForwardError::NonFinite);
    }
    Ok(())
}

impl Sonic52ReferenceNetwork {
    /// Forward evaluation of one H0 patch: tensor → conv (linear) →
    /// flatten → dense(200) → ReLU → dense(52) → sigmoid, yielding 52
    /// finite values in [0, 1]. Fails closed on non-finite input; shapes
    /// are enforced by construction ([`Sonic52MelPatch`] is 187×96 by
    /// type, so no runtime shape check is needed here).
    pub fn forward(&self, patch: &Sonic52MelPatch) -> Result<[f32; 52], ForwardError> {
        let tensor = mel_patch_to_tensor(patch)?;
        let convolved = conv2d_valid(&tensor, &self.conv, &self.conv_weights, &self.conv_bias)?;
        check_finite(&convolved.data)?;
        let mut hidden = self
            .dense_hidden
            .forward(&flatten_channel_major(&convolved))?;
        check_finite(&hidden)?;
        relu_in_place(&mut hidden);
        let logits = self.dense_output.forward(&hidden)?;
        check_finite(&logits)?;
        let activated = sigmoid_slice(&logits);
        let mut output = [0.0f32; DENSE_OUTPUT];
        output.copy_from_slice(&activated);
        Ok(output)
    }

    /// One 52-D vector per patch, in order.
    pub fn forward_many(
        &self,
        patches: &[Sonic52MelPatch],
    ) -> Result<Vec<[f32; 52]>, ForwardError> {
        patches.iter().map(|patch| self.forward(patch)).collect()
    }
}

/// Aggregates 52-D reference vectors with a Slice 3 aggregation into a
/// single 52-D research vector (`None` aggregation yields `None`).
/// Pure plumbing for the patch → vector → aggregation demonstration.
pub fn aggregate_reference_vectors(
    vectors: &[[f32; 52]],
    aggregation: Aggregation,
) -> Result<Option<[f32; 52]>, AggError> {
    let owned: Vec<Vec<f32>> = vectors.iter().map(|v| v.to_vec()).collect();
    match apply_aggregation(&owned, aggregation)? {
        Some(values) => {
            let mut output = [0.0f32; DENSE_OUTPUT];
            if values.len() != DENSE_OUTPUT {
                return Err(AggError::DimMismatch {
                    expected: DENSE_OUTPUT,
                    found: values.len(),
                });
            }
            output.copy_from_slice(&values);
            Ok(Some(output))
        }
        None => Ok(None),
    }
}

/// Little-endian serialization of one 52-D reference output: exactly
/// 208 bytes. Experiment-level representation only — never an `.msim`
/// record (see module docs).
pub fn serialize_output(output: &[f32; 52]) -> [u8; 208] {
    let mut bytes = [0u8; 208];
    for (slot, value) in bytes.chunks_exact_mut(4).zip(output.iter()) {
        slot.copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Bridges a reference output into the scaffold's 52-D embedding type
/// under a caller-supplied profile identity. The network never invents
/// identity: `profile` is explicit, so no fake fingerprint can slip in.
pub fn embedding_from_output(
    profile: &Sonic52Profile,
    output: [f32; 52],
) -> Result<Sonic52Embedding, EmbeddingError> {
    Sonic52Embedding::from_vec(profile, output.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experiment::EXPERIMENT_CONFIG_H0;
    use crate::frontend::MelFrontend;

    // -- tensor layout -----------------------------------------------------

    #[test]
    fn tensor_layout_is_channel_major_row_major() {
        // 2 channels × 2 rows × 3 cols, sequential values: any other
        // ordering (row-major-channel-last, transposed, …) fails below.
        let tensor = Tensor3::new(2, 2, 3, (0..12).map(|v| v as f32).collect()).unwrap();
        assert_eq!(tensor.get(0, 0, 0), 0.0);
        assert_eq!(tensor.get(0, 0, 2), 2.0);
        assert_eq!(tensor.get(0, 1, 0), 3.0);
        assert_eq!(tensor.get(1, 0, 0), 6.0);
        assert_eq!(tensor.get(1, 1, 2), 11.0);
        assert_eq!(
            flatten_channel_major(&tensor),
            (0..12).map(|v| v as f32).collect::<Vec<_>>()
        );
        assert_eq!(
            Tensor3::new(1, 2, 2, vec![0.0; 3]).unwrap_err(),
            NetworkError::ShapeMismatch {
                expected: (1, 2, 2),
                found: 3
            }
        );
    }

    #[test]
    fn mel_patch_converts_without_transpose() {
        let mut frames = [[0.0f32; MEL_BANDS]; PATCH_FRAMES];
        frames[3][7] = 1.5;
        frames[186][95] = -2.25;
        let patch = Sonic52MelPatch::from_frames(frames);
        let tensor = mel_patch_to_tensor(&patch).unwrap();
        assert_eq!((tensor.channels, tensor.height, tensor.width), (1, 187, 96));
        assert_eq!(tensor.get(0, 3, 7), 1.5);
        assert_eq!(tensor.get(0, 186, 95), -2.25);
        assert_eq!(tensor.get(0, 0, 0), 0.0);
    }

    // -- tiny hand-computed convolution ---------------------------------------

    fn tiny_conv() -> (Tensor3, ResearchConvConfig, Vec<f32>, Vec<f32>) {
        // Input 1×3×3, kernel 2×2 [[1,2],[3,4]], bias [100] → 1×2×2:
        // out = 1a+2b+3c+4d + 100 over each window of
        // [[1,2,3],[4,5,6],[7,8,9]] = [[137,147],[167,177]].
        let tensor = Tensor3::new(1, 3, 3, (1..=9).map(|v| v as f32).collect()).unwrap();
        let config = ResearchConvConfig {
            input_channels: 1,
            input_height: 3,
            input_width: 3,
            output_channels: 1,
            kernel_height: 2,
            kernel_width: 2,
            stride_height: 1,
            stride_width: 1,
            padding: Padding::Valid,
        };
        (tensor, config, vec![1.0, 2.0, 3.0, 4.0], vec![100.0])
    }

    #[test]
    fn conv2d_tiny_example_is_exact() {
        let (tensor, config, weights, bias) = tiny_conv();
        let out = conv2d_valid(&tensor, &config, &weights, &bias).unwrap();
        assert_eq!((out.channels, out.height, out.width), (1, 2, 2));
        assert_eq!(out.data, vec![137.0, 147.0, 167.0, 177.0]);
    }

    #[test]
    fn conv2d_channels_bias_and_rejection() {
        // 2 input channels, 1×2 spatial, 1×1 kernel, weights [2,3]:
        // out = 2*ch0 + 3*ch1 + bias per position.
        let tensor = Tensor3::new(2, 1, 2, vec![4.0, 5.0, 6.0, 7.0]).unwrap();
        let config = ResearchConvConfig {
            input_channels: 2,
            input_height: 1,
            input_width: 2,
            output_channels: 1,
            kernel_height: 1,
            kernel_width: 1,
            stride_height: 1,
            stride_width: 1,
            padding: Padding::Valid,
        };
        let out = conv2d_valid(&tensor, &config, &[2.0, 3.0], &[1.0]).unwrap();
        assert_eq!(
            out.data,
            vec![2.0 * 4.0 + 3.0 * 6.0 + 1.0, 2.0 * 5.0 + 3.0 * 7.0 + 1.0]
        );
        // Wrong input dimensions are rejected, never adapted.
        let wrong = Tensor3::new(1, 1, 2, vec![0.0; 2]).unwrap();
        assert_eq!(
            conv2d_valid(&wrong, &config, &[2.0, 3.0], &[1.0]).unwrap_err(),
            NetworkError::DimMismatch {
                expected: (2, 1, 2),
                found: (1, 1, 2)
            }
        );
    }

    #[test]
    fn conv2d_scatter_gather_agreement() {
        // Independent reimplementation with reversed data flow: the
        // implementation gathers each output from its receptive field,
        // while this reference scatters each input onto the outputs it
        // touches. Same mathematics, different loop nesting and
        // accumulation order — agreement within float-reordering noise
        // proves the indexing (index bugs show up as O(1) disagreements,
        // rounding as ~1e-7).
        let input = Tensor3::new(1, 4, 5, (1..=20).map(|v| v as f32).collect()).unwrap();
        let config = ResearchConvConfig {
            input_channels: 1,
            input_height: 4,
            input_width: 5,
            output_channels: 2,
            kernel_height: 2,
            kernel_width: 2,
            stride_height: 1,
            stride_width: 1,
            padding: Padding::Valid,
        };
        let weights: Vec<f32> = (1..=8).map(|v| v as f32).collect();
        let bias = vec![10.0f32, 20.0];
        let gathered = conv2d_valid(&input, &config, &weights, &bias).unwrap();
        // Scatter reference: out[oc, oh, ow] starts at bias; each input
        // (h, w) contributes to outputs (h-kh, w-kw).
        let (oh, ow) = (config.output_height(), config.output_width());
        let mut scattered = vec![0.0f32; 2 * oh * ow];
        for oc in 0..2 {
            for k in 0..oh * ow {
                scattered[(oc * oh * ow) + k] = bias[oc];
            }
        }
        for h in 0..4usize {
            for w in 0..5usize {
                let value = input.get(0, h, w);
                for oc in 0..2 {
                    for kh in 0..2 {
                        for kw in 0..2 {
                            if h >= kh && w >= kw && h - kh < oh && w - kw < ow {
                                let weight = weights[(oc * 4 + kh * 2) + kw];
                                scattered[(oc * oh + (h - kh)) * ow + (w - kw)] += weight * value;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(gathered.data.len(), scattered.len());
        for (gathered_value, scattered_value) in gathered.data.iter().zip(scattered.iter()) {
            let scale = gathered_value.abs().max(1.0);
            assert!(
                (gathered_value - scattered_value).abs() / scale < 1e-5,
                "gather {gathered_value} vs scatter {scattered_value}"
            );
        }
    }

    // -- flatten --------------------------------------------------------------

    #[test]
    fn flatten_order_is_pinned() {
        // Channel-major: channel 0 fully, then channel 1. The
        // row-major-channel-last alternative would read
        // [0,6,1,7,2,8,3,9,4,10,5,11] and fail here.
        let tensor = Tensor3::new(2, 2, 3, (0..12).map(|v| v as f32).collect()).unwrap();
        assert_eq!(
            flatten_channel_major(&tensor),
            vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0]
        );
    }

    // -- dense ------------------------------------------------------------------

    #[test]
    fn dense_tiny_example_is_exact() {
        // 3 → 2, row-major [[1,2,3],[4,5,6]], bias [10,20],
        // x = [1,1,1] → [16,35].
        let dense = Dense::new(3, 2, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0], vec![10.0, 20.0]).unwrap();
        assert_eq!(dense.forward(&[1.0, 1.0, 1.0]).unwrap(), vec![16.0, 35.0]);
        assert_eq!(
            dense.forward(&[1.0]).unwrap_err(),
            DenseError::DimMismatch {
                expected: 3,
                found: 1
            }
        );
        assert_eq!(
            Dense::new(3, 2, vec![1.0; 5], vec![0.0; 2]).unwrap_err(),
            DenseError::BadWeights {
                expected: 6,
                found: 5
            }
        );
        assert_eq!(
            Dense::new(3, 2, vec![1.0; 6], vec![0.0; 3]).unwrap_err(),
            DenseError::BadBias {
                expected: 2,
                found: 3
            }
        );
        // ReLU is explicit and separate: negatives zero, rest untouched.
        let mut values = vec![-1.5, 0.0, 2.5];
        relu_in_place(&mut values);
        assert_eq!(values, vec![0.0, 0.0, 2.5]);
    }

    // -- sigmoid ------------------------------------------------------------------

    #[test]
    fn sigmoid_values_and_extremes() {
        assert_eq!(sigmoid_scalar(0.0), 0.5);
        let plus = sigmoid_scalar(2.0);
        let expected = (1.0 / (1.0 + (-2.0f64).exp())) as f32;
        assert!((plus - expected).abs() < 1e-7);
        assert!((sigmoid_scalar(-2.0) - (1.0 - expected)).abs() < 1e-7);
        // Extremes stay finite and in range (exp overflow is absorbed).
        assert_eq!(sigmoid_scalar(1000.0), 1.0);
        assert_eq!(sigmoid_scalar(-1000.0), 0.0);
        assert_eq!(sigmoid_scalar(f32::MAX), 1.0);
        assert_eq!(sigmoid_scalar(f32::MIN), 0.0);
        for value in sigmoid_slice(&[-1000.0, -1.0, 0.0, 1.0, 1000.0]) {
            assert!(value.is_finite());
            assert!((0.0..=1.0).contains(&value));
        }
    }

    // -- reference weights ------------------------------------------------------------

    #[test]
    fn reference_weights_are_deterministic_and_bounded() {
        let a = reference_weights(7, 4, 9);
        let b = reference_weights(7, 4, 9);
        assert_eq!(a, b);
        assert!(a.iter().all(|w| w.abs() <= 0.1));
        assert!(a.iter().any(|w| *w != 0.0));
        let c = reference_weights(8, 4, 9);
        assert_ne!(a, c);
        let biases = reference_biases(7, 200);
        assert_eq!(biases.len(), 200);
        assert!(biases.iter().all(|w| w.abs() <= 0.05));
    }

    #[test]
    fn relu_placement_is_observable() {
        // Prove the ReLU sits after Dense(200) and only there: zero the
        // hidden weights with a negative bias so every pre-activation is
        // negative; the output must then equal sigmoid(output bias)
        // exactly. (If ReLU were missing, the negative bias would leak
        // through dense_output; if it were elsewhere, this equality
        // would not hold.)
        let mut network = reference_network(REFERENCE_SEED);
        let hidden_dim = network.dense_hidden.input_dim;
        network.dense_hidden.weights = vec![0.0; 200 * hidden_dim];
        network.dense_hidden.bias = vec![-5.0; 200];
        let output = network.forward(&formula_patch(0)).unwrap();
        let expected = sigmoid_slice(&network.dense_output.bias);
        assert_eq!(output.to_vec(), expected);
    }

    // -- full-shape network ------------------------------------------------------------

    fn formula_patch(offset: usize) -> Sonic52MelPatch {
        // Deterministic formula patch (no audio needed): values in
        // [-0.5, 0.5), full-rank content across rows and columns.
        let mut frames = [[0.0f32; MEL_BANDS]; PATCH_FRAMES];
        for (row, frame) in frames.iter_mut().enumerate() {
            for (col, slot) in frame.iter_mut().enumerate() {
                *slot = ((row * 97 + col * 13 + offset) % 89) as f32 / 89.0 - 0.5;
            }
        }
        Sonic52MelPatch::from_frames(frames)
    }

    #[test]
    fn full_network_topology_and_invariants() {
        let network = reference_network(REFERENCE_SEED);
        assert_eq!(RESEARCH_CONV_PLACEHOLDER.output_features(), 69_560);
        assert_eq!(network.dense_hidden.input_dim, 69_560);
        assert_eq!(network.dense_hidden.output_dim, 200);
        assert_eq!(network.dense_output.input_dim, 200);
        assert_eq!(network.dense_output.output_dim, 52);
        let output = network.forward(&formula_patch(0)).unwrap();
        assert_eq!(output.len(), 52);
        assert!(output.iter().all(|v| v.is_finite()));
        assert!(output.iter().all(|v| (0.0..=1.0).contains(v)));
        // A different patch gives a different vector (sensitivity smoke).
        let other = network.forward(&formula_patch(41)).unwrap();
        assert_ne!(output, other);
        // A different seed gives a different network.
        let network2 = reference_network(REFERENCE_SEED + 1);
        assert_ne!(
            network.forward(&formula_patch(0)).unwrap(),
            network2.forward(&formula_patch(0)).unwrap()
        );
    }

    #[test]
    fn full_network_digest_pending() {
        let network = reference_network(REFERENCE_SEED);
        let output = network.forward(&formula_patch(0)).unwrap();
        let digest = musicpack_core::format::checksum::sha256_hex(&serialize_output(&output));
        // Pinned only after independent verification above (exact tiny
        // conv/dense/flatten goldens, scatter/gather agreement, ReLU
        // placement proof, f64 sigmoid reference). Debug, release, and
        // MSRV profiles must all agree — see validation.
        assert_eq!(
            digest,
            "10afb15430c74125929265ad04af3613851f84e3692e9f8d1d9335b3c480613d"
        );
    }

    #[test]
    fn forward_rejects_non_finite_input() {
        let network = reference_network(REFERENCE_SEED);
        let mut frames = [[0.0f32; MEL_BANDS]; PATCH_FRAMES];
        frames[0][0] = f32::NAN;
        let patch = Sonic52MelPatch::from_frames(frames);
        assert_eq!(
            network.forward(&patch).unwrap_err(),
            ForwardError::Network(NetworkError::NonFiniteInput)
        );
    }

    // -- serialization + scaffold bridge -------------------------------------

    #[test]
    fn output_serialization_is_208_deterministic_bytes() {
        let output = reference_network(REFERENCE_SEED)
            .forward(&formula_patch(0))
            .unwrap();
        let bytes = serialize_output(&output);
        assert_eq!(bytes.len(), 208);
        assert_eq!(bytes, serialize_output(&output));
        // Round-trip: little-endian decode reproduces the vector.
        let mut decoded = [0.0f32; 52];
        for (slot, word) in decoded.iter_mut().zip(bytes.chunks_exact(4)) {
            *slot = f32::from_le_bytes(word.try_into().expect("4 bytes"));
        }
        assert_eq!(decoded, output);
    }

    #[test]
    fn embedding_bridge_uses_caller_identity() {
        // The scaffold type is reusable with no coupling: identity comes
        // from the caller, never from the network.
        let profile = Sonic52Profile::research_v1([0x5a; 32]).unwrap();
        let output = reference_network(REFERENCE_SEED)
            .forward(&formula_patch(0))
            .unwrap();
        let embedding = embedding_from_output(&profile, output).unwrap();
        assert_eq!(embedding.values(), &output);
        assert!(embedding.is_sigmoid_range());
    }

    // -- multi-patch plumbing ----------------------------------------------------

    #[test]
    fn multipatch_vectors_aggregate_to_one() {
        let network = reference_network(REFERENCE_SEED);
        let patches = [formula_patch(0), formula_patch(41), formula_patch(82)];
        let vectors = network.forward_many(&patches).unwrap();
        assert_eq!(vectors.len(), 3);
        // Research v1 aggregation (Mean) over the three vectors.
        let aggregate = aggregate_reference_vectors(&vectors, Aggregation::Mean)
            .unwrap()
            .unwrap();
        assert!(aggregate.iter().all(|v| v.is_finite()));
        let digest = musicpack_core::format::checksum::sha256_hex(&serialize_output(&aggregate));
        assert_eq!(
            digest,
            "ed802fe19360d17c2a83bdbc38fdc2ac14a612831b36ed2f5b917715ca3fedf1"
        );
        // None yields None; dimension safety holds by type.
        assert_eq!(
            aggregate_reference_vectors(&vectors, Aggregation::None).unwrap(),
            None
        );
        // The same vectors through the experiment harness agree.
        let owned: Vec<Vec<f32>> = vectors.iter().map(|v| v.to_vec()).collect();
        let via_harness = crate::experiment::mean_vector(&owned).unwrap();
        assert_eq!(via_harness, aggregate.to_vec());
    }

    // -- research-v1 end to end (audio-free patch path) -------------------------------

    #[test]
    fn research_v1_contract_runs_through_harness() {
        // Patches from the Slice 1 frontend flow through layout and the
        // reference network under the frozen v1 contract fields.
        assert_eq!(crate::experiment::RESEARCH_V1_CONTRACT.patch_stride, 93);
        let signal: Vec<f32> = (0..60_000)
            .map(|i| ((i as f64 * 0.021).sin() * 0.3) as f32)
            .collect();
        let frontend = MelFrontend::process_contiguous(&signal);
        let laid = crate::experiment::layout_patches(
            &frontend.mel_frames,
            crate::experiment::PATCH_CONFIG_H0,
        );
        assert!(!laid.is_empty());
        let network = reference_network(REFERENCE_SEED);
        let first = network.forward(&laid[0]).unwrap();
        assert!(first.iter().all(|v| (0.0..=1.0).contains(v)));
        // Under the v1 contract the experiment harness itself accepts
        // the configuration (shape-level check, no audio needed here).
        assert_eq!(EXPERIMENT_CONFIG_H0.patch_stride, 93);
    }
}
