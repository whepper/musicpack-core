# ADR 0017: Similarity profile mechanism and optional package representation

- **Status:** Proposed (2026-09-26; design only, nothing implemented)
- **Decision type:** Architecture boundary, format extension, and licensing gate
- **Production impact:** None from this document. No dependency, schema, format,
  API, UI, or runtime behaviour is changed by this ADR. It supersedes and
  narrows parts of ADR 0016 as recorded in §11.
- **Related decisions:** ADR 0006 (native clients boundary), ADR 0008 (offline
  model), ADR 0009 (library jobs and shutdown), ADR 0010 (core package builder),
  ADR 0014 (server production cutover), ADR 0016 (audio analysis replacement)

## 1. Decision summary

1. **MusicPack gains a similarity *capability*, not a similarity *model*.** The
   profile mechanism, the package representation, the server index, and the query
   surface are defined independently of any particular embedding model. A model
   is a profile *value*, never an architectural dependency.
2. **Similarity data is derived analysis, not musical content.** It is a function
   of (audio bytes, model weights, preprocessing, runtime). It does not
   participate in `group_key` or `release_key`, which are computed from a closed
   set of stable metadata fields.
3. **`.mpack` may optionally carry a similarity document** as a
   `analysis[]` entry with a new `kind`, reusing the existing
   forward-compatible extension point. **The package document is portable input
   only and is never authoritative.** The server-local similarity index is
   authoritative for every query.
4. **Two identifiers, both mandatory.** A stable semantic `profile_id` and a
   `profile_fingerprint` that is a hash of every profile-defining field.
   Vectors from different fingerprints are never compared, indexed together, or
   converted.
5. **The server never performs inference.** It indexes documents produced by
   Author. The server acquires no model runtime.
6. **The model is operator-supplied, out of band, hash-verified, and never
   downloaded automatically.** A `profile` string in a manifest is untrusted
   data and never selects, fetches, or loads anything.
7. **Discogs-EffNet is not a supported or default profile.** The licensing gate
   in §10 is unresolved. The mechanism may ship; a model profile ships only if
   the gate passes.
8. **No approximate-nearest-neighbour index.** Exact cosine over a
   profile-partitioned table, revisited only on a measured threshold.
9. **Similarity quality is not claimed.** The representation is deterministic and
   stable under the tested conditions; perceptual relevance is unvalidated.
10. **Sonic stays retired.** Nothing here revives it, converts its vectors, or
    reuses its runtime.

## 2. Context

### 2.1 What the research established

An isolated, removable experiment (`experiments/music-similarity-eval/`,
excluded from the workspace) ran Discogs-EffNet locally through the pure-Rust
`rten` ONNX runtime over `musicpack-core::audio`, with no Essentia, no Python, and
no external service. Measured results, in the narrow scope of that experiment:

| Property | Result |
| --- | --- |
| Local inference | Works; `multi` (1280-D) and `release` (512-D) both load |
| Determinism | Two independent full runs produced byte-identical per-track embedding digests and an identical pair-similarity file |
| Patch-hop stability | Hop 61 vs 62: 0/45 identical hashes; cosine min/mean/max 0.999106 / 0.999867 / 0.999977; top-10 overlap mean 0.9844; top-1 changed 9/45; max common-neighbour rank displacement 2 |
| Cross-codec stability | FLAC vs Musepack SV8, 5 sampled pairs per configuration, zero skips: cosine means ≈ 0.99996 (`multi` hop 61), ≈ 0.99996 (`multi` hop 62), ≈ 0.999998 (`release` both hops) |
| Candidate quality | Usable nearest neighbours on a 45-track / 15-album corpus |
| Independent signal | A CLAP diagnostic produced substantially different rankings: Spearman rho 0.365 on within-set ranks over 20 pairs |

### 2.2 What the research did **not** establish

- **No human ground truth.** The 20-case stratified review set is generated and
  blind; ratings do not exist. No human review has been performed.
- **No perceptual validation.** A third model disagreeing is not evidence of
  correctness. The right statement is: *the representation is deterministic and
  stable under the tested conditions, and its perceptual relevance is
  insufficiently validated to be a product-quality guarantee.*
- **No scale validation.** Corpus was 45 tracks / 15 albums, one album per
  artist, so the same-artist/different-album stratum is structurally empty.
- **Cross-codec stability was sampled, not proven.** Five pairs per configuration
  is a signal, not a contract.
- **Licence terms are unresolved** (§10).

### 2.3 Why the mechanism is worth defining despite 2.2

The capability is architecturally separable from any quality claim. ADR 0016
already established the ownership boundary and the profile discipline for a
*model-free* descriptor and deferred a learned model to a separate ADR. This
document is that ADR for the **mechanism**. A mechanism that is licence-clean and
model-agnostic can be built and reviewed now; a quality claim cannot.

### 2.4 The three identity levels, and which one similarity touches

From `src/identity.rs:1-36` and `docs/package-builder.md:43-48`:

| Level | Computed from | Touched by similarity? |
| --- | --- | --- |
| `group_key` | MusicBrainz release-group id, else a hash of title, original date, release type, artists sorted by (name, role) | **No** |
| `release_key` | MusicBrainz release id, else a hash of edition, release date, country, label, catalogue number, barcode | **No** |
| `package_fingerprint` | SHA-256 of the entire canonical manifest serialization | **Yes** — any manifest change yields a new fingerprint |

This is the crux of §5.

## 3. Terminology

These terms are used precisely throughout and must not be conflated.

| Term | Definition |
| --- | --- |
| **Model implementation** | A concrete executable graph plus weights that maps audio to a vector. Example: the Discogs-EffNet `multi` ONNX artifact. |
| **Model identity** | The identifying facts of a model implementation: family, variant, and the SHA-256 of the exact weights. |
| **Profile** | The complete, versioned definition of how a vector is produced and compared: model identity, preprocessing, hop, pooling, normalization, metric, output encoding, aggregation, runtime, and numeric policy. |
| **`profile_id`** | A stable, semantic, human-facing name for a profile. Changes only when the representation is no longer comparable. Displayed by APIs. Never a cache key. |
| **`profile_fingerprint`** | SHA-256 over a canonical tagged encoding of every profile-defining field. The comparison and cache key. Two fingerprints that differ ⇒ vectors are incomparable. |
| **Embedding / vector** | One fixed-length numeric vector produced by a profile for one track, or a deterministic aggregate of such vectors for one album. Meaningless without a profile. |
| **Similarity document** | A package-level referenced asset (`analysis[]` entry with `kind = "similarity"`) containing, for one profile, per-track vectors with explicit per-track status and an optional album aggregate. Portable input. |
| **Similarity set** | A server-side row identifying one `profile_id` + `profile_fingerprint` partition of vectors, with lifecycle state. |
| **Similarity index** | The server-side queryable structure: similarity sets plus their vector rows. Derived, rebuildable, authoritative for queries. |
| **Package analysis** | A referenced document inside `.mpack`, covered by the normal asset integrity model. Present or absent; never required. |
| **Server-derived data** | Data the server computes or indexes from package content. Never part of package identity, never portable, deletable without touching audio. |
| **Package identity** | `package_fingerprint` — the canonical manifest hash. Distinct from release identity (`group_key`/`release_key`), which is stable metadata only. |

## 4. Decision

### D-1. The profile mechanism is model-agnostic; models are profile values

The format, the document layout, the server schema, and the API are defined in
terms of a profile. No component outside the Author-side analyzer names a
specific model family. `discogs-effnet` is one value in a MusicPack-owned
namespace.

This is what makes §8 (model replacement) mechanical and what prevents one
model's licence status from becoming a permanent architectural dependency.

### D-2. Similarity is derived analysis, never musical identity

`group_key` and `release_key` continue to be computed exactly as today, from the
closed field set in `src/identity.rs`. No similarity-derived value enters them.

The experiment is the evidence that this is the correct classification: changing
only the patch hop — with identical audio and identical weights — changed 0 of 45
embedding digests. A quantity that changes when a numeric parameter changes is a
derivation, not an identity.

### D-3. `.mpack` may optionally carry a similarity document, as portable input only

Adopted as in `PRODUCTION_DESIGN.md` §2, with semantics fixed in §5.3. Summary:
a package-level `analysis[]` entry with a new `kind`, one document per
(package, profile), per-track explicit status, optional album aggregate.

**Disabled by default.** Enabled only through an explicit per-build option, and
released only after licence gate G-2 (§10.4) resolves.

### D-4. Two identifiers, both mandatory

`profile_id` is a stable semantic string. `profile_fingerprint` is a hash of all
defining fields. `profile_id` alone is **insufficient** and must never be used as
a comparison or cache key: the experiment shows two runs of one family at
different hops share a family name and share zero byte-identical vectors.

### D-5. The server never performs inference

The server parses and indexes similarity documents. It acquires no model
runtime, no model weights, and no model download path. This preserves ADR 0016
§11 and the fact that the server's only C dependency is bundled SQLite behind the
`Store` trait; that carve-out **does not extend** to an ONNX runtime.

### D-6. The model is operator-supplied and never auto-fetched

Model acquisition is an explicit, out-of-band operator action with a
caller-supplied expected SHA-256 verified before the artefact is opened. A
`profile` string inside a manifest is untrusted input: it never selects a model,
triggers a fetch, loads a plugin, or chooses an executable.

### D-7. No ANN initially

Exact cosine over a profile-partitioned table. Rationale in §6; revisit threshold
in §14 item 7.

### D-8. Playlists and radio are server-side, outside the format

See §9.

## 5. Detailed design

### 5.1 Analysis of options A / B / C

ADR 0016 §10.1 recommended a local/server index and rejected storing descriptors
in `.mpack`. `PRODUCTION_DESIGN.md` proposed the package document while calling
the server index authoritative. The conflict is real and is resolved explicitly
here.

**Option A — similarity is package content.** Rejected.

A vector is a function of (audio, weights, preprocessing, runtime). Treating it as
content means a patch-hop or runtime change rewrites package identity for
musically identical audio, and a model upgrade becomes a repackaging operation.
It also makes package validity depend on a profile, which is the coupling
ADR 0016 §4.1 warned about when it recorded that "vectors from different
profiles must never be compared."

**Option B — server-derived only.** Rejected as the sole model, for two concrete
reasons.

1. It forfeits portability, which the design goals require for offline and future
   native clients, and re-derivation is the expensive part.
2. The stated reason for rejecting package storage in ADR 0016 §10.1 — that
   adding analysis changes `package_fingerprint` — **does not discriminate**.
   Loudness and waveform already do this. `LoudnessMode::Omit`
   (`src/authoring/build.rs:67-75`, exposed as `--no-loudness`) and
   `--no-waveform` each produce a different fingerprint from a measured build.
   Derived, optional, Author-computed, fingerprint-affecting manifest data is an
   already-accepted category in this format. Similarity is a third instance.

There is a further consequence of the fingerprint that is worth stating because it
surprises people: server ownership arbitration (`crates/musicpack-server/src/ingest.rs:463-490`)
treats same `group_key`/`release_key` with a **different** fingerprint against a
present owner as an identity `conflict`, which is quarantined. So a library
holding one copy built with similarity and one copy without already has that
behaviour today, for loudness and for waveform. Adding similarity does not
introduce it.

**Option C — optional portable input, server index authoritative. Adopted**, with
§5.3 semantics. It is the only option that satisfies the portability goals
without making derived data authoritative, and it reuses the extension point,
integrity model, and graph-sync path that already exist.

### 5.2 What is portable, what is authoritative

| | Package similarity document | Server similarity index |
| --- | --- | --- |
| Location | Inside `.mpack`, `analysis[]`, `kind = "similarity"` | Server database, `similarity_sets` / `similarity_vectors` |
| Status | Optional, never required | Derived, rebuildable |
| Authority | **None.** Input only | **Authoritative for every query** |
| Integrity | Normal asset model: containment, size budgets, SHA-256 against the manifest declaration | Rebuildable from a scan; not package content |
| Affects `package_fingerprint` | **Yes** (it is manifest content) | No |
| Affects `group_key` / `release_key` | No | No |
| Portable | Yes, if the licence gate permits | No — never leaves the machine |
| Consumed by | Server ingestion; portable for any compliant reader, but no client consumption is in scope (§13) | Queries only |
| Deletable without touching audio | Yes (the file) | Yes (rows) |

### 5.3 Precise semantics

**Portable:** a similarity document is portable in exactly the sense that it is
valid, hash-checked package content that a compliant reader may use as
*candidate input*. It carries a profile fingerprint, per-track vectors with
explicit status, and an optional album aggregate.

**Authoritative:** the server-local similarity index, always. A package document
never determines a query result on its own.

**Package-provided data versus the server's configured profile:**

- The server compares the document's `profile_fingerprint` against its active
  similarity set.
  - **Match** ⇒ the document's vectors are indexed into that set.
  - **Mismatch** ⇒ the document is *retained, ignored for queries, and
    reported*. It is not an error and does not invalidate the package. The server
  does **not** recompute it, because the server has no runtime (D-5).
- **Can the server reject it?** It can reject it for *query* purposes — it will
  not mix fingerprints — but it must **not** reject the *package*. Verdict
  grammar follows the existing `verify()` vocabulary: containment, path
  validity, size budgets, and SHA-256 are existing asset checks and produce
  existing findings. A document that parses but carries an unknown fingerprint
  is a **warning**, not an error. A document that fails its declared SHA-256 is
  an existing checksum-mismatch error, identical to a corrupt waveform.
- **Can it coexist with a locally generated vector?** **No, not for one track
  under one fingerprint.** Within a similarity set, one `(track, set)` pair has
  exactly one vector, enforced by a unique index. Two packages offering
  different vectors for the same track and profile is a conflict resolved by
  ownership arbitration: only the ownership winner's content graph is synced
  (`replace_release_content` runs only for the owner), so exactly one package
  contributes. The two *may* coexist as separate profiles — that is the
  multi-profile case, which `analysis[]`'s array shape already represents and
  `MAX_ANALYSIS = 32` already bounds.
- **Does it affect package identity?** **Yes** — it changes
  `package_fingerprint`, exactly as loudness and waveform do. It does **not**
  affect `group_key` or `release_key`, so release identity, ownership grouping,
  and mirror detection are unaffected. A repack that adds or removes similarity
  data is a new package fingerprint over the same release. Nothing in the
  similarity subsystem may use `package_fingerprint` as a cache key; the cache
  key is `(source_audio_sha256, profile_fingerprint)`, preserving ADR 0016 §9.3.

### 5.4 `profile_id` and `profile_fingerprint`

`profile_id` follows the existing repository convention for profile names
(`musicpack-sonic-openl3-v1`, `author/src-tauri/src/sonic_model.rs:34`):

```text
musicpack-similarity-discogs-effnet-multi-v1
musicpack-similarity-clap-music-v1
```

`PRODUCTION_DESIGN.md` §1.1 proposed a slash-and-`@` form
(`music-similarity/discogs-effnet@multi`). **This ADR adopts the repository's
hyphenated convention instead**, so that similarity profile ids are
indistinguishable in shape from the existing sonic profile ids and can be stored,
displayed, and compared with the same tooling. The variant and the schema
revision are separate segments rather than an `@` suffix.

`profile_fingerprint` is SHA-256 over a canonical tagged encoding using the
existing TLV precedent (`src/identity.rs:41-89`: 1-byte tag, 4-byte big-endian
length, raw bytes; absent fields emit nothing). Fields:

| Tag | Field |
| --- | --- |
| 1 | `profile_id` |
| 2 | model family |
| 3 | model variant |
| 4 | model weights SHA-256 |
| 5 | preprocessing version (mel bands, FFT, hop, window, target rate, downmix) |
| 6 | patch hop |
| 7 | pooling |
| 8 | normalization |
| 9 | metric |
| 10 | dimensions |
| 11 | output encoding |
| 12 | album aggregation rule |
| 13 | inference runtime identity and version |
| 14 | numeric policy (scalar vs SIMD, no contraction, accumulation order) |

Two of these deserve emphasis:

- **`patch_hop` (6)** — 0/45 byte-identical embeddings between hop 61 and 62 with
  identical weights. Omitting it from identity would make a cache serve vectors
  from a different profile.
- **`runtime` (13)** — the experiment produced byte-identical output across two
  runs of one `rten` version on CPU. A runtime upgrade is a numeric-policy input
  and must invalidate rather than silently degrade ranking.

Explicitly **not** in the fingerprint: source audio hash (that is the cache key),
library path, package fingerprint, model download URL, or a dependency lockfile
hash (ADR 0016 §9.2).

### 5.5 Document representation

Placed in the existing `analysis[]` array with a new `kind`. The parser
(`src/format/manifest/parse.rs:311-349`) already accepts unknown kinds, requiring
`type`, `path`, and a valid `sha256`, and requiring `profile` only when
`kind == "sonic"` — so a `similarity` entry parses today with no format change
and no manifest version bump, under the `musicpack-v1.md` §7 growth rule.

```json
"analysis": [
  { "kind": "similarity", "profile": "musicpack-similarity-discogs-effnet-multi-v1",
    "path": "analysis/similarity/discogs-effnet-multi-v1.bin",
    "sha256": "<64 lowercase hex>" }
]
```

Why package-level and **one document per (package, profile)**, rather than
per-track assets: adding a new asset *group* requires editing eight registration
sites in lockstep, and the silent failure is that a directory bundle verifies
while a `.mpak` of the same package does not (`canonical_pack_order`
registration). Reusing the `analysis` group requires none of them. Per-track
assets would also consume the 4096 `MAX_REFERENCED_ASSETS` budget and add one
`DATA` block per track, against `MAX_MEMBERS = 4096`.

Binary layout: fixed 64-byte big-endian header (magic `MSIM`, format major/minor,
32-byte profile fingerprint, dimensions, vector encoding, flags, track and
contributor counts, table and album offsets), a 16-byte-per-track index table
sorted by (disc, track) carrying vector offset, length, and an explicit per-track
`status`, then the vector blocks. Big-endian framing so a Swift or Kotlin client
can unpack the header and table by hand. No compression, no varints.

**This layout is a proposal, not a settled contract.** The decision (D-3) requires
only a package-level `analysis[]` entry, one document per (package, profile),
explicit per-track status, and an optional album aggregate; the specific widths,
magic, and field order above are one way to satisfy that and are subject to the
fixture and reference-vector gate in §14 item 5a.

**Per-track status is explicit, and absent means absent, never zero.** A track
that produced no meaningful result records `status != 0` and carries **no vector
bytes**. Per ADR 0016 §4.1, "null/insufficient results must not become fabricated
zero vectors": a zero vector is not a low-similarity track, it is a maximally
uninformative point that would distort a nearest-neighbour query.

Album aggregate, when present: mean of L2-normalized unit vectors over
contributing tracks in canonical manifest order, then L2-normalized, with the
contributor count recorded. No aggregate when the count is zero. A changed
contributor set invalidates it.

### 5.6 Author responsibilities

**Opt-in, cancellable, and never required to build.** A build with no model, no
selected profile, or a failed model load produces a valid package with no
`similarity` entry — which is also the correct representation of every
pre-existing `.mpack`.

Shape follows the existing waveform split-stage
(`crates/musicpack-author/src/pipeline.rs:616-700`) rather than the inline
loudness measurement, because a learned model is much slower than a loudness
meter and must be cancellable: a new build phase, a mode option defaulting to
skip, and a stage entry point whose per-track callback returns `false` to cancel,
reusing the existing `stage_cancel` mechanism.

Analysis reads the **staged encoded audio**, so the vector describes the bytes
the package actually ships. The experiment's cross-codec measurements indicate
which codec a track ships as does not meaningfully change the embedding, so the
profile needs no per-codec variant.

Determinism requirements are those already recorded in ADR 0016 §9.1, applied to
this representation: stable within a build, chunk-size invariant, no FMA or
contraction or reassociation, non-finite inputs sanitized at the existing
analysis boundary and non-finite outputs rejected rather than clamped, and a
document free of timestamps, library paths, and filesystem-enumeration order.

Cache: outside the package, host-owned, keyed by
`(source_audio_sha256, profile_fingerprint)`, written atomically only after a
complete result, bounded, and removable without touching audio. A cache miss is a
normal result. Deleting a source offers to delete its rows — silently retaining
a derived vector after the user deleted the audio is a privacy defect (ADR 0016
§13).

Re-analysis on model change is a cold cache in a **new** namespace, because
`model_sha256` and `patch_hop` are fingerprint fields. Old results are retained
and marked inactive; queries never mix them.

### 5.7 Server responsibilities

Ingestion parses the document inside the existing per-package transaction and
upserts vector rows as part of the release content graph. Because
`replace_release_content` runs only for the ownership winner, exactly one package
contributes vectors per release with no new arbitration logic.

Storage, as a **v12** migration — the first genuinely free slot, since
migrations 1–10 are byte-frozen reference DDL compared byte-for-byte by
`tests/db_compat.rs` and v11 is the first Rust-defined one:

```text
similarity_sets:   id, profile_id, profile_fingerprint, producer_version,
                   state (active | inactive), created_at
similarity_vectors: set_id, track_id, dimensions, encoding, vector BLOB
                   UNIQUE (set_id, track_id)
similarity_album_vectors: set_id, release_id, dimensions, encoding,
                   contributor_count, vector BLOB
```

Properties that matter:

- **Separate from `assets`.** Analysis documents are deliberately not indexed as
  servable assets today (`store/sqlite.rs`, `resolve_asset` filters to
  `kind IN ('artwork','booklet','lyrics')`). Similarity vectors must not become
  servable objects, and adding a field to an existing response would break the
  byte-prefix invariant that `tests/api_oracle.rs` enforces.
- **`profile_fingerprint` is the partition key**, so profiles coexist and can
  never be compared.
- **`ON DELETE CASCADE`** gives incremental removal through the existing package
  sweep. No reindex job for deletion.
- **Vectors are `BLOB`.** No new dependency, and no `rusqlite` type escapes the
  `Store` trait.
- **`state = 'inactive'` until populated**, so a partial build is never queried.

`JobKind` stays `Scan | Verify` (ADR 0009: one slot, no queue, no job ids, no
cancellation framework). A similarity set is derived data, rebuildable by
rescanning. If rebuild time later proves unacceptable, a maintenance pass is a
**new** `JobKind` with its own identity, progress, cancellation, retry/resume,
and shutdown contract, decided in its own change — not a variant added to the
existing enum.

Query semantics when a package has no document, a track has no vector, or the set
is empty: a stable capability-absent code, never an empty result that reads as
"nothing is similar," and **never a fallback to a different profile**. A poor or
ambiguous result is a reason to disable the feature, not to substitute a model
(ADR 0016 §5.1).

### 5.8 API shape (sketch only; nothing implemented)

Additive and result-oriented. Every pre-existing response must remain a
byte-prefix, so new fields are omitted entirely when empty.

```http
GET /api/v1/similarity/status
GET /api/v1/tracks/{id}/similar?limit=20
GET /api/v1/albums/{id}/similar?limit=12
```

Responses carry `profileId` and within-model `score`/`rank`, and embed the
**existing** track/album JSON unchanged so clients reuse their types and caches.
**Raw vectors are never exposed** in any response. `VISIBLE` gating is inlined
into the similarity queries exactly as every other read does, so a neighbour from
an `unavailable`, `invalid`, or `conflict` package is never returned.

A `score` is a within-model cosine, not a calibrated probability. The CLAP
diagnostic is the cautionary example: CLAP cosines spanned 0.936–0.996 and EffNet
0.437–0.908 on the same 20 pairs, and comparing those numbers directly would be
meaningless. Only within-model ranks and z-scores are comparable, which is why
`profileId` accompanies every score.

### 5.9 Player consumption

The Player knows nothing about the embedding model. It consumes a list of
`{track, release, score, rank}`.

Mechanically: an `ApiClient` method through the existing fetch wrapper (inheriting
the 20 s timeout, `ApiError` mapping, and unauthorized hook); hand-written types
in the existing types file, because there is no codegen and no shared types crate
— the contract is pinned by `api_oracle.rs`; a state store alongside the existing
library store with an explicit `unavailable` state; and a section on the existing
track page, which already has a generic `?section=` mechanism.

**Not added to the offline asset plan.** The planner is a pure function over a
release detail, and its own comment already records that analysis stays deferred
because no client feature consumes it. Making similarity offline-capable needs a
new asset kind and a release-level vector surface — a separate product decision.

**No client-side analysis.** The browser does not decode audio to compute
descriptors.

## 6. Alternatives considered

| Alternative | Reason rejected |
| --- | --- |
| Similarity as package content (Option A) | A vector is a derivation, not content; a hop or runtime change would rewrite package identity for identical music, and a model upgrade becomes repackaging |
| Server-derived only (Option B) | Forfeits the portability the goals require; and ADR 0016 §10.1's stated reason does not discriminate, because loudness and waveform already change `package_fingerprint` |
| Per-track similarity assets instead of one document | Eight registration sites, per-track `DATA` blocks, budget pressure against 4096; the silent failure (directory bundle verifies, `.mpak` does not) is a real class of bug |
| Root-level `"embeddings": [...]` manifest member | Preserved by the Rust writer but **dropped by the reference C writer**; not durable across the legacy toolchain |
| A per-track `track.embedding` field | Unknown fields nested inside a known object are dropped on rewrite by **both** writers, so the data would be silently lost through the legacy CLI |
| A new MPAK block type | Buys per-track random access the document does not need; bytes after `TAIL` break total-size verification, and `TAIL.object_count` counts `DATA` members only |
| Raw vectors in the API | Locks in an unvalidated representation and invites cross-profile comparison (ADR 0016 §16) |
| ANN / vector database now | ~13 MB sequential read for a 5,000-track library at f16 — single-digit ms; a native/C dependency for an unmeasured problem |
| Extending `JobKind` with a similarity pass | ADR 0009 deliberately has one slot and no queue; a new pass needs its own contract |
| Converting vectors between profiles | Numerically unjustified; there is no ground truth to calibrate a conversion |
| Playlist/recommendation persistence in the format | Puts recommendation policy into a data interchange format and makes playlists stale on every index rebuild |
| Reviving Sonic, or converting its vectors | Sonic is retired by ADR 0012/0016 §15.1; conversion needs a separate semantic validator and migration ADR |
| Bundling any model weights | Licence and redistribution unresolved (§10) |
| A slash/`@` profile-id form | Deviates from the repository's existing hyphenated profile convention; §5.4 adopts the existing form |

## 7. Consequences

### 7.1 Benefits

- Similar tracks and albums become expressible with a small additive surface.
- Multiple embedding models coexist without migration (§8).
- Older `.mpack` files remain valid and fully supported with no similarity data.
- The document is portable, so a compliant reader — including a future offline
  or native client — may use it as input. **No such client consumption is in
  scope here** (§13); portability is a property of the artefact, not a delivered
  feature.
- The server gains no model runtime, no inference cost, and no new C dependency.
- The mechanism is licence-clean and can ship independently of §10.

### 7.2 Costs and risks

- A package carrying a similarity document is a **different package
  fingerprint** from the same release without it. This already happens for
  loudness and waveform; it is now more likely to be encountered, and library
  operators must build consistently if they want duplicate detection to behave.
- Two documents for the same release under different profiles raise the chance of
  hitting ownership `conflict` on mirror duplicates.
- The similarity index is a new rebuildable database structure with a lifecycle
  (active/inactive), a retention question, and a deletion policy.
- Vector storage is ~2.6 kB/track at 1280-D f16; ~12.8 MB for a 5,000-track
  library per profile, and 2× that with two profiles.
- f16 quantization must be **shown** to preserve useful ranking before adoption;
  it has not been measured.
- The quality of any shipped profile is unvalidated pending human review.

### 7.3 What this does not claim

- It does not claim music similarity "works" perceptually. No human ground truth
  exists.
- It does not claim cross-codec stability as a contract; five sampled pairs per
  configuration is a signal.
- It does not claim any model is better than another. The experiment's models
  disagreed substantially and neither was adjudicated.
- It does not claim scale readiness beyond a few thousand tracks.
- It does not revive, convert, or compare historical Sonic vectors.

## 8. Model replacement

| Step | Effect |
| --- | --- |
| New `profile_id` + `profile_fingerprint` are defined | Identity is a tagged hash (§5.4), not a code path |
| Format, document layout, API shape | **Unchanged.** Nothing outside the Author-side analyzer names a model family |
| `.mpack` v1 | **Unchanged.** No new field, no version bump; `kind = "similarity"` already parses |
| Existing package | May carry a second `analysis[]` entry; the array is bounded at `MAX_ANALYSIS = 32` |
| Author cache | New namespace keyed by the new fingerprint; old entries retained, marked inactive |
| Server | New `similarity_sets` row (`inactive`) plus a new partition; activated when populated |
| Queries | Serve the activated set only; partitions are never mixed |
| Vectors | **Never converted.** No code path compares vectors across fingerprints |

Cost is a second partition plus a re-analysis or rescan. There is no format
revision, no API break, no client change beyond a new `profileId` string, and no
attempted translation between models. A dimension collision is not evidence of
comparability.

## 9. Future playlists and radio (deliberately outside the format)

Similarity becomes a recommendation layer only as a **server-side derived graph**
that consumes the index. Nothing about it appears in the embedding document or the
manifest.

```text
similarity index (per profile_fingerprint)
   ├── k-nearest-neighbour edges      (ephemeral, per query)
   └── similarity graph                (persisted, derived, evictable)
          └── radio session            (seed, played set, next k)
```

- The format stores vectors, not playlists. A playlist is a time-ordered,
  preference-shaped object; encoding it would put recommendation policy into a
  data interchange format and make playlists stale on every index rebuild.
- The graph is server state, like tokens and sessions, not package content and not
  part of the package projection.
- Derived edges are a join of a vector edge with shaping inputs already present:
  the artist graph, genres, and playback history. The dependency direction is
  index → graph, never graph → format.
- A playlist is reproducible from seed plus policy version, not stored as a
  frozen track list, so a newer policy differing from an older one is expected
  rather than a defect.
- A radio session is bound to one `profile_fingerprint`; switching models
  invalidates it, because a graph over one model is meaningless over another.

## 10. Licensing and provenance

**This section makes no legal conclusion.** It separates the layers and states
what must be verified. MusicPack's own code is BSD-3-Clause; the analysis
architecture in this ADR is MusicPack's own design and carries no third-party
terms.

### 10.1 Layers

| Layer | Current position |
| --- | --- |
| MusicPack source (format, document layout, profile namespace, server schema, API) | BSD-3-Clause; no third-party terms; contains no source-derived code and never enters the LGPL-2.1 encoder/tools boundary |
| Inference runtime | `rten` — MIT OR Apache-2.0. Permissive. **Not** the licensing problem; the **MSRV** is (0.26.x reports 1.94 vs workspace 1.85) |
| DSP support | `rubato`, `microfft` — permissive |
| Model weights — Discogs-EffNet | Documented **CC BY-NC-SA 4.0**; produced by a project whose library is AGPL/commercial-oriented |
| Model weights — CLAP `larger_clap_music` | **Apache-2.0**, verified this session from repository and Hub metadata, digest recorded |
| Training data | **Not audited.** The Discogs research dataset's terms have not been reviewed against the artefact's NC/SA terms |
| Generated embeddings | **Unresolved** (§10.3 G-2) |
| Operator-supplied model capability | Permissible in principle as a local, out-of-band, hash-verified artefact; the *use* question is G-1 |
| Server-local derived data | Still subject to G-1 if the model terms are use-restricting; "local" is not a licence exemption |
| Hosted / commercial service | Would be a *use* of the model and of any generated embeddings; squarely inside G-1 and G-2 |

### 10.2 What is deliberately not assumed

- **An ONNX artefact is not assumed to be equivalent to, or exempt from,
  Essentia's licensing.** The experiment demonstrates only a technical fact:
  `rten` loaded and executed the ONNX graphs with no Essentia present. Whether
  that satisfies the licence terms of the producing project is a legal question
  this document does not answer. It is a useful input to the question, not an
  answer to it.
- **Generated embeddings are not assumed to inherit the model's licence**, in
  either direction. The argument that they are independent numerical statistics
  derived from the *user's* audio is plausible; so is the argument that NC + SA
  terms attach to adaptations. There is no clear precedent for ML-derived
  artefacts.
- **CC BY-NC-SA is not assumed to be straightforward.** "Non-commercial" is
  undefined for personal self-hosting and undefined for a hosted offering, and
  ShareAlike's reach over derived numerical artefacts is not settled.
- **"Server-local" is not assumed to resolve anything.** NC is plausibly a *use*
  restriction, not only a distribution one, so keeping vectors on the machine may
  not help; and a hosted service is a use regardless of locality.

### 10.3 The licensing position feeds back into option C

This is the part that makes the gate an architectural gate rather than paperwork.
Under option B (server-derived only) the derived data never leaves the machine,
so the redistribution half of the problem largely disappears. **Option C
reintroduces a distribution question, because a `.mpack` carrying embeddings is
itself a distributable artefact** that may be shared, uploaded, or sold.

That is a direct argument for the sub-gate in §5.3 and D-3: the package document
is **disabled by default and released only after G-2**, while the server-index
path proceeds on the mechanism alone.

### 10.4 Gates

| Gate | Question | Blocks |
| --- | --- | --- |
| **G-1** | What does "non-commercial" mean for MusicPack's own licence and for its users, for both self-hosted personal use and a future hosted or commercial offering? | Any supported Discogs-EffNet profile |
| **G-2** | Do generated embeddings inherit the model's NC/SA terms, and may a `.mpack` carrying them be redistributed? | The **package document** specifically (server-side use may also be affected) |
| **G-3** | Are the Discogs research dataset terms compatible with the artefact's NC/SA terms? | Any supported Discogs-EffNet profile |
| **G-4** | Does using an ONNX graph through a pure-Rust runtime satisfy the producing project's terms, without the AGPL-oriented library? | Production use of the artefact |
| **G-5** | MSRV exception for an ONNX runtime in a production crate (1.94 vs 1.85), and is an ML runtime permitted under `AGENTS.md`? | Any dependency change |
| **G-6** | Does f16 quantization preserve useful ranking, measured rather than assumed? | Adopting f16 |
| **G-7** | Human listening review of the stratified case set | Any user-facing quality claim |

G-1, G-2, G-3, and G-4 require qualified legal review. **Until G-1 through G-4
have answers, Discogs-EffNet cannot be a supported or default MusicPack profile
and the capability cannot be a documented MusicPack feature.** It may exist as an
opt-in, operator-supplied, explicitly experimental capability — the posture the
experiment already took.

## 11. Relationship to ADR 0016

ADR 0016 remains valid. This ADR narrows one of its positions (§10.1) and
activates the lane it reserved (§7/§17).

| ADR 0016 section | Disposition here |
| --- | --- |
| §5.1 model-free first slice | **Unchanged.** This is not that slice. It is the deferred model lane, and it ships no default model |
| §5.2 capabilities not in the first slice (learned embeddings, model downloads) | **Unchanged.** Learned embeddings remain opt-in with operator-supplied weights; no download path is introduced |
| §7 Rust ecosystem survey — "reconsider RTen first under a new ADR" | **Activated, conditionally.** `rten` is the candidate; the MSRV gate G-5 is unresolved |
| §9.2 two identifiers (stable id + fingerprint) | **Adopted** and made mandatory (§5.4) |
| §9.3 package fingerprints are not analysis cache keys; queries must not mix profiles | **Retained and reinforced** (§5.3, §5.6) |
| §10.1 storage: "store descriptors in `.mpack` → **Not the default**" | **Narrowed, not contradicted.** The portable export deferred there is now specified, and remains **opt-in**, so "not the default" stays literally true. The row's concerns about package-size and stale-travel are addressed by §7.2 and by the index being authoritative |
| §10.3 server-local storage | **Adopted** as authoritative for queries (§5.2) |
| §11 runtime ownership and platform boundary | **Unchanged and reinforced.** The server acquires no model runtime; the bundled-SQLite carve-out does not extend |
| §12 background processing and cancellation | **Unchanged.** `JobKind` is not extended |
| §13 privacy, security, licensing | **Retained and extended** by the gates in §10.4 |
| §14 performance assumptions | **Retained.** No performance claim is made here; the no-ANN revisit threshold (§14 item 7) requires measurement |
| §15.1 Sonic migration | **Unchanged.** Sonic stays retired; no conversion |
| §17 "Deferred model lane … requires a separate ADR" | **This is that ADR**, for the mechanism only, and only conditionally for a model profile |
| §18 open questions | Q3 (target rate), Q4 (cross-codec stability), Q6 (retention) remain open and are not answered by this ADR |

**Explicitly not superseded:** ADR 0016's recommendation against `.mpack` storage
**for a default profile**, its rejection of a general ML runtime in the core, and
its prohibition on reviving Sonic.

## 12. Compatibility and migration implications

- **`.mpack` v1 readers.** A `similarity` entry is an unknown `analysis[].kind`,
  already forward-compatible and structurally validated only. Old readers skip
  it. The document travels as an ordinary `DATA` member reached through the
  existing reference; **no new block type** is introduced, so no `TAIL` or
  `INDX` interaction changes.
- **Legacy writers.** The reference C writer preserves `analysis[]` (it is a v1
  field) and drops unknown *nested* fields. This is precisely why the design uses
  a package-level `analysis[]` entry and not a per-track field — a placement
  chosen for writer compatibility rather than convenience.
- **`.mpak` containers.** Bytes are covered by the existing `TAIL` package digest,
  and the member's SHA-256 appears in both `INDX` and the manifest. No new
  integrity surface, and no authenticity is claimed (the format has no signature
  or MAC anywhere; this is fixity).
- **Server schema.** Additive v12. Migrations 1–10 are byte-frozen reference DDL
  and remain untouched; a database created by either implementation still opens.
  New tables are additive and cascade-deleted with the existing package sweep.
- **Existing packages.** No migration, no re-verification, no rebuild. A package
  without similarity data behaves exactly as today. A repack that adds similarity
  yields a new `package_fingerprint` over the same `group_key`/`release_key`.
- **Clients.** No existing endpoint changes. New fields are omitted when empty, so
  pre-existing responses remain byte-prefixes and `api_oracle.rs` continues to
  pin the contract. A client that ignores similarity is unaffected.
- **Rollback.** An inactive similarity set and a retained package document can
  both be ignored by an older build. Disabling the feature is a configuration
  state, not a migration.

## 13. Explicit non-goals

- Claiming perceptual correctness, "AI understanding", mood, genre, style, or
  semantic similarity.
- Fingerprinting, duplicate detection, tempo, key, or beat tracking.
- Offline similarity, client-side analysis, or cross-collection federation.
- A general MIR platform, a vector database, or a generalized job queue.
- Converting or comparing historical Sonic vectors.
- Bundling, redistributing, or auto-downloading any model.
- Making similarity mandatory, default, or user-selectable at this time.
- Changing `.mpack` v1, the server's frozen migrations, or any public API.

## 14. Open questions and implementation gates

Ordered. Items 1–2 block any implementation; 3–6 block merging; the rest may be
deferred.

1. **Licensing G-1…G-4** (§10.4) with qualified legal review. *If G-1 fails, the
   capability ships with no supported model and operator-supplied weights only, or
   waits for a permissively licensed candidate.*
2. **Product decision** (ADR 0016 Slice 0): who asks for similar tracks, on which
   collection, and what counts as success **with no human ground truth**? The
   experiment showed two models disagreeing substantially; it did not show that
   either is good.
3. **MSRV and dependency policy** for an ONNX runtime in a production crate
   (G-5), and whether an ML runtime is permitted in Author under `AGENTS.md`.
4. **Supersession sign-off** for this ADR's narrowing of ADR 0016 §10.1 and its
   activation of the §7/§17 model lane, recorded explicitly rather than by
   silent edit.
5. **`.mpak` round-trip proof** that `analysis[]` members pack through the
   existing `canonical_pack_order` with no new block type — a test, not an
   assumption.
5a. **Similarity document format spec and reference vectors**, reviewable
   without a model download or a server schema, equivalent to the ADR 0016 §17
   Slice 1 exit gate. The binary layout in §5.5 is a proposal until this
   exists; no writer should be written first.
6. **Vector encoding** f16 vs f32, decided by a measured ranking-equivalence test
   (G-6).
7. **ANN revisit threshold** expressed as a measured library size or p99 latency,
   not as a library preference.
8. **Derived-data retention and deletion policy** for the similarity index,
   including behaviour when a source is deleted.
9. **Whether the analyzer ever belongs in `musicpack-core`.** Recommend it does
   not: Author-side placement keeps the model runtime out of the WASM-clean
   crates. Revisit only behind a separate WASM/transfer/memory policy.

Until items 1 and 2 have owners and answers, the correct implementation is no new
production similarity code.
