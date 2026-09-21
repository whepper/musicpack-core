# MusicPack Lyrics — v1

> **Status: NORMATIVE for R3.** Version 1. Lyrics are **optional** assets
> carried by `.mpack` v1 packages. This document defines the lyrics
> *content* format, the per-track manifest reference, the timing
> semantics, the server/API contract, the playback integration and the
> offline behavior. An incompatible change requires a version bump of
> this document, not of `.mpack` v1 (see §15).
>
> Lyrics are **authored package data** (not derived like waveforms): a
> human-authored text document with optional line timestamps. The Rust
> core (`musicpack-core`) is the authoritative parser and owns the
> timing model. Servers treat lyric bytes as opaque hash-verified
> assets; clients parse and render them. No component other than the
> core defines what a timestamp means.

---

## 1. Scope and purpose

A `.mpack` v1 package may carry lyrics documents for its tracks. v1
delivers the first consuming feature end to end:

```text
Author stores lyrics/<name>.lrc and references it from the track
      ↓
musicpack-server ingests + serves the stored bytes (hash-verified)
      ↓
web client fetches bytes through the existing asset endpoint
      ↓
musicpack-core (via wasm) parses, normalizes and answers
   "which line is active at position p" — the only timing authority
      ↓
the lyrics UI renders lines and follows playback
      ↓
offline downloads include the same bytes under the same hashes
```

The v1 goal is a **deterministic, well-specified lyrics model that works
from `.mpack` through core, server/API, Web Player, synchronized
playback and offline use** — with the UI consuming the domain contract
rather than defining it.

### Intended features (v1)

- Plain (unsynchronised) lyrics display.
- Line-synchronised lyrics with deterministic active-line highlighting.
- Automatic follow with manual-scroll suspension and an explicit
  resume-follow affordance.
- Offline availability identical to other track assets.
- Multiple lyric documents per track (e.g. languages) with a
  deterministic default.

### Explicitly not in v1 (see §18)

Word-level (karaoke) timing, translation display, a synchronized lyric
*editor* in Author, server-side parsing/validation of lyric content,
language-preference matching, per-line end times as stored data.

---

## 2. Current state (inspection record, 2026-09-20)

What already exists — the format/server work below is a **smallest
extension of real, shipped behavior**, not a greenfield design:

| Layer | State today |
|---|---|
| `.mpack` manifest | Root-level `lyrics[]` array of `{path, sha256}` opaque assets (`MAX_LYRICS = 512`), `lyrics/` directory convention, package-unique paths, sha-verified, MPAK pack order after booklet. **No track linkage, no language, no content semantics.** |
| Legacy C CLI | `create/import` copies loose `*.lrc` files from the source tree (names preserved as-is) and extracts **unsynchronised** `LYRICS`/`UNSYNCEDLYRICS` tags into `lyrics/{NN} - {Title}.txt`. **No LRC parsing anywhere.** |
| C + Rust server | Ingest syncs `lyrics[]` → `assets` rows (`kind='lyrics'`, release-scoped, `.lrc` → `text/plain`). Release detail exposes `assets[]`; `GET /api/v1/assets/{id}` serves artwork/booklet/lyrics bytes under the full byte contract (Range/ETag/304/416). **No track↔lyrics association exists** (no `track_id` on assets; track detail has no lyrics). |
| Web Player | Typed `AssetRef`/`ReleaseDetail.assets` consumed only by the Package inventory section. Offline plan **defers** booklet/lyrics/extras (recorded as D3 "back-fill without schema churn"). No lyrics UI; lyric bytes are never fetched. |
| Author | Draft carries `lyrics: AssetEntry[]`; the ArtworkManager panel can attach lyric *files* to the package (root `lyrics[]`). No parsing/editing. |
| Fixtures | `test-musicpack-album.mpack` / `test-flac-album.mpack` carry one-line `.lrc` files (`[00:00.00]…`) — evidence the ecosystem already uses LRC-shaped files, but nothing consumes their timing. |

**Conclusion:** the missing pieces are exactly (1) a lyrics content
format definition, (2) track linkage, (3) a core model/parser owning
timing, (4) per-track API exposure, (5) offline inclusion, (6) UI.

---

## 3. Lyrics types in v1

v1 supports exactly two content classes, distinguished **by parsing**,
never by stored metadata:

1. **Plain (unsynchronised) text** — zero or more lines, no valid line
   timestamps. Rendered as static text; the UI must never present it as
   synchronised.
2. **Line-synchronised** — at least one line carries a valid timestamp
   (§6). Rendered with a deterministic active line (§8).

**Word-level timing is not supported in v1.** There is no MusicPack
requirement for karaoke highlighting; the A2-enhanced LRC form
(`<mm:ss.xx>` word tags) is reduced deterministically (§7.5) rather
than modelled. If a concrete product need appears, that is a v2 with a
new document class — not an accretion to this one.

**A native MusicPack representation (JSON document) was considered and
rejected for v1:** line-timed LRC already covers the requirement; a
parallel native format would duplicate it, fork the authoring
ecosystem, and force a converter. Instead the *parsed model* (§8) —
not the file bytes — is MusicPack's native representation, and it is
what every consumer (wasm client, future native clients) programs
against. The file format remains an interchange detail owned by the
parser.

---

## 4. Content format: a strict LRC profile

A lyrics document is an **LRC file** under the following profile. The
profile is strict where LRC is ambiguous, so that parsing is a total
function with no implementation-defined outcomes.

### 4.1 Encoding and normalization (normative)

| Rule | Value |
|---|---|
| Encoding | UTF-8 exactly. Any invalid UTF-8 byte sequence → `Error::InvalidUtf8`. |
| BOM | A leading UTF-8 BOM (U+FEFF, `EF BB BF`) is stripped before parsing. |
| Newlines | CRLF and CR are normalized to LF before line splitting. |
| Trailing newline | Optional; does not create a line. |
| Empty lines | Preserved as empty lines (they are display content: stanza breaks). An empty line can carry a timestamp. |
| Unicode normalization | **None.** Bytes (post newline/BOM normalization) are preserved verbatim; text is never NFC/NFKC-folded. Display is the renderer's concern. |
| Whitespace | Leading/trailing ASCII whitespace is trimmed from line *text* (not from the raw line, which affects nothing else). Interior whitespace is preserved. |
| Tabs | Treated as whitespace for trimming; preserved in interior text. |

### 4.2 Size and count limits (normative)

Enforced **before** allocation where possible; a violation is a typed
error, never a panic, never a truncated accept.

| Limit | Value | Error |
|---|---|---|
| Document size | 524 288 bytes (512 KiB) | `DocTooLarge` |
| Line count | 10 000 | `TooManyLines` |
| Line length | 4 096 bytes (post-normalization) | `LineTooLong` |

Rationale: a full song lyric is 1–4 KiB; the limits bound a hostile
input at ~3 orders of magnitude above legitimate use while accepting
libretto-scale documents. The server separately enforces its own
object budgets; these limits govern the *parser*.

### 4.3 Grammar (normative)

```text
document   := (line)*
line       := [ tag ]* [ timestamp ]* text
tag        := "[" tag-key ":" tag-value "]"
timestamp  := "[" minutes ":" seconds [ "." fraction ] "]"
minutes    := 1*DIGIT            (* no sign, no leading '+'/'-' *)
seconds    := 2DIGIT             (* 00..=99 as written; see §6.2 *)
fraction   := 2DIGIT | 3DIGIT    (* centiseconds | milliseconds *)
text       := *UTF-8-char        (* may be empty *)
```

- A **timestamp** is a bracket token matching the timestamp shape with
  a *valid* numeric interpretation (§6.2). A bracket token whose
  content *looks like* `digits:digits` but violates the grammar
  (e.g. `[00:6a]`, `[1:2:3]`, `[-0:12.0]`, `[00:60.000]`) is a
  **`MalformedTimestamp` error** naming the line — a file that clearly
  intends timing must not silently degrade to plain text.
- A **tag** is a bracket token containing `:` whose key is any token —
  known keys are interpreted (§4.4), unknown keys are **ignored**
  (LRC metadata is a de-facto, unstandardized set; strictness here
  would reject real-world files for zero contract value).
- Any other bracket token at line start (no `:`, not timestamp-shaped,
  e.g. `[Chorus]`, `[Verse 1]`) is **ignored** — a section marker — and
  is not part of the text.
- Bracket tokens *after* the first non-bracket character are literal
  text. Leading ASCII whitespace is skipped before token extraction —
  an indented timestamp is a timestamp (a file that clearly intends
  timing must not degrade to plain text).
- **Multiple timestamps on one line** (`[00:12.00][01:45.00] text`) are
  standard LRC repetition: the line is expanded once per timestamp, in
  left-to-right order, each carrying the same text. Interleaved tags
  are allowed before/between timestamps.
- **Untimed lines in a document that has timestamps:** a line without
  a timestamp belongs to the **following** timestamped line's block —
  its text is prepended to that line's text, joined with LF. A leading
  untimed block therefore attaches to the first timestamped line, a
  trailing untimed block to the last. Document order is preserved
  inside the block; no content is ever dropped or reordered across
  blocks. (In a document with no timestamps at all every line is plain
  content — the §3 plain class — and this rule does not apply.)

### 4.4 Metadata tags

| Tag | Meaning in v1 |
|---|---|
| `[ar:…]` | Artist — stored in the parsed metadata (display fallback). |
| `[ti:…]` | Title — stored. |
| `[al:…]` | Album — stored. |
| `[by:]` | Author/transcriber — stored. |
| `[offset:+N|−N]` | Global timestamp shift in **milliseconds**; integer with optional sign. Applied once to every timestamp (§6.3). Bounded: `|offset| ≤ 3 600 000` (1 h); a value outside the bound, or a non-integer, is a `MalformedOffset` error. |
| anything else `key:value` | Parsed as a tag, **ignored**, not preserved. |

Metadata is **advisory**: it never affects timing, identity, hashing or
package validation. The manifest (not the document) is the authority
for document identity and language.

### 4.5 Word-level tags (A2-enhanced LRC)

Inline word tags (`<00:12.34>`, `<00:12.34>`) are **stripped** from the
text after timestamp extraction; the line's own timestamp governs. The
reduction is deterministic and total: the document is line-synchronised
with word information discarded. This is a *defined* representation
(reported in parse diagnostics as `wordTagsStripped: true`), not
silent ambiguity.

### 4.6 Malformed input — summary

| Input | Result |
|---|---|
| Invalid UTF-8 | `InvalidUtf8` error |
| > 512 KiB / > 10 000 lines / line > 4 096 B | limit error (§4.2) |
| Timestamp-shaped bracket token with bad numbers | `MalformedTimestamp{line}` error |
| `[offset:…]` malformed or out of bound | `MalformedOffset` error |
| Shifted timestamp < 0 (offset applied) | `NegativeTimestamp{line}` error |
| Unknown/section bracket tokens | ignored (§4.3) |
| Duplicate timestamps | valid; ordering rule §6.4 |
| Timestamp ≥ track duration | valid; the model is duration-blind |

Parsing is **strict**: no error recovery, no line skipping. A document
either parses completely into the model of §8 or fails with a typed
error that the UI surfaces as "lyrics unavailable (malformed)". Lenient
rescue of hostile files is an *import-time* concern (future Author
normalizer), never a silent reader behavior.

---

## 5. Language and metadata

A lyrics **document** may declare `lang` — an optional, free-form,
non-empty string; **BCP-47 (`language[-Script][-REGION]`) is the
recommended form** (`"en"`, `"pt-BR"`, `"ja"`). It is *not* validated
against an enum (precedent: `genres`), but it is validated as non-empty
and free of control characters (a lyrics-specific rule; existing
manifest free-text fields carry no charset constraint beyond JSON
escaping).

Deliberately **not** modelled in v1: script (derivable from text),
explicit/clean variants, translator/credit roles (the document's
`[by:]` tag covers transcriber credit), per-line speaker/duet flags.

**Cardinality:** a track may reference **zero or more** lyric documents
(§6 — an array). Each document is one language/variant; the spec does
not interpret the distinction. Package-level documents without track
linkage remain possible via the existing root `lyrics[]` (§6.4).
Client selection for multi-document tracks in v1: **first entry in
manifest order** — deterministic, simple; preference-driven selection
is future work (§18).

---

## 6. Package format

### 6.1 Per-track reference (new, additive)

A track may reference its lyrics with a new **optional** `lyrics`
array, added to the track object. The shape mirrors the existing
per-track reference assets (`waveform`, `representations[]`):

```json
{
  "track": 3,
  "title": "Big in Japan",
  "audio": { "path": "audio/03.mpc", "sha256": "…" },
  "lyrics": [
    { "path": "lyrics/03 - Big in Japan.lrc", "sha256": "…", "lang": "en" }
  ]
}
```

- `lyrics` is an array of `{path, sha256, lang?}` — 0..=`MAX_LYRICS`
  entries (the existing 512 cap applies per array).
- `path`: canonical package-relative path (existing `format::path`
  rules; convention `lyrics/*.lrc`, not enforced by path rules).
- `sha256`: required, lowercase hex — same integrity model as every
  asset (§"Integrity" in `musicpack-v1.md`).
- `lang`: optional, per §5.
- Absent and empty are equivalent on write (canonical output omits the
  field entirely when empty) — same convention as `representations`.

**Canonical key order:** the track object's canonical serialization
gains `lyrics` **between `waveform` and `representations`** (the
per-track reference group). This order is normative for the Rust
canonical writer; the reference C writer never emits the field (§6.5),
so no cross-writer byte expectation exists for it.

### 6.2 Why per-track linkage (and not more root-array fields)

The existing root `lyrics[]` cannot express "this document belongs to
that track", and the legacy filename conventions are provably not a
contract (loose `.lrc` imports keep arbitrary names; tag-derived ones
are `.txt`). Three options were considered:

1. **Filename-convention matching** — rejected: not even the reference
   tooling is consistent with itself; heuristics make active behavior
   unpredictable, which §8's determinism requirement forbids.
2. **Linkage fields on root `lyrics[]` entries** — rejected: diverges
   the entry shape from every sibling asset array and forces an
   indirect join at every consumer.
3. **Per-track `lyrics[]`** — **chosen**: it mirrors the two existing
   per-track reference assets exactly (`waveform`, `representations`),
   joins naturally into the API and offline planner, and leaves the
   frozen root array untouched.

### 6.3 Validation (normative)

Rust parsing/verification rules for the new field:

- Track `lyrics` paths join the **package-unique referenced-path set**
  (a path referenced both per-track and in root `lyrics[]` is a parse
  error — every referenced asset is referenced exactly once). The
  uniqueness rule is format-wide and frozen: **two tracks cannot share
  one lyrics file**; identical lyrics for two tracks mean two files
  (two hashes, two asset rows).
- Per-track lyrics paths are included in `referenced_paths()`: they
  are hashed by `verify` like every asset, and never reported as
  unreferenced.
- No content sniffing: the verifier does not read lyric bytes beyond
  hashing. A package with a non-lyrics payload under a `.lrc` name is
  valid; consumers that fail to parse it surface that locally (§12).

### 6.4 Root `lyrics[]` — unchanged (frozen)

The root array keeps its existing meaning: **package-level** documents
with no track association (liner-note lyrics, librettos). It is served
exactly as today (release `assets[]`, `/assets/{id}`). A package may
use either or both levels. Legacy packages created by the reference
CLI keep working byte-for-byte.

### 6.5 Backwards compatibility (normative)

- **Reference C parser:** track-level `lyrics` is an unknown field
  nested inside a known object → ignored on read, dropped on rewrite
  (the documented unknown-nested-field rule of `musicpack-v1.md` §7).
  C tooling neither breaks nor preserves it.
- **Rust canonical writer:** emits `lyrics` only when non-empty, in
  the §6.1 position. For every manifest the reference can represent,
  output remains byte-identical to the reference writer (the field
  never appears in C-authored files). Packages *using* the field are
  Rust-writeable; rewriting them with the C CLI drops the field (and
  leaves the files as unreferenced-warning orphans) — documented,
  accepted, and re-hashed cleanly by Rust verification of the rewritten
  package only if the field is re-added.
- **No manifest `version` bump:** `musicpack-v1.md` §7 defines the
  growth rule — new fields are added as optional fields; existing
  fields never change meaning. This change follows it.
- **MPAK container:** per-track lyric files are `DATA` members like any
  other referenced asset. They form their own traversal group **after
  waveforms and before artwork** (all per-track groups stay together
  ahead of the package-level groups); the root `lyrics[]` group keeps
  its existing position after booklet. Packages without the additive
  field pack byte-identically to before.

### 6.6 Author pipeline status

The Author drives the frozen legacy CLI (`build-draft`), whose
`draft_to_manifest` drops unknown draft fields — so **per-track lyrics
authoring is impossible until the R4 Rust pipeline** and is explicitly
deferred there (the draft model will gain the field then, additively).
R3 Author scope: the existing root-level lyric *file* attachment keeps
working unchanged; nothing in R3 changes the Author.

---

## 7. Server data model

### 7.1 Schema migration v11 (additive)

```sql
-- migration 11 (additive; no rewrites, no data changes)
ALTER TABLE assets ADD COLUMN track_id INTEGER REFERENCES tracks(id) ON DELETE CASCADE;
ALTER TABLE assets ADD COLUMN lang TEXT;
CREATE INDEX assets_track_idx ON assets(track_id);
```

- `track_id IS NULL` ⇔ package-level asset (all existing rows and all
  behavior today). Ingest sets it for per-track lyric references.
- `lang` carries the manifest's optional language tag.
  **Implementation clarification (R3.3):** §7.3 requires track detail
  to expose `lang`, and the server never re-reads packages nor parses
  lyric bytes at request time — so the tag must be persisted on the
  asset row. The column is nullable, invisible to every pre-v11 reader,
  and NULL for package-level rows.
- The reference C migration loop (`for (i = current; i < count; …)`)
  **no-ops on a higher recorded version**, so a v11 database still
  opens under the C server; the new columns are invisible to it. This
  preserves the "C can open a Rust-created database" direction of
  `tests/db_compat.rs`. (The Rust side's own refusal rule
  `DatabaseTooNew` moves up to 11 accordingly.)

### 7.2 Ingest

Per-track lyric references sync through the existing asset machinery
(`sync_one_asset` equivalent) with `kind='lyrics'`, `role=NULL`,
`track_id=<the track>`, `lang=<manifest tag or NULL>`, natural key
`(release_id, kind, track_id, relative_path)`. Root `lyrics[]` rows
keep `track_id=NULL` exactly as today. No probes, no content reads:
lyric assets are indexed like booklet files (path + sha + size + mime
from extension). A changed `lang` or content hash updates the row in
place (identity is the natural key); removing a reference or its track
deletes the row (explicit sweep / schema cascade); package-level asset
rows are assigned their ids **before** track-linked ones, matching the
reference's assignment order for the same package.

Differential-ingest note: the reference C never creates track-linked
rows, so oracle corpora (C-generated) are unaffected; the differential
guarantee for them is unchanged.

### 7.3 API contract (normative, additive to `/api/v1`)

**`GET /api/v1/tracks/{id}`** (track detail) gains an optional
response field:

```json
"lyrics": [
  {
    "id": 12,
    "url": "/api/v1/assets/12",
    "size": 842,
    "mimeType": "text/plain",
    "sha256": "…",
    "lang": "en"
  }
]
```

- Present only when the track has lyric documents; omitted entirely
  (never `null`, never `[]`) — the envelope convention. The member is
  **appended after `context`** (last in the object), so every
  pre-existing response is a byte-prefix of the new one.
- Array order = asset id order = first-inserted manifest order (the
  deterministic default-selection order, §5). As with every asset
  array, row identity is the natural key: reordering the manifest
  array and rescanning does not reshuffle ids (the reference behaves
  identically for root assets).
- `sha256` follows the existing additive content-hash rule (same as
  representations/assets).
- `lang` omitted when absent.
- **Track list responses are unchanged** — lyrics ride only on detail,
  so browse responses do not grow; the client fetches detail for the
  current track (it already does).

**Byte serving is unchanged:** lyric bytes flow through the existing
`GET|HEAD /api/v1/assets/{id}` with the full existing contract —
single-`bytes=` Range, strong sha256 ETag, `If-None-Match`/`If-Range`,
200/206/304/416, `nosniff` + sandbox CSP, `text/plain` MIME from the
frozen extension table, `Cache-Control: private, max-age=0,
must-revalidate`. **No new endpoint.** A dedicated
`/tracks/{id}/lyrics` was considered and rejected: it would duplicate
the serving machinery for zero contract gain.

**`GET /api/v1/releases/{id}`**: `assets[]` lists **package-level
assets only** (`track_id IS NULL`). For every pre-existing library the
response is byte-identical to today (no row has `track_id`); new
per-track rows appear only in track detail. Auth, CORS, error
envelopes: unchanged (single existing gate; 404/401 exactly as
today).

### 7.4 Release-level track-lyric index (R3.6, additive)

The same release response gains one **optional** top-level member — the
flat index the offline planner consumes, so planning needs no per-track
requests and stays a pure function of the release payload:

```json
"trackLyrics": [
  { "trackId": 12, "id": 501, "url": "/api/v1/assets/501",
    "size": 842, "sha256": "…", "lang": "en" }
]
```

- **Relationship to track detail:** both representations describe the
  *same* underlying track-level lyric assets (`kind='lyrics'`,
  `track_id` set). Track detail's `lyrics[]` (§7.3) remains the
  online/detail shape with its required per-document fields (`mimeType`
  included); `trackLyrics[]` is the offline-planning projection and
  carries no MIME (no consumer). They are served from one shared read,
  so they can never diverge.
- **Order:** deterministic `(trackId, assetId)` — track order from the
  manifest's canonical media/track traversal, then first-inserted
  asset order per track. Selection stays "first entry in manifest
  order" (per track, §5); offline selects identically.
- **`sha256` is mandatory** in this index (unlike the optional-hash
  online rule): offline staging verifies bytes against it and must not
  hash-compensate for a missing server hash. A hashless row never
  enters the index.
- **`lang`** stays optional, exactly as elsewhere.
- **Eligibility:** track-linked rows only (never package-level), same
  VISIBLE package gate as track detail (valid/warning and
  verified/warning), never conflicted/unavailable/unverified content.
- **Omission:** the member is omitted entirely — never `null`, never
  `[]` — when the release has no track lyrics, so lyric-less releases
  and the legacy C server's responses stay byte-identical. It is
  **appended after `assets`**, so every pre-existing response is a byte
  prefix of the Rust one (the C boundary documented in
  `docs/r3.6-offline-lyrics-contract.md` and asserted by
  `crates/musicpack-server/tests/lyrics_server.rs`).
- **Offline identity:** the offline download key is
  `t.<trackId>.lyr.<assetId>` — asset ids are the stable identity
  (R3.3), so adding, removing, reordering or re-tagging references
  never re-keys staged bytes. Filenames and paths are never identity.
- **Integrity policy:** a lyric document that fails size/hash
  verification during an offline install is **non-critical**: the
  install proceeds, the document is recorded damaged and excluded from
  offline availability (the same class as a corrupt alternate
  representation). Playback is unaffected; the offline panel simply
  has no bytes for it.
- **No root-level lyrics** are introduced here, and package-level
  `assets[]` keeps its exact semantics.

**The server never parses lyric content.** No ingest validation beyond
hashing, no per-request parsing, no derived `synced` flag in the API —
syncedness is a property of the parsed document (§3), computed by the
client through the core.

---

## 8. The lyrics model (core, normative)

`musicpack-core::lyrics` owns the domain: parsing (§4), the model, and
the timing semantics (§9). Strong, small types:

```text
LyricsDocument {
    lines:   Vec<LyricsLine>,     // sorted; see §6.4 ordering
    metadata: LyricsMetadata { artist?, title?, album?, by? },
    word_tags_stripped: bool,
}
LyricsLine { timestamp_ms: i64, text: String }
enum LyricsError { InvalidUtf8, DocTooLarge, TooManyLines, LineTooLong,
                   MalformedTimestamp { line: usize },
                   MalformedOffset, NegativeTimestamp { line: usize } }
```

- Timestamps are **`i64` milliseconds**. LRC fractions convert exactly
  (2 digits → ×10; 3 digits → as-is). No floating point ever touches a
  timestamp.
- The model is `Clone + PartialEq`, `forbid(unsafe)`-clean,
  wasm-clean, dependency-free, and I/O-free (bytes in, model out).
- The model is **duration-blind**: it never learns the track duration;
  "after the last timestamp" semantics (§9) need none.

The core does **not** own: DOM, fetch/HTTP, OPFS, SQLite, Web Audio,
Media Session, Svelte state, or any UI concern.

---

## 9. Timing semantics (the contract)

The single timing authority is a pure, total function:

```text
active_line(doc: &LyricsDocument, position_ms: i64) -> Option<usize>
```

**Definition:** the index of the **last line whose `timestamp_ms ≤
position_ms`**; `None` when `position_ms <` the first line's timestamp.

Normative consequences:

- **Units/precision:** milliseconds, integer. Clients convert their
  clock to ms with **floor** (`(p_seconds * 1000) | 0` for
  non-negative p) before calling; the conversion rule is part of this
  contract so every client computes identically.
- **Ordering:** lines are sorted by `timestamp_ms` ascending. **Ties
  (duplicate timestamps) keep document order** (stable sort); among
  equal timestamps the **last in document order** is the one
  `active_line` returns. Document order is therefore semantic, not
  incidental, and is preserved by the parser.
- **Zero timestamp:** `[00:00.00]` is a normal timestamp; a line at
  position 0 is active from the start of playback.
- **Negative timestamps:** unrepresentable. An offset shift that would
  produce one is a parse error (§4.6).
- **Before the first timestamp:** `None` — the UI shows the document
  with no active line (instrumental-intro state), never line 0.
- **Between timestamps:** the preceding line stays active (there are
  no end times; a line holds until its successor's timestamp).
- **After the last timestamp:** the last line stays active until the
  track ends.
- **Seeking:** `active_line` is a pure function of position — seek
  forward, backward, or arbitrarily and the active line is *recomputed*,
  with no hysteresis, smoothing, or memory of the previous line.
- **Pause / buffering:** the position source stops advancing, so the
  active line cannot change (no independent timers anywhere).
- **Backwards jumps** (previous track, rewind): recomputed identically.
- **Crossfade / gapless transitions:** the position consumed is the
  player's current-track position (the same value the waveform seek
  uses); at a boundary the current track changes, the lyrics document
  for the new track is installed, and evaluation restarts from that
  document — there is no cross-track timing state.
- **Monotonicity invariant** (property-test target): for `p1 ≤ p2`,
  `active_line(p1) ≤ active_line(p2)` (as indices, `None` = −∞).

**Where position comes from (web):** the existing player model
(`playerModel.positionSeconds`, the same coalesced tick feed the
waveform seek consumes) — mapped through the current item's
`trackStartSeconds` to within-track position. The lyrics controller
**subscribes**; it never runs a timer, never polls the audio graph,
and never derives timing itself. The lookup runs in Rust through the
wasm binding (`lyrics_active_line(handle, position_ms) → index`), so
the only timing logic in TypeScript is "which static line text does
this index point at".

---

## 10. Web client integration

- **API client** (`lib/api/client.ts`): `trackDetail` gains the typed
  `lyrics?: LyricsRef[]` field (DTO in `api/types.ts`). It is the only
  place that knows the endpoint shape; components never fetch lyrics.
- **Lyrics controller** (`lib/playback/lyrics.ts`, framework-free,
  unit-testable): for the current track — resolve document refs →
  fetch bytes (network path via the API client's asset URL; offline
  path via the offline store) → parse through the wasm core →
  subscribe to the player model → expose `{state, lines, activeIndex,
  isSynced}`. States: `no-lyrics | loading | plain | synced |
  error(malformed)`.
- **Refetch/staleness:** the controller re-evaluates on track change;
  within a track it does not refetch (a document is immutable content
  addressed by its sha256 — the server's ETag semantics make any
  intermediary cache safe anyway).
- **UI** (`lib/ui/lyrics/*`): renders from the controller's state;
  owns only presentation concerns (scroll position, follow state).
  Timing semantics, document structure and selection stay in the
  controller/core — no lyrics logic in `.svelte` files (R2 rule).

---

## 11. Offline behavior

**Implemented in R3.6.** The download planner (`lib/offline/plan.ts`,
still pure and fetch-free) stages every entry of the release-level
`trackLyrics[]` index (§7.4): kind `lyrics`, key
`t.<trackId>.lyr.<assetId>`, url/size/sha256 taken verbatim from the
server index — no hash is ever computed client-side to compensate, and a
hashless entry is skipped rather than guessed. **Package-level root
`lyrics[]` stay deferred** (D3 unchanged) — no consumer exists for them.
The investigation and the resolved contract live in
`docs/r3.6-offline-lyrics-contract.md`.

- **Staging:** the existing installer path is used unchanged: download →
  incremental SHA-256 → size verification → `FileStore` write → atomic
  catalog commit. Bytes move under the identity key, so adding,
  removing, reordering or re-tagging references never re-keys staged
  bytes.
- **Language** is not duplicated onto the asset record; it stays on the
  release index inside the committed package snapshot (the same place
  the online panel reads it from), so offline rendering can present the
  same tag.
- **Local-first:** an installed package's lyrics render from the local
  store, online or offline (same invariant as audio). The offline lookup
  is by identity (`trackId`, asset id) and returns only assets committed
  with `state:'ok'` — a damaged document has no addressable bytes.
- **Staleness:** the update check derives lyric hashes from the same
  release index, so a lyric replaced upstream flags the package stale
  exactly like audio; hashes ride the existing audit/update mechanism
  and replacement stays user-initiated (D2 unchanged).
- **Integrity / partial states:** a lyric document that fails its
  size or hash check is **non-critical** (the same class as a damaged
  alternate representation): the install still commits, the document is
  recorded damaged with no addressable bytes, and it is excluded from
  offline availability. Playback is unaffected; offline + missing bytes
  render as the plain "not available offline" state of the empty state
  (§13), never a broken panel. A hash mismatch is never silently
  accepted, and the installer's typed integrity error / atomicity
  behavior is unchanged.
- **No track-detail requests:** planning reads the release payload only;
  the e2e suite asserts an install makes zero `/tracks/{id}` requests.
- **UI:** offline lyric *presentation* remains deferred (no offline
  lyrics UI, no language selector, no root-level lyrics UI, no
  now-playing surface).

---

## 12. UI/UX contract (behavioral, normative for tests)

- **States:** unavailable (no refs) · loading (reserve space, no
  layout jump) · plain (scrollable text, never styled as synced) ·
  synced (active-line highlight + follow) · error (malformed document:
  explicit message, retry = refetch).
- **Auto-follow:** when synced and playing, scroll the active line into
  view smoothly; scrolling is **restrained** (only when the active line
  changes; honors `prefers-reduced-motion`).
- **Manual scroll:** any user wheel/touch/scroll input suspends
  auto-follow immediately (no fighting the user). Follow resumes only
  via the explicit **resume-follow** affordance (a floating "return to
  current line" control that appears while suspended and the active
  line is off-screen) or by seeking/track change.
- **Long lines:** wrap (`overflow-wrap`); never clip mid-glyph; the
  panel scrolls, the page does not.
- **Small screens:** lyrics render in the mobile player layout with
  ≥44 px touch targets for controls; text remains ≥ the base type
  scale.
- **Accessibility:** the lyrics region is a labelled landmark; the
  active line is exposed via `aria-current="step"` on the line element
  (per-line announcements are **not** fired — one region, no
  per-tick live-region chatter); full keyboard operation follows the
  R2 baseline (the panel is scrollable via keyboard; no keyboard trap);
  plain/empty/error states are announced as text, not color alone.

---

## 13. Synchronized behavior matrix (normative)

| Event | Expected behavior |
|---|---|
| Start playback | first applicable line per §9 (`None` before first ts) |
| Pause / resume | active line unchanged (position frozen) |
| Seek forward/backward | recompute; follow resumes if it was on |
| Track change (next/prev/queue) | new document install; recompute from 0-position state |
| Gapless boundary | as track change; no cross-track state |
| Crossfade | follows the player's current-track position; at swap, new document |
| Buffering | position frozen → line frozen |
| Playback error | follow stops; last state retained; no error UI beyond the player's own |
| Representation change | no lyrics effect (documents are per-track, not per-representation) |
| Document ref change (re-fetch after package update) | re-parse on next track entry |

Every row is a unit/e2e test target (§16).

---

## 14. Testing requirements

| Layer | Coverage |
|---|---|
| Core (Rust) | parser grammar table (valid/invalid fixtures inline), normalization (BOM/CRLF/whitespace), limits, offset math, multi-timestamp expansion, word-tag reduction, metadata capture; `active_line` table incl. ties, zero, before-first, after-last; monotonicity property test; fuzz target `lyrics_parse` |
| Package | manifest parse/write round-trip with `lyrics` (canonical position, omission when empty), uniqueness error (double reference), verify includes per-track lyric files, unknown-field behavior unchanged for C-authored files |
| Server | v11 migration on a v10 fixture DB; ingest of a package with per-track lyrics (rows, `track_id`, ids stable across rescan); track detail field (present/absent/`lang`/order); release `assets[]` filtering; auth/CORS/404 unchanged; asset byte-serving of a lyric file (Range/ETag/304) |
| Web unit | controller state machine (fetch/parse/track-change/error), DTO typing, planner inclusion |
| Web e2e | synced highlight follows a seek; manual scroll suspends; resume-follow; plain rendering; empty state; malformed state; mobile layout; offline install → airplane → lyrics render |
| Visual | synced + active line, plain, long lines, empty, mobile — added to the existing baseline set |

---

## 15. Forward compatibility

- Unknown tag keys, unknown root manifest fields, unknown track fields
  (for old readers) keep their existing ignore rules — a v2 document
  class (e.g. word-timed) can arrive as *new content the v1 parser
  rejects with a typed error* or as new optional manifest fields,
  without breaking v1 readers.
- `active_line`'s signature is total and version-free; a v2 with end
  times would extend the model, not redefine selection.
- The API field is additive within `/api/v1` (add-never-remove rule).

## 16. Resolved design questions

| # | Question | Resolution (and why) |
|---|---|---|
| Q1 | Existing format or native representation? | **LRC profile** (§4) as file format; the parsed model is MusicPack's native representation (§3). LRC is sufficient for line timing, is already the de-facto ecosystem format (fixtures, importer), and a parallel native format would duplicate it. |
| Q2 | Word-level timing? | **No** (§3): no concrete MusicPack requirement; deterministic reduction instead (§4.5). |
| Q3 | Track linkage mechanism? | **Per-track additive `lyrics[]`** (§6): mirrors `waveform`/`representations`; alternatives rejected with reasons (§6.2). |
| Q4 | Manifest version bump? | **No** — the format's own §7 additive-optional rule; the reference C ignores/drops the field by its documented unknown-nested-field behavior (§6.5). |
| Q5 | New byte endpoint? | **No** — reuse `/api/v1/assets/{id}` (§7.3); a dedicated endpoint duplicates serving machinery for zero contract gain. |
| Q6 | Server parses/validates content? | **Never** (§7.3): bytes are opaque hash-verified assets; parsing is client-side through the core; malformed content is a typed UI state, not server state. |
| Q7 | Where does timing live? | **Rust core only**, exposed via wasm (§9): one implementation, native-client-ready (ADR 0006), UI never authoritative. |
| Q8 | Multiple documents / languages? | **Array per track, first-in-manifest-order default** (§5); preference matching deferred (§18). |
| Q9 | Synced vs unsynced as stored metadata? | **No** — derived by parsing (§3); a stored flag invites contradiction with content. |
| Q10 | Offline scope? | Track-linked lyrics in; root `lyrics[]` stays deferred (§11) — the consumer defines the scope. |
| Q11 | Author in R3? | **Unchanged** — per-track authoring requires the R4 Rust pipeline (the frozen sidecar drops the field) (§6.6). |
| Q12 | Timestamp representation? | **i64 milliseconds** end to end; floor conversion at the client boundary; no floats (§9). |
| Q13 | Untimed lines inside a synced document? | Attached to the **following** timestamped line's block (§4.3): content- and order-preserving, model stays uniformly timestamped. |

## 17. License

BSD-3-Clause (repository `LICENSE`). The LRC profile is specified from
public de-facto LRC behavior; no third-party code or text is embedded.
