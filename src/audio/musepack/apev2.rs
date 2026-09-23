// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

//! Minimal APEv2 tag reader for `.mpc` pass-through sources.
//!
//! `flac2mpc` projects an album's tags into a trailing APEv2 tag on the
//! Musepack stream. The Musepack decoder stops at the stream's end blocks
//! and never reads that tail, so fresh-album discovery reads it here
//! directly from the file footer. The reader is deliberately **text-only**:
//! binary items (embedded cover art) are skipped without buffering, keeping
//! embedded-artwork extraction a separate, later concern.
//!
//! Reading is best-effort for callers: a file without an `APETAGEX` footer
//! yields an empty list, while a footer that *claims* to be an APEv2 tag
//! but is structurally inconsistent (bad size, truncation, non-ASCII key)
//! fails with [`Error::Invalid`](crate::Error::Invalid) so malformed tags
//! never silently masquerade as untagged.

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
/// Item flag: the value is binary (cover art, binary data) — skipped.
const ITEM_FLAG_BINARY: u32 = 0x8000_0000;
/// Footer flag: the tag is preceded by a 32-byte header.
const FLAG_HAS_HEADER: u32 = 0x8000_0000;

fn io_err(e: std::io::Error) -> Error {
    Error::Io {
        detail: format!("APEv2 tag read failed: {e}"),
    }
}

fn u32le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
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
    let len = source.seek(SeekFrom::End(0)).map_err(io_err)?;
    if len < FOOTER_LEN {
        return Ok(Vec::new());
    }

    // The footer is the last 32 bytes.
    source
        .seek(SeekFrom::End(-(FOOTER_LEN as i64)))
        .map_err(io_err)?;
    let mut footer = [0u8; FOOTER_LEN as usize];
    source.read_exact(&mut footer).map_err(io_err)?;
    if &footer[0..8] != MAGIC {
        return Ok(Vec::new()); // no tag
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

    source.seek(SeekFrom::Start(items_start)).map_err(io_err)?;
    let mut pos = items_start;
    let mut out = Vec::new();
    for _ in 0..item_count {
        if pos + 8 > items_end {
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
            if pos >= items_end || key.len() >= 255 {
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
        if value_end > items_end {
            return Err(invalid("APEv2 item value is truncated"));
        }
        if item_flags & ITEM_FLAG_BINARY == 0 {
            let mut buf = vec![0u8; value_size as usize];
            source.read_exact(&mut buf).map_err(io_err)?;
            pos = value_end;
            // Text items are UTF-8 by spec; anything else is skipped.
            if let Ok(value) = String::from_utf8(buf) {
                out.push((String::from_utf8_lossy(&key).into_owned(), value));
            }
        } else {
            // Binary item (e.g. cover art): skip its bytes untouched.
            source
                .seek(SeekFrom::Current(value_size as i64))
                .map_err(io_err)?;
            pos = value_end;
        }
    }
    Ok(out)
}
