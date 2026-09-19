//! Cross-reload session snapshot codec.
//!
//! Port of `web/player-core/src/snapshot.ts` (BSD-3-Clause). Pure
//! serialize/validate/normalize; storage I/O and throttling stay host-side.
//!
//! v1 (shipped): `{v:1, items, index, positionSeconds, volume, normalizeMode}`.
//! v2 adds `repeat` and `shuffle`; `crossfadeSeconds` is accepted when it is
//! one of the shipped cycle values (4/8/12), otherwise 0.
//!
//! # Deliberate deviations from TypeScript
//!
//! - The Rust JSON parser rejects duplicate object keys; a payload with
//!   duplicates decodes as `None` (a safe no-restore), whereas
//!   `JSON.parse` would keep the last value. This is a robustness hardening,
//!   not a semantic change for well-formed snapshots.
//! - Numbers are emitted with Rust's shortest round-trip formatting rather
//!   than `JSON.stringify`'s; the output is shape-compatible JSON, not
//!   necessarily byte-identical.

use crate::json::{self, Value};

use super::types::{NormalizationMode, PlaybackItem, RepeatMode};

/// Current snapshot version.
pub const SNAPSHOT_VERSION: u32 = 2;
/// The version the web client originally shipped.
pub const SNAPSHOT_VERSION_V1: u32 = 1;

/// A normalized session snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSnapshot {
    /// Snapshot version (always [`SNAPSHOT_VERSION`] after decode).
    pub v: u32,
    /// Restorable items (raw JSON objects, preserved verbatim).
    pub items: Vec<Value>,
    /// Cursor index into `items`.
    pub index: i64,
    /// Position within the current track, in seconds.
    pub position_seconds: f64,
    /// Saved volume, when present.
    pub volume: Option<f64>,
    /// Saved normalization mode, when present.
    pub normalize_mode: Option<NormalizationMode>,
    /// Saved repeat policy (v1 payloads default to `Off`).
    pub repeat: RepeatMode,
    /// Saved shuffle flag (v1 payloads default to `false`).
    pub shuffle: bool,
    /// Crossfade seconds (0 = off; only 4/8/12 are honored).
    pub crossfade_seconds: f64,
}

/// Clamps an index into `0..length` (mirrors the historical restore clamp,
/// including `floor(NaN) || 0`).
pub fn clamp_index(index: f64, length: usize) -> i64 {
    let floored = index.floor();
    let value = if floored.is_nan() { 0.0 } else { floored };
    let hi = (length as f64 - 1.0).max(0.0);
    value.max(0.0).min(hi) as i64
}

fn number_at(v: &Value, key: &str) -> Option<f64> {
    match v.get(key) {
        Some(Value::Number(n)) => Some(*n),
        _ => None,
    }
}

fn is_restorable(item: &Value) -> bool {
    PlaybackItem::from_value(item).is_some()
}

/// Parses a v1 or v2 payload. Returns `None` when absent/corrupt/empty —
/// restoration then simply does not happen (never fatal).
pub fn decode_snapshot(raw: &str) -> Option<SessionSnapshot> {
    if raw.is_empty() {
        return None;
    }
    let value = json::parse(raw.as_bytes()).ok()?;
    // The version is validated but the decoded snapshot is normalized to the
    // current version (matching the TypeScript `v: SNAPSHOT_VERSION`).
    match number_at(&value, "v") {
        Some(v) if v == SNAPSHOT_VERSION as f64 || v == SNAPSHOT_VERSION_V1 as f64 => {}
        _ => return None,
    };
    let v = SNAPSHOT_VERSION;
    let items_value = value.get("items")?;
    let Value::Array(items) = items_value else {
        return None;
    };
    let items: Vec<Value> = items.iter().filter(|i| is_restorable(i)).cloned().collect();
    if items.is_empty() {
        return None;
    }
    let index = clamp_index(number_at(&value, "index").unwrap_or(0.0), items.len());
    let crossfade = number_at(&value, "crossfadeSeconds")
        .filter(|c| [4.0, 8.0, 12.0].contains(c))
        .unwrap_or(0.0);
    let normalize_mode = match value.get("normalizeMode") {
        Some(Value::String(s)) if matches!(s.as_str(), "off" | "album" | "track") => {
            Some(NormalizationMode::parse(s))
        }
        _ => None,
    };
    let repeat = match value.get("repeat") {
        Some(Value::String(s)) => RepeatMode::parse(s),
        _ => RepeatMode::Off,
    };
    let shuffle = matches!(value.get("shuffle"), Some(Value::Bool(true)));
    Some(SessionSnapshot {
        v,
        items,
        index,
        position_seconds: number_at(&value, "positionSeconds").unwrap_or(0.0),
        volume: number_at(&value, "volume"),
        normalize_mode,
        repeat,
        shuffle,
        crossfade_seconds: crossfade,
    })
}

/// Encodes a snapshot as JSON.
pub fn encode_snapshot(snapshot: &SessionSnapshot) -> String {
    let mut members: Vec<(String, Value)> = vec![
        ("v".into(), Value::Number(SNAPSHOT_VERSION as f64)),
        ("items".into(), Value::Array(snapshot.items.clone())),
        ("index".into(), Value::Number(snapshot.index as f64)),
        (
            "positionSeconds".into(),
            Value::Number(snapshot.position_seconds),
        ),
    ];
    if let Some(volume) = snapshot.volume {
        members.push(("volume".into(), Value::Number(volume)));
    }
    if let Some(mode) = snapshot.normalize_mode {
        members.push(("normalizeMode".into(), Value::String(mode.as_str().into())));
    }
    members.push((
        "repeat".into(),
        Value::String(snapshot.repeat.as_str().into()),
    ));
    members.push(("shuffle".into(), Value::Bool(snapshot.shuffle)));
    members.push((
        "crossfadeSeconds".into(),
        Value::Number(snapshot.crossfade_seconds),
    ));
    let mut out = String::new();
    write_value(&Value::Object(members), &mut out);
    out
}

/// Shortest round-trip JSON writer (finite → number, non-finite → `null`).
fn write_value(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if n.is_finite() {
                out.push_str(&format!("{n}"));
            } else {
                out.push_str("null");
            }
        }
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Value::Object(members) => {
            out.push('{');
            for (i, (k, v)) in members.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(k, out);
                out.push(':');
                write_value(v, out);
            }
            out.push('}');
        }
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}
