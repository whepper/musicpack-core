# ADR 0010: Core `.mpack` directory builder and identity promotion

- Status: Accepted (2026-09-21, R4.1)

## Context

The R3.7 audit (`docs/r3.7-r4-readiness.md` §13, R4.1) established that the
one structural blocker between the current state and a Rust-authoritative
Author pipeline is that Rust cannot *write* a `.mpack` directory. The core
owns the reader (manifest parser, canonical writer, verifier, MPAK container,
decoders, waveform/loudness primitives) and the server owns the collector
identity algorithm, but there is no writable sibling of `verify_directory`
and no shared identity for an authoring builder to reuse.

The C `build-draft` currently performs the whole authoring build. R4.1 must
establish the core primitive that later removes that dependency, without
turning `musicpack-core` into the Author application.

## Decision

1. **Add a package-directory builder to the core**
   (`src/authoring/`, portable native-target code; the directory adapter
   applies POSIX hardening on unix and the reference's own relaxed checks
   elsewhere — see ADR 0015). It takes an explicit [`AuthoringDraft`] plus source files and
   produces a verified `.mpack` directory.
2. **The draft is authored input only.** It carries metadata, structure and
   source references; it never carries hashes, fingerprints, asset ids,
   waveform point counts, duration, loudness or verification status. The
   builder derives everything the package contract requires.
3. **The builder writes, the shared verifier judges.** Construction is
   staged in a sibling directory and published with a single rename; the
   finished directory is checked by the existing `validation::verify`
   through the directory backend before publication. No second verifier and
   no second lyrics model are introduced.
4. **Promote collector identity into the core** (`src/identity.rs`:
   package fingerprint, `group_key`, `release_key`, MBID validity, manifest
   hash). It is pure over the manifest model, so the server re-exports it
   (`musicpack_server::identity`) and the C-generated golden vectors in
   `crates/musicpack-server/tests/data/identity/` continue to pin it.
5. **No CLI in R4.1.** The builder is a library primitive; the authoring
   JSON surface (`inspect`/`validate-draft`/`encode-draft`/`build-draft`/…)
   is R4.2. The builder is exercised by `tests/package_build.rs`.

## Consequences

- The Rust core can now produce `manifest.json` and the `.mpack` directory
  for a release, byte-identical to the C `build-draft` output for the same
  semantic package (proven by a differential test), and the C verifier
  accepts the result.
- Identity is available from the same manifest the builder writes, so the
  future Author pipeline can compute group/release keys without the C
  implementation and without a second copy of the algorithm.
- Encoding, waveform synthesis, MusicBrainz matching and the authoring
  protocol remain out of scope here; the builder accepts a pre-computed
  waveform payload and measures loudness itself using the existing
  `audio` primitives.
- ~~The builder is `unix`-only, consistent with the directory backend; the
  core's wasm32 requirement is preserved.~~ **Amended by ADR 0015:** the
  unix-only scope was overly broad (it conflated "not-wasm" with "unix" and
  broke the Windows build once `musicpack-author` consumed the module). The
  builder and directory adapter are portable native-target code with
  reference-matching per-platform checks; the wasm32 check still passes with
  no gate at all.
- The C `build-draft` becomes oracle-only for this slice; it is not removed.
