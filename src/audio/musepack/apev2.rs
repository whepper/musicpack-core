// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

//! Minimal APEv2 tag reader for `.mpc` pass-through sources.
//!
//! `flac2mpc` projects an album's tags into a trailing APEv2 tag on the
//! Musepack stream. The Musepack decoder stops at the stream's end blocks
//! and never reads that tail, so fresh-album discovery reads it here
//! directly from the file footer. The reader stays deliberately narrow —
//! exactly two consumers, no general-purpose APEv2 framework:
//!
//! * [`read_tags`] — the trailing **text** items for discovery;
//! * [`read_cover_art`] — the single **binary** item embedded-artwork
//!   extraction needs (`Cover Art (Front)`, the reference's convention).
//!
//! Every other binary item is seeked past without buffering. Reading is
//! best-effort for callers: a file without an `APETAGEX` footer yields an
//! empty list (resp. `None`), while a footer that *claims* to be an APEv2
//! tag but is structurally inconsistent (bad size, truncation, non-ASCII
//! key) fails with [`Error::Invalid`](crate::Error::Invalid) so malformed
//! tags never silently masquerade as untagged.

use std::io::{Read, Seek, SeekFrom};

use super::super::invalid;
use crate::{Error, Result};

/// The footer (and optional header) length in bytes.
const FOOTER_LEN: u64 = 32;
/// The `APETAGEX` preamble shared by header and footer.
const MAGIC: &[u8; 8] = b"APETAGEX";
/// Structural bound on a single tag (header + items + footer). Real tags —
/// including cover art — stay well below this; beyond it the tail is not
/// treated as a tag rather than allocating attacker-chosen sizes.
const MAX_TAG_BYTES: u64 = 16 * 1024 * 1024;
/// Item-count bound (APEv2 keys are small; this is three orders of
/// magnitude above any real tag).
const MAX_ITEMS: u32 = 4096;
/// Item flag: the value is binary (cover art, binary data).
const ITEM_FLAG_BINARY: u32 = 0x8000_0000;
/// Footer flag: the tag is preceded by a 32-byte header.
const FLAG_HAS_HEADER: u32 = 0x8000_0000;
/// The APEv2 front-cover key (case-insensitive; the reference reads only
/// this item for embedded artwork).
const COVER_ART_FRONT: &[u8] = b"cover art (front)";
/// Bound on the file-name prefix of a cover-art item value. APEv2 stores
/// a short NUL-terminated file name before the image bytes; anything
/// longer than this is not a file name, so the item is rejected instead
/// of guessing where the image starts.
const COVER_PREFIX_MAX: usize = 4096;

fn io_err(e: std::io::Error) -> Error {
    Error::Io {
        detail: format!("APEv2 tag read failed: {e}"),
    }
}

fn u32le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// The located item region of a trailing tag.
struct TagLayout {
    items_start: u64,
    items_end: u64,
    item_count: u32,
}

/// An item selected by [`collect`]: its key and buffered value.
struct Item {
    key: Vec<u8>,
    value: Vec<u8>,
}

/// Locates the trailing APEv2 tag of `source`.
///
/// `Ok(None)` when the file ends without an `APETAGEX` footer (the
/// overwhelmingly common case) or ends within one; `Err` when a footer is
/// present but structurally inconsistent (unsupported version,
/// out-of-bounds size).
fn probe<S: Read + Seek>(source: &mut S) -> Result<Option<TagLayout>> {
    let len = source.seek(SeekFrom::End(0)).map_err(io_err)?;
    if len < FOOTER_LEN {
        return Ok(None);
    }

    // The footer is the last 32 bytes.
    source
        .seek(SeekFrom::End(-(FOOTER_LEN as i64)))
        .map_err(io_err)?;
    let mut footer = [0u8; FOOTER_LEN as usize];
    source.read_exact(&mut footer).map_err(io_err)?;
    if &footer[0..8] != MAGIC {
        return Ok(None); // no tag
    }

    let version = u32le(&footer[8..12]);
    let tag_size = u32le(&footer[12..16]) as u64;
    let item_count = u32le(&footer[16..20]);
    let flags = u32le(&footer[20..24]);

    if version != 1000 && version != 2000 {
        return Err(invalid("APEv2 tag has an unsupported version"));
    }
    if tag_size < FOOTER_LEN || tag_size > len || tag_size > MAX_TAG_BYTES {
        return Err(invalid("APEv2 tag size is out of bounds"));
    }
    if item_count > MAX_ITEMS {
        return Err(invalid("APEv2 tag has too many items"));
    }

    let tag_start = len - tag_size;
    let items_start = tag_start
        + if flags & FLAG_HAS_HEADER != 0 {
            FOOTER_LEN
        } else {
            0
        };
    let items_end = len - FOOTER_LEN;
    if items_start > items_end {
        return Err(invalid("APEv2 header/items region is inconsistent"));
    }
    Ok(Some(TagLayout {
        items_start,
        items_end,
        item_count,
    }))
}

/// Walks the tag's items once with the structural bounds enforced for
/// every item. `want(key, flags)` selects which values are buffered;
/// unselected values are seeked past untouched. The returned items keep
/// tag order.
fn collect<S: Read + Seek>(
    source: &mut S,
    layout: &TagLayout,
    want: impl Fn(&[u8], u32) -> bool,
) -> Result<Vec<Item>> {
    source
        .seek(SeekFrom::Start(layout.items_start))
        .map_err(io_err)?;
    let mut pos = layout.items_start;
    let mut out = Vec::new();
    for _ in 0..layout.item_count {
        if pos + 8 > layout.items_end {
            return Err(invalid("APEv2 item header is truncated"));
        }
        let mut head = [0u8; 8];
        source.read_exact(&mut head).map_err(io_err)?;
        pos += 8;
        let value_size = u32le(&head[0..4]) as u64;
        let item_flags = u32le(&head[4..8]);

        // Key: printable ASCII, NUL-terminated, 2..=255 bytes.
        let mut key = Vec::new();
        loop {
            if pos >= layout.items_end || key.len() >= 255 {
                return Err(invalid("APEv2 item key is malformed"));
            }
            let mut byte = [0u8; 1];
            source.read_exact(&mut byte).map_err(io_err)?;
            pos += 1;
            if byte[0] == 0 {
                break;
            }
            if !(0x20..=0x7e).contains(&byte[0]) {
                return Err(invalid("APEv2 item key is not printable ASCII"));
            }
            key.push(byte[0]);
        }
        if key.len() < 2 {
            return Err(invalid("APEv2 item key is too short"));
        }

        let value_end = pos + value_size;
        if value_end > layout.items_end {
            return Err(invalid("APEv2 item value is truncated"));
        }
        if want(&key, item_flags) {
            let mut value = vec![0u8; value_size as usize];
            source.read_exact(&mut value).map_err(io_err)?;
            out.push(Item { key, value });
        } else {
            // Unselected item (e.g. a binary item for the text reader):
            // skip its bytes untouched.
            source
                .seek(SeekFrom::Current(value_size as i64))
                .map_err(io_err)?;
        }
        pos = value_end;
    }
    Ok(out)
}

/// Reads the trailing APEv2 tag of `source` as `(key, value)` text items,
/// in tag order.
///
/// Returns an empty list when the file ends without an `APETAGEX` footer
/// (the overwhelmingly common case) or ends within one. Keys keep their
/// original casing; values must be valid UTF-8 (non-UTF-8 text items are
/// skipped). Binary items are never materialised. A footer that is present
/// but malformed (unsupported version, out-of-bounds size, truncation,
/// non-printable key) is an error.
pub fn read_tags<S: Read + Seek>(source: &mut S) -> Result<Vec<(String, String)>> {
    let Some(layout) = probe(source)? else {
        return Ok(Vec::new());
    };
    let items = collect(source, &layout, |_, flags| flags & ITEM_FLAG_BINARY == 0)?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        // Text items are UTF-8 by spec; anything else is skipped.
        if let Ok(text) = String::from_utf8(item.value) {
            out.push((String::from_utf8_lossy(&item.key).into_owned(), text));
        }
    }
    Ok(out)
}

/// Reads the trailing APEv2 tag's **front-cover** binary item
/// (`Cover Art (Front)` — the reference's embedded-artwork convention)
/// and returns the image bytes with the APEv2 file-name prefix stripped.
///
/// Returns `Ok(None)` when the tag carries no front-cover binary item (or
/// there is no tag at all); a present-but-malformed tag fails exactly
/// like [`read_tags`]. The value layout follows APEv2: a NUL-terminated
/// file name followed by the image bytes; a legacy value without any NUL
/// is taken as the image whole (the reference does the same). The payload
/// is returned verbatim — callers validate image signatures; nothing is
/// decoded or transcoded.
pub fn read_cover_art<S: Read + Seek>(source: &mut S) -> Result<Option<Vec<u8>>> {
    let Some(layout) = probe(source)? else {
        return Ok(None);
    };
    let items = collect(source, &layout, |key, flags| {
        flags & ITEM_FLAG_BINARY != 0 && key.eq_ignore_ascii_case(COVER_ART_FRONT)
    })?;
    let Some(item) = items.into_iter().next() else {
        return Ok(None); // first front-cover item wins; none present
    };
    split_cover_prefix(&item.value).map(Some)
}

/// Splits an APEv2 cover-art value into its image payload: everything
/// after the first NUL (the file name), or the whole value when the
/// prefix is absent.
fn split_cover_prefix(value: &[u8]) -> Result<Vec<u8>> {
    match value.iter().position(|&b| b == 0) {
        None => Ok(value.to_vec()),
        Some(at) if at > COVER_PREFIX_MAX => {
            Err(invalid("APEv2 cover-art file name prefix is too long"))
        }
        Some(at) => Ok(value[at + 1..].to_vec()),
    }
}
