//! Sonic52 Slice 11: deterministic multi-step loop (scaffolding only).
//!
//! The smallest possible composition of the trusted single-step chain
//! into a parameter-state trajectory — **loop scaffolding, not a
//! trainer**:
//!
//! ```text
//! θ₀ = initial parameters
//! for each selected sample in order:
//!     forward(θ) → loss → gradients → θ = SGD(θ, gradients)
//! return final parameters + per-step provenance
//! ```
//!
//! Step-numbering convention (documented once, used everywhere):
//! step 0 is the initial parameter state; step N (1-based) is the
//! state after the Nth sample/update. The initial state is always
//! represented explicitly, never implied.
//!
//! Design notes that matter:
//!
//! - Two entry points share one chaining core: `run_trajectory_states`
//!   steps precomputed (state, gradient) pairs (used by the
//!   independent tiny-reference check), while `run_trajectory_patches`
//!   derives gradients through the Slice 9 backward pass (used by the
//!   real path). Both enforce identical chaining, identity, and
//!   null-policy semantics.
//! - Empty input is a valid zero-step loop (final == initial), not an
//!   error — documented choice, matching the "no work requested, no
//!   work fabricated" reading of the null policy.
//! - No batches, shuffling, schedules, momentum, clipping,
//!   accumulation, checkpoints, or resume support. Deliberately absent.
//! - Per-step losses are recorded, never asserted monotonic: samples
//!   differ between steps, targets are synthetic, and SGD promises
//!   nothing across changing samples.

#![forbid(unsafe_code)]

use std::fmt;

use crate::frontend::Sonic52MelPatch;
use crate::gradient::{backward, NetworkGradients};
use crate::network::Sonic52ReferenceNetwork;
use crate::training::bce_with_logits;
use crate::update::{sgd_update, LearningRate, ParameterState, UpdateRecord, OPTIMIZER_ID};

// ---------------------------------------------------------------------
// errors (fail closed; reuse Slice 9/10 validation, add nothing lax)
// ---------------------------------------------------------------------

/// Loop failures. Every variant names the failing step (1-based update
/// count); upstream detail strings are quoted verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopError {
    /// A gradient set is malformed for its step.
    BadGradients {
        /// 1-based step index.
        step: usize,
        /// Cause.
        detail: String,
    },
    /// An update was rejected (shapes, finiteness, learning rate).
    UpdateRejected {
        /// 1-based step index.
        step: usize,
        /// Cause.
        detail: String,
    },
    /// An updated state is non-finite (f32 overflow is possible even
    /// from finite inputs — checked, never sanitized).
    NonFiniteUpdate {
        /// 1-based step index.
        step: usize,
    },
    /// Forward/loss evaluation failed for a step.
    ForwardFailed {
        /// 1-based step index.
        step: usize,
        /// Cause.
        detail: String,
    },
}

impl fmt::Display for LoopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoopError::BadGradients { step, detail } => {
                write!(f, "step {step}: bad gradients: {detail}")
            }
            LoopError::UpdateRejected { step, detail } => {
                write!(f, "step {step}: update rejected: {detail}")
            }
            LoopError::NonFiniteUpdate { step } => {
                write!(f, "step {step}: updated state non-finite")
            }
            LoopError::ForwardFailed { step, detail } => {
                write!(f, "step {step}: forward failed: {detail}")
            }
        }
    }
}

impl std::error::Error for LoopError {}

// ---------------------------------------------------------------------
// step records and trajectory identity
// ---------------------------------------------------------------------

/// One chained update step: the Slice 10 [`UpdateRecord`] plus the
/// 1-based step index, the sample identity that produced it, and the
/// step loss. The chain invariant — proven by test, not by inspection —
/// is `previous updated digest == this step's input digest`, anchored
/// at the initial state digest for step 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRecord {
    /// 1-based update count (step 0 is the initial state itself).
    pub step: usize,
    /// Stable sample identity for this step.
    pub sample_id: String,
    /// BCE loss that produced this step's gradients, formatted
    /// deterministically (see `format_loss`).
    pub loss_bits: String,
    /// The Slice 10 update provenance record.
    pub update: UpdateRecord,
}

/// Canonical deterministic encoding of one step.
impl StepRecord {
    /// Canonical deterministic encoding.
    pub fn canonical(&self) -> String {
        format!(
            "step-v1|{}|{}|{}|{}",
            self.step,
            self.sample_id,
            self.loss_bits,
            self.update.canonical()
        )
    }
}

/// Deterministic f32 rendering for digests: raw bits as 8 hex digits
/// (exact, no decimal formatting anywhere in an identity path).
pub fn format_loss(loss: f32) -> String {
    format!("{:08x}", loss.to_bits())
}

/// One deterministic multi-step trajectory: initial digest, ordered
/// step records, final digest, and the trajectory digest binding them
/// with the selection/optimizer/learning-rate identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trajectory {
    /// Digest of the initial parameter state (step 0).
    pub initial_digest: String,
    /// Step records in execution order.
    pub steps: Vec<StepRecord>,
    /// Digest of the final parameter state.
    pub final_digest: String,
    /// Selection identity digest (which samples, in which order).
    pub selection_digest: String,
    /// Learning-rate identity (8-hex-digit f32 bits).
    pub learning_rate_bits: String,
    /// Deterministic trajectory identity (see [`Trajectory::digest`]).
    pub trajectory_digest: String,
}

impl Trajectory {
    /// Canonical deterministic encoding: initial digest, selection,
    /// optimizer, learning rate, then every step record in order. No
    /// timestamps, hostnames, paths, PIDs, addresses, or durations.
    pub fn canonical(&self) -> String {
        let mut out = format!(
            "trajectory-v1|{}|{}|{}|{}",
            self.initial_digest, self.selection_digest, OPTIMIZER_ID, self.learning_rate_bits
        );
        for step in &self.steps {
            out.push_str(&step.canonical());
            out.push('|');
        }
        out.push_str(&self.final_digest);
        out
    }

    /// Deterministic trajectory identity.
    pub fn digest(&self) -> String {
        musicpack_core::format::checksum::sha256_hex(self.canonical().as_bytes())
    }
}

// ---------------------------------------------------------------------
// the loop: two entries, one chaining core
// ---------------------------------------------------------------------

/// One step's precomputed inputs for the state machine.
pub struct TrajectoryStep<'a> {
    /// Stable sample identity for this step.
    pub sample_id: &'a str,
    /// Step loss (recorded, not recomputed, by this entry point).
    pub loss: f32,
    /// Gradients to apply.
    pub gradients: &'a NetworkGradients,
}

/// Core state machine: applies precomputed (sample, loss, gradients)
/// steps in order, chaining digests and enforcing every Slice 10
/// validation on each transition. Returns the trajectory plus the
/// final state (callers that need intermediate states re-derive them
/// from the step records — nothing is hidden, nothing extra stored).
pub fn run_trajectory_states(
    initial: &ParameterState,
    learning_rate: LearningRate,
    selection_digest: &str,
    steps: &[TrajectoryStep<'_>],
) -> Result<(Trajectory, ParameterState), LoopError> {
    let mut state = initial.clone();
    let initial_digest = initial.digest();
    let mut step_records = Vec::with_capacity(steps.len());
    for (index, step) in steps.iter().enumerate() {
        let step_number = index + 1;
        let updated = sgd_update(&state, step.gradients, learning_rate).map_err(|error| {
            LoopError::UpdateRejected {
                step: step_number,
                detail: error.to_string(),
            }
        })?;
        if !updated
            .tensors
            .iter()
            .all(|tensor| tensor.values.iter().all(|v| v.is_finite()))
        {
            return Err(LoopError::NonFiniteUpdate { step: step_number });
        }
        let record = UpdateRecord::new(&state, step.gradients, learning_rate, &updated);
        // Chain invariant: this step consumed exactly the state the
        // previous step produced (recomputed digests, not carried
        // references — nothing to go stale).
        debug_assert_eq!(record.initial_digest, state.digest());
        step_records.push(StepRecord {
            step: step_number,
            sample_id: step.sample_id.to_string(),
            loss_bits: format_loss(step.loss),
            update: record,
        });
        state = updated;
    }
    let final_digest = state.digest();
    let mut trajectory = Trajectory {
        initial_digest,
        steps: step_records,
        final_digest,
        selection_digest: selection_digest.to_string(),
        learning_rate_bits: format!("{:08x}", learning_rate.bits()),
        trajectory_digest: String::new(),
    };
    let digest = trajectory.digest();
    trajectory.trajectory_digest = digest;
    Ok((trajectory, state))
}

/// Real-path entry: derives each step's loss and gradients through the
/// Slice 8/9 forward path (`forward_detailed` + BCE oracle +
/// [`backward`]) before delegating to the shared chaining core, so the
/// loop is composition rather than a second implementation.
pub fn run_trajectory_patches(
    initial: &ParameterState,
    network: &Sonic52ReferenceNetwork,
    learning_rate: LearningRate,
    selection_digest: &str,
    samples: &[PatchSample<'_>],
) -> Result<(Trajectory, ParameterState), LoopError> {
    let mut state = initial.clone();
    let initial_digest = initial.digest();
    let mut step_records = Vec::with_capacity(samples.len());
    // The reference network is fixed; per-step parameters come from
    // the evolving state. Slice 9 `backward` reads a full network, so
    // each step rebuilds the network view over the current state
    // through the one canonical constructor below (no second
    // forward implementation anywhere in this slice).
    for (index, sample) in samples.iter().enumerate() {
        let step_number = index + 1;
        let network_view =
            network_at_state(network, &state).map_err(|detail| LoopError::ForwardFailed {
                step: step_number,
                detail,
            })?;
        let detail = network_view
            .forward_detailed(sample.patch)
            .map_err(|error| LoopError::ForwardFailed {
                step: step_number,
                detail: error.to_string(),
            })?;
        let loss = bce_with_logits(&detail.logits, sample.target).map_err(|error| {
            LoopError::ForwardFailed {
                step: step_number,
                detail: error.to_string(),
            }
        })?;
        let gradients = backward(sample.patch, sample.target, &network_view).map_err(|error| {
            LoopError::BadGradients {
                step: step_number,
                detail: error.to_string(),
            }
        })?;
        let updated = sgd_update(&state, &gradients, learning_rate).map_err(|error| {
            LoopError::UpdateRejected {
                step: step_number,
                detail: error.to_string(),
            }
        })?;
        if !updated
            .tensors
            .iter()
            .all(|tensor| tensor.values.iter().all(|v| v.is_finite()))
        {
            return Err(LoopError::NonFiniteUpdate { step: step_number });
        }
        let record = UpdateRecord::new(&state, &gradients, learning_rate, &updated);
        debug_assert_eq!(record.initial_digest, state.digest());
        step_records.push(StepRecord {
            step: step_number,
            sample_id: sample.sample_id.to_string(),
            loss_bits: format_loss(loss),
            update: record,
        });
        state = updated;
    }
    let final_digest = state.digest();
    let mut trajectory = Trajectory {
        initial_digest,
        steps: step_records,
        final_digest,
        selection_digest: selection_digest.to_string(),
        learning_rate_bits: format!("{:08x}", learning_rate.bits()),
        trajectory_digest: String::new(),
    };
    let digest = trajectory.digest();
    trajectory.trajectory_digest = digest;
    Ok((trajectory, state))
}

/// One patch-level sample for the real-path loop.
pub struct PatchSample<'a> {
    /// Stable sample identity.
    pub sample_id: &'a str,
    /// The H0 patch to run forward.
    pub patch: &'a Sonic52MelPatch,
    /// 52-D target for the BCE oracle.
    pub target: &'a [f32; 52],
}

/// Rebuilds the reference-network view over an evolved parameter
/// state: identical topology and reference layout, values taken from
/// the state in canonical order. The single constructor both directions
/// of the loop depend on — forward and backward always see the state
/// the previous update produced, never a stale copy.
fn network_at_state(
    network: &Sonic52ReferenceNetwork,
    state: &ParameterState,
) -> Result<Sonic52ReferenceNetwork, String> {
    use crate::gradient::param_segments;
    if state.tensors.len() != crate::gradient::GRAD_PARAM_ORDER.len() {
        return Err(format!(
            "state has {} tensors, expected {}",
            state.tensors.len(),
            crate::gradient::GRAD_PARAM_ORDER.len()
        ));
    }
    let segments = param_segments(network);
    let mut view = network.clone();
    for segment in &segments {
        let tensor = state
            .tensors
            .iter()
            .find(|tensor| tensor.name == segment.name)
            .ok_or_else(|| format!("state lacks {}", segment.name))?;
        if tensor.shape != segment.shape || tensor.values.len() != segment.len {
            return Err(format!("state shape mismatch on {}", segment.name));
        }
        let values = tensor.values.clone();
        match segment.name {
            "conv.weight" => view.conv_weights = values,
            "conv.bias" => view.conv_bias = values,
            "dense200.weight" => view.dense_hidden.weights = values,
            "dense200.bias" => view.dense_hidden.bias = values,
            "dense52.weight" => view.dense_output.weights = values,
            "dense52.bias" => view.dense_output.bias = values,
            other => return Err(format!("unknown canonical parameter: {other}")),
        }
    }
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{Sonic52MelPatch, MEL_BANDS, PATCH_FRAMES};
    use crate::gradient::{backward, param_segments, NetworkGradients};
    use crate::network::{reference_network, REFERENCE_SEED};
    use crate::update::{LearningRate, ParameterState, OPTIMIZER_ID};

    fn tiny_state() -> ParameterState {
        // Six canonical tensors in miniature (shapes arbitrary but
        // consistent with the canonical names).
        let shapes: &[&[usize]] = &[&[2], &[1], &[2, 2], &[2], &[1, 2], &[1]];
        let values: &[&[f32]] = &[
            &[1.0, -2.0],
            &[0.5],
            &[1.0, 2.0, 3.0, 4.0],
            &[7.0, 8.0],
            &[10.0, -10.0],
            &[3.0],
        ];
        ParameterState::build(
            crate::gradient::GRAD_PARAM_ORDER
                .iter()
                .zip(shapes.iter())
                .zip(values.iter())
                .map(|((name, shape), vals)| crate::update::NamedTensor {
                    name: name.to_string(),
                    shape: shape.to_vec(),
                    values: vals.to_vec(),
                })
                .collect(),
        )
        .expect("valid tiny state")
    }

    fn tiny_grads(scale: f32) -> NetworkGradients {
        // Hand-built gradients matching the tiny state's shapes.
        let shapes: &[&[usize]] = &[&[2], &[1], &[2, 2], &[2], &[1, 2], &[1]];
        NetworkGradients {
            params: crate::gradient::GRAD_PARAM_ORDER
                .iter()
                .zip(shapes.iter())
                .map(|(name, shape)| {
                    let len: usize = shape.iter().product();
                    crate::gradient::ParameterGrad {
                        name: name.to_string(),
                        shape: shape.to_vec(),
                        values: vec![scale; len],
                    }
                })
                .collect(),
        }
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

    // -- empty loop ---------------------------------------------------------------

    #[test]
    fn empty_loop_is_identity() {
        // Zero selected samples: final == initial, zero steps — a valid
        // no-op, not an error (documented choice: no work requested,
        // no work fabricated).
        let state = tiny_state();
        let lr = LearningRate::new(0.1).unwrap();
        let (trajectory, final_state) =
            run_trajectory_states(&state, lr, "selection-x", &[]).expect("empty loop runs");
        assert!(trajectory.steps.is_empty());
        assert_eq!(final_state.serialize(), state.serialize());
        assert_eq!(trajectory.final_digest, trajectory.initial_digest);
        assert_eq!(trajectory.final_digest, state.digest());
    }

    // -- one-step loop ------------------------------------------------------------------

    #[test]
    fn one_step_loop_chains_single_update() {
        let state = tiny_state();
        let lr = LearningRate::new(0.1).unwrap();
        let grads = tiny_grads(0.5);
        let steps = [TrajectoryStep {
            sample_id: "s0",
            loss: 0.75,
            gradients: &grads,
        }];
        let (trajectory, final_state) =
            run_trajectory_states(&state, lr, "selection-x", &steps).expect("one step runs");
        assert_eq!(trajectory.steps.len(), 1);
        let record = &trajectory.steps[0];
        assert_eq!(record.step, 1);
        assert_eq!(record.sample_id, "s0");
        // Chaining: the step consumed the initial state exactly.
        assert_eq!(record.update.initial_digest, state.digest());
        assert_eq!(record.update.optimizer_id, OPTIMIZER_ID);
        assert_eq!(trajectory.initial_digest, state.digest());
        assert_eq!(trajectory.final_digest, final_state.digest());
        assert_ne!(trajectory.final_digest, trajectory.initial_digest);
        assert_eq!(trajectory.trajectory_digest, trajectory.digest());
        assert_eq!(trajectory.trajectory_digest.len(), 64);
    }

    // -- independent tiny-reference trajectory -----------------------------------------------

    #[test]
    fn tiny_trajectory_matches_independent_reference() {
        // Independent reference: plain f64 scalar loop over flat
        // vectors (different code shape from the per-tensor update),
        // compared element-wise. The loop under test must reproduce it
        // exactly — this is composition verified, not self-agreement.
        fn reference_sgd(params: &mut [f64], grads: &[f64], lr: f64) {
            for (param, grad) in params.iter_mut().zip(grads.iter()) {
                *param -= lr * grad;
            }
        }
        let state = tiny_state();
        let lr = LearningRate::new(0.1).unwrap();
        let grads = tiny_grads(0.5);
        let flat: Vec<f64> = state
            .tensors
            .iter()
            .flat_map(|tensor| tensor.values.iter().map(|v| f64::from(*v)))
            .collect();
        let flat_grads: Vec<f64> = grads
            .params
            .iter()
            .flat_map(|param| param.values.iter().map(|v| f64::from(*v)))
            .collect();
        // Three steps with per-step gradients (second step uses a
        // different scale to prove per-step independence).
        let grads_b = tiny_grads(-0.25);
        let flat_b: Vec<f64> = grads_b
            .params
            .iter()
            .flat_map(|param| param.values.iter().map(|v| f64::from(*v)))
            .collect();
        let steps = [
            TrajectoryStep {
                sample_id: "s0",
                loss: 0.75,
                gradients: &grads,
            },
            TrajectoryStep {
                sample_id: "s1",
                loss: 0.5,
                gradients: &grads_b,
            },
            TrajectoryStep {
                sample_id: "s2",
                loss: 0.25,
                gradients: &grads,
            },
        ];
        let (trajectory, final_state) =
            run_trajectory_states(&state, lr, "selection-x", &steps).expect("three steps run");
        assert_eq!(trajectory.steps.len(), 3);
        assert_eq!(
            trajectory
                .steps
                .iter()
                .map(|record| record.step)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        // Reference trajectory in f64, cast per element.
        let mut expected = flat.clone();
        reference_sgd(&mut expected, &flat_grads, 0.1);
        reference_sgd(&mut expected, &flat_b, 0.1);
        reference_sgd(&mut expected, &flat_grads, 0.1);
        let actual: Vec<f32> = final_state
            .tensors
            .iter()
            .flat_map(|tensor| tensor.values.iter().copied())
            .collect();
        assert_eq!(actual.len(), expected.len());
        // Tolerance, not exact equality: the implementation accumulates
        // in f32 step by step while the reference accumulates in f64 —
        // the last-ulp divergence this produces is expected and bounded.
        // A composition bug would diverge by orders of magnitude more.
        for (index, value) in actual.iter().enumerate() {
            let expected_value = expected[index] as f32;
            assert!(
                (value - expected_value).abs() < 1e-6,
                "element {index}: {value} vs {expected_value}"
            );
        }
        // Provenance chaining across the three steps.
        assert_eq!(trajectory.steps[0].update.initial_digest, state.digest());
        for pair in trajectory.steps.windows(2) {
            assert_eq!(pair[1].update.initial_digest, pair[0].update.updated_digest);
        }
        assert_eq!(
            trajectory.steps[2].update.updated_digest,
            trajectory.final_digest
        );
    }

    // -- one-step equivalence with Slice 10 -----------------------------------------------------

    #[test]
    fn one_step_loop_equals_slice10_golden() {
        // The composition proof that matters: one loop step over the
        // Slice 10 golden inputs (formula patch 0, formula target 7,
        // lr 0.01) MUST be byte-identical to the Slice 10 single-update
        // golden digest 37a27fee… — no new golden is created here.
        let network = reference_network(REFERENCE_SEED);
        let state = ParameterState::from_network(&network).expect("reference state builds");
        let patch = formula_patch(0);
        let target = formula_target(7);
        let grads = backward(&patch, &target, &network).expect("gradients run");
        let lr = LearningRate::new(0.01).unwrap();
        let steps = [TrajectoryStep {
            sample_id: "slice10-golden-sample",
            loss: 0.822_640_1,
            gradients: &grads,
        }];
        let (trajectory, final_state) =
            run_trajectory_states(&state, lr, "slice10-golden-selection", &steps)
                .expect("golden step runs");
        assert_eq!(
            final_state.digest(),
            "37a27fee86aeaf77da2cffd2334b28f36093699bcf05a7390814397206f3d076"
        );
        assert_eq!(trajectory.final_digest, final_state.digest());
        assert_eq!(trajectory.steps.len(), 1);
        assert_eq!(trajectory.steps[0].loss_bits, format_loss(0.822_640_1f32));
    }

    // -- multi-step trajectory on the reference network ----------------------------------------------

    #[test]
    fn multipatch_reference_trajectory_is_deterministic() {
        let network = reference_network(REFERENCE_SEED);
        let state = ParameterState::from_network(&network).expect("reference state builds");
        let lr = LearningRate::new(0.01).unwrap();
        let patches = [formula_patch(0), formula_patch(41), formula_patch(82)];
        let targets = [formula_target(7), formula_target(8), formula_target(9)];
        let samples = [
            PatchSample {
                sample_id: "t0",
                patch: &patches[0],
                target: &targets[0],
            },
            PatchSample {
                sample_id: "t1",
                patch: &patches[1],
                target: &targets[1],
            },
            PatchSample {
                sample_id: "t2",
                patch: &patches[2],
                target: &targets[2],
            },
        ];
        let (first, first_state) =
            run_trajectory_patches(&state, &network, lr, "selection-mp", &samples)
                .expect("trajectory runs");
        let (second, second_state) =
            run_trajectory_patches(&state, &network, lr, "selection-mp", &samples)
                .expect("trajectory runs");
        // Repeated execution is structurally identical…
        assert_eq!(first.canonical(), second.canonical());
        assert_eq!(first_state.serialize(), second_state.serialize());
        // …and the chain is internally consistent.
        assert_eq!(first.steps.len(), 3);
        assert_eq!(first.steps[0].update.initial_digest, state.digest());
        for pair in first.steps.windows(2) {
            assert_eq!(pair[1].update.initial_digest, pair[0].update.updated_digest);
        }
        // Per-step losses are finite (no monotonicity claim — samples
        // differ, targets are synthetic, lr is a research choice).
        // (Loss values are recorded in the trace; asserting finiteness
        // only, deliberately.)
        assert_eq!(first.final_digest, first_state.digest());
    }

    #[test]
    fn multipatch_trajectory_digest_pending() {
        let network = reference_network(REFERENCE_SEED);
        let state = ParameterState::from_network(&network).expect("reference state builds");
        let lr = LearningRate::new(0.01).unwrap();
        let patches = [formula_patch(0), formula_patch(41), formula_patch(82)];
        let targets = [formula_target(7), formula_target(8), formula_target(9)];
        let samples = [
            PatchSample {
                sample_id: "t0",
                patch: &patches[0],
                target: &targets[0],
            },
            PatchSample {
                sample_id: "t1",
                patch: &patches[1],
                target: &targets[1],
            },
            PatchSample {
                sample_id: "t2",
                patch: &patches[2],
                target: &targets[2],
            },
        ];
        let (trajectory, _) =
            run_trajectory_patches(&state, &network, lr, "selection-mp", &samples)
                .expect("trajectory runs");
        // Pinned multi-step digest (debug ≡ release ≡ MSRV verified at
        // validation time): three formula patches/targets through the
        // real-path loop at lr 0.01.
        assert_eq!(
            trajectory.trajectory_digest,
            "20886ce28ef7d7a9e17ffbcb9d0004203a879d9d8d50f638a7fe4afd3d2387f8"
        );
    }

    // -- immutability ------------------------------------------------------------------

    #[test]
    fn initial_state_survives_the_loop() {
        let network = reference_network(REFERENCE_SEED);
        let state = ParameterState::from_network(&network).expect("reference state builds");
        let before = state.digest();
        let lr = LearningRate::new(0.01).unwrap();
        let grads = backward(&formula_patch(0), &formula_target(7), &network).expect("gradients");
        let steps = [TrajectoryStep {
            sample_id: "s0",
            loss: 0.5,
            gradients: &grads,
        }];
        let (_, _) = run_trajectory_states(&state, lr, "selection-x", &steps).expect("loop runs");
        assert_eq!(state.digest(), before);
        // Same loop twice from the same initial state: identical.
        let (first, _) =
            run_trajectory_states(&state, lr, "selection-x", &steps).expect("loop runs");
        let (second, _) =
            run_trajectory_states(&state, lr, "selection-x", &steps).expect("loop runs");
        assert_eq!(first.canonical(), second.canonical());
    }

    // -- invalid states ------------------------------------------------------------------

    #[test]
    fn loop_fails_closed() {
        let network = reference_network(REFERENCE_SEED);
        let state = ParameterState::from_network(&network).expect("reference state builds");
        let lr = LearningRate::new(0.01).unwrap();
        let grads = backward(&formula_patch(0), &formula_target(7), &network).expect("gradients");
        // Non-finite gradients rejected at their step, naming it.
        let mut poisoned = grads.clone();
        poisoned.params[2].values[5] = f32::NAN;
        let steps = [TrajectoryStep {
            sample_id: "s0",
            loss: 0.5,
            gradients: &poisoned,
        }];
        assert!(matches!(
            run_trajectory_states(&state, lr, "selection-x", &steps).unwrap_err(),
            LoopError::UpdateRejected { step: 1, .. }
        ));
        // Shape-mismatched gradients rejected at their step.
        let mut misshapen = grads.clone();
        misshapen.params[4].values.pop();
        misshapen.params[4].shape = vec![52, 199];
        let steps = [TrajectoryStep {
            sample_id: "s0",
            loss: 0.5,
            gradients: &misshapen,
        }];
        assert!(matches!(
            run_trajectory_states(&state, lr, "selection-x", &steps).unwrap_err(),
            LoopError::UpdateRejected { step: 1, .. }
        ));
        // Non-finite initial state rejected before step 1.
        let mut poisoned_state = state.clone();
        poisoned_state.tensors[0].values[0] = f32::INFINITY;
        let steps = [TrajectoryStep {
            sample_id: "s0",
            loss: 0.5,
            gradients: &grads,
        }];
        assert!(matches!(
            run_trajectory_states(&poisoned_state, lr, "selection-x", &steps).unwrap_err(),
            LoopError::UpdateRejected { step: 1, .. }
        ));
    }

    // -- identity sensitivity ---------------------------------------------------------------

    #[test]
    fn trajectory_identity_separates_axes() {
        let network = reference_network(REFERENCE_SEED);
        let state = ParameterState::from_network(&network).expect("reference state builds");
        let lr = LearningRate::new(0.01).unwrap();
        let grads = backward(&formula_patch(0), &formula_target(7), &network).expect("gradients");
        let steps = [TrajectoryStep {
            sample_id: "s0",
            loss: 0.5,
            gradients: &grads,
        }];
        let base = run_trajectory_states(&state, lr, "selection-x", &steps)
            .expect("loop runs")
            .0
            .trajectory_digest;
        // Learning rate changes the identity.
        let other_lr = run_trajectory_states(
            &state,
            LearningRate::new(0.02).unwrap(),
            "selection-x",
            &steps,
        )
        .expect("loop runs")
        .0
        .trajectory_digest;
        assert_ne!(base, other_lr);
        // Sample order changes the identity (two-step loop, swapped).
        let grads_b =
            backward(&formula_patch(41), &formula_target(8), &network).expect("gradients");
        let order_a = [
            TrajectoryStep {
                sample_id: "s0",
                loss: 0.5,
                gradients: &grads,
            },
            TrajectoryStep {
                sample_id: "s1",
                loss: 0.25,
                gradients: &grads_b,
            },
        ];
        let order_b = [
            TrajectoryStep {
                sample_id: "s1",
                loss: 0.25,
                gradients: &grads_b,
            },
            TrajectoryStep {
                sample_id: "s0",
                loss: 0.5,
                gradients: &grads,
            },
        ];
        let digest_a = run_trajectory_states(&state, lr, "selection-x", &order_a)
            .expect("loop runs")
            .0
            .trajectory_digest;
        let digest_b = run_trajectory_states(&state, lr, "selection-x", &order_b)
            .expect("loop runs")
            .0
            .trajectory_digest;
        assert_ne!(digest_a, digest_b);
        // Initial state changes the identity.
        let mut other_state = state.clone();
        other_state.tensors[5].values[0] += 1.0;
        let digest_state = run_trajectory_states(&other_state, lr, "selection-x", &steps)
            .expect("loop runs")
            .0
            .trajectory_digest;
        assert_ne!(base, digest_state);
        // Sample identity changes the identity.
        let other_sample = [TrajectoryStep {
            sample_id: "s9",
            loss: 0.5,
            gradients: &grads,
        }];
        let digest_sample = run_trajectory_states(&state, lr, "selection-x", &other_sample)
            .expect("loop runs")
            .0
            .trajectory_digest;
        assert_ne!(base, digest_sample);
    }

    // -- selection binding ---------------------------------------------------------------------

    #[test]
    fn selection_digest_participates() {
        // Same steps under two selection digests diverge — selection is
        // bound, not decorative.
        let network = reference_network(REFERENCE_SEED);
        let state = ParameterState::from_network(&network).expect("reference state builds");
        let lr = LearningRate::new(0.01).unwrap();
        let grads = backward(&formula_patch(0), &formula_target(7), &network).expect("gradients");
        let steps = [TrajectoryStep {
            sample_id: "s0",
            loss: 0.5,
            gradients: &grads,
        }];
        let digest_a = run_trajectory_states(&state, lr, "selection-a", &steps)
            .expect("loop runs")
            .0
            .trajectory_digest;
        let digest_b = run_trajectory_states(&state, lr, "selection-b", &steps)
            .expect("loop runs")
            .0
            .trajectory_digest;
        assert_ne!(digest_a, digest_b);
    }

    // -- param layout sanity ----------------------------------------------------------------------

    #[test]
    fn flat_layout_covers_everything_once() {
        use crate::gradient::parameter_count;
        let network = reference_network(REFERENCE_SEED);
        let segments = param_segments(&network);
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.name)
                .collect::<Vec<_>>(),
            crate::gradient::GRAD_PARAM_ORDER
        );
        let total: usize = segments.iter().map(|segment| segment.len).sum();
        assert_eq!(total, parameter_count(&network));
        // Contiguous, gap-free, in canonical order.
        let mut offset = 0usize;
        for segment in &segments {
            assert_eq!(segment.offset, offset);
            offset += segment.len;
        }
    }
}
