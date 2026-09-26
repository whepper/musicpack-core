# Similarity document v1 — sign-off record

> **Status: SIGNED OFF 2026-09-26. The format gate is CLOSED.**
>
> Decisions A, B, C and D were reviewed and **accepted**. Accepting B required an
> explicit amendment and narrowing of ADR 0017 D-3, which is recorded in ADR
> 0017 §5.5.1 and in D-3 itself. Three further rules — full track coverage, the
> canonical profile-TLV encoding, and `u32` disc/track identifiers — were made
> normative at the same time.
>
> **Closing the format gate did not authorise implementation.** No production
> code implements the format, and the licensing, product and quality gates below
> are all still open. The recommendations in the body of this document are
> retained as the reasoning that led to each decision, so the path stays
> auditable; the decisions themselves are recorded in the table above and in
> `FORMAT_SPEC.md` / ADR 0017 §5.5.1.

## Decision status

| Item | Decision | Status |
| --- | --- | --- |
| A | Remove per-entry vector offsets, lengths, and the table offset | **ACCEPTED** |
| B | Defer the album aggregate from v1.0 | **ACCEPTED — required an explicit D-3 amendment, recorded in ADR 0017 §5.5.1** |
| C | `MAX_DIMENSIONS = 4096`, as a format validation/safety limit | **ACCEPTED** |
| D | `profile_id` is not stored in the document | **ACCEPTED** |

### Closed by this sign-off

- **ADR 0017 §14 item 5a** — the similarity document format gate: specification
  plus deterministic reference fixtures, reviewable without a model download or a
  server schema. Equivalently, the ADR 0016 §17 Slice 1 exit gate narrowed to the
  container format.
- ADR 0017 D-3, **amended and narrowed** (album aggregate removed from v1.0).
- The four layout/limit/identity decisions, now normative in `FORMAT_SPEC.md`.
- The `"kind"` vs `"type"` terminology correction (§5 below).

### Still open — explicitly NOT closed by this sign-off

| Gate | Question | Status |
| --- | --- | --- |
| **G-1** | What "non-commercial" means for MusicPack's licence and its users | **open, blocking** |
| **G-2** | Do generated embeddings inherit the model's NC/SA terms; may a `.mpack` carrying them be redistributed | **open, blocking** |
| **G-3** | Are the Discogs research dataset terms compatible with the artefact's NC/SA terms | **open, blocking** |
| **G-4** | Does using an ONNX graph through a pure-Rust runtime satisfy the producing project's terms | **open, blocking** |
| **G-5** | MSRV exception for an inference runtime (1.94 vs 1.85), and whether an ML runtime is permitted in Author | **open, blocking** for any dependency change |
| **G-6** | Does f16 quantization preserve useful ranking, measured rather than assumed | **open.** The experiment is specified in `FORMAT_SPEC.md` §7.3 with predeclared thresholds; it has not been run and the thresholds have not been agreed |
| **G-7** | Human listening review of the stratified case set | **open.** No perceptual claim is made anywhere |
| ADR 0016 Slice 0 | The product decision: who asks for similar tracks, on what collection, and what counts as success with no human ground truth | **open, blocking** |
| ADR 0017 §14 item 5 | `.mpak` round-trip proof through `canonical_pack_order` | **open.** The registration already exists (`src/format/mpak/write.rs:119`); the round-trip test does not |
| ADR 0017 §14 item 9 | Where a production reader lives | **open.** The recommendation is *not* `musicpack-core` |
| ADR 0017 §14 item 7 | The ANN revisit threshold | **open.** To be a measured library size or p99 latency |
| ADR 0017 §14 item 8 | Derived-data retention and deletion policy | **open** |
| — | A future album aggregate | **deferred.** Requires a decision covering all seven items in `FORMAT_SPEC.md` §9 |
| — | Should `profile` become REQUIRED for `type = "similarity"` | **open.** A manifest parser change |
| — | Any model-specific licensing decision | **open.** No model is a supported or default profile |

**The passing test suite closes none of these.** The 29 conformance tests
establish that the format is internally consistent, deterministic and validated
as specified. They say nothing about whether any of it should be built, and
nothing about whether similarity is perceptually meaningful.

## How this decision was reached

The four decisions below were presented for review with evidence, alternatives
and a recommendation. The recommendations were accepted; the reasoning is kept
here unchanged so a future reader can see what was proposed, what was chosen, and
why.

---

## Decision A — remove per-entry offsets and lengths

### The proposal

Track-table entries are a fixed **12 bytes**: `disc` u32, `track` u32, `status`
u8, 3 reserved bytes. A vector's position is **derived**, not stored:

```text
vector[i] = 64 + 12 * track_count + i * dimensions * element_size
```

No per-entry `vector_offset`. No per-entry `vector_length`. No `table_offset` in
the header. Total size is exact: `64 + 12n + contributors * dimensions *
element_size`, and it MUST equal the file length.

### Consistency with ADR 0017

ADR 0017 §5.5 describes a 16-byte entry carrying a `u32` offset and a `u32`
length. That is stated in the ADR as *"a proposal, not a settled contract"*,
expressly subject to the gate this memo prepares (§14 item 5a). So removing the
fields does not contradict the ADR — but it *does* settle a question the ADR left
open, and it changes the entry size, so it is a new architectural decision
requiring sign-off rather than an implementation detail.

### Evidence

**Fixed dimensionality and fixed encoding make the offsets a pure function.**
`dimensions` and `vector_encoding` are header fields (FORMAT_SPEC §5.1), so every
vector in a document has exactly `dimensions × element_size` bytes and the stride
is constant. The offset of vector *i* is therefore `base + i · stride`. This is a
property of the design, not an assumption about future profiles.

**Deterministic canonical layout is stronger without them.** The document is
fully determined by `(fingerprint, dimensions, encoding, ordered member set)`
(§5.4). Stored offsets would add a *second, independent* statement of the same
layout. Redundancy is where writers and readers disagree: with offsets, a writer
that computes `i · stride + 1` produces a document that is self-consistent in
size and wrong in content, and a reader that trusts offsets reads arbitrary bytes.
With derived offsets there is nothing to cross-check and nothing to get wrong.
Every read is bounds-checked against one validated `expected_total` instead of
against a per-entry `offset + length ≤ file_size` pair, which also removes the
integer-overflow surface that pair creates.

**Corruption and truncation detection lose nothing, because the surrounding model
already covers the file.** This is the load-bearing point and it is verifiable in
this repository:

- In a `.mpak`, the member's SHA-256 is written into `INDX`
  (`src/format/mpak/write.rs:40`, `:149-150`) and the `TAIL` digest covers every
  preceding byte (`src/format/mpak/mod.rs:16`).
- In a directory bundle, `verify()` hashes the referenced file against the
  manifest declaration (`src/validation/mod.rs`).

Offsets are not an integrity mechanism; they are a *second* integrity mechanism
inside a file that already has one, and a weaker one, because an attacker who can
rewrite offsets can also rewrite the declared SHA-256. What offsets would detect —
an internally inconsistent layout — is only reachable by a buggy writer, and a
buggy writer is caught by the round-trip test in the fixture suite, not by a
field.

**Random access is already O(1).** The one thing offsets are usually for. A
consumer wanting one track's vector computes the index and slices; it does not
scan. The server's per-track rows make per-vector addressing inside the document
largely irrelevant anyway.

**Variable-length payloads cannot be represented in v1.0. This is the real cost
and it should not be minimised.** A profile using per-vector adaptive
quantisation, product quantisation, or any compressed or ragged encoding cannot
use this layout. Two things bound that cost:

1. Such a profile would already be a different `output_encoding` and a different
   `dimensions` semantic — a different profile, not a different encoding of this
   one. Dense fixed-length embeddings are what the recorded candidates produce
   (1280-D, 512-D).
2. **A future version can introduce another representation, and the reserved
   regions are not even needed for it.** Because the acceptance rule is
   "major equal and minor ≤ supported" (§12.4), a v1.1 reader can define a
   per-entry length region appended after the table, and every v1.0 reader will
   reject a v1.1 document on `format_minor` at normative step 3 — *before* it
   looks at the table at all. So variable-length vectors cost a minor version, not
   a redesign, and the 3 reserved bytes per entry stay reserved.

**Would offsets provide meaningful value in the current design?** No:

| Motivation for offsets | Value here |
| --- | --- |
| Random access to one vector | None; arithmetic is O(1) |
| Skipping to one vector while streaming | None; the vector is a contiguous slice |
| Tolerating heterogeneous payloads | None; no current profile needs it |
| Self-describing file for a hex-dump reader | Marginal; one multiplication |
| Detecting corruption | None beyond what the manifest/`INDX` digest already does |
| Absorbing a future variable-length design | Only by deferring it to a minor version, which the version field already does |

### Trade-offs accepted

- A profile needing variable-length or compressed vectors requires a minor
  version bump, and consumers must be ≥ that version to read it.
- The file is 8 bytes per track smaller (12-byte vs 16-byte entries).
- A hand-written parser in another language computes one multiplication. Trivial.
- The layout is not independently readable without computing the stride. Accepted:
  the computation is one line and is specified exactly.

### DECISION: **ACCEPTED** (recommendation was ACCEPT)

---

## Decision B — defer the album aggregate

### The proposal

v1.0 contains no album aggregate. `flags` bit 0 is *named*
`has_album_aggregate` and **MUST be zero**; a document that sets it is rejected
with `reserved_nonzero`. 16 header bytes are reserved so a later revision can
carry an aggregate offset and contributor count.

### Consistency with ADR 0017

This is the one item that **narrows a recorded decision**. ADR 0017 D-3 lists "an
optional album aggregate" among the adopted properties, §5.5 specifies its
aggregation rule, and §5.7's schema sketch has `similarity_album_vectors`. ADR
0017 §14 item 4 requires supersession to be recorded explicitly rather than by
silent edit. Accepting B therefore **must** include an explicit amendment to D-3;
accepting the specification alone would leave the ADR claiming something the
artefact does not do.

### Evidence

**Track-level vectors are sufficient for the first capability, because the
aggregate is derivable.** Any consumer that wants an album vector can compute one
from the per-track table. The document's job is to be portable input; a consumer
that is going to compute the aggregate anyway gains nothing from a stored copy,
and gains a second thing that can be stale.

**There is no defined consumer.** ADR 0016 Slice 0 — the product decision — is
still unanswered, and ADR 0017 §5.8 is explicitly a sketch. `PRODUCTION_DESIGN.md`
§4.5 says an album aggregate with too few contributors should be absent or
low-confidence, which is a *quality policy* question that cannot be answered
before the consumer exists. Precomputing for an unnamed consumer is the definition
of premature.

**It requires a separate aggregation semantic, and that semantic is a profile
field, not a format field.** ADR 0017 §5.4 already reserves fingerprint **tag 12**
"album aggregation rule" for exactly this. The things a format would otherwise
have to freeze:

| Aggregation question | Why it is not a format decision |
| --- | --- |
| Accumulation order and precision (f32 vs f64 summation) | The repository's own numeric policy treats this as profile-defining (ADR 0016 §9.1) |
| Mean over *stored* values or over re-derived f32 values | Under `f16le` these differ, so the result would silently depend on tag 11 |
| What a changed contributor set does | The ADR's answer ("invalidates the aggregate") is a **cache** rule, not a byte rule |
| Minimum contributor threshold | A product-quality judgement, not a validity condition |

None of these can be settled honestly before a profile exists, because all of them
are properties of that profile's arithmetic. A container format encoding them
would freeze one profile's rule into the interchange format — which is the
coupling ADR 0017 D-1 exists to prevent.

**It can be added later without a major version, and the reservation is
sufficient — for a specific, checkable reason.** A v1.1 revision appends the
aggregate after the last vector, sets `flags` bit 0, and uses the reserved header
bytes. Because the normative order checks the version (step 3) *before* the flags
(step 4) — FORMAT_SPEC §10.1 — a v1.0 reader rejects a v1.1 document on
`format_minor` and never examines the flag at all. So naming bit 0 now creates no
conflict for a future v1.1 that assigns it, and a v1.0 document can never set it.
The committed fixture `reserved-flag-set.msim` makes this executable rather than
aspirational: if a future revision tried to set bit 0 inside a v1.0 document, the
suite fails.

The honest cost: a v1.0 reader cannot read v1.1 documents, so **consumers must be
≥1.1 to see album aggregates at all.** That is the ordinary price of a versioned
format and applies to any minor bump, but it should be stated rather than
discovered.

### Trade-offs accepted

- No album-level data in v1.0 documents; a consumer wanting it computes it or
  waits for v1.1.
- ADR 0017 D-3 must be amended explicitly. This is a real obligation, not a
  formality.
- A named reserved bit carries a small interpretive liability — an implementer
  could read the name as a promise. Mitigated by the MUST-be-zero rule, the
  `reserved_nonzero` code, and the fixture.
- A named unused flag is one bit of permanent spec surface. Cheaper than the
  alternative of leaving it anonymous, because the error detail can then say what
  was set.

### DECISION: **ACCEPTED**, and the ADR 0017 D-3 amendment it was conditional on
was recorded at the same time** — see ADR 0017 §5.5.1 and the note on D-3. No
album-aggregate semantics were invented.

---

## Decision C — `MAX_DIMENSIONS = 4096`

### The proposal

`dimensions` is a `u16` field constrained to `1 ..= 4096`. Exceeding it is
`dimensions_too_large`.

### Consistency with ADR 0017

ADR 0017 sets no dimension limit. This is entirely new, and it is the **only new
numeric limit** the format introduces.

### Evidence

**Relationship to existing repository limits: there is no analogue, so this is a
genuinely new constant.** The neighbouring limits are reused rather than
reinvented: `MAX_TRACKS` is derived as `MAX_DISCS × MAX_TRACKS_PER_DISC` =
32 × 512 = 16 384, and document size is bounded by the existing `MAX_FILE_BYTES`
and `MAX_TOTAL_BYTES` asset budgets. `dimensions` has no precedent in
`src/limits.rs`, which is why it needs its own number and its own justification.

**Worst-case resource implications are bounded twice, and only once by 4096.**

- *Arithmetic:* 16 384 tracks × 4096 dimensions × 4 bytes = 256 MiB of vectors,
  plus a 192 KiB table — about 256.2 MiB, roughly 32× inside the 8 GiB
  per-file budget. One vector is 16 KiB.
- *Security:* the normative order validates the declared total against the actual
  file length (step 10) **before** any vector buffer is allocated (step 11). A
  reader therefore never allocates more than the file it was handed, whatever the
  header claims. 4096 is a sanity bound, not the security bound, and the
  specification says so.

**It must be read as a format safety limit, never as a supported-model claim.**
The largest candidate in the recorded evidence is 1280 (Discogs-EffNet `multi`);
CLAP is 512. If 4096 were read as "MusicPack supports 4096-dimensional
embeddings", raising it would look like an endorsement of a model — precisely the
model-to-architecture coupling ADR 0017 D-1 forbids. The number is a bound on
untrusted input. It should be phrased that way wherever it is cited.

**A future model could reasonably exceed it.** Large self-supervised audio
encoders commonly land at 1024–2048 dimensions and some exceed 4096. So 4096 is
not generous indefinitely. That is acceptable *because* of the next point.

**Raising it later is a validation-rule change, not a format version.** This is
the fact that should drive the decision, and it is verifiable: `dimensions` is a
`u16`, so the field already supports up to 65 535 with no layout change.
Increasing the limit touches exactly three things — the `dimensions_too_large`
check, the `dimensions-too-large.msim` fixture (whose recorded detail text embeds
"max 4096"), and the corresponding manifest line. The header, the table, the
vector region and every other fixture are untouched. **The cost of choosing the
wrong number here is roughly four lines and one fixture regeneration**, which is
why this decision is lower-stakes than it looks and should not be agonised over.

The one lasting consequence: the limit is a de-facto interoperability floor. A
writer emitting 8192-D documents produces files no v1.0 reader will accept, because
every v1.0 reader enforces this bound. That argues for setting it where no
plausible profile sits for several years rather than setting it tightly.

**Is 4096 unnecessarily arbitrary?** Somewhat — but it is arbitrary-with-a-reason,
which is the right kind of arbitrary for an input bound: *some* finite bound must
exist, and the value is a judgement about headroom. The alternatives:

| Value | Assessment |
| --- | --- |
| `u16::MAX` (65 535) | Rejects the goal. Removes the limit's readability and lets a 4-byte header imply gigabytes before a buggy reader's size check. |
| 1024 | Rejects the goal. Below the observed 1280 — would exclude a real candidate. |
| 2048 | Defensible but thin: 1.6× headroom over the largest observed. |
| **4096** | **3.2× headroom. Recommended.** |
| 8192 | The main alternative. Removes the question for longer at the cost of a looser bound. |

Per the instruction for this memo, **no substitution is proposed**: 4096 is
recommended, and 8192 is recorded as the credible alternative a reviewer may
prefer.

### Trade-offs accepted

- A reviewer may reasonably prefer 8192. That is a one-constant change plus one
  fixture regeneration, and it does not require a version bump.
- The number must be documented as a safety limit, not a capability claim, or it
  becomes an accidental architectural statement.
- Every conformant reader enforces it, so it is an interop floor, not a hint.

### DECISION: **ACCEPTED** as a format safety limit, on the explicit
understanding that raising it is a validation-rule change rather than a format
revision. **4096 stands; it was not replaced with 8192 or any other value.**

---

## Decision D — `profile_id` is not stored in the document

### The proposal

The header carries the 32-byte `profile_fingerprint` and nothing else that
identifies a profile. `profile_id` lives in the manifest entry's `profile` field
and in the profile registry. Readers MUST NOT infer a name from a document, and
MUST NOT use a name as a comparison, partition or cache key.

### Verification of the rationale against the actual format

| Claim | Verified how |
| --- | --- |
| The fingerprint is 32 bytes | Header offset 8, size 32 (FORMAT_SPEC §5.1) |
| Identity is already carried by tag 1 | `docfmt::ProfileFields.profile_id` → TLV tag 1; the test `profile_identity_is_recomputable_from_its_documented_fields` asserts the emitted tag sequence `[1,2,3,5,6,7,8,9,10,11,14]`, so changing the name changes the fingerprint |
| A duplicated name can disagree | The name would then exist in three places — registry, manifest entry, document — of which the document controls none |
| The header stays fixed and compact | 64 bytes total, 16 of them reserved; a string needs a length, a limit and validation of untrusted text |
| A registry can map fingerprint → metadata | **Not verifiable in this repository — no registry exists yet.** See below. |

The first four hold. The fifth is a dependency on a future component and must be
recorded as a risk, not asserted as a fact.

### Is the omission safe?

The cost is precise and should be stated rather than discovered: **a bare
document cannot be named.** It can be compared within itself, and it can be
recognised as belonging to a known partition, and that is all. A consumer holding
only the file has 32 hex bytes and no label.

That is weaker than it first appears, for a reason that is the decisive argument
here: **a consumer without a registry cannot use the document meaningfully
anyway.** Using a vector requires the metric, the normalization guarantee and the
comparison rule. `metric` (tag 9) and `normalization` (tag 8) are **not** in the
document. `dimensions` is, but only as the value the document *uses*, not as the
value the profile *declares*. Storing the name would not make such a consumer
usable — it would label a thing it still cannot compute with. The name is the
least useful of the missing fields, so omitting it removes the least loss.

For the server the dependency is satisfied by construction: it already holds the
active set's fingerprint, and the admission decision is a byte comparison against
bytes it has (§12.2). For a future offline or native client, a profile registry
becomes a prerequisite — which is a real, named cost of this decision and the
reason sign-off is conditioned on recording it.

### Consistency with ADR 0017's distinction

ADR 0017 §3 and D-4 require **two identifiers, both mandatory**, and define
`profile_id` as "displayed by APIs. Never a cache key" and `profile_fingerprint` as
"the comparison and cache key". Read in context, "both mandatory" is a statement
about a *profile*, not about every artefact: the fingerprint is mandatory **in the
document**, the id is mandatory **in the registry and the API**. The document
carrying only the fingerprint is consistent with that distinction and arguably
enforces it — a document physically cannot be keyed by name, so the "never a cache
key" rule cannot be violated by someone who has only the file.

The narrowing to acknowledge: the document is no longer self-describing for a
human. Mitigations already in place are the annotated `docfmt dump` tool, the
manifest entry's `profile` field, and the registry. Accepted.

### Trade-offs accepted

- A bare document cannot be named, only compared.
- A profile registry becomes a prerequisite for any consumer that wants to
  describe a document rather than merely use it.
- A stray `.msim` on disk is opaque without tooling.
- In exchange: no string in the format, so no untrusted text to validate, no
  possibility of a name/fingerprint disagreement, and a 64-byte header.

### DECISION: **ACCEPTED**, with the profile registry recorded as a
prerequisite for consumers that need to describe a document, not merely use one.**

---

## 5. Mechanical correction already applied

Not a decision; a factual error verified against source and fixed without
changing any architectural meaning. Details are in the delivery report; the
substance is:

The `.mpack` manifest serialises the analysis kind under the JSON key **`type`**,
not `kind`:

- `src/format/manifest/parse.rs:326` — `require_string(o, "type")`, bound to the
  Rust field `Analysis.kind`
- `src/format/manifest/write.rs:319` — `put(&mut ao, "type", s(&a.kind))`

ADR 0017 §5.5 and `PRODUCTION_DESIGN.md` §2.2 both showed
`{ "kind": "similarity", … }` in their JSON examples, which no writer would
produce and no parser would read. ADR 0017 §5.5's own prose already said the
parser requires `type`, so the documents contradicted themselves.

Left untouched on purpose:

- The **Rust** field name `Analysis.kind` and the quoted struct in
  `PRODUCTION_DESIGN.md` — those are correct as written.
- The SQL column `assets.kind` in the `resolve_asset` filter — a different field
  in a different store, and correct.

---

## 6. Bidirectional consistency check

### 6.1 ADR 0017 → FORMAT_SPEC

Every normative *format* requirement in ADR 0017, and whether the specification
implements it.

| ADR 0017 requirement | Source | FORMAT_SPEC | Status |
| --- | --- | --- | --- |
| Package-level `analysis[]` entry with a new type | D-3, §5.5 | §3 | implemented (terminology corrected) |
| One document per (package, profile) | D-3, §5.5 | §3, §5.2, §11 | implemented |
| Explicit per-track status; absent means absent, never a zero vector | D-3, §5.5 | §6, §6.1, §6.2 | implemented, and stronger than the ADR: the substitution is structurally impossible |
| Optional album aggregate | D-3, §5.5 | §9 | **NOT implemented — narrowing, Decision B** |
| Two identifiers; `profile_id` never a comparison key | D-4, §5.4 | §5.1, §8, §8.3 | implemented, and enforced structurally — **Decision D** |
| `profile_fingerprint` = SHA-256 over the canonical tagged encoding, 14 tags | §5.4 | Appendix A | implemented; two sub-encodings added (see 6.2) |
| Big-endian header/table, hand-unpackable, no compression, no varints | §5.5 | §4, §5.1, §5.2 | implemented |
| Server never infers; document is input, never authoritative | D-5, §5.2, §5.3 | §12.2, §12.7, §13 | implemented, largely by explicit prohibition |
| A document finding must not invalidate the package | §5.3 | §13, and `package_effect=none` on every fixture | implemented and asserted by a test |
| No effect on `group_key` / `release_key` | D-2, §2.4 | §14 | implemented, argued from `src/identity.rs` |
| Multi-profile coexistence bounded by `MAX_ANALYSIS = 32` | §5.3, §8 | §3, §11 | implemented by reuse |
| f16 vs f32 decided by measured ranking equivalence (G-6) | §14 item 6 | §7.3 | implemented as a gate, with thresholds **predeclared** and marked proposed |
| Aggregation rule = mean of L2 unit vectors, then L2, contributor count | §5.5 | §9 | **NOT implemented — narrowing, Decision B** |
| Vectors are never converted between profiles | §8 | §7.1, §8.1, §12.3 | implemented |
| Document free of timestamps, paths, enumeration order | §5.6 | §5.4, plus a fixture containment test | implemented and tested |
| Non-finite output rejected, not clamped | §5.6 | §5.3, §10.2 | implemented |
| Disabled by default; released only after G-2 | D-3 | §1.2, §17 | not a format concern; retained as a gate |

**No contradiction found.** The only two gaps are Decision B, which is a
deliberate and declared narrowing rather than a contradiction.

### 6.2 FORMAT_SPEC → ADR 0017

Every architectural choice in the specification, classified. Nothing here has been
promoted into the ADR.

| Choice | vs ADR 0017 | Classification |
| --- | --- | --- |
| 64-byte header | same size | clarification (composition differs: a 16-byte reserved region replaces the removed fields) |
| 12-byte table entries | ADR: 16 | **new decision — Decision A** |
| `disc`/`track` as u32 | ADR: u16 | **new decision — Decision A**, and a correction of a defect (see below) |
| No per-entry offset/length; derived layout | ADR: both present | **new decision — Decision A** |
| No `table_offset` | ADR: present | part of Decision A |
| No album fields; `flags` bit 0 reserved | ADR: present and specified | **narrowing — Decision B** |
| `profile_id` absent from the document | ADR silent | **new decision — Decision D** |
| `MAX_DIMENSIONS = 4096` | ADR silent | **new decision — Decision C** |
| Full track coverage (one entry per manifest track) | ADR silent | **new decision**, small but normative for writers |
| Write a document when analysis was attempted even if all tracks failed | ADR silent | **new decision**, small but normative for writers |
| Canonical order = strictly ascending `(disc, track)` | ADR: "sorted by (disc, track)" | clarification; the specification adds *strictness* and the observation that manifest array order is not canonical |
| Non-`ok` status ⇒ zero vector bytes | ADR: "carries no vector bytes" | implemented as specified |
| `vector_encoding` registry (`f32le` required, `f16le` gated) | ADR: both named | clarification plus a **gating decision** that matches G-6 |
| 16 header reserved bytes; 3 reserved bytes per entry | ADR silent | clarification — the forward-compatibility mechanism, with a MUST-be-zero rule |
| Version acceptance: major equal, minor ≤ supported | ADR silent | clarification, required to make §12.4 decidable |
| Normative validation order and stable codes | ADR silent | clarification (implementation-visible, low stakes) |
| Package-coherence checks split from document validity | ADR silent | clarification |
| `MAX_TRACKS = 16384` | ADR silent | derivation from existing limits, not a new limit |
| Integer profile tags as 4 bytes BE; tag 13 NUL-joined | ADR: field list only | clarification of the encoding; **a new sub-encoding choice** that a registry must adopt identically |
| Reference codec, fixtures, dump tool, conformance suite | — | implementation detail of the spike, explicitly not production |

**The `u32` widening deserves a separate note.** ADR 0017 §5.5 specifies u16 disc
and track. `require_int` accepts any integer in `1 ..= i32::MAX`
(`src/format/manifest/parse.rs:606`, `:648`, via `:481-492`), so a legal manifest
may carry a track number above 65 535, and a u16 table field would make that
package **unrepresentable** — a document could not be written for it at all. The
widening is therefore not a preference; it closes a defect in the proposal. It is
still sign-off, because it changes the entry size.

### 6.3 New architectural decisions discovered by this check

Two, both surfaced by writing the check rather than by the earlier work:

1. **Full track coverage is a new normative requirement on writers.** ADR 0017
   requires explicit per-track status but never says whether a track may be
   omitted. The specification requires exactly one entry per manifest track. This
   is the rule that makes "absent means absent" unambiguous, and it is small, but
   it is a requirement a writer must implement and it is not in the ADR.
2. **The profile TLV needs two sub-encoding decisions** (integer tags as 4 bytes
   big-endian; tag 13 as `id` NUL `version`) that ADR 0017 §5.4 does not make.
   Without them two registries would compute different fingerprints for the same
   profile — which would silently create two partitions. These belong in the ADR
   or in the registry specification, not only in the format specification's
   appendix.

---

## 7. What is deliberately still open

Unchanged by this memo, and unaffected by any of A–D:

- G-1…G-5 licensing, dataset terms, AGPL question, and the `rten` MSRV exception.
  Still blocking for any profile at all.
- ADR 0016 Slice 0 — the product decision. No user story has been named, so no
  consumer exists for anything this format might eventually carry.
- G-7 — human listening review. Nothing here is a quality claim.
- The `.mpak` round-trip test (ADR 0017 §14 item 5). Note the *registration* is
  already in place: `canonical_pack_order` pushes the `analysis` group
  (`src/format/mpak/write.rs:119`). What is missing is the test, not the code —
  which makes this item cheaper than the ADR implies.
- Where a production reader lives (ADR 0017 §14 item 9). The reference codec in
  the experiment is explicitly not it.
- The ANN revisit threshold, expressed as a measured library size or p99 latency.
