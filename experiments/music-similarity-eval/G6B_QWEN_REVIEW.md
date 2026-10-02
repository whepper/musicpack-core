# G-6B — final adversarial red-team review (third independent reviewer)

> **Formal status at review time — unchanged by this document:**
>
> `G-6: FAIL — KEEP F32`
>
> `G-6B: METHODOLOGY READY FOR REVIEW`
>
> This is a review of the *methodology and instrument* for the planned
> real-corpus run. It is not a re-run, not a decision, and not criteria
> evidence. Nothing here modifies `G6B_METHODOLOGY.md`, `G6A_CRITERIA_REVIEW.md`,
> `G6A_GLM_REVIEW.md`, `G6_F16.md`, `FORMAT_SPEC.md`, `FORMAT_SIGNOFF.md`,
> ADR 0017, `fixtures/g6/*`, `fixtures/g6b/*`, or any production code.
>
> **Verification performed:** every claim below was checked against the actual
> repository contents — the methodology, both prior criteria documents, both
> prior independent reviews, the historical artefacts, `src/g6b.rs`,
> `src/g6.rs`, `src/g6b_tests.rs`, `src/eval.rs`, the `docfmt` binary16
> conversion and its write/read paths, `src/bin/g6.rs`, `src/bin/g6b.rs`,
> `src/lib.rs`, `report.rs`'s `embeddings.json` schema, and ADR 0017 §5.7/§5.8.
> The instrument tests were re-executed locally and pass:
> `g6b_tests` 10/10, `g6_tests` (artefact pins) pass, and
> `docfmt_tests::binary16_bit_patterns_are_the_standard_ones` /
> `binary16_rounding_is_to_nearest_with_ties_to_even` pass.

---

## 1. Verdict

### **READY WITH IMPORTANT NOTES**

**No blocker found.** The planned G-6B real-corpus experiment can answer its
question, and the frozen methodology, as written, cannot produce a misleading
PASS on the population it measures: the retrieval gates (B-B1/B-B2) compare the
presented ranked lists directly, in the symmetric storage-path regime, under an
identical ranking procedure in both worlds. That is the correct instrument for
"does replacing stored f32 with binary16 change what MusicPack returns", on the
declared reference corpus, under the declared product semantics (exact identity;
D-2 not assumed).

Five **IMPORTANT** findings must be recorded in the run's documentation and
implementation plan **before** execution (§13 below). None requires a change to
the methodology text, and none reopens a frozen threshold. Several are already
anticipated by the methodology (§9's runner requirement, §13's known gaps,
§15's limitations); this review sharpens them and adds two the methodology does
not state (the B-C2 budget-composition gap, and the album-input derivation
rule).

I explicitly did **not** find: a storage-simulation mismatch, a retrieval-
semantics mismatch against ADR 0017 §5.7/§5.8, a gate that can pass while a
measured presented result changes, a post-hoc threshold, a same-data loophole,
or an accidental universal claim.

---

## 2. Storage simulation audit — **NO ISSUE**

The modeled path (§4: `f32 → docfmt::f32_to_f16_bits → docfmt::f16_bits_to_f32
→ eval::cosine`) is bit-faithful to the actual `.msim`/`.mpack` f16le storage
path. Item by item:

| Check | Result |
| --- | --- |
| Both operands quantized | **Yes.** `compare_storage_path` ranks and pairs over `stored` on both sides (`g6b.rs:118-163`); the historical mixed-regime quantity is carried separately as M2. This matches the architecture: ADR 0017 §5.8 makes the query a stored vector. |
| Endianness | `f32_to_f16_bits` is pure value semantics → u16; the writer emits `to_le_bytes` (`docfmt.rs:397`), the reader parses `u16::from_le_bytes` (`docfmt.rs:601`). Endianness affects on-disk byte order only, never the decoded f32 value, so the in-memory simulation is value-identical to the file round trip. `f16be` is not defined (FORMAT_SPEC §7.1), so there is no alternative to mismatch. |
| RNE / ties-to-even | Verified against the code (`docfmt.rs:717-768`): `remainder > 0x1000 \|\| (remainder == 0x1000 && truncated odd)` for normals, the analogous half-step test for subnormals, carry at `0x400` with exponent bump and overflow re-check. Pinned by the two `binary16_*` conformance tests, which pass. |
| NaN / infinities | Rejected by the encoder (`exponent == 0xff → None`), exactly as FORMAT_SPEC §7.4 requires; the document writer validates before encoding (`docfmt.rs:315-327`). `f16_bits_to_f32` widens NaN/Inf correctly, but the round trip cannot produce them, so decode-side handling is never load-bearing. |
| Signed zero | ±0 → ±0 exactly (both f32 zeros and f32 subnormals map to `sign`); squared away in norms; harmless and faithful. |
| Subnormals | Produced and supported; `k·2^-24` path ends at `k = 1024`, which is the smallest normal's bit pattern — correct, no special case needed; values below `2^-25` → signed zero (correct RNE); f16 subnormals decode exactly via renormalization in `f16_bits_to_f32`. |
| Overflow | Rejected, not saturated (`unbiased > 15`, and the `significand == 0x400` carry at `unbiased == 15`). Unit vectors cannot hit it (components ≤ 1.0), but B-C3 measures it anyway — correctly. |
| Renormalization | **Not** applied, on both worlds' inputs to cosine; §4's stated reasons (bit-exactness with §7.4; norm perturbation is itself gated by B-A2) are right, and `eval::cosine` dividing by both norms makes the comparison scale-robust. Matches actual storage: the server stores and decodes exactly what Author encoded. |
| f64 accumulation | `eval::cosine` accumulates `dot`, `norm_a`, `norm_b` in f64 over f32 inputs and returns f32 — the same procedure in both worlds, so the A/B delta is purely a storage-path effect. |
| Cosine implementation | Reused unchanged (no reimplementation), as RUN.txt and §4 state. |
| Any gap that could invalidate the result | **None found.** The one place where the simulation is *ahead* of reality is that no production f16 consumer exists yet; but every specified layer (encoder, decoder, framing, cosine policy) is the one modeled. The JSON provenance risk (recorded f32 values re-read from `embeddings.json`) is neutralized by the mandated per-track `embedding_sha256` re-verification (§13), which pins bit-exact f32 values (`f32_le_bytes` digest). |

Classification: **NO ISSUE.**

---

## 3. Retrieval semantics audit — **NO ISSUE**, with one recorded dependency

Checked against ADR 0017 §5.7 (server: exact cosine over a profile-partitioned
table, no ANN, fingerprint never mixed, `similarity_album_vectors` computed
server-side) and §5.8 (sketch: `limit=20` tracks / `limit=12` albums, responses
carry `rank` and `score`, raw vectors never exposed):

* **Result ordering** — `eval::nearest` sorts by f32 score `total_cmp` descending
  and truncates. `compare_storage_path` takes `top_k = full − 1` (full ranking)
  and compares the *ordered k-prefixes*. A truncated ranking is a prefix of the
  full ranking under the same comparator, so the k-prefix comparison is exactly
  the presented list. ✔
* **Deterministic tie-breaking** — index tie-break, identical procedure both
  worlds; every new exact tie is recorded (M12). ✔ for the declared procedure.
* **Duplicate candidates** — two equal vectors produce an exact tie resolved by
  index, identically in both worlds; the census would surface any split. ✔
* **Query exclusion** — `nearest` filters `index != query`, both worlds. ✔
* **Album aggregation** — `eval::aggregate_albums` (mean then L2) is the
  server-side derivation the ADR §5.7/§5.8 shape implies, and its inputs are the
  stored vectors in each world, as §4 declares. Caveat in §6 below: v1.0 defers
  the semantics (decision B), so the gate is against the *documented candidate
  rule*, and the runner must recompute aggregates per world (not reuse the
  recorded `albums[]` block).
* **Track-vs-album semantics** — track endpoints gated at k ∈ {1,5,10,12,20};
  album endpoint at k = 12. Populations are the full 45-track / 15-album corpus
  per configured library. ✔ (adequate as an album population for this corpus;
  14 candidates ≥ 12+1 needed for the boundary margin.)
* **API limits** — k set covers both ADR-sketched limits plus the historical
  series; final set isolated as D-3. ✔

**The load-bearing question from the brief:** if f32 and f16 "have equal
similarity scores but a deterministic tie-break chooses different candidates",
is that detected? Under the declared procedure, yes: equal scores imply an index
tie-break that cannot differ between the worlds (indices are storage-invariant),
and *anything* that makes the server's presented list differ — i.e. scores that
are not actually equal at the ranking precision — is a score difference the
full-ranking comparison catches at the exact position it occurs. B-B1 catches a
presented top-1 change; B-B2 catches membership changes *and* intra-list
reorders (correct, because §5.8 responses carry `rank`); a change at ranks
beyond every evaluated k is recorded in the unconditional census (M11). The
committed 512-D surrogate artefact (`fixtures/g6b/REPORT.txt` line 38: seed 18,
ranks 11/12, margin 2.618e-5 ≈ 439 ULPs) demonstrates the detection works —
that event was invisible to G-6's k ∈ {1,5,10}.

The residual is a **scope caveat, not a defect**: "presented result" here means
the result under `eval::cosine`-returns-f32 + index-tie-break. If a future
production server ranked *finer* than the f32 score it reports (f64 ordering,
tag-14 numeric policy), a pair whose f64 orders oppose between the worlds while
both worlds' f32 scores tie would present identically in the experiment but
differ under that server — margin < ~1 f32 ULP (≈6e-8 near 1.0). Note the
subtlety: M12 does **not** capture this case, because its counting code skips
pairs whose reference scores are already equal (`reference_scores[i] !=
reference_scores[j]` guard); such events are recorded nowhere. The methodology
records the tie-break dependency (§8, §15 "Tie-break inheritance"); the
required follow-through is in note I-4.

Classification: **NO ISSUE** for the experiment's validity; see I-4 for what the
run must record.

---

## 4. Top-k audit — intentionally strict, correctly justified, **NO ISSUE**

Exact ordered identity is the correct reading of the current architecture, not
an overreach:

1. §5.8 responses carry `rank`, so order is user-visible; set identity would be
   genuinely insufficient (GLM review §6 C proved the band-set escape).
2. The strictness is a *policy echo*, not a preference: the product defines no
   interchangeability semantics today (statement 4 of §8 does not exist), so
   "a changed result is a changed result" is the only semantics-consistent
   threshold. Relaxation is isolated exactly where it belongs: **D-2**,
   product-owned, decided before any rerun, never a threshold edit after one
   (§14 rule 2).
3. The threshold is definitional, not statistical — nothing to fit to results;
   §12's provenance table is honest about this.
4. A zero/identity requirement here does not contradict the numerical bounds:
   the G-6B design deliberately does *not* claim B-A1's ceiling implies it
   (the verified `ulp_flip_case` proves the opposite implication fails: 46-ULP
   margin, 712× inside the old 2^-9 floor, presented top-1 flips). The
   near-tie-exemption idea was not merely dropped, it was shown unsafe and
   committed as a deterministic counterexample.

This reviewer considered whether exactness makes the gate unpassable-in-
principle on the real corpus (the task's "impossible to satisfy" concern):
measured boundary margins on the surrogate at k=20 bottom out at 1.775e-5 —
close to, above, the f16 error scale but the outcomes are zero-changed there;
on real embeddings it is genuinely possible that B-B2 fails at some deep k.
A FAIL would then be a *meaningful* result under the current semantics (the
methodology says so, §15 "Near-tie strictness"), not a methodological flaw.

Classification: **NO ISSUE.**

---

## 5. Numerical-gate audit (B-A1, B-A2)

**Preconditions explicit?** Yes: §5.1 states the normal-range/unit-norm
conditioning and §9 makes it checkable (this fixes the GLM review §10 complaint).
**Correct symmetric regime?** Yes — M1/M5 are measured over both-operand
quantized scores (`g6b.rs:143-152`), with the mixed quantity carried visibly.
**Cannot pass while a known numerical failure exists?** Correct by design: B-A1
is an existential gate over the full 1,980-pair population per corpus; the
subnormal collapse mode (7.954e-2, 81.5× the ceiling) would blow B-A1 instantly,
and B-C2 independently. The retrieval population is gated separately — the
"A passes so B is exempt" fallacy of G-6A is structurally gone (no code path
uses `NUMERICAL_CEILING` to suppress events; `retrieval_boundaries_cover_the_adr_sketch`
and the constant's sole usage confirm the ceiling is print-only).
**Not accidentally redundant?** B-A1 (cosine movement) and B-A2 (stored-norm
deviation) test different quantities; A2 is *implied by* the same theorem but
independently recomputed, which is the "free tripwire" design (§5.2), accepted.
**Impossible-to-satisfy from f32 reference precision?** No: measured maxima are
2.8e-5 (1280-D), 5.2e-5 (512-D) against 9.77e-4 — a 19×–35× margin; the f32
reference's own granularity (2^-24) enters nowhere in the A-gate computation
(both scores are f64-widened f32 cosines; the delta of two f32 numbers is exact).

**Findings:**

* **I-2 (IMPORTANT) — B-C2's budget arithmetic as *narrated* does not compose
  to the B-A1 bound.** §9 derives φ ≤ 2^-24 from "subnormal perturbation
  √φ ≤ *half* the normal-range budget 2^-11". As written, normal error
  (≤ 2^-11·√(1−φ)) *plus* a √φ ≤ 2^-12 term totals **1.5× the budget**, not
  1.0×; and §5.1's "dimension-free" label on |u−u'| ≤ 2^-11 is literally true
  only for strictly-normal vectors. The argument that *does* close the gap is
  the one §9 half-states in the parenthesis: a subnormal component's RNE error
  is at most **half the subnormal step, 2^-25, absolute**, so with
  `MAX_DIMENSIONS = 4096` (a normative format limit, decision C) the subnormal
  contribution is Σδ² ≤ 4096·2^-50 = 2^-38, giving
  |u−u'| ≤ 2^-11·(1+2^-17) and |Δcos| ≤ 2^-10·(1+5×10^-4) — an absolute excess of
  under 5×10^-7 over the gate constant. Consequences: (a) **no misleading PASS is
  possible** from this; (b) a knife-edge *false FAIL* is theoretically
  constructible (4096-D unit vector with φ at the B-C2 limit) but is unreachable
  by any real embedding population by three-plus orders of magnitude (real f32 norms
  deviate ~1e-7 from 1, and measured |Δcos| is 35× inside the ceiling, and the
  worst-case bound itself requires sin θ = 1 with every component rounding at
  full half-ulp in coordinated directions); (c) §11's claim that a B-A1
  violation "implies a precondition breach or instrument bug" is true *given
  the dimension cap*, which the derivation as written does not invoke. Minimum
  handling: record the corrected composition (half-step × MAX_DIMENSIONS, not
  the halving story) in the run record; treat §9/§5.1 wording as a
  documentation correction for any future revision. The gate constants
  themselves are unchanged and neither gate's evaluation depends on the fix.
* **MINOR — B-C1 tolerance vs B-A2 threshold composition.** B-A2 measures
  |‖v'‖−1| ≤ 2^-10; a reference vector sitting *at* the B-C1 limit would force
  stored deviation ≤ 2^-10 + 2^-11·(1+2^-10) ≈ 1.5×2^-10, so the two gates are
  knife-edge inconsistent (same failure direction: false FAIL only, impossible
  in practice with pipeline-normalized f32 vectors). Record, don't act.
* **MINOR — M13's population band** (±2^-10 around the k-th score) is the
  *derived* ceiling, ~50–65× wider than the measured error scale. Fine as
  labelled context; the run must not present those populations as "scores the
  f32 reference cannot resolve" — that band is the measured ε scale (~1e-5),
  which M11's per-event ULP margins already expose.

Classification: **no blocker**; I-2 is an interpretation/documentation
correction that must be recorded before execution.

---

## 6. Profile-safety audit (B-C1, B-C2, B-C3)

* **Actually evaluated from the real vectors?** In the *methodology*, yes
  (§9, §13: checked on the profile's reference vectors, i.e. the recorded
  embeddings). In the *committed instrument*, **not yet** — there is no function
  computing per-vector norm conformance (B-C1/M5-reference side), subnormal
  energy fraction φ (B-C2/M15), support/participation (M15 record), or a
  counting (non-panicking) overflow check (B-C3/M16). `g6::to_f16_candidate`
  `expect()`s, which is the panic the methodology itself forbids the runner
  (§9 note, §13 known gap). This is the largest pre-run work item: see I-1.
* **Sufficient for the bounds used by B-A1/B-A2?** Yes, with §5's I-2
  composition note: C1+C3 bound the population to finite, representable,
  unit-norm vectors, and C1+C2+MAX_DIMENSIONS yields
  |Δcos| ≤ 2^-10·(1+5×10^-4). No *known* failure mode slips through the three
  gates into a situation where B-A1 could still legitimately breach.
* **Necessary?** C3 (representability) is necessary (format rejects
  unrepresentable inputs). C1 (unit norm) is load-bearing — it is what keeps
  components ~d binades clear of the subnormal knee. C2 is, as the GLM review
  already said, near-vacuous defence-in-depth *given* C1 at plausible
  dimensions; keeping it as an explicit cheap tripwire is a legitimate choice,
  not a flaw (it also survives a future profile that normalizes per-window but
  not per-track).
* **Profile-specific, not universal f16 claims?** Yes — §9's closing paragraphs
  and D-6 restrict the rule to profiles declaring `f16le`, with D-5 keeping the
  fingerprint-tag question open. The B-C gates are evaluated on *this*
  profile's vectors; nothing in the text generalizes them.
* **Sparse/concentrated vectors — the requested counterexample hunt.** Can a
  vector satisfy C1/C2/C3 and still violate an A1 assumption? The component
  relative-error bound (2^-11) *is* violated by subnormal components (that is
  exactly what C2 bounds), and concentrated vectors measurably move more error
  (GLM §8: support-2 → 1.0e-4, support-4 → 2.1e-4; §9: "3.5–7.5×"), but every
  such population remains 4.6–9.7× *inside* the B-A1 ceiling and the C-gates
  close the only route out of it. The genuinely adversarial configuration
  (thousands of components at 2^-18, φ at the 2^-24 limit) exceeds the
  |u−u'| ≤ 2^-11 row by ~2^-17 relative and cannot move a cosine beyond
  2^-10·(1+5×10^-4). **So: no blocker.** The correct statements are: (a) §5.1's
  "dimension-free" applies to the normal-range case only, (b) C2 is stricter
  than provably necessary at d ≤ 4096 (conservative direction — fine), and
  (c) the support/participation record (M15) is the honest place where sparse
  profiles must be flagged for individual scrutiny, exactly as §9 requires.
  Real Discogs-EffNet vectors are dense with exact zeros where sparse (zeros
  are error-free), so the expected real-corpus picture is: C1 ~1e-7, φ ~1e-9
  or below, zero overflows, comfortable passes — which the same-data rule
  properly forbids us from pre-declaring (§14).

Classification: **NO ISSUE** as methodology; the unimplemented C-measurements
are folded into I-1.

---

## 7. Real-corpus adequacy

Working through the brief's checklist:

* **45 tracks enough for this gate?** For the *claim as scoped*, yes. The
  retrieval gates are exact-match properties evaluated exhaustively over the
  declared reference corpus: 45 seed queries (every track is a query —
  `for seed in 0..full`, so "15 queries per profile" understates the design),
  all 44 candidates each, all 1,980 ordered pairs for M1–M7 (4 corpora →
  7,920 pairs). There is no sampling to be underpowered. What 45 tracks *can*
  establish is "no measured change in the recorded corpus"; what it cannot
  establish is bounded-size behavior at larger libraries — see I-5. That is an
  interpretation limit, and §13/§15 already carry it (small boundary
  populations; the not-evaluated rule for empty bands).
* **All pairwise comparisons needed?** Yes for category A (tails are where
  subnormal/precision effects surface, per §7.3 step 2 and GLM §12), and they
  are cheap (O(n²d), n=45). Retrieval needs full rankings per seed, which the
  instrument produces (top_k = 44).
* **15 album clusters relevant?** Yes, for the B-B2 album component and M14;
  3-member aggregates are precisely the regime the mean-cancellation argument
  (§12's R-B5 disposition, odd-symmetry of RNE) applies to. They are not needed
  for anything else; track gates are unaffected by grouping.
* **Both multi and release, both hops?** Yes, required as four independent
  corpora (§13) — adequate and correctly scoped: multi/release are two profile
  identities (different dimensions and tag values); hop 61/62 are the closest
  available "independent-but-similar" instances (GLM §12's point), which also
  lets the record note, without gating on it, that hop instability (top-1
  changed 9/45) dwarfs f16's measured effect.
* **Cross-hop comparisons needed?** **No** — vectors from different fingerprints
  are never compared or indexed together (ADR D-4/§8); the four corpora are four
  separate evidence lines, and the methodology says so. Any cross-hop number
  would be report-only context at best.
* **Album aggregation additional data?** Only what `embeddings.json` already
  carries: `artist`/`album` per track and `albums[]` provenance. **The runner
  must recompute each world's aggregates from the per-world stored member
  vectors** (as `album_aggregate_drift` demonstrates) and must NOT read the
  recorded `albums[].embedding` field — that is the recording pipeline's f32
  aggregate, and mixing it into the f16 world (or vice versa) would recreate
  precisely the mixed-regime error G-6B exists to fix, at the album level.
  See I-3.
* **Do not demand a larger corpus** — agreed: larger is better for *scale
  evidence*, but the frozen question is this profile on this recorded corpus,
  and demanding new recordings (a model-and-audio operation, §13) would violate
  the methodology's own discipline.

Classification: **NO ISSUE** for the claim as scoped; scale caveat in I-5.

---

## 8. PASS-rule audit ("every gate green, or FAIL/INCONCLUSIVE")

* **A gate that could fail on irrelevant behaviour:** the album component of
  B-B2 gates a retrieval surface v1.0 explicitly defers (decision B). The
  failure would be real arithmetic on a real candidate rule, but it is not a
  v1.0 product behaviour — see I-3 for the required attribution rule. The A/C
  knife-edge false-fails (§5) are theoretical only. Everything else is
  product-relevant by construction (presented lists, format-level
  representability).
* **A gate that could pass while an important failure occurs:** none within the
  declared procedure. B-B1/B-B2 *are* measurements of the presented outcome;
  no numerical gate can mask a retrieval event (§5's enforced separation), and
  the retrieval gates cannot mask a numerical one (full-population M1/M5).
  Cross-procedure and cross-size escapes are the two recorded dependencies
  (I-4, I-5), not silent ones.
* **Contradictory gates:** none operationally; two knife-edge budget
  incompatibilities (B-C1↔B-A2, B-C2↔B-A1) exist in the derived-constant
  narration — conservative direction, never a false PASS (§5).
* **Gates measuring the same property:** the seven gates are seven distinct
  statements (pairwise cosine movement; stored norm; presented top-1; presented
  order; reference norm; reference subnormal energy; representability). GLM
  §13's redundancy critique is genuinely resolved in Appendix B — nothing here
  duplicates R-A3⊆R-A1 or R-B2≡R-B4.
* **Different populations/scope:** A and B gates run over all four corpora;
  C gates over the same corpora's reference vectors; M13/M9-albums over
  15 aggregates each. Consistent and stated.
* **Can a single changed presented result escape?** No: a changed top-1 →
  B-B1 (+B-B2 at k=1 — note B-B2 k=1 makes B-B1 formally implied; harmless
  duplication, both are definitional); any change inside positions 2..20 →
  B-B2's corresponding k-prefix; anything deeper → not presented at any
  ADR-sketched limit, and still recorded in M11 for the §17.6 human review.
  Truncation-vs-full-ranking equivalence means no prefix change can hide.
* **Not-evaluated gap in the verdict grammar:** if all evaluated gates are
  green but some gate is recorded "not evaluated" (§11's last rule), the
  outcome is literally neither PASS (requires every gate green), FAIL
  (requires a violation), nor INCONCLUSIVE (corpus was evaluable). The intended
  conservative reading is "not a PASS"; the run record must state the rule it
  applied (MINOR).

Classification: **NO ISSUE** (logically appropriate), one MINOR.

---

## 9. Frozen-criteria audit

* **Every load-bearing threshold specified before the real run:** yes — §11's
  table fixes 2^-10 ×3, 2^-24, exact identity ×2, 0; none appears in any
  observed real-corpus form, and §12 gives each a derivation with the
  "could an independent party derive it" test applied (this reviewer
  re-derived 2^-11/2^-10/2^-9/2^-24 independently and agrees).
* **Report-only metrics cannot silently become gates:** the report-only list
  (M2–M4, M6, M7, M10–M14, support record) has no thresholds anywhere, and
  §11's "an unjustified threshold is how G-6 arrived at C2 and C6" forecloses
  post-hoc promotion; the verdict rule only enumerates B-gates.
* **No change-and-rerun without a recorded revision:** §14 rule 2 names the
  prohibited sequence explicitly and routes malformed-criteria repair through a
  G-6C document reviewed *before* any rerun. Enforced, not just declared:
  `g6b_tests::the_methodology_freeze_and_prohibition_are_recorded` fails if the
  criteria table or the same-data prohibition is edited out, and §17.5 requires
  a byte-comparison of the criteria text at run time.
* **Same-data prohibition:** correctly absolute — the surrogate's
  `fixtures/g6b/REPORT.txt` self-labels as instrument check; no verdict path
  cites it; §6.2 honestly discloses that the surrogate *did* contain a gated
  retrieval event (k=12/20 at 512-D) and still refuses it evidentiary status.
  This is the correct handling of the GLM §5.4 hazard.
* **Historical immutability:** `fixtures/g6/REPORT.txt`/`RUN.txt` byte-pinned by
  `g6_tests` (pass locally); `g6.rs` untouched; the §4.1 correction leaves the
  historical headline (1.835e-5) in place and labelled, and
  `the_symmetric…` test proves the new instrument *reproduces* the historical
  mixed number rather than overwriting it. The G-6 FAIL and G-6A repair remain
  in the audit trail (§14 rule 4).
* **G-6A's lesson applied, not recited:** the two ways G-6A "repaired around"
  observed results (the 2^-13 expectation; R-B5's undervived threshold) were
  *dropped*, not re-derived, and the dispositions are recorded in Appendix B.

Classification: **NO ISSUE.**

---

## 10. Counterexample analysis (the final hunt)

**Direction 1 — all seven gates green, but the actual f16 result presented to a
user materially differs from f32.**

Within the declared experiment (recorded corpus × declared procedure):
**impossible**, and the reason is structural — B-B1/B-B2 are comparisons of the
presented lists themselves; a material difference *is* a gate failure. There is
no floor, band, or ceiling any event can hide behind, and the mixed-regime
escape (one-operand quantization) is closed by `compare_storage_path`.

Three *adjacent* constructions were tested and do not defeat it, given the
methodology's own disclosures:

1. **Sub-f32-ULP mask:** a pair with f64-score difference < ~6e-8 whose f64
   order opposes between the worlds while both worlds' f32 scores tie —
   invisible to B-B1/B-B2 **and** to M12, whose counting code skips pairs whose
   reference scores are already equal, so this class is recorded nowhere. Real,
   but (a) the pair is indistinguishable *in the presented
   f32 score* under the declared procedure, and (b) it becomes a genuine
   escape only if production ranks below the reported score's precision — a D-4
   property. → I-4 mitigation.
2. **Scale extrapolation:** a 5,000-track library's boundary populations grow
   ~100×, so flips that cannot occur among 44 candidates may occur at real
   serving sizes. The recorded corpus is the declared reference corpus; the
   PASS establishes "no measured change", never "no change at any size".
   → I-5 mitigation.
3. **Future product semantics:** τ crossings (D-1, instrumented as M6) and
   interchangeability bands (D-2) are *definitions the product has not made*;
   the methodology refuses to grant them — correctly — but that also means the
   PASS's meaning is pinned to the current (strict) semantics.

**Direction 2 — a gate fails but f16 has not changed meaningful retrieval
behaviour.**

1. **B-B2-album on a deferred surface:** v1.0 defines no album aggregate at all
   (decision B), so an album-only sequence failure (e.g. of the mean→L2
   candidate rule) fails the verdict while *no current user-visible v1.0
   behaviour changes*. Possible in principle; correctly conservative (FAIL
   keeps f32), and must be attributed as in I-3.
2. **B-C2 knife-edge vs B-A1** (§5, I-2) and **B-C1 knife-edge vs B-A2**:
   theoretically constructible false numerical failures; practically unreachable
   by real embeddings by orders of magnitude. Both fail toward "keep f32", the
   safe direction.
3. (Rejected: R-B5-style vacuity has been removed from the gate set; no
   always-green gate remains.)

Classification: **NO ISSUE** — neither direction defeats the design; the three
recorded mitigations (I-3/I-4/I-5) close the interpretive space.

---

## 11. Scope-of-conclusion audit

The methodology makes the right distinction at every level checked:

* **Per-profile, never universal:** §1 ("two questions, two evidence bases"),
  §9's closing paragraphs ("These are **profile** requirements…", "No universal
  claim is made from the 1280-D Discogs-EffNet corpus: the eventual rule is
  per-profile (D-6)"), §13's known gaps ("the conclusion is per-profile
  admissibility… never a universal claim"), and D-6's recommended wording —
  "admissible *for a profile* that passes category C and whose reference corpus
  passes categories A and B" — are all consistent. No sentence generalizes to
  "f16 is safe".
* **Per-model-family / per-dimension:** out of evidence scope, explicitly
  excluded (§15 "One model family"; concentration/sparse profiles flagged for
  their own B-C numbers).
* **Perceptual claims:** excluded (G-7; enforced by
  `the_experiment_claims_nothing_about_perceptual_quality`).
* **Discogs-EffNet → arbitrary future models:** *not* accidentally generalized.
  One caution the record should carry: the two *dimensions* tested (1280/512)
  are both deep in the regime where the normal-range margin (≈8.8/7.3 binades
  above the knee) makes category C near-free; the methodology's per-profile rule
  correctly forces a *new* profile to re-run A/B/C rather than inherit this
  PASS. Nothing in the text lets a future profile claim admissibility without
  its own gates. §13 additionally blocks the conclusion from becoming a feature
  claim while G-1…G-5, G-7 and ADR 0016 Slice 0 remain open — the PASS is
  admissibility *evidence* for human sign-off (§17), which is exactly the
  G-6→§14 posture ADR 0017's gate table expects.
* The G-6A failure mode (criteria reshaped around a result) cannot recur via
  scope drift because the scope statement itself is frozen (§14 rule 1) and
  §17.5 byte-checks it at run time.

Classification: **NO ISSUE.**

---

## 12. Four-configuration audit (multi/release × hop 61/62)

* **Separate evidence:** §13 requires four corpora, each evaluated on its own
  gate table; the verdict rule is "every gate green on **every** real corpus" —
  a pass for one cannot hide a failure in another; §17.2 requires a per-corpus
  artefact carrying `profile_fingerprint`, variant, hop and corpus digest.
* **Mixing risk:** the genuine risk is hop61-vs-hop62 within one variant
  (identical dimension and model file, different vectors). The designed
  defenses are sufficient: per-file `metadata` (model_name, patch_hop,
  model_sha256, corpus_identity, corpus_root_sha256 — all present in the
  `report.rs` schema), mandatory `embedding_sha256` re-verification *before*
  measurement (which catches mislabeling because 0/45 hop61 embeddings equal
  their hop62 counterparts), and separate fingerprints by construction
  (tag 6/11 partition identity). The multi-vs-release mix is self-catching
  (dimension mismatch → validation failure).
* **Fingerprint/model identity:** the experiment never needs a fingerprint —
  and §17.2 records it as *carried provenance*, not as something the
  measurement derives (consistent with RUN.txt: the spike produces vectors, not
  profiles). No path mixes variants inside a population: every loop in
  `compare_storage_path` is single-corpus.
* One practical note: the four corpora share the *same library and model
  weights* (only hop differs for the paired corpora), so the four results are
  not four fully independent trials of "some embedding profile" — the
  methodology's per-profile scoping (§11 above) already states this honestly.

Classification: **NO ISSUE.**

---

## 13. Required action (all pre-execution; none is a methodology edit)

**I-1 — Implement the missing gated quantities in the real-corpus runner, with
tests, before the run.** Not present anywhere today: B-C1/B-C2 measurement over
reference vectors (per-vector norm conformance; per-vector subnormal energy
fraction; support/participation record), M16 as a *counting* overflow check
(replace the `to_f16_candidate` panic on the real-corpus path, per §9/§13's own
requirement), album-level ordered k=12 sequence identity (the B-B2 album
component — `compare_storage_path` measures track retrieval and M14 drift
only), and the gate-evaluation/verdict machinery itself (nothing in `src/`
computes the seven gates). §17.5's criteria-text byte-comparison and §13's
digest re-verification are likewise specified but not yet implemented. A run
that cannot evaluate a gate must record it "not evaluated" (§11's own rule), so
the failure mode is contained — but the run must not proceed as if the
committed surrogate instrument covered these measurements.

**I-2 — Record the corrected B-C2→B-A1 composition** (half-step 2^-25 ×
MAX_DIMENSIONS 4096 ⟹ subnormal contribution ≤ 2^-38 to squared vector error;
ceiling 2^-10·(1+5×10^-4)) **in the run record**, and treat §9's "half the
budget" sentence and §5.1's "dimension-free" label as documentation corrections
for any future revision. No gate constant changes; the finding is that the
derivation *as narrated* does not close the theorem it claims, while a
slightly different argument does.

**I-3 — Pin the album semantics of the run:** aggregates must be recomputed per
world from each world's stored member vectors (never read from
`embeddings.json albums[]`); and the verdict record must attribute any
album-only failure to the deferred (non-normative, decision B) aggregation path
while explicitly declining to claim v1.0 album behaviour either way.

**I-4 — Carry the tie-break disclosure into §17.6's human review:** the review
checklist must enumerate the M12 new-tie events (and any reference-exact ties
that survive both worlds) and state that under a future D-4 content-key or
higher-precision server ordering, precisely those bands are the places where
the presented list could differ from what G-6B measured. One sentence in the
run record; no methodology change.

**I-5 — Phrase the conclusion with its size bound:** the admissibility statement
must say "no measured retrieval change in the recorded 45-track reference
corpus; boundary margins/populations (M13) are the scaling evidence — flips at
k-boundaries become more likely as candidate pools grow", and the record should
state the weakest measured boundary margin per corpus/k so the scaling
extrapolation is done from numbers, not vibes.

**MINOR items** (record, don't act): the verdict-grammar not-evaluated case
(§8); census-loop robustness if a stored score goes non-finite (possible only
on vectors that B-C1/C3 would already fail — evaluate B-C before B, or treat
the panic as instrument failure → INCONCLUSIVE); M13 band-radius framing;
extend the leak/perceptual test hygiene to the real-run artefacts (which will
contain real metadata paths and must stay outside Git, as the spike's outputs
already do); separate the two B-C3 rejection kinds (non-finite input vs
magnitude overflow) in the count record; the 44-candidate empty-band rule is
already stated (§11/§15) — just enforce it in the verdict table.

---

## 14. May the real-corpus run proceed?

**Yes.** With I-1 implemented (runner + gate evaluation + album re-derivation +
overflow counting, each pinned by tests) and I-2…I-5 recorded in the run's
documentation, the frozen methodology is capable of answering its question:
whether the deterministic f32→binary16→f32 storage path, applied to both query
and candidate vectors of the four recorded Discogs-EffNet corpora under
`eval::cosine`/`eval::nearest`, changes the presented retrieval result — and it
answers it directly, without a numerical bound ever being allowed to excuse a
retrieval event.

No blocker was found. No frozen criterion should move. On acceptance of this
review, the owner may record:

> `G-6B: READY FOR REAL-CORPUS RUN`

with:

> **`G-6: FAIL — KEEP F32` — unchanged.**

Nothing in this review constitutes that status change, a criteria revision,
or criteria evidence; it is a methodology review (§14's rules apply to its
author exactly as to anyone else), and this document itself was written before
any real-corpus measurement exists.

---

*Reviewer: Qwen (independent third adversarial reviewer). Method: full read of
the twelve named documents and artefacts, line-level read of `g6b.rs`, `g6.rs`,
`eval.rs`, the `docfmt` binary16 encode/decode/write/read paths, both runners,
`g6b_tests.rs`, `lib.rs`, ADR 0017 §5.4–§5.8 and §10.4, FORMAT_SPEC §7; local
re-execution of the instrument and conformance test suites; independent
re-derivation of 2^-11, the vector/norm/cosine/pair bounds, the B-C2 budget
composition, and the subnormal half-step argument. Prior reviews were read and
their dispositions checked against Appendix A/B rather than trusted.*
