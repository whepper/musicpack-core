# ADR 0016: Audio analysis replacement study and activation boundary

- **Status:** Proposed (2026-09-23; study only)
- **Decision type:** Architecture boundary and future activation gate
- **Production impact:** None from this document. No dependency, API, schema, UI, or runtime behavior is changed by this ADR.
- **Related decisions:** ADR 0008 (offline model), ADR 0009 (library jobs), ADR 0012 (Author runtime cutover), ADR 0014 (server production cutover)

## 1. Decision summary

1. **Sonic remains retired.** The default Rust Author runtime does not generate,
   semantically validate, download, index, serve, or successfully display Sonic
   analysis. The historical C analyzer and model cache remain
   development/oracle material only. This ADR does not revive them.
2. **The current product does not need a new content-analysis capability.**
   MusicPack already has the analysis it consumes: native WAV/FLAC/Musepack
   decoding, streaming BS.1770-5 loudness/true-peak measurement, and canonical
   100 ms `peak-rms-u8` waveform envelopes. Metadata search is not audio analysis.
   There is no current server, web, or Author workflow that consumes similarity
   vectors, fingerprints, tempo, key, or semantic labels.
3. **If a content-analysis product decision is later accepted, the first slice
   should be narrow:** an explicit, optional, collection-local *acoustic-neighbor*
   capability based on a compact, deterministic track descriptor and an
   album aggregate. “Similar” in this slice means a documented distance between
   descriptors; it does not promise mood, genre, style, semantic, or human
   similarity. It is a new task-specific profile, not a claim of drop-in
   compatibility with historical Sonic v1 embeddings. A fingerprint for
   duplicate/near-duplicate detection is a separate capability and is deferred
   until that workflow is requested.
4. **Do not put the new derived index in `.mpack` by default.** Keep the existing
   manifest `analysis[]` references structurally validated, hash-protected, and
   semantically opaque. Store future descriptors in an author-local cache and,
   if activated, a server-local versioned index. A portable analysis export can
   be designed later; it must not be an accidental side effect of analysis.
5. **The ownership boundary is:** `musicpack-core::audio` owns a typed,
   streaming, storage-independent analysis primitive; `musicpack-author` owns
   source selection, explicit invocation, progress, cache, and export policy;
   `musicpack-server` owns persistence, indexing, and queries; the web app
   consumes versioned results and does not analyze audio.
6. **No production dependency is selected by this study.** The preferred v1
   implementation basis is small, pure-Rust DSP over the existing PCM seam.
   FFT, resampling, and numerical policy remain an explicit benchmark/audit
   gate. Learned embeddings and general ML runtimes are not part of the first
   replacement slice.

This is deliberately an activation boundary, not a promise to ship a Sonic
replacement immediately.

## 2. Terminology

The word “analysis” is overloaded in the repository. This ADR uses the
following terms consistently:

- **Measurement:** an existing scalar or envelope product such as loudness or a
  waveform. Measurements are part of the current package/product contract.
- **Descriptor:** a fixed-length, derived vector or small set of statistics used
  for a declared comparison task.
- **Fingerprint:** an integer/hash-like identity designed to recognize the same
  recording across re-encoding or modest changes. A fingerprint is not a
  similarity embedding.
- **Analysis document:** a referenced package asset. The current manifest
  `analysis[]` entry is an opaque reference to such a document; it is not a
  promise that the core understands the document's semantics.

Keeping these terms separate prevents a future descriptor from being confused
with loudness, waveform data, or a package integrity reference.

## 3. Current product evidence

| Area | Evidence in the Rust repository | Consequence |
| --- | --- | --- |
| Decode seam | `src/audio/mod.rs`: `AudioDecoder::read_f32`, `audio::open`, WAV/FLAC/Musepack SV8 support | A future analyzer can consume the existing bounded PCM seam; it does not need FFmpeg, a new decoder, or a full-file buffer. |
| Loudness | `src/audio/loudness.rs`: streaming `LoudnessMeter`, integrated LUFS and true peak, repeatable chunk processing | Loudness is current product functionality, not a Sonic replacement. It remains separate from any descriptor. |
| Waveform | `src/audio/waveform_acc.rs`: constant-memory 100 ms peak/RMS accumulator and bounded payload | Waveform display/seek data is current product functionality. Do not put it in a generic analysis vector. |
| Hostile input | `src/audio/mod.rs`: analysis feeds sanitize NaN/infinities and do not trust untrusted lengths | A new stream must preserve total, bounded, deterministic behavior. |
| Manifest | `src/format/manifest/mod.rs`: `Analysis { kind, profile, asset }`; unknown kinds are forward-compatible | `analysis[]` is a generic reference contract in the current Rust implementation. |
| Verification | `src/validation/mod.rs`: analysis paths receive normal containment, budget, and SHA-256 checks; Sonic semantics are deferred | Do not claim that a Sonic document is semantically validated by the Rust verifier today. |
| Author | `author/src-tauri/src/lib.rs`: the Rust backend returns `sonic_retired`; `crates/musicpack-author/src/pipeline.rs` stages analysis bytes opaquely | Existing analysis references survive inspect/rebuild, but Author does not produce new Sonic data. |
| Server | `crates/musicpack-server/src/jobs.rs`: only `Scan` and `Verify`; `store/sqlite.rs` deliberately does not index analysis documents | There is no current analysis database, route, or job. |
| Web | `web/app/src/lib/offline/plan.ts` defers analysis; the analysis UI shows loudness and waveform | There is no client contract for embeddings or similarity. |

The current `.mpack` builder already measures loudness from primary audio and
can generate waveforms through the native decoder. Those are useful precedents
for streaming, cancellation, hostile-input handling, and deterministic output;
they are not evidence that a content-similarity feature is already needed.

## 4. Historical Sonic boundary and lessons

The sibling `musicpack` repository is an immutable historical reference. Its
Sonic work remains useful as compatibility and research evidence, but it is not
a runtime specification for this Rust product.

### 4.1 What the historical implementation did

The legacy pipeline was:

```text
decoded PCM
  -> mono conversion
  -> 48 kHz resampling
  -> mel frontend
  -> OpenL3 ONNX model
  -> mean-norm window pooling
  -> track vectors
  -> equal-track album aggregation
  -> analysis/sonic.json
  -> manifest analysis[] reference
```

The historical container was deliberately model-independent: a document carried
a profile id, dimensions, encoding, distance, provenance, track/album vectors,
and explicit nulls for tracks without an embedding. The important compatibility
lessons are:

- vectors from different profiles must never be compared;
- a profile-defining algorithm, preprocessing, parameter, runtime, or weights
  change requires a new profile identity;
- a package-provided profile must never cause a model download or model
  execution;
- null/insufficient results must not become fabricated zero vectors;
- album aggregation is a deterministic function of contributing track results;
- relational results such as “similar albums” are collection/index concerns,
  not package correctness.

The historical report measured OpenL3 at approximately `0.055x` realtime and
about `1.9 GB` peak RSS in the TensorFlow research stack. The pinned post-
frontend ONNX artifact was about 18.7 MB. A 10-track album's 512-dimensional
base64-f32le document was about 27.8 kB. The research found Discogs-EffNet
materially better for similarity, but its non-commercial terms made it
unsuitable as a mandatory MusicPack dependency. CLAP was faster/lighter in
that experiment but performed worse; its historical checkpoint was about
776 MB and the research runtime was about 1.5 GB. These results explain the old
burden; they are not a benchmark of a new Rust implementation.

### 4.2 Historical quality evidence (not a current product gate)

The committed research report evaluated a local 200-track subset (70 albums,
49 artists). The following figures are useful for understanding why the old
profile was not frozen as a universal answer:

| Retrieval result | Discogs-EffNet | OpenL3 | CLAP (staged 100-track subset) |
| --- | ---: | ---: | ---: |
| Same album @ 10 | 0.183 | 0.128 | 0.093 |
| Same artist @ 10 | 0.252 | 0.170 | 0.136 |
| Genre purity @ 10 | 0.523 | 0.390 | 0.270 |

The 16-seed blind review preferred Discogs-EffNet, considered OpenL3 acceptable
but weaker, and did not prefer CLAP. The report also measured a 20-track
cross-codec check with mean cosine around `0.9998` for FLAC versus Musepack Q6,
but that is a property of the historical OpenL3 profile and corpus, not a
promise for a new descriptor. These are local-corpus research results, not
current CI gates or evidence that a Rust runtime is viable.

The historical profile dispositions remain:

| Profile | Code/weights terms | Historical role | Current disposition |
| --- | --- | --- | --- |
| `musicpack-sonic-openl3-v1` | OpenL3 code MIT; weights CC BY 4.0 | Permissive default, weaker quality | Historical compatibility reference only; not a new default |
| `musicpack-sonic-discogs-v1` | Essentia AGPL; MTG weights CC BY-NC-SA | Quality/reference comparator | Research-only; never mandatory or auto-downloaded |
| `musicpack-sonic-clap-v1` | Checkpoint Apache-2.0 | Evaluated alternative | Rejected on measured quality; provenance only |
| Future similarity model | Must pass a new code/weights/dataset review | Watchlist | New profile and new benchmark required |

### 4.3 What this ADR does with that history

The historical `musicpack-sonic-v1` document remains a historical compatibility
artifact. The current Rust package layer does not parse it semantically, and
this study does not add that parser. In particular, a package containing
`analysis[]` can currently be verified by generic asset checks without proving
that the document is a valid Sonic document. That is an intentional current
boundary, not an invitation to add a second analyzer format by accident.

The old C+ONNX analyzer, model downloader, `sonic_analyze` UI, and sidecar are
not candidates for a quiet revival. If a future learned model is considered, it
requires a new product/licensing/runtime decision and cannot inherit the old
Author UI or model cache policy implicitly.

### 4.4 Legacy Sonic semantic validation is a separate lane

If a future product decision requires reading and validating historical Sonic
v1 documents, that work must be explicit and separate from the new descriptor
profile. It would be a compatibility module (for example, a future
`musicpack-core::sonic` domain) with strict parsing, profile-state handling,
base64/f32 limits, finite-value and normalization checks, track/album
structural checks, and manifest/document profile equality. Known malformed
documents would fail verification; unknown profiles would remain readable but
unsupported for comparison. A C parser could be ported only with its BSD
attribution and without moving source-derived code into the LGPL encoder/tools
boundary.

That parser would validate bytes; it would not run a model, download weights,
or make Sonic part of the default Author/server/web product. If a legacy Sonic
runtime is ever evaluated, the historical compatibility gates (track cosine
`>= 0.9999`, mean absolute difference `<= 1e-4`, maximum absolute difference
`<= 2e-3`) are useful starting points, not permission to widen a failed result.
It is justified only when a concrete consumer needs historical document
interoperability. Existing package tests intentionally use hash-correct but
semantically invalid Sonic bytes to prove the current generic boundary; changing
that behavior would be an intentional compatibility change with updated tests,
not a side effect of adding a new descriptor API. This ADR preserves the
current generic/opaque behavior and does not silently close that validation gap
as part of a replacement study.

## 5. Does MusicPack need content analysis today?

**No demonstrated current requirement exists.** The current product can author,
verify, ingest, serve, play, and display releases without a new derived content
index. Adding one would create cost in several dimensions:

- a new profile and numerical compatibility contract;
- a cache/index lifecycle and migration path;
- a server or Author invocation surface;
- a quality claim that needs a representative evaluation corpus;
- privacy and deletion questions for derived audio identifiers;
- additional WASM/binary-size and performance review.

That does not mean the capability is forbidden. It means activation must follow
a product decision rather than a library inventory.

### 5.1 Minimum useful conditional capability

The smallest useful *new* capability, if a user-facing requirement is approved,
is:

> An explicit local “find acoustically related tracks/albums” view backed by one
> compact, deterministic descriptor profile and a collection-local nearest-
> neighbor index.

The first profile should be model-free and interpretable enough to diagnose. A
reasonable candidate is a fixed set of pooled log-mel/spectral statistics,
optionally with chroma/tonal features, plus a small number of energy/dynamics
statistics. The exact bands, dimensions, window, hop, target rate, and pooling
must be frozen only after a corpus evaluation; this ADR does not pretend that a
particular vector has useful product quality.

The capability must be described honestly:

- it can support nearest neighbors and collection exploration;
- it must not be marketed as mood, genre, lyric, semantic, copyright, or
  “AI understanding” classification;
- it must expose its profile id and distance semantics to API/UI consumers;
- a poor or ambiguous result is a reason to disable the feature, not a reason
  to silently fall back to a different model.

### 5.2 Capabilities deliberately not in the first slice

- learned embeddings and model downloads;
- tempo/BPM, key, mode, beat tracking, or transition planning;
- mood, genre, style, instrumentation, or lyric-aware labels;
- audio classification and “AIDJ” recommendations;
- exact/near-duplicate fingerprinting;
- artist embeddings and cross-collection federation;
- automatic conversion between descriptor profiles;
- a vector database or ANN service;
- a new general-purpose MIR platform.

Each of these can be valuable, but each needs a separate user story, quality
target, data/license review, and operating model.

## 6. Capability decision matrix

“Needed now?” means supported by a current product workflow, not merely useful
in the abstract.

| Capability | User value | Needed now? | Determinism/versioning | Default storage | Implementation disposition |
| --- | --- | --- | --- | --- | --- |
| Integrated loudness/true peak | Consistent playback normalization and package facts | **Yes; already implemented** | Existing BS.1770-5 contract with documented numeric tolerance | Manifest scalars; computed at authoring time | Keep; do not fold into a new analysis profile |
| 100 ms peak/RMS waveform | Seek UI, overview, transition data | **Yes; already implemented** | Byte-exact canonical payload, independent of decoder chunk size | Per-track `waveform` reference in package | Keep; do not store a copy in a descriptor index |
| Spectral/acoustic descriptor | Local track/album neighbors and exploration | **No; conditional first slice** | Stable within an immutable profile; cross-profile vectors are incomparable | Author cache and server-local index; optional export later | Candidate after product/quality gate |
| Audio fingerprint | Same-recording and duplicate detection | No | Integer/hash output can be exact; algorithm and resampler are profile identity | Local fingerprint index | Separate future capability; evaluate `audiofp`/Chromaprint alternatives only when requested |
| Tempo/BPM | Browsing and transitions | No | BPM is algorithm- and octave-ambiguous; profile must pin estimator and policy | Sidecar/index | Defer; no aubio/FFmpeg dependency |
| Key/mode | Discovery and harmonic metadata | No | Algorithm variation and confidence semantics need a contract | Sidecar/index | Defer |
| Learned music embedding | Stronger semantic similarity if a model meets quality/licensing gates | No; historical Sonic only | Model graph, weights, runtime, operators, preprocessing, and numeric policy all define identity | Versioned local index; never automatic package import | Not v1; reconsider RTen first under a new ADR |
| Clustering/recommendation | Browsing and related-item UX | No | Derived from one profile; not an analyzer property | Server index/query | Build only after descriptor and API are accepted |
| Semantic labels/mood/genre | Taxonomy and human-facing discovery | No | Requires ontology/model governance and evaluation | Server index | Out of scope |

## 7. Rust ecosystem assessment

The survey below is deliberately a candidate survey, not a dependency proposal.
Versions and licenses change; the implementation slice must re-check the exact
release, transitive dependencies, MSRV, `unsafe`/FFI audit, and WASM build before
changing `Cargo.toml`.

`#![forbid(unsafe_code)]` is applied to MusicPack's own source. Whether a
pure-Rust dependency may contain audited upstream `unsafe` SIMD is a separate
policy decision. This study uses the conservative interpretation for the core:
a candidate with required C/FFI/build-script machinery is rejected, and any
upstream `unsafe` fast path must be explicitly audited, pinned, and kept out of
the canonical/WASM path until accepted. “Pure Rust” therefore does not by itself
mean “no unsafe code anywhere in the dependency graph.”

| Candidate | Relevant facts | Fit under MusicPack constraints | Disposition |
| --- | --- | --- | --- |
| [`rustfft`](https://docs.rs/rustfft/latest/rustfft/) | Pure Rust; MIT OR Apache-2.0; current 6.x line; arbitrary sizes; AVX/SSE/Neon and optional WASM SIMD; the normal planner chooses algorithms based on the machine; SIMD implementations contain upstream `unsafe` intrinsics | Strong DSP candidate and WASM-capable in principle. CPU-dependent algorithm selection means “bit-identical on every CPU” is not a safe promise without a scalar/reference policy and differential tests. `FftPlannerScalar`/disabled fast-path features are the reproducibility path to evaluate | **Conditional performance candidate; not selected yet** |
| [`realfft`](https://github.com/HEnquist/realfft) | MIT convenience wrapper over RustFFT for real transforms; propagates RustFFT SIMD/WASM features | Avoids carrying unused complex-transform machinery for real PCM, but inherits RustFFT's upstream `unsafe`, CPU dispatch, and numeric-policy questions; it is an API convenience, not a new determinism guarantee | **Conditional convenience candidate; not selected yet** |
| [`microfft`](https://docs.rs/microfft/latest/microfft/) | Pure Rust, MIT, `no_std` capable, in-place radix-2 FFT for fixed power-of-two sizes; no C/FFI/build script; no explicit WASM CI claim; latest published release is older than the other candidates | Very small deterministic reference candidate with no dynamic planner/SIMD selection. Fixed sizes and lower ecosystem activity are real limits; its tables/code size also need measurement | **Strongest strict primitive candidate; not selected yet** |
| [`rubato`](https://docs.rs/rubato/latest/rubato/) | Pure Rust; MIT OR Apache-2.0; chunked sinc and FFT resamplers; current 5.x metadata reports MSRV 1.87; SIMD/interpolator paths contain upstream `unsafe` and the project has rapid major-version churn | Resampling is useful, but current MSRV is above this workspace's 1.85, WASM support is not established by the survey, and CPU dispatch/numerical behavior is profile-defining | **Admit only after MSRV/WASM/unsafe/numerical audit or use a smaller local fixed resampler** |
| [`Symphonia`](https://github.com/pdeljanov/Symphonia) | 100% safe/pure Rust decoder framework; MPL-2.0; 0.6.x MSRV 1.85; broad codec support; its README lists a WASM API as planned rather than as a current tested target | Good future codec candidate, but the core already owns WAV/FLAC/Musepack decoding and Symphonia's format list does not replace the native Musepack decoder. MPL, feature, and target review are additional work | **Do not add for this study** |
| [`ebur128`](https://github.com/sdroege/ebur128) / `ebur128-stream` | MIT, established pure-Rust EBU R128 implementation; the newer stream crate advertises no-std/WASM and bounded streaming but is much newer | MusicPack already has a compatibility-oriented hand port with an explicit oracle and tolerance policy. Replacing it would add a dependency without solving a current product need; a future loudness redesign would need a separate numerical decision | **Do not replace the existing meter** |
| `hound` / `ndarray` | Hound is a pure-Rust Apache-2.0 WAV utility; ndarray is MIT/Apache-2.0 with optional native/parallel features | Either duplicates an existing seam or adds a broad dependency for a small fixed descriptor | **Do not add by default** |
| Small `mfcc`/music-feature crates | Narrow, older, lightly documented transforms rather than a maintained end-to-end analysis profile | A hand-written profile over a reviewed FFT primitive is smaller and makes numerical policy explicit; a poorly maintained feature crate would still need a correctness audit | **Do not select by name; implement only the evaluated profile** |
| [`dasp-rs`](https://github.com/dasp-rs/dasp-rs) / [`Spectrageist`](https://github.com/gijzelaerr/spectrageist) | MIT pure-Rust MIR/stream prototypes with spectral, chroma, onset, and tempo features; the surveyed versions either decode/retain whole tracks, lack a decoder, or have very limited adoption/maintenance | Useful algorithm references and possible throwaway prototypes, but neither establishes the bounded, versioned, chunk-invariant contract MusicPack needs | **Research/reference only** |
| [`bliss-audio`](https://github.com/Polochon-street/bliss-rs) | Compact 23-value descriptor covering tempo and spectral/loudness statistics; GPL-3.0 and default FFmpeg/native path, with whole-track analysis | Excellent black-box semantic reference for a future corpus, but its license/provenance and memory/runtime shape do not fit a permissive core crate; do not copy its implementation | **Corpus/oracle reference only** |
| [`audiofp`](https://docs.rs/audiofp/latest/audiofp/) | Pure-Rust Wang, Panako, and Haitsma–Kalker fingerprinting; streaming variants; claims bit-exact offline/streaming parity; MIT repository license; default `no_std + alloc`; current 0.4.x metadata reports MSRV 1.93; optional decoder/inference features | Closest domain fit for a future fingerprint prototype, but it is a very young project (created in 2026), its CI does not establish WASM, its MSRV is above this workspace, and its numerical claims have not been checked against MusicPack's native/WASM matrix. It solves a different problem from similarity | **Best fingerprint prototype candidate; not production-selected** |
| [`rusty-chromaprint`](https://github.com/darksv/rusty-chromaprint) | MIT pure-Rust Chromaprint port; README calls it in progress; depends on `rustfft` and an older `rubato`; intended for identification/duplicate detection | Better semantic fit than a general ML crate for fingerprints, but incomplete presets and non-reference-compatible resampling/behavior are reported; not a similarity engine | **Benchmark only for a new private format if duplicate detection is activated** |
| `chromaprint-next` | Pure-Rust project with strict Chromaprint parity goals, but its resampler is derived from FFmpeg and carries LGPL source-derived terms | It would introduce a licensing/provenance boundary even if the Rust wrapper itself is permissive; it is also a fingerprint format, not a similarity descriptor | **Reject under the current provenance boundary** |
| `chromaprint` C bindings | C API/FFI; compact fingerprints | Violates the no-FFI/no-C production boundary and adds a native runtime | **Reject** |
| [`rten`](https://github.com/robertknight/rten) | End-to-end Rust ONNX runtime; MIT OR Apache-2.0; CPU plus AVX/NEON/WASM SIMD; current 0.26.x metadata reports MSRV 1.94 | The cleanest future pure-Rust/WASM inference candidate in the survey, but it is above the workspace MSRV, uses architecture-specific SIMD/parallel execution, and still has no approved audio model or MusicPack parity result | **Best future model-runtime candidate; not current-core selectable** |
| [`tract-onnx`](https://docs.rs/tract-onnx/latest/tract-onnx/) | MIT OR Apache-2.0 Rust ONNX/TensorFlow inference with CPU/ARM/GPU and browser support, but the repository-relevant native linear-algebra path uses a build script/toolchain and upstream `unsafe` kernels; no MusicPack audio frontend or approved model | Technically capable, but the full dependency/build policy and architecture-specific kernels fail the current core boundary even where a WASM branch exists | **Hard no for the current core; revisit only behind a future policy decision** |
| [`candle-core`](https://docs.rs/candle-core/latest/candle_core/) | MIT/Apache Rust ML framework with CPU/CUDA/Metal and browser/WASM examples; core/backend code contains unsafe pointer/conversion paths and optional native FFI/build-script paths | Broad framework and model runtime for a problem that does not yet have a product requirement; no ready MusicPack audio model; backend rounding is not cross-target exact | **Defer; not a current-core candidate** |
| [`ort`](https://docs.rs/ort/latest/ort/) / `onnxruntime` | MIT/Apache-2.0 Rust facade over ONNX Runtime's C API; current builds exist for several targets including WASM, but `ort-sys` is explicitly unsafe FFI and links/downloads a native runtime | Target availability does not remove the C API/FFI/native-runtime boundary. It would recreate the old deployment burden | **Reject under current architecture** |
| [`aubio`](https://docs.rs/aubio/latest/aubio/) | GPL-3.0 bindings to the C aubio library; onset, tempo, MFCC, pitch, and resampling features | License, C, bindgen/`unsafe`, and optional FFTW/BLAS dependencies are all outside the core boundary | **Reject** |
| Essentia / Rust wrappers | Essentia is C++/AGPL-oriented and its bundled models have non-commercial terms; the `essentia-rs` wrapper links Essentia and native dependencies through a C++ sys crate. The name “equiz” could not be verified as a Rust audio project in the survey | Cannot be a mandatory dependency for this permissively licensed product; C++/runtime, model governance, and the unverified name must be resolved before assessment | **Reject as mandatory; research reference only** |
| [`ff-analysis`](https://docs.rs/ff-analysis/latest/ff_analysis/) | MIT/Apache-2.0 media analysis convenience crate; requires FFmpeg development libraries and filter graphs | Directly violates the no-FFmpeg rule and duplicates core seams | **Reject** |
| `timestretch` / small BPM crates | Pure-Rust examples exist, often specialized (for example, EDM-oriented tempo detection) | No evidence yet of a general, maintained, quality-validated tempo implementation | **Do not make a dependency claim** |
| [`beat-this`](https://github.com/danigb/beat-this-rs) | MIT Rust tempo/beat model using RTen by default; small/full models are roughly 10/83 MB; whole-track API and optional ORT backend | A serious future tempo option, but it exceeds the minimum descriptor budget, requires a separately licensed/hash-pinned model, and is not a bounded streaming contract | **Defer behind a tempo-specific product decision** |

The important conclusion is not “Rust has no audio ML.” It is that the current
replacement does not need a general ML runtime. A small deterministic DSP
profile is easier to test, package, explain, and keep WASM-clean than a learned
model whose quality and license are unresolved. If a learned model is later
approved, RTen is the strongest pure-Rust/WASM candidate in this survey, but its
current MSRV and SIMD policy still require a separate exception or upgrade
decision.

## 8. Proposed future profile and API shape

The following is a logical contract, not an implemented Rust API. Names and
serialization details should be settled in the implementation slice after the
product gate.

### 8.1 Streaming input contract

A future core analyzer should:

1. accept interleaved `f32` frames through a bounded `process(&[f32])` call;
2. obtain stream facts from `AudioInfo` and reject unsupported rates/channels
   with typed errors;
3. sanitize hostile non-finite samples at the analysis boundary, following the
   existing waveform/loudness precedent;
4. be chunk-size invariant: splitting the same decoded stream at different
   points must not change the result;
5. keep memory bounded by a fixed analysis window plus streaming statistics;
6. return an explicit insufficient/unsupported/no-meaningful-result status
   rather than a zero vector;
7. expose no filesystem, network, database, thread, or UI policy.

The core should consume the decoder seam, not accept a path or a `.mpack`
object. That lets Author analyze a source, the server analyze an indexed audio
object, and a future client analyze an already-decoded stream through the same
contract.

### 8.2 Input normalization

A profile must explicitly declare:

- target analysis sample rate and resampler;
- channel downmix rule and channel order;
- window function, FFT size, hop, overlap, and frequency-band definition;
- pooling and normalization;
- silence/short-track policy;
- scalar precision, accumulation order, rounding, and quantization;
- minimum valid duration and resource limits.

The first candidate should analyze a mono stream at one fixed rate. A rate of
16–22.05 kHz may be sufficient for a compact spectral/fingerprint profile, but
the rate must be selected by evaluation rather than assumed from the old
OpenL3 profile. Existing playback resampling is not automatically an analysis
resampler: playback and analysis have different quality and determinism
contracts.

### 8.3 Result model

A result should be typed and profile-scoped. At minimum it needs:

```text
profile_id                 stable semantic name
profile_fingerprint        hash of all profile-defining fields
algorithm_id/version       implementation identity
library_id/version         dependency identity, if used
model_id/sha256            null for model-free profiles
parameters_fingerprint     canonical parameter hash
source_audio_sha256        cache/invalidation identity
source_format              codec, rate, channels, and relevant facts
status                     ok | insufficient_audio | unsupported | failed
descriptor                 optional fixed-length values
contributing_frames        validity/quality accounting
```

The exact wire format is deliberately outside the core. An author cache may
use a compact binary record; a server may use a SQLite BLOB; a future portable
export may use a separately specified document. The core should return typed
values and never make SQLite, JSON, or package paths part of its domain API.

A fingerprint result must be a different type/profile, even if both ultimately
support a nearest-neighbor or duplicate workflow. Mixing a hash and a cosine
vector in one “embedding” field would make invalid comparisons too easy.

### 8.4 Album aggregation

If the first capability is activated, an album result is derived from track
results in canonical manifest order:

- only successful track descriptors contribute;
- track weighting is equal unless the profile explicitly says otherwise;
- the aggregate and contributor count are recorded;
- no aggregate is emitted when no track contributes;
- a changed contributor set invalidates the aggregate.

The aggregate belongs in the local/server index initially. It is not needed for
package correctness and should not be smuggled into the manifest as a side
effect.

## 9. Determinism and versioning policy

### 9.1 Two different guarantees

The product should not promise one overly broad “bit-exact” guarantee for every
numeric result:

- **Measurements and fingerprints:** waveform payloads are already byte-exact;
  future fingerprint hashes should have exact, chunk-invariant output where the
  algorithm permits it; loudness retains its documented numeric tolerance.
- **Similarity descriptors:** exact cross-CPU identity is not required for
  ranking, but the same profile and input must be stable within a build, and
  cross-platform differences must stay within a documented, tested envelope.
  Quantized fixed-point output may be used if evaluation shows that it preserves
  useful ranking while reducing storage and drift.

A future profile must state which guarantee it makes. It must not silently
change from a scalar reference to a SIMD implementation without tests and a
documented policy. Automatic CPU-specific FFT selection, libm differences,
threaded reductions, and quantization are all profile inputs or explicit
numeric-policy choices.

### 9.2 Stable identity versus implementation detail

Use two identifiers:

- **Stable profile id:** human-readable and semantic, for example a proposed
  `musicpack-audio-descriptor-v1`; it changes when the representation is no
  longer compatible.
- **Profile fingerprint:** SHA-256 of a canonical serialization of algorithm
  version, library/version, model hash, parameters, input policy, numeric
  policy, and output encoding.

The stable id is what a product/API displays. The fingerprint is what prevents
stale cache/index rows from being compared with a changed implementation. Do
not use a dynamic dependency lockfile hash as the user-facing profile name, and
do not compare vectors merely because their dimensions happen to match.

### 9.3 Invalidation and coexistence

- Cache/index key: source audio SHA-256 plus profile fingerprint.
- A successful new run atomically becomes the active result for that profile.
- Old profile rows may be retained for audit/rollback but are marked inactive;
  queries must not mix profiles.
- A failed reanalysis must not make a package invalid and should not erase the
  last successful result without an explicit retention policy.
- Package fingerprints are not analysis cache keys. Adding analysis data to a
  manifest would change package identity, so analysis must not depend on a
  post-analysis package fingerprint.
- Existing Sonic vectors are not automatically convertible to a new profile.
  A legacy importer would need a separate semantic validator and migration ADR.

## 10. Storage and indexing decision

### 10.1 Options considered

| Option | Benefits | Costs and risks | Decision |
| --- | --- | --- | --- |
| Store descriptors in `.mpack` | Portable with the release; self-contained offline consumers; natural use of existing `analysis[]` references | Changes package identity/size when analysis changes; couples package correctness to a profile; model/license/privacy baggage; stale results travel with audio; current server/web do not consume them | **Not the default** |
| Store descriptors in a local/server index | Independent profile lifecycle; reindex without rewriting packages; better privacy and deletion; natural server query path; works with current package verifier | Not portable by default; clients need a result API; index rebuild is required after profile changes | **Recommended for v1** |
| Hybrid: package carries a small document plus local index | Portability and local speed | Two authorities, duplicate invalidation, package bloat, and a new export format to specify | **Defer; only add an explicit portable-export use case later** |

The historical Sonic design made `analysis/sonic.json` authoritative inside the
package. That remains a possible choice for a future **legacy Sonic
compatibility** lane, but this ADR deliberately does not extend it to the new
descriptor capability. A future server index for a package-owned Sonic document
would be a projection of the package, not a second authority; a future new
descriptor index is independently derived and must not masquerade as a package
asset.

### 10.2 Author-local storage

An Author-side cache should live outside the package staging tree, for example
in the application's data/cache directory. It should be atomically written,
bounded, keyed by source hash plus profile fingerprint, and removable without
touching source audio or package bytes. The exact filesystem adapter belongs to
the Tauri/author host, not `musicpack-core`.

A cache miss is a normal result. Analysis must never be required for package
correctness, and a failed or cancelled analysis must not leave a partial result
that looks complete.

### 10.3 Server-local storage

If server discovery is activated, a future additive schema can contain records
conceptually equivalent to:

```text
analysis_runs:
  profile_id, profile_fingerprint, source_sha256, status, producer_version,
  created_at, input_facts, error_code

analysis_vectors:
  track_id, profile_fingerprint, dimensions, encoding, payload, contributor_count
```

This is a design sketch, not a migration to implement in this study. The
server should keep derived data separate from package `assets`; analysis is not
an HTTP-served artwork/lyrics object. Exact cosine scan is sufficient for a
small collection. An ANN/vector database is a later optimization only after
measurement shows a real need.

The current server schema and `scan`/`verify` behavior must remain unchanged
until a separate API/schema decision is accepted. In particular, analysis
documents currently pass generic verification but are deliberately not indexed;
this ADR preserves that behavior.

If a future API is approved, it should be result-oriented: expose readiness,
profile identity, and explainable neighbor scores, not raw vectors, model
paths, cache paths, or arbitrary index rows. The existing track/release JSON
and generic asset endpoint remain unchanged until that additive contract is
separately specified.

## 11. Runtime ownership and platform boundary

```text
                 decoded PCM / AudioDecoder
                              |
                              v
                 musicpack-core::audio
              streaming analysis primitive
                    (pure, bounded, I/O-free)
                              |
          +-------------------+-------------------+
          |                                       |
          v                                       v
   musicpack-author                         musicpack-server
   explicit invocation,                 local index, maintenance,
   progress, cache, export               query/API policy
          |                                       |
          v                                       v
   local Author result                     versioned server DTO
                                                      |
                                                      v
                                                  web consumer
```

### Core

The core owns numeric semantics, input validation, profile identity, and typed
results. It does not own paths, package manifests, HTTP, SQLite, model
acquisition, threads, or UI state. The existing decoder and analysis
sanitization are the required upstream seam.

### Author

`musicpack-author` may orchestrate an explicitly selected source and expose
stage/track progress and cancellation. It must not grow a second package
serializer, a Tauri-specific DSP implementation, or a hidden model download.
The default Rust Author runtime remains Sonic-free.

### Server

The server may call the core analyzer over an already-resolved audio object and
own persistence/query behavior. It must not put analysis in the serving
request path. Analysis is maintenance work, not playback work.

### Web/WASM

The web application consumes results; it does not decode a release solely to
analyze it or download a model in response to package metadata. The core
analyzer must remain compatible with `wasm32-unknown-unknown` if it is placed
in the core crate. A future browser analysis path, if ever wanted, must be an
explicit separate feature with memory/transfer limits; it is not implied by
WASM playback support.

No current crate may add FFI, C/C++ dependencies, FFmpeg, bindgen, or a
subprocess-based production dependency. The server's existing bundled SQLite
exception does not extend to an ONNX/audio runtime.

## 12. Background processing and cancellation

The core algorithm should be synchronous and chunk-driven. Threading, job
persistence, and cancellation belong to the caller:

- Author can run a worker and report progress between chunks/tracks;
- a cache row is committed only after a complete successful result;
- cancellation discards partial state and leaves the previous valid cache row;
- server maintenance can commit per-track prefixes and resume safely.

The current server has one `Scan`/`Verify` slot, no queue, no job ids, and no
general cancellation framework (ADR 0009). Adding an analysis endpoint or
silently extending `JobKind` would be a new product/architecture decision.
For the first activation, prefer one of these explicit choices:

1. an offline author/local-index command; or
2. a separately specified maintenance pass that replaces, rather than quietly
   generalizes, the current job model.

Do not make a normal library scan unexpectedly CPU-intensive. If server
analysis is activated later, its contract must define job identity, progress,
cancellation, retry/resume, profile migration, shutdown behavior, and what
happens when one track is unsupported.

## 13. Privacy, security, and licensing

- Analysis is local/offline by default. Do not add telemetry or send raw PCM,
  tags, descriptors, or fingerprints to a remote service.
- A package's `analysis[].profile` is untrusted data. It must never select a
  model, trigger a download, load a plugin, or choose an executable.
- Model acquisition, if ever approved, is an explicit trusted user action with
  a pinned URL/artifact/hash, atomic cache installation, and an offline cached
  path. This mirrors the historical safety lesson without retaining the old
  runtime.
- Analysis inputs are untrusted. Enforce frame/window/vector/document limits
  before allocation; reject non-finite output; do not let declared duration
  cause unbounded buffering.
- New Rust dependencies must be license-reviewed individually. BSD-3-Clause
  core/author code must not absorb GPL/AGPL or non-commercial model terms.
  Encoder LGPL isolation remains unchanged.
- If the server exposes derived vectors/fingerprints, they inherit the server's
  authentication and authorization policy. A self-hosted local database does not
  remove the need for access control when the API is network-reachable.
- Deleting a source/package should have a defined policy for derived cache and
  index rows. The first implementation must not silently retain a fingerprint
  after the user deletes the corresponding audio.

## 14. Performance and resource assumptions

The historical C+ONNX measurements are a warning, not a target:

- OpenL3: approximately 3.3 seconds of analysis per minute of music and about
  1.9 GB peak RSS in the research TensorFlow stack;
- model artifact: approximately 18.7 MB;
- 512-dimensional 10-track document: approximately 27.8 kB base64-f32le.

**Benchmark status:** no new prototype or benchmark was built for this study.
The figures above are historical measurements from the sibling research report,
not measurements of a candidate Rust implementation. A model-free streaming
descriptor should have a much smaller working set: a fixed window, a small
amount of state, and a bounded output vector. That is a design expectation, not
a measured claim. Before activation, run a representative benchmark over FLAC,
WAV, and Musepack inputs, mono/stereo, supported sample rates, short/long
tracks, silence, and hostile values. Measure at least:

- cold and warm throughput on the supported native desktop;
- peak RSS and allocation behavior;
- analysis time per track and per album;
- output size and index build/query time;
- native arm64/x86_64 behavior and the core WASM check;
- chunk-size invariance and scalar-versus-SIMD numerical differences;
- cancellation latency and recovery after an interrupted run.

The minimum product gate is faster than real time on the target machine with
bounded memory and no external runtime. Exact numeric targets should be set
after a baseline, not invented from the old TensorFlow experiment. No new
prototype or benchmark was run for this study; selecting a library without an
implementation would measure packaging noise rather than the product decision.

## 15. Migration and documentation disposition

### 15.1 Sonic compatibility

- Keep `sonic_analyze` returning `sonic_retired` in the default Rust runtime.
- Keep the legacy C analyzer/model path behind `MUSICPACK_AUTHOR_LEGACY=1` and
  label it development/oracle-only.
- Preserve all existing `analysis[]` entries and bytes through Author inspect
  and rebuild.
- Do not semantically validate, compare, serve, or index historical Sonic
  documents as part of this ADR.
- Do not migrate historical vectors into a new descriptor profile implicitly.

### 15.2 Stale Sonic surfaces

The study found documentation and UI history that still describes the retired
C sidecar and ONNX model as current. This ADR does not edit those files because
the requested change is an architecture study, but they need a separate,
reviewed cleanup slice:

| Surface | Current issue | Recommended follow-up |
| --- | --- | --- |
| `author/README.md` | Sections describing the C CLI wrapper, Sonic workflow, model download, and sidecar packaging predate R4.3 | Rewrite or clearly archive the pre-R4 material; link ADR 0012/0016 |
| `docs/author-pipeline.md` | Describes the old runtime and conflates application `sonicAnalysis` state with manifest `analysis[]` | Mark historical or update to the Rust runtime |
| `author/app/.../SonicPanel.svelte` and related API/types/tests | Still present an actionable “Analyse Sonic” surface while the default backend returns a terminal error | Hide/remove behind an explicit legacy capability or change tests to assert retirement |
| `docs/r3.7-r4-readiness.md`, migration maps, and old architecture reviews | Are historical snapshots but can read like current architecture | Add archival banners/links rather than silently rewriting history |
| `docs/architecture.md` O-Sonic/open question | Describes semantic validation as deferred | Link this ADR and state that activation, not mere document parsing, is the gate |

This cleanup must not delete the legacy oracle or frozen historical material.

## 16. Rejected alternatives

| Alternative | Reason for rejection |
| --- | --- |
| Revive `musicpack-sonic` as a C+ONNX sidecar | Reintroduces the old subprocess/runtime/model burden, licensing and download complexity, and the exact boundary retired by ADR 0012 |
| Make OpenL3 the new default | Historical quality is materially below the non-commercial reference; 18.7 MB model and historical runtime burden remain; it does not solve the missing product requirement |
| Make Discogs-EffNet mandatory | Non-commercial weights/library terms are incompatible with a mandatory open product dependency |
| Adopt Essentia wrappers or aubio | AGPL/GPL, C/C++/FFI, bindgen/`unsafe`, and external/native dependencies violate the architecture; the “equiz” name was not verifiable in the survey |
| Adopt `ort`/ONNX Runtime | C API/FFI/native runtime and deployment burden violate the current boundary; WASM target availability is not enough |
| Put every descriptor/fingerprint in `.mpack` | Couples package identity and correctness to a derived, versioned index; causes rebuilds and privacy/size problems |
| Add a generalized server job queue now | ADR 0009 deliberately keeps one scan/verify slot; no current requirement justifies a queue or async framework |
| Make the web app analyze audio | Duplicates decode work, expands WASM/model transfer and memory, and conflicts with the local-first server/client ownership model |
| Expose raw vectors as a public “AI” API | Locks in an unvalidated representation and encourages cross-profile comparisons; expose task-specific results instead |

## 17. Implementation slices and exit gates

No slice below is authorized solely by this study. Each requires the preceding
product/evidence gate.

### Slice 0 — product decision

Choose one concrete user story: local acoustic neighbors, duplicate detection,
tempo, or something else. Define the collection size, expected quality measure,
whether analysis is opt-in, and whether results must travel with a package.

**Exit:** a named capability, owner, corpus, and success/failure semantics.

### Slice 1 — profile specification

Write a versioned profile document before writing an analyzer. Define input
facts, rate/channel policy, DSP/model identity, pooling, output encoding,
numeric tolerance, limits, short-track behavior, and profile fingerprint rules.

**Exit:** a fixture format and scalar/reference vectors that can be reviewed
without a model download or server schema.

### Slice 2 — core streaming prototype

Implement only the selected pure-Rust DSP over `AudioDecoder::read_f32`, with
bounded memory, hostile-input handling, chunk invariance, cancellation-friendly
boundaries, and no storage/network/UI policy. The first comparison should cover
a strict fixed-size path (`microfft`) and, if performance requires it, a
`RealFFT`/RustFFT scalar-versus-SIMD path; do not select a general MIR crate by
default.

**Exit:** unit/fuzz/property tests, native/WASM checks, and a benchmark report.
This is the first point at which a dependency may be proposed.

### Slice 3 — author/local cache

Add an explicit opt-in command only after Slice 2 passes. Keep the cache outside
the package, report progress, and preserve packages on failure/cancellation.
Do not add a model download or a new Sonic document implicitly.

**Exit:** deterministic cache round-trip and interruption/cleanup tests.

### Slice 4 — server index/query decision

If local discovery is required, specify schema/API/privacy semantics and a
migration/reindex plan. Keep analysis separate from package assets and preserve
the current scan/verify API until a separate job decision is accepted.

**Exit:** migration, authorization, deletion, profile-staleness, and query tests.

### Slice 5 — web/offline consumption

Add a client only for an accepted server contract. Do not add analysis to the
offline audio plan unless the product explicitly needs the derived result
offline. Never make a package profile trigger a download or model execution.

**Exit:** API/UI tests and an explicit stale/missing-result state.

### Deferred model lane

A learned model requires a separate ADR covering model/weights license,
provenance, download/cache trust, runtime, operator support, memory, CPU/WASM
behavior, quality corpus, and profile migration. RTen is the leading future
pure-Rust/WASM runtime candidate; Tract or Candle may be reconsidered only
behind an explicit policy/deployment exception. None is allowed in the current
core without that decision, and `ort` is not an acceptable shortcut.

## 18. Open questions for the next decision

1. Is “find acoustically related tracks/albums” a real user problem in the
   self-hosted collection, or was Sonic only an aspirational historical feature?
2. What corpus and human/quantitative quality bar make a descriptor useful
   enough to expose? The historical OpenL3/Discogs results cannot be reused as
   a guarantee for a new profile.
3. Should the first target rate preserve more high-frequency detail than a
   compact 16–22.05 kHz profile, and what resampling quality is acceptable?
4. Is cross-codec stability a requirement, or is per-source analysis sufficient?
5. Must descriptors be portable to offline clients, or is a local/server index
   enough?
6. What is the retention/deletion policy for derived fingerprints and vectors?
7. Is a separate analysis maintenance pass worth replacing the current job
   model, or should analysis remain an offline author/local operation?
8. Which exact dependency versions pass license, MSRV, `unsafe`/FFI, WASM, and
   numerical review?

Until these questions have owners and answers, the correct implementation is no
new production analysis.

## 19. Evidence and research references

### Repository evidence

- `src/audio/mod.rs` — PCM/decode seam and hostile-input analysis boundary.
- `src/audio/loudness.rs` — streaming BS.1770-5 precedent and tolerance policy.
- `src/audio/waveform_acc.rs` — bounded streaming accumulator precedent.
- `src/format/manifest/mod.rs`, `parse.rs`, `write.rs` — current opaque
  `analysis[]` reference contract.
- `src/validation/mod.rs` — generic asset verification and deferred Sonic
  semantic validation.
- `tests/package_build.rs` — executable evidence that hash-correct but invalid
  Sonic bytes are currently accepted under the generic asset contract.
- `src/authoring/build.rs`, `crates/musicpack-author/src/pipeline.rs`,
  `crates/musicpack-author/src/inspect.rs` — current package construction and
  opaque analysis preservation.
- `author/src-tauri/src/lib.rs` — `sonic_retired` default and legacy-only path.
- `crates/musicpack-server/src/jobs.rs`, `store/sqlite.rs`, `store/read.rs`,
  `http/routes.rs` — current job/index/API boundary.
- `web/app/src/lib/offline/plan.ts` and analysis UI — current client scope.
- Sibling historical `specs/musicpack-sonic-v1.md` and
  `research/sonic/reports/results.md` — frozen historical contract and measured
  research evidence. The sibling repository was read-only and remains immutable.

### External candidate sources (checked 2026-09-23)

- [RustFFT documentation](https://docs.rs/rustfft/latest/rustfft/)
- [RealFFT repository](https://github.com/HEnquist/realfft)
- [microfft crate](https://crates.io/crates/microfft)
- [Rubato crate](https://crates.io/crates/rubato)
- [Symphonia repository](https://github.com/pdeljanov/Symphonia)
- [ebur128 repository](https://github.com/sdroege/ebur128)
- [dasp-rs repository](https://github.com/dasp-rs/dasp-rs)
- [Spectrageist repository](https://github.com/gijzelaerr/spectrageist)
- [bliss-rs repository](https://github.com/Polochon-street/bliss-rs)
- [beat-this repository](https://github.com/danigb/beat-this-rs)
- [audiofp documentation](https://docs.rs/audiofp/latest/audiofp/)
- [rusty-chromaprint repository](https://github.com/darksv/rusty-chromaprint)
- [chromaprint-next repository](https://github.com/attilagyorffy/chromaprint-next)
- [RTen repository](https://github.com/robertknight/rten)
- [tract-onnx documentation](https://docs.rs/tract-onnx/latest/tract-onnx/)
- [Candle core documentation](https://docs.rs/candle-core/latest/candle_core/)
- [ort documentation](https://docs.rs/ort/latest/ort/)
- [aubio documentation](https://docs.rs/aubio/latest/aubio/)
- [Essentia licensing](https://essentia.upf.edu/licensing_information.html)
- [ff-analysis documentation](https://docs.rs/ff-analysis/latest/ff_analysis/)

External versions, licenses, target support, and maintenance are time-sensitive.
They are recorded here to make the decision reviewable, not to pre-approve a
dependency. The implementation slice must pin and re-audit the exact dependency
and model artifacts.

## 20. Conclusion

MusicPack does not need to replace Sonic in order to be correct or complete
today. The right modern replacement boundary is narrower: a pure-Rust,
streaming, versioned descriptor primitive that can be activated only for a
real local-discovery workflow, with derived data kept outside packages by
default and no learned model or general ML runtime in the first slice.

Sonic's useful lesson is its explicit profile and provenance discipline. Its
C+ONNX deployment, automatic model path, and package-coupled index are not
requirements worth recreating. The next implementation should earn its way from
a product use case and a measured profile, not from the existence of an old
binary or an aspirational recommendation screen.
