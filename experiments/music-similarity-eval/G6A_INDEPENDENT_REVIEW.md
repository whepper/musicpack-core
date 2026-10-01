# G-6A Independent Adversarial Review

**Reviewer role:** second opinion. No prior involvement in G-6 or G-6A.
**Reviewer independence:** the derivations in `G6A_CRITERIA_REVIEW.md` were
re-derived from the binary16 specification and the source, not read off the
document. Two scratch harnesses were written, run, and deleted; no file in this
repository was modified to produce this review except this one.
**Status at time of review:**

- `G-6: FAIL — KEEP F32` (unchanged by this review)
- `G-6A: CRITERIA REPAIRED — READY FOR REVIEW` (assessed below)

**Scope of this document:** review only. It does not re-run G-6, does not
re-judge f16, does not approve f16, and does not touch `FORMAT_SPEC.md`,
`FORMAT_SIGNOFF.md`, ADR 0017, or any production code.

---

## 1. Executive verdict

**The core approach is sound. The operational floor and the coverage of the
retrieval criteria are not.**

Three things are genuinely well done and should survive this review unchanged:

1. **The A / B / C separation.** Numerical fidelity, retrieval fidelity, and
   representational safety are three different properties with three different
   evidence bases, and G-6's original failure was that C1–C10 made them share one
   threshold budget. That diagnosis is correct and the repair is structurally
   right.
2. **Deriving thresholds from binary16's structure rather than from the
   observation.** I re-derived `2^-11`, `2^-10`, and `2^-9` from the
   specification independently. They are correct, and I verified the component
   bound is *exactly tight*. This is the discipline the programme needed, and
   §9's claim that no threshold is `observed × k` is substantially true.
3. **Refusing to invent τ.** Correct. τ is a product decision and the document
   says so instead of fabricating a number.

Two things block the real-corpus rerun as currently specified:

1. **The derived floor `2^-9` is wider than the band in which the reference
   system is already known to change top-1.** `2^-9 = 1.953e-3` is 2.2× the
   *largest* pairwise cosine movement that a single model patch hop produces, and
   14.7× the mean (ADR 0017 line 64: hop 61 vs 62 → cosine drift 2.3e-5…8.9e-4,
   top-1 changed in **9/45** seeds). R-B1…R-B4 therefore license f16 to reorder
   candidates across a separation band in which the *reference* reorders them
   anyway. That is not conservative; it is calibrated to the wrong reference
   point. The measured floor (`2ε = 3.67e-5`) sits 3.6× *below* the mean model
   drift and is the correct operational choice. G6A §10 lists this as open
   decision #3 but frames it as an optional tightening; it is **required** for
   the criteria to mean anything.
2. **`k ∈ {1, 5, 10}` does not cover the product's own query sizes.** ADR 0017
   §5.8 sketches `GET /api/v1/tracks/{id}/similar?limit=20` and
   `GET /api/v1/albums/{id}/similar?limit=12`. Ranks 11–20 are entirely ungated.

Six further defects are individually minor but collectively mean the criteria
cannot yet be executed reproducibly: two criteria use undefined terms, two are
each other's duplicate, the derivations carry an unstated precondition, one
threshold has no derivation, and the subnormal-energy threshold is inconsistent
with its own stated goal.

**Verdict: ACCEPT WITH CHANGES.** See §12 for the twelve required changes.
`G-6: FAIL — KEEP F32` remains the correct formal state and this review does not
alter it.

---

## 2. Mathematical audit

### 2.1 What the implementation actually does

Read from source, not from the prose.

`src/docfmt.rs::f32_to_f16_bits` implements round-to-nearest-even for the normal
range by truncating 13 mantissa bits and conditionally incrementing on
`remainder > 0x1000 || (remainder == 0x1000 && truncated odd)`. That is correct
RNE, including the tie-to-even case. The significand carry at `0x400` is handled
and rejects overflow above `unbiased = 15`. f32 zero and f32 subnormals (`exponent
== 0`, all below `2^-126`, i.e. far below binary16's `2^-24` subnormal floor)
correctly return signed zero. The subnormal path shifts by `-(unbiased + 1)`
(binades 14…24) and also rounds to nearest even. `f16_bits_to_f32` is a widening
decode, which is exact.

Two properties of the harness matter for the bounds and are favourable:

- **f16 → f32 widening is exact**, so in the "f16 world" every input to the
  cosine kernel is exactly representable in `f32`.
- `src/eval.rs::cosine` accumulates `dot`, `norm_a`, `norm_b` in `f64` and
  returns `(…) as f32`. Both worlds therefore accumulate in `f64` with exactly
  representable inputs, so the comparison is arithmetically clean and the only
  f32 exposure is the final rounding of the returned cosine.

That final rounding is worth recording because it is not in any derivation: the
returned cosine is rounded to `f32`, so the *reference itself* is quantised at
`≈ 2^-24 ≈ 6e-8` absolute near `cos ≈ 1`. This is consistent with — and is the
explanation for — the `5.96e-8` resolution limit G-6 measured. It is
`2.9e-4 ×` the `2^-10` bound, so it does not threaten any proposed threshold, but
it belongs in the audit trail.

### 2.2 Component-level bound

**Claim:** a normal binary16 value satisfies `|Δ| ≤ ulp/2 ≤ 2^-11 |v|`.

**Independent check.** For `v` with unbiased exponent `e` and 23-bit significand,
`ulp_f32(v) = 2^(e-23)` and `ulp_f16(v) = 2^(e-10)`. RNE gives
`|Δ| ≤ ulp_f16/2 = 2^(e-11)`, so `|Δ| / |v| ≤ 2^(e-11)/2^e = 2^-11`. Correct.

**Empirical confirmation.** 2,000,000 samples uniformly across
`e ∈ [−14, 15]` (the whole normal range): worst relative error **4.879e-4**
against `2^-11 = 4.883e-4` — a ratio of **0.9993**. The bound is not merely
correct, it is *attained*, as expected at a tie point just above a power of two.
Boundary spot-checks: `2^-14`, `2^-14 × (1 + 2^-10)`, `2^-15`, and `65504` all
round-trip exactly; nothing between `65504` and the overflow threshold is
silently clamped.

**Is the bound absolute or relative?** Relative, and the distinction is
load-bearing. At the smallest normal (`2^-14`) the absolute error is
`2^-25 = 2.98e-8`; at `65504` it is `32`. The relative form is what makes the
vector-level bound work.

### 2.3 Vector-level bound

**Claim:** for a unit vector, `|u − u'| ≤ 2^-11`.

**Independent check.**
`|u−u'|² = Σδᵢ² ≤ Σ(2^-11 vᵢ)² = 2^-22 Σvᵢ² = 2^-22`, hence `|δ| ≤ 2^-11`. Correct,
and dimension-independent.

**This is where the precondition appears.** The step `|δᵢ| ≤ 2^-11|vᵢ|` requires
`|vᵢ| ≥ 2^-14`. For a subnormal-range component the *absolute* error is bounded by
`2^-25` instead, and the relative form fails. G-6's own
`subnormal-boundary` fixture measures `|Δcos| = 7.954e-2` — **81× R-A1's
`2^-10`**. So R-A1 is not an unconditional format guarantee; it is conditional on
the stored vector being L2-normalized with all components in the normal range.
See §6 and required change **R1**.

### 2.4 Cosine bound

**Claim:** `|cos(u,w) − cos(u',w')| ≤ |u−u'| + |w−w'| ≤ 2^-10`.

This step needs care, because `u'` and `w'` are **not unit vectors** — nothing
renormalises after decoding, and G-6 documents this deliberately. Writing
`N = |u'||w'|` and `c = u·w`:

```
c' − c  =  (u'·w' − cN) / N
|u'·w' − c|  ≤  |δu| + |δw| + |δu||δw|
|N − 1|      ≤  |δu| + |δw| + O(2^-22)
⇒ |c' − c|  ≤  (|δu| + |δw|)(1 + |c|) / N  +  O(2^-22)
```

A crude reading of that gives `2 × 2^-10 = 2^-9` for a *single* cosine, which
would have made the pair floor `2^-8`. **That reading is wrong, and the reason is
worth recording** because it is the non-obvious part of the whole review.

The bound that matters is first-order in the *angular* perturbation, not the
vector perturbation. For unit `u, w` separated by angle `θ`,
`|Δcos| ≈ sin θ · Δθ` and `Δθ ≈ 2^-11` per vector, so
`|Δcos| ≈ 2^-10 sin θ`, maximised at `θ = 90°`, i.e. `cos θ = 0`. Two
consequences:

- For **near-aligned** vectors (`cos → 1`), the cosine is *intrinsically
  insensitive*: `sin θ → 0`. The worst case is not where the pairs are most
  similar.
- If `u ≈ w` and `δu = δw` points along `u`, the errors cancel exactly
  (`(1+δ)²/((1+δ)(1+δ)) = 1`). The naïve alignment that maximises the
  perturbation bound minimises the actual error.

So `2^-10` is the correct first-order ceiling, attained near `cos ≈ 0` — which is
exactly the deep tail where G-6 observed the reversals. The bound and the
observation are coherent.

**Empirical confirmation.** Seven dimensions (4, 16, 64, 512, 1280, 2048, 4096) ×
40,000 pairs each, plus seven deliberate *concentration attacks* (target cosines
`−0.99 … +0.999`, where a naive construction would put most of the norm in one
component): worst `|Δcos|` anywhere was **3.886e-5** (d=1280) and **3.140e-4**
(d=16, target `cos = 0`) — ratio to `2^-10` **0.32**. Never within 3× of the
bound.

**Norm bound (R-A4).** Measured `max |‖v'‖ − 1|` = **3.94e-4** at d=4, 3.71e-5 at
d=1280 — ratio to `2^-10` 0.40. R-A4 is valid and conservative.

### 2.5 Summary of §2

| Bound | Verdict | Independent evidence |
| --- | --- | --- |
| `\|Δ\| ≤ 2^-11 \|v\|` | correct, **tight** | ratio 0.9993 over 2M samples |
| `\|u−u'\| ≤ 2^-11` (unit `u`) | correct, dimension-free | follows from Σδᵢ² |
| `\|Δcos\| ≤ 2^-10` | correct, conservative | worst ratio 0.32 |
| `\|Δ(cos_a − cos_b)\| ≤ 2^-9` | correct, conservative | 361,080 triples above the floor → **0 reversals** |
| `\|‖v'‖−1\| ≤ 2^-10` | correct, conservative | worst ratio 0.40 |

**No mathematical error was found in the derivations.** The defect is that one
precondition is unstated, not that the algebra is wrong.

---

## 3. The `2^-9` resolvability-floor audit

### 3.1 Is the derivation structurally correct?

**Yes**, and the structural point is right: `src/eval.rs::nearest` computes
`cosine(&records[query].vector, &record.vector)` on *stored* vectors, so the
query is quantized too. Both cosines in a pair comparison carry an independent
error, and the triangle inequality over two of them is the right instrument.
Nothing here involves three perturbations — the query's single quantization is
already inside `2^-10`, and the candidate's is the other half. G6A's factor of
two is correct.

### 3.2 Does it hold for the actual comparison, including normalisation?

Yes, as §2.4 shows, with the caveat that the crude bound is
`(1+|c|)·2^-10/N` and the *sharp* bound is `2^-10·sin θ`. Both are `≤ 2^-10` per
cosine, so the pair floor `2^-9` is valid. The renormalisation question answers
itself: since the bound holds with and without renormalisation (the norm term
carries the same `sin θ` factor), **R-C1 must not mandate post-decode
renormalisation** — doing so would change the decoded values away from the
bit-exact reference conversion in `FORMAT_SPEC.md` §7.4. No criterion should
require it. G6A does not; good, but the reason is not recorded and a future
editor might "helpfully" add it.

### 3.3 Is `2^-9` conservative? No — and this is the central finding.

Conservative with respect to **binary16** (the true error is `2^-10·sin θ`, well
under `2^-9` for most of the cosine range). But `2^-9` is *anti-conservative with
respect to the property it is meant to protect*, which is retrieval stability.

ADR 0017 line 64, the patch-hop experiment, is the repo's own measurement of how
much cosine movement it takes to change a user-visible answer:

> Hop 61 vs 62: 0/45 identical hashes; cosine min/mean/max
> **0.999106 / 0.999867 / 0.999977**; top-10 overlap mean 0.9844; **top-1 changed
> 9/45**; max common-neighbour rank displacement 2

So a *single model patch hop* — the same vectors, re-extracted, no encoding
change at all — moves pairwise cosines by 2.3e-5 to 8.9e-4 (mean 1.33e-4) and
flips top-1 in **20 % of seeds**.

| | value | ratio to `2^-9` |
| --- | ---: | ---: |
| proposed floor `2^-9` | 1.953e-3 | 1.00 |
| max patch-hop cosine drift | 8.94e-4 | 0.46 |
| mean patch-hop cosine drift | 1.33e-4 | 0.068 |
| G-6 observed floor `2ε` @1280-D | 3.67e-5 | 0.019 |

**`2^-9` is wider than the entire model-instability band.** A criterion that
permits reordering across a 1.95e-3 separation is permitting reordering across
separations at which the reference implementation is *documented to change top-1
in one seed in five*. That is not a safety margin; it is a licence.

**Required: the operational floor is the measured `2ε`, not the derived `2^-9`.**
The measured floor is 3.6× *below* the mean model drift — i.e. f16 is provably
less disruptive than a model patch hop — which is the actual argument for f16 and
is much stronger than the derived bound can ever be. The derived `2^-9` should be
retained as a **sanity ceiling** that the measured floor must not exceed (an
independent cross-check on the measurement), not as the threshold. G6A §10
decision #3 must be resolved as "measured", with the measured value recorded
per dimension.

### 3.4 One qualification, stated honestly

Any non-zero floor exempts a band, and a band-internal reordering is by
construction permitted. The finding is therefore not "the criteria are broken"
but "**the width of the band determines the width of the permitted answer
change**" — and `2^-9` makes that band 15× to 400× wider than the scale at which
answers are known to move. The band design is right; this floor is too wide.

---

## 4. Retrieval-criteria audit (R-B1…R-B5)

### 4.1 What each criterion actually constrains

| | as written | verdict |
| --- | --- | --- |
| R-B1 | reversals among pairs in the retrieval-relevant region with separation > floor | **valid**, but "retrieval-relevant region" is undefined (§4.3) |
| R-B2 | for each seed, each `k ∈ {1,5,10}`: set within floor of the k-th reference score identical | **valid**, but see R-B4 |
| R-B3 | max rank displacement among members in both top-k lists, restricted to the resolvable region | **under-specified** — "resolvable region" has no meaning for a rank difference |
| R-B4 | for each seed and boundary: the set within floor of the boundary score preserved | **duplicates R-B2** (§4.2) |
| R-B5 | `cos(aggregate_f16, aggregate_f32) ≥ 1 − 2^-10` | loose but conservative; derivation missing |

### 4.2 R-B2 and R-B4 are the same criterion

R-B2: set within `2^-9` of **the k-th reference score**, for `k ∈ {1,5,10}`.
R-B4: set within `2^-9` of **the boundary score**, for each seed and boundary.

These are the same statement. The only difference is the word "boundary", which
R-B4 never defines. If "boundary" means the same `{1,5,10}`, R-B4 is
*character-for-character* R-B2 and adds nothing. If it means *every* rank, R-B4 is
strictly stronger and subsumes R-B2 — a materially different gate. A reader
cannot tell which, so the criteria are not reproducible as written.

This is notable because G6A §3's central criticism of C8 was that it was redundant
with C5. **The same failure is present in the repair, in the same document.** It is
not fatal, but it means §11's "separates … and is stricter than the original"
overstates the result: two of the sixteen criteria are one criterion.

### 4.3 "Retrieval-relevant region" and "resolvable region" are undefined

- **R-B1** defines the region as "the union of the top-k across all queries, plus
  the members of every album aggregate". A union is a *set*; relevance is a
  *per-query* relation. A pair `(a, b)` competing for query `q`'s top-20 may not
  both appear in the global union, yet the reversal matters for `q`. The set
  framing loses exactly the structure R-B1 exists to capture.
- **R-B3** restricts rank displacement to "the resolvable region". A displacement
  is a difference of *ranks*, not a difference of *scores*, so the floor — which is
  a score quantity — cannot be applied to it without an explicit bridging rule.
  None is given. **R-B3 is not executable as written.**

### 4.4 The structural gap: the criteria constrain the answer *set*, not the answer

This is the most serious retrieval finding, and it is independent of the floor
value.

Construct: query `q`, candidates with reference scores
`c₁ = 0.9000000`, `c₂ = 0.8999995`. Both lie inside the `k = 1` band
(`|c₂ − c₁| = 5e-7 < 2^-9`).

| criterion | outcome |
| --- | --- |
| R-B1 | exempt — separation below the floor |
| R-B2 | band set `{c₁, c₂}` identical in both worlds → **pass** |
| R-B3 | displacement is band-internal; the exemption is undefined, and either reading exempts it |
| R-B4 | band set preserved → **pass** |
| **presented top-1** | **changes from candidate 1 to candidate 2** |

I built and ran this: **53 top-1 flips** were produced inside the band over 300,000
random triples, with reference margins from `4.17e-7` to `8.64e-6`. Example:

```
top-1 FLIP  margin=8.643e-6 (< floor 1.953e-3)
  candidate A: 0.696197569 -> 0.696193516
  candidate B: 0.696188927 -> 0.696196735
```

All four retrieval criteria pass; the answer the user sees changes.

The *generality* of this is the point: **any** non-zero floor has this property,
so the design cannot escape it by tightening. What it can do is make the
consequences explicit and bounded:

- the presented top-1 must be a member of the preserved band (it is, by R-B2), and
- selection within the band must be **deterministic and independent of
  accumulation order** — lowest `(package_id, asset_id)`, not lowest score.

The second point is an architectural requirement, not a metric. `eval::nearest`
currently breaks exact ties with `.then_with(|| left.index.cmp(&right.index))`,
which is arbitrary from a musical standpoint and can systematically favour low
track numbers. Once the band is the authoritative answer set, the tie-break inside
it becomes user-visible and must be a content-defined key. This is squarely
ADR 0017 §5.7's territory (exact cosine at the server) and is a retrieval
concern, not a format concern — but it must be written down, because today it is
an accident of implementation.

**Required change R6/R7.**

### 4.5 `k` coverage is short

ADR 0017 §5.8 sketches the API the server will expose:

```http
GET /api/v1/tracks/{id}/similar?limit=20
GET /api/v1/albums/{id}/similar?limit=12
```

R-B2/R-B4 gate `k ∈ {1,5,10}`. **Ranks 11–20 are ungated.** A candidate moving
11 → 10 is inside the band and permitted; a candidate moving 13 → 12 is entirely
unchecked. The album side is worse: `limit=12` is ungated past 10 too. Whatever
the reason for the `1,5,10` choice (inherited from C4/C5/C7, which used
top-10 Jaccard), it does not match the product's own proposal.

**Required change R3.**

### 4.6 On excluding noise-floor ordering (§6 of the review brief)

**The exclusion is appropriate, and it is not an f16 limitation.** Stated
precisely:

- Two candidates separated by `5e-7` in cosine are, *for this embedding model*,
  indistinguishable. Any representation whose arithmetic is not bit-identical to
  the reference reorders them, including a reordering of f32 accumulation order.
  Requiring preservation would assert that f16 must reproduce an ordering the
  reference never determined.
- f16 does not *create* the ambiguity. It **widens** the band by roughly `2^-9 /
  2^-10` × the already-existing reference granularity. The limitation belongs to
  **the embedding model plus the retrieval architecture**, and it is pre-existing.
- The obligation this creates is on the *retrieval architecture*: within the
  unresolvable band, results must be presented deterministically by a
  content-defined key (§4.4), and the product must not treat intra-band order as
  meaningful. That is ADR 0017 work, not format work.

Framing f16 as harmless here is correct; framing it as *sufficient* is not. The
band has to be governed.

### 4.7 Non-redundancy, checked

R-B2 and R-B4 do **not** cover what R-B1 covers: a swap between ranks 7 and 8
touches no `{1,5,10}` boundary, so the band-set criteria cannot see it while
R-B1 can. R-B1 and the band criteria are genuinely complementary. R-B3 would be
redundant *given* R-B1 (a resolvable swap is by definition a displacement), so
R-B3's value is entirely in the "same order, same relative positions" guarantee
for large displacements — which is a real property, but one that needs its
exemption defined.

---

## 5. C2 audit

**Is C2's reasoning correct? Substantively yes; the diagnosis is mislabelled.**

Points I agree with:

- C2 conflates two properties, only one of which is encoding-relative.
  G6A §4.1 is right.
- The **upward** half is fully expressible without τ, and one-sidedness is
  correct: promotion is the harmful direction (it manufactures false
  near-duplicates), demotion is the benign one. R-A3 is the right repair.
- The **downward** half genuinely requires a product-defined τ, and the
  τ-conditional form G6A gives — *"no pair whose f32 reference cosine lies within
  ε of τ may cross τ"* — is the correct statement. **I do not invent a τ, and I
  agree none should be.** It is an ADR 0016 Slice 0 decision.
- C2 as written is **unsatisfiable**, not merely strict. A gate that no
  representation of a realistic corpus can pass guarantees a FAIL regardless of
  the encoding, which is exactly what happened. G6A §4.2 states this well and it
  is the strongest part of the C2 analysis.

**Where the diagnosis is wrong.** G6A §4.4 concludes: *"C2 is not measuring
quantization impact. It is measuring embedding-model and corpus quality."* The
first clause is false. `FORMAT_SPEC.md` §7.3 states C2's own intent in its
parenthetical — *"quantization must not push distinct tracks into 'unrelated' or
create false near-duplicates"* — which is unambiguously a **quantization**
criterion. C2 was written as an f16 criterion; it was simply expressed as an
absolute level (`≥ 0.99`) when it should have been a change (`Δcos ≤ ε`).

The consequence is practical, not cosmetic. G6A §10 decision #2 asks whether to
"retire C2 outright or restate it as R-A3 + a τ-conditional criterion". A reader
who has just been told C2 "is measuring embedding-model and corpus quality" will
lean toward filing it under model validation, where it will rot next to an
unrelated model gate and eventually be satisfied by a corpus that happens to be
internally tight. The accurate label is:

> **C2 is void as written — an encoding criterion expressed in the wrong
> quantities. It is not a model-quality criterion and does not belong in model
> validation. Its two halves are disposed of separately: (i) becomes R-A3, (ii) is
> deferred pending τ.**

**Does C2 belong in model/profile validation instead?** No — for the reason above:
its *upward* half is a statement about quantization, and moving it to model
validation would leave f16's promotion behaviour ungated while creating the false
impression it is covered. Its *downward* half is not a model property at all; it
is a product semantics question.

**Can "false near-duplicate creation" be tested without τ?** Yes — R-A3 does
exactly that, and it is the τ-free half. G6A is right on this.

**One provenance slip.** §4.3 writes "the largest possible promotion is
`max abs(Δcos) = 1.835e-5`". `1.835e-5` is the **observed** maximum on the
surrogate at 1280-D, not a bound, and "largest possible" is the wrong word. The
criterion should reference R-A1's derived `2^-10`; the `1.835e-5` belongs in the
results column. This is exactly the `observed × k` pattern §9 claims is absent.

---

## 6. Subnormal and profile audit (R-C1)

### 6.1 Are the three clauses sufficient, necessary, or too strict?

Assessed individually, against the arithmetic rather than against intuition:

**Clause 1 — stored vectors are L2-normalized before serialization.**
*Necessary and sufficient*, and **load-bearing**. It is the only clause that
matters. Proof: a unit vector component in the subnormal range satisfies
`|vᵢ| < 2^-14`, so it carries at most `(2^-14)² = 3.7e-9` of the vector's energy,
and its quantization error is at most half the subnormal spacing, `2^-25 =
3.0e-8`. The induced cosine perturbation is bounded by `≈ 2·2^-25·|cos| ≈ 6e-8` —
**five orders of magnitude below `2^-10`**. So for any L2-normalized vector, the
subnormal regime is arithmetically irrelevant.

**Clause 3 — "no component that carries non-negligible energy lies in the
subnormal range."** For an L2-normalized vector this is **self-contradictory and
therefore vacuous**: a subnormal component *cannot* carry non-negligible energy,
by the bound just given. It is a restatement of clause 1 in more words.

**Clause 2 — subnormal energy fraction ≤ `2^-24`.** Also near-vacuous under clause
1: a vector would need > `2^6 = 64` components at `2^-15` to breach it, and even
63 such components contribute only `63 × 2^-30 = 5.9e-8` of L2 error.

**So R-C1 in practice reduces to one requirement: normalize.** G6A §6.3 does
identify clause 1 as "the load-bearing requirement" — that credit is due — but §7
lists all three as co-equal requirements and §11 counts the separation as
complete. Presenting three clauses when one is load-bearing and two are
defence-in-depth overstates the guarantee and obscures the actual architectural
ask.

**Recommendation:** keep all three (they are cheap and they are *checkable*, which
is the important property), but label 2 and 3 explicitly as defence-in-depth and
state in the criterion that clause 3 is implied by clause 1 for L2-normalized
vectors. A future profile author then knows which one to actually read.

### 6.2 Is R-C1 correctly placed?

**Yes, and well argued.** It is a *profile* requirement, not a container
requirement: the container already specifies the conversion exactly
(`FORMAT_SPEC.md` §7.4) and cannot know whether a profile's numbers are safe in
binary16. Putting the check in the registry is the same layer the
two-identifier discipline already establishes. This is architecturally correct and
I endorse it.

Two placement observations:

- **Clause 1 substantially duplicates fingerprint tag 8** (`normalization`), which
  already declares the normalization convention. If the convention is L2, clause 1
  is a conformance test of a value the fingerprint already carries, and should be
  phrased that way. If the convention is *not* L2, an f16 profile may be
  impossible. That interaction deserves one sentence.
- G6A correctly flags that a new requirement may need a new fingerprint tag and
  that this would change every fingerprint (open decision #4). I would go
  further: if L2 normalization is genuinely mandatory for f16 and profiles may
  legitimately declare other conventions, then **an f16 profile and a non-L2
  profile are mutually exclusive**, and the fingerprint should express that
  relationship — otherwise two fingerprints with the same tag 11 value will have
  different storage safety. This is a format decision requiring its own sign-off,
  and G6A is right to leave it to the format owner.

### 6.3 The `d ≤ 2^28` claim

`1/√d ≥ 2^-14 ⇔ d ≤ 2^28 ≈ 2.68e8` is arithmetically correct. But it reads as far
more permissive than reality: it is the point at which components *reach* the
normal-range floor, not where they are *comfortably* above it. The relevant margin
is the number of binades between the component magnitude and `2^-14`, which is
`log₂(√d) − 14 = d_bits/2 − 14`:

| d | `1/√d` | binades above `2^-14` |
| ---: | ---: | ---: |
| 1280 (measured) | 2^-5.16 | 8.8 |
| 65 536 | 2^-8 | 6 |
| 2²⁴ | 2^-12 | 2 |
| 2²⁸ | 2^-14 | 0 |

The measured 1280-D case sits 8.8 binades clear, which is why `|Δcos|` came out at
1.8e-5. A 2²⁴-dimensional profile would sit 2 binades clear and should be treated
as untested. **Restate as:** *"the normal range is not a binding constraint for any
dimension a real embedding model is likely to produce; profiles above ~2²⁰
dimensions are outside the evidence base and require their own measurement."*

### 6.4 The "ordinary embeddings are safely away" claim

G6A §6.2 is careful to say this about *the measured 1280-D corpus* and to
generalise only via the `1/√d` argument. That is the right discipline and the
brief's warning against generalising to arbitrary future models is **already
honoured in the document**. The residual risk is not the normal/subnormal
boundary; it is the case in §7 (H) below, where a *sparse or concentrated*
profile at ordinary dimensionality produces errors ~20× larger than a dense one.
The subnormal analysis is thorough; the concentration analysis is absent.

---

## 7. Threshold-provenance audit

Classifying every proposed threshold independently:

| Threshold | G6A's claim | My classification | Finding |
| --- | --- | --- | --- |
| `2^-11` | binary16 11 significand bits, RNE half-ulp | **mathematically derived** | verified tight (§2.2) |
| `2^-10` | `2 × 2^-11`, unit-vector cosine bound | **mathematically derived** | verified, conservative (ratio 0.32) |
| `2^-9` | triangle inequality over two stored cosines | **mathematically derived, wrong operational threshold** | exceeds the model-instability band (§3.3) |
| `2^-13` (R-A2) | "regression bound … should land near or below" | **arbitrary / unsupported** | **no derivation** — see below |
| `1 − 2^-10` (R-B5) | (not derived in the table) | **empirically motivated, unstated** | plausible, undocumented |
| `2^-24` energy fraction | `√φ ≤ 2^-12` from half-ulp | **derived, but inconsistent with its own goal** | see below |
| `2^-28` dimension | `1/√d ≥ 2^-14` | **derived, misleadingly framed** | §6.3 |
| `k = 1/5/10` | (not in the table) | **inherited** from C4/C5/C7 | **does not cover the product's `limit=20`/`limit=12`** |
| zero (R-B1, R-B3) | arithmetic identity above the floor | **derived** | sound, conditional on the floor |
| identical (R-B2, R-B4) | definitional | definitional | but duplicated |

### 7.1 `2^-13` (R-A2) has no derivation

The stated rationale is *"the half-ulp relative precision is 2^-11, and averaging
over `d` components with partly-cancelling errors should land near or below
2^-13"*. "Should land near" is an expectation, not a bound. §9 nonetheless
classifies it as "independent of the observed result — yes".

I measured the mean `|Δcos|` across dimensions to see how much slack there is:

| d | mean `\|Δcos\|` | ratio to `2^-13` | max `\|Δcos\|` |
| ---: | ---: | ---: | ---: |
| 2 | 4.61e-5 | 0.38 | 3.37e-4 |
| 4 | 7.34e-5 | **0.60** | 5.03e-4 |
| 8 | 6.34e-5 | 0.52 | 3.32e-4 |
| 16 | 5.37e-5 | 0.44 | 2.98e-4 |
| 32 | 3.71e-5 | 0.30 | 2.04e-4 |
| 128 | 1.93e-5 | 0.16 | 1.12e-4 |
| 512 | 9.63e-6 | 0.079 | 4.97e-5 |
| 1280 | 6.80e-6 | 0.056 | 3.60e-5 |

R-A2 survives everywhere, but at `d = 4` the margin is **1.7×**, and R-A1's
margin at `d = 4` is only **1.9×** (5.03e-4 against 9.77e-4). **Low-dimensional
profiles are the weak spot, and no criterion or note says so.** `2^-13` is also
dimension-blind in a way `2^-10` is not.

Either derive it properly or drop it and let R-A1 carry the weight. A regression
bound with no derivation is the exact thing §9 disclaims.

### 7.2 The subnormal threshold contradicts its own stated goal

§6.3 derives: *"to keep the induced cosine error within one half-ulp relative
precision, the subnormal energy fraction φ must satisfy `√φ ≤ 2^-12`, i.e.
`φ ≤ 2^-24`."*

Half-ulp relative precision is `2^-11`, not `2^-12`. The bound is one binade
stricter than the goal it claims to come from. Since `√φ` is exactly the L2 error
contribution, targeting `2^-12` corresponds to a cosine error of `2^-12`, i.e.
**four times stricter than the stated requirement**. Stricter is the safe
direction, so this is not a defect in the criterion — but it is a derivation that
does not compute what it claims, and under §9's standard that matters. Pick one:
state the goal as `2^-12` with a reason, or use `φ ≤ 2^-22`.

### 7.3 `k = 1/5/10` is inherited and incomplete

The brief's question — *why this number?* — has the answer *inherited*, which is
acceptable, but the inheritance source (`FORMAT_SPEC.md` §7.3's top-10 Jaccard
metric) describes a *diagnostic*, not a *query*. The product's own proposed query
is `limit=20`. Inheritance from a diagnostic to a gate without checking the
product's query sizes is how the coverage gap in §4.5 happened.

### 7.4 `1 − 2^-10` (R-B5) is undocumented

The aggregate is a per-window mean followed by track L2
(`eval::pool_mean_norm`), so averaging attenuates the per-member error before the
`f16` round trip; a cosine bound of `2^-10` on the aggregate is plausible. But the
threshold appears in the criteria table with no derivation and no entry in the §9
provenance table. G-6 measured `1.000000000000` on the surrogate, which is not
evidence about a real corpus. Derive it or mark it as inherited-from-R-A1.

### 7.5 §9's own claim

> Every proposed threshold traces to binary16's structure or to an identity. None
> is `observed × k`.

Substantially true and creditable — but `2^-13` has no derivation, `2^-12` does
not equal its stated goal, and `k = 1/5/10` is not listed. §9 should say
"derived except where marked", and mark them.

---

## 8. Surrogate-corpus audit

The surrogate is deterministic synthetic unit vectors. Establishing precisely what
it can and cannot support:

### 8.1 What it legitimately establishes

- **Arithmetic behaviour (R-A1…R-A4, R-A3).** Component-level error is a property
  of the conversion function, not of the data. My independent sweeps reproduce
  R-A1's 2^-10 and R-A4's 2^-10 on vectors that are not the surrogate's.
  *Legitimately established.*
- **The `1/√d` scaling.** Confirmed independently (§7.1 table): mean error falls
  from 7.3e-5 at d=4 to 6.8e-6 at d=1280. *Legitimately established.*
- **The resolution limit** — f32 binds before f16 at d ≥ 16. This is a property
  of the probe construction (a random orthogonal perturbation) and the f32
  spacing of `5.96e-8` near 1.0, not a universal statement about embeddings.
  *Legitimately established for what it is; should not be generalised.*
- **The subnormal failure mode and its magnitude.** Analytic and
  corpus-independent; the `7.95e-2` figure comes from a constructed case.
  *Legitimately established.*
- **That C2 is unsatisfiable in general.** The min-cosine argument does not
  depend on the surrogate's particular geometry, only on the fact that a
  45-track/15-album corpus has cross-album pairs. *Legitimately established as a
  structural argument; the exact margin is corpus-specific.*

### 8.2 What it cannot establish — and a conclusion that is too strong

**Ranking and retrieval stability: not established, and not even exercised.**

`fixtures/g6/REPORT.txt` reports `top1_changed_seeds=0`, `top1_set_changed_seeds=0`,
`top10_set_changed_seeds=0`, `top10_jaccard_mean=1.000000` at **every** dimension
(16, 512, 1280, and the f16-vs-f32 boundary case). **The surrogate produced zero
top-k changes at every dimension.** Every rank-reversal observation in G-6 came
from the *full pairwise universe*, not from any top-k.

Consequences:

1. **The R-B1…R-B5 machinery has never been run against a case that could fail
   it.** The surrogate cannot distinguish a well-designed criterion from a
   vacuous one, because it never produced a top-k change to constrain.
2. G6A §2's framing — that A came out fine and **B came out in the opposite
   direction**, so B's criteria are the mis-specified ones — is weak. B did not
   come out "pristine" on a hard corpus; it came out trivially perfect on an easy
   one. The surrogate's within-cluster geometry (`spread = 0.35`,
   `cluster_spread = 0.55`) is a *construction choice*, and a different choice
   would plausibly have produced B failures. A real B failure would have been
   **evidence about the encoding**; B's absence is evidence about the generator.
3. The specific geometry that matters for R-B* is the **density of near-tied pairs
   at each rank boundary**. That is a property of the learned embedding
   distribution, and the surrogate imposes none.

**So the review brief's warning is well founded and applies with more force than
G6A acknowledges:** the surrogate's "B was perfect" is not evidence that f16 is
retrieval-safe. It is evidence that the surrogate is not adversarial. G6A §8 does
say R-B* "needs the real corpus" and cannot be stood in for — that part is
right — but §2 and §11 lean on B's perfection in a way the evidence does not
support.

---

## 9. Real-corpus adequacy audit

The recorded run is **45 tracks / 15 albums / one model, two variants, two patch
hops**, with per-track digests. No model run is needed for a rerun; the vectors
exist.

**Is it sufficient?** For **R-A1…R-A5**: yes, with the caveat that dimension
coverage is limited to 1280 (the model's native width) — 512 and 16 in G-6 were
surrogate dimensions, not model dimensions, and R-A* claims corpus-independence so
that is tolerable.

For **R-B1…R-B5**: it is sufficient to *run* the criteria and **not sufficient to
conclude** from them. Three reasons:

1. **Boundary density is likely degenerate.** 45 tracks means 44 candidates per
   query and 1,980 ordered pairs. The band criteria (R-B2, R-B4) only bite where
   candidates land within the floor of a `k`-th boundary. On 44 candidates, the
   expected number of such pairs per seed is small; on a learned corpus with
   tight intra-album clustering, plausibly zero. **If the band is empty, the band
   criteria are vacuous** and will "pass" for the wrong reason. The rerun must
   *report the band population*, not just the pass/fail.
2. **A stratum is structurally absent.** 15 albums, one per artist (ADR 0017
   §2.2 already records this), so *same-artist / different-album* pairs — a
   retrieval case MusicPack will certainly hit — contribute nothing. The
   surrogate is uniform over the universe, so the gap is invisible there.
3. **The reversals in G-6 were all in the deep tail** (cosines near zero,
   ranks 20–45). Under the measured floor those are now *excluded* by R-B1's
   region restriction. So the real corpus may well produce **zero** constrained
   reversals for the same reason the surrogate did: there is nothing near the
   boundary to constrain.

**Is it necessary to gather more?** I do **not** claim a larger corpus is
required, and I will not invent one. But a *stratified* one is, and the
requirement is narrow and justified:

> The rerun must report, per seed and per `k`, the **number and margin
> distribution of candidates within the floor of the k-th boundary**. If that
> population is degenerate, the band criteria are untested and must be recorded
> as **untested**, not as passed.

This is a reporting requirement on the existing corpus, not a data-collection
demand. If it turns out degenerate, the correct next step is a documented
near-tied seed subset — justified by the demonstrated vacuity, not invented
up front.

**Reproducibility:** good. Digests are recorded, post-processing only, no model
download. This is the strongest part of the programme's hygiene.

---

## 10. Counterexamples and failure modes

The brief's nine cases, with the criterion that catches each. "Not caught" is
stated plainly.

### A. Two candidates differing just above the proposed floor
Reference separation `m` slightly `> 2^-9`; f16 reorders them.
**Caught by R-B1** (reversal counted as a failure; threshold is zero). The
triangle inequality guarantees this cannot happen, so it is a real guarantee, not
a statistical hope. ✅

### B. Three candidates clustered around a top-k boundary
All three inside the band; one is presented at rank 1.
**Not caught.** R-B1 exempts (separation below floor), R-B2/R-B4 preserve the
*set* (which still contains the presented answer), R-B3's exemption is undefined.
The **presented** order changes. Constructed: 53 top-1 flips, margins
4.17e-7…8.64e-6. ⚠ **See R6/R7.**

### C. A candidate moving rank 11 → 10
Changes the top-10 set. If both are in the band, R-B2 permits it (set preserved).
If the movement is inside the `{1,5,10}` gates, the set change is *intended*
tolerance. **Partially caught** — and if the query is `limit=20`, positions
11–20 are **not gated at all**. ⚠ **See R3.**

### D. A candidate moving rank 10 → 11
Symmetric to C. Same conclusion. ⚠ **See R3.**

### E. A candidate whose score crosses a product relevance threshold
**Not caught — correctly.** No criterion exists and none should: τ is undefined
(ADR 0016 Slice 0). G6A records this as an open product decision rather than
inventing a number, which is the right handling. ✅ (as a record)

### F. A vector with a small number of important subnormal components
**Self-contradictory for L2-normalized vectors.** A component below `2^-14`
carries at most `3.7e-9` of a unit vector's energy and perturbs the cosine by
`≈ 6e-8`. There is no such thing as an *important* subnormal component in a
normalized vector. R-C1 clause 1 makes the case impossible; clauses 2 and 3 are
belt-and-braces. The only live case is a **non-normalized** vector, which clause 1
excludes — correctly. ✅ (but see §6.1: the criterion is one requirement, not
three)

### G. A vector whose norm changes materially after the round trip
**Caught by R-A4** (`≤ 2^-10`). Independently measured worst case 3.94e-4
(d=4), ratio 0.40; at d=1280, 3.71e-5. Not a threat, and now gated — G-6 measured
it (2.118e-5) but never gated it, which G6A correctly identifies. ✅

### H. A model with highly sparse or concentrated embeddings
**Not caught by any criterion, and not even discussed.** Independently measured:
a 1280-D vector supported on 2–4 components gives `max |Δcos| = 2.65e-4`, **~19×
the dense 1280-D value** (1.40e-5), at ratio 0.27 of R-A1's bound. A one-component
vector is exactly representable (error 0), so the profile is non-monotonic in
sparsity, but 2–4 components is the worst region and it is 3.7× from the limit
rather than 53×. **R-A1 would pass, with a materially different risk profile than
the measured corpus, and nothing in R-A1…R-B5 or R-C1 distinguishes the two.**
⚠ **See R13.**

### I. A model with many components near the f16 normal/subnormal boundary
**Caught by R-A1** — the `subnormal-boundary` fixture measures `|Δcos| = 7.95e-2`,
81× the bound. The *partial* case is subtler: components at `~2^-15` have relative
error `~5.4e-4`, already above `2^-11`, so a vector that is mostly normal with a
handful of `2^-15` components violates the per-component premise while likely
passing R-A1. R-C1 clause 2 is the intended guard, and at `φ ≤ 2^-24` it requires
> 64 such components to bite — so it does not catch the premise violation, but the
premise violation is harmless in that regime (`63 × 2^-30` of L2 error). ✅ with
the documentation fix in **R1**.

---

## 11. Architectural audit

Assessing G-6A against ADR 0017's established architecture.

| Architectural property | Preserved? |
| --- | --- |
| model-agnostic profile mechanism | **Yes.** f16 is a `tag 11` `output_encoding` value; a new value is a new fingerprint, a new partition, a re-analysis, never a conversion of existing vectors (FORMAT_SPEC §12.3). No coupling. |
| `profile_id` / `profile_fingerprint` discipline | **Yes, with one interaction to resolve.** R-C1 introduces a *new* profile obligation. If that obligation is expressible in existing tags (L2 ⇔ tag 8), fine. If it needs a new tag, every fingerprint changes — G6A flags this (open decision #4) and I agree it is a format decision requiring its own sign-off. |
| model as a profile value, not a core dependency | **Yes.** Nothing in R-A…R-C references a specific model, architecture, or embedder. |
| server-side exact cosine | **Yes, and reinforced.** R-C1 must not require post-decode renormalisation (§3.2) — renormalising would break bit-exactness with `FORMAT_SPEC` §7.4's conversion. The server computes cosine from decoded `f32` values; that is unchanged. |
| similarity is derived analysis, not musical identity | **Preserved, and correctly not touched.** No criterion makes a perceptual claim. G-6's `g6_tests.rs` retains a no-perceptual-claim guard. G-7 remains out of scope. |
| optional package-carried similarity | **Yes.** Unaffected. |
| no automatic model fetching | **Yes.** Unaffected. |

### Hidden coupling introduced by f16

- **To a specific model:** none found. The only model-dependent quantity is
  sparsity (§10 H), which is a *measured property* and is properly a per-profile
  reporting obligation, not a format coupling.
- **To a specific dimensionality:** none in the criteria — `2^-10` is
  dimension-independent, and `MAX_DIMENSIONS = 4096` is a container safety limit
  that G6A correctly declines to reinterpret. But §7.1 shows low dimensions carry
  only ~1.9× margin, so the *evidence* is dimension-dependent even though the
  *criterion* is not. Worth recording.
- **To a specific runtime:** none. `rten` produces `f32`; quantization is
  post-processing. No float16 SIMD path, no f16-specific kernel.
- **To a specific normalization convention:** **yes, and correctly so.** R-C1
  clause 1 makes L2 normalization a *precondition* for f16. That is a genuine new
  coupling, it is unavoidable, and G6A states it as a profile requirement with the
  registry as enforcer. The one refinement: an f16 profile and a non-L2 profile
  become mutually exclusive, which the fingerprint should be able to express.

### One architectural concern the review raises

R-B2/R-B4 make the *band set* the authoritative answer set. That pushes a
retrieval-architecture decision into a format gate: **within the band, results
must be ordered deterministically by a content-defined key.** Today that key is
`eval::nearest`'s index tie-break, which is arbitrary and can favour low asset
ids. If the band becomes authoritative, that key becomes user-visible policy and
belongs in ADR 0017 §5.7, not in an experiment's criteria. G6A is right that this
is not the format's business — but it must be *recorded* as a dependency, or the
criteria will be read as guaranteeing an ordering they do not and cannot.

---

## 12. Required changes

Ordered by blocking status. **R1–R3 block the real-corpus rerun; R4–R13 must be
resolved before the criteria are signed off as a gate.**

### Blocking

**R1 — State the precondition on R-A1…R-A4 and R-B1.**
Every bound in §2 requires `|δᵢ| ≤ 2^-11 |vᵢ|`, which requires each component to
be in the binary16 normal range. §7's "Corpus: any" reads as unconditional and
implies a format guarantee. Add a `Precondition` column: *stored vectors are
L2-normalized; every component satisfies `|vᵢ| ≥ 2^-14`*, and cite the
`subnormal-boundary` measurement (`7.95e-2`, 81× R-A1) as the unconditioned
counterexample.

**R2 — Replace the operational floor with the measured `2ε`; demote `2^-9` to a
sanity ceiling.**
`2^-9 = 1.953e-3` is 2.2× the largest cosine drift a single model patch hop
produces and 14.7× the mean (ADR 0017 line 64: top-1 changed 9/45), i.e. wider
than the band in which the *reference* is known to change answers. The measured
floor `2ε = 3.67e-5` (1280-D) sits 3.6× below the mean model drift. Record the
measured ε per dimension, require `2ε_measured ≤ 2^-9` as an independent
cross-check, and resolve G6A §10 decision #3 as **measured**. The derived bound
stays in the document as the sanity ceiling that validates the measurement.

**R3 — Extend `k` to the product's own query sizes.**
ADR 0017 §5.8: `limit=20` (tracks), `limit=12` (albums). Gate
`k ∈ {1,5,10,20}` for tracks and `k ∈ {1,5,12}` for albums. Ranks 11–20 are
currently ungated. If there is a reason not to, state it.

### Required before sign-off

**R4 — Define R-B3's exemption.** A rank displacement is a difference of ranks,
not scores, so the score-based floor cannot be applied to it. Proposed rule: *a
displacement between members `a, b` is exempt only if
`|cos_f32(q,a) − cos_f32(q,b)|` is below the floor*, where `q` is the query at
whose boundary the movement occurs.

**R5 — Define R-B1's region per query.** Replace the global union with: *for
query `q`, the resolvable pairs are those `(a,b)` both present in `q`'s
top-`K` f32 reference ranking, `K = 20`.* A union of sets loses the per-query
relation the criterion depends on.

**R6 — Resolve the R-B2 / R-B4 duplication.** They are the same statement modulo
the undefined word "boundary". Either merge, or define "boundary" precisely and
state which of the two is stronger. Note in the document that this is the same
redundancy G6A §3 found in C8/C5 — the repair must not reproduce the defect it
set out to fix.

**R7 — Add a presented-answer criterion (R-B6).** The band criteria constrain the
answer *set*; nothing constrains the answer *presented* from it. Require: (a) the
presented top-1 lies in the preserved band; and (b) selection within the band is
by a deterministic, accumulation-order-independent key (lowest
`(package_id, asset_id)`), not by score. Record the dependency on ADR 0017 §5.7,
since `eval::nearest` currently uses an index tie-break that would become
user-visible.

**R8 — Derive or drop `2^-13` (R-A2).** "Should land near or below" is an
expectation, not a bound. Either derive it or defer entirely to R-A1's `2^-10`.
If retained, scope it by dimension and record the measured margin: the mean ratio
to `2^-13` ranges from 0.38 (d=2) to **0.60 (d=4)**, and R-A1's own margin at
d=4 is only 1.9×. Note that low-dimensional profiles are the weak spot.

**R9 — Fix the subnormal-energy derivation.** The stated goal is half-ulp
(`2^-11`); the bound used is `2^-12` → `φ ≤ 2^-24`. State the goal as `2^-12` with
a reason, or use `φ ≤ 2^-22`.

**R10 — Label R-C1's clauses by weight.** Clause 1 (L2 normalization) is
load-bearing and is what R-C1 actually reduces to. For an L2-normalized vector a
subnormal component carries `≤ (2^-14)² = 3.7e-9` of the energy and perturbs the
cosine by `≈ 6e-8`, so **clause 3 is implied by clause 1** and clause 2 is
defence-in-depth. Present them accordingly, and note that clause 1 substantially
duplicates fingerprint tag 8 (`normalization`).

**R11 — Add a band-population report to the rerun.** Per seed and per `k`, report
how many candidates fall within the floor of the k-th boundary and the margin
distribution. On 44 candidates per query this population may be **empty**, in
which case R-B2/R-B4 are vacuous and must be recorded as **untested**, not
passed. This is a reporting requirement on the existing 45-track run, not a
demand for a new corpus. Also record that the same-artist / different-album
stratum is structurally empty at one artist per album (ADR 0017 §2.2).

**R12 — Restate the `d ≤ 2^28` claim** as: the normal range is not a binding
constraint for any dimension a real embedding model is likely to produce; profiles
above ~2²⁰ dimensions are outside the evidence base and require their own
measurement. (At d=2²⁴, components sit only 2 binades above `2^-14`.)

**R13 — Report the sparsity profile per candidate profile.** A 1280-D vector
supported on 2–4 components gives `max |Δcos| = 2.65e-4`, ~19× the dense
1280-D value, at ratio 0.27 of R-A1's bound — R-A1 passes, but with 3.7× margin
instead of 53×. No criterion distinguishes a dense profile from a concentrated
one. Record the sparsity (support size) of reference vectors alongside the
R-C1 conformance test, and state that a concentrated profile warrants closer
scrutiny even when R-A1 passes.

### Documentation-only

**R14 — Correct the C2 diagnosis** (§5): C2 was written as a quantization
criterion and expressed in the wrong quantities. It is **not** a model-quality
criterion and must not be filed under model validation. Label it *void as
written*, with the two halves disposed of separately (R-A3 and τ-deferred).

**R15 — Correct §4.3's provenance slip:** `1.835e-5` is an observed maximum on
the surrogate, not "the largest possible promotion". R-A3 should reference
R-A1's derived `2^-10`; the observation belongs in the results column.

**R16 — Note the Spearman diagonal caveat if R-A5 is retained.**
`src/g6.rs:485-486` seeds both flat vectors with `1.0` on the diagonal, so the
reported ρ includes 45 tied self-pairs and is optimistic relative to a
diagonal-excluded Spearman. Harmless for a non-gated diagnostic, but the reported
0.999999226 should not be read as a tight bound.

**R17 — Amend §9** to mark `2^-13` (underived), the subnormal threshold
(goal/bound mismatch), and `k = 1/5/10` (inherited, incomplete) rather than
asserting that every threshold traces to binary16's structure or to an identity.

---

## 13. Final verdict

# ACCEPT WITH CHANGES

**The core approach is sound and materially better than C1–C10.** I re-derived
the mathematics from the binary16 specification rather than from the document and
found no error in it: the component bound `2^-11` is correct and *exactly tight*
(measured ratio 0.9993), the cosine bound `2^-10` is correct and conservative
(worst observed ratio 0.32 across seven dimensions and fourteen adversarial
constructions), and the triangle-inequality structure of the pair bound is
correct — including the non-obvious detail that the worst case sits at `cos ≈ 0`
rather than at high cosine, because `|Δcos| ≈ sin θ · Δθ`. I also found no
circular reasoning and no `observed × k` threshold among the load-bearing
criteria.

**Three defects block the real-corpus rerun.** The derived floor `2^-9` is
calibrated to binary16's worst case rather than to retrieval sensitivity, and is
wider than the band in which the reference system is *documented* to change top-1
in 20 % of seeds (R2). `k = {1,5,10}` does not cover the product's own proposed
`limit=20` / `limit=12` queries, leaving ranks 11–20 entirely ungated (R3). And
the derivations carry an unstated precondition — the f16 normal range — while the
criteria tables say "Corpus: any", which reads as an unconditional format
guarantee and is contradicted by G-6's own `subnormal-boundary` fixture at 81× the
bound (R1).

**The deeper weakness is evidential, not mathematical.** The surrogate produced
**zero** top-k changes at every dimension, so the entire R-B1…R-B5 machinery has
never been run against a case that could fail it. G6A §2 leans on "B came out in
the opposite direction from A" to argue that B's criteria were the mis-specified
ones; B's perfection is a property of the generator's geometry, not evidence
about the encoding. G6A §8 does correctly state that R-B* requires the real
corpus — but §2 and §11 should not treat B's clean bill of health as information.

**None of this changes the formal state.** `G-6: FAIL — KEEP F32` remains correct
and open. The reference encoding remains `f32le`. Nothing here approves f16, and
the arguments in this review do not constitute a case for adoption — R2's real
content is that the *measured* floor sits 3.6× below the mean cosine drift a
single model patch hop produces, which is an argument about how f16 compares to
**model instability**, not an argument that f16 is retrieval-safe. That question
is R-B*'s to answer, on the real corpus, with the band population reported.

---

## 14. Recommended next experiment

**Do not re-run G-6 until R1, R2 and R3 are settled.** Two of them (R1, R3) are
specification edits; R2 is a measurement that the recorded run already supports —
`ε` is a post-processing computation over `embeddings.json`, no model download,
no inference, no new corpus.

### Step 1 — Close the specification (no compute)

1. Apply R1 (precondition column), R3 (`k` coverage), R4–R7 (definitions, the
   R-B2/R-B4 merge, the presented-answer criterion), R8–R10, R12, R14–R17.
2. Resolve τ (G6A §10 decision #1). It remains a product decision; if it cannot
   be made now, record property (ii) as formally deferred rather than pending, so
   it is not silently inherited by a future gate.
3. Rule on whether R-C1 becomes a fingerprint tag (decision #4) and whether the
   format or the registry enforces it (decision #5). These are format-owner calls
   with their own sign-off; they are not the experiment's to make.

### Step 2 — Re-run G-6 on the recorded corpus, under R-*

Post-processing only, no model. Evaluate:

- **R-A1…R-A4** at 1280-D (the model's native width) and, for coverage of the
  `1/√d` claim, at the surrogate's 512 and 16 — labelled as surrogate, not model,
  so the corpus distinction in §8 stays visible.
- **R-B1…R-B6** on the real corpus, **reporting the band population per seed and
  per `k` (R11)**. This is the deliverable that matters: if the band is empty,
  the honest result is *"R-B2/R-B4 untested on this corpus"*, not *"passed"*.
- **R-C1** as a registry conformance test, **plus the sparsity report (R13)**,
  since a concentrated profile is the one case where R-A1's margin thins by an
  order of magnitude.

### Step 3 — What each outcome means

| Outcome | Consequence |
| --- | --- |
| R-A* pass, R-B* pass with a **non-empty** band population, R-C1 pass | f16 becomes a defensible candidate for a *new* profile — still a `tag 11` change, a new fingerprint, a new partition, a re-analysis, never a conversion. G-7 is closed as a technical release gate (ADR 0017 §10.5); human review is optional product feedback, not a requirement. |
| R-A* pass, band population **empty** | inconclusive. The rerun has measured arithmetic, not retrieval. Report as such; do **not** report a pass. |
| R-B* fails above the measured floor | G-6 stays FAIL. The `2ε` cross-check against `2^-9` is what makes this a real failure rather than a measurement artefact. |
| R-C1 fails for a candidate profile | that profile may not declare `f16le`, regardless of its corpus numbers. This is the criterion's main job. |

**And in every case:** `G-6: FAIL — KEEP F32` stays the formal state until a human
rules on a complete R-* result. This review is an input to that decision, not a
substitute for it.
