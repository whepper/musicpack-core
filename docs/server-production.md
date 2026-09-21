# MusicPack server — production runtime (R4.5)

Status: closed. Decisions: `docs/adr/0014-server-production-cutover.md`
(and the earlier `docs/adr/0009-library-jobs-and-shutdown.md`).
Migration history: `docs/server-migration.md`; cutover gate:
`docs/server-cutover-checklist.md`.

The production MusicPack server is the Rust `musicpack-server` binary. The
legacy C server is retained only as a behavioural/differential oracle.

## 1. Production architecture

```text
musicpack-server (Rust, native-only, #![forbid(unsafe_code)])
        │
        ├── musicpack-core      (format, manifest, verify, .mpak, identity, audio)
        └── SQLite              (bundled via rusqlite; the crate's only C)
                    └── the library directory (source of truth)
```

Runtime dependencies: the platform libc, one library directory, one database
file, and (for static hosting) a static tree. Nothing else — no legacy
`musicpack`/`libmusicpack`/`mpcenc`/`musicpack-sonic`, no FFmpeg, no
interpreter, no shell, no network egress. The release binary on macOS links
only `/usr/lib/libSystem.B.dylib` and `/usr/lib/libiconv.2.dylib`.

## 2. Release artifact

```sh
cargo build --release -p musicpack-server
# → target/release/musicpack-server  (single self-contained binary)
```

- Release profile of the workspace; no debug-only runtime requirement and no
  development filesystem assumption (paths are exactly the configured
  `--library`/`--database`/`--static-dir`).
- CI cross-checks the host trio (`ubuntu-latest`, `macos-latest`,
  `windows-latest`) with `cargo build --workspace --all-targets`; the server is
  never a wasm target.
- The artefact is distributed as-is (no packaging step exists in this
  repository; see §9).

## 3. Release-build verification

The full server suite runs green in **release** against the live C oracle:

```sh
cargo build --release -p musicpack-server
MUSICPACK_LEGACY_SERVER=…/musicpack-server \
  cargo test --release -p musicpack-server      # 188 passed / 0 failed
```

That includes the C-vs-Rust differentials for discovery, ingestion, identity,
the database (v10 → v11), the API, media/byte serving, static hosting, jobs,
tokens and track-linked lyrics.

## 4. Configuration

Precedence (reference-compatible): **flags > environment > defaults**.

| Flag | Env | Default | Meaning |
|---|---|---|---|
| `--library DIR` | `MUSICPACK_LIBRARY` | `./library` | package root |
| `--database PATH` | `MUSICPACK_DATABASE` | `./library.db` | SQLite file |
| `--listen IP` | `MUSICPACK_LISTEN` | `127.0.0.1` | bind address (loopback by default) |
| `--port N` | `MUSICPACK_PORT` | `8080` | listen port |
| `--verify` | — | off | full SHA-256 verification during scan |
| `--no-scan` | — | off | serve without a startup scan (requires an existing DB) |
| `--static-dir DIR` | — | disabled | serve a static tree (SPA + COOP/COEP) |
| `--allow-origin URL` | — | none | CORS allowlist (repeatable, ≤ 8) |
| `--secure-cookies` | — | off | always `Secure` on session cookies |
| `--shutdown-file PATH` | `MUSICPACK_SHUTDOWN_FILE` | disabled | graceful-drain trigger (§6) |

Logging ceiling: `MUSICPACK_LOG` (`debug` | `warn` | `error`; anything else =
`info`).

Production recommendations (not defaults): bind behind a reverse proxy that
terminates TLS and preserves `Range`; pass `--secure-cookies`; set
`MUSICPACK_LOG=warn`; point `--library`/`--database` at durable storage; watch
the library directory, not the database, for backups.

## 5. Startup, health, readiness

Startup order: open DB → migrate (forward-only, fail-closed) → startup scan
(unless `--no-scan`) → bind → serve. A failed startup scan is logged and the
server serves the previous library state.

Readiness probe:

```sh
curl -fsS http://127.0.0.1:8080/api/v1/health
# {"status":"ok","version":"0.1.0","apiVersion":1,"schemaVersion":11}
```

`/api/v1/health` is public (no token) and is suitable for supervisor and
load-balancer probes. There is no separate `/ready` endpoint; health answering
means the database is open and the process is serving.

## 6. Graceful shutdown

The server is std-only and signal-free by design (ADR 0009): `SIGTERM`/`SIGINT`
terminate the process directly, which is safe because every ingest/verify write
is its own short atomic WAL transaction (a killed pass leaves a committed
prefix; the next scan completes the sweep).

For an explicit drain, configure the **shutdown file** (`--shutdown-file` /
`MUSICPACK_SHUTDOWN_FILE`). When the file appears:

1. the accept loop stops accepting new connections;
2. in-flight requests are allowed to finish (bounded by a 30 s drain window);
3. the background scan/verify stops at the next package boundary (a cancelled
   scan deliberately skips the unavailable sweep so no package is orphaned);
4. the process exits with status 0 once the drain completes.

Example supervisor hook:

```sh
# ExecStop=/usr/bin/touch /run/musicpack/stop
musicpack-server serve --library /srv/music --database /var/lib/musicpack/library.db \
  --shutdown-file /run/musicpack/stop --secure-cookies
```

The trigger file is removed once observed, so a restart is not immediately
stopped. Without the flag the process lifecycle is exactly as before.

## 7. SV7

**Explicitly unsupported / rejected** (`docs/adr/0014` §5). The Rust decoder is
SV8-only. An SV7 `.mpc` (`MP+` magic) still resolves as a servable object, but
the probe claims no stream facts and ingest falls back to the extension codec
`musepack` with zeroed `streamVersion`/`sampleRate`/`channels`. This diverges
from the C server (its vendored libmpcdec reports `musepack-sv7` + facts); the
divergence is accepted because SV7 is retired product-wide, no SV7 fixture
exists, and the Rust playback engine cannot decode SV7 either. Pinned by
`probe::tests::sv7_streams_are_rejected_by_the_probe`.

## 8. Database compatibility

- Schema v11 = the C's migrations 1–10 (byte-identical SQL) plus the Rust-defined
  additive v11 (`assets.track_id`/`lang` + index, R3.3 lyrics).
- A C-created v10 database opens and migrates without conversion; a v11
  database still opens in the C server (its loop no-ops on the higher version).
- `schema_version > 11` is refused (`DatabaseTooNew`) and never modified;
  migrations are forward-only and idempotent; WAL, foreign keys and the busy
  timeout are configured at open.
- Pinned by `tests/db_compat.rs` (7) + `tests/lyrics_server.rs` (13) against the
  committed C-created fixture, with the live C server.
- Backups: the library directory is the source of truth; only `tokens`/`sessions`
  rows are not rebuildable by a scan.

## 9. Deployment and rollback

**No deployment mechanism exists in this repository** — no Dockerfile/compose,
systemd/launchd unit, container image or release/distribution workflow.
Deployment is external/manual. The repository-level procedure is:

```sh
# build
cargo build --release -p musicpack-server
# configure + verify (a scan is not servable until verified)
musicpack-server scan   --library /srv/music --database /var/lib/musicpack/library.db
musicpack-server verify --library /srv/music --database /var/lib/musicpack/library.db
musicpack-server token create --name web --database /var/lib/musicpack/library.db
# start
musicpack-server serve --library /srv/music --database /var/lib/musicpack/library.db \
  --secure-cookies --shutdown-file /run/musicpack/stop
# health
curl -fsS http://127.0.0.1:8080/api/v1/health
```

Rollback: stop the Rust server and start the C server binary on the *same*
database file — the two implementations are bidirectionally database-compatible
until the C server is formally retired. The C server is kept for exactly this
window (see `docs/r4-completion.md` for the inventory classification).

Executing a real production deployment requires infrastructure and credentials
that do not exist in this repository; see §11.

## 10. Observability and performance

Logging stays `musicpack-server[level]: …` on stderr; request logging is
deliberately absent; tokens/session secrets are never logged. Startup, config,
scan/ingest/conflict/verification milestones, job start/failure/cancel and
shutdown milestones are logged.

Performance sanity (release binary, one fixture package, 4 tracks;
`bash tools/server_bench.sh`):

| Operation | Time |
|---|---|
| scan + verify (1 package) | 18 ms |
| startup → health (`--no-scan`) | 33 ms |
| `GET /api/v1/health` | 14.5 ms\* |
| `GET /api/v1/albums?limit=50` (auth) | 17.5 ms\* |
| `GET /api/v1/tracks/1` (auth) | 17.2 ms\* |
| asset range 0–65535 | 17.1 ms\* |
| 8 concurrent range reads (64 KiB each) | 23 ms total |
| graceful shutdown drain | 74 ms |

\* Includes `curl` process startup (~14 ms floor on this host), so the server
component is small. No material regression was observed and no optimisation
was attempted.

## 11. Production cutover decision

```text
release artifact verified
deployment procedure documented
external production execution required
```

Repository-level production readiness is complete. No production deployment was
performed (no infrastructure/credentials are available in this environment),
and none is claimed.
