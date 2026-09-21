// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

use serde_json::Value;
use std::fmt;
use std::io::Read;
use std::time::Duration;

const USER_AGENT: &str = "musicpack/0.1.0 (https://musicpack.dev)";
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(20);
const MUSICBRAINZ_BASE: &str = "https://musicbrainz.org/ws/2";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MbError {
    Offline(String),
    Timeout(String),
    Http { status: u16 },
    InvalidResponse(String),
    InvalidInput(String),
}

impl fmt::Display for MbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MbError::Offline(detail) => write!(f, "MusicBrainz is unreachable: {detail}"),
            MbError::Timeout(detail) => write!(f, "MusicBrainz request timed out: {detail}"),
            MbError::Http { status: 404 } => write!(f, "MusicBrainz release was not found"),
            MbError::Http { status: 429 } => {
                write!(f, "MusicBrainz rate limit reached; wait a moment and retry")
            }
            MbError::Http { status } => write!(f, "MusicBrainz returned HTTP {status}"),
            MbError::InvalidResponse(detail) => {
                write!(f, "MusicBrainz returned an invalid response: {detail}")
            }
            MbError::InvalidInput(detail) => write!(f, "invalid MusicBrainz input: {detail}"),
        }
    }
}

impl std::error::Error for MbError {}

pub fn fetch_release(mbid: &str) -> Result<String, MbError> {
    if !valid_uuid(mbid) {
        return Err(MbError::InvalidInput(
            "release id must be a 36-character UUID".to_string(),
        ));
    }
    fetch_url(&format!(
        "{MUSICBRAINZ_BASE}/release/{mbid}?inc=artist-credits+labels+recordings+media+release-groups&fmt=json"
    ))
}

pub fn fetch_barcode_search(barcode: &str) -> Result<String, MbError> {
    if !all_digits(barcode) {
        return Err(MbError::InvalidInput(
            "barcode must contain digits only".to_string(),
        ));
    }
    fetch_url(&format!(
        "{MUSICBRAINZ_BASE}/release/?query=barcode:{barcode}&fmt=json&limit=5"
    ))
}

fn fetch_url(url: &str) -> Result<String, MbError> {
    fetch_url_with_timeouts(url, CONNECT_TIMEOUT, READ_TIMEOUT)
}

fn fetch_url_with_timeouts(
    url: &str,
    connect_timeout: Duration,
    read_timeout: Duration,
) -> Result<String, MbError> {
    let agent = ureq::AgentBuilder::new()
        .user_agent(USER_AGENT)
        .timeout_connect(connect_timeout)
        .timeout_read(read_timeout)
        .build();
    let response = agent.get(url).call().map_err(map_ureq_error)?;
    if let Some(length) = response
        .header("Content-Length")
        .and_then(|value| value.parse::<usize>().ok())
    {
        if length > MAX_RESPONSE_BYTES {
            return Err(MbError::InvalidResponse(
                "response exceeds 8 MiB".to_string(),
            ));
        }
    }
    let mut reader = response.into_reader().take((MAX_RESPONSE_BYTES + 1) as u64);
    let mut body = String::new();
    reader
        .read_to_string(&mut body)
        .map_err(|e| map_io_error(e, "reading response"))?;
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(MbError::InvalidResponse(
            "response exceeds 8 MiB".to_string(),
        ));
    }
    if body.trim().is_empty() {
        return Err(MbError::InvalidResponse("empty body".to_string()));
    }
    serde_json::from_str::<Value>(&body)
        .map_err(|e| MbError::InvalidResponse(format!("invalid JSON: {e}")))?;
    Ok(body)
}

fn map_ureq_error(error: ureq::Error) -> MbError {
    match error {
        ureq::Error::Status(status, _) => MbError::Http { status },
        ureq::Error::Transport(error) => {
            let detail = error.to_string();
            if detail.to_ascii_lowercase().contains("timed out") {
                MbError::Timeout(detail)
            } else {
                MbError::Offline(detail)
            }
        }
    }
}

fn map_io_error(error: std::io::Error, context: &str) -> MbError {
    let detail = format!("{context}: {error}");
    if error.kind() == std::io::ErrorKind::TimedOut {
        MbError::Timeout(detail)
    } else {
        MbError::Offline(detail)
    }
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.chars().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

fn all_digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;

    fn serve(status: u16, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        format!("http://{address}/")
    }

    #[test]
    fn response_status_and_json_are_classified() {
        assert_eq!(
            fetch_url(&serve(200, r#"{"id":"release"}"#)).unwrap(),
            r#"{"id":"release"}"#
        );
        assert!(matches!(
            fetch_url(&serve(404, "{}")),
            Err(MbError::Http { status: 404 })
        ));
        assert!(matches!(
            fetch_url(&serve(429, "{}")),
            Err(MbError::Http { status: 429 })
        ));
        assert!(matches!(
            fetch_url(&serve(500, "{}")),
            Err(MbError::Http { status: 500 })
        ));
        assert!(matches!(
            fetch_url(&serve(200, "not json")),
            Err(MbError::InvalidResponse(_))
        ));
    }

    #[test]
    fn captured_release_fixture_survives_transport_unchanged() {
        let fixture = include_str!("../tests/fixtures/mb-release-live-shape.json");
        let body = Box::leak(fixture.to_string().into_boxed_str());
        assert_eq!(fetch_url(&serve(200, body)).unwrap(), fixture);
    }

    #[test]
    fn invalid_input_never_constructs_a_url() {
        assert!(matches!(
            fetch_release("not-a-uuid"),
            Err(MbError::InvalidInput(_))
        ));
        assert!(matches!(
            fetch_barcode_search("abc"),
            Err(MbError::InvalidInput(_))
        ));
    }

    #[test]
    fn stalled_response_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            thread::sleep(Duration::from_millis(200));
        });
        assert!(matches!(
            fetch_url_with_timeouts(
                &format!("http://{address}/"),
                Duration::from_millis(50),
                Duration::from_millis(50)
            ),
            Err(MbError::Timeout(_))
        ));
    }
}
