//! Embedded artwork extraction: FLAC `PICTURE` blocks and the APEv2
//! front cover of `.mpc` sources.
//!
//! The draft already distinguishes file-based artwork (`path`) from
//! embedded artwork (`embedded` + `sourceAudio` — the reference's
//! `musicpack-draft` shape). This module is the single place that turns a
//! source audio file's embedded metadata into image bytes, so both halves
//! of the pipeline agree by construction:
//!
//! * discovery ([`crate::scan`]) calls [`read_embedded`] **best-effort**:
//!   structurally malformed embedded metadata yields no embedded artwork
//!   rather than failing the scan (tag reads already behave that way);
//! * the pipeline ([`crate::pipeline`]) calls [`extract_role`]
//!   **fail-closed** while staging, so a draft entry that promises
//!   embedded artwork must actually contain it at build time.
//!
//! Contract: only **JPEG and PNG** (the artwork formats external covers
//! are discovered with), selected by *signature* — extensions and declared
//! MIME types are hints, never the decision. Bytes are preserved exactly:
//! nothing here decodes, resizes or transcodes an image, and no image
//! format outside the contract is extracted.

use std::fs::File;
use std::path::Path;

use musicpack_core::audio::flac;
use musicpack_core::audio::musepack::apev2;
use musicpack_core::{Error, Result};

/// An embedded image format the MusicPack artwork contract accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// JPEG (`FF D8 FF` signature), staged with a `.jpg` extension.
    Jpeg,
    /// PNG (8-byte signature), staged with a `.png` extension.
    Png,
}

impl ImageFormat {
    /// The package-file extension for this format (lower-case, no dot).
    pub fn ext(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Png => "png",
        }
    }

    /// The canonical MIME type for this format.
    pub fn mime(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Png => "image/png",
        }
    }
}

/// Sniffs the supported image signature at the start of `bytes`.
///
/// This is the only format decision embedded artwork makes: a payload
/// whose signature is neither JPEG nor PNG is not artwork, whatever its
/// extension or declared MIME type claims.
pub fn sniff_image(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.len() >= 3 && bytes[0] == 0xff && bytes[1] == 0xd8 && bytes[2] == 0xff {
        return Some(ImageFormat::Jpeg);
    }
    if bytes.len() >= 8 && bytes[..8] == *b"\x89PNG\r\n\x1a\n" {
        return Some(ImageFormat::Png);
    }
    None
}

/// Maps a FLAC `PICTURE` type to the `.mpack` artwork role — the
/// reference's `musicpack_meta_picture_role`: 3 → `front`, 4 → `back`,
/// 7 → `booklet-page`, 8 → `medium`, anything else → `other`.
pub fn picture_role(picture_type: u32) -> &'static str {
    match picture_type {
        3 => "front",
        4 => "back",
        7 => "booklet-page",
        8 => "medium",
        _ => "other",
    }
}

/// One extracted embedded image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedImage {
    /// The artwork role the picture type maps to.
    pub role: &'static str,
    /// The declared MIME hint (FLAC pictures only; `None` for APEv2 —
    /// the signature, not the hint, decides the staged extension).
    pub mime: Option<String>,
    /// The signature-sniffed format.
    pub format: ImageFormat,
    /// The original encoded image bytes, preserved exactly.
    pub bytes: Vec<u8>,
}

/// Reads every usable embedded image from `source`, in deterministic
/// order (FLAC: metadata-block order; MPC: the single APEv2 front cover).
///
/// Dispatch is by extension: `.flac` reads `PICTURE` blocks, `.mpc` reads
/// the trailing APEv2 `Cover Art (Front)` item, anything else (`.wav`)
/// has no embedded artwork. Structurally malformed embedded metadata is
/// an error ([`Error::Invalid`]); payloads that are empty or not
/// signature-valid JPEG/PNG are silently skipped — they are not
/// extractable artwork, and discovery must not promise them.
pub fn read_embedded(source: &Path) -> Result<Vec<EmbeddedImage>> {
    let extension = source
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "flac" => {
            let file = File::open(source).map_err(|e| Error::Io {
                detail: format!("cannot open '{}': {e}", source.display()),
            })?;
            let pictures = flac::read_pictures(Box::new(file))?;
            Ok(pictures
                .into_iter()
                .filter_map(|picture| {
                    let format = sniff_image(&picture.data)?;
                    let mime = (!picture.mime.is_empty()).then_some(picture.mime);
                    Some(EmbeddedImage {
                        role: picture_role(picture.picture_type),
                        mime,
                        format,
                        bytes: picture.data,
                    })
                })
                .collect())
        }
        "mpc" => {
            let mut file = File::open(source).map_err(|e| Error::Io {
                detail: format!("cannot open '{}': {e}", source.display()),
            })?;
            let Some(bytes) = apev2::read_cover_art(&mut file)? else {
                return Ok(Vec::new());
            };
            let Some(format) = sniff_image(&bytes) else {
                return Ok(Vec::new());
            };
            Ok(vec![EmbeddedImage {
                role: "front",
                mime: None,
                format,
                bytes,
            }])
        }
        _ => Ok(Vec::new()),
    }
}

/// The first usable embedded image carrying `role` (staging's selection),
/// or `None` when the file has no such artwork.
///
/// "First" follows the deterministic read order of [`read_embedded`], so
/// a picture explicitly typed `front` (FLAC type 3 / the APEv2 front
/// cover) is selected for the `front` role instead of an arbitrary
/// earlier picture of another type.
pub fn extract_role(source: &Path, role: &str) -> Result<Option<EmbeddedImage>> {
    Ok(read_embedded(source)?
        .into_iter()
        .find(|image| image.role == role))
}
