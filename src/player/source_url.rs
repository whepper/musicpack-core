//! The `mpak:` container-member source form — one shared definition.
//!
//! A single-file `.mpak` container holds a whole package, so addressing one of
//! its members needs two locators: the container itself and the member path
//! inside it. The canonical way to write that pair in a
//! [`PlaybackSource::url`] is one URL-shaped key:
//!
//! ```text
//! mpak:<container-url>#<member-path>
//! ```
//!
//! `container-url` locates the container (an HTTP(S) URL, an offline store
//! key, or a local file path); `member-path` is the package-relative member
//! path, the same string a `PackageBackend` resolves. The form is parsed and
//! formatted **only here**, so every producer and consumer — the engine's
//! range-backed byte source, and the server's library index — agrees by
//! construction. Replacing it with a typed [`PlaybackSource`] variant later is
//! a change to this module alone.
//!
//! # Byte reads are container-absolute
//!
//! The member's offset and length are resolved by the container reader, never
//! by the host. A host therefore implements one flat range reader over the
//! *container* URL and never parses this form: [`transport_url`] is what it
//! needs to open its transport.
//!
//! ```
//! use musicpack_core::player::source_url::{format_container_source, parse_container_source};
//!
//! let key = format_container_source("https://host/album.mpak", "audio/01.mpc");
//! assert_eq!(key, "mpak:https://host/album.mpak#audio/01.mpc");
//! let parsed = parse_container_source(&key).unwrap().unwrap();
//! assert_eq!(parsed.container, "https://host/album.mpak");
//! assert_eq!(parsed.member, "audio/01.mpc");
//! ```

use crate::player::types::PlaybackSource;

/// The scheme prefix of a container-member source key.
pub const CONTAINER_PREFIX: &str = "mpak:";

/// The separator between the container URL and the member path.
pub const CONTAINER_SEPARATOR: char = '#';

/// A parsed container-member source key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSourceUrl {
    /// The container's own URL, store key or path (percent-decoded).
    pub container: String,
    /// The package-relative member path inside the container.
    pub member: String,
}

impl ContainerSourceUrl {
    /// The canonical source key for this container/member pair.
    pub fn to_source_key(&self) -> String {
        format_container_source(&self.container, &self.member)
    }
}

/// Why a `mpak:` source key could not be parsed.
///
/// Only reachable for keys that carry the [`CONTAINER_PREFIX`] but are not
/// well-formed; a key without the prefix is a plain source, not an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceUrlError {
    /// The key has the prefix but no `#` separator.
    MissingSeparator,
    /// The container part is empty.
    EmptyContainer,
    /// The member part is empty.
    EmptyMember,
    /// The container part contains a malformed percent escape.
    BadEscape,
}

impl std::fmt::Display for SourceUrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let detail = match self {
            SourceUrlError::MissingSeparator => {
                "no '#' separating the container URL from the member path"
            }
            SourceUrlError::EmptyContainer => "the container URL is empty",
            SourceUrlError::EmptyMember => "the member path is empty",
            SourceUrlError::BadEscape => "malformed percent escape in the container URL",
        };
        write!(f, "malformed '{CONTAINER_PREFIX}' source key: {detail}")
    }
}

impl std::error::Error for SourceUrlError {}

/// Parses a source key into its container URL and member path.
///
/// - `Ok(None)` — a plain source key; the bytes come from the key itself.
/// - `Ok(Some(..))` — a well-formed `mpak:` container-member key.
/// - `Err(..)` — a key carrying [`CONTAINER_PREFIX`] that is not well-formed.
///
/// The container part ends at the **first** `#`, so a member path may itself
/// contain `#`; a container URL containing one must be percent-encoded (see
/// [`format_container_source`]).
pub fn parse_container_source(url: &str) -> Result<Option<ContainerSourceUrl>, SourceUrlError> {
    let Some(rest) = url.strip_prefix(CONTAINER_PREFIX) else {
        return Ok(None);
    };
    let Some((container, member)) = rest.split_once(CONTAINER_SEPARATOR) else {
        return Err(SourceUrlError::MissingSeparator);
    };
    if container.is_empty() {
        return Err(SourceUrlError::EmptyContainer);
    }
    if member.is_empty() {
        return Err(SourceUrlError::EmptyMember);
    }
    let container = percent_decode(container).ok_or(SourceUrlError::BadEscape)?;
    Ok(Some(ContainerSourceUrl {
        container,
        member: member.to_string(),
    }))
}

/// Builds a container-member source key from a container URL and member path.
///
/// The container URL is percent-encoded (only the characters that would
/// otherwise be structural: `%` and `#`); the member path is used verbatim,
/// because `#` is legal inside a member path (the parser splits on the first
/// one).
pub fn format_container_source(container: &str, member: &str) -> String {
    format!(
        "{CONTAINER_PREFIX}{}{CONTAINER_SEPARATOR}{member}",
        percent_encode_container(container)
    )
}

/// The URL, store key or path that actually serves a source key's bytes: the
/// container for a container member, the key itself for a plain source.
///
/// A host needs this to open its transport, and it is deliberately total: an
/// unparsable `mpak:` key is returned unchanged so a host still reports its
/// own "no such source" error rather than a parse error it cannot interpret.
pub fn transport_url(url: &str) -> std::borrow::Cow<'_, str> {
    match parse_container_source(url) {
        Ok(Some(parsed)) => std::borrow::Cow::Owned(parsed.container),
        _ => std::borrow::Cow::Borrowed(url),
    }
}

/// `true` when `url` is a container-member source key.
///
/// A convenience for call sites that only need to branch, not to decompose.
pub fn is_container_source(url: &str) -> bool {
    url.starts_with(CONTAINER_PREFIX)
}

/// The canonical playback source for `member` inside the container at
/// `container`.
///
/// This is **the** way a host states "these bytes are a member of that
/// container"; no other code builds the string by hand.
pub fn container_playback_source(container: &str, member: &str) -> PlaybackSource {
    PlaybackSource {
        kind: crate::player::types::SourceKind::Other(
            CONTAINER_PREFIX.trim_end_matches(':').to_string(),
        ),
        url: format_container_source(container, member),
        byte_size: None,
    }
}

/// Decodes `%XX` escapes; `None` on a malformed escape.
fn percent_decode(input: &str) -> Option<String> {
    if !input.contains('%') {
        return Some(input.to_string());
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = input.get(i + 1..i + 3)?;
            if hex.len() != 2 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Escapes only the characters that are structural in a container-member key.
fn percent_encode_container(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte == b'%' || byte == CONTAINER_SEPARATOR as u8 {
            out.push_str(&format!("%{byte:02X}"));
        } else {
            out.push(byte as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_keys_are_not_container_keys() {
        assert_eq!(parse_container_source("/audio/01.mpc").unwrap(), None);
        assert_eq!(parse_container_source("https://h/media.mpc").unwrap(), None);
        assert_eq!(parse_container_source("").unwrap(), None);
        assert_eq!(transport_url("/audio/01.mpc"), "/audio/01.mpc");
        assert!(!is_container_source("/audio/01.mpc"));
    }

    #[test]
    fn container_keys_split_on_the_first_separator() {
        let parsed = parse_container_source("mpak:https://h/x.mpak#audio/01.mpc")
            .unwrap()
            .expect("container key");
        assert_eq!(parsed.container, "https://h/x.mpak");
        assert_eq!(parsed.member, "audio/01.mpc");
        // A member path may contain '#': only the first one separates.
        let parsed = parse_container_source("mpak:box://k#cover/front#2.jpg")
            .unwrap()
            .expect("container key");
        assert_eq!(parsed.container, "box://k");
        assert_eq!(parsed.member, "cover/front#2.jpg");
        assert!(parsed.to_source_key() == "mpak:box://k#cover/front#2.jpg");
    }

    #[test]
    fn container_keys_round_trip_through_the_formatter() {
        for (container, member) in [
            ("https://host/media/thrasher.mpak", "audio/01.mpc"),
            ("offline-v1/release-42", "cover/front.jpg"),
            // Structural characters in the container are escaped, not dropped.
            ("https://host/a%23b/c.mpak", "audio/01.mpc"),
            // A local path, as the server indexes it.
            ("/srv/library/album/thrasher.mpak", "audio/01.mpc"),
        ] {
            let key = format_container_source(container, member);
            let parsed = parse_container_source(&key)
                .unwrap_or_else(|e| panic!("{key} must parse: {e}"))
                .unwrap_or_else(|| panic!("{key} must be a container key"));
            assert_eq!(parsed.container, container);
            assert_eq!(parsed.member, member);
            assert_eq!(transport_url(&key), container);
        }
    }

    #[test]
    fn malformed_container_keys_are_reported_not_guessed() {
        assert_eq!(
            parse_container_source("mpak:https://h/x.mpak"),
            Err(SourceUrlError::MissingSeparator)
        );
        assert_eq!(
            parse_container_source("mpak:#audio/01.mpc"),
            Err(SourceUrlError::EmptyContainer)
        );
        assert_eq!(
            parse_container_source("mpak:https://h/x.mpak#"),
            Err(SourceUrlError::EmptyMember)
        );
        assert_eq!(
            parse_container_source("mpak:https://h/%zz.mpak#audio/01.mpc"),
            Err(SourceUrlError::BadEscape)
        );
        assert!(SourceUrlError::EmptyMember.to_string().contains("mpak:"));
    }

    #[test]
    fn transport_url_is_total() {
        // An unparsable key is passed through unchanged: a host must still be
        // able to report its own error for it.
        assert_eq!(transport_url("mpak:broken"), "mpak:broken");
        assert_eq!(
            transport_url("mpak:https%3A%2F%2Fh%2Fx.mpak#audio/01.mpc"),
            "https://h/x.mpak"
        );
    }

    #[test]
    fn the_playback_source_helper_builds_the_canonical_form() {
        let source = container_playback_source("/srv/library/a.mpak", "audio/01.mpc");
        assert_eq!(source.url, "mpak:/srv/library/a.mpak#audio/01.mpc");
        assert_eq!(source.kind.as_str(), "mpak");
        // The invariant the server relies on: the key parses back to exactly
        // the container and member it was built from.
        let parsed = parse_container_source(&source.url).unwrap().unwrap();
        assert_eq!(parsed.container, "/srv/library/a.mpak");
        assert_eq!(parsed.member, "audio/01.mpc");
    }
}
