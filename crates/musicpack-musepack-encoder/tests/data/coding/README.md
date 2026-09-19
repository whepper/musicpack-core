# Frozen coding-layer oracle (Phase 15E)

Bit-exact reference data from the C encoder's coding pipeline, used to gate the
Rust port of the stages **downstream of the psychoacoustic model**. Tests that
consume these files never invoke the C encoder.

## Boundary

The reference frame order is:

```
Analyse_Filter
  → Psychoakustisches_Modell   (frozen; NOT ported by this crate)
  → RaiseSMR / MS_LR_Entscheidung / TransientenCalc
  → SCF_Extraktion
  → NS_Analyse
  → Allocate (+ PNS)
  → Quantisierung
  → writeBitstream_SV8  (Huffman + AP payload)
```

The oracle captures the **inputs at the coding boundary** (post-MS subband
samples plus the psychoacoustic decisions) and every stage's output, so each
Rust stage can be tested independently against recorded values.

## Files

* `coding-q{4,5,6,7}.bin` — one 36864-byte little-endian record per frame.
* `ap-q{4,5,6,7}.bin` — AP block records: `u32 marker 0x41505A00`, `u32 byte
  length`, then the block bytes (`AP` key + size + payload). The tool uses
  `frames_per_block_pwr = 2` (4 frames per block: one key frame + three delta
  frames).
* `scf_tables.txt` — 134 `__SCF` then 134 `__invSCF` bit patterns (`%08x`).
* `penalty.txt` — the 256 `Penalty` bytes used by `SCF_Extraktion`.
* `ans_tables.txt` — `InvFourier[7][16]`, `Cos_Tab[16][7]`, `Sin_Tab[16][7]`
  bit patterns then the 32 `maxANSOrder` values, used by `NS_Analyse`.

## Per-frame record layout (little-endian)

| Offset | Section | Type / count |
|---|---|---|
| 0 | SMR L, R, M, S | f32 × 128 |
| 512 | ANSspec L, R, M, S (exact bits) | i32 × 2048 |
| 8704 | Transient[32] | i32 × 32 |
| 8832 | MS_Flag[32] | i32 × 32 |
| 8960 | X post-MS (pre-SCF) | f32 × 2304 |
| 18176 | Power_L[32][3], Power_R[32][3] | f32 × 192 |
| 18944 | SCF_Index_L[32][3], SCF_Index_R[32][3] (after SCF) | i32 × 192 |
| 19712 | SNR_comp_L[32], SNR_comp_R[32] (after SCF) | f32 × 64 |
| 19968 | NS_Order_L[32], NS_Order_R[32] | i32 × 64 |
| 20224 | FIR_L[32][6], FIR_R[32][6] | f32 × 384 |
| 21760 | SNR_comp_L[32], SNR_comp_R[32] (after NS) | f32 × 64 |
| 22016 | Res_L[32], Res_R[32] | i32 × 64 |
| 22272 | SCF_Index_L/R[32][3] (after Allocate) | i32 × 192 |
| 23040 | X after Allocate | f32 × 2304 |
| 32256 | Q[32].L[36], Q[32].R[36] | i16 × 2304 |

X is stored band-major: for each band 0..31, 36 L then 36 R.
Only subbands `0..=Max_Band` are meaningful (q5: 28, q6/q7: 31); the rest are
zero because the reference leaves them uninitialised.

## Cases

All three are `tests/fixtures/sine44-q5.wav` (44.1 kHz stereo, 1 s) encoded at
q5/q6/q7, four frames each, by the pinned reference build (`05d97a5`, Apple
clang 21, arm64, `-O0`, `-ffp-contract=off`, `FAST_MATH` + `CVD_FASTLOG`).

| Case | Quality | Max_Band | Frames | coding bytes | AP bytes |
|---|---|---|---|---|---|
| q4 | 4 | 22 | 4 | 147456 | 3589 |
| q5 | 5 | 28 | 4 | 147456 | 4629 |
| q6 | 6 | 31 | 4 | 147456 | 5043 |
| q7 | 7 | 31 | 4 | 147456 | 5346 |

`q4` (Radio) is the only case with `PNS > 0`, so it is the one that exercises
the `PNS_SCF` allocation path (`Res == -1`); q5/q6/q7 have `PNS == 0`.

## Allocation/PNS boundary

`Allocate` consumes `SMR`, `Power`, the post-SCF `SCF_Index`, the post-NS
`SNR_comp` and `Transient`, and produces `Res`, the post-allocation
`SCF_Index` and the post-allocation subband `X` (scaled by `SCFfac` during
fine-adaptation). `tests/coding_allocate.rs` reconstructs the post-SCF `X` with
the crate's already-proven SCF stage (the fixtures are frozen and must not be
regenerated) and compares all three outputs exactly.

## Quantisation boundary

`Quantisierung` consumes `Res`, the post-allocation `X`, `NS_Order` and `FIR`
and produces `Q` (`mpc_quantizer`: `i16 L[36]`/`R[36]` per band — quantiser bin
indices, not PCM). Plain bands hold `0..=2·D[res]`; noise-shaped bands hold the
same after a signed `[-D[res], +D[res]]` clamp plus `+D[res]` offset.
`tests/coding_quant.rs` reads the inputs straight from the fixture and compares
`Q` exactly.

`Q` is **not reset per frame** by the reference, so bands with `Res <= 0`
(including PNS `Res == -1`) retain their previous values; the Rust `Quantizer`
models this explicitly and keeps state across calls. q4 exercises PNS (31
`Res == -1` band/channel results over 4 frames) and matches exactly.

## Huffman / AP payload boundary

`writeBitstream_SV8` consumes `Res`, post-allocation `SCF_Index`, `MS_Flag` and
`Q` plus frame-to-frame state (`framesInBlock`, `MaxBand`, `DSCF_Flag_L/R`,
`SCF_Last_L/R`) and emits entropy-coded frames. `tests/coding_ap.rs` reconstructs
that state from `coding-q{4,5,6,7}.bin` and compares the flushed `AP` block
bytes with `ap-q{4,5,6,7}.bin`.

* **Tables** (frozen, literal): the `huffsv7.c` codebooks (`HuffBands`,
  `HuffRes`, `HuffSCFI_1/2`, `HuffDSCF_1/2`, `HuffQ1..Q8`, `HuffQ9up`),
  `HuffQ2_var` (`encode_sv7.c`), the `Cnk`/`Cnk_len`/`Cnk_lost` combinatorial
  tables (`common/cnk_tables.h`) and `mpc_log2`/`mpc_log2_lost`
  (`libmpcenc/bitstream.c`).
* **Primitives:** `encodeLog` (truncated binary) and `encodeEnum`
  (combinatorial rank) are implemented as `huffman::{encode_log, encode_enum}`.
* **Bit ordering:** MSB-first through the Phase 15C `BitWriter`; frames are
  concatenated with no inter-frame byte alignment and the block buffer is
  zero-padded to a byte boundary only at `AP` flush, matching `writeBlock`.
* **PNS:** `Res == -1` and `Res == 0` bands contribute no sample bits (the
  resolution is still coded); q4 exercises this.
* **`AP` fixture structure:** each `ap-qN.bin` record is
  `u32 marker 0x41505A00`, `u32 byte length`, then the complete `AP` block
  (`AP` key + self-including size + payload), i.e. exactly what `write_block`
  produces. With `frames_per_block_pwr = 2`, four frames form one block.

## Generation (temporary migration tooling)

`tools/extract_coding_oracle.c` `#include`s the unmodified `mpcenc.c` (with its
`main` renamed) to reach the static coding functions, then replays the
reference frame loop over a real WAV through the reference reader. It is not
built by Cargo and no test invokes it. See the tool header for the exact build
command; usage:

```sh
/tmp/extract_coding_oracle <outdir> <input.wav> <quality> [frames]
/tmp/extract_coding_oracle <outdir> --tables
```

`tools/extract_ans_tables.c` similarly `#include`s the unmodified
`libmpcpsy/ans.c` to dump `ans_tables.txt` (the static `InvFourier`/`Cos_Tab`/
`Sin_Tab` tables). Both are temporary migration tooling.

## These fixtures are frozen

Do not regenerate them to accommodate a Rust result. The recorded decisions are
the frozen psychoacoustic boundary; if a Rust stage disagrees, localise the
mismatch within that stage before touching anything.
