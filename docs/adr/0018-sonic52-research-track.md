# ADR 0018: Sonic52 independent similarity-model research track

- **Status:** Proposed (2026-10-02; research scaffold only)
- **Decision type:** Research boundary and experiment contract
- **Production impact:** None. No dependency, profile, format, schema,
  API, UI, or runtime behaviour is changed by this ADR or by the
  `experiments/sonic52-research/` scaffold it records.
- **Related decisions:** ADR 0016 (audio analysis replacement study),
  ADR 0017 (similarity profile mechanism and package representation)

## 1. Why Sonic52 research exists

MusicPack's production similarity baseline is Discogs-EffNet (1280-D
multi, 512-D release), implemented behind the default-off
`discogs-effnet` feature and indexed through the model-neutral profile
mechanism. Sonic52 opens a second, alternative research direction
alongside that baseline: a compact 52-dimensional sigmoid embedding
whose *output shape* is motivated by publicly reported structural
characteristics of Plex's historical Sonic Analysis model. The research
question for a future slice is whether an independently implemented and
trained 52-D model can rival the Discogs-EffNet baseline on the
MusicPack similarity benchmark — not whether a historical proprietary
model can be reproduced.

## 2. Independent implementation, not a reimplementation

Sonic52 is an independent research model inspired by publicly
reported/reverse-engineered structural characteristics of Plex's
historical Sonic Analysis model. It must never be described as "the Plex
model", "Plex Sonic Analysis", or a Plex reimplementation. No Plex
model weights are used, downloaded, vendored, or redistributed; no
proprietary Plex artifacts are reproduced. The scaffold at
`experiments/sonic52-research/` contains no weights, no training code,
no inference code, and no audio decoding.

## 3. Publicly reported structural characteristics

Recorded as reported, with provenance notes in the scaffold's
`PREPROCESSING_TABLE`:

- Network head `Conv2D → Flatten → Dense(200) → Dense(52)`. The
  `Dense(200)`-embedding-then-sigmoid structure is corroborated by
  public Essentia MusiCNN-family model material (published model
  metadata schemas and graph descriptions); the exact 52-wide output is
  the reported characteristic, not an independently verified
  measurement.
- 52-dimensional sigmoid embedding (values in `[0, 1]`).
- Lineage context (not copied as design facts): public MusiCNN-family
  material documents 16 kHz audio input, a `[frames, 96]`
  mel-spectrogram signature, and community reports describe the
  historical Plex analysis as using that model family with an
  Annoy cosine/angular index.

## 4. Preprocessing and model details that remain unknown

Explicitly classified as unknown in the scaffold contract, and they
must not be assumed: sample-rate handling beyond the partially
constrained lineage value, mono/stereo policy, FFT size, hop size,
window function, mel-bin count and frequency range, logarithmic
scaling, normalization, temporal window length, input tensor shape,
temporal aggregation, and similarity metric. Partially constrained
items (sample rate, mono/stereo, mel bins, metric) are bounded by
public lineage material but unverified for the reported artifact. The
eventual experimental values will be labelled as MusicPack's choices,
not Plex facts; this phase makes none.

## 5. Independent training, future slice

Any future Sonic52 model will be independently implemented and
trained. This phase creates no training dataset, downloads no music
dataset, trains no weights, generates no placeholder weights, runs no
quality benchmark, and claims no musical-similarity quality.

## 6. Research-only phase boundary

The following are prohibited without a new ADR: modifying or replacing
Discogs-EffNet or its profile IDs; changing `.msim` or `.mpak`
formats; changing production similarity ranking; adding Sonic52 to any
default or production profile set; changing server similarity
behaviour, indexing, playlists/radio, or the web/player implementation;
adding model weights; downloading or vendoring the Plex `Music.tflite`
model; reproducing or redistributing proprietary Plex artifacts;
introducing FFmpeg. The scaffold reuses no production abstraction at
runtime (zero dependencies); its one reuse relationship is
documentary: a future 52-D `f32le` vector fits the frozen `.msim`
container unchanged, so no format change is proposed.

## 7. Evidence required before evaluation against Discogs-EffNet

An independently implemented model; a frozen preprocessing contract
with every current unknown resolved as a labelled experimental choice;
byte-deterministic inference; and a disclosed training-data and
licensing posture. The intended comparison is: test corpus through
Discogs-EffNet, Sonic52-52, and Sonic52-N variants (ladder 32 / 52 /
64 / 96 / 128, 52 first), then embeddings to nearest neighbours to
quantitative metrics to human evaluation to the MusicPack similarity
benchmark.

## 8. Requirements before any future production consideration

All of §7, plus the ADR 0017 licensing-gate pattern resolved for the
new profile, measured benchmark results, and an explicit activation
decision. Human listening validation is product-quality feedback, not
an acceptance gate (ADR 0017 §10.5); no quality claim is made here.

## 9. What this ADR does not change

It does not rewrite ADR 0016 or ADR 0017, add a production profile,
revive Sonic, convert any vectors, or promise that Sonic52 will ever
become a production profile.
