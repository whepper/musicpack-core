//! Audio primitives: fixture parity, reference PCM differential, hostile WAV
//! input, and the WAVE_FORMAT_EXTENSIBLE matrix.
//!
//! Fixtures are the reference repository's committed
//! `tests/fixtures/audio/` set, vendored unmodified into
//! `fixtures/reference/audio/` (provenance: `fixtures/README.md`). The
//! expectations below are the Rust port of the reference's `audio_tests.c`
//! assertions.
//!
//! The FLAC PCM digests in `tests/data/audio_c_reference.txt` were produced
//! by decoding the same fixtures with the reference's **vendored dr_flac**
//! through `tools/audio_probe.c` (the same decoder `core/libmusicpack` uses),
//! so the `claxon`-backed adapter is compared against the reference decoder
//! rather than against assumptions.

use std::io::Cursor;
use std::path::PathBuf;

use musicpack_core::Error;
use musicpack_core::audio::{self, AudioDecoder, Codec};
use musicpack_core::format::checksum::sha256_hex;

fn audio_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/reference/audio")
}

fn fixture(name: &str) -> Vec<u8> {
    let path = audio_dir().join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn open_bytes(bytes: Vec<u8>) -> Result<Box<dyn AudioDecoder>, Error> {
    audio::open(Box::new(Cursor::new(bytes)))
}

fn open(name: &str) -> Box<dyn AudioDecoder> {
    open_bytes(fixture(name)).unwrap_or_else(|e| panic!("opening {name}: {e}"))
}

/// Decodes a whole stream into interleaved f32, asserting clean EOF.
fn decode_f32(dec: &mut dyn AudioDecoder) -> Vec<f32> {
    let channels = dec.info().channels as usize;
    let mut out = Vec::new();
    let mut buf = vec![0.0f32; 1152 * channels];
    loop {
        let n = dec.read_f32(&mut buf).expect("f32 decode");
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n * channels]);
    }
    out
}

/// Decodes a whole stream into interleaved left-aligned s32.
fn decode_s32(dec: &mut dyn AudioDecoder) -> Vec<i32> {
    let channels = dec.info().channels as usize;
    let mut out = Vec::new();
    let mut buf = vec![0i32; 1152 * channels];
    loop {
        let n = dec.read_s32(&mut buf).expect("s32 decode");
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n * channels]);
    }
    out
}

fn frames(dec: &mut dyn AudioDecoder) -> u64 {
    decode_f32(dec).len() as u64 / dec.info().channels as u64
}

/// Interleaved `s32`/`f32` streams as the canonical little-endian byte
/// representation hashed by `tools/audio_probe.c`.
fn s32_bytes(samples: &[i32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

fn f32_bytes(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

// --- fixture format + frame-count parity (audio_tests.c) -------------------

#[test]
fn flac_fixture_formats_and_frame_counts() {
    let cases: &[(&str, u32, u8, u8, u64)] = &[
        ("flac16-44k.flac", 44100, 2, 16, 88200),
        ("flac24-48k.flac", 48000, 2, 24, 96000),
        ("flac24-96k.flac", 96000, 2, 24, 192000),
        ("flac-mono-44k.flac", 44100, 1, 16, 88200),
    ];
    for &(name, rate, channels, bits, total) in cases {
        let mut dec = open(name);
        let info = dec.info().clone();
        assert_eq!(info.codec, Codec::Flac, "{name}");
        assert_eq!(info.sample_rate, rate, "{name}");
        assert_eq!(info.channels, channels, "{name}");
        assert_eq!(info.bits_per_sample, bits, "{name}");
        assert_eq!(info.total_frames, Some(total), "{name}");
        assert!(!info.is_float, "{name}");
        assert_eq!(frames(&mut *dec), total, "{name} fully decodes");
    }
}

#[test]
fn wav_fixture_formats_and_frame_counts() {
    let cases: &[(&str, u8, u8, Option<u64>)] = &[
        ("wav16-44k.wav", 16, 2, Some(88200)),
        ("wav24-44k.wav", 24, 2, Some(88200)),
        ("wav24-ext.wav", 24, 2, Some(88200)),
    ];
    for &(name, bits, channels, total) in cases {
        let mut dec = open(name);
        let info = dec.info().clone();
        assert_eq!(info.codec, Codec::Wav, "{name}");
        assert_eq!(info.sample_rate, 44100, "{name}");
        assert_eq!(info.channels, channels, "{name}");
        assert_eq!(info.bits_per_sample, bits, "{name}");
        assert_eq!(info.total_frames, total, "{name}");
        assert!(!info.is_float, "{name}");
        assert_eq!(frames(&mut *dec), total.unwrap(), "{name}");
    }

    let mut dec = open("wav-float.wav");
    let info = dec.info().clone();
    assert_eq!(info.codec, Codec::Wav);
    assert_eq!(info.bits_per_sample, 32);
    assert!(info.is_float);
    assert_eq!(info.total_frames, Some(88200));
    assert_eq!(frames(&mut *dec), 88200);
}

#[test]
fn flac_long_fixture_reports_its_format() {
    // 30-minute silent stress fixture: format-only check, no full decode.
    let mut dec = open("flac-long-48k.flac");
    let info = dec.info().clone();
    assert_eq!(info.codec, Codec::Flac);
    assert_eq!(info.sample_rate, 48000);
    assert_eq!(info.channels, 1);
    assert_eq!(info.bits_per_sample, 16);
    assert_eq!(info.total_frames, Some(172_800_000));
    let mut buf = [0i32; 1152];
    assert_eq!(dec.read_s32(&mut buf).unwrap(), 1152);
}

// --- reference PCM differential -------------------------------------------

#[test]
fn flac_pcm_matches_the_reference_dr_flac_digests() {
    let table = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/audio_c_reference.txt"),
    )
    .expect("committed reference digest table");
    let mut checked = 0;
    for line in table.lines().filter(|l| !l.trim().is_empty()) {
        let mut parts = line.split_whitespace();
        let name = parts.next().expect("fixture name");
        let mode = parts.next().expect("mode");
        let want = parts.next().expect("digest");
        assert!(
            parts.next().is_none(),
            "unexpected trailing field in {line:?}"
        );
        let got = match mode {
            "s32" => {
                let mut dec = open(name);
                sha256_hex(&s32_bytes(&decode_s32(&mut *dec)))
            }
            "f32" => {
                let mut dec = open(name);
                sha256_hex(&f32_bytes(&decode_f32(&mut *dec)))
            }
            other => panic!("unknown mode {other}"),
        };
        assert_eq!(got, want, "{name} {mode} must match the reference decoder");
        checked += 1;
    }
    assert_eq!(checked, 8, "four fixtures times two representations");
}

#[test]
fn flac_24bit_s32_f32_round_trip_is_exact() {
    // Port of audio_tests.c: round(f32 * 2^23) << 8 must be within ±1 LSB of s32.
    for name in ["flac24-48k.flac", "flac24-96k.flac"] {
        let s32 = {
            let mut dec = open(name);
            decode_s32(&mut *dec)
        };
        let f32 = {
            let mut dec = open(name);
            decode_f32(&mut *dec)
        };
        assert_eq!(s32.len(), f32.len(), "{name}");
        for (i, (&s, &f)) in s32.iter().zip(f32.iter()).enumerate() {
            let back = (f as f64 * 8388608.0).round();
            let want = (back as i32) << 8;
            assert!(
                (s as i64 - want as i64).abs() <= 1,
                "{name} sample {i}: s32={s} f32-derived={want}"
            );
        }
    }
}

// --- WAV rejection / truncation -------------------------------------------

#[test]
fn wav_adpcm_and_garbage_are_rejected() {
    let Err(err) = open_bytes(fixture("wav-adpcm.wav")) else {
        panic!("ADPCM WAV must be rejected");
    };
    assert!(matches!(err, Error::Invalid { .. }), "ADPCM: {err:?}");

    let Err(err) = open_bytes(b"this is not a RIFF file at all".to_vec()) else {
        panic!("garbage must be rejected");
    };
    assert!(matches!(err, Error::Invalid { .. }), "garbage: {err:?}");
}

#[test]
fn wav_truncated_preserves_declared_total_and_decodes_what_exists() {
    let mut dec = open("wav-truncated.wav");
    assert_eq!(dec.info().total_frames, Some(88200));
    let got = frames(&mut *dec);
    assert!(got > 0 && got < 88200, "short decode, got {got}");
}

#[test]
fn destination_length_must_be_a_multiple_of_channels() {
    let mut wav = open("wav16-44k.wav"); // stereo
    assert!(wav.read_f32(&mut [0.0f32; 3]).is_err());
    assert!(wav.read_s32(&mut [0i32; 3]).is_err());
    // An empty destination is a clean zero-frame read, not an error.
    assert_eq!(wav.read_f32(&mut []).unwrap(), 0);

    let mut flac = open("flac16-44k.flac"); // stereo
    assert!(flac.read_s32(&mut [0i32; 5]).is_err());
}

#[test]
fn float_wav_s32_reads_are_unsupported() {
    let mut dec = open("wav-float.wav");
    let err = dec.read_s32(&mut [0i32; 8]).unwrap_err();
    assert!(matches!(err, Error::Unsupported { .. }), "{err:?}");
}

// --- WAVE_FORMAT_EXTENSIBLE matrix (D11) ----------------------------------

/// The reference-specific sub-format GUID tail (see discrepancy D11).
const EXT_TAIL: [u8; 12] = [
    0x00, 0x00, 0x00, 0x00, 0x10, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b,
];

fn wav_with_fmt(
    tag: u16,
    channels: u16,
    rate: u32,
    align: u16,
    bits: u16,
    extra: &[u8],
    data: &[u8],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&(16u32 + extra.len() as u32).to_le_bytes());
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(extra);
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    out
}

fn ext_extra(
    valid_bits: u16,
    mask: [u8; 4],
    unchecked: [u8; 2],
    guid_tail: &[u8; 12],
    tag: [u8; 2],
) -> Vec<u8> {
    // cbSize(2) + validBits(2) + channelMask(4) + sub-format GUID(16).
    // The GUID is tag(2) + two unchecked bytes + the 12-byte tail.
    let mut extra = vec![22, 0];
    extra.extend_from_slice(&valid_bits.to_le_bytes());
    extra.extend_from_slice(&mask);
    extra.extend_from_slice(&tag);
    extra.extend_from_slice(&unchecked);
    extra.extend_from_slice(guid_tail);
    extra
}

fn stereo_pcm16_data() -> Vec<u8> {
    let mut data = Vec::new();
    for v in [-32768i16, 32767, 0, 1] {
        data.extend_from_slice(&v.to_le_bytes());
    }
    data
}

#[test]
fn extensible_matrix() {
    let data = stereo_pcm16_data();
    let mask = [0x03, 0x00, 0x00, 0x00];

    // Reference tail GUID → accepted.
    let extra = ext_extra(16, mask, [0, 0], &EXT_TAIL, [1, 0]);
    let dec = open_bytes(wav_with_fmt(0xFFFE, 2, 44100, 4, 16, &extra, &data)).unwrap();
    assert_eq!(dec.info().bits_per_sample, 16);

    // Canonical Windows PCM GUID → rejected in phase 8 (D11).
    let canonical = [
        0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
    ];
    let extra = ext_extra(16, mask, [0, 0], &canonical, [1, 0]);
    assert!(open_bytes(wav_with_fmt(0xFFFE, 2, 44100, 4, 16, &extra, &data)).is_err());

    // Non-PCM sub-format tag → rejected.
    let extra = ext_extra(16, mask, [0, 0], &EXT_TAIL, [0x50, 0x00]);
    assert!(open_bytes(wav_with_fmt(0xFFFE, 2, 44100, 4, 16, &extra, &data)).is_err());

    // The two unchecked GUID bytes (fmt+26..28) may be arbitrary.
    let extra = ext_extra(16, mask, [0xAB, 0xCD], &EXT_TAIL, [1, 0]);
    assert!(
        open_bytes(wav_with_fmt(0xFFFE, 2, 44100, 4, 16, &extra, &data)).is_ok(),
        "bytes fmt+26..28 are unchecked"
    );

    // validBits differing from the container and arbitrary channel masks are
    // both ignored/accepted.
    let extra = ext_extra(20, [0xDE, 0xAD, 0xBE, 0xEF], [0, 0], &EXT_TAIL, [1, 0]);
    let dec = open_bytes(wav_with_fmt(0xFFFE, 2, 44100, 4, 16, &extra, &data)).unwrap();
    assert_eq!(dec.info().bits_per_sample, 16);
    let extra = ext_extra(16, [0, 0, 0, 0], [0, 0], &EXT_TAIL, [1, 0]);
    assert!(open_bytes(wav_with_fmt(0xFFFE, 2, 44100, 4, 16, &extra, &data)).is_ok());
}

#[test]
fn wav24_ext_fixture_decodes_with_the_reference_guid() {
    let mut dec = open("wav24-ext.wav");
    assert_eq!(dec.info().bits_per_sample, 24);
    assert_eq!(frames(&mut *dec), 88200);
}

// --- hostile WAV input ----------------------------------------------------

fn valid_wav() -> Vec<u8> {
    wav_with_fmt(
        1,
        2,
        44100,
        4,
        16,
        &[],
        &[0u8; 16], // four stereo frames
    )
}

#[test]
fn riffs_and_wave_magic_are_required() {
    let mut b = valid_wav();
    b[0] = b'X';
    assert!(open_bytes(b).is_err());

    let mut b = valid_wav();
    b[8] = b'X';
    assert!(open_bytes(b).is_err());

    assert!(open_bytes(b"WAVE....".to_vec()).is_err());
}

#[test]
fn chunk_magic_and_ordering_rules() {
    // Corrupt fmt magic: the chunk becomes unknown, so `data` precedes any
    // accepted fmt → Invalid.
    let mut b = valid_wav();
    b[12] = b'x';
    assert!(open_bytes(b).is_err());

    // Corrupt data magic: the data chunk is skipped, then EOF → Invalid.
    let mut b = valid_wav();
    let data_pos = 12 + 8 + 16;
    b[data_pos] = b'x';
    assert!(open_bytes(b).is_err());

    // data before fmt.
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF\0\0\0\0WAVE");
    b.extend_from_slice(b"data");
    b.extend_from_slice(&4u32.to_le_bytes());
    b.extend_from_slice(&[0u8; 4]);
    b.extend_from_slice(b"fmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&44100u32.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    assert!(open_bytes(b).is_err());

    // Missing data chunk (fmt only).
    let mut b = valid_wav();
    b.truncate(12 + 8 + 16);
    assert!(open_bytes(b).is_err());

    // fmt shorter than 16 bytes.
    let mut short = Vec::new();
    short.extend_from_slice(b"RIFF\0\0\0\0WAVE");
    short.extend_from_slice(b"fmt ");
    short.extend_from_slice(&15u32.to_le_bytes());
    short.extend_from_slice(&[0u8; 15]);
    short.extend_from_slice(b"data");
    short.extend_from_slice(&0u32.to_le_bytes());
    assert!(open_bytes(short).is_err());
}

#[test]
fn multiple_fmt_chunks_last_one_wins() {
    // First fmt says 8-bit, second says 16-bit; the data is 16-bit stereo.
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF\0\0\0\0WAVE");
    let push_fmt = |b: &mut Vec<u8>, bits: u16, align: u16| {
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&44100u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&align.to_le_bytes());
        b.extend_from_slice(&bits.to_le_bytes());
    };
    push_fmt(&mut b, 8, 2);
    push_fmt(&mut b, 16, 4);
    b.extend_from_slice(b"data");
    b.extend_from_slice(&8u32.to_le_bytes());
    b.extend_from_slice(&[0u8; 8]);
    let dec = open_bytes(b).unwrap();
    assert_eq!(dec.info().bits_per_sample, 16, "last fmt wins");
}

#[test]
fn chunk_sizes_beyond_eof_are_rejected_or_trusted() {
    // A `fmt ` chunk size whose payload extends past EOF is Invalid.
    let mut b = valid_wav();
    b[16..20].copy_from_slice(&1000u32.to_le_bytes());
    assert!(open_bytes(b).is_err());

    // A `data` chunk size that exceeds the file is the declared length; the
    // stream opens and short-reads (truncated-data semantics).
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF\0\0\0\0WAVE");
    b.extend_from_slice(b"fmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&44100u32.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes()); // block align
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&200u32.to_le_bytes()); // declares 100 frames
    b.extend_from_slice(&[0u8; 4]); // only 2 frames present
    let mut dec = open_bytes(b).unwrap();
    assert_eq!(dec.info().total_frames, Some(100));
    let mut buf = [0i32; 16];
    assert_eq!(dec.read_s32(&mut buf).unwrap(), 2);
    assert_eq!(dec.read_s32(&mut buf).unwrap(), 0);

    // Unknown chunk with an overflowing (0xFFFFFFFF) size must terminate
    // cleanly, not wrap.
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF\0\0\0\0WAVE");
    b.extend_from_slice(b"junk");
    b.extend_from_slice(&u32::MAX.to_le_bytes());
    assert!(open_bytes(b).is_err());
}

#[test]
fn unknown_odd_sized_chunks_are_skipped_with_padding() {
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF\0\0\0\0WAVE");
    // A 3-byte unknown chunk followed by its pad byte.
    b.extend_from_slice(b"junk");
    b.extend_from_slice(&3u32.to_le_bytes());
    b.extend_from_slice(&[1, 2, 3, 0]);
    // Then a valid fmt + data.
    let tail = wav_with_fmt(1, 1, 44100, 2, 16, &[], &[0u8; 4]);
    b.extend_from_slice(&tail[12..]);
    let dec = open_bytes(b).unwrap();
    assert_eq!(dec.info().bits_per_sample, 16);
}

#[test]
fn format_field_validation() {
    let data = [0u8; 8];
    // Channels 0, 9, 255 rejected.
    for channels in [0u16, 9, 255] {
        let align = channels * 2;
        assert!(
            open_bytes(wav_with_fmt(1, channels, 44100, align, 16, &[], &data)).is_err(),
            "channels {channels}"
        );
    }
    // Sample rate 0 rejected; a huge non-zero rate is valid metadata.
    assert!(open_bytes(wav_with_fmt(1, 1, 0, 2, 16, &[], &data)).is_err());
    let huge = open_bytes(wav_with_fmt(1, 1, u32::MAX, 2, 16, &[], &data)).unwrap();
    assert_eq!(huge.info().sample_rate, u32::MAX);
    // block_align mismatch.
    assert!(open_bytes(wav_with_fmt(1, 2, 44100, 3, 16, &[], &data)).is_err());
    // 24-bit in a 32-bit container (align 8, bits 24) is rejected.
    assert!(open_bytes(wav_with_fmt(1, 2, 44100, 8, 24, &[], &data)).is_err());
    // ADPCM tag 2 and float with a non-32 bit depth are rejected.
    assert!(open_bytes(wav_with_fmt(2, 1, 44100, 5, 4, &[], &data)).is_err());
    assert!(open_bytes(wav_with_fmt(3, 1, 44100, 4, 16, &[], &data)).is_err());
    // Extensible fmt declared shorter than 40 bytes is rejected.
    let extra = [0u8; 8]; // 16 + 8 = 24 < 40
    assert!(open_bytes(wav_with_fmt(0xFFFE, 1, 44100, 2, 16, &extra, &data)).is_err());
}

#[test]
fn truncation_at_every_offset_is_a_clean_result() {
    let full = valid_wav();
    // Layout: 12-byte header, 24-byte fmt chunk, then the data chunk header
    // (8 bytes) at offset 36..44. Anything shorter than a complete data
    // chunk header cannot open; from 44 on, the declared-but-absent payload
    // is the reference's truncated-data success path.
    for len in 0..full.len() {
        let bytes = full[..len].to_vec();
        let result = open_bytes(bytes);
        if len < 44 {
            assert!(result.is_err(), "truncation at {len} must fail");
        } else {
            let mut dec = result.unwrap_or_else(|e| panic!("truncation at {len}: {e}"));
            // A short stream must end at Ok(0), never panic.
            let mut buf = [0.0f32; 16];
            while dec.read_f32(&mut buf).unwrap() > 0 {}
        }
    }
    // The untruncated stream opens and decodes.
    let dec = open_bytes(full).unwrap();
    assert_eq!(dec.info().total_frames, Some(4));
}

#[test]
fn open_sniffs_magic_not_extension() {
    // RIFF/WAVE and fLaC are recognized by content; anything else is Invalid.
    assert!(open_bytes(valid_wav()).is_ok());
    assert!(open_bytes(fixture("flac16-44k.flac")).is_ok());
    assert!(matches!(
        open_bytes(b"ID3\x04\x00\x00\x00\x00\x00\x00fLaC".to_vec()),
        Err(Error::Invalid { .. })
    ));
}

// ---------------------------------------------------------------------
// Tag readers for fresh-album discovery: FLAC Vorbis comments and the
// trailing APEv2 tag of `.mpc` pass-through sources.
// ---------------------------------------------------------------------

/// Replaces the fixture's `VORBIS_COMMENT` block payload with the given
/// comments (the block's position and the frames stay untouched; only
/// its length bytes and payload change).
fn flac_with_comments(comments: &[(&str, &str)]) -> Vec<u8> {
    let bytes = fixture("flac16-44k.flac");
    assert_eq!(&bytes[0..4], b"fLaC");

    // Locate the existing VORBIS_COMMENT block (type 4).
    let mut offset = 4usize;
    let (payload_at, frames_at) = loop {
        assert!(offset + 4 <= bytes.len(), "truncated metadata block header");
        let header = bytes[offset];
        let len = u32::from_be_bytes([0, bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
            as usize;
        let next = offset + 4 + len;
        assert!(next <= bytes.len(), "truncated metadata block payload");
        if header & 0x7f == 4 {
            break (offset + 4, next);
        }
        assert_eq!(header & 0x80, 0, "fixture must contain a comment block");
        offset = next;
    };

    let mut payload = Vec::new();
    payload.extend_from_slice(&9u32.to_le_bytes()); // vendor length
    payload.extend_from_slice(b"musicpack");
    payload.extend_from_slice(&(comments.len() as u32).to_le_bytes());
    for (key, value) in comments {
        let entry = format!("{key}={value}");
        payload.extend_from_slice(&(entry.len() as u32).to_le_bytes());
        payload.extend_from_slice(entry.as_bytes());
    }

    let mut out = Vec::with_capacity(bytes.len() + payload.len());
    out.extend_from_slice(&bytes[..payload_at - 3]); // through the block header byte
    let len = payload.len();
    out.extend_from_slice(&[(len >> 16) as u8, (len >> 8) as u8, len as u8]);
    out.extend_from_slice(&payload);
    out.extend_from_slice(&bytes[frames_at..]);
    out
}

/// A FLAC whose `VORBIS_COMMENT` block claims an entry far longer than
/// the block itself — structurally malformed metadata.
fn flac_with_malformed_comments() -> Vec<u8> {
    let mut tagged = flac_with_comments(&[("TITLE", "Alpha")]);
    // payload = vendor_len(4) + vendor(9) + count(4) + entry_len(4) …
    let entry_len_at = {
        let mut offset = 4usize;
        loop {
            let block_type = tagged[offset] & 0x7f;
            let len = u32::from_be_bytes([
                0,
                tagged[offset + 1],
                tagged[offset + 2],
                tagged[offset + 3],
            ]) as usize;
            if block_type == 4 {
                let vendor_len = u32::from_le_bytes([
                    tagged[offset + 4],
                    tagged[offset + 5],
                    tagged[offset + 6],
                    tagged[offset + 7],
                ]) as usize;
                break offset + 4 + 4 + vendor_len + 4;
            }
            offset += 4 + len;
        }
    };
    tagged[entry_len_at..entry_len_at + 4].copy_from_slice(&0x00FF_FFFFu32.to_le_bytes());
    tagged
}

#[test]
fn flac_vorbis_comments_round_trip_without_decoding() {
    let tagged = flac_with_comments(&[
        ("TITLE", "Alpha"),
        ("ARTIST", "A & B"),
        ("title", "duplicate"),
    ]);
    let tags = audio::flac::read_vorbis_comments(Box::new(Cursor::new(tagged))).unwrap();
    assert_eq!(
        tags,
        vec![
            ("TITLE".to_string(), "Alpha".to_string()),
            ("ARTIST".to_string(), "A & B".to_string()),
            // Original casing is preserved; the scan layer upper-cases.
            ("title".to_string(), "duplicate".to_string()),
        ]
    );

    // The untagged fixture still carries only its original single
    // comment (`encoder=Lavf63.1.101`, written by the encoder that made
    // it) — reading never errors and never invents entries.
    let plain = fixture("flac16-44k.flac");
    assert_eq!(
        audio::flac::read_vorbis_comments(Box::new(Cursor::new(plain))).unwrap(),
        vec![("encoder".to_string(), "Lavf63.1.101".to_string())]
    );
}

#[test]
fn flac_vorbis_comments_fail_closed_on_malformed_blocks() {
    let err =
        audio::flac::read_vorbis_comments(Box::new(Cursor::new(flac_with_malformed_comments())))
            .unwrap_err();
    assert!(!err.to_string().is_empty());
}

/// An APEv2 tag: optional 32-byte header + items + 32-byte footer.
fn apev2_bytes(items: &[(&str, &[u8], u32)]) -> Vec<u8> {
    fn footer(tag_size: u32, count: u32, flags: u32) -> Vec<u8> {
        let mut f = b"APETAGEX".to_vec();
        f.extend_from_slice(&2000u32.to_le_bytes()); // APEv2.00
        f.extend_from_slice(&tag_size.to_le_bytes());
        f.extend_from_slice(&count.to_le_bytes());
        f.extend_from_slice(&flags.to_le_bytes());
        f.extend_from_slice(&[0u8; 8]);
        f
    }
    let mut body = Vec::new();
    for (key, value, flags) in items {
        body.extend_from_slice(&(value.len() as u32).to_le_bytes());
        body.extend_from_slice(&flags.to_le_bytes());
        body.extend_from_slice(key.as_bytes());
        body.push(0);
        body.extend_from_slice(value);
    }
    let tag_size = (64 + body.len()) as u32;
    let count = items.len() as u32;
    let mut out = footer(tag_size, count, 0x2000_0000 | 0x4000_0000); // is-header|has-footer
    out.extend_from_slice(&body);
    out.extend_from_slice(&footer(tag_size, count, 0x8000_0000 | 0x4000_0000)); // has-header|has-footer
    out
}

#[test]
fn apev2_text_tags_round_trip_from_the_file_tail() {
    let mut data = b"not a real stream, but seekable bytes".to_vec();
    data.extend(apev2_bytes(&[
        ("Title", b"Alpha".as_slice(), 0),
        ("Track", b"3".as_slice(), 0),
    ]));
    let mut cur = Cursor::new(data);
    let tags = audio::musepack::apev2::read_tags(&mut cur).unwrap();
    assert_eq!(
        tags,
        vec![
            ("Title".to_string(), "Alpha".to_string()),
            ("Track".to_string(), "3".to_string()),
        ]
    );
}

#[test]
fn apev2_absent_binary_non_utf8_and_malformed() {
    // No tag at the tail → empty, not an error.
    let mut plain = Cursor::new(b"plain bytes".to_vec());
    assert!(
        audio::musepack::apev2::read_tags(&mut plain)
            .unwrap()
            .is_empty()
    );

    // Binary items (cover art) and non-UTF-8 text items are skipped;
    // valid text items around them are kept, in tag order.
    let mut data = b"payload".to_vec();
    data.extend(apev2_bytes(&[
        (
            "Cover Art (Front)",
            b"folder.jpg\0\xff\xd8binary".as_slice(),
            0x8000_0000, // binary
        ),
        ("Comment", [0xffu8, 0xfe].as_slice(), 0), // not UTF-8
        ("Title", b"Gamma".as_slice(), 0),
    ]));
    let mut cur = Cursor::new(data);
    assert_eq!(
        audio::musepack::apev2::read_tags(&mut cur).unwrap(),
        vec![("Title".to_string(), "Gamma".to_string())]
    );

    // A footer claiming more items than the tag holds → truncated → error.
    let mut item = Vec::new();
    item.extend_from_slice(&1u32.to_le_bytes()); // value size
    item.extend_from_slice(&0u32.to_le_bytes()); // flags (text)
    item.extend_from_slice(b"Title\0");
    item.extend_from_slice(b"x");
    let tag_size = (64 + item.len()) as u32; // header + one item + footer
    let mut f = b"APETAGEX".to_vec();
    f.extend_from_slice(&2000u32.to_le_bytes());
    f.extend_from_slice(&tag_size.to_le_bytes());
    f.extend_from_slice(&5u32.to_le_bytes()); // claims 5 items, holds 1
    f.extend_from_slice(&0x4000_0000u32.to_le_bytes()); // has-footer, no header
    f.extend_from_slice(&[0u8; 8]);
    let mut truncated = b"payload".to_vec();
    truncated.extend_from_slice(&f); // leading copy (no-header layout: items start at tag_start)
    truncated.extend_from_slice(&item);
    truncated.extend_from_slice(&f); // footer
    let mut cur = Cursor::new(truncated);
    assert!(audio::musepack::apev2::read_tags(&mut cur).is_err());

    // An unsupported footer version → error.
    let mut bad_version = b"payload".to_vec();
    let mut f = b"APETAGEX".to_vec();
    f.extend_from_slice(&4242u32.to_le_bytes());
    f.extend_from_slice(&32u32.to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(&[0u8; 8]);
    bad_version.extend_from_slice(&f);
    let mut cur = Cursor::new(bad_version);
    assert!(audio::musepack::apev2::read_tags(&mut cur).is_err());
}

// ---------------------------------------------------------------------
// Embedded artwork reader: FLAC PICTURE blocks.
// ---------------------------------------------------------------------

/// A minimal metadata-only FLAC stream: `fLaC` + the given blocks with
/// correct `is_last` flags (the first must be STREAMINFO). No frames —
/// the picture reader never touches audio.
fn flac_metadata(blocks: &[(u8, Vec<u8>)]) -> Vec<u8> {
    assert!(!blocks.is_empty());
    let mut out = b"fLaC".to_vec();
    for (i, (block_type, payload)) in blocks.iter().enumerate() {
        let len = payload.len();
        assert!(len <= 0x00FF_FFFF, "block length is a 24-bit field");
        let header = if i + 1 == blocks.len() {
            0x80 | block_type
        } else {
            *block_type
        };
        out.push(header);
        out.extend_from_slice(&[(len >> 16) as u8, (len >> 8) as u8, len as u8]);
        out.extend_from_slice(payload);
    }
    out
}

/// The mandatory first block (34 payload bytes; the reader checks the
/// block layout, not the acoustic facts).
fn streaminfo() -> (u8, Vec<u8>) {
    (0, vec![0u8; 34])
}

/// A well-formed PICTURE block payload.
fn picture_payload(picture_type: u32, mime: &str, description: &[u8], data: &[u8]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&picture_type.to_be_bytes());
    p.extend_from_slice(&(mime.len() as u32).to_be_bytes());
    p.extend_from_slice(mime.as_bytes());
    p.extend_from_slice(&(description.len() as u32).to_be_bytes());
    p.extend_from_slice(description);
    p.extend_from_slice(&1u32.to_be_bytes()); // width (never consumed)
    p.extend_from_slice(&1u32.to_be_bytes()); // height
    p.extend_from_slice(&24u32.to_be_bytes()); // depth
    p.extend_from_slice(&0u32.to_be_bytes()); // colors
    p.extend_from_slice(&(data.len() as u32).to_be_bytes());
    p.extend_from_slice(data);
    p
}

/// Signature-valid image payloads (the readers never decode, so the
/// bytes only need to be preserved exactly).
const JPEG_IMAGE: &[u8] = b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\xff\xd9";
const PNG_IMAGE: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\xff\xd9";

#[test]
fn flac_pictures_preserve_types_mime_and_exact_bytes() {
    let stream = flac_metadata(&[
        streaminfo(),
        (3, vec![0u8; 8]), // padding: exercised by the bounded skip path
        (
            6,
            picture_payload(3, "image/jpeg", b"front cover", JPEG_IMAGE),
        ),
        (6, picture_payload(4, "image/png", b"", PNG_IMAGE)),
    ]);
    let pictures = audio::flac::read_pictures(Box::new(Cursor::new(stream))).unwrap();
    assert_eq!(pictures.len(), 2, "both PICTURE blocks, in block order");
    assert_eq!(pictures[0].picture_type, 3);
    assert_eq!(pictures[0].mime, "image/jpeg");
    assert_eq!(pictures[0].data, JPEG_IMAGE, "payload preserved exactly");
    assert_eq!(pictures[1].picture_type, 4);
    assert_eq!(pictures[1].mime, "image/png");
    assert_eq!(pictures[1].data, PNG_IMAGE);
}

#[test]
fn flac_pictures_allow_empty_payloads() {
    // Structurally valid with zero image bytes; the consumer decides
    // whether an empty payload is usable artwork (it is not).
    let stream = flac_metadata(&[
        streaminfo(),
        (6, picture_payload(3, "image/jpeg", b"", &[])),
    ]);
    let pictures = audio::flac::read_pictures(Box::new(Cursor::new(stream))).unwrap();
    assert_eq!(pictures.len(), 1);
    assert!(pictures[0].data.is_empty());
}

#[test]
fn flac_pictures_reject_malformed_metadata() {
    let read = |stream: Vec<u8>| audio::flac::read_pictures(Box::new(Cursor::new(stream)));

    // No fLaC signature.
    assert!(read(b"OggSnot-a-flac".to_vec()).is_err());

    // The first block must be STREAMINFO.
    assert!(
        read(flac_metadata(&[(
            6,
            picture_payload(3, "image/jpeg", b"", JPEG_IMAGE)
        )]))
        .is_err()
    );

    // A second STREAMINFO block.
    assert!(read(flac_metadata(&[streaminfo(), streaminfo()])).is_err());

    // Reserved block type 127.
    assert!(read(flac_metadata(&[streaminfo(), (127, Vec::new())])).is_err());

    // Truncated picture payload: the block claims more bytes than exist.
    let mut truncated = flac_metadata(&[
        streaminfo(),
        (6, picture_payload(3, "image/jpeg", b"", JPEG_IMAGE)),
    ]);
    truncated.truncate(truncated.len() - 4);
    assert!(read(truncated).is_err());

    // Truncated header: the stream stops mid-header.
    let mut cut_header = b"fLaC".to_vec();
    cut_header.extend_from_slice(&[0x80, 0x00]);
    assert!(read(cut_header).is_err());

    // MIME longer than the reference's 128-byte bound.
    let mut long_mime = picture_payload(3, "image/jpeg", b"", JPEG_IMAGE);
    long_mime[4..8].copy_from_slice(&256u32.to_be_bytes());
    assert!(read(flac_metadata(&[streaminfo(), (6, long_mime)])).is_err());

    // MIME that is not valid UTF-8 (structurally invalid metadata).
    let mut bad_mime = Vec::new();
    bad_mime.extend_from_slice(&3u32.to_be_bytes()); // picture type
    bad_mime.extend_from_slice(&2u32.to_be_bytes()); // mime length
    bad_mime.extend_from_slice(&[0xff, 0xfe]); // invalid UTF-8
    assert!(read(flac_metadata(&[streaminfo(), (6, bad_mime)])).is_err());

    // Description longer than the reference's 4096-byte bound.
    let mut long_description = picture_payload(3, "image/jpeg", b"", JPEG_IMAGE);
    let description_len_at = 4 + 4 + "image/jpeg".len();
    long_description[description_len_at..description_len_at + 4]
        .copy_from_slice(&8193u32.to_be_bytes());
    assert!(read(flac_metadata(&[streaminfo(), (6, long_description)])).is_err());

    // Image data length beyond the block that declares it.
    let mut beyond_block = picture_payload(3, "image/jpeg", b"", JPEG_IMAGE);
    let data_len_at = beyond_block.len() - JPEG_IMAGE.len() - 4;
    beyond_block[data_len_at..data_len_at + 4].copy_from_slice(&0x00FF_FFFFu32.to_be_bytes());
    assert!(read(flac_metadata(&[streaminfo(), (6, beyond_block)])).is_err());

    // Image data length above the 32 MiB picture bound (checked before
    // any allocation follows the claimed length).
    let mut oversized = picture_payload(3, "image/jpeg", b"", JPEG_IMAGE);
    let data_len_at = oversized.len() - JPEG_IMAGE.len() - 4;
    oversized[data_len_at..data_len_at + 4].copy_from_slice(&0x0200_0001u32.to_be_bytes());
    assert!(read(flac_metadata(&[streaminfo(), (6, oversized)])).is_err());
}

#[test]
fn flac_metadata_block_count_is_bounded() {
    // STREAMINFO + 4096 further blocks: the 4097th header fails closed
    // (the reference's FLAC_BLOCK_MAX), so a hostile stream of zero-cost
    // blocks cannot spin the walker.
    let mut blocks: Vec<(u8, Vec<u8>)> = vec![streaminfo()];
    for _ in 0..4096 {
        blocks.push((1, Vec::new()));
    }
    assert!(audio::flac::read_pictures(Box::new(Cursor::new(flac_metadata(&blocks)))).is_err());
}
