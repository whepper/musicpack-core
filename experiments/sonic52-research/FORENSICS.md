# Sonic52 Phase 2: forensic preprocessing research report

Research only. No code was modified, no model was built, no weights or
proprietary artifacts were downloaded, obtained, or redistributed.
Nothing in this report changes production behaviour.

> Sonic52 is an independent research model inspired by publicly
> reported/reverse-engineered structural characteristics of Plex's
> historical Sonic Analysis model. It is not "the Plex model", not Plex
> Sonic Analysis, and not a Plex reimplementation.

## 1. Executive summary

- The public reverse-engineering landscape for Plex Sonic Analysis is a
  **single technical source**: `knoopx/plex-sonic-analysis.md`
  (gist `567b74342ad82eac1b97334caebb1d7a`, 4 revisions, May–Jul 2026).
  No independent replication of its findings was found.
- That source firmly establishes **output-side** facts (52 float32s per
  `.tree` slot; a `Music.tflite` model; a reported
  `conv → flatten → dense(200) → dense(52, sigmoid)` head; a reported
  normalized-Euclidean distance over mean track embeddings) but states
  the **model input shape is unknown** and gives **no preprocessing
  parameters at all** (no sample rate, framing, FFT, mel, or aggregation
  values).
- The MusiCNN lineage is, by contrast, documented down to the source
  line: 16 kHz mono, 512-frame / 256-hop, symmetric Hann (zero-phase,
  unnormalized), power spectrum, 96 Slaney mel bands (linear weighting,
  unit-triangle, 0–8000 Hz), `log10(10000·x+1)`, 187-frame (~3 s)
  patches, default 93-frame patch stride, mean-of-patches aggregation at
  validation time, dense-200-ReLU → dense-N-linear → sigmoid head.
- The single most important structural finding is a **difference**, not
  a match: stock public MusiCNN tagging models emit **50** outputs, not
  52, and their documented embedding layer is the pre-sigmoid
  200-vector, while Plex persists the **post-sigmoid 52-vector**. Plex's
  model is therefore **not** stock `msd-musicnn-1`; it is at most a
  retrained MusiCNN-style variant (same code family, different head).
  The `dense` / `dense_1` layer naming parallel and the ~3.1 MB size
  (≈787k float32 params) are consistent with that hypothesis and prove
  nothing on their own.
- **No preprocessing value is Plex-proven.** Every frontend parameter
  in the recommended Sonic52 contract below is a lineage-derived
  hypothesis or an explicit experimental choice — never a Plex fact.
- The recommended initial contract adopts the lineage frontend as
  hypothesis H0 (it is the only fully-specified candidate consistent
  with the reported architecture), keeps the input-tensor shape as an
  explicit open decision, and proposes ablations rather than invented
  values wherever evidence runs out.

## 2. Sources examined

### 2.1 Plex-specific reverse engineering (Category A material)

| # | Source | What it is | Method documented? |
| --- | --- | --- | --- |
| S1 | `https://gist.github.com/knoopx/567b74342ad82eac1b97334caebb1d7a` (`plex-sonic-analysis.md`) | RE of Plex Media Server 1.43.2.10687 (NixOS `plexmediaserver`): `.tree` format, `Music.tflite` architecture string, binary strings, `/nearest` API, distance formula with worked verification | Partially. `.tree` stride verified by `xxd` offsets; distance formula verified against one album's API results; **model-architecture method never stated** (no Netron screenshot, no layer dump, no input-tensor readout) |
| S1r | Same gist, revisions 1–4 (May–Jul 2026) | Claim evolution: v1 claimed raw Euclidean with ~1.8× absolute mismatch and guessed input `likely [1,H,W,1]`; v2 corrected to the normalized formula (0–2% match), dropped the shape guess and the `kTfLiteCustom` string, added `kTfLiteActRelu` | Self-correction is visible and honest, but it is still single-author iteration with no independent review |

### 2.2 MusiCNN-lineage primary sources (Category B material)

| # | Source | What was taken from it |
| --- | --- | --- |
| S2 | Essentia `src/algorithms/spectral/tensorflowinputmusicnn.cpp` (+ `.h`) | Exact inference frontend: 512 / 96 / 16 kHz / slaneyMel / linear / unit_tri, `highFrequencyBound = sr/2`, shift 1, scale 10000, log10; Hann unnormalized; hard 512-frame acceptance |
| S3 | Essentia `src/algorithms/machinelearning/tensorflowpredictmusicnn.cpp` (+ `.h`) | FrameCutter 512/256 (other params defaulted); `patchSize` 187; `patchHopSize` default 93; `batchSize` 64; `lastPatchMode` discard; input expectation "16 kHz"; recommended pipeline `MonoLoader(sampleRate=16000)` |
| S4 | Essentia `src/algorithms/standard/{framecutter,windowing,melbands}.h` | Framing/window/mel defaults: centered zero-padded framing, silent-frames→noise default, symmetric Hann zero-phase, MelBands `type=power` default, low bound 0 |
| S5 | `https://essentia.upf.edu/models/feature-extractors/musicnn/msd-musicnn-1.json` (public model metadata, read as text; no weights fetched) | Schema: input `model/Placeholder` float `[187, 96]`; outputs `model/Sigmoid` float `[1, 50]`, `model/dense_1/BiasAdd` `[1, 50]` logits, `model/dense/BiasAdd` `[1, 200]` embeddings; `inference.sample_rate 16000` |
| S6 | `jordipons/musicnn` `DOCUMENTATION.md` | 3-second training inputs; `penultimate` feature key; `MSD_musicnn_big` variant (×512 mid-end, 500-unit pooling); MTT/MSD 50-tag vocabularies |
| S7 | `jordipons/musicnn-training` `src/config_file.py` | Training config: 16 kHz, hop 256, n_fft 512, 96 mels; `n_frames` 187; `pre_processing logC`; `pad_short repeat-pad`; `num_classes_dataset` **50** |
| S8 | `jordipons/musicnn-training` `src/preprocess_librosa.py` | Training frontend: `librosa.load(sr=16000)` then `librosa.feature.melspectrogram` with library defaults (center=True, reflect padding, power 2.0, Slaney norm, fmin 0, fmax sr/2); float16 storage pickles |
| S9 | `jordipons/musicnn-training` `src/train.py` | `logC ≡ log10(10000·x+1)` — byte-identical formula to the Essentia inference frontend; validation sampling = non-overlapping patches, predictions **averaged** per id; terminal `tf.nn.sigmoid` over linear logits; sigmoid-cross-entropy loss |
| S10 | `jordipons/musicnn-training` `src/models.py`, `models_backend.py`, `models_frontend.py` | Model 11 (≈787k params): input batch-norm → 5 musically-motivated Conv2Ds (timbral 7×67 / 7×38 at 96 mels; temporal 128×1, 64×1, 32×1; all ReLU+BN, frequency max-pooled) → dense mid-end → global max+avg temporal pooling → flatten → BN → dropout → **dense(200, ReLU)** → BN → dropout → **dense(N, linear)** (+ sigmoid outside); all dense layers bias-inclusive by `tf.compat.v1.layers.dense` default |

### 2.3 Secondary / contextual sources

| # | Source | Assessment |
| --- | --- | --- |
| S11 | `https://forgejo.cwavs.xyz/Cwavs/Hedgehog` (archived hobby project README) | Asserts without evidence that Essentia/MusiCNN "powers Plex's Sonic Analysis" and that "Plex uses Annoy"; its own example contradicts itself (`Music.tflite` named in prose, `msd-musicnn-1.pb` in the command). Folk-knowledge corroboration only — no technical weight. |
| S12 | TensorFlow `AudioSpectrogram` op docs (`tensorflow.org/jvm/.../audio/AudioSpectrogram`) | Confirms an exact TF op name exists (`windowSize`, `stride`, `magnitudeSquared`); no Plex connection. See §6. |
| S13 | Plex support doc "Sonic Analysis for Music" | Feature-level only (Plex Pass, library opt-in). No parameters. |
| S14 | YAMNet→TFLite conversion write-up (method reference) | Establishes that TFLite audio models *can* carry fixed input shapes and STFT subgraphs inspectable in Netron — i.e. the inspection method S1 did not publish exists in principle. Not applied to any proprietary artifact here. |
| S15 | Targeted search `"Music.tflite" plex` | No second technical RE source found. The RE landscape is S1 alone. |

### 2.4 Deliberately not used

Leaked/private/proprietary material (none sought); Plex binaries (not
obtained); `Music.tflite` (not downloaded — its input tensor, layer
shapes, and quantization are therefore **unknown**, §10); the Plex
forum "how does it work" thread (user-level, no parameters).

## 3. Evidence table

Confidence: High / Medium / Low / Unknown. `Plex?` = directly tied to
Plex's implementation (A), lineage-only (B), or our hypothesis (C).

### 3.1 Audio input

| Parameter | Candidate value(s) | Evidence | Confidence | Plex? | Experimental implication |
| --------- | ------------------ | -------- | ---------- | ----- | ------------------------ |
| Sample rate | 16000 Hz | S3 input contract; S5 metadata; S7/S8 training | Medium (lineage unanimous) | B | Adopt 16 kHz as H0; resampler is our choice |
| Channel config | mono | S3 recommends `MonoLoader`; S8 `librosa.load` defaults mono | Medium | B | Adopt mono as H0; downmix rule is our choice |
| Mono conversion | arithmetic mean (Essentia `MonoMixer` default) vs librosa default (mean) | Both lineage ends average channels; exact Plex path unknown | Low | B | Choose explicitly; inaudible-difference risk is low but real |
| Resampling method/quality | unknown | S8 uses `librosa.load` resampling (library-default resampler, version-dependent); S3 just requires 16 kHz in | Unknown | — | Our choice (MusicPack already owns `rubato`-based resampling); record version exactly |
| Amplitude scaling | none documented (raw decoded range) | No lineage source scales amplitude before the frontend | Low | B | Assume unity; note as assumption |
| Clipping behaviour | unknown | Nothing documented anywhere | Unknown | — | Fail-closed saturation policy is our choice; keep out of v1 contract |

### 3.2 Framing

| Parameter | Candidate value(s) | Evidence | Confidence | Plex? | Experimental implication |
| --------- | ------------------ | -------- | ---------- | ----- | ------------------------ |
| Frame size | 512 | S2 hardcoded; S7 `n_fft` 512 | High (lineage) / Unknown (Plex) | B | H0 = 512 |
| Hop size | 256 | S3 hardcoded; S7 `hop` 256 | High (lineage) / Unknown (Plex) | B | H0 = 256 |
| Window function | symmetric Hann | S2 `Windowing(normalized=false)` + Essentia Hann default; S8 librosa default Hann | High (lineage) / Unknown (Plex) | B | H0 = symmetric Hann |
| Window normalization | unnormalized (area-preserving) | S2 sets `normalized=false` explicitly | High (lineage) / Unknown (Plex) | B | H0 = unnormalized |
| Zero-phase rotation | true | Essentia `zeroPhase` default, unset by S2 | Medium (lineage default, not explicit) | B | H0 = zero-phase; verify against Hann symmetry in implementation slice |
| Centered vs non-centered | centered, zero-padded boundaries | Streaming FrameCutter `startFromZero=false` default (first frame spans [−256, +256)); S8 librosa `center=True` (reflect-padded) | Medium (defaults agree on centered; **padding differs**: zeros vs reflect) | B | H0 = centered; boundary-padding rule is our choice — ablation candidate |
| Last partial frame | zero-padded and kept (`validFrameThresholdRatio` 0) then patch-level `discard` | S4 defaults + S3 `lastPatchMode=discard` | Medium | B | H0 = discard partial patch; note train/inference mismatch (S7 `repeat-pad` at train time) |

### 3.3 Spectral analysis

| Parameter | Candidate value(s) | Evidence | Confidence | Plex? | Experimental implication |
| --------- | ------------------ | -------- | ---------- | ----- | ------------------------ |
| FFT size | 512 (= frame) | S2 `Spectrum(size=512)`; S7 `n_fft` 512 | High (lineage) / Unknown (Plex) | B | H0 = 512-pt real FFT |
| Spectrum type | power/energy | S4 MelBands `type=power` default, unset by S2; S8 librosa `power=2.0` default | High (lineage) / Unknown (Plex) | B | H0 = power |
| Magnitude vs power | power (squared) | Same as above | High (lineage) / Unknown (Plex) | B | H0 = power; do not "try magnitude" without an experiment |
| Frequency range | 0–8000 Hz | S2 `highFrequencyBound = sr/2`; S4 low default 0; S8 librosa fmin 0 / fmax sr/2 | High (lineage) / Unknown (Plex) | B | H0 = 0–8 kHz |
| Nyquist handling | inclusive bound at sr/2 | S2 passes `sampleRate/2` as the bound | Medium | B | Carry the exact bound, not "≈8 kHz" |

### 3.4 Mel frontend

| Parameter | Candidate value(s) | Evidence | Confidence | Plex? | Experimental implication |
| --------- | ------------------ | -------- | ---------- | ----- | ------------------------ |
| Mel bands | 96 | S2/S3/S5/S7 unanimous | High (lineage) / Unknown (Plex) | B | H0 = 96 |
| Mel scale | Slaney | S2 `slaneyMel`; S8 librosa `norm='slaney', htk=False` | High (lineage) / Unknown (Plex) | B | H0 = Slaney; exact formula version must be pinned at implementation |
| Lower bound | 0 Hz | S4 default, unset by S2 | Medium | B | H0 = 0 Hz |
| Upper bound | 8000 Hz | S2 explicit | High (lineage) / Unknown (Plex) | B | H0 = 8000 Hz |
| Filter weighting | linear | S2 `weighting=linear` | High (lineage) / Unknown (Plex) | B | H0 = linear |
| Filter normalization | unit-triangle (area) | S2 `normalize=unit_tri` | High (lineage) / Unknown (Plex) | B | H0 = unit_tri; SI-unit audit at implementation (Essentia divides by theoretical triangle area) |
| Log compression | `log10(10000·x + 1)` | **Identical in S9 training code and S2 inference** — the single strongest train/inference consistency point | High (lineage) | B | Adopt; the one frontend value with two-sided evidence |
| Log floor / epsilon | none (`+1` shift makes floor `log10(1) = 0`) | S2/S9 formulas | High (lineage) / Unknown (Plex) | B | Non-negativity falls out of the formula; no separate floor |
| dB conversion | none | No source applies dB anywhere in this pipeline | High (lineage) | B | Do not add dB scaling |

### 3.5 Model input

| Parameter | Candidate value(s) | Evidence | Confidence | Plex? | Experimental implication |
| --------- | ------------------ | -------- | ---------- | ----- | ------------------------ |
| Tensor rank / layout | unknown for Plex; `[187, 96]` (S5 graph) / `[batch, 1, 187, 96]` (S3 feed) for lineage | S1 explicitly unknown; S3/S5 disagree in rank presentation (frozen-graph vs streaming feed) | Unknown (Plex) | — | **Our design decision**, not a fact to discover; see §11 |
| Time dimension | 187 frames ≈ 2.992 s @16 kHz/256 | S3 `patchSize`; S5 input shape; S6 "3 second inputs" | High (lineage) / Unknown (Plex) | B | H0 patch = 187×96 |
| Frequency dimension | 96 | Same as mel bands | High (lineage) / Unknown (Plex) | B | H0 = 96 |
| Channel dimension | 1 (lineage feed) / absent (graph) | S3 `inputShape {batch,1,patch,bands}` vs S5 `[187,96]` | Low even for lineage | B | Our choice at implementation |
| dtype | float32 assumed; unconfirmed for Plex | S5 `type: float`; S1 silent; 3.1 MB ≈ f32 params argues against quantization (§7) | Medium (lineage) / Unknown (Plex) | B | H0 = f32 |
| Value range | `[0, ~4+]` log-compressed mels (non-negative by construction) | Follows from §3.4 formula | High (derivation) | B | Useful as a sanity assertion, not a gate |
| Patch duration | ~3 s | Arithmetic + S6 | High (lineage) / Unknown (Plex) | B | H0 |
| Patch overlap (stride) | 93 frames (1.488 s) default; 0 and 187 also exposable | S3 `patchHopSize` default 93 | Medium (lineage default only) | B | H0 = 93; stride is a first-class ablation axis (cf. ADR 0017's hop-sensitivity lesson) |

### 3.6 Temporal handling

| Parameter | Candidate value(s) | Evidence | Confidence | Plex? | Experimental implication |
| --------- | ------------------ | -------- | ---------- | ----- | ------------------------ |
| Fixed-length patch | yes, 187 frames | S3/S5/S6 | High (lineage) / Unknown (Plex) | B | H0 |
| Sliding windows | yes, `patchHopSize` apart | S3 | Medium | B | H0 = 93 |
| Last partial patch | discarded (inference) / repeat-padded (training) | S3 vs S7 — an explicit train/inference mismatch inside the lineage itself | Medium | B | Adopt discard for inference; record the mismatch |
| Zero padding | frame-level zeros (Essentia) vs reflect (librosa training) | S4 vs S8 defaults | Low–Medium | B | Ablation candidate; boundary frames only |
| Aggregation across patches | arithmetic mean of per-patch outputs | S9 validation (`np.mean` per id) | Medium (validation-time; inference aggregation in the Essentia demo path is caller-side) | B | H0 = mean; alternatives (median, attention) are future experiments, not v1 |
| Track → album | arithmetic mean of track embeddings | S1, verified to 0–2% on one album | Medium (single-author, single-seed) | **A** | Future "Plex-like" comparison only; never production |

### 3.7 Model architecture

| Claim | Verdict | Evidence | Confidence | Plex? |
| --- | --- | --- | --- | --- |
| Conv2D front-end, several layers | Consistent: 5 musically-motivated Conv2Ds in model 11 (timbral 7×67, 7×38; temporal 128×1, 64×1, 32×1; ReLU+BN, frequency max-pool) | S10 | High (lineage) / Medium (reported) | B+A |
| Flatten placement (before dense-200) | Consistent: `flatten` after temporal pooling in S10; S1 reports flatten before dense | Medium | B+A | — |
| Dense(200, ReLU) | Confirmed in lineage (`num_units_backend=200`, ReLU); reported for Plex with matching `dense` layer-name idiom | High (lineage) / Medium (reported) | B+A |
| Final Dense(52, sigmoid) | **52 is Plex-specific and non-stock** (stock is 50, S5/S7); sigmoid terminal confirmed in lineage (S9 `tf.nn.sigmoid`); ordering dense→sigmoid confirmed | High (difference) / Medium (reported width) | **A** (width) |
| Biases | Present by framework default (`tf.compat.v1.layers.dense(use_bias=True)`); no Plex-specific evidence | Medium (lineage) / Unknown (Plex) | B |
| Extra layers (BN/dropout folding) | Training graph has BN+dropout around both dense layers; inference/TFLite folding unknown; dropout inactive at inference by construction | Low | B (+unknown) |
| Persisted vector = direct model output | 208-byte slots ⇒ 52 post-sigmoid f32s stored; no evidence of post-processing between inference and storage | Medium–High (format) / Medium (identity) | **A** |
| 208 bytes ⟺ 52×f32 | Arithmetic + `xxd` stride verification in S1 | High | **A** |

Architecture is **not** inferred from "typical MusiCNN" anywhere above:
every structural row cites S1's report or S2–S10 primary sources.

## 4. Plex-specific findings (Category A)

1. **`.tree` slot format — High.** 4-byte occupancy header + 52
   little-endian float32s (212-byte stride, `xxd`-verified in S1).
   This independently confirms a 52-wide float32 output regardless of
   what the model section claims.
2. **`Music.tflite` identity — Medium–High.** Filename, resource path,
   ~3.1 MB size, TFLite flatbuffer container, XNNPack delegate use.
   File size is consistent (not probative alone) with a ~787k-param
   float32 MusiCNN-style model (§7).
3. **Reported head `dense(200) → dense_1(52, sigmoid)` — Medium.** The
   Keras-style `dense`/`dense_1` naming matches S10's code idiom, which
   is genuinely suggestive of shared code origin — but S1 never states
   its inspection method, so this stays reported, not verified.
4. **`/nearest` API surface — Medium–High.** Route format string,
   `limit=30&maxDistance=0.25`, `distance` attribute with full
   float64-ish precision (`0.075230911374092102`).
5. **Normalized-Euclidean-over-means behaviour — Medium.** Formula
   `‖a−b‖ / mean(‖a‖,‖b‖)` reproduced one album's API ranking with
   0–2% absolute agreement. Single author, single seed album, four
   neighbours: real verification, narrow scope, no replication.
6. **Album.tree ≠ track-mean (Δ≈0.059) — Medium, unexplained.** Either
   album vectors come from a separate inference pass (whole-album
   audio?) or a different aggregation. Open question; it warns against
   assuming "mean everywhere".
7. **Embedding statistics (mean ≈0.03, ~25/52 near-zero, norms
   0.33–1.05) — Low–Medium.** Consistent with sparse sigmoid outputs
   over diverse music; descriptive of one library, not a specification.
8. **Artist.tree exists — Low.** Three-level (artist/album/track)
   persistence noted; artist aggregation unknown.

## 5. MusiCNN-lineage findings (Category B)

1. The inference frontend (S2) and the training frontend (S7+S8) agree
   on every numeric value: 16 kHz / 512 / 256 / 96 / Slaney /
   0–8000 Hz / power / `log10(10000·x+1)` / 187-frame patches — with
   S2's comments explicitly citing S7's config as the target.
2. Boundary behaviour is where the lineage disagrees with itself:
   training pads short clips by **repetition** (S7) while inference
   **discards** partial patches (S3); training frames are
   reflect-padded + centered (librosa defaults, S8) while Essentia
   inference is centered but zero-padded with a noise-injected silence
   policy by default (S4). Any "exact lineage replica" claim must pick
   a side on each of these.
3. Validation-time aggregation is the **mean** of per-patch outputs
   (S9) — the best-supported hypothesis for track-level aggregation,
   and notably the same operator S1 reports at album level.
4. The stock head is 200-ReLU → **50**-linear + external sigmoid (S5,
   S9, S10). A 52-wide variant is not a stock artifact; producing one
   requires retraining with `num_classes_dataset=52` (one-line change
   in S7's terms) or equivalent. This is the structural-consistency
   answer to §5.4: yes, plausibly a modified/retrained MusiCNN-style
   model; no, not a stock model.
5. Size check: model-11 ≈ 787k params × 4 B ≈ 3.15 MB ≈ S1's 3.1 MB.
   Consistent with float32 weights (a quantized model would be ~0.8
   MB). Weak supporting evidence; any ~790k-param float model matches
   equally well.
6. Training stored features as **float16** (S8) yet trained usable
   models — a mild prior that f16-scale frontend noise is tolerable,
   relevant only as background to our own precision policy, not as a
   decision.
7. **No public evidence connects Plex's input tensor to `[187, 96]`.**
   S1 says unknown; S5/S3 describe only public models. The shapes are
   compatible, not established. This is the central non-conclusion of
   the phase and must survive into the uncertainty matrix.

## 6. `AudioSpectrogram` investigation

- The exact string is a genuine TensorFlow op name: `AudioSpectrogram`
  (`windowSize`, `stride`, `magnitudeSquared` → magnitude spectrogram
  as a "3D representation"; S12). A same-named Wolfram encoder also
  exists (irrelevant — different ecosystem, no Plex connection).
- S1 glosses it as "Input type" with no stated method: no surrounding
  disassembly, no call-graph, no indication whether the string came
  from the model flatbuffer, the TF runtime, or Plex's own code.
- What it **cannot** establish: sample rate, framing, mel extraction,
  or any preprocessing parameter — the op produces a plain magnitude
  spectrogram and carries no mel, log, or normalization semantics.
- Candidate readings, all unconfirmed: (a) a TF-graph op inside the
  conversion lineage (YAMNet-style in-graph STFT, cf. S14); (b) a
  Plex-internal preprocessing class name that merely echoes TF
  vocabulary; (c) a linked-runtime symbol with no behavioural meaning.
  Reading (a) would, if confirmed, move spectrogram computation
  *inside* the model and change the input-tensor question entirely —
  which is exactly why the string must not be overinterpreted.
- Revision caution: S1r shows string evidence mutating between
  revisions (`kTfLiteCustom` dropped, `kTfLiteActRelu` added) without
  stated reason. `kTfLiteActRelu` is a TFLite schema enum that can
  arrive via linked runtime code, so it does not specifically prove
  ReLU in this model either.
- Resolving test (out of bounds for this phase — it needs the
  proprietary artifact, so it stays a documented non-experiment):
  `strings` + Netron op-listing of a legitimately held copy would show
  whether any spectrogram op is in-graph. **Not to be performed.**

## 7. 52-dimensional head investigation

- **Dense(200): confirmed** (lineage S5/S10; reported S1). The
  `dense`/`dense_1` naming idiom match is the strongest
  code-origin hint in the whole dossier.
- **52-wide final layer: Plex-specific, non-stock.** Every public
  musicnn artifact in S5–S7 is 50-wide. 52 appears only in S1.
- **Sigmoid: confirmed** as the terminal activation idiom (S9 applies
  `tf.nn.sigmoid` to linear logits; S5 schema exposes the Sigmoid node
  as the predictions output; S1 reports sigmoid with [0,1] values
  consistent with the stored bytes).
- **Ordering (dense → sigmoid): confirmed** in lineage; reported for
  Plex. No evidence of anything after the sigmoid.
- **Biases: assumed present**, framework-default in S10; no
  Plex-specific evidence for or against.
- **Additional layers:** training graphs wrap both dense layers in
  BN+dropout. Whether BN was folded at TFLite conversion is unknown;
  dropout is definitionally inactive at inference. Do not assume the
  deployed graph equals the training graph minus dropout.
- **Output ⟺ persisted vector:** the 208-byte slot content is
  consistent with storing the raw 52-sigmoid output (no room for
  anything else, no evidence of post-processing). Album.tree's
  divergence from track-means (§4.6) is the one caveat against
  "what-you-infer-is-what-you-store" as a universal rule.
- **Format vs architecture, separated:** 52×f32 storage is
  format evidence (High); dense-200→dense-52-sigmoid computation is
  architecture evidence (Medium, single-source, method unstated).

## 8. Similarity metric findings (not preprocessing — recorded separately)

- Reported behaviour: album distance = `‖mean(tracks_A) −
  mean(tracks_B)‖ / mean(‖mean(tracks_A)‖, ‖mean(tracks_B)‖)`,
  computed live from Track.tree for `/nearest` (S1+S1r).
- Confidence: **Medium** — one worked verification (TPAB + 4
  neighbours, rankings exact, absolutes within 0–2%), no independent
  replication, API internals unobserved.
- Hedgehog's "Plex uses Annoy" (S11) is unevidenced and carries no
  weight; note it also conflicts in spirit with "computed dynamically
  per query" unless the index is over precomputed means — unresolved,
  and Plex's problem, not ours.
- Cosine is reported as non-discriminative for these vectors
  (0.997+ clustering, first-orthant geometry) — plausible on its face
  and consistent with the stored statistics, but likewise
  single-source.
- Implications for Sonic52: a future "Plex-like" benchmark arm may use
  this formula on mean embeddings for comparison purposes. It must
  **not** enter MusicPack production ranking, which stays cosine over
  L2-normalized vectors per ADR 0017. Sonic52 remains decoupled from
  the production ranking architecture.

## 9. Contradictory / disconfirming evidence

Searched for specifically; each entry weakens the naive hypothesis
("Plex = stock Essentia/MusiCNN `[187, 96]` frontend + stock model"):

1. **50 ≠ 52** — stock head width contradicts the reported Plex head.
   A stock-model hypothesis is rejected; only a retrained-variant
   hypothesis survives.
2. **Persisted layer differs** — lineage documents the *pre-sigmoid
   200-vector* as "embeddings" (S5 `output_purpose`); Plex persists
   the *post-sigmoid 52-vector*. Different representation choice, not
   a drop-in lineage behaviour.
3. **Album.tree ≠ mean(Track.tree)** (Δ≈0.059, S1) — contradicts
   uniform mean-aggregation; album vectors have an unexplained origin.
4. **Train/inference padding mismatch inside the lineage** (repeat-pad
   vs discard, S7 vs S3) — there is no single "lineage behaviour" to
   copy for boundaries; any choice is ours.
5. **Reflect (training) vs zero+noise (Essentia inference) boundaries**
   (S8 vs S4) — same conclusion for framing edges.
6. **Family variance is documented**: VGG variants (3×3 stacks, no
   musically-motivated kernels), `MSD_musicnn_big` (×512 mid-end,
   500-unit head) — "MusiCNN-like" spans multiple incompatible
   front-ends/heads, so resemblance alone identifies nothing.
7. **Input shape explicitly unknown** (S1) — the load-bearing
   parameter has no Plex evidence at all; `[187, 96]` compatibility is
   not confirmation.
8. **No sample-rate evidence** — 16 kHz is lineage-unanimous but
   Plex-unattested; Plex ingests arbitrary-rate local files through an
   unknown resampler.
9. **String-evidence instability** across S1 revisions
   (`kTfLiteCustom` → `kTfLiteActRelu`) cautions against reading any
   single binary string as architectural proof — including
   `AudioSpectrogram` (§6).
10. **Size match is weak**: ~3.1 MB is consistent with *any* ~790k
    float32-param model, retrained or otherwise; it corroborates
    scale, not identity.

## 10. Uncertainty matrix

| Parameter | Current status | Best-supported value | Confidence | Why |
| --------- | -------------- | -------------------- | ---------- | --- |
| Sample rate | Hypothesis (lineage) | 16000 Hz | Medium | Lineage-unanimous (S3/S5/S7/S8); zero Plex evidence |
| Channels | Hypothesis (lineage) | mono | Medium | S3/S8; Plex downmix unknown |
| Frame size | Hypothesis (lineage) | 512 | Medium | S2/S7; Plex unattested |
| Hop | Hypothesis (lineage) | 256 | Medium | S3/S7; Plex unattested |
| FFT | Hypothesis (lineage) | 512-pt real, power | Medium | S2/S4/S8 defaults; Plex unattested |
| Window | Hypothesis (lineage) | symmetric Hann, zero-phase, unnormalized | Medium | S2 + Essentia defaults; zero-phase is default-dependent |
| Mel bands | Hypothesis (lineage) | 96 | Medium | Unanimous lineage; Plex unattested |
| Mel scale | Hypothesis (lineage) | Slaney | Medium | S2/S8 agree; formula version must be pinned later |
| Frequency range | Hypothesis (lineage) | 0–8000 Hz | Medium | S2 explicit; Plex unattested |
| Normalization (filter) | Hypothesis (lineage) | linear weighting, unit-triangle | Medium | S2 explicit |
| Log compression | Strong hypothesis (lineage, two-sided) | `log10(10000·x + 1)`, no dB | High (lineage) | Identical in training code and inference frontend |
| Patch size | Hypothesis (lineage) | 187 frames (~3 s) | Medium | S3/S5/S6; Plex shape unknown |
| Patch stride | Hypothesis (lineage default) | 93 frames (~1.49 s) | Low–Medium | Default only; ablate (ADR 0017 hop lesson) |
| Tensor shape | **Unknown** | — | Unknown | S1 explicitly unknown; S3/S5 describe other models |
| Tensor dtype | Hypothesis | float32 | Medium (lineage); Unknown (Plex) | S5; size argument against quantization |
| Aggregation (track) | Hypothesis (lineage validation) | mean of patch outputs | Medium | S9; inference path is caller-side |
| Aggregation (album) | Reported behaviour | mean of track embeddings | Medium | S1 verification; Album.tree caveat |
| Head (200→52 sigm) | Reported / non-stock | dense-200-ReLU → dense-52-linear → sigmoid | Medium | S1 + naming idiom; 52 is Plex-only |
| Distance metric | Reported behaviour | norm-Euclidean over means | Medium | S1 single-seed verification |
| Resampler | **Unknown** | — | Unknown | Nothing anywhere; our choice |
| Boundary/padding rule | **Undecided (lineage split)** | — | Unknown | Repeat vs discard vs reflect vs zeros; ablate |

Nothing above is a Plex fact except the Category-A items in §4, and
none of those is a preprocessing parameter.

## 11. Recommended initial Sonic52 preprocessing contract

One configuration (H0), labelled per parameter. All values are
implementable over MusicPack's existing audio infrastructure
(`musicpack-core::audio` decode + `rubato`/`microfft`, both already in
the tree for the Discogs-EffNet path — no new dependencies).

| # | Parameter | Recommended H0 | Label | Rationale |
| --- | --- | --- | --- | --- |
| 1 | Sample rate | 16000 Hz mono | Lineage-derived hypothesis | Unanimous lineage; cheapest falsifiable anchor |
| 2 | Downmix | arithmetic f64 mean | Experimental choice | Matches both lineage ends; rule recorded exactly |
| 3 | Resampler | existing `rubato` path, version pinned | Experimental choice | No evidence constrains this; reuse avoids a new dependency |
| 4 | Frame / hop | 512 / 256 | Lineage-derived hypothesis | S2/S3/S7 |
| 5 | Window | symmetric Hann, zero-phase, unnormalized | Lineage-derived hypothesis | S2 + defaults; symmetry/phase asserted in tests at implementation |
| 6 | Spectrum | 512-pt real FFT, power | Lineage-derived hypothesis | S2/S4/S8 |
| 7 | Mel | 96 bands, Slaney, linear, unit-triangle, 0–8000 Hz | Lineage-derived hypothesis | S2; formula version pinned at implementation, not here |
| 8 | Compression | `log10(10000·x + 1)` | Evidence-backed (lineage, two-sided) | Strongest row in the dossier (S2 ≡ S9) |
| 9 | Patch | 187 frames (~3 s), stride 93, discard partial | Lineage-derived hypothesis | S3/S5/S6; stride is ablation axis #1 |
| 10 | Track aggregation | mean of patch outputs | Lineage-derived hypothesis | S9 validation behaviour |
| 11 | Tensor shape/dtype | **unresolved — do not assume** | Unknown | S1 unknown; OUR shape decision belongs to the implementation slice, recommended `[batch, 187, 96]`-equivalent f32 as an explicit experimental choice, never a Plex claim |
| 12 | Comparison metric (benchmark arm only) | normalized Euclidean over means | Reported Plex behaviour | S1; quarantined to a "Plex-like" comparison, never production |

Where evidence was insufficient to select responsibly (tensor shape,
resampler, boundary rule, downmix rule), the recommendation is an
explicit choice plus a named ablation (§12), not a value laundered
into a fact.

## 12. Remaining research questions

### Known with high confidence

- Lineage frontend values (§3.1–§3.5, Medium–High as *lineage* facts).
- `log10(10000·x+1)` identically in training and inference code.
- Stock MusiCNN head is 200→50+sigmoid; 52 is non-stock.
- `.tree` slots hold 52 f32s (format fact, independent of all model claims).

### Strongly suggested but not Plex-proven

- Plex's model is a retrained MusiCNN-style variant (naming idiom +
  size + reported head shape), not stock `msd-musicnn-1`.
- Weights are float32 (size argument).
- Album-level aggregation is mean-of-tracks for the `/nearest` path.

### Unknown

- Every preprocessing parameter as a Plex fact (§10: all "Unknown (Plex)").
- Input tensor shape/dtype/rank; in-graph vs out-of-graph spectrogram (§6).
- Track-level aggregation inside Plex; Album.tree/Artist.tree provenance.
- Resampler, boundary rules, amplitude scaling, clipping.

### Experiments needed to resolve the unknowns

1. **Stride/aggregation ablation** on the future trained model
   (93 vs 0/187 stride; mean vs median): resolves #9–#10 without any
   Plex data, using our own benchmark.
2. **Boundary-rule ablation** (discard vs repeat-pad vs reflect):
   small, self-contained, uses our corpus.
3. **Frontend equivalence harness**: pin Slaney/unit-tri/Hann-zero-phase
   numerics against published formula values at implementation time
   (hand-computed goldens, no Essentia run needed).
4. **Behavioural comparison against real Plex vectors**: requires
   legitimately held Plex artifacts + a Plex library — possible future
   work, explicitly out of scope until a clean provenance path exists.
   Not to be improvised.

### What we should implement next

Smallest subsequent research slice (proposed, not started): **Sonic52
Slice 1 — frozen log-mel frontend producing `[187, 96]` f32 patches
over `musicpack-core::audio`, no model, no weights.** Acceptance:
deterministic bytes; chunk-size invariance; 187×256/16000 ≈ 3 s patch
geometry asserted; Slaney/unit-tri/Hann-zero-phase each pinned by a
hand-computed golden; partial-patch discard; every deviation from §11
recorded as an experimental choice. Tensor shape decision (#11) lands
in that slice's contract, not here.

## 13. Report metadata

- Scope: forensic research and documentation only. No source file was
  modified (scaffold, production, and experiment code all untouched);
  no dependencies added; no training, weights, inference, downloads,
  or Plex artifacts involved.
- Existing ADRs (0016, 0017, 0018) are not modified by this report; any
  future contract change belongs to a new slice, not an ADR rewrite.
- Classification discipline: every conclusion above carries its
  A/B/C category inline; §10 downgrades anything Plex-unattested to
  hypothesis or unknown. No uncertainty was converted into precision.
