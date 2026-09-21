//! Filesystem probing for ingestion — the port of the reference's
//! `codec.c` probe half and `mime.c` tables.
//!
//! The scanner needs lightweight per-object facts at ingest time: the codec
//! string, stream version, sample rate, channel count, MIME type and file
//! size. Decoding never happens here (and specifically never for serving):
//!
//! - Musepack (`.mpc`) opens the existing Rust SV8 decoder over the file
//!   and reads its stream facts. Anything the decoder rejects (including
//!   SV7 — see O-S3) falls back to codec `"musepack"` with zeroed numbers,
//!   exactly like the reference's failed-probe path.
//! - FLAC (`.flac`) parses the 42-byte STREAMINFO header directly,
//!   mirroring the reference's `flac_probe` byte-for-byte (magic, first
//!   block type, rate/channel bit arithmetic).
//! - Everything else reports the extension-derived codec with zeroed
//!   numbers.
//!
//! A probe never fails ingestion: unresolvable or unreadable objects yield
//! an empty codec (the sync falls back to the extension codec) and size 0.

use std::path::Path;

use crate::store::TrackProbe;

/// `true` when `path` names a servable-shaped file: not a symlink, a
/// regular file, link count 1 (the reference's `is_regular_path`; the link
/// check is Unix-only, like the core's directory adapter).
pub fn is_regular_file(path: &Path) -> bool {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return false;
        }
    }
    true
}

/// File size in bytes, or 0 when the file cannot be stated (the
/// reference's `file_size_of`: `stat`, follow links, 0 on failure).
pub fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// The extension-derived codec (`mp_codec_for_path`): case-sensitive,
/// last dot, `musepack`/`flac`/`wav`/`vorbis`/`unknown`.
pub fn codec_for_path(path: &str) -> &'static str {
    match extension(path) {
        "mpc" => "musepack",
        "flac" => "flac",
        "wav" => "wav",
        "ogg" => "vorbis",
        _ => "unknown",
    }
}

/// The MIME type for a relative path (`mp_mime_for_path`): case-sensitive,
/// last dot, `application/octet-stream` fallback.
pub fn mime_for_path(path: &str) -> &'static str {
    match extension(path) {
        "mpc" => "audio/musepack",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "wfm" => "application/vnd.musicpack.waveform-v1+octet-stream",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "pdf" => "application/pdf",
        "html" | "htm" => "text/html",
        "js" | "mjs" | "ts" => "text/javascript",
        "css" => "text/css",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        "lrc" | "txt" | "md" => "text/plain",
        _ => "application/octet-stream",
    }
}

fn extension(path: &str) -> &str {
    match path.rfind('.') {
        Some(i) => &path[i + 1..],
        None => "",
    }
}

/// Probes one manifest audio object. `root` is the package directory,
/// `rel` the canonical relative path. The returned probe always carries
/// the absolute path when the object resolved (even when the codec probe
/// itself failed — the reference still stats the file for its size).
pub fn probe_track(root: &Path, rel: &str) -> TrackProbe {
    let abs = root.join(rel);
    if !is_regular_file(&abs) {
        return TrackProbe {
            abs_path: None,
            codec: String::new(),
            stream_version: 0,
            sample_rate: 0,
            channels: 0,
        };
    }
    let probed = match codec_for_path(rel) {
        "musepack" => probe_musepack(&abs),
        "flac" => probe_flac(&abs),
        _ => None,
    };
    match probed {
        Some((codec, version, rate, channels)) => TrackProbe {
            abs_path: Some(abs),
            codec,
            stream_version: version,
            sample_rate: rate,
            channels,
        },
        None => TrackProbe {
            abs_path: Some(abs),
            codec: String::new(),
            stream_version: 0,
            sample_rate: 0,
            channels: 0,
        },
    }
}

/// Opens the SV8 decoder over the file and reads its stream facts (the
/// reference's musepack probe arm, minus SV7: the Rust decoder rejects SV7,
/// so those files take the failed-probe path — see O-S3).
fn probe_musepack(abs: &Path) -> Option<(String, i64, i64, i64)> {
    let file = std::fs::File::open(abs).ok()?;
    let decoder = musicpack_core::audio::musepack::MpcDecoder::from_reader(Box::new(file)).ok()?;
    let info = decoder.info();
    Some((
        format!("musepack-sv{}", info.stream_version),
        info.stream_version as i64,
        info.sample_rate as i64,
        info.channels as i64,
    ))
}

/// Parses the 42-byte STREAMINFO header (the reference's `flac_probe`):
/// `fLaC` magic, first metadata block type 0, then the packed
/// rate/channel bits at `si[10..13]`.
fn probe_flac(abs: &Path) -> Option<(String, i64, i64, i64)> {
    use std::io::Read;
    let mut file = std::fs::File::open(abs).ok()?;
    let mut header = [0u8; 42];
    file.read_exact(&mut header).ok()?;
    if &header[0..4] != b"fLaC" {
        return None;
    }
    if header[4] & 0x7f != 0 {
        return None;
    }
    let si = &header[8..];
    let rate = ((si[10] as i64) << 12) | ((si[11] as i64) << 4) | ((si[12] as i64) >> 4);
    let channels = (((si[12] & 0x0e) >> 1) + 1) as i64;
    Some(("flac".to_string(), 0, rate, channels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_mapping_matches_the_reference() {
        assert_eq!(codec_for_path("audio/01.mpc"), "musepack");
        assert_eq!(codec_for_path("audio/01.flac"), "flac");
        assert_eq!(codec_for_path("audio/01.wav"), "wav");
        assert_eq!(codec_for_path("audio/01.ogg"), "vorbis");
        assert_eq!(codec_for_path("audio/01.mp3"), "unknown");
        assert_eq!(codec_for_path("no-extension"), "unknown");
        // Case-sensitive, last dot — like the C strrchr table.
        assert_eq!(codec_for_path("audio/01.MPC"), "unknown");
        assert_eq!(codec_for_path("a.b.mpc"), "musepack");
    }

    #[test]
    fn mime_mapping_matches_the_reference() {
        assert_eq!(mime_for_path("a.mpc"), "audio/musepack");
        assert_eq!(mime_for_path("a.flac"), "audio/flac");
        assert_eq!(mime_for_path("a.wav"), "audio/wav");
        assert_eq!(
            mime_for_path("a.wfm"),
            "application/vnd.musicpack.waveform-v1+octet-stream"
        );
        assert_eq!(mime_for_path("a.jpg"), "image/jpeg");
        assert_eq!(mime_for_path("a.jpeg"), "image/jpeg");
        assert_eq!(mime_for_path("a.ts"), "text/javascript");
        assert_eq!(mime_for_path("a.lrc"), "text/plain");
        assert_eq!(mime_for_path("a.bin"), "application/octet-stream");
        assert_eq!(mime_for_path("a.JPG"), "application/octet-stream");
    }

    #[test]
    fn unresolvable_objects_probe_empty() {
        let probe = probe_track(Path::new("/nonexistent-root-xyz"), "audio/01.mpc");
        assert_eq!(probe.abs_path, None);
        assert_eq!(probe.codec, "");
    }

    /// R4.5 SV7 decision: SV7 is **explicitly unsupported**. The Rust decoder
    /// is SV8-only, so an SV7 stream (`MP+` magic, version nibble 7) takes the
    /// failed-probe path: the audio object resolves and is byte-servable, but
    /// no SV7 stream facts are claimed (the ingest layer falls back to the
    /// extension codec `musepack`). This is a documented, accepted divergence
    /// from the C server, whose vendored libmpcdec would report
    /// `musepack-sv7` + facts. No SV7 fixture exists in the corpus.
    #[test]
    fn sv7_streams_are_rejected_by_the_probe() {
        let dir = std::env::temp_dir().join(format!("mp-probe-sv7-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // SV7 magic: "MP+" + version nibble 7, then arbitrary payload.
        let mut bytes = vec![b'M', b'P', b'+', 7u8];
        bytes.extend_from_slice(&[0u8; 32]);
        std::fs::write(dir.join("old.mpc"), &bytes).unwrap();

        let probe = probe_track(&dir, "old.mpc");
        assert!(
            probe.abs_path.is_some(),
            "the object still resolves (bytes remain servable)"
        );
        assert_eq!(probe.codec, "", "no SV7 facts are claimed");
        assert_eq!(probe.stream_version, 0);
        assert_eq!(probe.sample_rate, 0);
        assert_eq!(probe.channels, 0);
        // The ingest layer's fallback is the extension codec.
        assert_eq!(codec_for_path("old.mpc"), "musepack");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
