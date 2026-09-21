//! Minimal JSON value model for the read-only API projection.
//!
//! The legacy server builds responses with a tiny ordered JSON builder
//! (`json.c`): objects preserve insertion order, strings use C's exact
//! escaping (`\"` `\\` `\b` `\f` `\n` `\r` `\t`, `\u00xx` lowercase hex for
//! other bytes below `0x20`, raw UTF-8 otherwise), integers print as
//! `%lld`, and doubles as `%.10g`. This module reproduces all three, so
//! response bodies are byte-identical to the C server. `NULL` columns map
//! to `""` through [`Json::jum_string`] (the C `mp_json_str`) and to key
//! omission through [`Json::opt_string`] (the C `mp_json_str_opt`).

/// A JSON value with insertion-ordered objects.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Int(i64),
    Float(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// An empty object.
    pub fn obj() -> Self {
        Json::Obj(Vec::new())
    }

    /// An empty array.
    pub fn arr() -> Self {
        Json::Arr(Vec::new())
    }

    /// Appends a member to an object (no-op on non-objects).
    pub fn member(&mut self, key: &str, value: Json) {
        if let Json::Obj(members) = self {
            members.push((key.to_string(), value));
        }
    }

    /// Appends an element to an array (no-op on non-arrays).
    pub fn push(&mut self, value: Json) {
        if let Json::Arr(items) = self {
            items.push(value);
        }
    }

    /// String member; a `None` (SQL NULL) column becomes `""`, exactly like
    /// the C `mp_json_str`.
    pub fn string(&mut self, key: &str, value: Option<&str>) {
        self.member(key, Json::Str(value.unwrap_or("").to_string()));
    }

    /// Optional string member; omitted when the column is NULL or empty,
    /// exactly like the C `mp_json_str_opt`.
    pub fn opt_string(&mut self, key: &str, value: Option<&str>) {
        match value {
            Some(v) if !v.is_empty() => self.member(key, Json::Str(v.to_string())),
            _ => {}
        }
    }

    /// Integer member (`%lld`).
    pub fn int(&mut self, key: &str, value: i64) {
        self.member(key, Json::Int(value));
    }

    /// Double member (`%.10g`).
    pub fn dbl(&mut self, key: &str, value: f64) {
        self.member(key, Json::Float(value));
    }

    /// Explicit null member.
    pub fn null(&mut self, key: &str) {
        self.member(key, Json::Null);
    }

    /// Renders the value (compact, no whitespace — the C `mp_json_render`).
    pub fn render(&self) -> String {
        let mut out = String::new();
        render_into(&mut out, self);
        out
    }

    /// The standard error envelope: `{"error":{"code","message"}}`.
    pub fn error(code: &str, message: &str) -> String {
        let mut inner = Json::obj();
        inner.member("code", Json::Str(code.to_string()));
        inner.member("message", Json::Str(message.to_string()));
        let mut outer = Json::obj();
        outer.member("error", inner);
        outer.render()
    }
}

fn render_into(out: &mut String, value: &Json) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Int(v) => out.push_str(&v.to_string()),
        Json::Float(v) => out.push_str(&format_g10(*v)),
        Json::Str(s) => escape_into(out, s),
        Json::Arr(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                render_into(out, item);
            }
            out.push(']');
        }
        Json::Obj(members) => {
            out.push('{');
            for (i, (key, value)) in members.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                escape_into(out, key);
                out.push(':');
                render_into(out, value);
            }
            out.push('}');
        }
    }
}

/// Escapes a string exactly like the C `sbuf_esc`: the seven short escapes,
/// `\u00xx` (lowercase hex) for other control bytes, raw bytes otherwise
/// (UTF-8 passes through untouched).
pub fn escape_into(out: &mut String, value: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    // Byte-built (then converted once): the passthrough arm must copy raw
    // bytes, exactly like the C `sbuf_char` appending `(char) c`, and a
    // `String` cannot take raw bytes without `unsafe`. The result is valid
    // UTF-8 by construction (ASCII escapes plus the original valid bytes).
    let mut buf: Vec<u8> = Vec::with_capacity(value.len() + 2);
    buf.push(b'"');
    for &b in value.as_bytes() {
        match b {
            b'"' => buf.extend_from_slice(b"\\\""),
            b'\\' => buf.extend_from_slice(b"\\\\"),
            0x08 => buf.extend_from_slice(b"\\b"),
            0x0c => buf.extend_from_slice(b"\\f"),
            b'\n' => buf.extend_from_slice(b"\\n"),
            b'\r' => buf.extend_from_slice(b"\\r"),
            b'\t' => buf.extend_from_slice(b"\\t"),
            0x00..=0x1f => {
                buf.extend_from_slice(b"\\u00");
                buf.push(HEX[(b >> 4) as usize]);
                buf.push(HEX[(b & 0x0f) as usize]);
            }
            _ => buf.push(b),
        }
    }
    buf.push(b'"');
    out.push_str(&String::from_utf8(buf).expect("escaped JSON is UTF-8"));
}

/// Formats a double like C `printf("%.10g")`.
///
/// Ten significant digits, correctly rounded (taken from Rust's `{:.9e}`
/// scientific rendering), trailing zeros stripped, exponential form when
/// the decimal exponent is `< -4` or `>= 10`, with a C-style exponent
/// (`e-05`, `e+14` — at least two digits). Non-finite inputs cannot arise
/// from the database (manifest validation rejects them); they fall back to
/// Rust's rendering rather than panicking.
pub fn format_g10(value: f64) -> String {
    if !value.is_finite() {
        return format!("{value}");
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0".to_string()
        } else {
            "0".to_string()
        };
    }
    // Ten significant digits: `1.234567890e+14` shape, mantissa normalized
    // to [1, 10) by the formatter.
    let sci = format!("{value:.9e}");
    let (mantissa, exp): (&str, i32) = match sci.rsplit_once('e') {
        Some((m, e)) => (m, e.parse().unwrap_or(0)),
        None => return format!("{value}"),
    };
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa
        .bytes()
        .filter(|b| b.is_ascii_digit())
        .map(|b| b as char)
        .collect();
    let stripped = digits.trim_end_matches('0');
    let significant = if stripped.is_empty() {
        "0".to_string()
    } else {
        stripped.to_string()
    };
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    if (-4..10).contains(&exp) {
        // Fixed notation.
        if exp >= 0 {
            let int_len = exp as usize + 1;
            if significant.len() <= int_len {
                out.push_str(&significant);
                out.push_str(&"0".repeat(int_len - significant.len()));
            } else {
                out.push_str(&significant[..int_len]);
                out.push('.');
                out.push_str(&significant[int_len..]);
            }
        } else {
            out.push_str("0.");
            out.push_str(&"0".repeat((-exp - 1) as usize));
            out.push_str(&significant);
        }
    } else {
        // Exponential notation: `d[.ddd]e±XX` (exponent ≥ 2 digits).
        out.push(significant.chars().next().unwrap_or('0'));
        if significant.len() > 1 {
            out.push('.');
            out.push_str(&significant[1..]);
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        let abs = exp.unsigned_abs().to_string();
        if abs.len() < 2 {
            out.push('0');
        }
        out.push_str(&abs);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_matches_the_c_builder() {
        let mut out = String::new();
        escape_into(&mut out, "a\"b\\c\n\td\x01\x1fé");
        assert_eq!(out, "\"a\\\"b\\\\c\\n\\td\\u0001\\u001fé\"");
        let mut backspace = String::new();
        escape_into(&mut backspace, "\x08\x0c\r");
        assert_eq!(backspace, "\"\\b\\f\\r\"");
    }

    #[test]
    fn null_and_opt_string_follow_the_c_rules() {
        let mut o = Json::obj();
        o.string("name", None);
        o.opt_string("missing", None);
        o.opt_string("empty", Some(""));
        o.opt_string("kept", Some("x"));
        o.int("n", -3);
        o.null("nothing");
        assert_eq!(
            o.render(),
            r#"{"name":"","kept":"x","n":-3,"nothing":null}"#
        );
    }

    #[test]
    fn error_envelope_shape() {
        assert_eq!(
            Json::error("not_found", "Album not found"),
            r#"{"error":{"code":"not_found","message":"Album not found"}}"#
        );
    }

    #[test]
    fn g10_matches_c_printf_vectors() {
        // Generated with CPython `{:.10g}` (C99-conformant) for the same
        // f64 values.
        let cases: &[(f64, &str)] = &[
            (0.0, "0"),
            (-0.0, "-0"),
            (1.0, "1"),
            (-1.0, "-1"),
            (180.5, "180.5"),
            (-7.1915088, "-7.1915088"),
            (-4.1897838, "-4.1897838"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (1e-5, "1e-05"),
            (123456789.12345679, "123456789.1"),
            (1e10, "1e+10"),
            (1e21, "1e+21"),
            (std::f64::consts::PI, "3.141592654"),
            (100.0, "100"),
            (0.5, "0.5"),
            (2.5e-7, "2.5e-07"),
            (9999999999.0, "9999999999"),
            (0.123456789012345, "0.123456789"),
            (1.5, "1.5"),
            (-0.001, "-0.001"),
            (123456789012345.0, "1.23456789e+14"),
        ];
        for (value, expected) in cases {
            assert_eq!(&format_g10(*value), expected, "value {value}");
        }
    }
}
