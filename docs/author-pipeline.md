# Rust authoring pipeline (R4.2)

Status: implemented (`crates/musicpack-author/`,
`crates/musicpack-server/tests/author_pipeline.rs`). Decision record:
`docs/adr/0011-authoring-pipeline.md`.

This document describes the pipeline that replaces the functional
responsibilities of the legacy C `musicpack` authoring CLI. The existing
Author runtime still uses the C sidecars; R4.3 performs the cutover.

## 1. Architecture

```text
                 draft JSON
                     │  draft::parse / draft::validate
                     ▼
     identify (optional) ── MusicBrainzProvider (transport injected by host)
                     │  identify::match_confidence / apply_release
                     ▼
     encode / pass-through ── musicpack-musepack-encoder (isolated LGPL)
                     │        core audio decoders (WAV/FLAC/Musepack)
                     ▼
     waveform envelopes ── core WaveformAccumulator
                     │
                     ▼
     AuthoringDraft ── musicpack_core::authoring::build_directory
                     │        (path safety, hashing, canonical manifest,
                     │         loudness, identity, core verification)
                     ▼
                  .mpack ── storage::directory::pack_directory ──► .mpak
```

Crate boundaries:

| Concern | Owner |
|---|---|
| package/domain models, validation, canonical manifest, identity, verification, lyric model, `.mpak` format, decoders, waveform/loudness kernels | `musicpack-core` |
| authoring orchestration, JSON draft, encoding invocation, waveform/loudness orchestration, MusicBrainz, `.mpak` invocation, CLI | `musicpack-author` |
| MusicBrainz HTTP transport | the host (Tauri app, R4.3); `MusicBrainzProvider` in this crate |
| Musepack encoding | `musicpack-musepack-encoder` (LGPL, isolated) |

The pipeline never writes `manifest.json`, never duplicates a verifier, and
never spawns a legacy binary.

## 2. JSON draft boundary

The draft schema mirrors the reference (`schema: "musicpack-draft"`,
`version: 1`, `sourceRoot`, `album`/`release`/`identifiers`/`identity`/
`source`, `media[].tracks[]`, `artwork`/`booklet`/`lyrics`/`extras`), plus
the R3.5 per-track `lyrics: [{path, lang?}]` extension.

**Authored** fields are captured; **derived** artifacts (`waveformAnalysis`,
`sonicAnalysis`, per-track `duration`/`sampleRate`/`codec` hints, any
`sha256`) are ignored and re-derived. Supplying them has no effect on the
output, so a draft cannot inject a hash, fingerprint, key, measurement,
point count or verification state.

Source paths (`audioPath`, artwork/booklet/lyrics/extras) are relative to
`sourceRoot`, validated with the core package-path rules and resolved with
containment (`..`/absolute escapes rejected).

`validate` ports the reference structural rules and returns
`{ok, errors, warnings}`. The authoritative gate is still the core builder,
which re-validates the assembled manifest and the verifier.

CLI:

```sh
musicpack-author validate <draft.json> [--json]
musicpack-author identify <draft.json> [--mb-json FILE] [--mbid UUID]
                                       [--mb-search-json FILE] [--json]
musicpack-author build    <draft.json> -o DIR [--mpak FILE] [--quality Q]
                                       [--no-waveform] [--no-loudness]
                                       [--replace] [--mb-json FILE] [--json]
```

## 3. Encoding

Sources: FLAC and integer-PCM WAV (the reference's `encode-draft`
contract). The source is decoded with the core's native decoders, reduced to
interleaved 16-bit PCM, and encoded by `MusepackEncoder`; the result is a
complete SV8 stream. `AudioInfo::is_float` is rejected, channels are limited
to 1–2, and sample rates to 32/37.8/44.1/48 kHz.

Settings: quality (default `6.0`, the Author default; the UI offers
5/6/7/8). No other setting is part of the contract (the reference passes
only `--quality`).

**Documented gap:** the encoder's frozen psychoacoustic tables exist only
for `{4,5,6,7} @ 44100` and `q5 @ {48000, 37800, 32000}`. Every other
`(quality, sample-rate)` pair fails closed with a typed error rather than
being silently remapped; expanding the tables is encoder-crate work.
Sources deeper than 16 bits are reduced to the top 16 bits (exact for
16-bit); the reference passes the source bit depth to `mpcenc`.

Compatibility: `tests/pipeline.rs` proves the Rust encoder is
**byte-identical** to `mpcenc` for a real 16-bit FLAC at q6/44100 (skipped
if `mpcenc` is absent), and the encoder's committed corpus remains the
authoritative bitstream evidence.

## 4. Waveform and analysis

Waveforms are generated from the packaged audio with the core
`WaveformAccumulator`: `analysis/waveform/<DD>-<TT>.wfm`, `peak-rms-u8`,
100 ms, −60 dB, 2 bytes/bucket. `points` and the SHA-256 are derived by the
core builder from the payload. There is no second waveform representation.

Loudness is **not** re-implemented: the core builder measures per-track and
album loudness (one concatenated program, `ITU-R BS.1770-5`) from the
staged audio. `--no-loudness` maps to `LoudnessMode::Omit`.

## 5. MusicBrainz

Transport is the host's concern; matching is this crate's:

```text
MusicBrainzProvider (fetch_release / search_barcode)
        │  raw JSON
        ▼
identify::parse_release → match_confidence → apply_release
```

Matching ports the reference ladder exactly (no new algorithm):
release-id equality → `exact`; barcode equality, or ISRC hit with an equal
track count → `confirmed`; ISRC hit or exact title → `probable`; otherwise
`none`. Artist and date do not participate. A barcode search returns
**all** candidates and never auto-selects; application requires an explicit
release id or an offline document, and `none` applies nothing.

The provider is a trait, so tests use a deterministic static provider; the
Tauri host supplies the live `ureq` transport at R4.3.

## 6. Package creation and `.mpak`

The pipeline assembles a
`musicpack_core::authoring::AuthoringDraft` and calls
`build_directory(draft, work_tree, output, options)` — the sole package
constructor. The builder validates paths, hashes assets, writes the
canonical manifest, measures loudness, verifies through the shared core
verifier, and publishes atomically. `.mpak` uses the existing
`storage::directory::pack_directory`; the R3.5 lyric-specific branch is no
longer needed because the Rust builder materializes every manifest-known
asset (lyrics included) before packing.

`--replace` is implemented at the application boundary: the new package is
built in a staging directory and swapped in atomically (with rollback); the
core builder remains a pure constructor, never an in-place editor.

## 7. Legacy runtime boundary

The Rust pipeline does **not** invoke the C `musicpack` CLI, `mpcenc` or
`musicpack-sonic`, does not spawn any subprocess, and adds no HTTP client,
FFmpeg or `unsafe`. Since R4.3 the Author desktop application uses this
pipeline as its default runtime through the in-process `RustBackend`; the C
CLI survives only as a non-default development escape hatch
(`MUSICPACK_AUTHOR_LEGACY=1`) and as a test oracle. See
`docs/author-runtime.md` and `docs/adr/0012-author-runtime-cutover.md`.

## 8. Intentional differences from the C path

| Area | Difference | Reason |
|---|---|---|
| Encoder quality/rate matrix | unsupported pairs error instead of encoding | the Rust encoder has no frozen tables for them; documented gap, not a silent remap |
| Source bit depth | >16-bit reduced to top 16 bits | current encoder API consumes `i16`; documented fidelity gap |
| Album loudness with mixed rate/channels | typed error | R4.1 robustness decision (the C mismeasures silently) |
| Embedded artwork | typed error; extract to a file first | not yet ported; fail-closed |
| `--sync-tags` (APEv2) | not implemented | optional tooling; not required for package correctness |
| Sonic analysis | out of the pipeline | no Rust equivalent; not required for the package contract |
| `representations[]` | materialized from draft source paths | the C CLI never staged them; R4.1 supports them |
| MusicBrainz transport | host-injected trait | keeps the crate dependency-free and testable |
