# MusicPack Similarity Document — v1 (format specification)

> **Status: the format gate is CLOSED. This specification is normative for the
> container format. Nothing implements it.**
>
> Four decisions were reviewed and accepted; the acceptance is recorded in
> [`FORMAT_SIGNOFF.md`](FORMAT_SIGNOFF.md) and in ADR 0017 §5.5.1.
>
> **Closing the format gate does not authorise implementation.** No `.mpack`
> reader, writer, manifest field, server schema, API, client, Author pipeline or
> `musicpack-core` module implements any of it, and ADR 0017 items 1 and 2
> (licensing, product decision) remain open and blocking. The only executable
> artefacts are a reference codec and a fixture set inside the excluded
> experiment crate, which exist so the bytes can be checked rather than believed.
>
> **G-6 (f16 vs f32), G-1…G-5 (licensing, MSRV) and G-7 (human review) are all
> still open.** Nothing in this document closes them.
>
> It is the deliverable of ADR 0017 §14 item 5a
> ("Similarity document format spec and reference vectors, reviewable without a
> model download or a server schema"). It is the equivalent of the ADR 0016 §17
> Slice 1 exit gate, narrowed to the container format.
>
> **This document does not claim that music similarity works.** It describes a
> container for numbers produced by a profile. Whether those numbers are
> perceptually meaningful is unvalidated (ADR 0017 §2.2, §7.3, gate G-7).

---

## 1. Scope, and what this document is not

### 1.1 What it specifies

The **similarity document**: a single binary file that, for one profile,
carries one vector per track of one `.mpack` package, with an explicit per-track
status and no vector at all for a track that has none. It is referenced from the
manifest as an `analysis[]` entry with `type = "similarity"`, and it is
**portable input only** — never authoritative, never a query result, never part
of musical identity.

It specifies: the byte layout, the field encodings, the status vocabulary, the
vector encodings, the limits, the validation rules and their order, the writer's
determinism obligations, and the boundaries a consumer must not cross.

### 1.2 What it does not specify, and why

| Not specified | Where it belongs |
| --- | --- |
| Which profiles exist, or what any profile means | A profile registry, gated on ADR 0017 G-1…G-5. The format only carries a fingerprint. |
| A supported or default model | Still blocked by G-1…G-4. `discogs-effnet` is not a supported profile. |
| An album aggregate | **Deferred by accepted decision B** (§9). A future decision must cover semantics, profile identity, contributor set, accumulation, stored-vs-rederived, invalidation and a consumer. |
| Server tables, API shapes, query semantics | ADR 0017 §5.7–5.8, a separate gate. |
| Author stage, cache keys, model acquisition | ADR 0017 §5.6. |
| Whether f16 is acceptable | Measured ranking equivalence, §7.3. Not decided here. |
| Offline / client consumption | ADR 0017 §13, explicitly a non-goal. |
| Anything that touches `group_key`, `release_key` or musical identity | Forbidden, §14. |

### 1.3 Relationship to the ADR's illustrative layout

ADR 0017 §5.5 proposed a 64-byte header and 16-byte table entries "as one way to
satisfy" D-3, and said explicitly that it is *"a proposal, not a settled
contract"*, subject to the gate this document closes. `PRODUCTION_DESIGN.md` §2.3
gave the same layout more detail. **Four deviations from it were reviewed and
accepted at the format gate** (see [`FORMAT_SIGNOFF.md`](FORMAT_SIGNOFF.md)
Decision A and ADR 0017 §5.5.1), each for a stated reason:

| # | ADR proposal | This specification | Why |
| --- | --- | --- | --- |
| 1 | `contributor_count` and `album_offset` in the header; `flags` bit 0 = album aggregate | Removed. `flags` bit 0 is **reserved and MUST be zero**; 16 header bytes are reserved | The album aggregate is deferred (§9). Encoding its semantics before it is needed would freeze an aggregation rule into a format, and the rule is a *profile* field (fingerprint tag 12), not a format field. |
| 2 | `table_offset` in the header | Removed. The table always begins at byte 64 | A field that can only hold one legal value is a field that can lie. The fixed offset is also what makes the layout checkable by hand. |
| 3 | Per-entry `vector_offset` and `vector_length` (u32 each) | Removed. Vector *i* begins at `64 + 12·n + i·dimensions·element_size` | Redundant, self-inconsistent data is a bug source with no benefit: every vector has the same length, so both fields are derivable, and the derivation makes "a status that has no vector" structurally impossible rather than merely checked. |
| 4 | `disc` and `track` as u16 | **u32** each; entry grows to 12 bytes | A `.mpack` manifest accepts any integer in `1 ..= i32::MAX` for `media[].disc` and `media[].tracks[].track` (`src/format/manifest/parse.rs:606` and `:648`, both via `require_int` at `:481-492`). A u16 table field would make a legal package unrepresentable. |

Nothing else changes. The magic, the 64-byte header, the big-endian framing, the
little-endian vector elements, the per-track explicit status and the "no vector
bytes for a non-vector status" rule are all as proposed. The §5.5 proposal is
retained in ADR 0017 unchanged as the historical record of what was proposed
before the gate; §5.5.1 records what was accepted and why.

### 1.4 Conformance language

**REQUIRED** / **MUST** / **MUST NOT** — an implementation that violates this is
not conformant.
**SHOULD** / **SHOULD NOT** — a conformant implementation may deviate for a
documented reason.
**MAY** — optional, at the implementer's discretion.
**Reserved** — the value or region is defined as unusable in v1.0; see §10.3.
**Proposed** — *this document's own suggestion*, not a requirement, and not yet
adopted by any decision record. Every "Proposed" item is collected in §17.

Anything not covered by one of those words is commentary.

---

## 2. Terminology

Uses ADR 0017 §3 verbatim. The terms that matter most here:

| Term | Meaning in this document |
| --- | --- |
| **profile** | The complete definition of how a vector is produced and compared. |
| **`profile_fingerprint`** | SHA-256 over a canonical tagged encoding of every profile-defining field. **The comparison and partition key.** Appendix A. |
| **`profile_id`** | A stable, human-facing name. Display only. **Never a comparison or cache key.** |
| **similarity document** | This file. Portable input. |
| **member** | One row of the track table: a package track's membership and status. |
| **vector** | `dimensions` numeric values in the encoding named by `vector_encoding`. |
| **contributor** | A member with `status = ok`. |

---

## 3. Placement in a package

The document is an ordinary referenced asset. The manifest entry parses today
with **no format change and no version bump**, under the `.mpack` v1 growth rule
(`docs/musicpack-lyrics-v1.md` §15 precedent: an incompatible change bumps *this*
document, never `.mpack` v1).

```json
"analysis": [
  { "type": "similarity",
    "profile": "musicpack-similarity-fixture-v1",
    "path": "analysis/similarity/fixture-v1.msim",
    "sha256": "abab581082ff61b12916d2065d006df89e6592f560f2edcf6cb81004d7cf5a47" }
]
```

Three facts about that snippet are load-bearing, and two of them correct the ADR:

1. **The key is `type`, not `kind`.** `src/format/manifest/parse.rs:326` reads
   `require_string(o, "type")` into `Analysis.kind`, and
   `src/format/manifest/write.rs:319` writes `"type"`. ADR 0017 §5.5 and
   `PRODUCTION_DESIGN.md` §2.2 both showed `"kind"`, which the parser would ignore
   and the writer would not reproduce. **Corrected in both documents**; see
   `FORMAT_SIGNOFF.md` §5. The Rust field name `Analysis.kind` is unchanged and
   is not an error.
2. **`profile` is optional for a non-`sonic` type.** The parser requires it only
   when `type == "sonic"` (`parse.rs:336-338`). A writer **SHOULD** emit it for
   a `similarity` entry — it is the human-readable identity and it costs nothing —
   but this specification does not make it REQUIRED, because making it REQUIRED
   is a parser change and this spike changes no production code. *Proposed
   follow-up, §17.*
3. **`profile` is untrusted data.** It never selects a model, triggers a fetch,
   loads a plugin or chooses an executable (ADR 0016 §13, ADR 0017 D-6). It is
   display text. The `analysis[]` array is bounded at `MAX_ANALYSIS = 32`
   (`src/limits.rs:73`), which is what bounds multi-profile packages.

The file extension is the writer's choice. `.msim` is used by the fixtures
because the magic is `MSIM`; nothing in the format depends on it.

In a `.mpak` container the document is an ordinary `DATA` member reached through
the existing `analysis` group. **No new block type.** ADR 0017 §14 item 5 (the
`.mpak` round-trip proof through `canonical_pack_order`) is a separate gate and
is not discharged here.

---

## 4. Conventions

| Concern | Rule |
| --- | --- |
| Integer framing | **Big-endian**, matching the MPAK header, `INDX`, `TAIL` and the identity TLV (`src/format/mpak/mod.rs`, `src/identity.rs:82-88`). A Swift or Kotlin client unpacks the header and table with a struct decoder and nothing else. |
| Vector elements | **Little-endian**, named explicitly by `vector_encoding` (`f32le`, `f16le`). The encoding tag makes the byte order a *field*, never an assumption. The two conventions never mix: framing integers are big-endian, vector elements are little-endian. |
| Compression | None. |
| Varints, length-prefixed nesting | None. Every field is at a fixed offset or derived arithmetically. |
| Floating point | Vector elements are IEEE 754 binary32 or binary16 bit patterns. No decimal text, ever. |
| Character data | **None.** The document has no string field of any kind — not a profile name, not a path, not a title, not a timestamp. |
| Padding | Only the two reserved regions (§5.2, §5.3), which MUST be written as zero. There is no implicit padding anywhere else, and therefore no padding rule to get wrong. |

---

## 5. Layout

```text
byte 0                    64                    64+12n              EOF
+------------------------+----------------------+-------------------+
| header (64 bytes)       | track table          | vector region     |
|                         | n x 12 bytes         | k x dim x elem    |
+------------------------+----------------------+-------------------+
                          ^ table starts here    ^ vectors start here
```

`n` = `track_count`, `k` = the number of members with `status = ok`,
`dim` = `dimensions`, `elem` = 2 or 4.

```text
total_size = 64 + 12 * track_count + contributors * dimensions * element_size
```

### 5.1 Header

| Offset | Size | Field | Type | Rule |
| ---: | ---: | --- | --- | --- |
| 0 | 4 | `magic` | bytes | REQUIRED `4D 53 49 4D` (`"MSIM"`). |
| 4 | 2 | `format_major` | u16 | REQUIRED. `1` in this specification. |
| 6 | 2 | `format_minor` | u16 | REQUIRED. `0` in this specification. |
| 8 | 32 | `profile_fingerprint` | bytes | REQUIRED. Raw digest. MUST NOT be 32 zero bytes. |
| 40 | 2 | `dimensions` | u16 | REQUIRED. `1 ..= 4096` (§11). A **format validation limit**, not a statement about which models MusicPack supports. |
| 42 | 1 | `vector_encoding` | u8 | REQUIRED. `1` = `f32le`, `2` = `f16le`. Others are unassigned. |
| 43 | 1 | `flags` | u8 | Reserved. **MUST be `0` in v1.0** (accepted decision B). Bit 0 is named `has_album_aggregate` and is reserved (§9). |
| 44 | 4 | `track_count` | u32 | REQUIRED. `1 ..= 16384` (§11). |
| 48 | 16 | `reserved` | bytes | Reserved. **Every byte MUST be `0` in v1.0.** |

**There is no `profile_id` in the header, and there is no `profile_id` anywhere in
the document** (accepted decision D). This is normative, not an omission:

- `profile_id` is already **fingerprint tag 1** (Appendix A), so it is already
  cryptographically bound to the 32 bytes in the header. Carrying the string as
  well would add no identity.
- A string would break the fixed 64-byte header, requiring a length field, a
  length limit, and validation of untrusted text — for information the digest
  already commits to.
- Two copies of a display name are two things that can disagree. The manifest
  entry's `profile` is the single place a name appears, and it is untrusted
  display text anyway.
- A name is self-asserted. A fingerprint cannot be relabelled by rewriting a
  string.

**REQUIRED:** a reader MUST NOT infer, derive or reconstruct a `profile_id` from
a document. **REQUIRED:** a reader MUST NOT use a `profile_id` — from the
manifest, from a filename, or from anywhere else — as a comparison, partition or
cache key. The cost is stated honestly in §12.1: a document on its own can be
compared, not named.

**Why `dimensions` is a header field and not per-entry:** every vector in a
document shares one dimension count, so per-entry copies would be redundant
state that can disagree with itself. A future profile with a different dimension
count needs no format change at all (§12.3).

**Why the 16 reserved bytes exist:** they are the room a *minor* revision needs in
order to add a field without redefining the layout (§12.4). They are not a
feature placeholder, and their only rule is that they must be zero.

### 5.2 Track table

`track_count` entries of exactly **12 bytes**, in ascending `(disc, track)`
order (§5.4).

| Offset | Size | Field | Type | Rule |
| ---: | ---: | --- | --- | --- |
| 0 | 4 | `disc` | u32 | REQUIRED. ≥ 1. The manifest's `media[].disc`. **u32, not u16** — see below. |
| 4 | 4 | `track` | u32 | REQUIRED. ≥ 1. The manifest's `media[].tracks[].track`. **u32, not u16** — see below. |
| 8 | 1 | `status` | u8 | REQUIRED. `0 ..= 3` (§6). |
| 9 | 3 | `reserved` | bytes | Reserved. **Every byte MUST be `0` in v1.0.** |

**Why `disc` and `track` are u32.** This is a correction to the earlier
illustrative proposal, not a preference. `require_int` accepts any integer in
`1 ..= i32::MAX` for both fields (`src/format/manifest/parse.rs:606` and `:648`,
via `:481-492`), so a valid manifest may carry a disc or track number above
65 535. A u16 table field would make such a package **unrepresentable** — no
conformant document could be written for it at all. u32 preserves the whole
representable manifest domain with room to spare. Widening the field is
independent of decision A: the offsets that decision removes are the reason the
entry shrank to 12 bytes, and the widening is what would have been needed had the
entry kept them.

**How a track is identified: by `(disc, track)`, not by position.** Position
would be a silent coupling to manifest array order, which `.mpack` v1 does not
canonicalise. `(disc, track)` is the manifest's own identity for a track and is
stable under array reordering.

**REQUIRED — full track coverage.** A conformant document contains **exactly one
table entry for every track in the package manifest**, no more and no fewer.
This is normative and was made explicit at the format gate. It means:

- **no track may be omitted** — absence is not a valid representation of "no
  similarity data", because omission is ambiguous between "not analysed",
  "analysed, no result" and "the writer lost it", which is the defect ADR 0016
  §4.1 warns about;
- **no track may appear twice** — `duplicate_member`;
- **ordering is canonical** — strictly ascending `(disc, track)`, below;
- **every track has exactly one status** — a member with no status is not a
  member;
- **a non-`ok` status has zero vector payload bytes** — §5.3;
- a document is written whenever analysis was **attempted** for the profile,
  including when every track failed. A document with all members non-`ok` is
  valid and is *not* the same statement as having no document (§6.3).

A document that satisfies all six is *structurally* valid; whether its member set
matches a particular manifest is the separate package-coherence check (§10.4).

**Duplicate references:** two entries with the same `(disc, track)` MUST be
rejected — `duplicate_member`. **References to a track the package does not
have** cannot be detected by a document-only reader; see §10.4.

**Ordering is canonical and REQUIRED:** strictly ascending by `(disc, track)`
compared as unsigned integers, disc first. A writer MUST sort; a reader MUST
reject a table that is not strictly ascending — `unsorted_table`. This is what
makes the byte output a function of the *content* rather than of the writer's
traversal, and it is why the same release produces the same document bytes
regardless of how its manifest array happens to be ordered.

**Entry size is fixed at 12 bytes and the vector region is 4-byte aligned**
(`64 + 12n`), so a consumer that maps the file can read elements directly on a
strict-alignment target. Nothing in the format *requires* mapping, and the
reference reader uses ordinary slicing.

### 5.3 Vector region

Vectors appear in table order, one per member with `status = ok`, back to back,
with no gaps, no per-vector headers and no padding.

```text
vector[i] starts at 64 + 12*track_count + i*dimensions*element_size
          where i is the index among *contributors*, 0-based
```

A member with `status != ok` occupies **zero bytes**. There is no length field
and no offset field, because both are computable; §1.3 explains why that is a
feature.

**REQUIRED:** every element of every `status = ok` vector MUST be finite. A NaN
or an infinity is `non_finite_vector`. There is no clamping, no sanitising and
no sentinel value: a writer that produces a non-finite element has a bug, and
turning the bug into a number would put it into a nearest-neighbour query.

**REQUIRED:** in `f16le`, a writer MUST reject any `f32` element that would round
to a binary16 infinity — underflow to ±0 is permitted, saturation is not. The
write-time code is `vector_element_not_representable`, because the cause is a
representability problem rather than a corrupt byte. At read time the same
document is `non_finite_vector`, because a stored infinity is simply a
non-finite element once decoded.

### 5.4 Canonical ordering, stated once

The document is **fully determined** by:

1. `profile_fingerprint` (32 bytes),
2. `dimensions`,
3. `vector_encoding`,
4. the set of `(disc, track, status, vector)` tuples, ordered by `(disc, track)`.

There is nothing else to choose. No timestamps, no paths, no producer version, no
sequence number, no host, no ordering that depends on filesystem enumeration, no
padding with undefined content. Two writers given equal logical content MUST
produce equal bytes; the fixture `determinism-ok.msim` and the test
`the_same_logical_document_encodes_to_identical_bytes` exist to hold that line.

---

## 6. Status

| Value | Name | Meaning | Vector |
| ---: | --- | --- | --- |
| 0 | `ok` | The profile produced a vector for this track. | present |
| 1 | `insufficient_audio` | The audio was too short, or otherwise below the profile's minimum, to say anything. A fact about the input, not a failure. | none |
| 2 | `unsupported` | The input is outside what the profile accepts (channel count, sample rate, or any other declared input restriction). | none |
| 3 | `failed` | Analysis was attempted and did not produce a result. An error, not a quality statement. | none |

Names are taken from the existing result model in ADR 0016 §8.3
(`ok | insufficient_audio | unsupported | failed`); no new vocabulary is
introduced. Values `4 ..= 255` are reserved and MUST be rejected —
`unknown_status`. The names exist for humans and for logs; **the byte is the
contract**, and a reader MUST NOT infer a status from anything else.

### 6.1 Absent means absent

**REQUIRED:** a track with no meaningful result records `status != 0` and carries
**no vector bytes**. A zero vector is not a low-similarity track; it is a
maximally uninformative point that would sit at an arbitrary place in a
nearest-neighbour query and would displace real neighbours (ADR 0016 §4.1).

Two error classes make the substitution impossible **at the byte level**, because
the layout already encodes the rule:

| Attempted | Result |
| --- | --- |
| `status = ok` with no vector | Unrepresentable. A member with `ok` always consumes `dimensions × element_size` bytes. A writer that has no vector cannot produce a conformant document; the reference writer rejects it as `vector_missing_for_ok`. |
| `status != ok` with a vector | Unrepresentable. A non-`ok` member consumes zero bytes, so there is nowhere to put one. The reference writer rejects it as `vector_present_for_non_ok`. |

### 6.2 The one hole, and who closes it

A writer that *lies* — writes four finite zero values and claims `ok` — produces
a structurally valid document. The format cannot detect this, and no format
should try: the document is deliberately opaque about what a vector means.

What closes it is that the check belongs to the layer that knows the profile:

- **Profile rule (consumer-enforced, not a format rule):** a member with
  `status = ok` MUST have a non-zero L2 norm when the profile declares L2
  normalization (fingerprint tag 8). A consumer that knows the profile SHOULD
  refuse to index a vector that violates it.
- The fixture `all-zero-vector.msim` is exactly this case, and it is committed
  deliberately: it is `result=valid` at the format layer, and the test
  `the_all_zero_vector_is_format_valid_and_only_a_consumer_can_judge_it` asserts
  that the norm is zero so the consumer's check has something to find.

So the guarantee is precise: **fabrication is impossible to do silently, and
possible only by asserting `ok` for a vector the profile itself would reject.**

### 6.3 No document, versus a document with no vectors

These are different statements and a consumer SHOULD be able to tell them apart:

| Situation | Manifest | Consumer sees |
| --- | --- | --- |
| Analysis not attempted (no model, feature off, profile not selected) | no `similarity` entry | nothing; capability absent for this package |
| Attempted, every track non-`ok` | `similarity` entry, all members non-`ok` | a document, zero contributors |
| Attempted, some tracks `ok` | `similarity` entry | a document, *k* contributors |
| Build cancelled mid-way | no entry | nothing. A cancelled build MUST NOT emit a partial document (§12.5). |

---

## 7. Vector representation

### 7.1 The registry

| Value | Name | Element | Bytes | Status in v1.0 |
| ---: | --- | --- | ---: | --- |
| 0 | — | — | — | unassigned; MUST be rejected |
| 1 | `f32le` | IEEE 754 binary32, little-endian | 4 | **REQUIRED to implement.** The reference encoding. |
| 2 | `f16le` | IEEE 754 binary16, little-endian | 2 | Defined and implementable; **adoption is gated, §7.3.** |
| 3…255 | — | — | — | unassigned; MUST be rejected |

A reader that does not implement a value it encounters MUST reject the document
(`unsupported_encoding`) rather than skip it, guess, or reinterpret it. A reader
**MUST NOT** convert between encodings, and **MUST NOT** compare vectors from
different encodings even when the dimension counts match. Output encoding is
fingerprint tag 11, so two documents with different encodings are different
profiles by construction (§12.3).

`f32be` / `f16be` are *not* defined. If a big-endian consumer ever needs them
they are values 3 and 4 in a future registry revision, and adding them is not an
architectural change — but they are not specified here, so a writer MUST NOT emit
them and a reader MUST NOT accept them.

### 7.2 Why f32le is the reference encoding

It is the encoding the profiles actually produce. The experiment's vectors are
`f32`; storing them as `f32le` is lossless and the document round-trips exactly.
Choosing half precision as the *default* would mean shipping a lossy
transformation that, as §7.3 records, has never been measured.

Storage cost is real but is not the deciding factor, and it is not hidden: at
1280 dimensions, `f32le` is 5 120 bytes per track, 51.2 MB for a 5 000-track
library, 256 MB at 100 000 tracks. The document is opt-in, the server index is
authoritative, and ADR 0017 D-7 already settles the scan question by measurement
rather than by preference. A lossy format that saves half of a number nobody has
measured the value of is the wrong trade for a *reference* encoding.

### 7.3 f16 adoption gate (G-6) — what would have to be measured

**f16 is specified but not adopted.** This document does not claim it is
acceptable, and this spike does not run the experiment. The following is the
experiment that would settle it, written down *before* any result exists so that
the threshold cannot be reverse-engineered from the answer.

**Precondition — the experiment already has the input.** No new model run is
needed and none is proposed. `PRODUCTION_DESIGN.md` §2.3 and the recorded runs
hold per-track `f32` vectors for two variants of one model over a 45-track /
15-album corpus, with per-track digests. The measurement is a post-processing
step over those vectors, not a new evaluation.

**Method.**

1. For each recorded track vector `v` (f32, L2-normalized), produce
   `q = f16_round_ties_to_even(v)` using the exact rule in §7.4, then widen back
   to f32 for arithmetic.
2. Over **all** ordered pairs in the corpus, compute cosine in f32 for
   `(v_i, v_j)` and for `(q_i, q_j)`. Do not restrict to neighbours: the tails are
   where a quantization error is most likely to reorder a result.
3. Report, per variant and per hop:
   - `max |Δcos|` and `mean |Δcos|`;
   - the minimum cosine observed in each world (the floor matters more than the
     mean: a floor that drops below a neighbour's score changes the answer);
   - Spearman ρ between the full pairwise rank orders;
   - for each of the 45 seeds: top-10 set overlap (Jaccard), top-10 rank
     displacement, and whether the top-1 neighbour changed;
   - the number of pairs whose *relative* order swaps — the count that actually
     matters for a nearest-neighbour query.
4. Repeat with the album aggregate rule, because an aggregate is a mean and
   accumulates error differently from a single vector.

**Acceptance thresholds — PROPOSED, not adopted. They require explicit sign-off
before the experiment runs.** Stating them now is the point; a threshold chosen
after seeing the numbers is not a gate.

| Metric | Proposed threshold |
| --- | --- |
| `max |Δcos|` | ≤ 1e-3 |
| Minimum cosine, f16 world | ≥ 0.99 for every pair (i.e. quantization must not push distinct tracks into "unrelated" or create false near-duplicates) |
| Spearman ρ, full pairwise order | ≥ 0.999 |
| Mean top-10 Jaccard over 45 seeds | ≥ 0.98 |
| Top-1 unchanged | ≥ 90 % of seeds |
| Relative-order swaps | ≤ 0.1 % of all pairs |

**If the thresholds are met**, adopting f16 is a *profile* decision, not a format
decision: a new `output_encoding` value in tag 11, therefore a **new
`profile_fingerprint`**, therefore a new partition, therefore a re-analysis — never
a conversion of existing vectors (§12.3). **If they are not met**, f32le remains
and the format is unchanged. There is no third outcome in which a document is
silently rewritten.

**What this gate is not:** it says nothing about whether either encoding produces
*perceptually* meaningful neighbours. That is G-7, it needs human review, and no
fixture in this directory speaks to it.

### 7.4 The binary16 conversion rule

Specified rather than delegated to a language conversion, so two independent
writers produce identical bytes.

- Round to nearest, ties to even.
- Subnormals are produced and supported. The smallest positive subnormal is
  `2^-24`; the smallest normal is `2^-14`. `k·2^-24` for `k = 0…1024` is one
  code path, because `k = 1024` *is* the smallest normal's bit pattern.
- An input that would round to ±infinity is **rejected**, not saturated.
- ±0 from underflow is permitted and is a real value, not a sentinel.
- NaN and ±infinity inputs are rejected.

The reference implementation is `docfmt::f32_to_f16_bits`
(`experiments/music-similarity-eval/src/docfmt.rs`), pinned by
`binary16_bit_patterns_are_the_standard_ones` and
`binary16_rounding_is_to_nearest_with_ties_to_even`, including the
smallest-subnormal and largest-finite boundaries.

---

## 8. Profile identity

### 8.1 What the document carries

32 bytes. That is the whole of it.

**REQUIRED:** a consumer MUST treat two documents with different fingerprints as
belonging to different partitions, MUST NOT compare vectors across fingerprints,
MUST NOT index them together, and MUST NOT convert between them. A dimension
collision is not evidence of comparability (ADR 0017 §8).

**REQUIRED:** a consumer MUST NOT recompute, infer or "repair" a fingerprint. It
does not have the profile definition, and the server has no runtime to produce one
(ADR 0017 D-5).

### 8.2 What the document does not carry, and cannot check

A document cannot verify that its fingerprint corresponds to any field list: doing
so would require the profile definition to travel inside the document, which would
make the document model-specific. Verification is a **registry lookup** — "is this
fingerprint one we know?" — and that is a consumer concern (§13).

### 8.3 `profile_id` naming (a registry rule, not a document rule)

A MusicPack-defined `profile_id` **SHOULD** follow the repository's existing
hyphenated convention (`author/src-tauri/src/sonic_model.rs:34`,
ADR 0017 §5.4):

```text
musicpack-similarity-<family>-<variant>-v<schema>
musicpack-similarity-fixture-v1
musicpack-similarity-fixture-f16-v1        # the two fixture profiles
```

Lowercase ASCII letters, digits and single hyphens; no `/`, no `@`, no spaces.
The variant and the schema revision are separate segments, not a suffix. The
namespace is MusicPack's, not a vendor's: swapping families changes a *value* in
this grammar, not the grammar, the fields, or the reader.

**REQUIRED:** a `profile_id` MUST NOT be used as a comparison, partition or cache
key under any circumstances (ADR 0017 D-4). The experiment's measured reason: two
runs of one family at different patch hops share a family name and share **zero**
byte-identical vectors.

This rule constrains no document byte, which is why it lives here and not in §5.

---

## 9. The album aggregate — deferred by accepted decision B

**v1.0 has no album aggregate.** This is a recorded architectural decision, not an
omission. ADR 0017 D-3 originally listed an optional album aggregate among the
adopted properties; accepting decision B **amended and narrowed D-3**, and that
amendment is recorded in ADR 0017 §5.5.1 and in D-3 itself.

**No album-aggregate semantics are specified here, and none are to be invented.**
A future decision to add one must cover, at minimum:

| # | What a future decision must settle | Why it cannot be settled now |
| ---: | --- | --- |
| 1 | **Aggregation semantics** | What "the album vector" means is a product statement, and ADR 0016 Slice 0 is unanswered |
| 2 | **Profile identity** | The rule is fingerprint **tag 12**, so it is part of what makes two profiles incomparable (D-1) |
| 3 | **Contributor set** | Which members contribute, and what a changed contributor set means |
| 4 | **Accumulation and normalization rules** | Summation order and precision are profile-defining numeric policy (ADR 0016 §9.1) |
| 5 | **Stored versus re-derived values** | Under `f16le` a mean over stored values differs from a mean over re-derived `f32` values, so the aggregate would silently depend on tag 11 |
| 6 | **Invalidation semantics** | "A changed contributor set invalidates it" is a **cache** rule, not a byte rule |
| 7 | **A defined consumer** | None is named; `GET /api/v1/albums/{id}/similar` in ADR 0017 §5.8 is explicitly a sketch |

The aggregate is also fully derivable: any consumer can compute one from the
per-track table, so nothing is lost by not storing it.

**What is reserved, precisely and normatively:**

- `flags` bit 0 is named `has_album_aggregate` and **MUST be `0` in v1.0**. A document that
  sets it is rejected — `reserved_nonzero`, detail `header.flags=0x01`
  `header.flags=0x01` (`reserved-flag-set.msim`). This is the point of reserving:
  a writer that sets the bit produces a document every v1.0 reader refuses,
  loudly, rather than one some readers interpret and others ignore.
- 16 header bytes are reserved (§5.1) so that a later revision can carry an
  aggregate offset and a contributor count without redefining the layout.
- A future minor revision **SHOULD** place the aggregate after the last track
  vector, so the v1.0 prefix is preserved (§12.4).

A v1.0 reader cannot read a v1.1 document, so consumers must be at least that
version to see album aggregates at all. That is the ordinary price of a versioned
format and applies to any minor bump.

The reservation is *sufficient*, and the reason is checkable: the normative order
tests the version (step 3) **before** the flags (step 4) — §10.1 — so a v1.0
reader rejects a v1.1 document on `format_minor` and never examines the flag. A
future v1.1 may therefore assign bit 0 without conflicting with v1.0.

---

## 10. Validation

### 10.1 Normative order

A reader MUST perform these checks in this order and stop at the first failure,
reporting its code. The order is normative because it is what makes "the expected
validation result" in the fixture manifest a single stable answer, and because
**no allocation is sized from an unvalidated field**: steps 1–9 use only the
header and the table, and the vector buffer is not allocated until step 10 has
proved the declared layout fits inside the actual bytes.

```text
 1  header present        bytes.len() >= 64
 2  magic
 3  version               major, then minor
 4  reserved              flags, then header reserved, then per-entry reserved
 5  profile_fingerprint   not 32 zero bytes
 6  dimensions            >= 1, then <= 4096
 7  vector_encoding       assigned and implemented
 8  track_count           >= 1, then <= 16384
 9  track table           per-entry: reserved, member, status
                          then duplicate pass, then ascending pass
10  declared size         computed layout vs actual length
11  vectors               element finiteness
```

The order encodes a trust gradient that matches the existing MPAK reader's
(`src/format/mpak/mod.rs`, "frame → CRC → length trust → bounds → member
consumption"): a cheap, total check on framing before any work that depends on it.

### 10.2 Codes

| Code | Trigger | Layer |
| --- | --- | --- |
| `truncated_header` | fewer than 64 bytes | document |
| `magic_invalid` | bytes 0..4 ≠ `MSIM` | document |
| `unsupported_version` | `format_major ≠ 1`, or `format_minor > 0` | document |
| `reserved_nonzero` | any reserved byte, in the header or in any entry, is non-zero | document |
| `zero_fingerprint` | the fingerprint is 32 zero bytes | document |
| `dimensions_zero` | `dimensions == 0` | document |
| `dimensions_too_large` | `dimensions > 4096` | document |
| `unsupported_encoding` | `vector_encoding` is unassigned or unimplemented | document |
| `track_count_zero` | `track_count == 0` | document |
| `track_count_too_large` | `track_count > 16384` | document |
| `invalid_member` | `disc == 0` or `track == 0` | document |
| `unknown_status` | `status > 3` | document |
| `duplicate_member` | two entries with the same `(disc, track)` | document |
| `unsorted_table` | entries not strictly ascending by `(disc, track)` | document |
| `truncated_table` | the bytes end inside the track table | document |
| `truncated` | fewer bytes than the declared layout requires | document |
| `trailing_bytes` | more bytes than the declared layout requires | document |
| `non_finite_vector` | a NaN or infinity in a `status = ok` vector | document |
| `member_count_mismatch` | member count ≠ the package's track count | package coherence |
| `unknown_member` | an entry whose `(disc, track)` is not a track of this package | package coherence |
| `missing_member` | a package track with no entry | package coherence |

### 10.3 Writer-side codes

Not reachable from bytes, because the layout already enforces them. They exist so
that a writer fails loudly instead of emitting something that means what it did
not mean.

| Code | Trigger |
| --- | --- |
| `vector_missing_for_ok` | `status = ok` with no vector |
| `vector_present_for_non_ok` | `status != ok` with a vector |
| `vector_length_invalid` | a vector whose length ≠ `dimensions` |
| `vector_element_not_representable` | an f32 that overflows binary16 in an `f16le` document |
| `non_finite_vector` (write) | a non-finite element offered for writing |

### 10.4 Package-coherence checks are separate

`member_count_mismatch`, `unknown_member` and `missing_member` need the manifest.
The document carries no manifest digest, so a reader that has only the file
**cannot** perform them and MUST NOT pretend to. A consumer that does have the
manifest **SHOULD** perform all three.

The separation matters: a document that fails coherence is still a *well-formed
document*. It describes a different package. The server is the consumer that has
the manifest, so the server is where these belong.

### 10.5 What validation does not cover, on purpose

- **The profile.** Unknown fingerprint, wrong dimension count for the profile,
  wrong metric, missing L2 normalisation: none of these are format errors. See
  §13.
- **Whether the vectors are meaningful.** Opaque by design.
- **The vector values beyond finiteness.** A zero-norm vector is a *profile*
  violation (§6.2), not a structural one.
- **Audio.** The document does not reference audio, and no consumer may use it to
  decide anything about the audio.

---

## 11. Limits

Reused from `src/limits.rs` wherever an existing limit applies. Making these
configurable would be a compatibility-policy change and must not happen silently.

| Limit | Value | Source |
| --- | ---: | --- |
| `MAX_ANALYSIS` (documents per package) | 32 | reused: `src/limits.rs:73` |
| Maximum `track_count` | 16 384 | reused: `MAX_DISCS` (32) × `MAX_TRACKS_PER_DISC` (512) |
| Maximum `dimensions` | 4 096 | **accepted (decision C)** — a format safety limit, see below |
| Maximum document size | 8 GiB | reused: `MAX_FILE_BYTES` (`src/limits.rs:36`), via the asset check |
| Aggregate referenced bytes | 64 GiB | reused: `MAX_TOTAL_BYTES` (`src/limits.rs:39`) |
| Package-relative path | 4 096 bytes | reused: `PATH_MAX_BYTES` |

**No new maximum document size is introduced.** The document is a referenced
asset, so the existing per-file and aggregate budgets already bound it, and the
format's own rule is exact: the declared layout MUST equal the actual byte count.
The declared layout is bounded by construction once `track_count` and
`dimensions` are bounded, so a hostile document cannot make a reader allocate
more than the file it was handed.

**`MAX_DIMENSIONS = 4096` is accepted (decision C) as a format
validation/safety limit. It is explicitly NOT a claim that MusicPack only
supports models with dimensions ≤ 4096.** Four things follow from that, and they
are normative:

1. **The field is `u16`.** The layout already supports any dimension count up to
   65 535 with no change to the header, the table or the vector region.
2. **A larger dimension count could be supported by a future validation-rule
   change, not a format revision.** Raising the limit touches exactly three
   things: the `dimensions_too_large` check, the `dimensions-too-large.msim`
   fixture (whose recorded detail text embeds "max 4096"), and that fixture's
   manifest line. No other field, offset or fixture is affected.
3. **Exceeding the limit is rejected**, with `dimensions_too_large` (§10.2). There
   is no truncation, no clamping and no silent reinterpretation.
4. **The limit is deliberately separate from model and profile capability.** It
   bounds untrusted input; it says nothing about which profiles exist, and a
   reviewer must not read it as an endorsement of any model dimension. A profile
   declaring more than 4096 dimensions is not thereby supported, approved or
   expected — it is simply a profile this format version cannot carry.

Rationale for the specific number: the largest candidate in the recorded evidence
is 1280 (Discogs-EffNet `multi`), so 4096 leaves better than 3× headroom; one
vector is bounded to 16 KiB in f32; and a maximal document is bounded to roughly
256 MiB, well inside the 8 GiB asset budget. It is **not** the security bound —
that is "validate the declared size against the actual file length before
allocating", which §10.1 mandates. What is not negotiable is that *some* finite
bound exists, and that changing it requires a specification revision rather than a
code edit.

**`track_count` requires full coverage, so the bound is not the format's
choice**: a `.mpack` manifest already admits at most 32 discs × 512 tracks, and a
document that claims more cannot describe a legal package.

---

## 12. Answering the hard questions

| # | Question | Answer | Where |
| ---: | --- | --- | --- |
| 1 | Self-contained enough to be portable? | **Yes for comparison, no for naming** — and the asymmetry is deliberate | §12.1 |
| 2 | Can a server safely reject a document for an unknown profile fingerprint? | **Yes, for indexing and queries; never for the package** | §12.2 |
| 3 | Can a future profile use a different vector dimension without changing the mechanism? | **Yes**, demonstrated by a test | §12.3 |
| 4 | Can a future format version evolve without ambiguity? | **Yes**, under three rules stated now | §12.4 |
| 5 | Can the server distinguish no vector / insufficient audio / malformed / unsupported profile? | **Yes — five distinct states**, §13 | §12.6, §13 |
| 6 | Is canonical ordering deterministic? | **Yes** — strictly ascending `(disc, track)`; the layout is fully determined | §5.4 |
| 7 | Could package similarity data accidentally affect musical identity? | **No**, and §14 shows why from the identity code | §14 |
| 8 | Does the representation preserve the package-vs-server authority rule? | **Yes** — input only, no query path, no authority language | §12.7 |
| 9 | Does the format introduce anything ADR 0017 deferred? | **No** — and the one place it *narrows* D-3 is flagged for sign-off | §9, §17 |

### 12.1 Is the document self-contained enough to be portable?

**Yes for comparison, no for naming — and that asymmetry is deliberate.**

A document alone is sufficient to: establish which profile partition it belongs
to, enumerate the package's tracks, read each track's status, and read each
present vector. That is everything a consumer needs to *use* it. It is also
sufficient to detect corruption of the vector payload, every structural fault in
§10.2, and a mismatch against a manifest the reader already has (§10.4).

What it deliberately does not carry: the profile's name, its definition, its
metric, its dimension count as *declared by the profile* (the header's value is
what the document *uses*, not what the profile *declares*), and any authentication
of the fingerprint. §1.1 and §5.1 explain why: all of it is either already bound
by the fingerprint, or belongs to a registry a portable file should not have to
embed.

A consequence worth stating plainly: a document found on its own, with no registry
and no manifest, can be **compared within itself** (two tracks of one package) and
**recognised as belonging to a known partition** — and nothing else. It cannot be
named, and it cannot be compared with any other document unless the fingerprints
match. That is the intended capability set, not a gap to be filled by adding
strings.

**What the format does not claim:** authenticity. There is no signature and no MAC
anywhere in MusicPack; the SHA-256 in the manifest is fixity, not provenance. A
tamperer who rewrites a document can rewrite its `sha256` too.

### 12.2 Can a server safely reject a document for an unknown profile fingerprint?

**Yes — for query purposes, and it must never reject the package.**

The safe behaviour is a byte comparison against bytes the consumer already holds:

```text
document.fingerprint == active_set.fingerprint  ->  index into that set
document.fingerprint != active_set.fingerprint  ->  retain, do not query, report
```

`!=` is the default answer for any consumer that knows no profile, which is the
safe default: never mix, never guess, never convert, never recompute (the server
has no runtime — ADR 0017 D-5).

The distinction that makes it safe is **which layer rejects**:

| Question | Answer | Effect |
| --- | --- | --- |
| May the server refuse to *index* the document? | Yes | no similarity rows for that package |
| May the server refuse to *query* with it? | Yes | a stable capability-absent code, never an empty result that reads as "nothing is similar" |
| May the server *recompute* it? | **No** | it has no model runtime, and untrusted `profile` data must never select one (D-6) |
| May the server *reject the package*? | **No** | a warning in the existing `verify()` vocabulary; the package stays valid and playable |

This matches ADR 0017 §5.3 and preserves the existing boundary in
`src/validation/mod.rs`, where analysis documents are structurally validated and
otherwise opaque.

### 12.3 Can a future profile use a different vector dimension without changing the core document mechanism?

**Yes, and the fixtures demonstrate it.**

`dimensions` is a header field, and both the table size and the vector region are
derived from it arithmetically. Nothing in the layout is sized for a particular
count. The test `dimensions_are_a_profile_value_not_a_format_value` re-encodes a
document at 8 dimensions and reads it back, with no change to any other field, no
new block, and no version bump.

The same holds for encoding (`f16le` is exercised end to end) and for profile
identity: a different profile is a different fingerprint, a different partition,
a second `analysis[]` entry (the array is already bounded at 32), and nothing
else. Vectors are never converted.

### 12.4 Can a future format version evolve without ambiguity?

**Yes, with three rules that are stated now so that a future revision cannot be
ambiguous by accident.**

1. **Acceptance is decidable from the header alone.** A reader accepts a document
   if and only if `format_major` equals a major it implements **and**
   `format_minor` is less than or equal to the highest minor it implements. This
   specification implements major 1, minor 0; anything else is
   `unsupported_version`. A v1.0 reader therefore rejects a v1.1 document
   *cleanly* instead of misreading it, which is the property that matters.
2. **A minor revision MUST preserve the v1.0 layout as a byte-identical prefix** —
   same offsets, widths, meanings, and validation rules — and append new regions
   after the last vector. The 16 reserved header bytes exist for this. It MUST NOT
   repurpose a reserved byte in place: v1.0 rejects any non-zero reserved byte, so
   a v1.1 field hidden there would be unreadable by design rather than by accident.
3. **A major revision may redefine anything**, including the magic. It MUST
   change `format_major`, so v1.0 readers reject it with `unsupported_version`
   rather than guessing.

Two consequences a future revision inherits, stated so they are not discovered
late: `total_size` is exact in v1.0, so a v1.1 region is only reachable by a
reader that knows about it (which is what rule 1 enforces); and a v1.1 reader MUST
still read v1.0 documents, which is why rule 2 is a constraint on the *new* spec
rather than a promise about the old reader.

### 12.5 Must a writer emit anything when analysis is cancelled?

**No.** A cancelled or failed build MUST NOT emit a similarity document, and MUST
NOT emit a partial one. There is no `cancelled` status, precisely because a
per-track status describes a *result* and cancellation means there is no set of
results to describe. The previous valid state is what remains — for the package,
by not writing the entry; for a cache row, by committing atomically after a
complete result (ADR 0017 §5.6).

### 12.6 The five states a consumer must be able to tell apart

Question 5 in the list above, as a table so that two implementations cannot
answer it differently. The full contract is §13; what matters here is that each
state has a *distinct, observable* source and that none of them is "an empty
result".

| State | Observable as | Distinct from the others because |
| --- | --- | --- |
| No document at all | no `similarity` entry in the manifest | the other four all require a document to exist |
| No vector for a track | a member with `status != ok` | the document is present and valid |
| Insufficient audio | `status == 1` specifically | the other non-`ok` values are 2 and 3, and mean different things |
| Malformed document | one specific code from §10.2 | the document does not validate, so no status is readable at all |
| Unsupported profile | validates, fingerprint ≠ the consumer's active fingerprint | the document is *fine*; it simply is not this consumer's partition |

The last row is the one that is easiest to get wrong, because it is the only
state where the bytes are healthy. A consumer that reports it as an error rather
than as "not mine" turns an ordinary situation — a library holding packages built
with two profiles — into a broken one.

### 12.7 Does this preserve the package-vs-server authority rule?

Yes, and the way it does so is largely by omission, which is worth making explicit
because "by omission" is easy to erode later:

- **The document defines no query.** No ranking, no neighbour, no distance, no
  score, no limit, no "similar" relation appears anywhere in this specification.
  A query surface is a separate decision (ADR 0017 §5.8).
- **The document defines no index.** No table, no partition lifecycle, no
  activation state, no retention. ADR 0017 §5.7 sketches those; they are not
  specified here.
- **The document never decides which document wins.** When two packages offer
  vectors for the same track and profile, that is settled by the existing
  ownership arbitration, not by anything in this format.
- **The document cannot influence what a consumer is allowed to ask.** A consumer
  decides which fingerprint it serves (§12.2); the document only says which one it
  is.
- **The one thing this format asserts** is that its bytes are internally
  consistent — which is all a portable input artefact has a right to assert.

Consequently a server that implemented this format and nothing else still could
not answer a similarity query, which is the correct outcome: the index is
authoritative, and the index does not exist yet.

---

## 13. The consumer contract (non-normative, boundary-defining)

This specification deliberately says almost nothing about consumers, because
consuming similarity is a separate gate (ADR 0017 §5.7). What it does fix is the
**vocabulary**, so that "the server cannot answer" is never rendered as "nothing
is similar". These five states are distinct and a consumer MUST be able to tell
them apart:

| State | How it is observable |
| --- | --- |
| **No document** | the package has no `similarity` entry |
| **No vector for this track** | a member with `status != ok`; the `status` says which of the three reasons |
| **Insufficient audio** | `status == 1` |
| **Malformed document** | a §10.2 code; the package is unaffected and playable |
| **Unsupported profile** | the document parses; its fingerprint is not the consumer's active fingerprint (§12.2) |

And two prohibitions, both restated because they are the ones that get violated
by accident:

- **Never fall back to a different profile** to fill a gap. A poor or ambiguous
  result is a reason to disable the feature (ADR 0016 §5.1).
- **Never mix partitions.** Not for a query, not for a "best of both" ranking, not
  as a tiebreak.

---

## 14. Similarity is not identity

**Similarity data MUST NOT participate in musical identity.** Specifically, it MUST
NOT affect `group_key` or `release_key`, and it MUST NOT introduce any new
identity mechanism. This is not a design preference; it follows from the existing
code:

- `group_key` is `mb:<uuid>` or a hash over a closed field set: title, original
  date, release type, and artists sorted by (name, role) — `src/identity.rs:104-143`.
- `release_key` is `mb:<uuid>` or a hash over a closed field set: edition, release
  date, country, label, catalogue number, barcode — `src/identity.rs:146-190`.
- Neither function reads `analysis[]`, a path, a hash, a profile, a vector, or
  any asset content. A `similarity` entry is an unknown `analysis[].type`,
  structurally validated only, so there is no input through which a vector could
  reach either key.

**What *does* change, stated exactly:** adding or removing the document changes
`package_fingerprint`, because that is the SHA-256 of the canonical manifest
serialization and the entry is manifest content (`src/identity.rs:92-95`). This is
the already-accepted category ADR 0017 §5.1 identifies — `LoudnessMode::Omit` and
`--no-waveform` each do the same — and it is *package* identity, not musical
identity. Release identity, ownership grouping and mirror detection are
unaffected. A repack that adds similarity is a new package fingerprint over the
same `group_key` and `release_key`.

**And the corollary, which is a cache rule, not a format rule:** nothing in this
subsystem may use `package_fingerprint` as a cache key. The cache key is
`(source_audio_sha256, profile_fingerprint)` (ADR 0016 §9.3, ADR 0017 §5.6).

---

## 15. Conformance fixtures

`fixtures/similarity-doc/`, described by `MANIFEST.txt`. Every fixture is
synthetic: no audio, no model, no library, no server, no filesystem path, no
artist, album, title, or username. The document has no string field, so there is
nowhere for one to hide — a test asserts that the only run of four or more
readable bytes in any fixture is the four-byte magic.

| File | What it pins |
| --- | --- |
| `minimal-ok.msim` | the minimal document: one member, one f32 vector |
| `multi-track.msim` | four members over two discs, all four status values |
| `determinism-ok.msim` | the determinism fixture: re-encoded and compared byte for byte |
| `f16-encoding.msim` | `f16le` end to end, under a **different** fingerprint |
| `all-zero-vector.msim` | the trap: `ok` with an all-zero vector (§6.2) |
| 21 further `*.msim` | one per validation code in §10.2 |

The `MANIFEST.txt` grammar is a harness convention, not part of the format: `#`
comments, an unindented fixture name, indented `key=value` lines carrying
`bytes`, `sha256`, `profile_fingerprint`, `header`, `entry<N>`, `result` and
`package_effect`. A test parses it and requires that it agree with the committed
bytes on every one of those, and that no file in the directory is missing from it.

Review and reproduce:

```sh
# annotated hex dump of any fixture
cargo run --manifest-path experiments/music-similarity-eval/Cargo.toml \
  --bin docfmt -- dump experiments/music-similarity-eval/fixtures/similarity-doc/minimal-ok.msim

# the fixture profiles' fingerprints, recomputed from their field lists
cargo run --manifest-path experiments/music-similarity-eval/Cargo.toml \
  --bin docfmt -- profiles

# the conformance suite (29 tests)
cargo test --manifest-path experiments/music-similarity-eval/Cargo.toml docfmt
```

The reference codec in `src/docfmt.rs` exists so the fixtures are reproducible and
so determinism and the validation rules are tests rather than claims. It is **not**
production code, is not a member of the root workspace, and a production reader and
writer are still to be written against this document and reviewed on their own
merits.

---

## 16. Fixture reference

Digests are of the committed bytes and are reproducible with `shasum -a 256`.

| File | Bytes | SHA-256 | Result |
| --- | ---: | --- | --- |
| `minimal-ok.msim` | 92 | `abab581082ff61b12916d2065d006df89e6592f560f2edcf6cb81004d7cf5a47` | valid |
| `multi-track.msim` | 128 | `e68d00a8247352b9ced4aa71292ece83db40f3c9d341aa2542a02b4ae722ae4d` | valid |
| `determinism-ok.msim` | 120 | `9048d2fffde4607c1366242fd7ee66e9ccbfca5f5b865ff885359247eb78bb80` | valid |
| `f16-encoding.msim` | 104 | `bd75bac50aca98f9e21ab7b1eb73af60c6c41d84ecf8d06629f50bd1cd0cd18b` | valid |
| `all-zero-vector.msim` | 92 | `14e534f9d9a354752a1cfc1fe9388db0566a1aad783608f2e76982ff083da131` | valid (profile-invalid) |
| `bad-magic.msim` | 92 | `38beb73ae9af65df951c9e104c053d7d457de7e191744786dd51f51b834cc75a` | `magic_invalid` |
| `unsupported-major.msim` | 92 | `e15442e65b18c9ea3c0e1376ae0366614d6650fedbfb5d23296d4027860581f8` | `unsupported_version` |
| `unsupported-minor.msim` | 92 | `3b2bf5c6b87c7d815a8871002abb1aa2a392f88bef591fd5971cdb152b57d302` | `unsupported_version` |
| `zero-fingerprint.msim` | 92 | `fc00a92fb66f46b03e932e0cb13a9f227659e774fe3b39dc0e8472f2716ebbf6` | `zero_fingerprint` |
| `dimensions-zero.msim` | 92 | `6fd3bbc783dbff1eba943ca2728da2fb0a54d2640b4b61b1e885c13e6c09b517` | `dimensions_zero` |
| `dimensions-too-large.msim` | 92 | `cd8a8b11e2a08be7cb63c8e04c8bb8737529a6a35e3be4b96a45c3922afe3bc8` | `dimensions_too_large` |
| `unsupported-encoding.msim` | 92 | `1f9a6674a6502dd0ffd0b19a01aeb6db22a91e11affbaa7d2a3a159a452fe9bb` | `unsupported_encoding` |
| `reserved-flag-set.msim` | 92 | `f2a5e39eccd7dac07f888b71fcddcbc09c0c112359e67fa00973b9023930cc76` | `reserved_nonzero` |
| `header-reserved-nonzero.msim` | 92 | `05a93345dbd4bf4d779dd22437a03a20645376e82ca99f7bcb13bafd3763f3fd` | `reserved_nonzero` |
| `entry-reserved-nonzero.msim` | 92 | `398e3abb9e90eb2589c23c6b84c5534a128669410c5e9e407ae796445cb400e9` | `reserved_nonzero` |
| `track-count-zero.msim` | 92 | `59483c1343647f87ba57808e7ef216f3f49e32c495b6a4c8892cb9ca2eda1dcc` | `track_count_zero` |
| `track-count-too-large.msim` | 92 | `de7c254192a40e634a75009ba39f760ce8e70722d9cd93fefb965fb8831fa29c` | `track_count_too_large` |
| `unknown-status.msim` | 92 | `4303149a2e72f204684f0493256a55f41130e14de53c05bc2eb384e551a05266` | `unknown_status` |
| `invalid-member.msim` | 92 | `2804b3220afba3e2b4b75bd612b46a20079ac258dde6c7ed48da11bf9ae59fbc` | `invalid_member` |
| `unsorted-table.msim` | 120 | `b63a54af334176e394ecfded7e563cee90e4e9b18aa88bccb4a5fdee9f5a673b` | `unsorted_table` |
| `duplicate-member.msim` | 120 | `34f29389f05fa08bc0f9e293d615afbf44f9232e4c720a1ace9ceed43499d9c0` | `duplicate_member` |
| `truncated-header.msim` | 40 | `320c1ae07a60c477778d1fc5e0637bb0592d6a574e0da332386e164616f6870f` | `truncated_header` |
| `truncated-table.msim` | 70 | `67bf26bac770b5c3e25ff77b04a159f30a2ecc918e76e2f3626b7d41fe5afb07` | `truncated_table` |
| `truncated.msim` | 88 | `4882ba93f7b1c6206a718b3530aa77d2eebbc48fd21a2866a4974a09d5a4e51a` | `truncated` |
| `trailing-bytes.msim` | 93 | `bf4367d66e38513da31fba5aac85358f298164051bca1b9d19137129109ac9a0` | `trailing_bytes` |
| `non-finite-vector.msim` | 92 | `c7fc1d55b228d4080a7cf00ab38856a27479e2614bd238ded897d5facd8673a0` | `non_finite_vector` |

Fixture profiles (Appendix A):

| `profile_id` | `profile_fingerprint` |
| --- | --- |
| `musicpack-similarity-fixture-v1` | `51d0d4b1997b7578fa290dcc2b89858f255b4493022282a4373b73ca6c2cbeee` |
| `musicpack-similarity-fixture-f16-v1` | `daf9588d95002d69c2919989fa81853f53e2793df003d57f082b0190ed931528` |

---

## 17. Open items

**Closed at the format gate** (recorded in [`FORMAT_SIGNOFF.md`](FORMAT_SIGNOFF.md)
and ADR 0017 §5.5.1): the layout and its four deviations from the ADR 0017 §5.5
proposal; the album-aggregate deferral, with the D-3 amendment; the
`MAX_DIMENSIONS = 4096` safety limit; and the omission of `profile_id` from the
document. The `"kind"` vs `"type"` terminology error in ADR 0017 §5.5 and
`PRODUCTION_DESIGN.md` §2.2 was corrected in both.

**Still open.** None of the following was decided by this document or by the
format gate, and none may be treated as settled:

| # | Item | Status |
| --- | --- | --- |
| 1 | **G-1…G-4 licensing** (non-commercial meaning, embedding redistribution, dataset terms, AGPL) | **open, blocking.** No profile can be supported until answered |
| 2 | **G-5 MSRV and dependency policy** for an inference runtime | **open, blocking** for any dependency change |
| 3 | **G-6 f16 vs f32** (§7.3) | **open.** The experiment is specified; it has not been run and the predeclared thresholds have not been agreed |
| 4 | **G-7 human listening review** | **open.** No perceptual claim is made anywhere |
| 5 | **ADR 0016 Slice 0** — the product decision | **open, blocking.** No user story, therefore no consumer for anything in this format |
| 6 | **An album aggregate** (§9) | **deferred.** Requires a future decision covering all seven items in §9 |
| 7 | **`.mpak` round-trip proof** through `canonical_pack_order` | **open** (ADR 0017 §14 item 5). The registration already exists at `src/format/mpak/write.rs:119`; the round-trip **test** does not |
| 8 | **Where a production reader lives** | **open** (ADR 0017 §14 item 9). The recommendation is *not* `musicpack-core` |
| 9 | **The ANN revisit threshold** | **open.** To be a measured library size or p99 latency, not a preference |
| 10 | **Should `profile` become REQUIRED for `type = "similarity"`?** (§3) | **open.** A manifest parser change; out of scope for the format |
| 11 | **Derived-data retention and deletion policy** for the index | **open** (ADR 0017 §14 item 8) |

**Nothing above is closed by the passing test suite.** The tests establish that
the format is internally consistent, deterministic and validated as specified.
They say nothing about whether any of it should be built.

---

## 18. What this document does not claim

- **It does not claim music similarity works.** No human ground truth exists
  (ADR 0017 §2.2). The fixtures are unit vectors chosen to be checkable by hand.
- **It does not claim f16 is acceptable.** §7.3 specifies the experiment and
  proposes thresholds; nothing has been measured.
- **It does not claim any model is better than another.** The experiment's two
  models disagreed substantially and neither was adjudicated.
- **It does not claim the layout is final in the sense of implemented.** The
  format gate is closed, so the layout is settled as a specification; nothing
  implements it, and §17 lists what is still open.
- **It does not claim authenticity.** Fixity, not provenance: no signature or MAC
  exists anywhere in MusicPack.
- **It does not claim cross-codec stability as a contract.** Five sampled pairs
  per configuration is a signal.
- **It does not revive Sonic**, convert its vectors, or compare them.

---

## Appendix A — profile fingerprint construction

The 32 bytes in the header are SHA-256 over a canonical tagged encoding of every
profile-defining field. This is the construction from ADR 0017 §5.4, using the
`src/identity.rs:82-88` TLV precedent.

```text
tlv      := field*                        fields in ascending tag order
field    := tag:u8 || len:u32be || value  absent field -> emitted as nothing
fingerprint := SHA-256(tlv)
```

| Tag | Field | Value encoding |
| ---: | --- | --- |
| 1 | `profile_id` | UTF-8 |
| 2 | model family | UTF-8 |
| 3 | model variant | UTF-8 |
| 4 | model weights SHA-256 | 32 raw bytes, or absent |
| 5 | preprocessing version (mel bands, FFT, hop, window, target rate, downmix) | UTF-8 |
| 6 | patch hop | 4 bytes big-endian |
| 7 | pooling | UTF-8 |
| 8 | normalization | UTF-8 |
| 9 | metric | UTF-8 |
| 10 | dimensions | 4 bytes big-endian |
| 11 | output encoding | UTF-8 (`f32le`, `f16le`) |
| 12 | album aggregation rule | UTF-8, or absent |
| 13 | runtime id and version | `runtime_id` `0x00` `runtime_version`, or absent |
| 14 | numeric policy | UTF-8 |

Integer-valued tags are encoded as **exactly four bytes, big-endian**. This is
one addition to the `src/identity.rs` precedent, which needs only strings, and it
is normative: without it, two registries would encode `dimensions = 1280`
differently and derive different fingerprints for the same logical profile,
silently creating two partitions.

Tag 13 is **one tag whose value is the runtime id, a single `0x00` byte, then the
runtime version** — the canonical `id NUL version` form. It is not two tags and
not a separator inside the framing.

Tag 4 is the raw 32 digest bytes, with `32` carried by the framing's length.

**These four rules are normative and are pinned by conformance tests**, so that
two independent implementations can derive the same fingerprint from the same
logical profile:

| Rule | Test |
| --- | --- |
| framing `tag:u8 ‖ len:u32be ‖ value`, ascending tags | `the_canonical_tlv_sub_encoding_rules_are_normative_and_pinned` |
| integer tags are exactly 4 bytes big-endian (tags 6, 10) | same |
| tag 4 is 32 raw bytes with a `u32` length prefix | same |
| tag 13 is `id` `0x00` `version` | same |
| an absent field emits **nothing**, not a zero length | `an_absent_field_emits_nothing_at_all` |
| one changed field ⇒ one changed fingerprint, nothing else disturbed | `a_single_field_change_changes_the_fingerprint_and_nothing_else` |
| the two fixture profiles' encodings, byte for byte | `the_fixture_profile_encodings_are_pinned_byte_for_byte` |
| this document carries the same hex the encoder produces | `the_specification_pins_the_same_tlv_hex_the_encoder_produces` |

`src/docfmt_tlv_tests.rs` contains a second, independent implementation of the
framing written from this table rather than reusing the reference codec, so a
disagreement between the two is a real finding and not a tautology.

**Explicitly not in the fingerprint:** the source audio hash (that is the *cache
key*), the library path, the package fingerprint, the model download URL, and a
dependency lockfile hash (ADR 0016 §9.2).

**Why the whole list is the fingerprint and not just the model hash:** the
experiment measured it. Changing only the patch hop, with identical audio and
identical weights, changed **0 of 45** embedding digests — so a cache keyed
without `patch_hop` would serve vectors from a different profile under the same
name. A runtime upgrade is the same class of input and must invalidate rather
than silently degrade ranking.

**The two fixture profiles.** Both are synthetic, model-free and runtime-free;
they exist to make the fingerprints in §16 recomputable, and they name no model
family.

| Tag | Profile A | Profile B |
| ---: | --- | --- |
| 1 | `musicpack-similarity-fixture-v1` | `musicpack-similarity-fixture-f16-v1` |
| 2 | `synthetic-fixture` | `synthetic-fixture` |
| 3 | `none` | `none` |
| 4 | absent | absent |
| 5 | `synthetic-linear-v1` | `synthetic-linear-v1` |
| 6 | 32 | 32 |
| 7 | `mean-of-l2-unit-then-l2` | `mean-of-l2-unit-then-l2` |
| 8 | `l2` | `l2` |
| 9 | `cosine` | `cosine` |
| 10 | 4 | 4 |
| 11 | `f32le` | `f16le` |
| 12 | absent | absent |
| 13 | absent | absent |
| 14 | `scalar-no-contraction` | `scalar-no-contraction` |

Tag 12 is absent in both, and that is not an omission: §9 gives v1.0 no album
aggregate, so a v1.0 profile has no aggregation rule to declare. Tag 11 is the
only difference between the two, and it is enough — different fingerprint,
different partition, no conversion, no mixing.

**Canonical TLV bytes.** These are the two golden vectors; `docfmt profiles`
prints the same hex, and a test asserts this document carries exactly it.

`musicpack-similarity-fixture-v1` — 191 bytes:

```text
010000001f6d757369637061636b2d73696d696c61726974792d666978747572
652d7631020000001173796e7468657469632d6669787475726503000000046e
6f6e65050000001373796e7468657469632d6c696e6561722d76310600000004
0000002007000000176d65616e2d6f662d6c322d756e69742d7468656e2d6c32
08000000026c320900000006636f73696e650a00000004000000040b00000005
6633326c650e000000157363616c61722d6e6f2d636f6e7472616374696f6e
```

`musicpack-similarity-fixture-f16-v1` — 195 bytes:

```text
01000000236d757369637061636b2d73696d696c61726974792d666978747572
652d6631362d7631020000001173796e7468657469632d666978747572650300
0000046e6f6e65050000001373796e7468657469632d6c696e6561722d763106
000000040000002007000000176d65616e2d6f662d6c322d756e69742d746865
6e2d6c3208000000026c320900000006636f73696e650a00000004000000040b
000000056631366c650e000000157363616c61722d6e6f2d636f6e7472616374
696f6e
```

(Line breaks are for readability only; the encoding is a single byte sequence.
These are unwrapped and compared by the test.)
