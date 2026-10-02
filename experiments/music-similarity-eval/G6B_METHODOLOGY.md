# G-6B — reconciled methodology for the f16 storage experiment

> **Status: `G-6B: READY FOR REAL-CORPUS RUN`.**
>
> The instrument is complete and deterministic (§18 R-1): the seven frozen
> gates of §11 are implemented, tested, and evaluated only against the real
> corpus, by the runner of §13/§17. The real-corpus run itself has **not**
> been executed.
>
> This document reconciles the G-6A criteria with the independent review
> ([`G6A_GLM_REVIEW.md`](G6A_GLM_REVIEW.md)) and defines the methodology for
> the final G-6 re-run against the real MusicPack similarity corpus. It is a
> methodology, not a result: no criterion here has been evaluated against the
> real corpus, and none is evaluated against the surrogate (§14).
>
> **The formal status of G-6 is unchanged and remains:**
>
> **`G-6: FAIL — KEEP F32`**
>
> The reference storage encoding remains `f32le`. Nothing in this document
> approves f16, modifies `FORMAT_SPEC.md`, `FORMAT_SIGNOFF.md`, ADR 0017, the
> `.mpack` format, or any production code.

---

## 1. Purpose

Determine whether replacing the stored f32 representation of MusicPack
similarity vectors with binary16 changes **what MusicPack returns to a user**,
and separately whether binary16 is a **safe representation** for the vector
distributions of profiles that might declare it. These are two different
questions with two different evidence bases; G-6B keeps them separate by
construction (§5, §6, §9).

G-6B supersedes the *instruments* of G-6/G-6A. It does not supersede their
history: the G-6 run, its FAIL verdict, the G-6A repair, and both independent
reviews remain part of the audit trail, untouched.

## 2. Formal status

| Item | Status |
| --- | --- |
| G-6 verdict | **`FAIL — KEEP F32`** — unchanged, still open |
| Reference encoding | `f32le` — unchanged |
| `fixtures/g6/*` | historical artefacts, byte-unchanged, still pinned to the untouched `g6` code by `g6_tests` |
| `G6A_CRITERIA_REVIEW.md` | historical record of the G-6A repair — unmodified |
| `G6A_GLM_REVIEW.md` | independent review — unmodified |
| This document | methodology, recorded review findings (§18); criteria frozen per §14 |
| `fixtures/g6b/*` | instrument check on the surrogate — **not** criteria evidence (§14) |

## 3. Scope

In scope: experiment code under `experiments/music-similarity-eval/`, the
corrected measurement instrument (`src/g6b.rs`), the real-corpus ingestion and
gate runner (`src/g6b_real.rs`, `src/bin/g6b_real.rs`), this methodology, and
the definition of the eventual real-corpus run.

Out of scope: production code, `.mpack`/`.msim` format changes, ADR 0017,
`FORMAT_SPEC.md`, `FORMAT_SIGNOFF.md`, model downloads, audio decoding,
inference, CLAP, perceptual claims (G-7), and any decision to adopt f16.

The reconciliation of every finding in the independent review
(ACCEPT / REJECT / DEFER / INVESTIGATE) is **Appendix A**. The disposition of
every prior criterion (C1–C10, R-A*, R-B*, R-C*) is **Appendix B**.

## 4. Storage path being tested

The experiment models the actual storage and retrieval path end to end:

```text
f32 reference vector
  -> binary16 serialization   (docfmt::f32_to_f16_bits, FORMAT_SPEC §7.4: RNE,
                               ties-to-even, subnormals produced, overflow rejected)
  -> binary16 decode          (docfmt::f16_bits_to_f32, exact widening)
  -> cosine                   (eval::cosine: f64 accumulation over f32 inputs)
```

**Both operands of every comparison travel through the path.** This follows
from the architecture, not from a guess: ADR 0017 §5.8 defines similarity
retrieval as `GET /api/v1/tracks/{id}/similar` — the query is itself a stored
track embedding, so in an f16 world the query is a decoded binary16 vector just
as every candidate is. Album aggregates are derived server-side from stored
vectors (`eval::aggregate_albums`: mean over members, then L2), so the
aggregate's inputs are decoded binary16 vectors as well.

Decoded vectors are **not** re-normalized: `eval::cosine` divides by both
norms, and re-normalizing would (a) break bit-exactness with the specified
conversion and (b) hide the norm perturbation, which is itself measured (B-A2).

### 4.1 The instrumentation defect this corrects

The independent review found, and this work verified, that `g6::compare`'s
pairwise loop (`src/g6.rs:489-491`) quantized only the *second* operand of each
pair, while the declared method (`FORMAT_SPEC.md` §7.3 step 2: compare
`(v_i, v_j)` against `(q_i, q_j)`) and the ranking loop inside the same
function both use two quantized operands. The committed G-6 headline numbers
(`max_abs_delta_cosine`, `mean_abs_delta_cosine`, `spearman_rho`,
`min_cosine_candidate`, `sign_crossings`) therefore describe the mixed
f32×f16 regime and understate the storage-path error by a measured factor of
**1.52 (1280-D), 1.53 (512-D), 1.29 (16-D)**.

Disposition of history (per §11 of the reconciliation brief):

- `src/g6.rs` is **deliberately left untouched**, so the committed
  `fixtures/g6/REPORT.txt` keeps matching the code that produced it and the
  G-6 audit trail stays intact. The historical measurement is hereby labelled
  **historical/asymmetric**.
- The corrected instrument is new code (`src/g6b.rs`,
  `compare_storage_path`), which measures the symmetric regime and carries the
  mixed regime alongside so the correction is visible, not silent.
- No historical G-6 number is edited, regenerated, or reinterpreted.

## 5. Numerical fidelity methodology (category A)

**Numerical statements bound how far a score can move. They never decide that
a moved score does not matter.** That separation is the central correction of
G-6B and is enforced in the instrument: the numerical ceiling
(`NUMERICAL_CEILING = 2^-10`) is computed and printed as context, and no code
path uses it to suppress, exempt, or filter a retrieval event.

### 5.1 Bounds retained, with their assumptions stated

All bounds are conditional on the profile preconditions of §9 (unit L2 norm;
negligible subnormal energy). Under those preconditions:

| Bound | Statement | Status |
| --- | --- | --- |
| Component error | `\|Δ\| ≤ 2^-11·\|v\|` for components in the binary16 normal range | proven; measured tight to 0.12 % |
| Vector error | `\|u − u'\| ≤ 2^-11` for unit `u` | proven, dimension-free |
| Cosine error | `\|Δcos\| ≤ 2^-10`, sharply `≈ 2^-10·sin θ` | proven; ~50–65× conservative for dense vectors |
| Pair difference | `\|Δ(cos_a − cos_b)\| ≤ 2^-9`, sharply `2^-11(2\|a−b\| + sinθ_a + sinθ_b)` | proven |
| Norm error | `\|\|v'\| − 1\| ≤ 2^-10` | proven |

**`2^-9` is retained as a numerical ceiling only.** It is a valid worst-case
bound on how far a pairwise score difference can move; it is *not* a
definition of "the reference cannot resolve this ordering", because the f32
reference resolves score differences down to its own granularity
(`2^-24 ≈ 5.96e-8` near 1.0 — `eval::cosine` returns f32), which is ~470×
below the binary16 error at 1280-D. The derived ceilings are reported next to
every retrieval event as context, never as permission.

### 5.2 What category A gates

The bounds above are theorems given the preconditions, so gating on them costs
nothing and catches precondition violations and instrument bugs. The gates are
B-A1 and B-A2 (§11). Everything else in category A — means, distributions,
Spearman — is reported, not gated, because no defensible pre-derivation exists
for their thresholds (Appendix A, finding 9).

## 6. Retrieval fidelity methodology (category B)

**The primary question is asked directly: does replacing the stored f32
representation with binary16 change the retrieval result?** It is never
answered by inference from a numerical bound.

For every seed track, in both worlds (f32 reference vs binary16 storage path,
identical `eval::nearest` ranking procedure, identical index tie-break):

- **top-1 identity** — the presented most-similar track;
- **ordered top-k sequence identity** for every k in §7 — the full ordered
  list the API would return, because ADR 0017 §5.8 responses carry `rank`, so
  an intra-list reorder is user-visible exactly as a membership change is;
- **entry/exit at each boundary** — which candidate crossed each k-th
  boundary, reported per event;
- **reversal census** — *every* pair whose relative order differs between the
  worlds, recorded unconditionally with its reference margin (in cosine units
  and in f32 score ULPs) and both ranks. **No exemption floor exists in the
  instrument.** A reversal with a 1-ULP margin and a reversal with a 0.5 margin
  are both recorded; both are facts;
- **new exact ties** introduced by quantization;
- **boundary margins and populations** — the reference score gap at each k-th
  boundary and how many candidates sit within the numerical ceiling of it,
  explicitly labelled context. These describe how *close* the corpus's
  decisions are; they never classify a change as acceptable.

### 6.1 The verified counterexample this methodology must survive

The independent review's central counterexample was reproduced deterministically
and is committed as code (`g6b::ulp_flip_case`, asserted by
`g6b_tests::the_f32_ulp_ordering_flip_is_reproduced_deterministically`):

```text
dimension=1280  reference_margin=2.742e-6 = 46.0 f32 ULPs
reference_top1=t01  stored_top1=t02
margin is 712x inside the old 2^-9 floor
```

The f32 reference distinguishes the two candidates (46 ULPs ≫ 1); binary16
storage reverses them; the *presented top-1 changes*. Under the G-6A criteria
this event is exempt (margin ≪ 2^-9) and invisible (band set unchanged). Under
G-6B it is a recorded retrieval change and a B-B1 violation.

**The principle encoded: numerical indistinguishability does not imply product
equivalence.** Whether two near-tied candidates may be freely interchanged is
a product semantics question (D-2), not an arithmetic fact, and until the
product answers it, a changed result is a changed result.

### 6.2 What the instrument check already surfaced

Running the corrected instrument over the unchanged surrogate corpora
(`fixtures/g6b/REPORT.txt` — an instrument check, not criteria evidence, §14)
found, at 512-D, one seed whose **ordered top-12 and top-20 sequences differ**
between the worlds (a rank-11/12 boundary crossing, reference margin
2.618e-5 ≈ 439 f32 ULPs). The historical experiment could not see this: it
evaluated k ∈ {1,5,10} only, and its displacement metric was measured inside
the top-10. The event was present in the committed G-6 data all along (it is
one of the four recorded 512-D swap margins) — unexamined, because no criterion
looked there. This is reported as validation of the methodology's coverage, and
as a demonstration of why the same-data prohibition (§14) is necessary.

## 7. Product retrieval boundaries

ADR 0017 §5.8 sketches:

```http
GET /api/v1/tracks/{id}/similar?limit=20
GET /api/v1/albums/{id}/similar?limit=12
```

G-6B therefore evaluates **k ∈ {1, 5, 10, 12, 20}**: the historical series
(1, 5, 10) for continuity, plus both ADR-sketched limits (12 albums, 20
tracks). Album-level retrieval is evaluated on the server-derived aggregate at
k = 12. The final limit set is **not finalized** — that is product decision
D-3; if the product settles on different limits, that is a methodology
revision recorded before any rerun (§14), not a silent edit.

## 8. Near-tie treatment

G-6B distinguishes exactly four statements and never confuses them:

1. **f32 ordering** — which candidate the f32 reference ranks first;
2. **f16 ordering** — which candidate the storage path ranks first;
3. **whether the user-visible result changes** — measured directly (B-B1,
   B-B2, and the census);
4. **whether the product considers either result equivalent** — a product
   semantics decision that **does not exist today**.

Statement 4 is never inferred from 1–3. Because the product currently defines
no notion of interchangeable results, a changed top-1 **is** a retrieval
change, is recorded as one, and violates B-B1. If the product later defines
equivalence semantics (e.g. "results within ε of the boundary score may be
returned in either order"), that definition relaxes B-B1/B-B2 by an explicit,
product-owned rule — decided and recorded **before** any rerun, with the
relaxation's user-visible consequences stated (caching, generated playlists,
deterministic re-serving). The experiment cannot grant itself that relaxation,
at any precision, for any candidate pair.

Determinism inside exact ties: the experiment inherits `eval::nearest`'s index
tie-break as its deterministic rule and records every tie. Which key the
*server* should use inside a tie band is architecture decision D-4.

## 9. Profile safety requirements (category C)

The genuine G-6 finding retained: binary16 behaves badly in the subnormal
regime (a constructed unnormalized case moved a cosine by 7.954e-2; below
`2^-14`, precision degrades linearly to nothing). G-6B therefore requires, of
any profile that declares `f16le`, checked on the profile's reference vectors:

- **B-C1 — L2 normalization.** Stored vectors are unit L2 norm within `2^-10`.
  This is the load-bearing requirement: it places components at scale
  `1/√d`, which for any plausible embedding dimension sits many binades above
  the subnormal knee (`d = 1280` → components ≈ `2^-5.2`, ~8.8 binades clear;
  the knee is reached only at `d ≈ 2^28`, and margins thin materially below
  ~`2^20`).
- **B-C2 — negligible subnormal energy.** The fraction φ of a vector's energy
  in components below `2^-14` is at most `2^-24`. Derivation: subnormal
  components contribute an angular perturbation bounded by their L2 mass
  `√φ` (plus at most `2^-25` per component); requiring this to be at most
  *half* the normal-range error budget `2^-11` gives `√φ ≤ 2^-12`, i.e.
  `φ ≤ 2^-24 ≈ 5.96e-8`. (The composition argument is corrected in §18 R-2:
  the operative bound is the subnormal half-step times MAX_DIMENSIONS, not
  the half-budget sentence; the threshold is unchanged.)
- **B-C3 — no conversion overflow.** Every component of every reference
  vector must be representable in binary16. The §7.4 conversion rejects
  unrepresentable values; a profile that produces any is ineligible, full
  stop. (The real-corpus runner must *count* overflows, not panic — a noted
  requirement of §13.)
- **Recorded alongside (report only):** the support size / participation ratio
  of the reference vectors. Concentrated vectors (support 2–8 at 1280-D)
  measured 3.5–7.5× the dense-vector error while still passing the bounds; no
  gate currently distinguishes them, so the profile review must see the number.
  Exact zeros are harmless (zero is exact in binary16) — which is why the
  requirement is an energy fraction, not a per-component floor.

These are **profile** requirements, not container requirements: the format
specifies the conversion exactly; whether a profile's numbers are safe in
binary16 is the registry's job. Whether these requirements enter profile
identity as a fingerprint tag is deferred architectural decision D-5 — no
format change is made now.

No universal claim is made from the 1280-D Discogs-EffNet corpus: the eventual
rule is per-profile (D-6).

## 10. Exact metrics

Instrument: `src/g6b.rs` (`compare_storage_path`, `ulp_flip_case`,
`render_report`), runner `src/bin/g6b.rs`. All metrics listed per corpus
(model variant × patch hop × dimension):

| # | Metric | Regime |
| --- | --- | --- |
| M1 | `max abs(Δcos)` over all ordered pairs | symmetric storage path |
| M2 | mixed-regime `max abs(Δcos)` | carried for continuity with the historical artefact |
| M3 | mean and quantiles of `abs(Δcos)` | symmetric |
| M4 | min cosine, both worlds; sign crossings | symmetric |
| M5 | `max abs(‖v'‖ − 1)` | stored vectors |
| M6 | one-sided max promotion / demotion `(cos_f16 − cos_f32)` | symmetric |
| M7 | Spearman ρ over pairs, **self-pair diagonal excluded** | symmetric |
| M8 | top-1 identity per seed | retrieval |
| M9 | ordered top-k sequence identity per seed, k ∈ {1,5,10,12,20} (tracks) and k = 12 (album aggregates) | retrieval |
| M10 | entry/exit events per k-boundary, with identities | retrieval |
| M11 | reversal census: every order flip with reference margin (cosine and f32 ULPs) and both ranks | retrieval |
| M12 | new exact ties | retrieval |
| M13 | boundary margin (last-in minus first-out score) and population within the numerical ceiling, per k | context |
| M14 | `cos(aggregate_f32, aggregate_f16)` min/mean | report only |
| M15 | subnormal energy fraction and support/participation per vector | profile |
| M16 | conversion overflow count | profile |

## 11. Proposed acceptance criteria

| ID | Category | Metric | Threshold | Rationale | Source | Frozen before run? |
| -- | -------- | ------ | --------- | --------- | ------ | ------------------ |
| **B-A1** | A — numerical | M1 `max abs(Δcos)`, symmetric storage path, all ordered pairs, every corpus | **≤ 2^-10 = 9.766e-4** | Theorem given §9 preconditions (component 2^-11 → angular bound 2^-10). A violation implies a precondition breach or instrument bug, so the gate is free. | binary16 structure; re-derived independently by both reviews | **YES** |
| **B-A2** | A — numerical | M5 `max abs(‖v'‖ − 1)` | **≤ 2^-10** | Theorem: `2 × 2^-11` on a unit vector. | binary16 structure | **YES** |
| **B-B1** | B — retrieval | M8 top-1 identity | **exact identity, every seed, every corpus** | The product defines no interchangeability of near-tied results, so a changed top-1 is a user-visible change. Threshold is definitional (a property), not statistical. Relaxable only by D-2. | ADR 0017 §5.8 (query shape); §8 of this document | **YES** |
| **B-B2** | B — retrieval | M9 ordered top-k sequence identity | **exact identity for k ∈ {1,5,10,12,20} (tracks) and k = 12 (albums), every seed, every corpus** | Responses carry `rank`, so intra-list order is user-visible; set-identity alone is insufficient. k-set from ADR 0017 §5.8 sketch (D-3). Definitional threshold. | ADR 0017 §5.8; verified k-coverage gap (§6.2) | **YES** |
| **B-C1** | C — profile | M15 L2 norm conformance | **`\|‖v‖ − 1\| ≤ 2^-10` for every reference vector** | Load-bearing safety requirement; places components at `1/√d`, clear of the subnormal knee. | FORMAT_SPEC §7.3 precondition; G-6 subnormal evidence | **YES** |
| **B-C2** | C — profile | M15 subnormal energy fraction φ | **≤ 2^-24 per vector** | Derived in §9 (subnormal perturbation ≤ half the normal-range budget). | binary16 structure | **YES** |
| **B-C3** | C — profile | M16 conversion overflow count | **0** | The specified conversion rejects unrepresentable values; a profile producing any is ineligible. | FORMAT_SPEC §7.4 | **YES** |

**Report-only quantities (NO THRESHOLD — REPORT ONLY):** M2 (continuity),
M3, M4, M6 (becomes the τ instrument under D-1), M7, M10–M13 (retrieval event
detail and context), M14, M15's support/participation record. No threshold is
stated for any of them because none can be justified before seeing real-corpus
results — and an unjustified threshold is how G-6 arrived at C2 and C6.

**Predeclared verdict rule for the final re-run:**

- **PASS** — every gate green on **every** real corpus evaluated
  (both model variants × both patch hops, §13), with corpus provenance
  recorded, and with D-1…D-6 each either resolved or explicitly recorded as
  non-blocking with rationale.
- **FAIL** — any gate violated on any corpus.
- **INCONCLUSIVE** — the real corpus cannot be evaluated (unavailable,
  unreadable, or the instrument detects its own failure).
- A gate that cannot be evaluated (e.g. an empty candidate set) is recorded as
  **not evaluated**, never as passed.

## 12. Threshold provenance

| Threshold | Kind | Derivation | Could an independent party derive it without seeing results? |
| --- | --- | --- | --- |
| 2^-10 (B-A1, B-A2, B-C1 tolerance) | mathematically derived | binary16 has 11 significand bits; RNE half-ulp is 2^-11 per component; unit-vector and angular bounds double it | **yes** — both reviews derived it independently; measured tight/conservative |
| 2^-24 (B-C2) | mathematically derived | §9: `√φ ≤ 2^-12` = half the normal-range budget 2^-11 | **yes** — the G-6A statement of this derivation was inconsistent (goal stated as 2^-11, bound used 2^-12); G-6B states the actual rule |
| exact identity (B-B1, B-B2) | definitional | the product has no interchangeability semantics; a changed result is a changed result | **yes** — it is a property, not a statistic; nothing to tune |
| 0 (B-C3) | definitional | the specified conversion rejects unrepresentable inputs | **yes** |
| k ∈ {1,5,10,12,20} | inherited + architecture | 1/5/10 continue the historical series; 12/20 are the ADR 0017 §5.8 sketched limits | **yes**, from ADR 0017; final set is D-3 |
| 2^-9, 2^-24-context, f32 ULP | context only | numerical ceilings and the reference's own granularity; printed beside events, never used to exempt | n/a — deliberately not thresholds |

**Dropped because underivable:** R-A2's `2^-13` (its stated rationale —
"should land near or below" — is an expectation, not a bound) and R-B5's
`1 − 2^-10` (no derivation; measured identically 1.0 in every configuration
tested, including adversarial member cancellation — quantization error is an
odd componentwise function of the value, so value cancellation implies error
cancellation).

## 13. Real-corpus requirements

**Data.** The recorded spike wrote, per run: `embeddings.json`
(`{metadata, tracks[{artist, album, title, path, duration_seconds,
source_sha256, frame_count, patch_count, embedding_sha256, embedding: Vec<f32>}],
albums[...]}`) and `neighbors.json` (`src/report.rs`). Required inputs:

- **Four corpora**: model variants {`multi` 1280-D, `release` 512-D} × patch
  hops {61, 62}, each run's `embeddings.json`, over the recorded 45-track /
  15-album library.
- Each file's `metadata` block (model, model_sha256, corpus_identity,
  corpus_root_sha256, runtime) is the provenance record and must be carried
  into the result.
- The per-track `embedding_sha256` values must be re-verified against the
  vectors before measurement, so the corpus identity is established, not
  assumed.

**Process.** Pure deterministic post-processing: read f32 vectors → apply the
storage path (§4) → measure §10. No model download, no audio decoding, no
inference, no Python, no network. If the recorded outputs cannot be recovered,
the minimum recreation is: one re-run of the existing spike over the same
45-track library for each of the four configurations — a model-and-audio
operation that must be justified on its own, not slipped in as an experiment
convenience.

**Known gaps to carry into the run:** 44 candidates per query makes boundary
populations small; the same-artist/different-album stratum is structurally
empty (one album per artist in the recorded corpus, ADR 0017 §2.2); the corpus
is one model family at one patch level, so the conclusion is per-profile
admissibility (D-6), never a universal claim; the runner must count conversion
overflows rather than panic on them.

**The surrogate is not a substitute** for any of this (§14).

## 14. Frozen-before-run rule (mandatory)

1. **The acceptance criteria in §11 are frozen as of this document's review
   acceptance.** The load-bearing thresholds are either mathematical theorems
   or definitional properties; none was chosen from, or fitted to, any
   observed result — the surrogate's numbers appear nowhere in §11 or §12.
2. **The prohibited sequence is:** run → observe failures → change threshold →
   rerun → declare pass. If a criterion proves *malformed* (measuring
   something other than intended — as C2 and C6 were), that is a **new
   methodology revision** (a G-6C document), recorded and reviewed **before**
   any rerun, with the malformation demonstrated. Thresholds are never edited
   in place; criteria are never removed because they failed.
3. **Same-data prohibition.** The G-6A repair was motivated by the surrogate
   corpus, therefore **the G-6B criteria may never be evaluated against that
   surrogate**. `fixtures/g6b/REPORT.txt` is an *instrument check* — it exists
   to show that the corrected instrument measures what it claims (and it does:
   its mixed column reproduces the historical artefact exactly, and its
   symmetric column shows the documented correction). It is not criteria
   evidence and no verdict may cite it.
4. The audit trail G-6 → G-6A → independent reviews → G-6B is preserved
   byte-for-byte; the historical FAIL is part of the record, not an
   embarrassment to be overwritten.

## 15. Known limitations

- **Corpus size and strata.** 45 tracks, one album per artist; boundary
  populations will be small; some gates may be *weakly exercised* — reported
  as such (a gate over an empty boundary is "not evaluated", not "passed").
- **One model family.** The run speaks to this profile (D-6). Concentrated or
  sparse future profiles need their own B-C conformance numbers (§9).
- **Aggregate semantics.** Album aggregates are server-derived and deferred
  from v1.0 documents; M14 is report-only and measured 1.0 everywhere tested.
- **Tie-break inheritance.** The experiment's deterministic order inherits
  `eval::nearest`'s index tie-break; the server's production rule is D-4 and
  may differ — a recorded dependency, not a silently shared assumption.
- **Instrument check ≠ validation.** The surrogate artefact validates the
  instrument, not the criteria, not the corpus, not f16.
- **Near-tie strictness.** B-B1/B-B2 identity may fail on near ties that a
  future product semantics would call interchangeable. That failure is
  meaningful information under the current (absent) semantics; the remedy is
  D-2, decided before a rerun — never a threshold edit after one.
- **Readiness-review findings.** Four findings from the final readiness
  review — the B-C2 budget composition (§18 R-2), the deferred album
  semantics (§18 R-3), the sub-f32-ULP ordering boundary under a future
  server procedure (§18 R-4), and corpus scaling (§18 R-5) — sharpen the
  limitations above without altering any criterion.

## 16. Open product decisions

| # | Decision | Owner | Why it cannot be decided here |
| --- | --- | --- | --- |
| **D-1** | Relevance threshold τ: what counts as "similar enough to be a neighbour" | product (ADR 0016 Slice 0) | No user story, no consumer, no ground truth. Until τ exists, M6 is report-only; with τ, the one-sided criterion is "no score within the established error bound may cross τ" — already instrumented. |
| **D-2** | Near-tie interchangeability: may results within a band be freely interchanged in presentations, caches, playlists? | product | A relaxation of B-B1/B-B2 with user-visible consequences (determinism, re-serving, generated playlists). Must be decided before any rerun if it is to apply. |
| **D-3** | Final API result limits (the k set) | product | ADR 0017 §5.8 is a sketch. G-6B evaluates {1,5,10,12,20}; a different final set is a recorded methodology revision. |
| **D-4** | The server's deterministic ordering key inside a tie band | architecture (ADR 0017 §5.7) | Recommended: a content-defined key (lowest `(package_id, asset_id)`), not score-then-index, so band-internal order is reproducible and not an implementation accident. |
| **D-5** | Do B-C1…B-C3 enter profile identity (a new fingerprint tag)? | format owner | ADR 0017's tag list is closed; a new tag changes every profile fingerprint and needs its own sign-off. No format change is made now. |
| **D-6** | Scope of the eventual decision | product + format owner | Recommended: "f16le is admissible *for a profile* that passes category C and whose reference corpus passes categories A and B" — never "f16 is safe". |

## 17. Required evidence for final G-6 sign-off

1. The four real corpora's `embeddings.json` with verified digests and their
   metadata blocks (§13).
2. A G-6B result artefact per corpus: all M1–M16, produced by the committed
   instrument, byte-deterministic, carrying `profile_fingerprint`, model
   variant, patch hop and corpus identity digest.
3. The gate table (B-A1, B-A2, B-B1, B-B2, B-C1, B-C2, B-C3) evaluated per
   corpus, with any "not evaluated" gates and their reasons.
4. Dispositions of D-1…D-6: resolved, or explicitly non-blocking with
   rationale.
5. Confirmation that §14's freeze held: the criteria text compared
   byte-for-byte against this document at run time.
6. Human review of the full reversal census — every event, no exemptions —
   with the product's explicit accept/reject of each user-visible change
   class. This, not a threshold, is where "acceptable" is decided.
7. Only then: a revision of the G-6 gate status. Until then, and regardless of
   any intermediate result: **`G-6: FAIL — KEEP F32`**.

---

## 18. Readiness-review findings (Qwen, 2026-09-27) — recorded before the real run

The final independent readiness review
([`G6B_QWEN_REVIEW.md`](G6B_QWEN_REVIEW.md)) returned **READY WITH IMPORTANT
NOTES — no blocker**. This section records its findings and their dispositions
in one place. It changes **no criterion, no threshold, and no verdict rule**:
the §11 table above is byte-identical to the version the review audited
(enforced at run time by `g6b::check_criteria_freeze`, §17.5) and remains
frozen (§14). The corrections below are documentation and instrumentation
only, made **before** any real-corpus measurement exists.

### R-1 (review finding I-1) — the gated instrumentation is complete

The gated quantities the review found missing are implemented in the
experiment crate, with tests:

- **B-C1** (`g6b::norm_deviation`, `g6b::measure_profile_safety`) —
  `|‖v‖ − 1|` per reference vector, gated at 2^-10 exactly as §11 states.
- **B-C2** (`g6b::subnormal_energy_fraction`) — per-reference-vector energy
  fraction φ below 2^-14, gated at 2^-24 exactly as §9/§11 state; support
  size and participation ratio are recorded alongside, report only (§9).
- **B-C3** (`g6b::encode_storage_path`, `g6b::encode_corpus_storage_path`) —
  representability is *measured*, never panicked: every rejected component is
  recorded with its vector index, component index, value and reason
  (non-finite vs. magnitude overflow). A corpus with any rejection has no f16
  storage representation, so the symmetric comparison is not fabricated:
  B-C3 fails and B-A*/B-B* are recorded **not evaluated** (§11's rule) — a
  configuration that can never produce a PASS.
- **B-B2 album component** (`g6b::compare_album_retrieval`) — album
  aggregates are recomputed *independently in each world* from that world's
  own stored vectors (reference vectors → reference aggregates → reference
  ranking; f16-decoded vectors → f16-world aggregates → f16 ranking) and
  compared as ordered sequences at k = 12, the §7 album limit. A run output's
  recorded `albums[]` block is never deserialized (see R-3).
- **Gate evaluation** (`g6b::evaluate_gates`, `verdict_of_gates`,
  `overall_verdict`) — the seven frozen gates and nothing else, PASS / FAIL /
  NOT EVALUATED per gate and PASS / FAIL / INCONCLUSIVE per run, per §11:
  PASS requires every gate evaluated and passing on every corpus;
  anything not evaluated can never yield PASS. The real-corpus runner
  (`src/g6b_real.rs`, `src/bin/g6b_real.rs`) adds the §13 provenance
  requirements — per-track `embedding_sha256` re-verification before any
  measurement — and the §17.5 freeze check at run time. A failed freeze
  aborts the run as a protocol failure instead of producing a verdict.

The gate machinery is deliberately **never** invoked on the surrogate (§14
rule 3): the surrogate artefact remains measurements-only, and the gate
machinery tests in `g6b_tests` run on small hand-built synthetic corpora.

### R-2 (review finding I-2) — B-C2 derivation corrected; threshold unchanged

§9's derivation sentence — subnormal mass √φ ≤ 2^-12 as "half the
normal-range error budget 2^-11" — does not compose as written: a half-budget
term *added* to the full normal-range bound yields 1.5 × 2^-11, not ≤ 2^-11,
and §5.1's "dimension-free" label on |u − u'| ≤ 2^-11 is literally true only
for strictly normal-range vectors. The operative argument is the one §9's
parenthesis already contains:

- a component in the binary16 subnormal range (|v_i| < 2^-14) is rounded on
  the 2^-24 grid, so its error is at most half a step, **2^-25 absolute**
  (and never more than |v_i| itself);
- the format caps dimensions at **MAX_DIMENSIONS = 4096** (FORMAT_SPEC
  decision C), so the total subnormal error energy is at most
  4096 · 2^-50 = 2^-38;
- hence ‖u − u'‖² ≤ 2^-22·(1 − φ) + 2^-38 ≤ 2^-22·(1 + 2^-16), i.e.
  ‖u − u'‖ ≤ 2^-11·(1 + 2^-17), and after the norm division in the cosine,
  **|Δcos| ≤ 2^-10 · (1 + 5×10^-4)** — an absolute excess below 5×10^-7 over
  the §11 constant, and the vector bound holds for every dimension the
  format admits.

**The B-C2 threshold stays 2^-24.** At bounded dimension it is far stricter
than this argument requires; it is retained deliberately as a conservative
profile tripwire. §9's "half the budget" sentence is to be read through this
R-2 note; the freeze discipline (§14) is why the correction is appended here
rather than rewritten into §9.

### R-3 (review finding I-3) — album semantics are deferred; G-6B uses only defined semantics

Album-level similarity depends on aggregation semantics that the format work
deliberately deferred: ADR 0017 §5.5.1 decision B removed the album aggregate
from v1.0 and states none are to be invented. G-6B therefore uses only
*already-specified* semantics: the mean-of-L2-normalized-members-then-L2 rule
that ADR 0017 §5.5 recorded as the candidate for fingerprint tag 12 and that
ADR 0017 §5.7 assigns to the server ("the album row is a server-side
derivation from the track rows"). `eval::aggregate_albums` implements exactly
that rule; no new aggregation behaviour is introduced. The experiment
recomputes aggregates **independently in each world** — never copying an
aggregate between worlds and never reading a run output's recorded
`albums[]` block, which is the recording pipeline's f32 aggregate (mixing it
would recreate the mixed-regime defect §4.1 corrected, one level up; the
loader in `src/g6b_real.rs` deliberately does not deserialize that block).

Interpretation rule: an album-component result is evidence about the storage
path under the documented candidate rule. It must **not** be read as a v1.0
product decision while the product's aggregation semantics remain deferred
(D-3 as amended), and an album-only failure must be attributed to that
deferred path when the verdict is written up.

### R-4 (review finding I-4) — "presented result" is defined by the declared ordering procedure

B-B1/B-B2 measure the presented result under the procedure both worlds
share: `eval::cosine` (f64 accumulation, f32 score) with `eval::nearest`'s
deterministic index tie-break. Within that procedure any presented-list
difference is detected: equal f32 scores imply the index tie-break, which
cannot differ between worlds, and unequal scores order the list exactly as
the API would return it. Two boundaries of that definition are recorded:

1. **Sub-f32-ULP ordering.** If a future server ranked *finer* than the f32
   score it reports (f64 ordering under fingerprint tag 14), a pair whose f64
   orders oppose between the worlds while both worlds' f32 scores tie would
   present identically here and differently there, at margins below ~1 f32
   ULP (≈6×10^-8 near 1.0). M12 does not capture this class: its counter
   skips pairs whose reference scores are already equal. This matters only
   if a future server changes its ordering semantics (D-4 / tag 14); until
   one exists, the declared procedure is the only defined semantics, and
   G-6B does not change it.
2. **New exact ties.** Quantization can create f32 ties where the reference
   distinguished; these are counted (M12) and, under the current procedure,
   resolved deterministically and identically in both worlds. A future D-4
   content key would need to re-adjudicate exactly those recorded bands
   before adoption.

### R-5 (review finding I-5) — corpus scaling is an extrapolation, not a claim

The exact-identity gates are evaluated exhaustively on the recorded
45-track / 15-album reference corpus per configuration; there is no sampling
and no statistical tolerance. A PASS therefore establishes "no measured
retrieval change on the tested corpus", **never** "no change at any
collection size": boundary populations grow with the candidate pool, and
rank-boundary flips that cannot occur among 44 candidates may occur in
larger libraries. The M13 boundary-margin and boundary-population
measurements are retained as the supporting evidence for that extrapolation;
no confidence interval is invented because the methodology calls for none.
The conclusion remains profile-specific (D-6): admissibility for *this*
profile whose *reference corpus* passed categories A and B — never a
universal claim.

---

## Appendix A — Reconciliation of the independent review

Every substantive finding in `G6A_GLM_REVIEW.md`, classified. "Accepted"
means addressed in this methodology or its instrument; nothing was implemented
on the review's authority alone.

| # | Finding | Classification | Disposition |
| --- | --- | --- | --- |
| 1 | Mixed-regime measurement defect (`g6.rs:489-491` quantizes one operand) | **ACCEPT** (verified independently: the mixed column of the new instrument reproduces the committed artefact exactly; symmetric exceeds it 1.52×/1.53×/1.29×) | Corrected in new code (`src/g6b.rs`); `g6.rs` untouched to preserve the audit trail; historical measurement labelled asymmetric (§4.1) |
| 2 | `2^-9` is a valid bound but an invalid product-equivalence definition; the f32-resolves/f16-flips window is ~470× | **ACCEPT** — with one refinement: rather than replacing the floor with a better floor, G-6B removes floors from retrieval entirely (§5, §6); ceilings survive as context |
| 3 | `k ∈ {1,5,10}` misses ADR §5.8's `limit=20`/`limit=12` | **ACCEPT** (verified from ADR 0017 §5.8) | k ∈ {1,5,10,12,20}; final set is D-3 |
| 4 | Presented-answer gap: band-set identity constrains the set, not the answer; index tie-break becomes load-bearing | **ACCEPT** (measurement half) / **DEFER** (architecture half) | Ordered-sequence identity gates B-B1/B-B2; tie census recorded; the server's ordering key is D-4 |
| 5 | Same-data re-pass hazard: the revised criteria pass on the data that produced the FAIL | **ACCEPT** | §14 rule 3, enforced by artefact labelling and tests |
| 6 | Redundancy: R-A3 ⊆ R-A1; R-B1 implied-or-vacuous; R-B3 undefined-or-redundant; R-B4 ≡ R-B2; R-B5 always 1.0 | **ACCEPT** (the implications are provable; R-B5's vacuity was re-measured and strengthened: RNE is an odd function of the value, so value cancellation implies error cancellation) | Pruned per Appendix B; the gate set is seven independent statements |
| 7 | Bounds carry an unstated normal-range precondition; "Corpus: any" overstates | **ACCEPT** | §5.1 states assumptions; §9 states preconditions; B-A1's violation semantics defined |
| 8 | §6.1's "constant 1.149e-4" is a single-mantissa artifact (true worst case 2^-11, 4.25× larger); §6.3's 2^-12-vs-2^-11 inconsistency | **ACCEPT** | §12 records the corrected provenance; the historical documents are not edited |
| 9 | `2^-13` (R-A2) is underived | **ACCEPT** | Dropped; mean movement is report-only (M3). No replacement threshold invented |
| 10 | R-C1 lacks a sparsity dimension; `2^-28` framing overstates margin | **ACCEPT** | Support/participation recorded (M15); margin framing in §9 |
| 11 | Real-corpus protocol needs band populations, both variants, both patch hops, fingerprints | **ACCEPT** | §13 |
| 12 | C2 mislabelled "model quality"; it was a quantization criterion expressed as an absolute level | **ACCEPT** | Appendix B; property (i) = M6 report + τ instrument under D-1; property (ii) = D-1; not filed under model validation |
| 13 | 45 % flip rate at 1–10 ULP margins; 12,625 constructed top-1 flips | **INVESTIGATE → ACCEPT** | Reproduced deterministically and committed (`ulp_flip_case`, §6.1): 46-ULP margin, 712× inside the old floor, presented top-1 changes |
| 14 | τ-crossings unconstrained by pairwise floors (12,395/80,000 measured) | **ACCEPT** (analytic: `\|Δcos\| ≤ ε > 0` admits crossings of any fixed threshold) | §8/D-1: with τ, the instrument is M6's one-sided bound at τ |
| 15 | Spearman includes self-pair diagonal | **ACCEPT** (minor) | M7 excludes the diagonal |
| 16 | Surrogate cannot establish retrieval fidelity (CLT predictability + margin-distribution mismatch) | **ACCEPT** | §13, §14 rule 3; surrogate used as instrument check only |
| 17 | Historical FAIL was driven by the two criteria G-6A reclassified; G-6's own rule expected INCONCLUSIVE | **ACCEPT as history** | Recorded (§1, §14 rule 4); the verdict is not relitigated and stands |
| 18 | Recommendation to adopt the cosine-dependent floor `2^-10·sin θ` or measured `2ε` as the operational band | **REJECT as stated** — the remedy still treats a numerical quantity as a retrieval-equivalence definition, which is the very conflation under repair | Ceilings are context (§5.1); retrieval is measured floor-free (§6). The sharper bounds are retained in §5.1 as mathematics |
| 19 | R-B5 worst-case cancellation could exceed `1 − 2^-10` | **ACCEPT the demotion, on a stronger argument** | The review's cancellation scenario is even more benign than argued (odd-symmetry of RNE); M14 is report-only regardless |

## Appendix B — Disposition of every prior criterion

| Prior | Disposition | Successor |
| --- | --- | --- |
| C1 `max abs(Δcos) ≤ 1e-3` | replace (threshold was undocumented; regime was asymmetric) | **B-A1** (symmetric, derived 2^-10) |
| C2 `min cosine f16 ≥ 0.99` | remove as **void as written** (the f32 reference fails it identically; it was a quantization criterion expressed as an absolute level) | property (i) → **M6** report + τ instrument under **D-1**; property (ii) → **D-1** |
| C3 `Spearman ≥ 0.999` | demote to diagnostic (cannot separate noise-floor reordering from retrieval change; diagonal inflated) | **M7** (report only, diagonal excluded) |
| C4 `top-10 Jaccard ≥ 0.98` | replace (a budget admits changes; set-identity misses order) | **B-B2** (exact ordered-sequence identity) |
| C5 `top-1 unchanged ≥ 90 %` | replace (a 10 % budget for the single most visible result is unjustifiable a priori) | **B-B1** (exact identity) |
| C6 `swaps ≤ 0.1 %` | replace (universe-wide budget exempted resolvable flips) | **M11 census** (no budget, no exemption) + B-B1/B-B2 |
| C7 `top-5 Jaccard ≥ 0.97` | replace | **B-B2** (k = 5) |
| C8 ≡ C5 | remove (duplicate, as G-6A found) | — |
| C9 `max displacement ≤ 10` | replace (subsumed: ordered-sequence identity bounds displacement; census records events) | **B-B2** + **M11** |
| C10 `mean abs(Δcos) ≤ 1e-4` | demote (threshold underived) | **M3** (report only) |
| R-A1 | retain, corrected regime, precondition stated | **B-A1** |
| R-A2 (`2^-13`) | remove (underived) | **M3** |
| R-A3 | remove as gate (implied by R-A1); retained as the τ instrument | **M6** + **D-1** |
| R-A4 | retain | **B-A2** |
| R-A5 | retain as diagnostic | **M7** |
| R-B1 | remove as gate (implied by R-A1 under the derived floor; vacuous under the measured floor); its *intent* — no resolvable order silently lost — survives as unconditional recording | **M11** + **B-B1/B-B2** |
| R-B2 | merge (duplicate of R-B4 modulo an undefined word) | **B-B2** |
| R-B3 | remove (its exemption was undefined; sequence identity subsumes it) | **B-B2** + **M11** |
| R-B4 | merge into R-B2's successor | **B-B2** |
| R-B5 | demote (no discriminating power; threshold underived) | **M14** |
| R-C1 | split and state derivations | **B-C1**, **B-C2**, **B-C3** + **M15** sparsity record |
| R-C2 | retain as documentation | §9 references the committed characterization |

**Net:** seven frozen gates (B-A1, B-A2, B-B1, B-B2, B-C1, B-C2, B-C3), each
an independent statement, plus eight report-only metrics and six recorded
product decisions. Every gate's threshold is a theorem or a definition. None
is a number chosen because a result made it look right.
