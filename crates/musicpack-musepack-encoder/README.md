# musicpack-musepack-encoder

The safe-Rust **Musepack SV8 encoder** that replaces the legacy C Musepack
encoder, plus its compatibility oracle.

> **Status: Phase 15L — migration complete; integer encoder parity (J.1)
> landed afterwards.**
> The crate is the production `PCM → SV8` encoder
> (`encoder::MusepackEncoder`): it reproduces the reference `mpcenc` stream
> byte-for-byte for the original **21** committed whole-encoder cases across
> q4–q7 and 44.1/48/37.8/32 kHz **and** for the full **integer quality
> matrix** — qualities `0..=10` × 44.1/48/37.8/32 kHz (44 configurations,
> two signal kinds each) plus mono at q5 × 4 rates, all pinned by
> `tests/data/encoder/matrix_manifest.txt` and the `encoder_matrix` test
> (15G.1 resolved the two decoder-delay-frame divergences — byte-swapped
> frozen CVD tables; see `tests/data/encoder/README.md`). Fractional
> quality (`--quality 5.5`-style interpolation and clipping) remains a
> deliberately deferred parity slice (J.2). The legacy C repository is
> retained as-is as the immutable historical reference; the frozen
> compatibility corpus in this crate is the permanent compatibility
> boundary.

## Why this crate exists separately

The legacy C encoder (`codec/libmpcenc`, `codec/libmpcpsy`, and the encoding
logic in `codec/mpcenc/mpcenc.c` in the reference repository
`github.com/whepper/musicpack`) is roughly two decades old, is not expected to
evolve upstream, and is **not** the desired long-term implementation. The
legacy repository is retained as-is as the immutable historical reference
(it must not be modified); the frozen compatibility corpus in this crate is
the permanent compatibility boundary, so the Rust encoder never depends on
the C sources at runtime.

## Licensing treatment (engineering boundary, not legal advice)

* The legacy C encoder is **LGPL-2.1-or-later**. A faithful Rust
  implementation derived from it is treated conservatively as
  **LGPL-2.1-or-later** as well.
* This crate therefore carries that license (`LICENSE`) and is isolated from
  the permissively-licensed core.
* **`musicpack-core` must never depend on this crate.** The dependency points
  one way only, and only if a later phase needs it.
* `publish = false` while the crate is under construction.
* This is the repository's chosen engineering treatment. It is **not** a legal
  conclusion: open questions (derivative-work status, static-link/WASM
  obligations, generated-table status) are tracked as open questions in the
  migration notes and must be resolved by counsel. No clean-room process is
  claimed.

## Scope established so far

Phase 15B:

1. the crate and its licensing boundary;
2. the frozen reference corpus (mirrored manifest) and the reference
   environment record;
3. the differential-oracle foundation (`differential`, plus the opt-in live
   reference test);
4. the bit-writing foundation.

Phase 15C (this phase):

5. SV8 size encoding — plain and self-including (`sv8::encode_size`,
   `sv8::encode_size_self_including`);
6. CRC-32/IEEE and block framing (`sv8::crc32`, `sv8::write_block`,
   `sv8::write_block_with_min_size`);
7. the `SH`/`EI`/`RG`/`SO`/`ST`/`SE` payload and stream writers (`blocks`),
   including the seek-table placeholder patch;
8. seven committed, self-contained byte-level container fixtures and the
   Rust-only tests that consume them (`tests/container_fixtures.rs`).

Phase 15D (this phase):

9. the scalar analysis filterbank (`filterbank`): the `Klemm` `Ci_opt`
   prototype window and frozen modulation matrix, `Analyse_Init` /
   `Analyse_Filter`, and the `FASTER` vectoring/matrixing kernels;
10. nine committed, C-free filterbank oracle fixtures plus the frozen
    `Ci_opt`/`M` bit dumps (`tests/data/filterbank/`) and the bit-exact
    differential test (`tests/filterbank_fixtures.rs`).

Phase 15E (complete):

11. SCF extraction, noise-shaping (ANS) analysis, allocation/PNS and
    quantisation, each gated bit-exactly against `tests/data/coding/`;
12. Huffman coding and the `AP` audio payload (`coding::frame`), including
    `encodeLog`/`encodeEnum`, the frozen `huffsv7` codebooks and `Cnk`/log
    tables (`coding::huffman_tables`).

Phase 15F (this phase):

13. the scalar psychoacoustic model (`psy`): the forward `rdft` and windowed
    spectrum kernels, the `FAST_MATH` primitives, the frozen reference tables,
    `Psychoakustisches_Modell`, `RaiseSMR`, `MS_LR_Entscheidung`,
    `TransientenCalc` and CVD;
14. the psychoacoustic oracle (`tests/data/psy/`) and C-free exact tests at
    three levels: primitives (`psy::math`/`psy::fft` unit tests),
    `MS_LR_Entscheidung` (`tests/psy_ms.rs`) and full frame outputs
    (`tests/psy_model.rs`).

See `PSYCHOACOUSTIC_CONTRACT.md` for the input/output/state mapping and the
numerical contract.

Module map:

| Module | Contents |
|---|---|
| `bitwriter` | MSB-first bit writer |
| `sv8` | size fields, CRC, Golomb, block keys, block framing |
| `blocks` | `SH`/`EI`/`RG`/`SO`/`ST`/`SE` payloads and `Sv8StreamWriter` |
| `filterbank` | `Klemm` tables, `Analyse_Init`/`Analyse_Filter`, vectoring/matrixing |
| `coding` | SCF, noise-shaping, allocation/PNS, quantisation, Huffman + `AP` frame coding (Phase 15E–15E.4) |
| `psy` | scalar psychoacoustic model: FFT/spectra, masking, SMR, transients, MS/LR, CVD, ANS thresholds (Phase 15F) |
| `encoder` | full `PCM → SV8` pipeline (`MusepackEncoder`) and the frame-level diagnostic trace (Phase 15G) |
| `differential` | pure byte-comparison primitives |
| `error` | `EncoderError` |

Explicitly **not** implemented (do not "prepare" these by copying C):

* production integration of the model into a full encoder, the live
  whole-corpus equality test, SIMD/optimisation and C-encoder removal (15G+).

## Architecture

```text
                     PCM (f32, planar/interleaved host input)
                              │
        ┌─────────────────────┴─────────────────────┐
        ▼                                           ▼
  C reference encoder                       Rust encoder (this crate)
  (immutable historical reference)          analysis → psy → coding → SV8
        │                                           │
        └──────────────► differential ◄─────────────┘
                     (see below)
```

Design rules for all of this crate:

* `#![forbid(unsafe_code)]`, no FFI, no bindgen, no FFmpeg, no runtime
  dependency on the C encoder.
* `std`-only and `wasm32-unknown-unknown`-clean.
* Explicit per-instance state; **no global mutable encoder state** (the C
  encoder's file-scope tables/state are a deliberate deviation that the Rust
  encoder removes while preserving output).
* Deterministic: identical input + configuration ⇒ identical bytes.

## SV8 primitives (target vocabulary for later phases)

These are the established Musepack SV8 primitives the bit writer feeds. They
are recorded here so the foundation has a clear target; they are **not**
implemented yet. Terminology follows the existing implementation rather than
inventing new vocabulary.

**Bit-level**

* MSB-first bit packing: the first bit written is the most significant bit of
  the first byte; multi-byte numeric fields are big-endian on the wire.
* Fixed-width fields of 1..32 bits (resolution, scale factors, flags, sizes,
  raw low bits in escape paths).

**Block framing** (keys emitted by the encoder)

| Key | Payload |
|---|---|
| `SH` | stream header / stream info; the only block written with a CRC |
| `RG` | replay gain (title/album gain/peak) |
| `EI` | encoder info: 7-bit profile, 1-bit PNS, 8/8/8-bit version |
| `SO` | reserved seek-table offset placeholder |
| `AP` | one or more audio frames |
| `ST` | seek table |
| `SE` | end of stream |

A block is `2-byte ASCII key` + `encodeSize(...)` + optional 32-bit big-endian
CRC (`SH` only) + payload. Payloads start byte-aligned.

**Variable-length / entropy primitives**

* `encodeSize(size)` — self-including base-128 varint: repeated 7-bit groups,
  continuation bit `0x80` on every byte except the last.
* `encodeGolomb(nb, k)` — unary quotient in 31-bit chunks, terminating one bit,
  then `k` remainder bits.
* `encodeEnum(bits, N)` — combinatorial rank via the `Cnk` tables, with an
  escape for the remaining ranks.
* `encodeLog(value, max)` — truncated binary log code (`mpc_log2` /
  `mpc_log2_lost`).
* `mpc_enc_seek_delta(diff)` — sign-magnitude `(magnitude << 1) | sign`,
  computed in unsigned 64-bit.

**`SH` field order and widths**

`version(8) = 8`, `samples` (varint), `beg_silence` (varint), sample-rate index
`(3)` (0=44100, 1=48000, 2=37800, 3=32000), `max_band - 1 (5)`,
`channels - 1 (4)`, MS-coding flag `(1)`, `frames_per_block_pwr >> 1 (3)`. The
CRC covers the payload after the CRC field.

**`EI` field order**

`profile(7)`, `pns(1)`, `major(8)`, `minor(8)`, `build(8)`.

**Audio frames (`AP`)**

No per-frame length is written to the wire; frames are self-delimiting and are
buffered into an `AP` block. Frame fields (implemented later) are: max used
band (log code on the first frame of a block, Huffman delta afterwards),
per-band resolution (Huffman), M/S flags (log + combinatorial), scale factors
(identical/differential Huffman), and quantised samples (per-resolution Huffman
codebooks).

**CRC-32**

IEEE reflected polynomial `0xEDB88320`, init and final XOR `0xFFFFFFFF`, stored
big-endian.

## Reference environment and the frozen corpus

The compatibility contract is **same-reference-environment bitstream
identity**, not universal cross-platform byte identity. See
`tests/data/README.md` for the full record. In brief:

* frozen manifest: 282 vectors (94 inputs × q5/q6/q7), mirrored at
  `tests/data/encoder_reference_manifest.txt`, SHA-256 `2f628c79…`;
* reference build: pristine r475 / git `05d97a5` patched to `1.32.0`, Apple
  clang 21, arm64, `-O0`, `-ffp-contract=off`, `FAST_MATH` + `CVD_FASTLOG`;
* the manifest is **not** cross-platform: other toolchains produce different
  bytes (FP codegen + libm-derived tables), so CI uses a live same-toolchain
  reference rather than the frozen manifest;
* the reference encoder is **never** modified to make a Rust result pass.

## Compatibility contract (frozen 15G.2)

```text
Rust Musepack Encoder Compatibility Contract

Reference:
  scalar-forced legacy C encoder (`--impl scalar --psy-impl scalar`)

Comparison:
  exact encoded SV8 byte identity

Floating-point target:
  frozen reference environment established during 15A–15F
  (Apple clang 21, arm64, -O0, -ffp-contract=off, FAST_MATH + CVD_FASTLOG)

Component boundaries:
  15C–15F frozen oracles (container, filterbank, coding, psychoacoustics)

Whole-encoder corpus:
  21 deterministic PCM cases (q4–q7 at 44.1 kHz; q5 at 48/37.8/32 kHz)

Integer-parity matrix (J.1):
  97 manifest rows: noise + transient for all 44 integer
  (quality 0..=10 × 44.1/48/37.8/32 kHz) configurations,
  mono q5 × 4 rates, and long multi-AP cases
  (`tests/data/encoder/matrix_manifest.txt`)

Tolerance:
  none

SIMD:
  not part of the strict compatibility target; the SIMD-dispatching C
  encoder is not guaranteed to produce identical bytes on all inputs

Optimisation:
  not yet performed
```

The committed whole-encoder fixtures (`tests/data/encoder/*.mpc`) were
verified byte-identical to scalar-forced reference output, so they already
embody the scalar target; `tools/gen_encoder_fixtures.py` passes the scalar
flags explicitly so any regeneration stays scalar by construction. No claim of
universal cross-platform bit identity is made.

## Compatibility oracle

Comparison levels, coarsest to finest:

| Level | Compared | Status |
|---|---|---|
| `Container` | block framing, stream header, metadata/gapless | `compare_bytes` available; structured comparison later |
| `Frame` | frame count/boundaries/coded fields | later |
| `Bitstream` | exact payload/whole-file bytes and SHA-256 | `compare_bytes` available; live oracle test available |
| `DecodedPcm` | PCM decoded from both streams (Rust decoder) | later |
| `Metadata` | tags, replay gain, encoder info, gapless counts | later |

Three tests anchor the oracle:

* `tests/reference_manifest.rs` — always runs; pins the frozen manifest bytes
  and structure and keeps working after the C encoder is deleted.
* `tests/reference_oracle.rs` — opt-in; runs the reference binary over the
  generated corpus and requires all 282 hashes to match. It skips cleanly when
  `MUSICPACK_MPCENC` / `MUSICPACK_ENCODER_CORPUS` are unset.
* `tests/container_fixtures.rs` — always runs, C-free; verifies the SV8
  container writers against the committed fixtures in `tests/data/`.
* `tests/filterbank_fixtures.rs` — always runs, C-free; reproduces the
  reference PCM inputs and requires the analysis filterbank to match the
  committed coefficient bit patterns exactly.
* `tests/coding_scf.rs`, `tests/coding_ns.rs`, `tests/coding_allocate.rs`,
  `tests/coding_quant.rs`, `tests/coding_ap.rs` — always run, C-free; verify
  SCF extraction, noise-shaping analysis, allocation/PNS, quantisation and the
  Huffman/`AP` payload against `tests/data/coding/`.
* `tests/psy_model.rs` — always runs, C-free; reproduces the reference PCM
  inputs and requires the full model (`SMR`, `Transient`, `TransientenCalc`,
  `ANSspec`) to match `tests/data/psy/` exactly for q4–q7 and 44.1/48/37.8/32
  kHz.
* `tests/psy_ms.rs` — always runs, C-free; verifies `MS_LR_Entscheidung`
  against the dedicated MS sub-oracle.
* `tests/encoder_whole.rs` — always runs, C-free; reproduces the reference
  `mpcenc` streams byte-for-byte for all 21 committed whole-encoder cases.
* `tests/encoder_matrix.rs` — always runs, C-free; replays
  `tests/data/encoder/matrix_manifest.txt`: byte-identical output plus
  structural SV8 validation for every integer `(quality, rate)` pair
  (`0..=10` × 44.1/48/37.8/32 kHz, two signals each), mono at q5 × 4 rates,
  and the long multi-`AP` cases.
* `psy::math`/`psy::fft` unit tests — always run, C-free; verify the
  `FAST_MATH` primitives and the spectrum/FFT kernels against
  `tests/data/psy/math.txt` and `tests/data/psy/fft/`.

## Migration and removal strategy

```text
C encoder (oracle) ──► Rust encoder reaches bitstream parity ──► C encoder deleted
                                                     │
                                        frozen manifest + corpus retained
```

Planned stages (revised from Phase 15A after the source investigation):

* **15B** ✅ foundation — this crate, frozen oracle, bit writer.
* **15C** ✅ SV8 container writer (size fields, CRC, block framing,
  `SH`/`EI`/`RG`/`SO`/`ST`/`SE`), validated against committed reference
  container fixtures.
* **15D** ✅ analysis filterbank (scalar), gated bit-exactly against committed
  reference coefficient fixtures.
* **15E** ✅ coding layers (SCF extraction, allocation/PNS, quantisation,
  Huffman, frame coding) driven by recorded psychoacoustic decisions so they
  can be proven independently.
* **15F** ✅ psychoacoustic model + FFT + tables (the frozen boundary, highest
  risk), proven bit-exactly against a dedicated oracle; scalar only.
* **15G** ✅ full-encoder integration; 21/21 whole-stream cases byte-identical
  (15G.1 fixed the two decoder-delay-frame divergences: byte-swapped frozen
  CVD tables).
* **15H** ✅ native/WASM integration and performance (correctness first;
  15H.3 productionised the approved joint-4 `vectoring` ILP only —
  `matrixing` untouched, scalar oracle retained in tests).
* **15I–15K** ✅ retirement/consumer audit: `musicpack-core` has no production
  C encoder dependency; `mpccut` gained its proven Rust replacement in
  `musicpack-mpc-tools` (12/12 byte-identical); SV7 is explicitly
  unsupported so `mpc2sv8` needs no replacement (SV7 decoding is decoder
  scope, out of scope).
* **15L** ✅ migration-scope audit and closure: nothing within Phases 15B–15K
  was accidentally omitted. Intentionally out of scope and not ported:
  `mpcgain`, `mpcchap`, the authoring-CLI draft/identify pipeline, the
  server/sonic components, and file-level `mpcdec`/`mpcenc` CLIs. No
  WAV→MPC file CLI is added unless a future product workflow requires one.

Legacy-repository boundary (corrected scope): the earlier removal plan below
is superseded. The legacy C repository is **not** modified or deleted — it
remains the immutable historical reference, and the frozen manifest + corpus
here are the permanent compatibility boundary. No production cutover touches
the legacy repository.

Superseded removal plan (kept for provenance; do not execute):

1. the Rust encoder passes the frozen 282-vector differential and the decoded
   PCM round-trip through the Rust decoder;
2. the frozen manifest + generator are retained (the manifest already has a
   permanent home here; the generator moves here or is reimplemented);
3. `codec/libmpcenc`, `codec/libmpcpsy`, `codec/mpcenc`, their CMake wiring
   and CTest cases are deleted;
4. the authoring pipeline's external `mpcenc` invocation is replaced by the
   Rust encoder;
5. the removal is recorded with the reference commit and manifest hash so the
   boundary remains auditable.

The C encoder must not be deleted earlier, and must not be kept indefinitely
by accident.

## Testing

```sh
cargo test -p musicpack-musepack-encoder
cargo fmt --all --check
cargo clippy -p musicpack-musepack-encoder --all-targets -- -D warnings
cargo check -p musicpack-musepack-encoder --target wasm32-unknown-unknown

# Opt-in live reference oracle (requires the C reference repo + generated corpus):
python3 /path/to/musicpack/tests/generate_encoder_corpus.py /tmp/enc-corpus
MUSICPACK_MPCENC=/path/to/musicpack/build/codec/mpcenc/mpcenc \
MUSICPACK_ENCODER_CORPUS=/tmp/enc-corpus \
  cargo test -p musicpack-musepack-encoder --test reference_oracle -- --nocapture

# Scalar filterbank baseline (release, ignored by default):
cargo test --release -p musicpack-musepack-encoder --lib \
  filterbank_throughput_baseline -- --ignored --nocapture
```

## Performance baseline (15G.2, measurement only)

Recorded so Phase 15H has a reproducible starting point. No optimisation has
been performed; the encoder is scalar debug-shaped code.

* input: deterministic stereo noise, 93,728 frames at 44.1 kHz (2.125 s
  audio), q5, release build;
* platform: Apple M5, 16 GB RAM, macOS;
* result: ~13–20 ms per encode (~52 KB output), i.e. comfortably over 100×
  realtime;
* method: wall-clock around one `MusepackEncoder::encode` call (temporary
  test, removed afterwards; first run includes warmup).


## License

LGPL-2.1-or-later. See `LICENSE`. The legacy C encoder it replaces is also
LGPL-2.1-or-later; `musicpack-core` remains BSD-3-Clause and independent.
