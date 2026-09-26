# MusicPack Author

MusicPack Author is the first-party **desktop GUI for authoring `.mpack`
releases**. It turns a tagged lossless FLAC/WAV album — or an already-tagged
Musepack album — into a curated, validated `.mpack` directory bundle,
presenting the album as a release/edition being authored rather than exposing
`manifest.json` as the UI. It can also export that package as a deterministic
single-file **`.mpak` container** (either when creating a new package or by
converting an opened `.mpack`), using the authoritative core `.mpak` writer.
Since Phase 3 (the MVP) it also **encodes** FLAC/WAV
sources to Musepack SV8 (q6 default) in-app, so a terminal is never required.
FLAC/WAV decoding is native (`musicpack-core`) — no FFmpeg or other external
multimedia tool is involved.

```text
FLAC / WAV
   ↓
native decode (musicpack-core) + Rust encoder (musicpack-musepack-encoder)
   ↓
tag-rich Musepack album                      ← or an existing MPC album
   ↓
.mpack (validated MusicPack v1 directory bundle)
   ↓ (optional export / convert)
.mpak (deterministic single-file container, via the core `.mpak` writer)
```

The application is part of the MusicPack product family: the same visual
language as the `web/` record-shelf client — the premium dark v2 system
(near-black canvas, warm cream serif type, one gold accent, green reserved
for verified states; see `docs/ui-v2-design.md`) — expressed in a
tool-oriented, workflow-first information architecture, Svelte 5 +
TypeScript, running as a Tauri 2 desktop app on macOS (Linux/Windows are
not yet a focus).

## Runtime (R4.3 cutover)

The application no longer ships or spawns the legacy C `musicpack` CLI,
`mpcenc` or `musicpack-sonic`. The default runtime is the in-process Rust
pipeline:

```text
Svelte 5 UI
   ↓  Tauri commands
RustBackend (src-tauri/src/rust_backend.rs)
   ↓
musicpack-author   (draft JSON, encode, waveform, identify, pack)
   ↓
musicpack-core     (format, manifest, verify, .mpak, identity)
```

The legacy C-CLI `AuthorService` (`src-tauri/src/author_service.rs`) remains
only as a development escape hatch, enabled explicitly with
`MUSICPACK_AUTHOR_LEGACY=1` (see `docs/author-runtime.md`). It is never
bundled and is never used after a Rust failure — errors are reported, not
silently retried.

## Architecture

```text
Svelte 5 app (app/)                     ← in-memory AuthoringDraft, dark v2 authoring UI
   │  invoke() via @tauri-apps/api
   ▼
Tauri commands (src-tauri/src/lib.rs)   ← thin JSON surface
   │  RustBackend (default)  ·  AuthorService (legacy escape hatch, R4.3)
   ▼
musicpack-author                        ← draft JSON, encode, waveform, identify, .mpak
   ▼
musicpack-core (source of truth)        ← manifest semantics, hashes, BS.1770, waveform, verify
   │
   └─ musicpack-musepack-encoder        ← Musepack SV8 encoder (isolated LGPL)
        (FLAC/WAV decoded natively by musicpack-core; no external tool)
```

**`musicpack-core` stays authoritative.** The GUI never reimplements the
`.mpack` format: package semantics, validation, checksums, metadata
reconciliation, MusicBrainz identity and loudness all come from the existing
`musicpack` implementation.

**Why the CLI is wrapped instead of linked directly.** The album-scanning and
package-assembly logic currently lives in the `musicpack` CLI
(`musicpack/main.c`), not in `libmusicpack`. Direct FFI would have meant
either reimplementing that logic in Rust (forbidden: no second metadata
parser / package logic) or first moving it into the library (a real
`libmusicpack` API change). So Phase 1 wraps the CLI behind a clean service:

- every interaction uses **structured JSON** (`inspect --json`, etc.), never
  prose parsing;
- subprocess spawning is isolated inside `author_service.rs` — the only place
  that touches the CLI;
- the `AuthorService` interface is designed so a direct `libmusicpack` binding
  can replace the subprocess later without touching the frontend.

The CLI gained draft commands for this purpose (see `musicpack/main.c`):
`inspect`, `validate-draft`, `encode-draft`, `waveform-draft`, `build-draft`, `identify-draft`,
and a `--json` mode on `verify`. `import`/`create`/`info` behaviour is
unchanged (the scan logic was extracted into a shared helper).
`author-api-version` is the machine-readable capability handshake the GUI
uses to verify backend compatibility (see [Backend compatibility](#backend-compatibility));
it is at version **7**, which keeps per-track `representations[]` in the
draft so Author saves preserve the Phase 3 alternates; version 6 added
opening existing `.mpack` packages (`inspect` on a package builds the draft
from its manifest) and in-place saves
(`build-draft --replace --sync-tags`).

## The authoring draft

The GUI edits an in-memory **authoring draft** (application state, not a
MusicPack format) and serializes it to a `musicpack-draft` JSON only to cross
the CLI boundary. It mirrors the `.mpack` v1 logical hierarchy — `album`
(release group), `release` (specific edition), `media[]`, `tracks[]` — plus
`identifiers`, `identity`, `source`, `artwork`, `booklet`, `lyrics`, `extras`,
and `waveformAnalysis`. Waveform generation is default-on: it performs a
separate native source-decode pass and creates one deterministic 100 ms
peak/RMS envelope per track. It can be explicitly disabled for packages that
must omit this optional derived asset.
`release`, `source` and `identity` are kept strictly separate, exactly as the
spec requires. Audio/artwork paths point at files under `sourceRoot`; no
half-created package directory is mutated as the user edits.

## Development

Requirements:

- macOS with **Xcode Command Line Tools** (`xcode-select --install`)
- **Node.js ≥ 20** and npm
- **Rust toolchain** (Tauri 2 backend): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- a built `musicpack` CLI (the backend): `cmake -S . -B build && cmake --build build -j --target musicpack_cmd`
- to use the encode stage: a built `mpcenc` (`cmake --build build -j --target mpcenc`, or on PATH). No external decoder is needed: FLAC/WAV sources are decoded natively by the backend.

Run the app:

```sh
# from the repository root
cmake -S . -B build && cmake --build build -j --target musicpack_cmd mpcenc

# from author/
npm install
npm run tauri dev
```

`npm run tauri dev` starts Vite on `http://localhost:5174` and opens the
native window. In development the Rust service resolves the `musicpack`
binary as `MUSICPACK_CLI` → `../build/core/musicpack/musicpack` →
`../build/musicpack/musicpack` (pre-reorg trees) →
`../build-static/musicpack/musicpack` → `PATH`; override with the
`MUSICPACK_CLI` environment variable. See
[Backend resolution](#backend-resolution) for the full policy.

### Dev build profile (DSP)

`tauri dev` builds the host with the unoptimized dev profile, but the
authoring DSP chain — native FLAC/WAV decode (`claxon`), waveform
accumulation, BS.1770 loudness, and Musepack encoding — is numeric
dependency code with no debug info worth stepping through. At
`opt-level = 0` the waveform stage measured **~26x slower** than a release
build (2.44 s vs 0.10 s for a 120 s 44.1 kHz stereo track; envelopes
byte-identical either way, and at parity with the reference C
`waveform-draft`), and a 10-track album measured **~24x slower** to encode
(250.9 s vs 10.1 s; 20.9–36.4 s per track down to 0.8–1.5 s).
`src-tauri/Cargo.toml` therefore pins the three hot dependencies
(`musicpack-core`, `claxon`, `musicpack-musepack-encoder`) to
`opt-level = 2`/`3` in the dev profile while the host itself stays
unoptimized, so incremental rebuilds remain fast.

Opt-level cannot move a single output bit: Rust emits no FP contraction and
no fast-math, so float evaluation order is independent of codegen level. A
real 302.7 s track encodes to a byte-identical stream (same SHA-256) at
opt-level 0, 2 and 3. The packaged app is unaffected: it builds with
`[profile.release]`.

This workspace is excluded from the root Cargo workspace, so it carries its
own profile block — a dev-profile pin added at the root does not apply here.

Quality commands:

```sh
npm run check          # svelte-check (types + a11y)
npm test               # vitest: unit (node) + component (jsdom)
npm run build:web      # plain web build of the frontend
```

Backend tests: the `author_backend` and `author_encode` CTest suites
(`tests/run_author.sh`, `tests/run_encode.sh`, UNIX) drive the CLI draft
commands end to end, including the `author-api-version` handshake, the
FLAC→MPC / WAV→MPC encode stages, the native `audio_decode` suite, and a
negative run with ffmpeg/ffprobe absent from PATH — no external tool is
required.

## Standalone macOS build

Build a self-contained, copy-anywhere `MusicPack Author.app` with one command
from the repository root:

```sh
./scripts/build-author-macos.sh
```

The resulting application bundle is:

```text
author/src-tauri/target/release/bundle/macos/MusicPack Author.app
```

The standalone application bundles its MusicPack authoring backend **and the
`mpcenc` encoder** and does not require a separate `musicpack` or `mpcenc`
installation. It also does not require CMake, Node.js, Rust, the source
repository, `MUSICPACK_CLI`, FFmpeg, Homebrew or MacPorts; copy the `.app` to
another compatible Mac and launch it. MusicBrainz requests use the bundled
Rust backend and do not require an external command-line tool.

The script (1) builds **fully static** `musicpack` and `mpcenc` binaries in
`build-author/`, (2) verifies with `otool -L` that they reference only macOS
system libraries, (3) stages them as Tauri sidecars, (4) runs the Svelte
frontend build and `npm run tauri build`, and (5) runs
`scripts/smoke-author-macos.sh` against the finished `.app`.

### Supported macOS architectures

Phase 1.1 builds for the **development machine's architecture** — arm64
(`aarch64-apple-darwin`) or x86_64 (`x86_64-apple-darwin`) — which is the
`host` reported by `rustc -vV`.

A universal binary is achievable later: build the C backend with
`-DCMAKE_OSX_ARCHITECTURES="arm64;x86_64"` (CMake emits a universal Mach-O for
the sidecar), build the Rust side per-target
(`cargo build --target aarch64-apple-darwin --target x86_64-apple-darwin`),
and pass `--target universal-apple-darwin` to `tauri build` (which lipos the
app, or lipo the sidecars manually). CI would use `macos-latest` (arm64) plus
`macos-13` (x86_64) runners. This is documented as future work, not built in
this phase.

### Dynamic-library strategy

The backend is built with `-DMPC_BUILD_SHARED=OFF`, so `musicpack` links
`libmusicpack` and `libmusepack` **statically**; `mpcenc` is statically built
the same way. The resulting Mach-Os reference only `/usr/lib/libSystem.B.dylib`
— there are no third-party dylibs to bundle, no install-name rewriting, and no
Homebrew paths. The build hard-fails if any sidecar references anything
outside `/usr/lib`/`/System/Library` (`scripts/verify-backend-dylibs.sh`).

MusicBrainz transport uses Rust `ureq`; release matching, candidate extraction,
and draft application remain in the local C backend. FLAC/WAV decode is native,
so no FFmpeg (or any other multimedia tool) is bundled or resolved at runtime;
the package audit rejects any bundled `ffmpeg*` file.

## Backend resolution

Resolution is decided once at startup and split into two explicit regimes so
a packaged app can never silently run an unrelated `musicpack`:

| Context                        | Order                                             |
|--------------------------------|---------------------------------------------------|
| Packaged app (release build)   | **bundled sidecar only** (`Contents/MacOS/musicpack`) |
| Development (`tauri dev`)      | `MUSICPACK_CLI` → `build/core/musicpack/musicpack` → `build/musicpack/musicpack` (pre-reorg) → `build-static/musicpack/musicpack` → `PATH` |

The encoder resolves the same way but separately: a packaged app uses the
`mpcenc` sidecar next to the bundled CLI; development uses `MUSICPACK_MPCENC`
→ the CMake build tree (`build/codec/mpcenc/mpcenc`, falling back to the
pre-reorg `build/mpcenc/mpcenc`) → PATH. No decoder binary is
resolved at all: FLAC/WAV sources are decoded in-process by the bundled
backend, so there is no FFmpeg dependency in either regime.

- The packaged sidecar is located next to the app executable
  (`std::env::current_exe().parent()`), not guessed filesystem paths.
- If the bundled backend is missing, the app starts but every backend
  operation (and the startup banner) reports an actionable
  *"reinstall MusicPack Author"* error. It never falls back to PATH.
- The pure resolution logic lives in `AuthorService::resolve_bundled` /
  `resolve_development` and is covered by unit tests
  (`cargo test` in `author/src-tauri`), including the rule that production
  never consults environment, build tree, or PATH.

## Backend compatibility

The GUI and the bundled backend evolve together, so `musicpack` reports a
machine-readable capability handshake:

```sh
musicpack author-api-version --json
# {"musicpackVersion":"0.1.0","authorApi":6}
```

On the first backend operation the `AuthorService` runs this and rejects a
mismatched `authorApi` with a clear error
(`IncompatibleBackend`). Compatibility is coupled to the explicit authoring
API version, not to patch versions — this also protects development mode if
`MUSICPACK_CLI` points at an older executable. The frontend surfaces the
handshake result at startup via the `backend_info` command
(`BackendBanner`).

## Security

- **Content Security Policy.** `tauri.conf.json` sets a strict CSP:
  `default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'
  data:; connect-src 'self' ipc: http://ipc.localhost ws://localhost:5174
  http://localhost:5174`. No `unsafe-eval`; no `unsafe-inline`. `data:` covers
  artwork previews; `ws/http://localhost:5174` is the Vite dev-server/HMR
origin and is inert in the packaged app. MusicBrainz lookups happen in the
native Rust backend (via `ureq`), so the webview is never granted internet
  access.
- **Capabilities (least privilege).** `capabilities/default.json` grants only
  `core:default` (drag/drop + IPC), `dialog:allow-open` (directory/image
  pickers), and `opener:allow-reveal-item-in-dir` ("Reveal in Finder"). No
  shell, filesystem, HTTP, or arbitrary command-execution permissions exist;
  all backend interaction flows through the typed Tauri commands and
  `AuthorService`.

## Current workflow

The UI organises authoring as a sticky **8-stage stepper** — Identity ·
Release · Tracks · Artwork · Encode · Sonic · Waveform · Validate
(`app/src/lib/authoring-state.ts`). The numbered journey below describes the
user-visible flow including the two steps that live outside the stepper
(adding an album on Welcome and the Create/Save dialog), so its ordering is
intentionally not identical to the stage order.

1. **Add album** — choose or drag/drop an album directory; `inspect` scans it
   into a draft (tags, disc grouping, track durations/codec via cheap header
   probes, artwork: file-based or embedded). No package is created.
   Dropping an existing `.mpack` package instead builds the draft from its
   manifest — the GUI then offers **Save changes** (write back in place,
   audio untouched) alongside *Save as copy*.
2. **Release metadata** — editable form covering the `.mpack` v1 model:
   release-group (title, artists, type, original date, genres), specific
   release (date, edition, country, label, catalogue number, notes),
   identifiers (MB release-group/release IDs, barcode), source (type, store,
   source ID), and media (disc number, format, title).
3. **Tracks** — disc-grouped list with number, title, per-track artist,
   duration, filename, codec, sample rate/bit depth, ISRC and MusicBrainz
   recording/track IDs; basic in-place editing (no full tag editor yet).
   Each track can carry lyric documents (R3.5): attach an `.lrc` file from
   inside the album directory, optionally tag its language, replace or
   remove it. The editor probes every file immediately (synced/plain line
   counts or a concrete parse error); the draft records `{path, lang?}`
   per track and the backend attaches them as the manifest's
   `track.lyrics[]` at build. Track lyrics are distinct from the
   package-level `lyrics` assets managed under Artwork (step 5).
4. **Encode to Musepack** — for lossless (FLAC/WAV) sources: encodes every
   track to Musepack SV8 (default q6, quality selectable in an advanced
   area) via the bundled `mpcenc` with native in-process decoding, showing
   current track, overall progress, current stage and errors, with
   cancellation. Unknown/custom source tags pass through to the `.mpc` APEv2
   tags. Your source files are never modified. (Skip this step to build a
   FLAC-backed package; already-Musepack albums skip it entirely.)
5. **Artwork/assets** — choose/change front artwork, add role-tagged artwork,
   and manage booklet/lyrics/extras. These are package-level assets: the
   `lyrics` row here feeds the manifest's root `lyrics[]`, not any track
   (per-track lyrics live on the Tracks step). Embedded artwork — FLAC
   `PICTURE` blocks and Musepack APEv2 `Cover Art (Front)` items — is
   discovered with the album and extracted at build time: **JPEG/PNG
   only**, chosen by byte signature (never by extension or declared MIME),
   preserved exactly — never decoded, resized or transcoded. An external
   `cover`/`front`/`folder` file always wins the `front` role; embedded
   pictures then fill the roles still free (`front`, `back`,
   `booklet-page`, `medium`, `other`), first per role in track order, so
   the same album always produces the same package. Malformed embedded
   artwork is skipped at import (the album and its tags still open) and
   fails only the build of an entry that can no longer be extracted.
   (Phase 1 artwork files must live inside the album directory — lyric
   source files follow the same rule.)
6. **Identity** — enter a MusicBrainz release ID (exact match applied) or
   search by barcode for candidates with per-release confidence; applying a
   candidate fetches and applies that release. Confidence is always visible
   (`exact` / `confirmed` / `probable` / `none`) and never silently promoted.
7. **Validation** — a preflight view shows errors and warnings separately,
   reusing `.mpack` validation semantics through `validate-draft`; the
   authoritative gate is `musicpack_manifest_parse()` over a synthesized
   manifest. `validate-draft` returns the structured `{ok, errors[],
   warnings[]}` verdict (invalid drafts included), with the host's
   track-lyrics findings merged in — missing/unreadable/malformed lyric
   files and duplicate lyric paths surface here. The GUI never invents
   required data to go green.
8. **Create MusicPack** — choose the packaging form (**`.mpack`** directory
   package or **`.mpak`** single-file container) and an output location (the
   package name is pre-filled from the album metadata, e.g. `Artist - Album`,
   and the extension follows the chosen form). The build narrates itself: the
   dialog shows a live `build-progress` line (phase, step *n* of 6, plus a
   counter) for the whole build, because a full album takes long enough that
   a bare "Creating…" reads as a hang. The phases are `audio`, `waveform`,
   `assets`, `draft`, `package` and `mpak`; `audio` and `waveform` count
   tracks. The `package` phase is the long one (~80% of a typical build), so
   the core builder reports its own sub-stages through it — as `detail` +
   `unit` rather than as separate phases: `assets` (copying and hashing every
   referenced asset, counted in assets), `loudness` (per-track BS.1770 and
   duration, counted in tracks) and `verify` (the authoritative verifier, one
   step, no counter). Progress is an observer only: cancelling a build would
   need a hook inside the core builder, so the encode and waveform stages stay
   the cancellable ones. For `.mpack`,
    `build-draft` copies and hashes every manifest-referenced asset (audio,
    artwork, booklet, lyrics, extras, and analysis), measures BS.1770-5
    loudness (album as one concatenated program), writes `manifest.json`, then
    runs full package verification. It preserves authored media and track order
    as canonical manifest order and rejects packages with more than 4096
    manifest-referenced assets. A package is never reported as successful if
    verification fails. Track-linked lyrics (R3.5) attach after the
    sidecar build: validated lyric files are copied to `lyrics/`,
    referenced from their owning tracks (`track.lyrics[]` with content
    hash and language), and the package is re-verified with the core
    verifier — a lyric failure fails the build, and a failed fresh build
    leaves no package behind. On macOS, “Reveal in Finder” opens the result.
    For a draft opened from an existing package, the dialog offers
    **Save changes** instead: `build-draft --replace` rebuilds into a
    verified staging directory and swaps it with the old package (rolled
    back on any failure), and `--sync-tags` re-projects the final manifest
    onto embedded APEv2 tags — only where they actually differ, so untouched
    tracks keep their bytes. Measured loudness and sonic/waveform documents
    are carried through unchanged; audio is never re-encoded. For `.mpak`,
    the same build runs into a private staging directory which is then packed
    via the authoritative `musicpack pack` and removed, so no intermediate
    `.mpack` is left next to the container. One R3.5 exception: a package
    carrying track-linked lyrics packs with the `musicpack-core` writer
    instead — the C packer only packs manifest-known references and would
    silently drop the lyric files while the manifest still references
    them; lyric-less packages keep the C path byte-identically. An opened
    package can also be converted with **Export as .mpak…**, which verifies
    the source and runs `musicpack pack`, leaving the `.mpack` directory
    untouched (lyrics-bearing sources take the same core-writer branch
    through the shared `pack_package` path).

## MusicBrainz identity

Uses the existing `musicpack` MusicBrainz code (`musicpack_mb_match_confidence`
/ `musicpack_mb_apply_release`). Exact-ID identification works now; barcode
candidate listing works; artist/title search is a Phase 2 backend item (the
UI/service boundaries already exist). Offline matching is supported via
`--mb-json`.

## Loudness

Canonical `.mpack` loudness is BS.1770-5, owned by `libmusicpack`. MusicPack
Author does not use ReplayGain tags as canonical loudness; it is measured at
package build time (the authoring view shows “Loudness · at build”).

Loudness is measured from **source-rate, source-channel PCM** decoded natively
(no resampling, no downmixing). Mono sources are measured as mono; the old
FFmpeg `-ac 2` upmix applied a −3 dB gain that skewed only the reported true
peak, so stereo/44.1 kHz results are unchanged while mono true peak is now
correct. The BS.1770 meter supports mono/stereo; multichannel sources are not
measured.

## Testing

- **Backend** (`tests/run_author.sh`, CTest `author_backend`): inspect a
  valid MPC album, import canonical metadata, preserve release vs source vs
  identity semantics, validation error propagation, successful package
  creation, post-build verification, failed verification surfaced as
  failure, traversal rejection, MB identity application, and the
  `author-api-version` handshake.
- **Encode stage** (`tests/run_encode.sh`, CTest `author_encode`): the full
  FLAC→MPC q6 flow against the committed 2-disc fixture — staged `.mpc`
  naming, transformed draft, projected + passthrough APEv2 tags (verified
  natively through libmusicpack, no ffprobe), WAV→MPC encoding, multi-disc
  build + `verify`, unsupported sample rate, mixed-source refusal,
  missing-tool pre-flight, SIGTERM cancel + staging cleanup, and a full
  authoring run with ffmpeg/ffprobe absent from PATH.
- **Rust** (`cargo test` in `author/src-tauri`): backend resolution
  (MUSICPACK_CLI override, build-tree fallback, PATH fallback, bundled
  sidecar, actionable missing-backend errors, and the rule that packaged
  resolution never consults environment/tree/PATH) plus the author-API
  handshake parser and gate, mpcenc resolution (no decoder is resolved),
  `encode_spawn` argument passing, and staging-cleanup refusal of foreign
  paths.
- **Frontend** — vitest `tests/unit` (draft store, API command surface
  incl. `backend_info`, `encode_tracks`/`encode_cancel`/`cleanup_staging`,
  formatting incl. `defaultPackageName`/`needsEncoding`) and jsdom component
  tests `tests/component` (track list rendering, release form editing,
  validation rendering, create-button state, EncodePanel progress/error/
  cancel/swap).
- **Packaging** — `scripts/smoke-author-macos.sh` (run by
  `scripts/build-author-macos.sh`): `.app` exists, bundled backend and
  `mpcenc` present and executable, backend runs from its bundled location and
   reports `authorApi: 4`, only system dylibs referenced, and a harmless
  structured backend operation succeeds.

## Limitations (Phase 3 MVP)

- Loudness is measured at build time, not shown live per track.
- **Metadata edited after encoding** updates the manifest but not the
  `.mpc` APEv2 tags already written at encode time — edit before encoding, or
  re-import the album.
- FLAC/WAV are the supported encode sources (16/24-bit PCM); ALAC/APE/etc. are
  future work. Sources above 48 kHz cannot be encoded to Musepack (fixed-rate
  codec) and are surfaced as warnings only. There is no external decoder to
  install — the app is fully self-contained for the supported workflow.
- Barcode candidate selection exists; artist/title MusicBrainz search does not.
- New artwork/assets must be inside the album directory (no external file
  copying yet).
- Embedded artwork covers the MusicPack artwork contract only: JPEG/PNG
  front and role pictures from FLAC `PICTURE` blocks and the APEv2 front
  cover, extracted byte-exactly at build. Other embedded image formats
  (WebP/AVIF/GIF/…) are ignored, embedded artwork is not previewed in the
  UI before a build, and images are never resized or transcoded.
- The standalone `.app` is built for the host architecture only (arm64 or
  x86_64, not yet universal).
- No signing/notarization/distribution: the bundle is ad-hoc signed by
  Tauri for local runs; notarized, signed releases and a GitHub Actions
  packaging job are future work (the build/smoke scripts are structured so a
  CI job is a thin wrapper).

## Sonic analysis

Sonic analysis computes a content-based audio embedding per track (and a
deterministic album embedding) into the package's optional
`analysis/sonic.json` (container format `musicpack-sonic` v1 — frozen and
model-independent; see `specs/musicpack-sonic-v1.md`). The UI exposes a
**model-independent** "Sonic Analysis" panel: profile *MusicPack OpenL3 v1*
(`musicpack-sonic-openl3-v1`, the default permissive profile — not a
permanent normative model), with states *not analysed → analysing n/m
tracks (cancelable) → ready / ready-with-warnings / error*, and a
re-analyse action. Analysis is never started automatically; a package can
always be built without sonic.

```text
Sonic Analysis panel (Author)
      ↓  sonic_analyze / sonic_cancel (Tauri commands)
AuthorService → spawns `musicpack-sonic` (the analyzer binary)
      ↓  job JSON (draft audio paths + app-data model/cache/output dirs)
      ↓  progress events (sonic-progress) + cancellation (SIGTERM)
sonic.json written to the app data directory (outside the package)
      ↓  Draft.sonicAnalysis.path
build-draft (create_package) copies it to analysis/sonic.json, validates it
      and writes the manifest's analysis[] reference (sha256-protected)
```

### The analyzer (`musicpack-sonic`)

The analyzer lives in `sonic/` (C11 + ONNX Runtime, single-threaded for
determinism): it decodes MPC/FLAC/WAV to mono float32, resamples with a
faithful port of resampy `kaiser_best`, runs the mel frontend (kapre
STFT/mel/decibel) and the SHA-256-pinned post-frontend ONNX graph, pools
with mean-norm and aggregates the album equal-track. Compatibility against
the research harness is measured by `research/sonic/compat_measure.py
--c-doc` (gates: cosine ≥ 0.9999, meandiff ≤ 1e-4, maxdiff ≤ 2e-3 — all
PASS on the deterministic corpus). libmusicpack remains the authority on
Sonic semantics; the analyzer never reimplements them.

The model is **not bundled** in the `.app`. The ONNX Runtime runtime is
bundled, but the ~18 MB post-frontend ONNX artifact is downloaded once on
first use and verified against a pinned SHA-256 (`fc51d01d…`, 18,742,941
bytes) before activation. Acquisition is trusted Author application logic
(`src-tauri/src/sonic_model.rs`) — a package-provided profile id can never
trigger a download or model execution. The model cache lives at:

```text
Application Support/MusicPack Author/sonic/models/musicpack-sonic-openl3-v1/openl3_post.onnx
```

(resolved through the platform-native Tauri app-data path, never hardcoded).
Offline with a valid cached model, analysis works normally; offline without
one, the panel reports that the model could not be downloaded and the package
can still be built without sonic. Per-track embeddings are cached by audio
SHA-256 + profile + weights, so re-analysis of unchanged audio is free.

The model artifact is generated **reproducibly** from the pinned OpenL3 0.4.0
weights (CC BY 4.0, marl/openl3) by `research/sonic/convert_openl3.py`;
normal users download the already-produced, SHA-pinned artifact from the
immutable release asset (`scripts/publish-sonic-model.sh`), never a `latest`
asset and never a Python/ONNX-conversion step.

### Backend resolution

The analyzer resolves like the CLI but separately: a packaged app uses the
`musicpack-sonic` sidecar next to the bundled CLI; development uses
`MUSICPACK_SONIC`, then the CMake build tree (`build/sonic/musicpack-sonic`).
It is only required when the user actually runs a sonic analysis.

### Standalone macOS implications

- The `.app` bundle gains the `musicpack-sonic` sidecar and a relocatable
  **ONNX Runtime dylib** (`Contents/Frameworks/libonnxruntime*.dylib`,
  loaded via `@loader_path/../Frameworks`). arm64 bundles ONNX Runtime
  1.28.0; x86_64 uses 1.23.0 (the last Intel-macOS ONNX Runtime release) —
  both pinned + checksummed by `scripts/build-author-macos.sh`.
- The ~18 MB post-frontend **model** is fetched once on first use
  (pinned + SHA-256-verified) into the app data directory; it is not in the
  bundle.
- The build runs `scripts/audit-author-macos.sh` as a gate: it fails on a
  missing piece, a Homebrew/local/external dependency, an absolute rpath, or
  a mixed architecture.
- Analysis RAM is far below the research TensorFlow stack (~1.9 GB): the
  ONNX Runtime path runs a single-threaded session with a few hundred MB.
- arm64 and x86_64 both build (host-architecture only); a universal build
  remains future work (the analyzer is compiled per-host like the CLI
  sidecar, and ONNX Runtime would need a universal build).

### Clean-machine smoke procedure

On a clean macOS user account (or equivalent isolated environment):

1. `./scripts/build-author-macos.sh` → `MusicPack Author.app`.
2. Confirm no Homebrew ONNX Runtime is reachable via loader paths
   (`echo $DYLD_LIBRARY_PATH` empty; `brew list | grep onnx` nothing).
3. Launch Author, import an album, click **Analyse Sonic**.
4. Confirm the first-use model acquisition (~18 MB) with progress and
   SHA-256 verification.
5. Confirm the analysis completes and the `.mpack` builds.
6. `musicpack verify <album>.mpack --json` — confirm `analysis/sonic.json`
   is present and valid.
7. Quit/relaunch Author and re-analyse — confirm the cached model is reused
   with no network access.

(Verified on this machine up to and including the build/audit/smoke gates and
the analyzer's relocatable load; the full GUI click-through + first-use
download is the remaining manual step on a clean Mac.)

---

## R2: home in `musicpack-core` (migration notes)

The Author now lives beside `crates/` in the Rust workspace repository.
`author/src-tauri` is **excluded from the root Cargo workspace** (own
`Cargo.lock`, Tauri toolchain, bundler profile) — build it with `cargo`
commands inside `author/src-tauri` or via `npm run tauri`. The legacy
repository is the immutable oracle this tree was migrated from
(`docs/r2-migration-map.md`).

### Tauri command classification (R2 audit)

The Tauri host is a **thin native boundary**; domain logic must not accrete
here. Every registered command, classified:

| command | class | notes |
|---|---|---|
| `backend_info` | UI capability probe | reports CLI/sidecar availability |
| `inspect_album`, `validate_draft`, `identify_draft`, `create_package`, `verify_package`, `create_mpak`, `pack_package` | **package authoring** (delegated) | shell out to the `musicpack` CLI sidecar in JSON mode via `author_service.rs`; no parsing of human CLI text. R3.5: `create_package`/`create_mpak` additionally attach the draft's track-linked lyrics through `musicpack-core` after the sidecar build (the sidecar predates the concept); `validate_draft` returns the C verdict with host-side lyric findings merged, so invalid drafts surface structured `{ok:false,…}` verdicts instead of a bare CLI error. `create_package`/`create_mpak` take an `AppHandle` and emit a `build-progress` event per pipeline phase (plus per track in the track-granular phases) so a long build is observable — see the workflow's step 8 |
| `encode_tracks`, `encode_cancel` | **audio/media** (delegated) | `mpcenc` sidecar; progress events |
| `waveform_analyze`, `waveform_cancel` | **audio/media** (in-host) | envelope computation over the native decoder |
| `sonic_analyze`, `sonic_cancel`, `sonic_model_status` | **audio/media** (in-host) | ONNX Runtime model host (`sonic_model.rs`) |
| `read_image` | **filesystem** | base64 image read for previews |
| `lyrics_probe` | **filesystem** | validates one lyric file with the core lyrics profile for the track editor (immediate feedback; authoritative gates are `validate-draft` and the build) |
| `draft_save`, `draft_load`, `draft_clear`, `recents_list`, `recents_add` | **filesystem/persistence** | app-data JSON persistence |
| `cleanup_staging` | **filesystem** | staging dir hygiene |

Plugins: `dialog` (pickers) and `opener` (reveal in Finder) are the only
native UI bridges, surfaced through the injectable `PluginFacade` in
`app/src/lib/api.ts` (tests substitute fakes, mirroring the web ApiClient
design). Nothing here is an obsolete compatibility layer.

### Authoring implementation inventory (R4 input — do not act yet)

| capability | today | Rust availability in this repo |
|---|---|---|
| package creation (draft → `.mpack` dir) | `musicpack` CLI sidecar (legacy C) | `crates/musicpack` CLI + core have the format writers; CLI-parity audit pending |
| validate / verify | `musicpack` CLI sidecar | core validators exist; same audit |
| `.mpak` export (`pack`) | `musicpack` CLI sidecar | MPAK writer exists in core |
| identity (MusicBrainz) | in-host `musicbrainz.rs` (HTTP) | not in core (network task; fine in host) |
| Musepack encoding | `mpcenc` sidecar (legacy C encoder) | `musicpack-musepack-encoder` crate (LGPL) — candidate |
| sonic analysis | in-host ONNX (`sonic_model.rs`) | research scope; stays host-side |
| waveform generation | in-host (native decode) | core audio primitives exist |
| draft persistence / recents | in-host filesystem | host concern, stays |

The R4 move is to swap the legacy `musicpack` sidecar for this
repository's Rust CLI once the JSON command surface is proven equivalent
(command-by-command differential against the oracle). Until then the
frozen sidecar binaries (gitignored; cut from the oracle build) keep
behavior identical.

R3.5 note (track-linked lyrics, not the R4 swap): the sidecar predates
the `track.lyrics` concept, so `author_service.rs` stages the draft's
per-track lyric files before the build and attaches them afterwards
(`src-tauri/src/track_lyrics.rs`) using `musicpack-core` primitives only
— manifest parse/mutate/canonical-write, the lyrics parser, content
hashing, package verification. No package semantics were reimplemented
in the host or the frontend; the frontend carries `{path, lang?}` per
track and never filters, ranks, or previews lyric content.

### CI

`.github/workflows/author.yml`: type-check + unit/component tests +
`build:web` on every `author/**` change, and a `cargo check` of the Tauri
host (with Tauri's Linux system packages; placeholder sidecars suffice for
type-checking). Desktop bundling/signing stays a local/release task.
