//! Embedded-artwork extraction primitives (`musicpack_author::artwork`):
//! signature sniffing, the FLAC picture-type → role mapping, and the
//! extension-dispatching readers discovery (`scan`) and staging
//! (`pipeline`) share.
//!
//! Fixtures are built in-test and deterministic — minimal
//! metadata-only FLAC streams and synthetic APEv2 tails. No legacy
//! process runs; no frozen corpus is touched.

use std::path::{Path, PathBuf};

use musicpack_author::artwork::{self, ImageFormat};

// ---------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("musicpack-artwork-{name}-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Signature-valid image payloads (never decoded — only preserved).
const JPEG_IMAGE: &[u8] = b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\xff\xd9";
const PNG_IMAGE: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\xff\xd9";

/// A well-formed PICTURE block payload.
fn picture_payload(picture_type: u32, mime: &str, data: &[u8]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&picture_type.to_be_bytes());
    p.extend_from_slice(&(mime.len() as u32).to_be_bytes());
    p.extend_from_slice(mime.as_bytes());
    p.extend_from_slice(&0u32.to_be_bytes()); // description length
    p.extend_from_slice(&1u32.to_be_bytes()); // width
    p.extend_from_slice(&1u32.to_be_bytes()); // height
    p.extend_from_slice(&24u32.to_be_bytes()); // depth
    p.extend_from_slice(&0u32.to_be_bytes()); // colors
    p.extend_from_slice(&(data.len() as u32).to_be_bytes());
    p.extend_from_slice(data);
    p
}

/// A minimal metadata-only FLAC stream carrying the given PICTURE blocks
/// (`fLaC` + STREAMINFO + pictures with correct `is_last` flags).
fn flac_with_pictures(pictures: &[(u32, &str, &[u8])]) -> Vec<u8> {
    let mut blocks: Vec<(u8, Vec<u8>)> = vec![(0, vec![0u8; 34])];
    blocks.extend(
        pictures
            .iter()
            .map(|(picture_type, mime, data)| (6u8, picture_payload(*picture_type, mime, data))),
    );
    let mut out = b"fLaC".to_vec();
    for (i, (block_type, payload)) in blocks.iter().enumerate() {
        let len = payload.len();
        out.push(if i + 1 == blocks.len() {
            0x80 | block_type
        } else {
            *block_type
        });
        out.extend_from_slice(&[(len >> 16) as u8, (len >> 8) as u8, len as u8]);
        out.extend_from_slice(payload);
    }
    out
}

/// An APEv2 tail: optional header + items + footer (mirrors the core
/// reader's fixture layout).
fn apev2_tag(items: &[(&str, &[u8], u32)]) -> Vec<u8> {
    fn footer(tag_size: u32, count: u32, flags: u32) -> Vec<u8> {
        let mut f = b"APETAGEX".to_vec();
        f.extend_from_slice(&2000u32.to_le_bytes());
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
    let mut out = footer(tag_size, count, 0x2000_0000 | 0x4000_0000);
    out.extend_from_slice(&body);
    out.extend_from_slice(&footer(tag_size, count, 0x8000_0000 | 0x4000_0000));
    out
}

/// A synthetic `.mpc`: stream bytes followed by an APEv2 tail carrying
/// the front-cover binary item (APEv2 layout: `name\0image`).
fn mpc_with_cover(name_and_image: &[u8]) -> Vec<u8> {
    let mut data = b"not really an mpc stream, but the reader only seeks the tail".to_vec();
    data.extend(apev2_tag(&[
        ("Title", b"Covered".as_slice(), 0),
        ("Cover Art (Front)", name_and_image, 0x8000_0000),
    ]));
    data
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

// ---------------------------------------------------------------------
// signatures and roles
// ---------------------------------------------------------------------

#[test]
fn sniff_image_recognises_exactly_the_contract_formats() {
    assert_eq!(artwork::sniff_image(JPEG_IMAGE), Some(ImageFormat::Jpeg));
    assert_eq!(artwork::sniff_image(PNG_IMAGE), Some(ImageFormat::Png));
    assert_eq!(
        artwork::sniff_image(b"\xff\xd8\xff"),
        Some(ImageFormat::Jpeg)
    );
    assert_eq!(
        artwork::sniff_image(b"\x89PNG\r\n\x1a\n"),
        Some(ImageFormat::Png)
    );

    // Everything else is not artwork, whatever it claims to be.
    assert_eq!(artwork::sniff_image(b"GIF89a..."), None);
    assert_eq!(artwork::sniff_image(b"RIFF....WEBP"), None);
    assert_eq!(artwork::sniff_image(b"BM"), None);
    assert_eq!(artwork::sniff_image(b"\xff\xd8"), None); // truncated JPEG
    assert_eq!(artwork::sniff_image(b"\x89PNG\r\n\x1a"), None); // truncated PNG
    assert_eq!(artwork::sniff_image(b""), None);
    assert_eq!(artwork::sniff_image(b"not an image"), None);

    assert_eq!(ImageFormat::Jpeg.ext(), "jpg");
    assert_eq!(ImageFormat::Png.ext(), "png");
    assert_eq!(ImageFormat::Jpeg.mime(), "image/jpeg");
    assert_eq!(ImageFormat::Png.mime(), "image/png");
}

#[test]
fn picture_role_maps_the_mpack_vocabulary() {
    assert_eq!(artwork::picture_role(3), "front");
    assert_eq!(artwork::picture_role(4), "back");
    assert_eq!(artwork::picture_role(7), "booklet-page");
    assert_eq!(artwork::picture_role(8), "medium");
    // Every other typed picture (other, file icons, logos, …) is `other`.
    for picture_type in [0u32, 1, 2, 5, 6, 9, 18, 19, 20, u32::MAX] {
        assert_eq!(artwork::picture_role(picture_type), "other");
    }
}

// ---------------------------------------------------------------------
// FLAC extraction
// ---------------------------------------------------------------------

#[test]
fn flac_embedded_images_keep_order_mime_and_exact_bytes() {
    let temp = TempDir::new("flac");
    let file = write(
        temp.path(),
        "01 - Song.flac",
        &flac_with_pictures(&[(3, "image/jpeg", JPEG_IMAGE), (4, "image/png", PNG_IMAGE)]),
    );

    let images = artwork::read_embedded(&file).unwrap();
    assert_eq!(images.len(), 2);
    assert_eq!(images[0].role, "front");
    assert_eq!(images[0].mime.as_deref(), Some("image/jpeg"));
    assert_eq!(images[0].format, ImageFormat::Jpeg);
    assert_eq!(images[0].bytes, JPEG_IMAGE, "bytes preserved exactly");
    assert_eq!(images[1].role, "back");
    assert_eq!(images[1].mime.as_deref(), Some("image/png"));
    assert_eq!(images[1].format, ImageFormat::Png);
    assert_eq!(images[1].bytes, PNG_IMAGE);
}

#[test]
fn embedded_selection_prefers_the_typed_front_cover() {
    let temp = TempDir::new("select");
    // The untyped `other` picture comes first in block order; the
    // explicitly typed front cover must still win the `front` role.
    let file = write(
        temp.path(),
        "01 - Song.flac",
        &flac_with_pictures(&[
            (0, "image/png", PNG_IMAGE),
            (4, "image/jpeg", JPEG_IMAGE),
            (3, "image/jpeg", b"\xff\xd8\xff\xf0front-bytes"),
        ]),
    );

    let front = artwork::extract_role(&file, "front").unwrap().unwrap();
    assert_eq!(front.bytes, b"\xff\xd8\xff\xf0front-bytes");
    let back = artwork::extract_role(&file, "back").unwrap().unwrap();
    assert_eq!(back.bytes, JPEG_IMAGE);
    assert!(
        artwork::extract_role(&file, "medium").unwrap().is_none(),
        "absent roles are not invented"
    );
    assert!(
        artwork::extract_role(&file, "other").unwrap().is_some(),
        "the untyped picture still fills `other`"
    );
}

#[test]
fn skips_unsupported_and_empty_embedded_payloads() {
    let temp = TempDir::new("skip");
    // A GIF payload, an empty payload and a 1-byte stub are not usable
    // artwork; only the signature-valid JPEG qualifies.
    let file = write(
        temp.path(),
        "01 - Song.flac",
        &flac_with_pictures(&[
            (0, "image/gif", b"GIF89a-not-supported"),
            (4, "image/png", b""),
            (7, "image/jpeg", b"\x00"),
            (3, "image/jpeg", JPEG_IMAGE),
        ]),
    );
    let images = artwork::read_embedded(&file).unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].role, "front");
    assert_eq!(images[0].bytes, JPEG_IMAGE);

    // Nothing usable at all → empty, not an error (best-effort discovery).
    let none = write(
        temp.path(),
        "02 - None.flac",
        &flac_with_pictures(&[(3, "image/gif", b"GIF89a-only")]),
    );
    assert!(artwork::read_embedded(&none).unwrap().is_empty());
    assert!(artwork::extract_role(&none, "front").unwrap().is_none());
}

#[test]
fn malformed_flac_picture_metadata_fails_closed() {
    let temp = TempDir::new("malformed");
    // The picture's data length claims far more bytes than the block
    // holds: structurally invalid → error (discovery skips the artwork,
    // staging fails closed).
    let mut payload = picture_payload(3, "image/jpeg", JPEG_IMAGE);
    let data_len_at = payload.len() - JPEG_IMAGE.len() - 4;
    payload[data_len_at..data_len_at + 4].copy_from_slice(&0x00FF_FFFFu32.to_be_bytes());

    let mut bytes = b"fLaC".to_vec();
    let streaminfo = [0u8; 34];
    bytes.push(0x00); // STREAMINFO, not last
    bytes.extend_from_slice(&[
        (streaminfo.len() >> 16) as u8,
        (streaminfo.len() >> 8) as u8,
        streaminfo.len() as u8,
    ]);
    bytes.extend_from_slice(&streaminfo);
    let len = payload.len();
    bytes.push(0x80 | 6); // the malformed picture is the last block
    bytes.extend_from_slice(&[(len >> 16) as u8, (len >> 8) as u8, len as u8]);
    bytes.extend_from_slice(&payload);

    let file = write(temp.path(), "03 - Broken.flac", &bytes);
    assert!(artwork::read_embedded(&file).is_err());
}

// ---------------------------------------------------------------------
// Musepack APEv2 extraction
// ---------------------------------------------------------------------

#[test]
fn mpc_front_cover_strips_the_filename_and_keeps_bytes() {
    let temp = TempDir::new("mpc");
    let file = write(
        temp.path(),
        "01 - Song.mpc",
        &mpc_with_cover(b"folder.png\0\x89PNG\r\n\x1a\ninside"),
    );

    let images = artwork::read_embedded(&file).unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].role, "front");
    assert_eq!(images[0].format, ImageFormat::Png);
    assert_eq!(
        images[0].mime, None,
        "APEv2 carries no MIME hint; the signature decides"
    );
    assert_eq!(images[0].bytes, b"\x89PNG\r\n\x1a\ninside");

    let front = artwork::extract_role(&file, "front").unwrap().unwrap();
    assert_eq!(front.bytes, images[0].bytes);
    // The APEv2 convention knows only the front cover.
    assert!(artwork::extract_role(&file, "back").unwrap().is_none());
}

#[test]
fn unsupported_and_untagged_sources_have_no_embedded_artwork() {
    let temp = TempDir::new("none");

    // WAV (and any non-FLAC/MPC extension) carries nothing embedded.
    let wav = write(temp.path(), "01 - Song.wav", b"RIFF....WAVEfmt ");
    assert!(artwork::read_embedded(&wav).unwrap().is_empty());

    // An MPC without an APEv2 front cover → empty, not an error.
    let untagged = write(temp.path(), "02 - Bare.mpc", b"just stream bytes");
    assert!(artwork::read_embedded(&untagged).unwrap().is_empty());

    // Extensions dispatch case-insensitively.
    let upper = write(
        temp.path(),
        "03 - Upper.FLAC",
        &flac_with_pictures(&[(3, "image/jpeg", JPEG_IMAGE)]),
    );
    let images = artwork::read_embedded(&upper).unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].bytes, JPEG_IMAGE);
}
