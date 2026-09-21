//! The MusicPack lyrics domain — model, timing semantics, and the strict
//! LRC parser.
//!
//! Normative specification: `docs/musicpack-lyrics-v1.md` (R3.1). This
//! module is the **only** timing authority for lyrics in the product: the
//! [`active_line`] function is the complete definition of "which lyric
//! line is active at playback position *p*", and every consumer (web via
//! `musicpack-wasm`, future native clients) must go through it rather
//! than reimplementing selection.
//!
//! Purity laws (mirroring `crate::player`): no I/O, no ambient time, no
//! randomness, no state across calls. Parsing is a total function from
//! bytes to a document or a typed [`LyricsError`]; timing is a pure
//! function of (document, position). All timestamps are `i64`
//! milliseconds — floats never touch a timestamp.
//!
//! The distinction between *unsynchronised* and *line-synchronised*
//! content is carried by [`LyricsContent`] (derived from the parsed
//! document, never from stored metadata): a document is synchronised
//! exactly when at least one line carries a valid timestamp.

mod parse;

pub use parse::{MAX_DOC_BYTES, MAX_LINE_BYTES, MAX_LINES, parse};

/// A parsed lyrics document (the semantic representation; a `.lrc` file
/// reference in the manifest is a separate [`crate::format::manifest`]
/// concern and never carries text).
#[derive(Debug, Clone, PartialEq)]
pub struct LyricsDocument {
    /// The parsed content (plain or line-synchronised).
    pub content: LyricsContent,
    /// Advisory metadata captured from `[ar:]/[ti:]/[al:]/[by:]` tags.
    /// Never affects timing, identity or validation.
    pub metadata: LyricsMetadata,
    /// `true` when A2-enhanced word tags (`<mm:ss.xx>`) were found and
    /// reduced (the defined §4.5 representation of word-timed input).
    pub word_tags_stripped: bool,
}

/// The two v1 content classes, distinguished by parsing (spec §3).
#[derive(Debug, Clone, PartialEq)]
pub enum LyricsContent {
    /// Unsynchronised text: zero timestamps. Lines are display content
    /// in document order (empty lines are stanza breaks).
    Plain(Vec<String>),
    /// Line-synchronised: one or more timestamped lines, sorted by
    /// [`LyricsLine::timestamp_ms`] ascending; equal timestamps keep
    /// document order (the tie-break rule of spec §6.4).
    Synced(Vec<LyricsLine>),
}

/// One timed line. `timestamp_ms` is a non-negative `i64` millisecond
/// offset from the start of the track.
#[derive(Debug, Clone, PartialEq)]
pub struct LyricsLine {
    /// Milliseconds from track start (≥ 0; an offset shift that would
    /// produce a negative value is a parse error).
    pub timestamp_ms: i64,
    /// Display text (normalized: leading/trailing ASCII whitespace
    /// trimmed; interior preserved verbatim; LF-joined block when
    /// untimed lines attach to this line, per spec §4.3).
    pub text: String,
}

/// Advisory document metadata (`[ar:]`, `[ti:]`, `[al:]`, `[by:]`).
///
/// Last occurrence wins for repeated tags (the uniform rule; spec §4.4).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LyricsMetadata {
    /// `[ar:]` — artist.
    pub artist: Option<String>,
    /// `[ti:]` — title.
    pub title: Option<String>,
    /// `[al:]` — album.
    pub album: Option<String>,
    /// `[by:]` — transcriber/author.
    pub by: Option<String>,
}

impl LyricsMetadata {
    /// `true` when at least one tag was captured.
    pub fn is_present(&self) -> bool {
        self.artist.is_some() || self.title.is_some() || self.album.is_some() || self.by.is_some()
    }
}

impl LyricsDocument {
    /// `true` when the document carries line timing (spec §3).
    pub fn is_synced(&self) -> bool {
        matches!(self.content, LyricsContent::Synced(_))
    }

    /// The sorted timed lines; `None` for a plain document.
    pub fn synced_lines(&self) -> Option<&[LyricsLine]> {
        match &self.content {
            LyricsContent::Synced(lines) => Some(lines),
            LyricsContent::Plain(_) => None,
        }
    }

    /// The plain text lines; `None` for a synchronised document.
    pub fn plain_lines(&self) -> Option<&[String]> {
        match &self.content {
            LyricsContent::Plain(lines) => Some(lines),
            LyricsContent::Synced(_) => None,
        }
    }
}

/// The active-line function — the normative timing semantics (spec §9).
///
/// Returns the index of the **last** line whose `timestamp_ms ≤
/// position_ms`, or [`None`] when `position_ms` is before the first
/// line's timestamp (or the document is plain). Equal timestamps are
/// broken by document order (the sort is stable, so the last line of a
/// tie group in document order wins). The function is total, pure and
/// stateless: no hysteresis, no memory of previous calls, arbitrary
/// `i64` positions handled (negative positions are before every
/// non-negative timestamp → `None`).
///
/// Monotonicity invariant: for `p1 ≤ p2`,
/// `active_line(p1) ≤ active_line(p2)` as indices (`None` = −∞).
pub fn active_line(doc: &LyricsDocument, position_ms: i64) -> Option<usize> {
    let lines = doc.synced_lines()?;
    // First index whose timestamp is strictly after the position; the
    // answer is its predecessor. For a tie group all members satisfy
    // `<= position`, so the group's last member (document order) wins.
    let idx = lines.partition_point(|line| line.timestamp_ms <= position_ms);
    if idx == 0 { None } else { Some(idx - 1) }
}

/// Parsing/validation errors (spec §4.6). Line numbers are 1-based
/// physical line numbers in the normalized input. No input content is
/// echoed back beyond the offending line number — errors are safe to
/// surface to a UI as-is.
#[derive(Debug, Clone, PartialEq)]
pub enum LyricsError {
    /// Input is not valid UTF-8.
    InvalidUtf8,
    /// Input exceeds [`MAX_DOC_BYTES`] bytes.
    DocTooLarge {
        /// Actual input size in bytes.
        size: usize,
    },
    /// More than [`MAX_LINES`] lines.
    TooManyLines {
        /// Actual line count.
        count: usize,
    },
    /// A physical line exceeds [`MAX_LINE_BYTES`] bytes.
    LineTooLong {
        /// 1-based physical line number.
        line: usize,
        /// Actual line length in bytes.
        length: usize,
    },
    /// A bracket token shaped like a timestamp violates the timestamp
    /// grammar (bad digits, bad range, bad fraction width, overflow).
    MalformedTimestamp {
        /// 1-based physical line number.
        line: usize,
    },
    /// The `[offset:…]` tag is not an in-range integer (spec §4.4).
    MalformedOffset {
        /// 1-based physical line number.
        line: usize,
    },
    /// Applying the offset shift would produce a negative timestamp.
    NegativeTimestamp {
        /// 1-based physical line number.
        line: usize,
    },
}

impl std::fmt::Display for LyricsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LyricsError::InvalidUtf8 => write!(f, "lyrics are not valid UTF-8"),
            LyricsError::DocTooLarge { size } => {
                write!(
                    f,
                    "lyrics document is {size} bytes; exceeds the limit of {MAX_DOC_BYTES}"
                )
            }
            LyricsError::TooManyLines { count } => {
                write!(
                    f,
                    "lyrics document has {count} lines; exceeds the limit of {MAX_LINES}"
                )
            }
            LyricsError::LineTooLong { line, length } => {
                write!(
                    f,
                    "lyrics line {line} is {length} bytes; exceeds the limit of {MAX_LINE_BYTES}"
                )
            }
            LyricsError::MalformedTimestamp { line } => {
                write!(f, "lyrics line {line} has a malformed timestamp")
            }
            LyricsError::MalformedOffset { line } => {
                write!(f, "lyrics line {line} has a malformed offset tag")
            }
            LyricsError::NegativeTimestamp { line } => {
                write!(
                    f,
                    "lyrics line {line} has a negative timestamp after the offset shift"
                )
            }
        }
    }
}

impl std::error::Error for LyricsError {}
