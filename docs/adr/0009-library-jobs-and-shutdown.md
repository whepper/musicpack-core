# ADR 0009: Library job model and shutdown semantics

- Status: Accepted (2026-09-20, implemented as server stage 6)

## Context

The legacy C server runs at most one library maintenance job (scan or
verify) at a time on a background thread with its **own SQLite connection**
(`jobs.c`), so the serving connection never blocks on filesystem work and
readers see the last committed state. The HTTP surface is three endpoints:
`POST /api/v1/library/scan|verify` (202 + fresh snapshot, 409
`scan_already_running` when busy) and `GET /api/v1/library/status` (live
counters). The C also installs signal handlers and waits for a running job
before exit (`mp_jobs_wait`).

## Decision

1. **Port the reference job model verbatim — no generalized framework.**
   One slot, two kinds, a mutex-protected snapshot, per-package progress
   callbacks, per-kind counter resets. No job ids, queues, cancellation,
   retries-beyond-the-reference, async runtime, or worker abstraction: no
   MusicPack requirement exists for them (a single-admin self-hosted
   server starts one maintenance pass at a time).
2. **The job worker opens its own `SqliteStore` connection** (exactly like
   the C). The serving connection is never held across job work; WAL plus
   the reference's busy-retry budgets (100 × 50 ms for verify verdict
   writes, ~2 s for session writes) handle contention.
3. **`verify_library` persists per-package verdicts as short atomic
   updates** (`warning/unverified` on open failure, `checksum-failed`
   twice on integrity errors, `warning` twice on warnings, else
   `valid/valid`), candidates collected before any write, `conflict` rows
   never touched — a crash mid-verify leaves the rest at their previous
   state and a re-run picks them up.
4. **No in-process signal handling.** The C's graceful job-wait is
   superseded (this is a documented divergence, not an oracle-compat
   break — signal handling is unobservable through the API): std-only
   means no `libc`/signal dependency, and termination during a job is
   safe because every write is its own short atomic WAL transaction.
   Operational guidance: run under a supervisor (launchd/systemd); after
   an interrupted job, the next scan completes the sweep.
5. **Docker/compose and deployment docs are deferred** to the cutover
   step (`docs/server-cutover-checklist.md` gates R2) — the server
   remains deployable as a simple process; containerization is not a
   server-code concern.

## Consequences

- The API contract is identical to the C (pinned by `tests/jobs_oracle.rs`
  with live differential comparison, including row-for-row verify-verdict
  equality in the database).
- Adding a third job kind or a queue later means replacing this model,
  not extending it — acceptable; the model is ~200 lines.
- An interrupted job is indistinguishable from a crash mid-scan to the
  data model, which the schema already tolerates by design.
