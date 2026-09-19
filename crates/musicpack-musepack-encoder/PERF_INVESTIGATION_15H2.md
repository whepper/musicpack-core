# Phase 15H.2 — Optimisation candidate investigation

Investigation only. No production code was changed: every experiment ran
bench-side or was fully reverted, and all gates below were re-verified on the
final tree. The 15H.0–15H.1 baseline (`PERF_BASELINE_15H.md`) is unchanged;
see it for methodology, environment, and throughput numbers.

## Baseline re-verified (this phase)

- `cargo test --workspace`: 568 passed, 0 failed (21/21 whole-encoder,
  140,420 bytes exact; 282-vector manifest `2f628c79…` unchanged).
- Scalar-forced C vs Rust streams remain byte-identical; no fixture was
  regenerated or modified.
- Micro-benchmarks added during investigation live as `#[ignore]`d tests next
  to the code they measure (`filterbank::tests::bench_{matrixing,vectoring}_ab`,
  `psy::fft::tests::bench_micro_polar_split`); the temporary `pub` markers
  used to prototype them were fully reverted — `src/` has no API changes.

## Candidate A — `matrixing` + `fb_expr` ( Ref. ~14% combined)

**Why LLVM doesn't vectorise.** `matrixing`'s 32-term dot is a single serial
FP chain: any lane split reassociates and changes rounding, so without
fast-math LLVM correctly keeps it scalar (0 vector ops, 0 calls, no bounds
checks in the emitted code). `expr`'s 8 taps load with stride 64 (gather
pattern, 8 disjoint cache lines per call); NEON has no gather, so it also
stays scalar. Both observations from 15H.0 verified in disassembly.

**Experiment A1 (Type 1): hoist the per-band channel `match` out of the
`matrixing` loop.** Arithmetic untouched. Interleaved A/B, bit-identity
asserted: **0.95–0.99× (slower)** across three runs. Abandoned; the branch is
perfectly predicted and the closure form schedules worse. Negative result kept
as `bench_matrixing_ab` to prevent re-investigation.

**Experiment A2 (Type 1): interleave four outputs' accumulators in
`vectoring`.** Each output still accumulates taps k=0..7 left-to-right, then
expr_a + expr_b — identical per-output operation sequences, so results are
bit-identical by construction (not by luck). One subtle trap was caught
during development: `y[16]` must keep the two 8-chains separate (`e1 + e2`);
a single 16-term chain reassociates across the expr boundary. Interleaved A/B
with a 200-trial bit-identity gate (magnitudes 1, 1e±20, zeros, DC):
**1.85–1.90× per call**, gate green on every trial.
Complete-encoder trial (temporarily applied, then reverted): noise-10s q5
47.3 ms → 45.3 ms (**~1.04× total**, ≈ the predicted 8.1% × (1 − 1/1.85));
all gates green during the trial (filterbank/coding/psy fixtures, 21/21,
traced equality, fmt, clippy incl. wasm).
**Recommendation: strongest evidence-backed candidate for the next phase.**
Risk: purely structural (no reduction reordering); the 200-trial gate plus
full 21/21 + component oracles is the acceptance bar for re-application.

## Candidate B — `polar_spec_1024` (~15% incl. FFT)

Split measured in `bench_micro_polar_split` (plus a raw `rdft_1024` probe):
per call, rdft-1024 ≈ 2.3–2.6 µs, windowing+power ≈ 0.1 µs, 512× `my_atan2` +
stores ≈ 1.0–1.4 µs. So the vectorisation-eligible part (window/power) is
~3% of the function; `atan2` (~30% of polar_spec, ~3–4% of total) must stay
scalar per the compatibility contract (table lookup + division + branches;
no compatible vector form exists). A power/phase loop split would save at
most ~0.2% total — not worth the churn. **Remain scalar** (except via rdft work
under Candidate E, deferred).

## Candidate C — spreading / threshold block (~7%)

Read, not rewritten. `spreading_signal`'s inner loop (13-wide, varying edges)
is order-preservingly vectorisable across `n`, but totals only ~6k MACs/frame
(~0.5% of encoder time). Everything else in the block is branchy and/or calls
libm `pow` per element (`apply_tonality_offset` 3-way branch,
`calc_temporal_threshold` nested branches + `pow`, `calc_ms_threshold`
mode logic, `adapt_ltq` 57-term order-sensitive dot). **Remain scalar**; be
particularly conservative here — this feeds SMR/ANSspec/allocation.

## Candidate D — `encode_samples` (6.3%)

Measured Res distribution (1 s samples, q5/q7): Res≥9 paths are only 2–4% of
coded band-channels (Res≤0, i.e. free, is 11–91%; Res 2 dominates noise at
~60%). Cost drivers are scalar by nature: per-coefficient table lookups,
`BitWriter::write_bits` (bit-at-a-time loop), and the loop-carried `idx`
recurrence in Res 2/5–8 paths. The Res≥9 raw-bit path is regular but too rare
to matter. **Remain scalar.** (Possible future Type-1, out of scope here: a
multi-bit `write_bits` fast path preserves bytes but touches the shared
writer; not recommended now.)

## Candidate E — `rdft` (29.8%, inspect only)

Structure: bit-reversal scatters (unvectorisable), radix-4 butterflies over
strided data (LLVM already emits 330 NEON ops here), strided `rftfsub`
post-pass, per-call twiddle loads. A faster FFT (precomputed per-size
twiddles, Stockham autosort, explicit NEON kernels) is the classic
real prize but changes evaluation order and is a large rewrite — the C
project itself carries separate scalar/SIMD variants as a warning. Per the
phase brief, inspected only. **Deferred.**

## Numerical findings (all preserved)

- f32 reductions must stay associated; FMA formation is off on both sides
  (Rust default, C `-ffp-contract=off`) — enabling it would change bits.
- f32/f64 boundaries (`sqrt`/`pow`/`log10` in f64, `lrintf` bit tricks,
  generic-`pow` `pow10`, `>=` tie comparisons) untouched.
- Cold `panic_bounds_check` sites remain in hot functions (1–5 each);
  `matrixing`/`calculate_smr`/`calc_ms_threshold`/`write_scf` have none.
- libm `pow` ≈ 1.4%, `log10` ≈ 0.2% — neither dominates; no action.

## Candidate table (§12)

| Candidate | Stage | % | SIMD | Complexity | Numerical risk | Recommendation |
|---|---|---|---|---|---|---|
| vectoring joint-4 (A2) | filterbank | 8.1 | n/a (Type 1) | low-med | none by construction + gates | **implement next** |
| matrixing hoist (A1) | filterbank | 5.8 | n/a | low | none, but slower | abandoned |
| polar power/phase split (B) | psy | ~0.2 saving | A (partial) | low | none | not worthwhile |
| my_atan2 vectorise (B) | psy | ~3 | none compatible | high | table/branch/division exactness | remain scalar |
| spreading inner (C) | psy | ~0.5 | A | low-med | none if outer kept serial | not worthwhile |
| threshold branches/pow (C) | psy | ~6 | C | high | SMR path — conservative | remain scalar |
| encode_samples paths (D) | coding | 6.3 | C (B pockets ≤4%) | medium | bit-exact writer | remain scalar |
| rdft rewrite (E) | psy | 29.8 | B | high | order/twiddle; dual-impl cost | deferred |
| write_bits bulk path | coding | ~2–4 est | n/a (Type 1) | medium | shared-writer risk | future option, not now |
| memmove/read_block | overhead | ~3.6 | memcpy-level | low | none (exact copies) | only if amplitudes matter |

## Files changed (this phase)

- `src/filterbank/mod.rs`: only `#[cfg(test)]` A/B tests + variants added;
  production code byte-identical to baseline (A2 trial fully reverted).
- `src/psy/fft.rs`: only the `bench_micro_polar_split` test added.
- `tests/encoder_bench.rs`: removed the temporary cross-crate micro tests
  (superseded by the in-module versions above).
- `PERF_INVESTIGATION_15H2.md` (this file).

## Gate results (final tree)

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- wasm32 clippy + check: pass.
- `cargo test --workspace`: 568 passed, 0 failed (ignored count grew only by
  the 3 new ignored micro-benchmarks).
- 21/21 whole-encoder, 140,420 bytes exact; manifest unchanged; fixtures
  unchanged; C reference untouched.
- No unsafe, FFI, SIMD, optimisation, or dependency changes.

## 15H.3 production implementation (A2)

The investigated `vectoring_joint4` was promoted verbatim into production
`vectoring` (`src/filterbank/mod.rs`); the only additions are a doc comment
stating the order invariant and the oracle reference. Terminology note: the
phase brief's "32-term dot product" language describes the general
joint-accumulator pattern loosely — the implemented and investigated A2 is
the `vectoring` joint-4 interleaving (per-output 8+8 tap chains), exactly as
15H.2 measured it. `matrixing`'s single 32-term chain is intentionally
untouched: lane-splitting it would reassociate. No other hotspot was
modified.

Order preservation (why bits are identical): each of the four interleaved
chains accumulates its own output's taps `k = 0..7` left-to-right, then adds
`expr_a + expr_b` — the same operation sequence, in the same order, as the
scalar reference. `y[16]` keeps its two 8-chains separate. No FMA is formed
(verified: only `fmul`/`fadd` in the emitted code, matching the scalar
 build, on both sides which default to no FP contraction).

Semantic oracle: `vectoring_scalar` (tests) retains the original formulation;
the permanent `vectoring_matches_scalar_reference` test gates production
against it over 200 diverse trials (magnitudes 1/1e±20, zeros, DC) with
`f32::to_bits` equality.

Measured production result (same 15H methodology):

| Measurement | 15H baseline | 15H.2 A2 trial | 15H.3 production | Change |
| ----------- | -----------: | -------------: | ---------------: | -----: |
| vectoring micro | ~164 ns/call | ~88 ns/call | ~178 ns/call vs scalar 393 ns/call (same session) | ~1.85–2.2× |
| encoder 10s q5/44.1 | 47.3 ms | 45.3 ms | 43.5–44.6 ms | ~1.06–1.09× total |
| encoder 60s q5/44.1 | 295.8 ms | — | 261–266 ms (3 runs) | ~1.11× total |

Absolute numbers vary run to run (machine state); ratios from interleaved
same-run A/B (1.85–2.2× micro) and the stable production totals are the
signal. Direction and magnitude are consistent across all sessions: the
~8% hotspot at ~1.9× yields the observed ~4–8% end-to-end win.

Generated code (release `vectoring`): dense `fmul`/`fadd` sequences for the
four independent chains (ILP visible), no `fmadd`/`fmla`, no calls except
cold `panic_bounds_check` sites, no explicit SIMD.

Compatibility (production tree):

```text
21/21 encoder fixtures: PASS (140,420 bytes exact)
Frozen manifest: unchanged (2f628c79…)
Component fixtures: unchanged
Legacy C reference: untouched
```
