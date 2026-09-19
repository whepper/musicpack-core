# Frozen filterbank oracle (Phase 15D)

Bit-exact reference outputs from the C analysis filterbank, used to gate the
Rust port. The tests that consume these files (`tests/filterbank_fixtures.rs`)
never invoke the C encoder, so they keep working after it is deleted.

## Representation

Raw **little-endian IEEE-754 `f32`** bit patterns (exact; never decimal text),
band-major:

```
for each Analyse_Filter call (in order):
    for band in 0..=31:
        L[0..36]   (36 f32)
        R[0..36]   (36 f32)
```

A single call is therefore `32 × 72 × 4 = 9216` bytes. `index.txt` records each
case's inputs, call count, byte count, SHA-256 and an input checksum.

## Cases

| Case | Kind | Calls | Purpose |
|---|---|---|---|
| `init` | constant 0.25/-0.5 | 1 | `Analyse_Init` output only |
| `silence` | zeros | 1 | no-signal steady state |
| `impulse` | unit impulse at the first analysed sample | 1 | broadband response |
| `constant` | 0.5 | 1 | DC |
| `alternating` | ±0.5 per sample | 1 | high frequency |
| `ramp` | period-512 triangle | 1 | low frequency |
| `transient` | spikes at j=100 and j=500 | 1 | transients/overlap |
| `stereo` | deterministic LCG, decorrelated L/R | 1 | stereo independence |
| `state_carry` | deterministic LCG | 3 | consecutive frames, overlap |

The LCG and the signal formulas are reproduced identically in the Rust test;
`input_xor` in `index.txt` is the XOR of every input-sample bit pattern and
fails fast if the two generators diverge.

## Reference environment

Extracted from the reference encoder build at `../musicpack` (pristine r475 /
git `05d97a5`, version normalised to 1.32.0), Apple clang 21, arm64, `-O0`,
`-ffp-contract=off`, with the scalar analyser kernels forced via
`mpc_enc_set_impl(MPC_ENC_SCALAR)`. The filterbank is sample-rate independent.

`ci_opt_bits.txt` is the post-`Klemm` `Ci_opt[512]` bit dump; the Rust
`CI_OPT` is computed from the base integers and must match it. `modulation_bits.txt`
is the `M[1024]` bit dump; `MODULATION` is frozen from it because it is derived
from libm `cos`, which is not bit-portable across toolchains.

## Generation (temporary migration tooling)

`tools/extract_filterbank_oracle.c` links the reference `libmpcenc_static.a`
and calls the production `Klemm`/`Analyse_Init`/`Analyse_Filter`; it is not built
by Cargo and no test invokes it.

```sh
REF=/path/to/musicpack
cc -O0 -ffp-contract=off -std=gnu11 \
   -I "$REF/codec/include" -I "$REF/codec/libmpcenc" \
   tools/extract_filterbank_oracle.c \
   "$REF/build/codec/libmpcenc/libmpcenc_static.a" -lm -o /tmp/extract
/tmp/extract cases  <outdir>   # *.f32le + manifest on stdout
/tmp/extract tables <outdir>   # ci_opt_bits.txt, modulation_bits.txt
```

## These fixtures are frozen

Do not regenerate them to accommodate a Rust result. If the reference
behaviour is genuinely wrong, document it and adjust the contract deliberately
before touching the oracle. Note that the reference's `FASTER` analysis loop
reads the 32 PCM samples in an interleaved order (not a simple reversal); the
Rust port reproduces that exactly, and these fixtures are what proved it.
