# musicpack-mpc-tools

Rust SV8 container tools: the migration home for the container-surgery side
of the legacy C Musepack tools (`codec/mpccut`, and the container half of
`codec/mpc2sv8`).

## What lives here

* `cut` — the Rust equivalent of `mpccut`: sample-range cutting of SV8
  streams (new `SH` with cut-relative counts, zeroed `RG`, verbatim `EI`/`AP`
  copies, rebuilt seek table, `ST`/`SE`). No psychoacoustic encoding, no
  codec state; `AP` payloads are never re-encoded.

## What deliberately does NOT live here

* `mpc2sv8` (SV7 → SV8 conversion) is **not** ported: beyond the container
  writes (already covered by `FrameEncoder` + block writers), it requires
  **SV7 decoding**, which is decoder scope — the Rust decoder is SV8-only and
  no SV7 corpus can be produced by the current reference toolchain. See
  Phase 15J report §mpc2sv8.

## Licensing treatment (engineering boundary, not legal advice)

Source-derived from LGPL-2.1-or-later C sources → **LGPL-2.1-or-later**,
`publish = false`. See the encoder crate README for the full boundary.

## Testing

```sh
cargo test -p musicpack-mpc-tools
cargo clippy -p musicpack-mpc-tools --all-targets -- -D warnings
cargo check -p musicpack-mpc-tools --target wasm32-unknown-unknown
```

`tests/cut_compat.rs` replays 12 committed C-`mpccut` outputs byte-for-byte
(fixtures via `tools/gen_cut_fixtures.py`; tests never invoke C).
