# API spec errata — behaviour corrections against the reference specs

The legacy specifications under `specs/` in the reference repository are
immutable, and the **reference implementation is the behavioural authority**
(the same rule recorded in `docs/architecture.md` §8 and applied throughout
`docs/server-migration.md`). Where spec text and observed reference behaviour
disagree, the behaviour wins and the divergence is recorded here. Do not
"fix" the Rust implementation to match stale spec text.

## E-1. Waveform endpoints DO support Range (supersedes two spec sentences)

- Superseded text:
  - `specs/musicpack-api-v1.md:365-373` — for
    `GET /api/v1/tracks/{id}/waveform`: "…the payload is tiny … and is
    consumed whole, so **`Range` is intentionally not supported**."
  - `specs/musicpack-waveform-v1.md:373-374` — "416 / Range not supported
    (waveform assets are ≤ 1.5 MiB; clients always consume them whole)."
- Observed reference behaviour: waveform objects are served by the *same*
  code path as audio and assets. `handle_stream` routes kind 2 (waveform)
  into `serve_object` exactly like audio (kind 1) and assets (kind 0)
  (`server/src/api.c:1340-1361`), and `serve_object` applies the full
  Range machinery to every object: `Range` lookup, `If-Range` gating,
  `Accept-Ranges: bytes`, 206 + `Content-Range`, 416 + `bytes */N`
  (`server/src/api.c:557-619`).
- Rust parity: `crates/musicpack-server` reproduces this, and the stage-5
  differential oracle pins waveform Range behaviour against the live C
  server (`tests/media_oracle.rs`, waveform matrix).
- **Correct statement**: waveform endpoints support the identical
  single-`bytes=` range contract as `/audio` (200 full / 206 partial /
  416 with `Content-Range: bytes */<size>`, `Accept-Ranges: bytes`,
  strong sha256 ETag, `If-Range`).
- Still-valid kernel of the superseded text: *clients* should consume
  waveforms whole — the payload is tiny and the shipped client does exactly
  that (`web/app/src/lib/playback/waveform.ts`). That is client guidance,
  not a server limitation.
