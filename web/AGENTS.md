# web/ — MusicPack web player

Frontend conventions for the Svelte 5 web player. Architecture, the WASM
pipeline, the state-ownership map, the API boundary, the design-token
rules, the accessibility baseline and the visual-regression process are
documented in `web/README.md` (R2 section) — read it before non-trivial
changes.

Non-negotiables:

- **Rust/WASM is the playback backend** (`chooseBackend` in
  `lib/playback/controller.ts`) for every codec it decodes — online
  (`http-range`) **and offline** (OPFS `local-file`) alike. The legacy
  Emscripten engine (`musepack-engine.ts` + frozen `musepack.{js,wasm}`) is
  an **oracle-only** compatibility shim (differential tests), never selected
  by product config — do not route new functionality through it.
- **Domain logic stays out of components.** Queue/transition/normalization
  rules live in `player-core/` (pure, ports-only) or in Rust
  (`crates/musicpack-{core,engine,wasm}`). Components own presentation and
  UI-local state only; derived display facts go into the `ui/*-facts.ts`
  pure modules (see `album-facts.ts` / `track-facts.ts`).
- **One API boundary**: `lib/api/client.ts`. Components never `fetch` API
  resources directly.
- **State ownership** follows the table in the README (server /
  application / playback / UI-local). Never duplicate playback state; the
  one canonical model is `player-core/src/player.ts`.
- **Design tokens**: consume `theme.css` CSS variables and primitive
  classes (`btn`, `badge`, `smallcaps`, …); never hard-code colors or
  sizes; every new interactive state defines hover/active/focus-visible
  together.
- **Generated artifacts**: `app/public/rust/` is produced by
  `scripts/build-wasm.mjs` — never commit it, never edit it. The frozen
  `musepack.{js,wasm}` are oracle artifacts — never edit, refresh only via
  a reviewed oracle re-cut (see `app/public/PROVENANCE-legacy-decoder.md`).
- **Tests**: unit (vitest) for every pure module; Playwright for user
  flows; `visual.spec.ts` baselines are re-cut with `--update-snapshots`
  only for *intentional* visual changes (review the image diffs).

Commands: `npm run dev | build | check | test | test:e2e` (all web-app
commands run the WASM staleness guard first). The e2e harness starts the
Rust server with a fixture library; the oracle repository is never
started or modified by tests.
