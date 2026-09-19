# mpccut differential corpus (Phase 15J)

Committed outputs of the reference C `mpccut` over committed encoder-fixture
streams, plus the manifest the Rust test replays. Generated once by
`tools/gen_cut_fixtures.py <mpccut> <encoder-data-dir>`; the Rust test never
invokes C.

## Provenance

* reference `mpccut` v0.9.0, same build/environment as the encoder corpus
  (`05d97a5`, Apple clang 21, arm64, `-O0`, `FAST_MATH` + `CVD_FASTLOG`).

## Corpus

| Case | Input | Start | End |
|---|---|---|---|
| `short-full/head/mid/tail` | `q5-44100-short.mpc` | full / 0–500 / 100–900 / 900–end | single-AP-block stream, SH/`beg_silence` variation |
| `multi-full/head/mid/tail/aligned/lastblock/unaligned` | `q5-44100-multiblock.mpc` | full / 0–20000 / 30000–60000 / 80000–end / 0–73728 / 73728–end / 12345–54321 | multi-AP-block stream, seek-table rebuild |
| `q7-transient-mid` | `q7-44100-transient.mpc` | 5000–15000 | different quality/content |

`manifest.txt` records `name input start end bytes sha256`.

## Compatibility result

`tests/cut_compat.rs` reproduces all 12 reference outputs byte-for-byte.
