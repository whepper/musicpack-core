# author/ — MusicPack Author (Tauri 2)

Frontend conventions for the desktop authoring app. The R2 sections of
`author/README.md` document the Tauri command classification and the
authoring implementation inventory — read them before changes.

Non-negotiables:

- **Tauri is a thin native host, not the business-logic layer.** Commands
  are classified (filesystem / package-authoring / audio-media / UI
  probe) in the README; new capability belongs in one of those buckets,
  and package/authoring semantics must stay in the Rust authoring
  pipeline (`musicpack-author` / `musicpack-core`) — never in ad-hoc
  TypeScript or in new Tauri commands. The default backend is the
  in-process `RustBackend`; the legacy C-CLI `AuthorService` is a
  non-default, development-only escape hatch behind
  `MUSICPACK_AUTHOR_LEGACY=1` (see `docs/author-runtime.md`).
- **One API boundary**: `app/src/lib/api.ts` (typed `AuthorApi` over
  injectable `invoke`/plugin facades). Components never call `invoke`
  directly.
- **The frontend never parses CLI text.** Every backend interaction is
  structured JSON through the Rust `RustBackend`/`AuthorService`.
- `src-tauri` is **excluded from the root Cargo workspace** (own lockfile
  and toolchain). Build it from inside `author/src-tauri` or via
  `npm run tauri`.
- `src-tauri/{target,gen}/` are gitignored. The legacy sidecar binaries
  are **no longer bundled** (R4.3 removed them from `externalBin`); they
  remain only as an optional development oracle for the escape hatch.
- Tests: vitest unit + component suites (`tests/unit`, `tests/component`)
  with the injectable fakes; `svelte-check` must stay clean.

Commands: `npm run dev | build:web | check | test | tauri`.
