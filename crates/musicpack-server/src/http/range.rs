//! Single-range parsing — a byte-exact port of the reference's `range.c`.
//!
//! Only `bytes=` single ranges are supported (a comma anywhere rejects,
//! like the C); `MP_RANGE_INVALID` and `MP_RANGE_UNSATISFIABLE` both
//! answer `416` with `Content-Range: bytes */N`. Arithmetic is `i64`
//! with the C's exact overflow rejection (`v > (i64::MAX - d) / 10`).
//!
//! Preserved edge semantics (all pinned by unit tests below):
//!
//! - `bytes=-0` is malformed (→ 416), not empty;
//! - `first >= size` is unsatisfiable — checked *before* the `last <
//!   first` rule, so `bytes=999-2` on a 100-byte object is 416 either way
//!   but for the C's reason;
//! - `bytes=5-2` is malformed (→ 416);
//! - any trailing bytes (even whitespace) are malformed;
//! - the `bytes=` prefix is case-sensitive (`Bytes=` → 416);
//! - suffix lengths clamp to the size; explicit ends clamp to `size - 1;
//! - an empty object satisfies nothing (`bytes=0-` on 0 bytes → 416).

/// A satisfiable byte range: `start` and `length` (the C `mp_range`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    /// First byte position.
    pub start: u64,
    /// Byte count (`last - first + 1` after clamping).
    pub length: u64,
}

/// The parse outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeOutcome {
    /// Satisfiable (`out` holds start/length).
    Ok(ByteRange),
    /// Malformed, non-`bytes` unit, or multiple ranges (→ 416).
    Invalid,
    /// Well-formed but unsatisfiable (→ 416 with the same headers).
    Unsatisfiable,
}

/// Parses one decimal number (`parse_num`): leading digit required, `+`
/// rejected, empty rejected, `i64` overflow rejected. Returns the value
/// and the byte offset just past the digits.
fn parse_num(input: &[u8]) -> Option<(i64, usize)> {
    let first = *input.first()?;
    if !first.is_ascii_digit() {
        return None;
    }
    let mut value: i64 = 0;
    let mut i = 0;
    while i < input.len() && input[i].is_ascii_digit() {
        let digit = (input[i] - b'0') as i64;
        if value > (i64::MAX - digit) / 10 {
            return None;
        }
        value = value * 10 + digit;
        i += 1;
    }
    Some((value, i))
}

/// Parses `header` against an object of `size` bytes (`mp_range_parse`).
pub fn parse_range(header: &str, size: i64) -> RangeOutcome {
    let bytes = header.as_bytes();
    if !bytes.starts_with(b"bytes=") {
        return RangeOutcome::Invalid;
    }
    let rest = &bytes["bytes=".len()..];
    // Multiple ranges are rejected.
    if rest.contains(&b',') {
        return RangeOutcome::Invalid;
    }

    if rest.first() == Some(&b'-') {
        // Suffix range: `bytes=-N` (last N bytes).
        let (first, used) = match parse_num(&rest[1..]) {
            Some(v) => v,
            None => return RangeOutcome::Invalid,
        };
        if 1 + used != rest.len() {
            return RangeOutcome::Invalid;
        }
        if first == 0 {
            return RangeOutcome::Invalid;
        }
        if size == 0 {
            return RangeOutcome::Unsatisfiable;
        }
        let first = first.min(size);
        return RangeOutcome::Ok(ByteRange {
            start: (size - first) as u64,
            length: first as u64,
        });
    }

    let (first, mut used) = match parse_num(rest) {
        Some(v) => v,
        None => return RangeOutcome::Invalid,
    };
    if rest.get(used) != Some(&b'-') {
        return RangeOutcome::Invalid;
    }
    used += 1;
    let (has_last, last) = if used == rest.len() {
        (false, 0)
    } else {
        match parse_num(&rest[used..]) {
            Some((v, n)) if used + n == rest.len() => (true, v),
            _ => return RangeOutcome::Invalid,
        }
    };
    if first >= size {
        return RangeOutcome::Unsatisfiable;
    }
    if has_last && last < first {
        return RangeOutcome::Invalid;
    }
    let mut last = last;
    if has_last && last >= size {
        last = size - 1;
    }
    if !has_last {
        last = size - 1;
    }
    RangeOutcome::Ok(ByteRange {
        start: first as u64,
        length: (last - first + 1) as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(header: &str, size: i64) -> Option<(u64, u64)> {
        match parse_range(header, size) {
            RangeOutcome::Ok(r) => Some((r.start, r.length)),
            _ => None,
        }
    }

    fn outcome(header: &str, size: i64) -> RangeOutcome {
        parse_range(header, size)
    }

    #[test]
    fn canonical_ranges() {
        assert_eq!(ok("bytes=0-99", 1000), Some((0, 100)));
        assert_eq!(ok("bytes=500-", 1000), Some((500, 500)));
        assert_eq!(ok("bytes=-100", 1000), Some((900, 100)));
        assert_eq!(ok("bytes=0-0", 1000), Some((0, 1)));
        // Explicit end clamps to size - 1.
        assert_eq!(ok("bytes=999-1999", 1000), Some((999, 1)));
        assert_eq!(ok("bytes=0-999", 1000), Some((0, 1000)));
        // Suffix longer than the object clamps to the whole object.
        assert_eq!(ok("bytes=-9999", 1000), Some((0, 1000)));
        // Single last byte.
        assert_eq!(ok("bytes=999-", 1000), Some((999, 1)));
    }

    #[test]
    fn unsatisfiable_ranges() {
        use RangeOutcome::Unsatisfiable;
        // First byte at/past EOF.
        assert_eq!(outcome("bytes=1000-", 1000), Unsatisfiable);
        assert_eq!(outcome("bytes=5000-6000", 1000), Unsatisfiable);
        // Checked before the last<first rule (the C order).
        assert_eq!(outcome("bytes=999-2", 100), Unsatisfiable);
        // Empty object satisfies nothing.
        assert_eq!(outcome("bytes=0-", 0), Unsatisfiable);
        assert_eq!(outcome("bytes=-5", 0), Unsatisfiable);
        assert_eq!(outcome("bytes=0-0", 0), Unsatisfiable);
    }

    #[test]
    fn malformed_ranges() {
        use RangeOutcome::Invalid;
        for header in [
            "bytes=",
            "bytes",
            "Bytes=0-1",
            "bytes=0-1,2-3",
            "bytes=abc",
            "bytes=5-2",
            "bytes=-0",
            "bytes=0-1 ",
            "bytes= 0-1",
            "bytes=+1-2",
            "bytes=1-2x",
            "bytes=--5",
            "bytes=-",
            "items=0-1",
            "",
            "bytes=0-99999999999999999999999",
        ] {
            assert_eq!(outcome(header, 1000), Invalid, "{header:?}");
        }
    }

    #[test]
    fn zero_size_edge_cases() {
        // Suffix zero is malformed even for empty objects (checked first).
        assert_eq!(outcome("bytes=-0", 0), RangeOutcome::Invalid);
    }
}
