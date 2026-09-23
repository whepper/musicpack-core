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

## 3. Source discovery (album directory → draft)

`musicpack-author` turns a conventional album directory into a draft
(`scan::source_to_draft`). `inspect::open_to_draft` dispatches exactly
like the reference `inspect`: a regular `manifest.json` means an
existing package (§ package → draft), anything else is a fresh source
directory. Discovery reports *what files exist, what their tags say, in
what order* — validation, encoding, waveform, build, identity and
verification are the existing, unchanged stages:

```text
album directory ──scan──► draft JSON ──► §2 boundary ──► existing pipeline
```

**Walk.** Files at the root and files directly inside disc-named
subdirectories (`CD1`, `Disc-2`, … — the directory name wins over a
`DISCNUMBER` tag, as in the reference); other subdirectories and deeper
nesting are ignored; symlinks are skipped (`lstat` semantics). Audio
extensions, case-insensitive: `.flac`, `.wav`, `.mpc`. Formats the
pipeline cannot build (`.ogg`) are *not* discovered — a
discovered-but-unbuildable draft would only fail later. Assets: one
deterministic `front` cover from `cover|front|folder` × `jpg|jpeg|png`
(root before disc dirs, then name, then extension order),
`booklet.pdf`, `*.lrc`, `*.txt`/`*.md`.

**Tags.** FLAC → Vorbis comments (the core's claxon metadata walk, no
audio decode; malformed blocks fail closed), `.mpc` → trailing APEv2
text items (binary items such as cover art are skipped — embedded
artwork is not extracted here), WAV → none (the reference scan reads
none either). Tag reads are best-effort: a malformed tag leaves the
file untagged — filename inference still applies — rather than failing
discovery. The surface is the MVP mapping the draft model and identify
consume: album `ALBUM`, `ALBUMARTIST`, `DATE|YEAR`, `GENRE`,
`RELEASETYPE`/`MUSICBRAINZ_ALBUMTYPE`, label/catalogue, `BARCODE`,
MusicBrainz release/release-group ids; per track `TITLE`,
`TRACKNUMBER`, `ARTIST`, `COMPOSER`, `ISRC`, MusicBrainz
recording/track ids. Album metadata is a first-wins union across the
tracks in final order (a richer tag on a later file is never shadowed
by an absent one).

**Numbering and order.** `TRACKNUMBER` (APEv2 `Track`) wins, else the
filename's leading digits, else the track is unnumbered. A disc with
any unnumbered **or duplicate** track is renumbered `1..n` in sorted
order — the reference `import` behaviour; the reference `inspect`
preserved duplicates for its GUI to surface, but the Rust draft
validator has no duplicate check (the builder fails closed), so
discovery normalises instead of deferring a late failure. Sort: disc,
then track number (unnumbered last), then relative path. Output is
deterministic: repeated scans of the same tree are byte-identical.

**Not done here** — derived facts and fail-closed concerns stay where
they were: no stream probing (per-track `codec`/`sampleRate`/
`duration` hints are re-derived at build; rate/channel support fails
closed in the encode stage), no embedded-artwork extraction (§9), no
resampling, downmixing, hashing or MusicBrainz lookup (identify stays
an explicit user action), and no `waveformAnalysis`/`identity`/
`openedFrom` blocks (absent means "new draft"; the runtime defaults
waveforms on).

Intentional divergences from the legacy C scan:

| Area | Difference | Reason |
|---|---|---|
| Album artists | fall back to the tracks' `ARTIST` union when no `ALBUMARTIST` exists | the reference produced an invalid `no artist` draft for ordinary rips (its own harness patched albums manually); MusicPack discovers a valid, still-editable draft |
| Filename titles | never empty (`"01.flac"` → title `"01"`) | the reference yielded an empty-title validation error; discovery stays fail-valid |
| Extensions | accepted case-insensitively; `.ogg` not discovered | `.FLAC` is common on case-insensitive filesystems; the Rust pipeline cannot build `.ogg` |
| Walk depth | files directly under disc dirs only | conventional album layouts; the reference's arbitrary depth accepted `CD1/sub/…` edge cases |
| Duplicates | renumbered at discovery | see numbering above |

## 4. Encoding

Sources: FLAC and integer-PCM WAV (the reference's `encode-draft`
contract). The source is decoded with the core's native decoders into
interleaved **full-scale left-aligned 32-bit PCM** (the decoder's `read_s32`
contract) and encoded by `MusepackEncoder::encode_s32`; the result is a
complete SV8 stream. `AudioInfo::is_float` is rejected, channels are limited
to 1–2, and sample rates to 32/37.8/44.1/48 kHz.

Settings: quality (default `6.0`, the Author default; the UI offers
5/6/7/8). No other setting is part of the contract (the reference passes
only `--quality`).

**Quality surface (J.1 + J.2, closed):** the encoder covers the full SV8
quality surface of the reference — any finite `f32` quality clipped to
`[0,10]` at `44100`, `48000`, `37800` and `32000` Hz. The44 integer pairs
keep frozen C-oracle tables (permanent regression oracles); fractional
qualities are computed deterministically and proven byte-identical against
the sparse fractional corpus (`fractional_manifest.txt`). Non-finite
qualities (`NaN`, `±inf`) and non-SV8 sample rates still fail closed with a
typed `unsupported` error — non-finite rejection is intentional (C's `NaN`
behaviour is undefined), never a silent remap. The Author UI continues to
offer only integer q5/6/7/8.
**Source precision (J.6, closed):** no reduction to 16 bits remains —
the stage keeps the decoder's left-aligned `i32` samples and encodes
through `encode_s32`, so 8/16/24/32-bit integer sources keep their full
source precision end-to-end (converted once, with the reference
conversion's rounding, into the encoder's `f32` analysis buffers). Byte
parity against scalar C `mpcenc` 1.32.0 for 24/32-bit inputs is pinned
hermetically by the encoder's `wide_manifest.txt` corpus and
end-to-end by the Author's wide-PCM differential (below).

Compatibility: `tests/pipeline.rs` proves the Rust encoder is
**byte-identical** to `mpcenc` for a real 16-bit FLAC at q6/44100 (needs
the reference build and its external `flac` decoder; skipped otherwise),
and for a 24-bit WAV through the wide-PCM differential; the encoder's
committed corpus remains the authoritative bitstream evidence.

## 5. Waveform and analysis

Waveforms are generated from the packaged audio with the core
`WaveformAccumulator`: `analysis/waveform/<DD>-<TT>.wfm`, `peak-rms-u8`,
100 ms, −60 dB, 2 bytes/bucket. `points` and the SHA-256 are derived by the
core builder from the payload. There is no second waveform representation.

Loudness is **not** re-implemented: the core builder measures per-track and
album loudness (one concatenated program, `ITU-R BS.1770-5`) from the
staged audio. `--no-loudness` maps to `LoudnessMode::Omit`.

## 6. MusicBrainz

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

## 7. Package creation and `.mpak`

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

## 8. Legacy runtime boundary

The Rust pipeline does **not** invoke the C `musicpack` CLI, `mpcenc` or
`musicpack-sonic`, does not spawn any subprocess, and adds no HTTP client,
FFmpeg or `unsafe`. Since R4.3 the Author desktop application uses this
pipeline as its default runtime through the in-process `RustBackend`; the C
CLI survives only as a non-default development escape hatch
(`MUSICPACK_AUTHOR_LEGACY=1`) and as a test oracle. See
`docs/author-runtime.md` and `docs/adr/0012-author-runtime-cutover.md`.

## 9. Intentional differences from the C path

| Area | Difference | Reason |
|---|---|---|
| Encoder quality/rate matrix | unsupported pairs error instead of encoding | the Rust encoder has no frozen tables for them; documented gap, not a silent remap |
| Album loudness with mixed rate/channels | typed error | R4.1 robustness decision (the C mismeasures silently) |
| Embedded artwork | typed error; extract to a file first | not yet ported; fail-closed |
| `--sync-tags` (APEv2) | not implemented | optional tooling; not required for package correctness |
| Sonic analysis | out of the pipeline | no Rust equivalent; not required for the package contract |
| `representations[]` | materialized from draft source paths | the C CLI never staged them; R4.1 supports them |
| MusicBrainz transport | host-injected trait | keeps the crate dependency-free and testable |
