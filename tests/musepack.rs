//! Direct Musepack decoder-seam tests: metadata, PCM, chunk equivalence, EOF.

use musicpack_core::Error;
use musicpack_core::audio::musepack::{MpcDecoder, sv8};
use musicpack_core::audio::{self, Codec};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/musepack/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn decode_all(bytes: &[u8], chunk: usize) -> (Vec<f32>, usize) {
    let channels = sv8::parse(bytes).unwrap().channels as usize;
    let mut decoder = MpcDecoder::new(bytes.to_vec()).unwrap();
    let mut pcm = Vec::new();
    let mut buffer = vec![0.0f32; chunk * channels];
    loop {
        let frames = decoder.read_f32(&mut buffer).unwrap();
        if frames == 0 {
            break;
        }
        pcm.extend_from_slice(&buffer[..frames * channels]);
    }
    (pcm, channels)
}

#[test]
fn open_recognizes_sv8_and_decodes_pcm() {
    let bytes = fixture("sine44-q5.mpc");
    let mut decoder =
        audio::open(Box::new(std::io::Cursor::new(bytes.clone()))).expect("SV8 container opens");
    let info = decoder.info();
    assert_eq!(info.codec, Codec::Musepack);
    assert_eq!(info.sample_rate, 44_100);
    assert!(info.channels == 1 || info.channels == 2);
    assert_eq!(info.bits_per_sample, 0, "codec-native float marker");
    assert!(info.is_float);
    let expected = info.total_frames.unwrap();
    assert!(expected > 0);

    let channels = info.channels as usize;
    let mut pcm = Vec::new();
    let mut buffer = vec![0.0f32; 4096];
    loop {
        let frames = decoder.read_f32(&mut buffer).unwrap();
        if frames == 0 {
            break;
        }
        pcm.extend_from_slice(&buffer[..frames * channels]);
    }
    assert_eq!(pcm.len() / channels, expected as usize, "playable length");
    assert!(
        pcm.iter().any(|s| s.abs() > 0.01),
        "decoded audio is non-silent"
    );
    assert!(pcm.iter().all(|s| s.is_finite()), "no NaN/Inf");

    // Float-only: no integer representation.
    let mut pcm32 = vec![0i32; 32];
    assert!(matches!(
        decoder.read_s32(&mut pcm32),
        Err(Error::Unsupported { .. })
    ));
}

#[test]
fn all_fixtures_decode_to_the_declared_length() {
    for name in [
        "sine32-q8.mpc",
        "sine37-q4.mpc",
        "sine44-q5.mpc",
        "sine44-q7.mpc",
        "sine44-q5-48s.mpc",
        "sine48-q6.mpc",
    ] {
        let bytes = fixture(name);
        let info = musicpack_core::audio::musepack::sv8::parse(&bytes).unwrap();
        let expected = info.length_samples() as usize;
        let (pcm, channels) = decode_all(&bytes, 1152);
        assert_eq!(pcm.len() / channels, expected, "{name}: decoded length");
        assert!(pcm.iter().any(|s| s.abs() > 1e-4), "{name}: non-silent");
    }
}

#[test]
fn chunked_reads_are_byte_identical() {
    let bytes = fixture("sine44-q5.mpc");
    let (whole, _) = decode_all(&bytes, 1 << 20);
    for chunk in [1usize, 7, 31, 1152, 17, 4096, 3, 100_000] {
        let (part, _) = decode_all(&bytes, chunk);
        assert_eq!(part, whole, "chunk size {chunk}");
    }
}

#[test]
fn eof_is_stable_across_repeated_reads() {
    let bytes = fixture("sine44-q5.mpc");
    let mut decoder = MpcDecoder::new(bytes).unwrap();
    let mut buffer = vec![0.0f32; 2048];
    // Drain to EOF.
    let mut total = 0usize;
    loop {
        let frames = decoder.read_f32(&mut buffer).unwrap();
        if frames == 0 {
            break;
        }
        total += frames;
    }
    // Repeated reads stay at EOF and produce nothing.
    for _ in 0..3 {
        let frames = decoder.read_f32(&mut buffer).unwrap();
        assert_eq!(frames, 0);
    }
    assert!(total > 0);
}

#[test]
fn malformed_and_truncated_inputs_error_cleanly() {
    assert!(audio::open(Box::new(std::io::Cursor::new(b"MPCK".to_vec()))).is_err());
    let mut junk = b"MPCK".to_vec();
    junk.extend_from_slice(&[0xff; 64]);
    assert!(audio::open(Box::new(std::io::Cursor::new(junk))).is_err());

    // Every truncation of a valid fixture either opens+decodes partially or
    // errors; it must never panic or hang.
    let bytes = fixture("sine44-q5.mpc");
    for cut in (0..bytes.len()).step_by(97) {
        if let Ok(mut decoder) = audio::open(Box::new(std::io::Cursor::new(bytes[..cut].to_vec())))
        {
            let mut buffer = vec![0.0f32; 2048];
            let _ = decoder.read_f32(&mut buffer);
        }
    }
}
