# G-6C admissibility review — recovered historical vectors as G-6B input

> **Verdict: ADMISSIBLE — under the explicit reconstruction procedure in §5.**
>
> No methodology, gate, threshold, loader, format, or ADR text is changed by
> this review. No evaluator run was performed. `G-6: FAIL — KEEP F32` and the
> G-6C `INCONCLUSIVE` verdict are unchanged; this review authorizes nothing
> beyond answering the admissibility question.

## 1. Question and answer

> **Can the recovered historical vectors be re-containerized into G-6B
> evaluator input without violating the frozen G-6B methodology and evidence
> rules?**

**Yes.** The frozen methodology's operative admission standard is
*content-and-digest correspondence*, not original-container byte identity —
and that standard is fully satisfiable for all four datasets, with two
environment-trivia metadata strings carried as explicitly-marked
non-historical placeholders that no gate, metric, verification, or verdict
consumes. Fabrication is structurally avoided by the procedure in §5; nothing
in it requires inventing a historical value or altering frozen code.

## 2. Evidence inventory

| Dataset | Historical vectors | Historical container | Per-vector hashes | Model identity | Corpus identity | Status |
| ------- | -----------------: | -------------------: | ----------------: | -------------: | --------------: | ------ |
| Multi hop61 | yes (45×1280, digest 45/45) | no | yes, all recompute | yes (`65cfde30…`) | yes, recorded **and** re-derived (full 64-hex match) | **admissible via §5 reconstruction** |
| Multi hop62 | yes | yes (`embeddings.json`, SHA `46e4ae08…`) | yes, 45/45 | yes (`65cfde30…`) | yes | **admissible directly** |
| Release hop61 | yes (45×512, digest 45/45) | no | yes, all recompute | yes (`fb49bd4e…`) | yes, recorded **and** re-derived (full 64-hex match) | **admissible via §5 reconstruction** |
| Release hop62 | yes | yes (`embeddings.json`, SHA `1bb182d4…`) | yes, 45/45 | yes (`fb49bd4e…`) | yes | **admissible directly** |

Corroboration established read-only for this review (no evaluator run):
same 45 source tracks hop61↔hop62 per variant; 0/45 shared vector digests
between hops (matches the historical "0/45 identical hashes" finding);
`neighbors.json` track order is index-sequential 0…44, preserving record
order; all ten per-track record fields survive per track.

## 3. Frozen-methodology requirements

| Requirement (source) | Required? | Available? | Evidence | Consequence |
| -------------------- | --------: | ---------: | -------- | ----------- |
| Four corpora as `embeddings.json` inputs, recorded 45-track/15-album library (§13) | yes | hop62 directly; hop61 as content + §5 container | vectors/records exact; digests verify | §5 procedure satisfies the schema |
| Metadata block carried into result (§13) | yes | model, model_sha256, corpus_identity, hop, variant, dims exact; `corpus_root_sha256`, `runtime` missing | recorded in compare JSONs + spike source | missing two carried as disclosed placeholders (§5); consumed by no gate |
| Per-track `embedding_sha256` re-verified before measurement (§13, loader) | yes | yes, 45/45 both hop61 variants (already demonstrated) | recomputation from parsed floats | corpus identity established, not assumed — the §13 standard, met |
| Freeze holds; no thresholds fitted to results (§14) | yes | yes | gate outcomes on real data unseen by anyone; surrogate untouched | reconstruction cannot violate §14: it changes no criterion |
| Same-data prohibition (§14.3) | yes | satisfied | prohibition covers the G-6A-motivating surrogate, not historical real vectors | no violation |
| Sign-off artefact contents (§17.1–17.2) | yes | satisfiable | variant, hop, model SHA, corpus identity all available; `profile_fingerprint` is inoperative for every corpus (Discogs-EffNet was never a registered profile — the frozen runner emits no fingerprint for any input) | hop61 adds no new gap |
| Original container bytes preserved | **not required anywhere** | n/a | no file hash was ever recorded; the loader has no originality check | byte-identity is unverifiable even for hop62 — the methodology cannot be demanding it |

### What `corpus_root_sha256` actually establishes (§2 of the brief)

Per `src/main.rs:105` it is `sha256` of the *library path string passed to
the spike* — scan-environment trivia (which directory string was scanned),
not vector provenance. It is **not** required to establish that the vectors
are historical (the per-track digests do that, airtight: any altered float
changes the digest); it is **not** used by evaluator integrity (the loader
never verifies it — `src/g6b_real.rs:88-131` verifies only per-track digests
plus dimension consistency); it is **not** consumed by any gate or metric
(it flows only into the `provenance_line` text). Its loss prevents exactly
one claim — attestation of the scanned-directory string — which no gate,
verdict, or conclusion depends on.

### What `runtime` establishes

Per `src/main.rs`, it is the hardcoded constant `"rten 0.26.0"` (corroborated
by the surviving hop62 file, which carries that exact string — corroboration
only, not transferable as hop61's record). Same classification: environment
trivia, unverified by the loader, consumed by nothing downstream.

### What the missing container metadata prevents proving (§4 of the brief)

(i) the exact scanned-directory string; (ii) the exact runtime string in the
lost container; (iii) original file bytes/timestamps (no recorded reference
exists for these anyway). It prevents **none of**: vector identity, track-set
identity, model identity, hop, dimensions, digest verification, or any gate
computation.

### Loader facts (`src/g6b_real.rs:28-138`, answering §7 of the brief)

- **Mandatory**: all seven metadata strings (no `Option`/defaults — absence
  is a load error), all ten per-track fields, non-empty track list.
- **Cryptographically verified**: only per-track `embedding_sha256`
  (recomputed via `f32_le_bytes` digest, mismatch names the track) and
  dimension consistency. Model SHA, corpus identity, root hash, and runtime
  are **never verified** — informational only.
- **Original container identity**: not required — no file-hash field exists.
- **Reconstructed containers**: acceptable to the loader iff schema-conformant
  and digest-consistent; reconstruction alters zero gate inputs (gates consume
  only vectors and artist/album grouping, all surviving exactly).

## 4. Hop-by-hop analysis (§§3–4 of the brief)

**Hop62 (both variants): admissible directly.** Complete historical
containers; the loader accepts them as-is; every §13 requirement is satisfiable
without reconstruction. Their metadata corroborates hop61's recorded fields
(same `corpus_identity`, same model SHAs) but must **not** be copied as
hop61's missing values — particularly `runtime`/`corpus_root_sha256`, which
belong to different runs on different days.

**Hop61 (both variants): admissible via §5.** The evidence is sufficient
because every *load-bearing* element survives exactly (vectors, records,
digests, identities, hop, dims, order), and every *missing* element is
non-load-bearing environment trivia. The digest match is airtight proof of
bit-exactness through the JSON round-trip: `sha256(f32_le_bytes(parsed)) ==
recorded` admits no altered float.

## 5. Reconstruction analysis — the required procedure (§5 of the brief)

Reconstruction (immutable historical bytes in a new, explicitly-marked
container) is legitimate here; fabrication (invented values presented as
history) is forbidden. The procedure that keeps the two apart, with **no
loader or methodology change**:

1. Transcribe the 45 per-track records (all ten fields) and the
   `corpus_identity`-consistent order verbatim from each hop61
   `neighbors.json`; `albums[]` omitted (the loader ignores it, §18 R-3).
2. Fill `model_name` (`Discogs-EffNet {variant}`, the spike's format rule),
   `model_sha256`, `patch_hop: 61`, `embedding_dimensions`,
   `corpus_identity` (`6d213f19…`, recorded and re-derived) from surviving
   records.
3. Fill `corpus_root_sha256` and `runtime` with **non-hex, self-describing
   placeholders** (e.g. `"UNRECOVERABLE: original container lost; see
   G6C_ARTIFACT_FORENSICS.md §4"`), which cannot be mistaken for digests and
   travel verbatim into the verdict artefact's provenance line as the
   disclosure itself. **Never** copy hop62's values, never hash the
   log-visible library path, never present placeholders as history.
4. Name files/directories distinctly from originals (e.g.
   `…/g6c-hop61-{multi,release}-reconstructed/embeddings.json`), record the
   new files' SHAs and the transcription procedure in the run documentation.
5. Do **not** invent any metadata field (no `container_status` — the existing
   schema is sufficient; the placeholders plus filenames plus run docs are
   the marking).

This satisfies every checkable rule in §§13–14/17: schema-conformant input,
digest-established identity, freeze intact, no surrogate, unchanged gates.
The two placeholders disclose rather than assert, so no historical claim is
made that the evidence cannot support.

## 6. Recommendation — smallest next step

Assemble the two hop61 containers **exactly** per §5 (transcription only; no
evaluator run), then have an independent party re-verify the transcription
(vector bytes, digests, order, placeholder presence) against the historical
files before any `g6b_real` invocation. The subsequent evaluation itself
remains the frozen mechanical run (four corpora, twice, per the G-6C report's
§7.4 command). No new experiment, profile, model, or methodology work is
recommended or required.
