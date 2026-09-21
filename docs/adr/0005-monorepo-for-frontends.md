# ADR 0005: Bring the Web Player and Author into this repository at server cutover

- Status: Accepted (2026-09-20)

## Context

The production Web Player (`web/`) and the Author app (`author/`) live in the
legacy repository, which is by charter immutable as an oracle. Meanwhile:

- The web build already reaches across the boundary:
  `web/scripts/sync-wasm.sh:28-41` copies Rust wasm artifacts from
  `../musicpack-core`.
- API, engine, and player changes cannot be atomic across repos; there is no
  single CI run covering server + wasm + web.
- The legacy repo cannot receive non-legacy evolution by our own rules, yet
  the web client is production software that must keep evolving.
- LLM agents must navigate two repos to change one feature, and the
  compatibility-oracle role of the legacy repo is blurred by hosting living
  production code.

## Decision

Migrate `web/` and `author/` into this repository (`web/`, `apps/author/`) as
part of server stage 9 (cutover), per the roadmap phase R2 in
`docs/architecture-review.md`. The legacy copies are never modified; they
become the frozen behavioural oracles for the web player, the TS engines, and
the author workflows — exactly the pattern already used for the C server and
the TS player-core domain (which was ported into `core::player` with oracle
tests).

Until R2 lands, the coupling is accepted and mitigated by pinning the wasm
artifact consumed by `sync-wasm.sh`.

**Status note (R2, executed):** the migration is complete. The chosen
layout is top-level `web/` and `author/` (not `apps/author/` — it mirrors
the oracle layout, keeping imports and test paths stable). `sync-wasm.sh`
was replaced by `web/scripts/build-wasm.mjs` (cargo → wasm-bindgen →
public/rust, staleness-guarded); the Emscripten decoder is a committed
frozen oracle artifact (`web/app/public/PROVENANCE-legacy-decoder.md`).

## Consequences

- One CI run builds server + wasm + web and runs every suite; cross-layer
  changes become atomic.
- The legacy repo's role becomes unambiguous: oracle only.
- Per-directory AGENTS.md files are added for `web/` and `apps/`
  (frontend conventions cannot live in the Rust root AGENTS.md).
- Migration is a copy + CI wiring, not a rewrite: the app code moves as-is;
  reorganization happens afterwards per ADR 0004.
