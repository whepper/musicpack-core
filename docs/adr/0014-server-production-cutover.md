# ADR 0014: Server production cutover closure

- Status: Accepted (2026-09-21, R4.5)

## Context

The Rust server (`crates/musicpack-server`) has been behaviour-compatible with
the legacy C server since stage 6 (API/media/ingestion/jobs oracles green), but
the cutover was never closed: no release-build proof, no deployment/rollback
documentation, a deliberately deferred graceful-shutdown decision (ADR 0009),
and an open SV7 probe question (`docs/r3.7-r4-readiness.md` O-S3). R4.5 closes
those without changing the server's contracts.

## Decision

1. **The Rust server is the production runtime.** Its dependency graph is
   `musicpack-server → musicpack-core → {SQLite (bundled via rusqlite),
   filesystem}`. The release binary links only the platform libc (no
   libmusicpack, `mpcenc`, `musicpack-sonic`, FFmpeg, interpreter or shell).
   The C server is retained **only** as a differential/behavioural oracle.
2. **Release build is the artefact of record.**
   `cargo build --release -p musicpack-server` produces one self-contained
   binary (`target/release/musicpack-server`); the full server suite runs green
   in release *with the live C oracle* (see `docs/server-production.md` §3).
3. **Graceful shutdown: an in-process drain plus an opt-in trigger.** A
   std-only `Shutdown` token (no signal FFI, no async runtime, `forbid(unsafe)`
   intact) lets the accept loop stop accepting and wait for in-flight
   connections, and lets the background job stop at a package boundary. The
   binary arms it with `--shutdown-file PATH` / `MUSICPACK_SHUTDOWN_FILE`, which
   a supervisor's `ExecStop`/pre-stop hook can create. **Signal handling is
   still out of scope** (ADR 0009): `SIGTERM`/`SIGINT` terminate directly,
   which the atomic-WAL data model tolerates. R4.5 therefore *supersedes ADR
   0009 decision 4 only to the extent that a drain is now possible when the
   trigger is configured*; the default lifecycle is unchanged.
4. **A cancelled maintenance pass is safe by construction.** A shutdown-cancelled
   scan stops before the unavailable sweep (a sweep would orphan
   not-yet-processed packages), verify stops after the last committed verdict,
   and the database keeps the committed prefix. Cancellation is not reported as
   a job failure.
5. **SV7 is explicitly unsupported / rejected** (not deferred). The Rust
   decoder is SV8-only; an SV7 stream (`MP+`, version nibble 7) resolves as a
   servable object but yields no stream facts, so ingest falls back to the
   extension codec `musepack` (zeroed `streamVersion`/`sampleRate`/`channels`).
   This is a documented, accepted divergence from the C server, whose vendored
   libmpcdec reports `musepack-sv7` + facts; no SV7 fixture exists and neither
   runtime can *play* SV7 through the Rust engine, so reporting the codec would
   advertise an unplayable format. Implementing an SV7 probe would be new
   codec work and is out of scope.
6. **Configuration gains one additive, default-off ops setting.** No C-visible
   default or precedence changes: flags > environment > defaults, and the new
   `--shutdown-file` / `MUSICPACK_SHUTDOWN_FILE` is empty (disabled) by
   default.
7. **Deployment stays external.** The repository contains no container,
   service unit or release/distribution mechanism, and none is invented here.
   R4.5 delivers the release artifact, the documented configuration/startup/
   health/rollback procedure, and the evidence needed to run it; executing a
   production deployment requires infrastructure and credentials that do not
   exist in this repository.

## Consequences

- The production server is Rust-authoritative end to end; the C server survives
  as an oracle and a rollback option for the same database file (bidirectional
  DB compatibility is unchanged).
- Operators who want a graceful drain configure a shutdown file; otherwise the
  process semantics are exactly as documented in ADR 0009.
- The SV7 limitation is explicit and tested, not silent (see
  `docs/server-production.md` §7).
- Nothing in the server's API, schema, HTTP transport or ingestion semantics
  changed; the oracle suites (including the live C differential) stay green.
