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

Not implemented yet (later stages of `docs/server-migration.md`):
docker/compose + deployment docs and the cutover sign-off (see
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
