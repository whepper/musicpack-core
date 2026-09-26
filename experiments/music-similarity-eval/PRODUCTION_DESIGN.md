# Production architecture: optional Music Similarity capability

**Status: DESIGN PROPOSAL. Nothing here is implemented. No production code was
modified to produce this document.**

This is the architecture design that follows the isolated spike in
`experiments/music-similarity-eval/`. It covers a capability that MusicPack does
not currently have. It is written to be implementable, and it deliberately does
not add capability beyond what the ten goals require.

## 0. Relationship to existing decisions

Three existing records constrain this design and are **not** superseded by it:

| Record | Bearing on this design |
| --- | --- |
| `docs/adr/0016-audio-analysis-replacement.md` | **Closest prior art.** Defines the ownership boundary (`core::audio` primitive → Author → Server → web), the two-identifier profile discipline, streaming/chunk-invariance requirements, and the prohibition on a general ML runtime in the *first* slice. This design is the "**Deferred model lane**" that §17 requires to be "a separate ADR". It is deliberately **not** the model-free descriptor of §5.1. |
| `docs/adr/0008-offline-model.md` | Offline availability = whole verified package in a platform-owned store. Fixity semantics, not embeddings, are what travels. |
| `docs/server-migration.md` (D-S1, D-S2, D-S5) + `docs/adr/0009-library-jobs-and-shutdown.md` | Server is native-only and consumes/indexes; it never decodes or transcodes. `JobKind` has exactly `Scan \| Verify`, one slot, no queue, no cancellation framework. |

**Two places where this proposal must supersede or extend ADR 0016 explicitly,
and therefore cannot be implemented as a quiet follow-up:**

1. **Storage location.** ADR 0016 §10.1 rejects "store descriptors in `.mpack`"
   and recommends a local/server index. This design puts an **opt-in**,
   **profile-scoped** document in the existing `analysis[]` array while keeping
   the server-local index authoritative for queries. See §2.7 — the divergence
   is narrow, and it is the only way to satisfy "model replacement without
   changing the `.mpack` conceptual contract" and native-client portability.
2. **Learned model.** ADR 0016 §7 rejects adding a model runtime to the core
   "without that decision". §9 and §10 below are that decision's evidence.

Activation therefore requires a new ADR (suggested: `0017-similarity-embeddings.md`)
that records §9's licensing resolution and the §2.7 storage supersession. This
document is its input, not its substitute.

## 1. Embedding identity

An embedding is only meaningful with everything that produced it. A bare vector
plus a dimension count is not a comparison unit — that is the single most
important invariant in this design.

**Two identifiers, following ADR 0016 §9.2:**

### 1.1 `profile_id` — stable, semantic, human-facing

```
musicpack-similarity/<family>@<variant>
```

Examples: `musicpack-similarity/discogs-effnet@multi`,
`musicpack-similarity/discogs-effnet@release`, `musicpack-similarity/clap@music`.

The `@variant` suffix is required because one model family can ship several
checkpoints with materially different retrieval behaviour. The experiment
measured this directly: `multi` (1280-D) and `release` (512-D) produced
different neighbours on the same corpus.

**The `profile_id` namespace is owned by MusicPack, not by the model vendor.**
`discogs-effnet` is a *family name inside our namespace*, not a bare vendor
string. This is what keeps the format from becoming EffNet-specific: swapping
vendors changes the string value, not the grammar, the fields, or the reader.

`profile_id` changes when the representation is no longer comparable. It is what
an API returns and a UI displays. It is **not** a user-facing product feature
name — the user never selects a profile; the product does.

### 1.2 `profile_fingerprint` — canonical hash of everything defining

SHA-256 over a canonical TLV serialization (the existing precedent in
`src/identity.rs:41-88`: 1-byte tag, 4-byte big-endian length, raw bytes). Tagged
fields, so adding a field is additive and ordering is fixed:

| Tag | Field | Why it is identity |
| --- | --- | --- |
| 1 | `profile_id` | the stable name |
| 2 | `model_family` | vendor/model family |
| 3 | `model_variant` | checkpoint variant within family |
| 4 | `model_sha256` | **exact weights**; a different checkpoint is a different profile |
| 5 | `preprocessing_version` | mel bands, FFT, hop, window, target rate, downmix rule |
| 6 | `patch_hop` | changes the embedding even for identical weights (measured: 0/45 identical hashes at hop 61 vs 62) |
| 7 | `pooling` | per-window mean, then track L2 |
| 8 | `normalization` | L2 |
| 9 | `metric` | `cosine` |
| 10 | `dimensions` | |
| 11 | `output_encoding` | e.g. `f16le`, `f32le` |
| 12 | `aggregation` | album rule, e.g. `mean-of-l2-unit-then-l2` |
| 13 | `runtime_id` + `runtime_version` | inference runtime participates in numerics |
| 14 | `numeric_policy` | scalar vs SIMD, no-contraction, accumulation order |

**Rule: if any tagged field changes, the fingerprint changes, and vectors from
the two fingerprints are never comparable, indexed together, or converted.**

`runtime_id` deserves emphasis. The spike proved byte-identical output across
two runs of `rten 0.26.0` on CPU, but the experiment's own findings record that
SIMD/threaded execution is a numeric-policy input. Including the runtime makes
that honest: a runtime upgrade that perturbs the low bits invalidates the cache
rather than silently degrading ranking.

**Not** in the fingerprint: the source audio hash (that is the *cache key*, not
the profile), the library path, the package fingerprint, the model download URL.
A dynamic dependency lockfile hash is explicitly not a profile name (ADR 0016
§9.2).

### 1.3 The `music-similarity/discogs-effnet@...` question

The user's proposed form `music-similarity/discogs-effnet@...` is adopted, with
one correction: it is the `profile_id`, and it is **one field of the
fingerprint, not the fingerprint**. A profile id cannot prevent comparing two
vectors that share a family name but differ in patch hop, output encoding, or
runtime. Only the fingerprint does that. Keeping both is what makes model
replacement (§8) mechanical instead of a data migration.

## 2. `.mpack` representation

### 2.1 Constraints that decide the shape

Four facts from the codebase are load-bearing:

1. **`analysis[]` already exists and is already forward-compatible.**
   `src/format/manifest/mod.rs:639-649`:
   ```rust
   pub struct Analysis {
       pub kind: String,          // required; "sonic" is the v1 type
       pub profile: Option<String>, // required only when kind == "sonic"
       pub asset: Asset,
   }
   ```
   Documented at `mod.rs:641-643`: *"`\"sonic\"` is the v1 type; **unknown types
   are forward-compatible and structurally validated only**"*. Parser
   (`parse.rs:325-347`) requires `kind` + `path` + valid `sha256`, requires
   `profile` only for `kind == "sonic"`, and bounds the array at
   `MAX_ANALYSIS = 32`. **This is the designed extension point, and it costs no
   manifest version bump** (growth rule: `musicpack-v1.md` §7 — new optional
   fields, existing fields never change meaning).

2. **Per-track nested fields are destroyed on rewrite.** Unknown fields nested
   inside a known object are ignored on read and *dropped on rewrite* — by the
   Rust writer and by the C reference alike
   (`docs/architecture.md:184-185`, `docs/musicpack-lyrics-v1.md` §6.5,
   `docs/architecture.md` lyrics precedent). A `track.embedding` field would
   therefore be **silently lost** by a rebuild through the legacy CLI.

3. **Root-level unknown arrays survive the Rust writer but not the C writer.**
   `write.rs:377-393` copies unknown root members in original order;
   `write.rs:19-21` records that the C writer drops them. A root
   `"embeddings": [...]` member is therefore *not* durable across the legacy
   toolchain.

4. **Assets are the only thing the verifier hashes.** Integrity is unkeyed
   SHA-256 per referenced asset (`src/validation/mod.rs:8-25`), plus
   `INDX`/`TAIL` in the container. There is no signature and no MAC anywhere —
   this design must not pretend otherwise.

Only `analysis[]` with a novel `kind` satisfies all four: it is already
forward-compatible, it is a **package-level** (not nested) reference so it
survives rewrite by both writers, it inherits the existing hash/containment/budget
machinery for free, and it needs no format change at all.

### 2.2 Placement: one package-level document, not per-track assets

```json
"analysis": [
  { "kind": "similarity", "profile": "musicpack-similarity/discogs-effnet@multi",
    "path": "analysis/similarity/discogs-effnet-multi.bin",
    "sha256": "<64 hex>" }
]
```

Rationale for **one** document per (package, profile) rather than one per track:

- **Budget and block count.** Per-track assets would add up to
  `MAX_TRACKS` entries against the 4096-asset package budget and one `DATA`
  block each (`src/format/mpak/mod.rs:82`). A 30-track album with two profiles
  would add 60 referenced assets and 60 `DATA` blocks. One document adds two.
- **Hash count.** One SHA-256 to verify instead of 512.
- **Writer registration sites.** Adding an asset *group* requires editing six
  places in lockstep (`referenced_paths`, `check_dup_paths`, `KNOWN_ROOT_FIELDS`/`build_tree`,
  `canonical_pack_order`, `validation::verify`, `collect_manifest_assets`, plus a
  `limits.rs` cap). Adding a new `analysis[].kind` requires **none** of them:
  the group already exists.
- **The silent failure mode.** Missing `canonical_pack_order` registration means
  a directory bundle verifies while a `.mpak` of the same package does not.
  Reusing the `analysis` group eliminates that class of bug entirely.

The cost is that a single track's vector is not independently addressable. That
is acceptable: the consumer that needs one vector (the server, per track) reads
the index; the consumer that needs portability (native client, offline) reads
the whole album document, which is small (§2.6).

### 2.3 Binary layout

The format must be parseable by a Swift/Kotlin client with a struct unpacker, so:
**big-endian** (network order, matching the MPAK header, `INDX`, `TAIL` and the
identity TLV), fixed-width fields, no compression, no varints, no length-prefixed
nesting beyond a small table.

```text
Header (64 bytes, fixed)
  0   4   magic            "MSIM"
  4   2   format_major     u16 = 1
  6   2   format_minor     u16 = 0
  8  32   profile_fingerprint   raw sha256
 40   2   dimensions      u16
 42   1   vector_encoding u8    1 = f16le, 2 = f32le
 43   1   flags           u8    bit0 = has_album_vector
 44   4   track_count     u32
 48   4   contributor_count u32  (album aggregate: how many tracks contributed)
 52   4   table_offset    u32   (byte offset of the index table)
 56   4   album_offset    u32   (byte offset of album vector; 0 if absent)
 60   4   reserved        u32   (zero)

Index table (track_count entries, 16 bytes each, ordered by (disc, track))
  0   2   disc            u16
  2   2   track           u16
  4   4   vector_offset   u32   (byte offset of this track's vector)
  8   4   vector_length   u32   (bytes; == dimensions * sizeof(encoding))
 12   2   status          u16   0 = ok, 1 = insufficient_audio, 2 = unsupported, 3 = failed
 14   2   reserved        u16

Vector block: dimensions * 2 bytes (f16le) or dimensions * 4 (f32le),
  C-order (element-major), little-endian element encoding regardless of the
  big-endian framing — the *header and table* are big-endian because they are
  parsed by hand, the *vectors* are little-endian because that is the numeric
  layout on every platform MusicPack targets. The encoding byte makes the
  distinction explicit rather than implicit.

Album vector (if flags bit0): same layout, aggregate of contributing tracks.
```

**Vector status is per track and explicit.** A track that produced no meaningful
result gets `status != 0` and **no vector bytes**. Per ADR 0016 §4.1: *"[n]ull/
insufficient results must not become fabricated zero vectors."* A zero vector is
maximally similar to nothing in particular under cosine and would poison a
nearest-neighbour query. `vector_length == 0` with `status != 0` is the
representation.

**Album aggregation** is a deterministic function of contributing track vectors,
in canonical manifest order (disc, track): mean of L2-normalized unit vectors,
then L2-normalize. Only `status == 0` tracks contribute. `contributor_count`
records how many did; no aggregate is emitted when the count is zero. A changed
contributor set invalidates the aggregate.

**Note on `.mpak` container placement.** No new block type is needed. The
document is an ordinary `DATA` member reached through the existing
`analysis[]` reference. Adding a private-namespace block (`mod.rs:128-133`
reserves any 4-byte code with a non-`A`–`Z` byte, skipped by conforming readers
without inspection) is the *later* option if per-track random access is ever
worth it; it is rejected now because it buys nothing the document does not
already provide and it would need the `TAIL` placement constraint
(bytes before `TAIL` are covered by the package digest; bytes after it break
`tail_total_size`).

### 2.4 Preprocessing must be explicit, because it *is* the profile

The spike established the exact chain that must be frozen into
`preprocessing_version`:

```
decode via musicpack-core::audio (WAV/FLAC/Musepack SV8)
  → arithmetic channel mean in f64 → f32        (mono downmix)
  → resample to 16 kHz                            (rubato 0.16.2, oversample 2)
  → 96-band Slaney mel, unit_tri area-normalised, power spectrum
  → log10(max(1e-30, mel * 10000 + 1))
  → 128-frame patches, patch hop 61
  → model (rten 0.26.0, ONNX)
  → per-window L2 mean → track L2
```

Every one of those is a profile-defining field, not an implementation detail.
`patch_hop` alone changed **0 of 45** embedding hashes between hop 61 and 62
while keeping cosine ≥ 0.9991 — a reminder that "nearly identical" is not
"identical", and that a cache keyed without patch hop would be wrong.

### 2.5 Integrity

Nothing new is required, and nothing stronger is claimed:

- The document is a referenced asset, so `src/validation::verify` already applies
  containment/type checks, the 8 GiB per-file and 64 GiB aggregate budgets, and
  **SHA-256 against the manifest declaration**. A corrupt or truncated embedding
  document fails package verification exactly like a corrupt waveform.
- In a `.mpak`, the member's SHA-256 appears in both `INDX` and the manifest, and
  the container writer verifies the declared digest while copying — a mismatch
  aborts the pack.
- The **profile fingerprint is not integrity-protected by the format** beyond
  being inside the hashed document. A tamperer who rewrites the document can
  also rewrite its `sha256`. This is fixity, not authenticity, consistent with
  the rest of MusicPack.

### 2.6 Storage cost

| Set | dims | encoding | per track | 10-track album | 5,000-track library |
| --- | --- | --- | --- | --- | --- |
| discogs-effnet@multi | 1280 | f16le | 2,560 B | ~26.7 kB | 12.8 MB |
| discogs-effnet@multi | 1280 | f32le | 5,120 B | ~52.5 kB | 25.6 MB |
| clap@music | 512 | f16le | 1,024 B | ~10.8 kB | 5.1 MB |

Against the historical Sonic document (~27.8 kB base64 f32 for 10 tracks at
512-D), `multi` at f16 is comparable and at f32 is ~1.9×. **f16 is the
recommended default**, with the caveat that any quantization must be shown to
preserve useful ranking (ADR 0016 §9.1) before it is adopted — that measurement
has not been made and is a gate in §10.

Package size impact is the reason embeddings are opt-in. A 45-track, 15-album
library grows by ~0.4–0.9 MB with one f16 profile across all packages: about
1.1% of the audio it describes. Acceptable, and still a user-visible decision,
so it stays a flag.

### 2.7 Divergence from ADR 0016, stated plainly

ADR 0016 §10.1 rejected `.mpack` storage because *adding analysis to a manifest
changes `package_fingerprint`, so analysis must not depend on a post-analysis
package fingerprint* (§9.3). That reasoning holds and is preserved:

- **The server-local index is authoritative for queries.** The package document
  is an *input* to index construction, not the query path. Server rows are keyed
  by `profile_fingerprint`; a package whose document disagrees with the index
  triggers an index row update, not a trust decision.
- **Nothing in the similarity subsystem consumes `package_fingerprint` as a
  cache key.** Cache key is `(source_audio_sha256, profile_fingerprint)`.
- **The document is opt-in per package.** A package built without it is valid and
  simply has no similarity data — the same status as every pre-existing
  `.mpack` (goal 6).

The narrowed divergence: because the document is opt-in and profile-scoped, and
because `analysis[]` is an existing forward-compatible field, the "package
correctness depends on a profile" coupling ADR 0016 warned about does **not**
arise. Package *identity* does change when the document is added — that is
unavoidable for any in-package representation, and is precisely why the index
must not be keyed on it. **This supersession needs explicit ADR sign-off.**

## 3. Author responsibilities

### 3.1 When analysis happens

**At build time, as an opt-in stage, reading the staged encoded audio.**

Two precedents exist and both matter:

- **Loudness** is measured *inside* the core builder from the **staged** bytes
  (`src/authoring/build.rs:705-807` calls `measure(&staging, …)`), via a mode
  flag `LoudnessMode::{Measure, Omit}`, and is **fail-closed** for measurement
  but **optional for the field** (`--no-loudness` leaves it `None` and the
  manifest simply omits the key).
- **Waveform** is a **split stage** (`waveform_stage_with`, `pipeline.rs:616-700`)
  that the Author UI drives for cancellable per-track progress, writing into a
  host-owned staging directory.

Similarity should follow the **waveform** shape, not the loudness shape, because
a learned model is far slower than a loudness meter and must be cancellable:

- new `BuildPhase::Similarity`, added to `BuildPhase::ALL` so progress fractions
  remain correct (`pipeline.rs:139-146` derives step/steps from `ALL`);
- new `PipelineOptions.similarity: SimilarityMode { Analyze { profile, model_path }, Skip }`,
  defaulting to `Skip` — **absence of a model must never fail a build**;
- a `similarity_stage_with(...)` split-stage entry point mirroring
  `waveform_stage_with`, with the same per-track callback contract: the callback
  returns `false` to cancel (`pipeline.rs:544-546` → `AuthorError::Cancelled`),
  and the Tauri layer's `stage_cancel: Arc<AtomicBool>` works unchanged;
- reads the **staged encoded audio** so the embedding describes the bytes the
  package actually ships.

Reading staged bytes also makes the cross-codec result in the spike directly
useful: FLAC and Musepack SV8 produced cosine ≈ 0.99996–0.99999 on the sampled
pairs, i.e. **which codec a track ships as does not meaningfully change its
embedding**. The profile does not need a per-codec variant.

### 3.2 Mandatory or optional

**Optional, always.** `SimilarityMode::Skip` is the default. A build with no
model installed, no model selected, or a failed model load must produce a valid
package with `analysis[]` containing no `similarity` entry — identical to goal 6
(older packages without embeddings). The builder never fabricates a result, and
never writes a zero vector for a track that produced none (§2.3).

**Fail-closed only for a result that was claimed.** If a track is reported as
`status != 0`, the package is still valid — that is a recorded, honest outcome
(`insufficient_audio` for a 4-second track is a fact, not a failure). What *is*
fail-closed: if the builder declares a `similarity` asset, its SHA-256 must
match the written bytes. The existing `verify_staged` + atomic `rename`
(`build.rs:276-305`) already gives this.

### 3.3 Determinism requirements

Inherited from the spike's verified result (two full runs, byte-identical
`clap-pairs.csv` and all 29 per-track embedding digests) and from ADR 0016 §9.1:

- **Similarity vectors are not required to be bit-exact across CPUs**, because
  ranking does not require it — but the same `(profile_fingerprint, source)` pair
  must be stable within a build, and cross-platform drift must stay inside a
  documented, tested envelope.
- Chunk-size invariance: splitting the decoded stream differently must not change
  the result. The profile is window/hop-defined, so this is a testable property,
  not an aspiration.
- No FMA/contraction, no fast-math, no reassociation — the same numeric policy the
  frozen encoder corpus depends on. A learned model must not be allowed to
  silently change FP evaluation order.
- Non-finite inputs are sanitized at the analysis boundary following the existing
  `src/audio/mod.rs` precedent. Non-finite **outputs** are rejected, not clamped.
- The document must contain no timestamps, no library paths, no ordering that
  depends on filesystem enumeration. The spike's `refuseness` to expose
  metadata is a design property worth keeping: a package carries a vector, not a
  history of where it came from.

### 3.4 Model installation and distribution

**This must not resurrect the Sonic download path.** `author/src-tauri/src/sonic_model.rs`
is a complete, well-tested, pinned-URL acquisition system (SHA-256 + size +
atomic rename + quarantine + cancel, 9 unit tests) that is *dead code under the
default Rust backend*. It is a good **template** and a bad **precedent**: it is
coupled to the retired backend, and its model is CC BY 4.0 OpenL3.

The spike deliberately never downloaded a model: `--model` and `--model-sha256`
are caller-supplied and verified before load, and the command never fetches
anything. **Keep that property.** Requirements:

- The model artifact is **supplied by the operator**, out of band, with an
  expected SHA-256 verified before the file is opened.
- **No automatic download on package open, on `analysis[].profile` presence, or
  on any manifest field.** A package is untrusted input; a `profile` string must
  never select a model, trigger a fetch, load a plugin, or choose an executable
  (ADR 0016 §13).
- If a download UX is ever approved, it is a **new, separate** decision that
  reuses the *shape* of `sonic_model.rs` with a new profile, new artifact, and a
  completed §9 licensing review — and it must not inherit the old runtime or
  its panel.
- The model is **not redistributed** by this repository. No weights, no
  `models/` directory, no default model.

### 3.5 Caching

- Cache lives **outside the package staging tree** and outside the package
  itself, in a host-owned directory: `app_data_dir()/similarity/cache/`. The
  filesystem adapter belongs to the Tauri host, not to `musicpack-author` — the
  existing convention is `app_data_dir()` with no env var (`lib.rs:1179-1187`).
- Key: `(source_audio_sha256, profile_fingerprint)`. Never
  `package_fingerprint` (ADR 0016 §9.3).
- Value: the per-track vector record **and** the input facts needed to detect a
  stale hit (codec, sample rate, channels, duration).
- Written **atomically** (temp + rename), only after a complete successful
  result. A cancelled or failed run leaves no partial row that looks complete.
- Bounded, and removable without touching source audio or package bytes. A cache
  miss is a normal result, not an error.
- **Deletion policy:** removing a source must offer to remove its cache rows.
  Silently retaining a derived vector after the user deleted the audio is a
  privacy defect (ADR 0016 §13).

### 3.6 Re-analysis when the model changes

Because `model_sha256` and `patch_hop` are fingerprint fields, a model change
produces a **new** fingerprint, which means a **new** cache namespace and a
**new** index partition. It never invalidates or mutates the old one. Re-analysis
is therefore a normal first-run cost against a cold cache, and old results are
retained (marked inactive) for audit and rollback. Queries never mix partitions.

## 4. Server responsibilities

### 4.1 Ingestion

**The server performs no inference.** It reads the `analysis[]` document that
Author already produced, and indexes the vectors.

The spike's evidence makes this split cheap rather than costly: because
FLAC and Musepack embeddings are near-identical, and because embeddings are
deterministic per `(audio, profile)`, the server can treat a package document as
**authoritative input** rather than something to recompute or re-verify
semantically. That matches the existing boundary — `store/sqlite.rs` already
"verifies but never indexes" analysis documents, and `resolve_asset` filters to
`kind IN ('artwork','booklet','lyrics')` so analysis is never HTTP-servable.

Ingestion change: in `replace_release_content`, when a `similarity` analysis
entry is present, parse the 64-byte header, and upsert vector rows for the
`status == 0` tracks. Keep the album aggregate in a separate nullable column or
row. All within the existing per-package transaction
(`store.begin()` … `store.commit()`), so a malformed document rolls back with the
package.

**A malformed or unreadable document is a finding, not a crash.** The server has
no PCM path and must gain no model runtime. A document that fails to parse
records `verify_status` as a warning and yields no similarity rows; the package
remains valid and playable.

### 4.2 Index creation and persistence

Schema **v12** — the first genuinely free slot, since migrations 1–10 are
byte-frozen C DDL (compared byte-for-byte against a C-created database by
`tests/db_compat.rs`) and v11 is the first Rust-defined one.

```sql
-- 11 -> 12
CREATE TABLE similarity_sets (
  id INTEGER PRIMARY KEY,
  profile_id TEXT NOT NULL,
  profile_fingerprint TEXT NOT NULL,
  producer_version TEXT,
  state TEXT NOT NULL DEFAULT 'active',   -- active | inactive
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE UNIQUE INDEX similarity_sets_fp_idx ON similarity_sets(profile_fingerprint);

CREATE TABLE similarity_vectors (
  id INTEGER PRIMARY KEY,
  set_id INTEGER NOT NULL REFERENCES similarity_sets(id) ON DELETE CASCADE,
  track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
  dimensions INTEGER NOT NULL,
  encoding INTEGER NOT NULL,               -- 1 = f16le, 2 = f32le
  vector BLOB NOT NULL
);
CREATE UNIQUE INDEX similarity_vectors_idx ON similarity_vectors(set_id, track_id);

-- album aggregates, one row per (set, release)
CREATE TABLE similarity_album_vectors (
  set_id INTEGER NOT NULL REFERENCES similarity_sets(id) ON DELETE CASCADE,
  release_id INTEGER NOT NULL REFERENCES releases(id) ON DELETE CASCADE,
  dimensions INTEGER NOT NULL,
  encoding INTEGER NOT NULL,
  contributor_count INTEGER NOT NULL,
  vector BLOB NOT NULL,
  PRIMARY KEY (set_id, release_id)
);
```

Deliberate properties:

- **Separate from `assets`.** Analysis is not a servable artwork/lyrics object
  and must not become one. This preserves `resolve_asset`'s existing filter and
  the existing track/release JSON byte-prefix invariant that
  `tests/api_oracle.rs` enforces.
- **`profile_fingerprint` is the partition key**, so different models coexist in
  one database and can never be compared.
- **`ON DELETE CASCADE` + the existing package sweep** give incremental updates
  for free: delete the package → its releases lose rows → vectors cascade. No
  reindex job needed for removal.
- **Row ids stay stable** (the existing `uid` pattern) so public ids remain
  durable across rescans.
- Vectors are `BLOB`, not JSON and not a custom SQLite extension. No new
  dependency, and no `rusqlite` type escapes the `Store` trait — the seam
  boundary (`#![forbid(unsafe_code)]`, no `rusqlite` in any trait signature)
  is preserved.

### 4.3 Incremental updates

- On package content sync, upsert/delete vector rows for that release inside the
  existing transaction. Natural keys: `(set_id, track_id)`, matched on
  `track_id` as the `replace_release_content` graph already does.
- On a new profile fingerprint appearing, create a new `similarity_sets` row
  (`state = 'inactive'`) and populate it; it is not queryable until explicitly
  activated. A partial population must never serve.
- No reindex *job* is required for correctness: a set is derived data and can be
  rebuilt by rescanning. Keep `JobKind` at `Scan | Verify` — ADR 0009
  deliberately has one slot, no queue, no job ids, and no cancellation framework.
  **Do not silently generalize it.**
- If a rebuild becomes too slow to be interactive, that is the moment to add an
  explicitly specified maintenance pass — a **new** `JobKind` with its own
  identity, progress, cancellation, retry/resume and shutdown contract, decided
  in its own change. Not a field added to the existing enum.

### 4.4 Querying

Exact cosine scan, computed in Rust over `vector BLOB` rows for one
`(profile_fingerprint)` partition.

**Justification for not choosing an ANN library yet — measured, not assumed:**

| Library size | Vectors read per query (f16) | Bytes scanned |
| --- | --- | --- |
| 5,000 tracks | 5,000 | 12.8 MB |
| 20,000 tracks | 20,000 | 51.2 MB |
| 100,000 tracks | 100,000 | 256 MB |

A 5,000-track self-hosted library is a ~13 MB sequential BLOB read and an
arithmetic pass — single-digit milliseconds on the target hardware, inside the
server's existing blocking, thread-per-connection model. Introducing hnswlib,
faiss, or a vector database would add a native/C dependency (banned outside the
`rusqlite` carve-out), a build-script or FFI surface, WASM pressure, and a
persistence/migration burden — to solve a problem that is not measured to exist
here.

**Revisit when a real library exceeds ~50,000 vectors** or when measured p99
latency breaches the target. The partition-by-fingerprint schema means an ANN
index can be added per set later without a data migration. That is the point of
designing the partition key now.

Cosine on stored vectors assumes they are L2-normalized at write time (the
profile guarantees it), so similarity is a dot product and the query needs no
denominator. Implementations should still assert the invariant at write.

### 4.5 Behaviour when packages lack embeddings

This is the **normal** case, not an error — it is what every pre-existing
`.mpack` looks like (goal 6).

- No `similarity` analysis entry → no `similarity_sets` row, no vectors. The
  package ingests, verifies, serves, and plays exactly as today.
- A track with `status != 0` → no row. The track is silently absent from
  similarity results rather than represented by a zero vector.
- A release with fewer than N contributing tracks → its album aggregate is either
  absent or marked low-confidence; **do not fabricate one**.
- Query semantics when the set is empty or the seed track has no vector:
  return **404-equivalent capability absence** with a stable code
  (`similarity_unavailable`), not an empty result masquerading as "nothing is
  similar". The client already has an `ApiErrorCode` union and a
  `friendlyMessage` mapping to extend (`web/app/src/lib/api/errors.ts`).
- **Never fall back to a different profile** to fill a gap. A poor or ambiguous
  result is a reason to disable the feature, not to silently substitute a model
  (ADR 0016 §5.1).

## 5. API sketch

Result-oriented, additive, and non-breaking. **Nothing here is implemented.**
Two hard constraints from the existing server:

- **D-S3**: *"no endpoint requires client changes"* for the existing contract, and
  every pre-existing response must stay a **byte-prefix** — new fields are
  omitted entirely when empty (`routes.rs:770-776`).
- `tests/api_oracle.rs` byte-compares responses against the C reference, so any
  change to an existing response shape is a test failure by construction.

```http
# Capability + active profile. Lets the client show/hide the surface honestly.
GET /api/v1/similarity/status
→ { "available": true,
    "profileId": "musicpack-similarity/discogs-effnet@multi",
    "profileFingerprint": "…64 hex…",
    "dimensions": 1280, "metric": "cosine", "encoding": "f16le",
    "vectorCount": 4820, "albumVectorCount": 471,
    "indexedAt": "2026-09-26 11:04:00" }

# Similar tracks.
GET /api/v1/tracks/{id}/similar?limit=20&excludeRelease=1
→ { "profileId": "…", "count": 12,
    "neighbors": [ { "track": { …existing Track JSON… },
                     "release": { "id": 42, "title": "…", "albumId": 7 },
                     "score": 0.8123, "rank": 1 } ] }

# Similar albums.
GET /api/v1/albums/{id}/similar?limit=12
→ { "profileId": "…", "count": 8,
    "neighbors": [ { "album": { …existing AlbumSummary JSON… },
                     "score": 0.7341, "rank": 1,
                     "contributorCount": 11 } ] }
```

Design notes:

- **`score` is a within-model cosine**, not a calibrated probability. It is
  documented as a distance in the profile's metric, and the response carries
  `profileId` so a client can never silently compare across profiles. The CLAP
  spike is the cautionary example: CLAP cosines spanned 0.936–0.996 and EffNet
  0.437–0.908, and a raw comparison of those numbers would be meaningless. Only
  within-model ranks and z-scores are comparable.
- **Existing entity JSON is embedded unchanged** — a neighbor's `track` object is
  byte-identical to what `GET /api/v1/tracks/{id}` returns, so the client reuses
  its types and its cache. No new track DTO.
- **No raw vectors in any response.** Not as base64, not as floats. Exposing them
  would lock in an unvalidated representation and invite cross-profile
  comparison (ADR 0016 §16).
- `VISIBLE` gating is inlined into the similarity queries exactly as
  `store/read.rs:17-19` does for every other read — a neighbor from an
  `unavailable`/`invalid`/`conflict` package must not be returned.
- Authentication and pagination clamps follow the existing conventions
  (`limit` clamp 1..=200, default 50).
- Adding routes to the flat `if path == …` chain in `http/routes.rs` behind the
  existing auth gate is the mechanical change; **the query grammar belongs on the
  `Store` seam, not in HTTP**, per `store/mod.rs:20-27`.

## 6. Player (Web)

The Player knows nothing about the embedding model. It consumes a
`{track, release, score, rank}` list.

- **API client:** add `ApiClient.similarityStatus()` and
  `ApiClient.similarTracks(id, opts)` next to the existing 13 methods in
  `web/app/src/lib/api/client.ts`, using the existing `raw`/`json<T>` wrapper so
  the 20 s timeout, `ApiError` mapping and `onUnauthorized` hook apply unchanged.
- **Types:** hand-written additions to `api/types.ts`, matching the existing
  convention. There is **no codegen and no shared types crate** — `grep` for
  `ts-rs|specta|typescript|codegen` across all `Cargo.toml` returns zero
  matches; the contract is pinned by `api_oracle.rs` instead. So the DTO is
  written twice, by hand, and the byte-prefix invariant is the safety net.
- **State:** a `state/similarity.ts` store alongside `state/library.ts`, with
  the existing `Map`-cache pattern, plus an explicit
  `idle | loading | ready | unavailable | error` state. ADR 0016 §17 Slice 5
  requires *"an explicit stale/missing-result state"*.
- **Route:** the track page already has a generic section mechanism
  (`AlbumPage.svelte:58-66` has a 7-entry `SECTIONS` array and
  `?section=` deep links that do not remount). A
  `ui/track/TrackSimilarSection.svelte` is the minimal-footprint home, next to
  the existing `TrackAnalysisSection`. Displaying `score` rounded, or a
  strength bar, is a UI decision; it must not be labelled "match" or "AI".
- **Offline:** similarity is **not** added to `plan.ts`. The offline planner is a
  pure function over `ReleaseDetail` and its own comment already states that
  "booklet/extras/analysis stay deferred (D3): no client feature consumes them".
  Offline pages get a `ReleaseDetail` snapshot with no similarity field, so the
  UI must render the `unavailable` state rather than break. Making similarity
  offline-capable would require a new `OfflineAssetKind` variant and a
  release-level vector surface — a separate product decision.
- **No client-side analysis.** ADR 0016 §11 and §16 are explicit; `web/README.md`
  already states "the browser never decodes audio to synthesize an envelope".
  The WASM decoder stays a playback concern.

## 7. Future playlists / radio (deliberately not in the format)

Similarity becomes a recommendation layer only as a **separate, server-side
graph entity** that *consumes* the index. Nothing about it appears in the
embedding document or the manifest.

```text
similarity vectors (index, per profile_fingerprint)
        │
        ├── k-nearest-neighbour edges        (ephemeral, per query)
        │
        └── similarity graph                 (persisted, derived, evictable)
                  │
                  ├── seed → expand with artist/genre/popularity shaping
                  └── radio session state     (seed, played set, next k)
```

Principles:

- **The embedding format stores vectors, not playlists.** A playlist is a
  time-ordered, user-shaped, preference-driven object. Encoding it in the format
  would put recommendation policy into a data interchange format, and would make
  playlists stale the moment the index is rebuilt.
- **Graph/recommendation is server state**, like `tokens`/`sessions`, not a
  package asset and not in `library.db`'s package projection. It is rebuildable
  and evictable.
- **Every derived edge is a join of a vector edge and shaping inputs** already
  present: the artist graph (`group_artists`, `track_artists`), genres
  (`release_groups.genres_json`), and playback history. The dependency direction
  is index → graph, never graph → format.
- **A playlist is reproducible from its seed plus a policy version**, not stored
  as a frozen track list. That keeps "generated radio" honest: the same seed
  under a newer policy may differ, and that is expected.
- **Profile isolation applies here too.** A radio session is bound to one
  `profile_fingerprint`; switching models invalidates the session, because a
  graph built over one model is meaningless over another.

## 8. Model replacement

This is the property the whole design exists to make cheap. Trace one event:
EffNet is replaced (or a licence resolution makes it unavailable) and a new
profile is activated.

| # | What happens | Why it is cheap |
| --- | --- | --- |
| 1 | New `profile_id` + `profile_fingerprint` (model sha256, variant, possibly preprocessing) are defined. | §1.2: identity is a tagged TLV hash, not a code path. |
| 2 | `musicpack-similarity/*` grammar, document layout, and API shape are **unchanged**. | Nothing in §1–§6 names a model family. `discogs-effnet` is one value in a namespace we own. |
| 3 | `.mpack` v1 is unchanged. No new manifest field, no version bump. `analysis[].kind == "similarity"` already parses; the `profile` string is data. | §2.1: the extension point is pre-existing and already forward-compatible. |
| 4 | A new build with the new model writes a **second** `analysis[]` entry. | `analysis` is an array bounded at 32 — multi-profile packages are representable today. |
| 5 | Author cache gets a **new namespace** keyed by the new fingerprint. Old entries are retained, marked inactive. | §3.5/§3.6: cache key contains the fingerprint. |
| 6 | Server creates a **new `similarity_sets` row** (`state = 'inactive'`) and populates a new partition. | §4.2: partition key is the fingerprint. |
| 7 | Queries serve the activated set only. The old partition is retained for audit/rollback. | §4.5: queries never mix partitions. |
| 8 | Activation flips `state`. No data migration, no cross-profile conversion, no re-verification of packages. | Conversions are explicitly *not* supported (ADR 0016 §5.2). |

**What does not happen:** no format revision, no API breaking change, no client
change beyond a new `profileId` string, no per-track re-index of the old set, no
attempt to translate vectors between models, no silent fallback to the old
profile.

**Cost:** storage for two partitions (a 5,000-track f16 library = 12.8 MB per
set — cheap), and a full re-analysis pass on the server side (or, more likely, a
rescan of already-authored packages, which is where the vectors come from
anyway).

**Hard rule:** vectors are never converted between profiles, and no code path
may compare vectors from different fingerprints. A dimension collision is not
evidence of comparability.

## 9. Licensing

Separated as requested. **This is the blocking section: §9.4 is unresolved.**

### 9.1 MusicPack's own code

- Core/permissive crates: **BSD-3-Clause** (root `LICENSE`).
- The embedding document format, the `profile_id` namespace, the profile
  fingerprint definition, and the binary layout in §2.3 are MusicPack's own
  design and carry no third-party terms. They are implementable in a
  BSD-3-Clause crate.
- The design deliberately contains **no source-derived code** from the C
  reference and never moves anything into the LGPL-2.1 encoder/tools boundary
  (`crates/musicpack-musepack-encoder/`, `crates/musicpack-mpc-tools/`).
- Similarity code goes in a **permissive crate**. It must never absorb
  AGPL/GPL or non-commercial terms.

### 9.2 Runtime

- **`rten` 0.26.0 — MIT OR Apache-2.0.** Permissive; no issue on its own.
- **`rubato` 0.16.2, `microfft` 0.6.0 — MIT OR Apache-2.0 / MIT.** Permissive.
- Runtime is *not* the licensing problem. Note the **MSRV exception**: the
  experiment ran at MSRV 1.94 (observed rustc 1.97.1) while the workspace is
  1.85. Adding `rten` to a production crate requires an explicit MSRV decision
  (ADR 0016 §7 flags this), not a licence fix.
- Bundled SQLite in the server stays inside `rusqlite` behind the `Store` trait
  (D-S2/O-S1). That carve-out **does not extend** to an ONNX runtime
  (`0016` §11: *"The server's existing bundled SQLite exception does not extend
  to an ONNX/audio runtime"*). A similarity runtime belongs on the Author side,
  which is why §4 keeps inference off the server.

### 9.3 Model weights

| Model | Weights | Library | Usable as mandatory dependency? |
| --- | --- | --- | --- |
| Discogs-EffNet `multi` | **CC BY-NC-SA 4.0** (documented) | Essentia **AGPL/commercial** | **No.** Non-commercial terms are incompatible with a mandatory open-product dependency. |
| Discogs-EffNet `release` | **CC BY-NC-SA 4.0** | same | No. |
| CLAP `larger_clap_music` | **Apache-2.0** (verified from repo + HF metadata this session) | torch/transformers (permissive) | Licence-clean, but **rejected on measured quality** in the historical report, and its compressed dynamic range (15/20 cosines in 0.962–0.996) makes single-case ranking fragile. |
| OpenL3 | CC BY 4.0 | MIT | Permissive; historically materially weaker quality. |

**One genuinely good piece of news, verified this session:** the spike loaded both
EffNet ONNX artifacts with `rten` and **no Essentia at all**. The AGPL exposure
lives in the Essentia *library*, not the ONNX graph. So the model artefact is
practically separable from the AGPL library — which narrows the problem to §9.4
and §9.5 rather than a library-level blocker. This should be stated plainly in
the ADR; it changes the shape of the decision.

**Essentia AGPL is not triggered by loading a `.onnx` file with a pure-Rust
runtime.** That should still be confirmed against Essentia's own terms by
qualified counsel before shipping, not assumed from this observation.

### 9.4 Training data / dataset provenance — **UNRESOLVED, BLOCKING**

CC BY-NC-SA 4.0 covers the *weights*; it does not by itself answer:

1. **Was the weight training set's licence compatible with the share-alike and
   non-commercial terms of the artefact?** The Discogs research dataset's own
   terms have not been audited here. The experiment's `FINDINGS.md` §4 notes the
   index "exposes `.onnx` files" but that *"will not assume that their presence
   grants redistribution rights"* — correct, and still open.
2. **What does non-commercial mean for MusicPack?** MusicPack is
   self-hosted/open-source today. "Non-commercial" is undefined for
   personal/non-commercial self-hosting, and undefined for a future hosted or
   commercial offering. **This is the single blocking question.**

Consequences to consider explicitly:

- Bundling weights in a release: **prohibited** (no redistribution under
  NC + share-alike without compliance).
- Requiring the operator to supply their own copy: still NC, so it constrains
  *use*, not only distribution.
- Shipping MusicPack under terms incompatible with NC: the product's own
  licence is implicated.

### 9.5 Redistribution of generated embeddings — **UNRESOLVED, SECOND BLOCKER**

A distinct question that is easy to miss: if the artefact is NC + share-alike,
are **vectors computed from a user's own audio** derivative works carrying those
terms?

Arguments that they are not: embeddings are not the weights; they are numeric
statistics derived from the *user's* audio, not from the model; no part of the
weights is reproduced.

Arguments that they might be: NC + ShareAlike can be argued to attach to
adaptations/derivatives, and this has no clear precedent for ML-derived
artefacts.

This directly affects whether the **portable `.mpack` document (§2) is
distributable at all** — a `.mpack` with an `analysis[].similarity` entry could
be shared, uploaded, or sold. A server-local index (§4) that never leaves the
machine is materially safer. **This is a licence question about the artefact
format, and it is a strong practical argument for making the package document
opt-in — which §2.2 already is.**

### 9.6 What must be resolved before production adoption

| # | Item | Status |
| --- | --- | --- |
| 1 | Meaning of "non-commercial" for MusicPack's licensing and use (self-hosted, personal vs commercial/hosted) | **Blocking** |
| 2 | Whether generated embeddings inherit NC/SA terms, and therefore whether an embedding-bearing `.mpack` may be redistributed | **Blocking** |
| 3 | Discogs research dataset terms vs. the artefact's NC/SA terms | **Blocking** |
| 4 | Counsel confirmation that ONNX-graph use via a pure-Rust runtime does not trigger Essentia AGPL | Needed before shipping |
| 5 | MSRV exception for `rten` (1.94 vs workspace 1.85) | Needed before merging any dependency |
| 6 | f16 quantization preserves useful ranking (measured, not assumed) | Needed before adopting f16 |
| 7 | Human listening review of the 20 stratified cases | Needed before any user-facing quality claim |
| 8 | Runtime/WASM policy if the analyzer is ever placed in `musicpack-core` | Deferred; recommend it is **not** |

**Until 1–3 are answered, Discogs-EffNet cannot be a mandatory dependency, and
the capability cannot be a documented MusicPack feature.** It may exist as an
opt-in, operator-supplied, explicitly experimental capability — which is exactly
the posture the experiment took.

## 10. Open questions

The smallest set that must be decided before implementation begins. Everything
else is deferrable.

**Blocking (no implementation until answered):**

1. **Licensing.** §9.4 items 1–3: is Discogs-EffNet's CC BY-NC-SA usable in
   MusicPack at all, and may a `.mpack` carrying embeddings be redistributed?
   *If the answer is no*, the capability ships with no default model and
   operator-supplied weights only, or waits for a permissively licensed
   candidate.
2. **Product decision (ADR 0016 Slice 0).** Who asks for similar tracks, on what
   collection, and what counts as success with **no human ground truth yet**?
   The spike proved the pipeline works and that models disagree substantially
   (Spearman 0.365 on within-set ranks). It did not establish that any model is
   *good*.

**Required before merging code:**

3. **MSRV policy** for an ONNX runtime in a production crate (1.94 vs 1.85), and
   whether an ML runtime is permitted in Author at all under
   `AGENTS.md`'s "no subprocess-based production dependencies" rule.
4. **Supersession sign-off** for ADR 0016 §10.1 (storage location) and §7
   (learned model lane) — as a new ADR, not a silent edit.
5. **`.mpak` placement confirmation** that `analysis[]` members are packed by
   the existing `canonical_pack_order` with no new block type, verified by a
   round-trip test rather than assumed.
6. **Vector encoding**: f16 vs f32, decided by a measured ranking-equivalence
   test.
7. **Index size trigger** for revisiting ANN, expressed as a measured threshold
   rather than a library preference.

**Deliberately deferred, and safe to defer:** offline similarity, native-client
on-device analysis, cross-collection federation, auto-conversion between
profiles, a general job queue, and any playlist/recommendation persistence. Each
has a stated extension point above and none is required by the ten goals.

---

## 11. What this design does not claim

- It does not claim any model produces musically good neighbours. There is **no
  human ground truth**; the 20 stratified cases are awaiting review, and CLAP
  disagreed with EffNet substantially on them.
- It does not claim cross-codec stability is a product guarantee. The spike
  measured cosine ≈ 0.99996–0.99999 on five sampled pairs per configuration —
  encouraging, not a contract.
- It does not revive Sonic. The retired boundary stands: `sonic_analyze` still
  returns `sonic_retired`, historical `analysis[]` entries are still carried
  opaquely, and nothing here converts or compares legacy Sonic vectors.
- It does not claim the format is final. The document layout in §2.3 is a
  proposal; a fixture format and reference vectors (ADR 0016 Slice 1) should be
  reviewed before any writer exists.
