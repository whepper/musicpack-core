# G-6A — review and repair of the f16 acceptance criteria

> **Status: CRITERIA REVIEW. Not a decision, and not a re-run.**
>
> The completed experiment is [`G6_F16.md`](G6_F16.md), whose verdict stands
> unchanged: **`G-6: FAIL — KEEP F32`**. G-6 remains **OPEN**.
>
> This document asks a narrower question than G-6 did: **were C1–C10 the right
> instruments?** It revises the criteria. It does not re-run the experiment, does
> not re-judge f16, does not touch the numerical implementation, and does not
> claim that anything passes.

---

## 1. What is preserved

| Artefact | Digest | Status |
| --- | --- | --- |
| `fixtures/g6/REPORT.txt` | `921e58f1c9e3ad4127ce87c0…` | unchanged |
| `fixtures/g6/RUN.txt` | `5aa9f0304f84b640b3f65910…` | unchanged |
| `G6_F16.md` §2 criteria C1–C10 | — | unchanged, still the historical G-6 criteria |
| `G6_F16.md` §9 verdict | — | unchanged: FAIL, 8 pass / 2 fail |

`src/g6.rs` is unchanged. `the_committed_report_matches_the_current_code` and
`the_report_is_byte_identical_across_independent_runs` still pass, so the original
experiment remains reproducible and auditable. Anything proposed below is
labelled **G-6A-R* (revised)** and is for a *future* G-6 re-run, not for
retro-fitting the completed one.

## 2. The root defect: three different properties shared one threshold budget

C1–C10 were not wrong so much as **conflated**. They mixed three properties that
have different failure modes, different remedies, and different evidence
requirements:

| | Property | Question | Failure looks like |
| --- | --- | --- | --- |
| **A** | **Numerical fidelity** | How much do the numbers move? | scores shift, norms drift |
| **B** | **Retrieval fidelity** | Do users get the same neighbours? | a different track is offered |
| **C** | **Representational safety** | Is f16 *usable* for this kind of vector? | silent collapse, resolution loss |

The decisive observation from the completed run: **A and B came out in opposite
directions on the two failing criteria.** C2 (a category-A-flavoured criterion,
stated as an absolute level) failed while A was excellent — `max abs(Δcos)`
1.835e-5, fifty-four times inside budget. C6 (a category-B criterion, stated over
the whole candidate universe) failed while B was perfect — every top-10 set
identical, maximum rank displacement zero. A criterion that can fail when the
property it names is pristine is measuring the wrong thing.

The revised set below separates A, B and C, and gives each its own evidence
requirement.

---

## 3. Assessment of C1–C10

| ID | As written | Verdict | Reasoning |
| --- | --- | --- | --- |
| **C1** | `max abs(Δcos)` ≤ 1e-3 | **sound, but the threshold was undocumented** | Correct property, correct category. 1e-3 sits within 2.4 % of the derived binary16 worst case 2^-10 = 9.766e-4, so the number is defensible — but nothing in `FORMAT_SPEC.md` §7.3 says so, so it reads as arbitrary. Repaired as **G-6A-R-A1** with the derivation attached. |
| **C2** | min cosine, f16 world ≥ 0.99 | **mis-specified** | Measures the corpus, not the encoding: the f32 reference fails it identically (§4). |
| **C3** | Spearman ρ ≥ 0.999 over the full pairwise order | **redundant with C6, and needs clarification** | Spearman over the full order and the swap count are two summaries of the same event set. A smooth global correlation also cannot distinguish a reordering inside the noise floor from a real retrieval change. Retained as a **diagnostic**, not a gate. |
| **C4** | mean top-10 Jaccard ≥ 0.98 | **sound, and weaker than necessary** | Real retrieval property, right category. But "98 %" admits two changed top-10 sets per 100 seeds, where the reference could resolve the difference. Repaired as **G-6A-R-B2**, which demands *identity* within the resolvable band. |
| **C5** | top-1 unchanged ≥ 90 % | **sound in intent, redundant as written** | Top-1 preservation is the most important single retrieval property. But 90 % is arbitrary, and 10 % of seeds may change their top-1 for reasons unconnected to resolvability. Repaired as **G-6A-R-B2**. |
| **C6** | relative-order swaps ≤ 0.1 % of all pairs | **mis-specified** | Counts reversals of pairs the reference representation cannot resolve (§5). A percentage over the whole universe also gates on orderings no query can surface. Repaired as **G-6A-R-B1**. |
| **C7** | mean top-5 Jaccard ≥ 0.97 | **sound, and weaker than necessary** | As C4, one level tighter. Repaired into **G-6A-R-B2**. |
| **C8** | mean top-1 unchanged ≥ 90 % | **redundant — identical to C5** | Verified in code: `mean_jaccard(1)` averages a per-seed Jaccard over a single-element set, so it is algebraically `1 − top1_changed()/seeds`. C5 and C8 are the same number with a different name. **Delete C8.** |
| **C9** | max rank displacement ≤ 10 | **sound, needs clarification** | Not implied by C4: a member can stay inside a top-10 while moving several positions. Keep, but bound it by resolvability like the rest, and state whether the bound is over the whole list or the top-k. Repaired as **G-6A-R-B3**. |
| **C10** | mean `abs(Δcos)` ≤ 1e-4 | **sound, threshold undocumented** | Correct category and a useful complement to C1 (typical vs worst case). 1e-4 is not derived from anything recorded. Repaired as **G-6A-R-A2**, with the threshold justified as a regression bound rather than a physical limit. |

**Net:** 1 sound-but-unjustified (C1), 1 sound-but-redundant (C8), 2
mis-specified (C2, C6), 3 sound-but-weaker-than-necessary (C4, C5, C7), 1
redundant-with-C6 (C3), 1 needing clarification (C9), 1 sound-but-unjustified
(C10). No criterion is discarded for being inconvenient; two are discarded for
being **duplicated** (C3 as a gate, C8 outright).

---

## 4. C2: what it was protecting, and why it cannot measure it

### 4.1 The two properties it was trying to protect

`FORMAT_SPEC.md` §7.3 records C2's intent in its own parenthetical: *"quantization
must not push distinct tracks into 'unrelated' or create false near-duplicates"*.
That is **two** properties, and they are different in kind:

- **(i) No false near-duplicates** — quantization must not make two unrelated
  tracks *look* nearly identical. This is a claim about **upward** movement in
  cosine, and it is fully expressible without knowing anything about the corpus:
  > no pair's cosine may increase by more than the established error bound.
- **(ii) No demotion below relevance** — quantization must not push genuinely
  similar tracks into "unrelated". This is a claim about **downward** movement
  *relative to a relevance threshold*, and the threshold is a **product**
  decision that has not been made.

### 4.2 Why the criterion as written measured neither

C2 states an **absolute level** — every pair ≥ 0.99 — rather than a **change**.
The completed run shows why that is unusable:

| | 1280-D | 512-D | 16-D |
| --- | ---: | ---: | ---: |
| min cosine, f32 reference | −0.076395817 | −0.132554457 | −0.271821439 |
| min cosine, f16 world | −0.076397888 | −0.132560328 | −0.271796405 |
| movement | 2.07e-6 | 5.87e-6 | 2.50e-5 |

The f16 world is within 2.1e-6 of the f32 reference at the extreme. **A
criterion that the unmodified reference representation also fails is a statement
about the corpus.** No realistic 45-track corpus spanning 15 albums has every
pair above 0.99: cross-album pairs are not, and should not be, above 0.99. C2 as
written therefore cannot be satisfied by *any* representation of a realistic
corpus, f16 or otherwise. It is unsatisfiable rather than merely strict.

### 4.3 The quantization-relative form

Property (i) becomes a sound criterion immediately:

> **No pair may be promoted toward similarity by more than the established error
> bound ε.** Empirically, at 1280-D the largest possible promotion is
> `max abs(Δcos) = 1.835e-5`, and the observed extremes moved *downward* (the
> minimum cosine became slightly more negative at all three dimensions), with one
> cosine sign crossing in 1980 pairs at 512-D and none at 1280-D or 16-D. Nothing
> was promoted toward a near-duplicate.

Property (ii) can only be stated once a relevance threshold τ exists. The
quantization-relative form is:

> **No pair whose f32 reference cosine lies within ε of τ may cross τ.**

That is exact and testable — but it needs τ, and τ is the open product decision
(ADR 0016 Slice 0: who asks for similar tracks and what counts as success). **So
property (ii) is not yet definable, and G-6A does not invent it.** It is carried
as an open decision in §10.

### 4.4 Conclusion on C2

C2 is **not** measuring quantization impact. It is measuring embedding-model and
corpus quality, which is a real property but not this gate's, and it is
unsatisfiable as written. Property (i) is repaired as **G-6A-R-A3**; property
(ii) is deferred pending a product decision.

---

## 5. C6: the quantization noise floor

### 5.1 The two kinds of reversal are not the same event

A reversal between candidates `a` and `b` for query `q` happens when
`sign(cos_f32(q,a) − cos_f32(q,b))` differs from
`sign(cos_f16(q,a) − cos_f16(q,b))`. Whether that matters depends entirely on the
**reference separation** `m = |cos_f32(q,a) − cos_f32(q,b)|`:

- If `m` is far above the quantization error, the reference *determined* that
  order, and losing it is a genuine retrieval defect.
- If `m` is far below it, the reference did **not** determine that order. Any
  representation whose arithmetic is not bit-identical will reorder those
  candidates, including a change of f32 accumulation order. Counting them as
  "ranking damage" penalises the encoding for a property the reference never had.

C6 counted both equally.

### 5.2 The resolvability bound is derived, not chosen

Let `u` and `w` be unit vectors and `u'`, `w'` their binary16 round trips. A
normal binary16 value satisfies `|Δ| ≤ ulp/2 ≤ 2^-11 |v|`, so for a unit vector
`|u − u'| ≤ 2^-11`. For unit vectors the standard perturbation bound gives
`|cos(u,w) − cos(u',w')| ≤ |u−u'| + |w−w'| ≤ 2^-10`.

A nearest-neighbour query compares **two stored** vectors, so each of the two
cosines carries its own error and the *relative* movement of the pair is bounded
by the triangle inequality:

```text
|[cos(q',a') − cos(q',b')] − [cos(q,a) − cos(q,b)]|  ≤  2^-10 + 2^-10  =  2^-9
```

**Therefore: if the f32 reference separation `m` exceeds `2^-9 ≈ 1.953e-3`, the
f16 order is preserved as a matter of arithmetic, not of statistics.** This is the
resolvability floor. It is derived from binary16's 11-bit significand before any
measurement, and it holds for *any* corpus.

### 5.3 Every observed reversal was inside the floor

Using the corpus-specific ε as well, which is the tighter test:

| Corpus | ε = `max abs(Δcos)` | Resolvability floor 2ε (observed) | Derived floor 2^-9 | Largest observed reversal margin `m` | Verdict |
| --- | ---: | ---: | ---: | ---: | --- |
| 1280-D | 1.835e-5 | 3.670e-5 | 1.953e-3 | 2.491e-6 | all unresolvable |
| 512-D | 3.424e-5 | 6.848e-5 | 1.953e-3 | 3.840e-5 | all unresolvable |
| 16-D | 1.401e-4 | 2.802e-4 | 1.953e-3 | 3.690e-5 | all unresolvable |

All **ten** observed reversals across all three corpora — including the two
closest calls, 3.840e-5 at 512-D and 3.690e-5 at 16-D — fall below the observed
floor, and every one falls at least 14× below the corpus-independent derived
floor. **There were zero reversals among resolvable pairs at every dimension.**

The conclusion does not depend on which floor is used, which is the property that
makes it trustworthy.

### 5.4 Universe or retrieval-relevant ranks?

C6 ranged over the entire candidate universe. A reversal at rank 40 of 45 cannot
change any query result, cannot alter an album aggregate, and cannot surface in
any API. Gating on it tests a property no product depends on, and it is the
mechanism by which C6 failed: the reversals were all in the deep tail, where
cosines sit near zero and the ordering is arbitrary.

**G-6A-R-B1 therefore applies to the retrieval-relevant region only** — the
union of the top-k across all queries, plus the members of every album aggregate.
The full universe is retained as a reported diagnostic.

### 5.5 Is a zero-swap requirement appropriate for resolvable pairs?

**Yes, and this is the one place where a zero threshold is not a wish.** Above the
derived floor the order is preserved by the triangle inequality; there is no
distribution to hedge against and no tolerance to choose. A non-zero budget there
would be asserting that arithmetic failure is acceptable.

### 5.6 What happens to pairs inside the floor

They are **excluded from the ordering requirement, not exempted from
observation.** The requirement becomes: for each seed and each rank boundary, the
*set* of candidates whose f32 reference score lies within the floor of the
boundary score must be identical in both worlds. Their internal order is
unconstrained, because the reference does not determine it. This is a real
constraint — it forbids a candidate from escaping the band — and it is the
correct one.

---

## 6. The f16 subnormal regime (category C)

### 6.1 What the evidence shows

| Component magnitude | f16 relative error |
| --- | ---: |
| ≥ 1.3e-2 (the whole normal range) | 1.149e-4, constant |
| 1.6e-5 | 1.838e-3 |
| 1.0e-6 | 7.353e-3 |
| 5.1e-7 | 1.765e-1 |
| 2.5e-8 | 1.000e0 — rounds to zero |

The constructed `subnormal-boundary` case moved a cosine by **7.954e-2**, four
orders of magnitude worse than anything else in the suite. Below `2^-14` a
binary16 value is a subnormal, quantized to integer multiples of `2^-24`, so
relative precision degrades linearly as the value shrinks instead of staying at
the half-ulp.

### 6.2 Why this does not threaten the format, quantitatively

A unit vector in dimension `d` has components of order `1/√d`. The binary16
normal range begins at `2^-14 = 6.104e-5`, and

```text
1/√d ≥ 2^-14   ⇔   d ≤ 2^28 ≈ 2.68 × 10^8
```

Any L2-normalized embedding of dimension up to ~268 million has its components in
the normal range. The measured 1280-D corpus sits at ≈ 0.028 ≈ 2^-5.2, roughly
2^9 above the knee. This is why `max abs(Δcos)` came out at 1.8e-5 rather than at
the constructed 8e-2.

### 6.3 The architectural question

> What must a future similarity profile guarantee before it may declare `f16le` as
> its storage representation?

Three candidate requirements, assessed against the evidence rather than general
knowledge:

| Candidate | Assessment |
| --- | --- |
| **L2 normalization before serialization** | **Required, and already satisfied.** The producing pipeline ends with track L2 (`eval::pool_mean_norm`), which is what places components at `1/√d` and clear of the knee. Without it, a vector with small overall magnitude is entirely in the subnormal regime. This is the load-bearing requirement. |
| **A minimum component-magnitude check per document** | **Not recommended as a per-component floor.** A single component at 1e-8 in a unit vector contributes ~1e-9 to the cosine and rounds to zero harmlessly. A floor on *every* component would forbid legitimate sparse representations for no benefit. |
| **An explicit prohibition on profiles that rely on f16 subnormals** | **Adopted in a different, checkable form:** a profile declaring `f16le` must guarantee that the **subnormal energy fraction** of its vectors is negligible, and the registry must verify that on reference vectors. The threshold is derivable from §6.1: to keep the induced cosine error within one half-ulp relative precision, the subnormal energy fraction φ must satisfy `√φ ≤ 2^-12`, i.e. `φ ≤ 2^-24 ≈ 5.96e-8`. |

### 6.4 G-6A-R-C1, proposed

> A profile that declares `f16le` in fingerprint tag 11 **MUST** guarantee, and
> the profile registry **MUST** verify on reference vectors, that:
>
> 1. stored vectors are L2-normalized before serialization;
> 2. the fraction of vector energy in components below `2^-14` is at most
>    `2^-24`; and
> 3. no component that carries non-negligible energy lies in the subnormal range.

This is a **profile** requirement, not a container requirement. The container
format already specifies the conversion exactly; what it cannot know is whether a
particular profile's numbers are safe in binary16, and that is the registry's
job — the same division the two-identifier discipline already establishes.

**This is a proposed architectural addition to the profile contract, and it
requires sign-off.** It is not a change to `FORMAT_SPEC.md`, `FORMAT_SIGNOFF.md`
or ADR 0017, all of which are closed and untouched by this document.

---

## 7. Proposed G-6 criteria

For a **future** G-6 re-run, after human approval. These do not replace the
historical C1–C10, which remain the record of what G-6 actually tested.

### 7.1 Category A — numerical fidelity

| ID | Property | Proposed metric | Threshold | Rationale | Corpus |
| --- | --- | --- | --- | --- | --- |
| **R-A1** | Worst-case cosine movement | `max abs(Δcos)` over all pairs and seeds | **≤ 2^-10 = 9.766e-4** | Derived: unit-vector round trip has relative error ≤ 2^-11, and the cosine perturbation bound is `2 × 2^-11`. Independent of any corpus or result. Observed 1.835e-5, i.e. 53× inside. Had the observation exceeded this, the criterion would fail — the derivation does not move. | any |
| **R-A2** | Typical cosine movement | `mean abs(Δcos)` | **≤ 2^-13 = 1.221e-4** | A regression bound, not a physical limit: the half-ulp relative precision is 2^-11, and averaging over `d` components with partly-cancelling errors should land near or below `2^-13`. Observed 4.743e-6, 26× inside. Chosen from binary16's precision structure, not from the observation. | any |
| **R-A3** | No false near-duplicates (C2 property i) | `max (cos_f16 − cos_f32)` over all pairs, one-sided | **≤ 2^-10** | The upward-only form of R-A1, which is what C2's "no false near-duplicates" intent actually protected. One-sided because promotion is the harmful direction. Derived, not observed. | any |
| **R-A4** | Norm perturbation | `max abs(‖v'‖ − 1)` | **≤ 2^-10** | Derived from the same bound: a unit vector's norm can move by at most `2 × 2^-11`. This was measured in G-6 as a diagnostic (2.118e-5) but never gated. | any |
| **R-A5** | Full-order agreement | Spearman ρ over the full pairwise order | **reported, not gated** | Cannot distinguish a noise-floor reordering from a retrieval change, and its value is corpus-size dependent. Retained as a diagnostic; C3's gate is withdrawn. | any |

### 7.2 Category B — retrieval fidelity

The resolvability floor is **2^-9 = 1.953e-3** by the derivation in §5.2. All
bands below use it.

| ID | Property | Proposed metric | Threshold | Rationale | Corpus |
| --- | --- | --- | --- | --- | --- |
| **R-B1** | No resolvable order is lost | reversals among pairs in the retrieval-relevant region whose reference separation exceeds 2^-9 | **zero** | Above the derived floor the order is preserved by the triangle inequality, not statistically. A non-zero budget would assert that arithmetic failure is acceptable. This is C6 repaired. | **real corpus required** |
| **R-B2** | Resolvable neighbourhoods are identical | for each seed and each `k ∈ {1,5,10}`, the set of candidates within 2^-9 of the k-th reference score | **identical in both worlds, for every seed** | Replaces C4/C5/C7 with *identity* rather than 90–98 % overlap — stronger, because the relaxation applies only where the reference cannot resolve. Directly expresses "the user gets the same neighbours". | **real corpus required** |
| **R-B3** | No resolvable member moves | max rank displacement among members in both top-k lists, restricted to the resolvable region | **0 positions** | Replaces C9 with a resolvability restriction. Zero is defensible here for the same reason as R-B1: above the floor, displacement is impossible. | **real corpus required** |
| **R-B4** | Nothing escapes the unresolvable band | for each seed and boundary, the *set* of candidates within 2^-9 of the boundary must be preserved | **identical** | The constraint that replaces "order inside the band", which is deliberately unconstrained (§5.6). Forbids a candidate leaving the band while permitting it to move within it. | **real corpus required** |
| **R-B5** | Album aggregate is unaffected | `cos(aggregate_f16-stored, aggregate_f32-stored)` | **≥ 1 − 2^-10** | The server derives aggregates from stored vectors, so quantization error accumulates through the mean. G-6 measured 1.000000000000. Kept because v1.0 documents carry no aggregate (decision B) but the server still computes one. | **real corpus required** |

### 7.3 Category C — representational safety

| ID | Property | Proposed metric | Threshold | Rationale | Corpus |
| --- | --- | --- | --- | --- | --- |
| **R-C1** | Profile is safe for binary16 | profile registry conformance test over reference vectors | the three guarantees in §6.4 | Not a corpus statistic. A profile that cannot pass this test must not declare `f16le`, whatever its corpus numbers look like. | profile reference vectors |
| **R-C2** | The subnormal regime is characterised, not assumed | documented relative-error curve (§6.1) | informational, gated as documentation | Prevents a future profile author from assuming binary16 is uniformly 11-bit accurate. | none |

### 7.4 What the revised set deliberately does not do

- It does **not** use any observed value as a threshold. R-A1 and the 2^-9 floor
  are derived from binary16's significand width; R-A2 from its precision
  structure. The observations are reported as evidence that the bounds are not
  near-binding, not as their source.
- It does **not** weaken retrieval. R-B1 through R-B4 are *stricter* than C4, C5,
  C6, C7 and C9: they demand identity where C4/C5/C7 permitted 2–10 % change.
  The relaxation is confined to orderings the reference cannot resolve, and R-B4
  keeps a constraint there.
- It does **not** invent a relevance threshold. C2 property (ii) is absent
  because it is not definable yet (§10).
- It does **not** assert f16 is acceptable. Whether the revised set passes is a
  separate, future question requiring the real corpus.

### 7.5 What would falsify the revised set

Stated so it is not unfalsifiable: R-A1 fails if any vector population produces
cosine movement above 2^-10; R-B1 fails if any resolvable ordering reverses; R-C1
fails for any profile whose vectors are not L2-normalized or carry subnormal
energy. A real-corpus run that produced reversals above the floor would be a
genuine finding against f16, and the correct response would be to keep `f32le`.

---

## 8. Surrogate versus real MusicPack corpus

| Established on the deterministic surrogate | Still unverified on the real corpus |
| --- | --- |
| R-A1…R-A4 arithmetic behaviour; the `1/√d` scaling; the resolution limit (f32, not f16, binds at d ≥ 16) | whether a real 45-track / 15-album corpus stays inside R-A1…R-A4 |
| the subnormal failure mode and its quantification (§6) | nothing — §6 is analytic and corpus-independent |
| that C2 is unsatisfiable for a realistic corpus | **nothing.** C2 must **not** be considered settled: its value depends entirely on the real corpus's cosine distribution |
| that every observed reversal sat inside a noise floor | **nothing.** R-B1…R-B5 are the criteria that need the real corpus, and the surrogate cannot stand in for them |

**C2 and C6, in their original form, remain unsettled.** The surrogate showed
that C2 is unsatisfiable *in general* — the f32 reference fails it too, which is
a structural argument, not a corpus artefact — but the exact margin by which a
real corpus fails it is unmeasured, and a human deciding to retire C2 should see
the real distribution first.

---

## 9. Threshold provenance summary

| Threshold | Source | Independent of the observed result? |
| --- | --- | --- |
| 2^-11 relative component error | binary16 has 11 significand bits; RNE half-ulp | yes |
| 2^-10 = 9.766e-4 (R-A1, R-A3, R-A4) | `2 × 2^-11`, the unit-vector cosine perturbation bound | yes |
| 2^-9 = 1.953e-3 (resolvability floor) | triangle inequality over two stored-vector cosines | yes |
| 2^-13 = 1.221e-4 (R-A2) | regression bound from binary16 precision structure | yes |
| 2^-24 ≈ 5.96e-8 (R-C1 energy fraction) | `√φ ≤ 2^-12` from the half-ulp precision target | yes |
| 2^-28 ≈ 2.68e8 (dimension at which `1/√d` reaches the normal range) | `1/√d ≥ 2^-14` | yes |
| zero (R-B1, R-B3) | arithmetic identity above the floor | yes |
| identical (R-B2, R-B4) | definitional | yes |

Every proposed threshold traces to binary16's structure or to an identity. None
is `observed × k`. **The one threshold G-6A cannot derive is the relevance
threshold τ** required for C2 property (ii), and it is not proposed.

---

## 10. Unresolved product and format decisions

| # | Decision | Owner | Why it is not decided here |
| --- | --- | --- | --- |
| 1 | **A relevance threshold τ** for "similar enough to be a neighbour" | product (ADR 0016 Slice 0) | No user story, no corpus, no consumer. C2 property (ii) cannot be stated without it. |
| 2 | **Whether to retire C2 outright or restate it as R-A3 + a τ-conditional criterion** | human reviewer | Depends on (1). |
| 3 | **Whether the resolvability floor uses the derived 2^-9 or a measured corpus bound** | human reviewer | The derived bound is corpus-independent and conservative; a measured bound is ~53× tighter and more informative, but only valid for the corpus it was measured on. |
| 4 | **Whether R-C1 enters the profile fingerprint as a new tag** | format owner | ADR 0017's 14-tag list is closed. A new tag changes every profile fingerprint and is a format decision requiring its own sign-off. |
| 5 | **Whether the format or the profile registry enforces R-C1** | format owner | A per-document check is possible but the registry is the layer that knows what the numbers mean. |
| 6 | **Whether the revised criteria apply retroactively to C3's withdrawal** | human reviewer | Withdrawing a gate is as much a decision as setting one. |

---

## 11. Verdict

The completed experiment found that C1, C3, C4, C5, C7, C9 and C10 are sound or
sound-but-redundant, that **C2 and C6 were mis-specified and failed for reasons
unconnected to f16**, and that one of them (C2) is unsatisfiable for any
realistic corpus. The repaired set separates numerical fidelity, retrieval
fidelity and representational safety, derives every threshold from binary16's
structure rather than from the observation, and is *stricter* than the original
where it matters — demanding identity rather than 90–98 % overlap wherever the
reference representation can actually resolve a difference.

One part of the original intent — C2's "do not demote genuinely similar tracks
into unrelated" — **cannot be repaired**, because it requires a relevance
threshold that is an open product decision. That is recorded as such rather than
filled with an invented number.

G-6 is unchanged: **FAIL — KEEP F32**, and open. The reference storage encoding
remains `f32le`. Nothing in this document re-runs the experiment, re-judges f16, or
alters the closed format gate.

**G-6A: CRITERIA REPAIRED — READY FOR REVIEW**
