# G-6C — real-corpus evaluation: execution report

> **Verdict (frozen §11 semantics): `INCONCLUSIVE` — the real corpus could not
> be evaluated because it does not exist on this machine or in this
> repository's history. The evaluation was NOT executed.**
>
> **`G-6: FAIL — KEEP F32` — unchanged and still the operative state.**
> `G-6B: READY FOR REAL-CORPUS RUN` — unchanged (methodology and runner are
> ready; the run itself did not happen).
>
> This report records an attempted execution honestly. No gate was evaluated,
> no data was substituted, no synthetic corpus was manufactured, and no
> methodology text was changed. Per `G6B_METHODOLOGY.md` §11: *"INCONCLUSIVE —
> the real corpus cannot be evaluated (unavailable, unreadable, or the
> instrument detects its own failure)."* An incomplete evaluation is not a
> recommendation.

## 1. Execution identity

| Item | Value |
| --- | --- |
| Date | 2026-09-27 |
| Git commit | `13ddb074ae3da435c7557917d9bac12f9cf05fbe` ("docs: close similarity format gate"), experiment tree uncommitted on top |
| Evaluator | `src/g6b.rs` `d312a10ec229018fccb3631e609ec99b…`, `src/g6b_real.rs` `3c5d84d3e08d43d9f25b24eb85cbf1d1…`, `src/bin/g6b_real.rs` `9726280dea0689712d9d88fc9de93e7b…` (SHA-256 prefixes) |
| Methodology | `G6B_METHODOLOGY.md` `f1afb244dd21f9ad382b952f4457f81b…` (includes frozen §11 table + §18 R-1…R-5 records); freeze rows pinned byte-for-byte by `g6b::FROZEN_GATE_ROWS` and enforced by `check_criteria_freeze` at run time |
| Evaluator execution | **None.** `g6b_real` was not run against any data in this phase — the four required inputs do not exist to feed it. Running it against anything else would manufacture evidence. |

## 2. Corpus recovery investigation

Required inputs (per `G6B_METHODOLOGY.md` §13 and the `g6b_real` loader
contract — metadata `model_name, model_sha256, patch_hop,
embedding_dimensions, corpus_identity, corpus_root_sha256, runtime`; per-track
`artist, album, title, path, duration_seconds, source_sha256, frame_count,
patch_count, embedding_sha256, embedding: Vec<f32>`, with every
`embedding_sha256` re-verified before measurement):

| # | Required dataset | Status |
| --- | --- | --- |
| 1 | Discogs-EffNet `multi`, 1280-D, hop 61 | **NOT FOUND** |
| 2 | Discogs-EffNet `multi`, 1280-D, hop 62 | **NOT FOUND** |
| 3 | Discogs-EffNet `release`, 512-D, hop 61 | **NOT FOUND** |
| 4 | Discogs-EffNet `release`, 512-D, hop 62 | **NOT FOUND** |

Places searched (all negative):

- repository tree including ignored paths (`*.json` is gitignored for the
  experiment; no `embeddings.json`/`neighbors.json`/`*.onnx` present);
- full git history (`git log --all --diff-filter=A` — no embedding artifact
  was ever committed);
- Spotlight index (verified functional), home-wide filesystem sweep (pruned),
  `~/Downloads`, `~/Desktop`, `/tmp`, Trash, iCloud Drive, caches, external
  volumes (none mounted);
- the legacy sibling repository `~/Documents/VSCode/musicpack` — contains the
  **TensorFlow `.pb`** models (`discogs_multi_embeddings-effnet-bs64-1.pb`,
  `discogs_release_embeddings-effnet-bs64-1.pb`) and their Essentia metadata
  JSONs, but **no run outputs** and **no ONNX artifacts**;
- the wiki (Outline): no records of the experiment outputs or their location;
- `REPORT.md`/`FINDINGS.md` confirm the run outputs live (lived) only in an
  **external, gitignored output directory** whose location is deliberately
  not recorded in the repository.

The evaluator's own smoke test during G-6B implementation used a hand-built
synthetic fixture in temporary storage (since deleted) and was explicitly
labelled a wiring check — it is **not** corpus evidence and is not reusable
here.

**Why the surrogate was not reused as a substitute.** The G-6 surrogate is
prohibited as criteria evidence twice over: §14 rule 3 (the same-data
prohibition — the corpus that motivated the G-6A criteria repair may never be
used to evaluate them) and the G-6B review's finding that its clean results
are structural properties of the generator, not measurements (CLT
cancellation plus a margin distribution that never visits the region where
f16 flips answers). Evaluating the frozen gates on it would manufacture a
PASS with no evidentiary content, and a real-corpus-shaped synthetic set
would be fabrication by another name. `G6B_METHODOLOGY.md` §13 is explicit:
the surrogate is not a substitute for any of the real-corpus requirements.

## 3. Recreation assessment (per §13 — no improvisation executed)

`G6B_METHODOLOGY.md` §13: *"If the recorded outputs cannot be recovered, the
minimum recreation is: one re-run of the existing spike over the same
45-track library for each of the four configurations — a model-and-audio
operation that must be justified on its own, not slipped in as an experiment
convenience."* Recreation is currently **impossible without operator
inputs**, for three independent reasons:

1. **The model artifacts are gone.** The spike requires the operator-supplied
   ONNX files `discogs_multi_embeddings-effnet-bs64-1.onnx`
   (SHA-256 `65cfde30655a939de420e5c09a49a43648336fdbbf79eb3af6d3b40176339d8e`)
   and `discogs_release_embeddings-effnet-bs64-1.onnx`
   (SHA-256 `fb49bd4e906624bbc5ee09e648e88d53b28c23870a8a6bed145624fd062530e7`).
   No ONNX copy of either model exists on this machine. Only the legacy
   TensorFlow `.pb` files remain; a fresh `.pb`→ONNX conversion produces
   different bytes, therefore a different `model_sha256`, therefore a
   **different profile fingerprint** — it would be a new profile, not the
   recorded one, and FORMAT_SPEC §7/ADR 0017 identity rules forbid
   presenting it as the tested profile.
2. **The 45-track / 15-album library state is unrecoverable.** The library
   root was never committed (privacy), and `corpus_identity` is derived from
   the selected tracks' `source_sha256` values. `~/Music` exists but the
   recorded selection (15 album groups × 3 evenly spaced tracks per group)
   cannot be re-identified or verified against the recorded corpus identity.
3. **It is a model-and-audio operation requiring explicit authorization** —
   model inference over a private music collection, with the G-1…G-4
   licensing posture (ADR 0017 §10) unchanged. §13 requires this to be
   justified on its own; this phase does not authorize it.

Everything else recreation needs is present and pinned: the spike code
(`src/main.rs`), `rten 0.26.0` (locked), toolchain `rustc 1.97.1`, the
decoder/preprocessing chain, the serializer (`src/report.rs`, which writes
exactly the `embeddings.json` schema `g6b_real` consumes), and the frozen
evaluator.

## 4. Gate results

| Gate | Result | Evidence |
| ---- | ------ | -------- |
| B-A1 | NOT EXECUTED | corpus unavailable — no evaluation was run |
| B-A2 | NOT EXECUTED | corpus unavailable — no evaluation was run |
| B-B1 | NOT EXECUTED | corpus unavailable — no evaluation was run |
| B-B2 | NOT EXECUTED | corpus unavailable — no evaluation was run |
| B-C1 | NOT EXECUTED | corpus unavailable — no evaluation was run |
| B-C2 | NOT EXECUTED | corpus unavailable — no evaluation was run |
| B-C3 | NOT EXECUTED | corpus unavailable — no evaluation was run |

No numerical evidence, retrieval evidence, or M13 scaling evidence exists to
report; there was no measurement. The frozen fail-closed semantics are
trivially respected: nothing was converted into a PASS.

## 5. Determinism

Not applicable to a run that did not occur. For the record, the committed
surrogate instrument check remains byte-deterministic (two consecutive runs,
identical digest `c67e0992381813d0a3c2d3523f306003059a0122b8b3ab12c803ee184ef9468b`),
and the evaluator itself is unchanged since the G-6B implementation report.

## 6. Verdict

**`INCONCLUSIVE`** — under the frozen §11 rule ("the real corpus cannot be
evaluated (unavailable…)"). This is an execution-phase outcome, not a
methodology failure and not evidence about f16. It does not revise, weaken,
or strengthen any G-6B criterion.

## 7. Recommendation

- The reference storage encoding remains **`f32le`** and the formal state
  remains **`G-6: FAIL — KEEP F32`**. Nothing in this phase supports f16
  admissibility for any profile; equally, nothing new counts against it.
- The strongest permissible conclusion is unchanged: f16 admissibility for
  the evaluated profile is **undetermined** until the real corpora are
  available and the frozen evaluation actually runs.

### Precise unblock requirements (all operator-side; no code work remains)

1. Supply (or re-obtain) the two ONNX model artifacts with the recorded
   SHA-256 digests (`65cfde30…`, `fb49bd4e…`). A re-conversion from `.pb` is
   a **new model identity** and would require re-recording the profile, not
   reusing the recorded corpus identity.
2. Provide access to the original (or content-identical) 45-track /
   15-album library root so the spike can re-run; the resulting
   `corpus_identity` will be re-derived and must be recorded as such — it
   cannot be assumed equal to the historical one.
3. Explicitly authorize the model-and-audio re-run (§13's own requirement),
   one spike execution per configuration ({multi, release} × {hop 61, 62}),
   writing the four `embeddings.json` files to an external directory.
4. Then the evaluation itself is mechanical and frozen:
   `cargo run --bin g6b_real -- --corpus multi61=… --corpus multi62=… --corpus
   release61=… --corpus release62=… --criteria G6B_METHODOLOGY.md --out DIR`,
   run twice for determinism, with this report's §4 table filled from the
   artefact.

Alternative worth naming: if the original artifacts are considered lost
rather than recoverable, the honest paths are (a) declare the historical
profile untested and retire the question until a new profile is actually
produced (at which point G-6B applies to it fresh), or (b) authorize a new
recorded run as a *new* profile identity with the full evidence chain above.
Both are decisions for the format/product owner; neither is made here.

## 8. Hygiene and tests

- `cargo test` (experiment crate): **87/87 pass** before and after this
  phase; `cargo fmt --check` clean; `cargo clippy --all-targets -D warnings`
  clean.
- `git status --short` inspected: changes exist only under
  `experiments/music-similarity-eval/` (the G-6B implementation work and this
  report). No model weights, audio, private metadata, or corpus artifacts
  were added; no production code, `.mpack` format, ADR, or historical
  G-6/G-6A/G-6B evidence was modified. Historical digests re-verified:
  `fixtures/g6/REPORT.txt` = `921e58f1c9e3ad41…`, `fixtures/g6/RUN.txt` =
  `5aa9f0304f84b640…`.
- Nothing was committed.
