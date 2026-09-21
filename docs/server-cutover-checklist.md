# MusicPack server cutover checklist

*Gate for R2 (web/author migration). The Rust server becomes the
production backend only after every item here is checked. Companion to
`docs/server-migration.md` (stages 1–6) and ADR 0007/0009.*

> **Status: CLOSED (R4.5).** Every item below is verified; the closure
> evidence, the release-build differential, the graceful-shutdown
> mechanism and the SV7 decision are recorded in
> `docs/adr/0014-server-production-cutover.md`,
> `docs/server-production.md` and `docs/r4-completion.md`. Deployment
> itself remains external (no mechanism exists in the repository).

---

## 1. Build

- [ ] **Command**: `cargo build --release -p musicpack-server --bin musicpack-server`
      (or `cargo build -p musicpack-server` for a debug build).
- [ ] **Artifacts**: one self-contained binary, `target/release/musicpack-server`.
- [ ] **Runtime dependencies**: none beyond the platform libc. SQLite is
      compiled in (`rusqlite`/`bundled` — the workspace's only C
      dependency, confined to this crate per D-S2). No dynamic library,
      interpreter, or data files needed at runtime.
- [ ] **Cross-compile targets in use**: macOS arm64/x86_64 and Linux
      x86_64/aarch64 all build clean (CI matrix covers the host trio);
      the crate is never a wasm target.
- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets
      --all-features -- -D warnings`, `cargo test --workspace`, wasm
      checks, and `cargo check --manifest-path fuzz/Cargo.toml` are green
      at the pinned commit (canonical gates in root `AGENTS.md`).

## 2. Configuration

- [ ] **Precedence** (final, reference-compatible): flags > environment >
      defaults. Environment: `MUSICPACK_LIBRARY`, `MUSICPACK_DATABASE`,
      `MUSICPACK_LISTEN`, `MUSICPACK_PORT` (empty values ignored).
- [ ] **Safe defaults**: library `./library`, database `./library.db`,
      bind `127.0.0.1:8080` (loopback only — never an accidental
      wildcard), static hosting off (`--static-dir` empty), CORS empty
      (deny-by-default), cookies not forced Secure.
- [ ] **Flags**: `--library --database --listen --port --verify --no-scan
      --static-dir --allow-origin (≤8) --secure-cookies`.
- [ ] **Secrets/tokens**: `musicpack-server token create --name NAME`
      prints the `mpk_…` bearer secret **once**; only its SHA-256 hash is
      stored (in `tokens`, alongside sessions). Rotation = create + swap
      + `token revoke <id>`. There are no other secrets (no TLS keys —
      TLS terminates at the proxy).
- [ ] Deployment requires exactly: the binary, a library directory, a
      database path, one token, and (for web serving) the static tree.

## 3. Data

- [ ] **Initialization**: the database is created and migrated
      automatically on first open (forward-only migrations 1–10 from the
      C reference plus the Rust-defined additive v11 —
      `assets.track_id`/`lang` + `assets_track_idx`, R3.3 lyrics). A
      database with `schema_version > 11` is refused with a clear error
      (`DatabaseTooNew`), never modified.
- [ ] **Adopting an existing library**: run `musicpack-server scan
      --library DIR --database PATH` (or let `serve` do the startup
      scan). A C-created `library.db` opens as-is — bidirectional
      compatibility pinned by `tests/db_compat.rs` against a committed
      C-created fixture (Rust lists/revokes tokens in C databases and
      vice versa).
- [ ] **Disappearance/moves**: vanished directories are marked
      `unavailable` (rows and ids preserved — never deleted); a returning
      directory returns to `valid`; a moved directory (same manifest
      fingerprint elsewhere, old path gone) keeps its row; duplicate
      fingerprints mirror one owner; conflicting duplicates quarantine as
      `conflict` without touching the served graph. All pinned by
      `tests/ingest_oracle.rs` (13 live-C scenarios).
- [ ] **Verification**: `verify` (CLI) or the verify job (HTTP) re-hashes
      every non-quarantined package and persists verdicts per package;
      `checksum-failed` packages stop serving (503) while the library
      state stays rebuildable (the database is a projection of the
      packages, except tokens/sessions).
- [ ] **Backups**: back up the library directory (source of truth) and
      `tokens`/`sessions` rows; everything else is rebuildable by scan.

## 4. Security

- [ ] **Authentication**: single gate before every route except
      `/api/v1/health` and `/api/v1/session` — Bearer token first,
      `musicpack_session` cookie fallback, one 401 envelope. Pinned by
      `tests/api_oracle.rs`.
- [ ] **Sessions**: 30-day sliding, `HttpOnly; SameSite=Strict; Path=/`,
      `Secure` when `--secure-cookies` or `X-Forwarded-Proto: https`.
      Sign-out clears + revokes.
- [ ] **CORS**: deny-by-default; explicit origins only (`--allow-origin`,
      max 8); preflight handling; origin-less OPTIONS rejected. The
      migrated web app is same-origin and needs no entry.
- [ ] **Filesystem containment** (shared `pathsafe` discipline):
      strict relative-path validation, root canonicalization,
      existing-ancestor containment, final-component symlink rejection,
      regular-file checks; media objects additionally require
      link-count 1 and (for inline images) magic-byte safety. Static
      hosting serves only under `--static-dir` (traversal shapes 404 —
      pinned by `tests/static_oracle.rs`); physical paths never appear
      in any response.
- [ ] **Byte-serving hardening**: `nosniff` + sandbox CSP on every byte
      response, `attachment` unless inline-allowed and magic-safe,
      strong sha256 ETags, sizes from `fstat` only.
- [ ] **Transport limits**: 32 KiB head, 4 KiB body, 2 KiB path, no
      chunked requests; violations close the connection without a
      response.
- [ ] **Proxy/TLS assumptions**: run behind a reverse proxy that
      terminates TLS, preserves `Range` headers (byte-exact 206 is the
      client contract), and forwards `X-Forwarded-Proto: https` for
      Secure cookies. COOP/COEP on static responses are set by the
      server itself (SharedArrayBuffer requirement).

## 5. Operations

- [ ] **Startup order**: open DB → migrate → startup scan (unless
      `--no-scan`, which requires an existing database) → bind → serve.
      A failed startup scan logs and serves the previous library state
      (never aborts serving).
- [ ] **Health**: `GET /api/v1/health` (public) returns status/version/
      apiVersion/schemaVersion — suitable for supervisor and
      load-checker probes.
- [ ] **Logging**: `musicpack-server[level]: …` on stderr; ceiling from
      `MUSICPACK_LOG`. Startup, job start/failure, and scan anomalies
      are logged; request logging is deliberately absent.
- [x] **Shutdown**: `SIGTERM`/`SIGINT` terminate the process directly
      (**documented divergence**: the C waits for a running job —
      ADR 0009). Safe by design: every ingest/verify write is its own
      short atomic WAL transaction, so an interrupted job leaves
      committed-prefix state and the next scan completes the sweep.
      **R4.5 adds an opt-in graceful drain** (`--shutdown-file` /
      `MUSICPACK_SHUTDOWN_FILE`): the accept loop stops accepting, waits
      (≤30 s) for in-flight requests, the background job stops at a package
      boundary (a cancelled scan skips the unavailable sweep), and the
      process exits 0. See `docs/server-production.md` §6. Run under a
      supervisor with automatic restart.
- [ ] **Jobs**: `POST /api/v1/library/scan|verify` (single slot; 409
      `scan_already_running` when busy), live progress on
      `GET /api/v1/library/status`. Startup scan is synchronous;
      rescan/verify through the job endpoints.
- [ ] **Adoption visibility** (reference behaviour, verified against the
      C): a *lightweight* scan leaves `verify_status = 'unverified'`,
      and the visibility gate requires `verify_status IN
      ('valid','warning')` — so newly adopted content stays invisible
      until a verifying pass runs. Deployment procedure: after the first
      `scan`, run `verify` once (CLI or job); the smoke in §8 covers
      this. Both implementations behave identically.
- [ ] **Failure modes**: bind conflict → exit 1 with message; missing
      `--no-scan` database → exit 1 with guidance; corrupt/missing
      packages → status rows (`warning`/`unavailable`/`checksum-failed`),
      API 503s on unservable objects, never a crash; disk-full/DB-busy →
      reference busy-retry budgets, then a logged failure.

- [x] **SV7**: explicitly unsupported/rejected (R4.5, ADR 0014 §5). The
      Rust decoder is SV8-only; an SV7 `.mpc` resolves and is byte-servable
      but the probe claims no facts, so ingest falls back to the extension
      codec `musepack` (zeroed `streamVersion`/`sampleRate`/`channels`). The
      C server (vendored libmpcdec) would report `musepack-sv7` + facts — an
      accepted, documented divergence. Pinned by
      `probe::tests::sv7_streams_are_rejected_by_the_probe`. No SV7 fixture
      exists and no Rust playback path can decode SV7.

## 6. Compatibility

- [ ] **Package compatibility**: `.mpack` v1 manifests (strict parse,
      canonical serialization), MPAK v1 container, waveform envelope v1,
      identity keys — all permanent contracts (ADR 0007).
- [ ] **API compatibility**: HTTP API v1 exactly as the legacy spec +
      errata (`docs/api-spec-errata.md` — waveform Range **is**
      supported). Fields are added, never removed, within v1.
- [ ] **Database compatibility**: migrations 1–10 byte-identical to the
      C; v11 is Rust-defined and additive (the C opens a v11 database
      no-op — its loop falls through on the higher version, proven by
      `tests/db_compat.rs` + `tests/lyrics_server.rs` when
      `MUSICPACK_LEGACY_SERVER` is set). Rust-created v10-era databases
      are still usable by the C server and vice versa for the C-visible
      schema (the C neither reads nor writes the v11 columns).
- [ ] **Oracle coverage at cutover** (with `MUSICPACK_LEGACY_SERVER`
      set, all green): `api_oracle` 10 · `media_oracle` 14 ·
      `ingest_oracle` 13 · `static_oracle` 5 · `jobs_oracle` 3 ·
      `db_compat` 7 · `identity` 7 · `discovery` 12 · `cli_token` 6 ·
      plus 90 in-crate unit tests.
- [ ] **Known, documented divergences** (accepted, none API-observable):
      `Connection: close` (no keep-alive — revisit trigger in ADR 0001);
      no in-process signal/job-wait handling (ADR 0009); the C's
      `mime[48]` Content-Type truncation is *reproduced*, not diverged.

## 7. Web handoff — what the migrated Web Player/Author may rely on

- **Session bootstrap**: `POST /api/v1/session` with a bearer token →
  HttpOnly session cookie; `GET`/`DELETE /api/v1/session` for probe and
  sign-out. 401 envelope semantics unchanged.
- **Browse endpoints** (unchanged shapes, pagination clamps, search
  `q` with LIKE-escaping, sort keys): `GET /api/v1/albums[/{id}]`,
  `/api/v1/releases/{id}`, `/api/v1/tracks/{id}` (with the wire `context`
  quirk the client already normalizes), `/api/v1/artists[/{id}]`,
  `/api/v1/library/status`.
- **Byte serving**: `GET|HEAD …/audio|waveform`,
  `/api/v1/tracks/{id}/representations/{rid}/audio`, `/api/v1/assets/{id}`
  — single `bytes=` ranges with strong sha256 ETags; the web block
  reader's 64 KiB contract (206-required, exact Content-Range) is
  replicated byte-for-byte by `tests/media_oracle.rs`.
- **Static hosting + isolation**: SPA fallback for extension-less GETs,
  COOP/COEP on static responses (the SharedArrayBuffer reader works
  without proxy header surgery), correct MIMEs from the shared table,
  `no-cache` on the shell.
- **Library maintenance from the UI/admin**: start scan/verify via the
  job endpoints; poll `library/status` (the `scan_already_running` code
  the client already knows is now reachable in reality).
- **No server-side changes needed by the Author**: the Author wraps
  CLI-shaped functionality (`inspect/validate/pack`), which lives in the
  `musicpack` CLI crate, not the server.

## 8. Sign-off

- [x] All gates green at the cutover commit (R4.5 gate run; exact counts in
      `docs/r4-completion.md` §5).
- [x] Differential suite executed against the built legacy binary
      (`MUSICPACK_LEGACY_SERVER=… cargo test --release -p musicpack-server`)
      — **188 passed / 0 failed on the release build**, every oracle file green.
- [x] A manual smoke on the deployment host is documented in
      `docs/server-production.md` §9 (scan → verify → token → serve → health →
      browse → play → offline download → `--static-dir`). It is the operator's
      step; this repository executes the equivalent verticals in CI/e2e.
- [x] Rollback plan: the C server binary is kept and can serve the same
      database file (bidirectional DB compat) until the legacy server is
      formally retired (stage 9; disposition recorded in
      `docs/r4-completion.md` §2).
