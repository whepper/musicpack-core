# The `mpak:` container source

A `.mpak` container holds a whole package — manifest, audio, waveforms,
artwork, lyrics — in one file. Two independent capabilities exist for it, and
they are deliberately separate:

| Capability | Status | Owner |
|---|---|---|
| **Client byte transport** — read a member over ranged HTTP/OPFS reads | done (Stage 1) | `crates/musicpack-engine/src/mpak_source.rs` |
| **Server ingestion and indexing** — discover, open, verify, project a container into the library index | done | `crates/musicpack-server/src/source.rs` |
| **Server HTTP byte serving** of an indexed container member | done | `crates/musicpack-server/src/media.rs` |
| **MANF → client queue → playback** | done | `crates/musicpack-wasm/src/lib.rs`, `web/app/src/lib/mpak/container-tracks.ts` |

Both halves address a member the same way, with one shared definition of the
key, so neither can drift from the other.

## The shared key form

A container member is addressed by one URL-shaped key:

```text
mpak:<container-url>#<member-path>
```

`container-url` locates the container (an HTTP(S) URL, an offline store key, or
a local path). `member-path` is the package-relative member path, the same
string a `PackageBackend` resolves.

The form is defined **once**, in `musicpack_core::player::source_url`
(`parse_container_source`, `format_container_source`, `transport_url`), because
both the client transport and the server's index must produce byte-identical
keys. The engine re-exports it, so its public API is unchanged. Nothing else in
either codebase parses the form.

## Server ingestion and indexing

`crates/musicpack-server/src/source.rs` defines `PackageSource`, the server's
single source abstraction:

```text
PackageSource
  ├── Directory { root }  → manifest.json + regular files
  └── Container { .. }    → MANF + member table, via musicpack-core
```

Everything downstream — identity, the missing-object count, the codec probe,
verification, the content sync — asks the source and never branches on the
kind. The container arm never parses the container: it goes through
`MpakBackend::open_file`, i.e. core's authoritative implementation over core's
native `FileSource` (a seek + read handle, not the client transport's range
adapter).

### Discovery

`discover::classify` is the server's **one** classification point: a directory
named `*.mpack` is a bundle source, a regular file named `*.mpak` is a
container source, and a name that disagrees with its physical shape is skipped.
`discover::classify_source_path` reuses it to recover a recorded package's kind
during `verify`, so the walk and the verify pass cannot disagree.

The C reference only ever walked `.mpack` directories, so the Rust walker is a
deliberate **superset**. `tests/discovery.rs` pins that difference, and its
differential cross-check against the C scanner still passes because it compares
only *valid* packages.

### The index

A container is recorded in the same `packages` row as a bundle, with its
**file** as `path`; a member is recorded in `audio_objects.relative_path`
exactly as a bundle's relative path is. No schema change and no new column: the
locator and the member are already the two facts a container track needs.

`Store::replace_release_content` and the probe/size helpers therefore take a
`&PackageSource` instead of a `&Path`, because a member's size comes from the
member table rather than a `stat` that cannot exist.

### The canonical playback source

`source::playback_source` builds a track's key with core's formatter;
`source::indexed_source` derives it from the **stored** row
(`MediaRef::package_path` + `MediaRef::relative_path`), and
`source::resolve_playback_source` parses it back. Together they are the
server's round-trip guarantee, asserted in `tests/mpak_ingest.rs`:

> an indexed container track resolves back to exactly the container and member
> it was indexed from.

## HTTP byte serving

An indexed container member is served through the **existing** media endpoint,
the existing range decision tree and the existing `Body::FileRange` — there is
no second response body and no second range parser.

```text
HTTP Range (member-relative)
    ↓  serve_media: parse_range against the MEMBER's length
logical slice (offset, len)
    ↓  + MediaResource::base
file seek  =  base + offset      ← the only place the container offset appears
```

`media::open` resolves the stored locator with the same classifier the walk
uses, so it cannot disagree with the index about what a package is:

- **Directory bundle** — unchanged: `resolve_contained(package_path,
  relative_path)`, then `open_regular_file`. `base = 0`, `size` = the file's
  `fstat`. Byte-for-byte the previous behaviour.
- **Container** — the locator *is* the `.mpak` file, opened under the same
  discipline as any served file (not a symlink, regular, single link). The
  member's `(offset, length)` come from `PackageSource::member_extent`, i.e.
  from core's member table, and become `base` and `size`.

Three properties follow structurally rather than by convention:

- **`size` is the member's length**, never the container's. So every
  `Content-Range` total, `Content-Length` and `416` boundary is
  member-relative, and the client never learns the container's size.
- **Bytes outside `[base, base + size)` are unreachable.** The only consumer of
  `base` is the file seek, and the range is parsed against `size`, so a request
  cannot address a byte the member does not own.
- **The member path is never joined onto anything.** It is a lookup key in the
  container's member table, so `../`, absolute paths and unknown members all
  fail closed (`503 audio object not found`) instead of resolving to a file. A
  container that no longer validates serves nothing at all.

`magic_safe` reads the inline-safety header from `base`, so an image member is
judged on its own leading bytes and not on the container header in front of it.

Cost note: a container request re-scans the container index per request (a
bounded `TAIL`/`INDX`/`MANF` read, never a whole-file read), because
`media::open` is stateless by design — exactly as it opens a fresh file handle
per request.

## MANF → client queue → playback

A container that is **already available to the client** (reachable at a URL or
offline key the range transport can read) exposes its `MANF` tracks through the
ordinary queue and player. There is no container queue, no container player and
no second source kind; the tracks are ordinary `QueueItem`s.

```text
.mpak
  ↓  RustPlaybackEngine.openContainer(url, size)   → worker `openContainer`
MANF                                              → core MpakBackend + manifest parser
  ↓  track/member identity
mpak:<container>#<member>                          ← core's formatter, once
  ↓  itemsForContainer() → queue.playItems()
existing playback source routing
  ↓  worker's transportUrlFor + readRange
RangeByteSource → core MpakBackend
  ↓
audio member bytes → existing decoder/playback engine
```

| Item | Role |
|---|---|
| `core_impl::container_album` (`crates/musicpack-wasm`) | opens a container over a host's **synchronous** range reads and reports its `MANF` tracks: album identity, per-track member, member length, sniffed codec hint, declared duration |
| `wasm.containerTracks(read, container, size)` | the browser binding for the above; returns plain JSON, no handles |
| `handleOpenContainer` (`rust-playback.worker.js`) | registers the container's transport (idempotent, and **without** closing existing sources) and calls the binding with the same `readRange` playback uses |
| `RustPlaybackEngine.openContainer` | the main-thread call: spawns a worker on demand, needs no audio context, and leaves any live playback session alone |
| `itemsForContainer` / `containerQueueItems` (`web/app/src/lib/mpak/container-tracks.ts`) | field mapping only — Rust hands over each `mpak:` key and it is passed through untouched, so client and container can never disagree about a member |
| `queue.playItems` / `queue.addItems` (`web/app/src/lib/state/queue.ts`) | generic entry points for a ready-made item sequence; a server-backed release still uses `playAlbum` |

`size` (the container's length in bytes) is a required argument: a container's
`TAIL` framing is at the end of the file, so it cannot be scanned without it.

The codec hint is **sniffed from the member's own leading bytes** through
`musicpack_core::audio::open`, not guessed from the member's extension — the
core deliberately has no extension→codec table. A member that is not a
decodable stream is reported with no hint rather than a wrong one: the track
still exists (`MANF` is authoritative for membership), and playing it fails
loudly at backend selection instead of the album looking complete.

A container track has no server row, so its `QueueItem.trackId` is a stable
hash of the canonical key (negative, so it can never collide with a positive
SQLite rowid) and `releaseId`/`albumId` are absent. `QueueItem` types those two
as optional, which every existing reader already tolerates.

### Tests

- `crates/musicpack-wasm` → `container_manf_tracks_are_discovered_with_canonical_sources`,
  `a_manf_discovered_member_decodes_identically`,
  `a_member_that_is_not_audio_is_reported_not_hidden`,
  `an_invalid_container_fails_closed`. The middle one drives the **complete**
  path against a container written by core's own writer and asserts the PCM
  equals a whole-file decode of the same member **and** the frozen oracle
  digest, with two members proven independent.
- `web/tests/unit/mpak-client-tracks.test.ts` → the browser half against the
  committed `fixtures/reference/reference-small.mpak`, read back by core's real
  reader through the generated wasm module.
- `web/tests/unit/rust-playback-engine.test.ts` → the worker protocol mapping,
  generation isolation, and that opening a container does not disturb playback.

## Known gaps

- **Offline install of a container** as one stored file, and any container
  acquisition (downloading, syncing, a container manager). The `MANF` → queue →
  playback path above assumes the container is already available to the client.
- **`PlaybackSource` still has no typed member field.** The `mpak:` key remains
  an opaque URL-shaped string, so a member is not inspectable as a field
  (playback routing does not need it).

## Client byte transport (Stage 1)

## The abstraction

`crates/musicpack-engine/src/mpak_source.rs`:

| Item | Role |
|---|---|
| `parse_container_source` | the single parser: `Ok(None)` plain, `Ok(Some)` container, `Err` malformed `mpak:` key |
| `format_container_source` | the inverse, percent-encoding the container part |
| `transport_url` | the URL/key that actually serves bytes (container URL, or the key itself) — total, never fails |
| `RangeByteSource` | core's `ByteSource` over one transport URL: what makes a container scannable by ranged reads |
| `RangeSourceBackend` | the host `SourceBackend`: plain ranges *and* container members behind one synchronous range reader |

`RangeSourceBackend` composes two things that already existed rather than
reimplementing them: `RangeByteSource` → core's `MpakBackend` (so the container
index, member bounds and member rules are core's, unchanged) →
`PackageSourceBackend` (so a member open is the same code path a directory
bundle member takes). No change to `musicpack-core` was needed.

## Range and EOF behaviour

The host contract is one synchronous function:

```rust
fetch(url, offset, len) -> Result<Vec<u8>, String>
```

- `url` is always the **transport** URL. A host never sees a member key, so it
  implements one flat reader and nothing else.
- A **short reply is normal** and is resumed, not treated as failure: a
  block-aligned host (`networker.js`, `localreader.js`) caps every reply to its
  64 KiB block. `RangeByteSource` loops until the exact read core's `ByteSource`
  demands is satisfied.
- **A reply of zero bytes before the read is satisfied is an error**, not EOF —
  silently treating it as EOF would hand a decoder a truncated member.
- A read range outside `[0, size)` is refused before any host call.
- `RangeRead` (the plain-source path) caps each host call at 64 KiB and lets a
  short reply simply advance; `read` returning `0` is the EOF signal.

Reading a member therefore performs *more* host calls than reading the same
member whole, and reads only that member's byte span. Nothing pulls in the rest
of the container.

## Cache behaviour

A scanned container is cached per **(container URL, transport size)** for the
lifetime of the backend, so a second member of the same container costs no
re-scan. The size is part of the key because it is what located the tail
framing: the same URL with a different size is a different container.

Only successful scans are cached. A failed scan (unreachable or malformed
container) is retried on the next open instead of being remembered as broken.
The cache is **not** invalidated when a container's bytes change under a stable
URL — a host that can rewrite a container must build a new backend. In the web
player this costs nothing: `rust-playback.worker.js` builds a fresh engine, and
with it a fresh backend, on every `open`.

## Sizing

A container source must carry the **transport size** — the *container's* length,
not the member's — in `PlaybackSource.byte_size`, because the container tail
framing is at the end of the file and a member length would place the lookup in
the wrong place. A member source without it fails with an explicit error naming
both the member and the container. Plain sources ignore `byte_size`, exactly as
before; `crates/musicpack-wasm` reads it from the item JSON only for this case.

## WASM wiring

`musicpack-wasm`'s `RangeBackend` now delegates to `RangeSourceBackend`, so the
browser host needed no container logic. The wasm-side change is three small
pieces:

- `item_from_value` reads `byteSize` (previously dropped as `None`).
- `RangeFetch`'s doc states that `url` is always the transport URL and that a
  short reply is normal but a no-progress request is an error.
- `toWasmItem` (`rust-playback-engine.ts`) passes `byteSize` through.

In the worker, `transportUrlFor` is the one place that knows the key form
(it only answers "which transport serves this key"), and `openSource` opens the
container rather than the member key. All container semantics stay in Rust; the
JS mirror is deliberately trivial and covered by the same test vectors.

## Tests

- `crates/musicpack-engine/src/mpak_source.rs` — unit: the shared form resolves
  to core's definition, exact reads across a host that caps every reply at 64
  bytes, out-of-range refusal, no-progress and transport-failure handling,
  plain sources, the missing-size error. (The parser's own vectors moved to
  core with the form itself.)
- `src/player/source_url.rs` — the key form's tests: plain vs container, `#` in a
  member path, percent round-trip, each malformed case, totality of
  `transport_url`, and the playback-source helper.
- `crates/musicpack-engine/tests/mpak_range_source.rs` — **the acceptance
  criterion**, over a transport that models the browser's block alignment:
  every member equals the source bytes, and the decoded PCM of a member read
  through ranged container reads is **identical** to decoding the same member
  whole. Plus cache reuse, error paths, and an offline-shaped store-key case.
  `a_real_container_exposes_its_members` is opt-in via `MUSICPACK_MPAK_FIXTURE`
  and reads every member of a real container through the range transport,
  comparing against the same member read from the whole file.
- `crates/musicpack-wasm` — `a_container_member_decodes_identically_over_the_range_source`
  proves the same invariant at the wasm boundary with a real SV8 member, and
  additionally checks the PCM against the frozen `musepack_oracle.jsonl` digest
  when the corpus is present.
- `web/tests/unit/mpak-source.test.ts` — the browser-layer facts only: the host
  is asked for the container URL and never the member key, the scan really
  reaches the tail framing through ranged reads, an offline store key works
  identically, a missing size is reported, a malformed key is rejected, and
  plain sources are untouched. Decode equivalence is deliberately *not*
  re-proven here — that would mean writing a container writer in TypeScript,
  putting format logic in the wrong layer.
- `crates/musicpack-server/tests/mpak_ingest.rs` — the server slice: a real
  container (the committed `fixtures/reference/reference-small.mpak`, and one
  packed at runtime with core's writer holding real SV8 audio) is discovered,
  opened, verified and projected; every indexed track has the canonical key;
  the key resolves back to the exact container and member; the member's bytes
  come back unchanged; an invalid container is recorded invalid and projects
  nothing; a verifying scan uses core's container verifier; and a directory-only
  library indexes exactly as before.
- `crates/musicpack-server/tests/mpak_media_serving.rs` — the byte slice, over
  the real scan → store → `media::open` → `serve_media` path, reading the
  produced `Response` body: full member, bounded range, open-ended range,
  suffix range, the final member byte (with a non-vacuity check that the member
  does not end the file), out-of-range and malformed ranges leaking nothing,
  two members resolving to different bytes, an unknown/hostile member path, a
  corrupted container, a 1.5 MB member at depth, and the committed fixture
  served both whole and ranged.
- `crates/musicpack-server/src/discover.rs` — unit: the classification table
  (name × physical shape) and recovering a recorded locator's kind.

## Not in scope

Deliberately unimplemented, pending product decisions:

- Container *acquisition* — downloading, syncing, or a container manager. A
  container is something a host hands the client by key; `MANF` → track
  discovery → queue → playback for a container that is **already available** is
  implemented (see above).
- Offline install/audit of a container as one stored file, and any change to
  the per-track OPFS layout.
- A typed source representation for the member. The `mpak:` key is the interim
  form; replacing it with a typed `PlaybackSource` field is a single-module
  change because only `musicpack_core::player::source_url` parses it.
