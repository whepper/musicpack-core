# ADR 0020: Sonic52 learning objective and training design

- **Status:** Proposed (2026-10-02; formulation only, no training)
- **Decision type:** Research-design selection within the training gate
- **Production impact:** None. No dependency, profile, format, schema,
  API, UI, or runtime behaviour is changed by this ADR or by the
  `experiments/sonic52-research/src/objective.rs` design it records.
- **Related decisions:** ADR 0017 (similarity mechanism), ADR 0018
  (Sonic52 research track), ADR 0019 (training-readiness gate)

## 1. Retained working assumption

The first Sonic52 training experiments assume **52-output multi-label
prediction trained with binary cross-entropy** (`src/objective.rs`
`V1_OBJECTIVE`). Reasons: it is the lineage training formulation
(FORENSICS.md S9, Slice 6 oracle), and it is the only candidate
directly compatible with the reported 52-D sigmoid head. This is an
assumption, not a finding — Slice 6's load-bearing status for it is
unchanged.

## 2. Alternatives kept open

Triplet metric learning, supervised contrastive learning, and
classification-derived embeddings are documented with their dataset
requirements and stay blocked on curated pair/label data that does not
exist. Target semantics (artist/album/genre/tag/curated/listening)
carry stated shortcut risks; no target is selected.

## 3. Output-vs-embedding decision

The persisted similarity embedding is the **post-sigmoid 52-D
output**: training target and stored representation stay identical
with no extra normalization step to justify. Whether the historical
objective was simply BCE on those outputs is explicitly unknown.
Post-aggregation normalization is UNDECIDED.

## 4. Framework and reproducibility

No training stack is adopted: objective, data, and scale are not yet
fixed, so no framework can be justified. The production constraint
(no heavyweight runtime for inference) stands; any research-only
trainer must export frozen weights through the Slice 6 weight
contract. Every future run is identified by the Slice 7 run manifest
(dataset, preprocessing, architecture, init, seed, objective,
augmentation, optimizer, schedule, batch, budget, toolchain, weight
and evaluation digests).

## 5. What this ADR does not do

No training, gradients (beyond hand-derived formulation checks),
optimizers, loops, checkpoints, learned weights, corpus, quality
evaluation, or production integration. No quality claim. No change to
ADR 0016–0019 findings.
