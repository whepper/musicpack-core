# musicpack-server

The self-hosted MusicPack library server — the Rust port of the legacy C
`musicpack-server`, built on `musicpack-core`.

**Stage 1 (complete):** CLI/configuration skeleton plus the byte-compatible
SQLite persistence layer. The store opens databases created by the legacy C
server without conversion (same ten forward-only migrations plus the
Rust-defined additive v11, same pragmas)
and vice versa; `token create|list|revoke` is functional end-to-end.

**Stage 2 (complete):** collector identity (`identity`: package
fingerprint, `group_key`, `release_key` as pure functions, proven against
20 C-generated golden vectors) and the bounded filesystem discovery walk
(`discover`: `.mpack` candidates with identity keys, no database writes).

**Stage 3 (complete):** the ingestion state machine (`ingest`: fast
paths, moves, ownership/mirror/conflict arbitration, in-place content
sync with stable ids, sweep) with table-by-table C-oracle parity
(`tests/ingest_oracle.rs`); codec probing without serving (`probe`);
`scan`/`verify` wired to the real implementation.

**Stage 4 (complete):** the read-only JSON API (`http`: hand-rolled
std-only blocking HTTP/1.1, no new dependencies) with live C-oracle
parity (`tests/api_oracle.rs`): health, session lifecycle, artists,
albums, releases, tracks, library status; `serve` wired to the real
server.

**Stage 5 (complete):** secure byte/media serving (`store` resolvers +
`media` resolution + `http::{range,serve}` decision tree, same transport)
with live C-oracle parity (`tests/media_oracle.rs`): track/representation
audio, waveforms and assets with exact 200/206/304/416, strong content-SHA
ETags, `If-Range`, fstat sizes, 64 KiB streaming, and the 64 KiB
Web block-reader contract.

**Stage 6 (complete):** static hosting + SPA fallback (`--static-dir`,
COOP/COEP, `pathsafe` containment — `tests/static_oracle.rs`) and the
library job APIs (`POST /api/v1/library/scan|verify`, live
`GET /api/v1/library/status`, single-slot worker on its own SQLite
connection, `verify_library` verdict persistence — `tests/jobs_oracle.rs`),
all differentially proven against the live C server.

**Stage 7 (complete): `.mpak` container sources.** A library source is now
either a `.mpack` directory bundle or a single-file `.mpak` container.
`discover::classify` is the one classification point; `source::PackageSource`
is the one abstraction the rest of the pipeline asks, so identity, the
missing-object count, the codec probe, verification and the content sync never
branch on the kind. Containers are read through `musicpack-core`'s
`MpakBackend` (the authoritative implementation) over its native `FileSource`,
never a second parser. An indexed container track carries the canonical
`mpak:<container>#<member>` source, produced by
`musicpack_core::player::source_url` — the same definition the client's range
transport uses — and is served over the existing media endpoint as a byte range
inside its container: HTTP ranges stay member-relative, the container's size is
never exposed, and a member is reached only through core's member table
(`docs/mpak-source.md`). No schema change: `packages.path` is the locator and
`audio_objects.relative_path` the member. Covered by `tests/mpak_ingest.rs`
and `tests/mpak_media_serving.rs`.

This is a deliberate **superset** of the C reference, which only walked
`.mpack` directories; `tests/discovery.rs` pins the difference while the
differential oracles continue to compare only valid packages.

**Stage 8 (complete): model-neutral music similarity (Slice 0).** The server
indexes supplied `.msim` v1 similarity documents (FORMAT_SPEC.md) into a
profile-partitioned exact-cosine index and serves ordered similar-track
queries — with no model, no runtime, no inference and no ANN anywhere in
this crate (`src/similarity/`, schema v12, `tests/similarity_server.rs`):

- **Optional.** A package without a `similarity` document is completely
  valid; an empty index is a normal state, not an error.
- **Model-neutral.** Documents are validated structurally; the server never
  runs a model and never downloads weights. Discogs-EffNet is a candidate
  profile, not a cleared production profile; no profile is bundled,
  named in code, or treated as default.
- **Profile-isolated.** The header `profile_fingerprint` is the partition
  key; the display `profile_id` is never a comparison key. Vectors from
  different fingerprints are never compared, mixed, or converted.
- **Not identity.** Similarity rows key off track row ids and cascade with
  the content graph; `group_key`, `release_key` and package identity are
  untouched by similarity data.
- **Exact cosine, Slice 0.** Scalar f64 loop in element order; ties break by
  ascending track id (deterministic; a content-defined key remains open as
  architecture decision D-4). `f32le` is the indexed representation
  (G-6 closed as KEEP F32LE); `f16le` documents parse but are not indexed.
  ANN is deliberately deferred.
- **Fail-closed.** Malformed, incoherent, dimension-mismatched or
  unsupported documents yield no rows while the package stays valid and
  playable; unknown profiles, vectorless tracks and empty indexes answer
  404 `similarity_unavailable`, never a fabricated score; neighbours from
  non-visible packages are never returned.
- **Licensing is separate from the mechanism.** Whether a concrete
  model/profile may be supported — and whether generated embeddings are
  legally unrestricted — is unresolved (ADR 0017 G-1…G-4) and is not decided
  by this code. No claim is made here either way.

```text
musicpack-server scan    --library DIR [--database PATH] [--verify]
musicpack-server serve   --library DIR [--database PATH] [--listen IP]
                          [--port N] [--no-scan] ...
musicpack-server verify  --library DIR [--database PATH]
musicpack-server token create --name NAME | list | revoke <id>
```

Configuration precedence: flags > `MUSICPACK_LIBRARY` / `MUSICPACK_DATABASE`
/ `MUSICPACK_LISTEN` / `MUSICPACK_PORT` > defaults (`./library`,
`./library.db`, loopback `127.0.0.1:8080`). Exit codes: 0 success, 1
failure, 2 usage error.

Not yet: docker/compose + deployment docs and the cutover sign-off (see
`docs/server-cutover-checklist.md`). `scan`, `verify`, `serve`, `token`
and the full HTTP contract — JSON API, byte serving, static hosting and
library jobs — are functional; out-of-scope HTTP routes answer explicit
404s.

## Crate boundary

- Depends on `musicpack-core` (path dependency); nothing depends on this
  crate; it is **native-only** and never a WASM target.
- The one C dependency in the workspace lives here: `rusqlite` with
  `bundled` SQLite (contains C and internal `unsafe` inside the
  dependency). The crate itself is `#![forbid(unsafe_code)]`. See
  `docs/server-migration.md` (D-S2/O-S1) and the root `AGENTS.md` boundary
  note.

## Tests

`tests/db_compat.rs` is the compatibility gate: it opens the committed
**C-created** reference database (`tests/data/library-c-reference.db`),
proves a Rust-migrated database is structurally identical (`sqlite_master`
including DDL text), verifies forward-only/too-new semantics, and — when
`MUSICPACK_LEGACY_SERVER` points at a built legacy binary — checks the
reverse direction (the C server lists/revokes a token written by the Rust
store). Without that variable it skips with an explicit notice.
