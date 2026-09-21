# ADR 0004: Retain Svelte 5 for the Web Player and Author; reorganize, do not rewrite

- Status: Accepted (2026-09-20)

## Context

The production Player (legacy repo `web/`) and the Author (`author/`) are
Svelte 5 + TypeScript apps with **zero runtime dependencies**, a shared
dark-v2 design language (tokenized `theme.css`, 2,303 lines), a
framework-free domain layer (`player-core` TS; Rust below it), 30 vitest unit
files (incl. a purity gate over player-core), and 25+ Playwright e2e specs
(incl. the 11-spec Rust-backend program). Alternatives considered: React,
Vue, Solid, Flutter Web, React Native Web.

## Decision

Keep Svelte 5. Invest in reorganization (component splits, token extraction,
canonical screen specs, visual regression), not replacement.

Decisive, MusicPack-specific reasons:
- The framework's only job is "premium dark UI over a typed domain" — the
  portable parts already live outside it, so a framework swap buys no
  architectural capability.
- The Author shares the language and the design system; one framework keeps
  one design language.
- Rewriting would destroy 55+ test files of oracle value and the largest
  body of UX truth in the project.
- Zero runtime dependencies and compiler-enforced templates keep the agent
  surface small.

## Migration boundary (recorded so the option stays cheap, not taken)

If ever needed: `app/src/lib/ui` + `app/src/lib/state` swap out;
`player-core`, `api/`, `offline/` engines, workers, and everything Rust stay.
No trigger for this exists today.

## Reorganization commitments (when the web tree lives in this repo)

- Split `AlbumPage.svelte` (568) and `TrackPage.svelte` (563) into
  container + section components; new components stay well under ~300 lines.
- Extract design tokens into a documented `tokens.css` (see the UX section of
  `docs/architecture-review.md`).
- Canonical screen specs + Playwright visual baselines.
- Keep the hand-rolled store primitives and the "domain logic never in
  `.svelte` files" rule; runes migration only with a concrete win on a
  touched component.
