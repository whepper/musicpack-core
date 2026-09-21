//! Strict LRC parser — implements §4 of `docs/musicpack-lyrics-v1.md`
//! exactly. Parsing is total: every input either yields a
//! [`LyricsDocument`](super::LyricsDocument) or one typed
//! [`LyricsError`](super::LyricsError); nothing is silently discarded
//! or degraded (the two defined reductions — word-tag stripping and
//! unknown-tag ignoring — are explicit spec rules).

use super::{LyricsContent, LyricsDocument, LyricsError, LyricsLine, LyricsMetadata};

/// Maximum document size in bytes (spec §4.2), enforced on the raw
/// input before any other work.
pub const MAX_DOC_BYTES: usize = 512 * 1024;
/// Maximum number of physical lines (spec §4.2).
pub const MAX_LINES: usize = 10_000;
/// Maximum length of one physical line in bytes, post-normalization
/// (spec §4.2).
pub const MAX_LINE_BYTES: usize = 4_096;

/// Maximum `|offset|` in milliseconds (spec §4.4: one hour).
const MAX_OFFSET_MS: i64 = 3_600_000;

/// Parses LRC bytes into a [`LyricsDocument`] under the strict profile.
///
/// Normalization order (spec §4.1): size limit → UTF-8 validation →
/// BOM strip → CRLF/CR → LF → line splitting. Parsing is stateless and
/// deterministic: the same bytes always produce the same document or
/// the same error.
pub fn parse(bytes: &[u8]) -> Result<LyricsDocument, LyricsError> {
    if bytes.len() > MAX_DOC_BYTES {
        return Err(LyricsError::DocTooLarge { size: bytes.len() });
    }
    let text = std::str::from_utf8(bytes).map_err(|_| LyricsError::InvalidUtf8)?;
    // A single leading UTF-8 BOM is stripped (spec §4.1).
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    // CRLF and CR normalize to LF before splitting (spec §4.1).
    let text = if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.to_string()
    };

    let line_count = text.lines().count();
    if line_count > MAX_LINES {
        return Err(LyricsError::TooManyLines { count: line_count });
    }

    // Scan state. Timestamps are collected raw; the offset tag is applied
    // in a second pass because it may appear anywhere in the document.
    let mut metadata = LyricsMetadata::default();
    let mut offset: Option<i64> = None;
    let mut word_tags_stripped = false;
    let mut raw_entries: Vec<(usize, i64, String)> = Vec::new(); // (1-based line, raw ms, block text)
    let mut pending: Vec<String> = Vec::new(); // untimed content lines, document order

    for (idx, line) in text.lines().enumerate() {
        let line_no = idx + 1;
        if line.len() > MAX_LINE_BYTES {
            return Err(LyricsError::LineTooLong {
                line: line_no,
                length: line.len(),
            });
        }

        // Leading ASCII whitespace is skipped before token extraction
        // (spec §4.3: an indented timestamp is a timestamp — a file that
        // clearly intends timing must not degrade to plain text).
        let line = line.trim_start_matches(|c: char| c.is_ascii_whitespace());
        let (tokens, rest) = split_leading_tokens(line);
        let mut timestamps: Vec<i64> = Vec::new();
        for token in &tokens {
            if is_timestamp_shaped(token) {
                match parse_timestamp(token) {
                    Some(ms) => timestamps.push(ms),
                    None => return Err(LyricsError::MalformedTimestamp { line: line_no }),
                }
                continue;
            }
            // Tag or section marker. Only `key:value` tokens carry meaning;
            // bare tokens (`[Chorus]`) are ignored markers (spec §4.3).
            if let Some((key, value)) = token.split_once(':') {
                let key = key.trim().to_ascii_lowercase();
                let value = value.trim();
                match key.as_str() {
                    "offset" => {
                        offset = Some(
                            parse_offset(value)
                                .ok_or(LyricsError::MalformedOffset { line: line_no })?,
                        );
                    }
                    "ar" => set_last(&mut metadata.artist, value),
                    "ti" => set_last(&mut metadata.title, value),
                    "al" => set_last(&mut metadata.album, value),
                    "by" => set_last(&mut metadata.by, value), // Unknown tags are parsed and ignored (spec §4.4).
                    _ => {}
                }
            }
        }

        let (owned_text, stripped) = strip_word_tags(rest);
        let text = owned_text.trim_ascii();
        word_tags_stripped |= stripped;

        if !timestamps.is_empty() {
            // The pending untimed block attaches to this line's block,
            // prepended in document order (spec §4.3).
            let block = if pending.is_empty() {
                text.to_string()
            } else {
                let mut block = pending.join("\n");
                block.push('\n');
                block.push_str(text);
                block
            };
            pending.clear();
            for ts in timestamps {
                raw_entries.push((line_no, ts, block.clone()));
            }
        } else if text.is_empty() && !tokens.is_empty() {
            // A pure tag/marker line carries no content (spec §4.3).
        } else {
            // Content: a text line, or an empty line (stanza break).
            pending.push(text.to_string());
        }
    }

    if raw_entries.is_empty() {
        // Plain document: every collected content line, in document order.
        return Ok(LyricsDocument {
            content: LyricsContent::Plain(pending),
            metadata,
            word_tags_stripped,
        });
    }

    // Apply the offset shift (last tag wins); a shift below zero is a
    // typed error, never a clamp, and an unrepresentable shift is
    // malformed — the spec folds overflow into MalformedTimestamp
    // (spec §4.6).
    let mut entries: Vec<LyricsLine> = Vec::with_capacity(raw_entries.len());
    for (line_no, raw, text) in raw_entries {
        let shift = offset.unwrap_or(0);
        let shifted = match raw.checked_add(shift) {
            Some(v) => v,
            None => return Err(LyricsError::MalformedTimestamp { line: line_no }),
        };
        if shifted < 0 {
            return Err(LyricsError::NegativeTimestamp { line: line_no });
        }
        entries.push(LyricsLine {
            timestamp_ms: shifted,
            text,
        });
    }

    // A trailing untimed block attaches to the last timestamped line
    // (spec §4.3).
    if !pending.is_empty() {
        let last = entries.last_mut().expect("entries is non-empty");
        last.text.push('\n');
        last.text.push_str(&pending.join("\n"));
    }

    // Stable sort: equal timestamps keep document order — the normative
    // tie-break for active-line selection (spec §6.4/§9).
    entries.sort_by_key(|line| line.timestamp_ms);

    Ok(LyricsDocument {
        content: LyricsContent::Synced(entries),
        metadata,
        word_tags_stripped,
    })
}

/// Splits the leading bracket tokens off a line. A token runs from `[`
/// to the first `]`; an unterminated `[` leaves the rest as literal
/// text (spec §4.3: tokens require a closing bracket).
fn split_leading_tokens(line: &str) -> (Vec<&str>, &str) {
    let mut tokens = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        if open != 0 {
            break;
        }
        match rest.find(']') {
            Some(close) => {
                tokens.push(&rest[1..close]);
                rest = &rest[close + 1..];
            }
            None => break,
        }
    }
    (tokens, rest)
}

/// A bracket token is *timestamp-shaped* — and therefore held to the
/// strict timestamp grammar, with violations reported as
/// [`LyricsError::MalformedTimestamp`] — when it starts with a digit or
/// sign and contains a colon (spec §4.3: a file that clearly intends
/// timing must not silently degrade to text).
fn is_timestamp_shaped(token: &str) -> bool {
    let mut bytes = token.bytes();
    match bytes.next() {
        Some(b'0'..=b'9') | Some(b'+') | Some(b'-') => {}
        _ => return false,
    }
    token.as_bytes().contains(&b':')
}

/// Parses `[mm:ss]` / `[mm:ss.xx]` / `[mm:ss.xxx]` into milliseconds.
///
/// Grammar (spec §4.3): minutes are one or more digits (no sign);
/// seconds are exactly two digits with value ≤ 59; the fraction, when
/// present, is exactly two digits (centiseconds) or three digits
/// (milliseconds). Any overflow beyond `i64` milliseconds is malformed.
fn parse_timestamp(token: &str) -> Option<i64> {
    let (minutes, rest) = token.split_once(':')?;
    if minutes.is_empty() || !minutes.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (seconds, fraction) = match rest.split_once('.') {
        Some((s, f)) => (s, Some(f)),
        None => (rest, None),
    };
    let seconds_bytes = seconds.as_bytes();
    if seconds_bytes.len() != 2 || !seconds_bytes.iter().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let seconds: u64 = seconds.parse().ok()?;
    if seconds > 59 {
        return None;
    }
    let fraction_ms: u64 = match fraction {
        None => 0,
        Some(f) if f.len() == 2 && f.bytes().all(|b| b.is_ascii_digit()) => {
            f.parse::<u64>().ok()? * 10
        }
        Some(f) if f.len() == 3 && f.bytes().all(|b| b.is_ascii_digit()) => f.parse().ok()?,
        Some(_) => return None,
    };
    let minutes: u64 = minutes.parse().ok()?;
    let total = minutes
        .checked_mul(60)?
        .checked_add(seconds)?
        .checked_mul(1000)?
        .checked_add(fraction_ms)?;
    if total > i64::MAX as u64 {
        return None;
    }
    Some(total as i64)
}

/// Parses the `[offset:…]` value: an optionally signed integer of
/// milliseconds within `±MAX_OFFSET_MS` (spec §4.4).
fn parse_offset(value: &str) -> Option<i64> {
    let (sign, digits) = match value.strip_prefix('-') {
        Some(rest) => (-1i64, rest),
        None => (1i64, value.strip_prefix('+').unwrap_or(value)),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let magnitude: i64 = digits.parse().ok()?;
    if magnitude > MAX_OFFSET_MS {
        return None;
    }
    Some(sign * magnitude)
}

/// Last occurrence wins for repeated metadata tags (spec §4.4); an
/// empty value clears the field.
fn set_last(slot: &mut Option<String>, value: &str) {
    if value.is_empty() {
        *slot = None;
    } else {
        *slot = Some(value.to_string());
    }
}

/// Strips A2-enhanced word tags (`<mm:ss.xx>`) from line text — the
/// defined §4.5 reduction. A `<…>` span whose content is
/// timestamp-shaped (same shape rule as bracket tokens) is removed and
/// reported; anything else is literal text (`"i <3 you"` survives).
fn strip_word_tags(text: &str) -> (String, bool) {
    if !text.contains('<') {
        return (text.to_string(), false);
    }
    let mut out = String::with_capacity(text.len());
    let mut stripped = false;
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let (before, after) = rest.split_at(open);
        out.push_str(before);
        match after.find('>') {
            Some(close) => {
                let content = &after[1..close];
                if is_timestamp_shaped(content) {
                    stripped = true;
                    rest = &after[close + 1..];
                } else {
                    out.push('<');
                    rest = &after[1..];
                }
            }
            None => {
                out.push('<');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    (out, stripped)
}
