//! Sonic52 Slice 10: deterministic SGD update (one step, no loop).
//!
//! The smallest possible optimizer layer over the trusted Slice 9
//! chain: plain stochastic gradient descent applied exactly once.
//!
//! ```text
//! (parameters, gradients, learning_rate) → updated_parameters
//! ```
//!
//! Update equation (the whole optimizer — nothing else exists here):
//!
//! ```text
//! updated[i] = parameter[i] − learning_rate × gradient[i]
//! ```
//!
//! f32 arithmetic, fixed canonical tensor/element order, functional
//! semantics (the input state is never mutated; a new state is
//! produced). Research choice throughout — plain SGD is the first,
//! most boring optimizer that could work, not evidence of anyone
//! else's training procedure. No momentum, schedules, batches,
//! epochs, checkpoints, or learned-weight claims: Slice 10 ends at
//! one deterministic updated parameter state.

#![forbid(unsafe_code)]

use std::fmt;

use crate::gradient::{
    decode_named, encode_named, param_segments, GradSerError, NetworkGradients, GRAD_PARAM_ORDER,
};
use crate::network::Sonic52ReferenceNetwork;

// ---------------------------------------------------------------------
// optimizer identity + learning rate
// ---------------------------------------------------------------------

/// Research contract id of the plain-SGD optimizer. Displayed in every
/// update record; never a production id, never evidence of Plex's
/// optimizer (which is unknown).
pub const OPTIMIZER_ID: &str = "sonic52-optimizer-sgd-v1";

/// Serialization magic of the research parameter-state format.
pub const STATE_MAGIC: &[u8; 4] = b"SP01";

/// What is wrong with a learning rate or an update's inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    /// Learning rate is NaN, infinite, zero, or negative. Zero is
    /// rejected deliberately: an lr=0 step would mint a distinct update
    /// identity for a byte-identical state, breaking the "different
    /// identity ⟹ different result" hygiene the identity exists for.
    BadLearningRate,
    /// A parameter tensor fails validation (see [`ParameterState`]).
    BadParameter {
        /// Human-readable cause.
        detail: String,
    },
    /// State and gradient tensor sets do not correspond exactly.
    StateGradientMismatch {
        /// Human-readable cause.
        detail: String,
    },
    /// A non-finite parameter value (never sanitized).
    NonFiniteParam,
    /// A non-finite gradient value (never sanitized).
    NonFiniteGradient,
    /// State serialization failure.
    Serialization(GradSerError),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpdateError::BadLearningRate => {
                write!(f, "learning rate must be finite and positive")
            }
            UpdateError::BadParameter { detail } => {
                write!(f, "bad parameter state: {detail}")
            }
            UpdateError::StateGradientMismatch { detail } => {
                write!(f, "state/gradient mismatch: {detail}")
            }
            UpdateError::NonFiniteParam => write!(f, "non-finite parameter value"),
            UpdateError::NonFiniteGradient => write!(f, "non-finite gradient value"),
            UpdateError::Serialization(error) => {
                write!(f, "parameter-state serialization: {error}")
            }
        }
    }
}

impl std::error::Error for UpdateError {}

/// Positive finite f32 learning rate. Construction is the only gate:
/// NaN, ±Inf, zero, and negatives are rejected, never clamped (a
/// clamped rate would silently change the experiment being recorded).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LearningRate(f32);

impl LearningRate {
    /// Validates and wraps a learning rate.
    pub fn new(value: f32) -> Result<Self, UpdateError> {
        if !value.is_finite() || value <= 0.0 {
            return Err(UpdateError::BadLearningRate);
        }
        Ok(Self(value))
    }

    /// The wrapped value.
    pub fn value(self) -> f32 {
        self.0
    }

    /// Raw bits, the deterministic learning-rate identity: changing
    /// the rate changes these bits, which changes the update identity.
    pub fn bits(self) -> u32 {
        self.0.to_bits()
    }
}

// ---------------------------------------------------------------------
// parameter state (the six tensors, canonical order, validated)
// ---------------------------------------------------------------------

/// One named parameter tensor: name, shape, and values in the
/// row-major layout the forward pass documents for that tensor.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedTensor {
    /// Canonical parameter name (see [`GRAD_PARAM_ORDER`]).
    pub name: String,
    /// Tensor shape.
    pub shape: Vec<usize>,
    /// Values in the documented layout.
    pub values: Vec<f32>,
}

/// Six validated parameter tensors in canonical order. Shapes are
/// validated for internal consistency (shape product vs value count)
/// and the set is validated for exact canonical membership; binding to
/// a concrete architecture happens in [`ParameterState::from_network`]
/// and correspondence to gradients in [`sgd_update`].
#[derive(Debug, Clone, PartialEq)]
pub struct ParameterState {
    /// Six tensors in [`GRAD_PARAM_ORDER`].
    pub tensors: Vec<NamedTensor>,
}

impl ParameterState {
    /// Builds the state for one reference network: exact shapes from
    /// [`param_segments`], exact values cloned, all validated finite.
    pub fn from_network(network: &Sonic52ReferenceNetwork) -> Result<Self, UpdateError> {
        let mut tensors = Vec::with_capacity(GRAD_PARAM_ORDER.len());
        for segment in param_segments(network) {
            let values = match segment.name {
                "conv.weight" => network.conv_weights.clone(),
                "conv.bias" => network.conv_bias.clone(),
                "dense200.weight" => network.dense_hidden.weights.clone(),
                "dense200.bias" => network.dense_hidden.bias.clone(),
                "dense52.weight" => network.dense_output.weights.clone(),
                "dense52.bias" => network.dense_output.bias.clone(),
                _ => {
                    return Err(UpdateError::BadParameter {
                        detail: format!("unknown canonical parameter: {}", segment.name),
                    });
                }
            };
            if values.len() != segment.len {
                return Err(UpdateError::BadParameter {
                    detail: format!(
                        "{} has {} values, expected {}",
                        segment.name,
                        values.len(),
                        segment.len
                    ),
                });
            }
            tensors.push(NamedTensor {
                name: segment.name.to_string(),
                shape: segment.shape,
                values,
            });
        }
        let state = Self { tensors };
        state.validate()?;
        Ok(state)
    }

    /// Validates a hand-built or parsed state: exactly the six
    /// canonical names in order, no duplicates, shape/value-count
    /// agreement, all values finite.
    pub fn build(tensors: Vec<NamedTensor>) -> Result<Self, UpdateError> {
        let state = Self { tensors };
        state.validate()?;
        Ok(state)
    }

    fn validate(&self) -> Result<(), UpdateError> {
        if self.tensors.len() != GRAD_PARAM_ORDER.len() {
            return Err(UpdateError::BadParameter {
                detail: format!(
                    "{} tensors, expected {}",
                    self.tensors.len(),
                    GRAD_PARAM_ORDER.len()
                ),
            });
        }
        for (tensor, expected) in self.tensors.iter().zip(GRAD_PARAM_ORDER.iter()) {
            if tensor.name.as_str() != *expected {
                return Err(UpdateError::BadParameter {
                    detail: format!("expected parameter {expected}, found {}", tensor.name),
                });
            }
            let elements: usize = tensor.shape.iter().product();
            if elements != tensor.values.len() {
                return Err(UpdateError::BadParameter {
                    detail: format!(
                        "{} has {} values for shape product {elements}",
                        tensor.name,
                        tensor.values.len()
                    ),
                });
            }
            if tensor.values.iter().any(|v| !v.is_finite()) {
                return Err(UpdateError::NonFiniteParam);
            }
        }
        Ok(())
    }

    /// Total scalar count (13,922,692 for the reference topology).
    pub fn parameter_count(&self) -> usize {
        self.tensors.iter().map(|tensor| tensor.values.len()).sum()
    }

    /// Canonical deterministic representation (digested, not displayed).
    pub fn canonical(&self) -> String {
        let mut out = format!("state-v1|{}", OPTIMIZER_ID);
        for tensor in &self.tensors {
            let shape = tensor
                .shape
                .iter()
                .map(|dim| dim.to_string())
                .collect::<Vec<_>>()
                .join("x");
            out.push_str(&format!("\n{}|{}", tensor.name, shape));
        }
        out
    }

    /// Deterministic state digest: canonical text plus every value's
    /// little-endian bytes.
    pub fn digest(&self) -> String {
        let mut bytes = self.canonical().into_bytes();
        bytes.push(0);
        for tensor in &self.tensors {
            for value in &tensor.values {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        musicpack_core::format::checksum::sha256_hex(&bytes)
    }

    /// Research-only serialization reusing the Slice 9 byte codec with
    /// the `SP01` magic: same record layout, distinct magic so state
    /// bytes can never parse as gradients or vice versa. Not a
    /// checkpoint system — parameter-state serialization only.
    pub fn serialize(&self) -> Vec<u8> {
        encode_named(
            STATE_MAGIC,
            &self
                .tensors
                .iter()
                .map(|tensor| crate::gradient::ParameterGrad {
                    name: tensor.name.clone(),
                    shape: tensor.shape.clone(),
                    values: tensor.values.clone(),
                })
                .collect::<Vec<_>>(),
        )
    }

    /// Parses `serialize` output back through the shared codec, then
    /// applies full state validation (canonical set, order, shapes,
    /// finiteness). Rejects truncated/trailing/unknown/duplicate/
    /// missing/misshapen/non-finite input.
    pub fn parse(bytes: &[u8]) -> Result<Self, UpdateError> {
        let params = decode_named(STATE_MAGIC, bytes).map_err(UpdateError::Serialization)?;
        let mut tensors = Vec::with_capacity(params.len());
        for param in params {
            tensors.push(NamedTensor {
                name: param.name,
                shape: param.shape,
                values: param.values,
            });
        }
        // Order normalization matches the gradient format: the byte
        // stream order is not significant, the canonical digest is.
        tensors.sort_by(|a, b| {
            GRAD_PARAM_ORDER
                .iter()
                .position(|name| *name == a.name)
                .cmp(&GRAD_PARAM_ORDER.iter().position(|name| *name == b.name))
        });
        Self::build(tensors)
    }
}

// ---------------------------------------------------------------------
// one SGD step (pure: new state, inputs untouched)
// ---------------------------------------------------------------------

/// Plain SGD update over the validated correspondence of `state` and
/// `gradients`: `updated[i] = parameter[i] − lr × gradient[i]` in
/// fixed canonical tensor/element order, f32. Returns a new state;
/// both inputs are untouched (functional semantics — the "original
/// unchanged" test pins this).
pub fn sgd_update(
    state: &ParameterState,
    gradients: &NetworkGradients,
    learning_rate: LearningRate,
) -> Result<ParameterState, UpdateError> {
    state.validate()?;
    if gradients.params.len() != state.tensors.len() {
        return Err(UpdateError::StateGradientMismatch {
            detail: format!(
                "{} gradient tensors for {} state tensors",
                gradients.params.len(),
                state.tensors.len()
            ),
        });
    }
    let lr = learning_rate.value();
    let mut tensors = Vec::with_capacity(state.tensors.len());
    for (tensor, grad) in state.tensors.iter().zip(gradients.params.iter()) {
        if grad.name != tensor.name {
            return Err(UpdateError::StateGradientMismatch {
                detail: format!(
                    "gradient {} does not match state {}",
                    grad.name, tensor.name
                ),
            });
        }
        if grad.shape != tensor.shape || grad.values.len() != tensor.values.len() {
            return Err(UpdateError::StateGradientMismatch {
                detail: format!("shape mismatch on {}", tensor.name),
            });
        }
        if grad.values.iter().any(|v| !v.is_finite()) {
            return Err(UpdateError::NonFiniteGradient);
        }
        let mut values = Vec::with_capacity(tensor.values.len());
        for (parameter, gradient) in tensor.values.iter().zip(grad.values.iter()) {
            values.push(*parameter - lr * *gradient);
        }
        tensors.push(NamedTensor {
            name: tensor.name.clone(),
            shape: tensor.shape.clone(),
            values,
        });
    }
    ParameterState::build(tensors)
}

// ---------------------------------------------------------------------
// update record: which state + gradient made which state
// ---------------------------------------------------------------------

/// Provenance of one update step: initial state, gradient, optimizer,
/// learning rate, and result — each as a digest or pinned id, so the
/// exact inputs of any updated state stay determinable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateRecord {
    /// Optimizer contract id.
    pub optimizer_id: String,
    /// Digest of the initial parameter state.
    pub initial_digest: String,
    /// Digest of the applied gradients.
    pub gradient_digest: String,
    /// Learning rate as 8-hex-digit bits (deterministic representation).
    pub learning_rate_bits: String,
    /// Digest of the updated parameter state.
    pub updated_digest: String,
}

impl UpdateRecord {
    /// Builds the record from the three validated inputs plus both
    /// digests' sources (no re-validation: callers pass what
    /// `sgd_update` consumed and produced).
    pub fn new(
        initial: &ParameterState,
        gradients: &NetworkGradients,
        learning_rate: LearningRate,
        updated: &ParameterState,
    ) -> Self {
        Self {
            optimizer_id: OPTIMIZER_ID.to_string(),
            initial_digest: initial.digest(),
            gradient_digest: gradients.digest(),
            learning_rate_bits: format!("{:08x}", learning_rate.bits()),
            updated_digest: updated.digest(),
        }
    }

    /// Canonical deterministic encoding.
    pub fn canonical(&self) -> String {
        format!(
            "update-v1|{}|{}|{}|{}|{}",
            self.optimizer_id,
            self.initial_digest,
            self.gradient_digest,
            self.learning_rate_bits,
            self.updated_digest
        )
    }

    /// Deterministic update identity.
    pub fn digest(&self) -> String {
        musicpack_core::format::checksum::sha256_hex(self.canonical().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{Sonic52MelPatch, MEL_BANDS, PATCH_FRAMES};
    use crate::gradient::backward;
    use crate::network::{reference_network, REFERENCE_SEED};

    fn tiny_state_and_grads() -> (ParameterState, NetworkGradients) {
        // Six canonical tensors in miniature (shapes arbitrary but
        // consistent): mixed positive, negative, and zero gradients.
        let shapes: &[&[usize]] = &[&[2], &[1], &[2, 2], &[2], &[1, 2], &[1]];
        let state_values: &[&[f32]] = &[
            &[2.0, 2.0],
            &[0.5],
            &[1.0, 2.0, 3.0, 4.0],
            &[7.0, 8.0],
            &[10.0, -10.0],
            &[3.0],
        ];
        let grad_values: &[&[f32]] = &[
            &[0.5, -0.5],
            &[0.0],
            &[1.0, -1.0, 0.25, 0.0],
            &[0.0, 0.0],
            &[-4.0, 4.0],
            &[0.0],
        ];
        let tensors = GRAD_PARAM_ORDER
            .iter()
            .zip(shapes.iter())
            .zip(state_values.iter())
            .map(|((name, shape), values)| NamedTensor {
                name: name.to_string(),
                shape: shape.to_vec(),
                values: values.to_vec(),
            })
            .collect();
        let grads = GRAD_PARAM_ORDER
            .iter()
            .zip(shapes.iter())
            .zip(grad_values.iter())
            .map(|((name, shape), values)| crate::gradient::ParameterGrad {
                name: name.to_string(),
                shape: shape.to_vec(),
                values: values.to_vec(),
            })
            .collect();
        (
            ParameterState::build(tensors).expect("valid tiny state"),
            NetworkGradients { params: grads },
        )
    }

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

    // -- scalar and tiny-tensor mathematics ----------------------------------------

    #[test]
    fn sgd_scalar_goldens() {
        // p = 2.0, g = 0.5, lr = 0.1 → 1.95 (exact in binary32:
        // 0.05 is not exact, but 2 − 0.1·0.5 is — verified below
        // against an f64 reference, not assumed).
        let (state, grads) = tiny_state_and_grads();
        let updated = sgd_update(&state, &grads, LearningRate::new(0.1).unwrap()).unwrap();
        let expected = 2.0f64 - 0.1 * 0.5;
        assert!((updated.tensors[0].values[0] as f64 - expected).abs() < 1e-7);
        // Negative gradient ascends: p = 2.0, g = −0.5 → 2.05.
        assert!((updated.tensors[0].values[1] as f64 - 2.05).abs() < 1e-7);
        // Zero gradient leaves the parameter bit-identical.
        assert_eq!(updated.tensors[1].values, vec![0.5]);
        assert_eq!(updated.tensors[3].values, vec![7.0, 8.0]);
        assert_eq!(updated.tensors[5].values, vec![3.0]);
    }

    #[test]
    fn tiny_update_matches_independent_reference() {
        // Independent reference: f64 arithmetic over a flat zipped
        // layout (different loop structure from the implementation's
        // per-tensor loops), compared element-wise.
        let (state, grads) = tiny_state_and_grads();
        let lr = LearningRate::new(0.25).unwrap();
        let updated = sgd_update(&state, &grads, lr).unwrap();
        let flat_params: Vec<f64> = state
            .tensors
            .iter()
            .flat_map(|tensor| tensor.values.iter().map(|v| f64::from(*v)))
            .collect();
        let flat_grads: Vec<f64> = grads
            .params
            .iter()
            .flat_map(|param| param.values.iter().map(|v| f64::from(*v)))
            .collect();
        let flat_updated: Vec<f32> = updated
            .tensors
            .iter()
            .flat_map(|tensor| tensor.values.iter().copied())
            .collect();
        assert_eq!(flat_params.len(), flat_grads.len());
        assert_eq!(flat_params.len(), flat_updated.len());
        for (index, value) in flat_updated.iter().enumerate() {
            let expected = (flat_params[index] - 0.25 * flat_grads[index]) as f32;
            assert_eq!(*value, expected, "element {index}");
        }
    }

    // -- learning-rate contract -------------------------------------------------------

    #[test]
    fn learning_rate_rejects_non_positive() {
        assert!(LearningRate::new(0.1).is_ok());
        assert!(LearningRate::new(f32::MIN_POSITIVE).is_ok());
        assert_eq!(
            LearningRate::new(0.0).unwrap_err(),
            UpdateError::BadLearningRate
        );
        assert_eq!(
            LearningRate::new(-0.1).unwrap_err(),
            UpdateError::BadLearningRate
        );
        assert_eq!(
            LearningRate::new(f32::NAN).unwrap_err(),
            UpdateError::BadLearningRate
        );
        assert_eq!(
            LearningRate::new(f32::INFINITY).unwrap_err(),
            UpdateError::BadLearningRate
        );
        assert_eq!(
            LearningRate::new(f32::NEG_INFINITY).unwrap_err(),
            UpdateError::BadLearningRate
        );
        // Bits are the deterministic identity (0.1f32 = 0x3DCCCCCD).
        assert_eq!(LearningRate::new(0.1).unwrap().bits(), 0x3DCC_CCCD);
    }

    // -- full-network update ------------------------------------------------------------

    #[test]
    fn full_update_digest_pending() {
        let network = reference_network(REFERENCE_SEED);
        let state = ParameterState::from_network(&network).expect("valid state");
        assert_eq!(state.parameter_count(), 13_922_692);
        let grads = backward(&formula_patch(0), &formula_target(7), &network).expect("gradients");
        let before = state.digest();
        let updated =
            sgd_update(&state, &grads, LearningRate::new(0.01).unwrap()).expect("update runs");
        // Functional semantics: the input state is untouched.
        assert_eq!(state.digest(), before);
        // Shapes and count preserved; everything finite.
        assert_eq!(updated.parameter_count(), 13_922_692);
        assert!(updated
            .tensors
            .iter()
            .all(|tensor| tensor.values.iter().all(|v| v.is_finite())));
        // Repeated execution is byte-identical.
        let again =
            sgd_update(&state, &grads, LearningRate::new(0.01).unwrap()).expect("update runs");
        assert_eq!(updated.serialize(), again.serialize());
        // Pinned full-update digest (debug ≡ release ≡ MSRV verified
        // at validation time): reference network + Slice 9 golden
        // gradients + lr 0.01 through plain SGD.
        assert_eq!(
            updated.digest(),
            "37a27fee86aeaf77da2cffd2334b28f36093699bcf05a7390814397206f3d076"
        );
    }

    #[test]
    fn update_record_binds_all_inputs() {
        let network = reference_network(REFERENCE_SEED);
        let state = ParameterState::from_network(&network).expect("valid state");
        let grads = backward(&formula_patch(0), &formula_target(7), &network).expect("gradients");
        let lr = LearningRate::new(0.01).unwrap();
        let updated = sgd_update(&state, &grads, lr).expect("update runs");
        let record = UpdateRecord::new(&state, &grads, lr, &updated);
        assert_eq!(record.optimizer_id, OPTIMIZER_ID);
        assert_eq!(record.learning_rate_bits, "3c23d70a");
        assert_eq!(record.initial_digest, state.digest());
        assert_eq!(record.gradient_digest, grads.digest());
        assert_eq!(record.updated_digest, updated.digest());
        assert_eq!(record.digest().len(), 64);
        assert_eq!(
            record.digest(),
            UpdateRecord::new(&state, &grads, lr, &updated).digest()
        );
        // Every axis changes the update identity: rate, gradient,
        // initial state, and optimizer id.
        let other_lr =
            UpdateRecord::new(&state, &grads, LearningRate::new(0.02).unwrap(), &updated);
        assert_ne!(record.digest(), other_lr.digest());
        let other_grads =
            backward(&formula_patch(0), &formula_target(8), &network).expect("gradients");
        let other_updated = sgd_update(&state, &other_grads, lr).expect("update runs");
        let other_grad = UpdateRecord::new(&state, &other_grads, lr, &other_updated);
        assert_ne!(record.digest(), other_grad.digest());
        assert_ne!(record.updated_digest, other_grad.updated_digest);
    }

    // -- state serialization ---------------------------------------------------------------

    #[test]
    fn state_serialization_round_trips() {
        let network = reference_network(REFERENCE_SEED);
        let state = ParameterState::from_network(&network).expect("valid state");
        let bytes = state.serialize();
        assert!(bytes.starts_with(b"SP01"));
        assert_ne!(&bytes[0..4], b"SG01");
        let parsed = ParameterState::parse(&bytes).expect("parse runs");
        assert_eq!(parsed, state);
        assert_eq!(parsed.digest(), state.digest());
    }

    // -- negative tests --------------------------------------------------------------------------

    #[test]
    fn update_rejects_everything_invalid() {
        let network = reference_network(REFERENCE_SEED);
        let state = ParameterState::from_network(&network).expect("valid state");
        let grads = backward(&formula_patch(0), &formula_target(7), &network).expect("gradients");
        let lr = LearningRate::new(0.01).unwrap();
        // Non-finite gradient.
        let mut bad_grads = grads.clone();
        bad_grads.params[0].values[0] = f32::NAN;
        assert_eq!(
            sgd_update(&state, &bad_grads, lr).unwrap_err(),
            UpdateError::NonFiniteGradient
        );
        // Non-finite parameter.
        let mut bad_state = state.clone();
        bad_state.tensors[5].values[3] = f32::INFINITY;
        assert_eq!(
            sgd_update(&bad_state, &grads, lr).unwrap_err(),
            UpdateError::NonFiniteParam
        );
        // Shape mismatch (gradients for a different topology).
        let mut misshapen = grads.clone();
        misshapen.params[2].shape = vec![200, 69_559];
        misshapen.params[2].values.pop();
        assert!(matches!(
            sgd_update(&state, &misshapen, lr).unwrap_err(),
            UpdateError::StateGradientMismatch { .. }
        ));
        // Count mismatch.
        let mut short = grads.clone();
        short.params.pop();
        assert!(matches!(
            sgd_update(&state, &short, lr).unwrap_err(),
            UpdateError::StateGradientMismatch { .. }
        ));
        // State construction itself rejects unknown names.
        let mut unknown = state.clone();
        unknown.tensors[0].name = "nope.weight".to_string();
        assert!(matches!(
            ParameterState::build(unknown.tensors).unwrap_err(),
            UpdateError::BadParameter { .. }
        ));
        // State parse rejects wrong magic, truncation, trailing bytes.
        let bytes = state.serialize();
        let mut wrong_magic = bytes.clone();
        wrong_magic[0..4].copy_from_slice(b"SG01");
        assert!(matches!(
            ParameterState::parse(&wrong_magic).unwrap_err(),
            UpdateError::Serialization(_)
        ));
        assert!(matches!(
            ParameterState::parse(&bytes[..20]).unwrap_err(),
            UpdateError::Serialization(_)
        ));
        let mut trailing = bytes.clone();
        trailing.push(0xff);
        assert!(matches!(
            ParameterState::parse(&trailing).unwrap_err(),
            UpdateError::Serialization(_)
        ));
        // Missing and duplicate parameters rejected on parse.
        let mut partial = state.clone();
        partial.tensors.pop();
        assert!(matches!(
            ParameterState::parse(&partial.serialize()).unwrap_err(),
            UpdateError::BadParameter { .. }
        ));
        let mut doubled = state.clone();
        doubled.tensors[1] = doubled.tensors[0].clone();
        assert!(matches!(
            ParameterState::parse(&doubled.serialize()).unwrap_err(),
            UpdateError::BadParameter { .. }
        ));
    }
}
