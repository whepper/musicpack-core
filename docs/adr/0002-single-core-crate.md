# ADR 0002: Keep musicpack-core as a single crate; split only on explicit triggers

- Status: Accepted (2026-09-20)

## Context

`musicpack-core` (repo root, ~15.5k LOC) contains format (`.mpack` manifest,
MPAK container, waveform, canonical serialization), storage backends
(`PackageBackend` trait), validation, audio decoding + analysis, the playback
domain (`core::player`), and representation policy. It is wasm-clean via
dependency choice (only `sha2` + `claxon`) and CI checks, not via cfg forks.

## Decision

One crate. No `musicpack-format` / `musicpack-audio` / `musicpack-player`
split, and no features/`no_std` fork, for the foreseeable future.

What core gains later, on evidence (not speculation):
- **Directory-package builder** (canonical `.mpack` dir writer) when the Author
  phase starts — the writable sibling of `verify_directory`.
- **Identity promotion** (fingerprint/group/release keys) from the server crate
  when the Author's Rust pipeline needs it (the trigger named by O-S4).

What never enters core: HTTP, auth, sessions, SQL, library/ingest projection,
offline orchestration, MusicBrainz, draft/authoring workflow, UI state.

## Split triggers (any one justifies revisiting)

1. A consumer needs format-only functionality where the audio/player code or
   `claxon` compile cost is a real problem (e.g. a minimal FFI surface).
2. wasm binary size forces crate-level tree-shaking.
3. Compile times become a measured development bottleneck.

If triggered, split along the existing module seams — they are already clean.

## Consequences

- Compatibility oracles, fixtures, and docs stay in one place; agents and
  reviewers navigate one core.
- The crate is bigger than strictly necessary for some hypothetical consumer;
  accepted until that consumer exists.
