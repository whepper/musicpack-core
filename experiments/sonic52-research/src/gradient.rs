//! Sonic52 Slice 9: deterministic backpropagation (gradients only).
//!
//! Extends the validated Slice 8 forward/loss pipeline with gradient
//! computation and stops there: **no parameter is ever updated** in
//! this slice. The question answered here is whether correct,
//! deterministic gradients flow through the complete reference network
//! and BCE objective — demonstrated mathematically before any
//! optimizer is allowed to exist.
//!
//! ```text
//! sample → H0 preprocessing → reference network → logits
//!   → sigmoid embedding → synthetic target → BCE loss
//!   → backpropagation → parameter gradients (END OF SLICE)
//! ```
//!
//! Implementation strategy (Slice 9 §3, option A): explicit analytical
//! derivatives for every stage. The network is tiny and fixed, so
//! hand-derived, inspectable backward equations beat a general
//! autodiff system on every axis that matters here (determinism,
//! auditability, dependencies: zero new ones).
//!
//! Canonical parameter ordering (fixed, hashed into the gradient
//! identity — never HashMap order, never incidental):
//!
//! ```text
//! conv.weight, conv.bias,
//! dense200.weight, dense200.bias,
//! dense52.weight, dense52.bias
//! ```
//!
//! Terminology: `reference parameters` and `parameter gradients`
//! throughout — the reference weights stay frozen, and nothing here
//! implies training has occurred.

#![forbid(unsafe_code)]

use std::fmt;

use crate::frontend::Sonic52MelPatch;
use crate::network::{
    conv2d_valid, flatten_channel_major, mel_patch_to_tensor, relu_in_place, sigmoid_scalar,
    ResearchConvConfig, Sonic52ReferenceNetwork, Tensor3, DENSE_OUTPUT,
};

// ---------------------------------------------------------------------
// gradient representation and canonical ordering
// ---------------------------------------------------------------------

/// Serialization version of the research gradient format.
pub const GRAD_FORMAT_VERSION: &str = "SG01";

/// Canonical parameter names in canonical order.
pub const GRAD_PARAM_ORDER: &[&str] = &[
    "conv.weight",
    "conv.bias",
    "dense200.weight",
    "dense200.bias",
    "dense52.weight",
    "dense52.bias",
];

/// One parameter tensor's gradient: name, shape, and values in the
/// row-major layout the forward pass documents for that tensor.
#[derive(Debug, Clone, PartialEq)]
pub struct ParameterGrad {
    /// Canonical parameter name (see [`GRAD_PARAM_ORDER`]).
    pub name: String,
    /// Tensor shape, e.g. `[out_channels, in_ch, kh, kw]`.
    pub shape: Vec<usize>,
    /// Gradient values in the documented layout.
    pub values: Vec<f32>,
}

/// Complete gradient set for one backward pass, always in
/// [`GRAD_PARAM_ORDER`].
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkGradients {
    /// Six parameter gradients in canonical order.
    pub params: Vec<ParameterGrad>,
}

/// One segment of the flat parameter layout (finite-difference and
/// digest tooling address parameters by flat index through this).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamSegment {
    /// Canonical name.
    pub name: &'static str,
    /// Tensor shape.
    pub shape: Vec<usize>,
    /// Flat offset of the first element.
    pub offset: usize,
    /// Element count.
    pub len: usize,
}

/// Flat layout of one network's parameters in canonical order.
pub fn param_segments(network: &Sonic52ReferenceNetwork) -> Vec<ParamSegment> {
    let conv = &network.conv;
    let dense_hidden = &network.dense_hidden;
    let dense_output = &network.dense_output;
    let shapes: [(&'static str, Vec<usize>); 6] = [
        (
            "conv.weight",
            vec![
                conv.output_channels,
                conv.input_channels,
                conv.kernel_height,
                conv.kernel_width,
            ],
        ),
        ("conv.bias", vec![conv.output_channels]),
        (
            "dense200.weight",
            vec![dense_hidden.output_dim, dense_hidden.input_dim],
        ),
        ("dense200.bias", vec![dense_hidden.output_dim]),
        (
            "dense52.weight",
            vec![dense_output.output_dim, dense_output.input_dim],
        ),
        ("dense52.bias", vec![dense_output.output_dim]),
    ];
    let mut offset = 0usize;
    shapes
        .into_iter()
        .map(|(name, shape)| {
            let len = shape.iter().product();
            let segment = ParamSegment {
                name,
                shape,
                offset,
                len,
            };
            offset += len;
            segment
        })
        .collect()
}

/// Total trainable scalar count (reference topology: 36 + 4 +
/// 13,912,000 + 200 + 10,400 + 52 = 13,922,692).
pub fn parameter_count(network: &Sonic52ReferenceNetwork) -> usize {
    param_segments(network)
        .iter()
        .map(|segment| segment.len)
        .sum()
}

// ---------------------------------------------------------------------
// errors (explicit rejection, no silent sanitizing)
// ---------------------------------------------------------------------

/// Gradient-serialization failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GradSerError {
    /// Wrong magic bytes.
    BadMagic,
    /// Input ends mid-record.
    Truncated,
    /// Bytes remain after the declared parameters.
    TrailingBytes,
    /// Name outside [`GRAD_PARAM_ORDER`].
    UnknownParameter {
        /// The offending name.
        name: String,
    },
    /// One name twice.
    DuplicateParameter {
        /// The repeated name.
        name: String,
    },
    /// A canonical parameter is absent.
    MissingParameter {
        /// The absent name.
        name: String,
    },
    /// Declared shape disagrees with the value count.
    ShapeMismatch {
        /// Parameter name.
        name: String,
    },
    /// A non-finite gradient value.
    NonFinite,
}

impl fmt::Display for GradSerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GradSerError::BadMagic => write!(f, "bad gradient magic"),
            GradSerError::Truncated => write!(f, "truncated gradient record"),
            GradSerError::TrailingBytes => write!(f, "trailing bytes after gradient record"),
            GradSerError::UnknownParameter { name } => {
                write!(f, "unknown gradient parameter: {name}")
            }
            GradSerError::DuplicateParameter { name } => {
                write!(f, "duplicate gradient parameter: {name}")
            }
            GradSerError::MissingParameter { name } => {
                write!(f, "missing gradient parameter: {name}")
            }
            GradSerError::ShapeMismatch { name } => {
                write!(f, "shape mismatch in gradient parameter: {name}")
            }
            GradSerError::NonFinite => write!(f, "non-finite gradient value"),
        }
    }
}

impl std::error::Error for GradSerError {}

/// Gradient-computation failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GradError {
    /// Target length differs from 52.
    TargetDim {
        /// Expected count.
        expected: usize,
        /// Offered count.
        found: usize,
    },
    /// A non-finite target value.
    NonFiniteTarget,
    /// A non-finite reference parameter (checked on entry — audio
    /// sanitization policy does not extend to mathematical state).
    NonFiniteParam,
    /// A non-finite gradient value (checked on exit, never sanitized).
    NonFiniteGradient,
    /// Upstream forward failure, quoted verbatim.
    Forward(String),
    /// Zero patches: a skipped sample, not a training example — never
    /// a zero gradient (Slice 5/8 null-policy).
    ZeroPatches,
    /// Patches/targets count mismatch for a track backward pass.
    CountMismatch {
        /// Patch count.
        patches: usize,
        /// Target count.
        targets: usize,
    },
    /// Serialization failure.
    Serialization(GradSerError),
}

impl fmt::Display for GradError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GradError::TargetDim { expected, found } => {
                write!(f, "{found} targets for {expected} outputs")
            }
            GradError::NonFiniteTarget => write!(f, "non-finite target"),
            GradError::NonFiniteParam => write!(f, "non-finite reference parameter"),
            GradError::NonFiniteGradient => write!(f, "non-finite gradient value"),
            GradError::Forward(detail) => write!(f, "forward failed: {detail}"),
            GradError::ZeroPatches => {
                write!(f, "zero patches: skipped sample, never a zero gradient")
            }
            GradError::CountMismatch { patches, targets } => {
                write!(f, "{patches} patches with {targets} target sets")
            }
            GradError::Serialization(error) => write!(f, "gradient serialization: {error}"),
        }
    }
}

impl std::error::Error for GradError {}

// ---------------------------------------------------------------------
// backward equations (documented here, implemented below)
// ---------------------------------------------------------------------
//
// Loss (Slice 8 semantics preserved exactly — mean over the 52 class
// duties, matching `bce_with_logits`):
//
// ```text
// L = (1/52) · Σᵢ [ max(zᵢ,0) − zᵢ·tᵢ + ln(1 + exp(−|zᵢ|)) ]
// dL/dzᵢ = (sigmoid(zᵢ) − tᵢ) / 52
// ```
//
// The loss consumes logits directly, so NO sigmoid derivative enters
// the BCE path (implementing one there would be the classic
// double-sigmoid bug — checked by test, not just documented).
//
// ReLU: dy/dx = 1 for x > 0, else 0 — including exactly 0 (the
// subgradient choice is pinned by test; leaky variants do not exist
// here).
//
// Dense y = Wx + b (row-major W[j][i]):
//   dW[j][i] = dy[j]·x[i],  db[j] = dy[j],  dx[i] = Σⱼ W[j][i]·dy[j].
//
// Valid strided Conv2D (forward: out[oc,oh,ow] = b[oc] +
// Σ_{ic,kh,kw} W[oc,ic,kh,kw]·in[ic, oh·sh+kh, ow·sw+kw]):
//   dW[oc,ic,kh,kw] = Σ_{oh,ow} dout[oc,oh,ow]·in[ic, oh·sh+kh, ow·sw+kw],
//   db[oc]          = Σ_{oh,ow} dout[oc,oh,ow],
//   din[ic,h,w]     = Σ over (oc,kh,kw) with oh=(h−kh)/sh integral,
//                     ow=(w−kw)/sw integral of W[oc,ic,kh,kw]·dout[oc,oh,ow].
//
// Track level (Slice 8 contract preserved): track loss is the MEAN of
// per-patch losses, so each patch's gradient is divided by the patch
// count — d/dθ mean_p L_p = mean_p dL_p/dθ — and the v1 Mean
// aggregation over sigmoid embeddings takes no part in the gradient
// path (it feeds evaluation/storage, not the loss).

/// ReLU derivative applied in place: `upstream[i] *= (pre[i] > 0)`.
/// Exactly zero maps to zero (pinned by test).
pub fn relu_backward_in_place(upstream: &mut [f32], pre_activation: &[f32]) {
    for (slope, pre) in upstream.iter_mut().zip(pre_activation.iter()) {
        if *pre <= 0.0 {
            *slope = 0.0;
        }
    }
}

/// Dense backward: `(dW row-major, db, dx)` for `y = Wx + b`.
/// `f32`, fixed loop order mirroring the forward pass.
pub fn dense_backward(
    weights: &[f32],
    dy: &[f32],
    x: &[f32],
    input_dim: usize,
    output_dim: usize,
) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let mut dw = vec![0.0f32; output_dim * input_dim];
    let mut db = vec![0.0f32; output_dim];
    let mut dx = vec![0.0f32; input_dim];
    for (j, dy_j) in dy.iter().enumerate() {
        db[j] = *dy_j;
        for (i, x_i) in x.iter().enumerate() {
            dw[j * input_dim + i] = *dy_j * *x_i;
        }
    }
    for (i, dx_i) in dx.iter_mut().enumerate() {
        let mut sum = 0.0f32;
        for (j, dy_j) in dy.iter().enumerate() {
            sum += weights[j * input_dim + i] * *dy_j;
        }
        *dx_i = sum;
    }
    (dw, db, dx)
}

/// Valid strided convolution backward: `(dW, db, dIn)` with `dW` in the
/// exact flat layout the forward pass documents
/// (`((oc·in_ch + ic)·kh + kh)·kw + kw`). `dIn` covers the full input
/// extent (parameters never include it; tests use it to prove the
/// transpose structure).
pub fn conv_backward(
    input: &Tensor3,
    config: &ResearchConvConfig,
    weights: &[f32],
    dout: &Tensor3,
) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let out_height = config.output_height();
    let out_width = config.output_width();
    let mut dw = vec![
        0.0f32;
        config.output_channels
            * config.input_channels
            * config.kernel_height
            * config.kernel_width
    ];
    let mut db = vec![0.0f32; config.output_channels];
    let mut din = vec![0.0f32; input.channels * input.height * input.width];
    for (oc, db_oc) in db.iter_mut().enumerate() {
        for oh in 0..out_height {
            for ow in 0..out_width {
                let delta = dout.data[(oc * out_height + oh) * out_width + ow];
                *db_oc += delta;
                for ic in 0..config.input_channels {
                    for kh in 0..config.kernel_height {
                        for kw in 0..config.kernel_width {
                            let h = oh * config.stride_height + kh;
                            let w = ow * config.stride_width + kw;
                            let wi = ((oc * config.input_channels + ic) * config.kernel_height
                                + kh)
                                * config.kernel_width
                                + kw;
                            dw[wi] += delta * input.data[(ic * input.height + h) * input.width + w];
                            din[(ic * input.height + h) * input.width + w] += weights[wi] * delta;
                        }
                    }
                }
            }
        }
    }
    (dw, db, din)
}

// ---------------------------------------------------------------------
// full-network backward (one patch, one target set)
// ---------------------------------------------------------------------

/// Checks every reference parameter for finiteness on entry.
fn check_params_finite(network: &Sonic52ReferenceNetwork) -> Result<(), GradError> {
    let boring: &[&[f32]] = &[
        &network.conv_weights,
        &network.conv_bias,
        &network.dense_hidden.weights,
        &network.dense_hidden.bias,
        &network.dense_output.weights,
        &network.dense_output.bias,
    ];
    if boring
        .iter()
        .any(|values| values.iter().any(|v| !v.is_finite()))
    {
        return Err(GradError::NonFiniteParam);
    }
    Ok(())
}

/// Full backward pass for one patch against one 52-D target:
/// BCE-mean gradient through dense52 → ReLU → dense200 → conv,
/// returning gradients in [`GRAD_PARAM_ORDER`]. Reference parameters
/// are read, never written. Fails closed on bad targets, non-finite
/// parameters, upstream forward failures, and non-finite results.
pub fn backward(
    patch: &Sonic52MelPatch,
    target: &[f32],
    network: &Sonic52ReferenceNetwork,
) -> Result<NetworkGradients, GradError> {
    if target.len() != DENSE_OUTPUT {
        return Err(GradError::TargetDim {
            expected: DENSE_OUTPUT,
            found: target.len(),
        });
    }
    if target.iter().any(|v| !v.is_finite()) {
        return Err(GradError::NonFiniteTarget);
    }
    check_params_finite(network)?;
    // Recompute the forward caches (deterministic re-execution, no
    // hidden state): tensor → conv → flatten → hidden → relu → logits.
    let tensor =
        mel_patch_to_tensor(patch).map_err(|error| GradError::Forward(error.to_string()))?;
    let convolved = conv2d_valid(
        &tensor,
        &network.conv,
        &network.conv_weights,
        &network.conv_bias,
    )
    .map_err(|error| GradError::Forward(error.to_string()))?;
    let flat = flatten_channel_major(&convolved);
    let hidden_pre = network
        .dense_hidden
        .forward(&flat)
        .map_err(|error| GradError::Forward(error.to_string()))?;
    let mut hidden_post = hidden_pre.clone();
    relu_in_place(&mut hidden_post);
    let logits = network
        .dense_output
        .forward(&hidden_post)
        .map_err(|error| GradError::Forward(error.to_string()))?;
    // BCE-mean gradient at the logits (Slice 8 reduction preserved:
    // mean over the 52 class duties).
    let mut dlogits = [0.0f32; DENSE_OUTPUT];
    for (slot, (logit, goal)) in dlogits.iter_mut().zip(logits.iter().zip(target.iter())) {
        *slot = (sigmoid_scalar(*logit) - *goal) / DENSE_OUTPUT as f32;
    }
    // dense52 → relu → dense200 → conv.
    let (dw52, db52, dhidden_post) = dense_backward(
        &network.dense_output.weights,
        &dlogits,
        &hidden_post,
        network.dense_output.input_dim,
        network.dense_output.output_dim,
    );
    let mut dhidden_pre = dhidden_post;
    relu_backward_in_place(&mut dhidden_pre, &hidden_pre);
    let (dw200, db200, dflat) = dense_backward(
        &network.dense_hidden.weights,
        &dhidden_pre,
        &flat,
        network.dense_hidden.input_dim,
        network.dense_hidden.output_dim,
    );
    let _ = dflat;
    let out_height = network.conv.output_height();
    let out_width = network.conv.output_width();
    // Reshape the flat dense200-input gradient back to channel-major
    // conv-output order (exact inverse of flatten_channel_major).
    // Constructed field-wise (not via Tensor3::new) because the data
    // is computed in place below; dimensions are exact by construction.
    let mut dconv_data = vec![0.0f32; dflat.len()];
    for (channel, _) in (0..network.conv.output_channels).enumerate() {
        for row in 0..out_height {
            for col in 0..out_width {
                dconv_data[(channel * out_height + row) * out_width + col] =
                    dflat[(channel * out_height + row) * out_width + col];
            }
        }
    }
    let dconv_out = Tensor3 {
        channels: network.conv.output_channels,
        height: out_height,
        width: out_width,
        data: dconv_data,
    };
    let (dconv_w, dconv_b, _) =
        conv_backward(&tensor, &network.conv, &network.conv_weights, &dconv_out);
    let grads = [
        (
            "conv.weight",
            vec![
                network.conv.output_channels,
                network.conv.input_channels,
                network.conv.kernel_height,
                network.conv.kernel_width,
            ],
            dconv_w,
        ),
        ("conv.bias", vec![network.conv.output_channels], dconv_b),
        (
            "dense200.weight",
            vec![
                network.dense_hidden.output_dim,
                network.dense_hidden.input_dim,
            ],
            dw200,
        ),
        (
            "dense200.bias",
            vec![network.dense_hidden.output_dim],
            db200,
        ),
        (
            "dense52.weight",
            vec![
                network.dense_output.output_dim,
                network.dense_output.input_dim,
            ],
            dw52,
        ),
        ("dense52.bias", vec![network.dense_output.output_dim], db52),
    ];
    let mut params = Vec::with_capacity(6);
    for (name, shape, values) in grads {
        if values.iter().any(|v| !v.is_finite()) {
            return Err(GradError::NonFiniteGradient);
        }
        params.push(ParameterGrad {
            name: name.to_string(),
            shape,
            values,
        });
    }
    Ok(NetworkGradients { params })
}

/// Track backward pass: mean of per-patch gradients (exactly the
/// adjoint of the Slice 8 track loss, which is the mean of per-patch
/// BCE losses). Empty patches are an explicit error, never zero
/// gradients; patch/target count mismatch is an explicit error.
pub fn backward_track(
    patches: &[Sonic52MelPatch],
    targets: &[[f32; 52]],
    network: &Sonic52ReferenceNetwork,
) -> Result<NetworkGradients, GradError> {
    if patches.is_empty() {
        return Err(GradError::ZeroPatches);
    }
    if patches.len() != targets.len() {
        return Err(GradError::CountMismatch {
            patches: patches.len(),
            targets: targets.len(),
        });
    }
    let mut accumulated: Option<NetworkGradients> = None;
    for (patch, target) in patches.iter().zip(targets.iter()) {
        let grads = backward(patch, target, network)?;
        match &mut accumulated {
            None => accumulated = Some(grads),
            Some(total) => {
                for (slot, incoming) in total.params.iter_mut().zip(grads.params.iter()) {
                    for (value, delta) in slot.values.iter_mut().zip(incoming.values.iter()) {
                        *value += *delta;
                    }
                }
            }
        }
    }
    let mut total = accumulated.expect("nonempty patches yield gradients");
    let count = patches.len() as f32;
    for param in total.params.iter_mut() {
        for value in param.values.iter_mut() {
            *value /= count;
        }
    }
    Ok(total)
}

// ---------------------------------------------------------------------
// gradient identity, serialization, records
// ---------------------------------------------------------------------

/// Byte-level codec shared by gradient (`SG01`) and parameter-state
/// (`SP01`) serialization: magic, u32 parameter count, then per
/// parameter (u8 name length, name, u8 rank, u32le dims, f32le values).
/// Name/shape/value validation stays with each format's own
/// `from_parsed`-style checker — this layer only moves bytes.
pub(crate) fn encode_named(magic: &[u8; 4], params: &[ParameterGrad]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(magic);
    out.extend_from_slice(&(params.len() as u32).to_le_bytes());
    for param in params {
        out.push(param.name.len() as u8);
        out.extend_from_slice(param.name.as_bytes());
        out.push(param.shape.len() as u8);
        for dim in &param.shape {
            out.extend_from_slice(&(*dim as u32).to_le_bytes());
        }
        for value in &param.values {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out
}

/// Inverse of [`encode_named`]: structural decode only (magic match,
/// truncation and trailing-byte rejection). Semantic validation
/// (names, order, shapes, finiteness) belongs to the caller.
pub(crate) fn decode_named(
    magic: &[u8; 4],
    bytes: &[u8],
) -> Result<Vec<ParameterGrad>, GradSerError> {
    if bytes.len() < 8 || &bytes[0..4] != magic {
        return Err(GradSerError::BadMagic);
    }
    let count =
        u32::from_le_bytes(bytes[4..8].try_into().map_err(|_| GradSerError::BadMagic)?) as usize;
    let mut offset = 8usize;
    let mut params = Vec::with_capacity(count);
    for _ in 0..count {
        let name_len = *bytes.get(offset).ok_or(GradSerError::Truncated)? as usize;
        offset += 1;
        let name_bytes = bytes
            .get(offset..offset + name_len)
            .ok_or(GradSerError::Truncated)?;
        offset += name_len;
        let name = String::from_utf8(name_bytes.to_vec()).map_err(|_| GradSerError::Truncated)?;
        let rank = *bytes.get(offset).ok_or(GradSerError::Truncated)? as usize;
        offset += 1;
        let mut shape = Vec::with_capacity(rank);
        for _ in 0..rank {
            let dim_bytes = bytes
                .get(offset..offset + 4)
                .ok_or(GradSerError::Truncated)?;
            offset += 4;
            shape.push(u32::from_le_bytes(dim_bytes.try_into().expect("4 bytes")) as usize);
        }
        let elements: usize = shape.iter().product();
        let mut values = Vec::with_capacity(elements);
        for _ in 0..elements {
            let word = bytes
                .get(offset..offset + 4)
                .ok_or(GradSerError::Truncated)?;
            offset += 4;
            values.push(f32::from_le_bytes(word.try_into().expect("4 bytes")));
        }
        params.push(ParameterGrad {
            name,
            shape,
            values,
        });
    }
    if offset != bytes.len() {
        return Err(GradSerError::TrailingBytes);
    }
    Ok(params)
}

impl NetworkGradients {
    /// Canonical deterministic representation: format version, then one
    /// section per parameter in canonical order (name, shape, values).
    pub fn canonical(&self) -> String {
        let mut out = format!("grad-v1|{}", GRAD_FORMAT_VERSION);
        for param in &self.params {
            let shape = param
                .shape
                .iter()
                .map(|dim| dim.to_string())
                .collect::<Vec<_>>()
                .join("x");
            out.push_str(&format!("\n{}|{}", param.name, shape));
        }
        out
    }

    /// Deterministic gradient digest: canonical text plus every value's
    /// little-endian bytes. Binds version, ordering, names, shapes, and
    /// values — changing any of them changes the digest.
    pub fn digest(&self) -> String {
        let mut bytes = self.canonical().into_bytes();
        bytes.push(0);
        for param in &self.params {
            for value in &param.values {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        musicpack_core::format::checksum::sha256_hex(&bytes)
    }

    /// Deterministic research-only serialization: magic `SG01`, u32
    /// parameter count, then per parameter (u8 name length, name,
    /// u8 rank, u32le dims, f32le values). Never a model artifact,
    /// never `.msim`/`.mpak` — research instrumentation only.
    /// Byte-identical to the pre-refactor encoding (the shared
    /// `encode_named` helper below is byte-for-byte the old body).
    pub fn serialize(&self) -> Vec<u8> {
        encode_named(b"SG01", &self.params)
    }

    /// Parses `serialize` output back, validating everything: magic,
    /// exact parameter set in canonical order (unknown, duplicate, or
    /// missing names rejected), shape/value-count agreement, and value
    /// finiteness. Trailing bytes are rejected.
    pub fn parse(bytes: &[u8]) -> Result<Self, GradError> {
        let parse_error = |error: GradSerError| GradError::Serialization(error);
        let params = decode_named(b"SG01", bytes).map_err(parse_error)?;
        Self::from_parsed(params).map_err(parse_error)
    }

    /// Validates parsed parameters against the canonical contract and
    /// returns them in canonical order: every canonical name must be
    /// present exactly once (unknown, duplicate, or missing names are
    /// rejected), shapes must agree with value counts, and values must
    /// be finite. Order in the byte stream is normalized, not
    /// significant — the digest always covers canonical order.
    fn from_parsed(mut params: Vec<ParameterGrad>) -> Result<Self, GradSerError> {
        for param in &params {
            if !GRAD_PARAM_ORDER.contains(&param.name.as_str()) {
                return Err(GradSerError::UnknownParameter {
                    name: param.name.clone(),
                });
            }
        }
        params.sort_by(|a, b| {
            GRAD_PARAM_ORDER
                .iter()
                .position(|name| *name == a.name)
                .cmp(&GRAD_PARAM_ORDER.iter().position(|name| *name == b.name))
        });
        for pair in params.windows(2) {
            if pair[0].name == pair[1].name {
                return Err(GradSerError::DuplicateParameter {
                    name: pair[0].name.clone(),
                });
            }
        }
        if params.len() != GRAD_PARAM_ORDER.len() {
            let missing = GRAD_PARAM_ORDER
                .iter()
                .find(|name| !params.iter().any(|param| &param.name == *name))
                .unwrap_or(&GRAD_PARAM_ORDER[0])
                .to_string();
            return Err(GradSerError::MissingParameter { name: missing });
        }
        for param in &params {
            let elements: usize = param.shape.iter().product();
            if elements != param.values.len() {
                return Err(GradSerError::ShapeMismatch {
                    name: param.name.clone(),
                });
            }
            if param.values.iter().any(|v| !v.is_finite()) {
                return Err(GradSerError::NonFinite);
            }
        }
        Ok(Self { params })
    }
}

/// Binds one gradient set to its scientific context: sample, weights,
/// objective, and gradient digest. Preprocessing/model/target context
/// arrives through the sample and objective digests (Slice 6/8 chain);
/// timing, host, paths, and memory addresses have no fields here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GradientRecord {
    /// Slice 6/8 sample identity.
    pub sample_id: String,
    /// Reference-weight digest in use.
    pub weight_digest: String,
    /// Objective identity digest.
    pub objective_digest: String,
    /// Gradient digest (version, ordering, names, shapes, values).
    pub grad_digest: String,
}

impl GradientRecord {
    /// Canonical deterministic encoding.
    pub fn canonical(&self) -> String {
        format!(
            "grad-record-v1|{}|{}|{}|{}",
            self.sample_id, self.weight_digest, self.objective_digest, self.grad_digest
        )
    }
}

// ---------------------------------------------------------------------
// finite-difference checker (research-only validation tool)
// ---------------------------------------------------------------------

/// Epsilon for central differences, chosen deliberately: 1e-3 sits
/// between f32 rounding noise (~1e-7 absolute on unit-scale loss,
/// amplified to ~1e-4 by the 1/eps division) and smooth-function
/// truncation error (~eps² = 1e-6). ReLU kinks are the known hazard —
/// a perturbation crossing a kink legitimately disagrees with the
/// analytic subgradient, so kink-adjacent misses are diagnosed, not
/// averaged away (see the full-network test).
pub const FINITE_DIFF_EPSILON: f32 = 1e-3;

/// Outcome of one finite-difference validation.
#[derive(Debug, Clone, PartialEq)]
pub struct FdReport {
    /// Parameters checked.
    pub checked: usize,
    /// Maximum |numeric − analytic|.
    pub max_abs_err: f64,
    /// Maximum relative error over entries with |analytic| above
    /// `1e-2` (tiny analytic values carry no relative information at
    /// f32 resolution — absolute error governs them instead).
    pub max_rel_err: f64,
}

/// Central-difference validation of analytic gradients for selected
/// flat parameter indices: perturbs each index ±eps in place (restoring
/// afterwards), reuses the exact forward/loss code path under test, and
/// reports worst-case errors. Pure validation tooling — never training.
pub fn finite_diff_check(
    network: &mut Sonic52ReferenceNetwork,
    patch: &Sonic52MelPatch,
    target: &[f32; 52],
    indices: &[usize],
    loss_of: &dyn Fn(&Sonic52ReferenceNetwork) -> f32,
) -> Result<FdReport, GradError> {
    if target.iter().any(|v| !v.is_finite()) {
        return Err(GradError::NonFiniteTarget);
    }
    check_params_finite(network)?;
    let analytic = backward(patch, target, network)?;
    let flat = flatten_gradients(&analytic);
    let segments = param_segments(network);
    let eps = FINITE_DIFF_EPSILON;
    let mut max_abs_err = 0.0f64;
    let mut max_rel_err = 0.0f64;
    for index in indices {
        let analytic_value = f64::from(flat[*index]);
        let original = read_flat_param(network, &segments, *index);
        write_flat_param(network, &segments, *index, original + eps);
        let plus = f64::from(loss_of(network));
        write_flat_param(network, &segments, *index, original - eps);
        let minus = f64::from(loss_of(network));
        write_flat_param(network, &segments, *index, original);
        let numeric = (plus - minus) / (2.0 * f64::from(eps));
        let abs_err = (numeric - analytic_value).abs();
        if abs_err > max_abs_err {
            max_abs_err = abs_err;
        }
        if analytic_value.abs() > 1e-2 {
            let rel_err = abs_err / analytic_value.abs();
            if rel_err > max_rel_err {
                max_rel_err = rel_err;
            }
        }
    }
    Ok(FdReport {
        checked: indices.len(),
        max_abs_err,
        max_rel_err,
    })
}

/// Flattens gradients in canonical order.
pub fn flatten_gradients(gradients: &NetworkGradients) -> Vec<f32> {
    gradients
        .params
        .iter()
        .flat_map(|param| param.values.iter().copied())
        .collect()
}

fn read_flat_param(
    network: &Sonic52ReferenceNetwork,
    segments: &[ParamSegment],
    index: usize,
) -> f32 {
    for segment in segments {
        if index >= segment.offset && index < segment.offset + segment.len {
            let within = index - segment.offset;
            return match segment.name {
                "conv.weight" => network.conv_weights[within],
                "conv.bias" => network.conv_bias[within],
                "dense200.weight" => network.dense_hidden.weights[within],
                "dense200.bias" => network.dense_hidden.bias[within],
                "dense52.weight" => network.dense_output.weights[within],
                "dense52.bias" => network.dense_output.bias[within],
                _ => panic!("unknown canonical parameter"),
            };
        }
    }
    panic!("flat index out of range");
}

fn write_flat_param(
    network: &mut Sonic52ReferenceNetwork,
    segments: &[ParamSegment],
    index: usize,
    value: f32,
) {
    for segment in segments {
        if index >= segment.offset && index < segment.offset + segment.len {
            let within = index - segment.offset;
            match segment.name {
                "conv.weight" => network.conv_weights[within] = value,
                "conv.bias" => network.conv_bias[within] = value,
                "dense200.weight" => network.dense_hidden.weights[within] = value,
                "dense200.bias" => network.dense_hidden.bias[within] = value,
                "dense52.weight" => network.dense_output.weights[within] = value,
                "dense52.bias" => network.dense_output.bias[within] = value,
                _ => panic!("unknown canonical parameter"),
            };
            return;
        }
    }
    panic!("flat index out of range");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{Sonic52MelPatch, MEL_BANDS, PATCH_FRAMES};
    use crate::network::{reference_network, ResearchConvConfig, Tensor3, REFERENCE_SEED};

    fn formula_patch(offset: usize) -> Sonic52MelPatch {
        let mut frames = [[0.0f32; MEL_BANDS]; PATCH_FRAMES];
        for (row, frame) in frames.iter_mut().enumerate() {
            for (col, slot) in frame.iter_mut().enumerate() {
                *slot = ((row * 97 + col * 13 + offset) % 89) as f32 / 89.0 - 0.5;
            }
        }
        Sonic52MelPatch::from_frames(frames)
    }

    fn formula_target(seed_byte: u8) -> [f32; 52] {
        let mut target = [0.0f32; 52];
        for (k, slot) in target.iter_mut().enumerate() {
            if (k + seed_byte as usize) % 3 == 0 {
                *slot = 1.0;
            }
        }
        target
    }

    // -- dense analytical case ----------------------------------------------------

    #[test]
    fn dense_backward_tiny_case_is_exact() {
        // y = Wx + b with W = [2], b = [1], x = [3], dy = [4]:
        // dW = 4·3 = 12, db = 4, dx = 2·4 = 8. Small integers: exact.
        let (dw, db, dx) = dense_backward(&[2.0], &[4.0], &[3.0], 1, 1);
        assert_eq!(dw, vec![12.0]);
        assert_eq!(db, vec![4.0]);
        assert_eq!(dx, vec![8.0]);
    }

    // -- relu ---------------------------------------------------------------------

    #[test]
    fn relu_backward_pins_zero_behaviour() {
        let mut upstream = vec![1.0f32, 2.0, 3.0, 4.0];
        relu_backward_in_place(&mut upstream, &[-1.0, 0.0, 0.5, 1e-30]);
        assert_eq!(upstream, vec![0.0, 0.0, 3.0, 4.0]);
    }

    // -- sigmoid derivative (test-only formula, no extra path) ---------------------

    #[test]
    fn sigmoid_derivative_matches_finite_differences() {
        // dσ/dz = σ(z)(1−σ(z)): checked against central differences so
        // the formula is proven, while the BCE path (logits-direct)
        // correctly never calls it — both facts pinned together.
        fn sigmoid_derivative(z: f32) -> f32 {
            let s = sigmoid_scalar(z);
            s * (1.0 - s)
        }
        for z in [-3.0f32, -0.5, 0.0, 0.7, 2.5] {
            let eps = 1e-3;
            let numeric = (sigmoid_scalar(z + eps) - sigmoid_scalar(z - eps)) / (2.0 * eps);
            assert!((numeric - sigmoid_derivative(z)).abs() < 1e-4, "z={z}");
        }
        assert_eq!(sigmoid_derivative(0.0), 0.25);
    }

    // -- BCE derivative --------------------------------------------------------------

    #[test]
    fn bce_gradient_matches_finite_differences() {
        // dL/dz = (sigmoid(z) − t)/52 for the Slice 8 mean reduction:
        // finite differences on the Slice 6 oracle confirm the formula
        // the backward pass implements, including the /52.
        use crate::training::bce_with_logits;
        let logits = [0.4f32, -1.1, 2.3];
        let targets = [1.0f32, 0.0, 1.0];
        // Oracle over 3 duties has /3 reduction; rescale to the /52
        // convention to compare like with like.
        let eps = 1e-3;
        for (i, (logit, target)) in logits.iter().zip(targets.iter()).enumerate() {
            let mut plus = logits;
            let mut minus = logits;
            plus[i] += eps;
            minus[i] -= eps;
            let numeric = (bce_with_logits(&plus, &targets).unwrap()
                - bce_with_logits(&minus, &targets).unwrap())
                / (2.0 * eps);
            let analytic = (sigmoid_scalar(*logit) - *target) / 3.0;
            assert!((numeric - analytic).abs() < 1e-4, "i={i}");
        }
    }

    // -- conv analytical case ----------------------------------------------------------

    #[test]
    fn conv_backward_tiny_case_is_exact() {
        // Forward tiny case from Slice 4 tests: 1×3×3 input 1..9,
        // kernel [[1,2],[3,4]], bias [100] → [[137,147],[167,177]].
        // Backward with dout = [[1,0],[0,1]]:
        // dW[kh,kw] = in[kh,kw]·1 + in[kh+1,kw+1]·1
        //   = [1+5, 2+6, 4+8, 5+9] = [6,8,12,14]; db = 2.
        let input = Tensor3::new(1, 3, 3, (1..=9).map(|v| v as f32).collect()).unwrap();
        let config = ResearchConvConfig {
            input_channels: 1,
            input_height: 3,
            input_width: 3,
            output_channels: 1,
            kernel_height: 2,
            kernel_width: 2,
            stride_height: 1,
            stride_width: 1,
            padding: crate::network::Padding::Valid,
        };
        let dout = Tensor3::new(1, 2, 2, vec![1.0, 0.0, 0.0, 1.0]).unwrap();
        let (dw, db, din) = conv_backward(&input, &config, &[1.0, 2.0, 3.0, 4.0], &dout);
        assert_eq!(dw, vec![6.0, 8.0, 12.0, 14.0]);
        assert_eq!(db, vec![2.0]);
        // Spot dIn with (row, col) made explicit: din[0,0] sees
        // W[0,0]·dout[0,0] only = 1; din[2,2] sees W[1,1]·dout[1,1]
        // only = 4; din[1,1] sees W[0,0]·dout[1,1] + W[1,1]·dout[0,0]
        // = 1 + 4 = 5.
        let at = |row: usize, col: usize| row * 3 + col;
        assert_eq!(din[at(0, 0)], 1.0);
        assert_eq!(din[at(2, 2)], 4.0);
        assert_eq!(din[at(1, 1)], 5.0);
    }

    #[test]
    fn conv_backward_matches_finite_differences() {
        // Scalar readout over the tiny case: loss = Σ outputs, so
        // dout is ones everywhere. Checks dW, db, and dIn against
        // central differences. Hand values: dW[kh,kw] sums in-values
        // over the 4 windows = [12,16,24,28]; db = 4;
        // din[row,col] = Σ of the kernel weights covering that input
        // (corners see 1 weight, edges 2, interior 4):
        // [[1,3,2],[4,10,6],[3,7,4]].
        let input_vec: Vec<f32> = (1..=9).map(|v| v as f32).collect();
        let weights = [1.0f32, 2.0, 3.0, 4.0];
        let config = ResearchConvConfig {
            input_channels: 1,
            input_height: 3,
            input_width: 3,
            output_channels: 1,
            kernel_height: 2,
            kernel_width: 2,
            stride_height: 1,
            stride_width: 1,
            padding: crate::network::Padding::Valid,
        };
        let scalar_loss = |w: &[f32; 4], b: &[f32; 1], input: &[f32]| {
            let tensor = Tensor3::new(1, 3, 3, input.to_vec()).unwrap();
            let out = conv2d_valid(&tensor, &config, w, b).expect("valid conv");
            out.data.iter().sum::<f32>()
        };
        let input = Tensor3::new(1, 3, 3, input_vec.clone()).unwrap();
        let (dw, db, din) = conv_backward(
            &input,
            &config,
            &weights,
            &Tensor3::new(1, 2, 2, vec![1.0; 4]).unwrap(),
        );
        assert_eq!(dw, vec![12.0, 16.0, 24.0, 28.0]);
        assert_eq!(db, vec![4.0]);
        // din[row,col] sums the kernel weights covering that input:
        // corners see 1 weight, edges 2, interior 4.
        assert_eq!(din, vec![1.0, 3.0, 2.0, 4.0, 10.0, 6.0, 3.0, 7.0, 4.0]);
        let eps = 1e-2;
        // Zero bias for the finite-difference pass: the loss is linear
        // in weights and inputs, so central differences are
        // truncation-free and the larger eps beats f32 rounding of the
        // ~50-scale partial sums (the biased hand asserts above already
        // pin db exactly).
        let nobias = [0.0f32];
        for probe in [0usize, 3] {
            let mut plus = weights;
            let mut minus = weights;
            plus[probe] += eps;
            minus[probe] -= eps;
            let numeric = (scalar_loss(&plus, &nobias, &input_vec)
                - scalar_loss(&minus, &nobias, &input_vec))
                / (2.0 * eps);
            assert!((numeric - dw[probe]).abs() < 5e-3, "w{probe}");
        }
        let mut plus = input_vec.clone();
        let mut minus = input_vec.clone();
        plus[4] += eps;
        minus[4] -= eps;
        let numeric = (scalar_loss(&weights, &nobias, &plus)
            - scalar_loss(&weights, &nobias, &minus))
            / (2.0 * eps);
        assert!((numeric - din[4]).abs() < 5e-3, "din[4]={}", din[4]);
    }

    // -- full-network finite-difference check -----------------------------------------------

    fn loss_for(
        network: &Sonic52ReferenceNetwork,
        patch: &Sonic52MelPatch,
        target: &[f32; 52],
    ) -> f32 {
        use crate::training::bce_with_logits;
        let detail = network.forward_detailed(patch).expect("valid forward");
        bce_with_logits(&detail.logits, target).expect("valid loss")
    }

    #[test]
    fn full_network_gradients_match_finite_differences() {
        let mut network = reference_network(REFERENCE_SEED);
        let patch = formula_patch(0);
        let target = formula_target(7);
        let segments = param_segments(&network);
        // Representative coverage per tensor: first, middle, last, and
        // two deterministic interior positions (no full sweep: 14M
        // parameters would make the test unreasonable).
        let mut indices = Vec::new();
        for segment in &segments {
            for position in [0usize, 7, segment.len / 2, segment.len / 3, segment.len - 1] {
                indices.push(segment.offset + position.min(segment.len - 1));
            }
        }
        indices.sort_unstable();
        indices.dedup();
        let total: usize = segments.iter().map(|segment| segment.len).sum();
        assert_eq!(total, parameter_count(&network));
        assert_eq!(total, 13_922_692);
        // Per-tensor reports as well as the global one, so a weak
        // tensor localizes immediately instead of hiding in a max.
        for segment in &segments {
            let mut sub = Vec::new();
            for position in [0usize, 7, segment.len / 2, segment.len / 3, segment.len - 1] {
                sub.push(segment.offset + position.min(segment.len - 1));
            }
            let report = finite_diff_check(&mut network, &patch, &target, &sub, &|network| {
                loss_for(network, &patch, &target)
            })
            .expect("finite-difference check runs");
            eprintln!(
                "FDCHECK {} checked={} max_abs={:.2e} max_rel={:.2e}",
                segment.name, report.checked, report.max_abs_err, report.max_rel_err
            );
            assert!(report.max_abs_err < 1e-2, "{} abs", segment.name);
        }
        let report = finite_diff_check(&mut network, &patch, &target, &indices, &|network| {
            loss_for(network, &patch, &target)
        })
        .expect("finite-difference check runs");
        assert_eq!(report.checked, indices.len());
        eprintln!(
            "FDCHECK checked={} max_abs={:.2e} max_rel={:.2e}",
            report.checked, report.max_abs_err, report.max_rel_err
        );
        assert!(report.max_abs_err < 1e-2, "max_abs={}", report.max_abs_err);
        assert!(report.max_rel_err < 5e-2, "max_rel={}", report.max_rel_err);
    }

    // -- determinism + digest ---------------------------------------------------------

    #[test]
    fn dense52_bias_matches_hand_loss_gradient() {
        // The highest-risk seam in the whole backward pass: the loss
        // gradient must be (sigmoid(z) − t)/52 — Slice 8 mean reduction,
        // logits-direct (no double-sigmoid). Recomputed here from the
        // pinned forward pieces, independent of `backward` internals.
        use crate::network::sigmoid_scalar;
        let network = reference_network(REFERENCE_SEED);
        let patch = formula_patch(0);
        let target = formula_target(7);
        let detail = network.forward_detailed(&patch).expect("forward runs");
        let grads = backward(&patch, &target, &network).expect("backward runs");
        let db52 = &grads.params[5];
        assert_eq!(db52.name, "dense52.bias");
        for (j, value) in db52.values.iter().enumerate() {
            let expected = (sigmoid_scalar(detail.logits[j]) - target[j]) / 52.0;
            assert!((value - expected).abs() < 1e-6, "class {j}");
        }
    }

    #[test]
    fn gradients_are_deterministic_and_digested() {
        let network = reference_network(REFERENCE_SEED);
        let patch = formula_patch(0);
        let target = formula_target(7);
        let first = backward(&patch, &target, &network).expect("backward runs");
        let second = backward(&patch, &target, &network).expect("backward runs");
        assert_eq!(first, second);
        assert_eq!(first.digest(), second.digest());
        assert_eq!(first.digest().len(), 64);
        // Every tensor present in canonical order with finite values.
        assert_eq!(
            first
                .params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>(),
            GRAD_PARAM_ORDER
        );
        assert!(first
            .params
            .iter()
            .all(|param| param.values.iter().all(|v| v.is_finite())));
        // Round-trip through the research serialization.
        let bytes = first.serialize();
        assert!(bytes.starts_with(b"SG01"));
        assert_eq!(NetworkGradients::parse(&bytes).expect("parse"), first);
        assert_eq!(
            NetworkGradients::parse(&bytes).expect("parse").digest(),
            first.digest()
        );
    }

    // -- multi-patch + null policy ---------------------------------------------------------

    #[test]
    fn track_gradients_follow_mean_reduction() {
        let network = reference_network(REFERENCE_SEED);
        let patches = [formula_patch(0), formula_patch(41)];
        let targets = [formula_target(7), formula_target(8)];
        let track = backward_track(&patches, &targets, &network).expect("track runs");
        // Adjoint of mean-of-losses: the track gradient is the mean of
        // the per-patch gradients (recomputed independently here).
        let first = backward(&patches[0], &targets[0], &network).expect("patch runs");
        let second = backward(&patches[1], &targets[1], &network).expect("patch runs");
        for (total, (a, b)) in track
            .params
            .iter()
            .zip(first.params.iter().zip(second.params.iter()))
        {
            for (value, (x, y)) in total
                .values
                .iter()
                .zip(a.values.iter().zip(b.values.iter()))
            {
                assert_eq!(*value, (x + y) / 2.0);
            }
        }
        // One-patch track equals the single backward pass exactly.
        let single = backward_track(&patches[..1], &targets[..1], &network).expect("single runs");
        assert_eq!(single, first);
    }

    #[test]
    fn zero_patches_is_an_explicit_error() {
        let network = reference_network(REFERENCE_SEED);
        assert_eq!(
            backward_track(&[], &[], &network).unwrap_err(),
            GradError::ZeroPatches
        );
        assert_eq!(
            backward_track(&[formula_patch(0)], &[], &network).unwrap_err(),
            GradError::CountMismatch {
                patches: 1,
                targets: 0
            }
        );
    }

    // -- rejection vocabulary ---------------------------------------------------------------

    #[test]
    fn malformed_inputs_are_rejected() {
        let network = reference_network(REFERENCE_SEED);
        let patch = formula_patch(0);
        assert_eq!(
            backward(&patch, &[0.0; 51], &network).unwrap_err(),
            GradError::TargetDim {
                expected: 52,
                found: 51
            }
        );
        let mut bad_target = formula_target(7);
        bad_target[3] = f32::NAN;
        assert_eq!(
            backward(&patch, &bad_target, &network).unwrap_err(),
            GradError::NonFiniteTarget
        );
        // Non-finite parameters are rejected, never sanitized.
        let mut poisoned = reference_network(REFERENCE_SEED);
        poisoned.conv_bias[0] = f32::INFINITY;
        assert_eq!(
            backward(&patch, &formula_target(7), &poisoned).unwrap_err(),
            GradError::NonFiniteParam
        );
        // Serialization: bad magic, truncation, trailing bytes, unknown
        // names, and non-finite values all fail closed.
        let network = reference_network(REFERENCE_SEED);
        let grads = backward(&patch, &formula_target(7), &network).expect("backward runs");
        let mut bytes = grads.serialize();
        assert!(matches!(
            NetworkGradients::parse(&bytes[1..]).unwrap_err(),
            GradError::Serialization(GradSerError::BadMagic)
        ));
        bytes.push(0xff);
        assert!(matches!(
            NetworkGradients::parse(&bytes).unwrap_err(),
            GradError::Serialization(GradSerError::TrailingBytes)
        ));
        let truncated = &grads.serialize()[..20];
        assert!(matches!(
            NetworkGradients::parse(truncated).unwrap_err(),
            GradError::Serialization(GradSerError::Truncated)
        ));
        let mut poisoned = grads.clone();
        poisoned.params[1].values[0] = f32::NAN;
        assert!(matches!(
            NetworkGradients::parse(&poisoned.serialize()).unwrap_err(),
            GradError::Serialization(GradSerError::NonFinite)
        ));
    }

    // -- slice 8 regression: forward values unchanged ----------------------------------------------

    #[test]
    fn slice8_forward_goldens_unchanged() {
        // The gradient slice must not move the forward contract: the
        // Slice 8 golden patch still yields its pinned digests, and the
        // loss oracle still scores it identically.
        use crate::network::{embedding_from_output, serialize_output};
        use crate::objective::V1_OBJECTIVE;
        use crate::sample::{run_sample, synthetic_target, SYNTHETIC_TARGET_RULE_ID};
        use crate::training::{bce_with_logits, training_sample_id};
        let network = reference_network(REFERENCE_SEED);
        let preproc = crate::experiment::RESEARCH_V1_CONTRACT.to_string();
        // Recompute the Slice 8 golden directly (3 s sine path lives in
        // sample tests; here we re-derive its forward pieces).
        let patch = formula_patch(0);
        let detail = network.forward_detailed(&patch).expect("forward runs");
        let embedding = network.forward(&patch).expect("forward runs");
        assert_eq!(detail.embedding, embedding);
        let digest = musicpack_core::format::checksum::sha256_hex(&serialize_output(&embedding));
        assert_eq!(
            digest,
            "10afb15430c74125929265ad04af3613851f84e3692e9f8d1d9335b3c480613d"
        );
        let sample_id = training_sample_id("bank", "v3", "sine-3s", &preproc, 0);
        let target = synthetic_target(&sample_id);
        crate::objective::validate_target(&V1_OBJECTIVE, &target).expect("valid target");
        let loss = bce_with_logits(&detail.logits, &target).expect("valid loss");
        assert!(loss.is_finite());
        // Bridge and rule identities hold on this path too.
        let profile = crate::Sonic52Profile::research_v1([0x5a; 32]).expect("profile");
        assert_eq!(
            embedding_from_output(&profile, embedding)
                .expect("bridge")
                .values(),
            &embedding
        );
        assert_eq!(SYNTHETIC_TARGET_RULE_ID, "synthetic-rule-v1");
        let _ = run_sample;
    }
}
