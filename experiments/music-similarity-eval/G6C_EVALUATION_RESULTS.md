# G-6C evaluation results — frozen real-corpus run, all four datasets

> **Overall verdict (frozen §11 semantics): `FAIL`.** Any gate violated on
> any corpus.
>
> **`G-6: FAIL — KEEP F32` — unchanged.** No gate, threshold, loader,
> methodology, format, or ADR text was modified in this phase. The reference
> storage encoding remains `f32le`.

## 1. Verdict

`FAIL` — any gate violated on any corpus (G6B_METHODOLOGY.md §11). The
violated gate is **B-B2** (exact ordered top-k sequence identity), on
**release hop 61** (track k=5) and **release hop 62** (track k=20, plus an
independently failing album k=12 event). **Multi / 1280-D passed 7/7 on both
hops**, but the run verdict is FAIL regardless. Every numerical gate (B-A1, B-A2),
top-1 identity (B-B1), and all three profile gates (B-C1, B-C2, B-C3) pass on
all four corpora. `criteria_freeze=verified`; `gate_set_complete=true` on all
four corpora.

## 2. Corpus provenance

| Input file (external, uncommitted) | Profile | Dim | Hop | Tracks | Corpus identity | Model SHA | Status | File SHA-256 |
| --- | --- | --: | --: | --: | --- | --- | --- | --- |
| `g6c-hop61-multi-reconstructed/embeddings.json` | multi | 1280 | 61 | 45 | `6d213f19…cdcf45` | `65cfde30…` | **reconstructed** (§3) | `fc4507df…ea2637` |
| `out-hop62-multi/embeddings.json` | multi | 1280 | 62 | 45 | `6d213f19…cdcf45` | `65cfde30…` | **historical** | `46e4ae08…46dcd3fbf` |
| `g6c-hop61-release-reconstructed/embeddings.json` | release | 512 | 61 | 45 | `6d213f19…cdcf45` | `fb49bd4e…` | **reconstructed** (§3) | `3b8f8aad…e0929aec5` |
| `out-hop62-release/embeddings.json` | release | 512 | 62 | 45 | `6d213f19…cdcf45` | `fb49bd4e…` | **historical** | `1bb182d4…a975b2521a` |

All under `…/T/opencode/music-similarity-eval/`, outside the repository, never
committed. 15 albums per corpus. No surrogate, no synthetic data.

## 3. Reconstruction evidence (hop 61)

Per `G6C_ADMISSIBILITY_REVIEW.md` §5, transcribed from the historical
`out-hashed-{multi,release}/neighbors.json` with verbatim embedding decimals
(no binary float round-trip in the transcription script):

- 45/45 tracks, index order 0…44, all ten per-track fields, recorded
  `corpus_identity`, model identity, `patch_hop: 61`, recorded dimensions;
- `corpus_root_sha256` and `runtime` are the authorized non-hex placeholders
  (`UNRECOVERABLE: original container lost; see G6C_ARTIFACT_FORENSICS.md
  §4`); never copied from hop 62, never presented as history; no new schema
  field; no `albums[]` block.
- Independent verification (`g6c-verify.py`, separate code path, exit 0 both
  variants): all 45 vectors byte-identical to historical records with digests
  recomputed exactly; `corpus_identity` re-derived to the full recorded
  64-hex value; placeholders exact, non-hex, and unequal to hop62's values.
  One genuine transcription defect (a `duration_seconds` string-vs-number
  encoding) was caught by verification in the first attempt and fixed before
  any evaluator run — the fail-closed design working as intended. Final
  container SHAs are the table in §2.

**Historical vs reconstructed:** the *vectors* are historical (45/45
digest-verified per variant); the *hop-61 containers* (bytes, SHAs above,
placeholder metadata) are reconstructed and must never be described as the
original files.

## 4. Gate table (frozen definitions, G6B_METHODOLOGY.md §11)

| Gate | multi61 | multi62 | release61 | release62 |
| ---- | ------- | ------- | --------- | --------- |
| B-A1 `max|Δcos|` ≤ 2^-10 | PASS 3.2723e-5 | PASS 2.8625e-5 | PASS 9.8944e-6 | PASS 1.1623e-5 |
| B-A2 `max|‖v'‖−1|` ≤ 2^-10 | PASS 3.9089e-5 | PASS 3.9751e-5 | PASS 8.5731e-5 | PASS 5.9473e-5 |
| B-B1 top-1 identity | PASS 0/45 | PASS 0/45 | PASS 0/45 | PASS 0/45 |
| B-B2 ordered top-k identity | PASS all k, album 0/15 | PASS all k, album 0/15 | **FAIL** k=5 seed 32 | **FAIL** k=20 seed 5 (+album k=12, §5) |
| B-C1 `|‖v‖−1|` ≤ 2^-10 | PASS 3.6285e-8 | PASS 3.7780e-8 | PASS 3.5555e-8 | PASS 3.6614e-8 |
| B-C2 φ ≤ 2^-24 | PASS 1.2694e-8 | PASS 1.1350e-8 | PASS 6.2935e-9 | PASS 5.2780e-9 |
| B-C3 rejections = 0 | PASS 0 | PASS 0 | PASS 0 | PASS 0 |
| **Corpus verdict** | **PASS** | **PASS** | **FAIL** | **FAIL** |

## 5. Retrieval evidence (all k ∈ {1,5,10,12,20} + album k=12)

Top-1 identity holds on all 45 seeds of all four corpora. No new exact ties,
no sign crossings anywhere. Load-bearing sequence changes (reference margins
in f32 ULPs — the f32 reference resolves every one of them):

- **release61, seed 32**: candidates 16/18 at reference ranks 5/4 swap;
  margin 8.941e-7 = **15 ULPs**. Breaks the top-5 (hence top-10/12/20)
  presented sequence. → B-B2 FAIL.
- **release61, seed 33** (derived from census): candidates 08/16 at ranks
  16/17 swap; margin 5.066e-6 = **85 ULPs**. Independently breaks the top-20
  sequence.
- **release62, seed 5**: candidates 00/26 at reference ranks 19/18 swap;
  margin 3.397e-6 = **57 ULPs**. Breaks the top-20 presented sequence. →
  B-B2 FAIL (first differing boundary k=20; gate short-circuits here).
- **release62, album seed 12** (derived from census; masked in the gate line
  by the short-circuit): album candidates 06/09 at ranks 8/9 swap; margin
  6.557e-7 = **11 ULPs**. Independently breaks the album top-12 sequence.

Census-only flips beyond every gated prefix (recorded, no gate impact):
multi61 1 event (ranks 34/33, 3.5 ULPs); multi62 1 event (ranks 27/28,
**461 ULPs** — f32-resolved, f16-flipped, beyond top-20); release61 5 events
(ranks ≥22); release62 4 track events (ranks ≥22). The multi62 event is the
methodology working as designed: the presented lists (all gated k) are
identical, and the out-of-range flip is recorded, not exempted, not gated.

Per §18 R-3, the release62 album event is evidence about the storage path
under the documented candidate aggregation rule, not a v1.0 product decision.

## 6. Numerical evidence

Worst symmetric `|Δcos|` over all 7,920 ordered pairs: 3.2723e-5 (multi61) —
~30× inside the 2^-10 ceiling. Worst stored-norm deviation: 8.5731e-5
(release61) — ~11× inside. Reference norms deviate ≤3.8e-8 (pipeline
L2-normalized, as specified). Subnormal energy ≤1.27e-8 (≤4.7× inside B-C2).
Zero overflows. Report-only: mean `|Δcos|` 1.9e-6…7.7e-6; Spearman (diagonal
excluded) 0.999999462…0.999999913; effective support 26 (release) / ~115
(multi) — dense profiles, as expected.

## 7. Determinism

Two independent full runs (`g6c-run-a`, `g6c-run-b`), identical command,
identical criteria file (`G6B_METHODOLOGY.md` SHA `f1afb244…`):

- run A exit 1, run B exit 1 (fail-closed: FAIL verdict → non-zero);
- `REPORT.txt` byte-identical across runs (`cmp` clean);
- `report_sha256=d90a138c1d3c474fbefb7510db53a199ea31f1be6632c4c13a051064680b9fed`
  both runs. Stderr is cargo build chatter only.

## 8. Final interpretation

Under the frozen verdict rule the outcome is FAIL, so no admissibility claim
follows for any profile and no G-6 status revision arises (§17.7 requires a
PASS plus the §17.4/§17.6 human steps, neither of which is reached). The
profile-specific reading (D-6):

- **Multi / 1280-D passed 7/7 on both hops** — but with the run verdict FAIL,
  this is evidence, not a decision; it cannot by itself admit f16.
- **Release / 512-D failed B-B2 on both hops**: f16 storage changes the
  presented ordered sequences the methodology defines as user-visible
  (a top-5 order change at 15 ULPs; a top-20 boundary crossing at 57 ULPs;
  an album top-12 order change at 11 ULPs). Under the current (absent)
  near-tie semantics (D-2 undecided), a changed result is a changed result —
  recorded, not dismissed as noise.

This establishes **nothing** about f16 in general, about other profiles, or
about perceptual quality (G-7). It establishes, for the evaluated
Discogs-EffNet release profile under exact-identity retrieval semantics, that
the f16 storage path changes presented results the f32 reference resolves.
The existing decision stands: **`f32le` remains the reference encoding;
`G-6: FAIL — KEEP F32`.**
