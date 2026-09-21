# Author runtime (R4.3 cutover)

Status: implemented. Decision record: `docs/adr/0012-author-runtime-cutover.md`.
Pipeline reference: `docs/author-pipeline.md`.

This document describes the Author application's runtime after the R4.3
cutover: what the Tauri UI talks to, how errors and progress map, which
legacy dependencies were removed, and what remains.

## 1. Final dependency graph

```text
Svelte 5 UI (author/app)
   │  invoke() — typed AuthorApi
   ▼
Tauri commands (author/src-tauri/src/lib.rs)
   │  RustBackend            ← default
   │  AuthorService          ← legacy escape hatch (MUSICPACK_AUTHOR_LEGACY=1)
   ▼
musicpack-author            ← draft JSON, encode, waveform, identify, .mpak
   ├─ musicpack-musepack-encoder   (isolated LGPL-2.1-or-later)
   ▼
musicpack-core              ← format, manifest, verify, .mpak, identity, audio
   │
   └─ core storage: directory::pack_directory / verify_directory
```

Direction is one-way: the host depends on `musicpack-author`, which depends
on `musicpack-core`. Neither core crate depends on Tauri, and
`musicpack-author` never invokes a subprocess.

## 2. UI ↔ Rust boundary

`RustBackend` (`author/src-tauri/src/rust_backend.rs`) implements the same
command surface the frontend already used:

| Command | Rust behaviour |
|---|---|
| `backend_info` | reports `authorApi = 8`, location `rust` |
| `inspect_album` | `musicpack_author::inspect::package_to_draft` (`.mpack` dir → draft JSON) |
| `validate_draft` | `pipeline::validate_json` + R3.5 lyric findings |
| `identify_draft` | host transport + `identify_apply_json` / `identify_candidates_json` |
| `create_package` | `pipeline::run` (encode + waveform + loudness + build + verify), optional atomic replace |
| `create_mpak` | `pipeline::run` with `mpak: Some(..)`, intermediate `.mpack` removed |
| `pack_package` | core verify, then `storage::directory::pack_directory` |
| `verify_package` | core `verify_directory` / `verify_mpak_file` → `{ok, errors, warnings}` |
| `encode_tracks` | `pipeline::encode_stage_with` (per-track progress) |
| `waveform_analyze` | `pipeline::waveform_stage_with` (per-track progress) |
| `lyrics_probe` | core lyrics parse (unchanged) |
| `sonic_analyze` | retired: typed `sonic_retired` error |

`create_package` honours `waveformAnalysis.status == "disabled"` from the
draft; otherwise waveforms are generated. Loudness is always measured by the
core builder. `sync_tags` is accepted and ignored (see §7).

## 3. API version

The UI↔host contract is versioned explicitly: `AUTHOR_API = 8` in
`rust_backend.rs`, reported through `backend_info.authorApi` and rendered by
the backend banner. The legacy CLI keeps the C `MUSICPACK_AUTHOR_API` value;
a mismatch is a hard error, never auto-negotiated. The version lives at the
host boundary — `musicpack-core`/`musicpack-author` carry no UI API version.

## 4. Progress

Progress is stage/track-level, derived from real work (never fabricated):

- **encode** — one `encode-progress` `{event:"track", done, total, disc,
  track, title, status:"ok"}` event per encoded track, emitted from the
  stage's per-track callback.
- **waveform** — one `waveform-progress` `{event:"track", done, total, disc,
  track, status:"ok", points, sha256, path}` event per track.
- **build/validate/identify/pack** — command results (no fake percentages).

Cancellation sets a shared flag; the stages stop between tracks and return a
`cancelled` result. There are no misleading percentage estimates.

## 5. Error mapping

`HostError { code, message }` is the single error shape. Codes:

| Code | Meaning |
|---|---|
| `invalid_draft` | malformed or structurally invalid draft |
| `invalid_lyrics` | unreadable/malformed lyric input |
| `missing_source` / `io_failed` | missing source or filesystem failure |
| `unsupported` | unsupported audio / encoder configuration |
| `encode_failed` | decoder/encoder failure (disc+track context) |
| `build_failed` | core builder/verification failure |
| `pack_failed` | `.mpak` packing failure |
| `verification_failed` | source package failed verification before packing |
| `output_exists` | destination already exists (no replace) |
| `musicbrainz_failed` | transport or response validation failure |
| `sonic_retired` | sonic analysis is not part of the runtime |
| `cancelled` | user cancellation |

The frontend surfaces `message` (CreateDialog and Export now read structured
or string errors instead of collapsing to "Create failed"), so failures stay
distinguishable and actionable.

## 6. MusicBrainz

Transport is host-side (`musicbrainz.rs`, `ureq`, paced ~1/s); matching and
application are `musicpack-author::identify` (unchanged R4.2 ladder:
release-id → exact; barcode / ISRC+count → confirmed; ISRC or exact title →
probable; else none). Barcode search returns candidates without selecting.
Responses are parsed and validated before application; malformed responses
fail closed. Tests use a deterministic static provider and offline documents —
the suite never depends on live MusicBrainz.

## 7. Sonic and APEv2 dispositions

- **Sonic — retired from the runtime.** No Rust equivalent; not required for
  package correctness. The sidecar is no longer bundled, `sonic_analyze`
  returns `sonic_retired`, and the panel shows "Sonic analysis is not part of
  the Rust authoring runtime." Existing manifest `analysis[]` references are
  carried through the draft so a rebuild preserves them.
- **`--sync-tags` / APEv2 — retired.** The Rust path rebuilds the package
  deterministically; in-place APEv2 tag projection is not part of authoring
  correctness. The flag is accepted for UI compatibility and ignored.

## 8. Encoder limitations

The isolated Rust encoder's frozen psychoacoustic tables cover
`{4,5,6,7} @ 44100 Hz` and `q5 @ {48000, 37800, 32000} Hz`. Outside that
matrix the runtime fails closed with `unsupported` — it never silently
remaps quality or sample rate. Concretely, for the current Author product:

- the default **q6 works at 44.1 kHz** (the common CD case);
- **48 kHz (and 37.8/32 kHz) support only q5**;
- **q8 is unsupported** by the Rust encoder.

This is a genuine capability gap of the encoder crate, not of the runtime.
Extending the frozen tables is encoder-crate work; the legacy escape hatch
covers those specific configurations in development until then. Sources
deeper than 16 bits are reduced to the top 16 bits (documented fidelity gap;
exact for 16-bit sources).

## 9. Replacement safety

`create_package(..., replace: true)` builds the new package in a sibling
staging directory and swaps it in (old → backup, new → destination, backup
removed; rollback on failure). A failed build never destroys a previously
valid package. A build without `replace` refuses an existing destination.
This is an application-boundary operation; the core builder remains a pure
constructor.

## 10. Legacy removal

- `externalBin` sidecars removed from `author/src-tauri/tauri.conf.json`
  (`musicpack`, `musicpack-sonic`, `mpcenc`).
- The default runtime no longer resolves, probes on `PATH`, or bundles any
  legacy binary. `MUSICPACK_CLI`/`MUSICPACK_MPCENC`/`MUSICPACK_SONIC` are
  consulted only when the escape hatch is enabled.
- The CI placeholder-sidecar step was removed.
- `author_service.rs` (the C-CLI service) is retained as the explicit,
  non-default escape hatch.

## 11. Oracle retention

The C implementation remains a test/compatibility oracle:

- `crates/musicpack-author/tests/pipeline.rs` — Rust encoder byte-identical
  to `mpcenc`; pipeline manifest byte-identical to C `build-draft`; C
  verifier accepts Rust packages.
- `crates/musicpack-server/tests/author_pipeline.rs` — Rust-authored package
  ingests through the real server path.
- The committed corpora (`encoder_whole`, `cut_compat`, manifest/MPAK
  byte-identity, analysis/audio C references) are unchanged.
- `author/src-tauri/tests/rust_backend.rs` — host vertical plus the permanent
  no-subprocess invariant (source scan + poisoned `PATH`).
