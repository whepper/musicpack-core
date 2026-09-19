# Phase 15H performance baseline (frozen)

> See `PERF_INVESTIGATION_15H2.md` for the follow-up candidate
> investigation. The measurements below are unchanged.

Measurement and investigation only. No codec behaviour was changed to produce
these numbers: no algorithm, table, constant, ordering, lifetime, or
bitstream changes; no SIMD, unsafe, FFI, or optimisation.

## Methodology

- Harness: `tests/encoder_bench.rs` (ignored tests, release only):
  `cargo test --release -p musicpack-musepack-encoder --test encoder_bench -- --ignored --nocapture`.
- Deterministic PCM (LCG noise, integer ramp/alternating, f32 sine/multitone,
  transient-rich); stereo i16 interleaved, matching the whole-encoder corpus
  generator family.
- Production path only: `MusepackEncoder::encode` (never `encode_traced`).
- Fresh encoder per iteration; PCM built once up front (setup excluded).
- Warm-up 2 iterations (discarded), 5 measured iterations; min and median
  reported. Output sunk via `black_box` plus a length assert.
- Stage timings: a manual loop over the same public component APIs in the same
  order, with per-stage `Instant` accumulation. It asserts byte-identity with
  `encode()` on the measured input, so the breakdown genuinely describes the
  integrated encoder. Timer overhead is included in the totals, not subtracted.
- Function profile: Apple `sample` (1 ms, 15 s) on a release loop encoding
  60 s of q5 noise ×100; binary rebuilt with `CARGO_PROFILE_RELEASE_DEBUG=true`
  (debug info only, same codegen) for symbolication.

## Environment

```text
CPU:          Apple M5 (arm64, Apple Silicon)
OS:           macOS 27.0
Rust:         1.97.1
Build:        cargo release profile, defaults (opt-level 3, no RUSTFLAGS,
              no target-cpu flags)
Input:        deterministic stereo i16 (see matrix)
```

## Complete encoder throughput (release, median of 5)

```text
case              qual   audio      bytes     median  realtime
silence-1s        q5      1.0s         63     165µs     6045x
noise-1s          q5      1.0s      26588       4.9ms    204x
sine-1s           q5      1.0s       6595       3.9ms    256x
multitone-1s      q5      1.0s      12140       4.0ms    249x
transient-1s      q5      1.0s      25611       4.8ms    207x
stereo-1s         q5      1.0s      13680       4.0ms    251x
noise-10s         q5     10.0s     234460      47.3ms    211x
noise-10s         q4     10.0s     154530      45.1ms    222x
noise-10s         q6     10.0s     289871      48.3ms    207x
noise-10s         q7     10.0s     327138      48.6ms    206x
noise-10s-48k     q5     10.0s     237669      51.1ms    196x
noise-60s         q5     60.0s    1387551     295.8ms    203x
```

Steady state is ~200–250× realtime for real signals; silence is ~6000×
(silence-skipped frames). Quality scaling is mild (q4→q7 ≈ +8% time).

## Stage breakdown (10 s noise, q5/44.1 kHz, total 43.3 ms)

```text
Stage                         Time       %
Psychoacoustic model        29.6ms   68.4%
Analysis filterbank          6.8ms   15.6%
Huffman/AP coding            3.3ms    7.6%
Allocation/PNS               1.6ms    3.7%
SCF extraction               0.7ms    1.6%
NS analysis                  0.6ms    1.4%
Quantisation                 0.3ms    0.7%
PCM/frame preparation        0.3ms    0.6%
MS_LR_Entscheidung           0.1ms    0.3%
RaiseSMR                     0.0ms    0.1%
Transient analysis           0.0ms    0.0%
SV8/container                0.0ms    0.0%
```

This independently reproduces the historical C observation (~3/4 psy,
remainder filterbank + coding) for the Rust implementation.

## Function-level hotspots (`sample`, n = 12484, collapsed weights)

Weights include inlined callees (e.g. `SpreadingSignal`, `my_cos`/`my_atan2`
fold into their callers; `bitrv2`/butterflies fold into `rdft`).

```text
rdft                       3716  29.8%  FFT butterflies (psy spectra + cepstrum)
polar_spec_1024            1889  15.1%  window + power/phase incl. atan2 table
filterbank expr            1010   8.1%  8-tap strided prototype FIR
analyse_frame (self)        797   6.4%  spreading + inlined psy misc
encode_samples              784   6.3%  Huffman residual switch + bit writes
filterbank matrixing        726   5.8%  32-wide modulation dot products
cvd2048                     578   4.6%  log-window + cepstral search
allocate_channel            456   3.7%  candidate Res search (ISNR estimators)
memmove (libc)              328   2.6%  analysis-buffer advances
calc_unpred                 281   2.3%  cos-table + sqrt per spectral line
calc_short_threshold        207   1.7%  4×256-pt spectra + threshold loop
pow (libm)                  180   1.4%  tonality/temporal offsets, pow10
cepstrum2048                154   1.2%  mirror + scale loops (FFT separate)
find_optimal_ans            143   1.1%  Durbin + 16-pt evaluations
read_block                  129   1.0%  i16→f32 convert + M/S interleave
vectoring                    98   0.8%
memset                       89   0.7%
combine_scf                  88   0.7%
partition_energy             87   0.7%
subband_energy               81   0.6%
write_scf                    76   0.6%
```

Cross-check: psy-attributable ≈ 63–65% (rdft + polar + analyse_frame +
cvd + unpred + short-threshold + cepstrum + pow + energy fns), filterbank ≈
15%, Huffman/AP ≈ 8%, allocation ≈ 4% — coherent with the stage timers.

## Compiler-output and memory observations (§8–§9)

- LLVM already emits NEON in: `rdft`, `allocate_channel`, `quantize`,
  `subband_energy`, `cvd2048`, `find_optimal_ans`, `weighted_partition`,
  `scf_extraktion`, `calc_unpred`, `pow_spec_2048`, `calc_short_threshold`,
  `vectoring`, `polar_spec_1024` (partially).
- Still scalar: `filterbank expr` (stride-64 gather pattern), `matrixing`
  (runtime `max_band` bound blocks bound-check elimination/vectorisation),
  `encode_samples` (data-dependent switch), `partition_energy`,
  `calculate_smr`, `encode_enum`, `read_block`, `calc_ms_threshold`,
  `cepstrum2048` mirror/scale loops.
- Cold `panic_bounds_check` sites remain in most hot functions (1–5 each);
  `matrixing`, `calculate_smr`, `calc_ms_threshold`, `write_scf` have none.
- `fdiv` instructions concentrate in `find_optimal_ans` (Durbin, 17 sites),
  `calc_ms_threshold` (ratios, 9), `cvd2048` (5), `allocate_channel` (4),
  `polar_spec_1024` (atan2 divisions, 3).
- libm `pow` (~1.4%) serves tonality/temporal offsets; `log10` (~0.2%) serves
  SCF index estimation. Neither dominates.
- Working sets are tiny (largest tables: `SPRD` 13 KB, FFT twiddles 16 KB;
  state arrays a few KB) — everything fits in L1/L2. Cost is arithmetic
  (butterflies, divisions, atan2/pow), not bandwidth. No allocator hotspot
  (`libsystem_malloc` absent from the profile); per-frame copies are limited
  to `memmove` buffer advances (2.6%) and the `read_block` convert loop (1%).
- Rust-specific notes: bounds checks survive only as cold paths; the
  `encode()` path builds no trace state (15G.2); `black_box` guards the
  `pow10` lowering.

## SIMD feasibility (§10; no implementation)

- **A — strong candidates** (regular, independent, contiguous):
  - `filterbank matrixing` (~6%): 32-wide dots; needs exact-order-preserving
    vectorisation (association/FMA change bits). Medium complexity.
  - `filterbank expr` (~8%): 8-tap strided dots; gather pattern, moderate
    NEON gain. Same numerical caveat.
  - `polar_spec_1024` window/power loops (part of 15%): contiguous; keep
    `atan2` scalar. Low-medium complexity, exactness-preserving if the two
    accumulations keep order.
  - Spreading/ApplyLtq/AdaptThresholds/CalculateSMR block (~7% incl. inlined
    share): elementwise + tiny reductions over 57 partitions; branches only
    in tonality offset (predictable). Low complexity.
- **B — possible** (dependencies/branches, less obvious):
  - `rdft` butterflies (~30%): standard vectorisable FFT, but in-place
    bit-reversal + stage dependencies + twiddle exactness make it the highest
    complexity item. The C project itself maintains separate scalar/SIMD
    variants — a warning about cost/benefit.
  - `calc_unpred` (~2%): independent per line; table + sqrt + divide.
  - `allocate_channel` (~4%): branchy candidate search; already partly
    vectorised by LLVM.
  - `encode_samples` (~6%): mostly data-dependent Huffman walks; only the
    Res≥9 raw-bit path is regular.
  - `find_optimal_ans` (~1%): sequential Durbin + vectorisable 16-pt loop.
- **C — poor** (stateful/branchy/table-driven/delicate, tiny):
  `calc_ms_threshold`, `encode_enum`/`encode_log`/`write_scf`,
  CVD max-search chains, `combine_scf`, transient logic, `RaiseSMR`,
  container writers.

## Numerical compatibility constraints (§11)

Any future change must preserve exact SV8 bytes. Concretely: f32 summation
association and FMA formation must not change (matrixing, spreading,
reductions); f32/f64 boundaries stay (`sqrt`/`pow`/`log10` in f64, `lrintf`
bit tricks, `pow10` via generic `pow`); FFT twiddle/mirror exactness;
threshold `>=`/tie comparisons. "Sounds identical" is not acceptance.

## Optimisation candidates and trade-offs (§12)

| Candidate | Stage | % | SIMD | Complexity | Numerical risk | Recommendation |
|---|---|---|---|---|---|---|
| matrixing+expr | filterbank | ~14 | A | medium | order/FMA — must preserve | investigate first; biggest regular block |
| polar window/power | psy | ~5–8 of 15 | A | low-med | keep 2-term order | investigate; split off atan2 |
| spreading/threshold block | psy | ~7 | A | low | branches predictable | investigate after above |
| rdft | psy | ~30 | B | high | twiddle/order; dual-impl cost | defer; biggest prize but riskiest |
| encode_samples | coding | ~6 | B/C | medium | codebook walk stays scalar | profile raw-bit vs table paths first |
| allocate_channel | coding | ~4 | B | medium | estimator branches | low priority; LLVM already helps |
| calc_unpred/CVD | psy | ~7 tot | B/C | medium | search ties | low priority |
| memmove/read_block | overhead | ~4 | A (memcpy) | low | none (exact copies) | trivial; only if amplitudes matter |
| everything <1% | — | — | C | — | — | remain scalar; not justified |

## Files changed (this phase)

- `tests/encoder_bench.rs`: reproducible release benchmark (throughput
  matrix + stage breakdown with byte-identity self-check). Follows the
  existing ignored-test pattern; no new dependencies.
- `PERF_BASELINE_15H.md` (this file).
- Temporary profiling driver removed after use; no production code touched.
