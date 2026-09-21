# Core package-directory builder (R4.1)

Status: implemented (`src/authoring/`, `tests/package_build.rs`). Decision
record: `docs/adr/0010-core-package-builder.md`.

This document records the `.mpack` package contract the builder must satisfy
(derived from the existing reader/verifier, not invented here) and the exact
contract of the new Rust builder.

## 1. The `.mpack` v1 directory contract

A directory bundle is valid exactly when the core verifier says so. From the
implementation (`src/validation/mod.rs`, `src/storage/directory.rs`,
`src/format/manifest/parse.rs`, `specs/musicpack-v1.md`):

- **`manifest.json`** at the package root is the only normative file. It is
  a strict JSON document parsed by the core parser; canonical serialization
  is fixed (2-space indent, fixed key order, absent optionals omitted, one
  trailing newline) and is what the package fingerprint hashes.
- **Referenced assets** are the nested arrays the manifest declares. There
  is no root `assets` array in v1. Every referenced path is:
  - a canonical package-relative path (`src/format/path.rs`): no
    backslashes, colons, control characters, absolute lead, empty, `.` or
    `..` segments, or trailing `/`; ≤ 4096 bytes;
  - globally unique across the whole package;
  - backed by a regular file (no symlink, no hard link) inside the package;
  - within budget: ≤ 8 GiB per file, ≤ 64 GiB aggregate, ≤ 4096 referenced
    assets.
- **Directory conventions** (layout, not format): `audio/` for track audio,
  `artwork/` for role-tagged images, `booklet/`, `lyrics/`, `extras/`,
  and `analysis/` (`analysis/sonic.json`, `analysis/waveform/<DD>-<TT>.wfm`).
  The manifest `path` is the only linkage; a `path` may legally live
  elsewhere.
- **Waveform references** additionally require `payload_bytes == points × 2`
  and ≤ 1 728 000 bytes.
- **Loudness** is optional: per-track `loudness` needs both `trackLUFS` and
  `truePeakDbTP` in `[-70, 6]`; album `loudness` is measured as one
  concatenated program and carries the `"ITU-R BS.1770-5"` algorithm string.
- **Lyrics**: root-level `lyrics[]` is opaque `{path, sha256}`; per-track
  `track.lyrics[]` adds `{path, sha256, lang?}` (R3.2–R3.6,
  `docs/musicpack-lyrics-v1.md`). `lang` is free-form, non-empty, no
  control characters.
- **Identity-bearing fields** are the MusicBrainz ids, album title, release
  date/type and artist credits (`group_key`) and the release edition fields
  (`release_key`); the package fingerprint is the canonical manifest
  itself. Nothing else is identity (paths are not).
- **Derived, not authored**: every `sha256`; waveform `points`; track
  `duration` and `loudness`; album `loudness`; the fingerprint and keys.
- **Deterministic**: the format has no timestamp field; provenance is
  optional and deliberately not written by the builder. Output must be a
  pure function of the draft and the source bytes.

## 2. The authoring draft

`musicpack_core::authoring::AuthoringDraft` reuses the manifest's metadata
types (`Album`, `Release`, `Identifiers`, `Identity`, `Source`, `Artist`,
`TrackIdentifiers`, `TrackSource`, `SourceAudio`, `Provenance`, `Analysis`)
and adds only source-file references the manifest cannot express:

| Draft type | Authored | Derived by the builder |
|---|---|---|
| `DraftAsset { path, source }` | package path + source file | `sha256` |
| `DraftWaveform { path, source }` | package path + payload file | `sha256`, `points` |
| `DraftLyrics { path, source, lang? }` | package path + `.lrc` + lang | `sha256` |
| `DraftRepresentation { path, source, label?, codec? }` | as above | `sha256` |
| `DraftArtwork { role, asset }` | role + asset | `sha256` |
| `DraftAnalysis { kind, profile?, asset }` | kind/profile + asset | `sha256` |
| `AuthoringDraft` metadata | copied verbatim | — |
| — | — | duration, loudness, fingerprint, keys, verification |

`source` paths are resolved against the builder's `source_root`, must be
relative and must stay inside it (absolute paths, `..` and symlink escapes
are rejected). `path` values are canonical package-relative paths.

## 3. Builder contract

```rust
pub fn build_directory(
    draft: &AuthoringDraft,
    source_root: &Path,
    output: &Path,
    options: &BuildOptions,
) -> Result<BuildOutcome, Error>;
```

1. **Validate** the draft (structure, numbering, counts, lyrics language,
   path safety and package-wide uniqueness) before touching the filesystem.
2. **Stage** in a sibling `.‹name›.build-‹pid›` directory.
3. **Materialize** every asset (copy + SHA-256 in one streaming pass),
   creating parent directories as needed.
4. **Measure** loudness/duration when `LoudnessMode::Measure` (the default;
   the reference `build-draft` default): one meter per track and one album
   meter fed the concatenated program in manifest order. Tracks must agree
   on channel count and sample rate (1–2 channels); `LoudnessMode::Omit`
   skips measurement entirely (`--no-loudness`).
5. **Assemble** the manifest from the draft plus the derived facts and
   serialize it canonically (the canonical writer re-validates before
   writing).
6. **Verify** the staged directory with the shared
   `validation::verify`; a package that would not verify is never published.
7. **Publish** with a single `rename`. On any failure the staging tree is
   removed and the destination is left untouched.

`BuildOutcome` returns the `Manifest`, the canonical `manifest_sha256`, the
promoted identity keys (`fingerprint`, `group_key`, `release_key`), the
verification `Report`, and the output path. The existing
`storage::directory::pack_directory` consumes the result unchanged for
`.mpak` export — there is no second MPAK writer.

### Errors

Failures use the core error taxonomy (`src/error.rs`):

- `Error::Invalid` — malformed drafts (empty media, no album artists, empty
  title, duplicate disc/track numbers, duplicate paths, bad lyrics language,
  odd/oversized waveform payloads, non-uniform album metering, measured
  loudness out of range).
- `Error::Path` — a package-relative path violates the canonical rules.
- `Error::Missing` — a referenced source file does not exist.
- `Error::Io` — filesystem failures.
- `Error::Checksum`/`Error::Invalid` — a package that fails final
  verification (reported with the verifier's findings).

## 4. Compatibility with the C builder

- **Byte identity:** for the same semantic package the Rust builder's
  `manifest.json` is byte-identical to the C `build-draft` output, and the
  copied audio bytes are identical. `tests/package_build.rs`
  (`c_builder_and_rust_builder_agree_byte_for_byte`) asserts this against
  the reference CLI when it is available; it skips otherwise.
- **Semantic compatibility:** the C verifier accepts the Rust-built package
  (`rust_built_package_is_accepted_by_the_c_verifier`).
- **Intentional difference:** C silently mismeasures an album whose tracks
  differ in sample rate or channel count (the first track's configuration
  wins, per `src/audio/mod.rs`). The builder rejects that with a typed
  error instead of writing a wrong measurement; `LoudnessMode::Omit` is the
  escape. This is a deliberate robustness difference, not a format change.

## 5. Out of scope (R4.2)

Encoding audio, synthesizing waveform envelopes, MusicBrainz matching, the
authoring JSON command surface, draft persistence and the Tauri integration
are **not** part of R4.1. The builder accepts a pre-computed waveform
payload and materializes it; it never generates one.
