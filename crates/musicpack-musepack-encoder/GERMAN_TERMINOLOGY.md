# German encoder terminology map (migration archaeology)

The legacy Musepack encoder names most of its stages, state and helper
functions in German. This map records the **semantics derived from the
implementation and its consumers**, not dictionary translations, so the Rust
integration can avoid incorrect assumptions. It is an archaeology/reference
artifact; the Rust API uses English names and the C source is not renamed.

The 15F psychoacoustic entries below reuse the mappings already established by
`PSYCHOACOUSTIC_CONTRACT.md`; they are repeated here only for traceability.

## Integration-facing pipeline stages

### `Analyse_Filter` / `Analyse_Init` (`libmpcenc/analy_filter.c`)

1. **Literal:** "analysis filter" / "analysis initialise".
2. **Computes:** the 32-band polyphase analysis filterbank. `Analyse_Init`
   primes the 1632-sample per-channel history from one constant sample and
   writes a first subband frame; `Analyse_Filter` consumes one `PCMDataTyp`
   (1600 samples) and produces `SubbandFloatTyp[32]` (`L`/`R`, 36 samples).
3. **Domain:** input PCM is normalised float (`int16 * scale + denormal fix`);
   output subband coefficients are unnormalised floats, later normalised by
   `SCF_Extraktion`.
4. **State origin:** `X_L`/`X_R` history (480-sample overlap + 1152) owned by
   the encoder.
5. **Consumers:** `Psychoakustisches_Modell` (via the same PCM), `SCF_Extraktion`.
6. **Lifetime:** persistent across frames; initialised once per stream.
7. **Rust:** `filterbank::AnalysisFilterbank::{init, process}`.

### `SCF_Extraktion` (`codec/mpcenc/mpcenc.c`)

1. **Literal:** "scale-factor extraction" (`SCF` = *ScaleFactor*).
2. **Computes:** per band and per 12-sample subframe, the peak magnitude and
   summed power; derives the 3-element scale-factor index triple
   (`SCF_Index`), clamps it to `-6..=121`, combines the three subframes using
   the `Penalty` table and `CombPenalities`, computes the `SNR_comp`
   compensation, and normalises the subband samples in place with `invSCF`.
   On overflow it clamps to ±32767 and records a warning.
3. **Domain:** input `X` floats (post-M/S); output `SCF_Index` integers
   (`-6..=121`), `Power_L/R[32][3]` floats, `SNR_comp[32]` floats, and
   normalised/clamped `X`.
4. **State origin:** `SCF_Index` persists across frames (a zero-peak subframe
   keeps the previous index).
5. **Consumers:** `Allocate` (uses `SCF_Index`, `Power`, `SNR_comp`),
   `writeBitstream_SV8` (codes `SCF_Index`), `NS_Analyse` (uses `SCF_Index`).
6. **Lifetime:** `SCF_Index` cross-frame; the rest frame-local.
7. **Rust:** `coding::{ScfState, scf_extraktion, ScfOutput}`.

### `Allocate` (`codec/mpcenc/mpcenc.c`)

1. **Literal:** "allocate" (bit allocation).
2. **Computes:** chooses a per-band resolution (`Res`, roughly the quantiser
   bit depth) from the SMR, band power, `SNR_comp`, transient flags and the
   PNS probability; also emits PNS decisions (`Res == -1`).
3. **Domain:** in `SMR`/`Power`/`SNR_comp` floats and `Transient` flags; out
   `Res[32]` integers (`-1` PNS, `0` zero, `1..` resolution).
4. **State origin:** none (frame-local), but it mutates `SCF_Index` and `X`
   in place.
5. **Consumers:** `Quantisierung` (uses `Res`), `writeBitstream_SV8`.
6. **Lifetime:** frame-local.
7. **Rust:** `coding::{allocate, AllocationOutput}`.

### `PNS_SCF` (`codec/mpcenc/mpcenc.c`)

1. **Literal:** "PNS scale factor" (PNS = *Perceptual Noise Substitution*).
2. **Computes:** the scale factor used when a band is coded as PNS noise
   (`Res == -1`); derived from the band power and the profile PNS probability,
   with the `0.5`/`0.25`/`0.8` blend and the `1.2005…` factor.
3. **Domain:** float power → integer scale factor.
4. **State origin:** none.
5. **Consumers:** `Allocate` (PNS bands), `writeBitstream_SV8`.
6. **Lifetime:** frame-local.
7. **Rust:** inside `coding::allocate` (PNS path).

### `Quantisierung` (`codec/mpcenc/mpcenc.c`, `libmpcenc/quant.c`)

1. **Literal:** "quantisation".
2. **Computes:** maps normalised subband samples to signed integer `Q` using
   the resolution-dependent multiplier/offset, with optional noise shaping
   (`FIR`), and computes the error term from the **unclamped** rounded value.
3. **Domain:** input normalised floats + `Res` + `FIR`; output `Q` `int16`.
4. **State origin:** `Q` persists across frames (bands with `Res <= 0` keep
   their previous values).
5. **Consumers:** `writeBitstream_SV8` (Huffman-codes `Q`).
6. **Lifetime:** `Q` cross-frame.
7. **Rust:** `coding::{Quantizer, quantize_subband*}`.

### `NS_Analyse` / `FindOptimalANS` (`libmpcpsy/ans.c`)

1. **Literal:** "noise-shaping analysis" (ANS = *Adaptive Noise Shaping*).
2. **Computes:** per band, the LPC reflection coefficients / FIR filter and the
   noise-shaping order that maximise gain without exceeding the SMR; scales
   `SNR_comp`.
3. **Domain:** input `ANSspec` thresholds, `SMR`, `SCF_Index`, `Transient`;
   output `NS_Order[32]`, `FIR[32][6]`, scaled `SNR_comp`.
4. **State origin:** `FIR`/`NS_Order` are reset per frame inside `NS_Analyse`.
5. **Consumers:** `Quantisierung` (FIR), `writeBitstream_SV8` (order).
6. **Lifetime:** frame-local (reset each call).
7. **Rust:** `coding::{ns_analyse, NsOutput}`.

### `TransientenCalc` (`libmpcpsy/psy.c`)

1. **Literal:** "transient calculation".
2. **Computes:** expands the 19 short-partition transient flags to 32
   FFT-partition flags by mapping `wl_short[i] >> 2 ..= wh_short[i] >> 2`.
3. **Domain:** in `TransientL/R[19]` int flags; out `Transient[32]` int flags.
4. **State origin:** none.
5. **Consumers:** `NS_Analyse`, `Allocate`.
6. **Lifetime:** frame-local.
7. **Rust:** `psy::transienten_calc`.

### `Psychoakustisches_Modell` (`libmpcpsy/psy.c`)

See `PSYCHOACOUSTIC_CONTRACT.md`. Produces `SMR.L/R/M/S`, `TransientL/R`,
`ANSspec.*`; stateful across frames; Rust `psy::PsychoacousticModel`.

### `RaiseSMR` (`libmpcpsy/psy.c`)

See the contract. Enforces a minimum SMR (`minSMR`) and monotonicity across
bands. Rust `psy::raise_smr`.

### `MS_LR_Entscheidung` (`libmpcpsy/psy.c`)

See the contract. Per band, compares M/S vs L/R perceptual entropy, sets
`MS_Flag`, rewrites `SMR.L/R` and the subband samples to M/S. Rust
`psy::ms_lr_entscheidung`.

## Numeric / helper terminology

### `Schaetzer`, `ISNR_Schaetzer`, `ISNR_Schaetzer_Trans` (`libmpcenc/quant.c`)

1. **Literal:** "estimator"; `ISNR` = "instantaneous SNR" estimate.
2. **Computes:** an estimate of the signal-to-noise ratio obtainable at a
   given resolution from the sample statistics; `_Trans` is the transient
   variant. Used by `Allocate` to compare candidate resolutions.
3. **Domain:** float sample pointers + resolution → float SNR estimate.
4. **State origin:** none.
5. **Consumers:** `Allocate`.
6. **Lifetime:** frame-local.
7. **Rust:** inside `coding::allocate`.

### `Klemm` (`libmpcenc/analy_filter.c`)

1. **Literal:** surname of a co-author, not a word.
2. **Computes:** builds the post-transform `Ci_opt` prototype window in place.
3. **State origin:** called once at encoder init.
4. **Rust:** `filterbank::tables` (`CI_OPT`).

### `Skalenfaktoren` / `Init_Skalenfaktoren` (`libmpcenc/quant.c`)

1. **Literal:** "scale factors".
2. **Computes:** fills the `__SCF`/`__invSCF` tables.
3. **Rust:** frozen `coding::tables::{SCF_BITS, INV_SCF_BITS}`.

### `Ruhehoerschwelle` (`libmpcpsy/psy_tab.c`)

1. **Literal:** "threshold of quiet" / absolute hearing threshold.
2. **Computes:** `fftLtq`, `partLtq`, `invLtq` from the ear-model flag,
   `Ltq_offset`, `Ltq_max` and sample rate.
3. **Rust:** frozen per-config `PsyTables::{fft_ltq, part_ltq, inv_ltq}`.

### `Loudness` / `Lautheit` (`libmpcpsy/psy_tab.c`)

1. **Literal:** "loudness".
2. **Computes:** the per-partition A-weighting-like power factors.
3. **Rust:** frozen `PsyTables::loudness`.

### `Tonality` / `Tonalitaetskoeffizienten` (`libmpcpsy/psy_tab.c`)

1. **Literal:** "tonality coefficients".
2. **Computes:** `MinVal`, `O_MAX`, `O_MIN`, `FAC1`, `FAC2` from `TMN`/`NMT`.
3. **Rust:** frozen `PsyTables::{min_val, o_max, o_min, fac1, fac2}`.

### `Maskierung` / `Maskierungsschwelle`

1. **Literal:** "masking" / "masking threshold".
2. **Semantics:** the spread, tonality-offset and post-masked thresholds
   (`sim_Mask`, `tmp_Mask`, `PartThr`, `Thr`) and the `ANSspec` thresholds.
3. **Rust:** `psy` model internals + `ANSspec`.

### `Pegel` / `Schwelle` / `Grenze`

* **`Pegel`** — "level" (signal/SPL); used for power/energy quantities.
* **`Schwelle`** — "threshold" (hearing/masking); `Ruhehoerschwelle` above.
* **`Grenze`** — "bound/limit"; e.g. the SCF clamp to `-6..=121`.

### `Frequenz` / `Spektrum` / `Energie` / `Rauschen` / `Ton` / `tonal`

* **`Frequenz`** — frequency (Hz or FFT-bin index).
* **`Spektrum`** — spectrum (`erg`, `phs`, `cep`).
* **`Energie`** — energy/power (squared magnitude).
* **`Rauschen`** — noise; `PNS` substitutes noise for inaudible bands.
* **`Ton`/`tonal`** — tone/tonal vs noisy components; drives the tonality
  offset (`ApplyTonalityOffset`).

### `Berechnung` / `Entscheidung` / `Gewicht` / `Anpassung` / `Mittelwert`

* **`Berechnung`** — "calculation/computation".
* **`Entscheidung`** — "decision" (e.g. `MS_LR_Entscheidung`).
* **`Gewicht`/`Gewichtung`** — "weight/weighting" (loudness, weighted energy).
* **`Anpassung`** — "adaptation" (`AdaptLtq`, `AdaptThresholds`).
* **`Mittelwert`** — "mean/average" (integrators `a/b/c/d`).

### `Verzögerung` / `Fenster` / `Vorhersage`

* **`Verzögerung`** — "delay"; the encoder emits `DECODER_DELAY = 481` extra
  samples of frames so the decoder's delay is flushed.
* **`Fenster`** — "window" (analysis windows `Hann_*`).
* **`Vorhersage`** — "prediction" (phase/amplitude prediction in `CalcUnpred`;
  the `--predict` option is dead in this revision).

### `Kanal` / `Band` / `Bänder`

* **`Kanal`** — channel (L/R or M/S).
* **`Band`/`Bänder`** — subband / bands; `Max_Band` is the highest coded
  subband index (bandwidth-derived), while the psychoacoustic model is always
  called with `31`.

## Bitstream/container terms

* **`writeBitstream_SV8`** — writes one audio frame's entropy-coded fields and
  flushes an `AP` block when full. Rust `coding::FrameEncoder`.
* **`writeBlock`** — frames `key + size [+ CRC] + payload`. Rust
  `sv8::write_block`.
* **`writeSeekTable`** — emits the `ST` seek table and patches the `SO`
  placeholder. Rust `blocks::Sv8StreamWriter::write_seek_table`.
