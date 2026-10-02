# G-6A Independent Review (second reviewer)

> **Role.** Second, fully independent adversarial review of the proposed G-6A
> criteria. Conducted without reliance on any other review document, prior
> conclusion, or conversation. Every quantitative claim below was either
> re-derived from the binary16 specification or measured by a scratch harness
> built for this review against the committed code, then deleted.
>
> **Formal status at review time — unchanged by this document:**
>
> `G-6: FAIL — KEEP F32`
>
> This is a review of *criteria*, not of f16. Nothing here approves f16, and
> nothing here modifies `G6_F16.md`, `G6A_CRITERIA_REVIEW.md`,
> `FORMAT_SPEC.md`, `FORMAT_SIGNOFF.md`, ADR 0017, or any production code.

---

## 1. Executive verdict

**ACCEPT WITH CHANGES.**

The G-6A methodology — separating numerical fidelity (A), retrieval fidelity (B)
and representational safety (C), and deriving thresholds from binary16's
structure instead of from observations — is the right architecture for this
decision, and I found **no error in its mathematics**: I re-derived `2^-11`,
`2^-10` and `2^-9` independently and confirmed each, and measured the component
bound to be tight to within 0.12 %.

Five findings block the real-corpus experiment as currently specified. Two are
about the *instrument*, two about the *floor*, one about the *decision hygiene*:

1. **The committed measurement is not the declared measurement** (§3.5).
   `g6::compare`'s pairwise loop quantizes only one operand of each pair, while
   the declared method (`FORMAT_SPEC.md` §7.3 step 2: compare `(v_i, v_j)`
   against `(q_i, q_j)`) and the actual rankings use both. On the committed
   surrogate the symmetric-regime error is **1.52× / 1.53× / 1.29×** the
   committed values at d = 1280 / 512 / 16. Every headline number G-6A cites as
   "observed ε" is the mixed-regime quantity.
2. **There is a ~470×-wide window in which the f32 reference resolves the order
   and f16 reverses it anyway, and every proposed criterion exempts that
   window** (§4.3, §6). f32's own score granularity is `2^-24 ≈ 5.96e-8`
   (`eval::cosine` returns f32; the resolution-limit probe measured exactly
   `5.960e-8`). f16's symmetric error at 1280-D is `2.78e-5`. In controlled
   trials at margins of 1–10 f32 ULPs, **the ordering flipped 40–45 % of the
   time** — 12,625 user-visible top-1 changes, every one permitted by R-B1
   (margin ≪ `2^-9`), every one invisible to R-B2/R-B4 (band set identical).
3. **`2^-9` is mathematically valid but calibrated to the wrong reference
   point.** ADR 0017 line 64 records that a single model patch hop moves pairwise
   cosines by 2.3e-5…8.94e-4 and changes top-1 in 9/45 seeds; `2^-9 = 1.953e-3`
   sits *above* that entire band. A floor that licenses reordering across
   separations where the model itself is documented to change answers is not a
   product-level safety margin.
4. **The revised criteria would pass on the same surrogate data that produced
   the FAIL** (§5.4). All sixteen R-* criteria are satisfied by the committed
   `REPORT.txt` numbers. The repair is defensible — C2 and C6 genuinely were
   mis-specified — but converting a FAIL into a PASS on identical data is
   exactly the hazard pre-registration exists to prevent, and the document never
   states the resulting prohibition.
5. **At least five of the sixteen criteria are redundant, implied, or vacuous**
   (§13). Provably: R-A3 ⊆ R-A1; R-B1 with the derived floor is *implied by*
   R-A1 and with the measured floor is *vacuous by construction*; R-B4 ≡ R-B2
   modulo an undefined word; R-B5 always reads 1.0 (measured across
   n ∈ {3,10,20} × intra-album cosine ∈ {0.95,…,−0.5}: minimum aggregate
   cosine 1.000000000).

The required changes are in §15. None of them is a reason to keep the old
C1–C10; the old set was worse on every axis these findings measure.

---

## 2. Reconstruction of G-6A (in my own words)

1. **What f16 is proposed to do.** Serve as an alternative *storage* encoding
   (`output_encoding = f16le`, a fingerprint tag-11 value) for per-track
   similarity vectors inside `.msim` documents, halving vector bytes. It is not
   proposed as the compute format: cosine is computed in f64 over f32-widened
   values.
2. **What f32 does today.** `f32le` is the specified, signed-off reference
   encoding. The producing pipeline delivers L2-normalized f32 vectors
   (`eval::pool_mean_norm`: per-window L2 → mean → track L2).
3. **What G-6 is supposed to establish** (FORMAT_SIGNOFF gate table): *"Does f16
   quantization preserve useful ranking, measured rather than assumed."* Not
   perceptual quality (G-7), not corpus quality, not licensing (G-1…G-5).
4. **What the G-6A criteria measure.** A: how far cosines/norms move (R-A1…A5);
   B: whether retrieval results move *above a resolvability floor* (R-B1…B5);
   C: whether a profile's vector distribution is safe for binary16 at all
   (R-C1, R-C2).
5. **What the surrogate establishes.** Arithmetic only: the conversion is
   correctly implemented, errors scale as expected with dimension, subnormals
   fail catastrophically, f32 — not f16 — is the binding resolution at d ≥ 16.
   It does **not** establish retrieval fidelity: see §11.
6. **What the real corpus would establish.** Whether the *margin distribution*
   of a learned embedding interacts with f16's error band at rank boundaries —
   the only question category B can actually answer — plus confirmation that a
   real profile satisfies the category-C preconditions.

**The four properties, kept distinct** (the task is right that these are
routinely conflated):

| Property | Question | Owned by |
| --- | --- | --- |
| Numerical fidelity | how much do numbers move | R-A* (mostly *theorems*, see §13) |
| Retrieval fidelity | does the ranked answer move | R-B* (the only empirical criteria) |
| Product fidelity | does the *user-visible* result move | **partially uncovered** — the band exempts a window f32 resolves (§4.3, §6) and `k` stops at 10 while the API sketch proposes 20/12 (§6 E) |
| Representation safety | are there vectors f16 cannot serve | R-C1 (load-bearing), R-C2 |

Numerical ≡ retrieval is *not* assumed by G-6A — that is its main virtue. But
G-6A does assume retrieval ≡ product, and that assumption is where the floor
problems live.

---

## 3. Binary16 mathematical audit

### 3.1 The conversion implementation

`docfmt::f32_to_f16_bits`: RNE with correct tie-to-even (`remainder > 0x1000 ||
(remainder == 0x1000 && truncated odd)`), correct significand carry at `0x400`,
overflow rejected above `unbiased = 15`, f32 zeros/subnormals correctly mapped
to signed zero (all f32 subnormals are below binary16's `2^-24` subnormal
floor), and a correct subnormal path for binades 14…24. `f16_bits_to_f32` is an
exact widening. **The instrument at the bottom is sound.**

### 3.2 Component bound — correct and tight

For `v` in binade `2^e`, `ulp_f16(v) = 2^(e-10)`; RNE gives
`|Δ| ≤ 2^(e-11)`, so `|Δ|/|v| ≤ 2^-11` **exactly**, attained just above a binade
boundary. Measured: 1,000,000 random mantissas across `e ∈ [−14, 15]` give worst
relative error **4.8769e-4** against `2^-11 = 4.8828e-4` — ratio **0.9988**.
*Assumptions:* value in the normal range; fails for subnormals (relative error
→ 1 as values round to zero — REPORT's own sweep shows `2.533e-8 → 1.000`).

### 3.3 Vector and norm bounds — correct

`|u−u'|² = Σδᵢ² ≤ Σ(2^-11 uᵢ)² = 2^-22` for unit `u`, so `|δ| ≤ 2^-11`,
dimension-free. Norm: measured deviation on the committed surrogate
`2.118e-5 ≤ 2^-10` ✓. *Assumption:* every component in the normal range — see
§8.

### 3.4 Cosine bound — correct, with the non-obvious reason

`|Δcos| ≤ |δu| + |δw| ≤ 2^-10` holds *including* the non-renormalized decode,
because `cos(u',w')` divides by the perturbed norms and the exact statement is
angular: each vector's direction moves by ≤ `arcsin(2^-11)`, the angle between
them by ≤ `2^-10`, and `d cos/dθ = −sin θ`, so **`|Δcos| ≤ 2^-10 · sin θ`** —
the worst case is at `θ = 90°` (cos = 0), *not* at high similarity. Two
corollaries worth recording:

- Near-aligned vectors are intrinsically protected (`sin θ → 0`). If `u ≈ w` and
  the perturbation is along `u`, the errors cancel exactly.
- The deep tail — where G-6 observed all its reversals — is exactly where the
  bound is largest. Bound and observation are coherent.

Empirically the bound is ~65× conservative for dense random vectors (see §4.3),
which is the central limit theorem over `d` components, not luck.

### 3.5 Pair bound — correct, but the *measurement* is of the wrong regime

`|Δ(cos_a − cos_b)| ≤ 2^-10 + 2^-10 = 2^-9`: valid, since `eval::nearest`
compares two *stored* vectors (query quantized too). My refined first-order
derivation (§4.2) gives the sharper form `2^-11(2|a−b| + sinθ_a + sinθ_b)`.

**However — instrument finding (verified).** In `g6::compare`
(`src/g6.rs:489-491`):

```rust
let reference = eval::cosine(&corpus.vectors[a], &corpus.vectors[b]);
let narrowed  = to_f16_candidate(&corpus.vectors[b]);
let candidate = eval::cosine(&corpus.vectors[a], &narrowed);
```

Only `b` is quantized. The declared method (`FORMAT_SPEC.md` §7.3 step 2:
*"(v_i, v_j)" vs "(q_i, q_j)"*) and the rankings (`eval::nearest` over the
candidate set) quantize **both**. Measured on the committed surrogate:

| corpus | committed (mixed) | symmetric (both) | ratio |
| --- | ---: | ---: | ---: |
| 1280-D | 1.8353e-5 (= REPORT) | **2.7827e-5** | 1.52 |
| 512-D | 3.4243e-5 (= REPORT) | **5.2296e-5** | 1.53 |
| 16-D | 1.4007e-4 (= REPORT) | **1.8090e-4** | 1.29 |

So `max_abs_delta_cosine`, `mean_abs_delta_cosine`, `spearman_rho`,
`min_cosine_candidate`, `sign_crossings` and `max_meaningful_relative` in the
committed artefact are mixed-regime numbers, understating the ranking-relevant
error by ~1.3–1.5×. `boundary_cases::measure` has the same asymmetry, while
`resolution_sweep` quantizes both — the module is internally inconsistent.
**No verdict changes** (2.78e-5 ≪ 2^-10 ≪ 1e-3), but G-6A's "observed
ε = 1.835e-5" and its "2ε floor" double a mixed-regime measurement as a
heuristic patch rather than measuring the symmetric quantity. The real-corpus
run must measure `(q_i, q_j)` or the criterion labels are wrong.

### 3.6 Subnormal threshold derivation — internally inconsistent

§6.3 derives `φ ≤ 2^-24` from *"√φ ≤ 2^-12"* but states the goal as *"one
half-ulp relative precision"*, which is `2^-11`, not `2^-12`. One binade of
slack is unexplained. Stricter is safe, but a derivation that does not compute
what it claims is exactly the provenance failure §9 denies.

### 3.7 §6.1's evidence table understates the worst case by 4.25×

`magnitude_precision_sweep` uses fixed `MANTISSA = 1.7`. I confirmed
`1.7·2^e` yields relative error `1.1486e-4` at *every* binade — the "constant
1.149e-4" in REPORT/§6.1 is the rounding behaviour of one mantissa, not a
bound. The worst case over mantissas is `2^-11 = 4.883e-4`, 4.25× larger. The
criteria (R-A1 etc.) use the correct `2^-11`, so nothing gates wrongly — but the
evidence table presented as "the quantitative form" of the safety argument
mischaracterizes the format's precision.

---

## 4. Resolvability-floor audit

### 4.1 Is the concept valid?

Yes, with a precise scope. If the reference cannot distinguish two candidates,
requiring f16 to preserve their order demands reproduction of an ordering that
does not exist. That much is sound and G-6A §5 argues it well. The *scope* is
where it breaks — see §4.3.

### 4.2 Sharper derivation (mine, independent)

First-order in the perturbations, for unit `q, a, b` (writing `m =
|cos(q,a) − cos(q,b)|`, `θ_x` the angle to `q`, `η = 2^-11`):

```
Δ(cos_a − cos_b) = δq·(a − b − m·q) + δa·(q − cos_a·a) − δb·(q − cos_b·b) + O(η²)
|Δ(cos_a − cos_b)| ≤ η·(2|a−b| + sinθ_a + sinθ_b)        [using m ≤ |a−b|]
```

Two consequences the global `2^-9` misses:

- **The query's error mostly cancels** — `δq` enters multiplied by
  `(a − b) − m·q`, which is small for near-tied candidates. The pair comparison
  is dominated by the *candidate* perturbations, each weighted by `sin θ`.
- For near-tied candidates the bound collapses to ≈ `2^-10·sin θ`. Measured
  largest reversing margin (120,000 triples per level, d = 1280, symmetric
  regime):

| reference cosine | sin θ | largest reversing margin | refined bound `2·sinθ·2^-10` | `2^-9` |
| ---: | ---: | ---: | ---: | ---: |
| 0.00 | 1.000 | 2.998e-5 | 1.953e-3 | 1.953e-3 |
| 0.30 | 0.954 | 2.992e-5 | 1.863e-3 | 1.953e-3 |
| 0.60 | 0.800 | 2.730e-5 | 1.563e-3 | 1.953e-3 |
| 0.90 | 0.436 | 1.556e-5 | 8.513e-4 | 1.953e-3 |
| 0.99 | 0.141 | 5.484e-6 | 2.755e-4 | 1.953e-3 |

The empirical ceiling tracks `sin θ` as the derivation predicts, and sits
50–65× below the (never violated) refined bound. **Conclusion: `2^-9` is
mathematically valid and conservative, but cosine-blind — up to 7× looser than
the derived cosine-dependent form in the retrieval region, and ~65× looser than
the measured ceiling.**

### 4.3 The window `2^-9` hides (the decisive finding)

- f32's own score granularity: `eval::cosine` returns f32; the ULP below 1.0 is
  `2^-24 = 5.96e-8`. The committed resolution-limit probe measured exactly
  `5.960e-8`.
- f16's symmetric error at 1280-D: `2.78e-5` — **470× f32's granularity**.

So there is a window `[5.96e-8, 2.78e-5]` in which **the f32 reference genuinely
determines the order** (1–470 ULPs above its own floor) **and f16 reverses it
anyway**. Controlled experiment (d = 1280, candidates at cos ≈ 0.93, second
candidate a tiny multiplicative perturbation of the first so the margin is
set directly):

| perturbation δ | distinct trials | order flips | flip rate |
| ---: | ---: | ---: | ---: |
| 1e-8 … 1e-6 | 0 (perturbation vanishes in f32) | — | — |
| 1e-5 | 11,136 | **5,040** | 45 % |
| 1e-4 | 19,087 | **7,585** | 40 % |

Flip margins were quantized at exactly `5.96e-8` (the f32 ULP). Every flip is
exempt under R-B1 (`m ≪ 2^-9`) and invisible to R-B2/R-B4 (the band set still
contains both candidates). **G6A §5.1's claim — "if `m` is far below the
quantization error, the reference did *not* determine that order" — is false
across this 470× window.** The reference did determine it. What may be true is
that the *model* does not make those candidates meaningfully different — which
is an unmeasured product/model question, not an arithmetic fact, and must not
be presented as one.

### 4.4 Valid vs suitable — the two conclusions separated

- **Mathematically:** `2^-9` is a valid, conservative, corpus-independent
  worst-case floor. Refined form: `2^-11(2|a−b| + sinθ_a + sinθ_b)`; practical
  ceiling ~`3e-5·sin θ` at 1280-D.
- **As a product-level criterion:** unsuitable, for two independent reasons.
  First, the 470× window of §4.3. Second, calibration: ADR 0017 line 64 (patch
  hop 61 vs 62) records cosine drift 2.3e-5…8.94e-4 with **top-1 changed 9/45**
  — the model's own reproducibility band lies *entirely inside* `2^-9`. A floor
  that wide licenses reordering the reference performs on itself.

---

## 5. Retrieval-criteria audit

### 5.1 R-B1 — implied or vacuous, never informative

With the derived floor: a reversal at margin `> 2^-9` requires
`|Δ(cos_a − cos_b)| > 2^-9`, hence some `|Δcos| > 2^-10`, hence R-A1 fails.
**R-A1 ⇒ R-B1.** With the measured floor `2ε` of the same run: no pair can
reverse above twice the run's own observed maximum — **vacuous by
construction**. Either parameterization gives R-B1 zero discriminating power
beyond R-A1. (Also, R-B1's "retrieval-relevant region" is defined as a *union*
of top-k sets, which discards the per-query relation it needs.)

### 5.2 R-B2 / R-B4 — the same criterion twice

R-B2: set within the floor of the k-th reference score, `k ∈ {1,5,10}`,
identical. R-B4: set within the floor of "the boundary score", identical. They
differ only in the undefined word "boundary". If it means `{1,5,10}` they are
duplicates; if it means every rank, R-B4 strictly subsumes R-B2. G6A deleted C8
for precisely this defect class and then introduced another instance.

### 5.3 R-B3 — undefined where it would bite

"Max rank displacement … restricted to the resolvable region": a displacement
is a difference of *ranks*; the floor is a *score* quantity. No bridging rule is
given. Any reading that defines the bridge reduces R-B3 to R-B1 (hence to R-A1).

### 5.4 What the criteria leave unprotected — and the same-data hazard

Unprotected: the *presented* answer inside the band (§4.3's 12,625 flips); rank
movements between 11 and 20 (§6 E); score crossings of any future relevance
threshold (§6 D, and E7 below).

**Same-data re-pass.** Evaluate all sixteen R-* criteria against the committed
surrogate numbers: R-A1 2.78e-5 (or the committed 1.835e-5) ≤ 9.766e-4 ✓;
R-A2 4.743e-6 ≤ 1.221e-4 ✓; R-A3 ✓; R-A4 2.118e-5 ✓; R-B1 zero resolvable
reversals (all margins ≤ 3.84e-5 < 2^-9) ✓; R-B2/R-B4 trivially identical (no
top-k changed at all, displacement 0) ✓; R-B5 1.0 ✓; R-C1 (surrogate is dense
L2-normalized) ✓; R-C2 ✓. **The identical measurements that produced FAIL
produce PASS under the revision.** The repair is defensible on the merits — C2
and C6 really were mis-specified — but G6A must state the resulting prohibition
explicitly: *the revised criteria may never be evaluated on the corpus that
motivated the repair.* It routes R-B* to the real corpus but never says why that
is mandatory for the credibility of the whole exercise.

### 5.5 R-B5 — no discriminating power (measured)

Across album sizes n ∈ {3, 10, 20} and intra-album cosine ∈ {0.95, 0.5, 0.0,
−0.5}, 60 repetitions each: minimum `cos(agg_f32, agg_f16)` = **1.000000000**
(below 5e-10) in every configuration; the committed run measured
1.000000000000. The worst case can breach `1 − 2^-10` only if member rounding
errors *align* (requires similar members) while the mean *cancels* (requires
dissimilar members) — contradictory requirements. The threshold is also
underived (no entry in §9's provenance table). Keep as a diagnostic; it is not a
gate.

---

## 6. Counterexamples

### A — top-1 flip (constructed, quantified)

Yes: §4.3's table. Margins of 1–10 f32 ULPs flip 40–45 % of the time.
Criterion outcome: R-B1 exempts (margin < floor), R-B2/R-B4 pass (band set
identical), R-B3 undefined. User-visible answer: changes. Acceptable? Only if
the product declares intra-band selection arbitrary — which nothing currently
does (§7).

### B — rank-10/11 boundary (constructed)

Same construction at the boundary: a rank-11 candidate within the floor of the
rank-10 score may swap in. R-B2's band *set* is preserved (both members), so it
passes; the top-10 the user receives changed. Additionally, movements among
ranks 11–20 are entirely ungated because `TOP_KS = {1,5,10}` while ADR 0017
§5.8 sketches `limit=20` (tracks) and `limit=12` (albums).

### C — multiple near ties (constructed)

E6's construction *is* this: any number of candidates may permute inside the
band; R-B2/R-B4 constrain only membership, never order, so the entire presented
ordering can change while every criterion passes.

### D — product threshold τ (analytic + quantified)

A per-pair floor cannot protect a per-score threshold: `|Δcos| ≤ ε > 0` means
any score within ε of τ may cross. Measured: 80,000 candidates placed 5e-6
below τ = 0.7 → **12,395 crossed up, 0 crossed down**. The relevant instrument
already exists — R-A3, the one-sided promotion bound, applied at τ once τ
exists. τ itself is a product decision (ADR 0016 Slice 0); I do not invent one.

### E — ranking beyond k

Real: see B. `limit=20`/`limit=12` in ADR 0017 §5.8 vs `TOP_KS = {1,5,10}`.

### Cases where the criteria fail while f16 has not harmed retrieval

- R-A2 (`2^-13`, underived) at low dimension: my symmetric measurements give
  mean 4.60e-5 at d=16 (0.38× of `2^-13`) — safe, but the d=4 region of the
  separation probe shows errors up to 1.6e-4 with *max* values ~5e-4; a
  low-dimensional profile could fail R-A2 while nothing user-visible moves.
- R-C1 clause 2 as an absolute energy fraction will fail legitimately sparse
  profiles whose retrieval is nonetheless fine (zeros are exact in f16; the
  measured error for support-2 vectors is 1.0e-4, passing R-A1 with 9.7×
  margin).

---

## 7. "Unresolvable" pairs — architecture analysis

The correct distinction:

> *"The model says these are effectively tied"* — an assertion about the
> embedding, unsupported by any evidence in this repository (no ground truth,
> no human review; ADR 0017 §2.2 says so explicitly).
>
> *"The product is allowed to return either one"* — a policy decision that
> must be made somewhere and currently is made nowhere.

Two facts make the policy decision non-optional rather than theoretical:

1. **Determinism and reproducibility.** `eval::nearest` sorts by score with
   `.then_with(|| left.index.cmp(&right.index))`. Inside the band — which under
   `2^-9` is up to 470× wider than f32's granularity — the index tie-break and
   accumulation order decide what the user sees. Today that is an implementation
   accident; for caches, generated playlists, and any future radio/playlist
   graph, an answer that depends on asset insertion order is a defect waiting to
   be reported. The fix is architectural and cheap: within the band, order by a
   content-defined key (e.g. lowest `(package_id, asset_id)`), recorded as an
   ADR 0017 §5.7 (server exact-cosine) requirement.
2. **`2^-9` is wider than the model's own reproducibility band** (§4.4). Under
   that floor, "unresolvable" includes separations at which the model itself
   flips top-1 in one seed of five — so the band is not distinguishing f16 noise
   from model noise; it is swallowing both.

Blame assignment: the *existence* of near-ties is a property of the embedding
model and the retrieval architecture, not of f16 — f16 widens the affected band
from f32's `6e-8` to ~`3e-5`. The correct conclusion is not "f16 is unsafe" but
"the band must be governed, and its width must be justified against product
sensitivity, not against binary16 alone."

---

## 8. Subnormal and profile audit

**What the evidence shows (re-verified):** the `subnormal-boundary` fixture
(unnormalized vectors at `~2^-24`) moves a cosine by **7.954e-2** — 81.5× R-A1's
bound. The sweep's knee is real. The `d ≤ 2^28` arithmetic
(`1/√d ≥ 2^-14`) is correct but should be framed by margin: at d = 1280
components sit 8.8 binades above the knee; at d = 2²⁴ they sit 2 binades above
it and are outside any evidence base.

**What R-C1 should be.** For a *unit* vector, a subnormal component carries at
most `(2^-14)² = 3.7e-9` of the energy and perturbs the cosine by ~`2^-25` — so
clause 1 (L2 normalization) is load-bearing and clauses 2–3 are near-vacuous
defence-in-depth given clause 1 (a "non-negligible-energy subnormal component"
cannot exist in a unit vector). G6A presents three co-equal requirements; it
should present one requirement plus two tripwires, and note that clause 1
substantially duplicates fingerprint tag 8 (`normalization`).

**Is a dense random-vector experiment sufficient evidence for future learned
embeddings? No.** Measured error by component distribution (d = 1280,
symmetric regime):

| distribution | max \|Δcos\| | ratio to `2^-10` |
| --- | ---: | ---: |
| dense random (committed) | 2.78e-5 | 0.028 |
| heavy-tail `1/k` | 4.76e-5 | 0.049 |
| support 2 (worst of 4,000) | 1.00e-4 | 0.102 |
| support 4 (worst of 4,000) | 2.10e-4 | 0.215 |

Concentrated profiles thin R-A1's margin by ~7.6× while still passing, and
**no proposed criterion distinguishes them**. Required addition to R-C1's
registry check: record the support size / participation ratio of reference
vectors next to the subnormal energy fraction, and flag concentrated profiles
for individual scrutiny. (Profiles with many near-exact zeros are fine — zero
is exact in f16 — which is exactly why an energy-fraction test, not a
per-component floor, is the right shape; G6A gets that part right.)

---

## 9. Distributional audit

| distribution | what happens | caught by |
| --- | --- | --- |
| sparse (support 2–8) | error 3.5–7.5× dense; still passes R-A1 | nothing flags it (§8) |
| heavy-tailed (`1/k^p`) | 1.7–2.6× dense | nothing flags it |
| components near `2^-14` | precision at the knee; harmless if energy is small | R-C1 clause 2 |
| values in the subnormal region | catastrophic (7.95e-2 measured) | R-C1 clause 1 / R-A1 |
| highly correlated / near-duplicate tracks | margins quantized at f32's `2^-24`; **45 % flip rate** in the window | *exempted* by R-B1 (§4.3) |
| exact ties | `eval::nearest` breaks by index — deterministic but arbitrary | nothing (§7) |
| many candidates near one boundary | whole presented ordering can permute | R-B2/R-B4 see only set membership |

The two failure directions requested:

- **All criteria pass, meaningful result changes:** §4.3 (12,625 top-1 flips),
  §6 B/C (boundary and multi-tie permutations), §6 D (τ crossings).
- **Criteria fail, nothing meaningful harmed:** R-A2 at low dimension (§6);
  R-C1 clause 2 for legitimately sparse profiles (§8); R-B5's threshold if an
  adversarial cancellation album ever appeared (§5.5).

---

## 10. Threshold-provenance audit

| Threshold | Claimed purpose | Derivation | Independently justifiable? | Risk |
| --- | --- | --- | --- | --- |
| `2^-11` | component half-ulp | binary16 significand width | **yes — proven, tight (0.9988)** | none |
| `2^-10` (R-A1, R-A4) | vector/cosine/norm bound | `2 × 2^-11` + angular argument | **yes — proven, conservative ~65×** | conditional on normal range, unstated in tables |
| `2^-9` (floor) | pair-ordering resolvability | triangle inequality | valid arithmetically; **not justifiable as a product criterion** (§4.3, §4.4) | hides a 470× window |
| `2^-13` (R-A2) | mean movement | "*should land near or below*" | **no — no derivation exists** | spurious failure at low d |
| `1 − 2^-10` (R-B5) | aggregate stability | none given | no — and measured to be ≥6 orders loose | false confidence |
| `2^-24` (R-C1 φ) | subnormal energy | `√φ ≤ 2^-12` | derivation inconsistent with its stated goal (`2^-11`, §3.6) | minor (stricter direction) |
| `2^-28` (dimension) | normal-range reach | `1/√d ≥ 2^-14` | yes, arithmetic; framing overstates margin | reads as more permissive than it is |
| `k = 1,5,10` | neighbourhoods | inherited from C4/C5/C7 diagnostics | inherited, **and incomplete vs `limit=20`/`limit=12`** | ranks 11–20 ungated |
| zero / identical (R-B1–B4) | identity above floor | definitional | definitional, but see redundancy (§13) | false sense of coverage |

Could someone derive these without seeing the result being judged? `2^-11`,
`2^-10`, `2^-9`: yes, I did. `2^-13`: no. `1 − 2^-10`: no. `k = {1,5,10}`: yes
but wrongly scoped. §9 of G6A asserts "*none is `observed × k`*" — true for the
load-bearing numbers, not true of `2^-13`'s status as a bound (it is a guess
dressed as one) or of R-B5's threshold (absent from the table entirely).

---

## 11. Surrogate-corpus audit

**What it legitimately establishes:** the conversion is correct; component,
vector and cosine bounds behave as derived; error scales with dimension
(1.4e-4 at d=16 → 1.8e-5 at d=1280, consistent with `1/√d`); subnormals fail;
f32 is the binding resolution at d ≥ 16 (`5.960e-8` everywhere).

**What it cannot establish — and why its clean result is structural, not
evidence.** The surrogate — including the deliberately near-tied supplementary
corpus (`spread = 0.02`) — produced **zero** top-1/top-5/top-10 changes and zero
rank displacement at every dimension. Two mechanisms guarantee this regardless
of f16:

1. *CLT cancellation*: random rounding over 1280 components lands ~50–65× below
   the worst-case bound, predictably, for any dense random vector population.
   The favourable ε was knowable before running anything.
2. *Margin-distribution mismatch*: the near-tie construction makes within-cluster
   cosines **high**, not competing-candidate **margins small**. Its swap margins
   (3.99e-6, 4.29e-6) are typical of the construction, not of a boundary
   population. Nothing in the generator places candidates at 1–10 f32 ULPs of
   separation — which is precisely where f16 flips (§4.3).

So "zero retrieval changes on the surrogate" is a property of the generator's
margin distribution. It is consistent with f16 being harmless *and* with the
surrogate never having visited the region where harm occurs. G6A §8 concedes
R-B* needs the real corpus but still reads the surrogate's clean B results as
informative ("B was perfect"); they are not.

---

## 12. Real-corpus experiment requirements

- **Metrics:** R-A1/R-A4 (as precondition confirmation), R-A2 (if retained),
  R-B2-merged (band identity) **plus the band population per seed and k** and a
  margin histogram near each boundary, R-A3-at-τ once τ exists, R-C1 registry
  conformance with the sparsity record (§8). All pairwise metrics in the
  **symmetric regime** (§3.5).
- **Pairs:** all ordered pairs (the tails are where subnormal/precision effects
  surface) *and* the retrieval-relevant region defined per query, not as a
  global union.
- **Is 45 tracks enough?** To *run*: yes. To *conclude*: only with the band
  population reported — on 44 candidates per query the band may be empty, in
  which case R-B2 is untested and must be recorded as such, not as passed. The
  same-artist/different-album stratum is structurally empty (one album per
  artist, ADR 0017 §2.2); note it, don't fix it by inventing data.
- **Dimensions/profiles:** both recorded variants — `multi` 1280-D and
  `release` 512-D — and both recorded patch hops (61, 62), giving four corpora;
  the patch-hop corpora are the closest available thing to a
  "different-but-similar model" and directly calibrate the floor against model
  instability. No new inference: all of it is post-processing over recorded
  `embeddings.json` (absent from the repo — that availability gap is the actual
  blocker).
- **Scope of the conclusion:** per §15 below, the run decides *profile-level
  admissibility*, never universal f16 safety. The record must carry the
  `profile_fingerprint`, model variant, patch hop and corpus identity digest
  (RUN.txt already demands this).
- **Cannot establish:** anything about other models, other dimensions, or
  perceptual adequacy (G-7).

---

## 13. Redundancy audit

| Criterion | Status |
| --- | --- |
| R-A1 | **load-bearing** (theorem given R-C1) |
| R-A2 | independent, but threshold underived — derive or drop |
| R-A3 | **implied by R-A1** (one-sided ≤ two-sided); keep only as the τ-instrument |
| R-A4 | independent theorem — keep |
| R-A5 | diagnostic only — fine |
| R-B1 | **implied by R-A1** (derived floor) or **vacuous** (measured floor) — delete or re-parameterize with an external floor |
| R-B2 | the one real retrieval criterion — keep, define "boundary" |
| R-B3 | undefined or reduces to R-B1 — delete or define |
| R-B4 | **≡ R-B2** — merge |
| R-B5 | no discriminating power — demote to diagnostic |
| R-C1 | **load-bearing** — keep, restructure (§8) |
| R-C2 | documentation — fine |

Independent statements that survive: **R-A1, R-A4, R-B2, R-C1**, plus R-A2 if
derived and R-A3 once τ exists. The "sixteen criteria" framing overstates the
gate's content by ~2.5×, which matters because redundancy reads as coverage.

---

## 14. Architectural audit

Against ADR 0017: model-agnostic profile mechanism ✓ (f16 is a tag-11 value;
new value ⇒ new fingerprint ⇒ new partition ⇒ re-analysis, never conversion);
`profile_id`/`profile_fingerprint` discipline ✓ (with the R-C1-needs-a-tag
question already flagged by G6A §10 as a format decision); model-as-profile-value
✓; exact cosine initially ✓ — **and R-C1 must not require post-decode
renormalization**: it would break bit-exactness with `FORMAT_SPEC` §7.4 and is
unnecessary because `eval::cosine` is norm-robust (G6A does not require it but
never records why); server-local indexing ✓ (no assumption about ANN
structure); optional package-carried similarity ✓; no automatic model fetching
✓; derived-not-identity ✓.

Hidden couplings introduced by f16: **none found** to a model, dimension,
runtime, or normalization convention beyond the (correct, profile-level)
L2-normalization precondition, which should be phrased as a conformance check of
tag 8 rather than a parallel rule. One coupling is *created by the criteria
rather than by f16*: making the band authoritative pushes a deterministic
intra-band ordering requirement onto ADR 0017 §5.7 (§7). That is a dependency to
record, not a format change.

---

## 15. Required changes

Blocking for the real-corpus experiment:

1. **Fix the instrument.** The pairwise metrics must be computed in the
   symmetric regime `(q_i, q_j)` — currently only one operand is quantized
   (`src/g6.rs:489-491`), understating the ranking-relevant error by
   ~1.3–1.5×. (Experiment code; not changed by this review.)
2. **Re-justify the floor.** Retain `2^-9` only as a derived ceiling. Use either
   the cosine-dependent `2^-10·sin θ` or the measured symmetric `2ε` as the
   operational band, and reconcile it explicitly against the patch-hop
   instability band (2.3e-5…8.94e-4, top-1 changed 9/45). Resolve G6A §10
   decision 3 as "measured symmetric", cross-checked against the derived bound.
3. **Cover the product's query sizes:** `k ∈ {1,5,10,12,20}` (ADR 0017 §5.8), or
   record why not.
4. **Add a presented-answer criterion:** deterministic intra-band selection by a
   content-defined key (`(package_id, asset_id)`), recorded as an ADR 0017 §5.7
   dependency; note that `eval::nearest`'s index tie-break is currently
   load-bearing for exactly the cases the band exempts.
5. **Record the same-data prohibition:** the revised criteria may not be
   evaluated on the surrogate that motivated the repair (§5.4).

Required before sign-off:

6. **Prune to independent statements** (§13): delete/absorb R-A3 (keep as the
   τ-instrument), R-B1, R-B3; merge R-B2/R-B4 with "boundary" defined; demote
   R-B5 to diagnostic.
7. **State the precondition** in the criteria tables (normal range / negligible
   subnormal energy) and correct "Corpus: any"; state the implication
   R-C1 ⇒ R-A1 explicitly.
8. **Correct §6.1** (single-mantissa artifact, worst case is `2^-11`, 4.25× the
   tabulated value) and **§6.3** (`2^-12` vs `2^-11` inconsistency).
9. **Derive or drop `2^-13`** (R-A2).
10. **Extend R-C1's registry check with support size / participation ratio**;
    correct the `2^-28` framing to a margin statement.
11. **Real-corpus protocol** per §12: band populations and margin histograms
    reported, both model variants, both patch hops, profile fingerprint and
    corpus digest recorded, result phrased as profile-level admissibility.
12. **Relabel C2** as *void as written* — a quantization criterion expressed as
    an absolute level; property (i) = R-A3 (at τ when τ exists), property (ii)
    deferred to ADR 0016 Slice 0; do not file it under model validation.

---

## 16. Final verdict

# ACCEPT WITH CHANGES

The methodology gives MusicPack a defensible way to decide, which is the
question asked. The A/B/C separation is correct and necessary; the mathematics
is correct (independently re-derived, including the tighter form
`2^-11(2|a−b| + sinθ_a + sinθ_b)`); the refusal to invent τ is right; R-C1 as a
profile-registry conformance test is architecturally exactly right for a
model-agnostic format.

It does not yet justify *running* the final experiment, because two of its
instruments are mis-specified (the mixed-regime measurement; the `k` set), its
central floor is calibrated to binary16 rather than to the product's own
sensitivity (the 470× window; the patch-hop band), roughly a third of the
criteria are redundant or vacuous, and the same-data re-pass hazard is real and
unrecorded. All five are repairable without touching production code, the
format, or ADR 0017, and without weakening anything that actually protects the
user.

**What is mathematically proven:** §3.2–3.5, §4.2 (bounds), §5.1–5.3
(implications), §13 (redundancy).
**What is experimentally demonstrated:** §3.5 (regime ratios), §4.3 (45 % flip
rate in the f32-resolvable window), §5.5 (R-B5 vacuity), §6 D (τ crossings),
§8 (distribution table).
**Assumptions requiring statement:** normal-range precondition; band-emptiness
risk on 44-candidate queries; intra-band arbitrariness being acceptable to the
product.
**Product decisions, not mine to make:** τ (ADR 0016 Slice 0); whether
intra-band reordering is user-acceptable; the intra-band ordering key.
**Unresolved:** real-corpus availability; whether the band will contain any
members at all; R-C1's fingerprint-tag question (G6A §10 item 4).

`G-6: FAIL — KEEP F32` — unchanged.

---

## 17. Recommended next step

**Do not run the real-corpus experiment yet.** Sequence:

1. Apply changes 1–11 (criteria text plus the one instrument fix in experiment
   code; ~a day, no production impact). Change 1 is the only code change and it
   is inside `experiments/`.
2. Re-generate the surrogate artefact once, under the symmetric regime, *as an
   instrument check only* — with the same-data prohibition (change 5) recorded
   in `RUN.txt`, so the regenerated numbers cannot be cited as criteria
   evidence.
3. Locate or re-record a real run's `embeddings.json` (the availability gap is
   the actual blocker; no new inference is required if the recorded outputs
   exist), then run the pruned criterion set (R-A1, R-A4, R-B2-merged, R-C1 +
   sparsity record, band populations, both variants, both patch hops) as
   post-processing.
4. Human decision, phrased per §15 of the task: *is `f16le` admissible for this
   profile, subject to R-C1 conformance and a non-empty, well-behaved band* —
   never "is f16 universally safe". Until that decision, `G-6` stays
   `FAIL — KEEP F32` and the reference encoding stays `f32le`.

---

*Provenance note: the verification harness used for §3.5, §4.3, §5.5, §6, §8
and §11 was written for this review, run against the committed `g6`/`eval`
code, and deleted. Its mixed-regime maxima reproduce the committed
`REPORT.txt` values exactly (1.8353e-5 / 3.4243e-5 / 1.4007e-4), which is the
evidence that the harness measured the same corpus the artefact did. This review
was conducted without reliance on `G6A_INDEPENDENT_REVIEW.md`; where the two
reviews overlap (the floor's width, `k` coverage, C2's mislabelling, the
unstated precondition), the conclusions here were reached by independent
derivation, and this review additionally found the regime mismatch, the
implication chain, the same-data re-pass, the `1.149e-4` artifact, R-B5's
vacuity, and the measured 45 % flip rate in the f32-resolvable window.*
