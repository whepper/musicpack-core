# ADR 0019: Sonic52 training-readiness gate

- **Status:** Proposed (2026-10-02; contract only, no training)
- **Decision type:** Research boundary and versioned contract freeze
- **Production impact:** None. No dependency, profile, format, schema,
  API, UI, or runtime behaviour is changed by this ADR or by the
  `experiments/sonic52-research/src/training.rs` contract it records.
- **Related decisions:** ADR 0017 (similarity mechanism), ADR 0018
  (Sonic52 research track)

## 1. Why this ADR exists

Slices 1–5 built deterministic research plumbing (frontend, ingestion,
patch harness, reference network, corpus runner) with all values
labelled as evidence, choice, or unknown. Before any learning code is
written, the contracts a future trainer must obey are frozen here so
Slice 7 can implement without rediscovering them — and without
silently redefining them.

## 2. What is frozen

- **Dataset manifest** (`sonic52-dataset-v1`): metadata + identity
  only; rows sorted by track id; duplicates and missing metadata fail
  closed; canonical serialization + digest.
- **Sample identity**: SHA-256 over dataset, version, track,
  preprocessing contract, and patch index. Path-, order-, and
  timing-independent; version-sensitive for leak detection.
- **Splits** (`split-v1`): deterministic hash-bucket assignment with
  validated ratios; one track, one split; cross-manifest leakage
  detector. Track-level separation is the minimum contract —
  artist/album/version relations are explicitly out of scope.
- **Model contract**: decided fields (52-D sigmoid output; hidden 200
  + relu as S1-reported; Mean aggregation as the v1 choice) alongside
  explicit `undecided` fields (conv config, normalization, …).
  `is_complete()` is false by design.
- **Weight contract** (`sonic52-weights-v1`): named tensors,
  shape/dtype/finiteness validation, canonical bytes, digest. No
  learned weights exist.
- **Loss oracle**: numerically stable binary cross-entropy with
  logits, hand golden `ln(2)`. Assumes multi-label sigmoid outputs
  (the lineage training formulation); a softmax future supersedes it.
- **Evaluation** (`sonic52-eval-v1`, pre-registered): held-out
  disjointness rule, fixed pipeline steps, a human-listening procedure
  (not performed), Discogs-EffNet baselines named but never invoked,
  52/128 arms open with no superiority claim.
- **Provenance gate**: eligible-with-licence-id admitted;
  excluded/unknown fail closed. No corpus, no invented licences.
- **Slice 7 boundary**: explicit MAY / MUST-NOT-REDEFINE lists. Slice
  7 may build ingestion, parameterization, forward, loss, gradients,
  optimizer, loops, checkpoints, and weight serialization — but must
  not silently redefine dataset, preprocessing, sample, model, split,
  weight-serialization, or evaluation semantics.

## 3. What this ADR does not do

No training, gradients, optimizers, loops, checkpoints, learned
weights, corpus, quality evaluation, or production integration. No
quality claim about Sonic52. No change to ADR 0016/0017/0018 findings.
The Slice 4 reference network remains an untrained plumbing artifact;
its placeholder convolution is not promoted to a training decision.

## 4. Licence posture

Unchanged from ADR 0017/0018: training-data licensing is unresolved
and gated per-sample. Nothing here clears any dataset, model, or
weight for production use.
