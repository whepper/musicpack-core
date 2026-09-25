# Server migration — architecture investigation (phase 14)

Investigation-only output: how the legacy C `musicpack-server`
(`../musicpack/server`, read-only reference) maps onto the Rust workspace.
No server code exists yet; this document is the decision record the
implementation phases will follow. Behavioural authority for the server is
the legacy implementation plus `specs/musicpack-api-v1.md` — the same
"reference is the oracle" rule used for every earlier phase.

## Findings that shape the boundary

1. **The legacy server is a thin orchestrator over `libmusicpack`.** It never
   parses manifests: discovery, parse/validate, path containment and full
   verification are library calls. Everything the server adds is *collector*
   semantics: a three-level identity model (package fingerprint /
   `group_key` / `release_key`), an ingestion state machine, a rebuildable
   SQLite index, an HTTP API, and auth. None of that belongs in
   `musicpack-core` (package domain); it belongs in a server crate.
2. **The library on disk is `.mpack` directories and `.mpak` containers.**
   Both are first-class library sources: a `.mpack` directory bundle is read
   through `DirectoryBackend`, a `.mpak` single-file container through
   `MpakBackend`, and both project into the same SQLite index with the same
   identity, ownership and content-sync rules. This is a deliberate **superset**
   of the C reference, which only ever walked `.mpack` directories; the
   differential tests pin the difference. Container *acquisition and transport*
   is a separate concern: the engine reads container members over a range
   source (`docs/mpak-source.md`, Stage 1), and the server serves an indexed
   member as a byte range inside its container, so the client never sees a
   container offset or a container size.
3. **The server never transcodes and never decodes.** Audio delivery is byte
   serving of the original stored members (RFC 7233 single range, strong
   sha256 ETag, `If-Range`, 304/416). Decoding happens client-side
   (WASM/native). The only decoder interaction is a scan-time *probe*
   (codec string, stream version, rate, channels) — server-side PCM is out
   of scope by architecture, so the Rust audio foundation is used for
   probing/metadata only, not streaming.
4. **`library.db` is a rebuildable projection** of the library — except
   `tokens`/`sessions`, which live only in the DB. Public ids are SQLite
   rowids kept stable across rescans by in-place upserts; `uid` columns are
   internal insurance, never serialized.
5. **Client compatibility is the binding constraint.** The web client keys
   offline catalogs and persisted queue snapshots on numeric track ids,
   hard-errors on `200`-for-Range, requires the flattened track+`context`
   JSON quirk, cookie session exchange, COOP/COEP static hosting and SPA
   fallback. The API contract must be reproduced exactly; the client does
   not change.

## Destination map (summary)

| Legacy responsibility | Rust destination | Reuse |
|---|---|---|
| Manifest parse/validate, canonical serialization, SHA-256, path containment, package verify, waveform payload checks | `musicpack-core` | ✅ existing (`format::*`, `storage::DirectoryBackend`, `validation::verify`) |
| Codec/MIME probe metadata | server crate over `musicpack-core::audio` | mostly (SV7 header parse gap, see O-S3) |
| Identity (fingerprint / group_key / release_key), scanner state machine, ownership/conflict quarantine, collector model, id-stability matching | `crates/musicpack-server` (new) | no — port with differential oracles |
| SQLite schema/migrations, store, tokens/sessions | server crate, schema kept byte-compatible | no (driver decision O-S1) |
| HTTP/1.1 + Range + ETag serving, static+SPA+COOP/COEP, CORS, cookie auth | server crate | no — hand-rolled minimal HTTP/1.1 (O-S2) |
| Config precedence, logging, job slot, graceful shutdown | server crate | trivial ports |
| Player/engine/audio decode | unchanged (client-side) | — |

## Decisions recorded

- **D-S1 — crate boundary.** `crates/musicpack-server/` inside this
  workspace (single repo, shared core via path dependency). No separate
  repository: no independent release cadence, licensing or ownership
  boundary exists that would justify one. The crate is native-only (never a
  wasm target), `#![forbid(unsafe_code)]`, BSD-3-Clause, and must not
  become a dependency of any wasm-targeted crate.
- **D-S2 — database.** Compatibility-first: the C schema is retained
  verbatim — same 10 migrations, same `schema_version` mechanics, same DDL
  — so an existing `library.db` opens under either implementation with zero
  conversion. Future changes are additive migrations appended as v11+.
  Rowid/id semantics, `COLLATE NOCASE`, pragma set (`WAL`,
  `foreign_keys=ON`, `synchronous=NORMAL`) and the datetime-string
  conventions are part of the compatibility surface.
- **D-S3 — API.** `existing Web client → existing API contract → new Rust
  server`. Every `/api/v1` endpoint is reproducible from the core plus
  collector logic; no endpoint requires client changes. Exact JSON shapes
  (including the track+`context` flattening), deterministic orderings,
  pagination clamps (limit ≤ 200), the `{error:{code,message}}` envelope,
  the VISIBLE servability gate and strict Range/ETag discipline are the
  compatibility contract.
- **D-S4 — security.** No weakening: opaque tokens/sessions (sha256-only
  storage, session inherits token revocation/expiry, sliding 30-day
  cookie), deny-by-default CORS, containment-checked + `O_NOFOLLOW` +
  regular-file + `nlink==1` opens for every served byte, the durable
  `conflict` quarantine, fail-closed unverified ingestion and the sandbox
  security headers are all reproduced. Core already provides path/containment
  and verify semantics; transport, auth and quarantine remain server-side.
- **D-S5 — layering inside the server.** `file/package access →
  representation selection → HTTP byte delivery` and `decoder → PCM` stay
  separate, exactly as the legacy architecture demonstrates. The server has
  no PCM path.

## Staged plan (each stage leaves the workspace green)

**Stage 1 is implemented** (`crates/musicpack-server`: CLI/config/logging
skeleton, byte-compatible SQLite layer for migrations 1–10 with the
verbatim C SQL, `Store` trait, functional `token create|list|revoke`;
`scan`/`verify`/`serve` report an explicit deferred result). Compatibility
is proven structurally (`sqlite_master`, columns, indexes, version 10
against a committed C-created fixture) and live in both directions (the
built legacy binary lists/revokes tokens in a Rust-created database and
vice versa; CLI output is byte-identical). O-S1 is settled as recorded
(D-S2); O-S2–O-S4 remain open for their stages.

**Stage 2 is implemented** (`identity` + `discover`, no database writes):

- Collector identity as pure functions over the core `Manifest` model:
  `package_fingerprint` (SHA-256 of the core canonical serialization —
  the C `musicpack_manifest_write` path, *without* unknown-field
  preservation), `group_key` / `release_key` (the exact TLV algorithm:
  1-byte tag + BE32 length + raw bytes, absent skipped, `Some("")`
  emitted zero-length, artists stably sorted by (name, role), `mb:<uuid>`
  anchors with the C's `isxdigit` shape rule). Proven by 20 golden vectors
  generated from the built legacy binary (18 synthetic edge cases +
  2 real-world manifests; `tests/data/identity/`), replayed by
  `tests/identity.rs`, plus a `server_identity` fuzz target (determinism +
  key-shape discipline).
- Bounded discovery walk mirroring the C budgets (depth 64, 100k objects,
  10k packages, 64 MiB path bytes) and safety rules (symlinks never
  followed, `.mpack`-suffixed directories only, unreadable/overlong
  entries fail the whole discovery, candidates sorted by path for
  determinism). Invalid packages are reported with reasons, never dropped.
  SV7 needs no handling here (no decoding in discovery); the O-S3 probe
  gap is a stage-3 concern.

**Stage 3 is implemented** (`ingest`, `probe`, collector `Store` ops;
`scan`/`verify` CLI wired to the real scan):

- The full ingestion state machine over `PackageCandidate`s: per-package
  transactions, invalid/unchanged fast paths, move detection (conflict
  rows stay quarantined), ownership arbitration (first-discovered wins;
  mirrors attach, conflicts quarantine without touching the owner),
  group/release upserts with metadata-refresh-on-takeover, in-place
  content sync (media by disc number, tracks by position-then-content,
  assets/variants by natural key — stable numeric ids), `last_scan`
  sweep, and the VISIBLE-unrelated status/verify_status model verbatim.
- Codec probing without decoding for serving: SV8 stream facts through
  the existing Rust decoder, FLAC through a byte-exact 42-byte STREAMINFO
  parse, extension fallback otherwise (SV7 takes the failed-probe path —
  O-S3 remains open but is not triggered by any fixture).
- Parity is proven by `tests/ingest_oracle.rs`: 13 scenarios (rich ingest,
  rescan stability, retitle/audio/track mutations, removal/reappearance,
  invalid lifecycle, mirrors + owner transfer, sequential conflicts,
  sticky warnings, moves, verify mode, MB grouping) run round-by-round
  against the built legacy binary, comparing scan counters and
  table-by-table normalized databases (timestamps/uids/scan-tokens
  shape-asserted, row ids remapped through natural keys).
- Deliberate, documented comparison boundaries: `owner_package_id` is
  shape-checked (first-discovered-wins is order-dependent and discovery
  order differs: C readdir vs Rust sorted — the rule itself is asserted
  per side); the harness waits for epoch-second rollover between C scans
  (separate CLI processes restart the scan-token counter, so same-second
  scans mint identical tokens and the C sweep no-ops — a genuine upstream
  quirk, reproduced faithfully, not a deviation).

**Stage 4 is implemented** (`http`, read-only JSON API; `serve` CLI wired
to the real server):

- Hand-rolled std-only blocking HTTP/1.1 (O-S2 settled as recommended):
  thread-per-connection, one shared store connection behind a lock (like
  the C serving process), always `Connection: close`, explicit
  `Content-Length`, IMF-fixdate `Date`. No async runtime, no framework,
  no new dependencies. Short-lived, non-overlapping mutex guards
  throughout the routes (std mutexes are not reentrant — nesting would
  wedge the connection thread; this was caught by the oracle hanging on
  album detail and fixed by scoping).
- Contract ports: health, session lifecycle (bearer→cookie exchange,
  sliding 30-day renewal, revocation inheritance, `Secure` via
  `--secure-cookies`/`X-Forwarded-Proto`), artists/albums/releases/tracks
  with byte-exact envelopes (insertion-ordered JSON builder with the C
  escaping rules, `%lld` ints, a from-scratch `%.10g` port pinned against
  CPython vectors), pagination clamps, escaped-LIKE `?q=`, `sort=recent`,
  verbatim SQL `ORDER BY`s, the flattened track+`context` quirk,
  detail-layout waveform sha vs track-layout omission, and the exact error
  envelope/codes/messages. CORS deny-by-default with the C gate order
  (present-disallowed origin → `origin not allowed` before method checks;
  the `preflight origin not allowed` message fires only for origin-less
  OPTIONS). MHD transport specifics intentionally not reproduced: duplicate
  `Host` headers (MHD's own HTML 400), keep-alive negotiation.
- Parity is proven by `tests/api_oracle.rs`: 10 tests comparing status,
  headers (minus `date`/`content-length`/`connection` transport
  artifacts) and bodies against the live C server on the same database
  file — timestamps/uids/session-ids redacted by shape, row ids identical
  (shared database). Genuine transport finding, not a contract deviation:
  the legacy MHD stack intermittently stalls POST-with-body requests from
  socket clients (reads every byte, never dispatches; GETs unaffected),
  so session POSTs compare via curl on both sides (with raw fallback).
  Out-of-scope routes (byte serving, `POST /library/scan|verify`) answer
  explicit 404s, pinned by tests.
- `GET /api/v1/library/status` serves the idle zero snapshot (no job
  system yet — stage 8).

**Stage 5 is implemented** (secure byte/media serving; `serve` delivers
the full read contract):

- Architecture (no new crates, no new dependencies, no framework): the
  stage-4 transport is **retained** — blocking HTTP/1.1, std-only,
  thread-per-connection, `Connection: close` — because media serving
  exposed no genuine limitation: sending is `sendfile`-style streaming
  (seek once, pump bounded 64 KiB chunks), the store lock is held only
  for metadata resolution and released before any filesystem I/O, and
  the 8-worker mixed media+JSON concurrency test shows no wedging or
  cross-talk. HTTP stays an adapter: `store` resolves a `MediaRef`,
  `media::open` validates it into a `MediaResource` (identity, type,
  length, hash, file/range access, availability), and `http::serve`
  evaluates the HTTP decision tree (Range, If-Range, If-None-Match,
  200/206/304/416 + headers) at the boundary. `musicpack-core` is
  untouched by server concerns.
- Endpoints: `GET|HEAD /api/v1/tracks/{id}/audio|waveform`,
  `/api/v1/tracks/{id}/representations/{rid}/audio`,
  `/api/v1/assets/{id}` — all behind the stage-4 auth/CORS/error
  behavior, no second auth system. Dispatch matches the C order
  (shape-first, then strict id parsing).
- Byte contract, all ported from `api.c`/`range.c` and proven against
  the live C server: single `bytes=` ranges only (multi-range,
  `bytes=-0`, reversed, overflow, trailing bytes → 416 with
  `Content-Range: bytes */N`); suffix clamping; `If-None-Match` exact
  match wins over Range with minimal 304 headers; `If-Range` mismatch
  serves full 200; strong `"sha256"` ETags from the stored content hash
  (never re-hashed); sizes always from `fstat`, never the DB; full
  `Content-Type` verbatim including the C's 47-char `mime[48]`
  truncation of the waveform type (reproduced as `c_mime_truncate` —
  accidental upstream field width, zero client effect since no consumer
  reads response MIMEs, but byte-identical); safe
  `attachment; filename="…"` disposition unless inline-allowed *and*
  magic-safe (raster magic checked, audio exempt); always `nosniff` +
  sandbox CSP; `Accept-Ranges: bytes`; `Cache-Control: private,
  max-age=0, must-revalidate`; default-representation track semantics;
  extras/analysis kinds never servable; stale/missing/symlink/
  non-regular/multi-link files → 503 without path leaks; unknown ids →
  404 with the C messages; HEAD shares resolution and emits GET headers
  with no body.
- Filesystem security reuses the collector guarantees: canonical
  manifest-path re-validation of the DB relative path, package-root
  canonicalization with existing-ancestor containment, final-component
  symlink rejection + regular-file + link-count checks, physical paths
  never leaving `media.rs`. Traversal shapes (`../`, encoded
  dot-segments, bad-id/bad-shape) 404 exactly like the C
  (shape-first dispatch); oversized heads/bodies still close the
  connection with no response.
- Parity is proven by `tests/media_oracle.rs`: 14 tests comparing
  status, byte-contract headers and full body bytes against the live C
  server on copied databases (same library dir) — full/head/ranges
  (first-64K/middle/EOF/suffix/one-byte/clamped)/416-matrix/
  conditionals/missing/unavailable, representations, waveforms,
  assets (inline-vs-attachment incl. mislabeled JPEG, SVG, spaced
  names), stale-record 503s, filesystem edges, web block-reader
  reassembly, and 8-thread concurrent media+API load. The Web client's
  `networker.js` 64 KiB block validation is replicated byte-for-byte
  (206-required, Content-Range parse, start==base, declared==body) and
  reassembled blocks equal the file; no browser E2E was available in
  this environment, compensated by the oracle + contract test.

**Stage 6 is implemented** (static hosting + library jobs; `serve` covers
the full legacy contract):

- **Static hosting + SPA fallback** (the C `http.c` `serve_static`, gated
  by `--static-dir`): runs entirely before the API's CORS/method gates;
  `/api/` is reserved (the exact 5-byte prefix check — `/api` alone is
  *not* reserved); files resolve through the Stage-5 containment
  discipline (`pathsafe`: canonical path rules, existing-ancestor
  containment, final-component NOFOLLOW + regular file — hard links
  allowed, like the C static branch); 200s carry the shared MIME table,
  `Cache-Control: no-cache` and the cross-origin isolation pair
  (`COOP: same-origin`, `COEP: require-corp`) the SharedArrayBuffer range
  reader requires; SPA fallback serves `index.html` only for GET of an
  unknown *extension-less* path (HEAD and dotted asset paths 404); the
  static 404 is the JSON envelope with `Content-Type` only (no
  `no-store`, no isolation headers); traversal shapes 404 exactly like
  the C (every decoded `..` shape contains a dot, so it misses the
  fallback). No web framework, no new dependency; differential coverage
  in `tests/static_oracle.rs` (files/MIMEs/headers/bytes, SPA and HEAD
  semantics, traversal matrix, symlink rejection, `/api/` reservation —
  all against the live C server).
- **Library jobs** (the C `jobs.c` ported verbatim — no generalized
  framework, per ADR 0009): a single job slot running scan or verify on
  **its own SQLite connection** (the serving connection never blocks on
  filesystem work; WAL + the C's busy-retry budgets). `POST
  /api/v1/library/scan|verify` start a job (202 with the fresh status
  snapshot; 409 `scan_already_running` when busy) and `GET
  /api/v1/library/status` renders the live snapshot — both blocks always
  present, `running` as 1/0, shared timestamps. `ingest::scan` gained the
  reference's per-package progress callback (fired per candidate and once
  after the sweep, exactly like `mp_scan_library`), and
  `ingest::verify_library` is the `mp_verify_library` port: candidates
  collected before any write, per-package verdicts (`warning/unverified`
  on open failure, `checksum-failed` twice on integrity errors,
  `warning` twice on warnings) written as short atomic updates with the
  100 × 50 ms busy retry, never touching `conflict` rows. Differential
  coverage in `tests/jobs_oracle.rs`: scan/verify lifecycles with
  counter-parity against the live C server and **row-for-row database
  verdict equality** after a verify job.
- Deliberate divergence (documented, not code): the C installs signal
  handlers and waits for a running job before exit (`mp_jobs_wait`);
  this server has no signal handling (std-only, no new dependency) —
  process termination ends a running job directly, which is safe because
  every ingest/verify write is its own short atomic WAL transaction (a
  killed job leaves committed-prefix state; the next scan completes the
  sweep). Recorded in the cutover checklist.

1. **Skeleton + DB layer**: crate, CLI (`scan|verify|serve|token`),
   config precedence, logging; embedded migrations 1–10; open a C-created
   `library.db`; `token` subcommands working.
2. **Identity + store**: fingerprint/group_key/release_key ports with
   golden-vector oracles generated from the C implementation.
3. **Scanner**: walk budgets, ingest transaction, ownership/conflict
   state machine, sweep; differential test = same library scanned by both
   implementations, table-by-table `library.db` comparison.
4. **Verify job + `library/status`** semantics. *(completed in stage 6 —
   the job system serves real scan/verify state and `verify_library`
   persists per-package verdicts)*
5. **Read-only JSON API** on a minimal hand-rolled HTTP/1.1 subset
   (std-only, blocking threads; no async runtime): health, session
   lifecycle, artists/albums/releases/tracks with exact envelopes.
   Golden-JSON oracles recorded from the C server.
6. **Byte serving**: audio/waveform/assets with exact 206/304/416,
   `Accept-Ranges`, strong ETag/`If-Range`, security headers,
   inline-safe logic; validated against the client's 64 KiB block-reader
   contract.
7. **Static hosting**: SPA fallback + COOP/COEP; end-to-end smoke via the
   existing Playwright suite. *(implemented in stage 6; the Playwright
   smoke runs against the migrated web tree in R2)*
8. **Operations**: jobs API, startup scan, graceful shutdown, docker/compose,
   deployment docs. *(jobs API + startup scan implemented in stage 6.
   R4.5 adds an opt-in graceful drain — `--shutdown-file` /
   `MUSICPACK_SHUTDOWN_FILE`, ADR 0014 — while signal handling stays out of
   scope by ADR 0009. No container/compose artefact exists in the repository;
   deployment is documented as external in `docs/server-production.md`.
   Closed.)*
9. **Cutover**: parity sign-off, legacy server retirement decision.
   *(executed at R2: the migrated web Playwright suite runs the cutover
   smoke against the Rust server — session, browse, search, play, seek,
   waveform, representations, offline — with the C server's observable
   behavior as the oracle; `docs/server-cutover-checklist.md` §7/§8 is
   the standing handoff contract. R4.5 closes this: the checklist is
   CLOSED, the release build is verified against the live C oracle
   (188/0), and the C server is classified oracle-only with a documented
   rollback role — `docs/r4-completion.md`.)*

## Open questions (decisions needed before stage 1)

- **O-S1 — SQLite driver vs. the no-C-dependency rule.** `AGENTS.md` bans
  C dependencies (motivated by wasm compatibility). A server is inherently
  native; `rusqlite` (bundled SQLite) contains C + internal `unsafe`,
  isolated inside the server crate behind a store trait. Precedent exists
  (`claxon`'s five audited internal `unsafe` blocks). Requires explicit
  sign-off to scope the rule to wasm-targeted crates, or an alternative
  driver decision.

## R3.3 — schema v11: track-linked lyrics (implemented)

The first additive migration beyond the C's ten (D-S2 sanctioned
"additive v11+ only"; ADR 0007 permanent class). Full contract:
`docs/musicpack-lyrics-v1.md` §7.

- **Migration 11** (the first Rust-defined migration, appended in
  `store/schema.rs`): `assets.track_id INTEGER REFERENCES tracks(id) ON
  DELETE CASCADE` (nullable; NULL ⇔ package-level), `assets.lang TEXT`
  (the manifest's language tag, persisted because track detail must
  expose it while the server never re-reads packages or parses bytes —
  see the §7.1 clarification), and `assets_track_idx`. The C migration
  loop (`for (i = current; i < count; …)`) falls through on a higher
  recorded version, so a Rust v11 database still opens under the C
  server (proven by `db_compat::legacy_server_opens_a_rust_created_database`
  when `MUSICPACK_LEGACY_SERVER` is set); the C simply cannot see the
  new columns. The Rust `DatabaseTooNew` ceiling moved 10 → 11.
- **Ingest:** per-track `lyrics[]` sync through the same
  `sync_one_asset` machinery with the track-extended natural key
  `(release_id, kind, track_id, relative_path)`; rows insert after the
  package-level asset groups so package-level ids stay identical to the
  reference's assignment order. No probes, no content reads.
- **API:** release `assets[]` filters `track_id IS NULL` (byte-identical
  for every pre-existing library); track detail appends an optional
  `lyrics[]` of refs (`id`/`url`/`size`/`mimeType`/`sha256`/`lang`)
  after `context`; bytes flow through the unchanged
  `GET|HEAD /api/v1/assets/{id}`. The server never parses lyric content.
- **Differential boundary:** the C (a) never creates track-linked rows,
  so `tests/ingest_oracle.rs` compares the assets table on the
  C-visible columns over `track_id IS NULL` rows and shape-asserts the
  schema version (10 vs 11); (b) drops the track-level field on read,
  so a C-verified scan of a lyrics package reports the orphan files as
  `warning` (spec §6.5) — asserted, not smoothed, in
  `tests/lyrics_server.rs::c_differential_lyrics_package_boundary`,
  which also proves the byte-identical release-level asset region and
  the exactly-additive track-detail member against the live C server.

- **O-S2 — HTTP stack.** ✅ **Settled (stage 4) as recommended.** hand-rolled std-only HTTP/1.1 subset
  (the served surface is ~17 endpoints plus byte serving; semantics are
  fully specified by the legacy behaviour; zero new dependency risk).
  Revisit a framework only if write APIs/TLS/HTTP/2 appear.
- **O-S3 — SV7 probe gap.** The legacy server indexes `musepack-sv7`
  streams; the Rust core rejects SV7 by decision. Byte serving is
  codec-agnostic (unaffected), but the probe needs an SV7 *header-parse
  only* path (no synthesis) — small, decoder-scope-adjacent decision.
- **O-S4 — identity placement.** Collector identity lives in the server
  crate initially; if the Rust Author app ever needs it, promote to a
  shared module then, not now.
