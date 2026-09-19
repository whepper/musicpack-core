//! Ordered JSON value, strict parser, and canonical printer.
//!
//! The manifest grammar is "the JSON the C reference actually accepts" —
//! i.e. the vendored cJSON parser plus the library's own checks. This
//! module ports that behaviour exactly, because accept/reject decisions are
//! compatibility surface. The notable cJSON behaviours (all deliberate
//! ports, see `docs/architecture.md` §4):
//!
//! - a UTF-8 BOM at the very start of the input is skipped;
//! - "whitespace" is any byte ≤ 0x20 (not just space/tab/CR/LF), between
//!   all tokens and before the end-of-input check;
//! - raw control bytes *inside* strings are accepted (copied verbatim);
//! - numbers start with `-` or a digit, are copied from the charset
//!   `[0-9+-.eE]` (at most 63 characters) and then parsed like C `strtod`
//!   (longest valid prefix, overflow → ±∞, so `01`, `1.`, `1e999` are all
//!   accepted; `.5` and `+1` are rejected at the value gate);
//! - `\uXXXX` escapes: unpaired surrogates (high or low) are rejected;
//! - a NUL character terminates a string (the C library stores strings as
//!   C strings; everything from the first NUL is invisible to it). This
//!   module truncates decoded strings (and keys) at the first U+0000;
//! - container nesting is bounded (cJSON `CJSON_NESTING_LIMIT` = 1000).
//!   Note: `specs/musicpack-v1.md` §8 claims 100; the implementation
//!   enforces 1000. The implementation is the behavioural authority; see
//!   the discrepancy log in `docs/architecture.md`;
//! - after the root value only whitespace may follow (the manifest parse
//!   uses cJSON's `require_null_terminated` mode; trailing non-whitespace
//!   bytes are a `Json` error).
//!
//! Member order is preserved (`Object` is a `Vec` of pairs), because the
//! canonical writer must re-emit unknown root fields in their original
//! order.

use std::fmt;

use crate::format::number::format_json_number;

/// Maximum container nesting accepted by the parser.
///
/// Matches the vendored cJSON `CJSON_NESTING_LIMIT` (1000), **not** the
/// spec's stated 100 — see the module documentation and the discrepancy
/// log in `docs/architecture.md`.
pub const NESTING_LIMIT: usize = 1000;

/// A parsed JSON value. Order-preserving for objects.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// `null`
    Null,
    /// `true` / `false`
    Bool(bool),
    /// A number (always the `f64` the C library would store).
    Number(f64),
    /// A string (NUL-truncated like the C library's `char *`).
    String(String),
    /// An array (in order).
    Array(Vec<Value>),
    /// An object (members in document order; duplicates are rejected by
    /// the manifest parser but representable here).
    Object(Vec<(String, Value)>),
}

impl Value {
    /// Returns the object member list, or `None` for non-objects.
    pub fn as_object(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Object(members) => Some(members),
            _ => None,
        }
    }

    /// Case-sensitive key lookup, mirroring
    /// `cJSON_GetObjectItemCaseSensitive`: returns the first match.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// `true` when this value is an object (cJSON_IsObject).
    pub fn is_object(&self) -> bool {
        matches!(self, Value::Object(_))
    }

    /// `true` when this value is an array (cJSON_IsArray).
    pub fn is_array(&self) -> bool {
        matches!(self, Value::Array(_))
    }

    /// `true` when this value is a string (cJSON_IsString).
    pub fn is_string(&self) -> bool {
        matches!(self, Value::String(_))
    }

    /// `true` when this value is a number (cJSON_IsNumber).
    pub fn is_number(&self) -> bool {
        matches!(self, Value::Number(_))
    }

    /// True when any object at any depth contains a duplicate key.
    ///
    /// Port of `has_duplicate_keys` (`manifest.c`): applied to the whole
    /// parsed tree before any semantic parsing; arrays are recursed into.
    pub fn has_duplicate_keys(&self) -> bool {
        match self {
            Value::Object(members) => {
                for (i, (key, _)) in members.iter().enumerate() {
                    for (other_key, _) in members.iter().skip(i + 1) {
                        if key == other_key {
                            return true;
                        }
                    }
                }
                members.iter().any(|(_, v)| v.has_duplicate_keys())
            }
            Value::Array(items) => items.iter().any(Value::has_duplicate_keys),
            _ => false,
        }
    }
}

/// JSON parse failure (`MUSICPACK_ERR_JSON` territory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    /// Byte offset where the failure was detected.
    pub position: usize,
    /// What went wrong.
    pub detail: String,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at byte {})", self.detail, self.position)
    }
}

impl std::error::Error for JsonError {}

/// Parses a complete JSON document from bytes, mirroring the vendored
/// cJSON parser with `require_null_terminated` semantics (the mode the
/// manifest parse uses).
///
/// The input is exact bytes (the manifest file contents). Unlike the C
/// entry point there is no `strlen` shortening: an embedded NUL is just a
/// byte (inside strings it truncates the decoded value, exactly like the
/// C library's observable behaviour).
pub fn parse(bytes: &[u8]) -> Result<Value, JsonError> {
    // The Rust model stores strings as `String` (UTF-8), so raw non-UTF-8
    // bytes in string literals are rejected here. The C library copies
    // such bytes verbatim; this is a documented behavioural difference
    // (docs/architecture.md, Open Questions).
    let text = std::str::from_utf8(bytes).map_err(|e| JsonError {
        position: e.valid_up_to(),
        detail: "input is not valid UTF-8".to_string(),
    })?;
    let mut p = Parser {
        bytes: text.as_bytes(),
        pos: 0,
        depth: 0,
    };

    // skip_utf8_bom: only at offset 0.
    if p.bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        p.pos = 3;
    }
    p.skip_ws();
    let value = p.parse_value()?;
    p.skip_ws();
    if p.pos < p.bytes.len() {
        // Trailing content after the root value (require_null_terminated).
        return Err(p.err("trailing data after JSON document"));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, detail: &str) -> JsonError {
        JsonError {
            position: self.pos,
            detail: detail.to_string(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    /// cJSON "whitespace": any byte ≤ 0x20.
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos] <= 0x20 {
            self.pos += 1;
        }
    }

    fn parse_value(&mut self) -> Result<Value, JsonError> {
        match self.peek() {
            None => Err(self.err("unexpected end of input")),
            Some(b'n') if self.bytes[self.pos..].starts_with(b"null") => {
                self.pos += 4;
                Ok(Value::Null)
            }
            Some(b'f') if self.bytes[self.pos..].starts_with(b"false") => {
                self.pos += 5;
                Ok(Value::Bool(false))
            }
            Some(b't') if self.bytes[self.pos..].starts_with(b"true") => {
                self.pos += 4;
                Ok(Value::Bool(true))
            }
            Some(b'"') => self.parse_string().map(Value::String),
            Some(b'-') | Some(b'0'..=b'9') => self.parse_number(),
            Some(b'[') => self.parse_array(),
            Some(b'{') => self.parse_object(),
            Some(_) => Err(self.err("unexpected character")),
        }
    }

    fn parse_array(&mut self) -> Result<Value, JsonError> {
        if self.depth >= NESTING_LIMIT {
            return Err(self.err("nesting too deep"));
        }
        self.depth += 1;
        self.pos += 1; // '['
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.skip_ws();
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    self.depth -= 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.err("expected ',' or ']'")),
            }
        }
    }

    fn parse_object(&mut self) -> Result<Value, JsonError> {
        if self.depth >= NESTING_LIMIT {
            return Err(self.err("nesting too deep"));
        }
        self.depth += 1;
        self.pos += 1; // '{'
        let mut members: Vec<(String, Value)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(Value::Object(members));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(self.err("expected ':'"));
            }
            self.pos += 1;
            self.skip_ws();
            let value = self.parse_value()?;
            members.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    self.depth -= 1;
                    return Ok(Value::Object(members));
                }
                _ => return Err(self.err("expected ',' or '}'")),
            }
        }
    }

    /// Parses a string literal. Decodes escapes with cJSON's surrogate
    /// rules, then truncates at the first NUL (C-string semantics).
    fn parse_string(&mut self) -> Result<String, JsonError> {
        if self.peek() != Some(b'"') {
            return Err(self.err("expected string"));
        }
        let start = self.pos + 1;
        // Find the closing quote, skipping escaped characters exactly like
        // cJSON's pre-scan (a backslash consumes the following byte).
        let mut end = start;
        while end < self.bytes.len() {
            let c = self.bytes[end];
            if c == b'"' {
                break;
            }
            if c == b'\\' {
                if end + 1 >= self.bytes.len() {
                    return Err(JsonError {
                        position: end,
                        detail: "unterminated escape".to_string(),
                    });
                }
                end += 2;
            } else {
                end += 1;
            }
        }
        if end >= self.bytes.len() {
            return Err(self.err("unterminated string"));
        }
        let literal = &self.bytes[start..end];
        let mut out = String::new();
        let mut i = 0;
        while i < literal.len() {
            let c = literal[i];
            if c != b'\\' {
                // Raw bytes are copied verbatim, including control bytes
                // and NUL (cJSON is lenient here; strict JSON is not).
                // The input is valid UTF-8, so decode raw runs as such.
                let run_start = i;
                while i < literal.len() && literal[i] != b'\\' {
                    i += 1;
                }
                out.push_str(
                    std::str::from_utf8(&literal[run_start..i])
                        .map_err(|_| self.err("invalid UTF-8 in string"))?,
                );
                continue;
            }
            i += 1;
            let esc = *literal
                .get(i)
                .ok_or_else(|| self.err("unterminated escape"))?;
            match esc {
                b'b' => {
                    out.push('\u{8}');
                    i += 1;
                }
                b'f' => {
                    out.push('\u{c}');
                    i += 1;
                }
                b'n' => {
                    out.push('\n');
                    i += 1;
                }
                b'r' => {
                    out.push('\r');
                    i += 1;
                }
                b't' => {
                    out.push('\t');
                    i += 1;
                }
                b'"' | b'\\' | b'/' => {
                    out.push(esc as char);
                    i += 1;
                }
                b'u' => {
                    let rest = &literal[i + 1..];
                    if rest.len() < 4 {
                        return Err(self.err("truncated \\u escape"));
                    }
                    let hex = |b: u8| -> Option<u16> {
                        match b {
                            b'0'..=b'9' => Some((b - b'0') as u16),
                            b'a'..=b'f' => Some((b - b'a' + 10) as u16),
                            b'A'..=b'F' => Some((b - b'A' + 10) as u16),
                            _ => None,
                        }
                    };
                    let mut code: u16 = 0;
                    for &h in &rest[..4] {
                        code =
                            (code << 4) + hex(h).ok_or_else(|| self.err("invalid \\u escape"))?;
                    }
                    i += 5;
                    let codepoint: u32 = match code {
                        // cJSON rejects lone low surrogates outright, and
                        // requires a valid low surrogate after a high one.
                        0xDC00..=0xDFFF => {
                            return Err(self.err("lone low surrogate"));
                        }
                        0xD800..=0xDBFF => {
                            if rest.len() < 10 || rest[4] != b'\\' || rest[5] != b'u' {
                                return Err(self.err("unpaired high surrogate"));
                            }
                            let mut low: u16 = 0;
                            for &h in &rest[6..10] {
                                low = (low << 4)
                                    + hex(h).ok_or_else(|| self.err("invalid \\u escape"))?;
                            }
                            if !(0xDC00..=0xDFFF).contains(&low) {
                                return Err(self.err("invalid low surrogate"));
                            }
                            i += 6;
                            0x10000 + (((code as u32) & 0x3FF) << 10) + ((low as u32) & 0x3FF)
                        }
                        _ => code as u32,
                    };
                    out.push(
                        char::from_u32(codepoint).ok_or_else(|| self.err("invalid codepoint"))?,
                    );
                }
                _ => return Err(self.err("invalid escape")),
            }
        }
        self.pos = end + 1;
        // C-string semantics: everything from the first NUL is invisible.
        Ok(match out.find('\0') {
            Some(n) => out[..n].to_string(),
            None => out,
        })
    }

    /// Parses a number with cJSON's grammar: charset copy (≤ 63 bytes)
    /// then C-`strtod`-style longest-prefix conversion.
    fn parse_number(&mut self) -> Result<Value, JsonError> {
        let start = self.pos;
        let mut len = 0;
        while len < 63 {
            match self.bytes.get(start + len) {
                Some(b'0'..=b'9') | Some(b'+') | Some(b'-') | Some(b'e') | Some(b'E')
                | Some(b'.') => len += 1,
                _ => break,
            }
        }
        let prefix = &self.bytes[start..start + len];
        let consumed = strtod_scan(prefix);
        if consumed == 0 {
            return Err(self.err("invalid number"));
        }
        // `consumed` bytes form a valid C strtod number; Rust's f64 parser
        // is correctly rounded like strtod and accepts the same shapes.
        let text = std::str::from_utf8(&prefix[..consumed]).expect("number charset is ASCII");
        let value: f64 = text.parse().map_err(|_| self.err("invalid number"))?;
        self.pos = start + consumed;
        Ok(Value::Number(value))
    }
}

/// Mirrors C `strtod`'s longest-valid-prefix consumption over an ASCII
/// byte slice drawn from the number charset. Returns the number of bytes
/// consumed (0 if none).
fn strtod_scan(bytes: &[u8]) -> usize {
    let mut i = 0;
    if matches!(bytes.first(), Some(b'+') | Some(b'-')) {
        i += 1;
    }
    let int_digits = {
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        i - start
    };
    let frac_digits = if i < bytes.len() && bytes[i] == b'.' {
        let start = i + 1;
        let mut j = i + 1;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        i = j;
        j - start
    } else {
        0
    };
    // strtod requires at least one digit in the significand.
    if int_digits == 0 && frac_digits == 0 {
        return 0;
    }
    // Optional exponent (only consumed when fully formed).
    if i < bytes.len() && matches!(bytes[i], b'e' | b'E') {
        let mut j = i + 1;
        if j < bytes.len() && matches!(bytes[j], b'+' | b'-') {
            j += 1;
        }
        let exp_start = j;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_start {
            i = j;
        }
    }
    i
}

/// Prints a value in the canonical manifest form: 2-space indent, `": "`
/// member separator, empty containers inline, numbers via
/// [`crate::format::number::format_json_number`], strings escaped with
/// lowercase `\u00xx` for control bytes, and a single trailing newline.
///
/// Port of the custom canonical printer in `manifest.c`
/// (`musicpack_json_print`, which appends the final newline).
pub fn print_canonical(value: &Value) -> String {
    let mut out = String::new();
    print_value(value, 0, &mut out);
    out.push('\n');
    out
}

fn print_indent(depth: usize, out: &mut String) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn print_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let code = c as u32;
                out.push_str(&format!("\\u{code:04x}"));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn print_value(value: &Value, depth: usize, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(false) => out.push_str("false"),
        Value::Bool(true) => out.push_str("true"),
        Value::Number(n) => out.push_str(&format_json_number(*n)),
        Value::String(s) => print_string(s, out),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                print_indent(depth + 1, out);
                print_value(item, depth + 1, out);
                if i + 1 < items.len() {
                    out.push_str(",\n");
                } else {
                    out.push('\n');
                }
            }
            print_indent(depth, out);
            out.push(']');
        }
        Value::Object(members) => {
            if members.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            for (i, (key, val)) in members.iter().enumerate() {
                print_indent(depth + 1, out);
                print_string(key, out);
                out.push_str(": ");
                print_value(val, depth + 1, out);
                if i + 1 < members.len() {
                    out.push_str(",\n");
                } else {
                    out.push('\n');
                }
            }
            print_indent(depth, out);
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> Value {
        parse(s.as_bytes()).expect("parses")
    }

    fn err(s: &[u8]) -> JsonError {
        parse(s).expect_err("rejects")
    }

    #[test]
    fn parses_json_shapes() {
        assert_eq!(ok("null"), Value::Null);
        assert_eq!(ok(" true "), Value::Bool(true));
        assert_eq!(ok("false"), Value::Bool(false));
        assert_eq!(ok("1.5"), Value::Number(1.5));
        assert_eq!(ok("\"hi\""), Value::String("hi".into()));
        let v = ok("{\"a\": [1, 2], \"b\": {\"c\": null}}");
        assert_eq!(v.get("b").and_then(|b| b.get("c")), Some(&Value::Null));
        // Order is preserved.
        assert_eq!(
            v.as_object()
                .map(|m| m.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>()),
            Some(vec!["a", "b"])
        );
    }

    #[test]
    fn bom_and_whitespace_quirks() {
        // BOM is skipped at offset 0.
        let bom = [0xEF, 0xBB, 0xBF];
        let mut doc = bom.to_vec();
        doc.extend_from_slice(b"{}");
        assert!(parse(&doc).is_ok());
        // Whitespace before a BOM is NOT skipped first: parse fails.
        let mut doc2 = b" ".to_vec();
        doc2.extend_from_slice(&bom);
        doc2.extend_from_slice(b"{}");
        assert!(parse(&doc2).is_err());
        // cJSON "whitespace" is any byte ≤ 0x20, including \x01.
        assert!(parse(b"{}\x01").is_ok());
        assert!(parse(b"{\x01\"a\"\x01:\x021\x01,\x01\"b\":2\x01}").is_ok());
        // Non-whitespace trailing byte is rejected.
        assert_eq!(err(b"{} x").detail, "trailing data after JSON document");
        // The conformance `trailing-json` and `nul-suffix` shapes.
        assert!(parse(b"{\"a\":1} trailing").is_err());
        assert!(parse(b"{\"a\":1}\0suffix").is_err());
    }

    #[test]
    fn strict_trailing_rules() {
        assert!(parse(b"{} ").is_ok());
        assert!(parse(b"{}\n\n").is_ok());
        assert!(parse(b"{}x").is_err());
    }

    #[test]
    fn duplicate_keys_representable_but_detectable() {
        let v = ok(r#"{"a": 1, "a": 2}"#);
        assert!(v.has_duplicate_keys());
        let v = ok(r#"{"x": {"a": 1, "a": 2}, "y": [{"b": 1, "b": 2}]}"#);
        assert!(v.has_duplicate_keys());
        let v = ok(r#"{"a": 1, "b": {"a": 2}}"#);
        assert!(!v.has_duplicate_keys());
    }

    #[test]
    fn number_grammar_mirrors_cjson() {
        // Accepted by cJSON (strtod semantics), though not strict JSON.
        assert_eq!(ok("01"), Value::Number(1.0));
        assert_eq!(ok("1."), Value::Number(1.0));
        assert_eq!(ok("-0"), Value::Number(-0.0));
        assert_eq!(ok("1e2"), Value::Number(100.0));
        assert_eq!(ok("1E+2"), Value::Number(100.0));
        assert_eq!(ok("1.5e-2"), Value::Number(0.015));
        // Overflow behaves like strtod: ±inf, no error.
        assert_eq!(ok("1e999"), Value::Number(f64::INFINITY));
        assert_eq!(ok("-1e999"), Value::Number(f64::NEG_INFINITY));
        // Underflow → 0.
        assert_eq!(ok("1e-999"), Value::Number(0.0));
        // Rejected at the value gate (cJSON requires '-' or digit first).
        assert!(parse(b".5").is_err());
        assert!(parse(b"+1").is_err());
        // Hex and literals are not numbers.
        assert!(parse(b"0x10").is_err());
        assert!(parse(b"Infinity").is_err());
        assert!(parse(b"NaN").is_err());
        // Malformed numbers fail.
        assert!(parse(b"-").is_err());
        assert!(parse(b"1e").is_err());
        assert!(parse(b"1..2").is_err());
        // 63-character truncation quirk: a long number consumes only what
        // strtod does, then the remainder is a new token → trailing data.
        let long = format!("{}x", "1".repeat(80));
        assert!(parse(long.as_bytes()).is_err());
    }

    #[test]
    fn string_rules_mirroring_cjson() {
        assert_eq!(ok(r#""a\/b""#), Value::String("a/b".into()));
        assert_eq!(ok(r#""A""#), Value::String("A".into()));
        assert_eq!(ok("\"\\u0041\""), Value::String("A".into()));
        // Surrogate pair decodes.
        assert_eq!(ok("\"\\ud83d\\ude00\""), Value::String("\u{1F600}".into()));
        // Unpaired surrogates rejected (both directions, like cJSON).
        assert!(parse(b"\"\\ud83d\"").is_err());
        assert!(parse(b"\"\\ude00\"").is_err());
        assert!(parse(b"\"\\ud83d\\u0041\"").is_err());
        // Raw control bytes accepted inside strings (cJSON is lenient).
        assert_eq!(ok("\"a\u{1}b\""), Value::String("a\u{1}b".into()));
        // Raw invalid UTF-8 is unrepresentable in Rust: rejected.
        let mut bad = b"\"a".to_vec();
        bad.extend_from_slice(&[0xFF]);
        bad.extend_from_slice(&[0xFE, 0x22]); // "a<FF><FE>"
        assert!(parse(&bad).is_err());
        // Invalid escapes rejected.
        assert!(parse(b"\"\\q\"").is_err());
        assert!(parse(b"\"\\u12\"").is_err());
        // NUL truncation: value and duplicate detection see "ab".
        assert_eq!(ok("\"ab\\u0000cd\""), Value::String("ab".into()));
        let v = ok("{\"ab\\u0000x\": 1, \"ab\": 2}");
        assert!(v.has_duplicate_keys());
    }

    #[test]
    fn nesting_limit_is_cjsons_1000() {
        let deep_ok = format!("{}{}", "[".repeat(1000), "]".repeat(1000));
        assert!(parse(deep_ok.as_bytes()).is_ok());
        let deep_bad = format!("{}{}", "[".repeat(1001), "]".repeat(1001));
        assert_eq!(err(deep_bad.as_bytes()).detail, "nesting too deep");
    }

    #[test]
    fn canonical_printing() {
        let v = ok(r#"{"b":1,"a":{"z":[],"y":{}},"c":[1,2.5]}"#);
        assert_eq!(
            print_canonical(&v),
            "{\n  \"b\": 1,\n  \"a\": {\n    \"z\": [],\n    \"y\": {}\n  },\n  \"c\": [\n    1,\n    2.5\n  ]\n}\n"
        );
        // Number formatting goes through the C-compatible printer.
        let v = ok(r#"{"x": -12.0, "y": 1e-5}"#);
        assert_eq!(print_canonical(&v), "{\n  \"x\": -12,\n  \"y\": 1e-05\n}\n");
        // String escaping: quotes, backslashes, control bytes; DEL raw.
        // print_canonical appends a final newline (musicpack_json_print).
        let v = Value::String("a\"b\\c\u{1}\u{7f}".into());
        assert_eq!(print_canonical(&v), "\"a\\\"b\\\\c\\u0001\u{7f}\"\n");
    }
}
