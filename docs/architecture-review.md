# MusicPack architecture review — checkpoint after server stage 5

*Date: 2026-09-20. Scope: whole project (Rust workspace, Web Player, Author,
future native clients). Status: adopted as the working architecture plan;
durable decisions are recorded as ADRs under `docs/adr/`.*

**Ground rules observed by this review.** The legacy repository
(`../musicpack`) is the immutable behavioural oracle: nothing in it was
modified, and no recommendation below depends on changing it. Every claim
about current structure cites the actual code. This document recommends; it
does not re-implement. The only code-adjacent corrections made alongside it
are documentation corrections (§14 and the stale statements fixed in
`docs/architecture.md`).

---

## 1. Executive summary

The MusicPack architecture is in unusually good shape for its age. The
decision to make the Rust workspace a **portable domain core first** —
format, validation, codecs, analysis, and even the playback domain
(`core::player`) with oracle ties back to the TypeScript production player —
means the hardest architectural problem (reusing semantics across Web,
server, and future native platforms) is already largely solved rather than
pending. The server now has five stages of live-C-oracle parity behind it,
and its `store → media → http` layering held up under byte-serving without
structural change.

The review reaches four headline conclusions:

1. **The Rust architecture is right. Keep it.** One core crate, trait-seamed
   storage and backends, std-only HTTP transport, one scoped C dependency
   (SQLite) in the native-only server crate. No service-layer ceremony, no
   framework, no crate splits are warranted today; this review defines the
   explicit triggers that would change each of those (ADRs 0001, 0002).
2. **The biggest structural risk is not in the Rust — it is that the
   production Web Player and the Author app still live in the immutable
   legacy repository.** The web build already reaches across the repo
   boundary (`web/scripts/sync-wasm.sh` copies artifacts from
   `../musicpack-core`), API and playback changes cannot be atomic across
   repos, and LLM agents must navigate two repos to change one feature.
   Recommendation: migrate `web/` and `author/` into this workspace at
   server cutover (server-migration stage 9), keeping the legacy copies
   frozen as oracles — exactly the pattern already used for the C server
   (ADR 0005).
3. **The playback boundary is already the correct one: share deterministic
   domain and audio sequencing in Rust; own audio output and Media Session
   integration per platform.** The Rust engine is sample-accurate-gapless
   capable and crossfade-oracle-tested against the production TypeScript
   worklet; that asset should be the audio core on iOS and Android too,
   rendered through thin platform output adapters, with fully native UI and
   platform media sessions for CarPlay / Android Auto (ADR 0003, 0006).
4. **Svelte stays.** The Player and Author share a mature design language
   (tokens, dark-v2 theme), the domain layer is deliberately
   UI-framework-free, and the test pyramid (30 vitest unit files, 25+
   Playwright specs) is tied to the existing app. Rewriting would destroy
   oracle value and gain nothing that matters to MusicPack's requirements
   (ADR 0004).

The recommended next phases, in order: finish the server (static hosting,
jobs/ops, cutover), migrate web + author into this repo, then Player
features (lyrics, offline hardening, keep-alive measurement), then the
Author's Rust pipeline, and only then the native FFI spike. §20 gives the
full roadmap with acceptance criteria.

---

## 2. Current architecture map

### 2.1 Rust workspace (musicpack-core repository)

From the root `Cargo.toml` (lines 18–20) and each crate's manifest —
dependency edges verified, not inferred:

```text
                    ┌──────────────────────────────┐
                    │        musicpack-core        │   BSD-3, wasm-clean,
                    │  format · storage · audio    │   ~15.5k LOC, deps:
                    │  validation · player · policy│   sha2, claxon only
                    └──────────────┬───────────────┘
        ┌──────────┬───────────┬───┴───────┬─────────────┬────────────┐
        │          │           │           │             │            │
  musicpack    musicpack-  musicpack-  musicpack-   musicpack-   musicpack-
  (CLI)        engine      host        wasm         server       mpc-tools
  native       std PCM     render loop wasm-bindgen native-only  LGPL-2.1
               adapter                 (cdylib)     rusqlite     │
                                        │            (only C dep) │
                                        │                         │
                                        │              musicpack-musepack-encoder
                                        │              LGPL-2.1, zero prod deps
                                        └─ nothing depends on musicpack-server
```

| Crate | LOC (≈) | License | Depends on | Target posture |
|---|---|---|---|---|
| `musicpack-core` (repo root) | 15,491 | BSD-3-Clause | sha2, claxon | wasm32-checked in CI |
| `crates/musicpack` (CLI) | small | BSD-3-Clause | core | native binary |
| `crates/musicpack-engine` | 1,899 | BSD-3-Clause | core | wasm-clean |
| `crates/musicpack-host` | ~370 | BSD-3-Clause | core, engine | host-side pull loop |
| `crates/musicpack-wasm` | 1,514 | BSD-3-Clause | core, engine, wasm-bindgen | the only wasm-bindgen surface |
| `crates/musicpack-server` | ~8,650 | BSD-3-Clause | core, rusqlite(bundled), getrandom | native-only, never a wasm target |
| `crates/musicpack-musepack-encoder` | ~19,600 | **LGPL-2.1-or-later** | none (prod) | wasm-clean by design |
| `crates/musicpack-mpc-tools` | ~310 | **LGPL-2.1-or-later** | core, encoder | native tooling |

Every crate carries `#![forbid(unsafe_code)]`; the only C/`unsafe` in the
tree is inside the `rusqlite` bundled-SQLite dependency, confined to the
server crate behind the `Store` trait. `fuzz/` is a separate workspace with
nine fuzz targets, compile-checked in CI but never a merge gate.

### 2.2 core module map (with the load-bearing seams)

- `format/` — manifest model + strict parser + **canonical writer**
  (`format/manifest/{mod,parse,write}.rs`, 719/865/379), MPAK v1 container
  reader/writer with CRC-before-length and deterministic pack order
  (`format/mpak/`, read 693 / write 313), canonical path rules
  (`format/path.rs`), checksums, exact C `%.8g` number printer
  (`format/number.rs`), waveform envelope constants
  (`format/waveform.rs`). Ordered JSON parser/printer compatible with the
  reference (`json.rs`, 731).
- `storage/` — the filesystem seam: `trait PackageBackend`
  (`storage/mod.rs:86`) with directory backend (`storage/directory.rs`,
  `#[cfg(unix)]`) and MPAK-member backend (`storage/mpak.rs`). This trait
  is *why* core never touches `std::fs` in parse/decode paths
  (`docs/architecture.md:72-77`).
- `validation/` — the `verify` semantics as an ordered `Report` of
  `Finding`s (725 LOC).
- `audio/` — `trait AudioDecoder` (`audio/mod.rs:196`), magic-sniffing
  `open()` (line 226), WAV reader, FLAC via `claxon` adapter, full Musepack
  SV8 decoder (byte-exact libmpcdec port, `decoder.rs` 1,329), BS.1770-5
  loudness (637), streaming waveform accumulator (375).
- **`player/` — the playback domain, portable and deterministic**:
  `Player` orchestrator (1,564), `Engine` trait + capabilities
  (`player/engine.rs:78`, incl. `sample_accurate_gapless`,
  `crossfade`), Sweet-Fades `TransitionPlan` (`player/transition.rs`),
  `QueueModel` + injected RNG (`player/queue.rs`, `order.rs`), −16 LUFS
  gain policy (`player/gain.rs`), versioned session snapshot codec
  (`player/snapshot.rs`), events-as-values (`player/events.rs`). Purity
  laws are documented in `player/mod.rs:9-19` (no ambient time, no
  randomness, no I/O).
- `policy/representation.rs` (567) — the pure, total
  representation-selection resolver.

### 2.3 engine / host / wasm playback stack

- `musicpack-engine`: `DecoderEngine` implements core's `Engine` trait
  (`engine.rs:290`); `DecoderFactory` and `SourceBackend` seams
  (`decoder.rs:18`, `source.rs:29`); ring buffer (255), streaming resampler
  (324), equal-power `Mixer` (167), `DecodeSession` with watermarks (313).
  Gapless handoff via `prepare_next`/`advance`; crossfade via a second
  session lane through the mixer. **There is no audio-output trait — the
  engine is pull-based** (`DecoderEngine::consume(frames, dst)`).
- `musicpack-host`: deliberately thin `struct Host` render loop
  (`host.rs:93-189`) + source selection (`selection.rs`). No device is
  wired; this is the proven seam (`docs/architecture.md:151-153`).
- `musicpack-wasm`: two layers — plain-Rust `core_impl` (natively
  unit-tested) and a thin `#[cfg(target_arch = "wasm32")]` bindgen surface
  (`src/lib.rs:995`). All async (fetch, OPFS, AudioContext) stays in JS
  (`lib.rs:14-16`).
- Cross-implementation oracle: `crates/musicpack-engine/tests/
  xfade_oracle.jsonl` replays output generated by the *real production TS*
  `MusicPackPcmProcessor` — the Rust and TypeScript engines are pinned to
  identical crossfade PCM.

### 2.4 Server (`crates/musicpack-server`)

Call structure (traced, not assumed): `main.rs` → `cli::run_serve` →
`SqliteStore::open` (WAL, busy-timeout, migrations 1–10 verbatim) → startup
scan unless `--no-scan` → `Arc<routes::Context>` (config + `Mutex<Store>` +
jobs snapshot) → `http::serve` accept loop, **one thread per connection**
(`http/mod.rs:57`) → `request::parse` (bounded: 32 KiB head, 4 KiB body,
2 KiB path, no chunked) → `routes::dispatch`:

1. CORS classification (deny-by-default, preflight support) → 403/preflight;
2. method gate 405; path-length 400;
3. public routes: `/health`, `/session`;
4. **single auth gate** (`routes.rs:169-186`): Bearer → cookie fallback,
   hash lookup, one 401 envelope;
5. route table → handlers.

JSON handlers: short store lock → `store.<read>` → ordered-JSON response
(`http/json.rs` — C-exact escaping and `%.10g` doubles). Media handlers
(`GET|HEAD …/audio|waveform|representations/{rid}/audio`, `/assets/{id}`):
short store lock → `MediaRef` resolver (4 methods on the trait) → **lock
dropped** → `media::open` (serveability gate, canonical path re-validation,
existing-ancestor containment, final-component NOFOLLOW + regular-file +
nlink==1, magic-byte inline safety, C `mime[48]` truncation emulation) →
`http::serve::serve_media` decision tree (If-None-Match 304 → If-Range →
Range → 200/206/416) → `Body::FileRange` streamed in 64 KiB chunks.

The `Store` trait has 41 methods across schema/tokens/sessions/ingest/
library-reads/media-refs (`store/mod.rs:337-566`); SQL lives only in
`store/` (`sqlite.rs` 1,852; `read.rs` 1,138; `schema.rs` verbatim C
migrations). Layering audit results: `store/` never imports `http/` or
`media/`; `http/` contains no SQL and no filesystem paths; the single seam
leak found is `http/routes.rs` naming the concrete `SqliteStore` for two
static helpers (`escape_like` at 465/573, `parse_genres` at 547).

### 2.5 Web Player (legacy repo, `web/` — production)

- **Stack**: Svelte 5.20 + Vite 6 + TypeScript strict, **zero runtime
  dependencies**; hand-rolled History-API router (`app/src/lib/router.ts:30-40`
  — 8 routes); hand-rolled Svelte-compatible store primitives in plain TS
  (`app/src/lib/store.ts:7-45`, "plain TS so the core logic is
  unit-testable without the Svelte runtime").
- **Playback**: transport/queue/persistence semantics live in the
  framework-free `player-core` TS package (`player.ts` 1,361 — state
  machine, preload/standby gapless, snapshot persistence;
  `transition.ts` — Sweet-Fades policy with exact constants; `engine.ts`
  — `PlayerPorts`/engine capabilities). `controller.ts` (432) is a
  composition facade; `chooseBackend()` defaults to the **Rust WASM
  engine** for network musepack/FLAC/WAV, with the legacy TS engine as
  escape hatch (`controller.ts:168-175`; pinned by 11 `rust-*.spec.ts` e2e
  specs). Audio output is platform-owned: AudioWorklet sinks; the Rust
  path renders in a worker with a dumb PCM sink
  (`rust-playback-sink.js` — "must never implement crossfade policy").
  Media Session wired via a `MediaControlsPort`
  (`playback/media-session.ts`, 71 lines).
- **Byte transport**: `networker.js` + `reader_mailbox.js` — 64 KiB
  block-aligned Range reads, `DATA_CAP` 256 KiB, requires 206 + exact
  Content-Range (`reader_mailbox.js:42-56`), Bearer auth.
- **Offline**: OPFS byte store + IndexedDB catalog with sha-verified
  installer/planner/audit (`app/src/lib/offline/*`, 10 files); service
  worker is static-shell-only and never intercepts `/api/**`
  (`app/public/sw.js:63-68`); PWA manifest (standalone, dark).
- **Design system**: single `theme.css` (2,303 lines) with design tokens
  (`:root` lines 15–66 — surfaces, warm-cream text, single gold accent,
  type scale, spacing scale, one allowed shadow), inline SVG icons,
  dark-only, breakpoints at 1024/680 px, `prefers-reduced-motion`
  kill-switch.
- **Tests**: 30 vitest unit files (node env) incl. a purity gate over
  player-core; 25 Playwright e2e specs (workers=1) covering collection,
  playback incl. gapless boundary, offline lifecycle, representations,
  the 11-spec Rust-backend program, waveform seek; **no visual regression
  tooling**.
- **Largest components**: `AlbumPage.svelte` 568, `TrackPage.svelte` 563,
  `WaveformSeek.svelte` 304.

### 2.6 Author (legacy repo, `author/` — Tauri 2 desktop app)

- **Svelte 5 frontend** (15 components: AlbumAuthoring, ReleaseForm,
  TrackList, EncodePanel, WaveformPanel, ValidationPanel, ArtworkManager,
  IdentityPanel, SonicPanel, WorkflowNav, …) mirroring the same dark-v2
  theme.
- **Rust side (`src-tauri`)**: `author_service.rs` wraps the legacy
  `musicpack` CLI through structured JSON (`inspect --json`,
  `validate-draft`, `encode-draft`, `waveform-draft`, `build-draft`,
  `identify-draft`, `pack`); `musicbrainz.rs` (identify lookups),
  `sonic_model.rs`; ships the legacy `mpcenc` as a **sidecar binary**.
- Tests: 3 unit + 12 component (vitest + jsdom).
- The Author workflow therefore currently depends on the legacy C toolchain
  at runtime, even though the Rust workspace already contains bit-exact
  replacements for its encode/waveform/validate/pack pieces.

### 2.7 Testing and oracle strategy (the project's core competency)

- Committed-artifact oracles everywhere: `cargo test` never invokes C.
  Golden vectors recorded from the reference (identity 20 vectors,
  `g8_c_reference.txt`, analysis reference, xfade `jsonl`, encoder
  manifests 94 inputs / 282 hashes, cut corpus 12 cases, C-created
  `library-c-reference.db`).
- Live C-server oracles for the server opt-in via
  `MUSICPACK_LEGACY_SERVER=<binary>`; Rust-side assertions always run.
  Server totals: 142 tests (74 lib, 10 api_oracle, 14 media_oracle,
  13 ingest_oracle, 7 db_compat, 7 identity, 12 discovery, 6 cli_token,
  …); workspace total 714 passing across 63 suites at the time of this
  review.
- Fuzz targets for every parser and for state machines (`json_manifest`,
  `mpak_scan`, `audio_decode`, `analysis_feed`, `player_state`,
  `engine_adapter`, `policy`, `musepack`, `server_identity`).

---

## 3. Current strengths

1. **The portability bet already paid off.** Because the domain (including
   the *playback* domain) was ported to Rust with TS-parity oracles, the
   "share semantics, not UI" strategy for native clients is a continuation,
   not a pivot. The xfade oracle and the 107-test player suite are the
   proof.
2. **Boundary discipline is real and verified.** One-way encoder
   isolation, server isolation (nothing depends on it), `store/` never
   importing `http/`, rusqlite confined to one native-only crate, no
   `std::fs` in core parsing paths — all confirmed by dependency inspection
   in §2, not just by docs.
3. **Oracle testing culture** is the project's defining strength:
   byte-exact compat is executable, hermetic by default, and 714 tests
   strong. This is precisely what makes future evolution safe for LLM
   agents: contracts are pinned by tests, not by convention.
4. **The server layering survived contact with its hardest requirement**
   (byte serving): no layer was violated to ship 206/304/416, streaming,
   and filesystem security. That is strong evidence the layering is the
   right shape, not decoration.
5. **Explicit-compat documentation**: `docs/architecture.md` records
   binding constraints and even *spec-vs-reference discrepancies* as data
   (D1–D21), and `docs/server-migration.md` records decisions (D-S1…S5)
   and open questions with statuses. Rare and valuable.
6. **Small, boring technology choices** (std-only HTTP, hand-rolled JSON
   for C-exact bytes, zero-dep web app, two prod deps in core) keep the
   audit surface tiny — a direct maintainability win for humans and LLMs.
7. **Design language already exists**: tokens, type/spacing scales, one
   accent, motion rules, responsive breakpoints — shared by Player and
   Author. A design system is a refactor away, not a creation away.

## 4. Current weaknesses

1. **Production web + author live in the immutable legacy repo.** The web
   build already reaches into `../musicpack-core` (`sync-wasm.sh:28-41`);
   every cross-layer change (API, engine, player) spans two repos with no
   atomic commit or single CI; the legacy repo cannot receive non-legacy
   evolution by our own rules. This is the largest structural debt.
2. **Stale statements in our own docs.** `docs/architecture.md` still said
   server "stages 1–4" in §7 (fixed alongside this review), and its
   documented limitation "the reference TypeScript engines remain the
   production browser implementation" no longer matches the web client,
   which now defaults to the Rust backend for network musepack/FLAC/WAV
   (`controller.ts:168-175`, `rust-default.spec.ts`) — also fixed
   alongside this review. `docs/architecture.md` nowhere mentions the
   encoder/mpc-tools crates or the LGPL boundary (that knowledge lives
   only in root `AGENTS.md`).
3. **God files in the server store and two god components in the Player.**
   `store/sqlite.rs` 1,852 lines (trait impl + entire write-side
   collector), `store/read.rs` 1,138, `http/routes.rs` 892;
   `AlbumPage.svelte` 568 / `TrackPage.svelte` 563. None is urgent; all
   are at the edge of comfortable LLM context and review size.
4. **One seam leak**: `routes.rs` names the concrete `SqliteStore` for two
   static helpers (`escape_like`, `parse_genres`) instead of a seam type.
5. **Spec drift, discovered by implementation**: the legacy API spec claims
   waveform `Range` is "intentionally not supported"
   (`specs/musicpack-api-v1.md:365-373`,
   `specs/musicpack-waveform-v1.md:373-374`), while the C server — the
   declared behavioural authority — runs waveforms through the identical
   range path as audio (`server/src/api.c:557-619`, `1340-1361`), and our
   stage-5 oracle pins that. The legacy specs are immutable, so the
   correction is recorded as an erratum in this repo
   (`docs/api-spec-errata.md`). The *process* weakness: nothing told us
   earlier; oracle tests are the only drift detector.
6. **UX knowledge is implicit.** The design system lives in one 2,303-line
   CSS file plus one legacy design doc; there are no canonical screen
   specifications and no visual regression tests. Future agents (or
   humans) will re-invent UX per screen — the exact failure mode this
   review is asked to prevent.
7. **Test-harness duplication across server oracle files** (fixture
   drop-guards, raw HTTP clients, port helpers re-implemented per file).
   Accepted as convention today; will grow with each new oracle file.
8. **`Connection: close` transport** means the web block reader opens a
   TCP connection per 64 KiB block. Fine on LAN, unmeasured on
   high-latency links. (The legacy MHD server supports keep-alive; this is
   a known, deliberate transport divergence from stage 4.)
9. **Minor**: `ingest.rs` repeats the invalid-manifest touch transaction
   twice (471–513, 519–547); `cli.rs` repeats the store-open block three
   times; the visible-gate predicate is written in two styles inside
   `read.rs`. Cosmetic, but they are the kind of drift LLM agents amplify.

## 5. Rust architecture assessment

### 5.1 What belongs in core — verdict on the current contents

Core today contains: package model, manifest (+ canonical serialization),
MPAK container, path/checksum/number primitives, storage backends (trait),
validation, audio decoding, waveform/loudness analysis, **the playback
domain**, representation policy. Judged against the checklist in the
review brief:

| Concern | In core today? | Verdict |
|---|---|---|
| Package model / manifest / canonical serialization | yes | correct; it is the compatibility surface |
| Package reading/validation | yes (`storage`, `validation`) | correct |
| Package writing | manifest + MPAK writers yes; directory-bundle **builder** no | correct gap to close *when Author work starts* (§11) |
| Identity (fingerprint/group/release keys) | no — server crate | correct today (O-S4); promote to core when the Author's Rust pipeline needs it (§11) |
| Audio metadata / representations | yes (`audio::AudioInfo`, `policy::representation`) | correct |
| Waveform / loudness | yes | correct; byte-exact vs C |
| Lyrics | **nothing anywhere** (no TS either; server serves lyrics *assets* only) | acceptable; define a v1 synced-lyrics format when Player lyrics work is scheduled (deferred decision, §17) |
| Playback domain concepts | yes (`player/`) | correct and unusually far-sighted; keep |
| Deterministic behavior | yes (purity laws, injected RNG/time) | correct |

What must **never** enter core: HTTP, auth, sessions, SQL, library/ingest
projection, offline download orchestration, MB lookups, draft/authoring
workflow, UI state. The existing boundary statements
(`docs/architecture.md:69-77`, `core/src/lib.rs:41-46` non-goals) already
say this; they are holding.

### 5.2 Should core be split?

No — not now. One crate keeps the compatibility story (oracles, fixtures,
docs) in one place, and no consumer needs a subset. Explicit triggers that
would justify a split (recorded in ADR 0002): (a) a consumer needs
format-only without the audio/player code (e.g., a tiny FFI surface where
claxon's compile cost matters); (b) wasm binary size forces tree-shaking
by crate; (c) compile times become a measured bottleneck. Split along the
existing module seams (`format` / `audio` / `player`) if ever triggered —
the seams are already clean.

### 5.3 Genuinely missing abstractions (and ones to *not* add)

Missing and eventually needed:
- **Directory-package builder** (create/modify `.mpack` dir with canonical
  layout + integrity): sibling of `verify_directory`; belongs in core when
  Author phase starts. The MPAK writer (`format/mpak/write.rs`) proves the
  pattern.
- **HTTP-range / OPFS byte sources** behind the existing `ByteSource` seam
  (open question O14): only when a Rust-side consumer needs to read MPAK
  over the network. The web currently does range-reading in JS
  (`networker.js`), which is fine; do not port it speculatively.
- **Portable snapshot *spec* document**: `player/snapshot.rs` implements
  v1/v2 codecs; the format deserves a `specs/`-style document when native
  clients approach, since it becomes the cross-platform saved-state
  contract (ADR 0003).

Not missing, do not add: service/DI frameworks, async in core/engine, a
plugin system, generic "repository" abstractions over `PackageBackend`,
error crates. The review explicitly endorses the current rejection list in
`docs/architecture.md` §6.

## 6. Server architecture assessment

**Verdict: `store → media → http` is sufficient for long-term growth; do
not introduce a distinct application/service layer now.**

Reasoning from the code:
- The handlers *are* the application layer, and they are thin: parse ids,
  lock store briefly, call one trait method, shape JSON. There is no
  business logic marooned in HTTP that a second front-end would need
  — the only other entry points (CLI `scan|verify|token`) already share
  the real logic through `ingest` and the `Store` trait, which is exactly
  what a service layer would provide. Creating one would add a pass-through
  layer with a single caller per method.
- `ingest.rs` is the scan/verify service and is already front-end-agnostic.
- The stage-5 test proved the model under the hardest case: byte serving
  with filesystem security and streaming worked *without* touching the
  layering.

What to do instead (small, concrete):
1. Fix the `SqliteStore` leak: move `escape_like` / `parse_genres` onto
   the `Store` trait (or a seam module) so `http/` never names a concrete
   backend (§4.4). Next time `routes.rs` is touched — not before.
2. When stage 8 (jobs) arrives, keep the existing `JobSnapshot` pattern and
   put the job runner in its own module (e.g. `jobs.rs`) that owns the
   scan mutex; handlers stay thin readers of the snapshot. That is the
   natural extension of the current shape, not a new layer.
3. Split `store/sqlite.rs` along its internal seam (token/session CRUD |
   collector/write side | delegation) when next substantively modified —
   1,852 lines is past comfortable review size. Same eventually for
   `read.rs` (reads | auth SQL).
4. Extract the duplicated oracle-test harness (fixture guard, raw client,
   port helpers) into `tests/support/` when the third oracle file clones
   them again — the convention is one shared harness per *kind* of oracle,
   not a framework.

Database and filesystem abstractions: **keep**. The `Store` trait (41
methods, single SQL impl) plus the C-verbatim migration table is the right
compatibility device; additive v11+ migrations are already sanctioned
(D-S2). The filesystem seam is `PackageBackend` in core and `media.rs` in
the server; both are narrow and tested. No change.

## 7. HTTP assessment

**Verdict: retain the hand-rolled std-only blocking HTTP/1.1 transport. It
is appropriate, and the oracle evidence supports keeping it.** (ADR 0001.)

Evidence-based assessment against the brief's criteria:

- **Performance requirements**: single-user/self-hosted deployment, LAN
  latency. Thread-per-connection with `Connection: close` handled the
  8-thread mixed media+JSON concurrency test in `media_oracle.rs` without
  wedging or cross-talk. Media serving streams in bounded 64 KiB chunks;
  memory is independent of file size. There is no requirement today that
  this design fails.
- **Streaming / Range / concurrency**: proven by `media_oracle.rs` (14
  tests) including the web client's exact 64 KiB block-reader contract.
- **Simplicity / security**: ~1,400 lines for the entire transport +
  request/response layer; request limits enforced at parse; no dependency
  risk; every byte of behavior is oracle-pinned. A framework would *add*
  unpinned behavior, not remove complexity.
- **HTTP/2 / HTTP/3**: irrelevant for direct LAN use; irrelevant behind a
  reverse proxy (the documented deployment terminates TLS and can speak
  HTTP/2 to clients while proxying HTTP/1.1). The legacy deployment doc
  already standardizes on a proxy with Range preservation.
- **WebSocket/SSE**: no requirement. Job progress (stage 8) is correctly
  served by the existing polling endpoint (`library/status`). Revisit only
  if a feature genuinely needs push; SSE is the fallback shape (one more
  response type, no protocol change).

**Documented migration triggers** (any one justifies revisiting, none
exists today):
1. A feature needs server-initiated push (live scan progress streams,
   multi-client sync).
2. Measured evidence that per-block reconnection hurts real clients on
   high-latency links → first enhancement is **HTTP keep-alive**, still
   std-only, additive, and already a deliberate divergence from the C
   transport (which keeps connections alive) — so it does not break the
   oracle's semantics, only its socket behavior (which the oracle already
   treats as per-request).
3. TLS must be terminated in-process (recommendation remains: use a proxy).
4. A public, multi-tenant deployment appears (out of scope by charter).

## 8. Playback architecture

### 8.1 What already exists (ratified, not proposed)

MusicPack already has the four-layer playback architecture the brief asks
us to design, implemented twice with oracle ties:

| Layer | Rust | TypeScript (production, legacy repo) | Oracle between them |
|---|---|---|---|
| Playback domain (pure) | `core::player` — Player, QueueModel, TransitionPlan (Sweet Fades), gain policy, snapshot v1/v2, events | `web/player-core` — same concepts, same constants | 107-test TS-suite port (`core/tests/player.rs`); shared constants |
| Sequencing engine (PCM, deterministic) | `musicpack-engine` — decode sessions, ring, resampler, equal-power mixer, crossfade lanes, capabilities (`sample_accurate_gapless`) | AudioWorklet engines (`musepack-engine.ts`, rust worker) | `xfade_oracle.jsonl` generated from the real TS processor |
| Host seam | `musicpack-host` render loop; `musicpack-wasm` thin binding | `controller.ts` facade + `PlayerPorts` | host integration tests |
| Platform output + OS integration | **none yet (by design)** | AudioWorklet sinks, `media-session.ts`, OPFS offline | e2e specs |

The important architectural reading: **the portable/shared layer is
"intent + state + PCM sequencing"; each platform owns output, clock, and
OS media integration.** That is exactly the right boundary, and it is
already enforced by trait shape (`Engine` capabilities are declared, not
assumed; the TS sink is "dumb"; Rust owns the crossfade mix).

### 8.2 Concept classification (the brief's core question)

- **Portable domain concepts** (shared, in `core::player` / core formats):
  queue and ordering policy (repeat/shuffle with injected RNG), transition
  planning (gapless vs hard-cut vs Sweet-Fade, all constants), replaygain/
  loudness targets, representation-selection policy
  (`policy/representation.rs`), persisted playback state (snapshot v1/v2),
  waveform envelope format, position/seek semantics, error taxonomy,
  Media-Session-*shaped* metadata/commands (as plain domain values).
- **Web-specific implementation**: AudioWorklet sinks and SAB wiring,
  `networker.js` range transport, OPFS/IndexedDB offline stores, service
  worker/PWA, Media Session API bindings, capability probing via
  `canPlayType`.
- **iOS-specific**: AVAudioEngine source-node rendering the shared engine,
  `MPNowPlayingInfoCenter`/`MPRemoteCommandCenter`, background audio mode,
  CarPlay template app, iOS file/Keychain token storage.
- **Android-specific**: Oboe/AudioTrack stream pulling the shared engine,
  Media3 `MediaSession` (+ `MediaLibraryService` for Android Auto),
  foreground service, storage/SAF offline store.

Explicit non-goals (endorsed): do **not** force WebAudio into Rust; do
**not** put AVFoundation/Media3 behind the shared seam; do **not** share
UI.

### 8.3 The native decision and why

Native players (AVPlayer, ExoPlayer) cannot deliver MusicPack's
differentiators — sample-accurate gapless and oracle-pinned Sweet
Fades/crossfade — without reimplementing exactly what `musicpack-engine`
already does and proves. Therefore (ADR 0003/0006): **the Rust engine is
the audio core on native platforms too**, fed by the same source seam,
rendered through a thin platform output adapter; queue/transition/
snapshot come from `core::player`; the platform media-session layer
projects domain state onto OS integrations. CarPlay and Android Auto are
*consumers of that projection*, not separate playback stacks. The FFI
mechanism (UniFFI-style generated bindings in a dedicated native-only
crate, mirroring the rusqlite precedent for scoped exception) is named as
the direction but deliberately not final until the spike (§17).

### 8.4 Gaps to close before native (none are code now)

1. Snapshot format spec document (it becomes a cross-platform contract).
2. An engine capability contract for "decoded from local package store"
   (offline) — the SourceBackend seam already permits it; document it.
3. Lyrics model (deferred decision, §17) — position events already exist
   to drive synced-lyrics UI on every platform.

## 9. Web framework comparison

Decision: **retain Svelte 5; reorganize, do not replace** (ADR 0004).
Comparison against MusicPack's actual requirements (not generic
benchmarks):

| Requirement | Svelte 5 (current) | React | Vue 3 | Solid | Flutter Web | React Native Web |
|---|---|---|---|---|---|---|
| Sophisticated audio UI (waveform canvas, worklet interop) | strong; existing WaveformSeek + worklet glue proven | equivalent possible | equivalent | equivalent | poor fit: no DOM audio stack, canvas-first model fights worklet/SAB | worst: native-only audio assumptions |
| Responsive desktop/mobile web | existing CSS breakpoints | same | same | same | different paradigm (widgets) | different paradigm |
| Offline/PWA | existing SW+OPFS code | same | same | same | weak | poor |
| Accessibility/keyboard | standard DOM semantics | standard | standard | standard | weakest of the set | weakest |
| Large collections/search/queue | proven at current scale; virtualization is DOM-normal | same | same | same | n/a | n/a |
| Author app reuse | Author is already Svelte 5 + same theme | would fork design language | — | — | — | — |
| Testing (unit+e2e+visual) | vitest+Playwright in place; purity gate pattern | equivalent | equivalent | smaller ecosystem | visual story differs | e2e story differs |
| Ecosystem maturity | adequate: zero runtime deps needed so far | largest | large | smaller | large but different | large but different |
| LLM-assisted development | small file count, compiler-enforced templates, no state lib | larger boilerplate surface, more convention | comparable | comparable | Dart + widget trees (worse agent fit for this codebase) | worse |
| Migration cost from today | zero | rewrite 33+15 components + 55 test files, lose design-language continuity | same order | same order | total rewrite | total rewrite |

The decisive facts are MusicPack-specific: the domain layer is already
framework-free (`player-core`, and Rust below it), so the framework's job
is "premium dark UI over a typed domain" — which the existing Svelte app
already does with zero runtime dependencies and a shared language with the
Author. Replacement would destroy oracle value (55 test files), fork the
design system, and buy no capability the roadmap needs. The honest
conditional: if the team ever wanted React specifically for a hiring or
ecosystem reason, the migration boundary is clean — `app/src/lib/ui` +
`state` swap out; `player-core`, `api`, `offline` engines, and everything
Rust stay. That boundary is recorded so the option remains cheap to
evaluate later; it is not recommended.

## 10. UX architecture

The project does not need a new design; it needs the existing one made
**explicit, portable, and testable**. Recommendations (documentation-first,
executed when web migrates into this repo):

### 10.1 Information architecture (codify what exists)
Shelf (`/albums`, recently-added sort) → album detail with release
editions (`?release=N&section=S`) → track detail; artists index/artist
page; search (server-side `q`); queue page; settings (audio preference,
offline storage panel); offline badges/filters; player bar → mobile
player. This is the current router (`router.ts:30-40`) plus observed
behavior — write it down as the IA spec so additions (lyrics, downloads
per track) have a home and a rule ("new surfaces must mount in an existing
nav region; new routes require a canonical screen spec first").

### 10.2 Interaction patterns (one rulebook)
Define once, apply everywhere: play (click = play now + replace queue from
context), queueing (add-next / add-last affordances), selection model,
download/offline states (⤓ badge → progress → Available offline → Needs
repair → Reinstall — the offline e2e lifecycle already defines this),
representation selection (preference applies to *future* items — pinned by
`representations.spec.ts`), error presentation (friendly copy via
`friendlyMessage()`, one toast region), loading (skeleton vs spinner
rule), empty states (search empty state exists — make the pattern
canonical), confirmations (destructive only: remove-offline, sign-out),
dialogs vs menus, keyboard shortcuts (space, arrows, `/` for search —
formalize the existing set), mobile touch (bottom player, ≥44 px targets).

### 10.3 Responsive behavior
Codify the existing three regimes: desktop ≥1024 px (sidebar + player
bar), tablet 680–1024 (icon rail), mobile <680 (off-canvas sidebar,
`MobilePlayer` replaces the bar). Rule: every new screen declares its
behavior in all three regimes in its canonical spec.

### 10.4 Accessibility (expectations, auditable)
Keyboard-complete navigation with visible focus (`--focus` token exists),
semantic HTML landmarks, labelled media controls, contrast checked against
the cream-on-dark palette, `prefers-reduced-motion` respected (kill-switch
exists), screen-reader announcements for transport state changes. Make the
existing waveform focus-ring behavior (`waveform.spec.ts`) the template.

### 10.5 Design system
Extract, don't invent: `theme.css` tokens (surfaces, text, single accent,
type scale `--fs-*`, spacing `--space-1..7`, radii, the one allowed
shadow) become a documented `tokens.css` + a short design-system page
listing the component inventory (player components, nav, data lists,
dialogs, notifications, loading/error/empty states) with do/don't rules.
Icons stay inline SVG with a naming rule.

### 10.6 Canonical screens and visual regression
Write canonical screen specs (markdown + annotated screenshots) for the
~9 core screens plus Author workflow stages; add Playwright
`toHaveScreenshot` baselines for them when the web tree lives here (today
there is **zero** visual regression tooling — evidence in §2.5). Visual
diffs run in CI with a documented update procedure.

## 11. Author architecture

Current shape: Tauri 2 + Svelte 5 UI wrapping the **legacy CLI** over
JSON, with the legacy `mpcenc` as a sidecar (`author/src-tauri/
author_service.rs`). The Rust workspace already contains bit-exact
replacements for most of what it invokes: encode
(`musicpack-musepack-encoder`, 21/21 whole-file parity), waveform/analysis
(byte-exact), validation (`core::validation`), MPAK packing
(`write_mpak`), identity vectors (server crate).

Target architecture (phase R4 in the roadmap):
1. **Add a directory-package builder to core** (canonical `.mpack`
   dir writer — the missing writable sibling of `verify_directory`), plus
   promote `identity` from the server crate into core (O-S4's named
   trigger has arrived: the Author needs group/release keys).
2. **New app-level crate `crates/musicpack-author`** (or an extension of
   the CLI): draft model, draft validation, encode/waveform/build
   orchestration, `identify` support (the one capability with no Rust
   equivalent — AGENTS.md's "add when a product workflow requires it"
   trigger is now this). MusicBrainz lookup stays an app concern (HTTP +
   JSON, not domain).
3. **Author UI keeps Tauri + Svelte**, re-pointed from the legacy CLI to
   the workspace (CLI JSON surface retained as the seam so the UI barely
   changes). The `mpcenc` sidecar is retired. The legacy author remains
   frozen as the UX/behavior oracle.
4. Sonic (`sonic_model.rs`) stays app-level and out of core (O11
   validation still open).

Anti-leak rule: draft/encode-queue/MB-confidence concepts never enter
`core::player` or the Player UI; package-*reading* concepts never gain
authoring mutators in the Player path.

## 12. Native architecture

Target shape (ADRs 0003/0006):

```text
        shared (Rust, oracle-pinned)         per-platform (native)
  ┌────────────────────────────────┐   ┌──────────────────────────────┐
  │ core: formats, validation,     │   │ iOS: SwiftUI UI,             │
  │ policy::representation         │   │  AVAudioEngine render node,  │
  │ core::player: queue/transition │   │  NowPlaying/remote commands, │
  │  /snapshot/gain/events         │   │  CarPlay template, Keychain  │
  │ engine: decode→mix→crossfade   │←─▶│ Android: Compose UI, Oboe/   │
  │ host: render loop              │FFI│  AudioTrack sink, Media3     │
  │ (new) ffi binding crate:       │   │  MediaSession + Auto browse, │
  │  types + engine + player       │   │  foreground service, storage │
  └────────────────────────────┬───┘   └──────────────────────────────┘
                               │ same HTTP API v1 + byte serving
                        ┌──────┴───────┐
                        │ musicpack-   │
                        │ server       │
                        └──────────────┘
```

What is shared: domain types, queue/transition semantics, the audio engine
(decoded PCM through crossfade), package reading/validation for offline
integrity, snapshot persistence format, representation policy.
What stays native: **all UI**, audio *output* (the render callback into
the platform audio system), OS media sessions, CarPlay/Android Auto
integration, storage/permissions, push/foreground services.

Why not share UI (Flutter/KMP/RN): MusicPack's UI value is deep platform
integration (CarPlay, Auto, Now Playing, widgets) and a design language
already expressed twice in Svelte; cross-platform UI would degrade the
integrations that motivate native in the first place. Why not platform
players for audio: §8.3. Offline model: whole-package integrity-verified
download (core `validation` reused) into a platform store; streaming uses
the same HTTP range contract the web block reader uses (ADR 0008).

Decision posture: the *shape* is decided now; the FFI tooling, minimum OS
versions, and audio-output specifics (AVAudioEngine source node vs
render-only; Oboe vs AAudio) are explicitly deferred to the spike phase
(§17) so evidence, not enthusiasm, picks them.

## 13. LLM-friendly development architecture

The codebase is already unusually agent-friendly; these are the patterns
to keep and the gaps to close.

Keep (observed, working):
- **Trait seams with one implementation** (`Store`, `PackageBackend`,
  `AudioDecoder`, `Engine`, `SourceBackend`) — an agent can read the trait
  and one impl and knows the whole story. No DI containers anywhere.
- **Executable contracts**: golden/oracle tests are the primary
  specification; naming conventions (`*_oracle.rs`, `db_compat.rs`,
  `*_compat`) tell an agent what is frozen. Keep the convention sacred:
  compatibility expectations are never weakened to make tests pass
  (`docs/architecture.md:571-574`).
- **Doc-comment headers that state provenance and rules** (e.g. purity
  laws in `player/mod.rs:9-19`, engine layering in engine `lib.rs:12-22`,
  serve-tree documentation in `http/serve.rs:5-23`). This is the single
  highest-value LLM affordance in the repo — mandate it for new modules.
- **Bounded, explicit constants** (`limits.rs`, watermarks, budgets)
  instead of magic numbers; deterministic tests via injected time/RNG.
- **AGENTS.md at the root** carrying the non-negotiables.

Close (gaps):
1. **File-size budget**: src files should stay ≤ ~800 lines with a soft
   target near 500. Current violations: `store/sqlite.rs` (1,852),
   `store/read.rs` (1,138), `http/routes.rs` (892). Oracle *test* files
   (~1,200–1,300) are acceptable as data-driven scripts but should share
   harness code. Split at the next substantive touch (§6).
2. **Per-directory AGENTS.md** when `web/` and `apps/` land: frontend
   conventions (store pattern, component budgets, a11y rules, tokens-only
   styling) cannot live in the Rust root file without becoming noise.
3. **ADR discipline**: decisions like the ones in this review go to
   `docs/adr/` with status + triggers, so agents don't re-litigate or
   silently violate them.
4. **Canonical screen specs** (§10.6) so UI agents implement against a
   spec, not against screenshots they invent.
5. **No new patterns without a concrete second use** — codified as a rule:
   abstraction requires two call sites or an oracle; "generic layer" PRs
   are rejected.
6. Anti-pattern list (explicitly banned): god components/services; hidden
   global mutable state (the server's one `Mutex<Store>` in `Context` is
   the sole sanctioned global); deeply nested frontend stores; domain
   logic in `.svelte` files (belongs in `player-core`/`state` modules —
   the existing purity gate test is the model); duplicated business logic
   across TS/Rust (oracle tests are the tripwire); implicit conventions
   and magic configuration; speculative abstraction layers.

## 14. Compatibility strategy (and the waveform erratum)

**Permanent compatibility** (these are contracts, forever):
- `.mpack` manifest v1 parsing + canonical serialization; MPAK v1
  container (CRC-before-length, pack order); waveform envelope v1;
  identity keys (fingerprints/group/release keys are persisted in
  existing databases — they can never change meaning).
- The C `library.db` schema migrations 1–10 (bidirectional compat while
  any legacy-created database may be opened); additive v11+ only (D-S2).
- HTTP API v1 envelope and byte-serving semantics: exact error codes/
  messages, pagination quirks, VISIBLE gate, single-range discipline,
  strong sha256 ETags. Versioning rules follow the legacy spec §5: URL
  prefix is the version, fields added never removed within v1, API
  versioning independent of manifest versioning
  (`specs/musicpack-api-v1.md:465-471`).
- Externally observable implementation quirks are *kept and documented*,
  because they are contract: the `mime[48]` waveform Content-Type
  truncation (`c_mime_truncate`), MHD-style percent-decode leniency,
  `(int)` pagination wrap. The test: if a byte-level differential test
  can observe it, it is contract.

**Transitional compatibility** (exists only to serve migration, retired at
its milestone): live C-server oracle testing (retire at stage-9 cutover);
bidirectional DB compat with the C server (same milestone); the
`Connection: close` divergence (already recorded; keep-alive may be added
later as an improvement, §7).

**Erratum (the §14 task).** The mismatch exists — but in the *legacy*
specs, which are immutable:
`specs/musicpack-api-v1.md:365-373` ("`Range` is intentionally not
supported" for the waveform endpoint) and
`specs/musicpack-waveform-v1.md:373-374` ("416 / Range not supported").
The behavioural authority — the C implementation — does support Range on
waveforms: `handle_stream` routes kind 2 (waveform) through the identical
`serve_object` range path as audio and assets
(`server/src/api.c:1340-1361`, `557-619`), and the stage-5 Rust oracle
pins waveform Range behavior against the live C server
(`tests/media_oracle.rs`, waveform matrix). Correction made in this repo:
`docs/api-spec-errata.md` records the supersession; the Stage-5
implementation is unchanged (oracle behaviour is authoritative), and our
own docs never carried the wrong claim. Note the spec sentence retains one
true kernel: *clients* consume waveforms whole — that guidance stands;
only the "server does not support Range" claim is wrong.

## 15. Target architecture

```text
┌────────────────────────────── one repository (musicpack-core) ─────────────────────────────┐
│                                                                                            │
│  Rust domain (portable, oracle-pinned, forbid(unsafe), wasm-clean unless marked native)    │
│  ┌──────────────────────────────────────────────────────────────────────────────────────┐  │
│  │ musicpack-core: format(.mpack/.mpak/waveform) · storage · validation · audio ·       │  │
│  │                 player · policy          [+ identity, dir-builder when Author phase] │  │
│  ├──────────────────────────────────────────────────────────────────────────────────────┤  │
│  │ musicpack-engine (PCM sequencing) · musicpack-host (render seam) ·                   │  │
│  │ musicpack-wasm (web binding)      [future: musicpack-ffi (native-only binding crate)]│  │
│  ├──────────────────────────────────────────────────────────────────────────────────────┤  │
│  │ musicpack-server (native-only): ingest · store(SQLite) · media · http(v1 + bytes)    │  │
│  │ musicpack-musepack-encoder / mpc-tools (LGPL wall)  ·  musicpack CLI                 │  │
│  └──────────────────────────────────────────────────────────────────────────────────────┘  │
│                                                                                            │
│  Frontends (UI owns platform; domain never duplicated)                                     │
│  ├─ web/  Player: Svelte 5 + player-core(TS, frozen domain port) + rust/wasm engine       │
│  │         AudioWorklet output · networker range transport · OPFS offline · PWA           │
│  ├─ apps/author: Tauri 2 + Svelte 5 → workspace pipeline (encode/analyze/validate/pack)   │
│  └─ apps/ios · apps/android (future): native UI + platform output/sessions over FFI       │
│                                                                                            │
│  Contracts: HTTP API v1 · .mpack/MPAK/waveform/identity · snapshot v1/v2 · design tokens  │
│  Oracles:   legacy repo (frozen): C server · TS player · TS engines · Author behavior      │
└────────────────────────────────────────────────────────────────────────────────────────────┘
```

Differences from the brief's sketch, and why: (1) the Web Player sits on
*Rust* domain/engine via wasm, with TS player-core as orchestrator — not a
from-scratch browser client; (2) native apps consume the same Rust stack
through a binding crate rather than speaking only HTTP; (3) the Author is
a first-class app over the workspace, not a server feature; (4) everything
shares one repository and one contract table.

## 16. Decisions to make now

| Decision | Decide now? | Reason | Recommendation |
|---|---|---|---|
| Rust core boundaries | **Yes** | every later phase depends on them | Ratify current contents incl. `player`; add dir-builder + identity at Author phase; never server/UI/offline concepts (ADR 0002) |
| Server application layer | **Yes** | avoids fashionable over-engineering | None; handlers+`ingest` are the layer; fix the one seam leak; jobs module at stage 8 |
| HTTP implementation | **Yes** | repeated question, now settled with triggers | Retain std-only HTTP/1.1; triggers in ADR 0001; keep-alive is the first candidate *enhancement*, evidence-gated |
| API versioning | **Yes** | needed before cutover | `/api/v1` frozen at cutover; additive fields only; v2 only for breaking change; follow legacy §5 rules |
| Database abstraction | **Yes** | stability | Keep `Store` trait + verbatim migrations; additive v11+; split sqlite.rs file at next touch |
| Filesystem abstraction | **Yes** | stability | Keep `PackageBackend`/`ByteSource` + server `media.rs`; no new layers |
| Playback model | **Yes** | native work depends on it | Ratify 4-layer split; snapshot format becomes documented contract; platform-owned output/sessions (ADR 0003) |
| Web framework | **Yes** | unblocks all Player/Author work | Retain Svelte 5; reorganize (ADR 0004) |
| Frontend state management | **Yes** | avoid churn | Keep hand-rolled store primitives + `player-core` as state owner; no state library; runes only with a concrete win |
| Design system | **Yes** | needed by every future UI agent | Extract tokens from `theme.css`; document inventory + rules (§10.5) |
| UX documentation | **Yes** | agents need specs | Canonical screen specs + interaction rulebook + visual regression (§10) |
| Monorepo for web/author | **Yes** | largest structural risk | Migrate `web/` + `author/` into this repo at server cutover; legacy copies frozen as oracles (ADR 0005) |
| Author architecture | **Yes (direction)** | long lead time | Tauri+Svelte shell retained; Rust pipeline replaces legacy CLI+sidecar; core gains dir-builder+identity (§11) |
| Native binding strategy | **Yes (direction)** | shapes crates now | Native-only binding crate over domain+engine (rusqlite-style scoped exception); tooling (UniFFI etc.) deferred (ADR 0006) |
| Offline model | **Yes** | affects server+clients | Whole-package integrity-verified download per platform; range streaming for on-demand; no server transcode, ever (ADR 0008) |
| Waveform Range spec | **Yes (done)** | contract clarity | Erratum recorded; oracle authoritative (`docs/api-spec-errata.md`) |
| Representation selection | **Yes** | already central | `policy::representation` is the single resolver for all platforms; hosts persist the *preference* only (current web behavior) |

## 17. Decisions to defer

| Decision | Defer until | Trigger / evidence needed |
|---|---|---|
| HTTP keep-alive | after measurement | high-latency client evidence (mobile/WAN block-read latency) |
| HTTP framework / HTTP2/SSE | never without a trigger | ADR 0001 triggers |
| Core crate split | a second consumer forces it | ADR 0002 triggers (wasm size, format-only FFI consumer, compile time) |
| FFI tooling (UniFFI vs manual) | native spike (phase R5) | working iOS render prototype |
| iOS details (min OS, AVAudioEngine shape, CarPlay template) | phase R5 | spike results |
| Android details (Oboe vs AAudio, Media3 session adapter) | phase R5 | spike results |
| Lyrics model + synced-lyrics format spec | Player lyrics phase | product decision that lyrics are next; then define `musicpack-lyrics-v1` before any UI |
| Runes migration in Svelte app | never by default | concrete readability/perf win on a touched component |
| Visual regression rollout | web migration (R2) | baselines only meaningful in-repo |
| SV7 header-probe (O-S3) | when an SV7 package is a real fixture | evidence of SV7 packages in the wild |
| Sonic validation (O11), Windows adapter (O12), UTF-8 boundary (O4) | respective products need them | per open question |
| crates.io publishing (O9) | stable API surface demand | external consumer |

## 18. Migration roadmap

Ordered, each phase leaving the workspace green (canonical gates in root
`AGENTS.md`). No phase in this list rewrites a working subsystem.

- **R0 — Documentation checkpoint (this document).** Review + ADRs 0001–0008
  + `docs/api-spec-errata.md` + stale-statement fixes in
  `docs/architecture.md`. Done alongside this review.
- **R1 — Finish the server (stages 7–9).** Static hosting + SPA fallback +
  COOP/COEP headers + Playwright smoke (stage 7); real jobs API
  (scan/verify progress through the existing `JobSnapshot` seam), graceful
  shutdown, deployment docs (stage 8); parity sign-off & cutover checklist
  (stage 9). Includes the small cleanups: routes seam leak, sqlite.rs
  split, ingest dedup.
- **R2 — Bring the frontends home.** Copy `web/` and `author/` from the
  legacy repo into `web/` and `apps/author/` (legacy copies untouched,
  frozen); wire vitest+Playwright into CI; replace `sync-wasm.sh` with a
  cargo/vite build integration; add visual-regression baselines (§10.6);
  split the two god components opportunistically; add per-directory
  AGENTS.md files. Acceptance: one CI run builds server+wasm+web and runs
  all suites; the legacy repo is referenced only as oracle.
- **R3 — Player work.** Lyrics (spec first, then UI), offline
  hardening/UX polish per the rulebook, keep-alive measurement (decide per
  ADR 0001), canonical screen specs retroactively written for the 9 core
  screens.
- **R4 — Author pipeline.** Core dir-builder + identity promotion;
  `crates/musicpack-author` (draft/validate/encode/waveform/build/
  identify); re-point the Tauri app; retire the `mpcenc` sidecar; the
  legacy author becomes the frozen UX oracle.
- **R5 — Native spike and foundations.** `musicpack-ffi` (or chosen
  tooling) crate; iOS render-loop prototype over `musicpack-host`;
  snapshot-format spec document; CarPlay shell consuming the queue
  projection; Android equivalent with Media3 session adapter. Gate:
  R1–R3 stable; exit criteria written before the spike starts.

## 19. Risks and trade-offs

1. **Two-repo coupling until R2** — every cross-layer change stays
   non-atomic and the web build depends on a sibling checkout
   (`sync-wasm.sh`). Mitigation: R2 early; until then, pin the wasm
   artifact version in web.
2. **Oracle rigidity vs evolution** — byte-exact contracts make some
   changes expensive by design. Mitigation: the compat classes in §14
   (permanent vs transitional) tell us what we are *allowed* to evolve and
   when; ADR 0007 records it.
3. **Dual playback implementations (TS + Rust) can drift** — mitigated
   today by oracles (player suite, xfade jsonl) and by the Rust backend
   being the web default; the endgame is one implementation, with TS
   player-core remaining only as the frozen oracle after any future
   cutover decision (explicitly *not* scheduled — evidence first).
4. **Store trait width (41 methods)** — a god interface risk. Single impl
   keeps it honest; if a second backend appears, split by area then.
5. **LGPL wall erosion** — the encoder/tools crates stay source-derived
   LGPL; the guard is AGENTS.md plus dependency direction, not tooling.
   Risk accepted; a cargo-deny check could be added cheaply in R1 if
   desired (optional, not required).
6. **Native ambition scope creep** — the biggest schedule risk. Mitigated
   by deferring everything behind a spike with exit criteria (§17) and by
   the fact that the shared layer already exists.
7. **Keep-alive/transport improvements breaking oracle tests** — any
   transport change must keep per-request semantics identical and update
   the raw-socket oracle accordingly; the divergence is already documented
   so the change is additive, not contractual.
8. **Frontend god-component drift** continues until R2 splits them;
   bounded by the 55 existing tests.

## 20. Recommended next implementation phases

1. **R1 server completion** (static hosting → ops/jobs → cutover
   checklist). Rationale: the server is 5 stages deep with proven parity;
   finishing it unlocks the monorepo migration and retires the riskiest
   transitional compatibility. Small cleanups ride along.
2. **R2 monorepo migration of web + author** with CI unification and
   visual-regression baselines. Rationale: removes the structural risk
   before adding frontend features; makes every later phase atomic and
   agent-navigable.
3. **R3 player features** (lyrics spec+UI, offline polish, keep-alive
   decision, canonical specs). Rationale: highest user-visible value once
   everything is in one repo; lyrics is the one gap in the IA and needs a
   format decision made *with* evidence.
4. **R4 author pipeline**. Rationale: retires the last runtime dependency
   on the legacy toolchain (sidecar `mpcenc`) using already-proven Rust
   pieces; promotes identity/dir-builder into core with a concrete second
   consumer — the exact evidence our rules require.
5. **R5 native spike** (iOS first, then Android), gated on R1–R3 and on
   written exit criteria. Rationale: the shared domain/engine investment
   makes this a continuation; starting earlier would build against a
   moving server and an unmigrated frontend.

**Answer to "what should we build next, and in what order":** finish the
server, move the frontends home, then improve the Player, then replace
the Author's engine, then go native — with every phase's contracts already
pinned by the oracle tests this project does better than anything else.
