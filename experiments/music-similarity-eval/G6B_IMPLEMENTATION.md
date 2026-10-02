# G-6B — implementation readiness report

> **Status: `G-6B: READY FOR REAL-CORPUS RUN`**
>
> **`G-6: FAIL — KEEP F32` — unchanged.**
>
> This is the implementation record for the readiness work identified by the
> final adversarial review ([`G6B_QWEN_REVIEW.md`](G6B_QWEN_REVIEW.md),
> verdict: *READY WITH IMPORTANT NOTES — no blocker*). The real-corpus
> experiment has **not** been executed: no model, no audio, no inference, no
> recorded embeddings were touched, recovered, or recreated. Everything below
> is experiment-crate code, tests, and documentation inside
> `experiments/music-similarity-eval/`.

## 1. I-1 implementation status — complete

All five missing gated quantities are implemented, deterministic, and tested:

| Item | Where | Notes |
| --- | --- | --- |
| B-C1 measurement | `src/g6b.rs` (`norm_deviation`, `measure_profile_safety`) | per-reference-vector `|‖v‖ − 1|`; worst vector identified |
| B-C2 measurement | `src/g6b.rs` (`subnormal_energy_fraction`, `support_record`) | per-reference-vector subnormal energy fraction φ; support/participation record (report only) |
| B-C3 measurement | `src/g6b.rs` (`encode_storage_path`, `encode_corpus_storage_path`) | rejections recorded, never panicked |
| B-B2 album retrieval | `src/g6b.rs` (`compare_album_retrieval`) | ordered sequences at k = 12 over per-world recomputed aggregates |
| Gate evaluation | `src/g6b.rs` (`evaluate_gates`, `verdict_of_gates`, `overall_verdict`, `run_corpus_experiment`) | the seven frozen gates and nothing else |
| Real-corpus ingestion | `src/g6b_real.rs` | `embeddings.json` loader with §13 digest re-verification; `albums[]` deliberately not deserialized |
| Real-corpus runner | `src/bin/g6b_real.rs` | §17.5 criteria-freeze check at run time; deterministic verdict artefact; fail-closed exit codes |

`src/g6b.rs` was also refactored so the same code path serves both worlds:
`compare_worlds` takes two fully built record sets, `compare_storage_path_records`
encodes and compares (returning B-C3 rejections instead of panicking), and the
pre-existing `compare_storage_path(corpus)` surrogate wrapper is unchanged in
behaviour. The instrument-check artefact was regenerated accordingly.

## 2. B-C1 measurement

`norm_deviation(v) = | ‖v‖₂ − 1 |` with f64 accumulation, measured per
**reference** (pre-quantization) vector; `measure_profile_safety` reports the
maximum, the vector it occurred on, and the §9 support record (nonzero count,
participation ratio `(Σv²)²/Σv⁴`, report only). Gate **B-C1** passes iff
`max |‖v‖ − 1| ≤ 2^-10` over every reference vector — the exact §11 wording;
no new normalization criterion was introduced. An empty corpus yields
NOT EVALUATED, never a pass.

## 3. B-C2 measurement and corrected derivation

`subnormal_energy_fraction(v)` is the fraction of the vector's energy in
components with magnitude strictly below `2^-14` (the binary16 subnormal
range). **The frozen threshold is unchanged: φ ≤ 2^-24 per vector**; exact
zeros are harmless, exactly as §9 specifies. Gate B-C2 is measured on the
profile's reference vectors, per vector, with the worst vector identified.

**Corrected derivation (recorded as §18 R-2 of `G6B_METHODOLOGY.md`).** §9's
"half the normal-range error budget" sentence composes to 1.5 × the budget,
not ≤ 1 ×; the operative argument is the one its parenthesis already contains:
a subnormal component's RNE error is at most **2^-25 absolute** (half the
2^-24 subnormal step), so with **MAX_DIMENSIONS = 4096** (FORMAT_SPEC decision
C) the subnormal contribution is ≤ 4096·2^-50 = 2^-38, giving
‖u − u'‖² ≤ 2^-22·(1 − φ) + 2^-38 ≤ 2^-22·(1 + 2^-16) and
**|Δcos| ≤ 2^-10·(1 + 5×10^-4)** — an absolute excess below 5×10^-7 over the
B-A1 constant. No constant was weakened or strengthened; B-C2 remains
deliberately conservative at bounded dimension. §9 now points to §18 R-2.

## 4. B-C3 overflow handling

`to_f16_candidate` (untouched, historical) still `expect`s — it is the
surrogate instrument's shortcut. The G-6B path does not rely on a panic:

- `encode_storage_path(vector, index)` returns `Ok(decoded)` or
  `Err(ComponentRejection { vector_index, component_index, value, reason })`
  with `reason ∈ {NonFinite, NotRepresentable}`;
- `encode_corpus_storage_path` collects **every** rejection across the corpus
  (affected vectors counted) and is all-or-nothing: a vector with any
  rejection has no f16 representation, so the symmetric comparison is
  **not fabricated** — B-C3 is recorded FAIL and B-A1/B-A2/B-B1/B-B2 are
  recorded NOT EVALUATED with that reason, a configuration that can never
  yield a PASS;
- the runner prints every rejection with the information needed to identify
  the affected vector, and exits non-zero.

The production binary16 writer (`docfmt`) is unchanged; B-C3 is experiment
instrumentation only.

## 5. B-B2 album comparison

`compare_album_retrieval(reference_records, stored_records)`:

1. derives the **reference** album aggregates from the reference vectors and
   the **f16** album aggregates from the f16-decoded vectors — each world
   independently, using only the already-specified
   `eval::aggregate_albums` rule (mean of members, then L2);
2. ranks each world with the unchanged `eval::album_nearest` (query album
   excluded, score descending, index tie-break);
3. compares ordered prefixes at every evaluated k and gates on **k = 12**
   (the ADR 0017 §5.8 album limit), with per-k evaluability flags so an
   insufficient album population is NOT EVALUATED rather than spuriously
   green.

The recorded `albums[]` block of a run output is never deserialized
(`src/g6b_real.rs` omits the field entirely), so no f32 aggregate can leak
into the f16 world. No new album semantics were introduced (§7 below).

## 6. Gate-evaluation semantics

Exactly the frozen seven gates, in §11 table order:
`GATE_IDS = [B-A1, B-A2, B-B1, B-B2, B-C1, B-C2, B-C3]`, each threshold a
single named constant (`BA1_LIMIT`, `BA2_LIMIT`, `BC1_TOLERANCE`,
`BC2_MAX_SUBNORMAL_ENERGY`, `BC3_MAX_REJECTIONS`, `ALBUM_GATE_K`) sourced
from §11. B-B1/B-B2 are definitional (exact identity).

- **Gate state**: `PASS | FAIL(reason) | NOT EVALUATED(reason)` — an absent
  measurement is never a pass (§11's own rule).
- **Corpus verdict** (`verdict_of_gates`): any FAIL → FAIL; else any
  NOT EVALUATED → INCONCLUSIVE; else PASS.
- **Run verdict** (`overall_verdict`): PASS only when every required gate is
  evaluated and passes on **every** real corpus/configuration; any corpus
  FAIL dominates; any INCONCLUSIVE corpus makes the run INCONCLUSIVE.
- **Completeness**: `gate_set_complete` requires exactly the seven ids in §11
  order; the artefact prints `gate_set_complete=true|false`.
- **Dependency ordering**: B-C3 failure ⇒ the storage path is undefined ⇒
  B-A*/B-B* are NOT EVALUATED (never fabricated, never PASS).
- **Runner**: `--bin g6b_real` verifies the §17.5 freeze **before** any gate
  is evaluated (a drift aborts with no verdict artefact), verifies every
  track digest (§13), writes the deterministic gate artefact, and exits 0
  only on an overall PASS.

The verdict vocabulary is exactly §11's: PASS / FAIL / INCONCLUSIVE, with
per-gate NOT EVALUATED — no new categories.

## 7. I-3 album-semantic limitation — recorded

Recorded as **§18 R-3** of `G6B_METHODOLOGY.md`: album similarity depends on
aggregation semantics that the format work deliberately deferred (ADR 0017
§5.5.1 decision B); G-6B uses only already-specified semantics (the §5.5
candidate rule that `eval::aggregate_albums` implements, and §5.7's
"server-side derivation"); aggregates are recomputed per world; an
album-component result must not be read as a v1.0 product decision while the
product semantics remain deferred. The existing semantics *are* sufficient
for the comparison, so it was implemented rather than blocked.

## 8. I-4 f32-tie / future-ordering limitation — recorded

Recorded as **§18 R-4**: B-B1/B-B2 measure the presented result under the
declared procedure (f32 scores, index tie-break). A hypothetical future
server ordering with finer-than-f32 score semantics (D-4 / tag 14) could
present differently at sub-f32-ULP margins inside exact f32 ties — a class
M12 does not capture (its counter skips reference-equal pairs). The current
retrieval semantics are unchanged; the limitation is an explicit recorded
dependency on a product/architecture decision that has not been made.

## 9. I-5 corpus-scaling limitation — recorded

Recorded as **§18 R-5**: exact retrieval identity is established only for the
tested 45-track reference corpus, exhaustively (no sampling); the conclusion
is profile-specific (D-6); collection-scale behaviour beyond the tested
population is an extrapolation, with M13 margins/populations retained as the
supporting evidence. No confidence interval was invented.

## 10. Tests added

Seventeen tests in `src/g6b_tests.rs` (suite total: 87, all passing). All
gate-machinery tests run on hand-built synthetic corpora — **never on the
surrogate** (same-data prohibition, §14 rule 3):

- **B-C1**: `bc1_norm_deviation_measures_each_reference_vector` (valid
  normalized, off-norm, deterministic, per-vector identification);
  `bc1_gate_passes_unit_vectors_and_fails_off_norm_ones`.
- **B-C2**: `bc2_subnormal_energy_fraction_matches_the_frozen_definition`
  (no subnormal energy, at/below the 2^-14 boundary, exceeding, determinism);
  `bc2_gate_passes_at_the_boundary_and_fails_past_the_threshold`.
- **B-C3**: `storage_path_reports_rejections_instead_of_panicking`
  (representable round trip, overflow, non-finite, corpus-level collection
  with indices); `bc3_violation_yields_fail_and_never_an_evaluated_pass`.
- **B-B2 album**: `album_comparison_ranks_each_world_from_its_own_aggregates`
  (deterministic changed ranking from per-world aggregates; identical worlds
  agree); `album_comparison_marks_degenerate_aggregation_not_evaluable`.
- **Verdict**: `all_gates_green_on_a_clean_corpus_yields_pass`;
  `an_unevaluatable_gate_is_reported_and_never_passes` (missing population →
  NOT EVALUATED → INCONCLUSIVE, never PASS);
  `a_changed_presented_result_fails_the_retrieval_gates`;
  `verdict_requires_every_gate_evaluated_and_passing`;
  `overall_verdict_fails_closed_across_corpora`.
- **Freeze**: `the_frozen_gate_rows_are_present_in_the_methodology`;
  `criteria_freeze_check_rejects_drift`.
- **Ingestion/artefact**: `the_real_loader_verifies_embedding_digests_before_measurement`
  (digest mismatch names the track; dimension drift fails; `albums[]`
  ignored); `the_real_run_report_carries_no_track_metadata`.

The all-green fixture is **f16-exact by construction** (components n/32 with
Σn² = 1024, so ‖v‖ = 1 exactly and the storage path is the identity): both
worlds are bit-identical by construction, making the PASS provable rather
than probabilistic. Construction asserts Σn² = 1024 so the fixture cannot
silently rot.

## 11. Determinism results

- `cargo run --bin g6b` executed twice: identical measurements, rankings and
  gate-relevant state; report digest
  `c67e0992381813d0a3c2d3523f306003059a0122b8b3ab12c803ee184ef9468b`
  in both runs.
- `g6b_tests::the_report_is_byte_identical_across_independent_runs` and
  `the_committed_report_matches_the_current_code` pass against the
  regenerated `fixtures/g6b/REPORT.txt` (an instrument-check artefact only —
  measurements; gates are never evaluated on the surrogate).
- The runner binary was smoke-tested end-to-end against a hand-built
  synthetic `embeddings.json` in temporary storage (not committed, since
  removed): clean run → all seven gates evaluated, overall PASS, exit 0;
  drifted criteria → refusal before any gate evaluation, exit 1; tampered
  track digest → failure naming the track, exit 1. No real data involved.

## 12. Historical G-6 preservation

- `src/g6.rs`, `fixtures/g6/REPORT.txt`, `fixtures/g6/RUN.txt`: untouched.
  Digests re-verified:
  - `fixtures/g6/REPORT.txt` = `921e58f1c9e3ad4127ce87c0566741fb…` — the
    digest pinned in `G6A_CRITERIA_REVIEW.md` §1;
  - `fixtures/g6/RUN.txt` = `5aa9f0304f84b640b3f65910ae4da03424d0…`.
- `g6_tests` (historical pins: committed report matches untouched code,
  byte-identical reruns, no leakage, no perceptual claims) all pass.
- The historical verdict **`G-6: FAIL — KEEP F32`** stands, is quoted
  unchanged in the new artefacts, and is enforced in
  `g6b_tests::the_methodology_freeze_and_prohibition_are_recorded`.

## 13. Files changed

All inside `experiments/music-similarity-eval/`:

| File | Change |
| --- | --- |
| `src/g6b.rs` | extended: frozen gate constants, B-C1/B-C2/support measurements, B-C3 rejection-recording encode path, album-level k=12 comparison, `compare_worlds`/`compare_storage_path_records` refactor, gate evaluator + verdict machinery, §17.5 freeze check, real-run artefact rendering; surrogate report extended with B-C measurements and album metrics (measurements only) |
| `src/g6b_real.rs` | new: digest-verifying `embeddings.json` loader + provenance lines (`albums[]` never read) |
| `src/bin/g6b_real.rs` | new: real-corpus runner (freeze check, digest verification, gate evaluation, deterministic artefact, fail-closed exit codes) |
| `src/g6b_tests.rs` | +17 gate-machinery/ingestion/freeze tests; existing tests preserved |
| `src/lib.rs` | `pub mod g6b_real;` registered |
| `G6B_METHODOLOGY.md` | status → READY FOR REAL-CORPUS RUN; §9 pointer to R-2; §15 pointer; new §18 (R-1…R-5); §3 scope line |
| `README.md` | G-6B status paragraph updated |
| `G6B_IMPLEMENTATION.md` | new: this report |
| `fixtures/g6b/REPORT.txt` | regenerated instrument-check artefact (measurements only; digest `c67e0992…`) |
| `fixtures/g6b/RUN.txt` | provenance record updated for the regenerated artefact |

## 14. No production or format changes

- `git status` shows changes only under `experiments/music-similarity-eval/`;
  `crates/`, `docs/`, `web/`, `author/`, and production `src/` are untouched.
- No dependency added; the runner uses the crate's existing
  `serde`/`serde_json`/`sha2`.
- `FORMAT_SPEC.md`, `FORMAT_SIGNOFF.md`, ADR 0017, and the `.mpack`/`.msim`
  formats are untouched; the production binary16 writer (`docfmt`) is
  untouched.
- The §11 criteria table is byte-identical to the reviewed version
  (`g6b::check_criteria_freeze` + tests pin all seven rows); D-1…D-6 remain
  unresolved; no gate was added, removed, weakened, or re-thresholded; the
  `2^-9` floor and τ remain non-gates.

## How the real-corpus run will be executed (when authorized)

```sh
cargo run --manifest-path experiments/music-similarity-eval/Cargo.toml --bin g6b_real -- \
  --corpus multi61=PATH/multi-hop61/embeddings.json \
  --corpus multi62=PATH/multi-hop62/embeddings.json \
  --corpus release61=PATH/release-hop61/embeddings.json \
  --corpus release62=PATH/release-hop62/embeddings.json \
  --criteria experiments/music-similarity-eval/G6B_METHODOLOGY.md \
  --out SOME_EXTERNAL_DIR
```

Pure post-processing of recorded vectors: no model download, no audio, no
inference, no Python, no network. The four configurations are evaluated as
four separate corpora with their own provenance lines and one overall
verdict; `PASS` additionally requires the §17 human steps (reversal-census
review, D-1…D-6 dispositions) before any G-6 status revision.
