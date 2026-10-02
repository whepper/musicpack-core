# G-6C artifact forensics — search for the historical Discogs-EffNet corpora

> **Finding: PARTIALLY RECOVERED — 2 of 4 embedding datasets exist as exact
> historical artifacts; the other 2 survive as digest-verified vectors inside
> historical summary files, awaiting an evidence-chain decision.**
>
> No file was modified, copied into the repository, or committed. No gate was
> run. `G-6: FAIL — KEEP F32` and the G-6C `INCONCLUSIVE` verdict are
> unchanged. Anything new generated during this search is **not** historical
> evidence (§8 below).

## 1. Evidence searched

- Experiment reports (`experiments/music-similarity-eval/`, all `.md`):
  `FINDINGS.md`, `REPORT.md`, `README.md`, review/worksheet docs, G-6…G-6C
  and both independent reviews. Every run-output reference resolves to the
  literal `$EXTERNAL_OUTPUT` variable — no expanded path was ever recorded.
- Git history (`git log --all -S…`): artifact names, model hashes and
  `EXTERNAL_OUTPUT` appear **only as text** in two doc/test commits
  (`1433aca`, `4784f51`). No embedding, neighbor, CSV, ONNX, or output file
  was ever committed, staged, or deleted from the tree. Nothing to restore.
- Local filesystem: the repo tree, the legacy sibling repo, home sweeps,
  Trash, iCloud Drive, caches, external volumes (none mounted) — and, newly,
  the OpenCode record store named by `~/.local/share/opencode/log/opencode.log`.
- Other artifacts: CLAP diagnostics, cross-codec runs, blind/stratified
  review sets, the legacy `.pb` models (different artifact class — §6).

## 2. The decisive discovery

The historical external output directory **still exists** and was never
searched by path before, because no report ever expanded the variable:

```text
/private/var/folders/6h/83xm7ljx4sz5njxdhkjsq53h0000gn/T/opencode/music-similarity-eval/
```

(Float paths: this is macOS per-boot `TMPDIR` material — its survival is
luck, not policy. Nothing here should be relied on as archival storage.)

## 3. Artifacts

| Artifact | Historical reference found? | Actual file found? | Hash verified? | Status |
| -------- | --------------------------- | ------------------ | -------------- | ------ |
| Multi 1280-D, hop 61 embeddings | yes (`compare-hop-multi`, `patch_hop_a: 61`, corpus `6d213f19…`) | **vectors only** — inside `out-hashed-multi/neighbors.json` (45×1280, digest 45/45) | per-vector digests recompute exactly | **vectors recovered; container lost** |
| Multi 1280-D, hop 62 embeddings | yes (`out-hop62-multi` metadata) | **yes** — `out-hop62-multi/embeddings.json` (2,356,155 B; file SHA `46e4ae08…`) | 45/45 per-track digests; model `65cfde30…`, corpus `6d213f19…`, 45 tracks / 15 albums | **RECOVERED** |
| Release 512-D, hop 61 embeddings | yes (`compare-hop-release`, `patch_hop_a: 61`, corpus `6d213f19…`) | **vectors only** — inside `out-hashed-release/neighbors.json` (45×512, digest 45/45) | per-vector digests recompute exactly | **vectors recovered; container lost** |
| Release 512-D, hop 62 embeddings | yes (`out-hop62-release` metadata) | **yes** — `out-hop62-release/embeddings.json` (960,851 B; file SHA `1bb182d4…`) | 45/45 per-track digests; model `fb49bd4e…`, corpus `6d213f19…`, 45 tracks / 15 albums | **RECOVERED** |
| ONNX multi (`65cfde30…`, 15,998,047 B) | yes (README/FINDINGS/REPORT + log: downloaded from essentia.upf.edu, shasum-verified at use) | **no** (log path gone) | n/a | missing |
| ONNX release (`fb49bd4e…`, 18,621,961 B) | yes (same) | **no** (log path gone) | n/a | missing |
| Corpus manifest (45 tracks = hop62 track set) | — | — | source-track sets identical hop61↔hop62; 0/45 shared vector digests (matches historical "0/45 identical hashes") | corroborated |

File SHAs (record, not evidence): hashed-multi `neighbors.json`
`5bc52e68…`, hashed-release `neighbors.json` `af5d27d3…`,
`compare-hop-multi/comparison.json` `40384a3e…`,
`compare-hop-release/comparison.json` `02b4f4af…`.

Classification notes: the legacy `.pb` models are **class C** (different
format and bytes; conversion would be a new profile fingerprint — exactly the
G-6C report's reason recreation is a new profile). The per-track vectors in
the hop61 `neighbors.json` files are **exact artifact content, not
provenance**: f32 values whose digests recompute to the recorded
`embedding_sha256` values 45/45 per variant — i.e. bit-identical to the
vectors the lost `embeddings.json` files carried (same source tracks, same
dims, zero overlap with hop62, matching the recorded patch-hop comparison).

## 4. Important discoveries

- **Where files were:** `$EXTERNAL_OUTPUT` =
  `…/T/opencode/music-similarity-eval/`; the two `out-hop62-*` runs
  (Sep 25 21:37–21:39) and the two `out-hashed-*` pilot runs
  (Sep 24 00:59–01:01; `neighbors.json` only) survive. All other run dirs
  (`out-evidence*`, `out-determinism-*`, `out-full-*`, `out-final-*`,
  `out-small`, `out-smoke4`, …) are **empty** — their `embeddings.json`
  files existed per the log and were later deleted. No other `embeddings*`
  file exists anywhere found.
- **How hop61 is identified:** `compare-hop-{multi,release}` were invoked
  `--a …/out-hashed-{multi,release} --b …/out-hop62-{multi,release}` with
  `patch_hop_a: 61, patch_hop_b: 62`, same `corpus_identity 6d213f19…`,
  and model SHAs `65cfde30…` / `fb49bd4e…` — so `out-hashed-*` are the
  historical hop61 runs despite their names.
- **What the log preserves:** full historical command lines (model paths,
  `--model-sha256`, `--library /Users/jeroen/Music`, `--albums 15
  --tracks-per-album {2,3}`, `--patch-hop`, `--out`), the Essentia download
  URLs, and every deleted run directory name. No file contents beyond that.
- **What provenance is missing for a hop61 rerun container:** the
  `g6b_real` loader requires `corpus_root_sha256` and `runtime` metadata
  fields, which survive nowhere for the hop61 runs (they lived in the lost
  `embeddings.json` metadata blocks). `model_name` format is confirmed
  (`Discogs-EffNet {variant}`, `src/main.rs:86`).

## 5. Conclusion

**G-6C still cannot be executed as specified** — the two hop61
`embeddings.json` containers are gone and must not be silently reconstituted.
But the situation changed materially: the hop61 *vectors* exist, exactly,
with 45/45 digest verification per variant against the historical records.

The one open question is an **evidence-chain decision for the owner**, not a
technical one: whether assembling the two missing `embeddings.json`
containers from (a) digest-verified historical vectors, (b) recorded
per-track metadata carried alongside them, and (c) recorded run metadata
(model SHA, hop, corpus identity, dimensions, model-name format) — with
`corpus_root_sha256` honestly re-derived or marked unrecoverable rather than
assumed — constitutes admissible input to the frozen evaluator, or whether
that assembly is itself a new artifact that the methodology cannot bless.
Until that decision is made explicitly, **no evaluator run, no container
assembly, and no G-6C revision**; the verdict stays `INCONCLUSIVE` and the
storage decision stays `f32le`.
