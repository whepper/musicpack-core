# musicpack-core — architecture

This document records the architecture of `musicpack-core`: the boundaries,
the design decisions and their rationale, the compatibility constraints
discovered in the existing MusicPack implementation, and the open
questions that must be resolved rather than guessed.

> Project-level architectural decisions (transport, playback boundary,
> frontend, native clients, compatibility classes) live in
> `docs/architecture-review.md` and `docs/adr/`; this document remains the
> map of the Rust workspace itself.

## 1. What exists today (discovered in the reference repository)

The existing MusicPack application repository is a C/CMake codebase with a
TypeScript web client. The parts that matter to this crate:

| Reference component | Language | Role |
|---|---|---|
| `core/libmusicpack` | C (BSD-3) | `.mpack` v1 manifest model, path rules, SHA-256 integrity, BS.1770-5 loudness, waveform envelope format, sonic analysis model, MPAK v1 container, native source decoding (FLAC via vendored `dr_flac`, native WAV reader, Musepack via `libmusepack`) |
| `core/libmusicpack` + CLI `core/musicpack` | C | authoring draft pipeline (`inspect`/`validate-draft`/`encode-draft`/`waveform-draft`/`build-draft`), `info`/`verify`/`create`/`import`/`identify`/`update-metadata`/`pack` |
| `specs/musicpack-v1.md`, `mpak-v1.md`, `musicpack-waveform-v1.md`, `musicpack-sonic-v1.md` | — | normative, frozen v1 specifications |
| `web/player-core` (`@musicpack/player-core`) | TypeScript, zero deps | platform-independent player core: types/engine ports, queue model, ordering policy, gain math, session snapshots, typed events, **Sweet Fades transition planner**, crossfade orchestration; purity-gated in CI |
| `web` (host side) | TypeScript | representation selection, offline downloads (OPFS + IndexedDB), waveform fetching/windowing, audio engines (WASM Musepack + native), persistence wrappers |
| `tests/` | C/Python/shell | reference packages, generated v1 conformance corpus (6 valid / 57 invalid-manifest / 9 invalid-verify cases), hostile-package suite, manifest fuzz-lite, MPAK container suite |

Two facts shape the architecture:

1. **The domain already splits cleanly along the same seams we want.**
   `libmusicpack` is network-free and platform-independent by rule; the
   player-core is purity-gated to no-ambient-globals. The Rust crate ports
   domains that were *designed* to be portable.
2. **Codec behaviour lives below the package layer.** `libmusicpack` uses
   `libmusepack` (decoder, BSD-3) through a narrow handoff; the codec is
   deliberately conservative territory. The Rust core must keep the same
   separation and must not commit to a codec strategy prematurely
   (Open Question O5).

## 2. Boundaries

```text
                    ┌─────────────────────────────────────────────┐
                    │                musicpack-core               │
                    │                                             │
                    │  format/    .mpack v1 manifest, paths,      │
                    │              checksums, MPAK v1 container,  │
                    │              waveform envelope format       │
                    │  validation/ verify semantics, reports      │
                    │  audio/      PCM types, decode seam,        │
                    │              BS.1770-5 meter, accumulator   │
                    │  player/     queue, engine ports, gain,     │
                    │              snapshots, transitions         │
                    │  policy/     representation selection,      │
                    │              playback loudness policy       │
                    └─────────────────────────────────────────────┘
                          │                    │                 │
             native apps /│ server integration │ musicpack-wasm │ CLI (differential harness)
                          ▼                    ▼                 ▼
                    filesystem, HTTP      browser/Svelte       corpus runner
                    adapters              (Web Audio, storage)
                              ▲
                              │  (musicpack-engine: deterministic
                              │   ring · resampler · equal-power mixer ·
                              │   source/decoder factories; std-only)
                              │
                    musicpack-host (pull loop · shared engine handle ·
                              │      source selection); owns the clock/device
                              ▲
                    device callback / AudioWorklet / native app
```

Hard rules (mirroring the reference's own invariants):

- **No HTTP, UI, browser or Node APIs, auth, sessions, database access,
  or deployment concerns in the core.** The core takes bytes and gives
  judgements.
- **No direct `std::fs` in parsing/decoding paths.** Storage backends
  (directory bundle, MPAK over a byte source) are trait-implemented
  adapters; the core defines the traits. This is what keeps WebAssembly a
  first-class target (the reference achieves the same with
  `musicpack_range_source` — "acquire then serve", never HTTP client code
  in the library).
- **No PCM streaming through the player core.** Engines own audio; the
  player core sees control flow and coarse metadata (player-core purity
  law 4). The engine *adapter* (`crates/musicpack-engine`) owns PCM but also
  sees no device, browser, network, filesystem, timer, thread or async
  machinery: a host supplies readable bytes and drives `consume()`.
- **No ambient time/randomness in the player core** — clock and scheduling
  arrive through ports (purity law 2).
- **`unsafe` is forbidden** (`#![forbid(unsafe_code)]`). The only
  conceivable future exception is a documented FFI boundary to the C
  codec, which would live in an adapter crate, not here.
- **Untrusted input everywhere `.mpack`/`.mpak` is parsed**: checked
  arithmetic on every untrusted length/offset (MPAK rule §8), budget
  enforcement before allocation (`limits`), errors instead of panics on
  malformed input. Panics are bugs.

### Workspace and CLI

The repository is a Cargo workspace whose **root is the core package**
(chosen over moving the core into `crates/` so the established layout,
fixtures, docs and tooling stay put — O8). `crates/musicpack` is a thin
binary that *consumes* the core; it contains argument parsing,
presentation, and the one piece of application policy the core does not
own: safe container extraction. It reimplements no format, verification
or container semantics, and the core never depends on it.

`crates/musicpack-engine` is the deterministic PCM adapter: it implements
the core's `player::Engine` seam over `AudioDecoder` with a bounded ring, a
streaming linear resampler and an equal-power crossfade mixer (hand-ports
of the reference TypeScript). It depends on the core and `std` only, and
performs no I/O.

`crates/musicpack-host` is the thinnest practical **host adapter**: it owns
the host concerns the engine must not — a `SourceBackend`, a shared
(`Rc<RefCell<…>>`) engine handle, the pull loop a device or AudioWorklet
callback drives (`Host::render`), and the mapping from the core
representation policy to a concrete source (`select_source`). It depends on
the core and the engine only. `crates/musicpack-wasm` is a thin
`wasm-bindgen` binding over both; it is not the browser application, the Web
Audio engine, HTTP, persistence, or UI.

```text
Cargo.toml (package musicpack-core + workspace)   src/ tests/ fixtures/ tools/ docs/
crates/musicpack/         thin CLI (info | verify | pack | unpack)
crates/musicpack-engine/  deterministic PCM adapter (std-only, no I/O)
crates/musicpack-host/    host pull loop + source selection (device/worklet side)
crates/musicpack-wasm/    thin wasm-bindgen binding foundation
fuzz/                     cargo-fuzz targets (excluded from the workspace; nightly)
```

## 3. Module decisions

| Module | Why it exists now | Shape |
|---|---|---|
| `error` | The C status taxonomy is the differential vocabulary; message wording is grepped by the C suites ("checksum mismatch", "missing file", "exceeds") | `#[non_exhaustive]` enum, hand-rolled `Display`/`Error` (no `thiserror`) |
| `limits` | Security budgets are compatibility surface (hostile suite asserts fast rejection of oversized objects) | plain `const`s pinned by a test against the C header values |
| `json` | The manifest dialect is "the JSON cJSON actually accepts" — a hand-rolled ordered `Value` + parser reproducing cJSON's number grammar, whitespace rule, NUL truncation, BOM handling, surrogate rules and the 1000-level nesting limit, plus the canonical printer | complete; corpus- and quirk-tested |
| `format::number` | Exact port of the C `json_number` / `%.8g` behaviour — the precondition for byte-identical canonical manifests | complete; proven against 3394 C-generated vectors |
| `format::path` | Fully specified, pure, high-value: every conformance case exercises it | complete port of `musicpack_path_validate`; containment lives in the storage adapter |
| `format::checksum` | SHA-256 is the integrity model: declaration format *and* production digest computation (`sha2`) | complete; known vectors + an independent oracle cross-check |
| `format::manifest` | The typed model, strict parser and canonical writer, including unknown-root-field preservation | complete; byte-identity proven against reference-CLI output |
| `format::mpak` | Complete MPAK v1 container: framing/CRC constants, scan-oriented reader, deterministic writer | reader + writer proven byte-identical to the reference CLI packs |
| `format::waveform` | Quantization kernel is a pure function with exact spec cases; payload limits feed verification | `quantize_amplitude` + limits ported; accumulator later in `audio` |
| `lyrics` | Lyrics domain (`docs/musicpack-lyrics-v1.md`): strict LRC profile parser, plain/synced model, pure active-line timing; per-track `lyrics[]` manifest references (`path`, `sha256`, optional `lang`) | complete (R3.2); canonical position between `waveform` and `representations`; root `lyrics[]` unchanged; pack order gains a per-track group after waveforms |
| `storage` | Platform-independent seam: the verifier asks a backend for objects; no filesystem API in the domain core | `PackageBackend`, `VerificationSink`, directory adapter (unix), MPAK adapter (portable), in-memory/file byte sources |
| `validation` | Verify semantics: report model, traversal order, budgets, checksums, containment classification, waveform payload checks, unreferenced warnings, backend container findings | complete (port of `musicpack_package_verify`) |
| `audio` | PCM contract + decode seam + native WAV reader + FLAC (`claxon`) adapter (phase 8); streaming waveform accumulator + BS.1770-5 loudness/true-peak meter (phase 9); native Musepack SV8 decoder (phase 13B: container, bitstream, requantisation, synthesis; PCM byte-identical to libmpcdec) | `AudioInfo`/`Codec`, `AudioDecoder`, `open`, `wav`, `flac`, `musepack::{sv8, decoder}`, `waveform_acc::WaveformAccumulator`, `loudness::{LoudnessMeter, Loudness, gain_db}` |
| `player` | Platform-independent player core: types, engine seam, queue/order, gain, snapshot, transition planner, events, orchestrator | complete (phase 10); the deterministic PCM adapter is `crates/musicpack-engine` (phase 11); native device engines, persistence and Media Session remain host/later phases |
| `policy` | Representation selection (phase 12) + playback loudness policy: the pure rules deciding which audio object a track plays | `policy::representation` is a total, I/O-free port of the Phase 4 TS resolver (manifest order is the only tie-break; injected playability; primary rescue); `player::gain` owns the loudness policy |
| `crates/musicpack` (CLI) | Thin real-world consumer: `info`/`verify`/`pack`/`unpack`; exits non-zero exactly when the core report has errors; `pack` stages and verifies before publishing; `unpack` is the application-layer safe extractor | delegates all semantics to the core; no `unsafe`; no argument-parsing dependency |
| `crates/musicpack-engine` | The deterministic bridge from `player::Engine` to decoded PCM: bounded ring, streaming linear resampler, equal-power crossfade mixer + swap accounting, source/decoder factories, per-track decode sessions, host-driven `consume()` | port of the reference TypeScript `audio-worklet.ts` / `ring-buffer.ts` / `streaming-resampler.ts`; `std`-only, `#![forbid(unsafe_code)]`, no I/O, no threads, no async; depends on the core only |
| `crates/musicpack-host` | The thinnest practical host adapter: `Player` + `DecoderEngine` + a `SourceBackend`, a `render(frames)` pull loop (device/worklet callback), and `select_source` mapping the policy decision to a URL | host concerns only; owns the shared engine handle (`Rc<RefCell<…>>`); depends on the core + engine; no device API of its own |
| `crates/musicpack-wasm` | Thin `wasm-bindgen` binding over the core and the adapter: byte-backed decode handles, a player handle, and the representation resolver | not the browser app / Web Audio / HTTP / persistence / UI; `wasm-bindgen` + `js-sys` only |
| `crates/musicpack-server` | Self-hosted library server (stages 1–6: CLI/config skeleton, byte-compatible SQLite layer for migrations 1–10 plus the Rust-defined additive v11 (`assets.track_id`/`lang`, R3.3), `token create‖list‖revoke`, collector identity, bounded discovery, ingestion state machine with C-oracle parity, read-only JSON API with C-oracle parity, secure byte/media serving with C-oracle parity, static hosting + SPA fallback and library jobs with C-oracle parity; deployment/cutover later) — native-only, `#![forbid(unsafe_code)]`, the one crate whose `rusqlite`-bundled SQLite stays inside the dependency (see §6/D-S2). **Track-linked lyrics** (`docs/musicpack-lyrics-v1.md` §7) are persisted as ordinary `lyrics` assets with a nullable `track_id`; lyric bytes are served through the existing asset endpoint, while lyric parsing and timing remain exclusively in `musicpack-core` |

Deliberately **not** built yet: a native device backend and the production
browser/AudioWorklet cutover (the host seam is proven in
`crates/musicpack-host`, but no device or browser is wired up). The Musepack
SV8 decoder is built (§4, phase 13B; O5 records the FLAC/Musepack history).

## 4. Compatibility constraints (binding)

These are behaviours the Rust implementation must reproduce; they were
read out of the specs and the C/TS sources, not invented:

### Manifest parsing

- Strict JSON: **single document, no trailing bytes** (conformance
  `trailing-json`, `nul-suffix`), **duplicate keys rejected at any depth**
  (`duplicate-format`, `duplicate-credit-mbid`, ...), nesting bounded,
  manifest ≤ 16 MiB, numbers finite and range-safe.
- `format == "musicpack"`, `version == 1`; unknown majors rejected cleanly.
- Closed enums: `releaseType` (10), `media[].format` (8),
  `identity.source` (3), `identity.confidence` (4). Unknown → error.
- Loudness values finite in `[-70, 6]`; `trackLUFS`/`truePeakDbTP` (and
  the album pair) are both-or-neither.
- Numbering: `disc ≥ 1` unique; `track ≥ 1` unique per disc (integers —
  `fractional-track` rejected; `oversized-disc` > i32 rejected).
- Asset paths unique across the whole package (primary audio,
  representations, artwork, booklet, lyrics, extras, analysis, waveform).
- Budgets: 4096 referenced assets total; per-array caps (32 discs,
  512 tracks/disc, ...); 8 GiB per file; 64 GiB aggregate.
- Unknown **root-level** fields survive a read/write round trip; unknown
  fields nested inside known objects do not.

### Canonical serialization

- Fixed key order (credits: `musicbrainzId, name, role, sortName`),
  2-space indent, absent optionals omitted entirely (never `null`),
  no timestamps.
- Numbers: **integral values within ±9.2e18 print as plain integers**
  (`json_number`'s first branch — `123456789.0` prints as `123456789`,
  never `1.2345679e+08`; `-12.0` as `-12`), everything else through the
  exact `%.8g` port in `format::number`. Byte-identity is proven against
  a committed table of 3394 vectors generated by running the real C
  `snprintf` (`tools/g8_probe.c` → `tests/data/g8_c_reference.txt`) and
  against canonical manifests rewritten by the reference CLI.
- Unknown root fields are **appended last** in canonical output, in
  original order, with values re-serialized canonically.
- A write is only performed after the built tree re-parses cleanly
  (`validate_for_write` discipline).

### Paths

- The canonical rules in `format::path` (port of `path.c`); length
  measured in **bytes** (C `strlen`), not characters.

### MPAK container

- Big-endian throughout; 16-byte header; 14-byte block framing;
  **CRC-16/BUYPASS verified before the declared `length` is trusted**
  (frame → CRC → length → overflow/bounds → consumption); unknown and
  private block types skipped by their CRC-validated length.
- Readers locate blocks by scanning, never by assumed position; `INDX` is
  acceleration only and is **discarded whenever it disagrees with the scan**
  (path/offset/length reconciliation), so index lookups can never point
  where the stream does not.
- Deterministic writes: block order `header, INDX, MANF, DATA…, TAIL`;
  `DATA` in the reference's canonical traversal order — **all primary
  audio, then all representations, then all waveforms, then artwork,
  booklet, lyrics, extras, analysis** (see D9); `INDX` lexicographically
  sorted by path bytes; two identical logical packages produce
  byte-identical files (proven against reference CLI packs).
- `TAIL` covers every preceding byte (`sha256`), reports the total file
  size, object count and INDX offset; its absence is a warning
  (`completeness unproven (no TAIL)`), never an error.
- Container limits: `length ≤ 2^63−1`; `INDX` payload ≤ 32 MiB; members
  ≤ 4096; per-file/aggregate/manifest budgets reuse [`crate::limits`].
- Container verification findings are the backend's concern and are
  reported through `PackageBackend::verify_extra` at the reference's call
  site: minor version, reserved bytes and resync are warnings; duplicate
  members, INDX/hash disagreement and any `TAIL` inconsistency are errors.
- Memory model: containers are **streamed from a `ByteSource`**, never
  buffered whole; member access returns a bounded range reader over the
  shared source (the reference's seekable `FILE*` design). Native files,
  in-memory buffers and future range/OPFS sources implement the same seam.

### Verification

- Every referenced asset is opened through the storage backend, which
  enforces containment, regular-file and link-count checks; the verifier
  then applies the per-file budget (8 GiB), the aggregate budget (64 GiB,
  with same-pass object deduplication on POSIX), and SHA-256.
- Failure vocabulary is compatibility surface (CLI stderr and tests):
  `unsafe path`, `missing file`, `exceeds …-byte file limit`,
  `aggregate referenced bytes exceed …-byte limit`, `cannot hash`,
  `checksum mismatch`, plus waveform-specific messages; unreferenced files
  are warnings.
- Symlinks: a **final-component** symlink is rejected as `missing file`
  (the reference opens with `O_NOFOLLOW`); a final symlink resolving
  outside the root is `unsafe path`; an **intermediate** directory symlink
  resolving inside the root is followed (accepted), one resolving outside
  is `unsafe path`.
- Hard links (`nlink > 1`), directories, FIFOs, sockets and device nodes
  at a referenced path are all `missing file`; FIFOs are rejected without
  opening (never block).
- Traversal order and reports are deterministic (the reference's asset
  order; the unreferenced-file list is sorted because the reference's
  `readdir` order is unspecified).
- Sonic `analysis[]` document validation is deferred (see Open Questions).

### Audio decoding (phase 8)

The reference's `musicpack_audio_*` seam is ported as the [`crate::audio`]
module:

- **PCM contract.** Interleaved `f32` in `~[-1, 1]` and interleaved
  **left-aligned** `i32` (16-bit content in bits 31..16, 24-bit in bits
  31..8), 1..=8 channels. Decoders write into **caller-owned** slices, so
  the core never allocates from an untrusted audio length. Reads return a
  *frame* count; `Ok(0)` is clean EOF, a short read is a success, and the
  destination length must be a multiple of the channel count.
- **Integer→float conversion** is exactly `(sample as f32) * 2^-(bits-1)`
  evaluated in `f32` (cast first, then multiply — no `f64` intermediate, no
  clipping, no symmetric remap). The integer minimum is `-1.0`; `+1.0` is
  unreachable from integer PCM. IEEE-float WAV is a raw little-endian bit
  copy (NaN payloads, infinities, denormals and out-of-range values are
  preserved); `s32` reads from an IEEE-float source are
  [`Error::Unsupported`] and never silently convert.
- **`s32` left-alignment.** u8 `(sample − 128) << 24`, i16 `<< 16`, i24
  sign-extend then `<< 8`, i32 verbatim; FLAC `sample << (32 − bits)`.
- **Decoder seam.** `audio::open(Box<dyn std::io::Read>)` sniffs the magic
  bytes (`RIFF` → WAVE, `fLaC` → FLAC, `MPCK` → Musepack SV8) and returns a
  boxed [`AudioDecoder`]. The decoder owns its reader — exactly the shape
  [`crate::storage::OpenedAsset::reader`] already yields for MPAK members —
  so no new byte-stream trait is introduced and [`crate::format::mpak::ByteSource`]
  remains the random-access container seam. Extension-based dispatch is the
  reference CLI's application policy and stays out of the core (a
  deliberate layering difference from `musicpack_audio_open`).
- **WAV acceptance matrix** (port of `audio_open_wav`): `RIFF`/`WAVE`
  required, the RIFF size field ignored; the chunk scan stops once both
  `fmt ` and `data` have been seen; `data` before `fmt ` is rejected; the
  last `fmt ` before `data` wins; `fmt ` must be ≥ 16 bytes with ≤ 40
  parsed; unknown chunks are skipped by `size + (size & 1)` (no pad byte is
  skipped after `fmt `); tag 1 = 8/16/24/32-bit PCM, tag 3 = 32-bit IEEE
  float only, and every other tag (ADPCM included) is
  [`Error::Invalid`]; channels 1..=8; sample rate non-zero with no upper
  bound; `block_align == channels × bytes_per_sample` exactly (rejecting
  24-bit-in-32-bit-container layouts); `byte_rate` is never checked.
- **Truncated data.** `total_frames = data_len / block_align` is the
  **declared** length and is never clamped to the bytes physically present.
  Reads return every complete frame the source yields, discard a trailing
  partial frame, and report `Ok(0)` at EOF — physical truncation is not an
  error (the committed `wav-truncated.wav` fixture drives this).
- **FLAC** is decoded through `claxon` behind `audio::flac::FlacDecoder`;
  no `claxon` type appears in the public API. `s32` is the native sample
  left-aligned to 32 bits and `f32` is the same `2^-(bits-1)` rule. The
  adapter's PCM is differential-tested against the reference's vendored
  `dr_flac` (see §5).

### Waveform and loudness analysis (phase 9)

Both analysis paths consume interleaved `f32` from
[`crate::audio::AudioDecoder::read_f32`]; neither decodes, resamples or
buffers whole files.

- **Waveform** ([`crate::audio::waveform_acc::WaveformAccumulator`]): a
  streaming 100 ms-bucket accumulator on **cumulative** sample-time
  boundaries (`floor(frames·1000 / (rate·100))`). Per bucket, `peak = max
  |sample|` over all channels and frames, `rms = sqrt(Σs² / N)` with a
  single denominator `N = frames·channels`; silent buckets emit `(0, 0)`
  without the log mapping. Quantization reuses
  [`crate::format::waveform::quantize_amplitude`] unchanged. Output is the
  `peak-rms-u8` payload, capped at `MAX_POINTS` (864 000 buckets,
  1 728 000 bytes); reaching the cap is allowed, exceeding it is
  [`Error::Invalid`] with state intact, exactly like the reference. Empty
  streams produce zero buckets; the final partial bucket is flushed.
- **Loudness** ([`crate::audio::loudness::LoudnessMeter`]): a hand-port of
  the vendored **libebur128 1.2.6** (MIT) subset the reference actually
  uses — 5th-order K-weighting (shelf × RLB, `f64` state, the reference's
  `tan`/`pow` coefficient formulas and update order), 400 ms first block /
  100 ms hop, absolute gate `10^((−70+0.691)/10)`, −10 LU relative gate,
  integrated `10·log10(mean) − 0.691`, and the 49-tap polyphase true peak
  (4×/2×/none by rate band). Momentary, short-term, LRA, histogram,
  window/history and `change_parameters` are not ported. Only channels 1–2
  and rates `16..=2 822 400` (the `ebur128_init` bounds) are accepted; mono
  is a single LEFT channel, stereo LEFT/RIGHT. `result()` is repeatable and
  floors loudness/true peak at −70; short input keeps a live true peak while
  integrated loudness stays −70 (no complete gating block). At ≥ 192 kHz the
  interpolator is disabled and **true peak degrades to the sample peak**,
  reproducing the reference.
- **Album semantics are caller policy:** one meter fed every track in
  manifest order (sequential feeds are equivalent to one concatenated
  stream). There is no album type and no automatic mixed-rate/channel
  detection; the reference's first-track-configuration-wins quirk is
  documented as a caller contract, not corrected.
- **Chunk invariance:** both accumulators are stateful across calls and
  independent of the caller's buffer size. Waveform output is byte-identical
  and loudness results are bit-identical across arbitrary chunkings; this is
  pinned by tests.
- **Hostile input:** NaN→0, ±Inf→±1 at both feed boundaries
  ([`crate::audio::sanitize_sample`]); finite out-of-range values are
  carried through exactly as the reference carries them. This is a
  deliberate deviation recorded as **D12** (the C code is undefined for
  NaN/Inf). The phase 8 decoder contract is not modified.
- **Numerical equivalence:** waveform payloads are required **byte-exact**
  against the reference corpus; loudness is required within **±0.05 LU /
  ±0.05 dBTP** (libm/`long double` platform variance — D13, O6). The
  algorithm structure is reproduced exactly regardless of that tolerance.
- **Memory:** the waveform accumulator is constant-memory apart from its
  capped output; the loudness meter retains one block energy per 100 ms for
  the whole program (8 bytes/100 ms ≈ 288 KiB/hour), matching the
  reference's effectively-unbounded history.

### Waveform

- 100 ms buckets on cumulative sample-time boundaries; `peak_u8`/`rms_u8`
  interleaved, no header; quantization per `format::waveform`; `points`
  must equal payload bytes / 2; `points ≤ 864 000`; closed enums
  (`version=1`, `intervalMs=100`, `encoding="peak-rms-u8"`,
  `floorDb=-60`); duration-vs-points divergence > 2 buckets is a
  **warning**.

### Loudness

- BS.1770-5, album measured as one concatenated program in manifest order
  (never averaged); true peak = max across tracks; gain derived, never
  stored.

### Player domain (phase 10)

Implemented as the [`crate::player`] module — a behavioural port of the
TypeScript `web/player-core` package (BSD-3-Clause), which remains the
oracle. The numeric constants, queue semantics, standby/policy agreement,
album-clock compression and EOS-jump repair are ported exactly; equal-power
fade *curves* stay engine-side (the core only plans the overlap):

- **Queue** ([`crate::player::queue`]): canonical list + cursor; repeat
  modes; current-first shuffle via an injected RNG (history-bounded at 500,
  defensive clamp for a host RNG returning `1.0`); `next`/`previous`/`move`/
  `removeAt`/`clear` with the reference's cursor and history remapping.
- **Engine seam** ([`crate::player::engine`]): a synchronous `Engine` trait;
  the TypeScript `PreloadEngine`/`CrossfadeEngine`/`DecodeGate` capability
  concepts collapse into defaulted methods gated by `EngineCapabilities`.
  The host owns all real audio; no PCM crosses the core (purity law 4).
- **Player** ([`crate::player::player`]): the transport state machine
  (`idle`/`loading`/`buffering`/`playing`/`paused`/`ended`/`error`),
  generation/epoch stale-event suppression, album-absolute position with
  track-relative position events, gapless standby promotion, tick
  catch-up (forward, one step, proven length, boundary ownership) and the
  crossfade trigger with `{0,4,8,12}` presets, `[0.25, 15]` clamp,
  `END_TOLERANCE_SAMPLES = 256`, pausable in-flight fades, album-clock
  compression by the actual overlap, and the `boundary-drift` diagnostic.
- **Transitions** ([`crate::player::transition`]): the pure Sweet-Fades
  planner (gapless / hard-cut / sweet-fade) with the exact rule order,
  constants and 2-decimal overlap rounding. Hosts supply the boundary
  profiles (waveform-derived); the core fetches nothing.
- **Gain** ([`crate::player::gain`]): the `-16 LUFS` / `-1 dBTP` policy
  layered on Phase 9's [`crate::audio::gain_db`] measurement primitive.
- **Snapshot** ([`crate::player::snapshot`]) and **events**
  ([`crate::player::events`]): the v1/v2 session codec and the seven-event
  union.

Deliberate deviations from the TypeScript reference (all documented in the
module docs and §8): a synchronous engine seam with explicit `on_*` event
injections, `Vec<PlayerEvent>` return values instead of a subscription sink,
host-side persistence scheduling/Media Session, a safe duplicate-key
snapshot rejection, and `f64` shortest-round-trip snapshot number
formatting. The TS unit suites are the acceptance tests and are ported
directly (`tests/player.rs`) plus replayed differentially
(`tests/player_oracle.rs` → `tests/data/player_oracle.jsonl`).

### Engine adapter and WASM binding (phase 11)

`crates/musicpack-engine` is the deterministic, platform-neutral adapter
between the player's `Engine` seam and decoded PCM. It is a **port of the
reference TypeScript PCM machinery**
(`web/app/src/lib/playback/audio-worklet.ts`, `ring-buffer.ts`,
`streaming-resampler.ts`, `worklet-protocol.ts`, `musepack-engine.ts`),
which remains the behavioural oracle. Ownership:

```text
PlaybackItem → SourceBackend → Box<dyn Read> → audio::open() → AudioDecoder
                                                                     │
            DecodeSession { decoder · bounded ring · streaming resampler }
                                                                     │
                              DecoderEngine (implements player::Engine)
                                  │                       │
                             consume()      prepare_next / advance / begin_crossfade
                                  │                       │
                             host output     host completion → Player::on_crossfade_complete
```

- **Ring** (`ring.rs`): fixed-capacity interleaved `f32`; absolute
  read/write playheads; drop-on-overflow; underrun returns fewer frames; a
  *reported* base (`continue_playhead_from`) that rebases the reported
  playhead across a crossfade swap without moving physical reads/writes.
- **Resampler** (`resampler.rs`): the reference's streaming linear
  interpolation, exactly — state between chunks, `ceil(src·out/src_rate)`
  length normalization, mono→stereo duplication and >stereo truncation,
  and the tail flush.
- **Mixer** (`mixer.rs`): equal-power `cos(t·π/2)` / `sin(t·π/2)`, missing
  lane rendered as zero, `overlap_frames` as the *actual* consumed outgoing
  tail, and the swap rebase `delta = outgoingAtMixStart − incomingAtSwap`.
- **DecodeSession** (`session.rs`): decoder + ring + resampler; demand-driven
  bounded pumping (decoder `read_f32` chunking like the worker's
  `8 × 1152`), high/low watermark backpressure (0.8 / 0.2 of an 8 s ring),
  EOF vs underrun kept distinct (`exhausted = eos && tail flushed && ring
  empty`), and seek by reopen + skip with a ring-playhead rebase.
- **Rendered samples** = output-rate frames actually consumed since the last
  open/seek reset (the ring read playhead, rebased at a swap). It is **not**
  decoded, buffered, scheduled or source frames.
- **Gain** is applied once at the output seam: the player's linear factor is
  multiplied into each drained `f32` sample (no smoothing, no clipping).
- **`CrossfadeStart`**: the adapter primes the standby lane synchronously and
  returns `Pending`; the host consumes frames until the swap completes, then
  takes the `CrossfadeResult` and calls `Player::on_crossfade_complete`.
  No futures enter the adapter. Cancellation is driven by open/close/seek/
  superseding attempts; pause does not cancel a running fade.
- **Sources** are abstract readable bytes. `MemorySourceBackend` (tests,
  embedded bytes) and `PackageSourceBackend` (over an existing core
  `PackageBackend`, e.g. MPAK members) ship; filesystem, HTTP, OPFS and
  device output stay in host adapters. `AudioDecoder` is unchanged; a future
  Musepack decoder plugs into the same `DecoderFactory`.
- **Capabilities** are honest: `preload_next`, `sample_accurate_gapless`,
  `decode_gate` and `crossfade` are `true` because the adapter genuinely
  implements them.

`crates/musicpack-wasm` exposes a deliberately small binding: `decode_open`
/ `decode_info` / `decode_read` / `decode_seek` / `decode_close` and a
`WasmPlayer` (`add_source`, `load`, `command`, `render`, `info`, `snapshot`,
`restore`). Sources are complete byte buffers; PCM crosses as a
`Float32Array` with one explicit copy (no zero-copy claim). The
production browser cutover (AudioWorklet, Media Session, range fetching)
is out of scope and remains TypeScript.

The mixer is differentially gated: `tools/xfade_oracle.ts` drives the real
`MusicPackPcmProcessor` over deterministic scripted PCM and writes
`tests/data/xfade_oracle.jsonl`; `tests/xfade_oracle.rs` replays it and
requires **byte-identical** output (10 cases: silence, DC, ramp, PRNG,
stereo, short outgoing/incoming, both-short, impulse; no tolerance).

### Representation policy and host integration (phase 12)

`policy::representation` is a port of the reference's Phase 4 resolver
(`web/app/src/lib/state/representation-selection.ts`, BSD-3-Clause). It is
pure, total, I/O-free and player-blind:

```text
resolve_audio(track: TrackAudio, pref: Option<&AudioPreference>,
              can_play: &impl Playability) -> SelectedAudio
```

- **Inputs**: a track's primary codec/MIME and its representations in
  manifest order (`id`, codec, MIME); the persisted preference
  (`None`/`Default`, `Representation { id }`, `Codec { codec }`, `Lossless`);
  an injected playability predicate (`Playability`, with a blanket impl for
  closures) covering browser capability *or* offline availability.
- **Output**: `SelectedAudio { representation: Option<usize> }` — an index
  into the candidate list, or `None` for the primary.
- **Rules** (exact reference order): default → primary; explicit id if
  present and playable; codec (case-insensitive) first playable; lossless
  (`flac`/`wav`/`aiff`) first playable; then, if the preference resolved
  nothing and the primary is unplayable but candidates exist, the first
  playable alternate (rescue); otherwise the primary. Manifest order is the
  only tie-break; size never decides; the resolver never invents
  representations and never fails.
- **Ownership**: the policy never touches HTTP, filesystem, player state,
  decoder implementation or browser APIs; `musicpack-host`'s `select_source`
  maps its result to a concrete URL. The wasm binding exposes the same
  resolver as `representation_select(track, pref, predicate)` for browser
  hosts.

`crates/musicpack-host` is the host seam. It owns a `SourceBackend`, one
`DecoderEngine` behind an `Rc<RefCell<…>>`, and a `Player` wired to it
(`resolve_kind` reports the engine's continuous "reset-offset + rendered"
album clock). The device/worklet callback calls `Host::render(frames)`,
which:

1. calls `DecoderEngine::consume(frames, &mut buffer)`;
2. feeds every engine fact back: `take_crossfade_result` →
   `Player::on_crossfade_complete`, `take_error` → `Player::on_engine_error`,
   `is_output_drained` → `Player::on_eos`, then `Player::on_tick`.

The engine never sees a clock or a device. A paused host simply stops
pulling (the reference suspends the audio context), so no time enters the
engine. Crossfade completion stays: `begin_crossfade` (engine) → `Pending` →
host `consume` until the swap → `take_crossfade_result` →
`Player::on_crossfade_complete`.

Differential policy coverage: `tools/policy_oracle.ts` drives the real TS
resolver over 32 scripted (track, preference, playability) records and
writes `tests/data/policy_oracle.jsonl`; `tests/policy_oracle.rs` requires
the same normalized preference and the same selected representation id, with
no tolerance.

## 5. Testing strategy

| Layer | Tool | Status |
|---|---|---|
| Unit | `#[cfg(test)]` in-module | established (paths, enums, CRC-16, quantization, limits, checksum form, JSON quirks, %.8g, SHA-256 vectors, verify orchestration with in-memory backends) |
| Integration | `tests/` | established: `numeric_format` (3394 C vectors), `manifest_write` (byte-identity), `manifest_parse` (budgets/enums/quirks), `verification` (18 checksum/filesystem/containment/budget/report cases) |
| **Conformance corpus** | `tests/conformance_corpus.rs` + `tests/support` (Rust port byte-compared against the authoritative Python generator) | **regression gate: all 72 cases pass parse + full verification, zero gaps** (verification stage unix-only; parse stage everywhere) |
| **Differential (reference CLI)** | `tests/conformance_differential.rs` — corpus through the reference `info` and `verify` and the Rust `verify_directory`, comparing outcomes | **operational: 72 cases × (info, verify) match, zero gaps**; skips with a notice when no reference binary is built |
| Hostile filesystem | `tests/verification.rs` (final/intermediate symlinks, hard links, FIFOs, directories, sparse oversize) | established, mirrors the reference's `run_mpack_hostile.sh` scenarios |
| **MPAK container** | `tests/mpak.rs` (37 portable reader/writer/backend/malformed/security cases), `tests/mpak_compat.rs` (committed reference container + **byte-identical packing**), `tests/mpak_differential.rs` (reference CLI pack ⇄ Rust pack, verify/unpack) | established; Rust packs byte-identical to the reference, reference CLI reads Rust containers |
| Round-trip | parse → serialize → byte-compare against canonical fixtures | established |
| **Hostile replay** | `tests/hostile.rs` (manifest + container hostile corpus), CLI `tests/cli.rs` (reference `run_mpack_hostile.sh` cases end-to-end: FIFO/dir/symlink escape/FIFO manifest/hard link/sparse oversize) | established |
| Fuzz-lite replay | `tests/fuzz_lite.rs` — deterministic 83-case corpus (truncations, CPython-identical bit flips, path injections) through the Rust parser/verifier and, when a reference CLI exists, through the reference tool (crash oracle) | established |
| **Reference TS suites** | `../musicpack/web`: `vitest run` (368 unit tests, incl. the 27 Phase 4 representation tests) and `node tests/node/run.mjs` — run against the unmodified reference; the Rust ports are gated against the same behavior via the committed oracles | established (phase 12 re-run) |
| **CLI differential** | `crates/musicpack/tests/differential.rs` — exit codes, byte-identical `pack`, cross-verification, unpack-tree equality, corrupt-container behaviour | established |
| **Audio fixtures/differential** | `tests/audio.rs` + in-module WAV tests: reference fixture parity (port of `audio_tests.c`), the hostile/truncated WAV table, the `WAVE_FORMAT_EXTENSIBLE` matrix (D11), exact conversion vectors, and FLAC PCM SHA-256 against the reference's vendored `dr_flac` (`tools/audio_probe.c` → `tests/data/audio_c_reference.txt`) | established (phase 8) |
| **Analysis differential** | `tests/analysis.rs` + in-module tests: waveform payloads **byte-exact** and loudness within ±0.05 against the reference `waveform.c`/`loudness.c`/`ebur128` (`tools/analysis_probe.c` → `tests/data/analysis_c_reference.txt`); fixture parity, chunk invariance, hostile float coverage | established (phase 9) |
| **Player differential** | `tests/player.rs` (port of the TS `player-core` unit suites: transition/gain/snapshot/queue + player/crossfade scenarios) and `tests/player_oracle.rs` (replay of `tests/data/player_oracle.jsonl`, generated by `tools/player_oracle.ts` through the reference TS package) | established (phase 10) |
| **Engine adapter** | `crates/musicpack-engine`: in-module ring/resampler/mixer/session vectors ported from the TS suites, `tests/integration.rs` (13 host-level `Player` scenarios: open/play, pause/resume, seek, EOS, gapless standby, crossfade + continuity, decline, short-track fade, pause-during-fade, gain, teardown, FLAC fixture), `tests/mpak_integration.rs` (MPAK member → backend → decoder → engine) | established (phase 11) |
| **Crossfade mixer differential** | `crates/musicpack-engine/tests/xfade_oracle.rs` replays `tests/data/xfade_oracle.jsonl` (from `tools/xfade_oracle.ts` through the real `MusicPackPcmProcessor`) — **byte-identical**, no tolerance | established (phase 11) |
| **Representation policy differential** | `tests/policy_oracle.rs` replays `tests/data/policy_oracle.jsonl` (from `tools/policy_oracle.ts` through the real Phase 4 TS resolver) — identical normalized preference and selection, 32 records, no tolerance | established (phase 12) |
| **Musepack differential** | `tests/musepack_oracle.rs` parses and decodes the six committed SV8 fixtures and requires the reference facts (stream version, sample rate, channels, playable length) **and the decoded PCM SHA-256** to agree exactly; `tests/data/musepack_oracle.jsonl` is generated by `tools/musepack_oracle.mjs` from the vendored libmpcdec (WASM). `tests/musepack.rs` adds chunk-equivalence, EOF and length checks | established (phase 13B, PCM byte-identical) |
| **Host integration** | `crates/musicpack-host/tests/host_integration.rs` (16 scenarios through the reusable `Host`: load/play, pause/resume, seek, EOS, underrun, gapless, crossfade, short outgoing/incoming, pause-during-fade, cancel-by-seek, stale completion, gain, teardown, MPAK-backed playback, representation selection → source → playback) | established (phase 12) |
| **WASM binding** | `crates/musicpack-wasm` native unit tests plus `tools/wasm_smoke.sh` → `crates/musicpack-wasm/tests/node_smoke.mjs` (Node, `wasm-bindgen --target nodejs`; no browser, no `wasm-pack`) | established (phase 11) |
| Fuzz targets | `fuzz/` (`json_manifest`, `mpak_scan`, `audio_decode`, `analysis_feed`, `player_state`, `engine_adapter`, `policy`, `musepack`), excluded from the workspace and type-checked in CI; run manually (`fuzz/README.md`) | established (not a CI gate; O16) |
| Property-based | `proptest` for path rules, quantization monotonicity, MPAK framing skip arithmetic | planned — deliberately deferred until the types it exercises exist |
| Fuzz | `cargo-fuzz` targets: manifest parser, MPAK scan, waveform payload | planned (the JSON layer is the highest-value target) |
| Benchmarks | `criterion` benches for parse/verify/hot paths | planned (phase 15; "no premature optimization") |

Ground rules: fixtures are inputs only; expectations live in test code;
golden bytes only where the reference itself pins bytes (canonical
manifests, MPAK determinism). Compatibility expectations are never
weakened to make Rust tests pass.

## 6. Dependency policy

Current dependencies: **`sha2` (RustCrypto)** and, since phase 8,
**`claxon` (pure-Rust FLAC decoder)**. The format layer's parsing surface is
security-sensitive, so each dependency is justified against the
compatibility constraints; nothing else is needed yet.

| Crate | Phase | Why appropriate | Caveats |
|---|---|---|---|
| `sha2` 0.10 (RustCrypto) | 5 | SHA-256 is format-mandated; `sha2` is the maintained pure-Rust reference implementation, `no_std`-capable, compiles on wasm32-unknown-unknown, MIT OR Apache-2.0, MSRV 1.56 ≪ this crate's 1.85. Digest output is pinned by FIPS vectors and cross-checked against the independent test-support SHA-256 | none significant for this use |
| `claxon` 0.4.3 | 8 | FLAC is an authoring input. The reference itself vendors a third-party decoder rather than writing one (`dr_flac`), and `claxon` is the Rust-native equivalent: **Apache-2.0**, **zero runtime dependencies** (`cargo tree` shows no transitive deps), std-only and wasm32-unknown-unknown-clean, with a streaming `Read` API. Pinned exactly (`=0.4.3`) and hidden behind `audio::flac::FlacDecoder`, so it never appears in the public API | dormant since 2020 (the FLAC format is frozen and the surface is small); contains **five internal `unsafe` blocks** (audited: two `get_unchecked` hot-loop indexing operations and three `Vec::set_len` after a `read_into`), all inside the dependency — the core's `#![forbid(unsafe_code)]` is unaffected and no unsafe enters `musicpack-core`; its metadata parsing is stricter than `dr_flac` (see §8 limitations) |
| `wasm-bindgen` + `js-sys` | 11 | the WASM binding *mechanism* itself; only `crates/musicpack-wasm` depends on them, never the core or the engine adapter (both stay dependency-free and wasm32-clean) | binding-only: no async runtime, no browser framework, no audio framework |
| `rusqlite` 0.40.2 (`bundled`) + `getrandom` 0.4.3 | 15 (server) | SQLite is the legacy server's index format; byte-compatibility with C-created databases requires a real SQLite. The amalgamation's C/`unsafe` is confined to the dependency inside the native-only `crates/musicpack-server` (behind its `Store` trait); the crate itself keeps `#![forbid(unsafe_code)]` and no other crate may depend on it. `getrandom` supplies the OS CSPRNG for token secrets | server-crate-only; the AGENTS.md rule is unchanged for every other crate |
| `proptest` / `cargo-fuzz` | dev | property testing and fuzzing are explicit requirements | dev-dependencies only |
| `criterion` | bench | statistically sound benchmarking | bench-only |

Phase 9 adds **no** dependency: the BS.1770 subset is a hand-port of the
reference's vendored **libebur128 1.2.6** (MIT — Copyright © 2011 Jan
Kokemüller), preserving attribution in `src/audio/loudness.rs`. A
third-party Rust DSP/loudness crate would still have to reproduce this exact
filter topology, block/gate order and interpolator to meet the differential
gate, while adding audit surface for no compatibility gain.

Phases 10–12 add **no** dependency to the core, to `musicpack-engine` or to
`musicpack-host`: the ring, streaming resampler and equal-power mixer are
hand-ports of the reference TypeScript (differentially gated byte-for-byte),
and the representation resolver is a hand-port of the Phase 4 TypeScript, so
a DSP or policy crate would still have to reproduce the reference's exact
semantics. Only the binding crate `crates/musicpack-wasm` adds
`wasm-bindgen`/`js-sys`.

Explicitly rejected: a JSON parser chosen for convenience (the hand-rolled
cJSON-compatible parser resolved O1), `libc` (the directory adapter uses
std-only `MetadataExt` plus an explicit `lstat` check, documenting the
resulting TOCTOU window rather than adding a dependency), any audio codec
*FFI binding* (a `dr_flac`/`libmusepack` binding would break
`#![forbid(unsafe_code)]` and the wasm32 build; FLAC uses the pure-Rust
`claxon`, and the Musepack decision remains open as O5), any async runtime,
any platform crate in the core. CRC-16 is 10 lines and stays hand-written;
SHA-256 is not hand-rolled.

## 7. Migration strategy

Adapted from the task brief after inspection; the dependency order
reflects what the reference actually layers:

1. **Repository/toolchain foundation** ✅
2. **Domain types (manifest model)** ✅
3. **`.mpack` format model + strict JSON parser** ✅ (resolved O1)
4. **Canonical writer + round-trip tests** ✅ (resolved O3)
5. **Validation/verification semantics** ✅ (storage seam, SHA-256,
   report model, budgets, containment; zero differential gaps)
6. **MPAK v1 container** ✅ (scan-oriented reader, deterministic writer,
   `PackageBackend` adapters, container verification findings; Rust packs
   are byte-identical to reference CLI packs and the reference CLI
   verifies/unpacks Rust containers)
7. **Compatibility-gate hardening + CLI** ✅ (hostile replay, deterministic
   fuzz-lite replay, cargo-fuzz targets, workspace + thin `musicpack` CLI
   with `info`/`verify`/`pack`/`unpack`; CLI differential green)
8. **Audio primitives** ✅ (PCM contract, decode seam trait, native WAV
   reader, `claxon`-backed FLAC adapter, fixture/differential/hostile tests,
   `audio_decode` fuzz target; FLAC PCM proven byte-identical to the
   reference's vendored `dr_flac`)
9. **Waveform accumulator + BS.1770-5 meter** ✅ (streaming accumulator,
   hand-ported ebur128 subset, `gain_db`, `analysis_probe.c` golden data;
   waveform byte-exact and loudness within ±0.05 vs the reference, zero
   regressions)
10. **Player core port** ✅ (types, synchronous engine seam, queue/order,
    gain, snapshot, transition planner, events, orchestrator; the TS unit
    suites ported to `tests/player.rs` and differentially replayed via
    `tools/player_oracle.ts` → `tests/data/player_oracle.jsonl`; zero
    regressions). The transition planner and crossfade logic from item 11
    are included here.
11. **Engine adapter (PCM mixing)** ✅ (the pure planner and crossfade/EOS
    interleavings were absorbed into phase 10; phase 11 adds
    `crates/musicpack-engine` — bounded ring, streaming linear resampler,
    equal-power mixer with the reference swap accounting, source/decoder
    factories, per-track decode sessions, and the host-driven `consume()`
    seam; ring/resampler/mixer are differentially byte-exact against the
    reference TypeScript and the Level-3 `Player` integration is green)
12. **Representation-selection policy** ✅ (`policy::representation`: a pure,
    total, I/O-free port of the Phase 4 TypeScript resolver; differentially
    replayed against `tools/policy_oracle.ts` → `tests/data/policy_oracle.jsonl`
    with 32 records, zero gaps). The host seam that consumes it
    (`crates/musicpack-host`: pull loop, shared engine handle, source
    selection) is proven by 16 host integration tests; the real device /
    AudioWorklet cutover remains staged.
13. **`musicpack-wasm` binding foundation** ✅ (thin byte-backed decode and
    player handles plus the representation resolver with `wasm-bindgen`; the
    Node smoke test runs without a browser or `wasm-pack`). The production
    browser/Svelte cutover — Web Audio, Media Session, HTTP/OPFS range
    fetching — remains future work in the TypeScript host.
14. Integration with the existing application — phase 14 investigated the
    server migration (`docs/server-migration.md`); **stages 1–6 are
    implemented** (`crates/musicpack-server`: CLI/config skeleton,
    byte-compatible SQLite layer, token management, collector identity,
    bounded discovery, full ingestion state machine with live C-oracle
    parity, read-only JSON API with live C-oracle parity, secure byte/media
    serving with live C-oracle parity, static hosting/SPA fallback and
    library jobs with live C-oracle parity; deployment/cutover are the
    remaining steps, gated by `docs/server-cutover-checklist.md`).
    The legacy server remains the production implementation until parity is
    proven stage by stage
15. Performance optimization (benchmarked, correctness-preserving)

Phases 3–7 are the compatibility heart: the format must be byte- and
behaviour-identical before any player/audio work begins.

## 8. Discrepancies discovered (spec vs reference implementation)

These were found by reading the C sources and by empirical differential
runs. In each case the **reference implementation is the behavioural
authority** and the Rust port follows it; the normative spec text is
documented as divergent rather than silently "corrected" in either
direction.

| # | Topic | Spec / docs say | Reference implementation does | Resolution |
|---|-------|-----------------|-------------------------------|------------|
| D1 | JSON nesting limit | `musicpack-v1.md` §8 and the reference README: "bounds JSON nesting (100)" | the vendored cJSON enforces `CJSON_NESTING_LIMIT` = **1000** (`cJSON.h`) | port follows the implementation: 1000 (`json::NESTING_LIMIT`) |
| D2 | Integral number printing | — (spec is silent on serialization numerics) | `json_number` prints integral doubles within ±9.2e18 as `%lld` integers; the `(long long)` cast happens **before** the range guard, so for integral doubles beyond i64 range the branch decision is **undefined behaviour** — the same bits produced different outputs at different call sites in a single process | Rust takes the `%.8g` branch deterministically outside ±9.2e18; those values cannot be differentially pinned and are excluded from the vector table (`tools/g8_probe.c` header documents this) |
| D3 | Number grammar | strict JSON | cJSON copies the charset `[0-9+-.eE]` (≤ 63 chars) and calls `strtod`: `01`, `1.`, `1e999`→∞ are accepted; `.5`/`+1` are rejected at the value gate | ported exactly in `json::parse` |
| D4 | Whitespace | JSON whitespace is space/tab/CR/LF | cJSON skips **any byte ≤ 0x20** between tokens and before the end check | ported; raw control bytes inside strings are also accepted (ported) |
| D5 | Strings with NUL | — | `\u0000` (or a raw NUL) truncates the decoded string at the C-string boundary; duplicate-key detection sees truncated keys | ported (`json` truncates decoded strings/keys at the first U+0000) |
| D6 | Fixture manifests | — | the *committed* reference `tests/reference/*/manifest.json` files are Python-generated with alphabetically sorted keys, i.e. **not** canonical-writer output; true canonical bytes come from the reference CLI (`update-metadata` rewrites through the writer) | byte-identity fixtures here are CLI-rewritten copies (`fixtures/reference/canonical-*.json`); the original files are kept for semantic tests |
| D7 | Conformance corpus size | reference README: "3 valid manifests, 49 invalid manifests, 9 invalid asset cases" (also stale: 42/8 elsewhere) | the generator produces **6 / 57 / 9** | trust the generator |
| D8 | Windows hardening | `musicpack-v1.md` §8 describes the untrusted-input model generally | the reference's Windows path uses `_stat` only: no `O_NOFOLLOW` symlink rejection, no hard-link rejection (`st_nlink`), no inode dedup — and it documents this | the Rust directory adapter implements the **POSIX** semantics and is `#[cfg(unix)]`; a Windows adapter is future work (O12) |
| D9 | MPAK `DATA` order | `mpak-v1.md` §7: "audio objects first …, then `representations[]`, `artwork[]`, `booklet[]`, `lyrics[]`, `extras[]`, `analysis[]`" (waveforms omitted) | the writer groups **all** audio, then **all** representations, then **all** waveforms, then artwork/booklet/lyrics/extras/analysis | the writer follows the implementation (required for byte-identity); documented in `canonical_pack_order` |
| D10 | MPAK `TAIL` optionality | `mpak-v1.md` §6 calls TAIL "optional (RECOMMENDED)" | the reference writer always emits it, and the Rust writer matches | writers always emit TAIL (byte-identity); readers treat absence as the specified warning |
| D11 | `WAVE_FORMAT_EXTENSIBLE` GUID | the Windows canonical sub-format GUIDs are `KSDATAFORMAT_SUBTYPE_PCM` `00000001-0000-0010-8000-00AA00389B71` / `_IEEE_FLOAT` `00000003-…` | `extensible_subformat()` requires the sub-format tag at `fmt+24..26` to be 1 or 3 and the **12 bytes at `fmt+28..40`** to equal `00 00 00 00 10 80 00 00 AA 00 38 9B`, leaving `fmt+26..28` unchecked and never validating `wValidBitsPerSample`/`dwChannelMask`; the committed `wav24-ext.wav` fixture carries exactly this non-canonical form | the Rust WAV reader ports the reference check byte-for-byte; the canonical Windows GUID is deliberately **rejected** in phase 8 (`audio::wav`; pinned by the `extensible_matrix` test), so `wav24-ext.wav` stays the compatibility fixture |
| D12 | Analysis NaN/Inf handling | — (specs are silent) | undefined/implicit: the C waveform quantizer's `float`→`uint8_t` cast is undefined for NaN, and a NaN true peak propagates into a measurement | Phase 9 sanitizes **at the analysis feed boundary only** (`audio::sanitize_sample`): NaN→0, +Inf→+1, −Inf→−1, finite out-of-range unchanged. The phase 8 decoder still preserves raw IEEE-754 bits |
| D13 | Waveform `Σs²` precision | `musicpack-waveform-v1.md` §5 says `long double` | `long double` is 80-bit on x86 but 64-bit on AArch64/Emscripten, so the reference result is itself platform-dependent | the Rust accumulator uses `f64` (documented numerical-portability deviation) and is required to match the committed reference payloads **byte-for-byte**; any divergence is investigated, never smoothed with a tolerance |
| D14 | Player engine seam | `engine.ts` is an asynchronous callback port (`Promise` + `on(name, cb)`) | the TypeScript orchestrator parks on `await engine.open/play/advance/beginCrossfade` and re-checks generations after each await | the Rust seam is **synchronous**; engine facts are injected through explicit `Player::on_*` methods and an in-flight crossfade completes through `Player::on_crossfade_complete`. The generation/epoch guards are retained for injected events; commands complete atomically |
| D15 | Player event delivery | events fan out synchronously through a subscription sink (`PlayerEventSink`) | subscribers run in emission order on the calling thread | mutating operations **return** `Vec<PlayerEvent>`; ordering is the same observable contract and no callback/event framework is introduced |
| D16 | Snapshot duplicate keys | `JSON.parse` keeps the last value for duplicate keys | a hand-written snapshot with duplicates silently last-wins | the Rust parser rejects duplicates, so `decode_snapshot` returns `None` (a safe no-restore). Well-formed snapshots are unaffected; number formatting is shortest-round-trip rather than `JSON.stringify`-identical |
| D17 | Adapter crossfade completion | the worklet is message-driven/asynchronous (`xfade-go` … `xcomplete`), and the TS engine `await`s it | the Rust adapter is synchronous: `begin_crossfade` primes the standby lane and returns `CrossfadeStart::Pending`; the host consumes frames until the swap completes, then takes the `CrossfadeResult` and calls `Player::on_crossfade_complete`. No futures/async enter the adapter (phase 11, D14's synchronous seam extended to PCM) |
| D18 | Adapter seek | the worklet `seekSample` resets the ring and re-primes the decoder at `resetBase` | no codec seek: the adapter reopens the source and skips N output frames, then rebases the ring playhead to zero so the player's reset offset carries the absolute position. Intentionally simple and possibly expensive; `AudioDecoder` is unchanged |
| D19 | Adapter channel handling | the pipeline duplicates mono→stereo and truncates excess channels at the resampler | identical arithmetic, but the adapter **rejects** source channel counts >2 at session open (mono/stereo only). No surround or downmix policy is invented |
| D20 | Representation identity type | the reference uses a JS `number` id and template-literal identity `t{track}r{rep}` | `policy::representation` stores ids as `f64` and compares by exact equality (faithful); ids are expected integral for [`policy::item_id`]. The wasm binding takes playability as a JSON predicate spec (`{codecs?, rejectMimes?, rejectIds?}`) rather than a JS callback, because a `#[wasm_bindgen]` function cannot invoke an arbitrary JS predicate per candidate. The resolver itself is unchanged |
| D21 | Musepack input model | libmpcdec streams the compressed file through a 64 KiB demux window | `audio::musepack` **streams** the same way (phase 14C): `MpcDecoder::from_reader` reads the header then one SV8 `AP` block at a time, buffering ≤ `MAX_BLOCK_BYTES` (`DEMUX_BUFFER_SIZE - 11`) — no whole-member read. Decoded PCM is byte-identical (verified at chunk sizes 1..=usize::MAX). `read_s32` returns `Unsupported` (Musepack is float-only, like IEEE-float WAV); seeking is the generic engine reopen+skip. Round-trip offset reads were verified in `tests/musepack_streaming.rs`; a random-access seek index remains future work |

Additionally, two reference behaviours are intentionally **not**
reproducible in Rust and are documented differences (not gaps):
raw **non-UTF-8 bytes** inside JSON strings parse in C but are rejected at
the byte→`String` boundary here (the whole input is UTF-8-validated first,
Open Question O4); and `NaN` rendering can never be reached through the
parser because `strtod` cannot produce NaN from the accepted grammar.

### Documented limitations

- **Containment is pathname-based, not descriptor-relative.** The
  reference resolves existing path prefixes with `realpath` and opens the
  resulting path; a concurrent renames race is inherent to that design and
  is documented by the reference project itself.
- **Final-symlink TOCTOU window.** The reference closes it with
  `O_NOFOLLOW` at open time; this port uses an explicit `lstat` rejection
  followed by `open` (std-only, no `libc`), so a path swapped from a
  regular file to a symlink *between* the two syscalls would be followed.
  The checks bound to the opened handle (regular file, `nlink`, size) still
  apply, and the observable classification is identical in the absence of
  a race. Adding `libc` solely for `O_NOFOLLOW` was judged not worth a
  dependency; the trade-off is recorded here.
- **Sonic documents are not validated.** See O11.
- **Extraction (`unpack`) is safe but not transactional in the findings
  sense.** Member paths are re-validated, written into a fresh staging
  tree with `create_new` (no overwrite, no symlink following, regular
  files only) and published with a single `rename`; a *hard* failure leaves
  no destination. When the recovery scan skips damaged members or a member
  checksum mismatches, extraction completes into the destination and exits
  non-zero (matching the reference's "extracted with errors" behaviour), so
  the destination may be incomplete — it is never outside the requested
  tree.
- **Extraction TOCTOU.** A concurrent attacker with write access inside the
  destination could win a check-then-open race during staging; the core and
  CLI follow the reference's pathname-based discipline rather than
  `openat`-relative traversal. Documented, not hidden.
- **The HTTP-range/OPFS byte source is not implemented here.** The
  `ByteSource` seam is exactly the reference's `mpak_cio` shape, so a
  range-backed source (with the design spec's block cache) plugs in
  without touching the reader; that transport is a later phase.
- **File-backed container sources assume immutability.** `FileSource`
  seeks and reads on demand; the reference documents that verified bytes
  must not be mutated concurrently, and the same contract applies here.
- **FLAC metadata strictness and ID3.** The `claxon` adapter parses FLAC
  metadata blocks that `dr_flac` merely skips, so a malformed
  Vorbis-comment/application block is rejected where the reference would
  decode the audio; `claxon` also does not skip an ID3v2 prefix, and
  `audio::open` sniffs the `fLaC` magic directly, so ID3-prefixed FLAC is
  not recognized (the reference selected by extension and let `dr_flac`
  skip ID3). For valid streams the decoded PCM is **byte-identical** to
  the reference decoder (differential digests in
  `tests/data/audio_c_reference.txt`).
- **`claxon` is dormant.** 0.4.3 (2020) is the current release. The FLAC
  format is frozen, the dependency is pinned and zero-dependency, and the
  adapter boundary means a replacement does not touch the public API; the
  risk is accepted and revisited only if a concrete decode gap appears.
- **Loudness is tolerant, not bit-exact.** The integrated-loudness/true-peak
  port matches the reference within ±0.05 LU / ±0.05 dBTP. The residual is
  libm (`tan`/`pow`/`log10`) and `long double` platform variance in the
  vendored ebur128 code; the algorithm structure is reproduced exactly. Any
  future tightening must be justified against the reference, not assumed.
- **Loudness history is unbounded by design.** The meter keeps one gating
  energy per 100 ms for the whole program (≈ 288 KiB/hour), like the
  reference's default history. It is not capped because the specification
  does not cap it; a caller analysing an unbounded stream should account for
  that growth. The waveform output is capped at 864 000 buckets.
- **Adapter seeking is reopen + skip.** `musicpack-engine` has no
  codec-specific seek; a seek drops the decoder, reopens the source and
  decodes/discards N output frames before rebasing the ring playhead. This
  is deliberately simple and portable and can be expensive for large seeks.
- **The adapter owns no device and no clock.** It exposes
  `consume(frames, dst)`; a host owns the real output path, the clock and any
  concurrency. Real-time safety is *not* claimed: `consume` allocates
  nothing per call beyond what the caller passes, but seek and crossfade
  priming do allocate (bounded by the ring).
- **WASM PCM crosses by value.** `decode_read`/`render` return a JS-owned
  `Float32Array` (one copy out of WASM memory). No zero-copy or transfer
  claim is made; the binding is correct and small first.
- **Musepack decoding is incremental (streaming demux).** `audio::musepack`
  reads the `MPCK` header and then one SV8 audio block at a time from its
  `Read`, buffering at most one block payload (`MAX_BLOCK_BYTES`, the
  reference's `DEMUX_BUFFER_SIZE - 11`), independent of the member size —
  `open` no longer consumes the whole compressed file. PCM is
  **byte-identical** to the vendored `libmpcdec` on the fixture corpus (six
  files, all sample rates and quality levels, verified across chunk sizes
  `1..=usize::MAX`), and seeking uses the generic engine reopen+skip path.
  Supported: SV8 only, 32/37.8/44.1/48 kHz, mono/stereo. SV7 and >2 channels
  are rejected.
- **The host seam is proven, not wired to a device.** `crates/musicpack-host`
  implements the pull loop a device/AudioWorklet callback would drive and is
  covered by 16 integration tests, but no native device backend is wired up
  here. On the web, the Rust WASM engine is now the *default* backend for
  network musepack/FLAC/WAV sources in the production client (with the
  reference TypeScript engines as the tested escape hatch — see the
  `rust-default` e2e spec and `controller.ts` backend selection); audio
  output itself remains platform/TS-owned per the playback ADR
  (`docs/adr/0003-playback-architecture.md`).
- **The host handle is single-threaded.** `musicpack-host` shares the engine
  through `Rc<RefCell<…>>` (browser main thread / one device callback). A
  device host that runs the callback on another thread owns its own
  synchronization; the deterministic engine stays lock-free and single-owner.
- **Representation preferences persist host-side.** The core policy exposes
  `AudioPreference::from_value`/`to_value` for the
  `musicpack.audio-preference.v1` payload, but the storage key, `localStorage`
  access and "future items only" application remain host concerns (matching
  the reference); the core never reads or writes a preference store.

## 9. Open questions

Unresolved questions are recorded here instead of being silently decided.

- **O4 — UTF-8 boundary (refined).** The JSON layer rejects raw non-UTF-8
  input (a documented difference from cJSON, which copies such bytes
  verbatim). Unpaired surrogates in `\u` escapes are rejected exactly like
  cJSON. Whether any *valid* package in the wild could contain non-UTF-8
  manifest bytes is an empirical question for phase 7; the conformance
  corpus contains none.
- **O5 — Codec strategy (FLAC resolved, Musepack open).** ✅ **FLAC:**
  phase 8 implements FLAC decode with the pure-Rust `claxon` crate behind
  `audio::flac::FlacDecoder` (see §6/§8), proven byte-identical to the
  reference's vendored `dr_flac`. **Musepack** SV7/SV8 decode (and
  eventually encode) is still required by later phases. Options: FFI to the
  C `libmusepack` (native adapters), reuse of the existing Emscripten build
  (web), or a Rust port. Licensing is permissive either way (BSD-3
  decoder), but the decoder is pinned bit-exact by golden fixtures — a port
  must reproduce decode output within the ±1-LSB tolerance the reference
  tests use. The decode seam trait now exists (`AudioDecoder`,
  `bits_per_sample == 0` reserved for codec-native float), so Musepack can
  join as another adapter without touching consumers. Phase 11 adds the
  `DecoderFactory` insertion point in `crates/musicpack-engine`: a Musepack
  `AudioDecoder` flows through the same `DecodeSession` → `DecoderEngine` →
  `Player` path with **no** player or adapter change (only the factory arm).
  **Phase 13B status:** the full decoder is implemented in
  `src/audio/musepack` (container/stream-info, bit reader, Huffman/CAN,
  requantisation, synthesis filterbank, SV8 packet framing), ported from the
  project's vendored `libmpcdec`, and registered in `audio::open`. Decoded PCM
  is **byte-identical** to the reference over the six-fixture corpus
  (`tests/data/musepack_oracle.jsonl`). SV7 and >2 channels are out of scope.
- **O6 — Loudness numeric equivalence.** ✅ **Resolved (phase 9):** the
  hand-ported ebur128 subset matches the reference within ±0.05 LU / ±0.05
  dBTP over the committed `tests/data/analysis_c_reference.txt` corpus
  (the project's existing regression tolerance), while waveform payloads are
  byte-exact. The residual is documented platform/libm variance (§8 D13 and
  the limitations list).
- **O7 — Player-core port shape.** ✅ **Resolved (phase 10):** a direct
  structural port of the TS orchestrator (§4 "Player domain"), with
  generations preserved, a **synchronous** `Engine` trait (no async
  runtime), explicit `on_*` event injections, and `Vec<PlayerEvent>`
  returns instead of a callback sink. Host adapters that wrap genuinely
  asynchronous platform engines drive the trait from their own event loop.
  The TS unit suites are ported directly and differentially replayed;
  see §8 D14–D16 and `src/player/`.
- **O8 — Workspace layout.** ✅ **Resolved:** the root is both the core
  package and the workspace root; the CLI is `crates/musicpack` and
  `fuzz/` is a cargo-fuzz crate excluded from the workspace. The core
  stays usable on its own (`cargo add musicpack-core` / a path
  dependency); the CLI depends on it, never the reverse.
- **O9 — Publishing.** Whether `musicpack-core` is published to
  crates.io, and its MSRV policy beyond "stable Rust, edition 2024".
- **O11 — Sonic document validation.** The reference's `verify` parses and
  semantically validates `analysis[]` documents of type `sonic`
  (`musicpack-sonic-v1.md`, `sonic.c`): malformed documents, profile
  mismatches and validation failures are errors; unknown/research-only
  profiles are warnings. The Rust verifier currently verifies a sonic
  document only as a generic SHA-256-protected asset. No conformance
  corpus case exercises a sonic document, and the differential run is
  unaffected; implement with the sonic domain in a dedicated phase.
- **O12 — Windows directory adapter.** The unix adapter cannot be used on
  Windows, and the reference's Windows checks are genuinely weaker (D8).
  Either port the weaker Windows behaviour or (preferred) implement the
  stronger POSIX checks with Windows equivalents
  (`GetFinalPathNameByHandle`, reparse-point handling, link-count checks
  where available). Until then the differential verification layer is
  unix-only. (The MPAK layer itself is portable: `ByteSource`,
  `FileSource`, `MemorySource` and the writer compile and run on every
  target.)
- **O13 — Report message wording levels.** Messages are ported verbatim
  (including `%g` duration formatting at precision 6). If a future CLI
  localizes output, the compatibility surface moves to a structured
  finding type; no structured `code` field exists in the reference.
- **O14 — MPAK byte sources beyond files/memory.** The `ByteSource` seam
  is in place; the HTTP-range transport (with the design spec's 64 KiB
  block cache) and a browser/OPFS source are later-phase adapters. Until
  they exist, remote containers are out of scope.
- **O15 — `unpack`/extraction surface.** ✅ **Resolved:** extraction is
  implemented in the CLI (`crates/musicpack`) with a documented safety
  model (staging + rename, canonical path re-validation, `create_new`,
  regular files only, aggregate budget); the core gained no filesystem
  responsibilities. A future `musicpack-wasm` may offer an in-memory
  variant of the same policy.
- **O16 — Statistical fuzzing cadence.** Fuzz targets exist
  (`fuzz/`: `json_manifest`, `mpak_scan`, `audio_decode`, `analysis_feed`,
  `player_state`, `engine_adapter`) and are type-checked in CI, but
  fuzzing itself is not a CI gate. Decide whether to run a bounded
  `-max_total_time` fuzz job on a schedule and how crash artifacts enter
  the deterministic regression corpus.
- **O17 — Device output and the browser cutover.** The engine adapter is
  deterministic and device-free by design; `crates/musicpack-host` now proves
  the complete host seam (source → decoder → engine → `consume` → host
  output, plus representation selection and MPAK playback) by integration
  tests. What remains a host integration, not a core/adapter change: a native
  output backend (an audio-device callback pulling `Host::render`) and the
  browser cutover (AudioWorklet rendering, Media Session, HTTP/OPFS range
  sources). Referenced in
  (`crates/musicpack-host/src/host.rs`). The boundary is settled (O14, §2);
  the implementations are future phases.

Resolved questions (kept for the record, with their resolutions):

- ~~**O1 — Strict JSON strategy.**~~ **Resolved: hand-rolled
  cJSON-compatible parser** (`src/json.rs`). serde_json cannot observe
  duplicate keys (silent last-wins) or preserve member order without
  non-std features, and cannot reproduce cJSON's number grammar
  (charset+strtod semantics), NUL truncation, BOM skipping, or the ≤0x20
  whitespace rule. The parser is ~450 lines, dependency-free, and its
  accept/reject behaviour is exercised by the full 72-case corpus plus
  focused quirk tests. See D1–D5 for the exact behaviours ported.
- ~~**O3 — `%.8g` canonical number formatting.**~~ **Resolved:** exact
  port in `src/format/number.rs` (bignum-based exact decimal expansion,
  round-half-even at 8 significant digits, C `%g` style selection),
  proven byte-identical against 3394 vectors generated from the real
  `snprintf` on this platform and against reference-CLI-rewritten
  manifests. The reference's own UB for integral doubles beyond ±9.2e18
  is documented as D2.
- ~~**O2 — SHA-256 crate confirmation.**~~ **Resolved:** `sha2` 0.10 is a
  dependency (see §6); digest output is pinned by FIPS vectors and
  cross-checked against the independent test-support implementation.

## 10. License

BSD-3-Clause (repository `LICENSE`), matching the MusicPack ecosystem
components in the reference repository. Code ported from the C reference
(`path.c`, `mpak.c` CRC routine, `waveform.c` quantization) derives from
BSD-3-Clause sources and remains BSD-3-Clause with attribution noted in
module documentation.
