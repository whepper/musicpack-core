//! Stage 1 acceptance: a container member read through a range transport is
//! byte- and PCM-identical to reading the same bytes directly.
//!
//! ```text
//! complete .mpak bytes  ≡  range-backed .mpak member reads
//! ```
//!
//! The transport here is a deliberate model of the browser's: reads are capped
//! to a 64 KiB **block-aligned** window served from a small block cache, exactly
//! like `networker.js` behind the mailbox (`DATA_CAP`, `BLOCK`, block-aligned
//! `base`). The container is therefore never read in one piece, and a member is
//! reached only through offsets the host is willing to serve.
//!
//! The real-artifact check is opt-in: set `MUSICPACK_MPAK_FIXTURE` to a `.mpak`
//! path. It is skipped (loudly) without it, like `tests/db_compat.rs` does for
//! the legacy server.

use std::cell::{Cell, RefCell};
use std::io::Read;
use std::rc::Rc;
use std::sync::Arc;

use musicpack_core::format::checksum::sha256_hex;
use musicpack_core::format::mpak::{MemorySource, PackMember, PackSource, write_mpak};
use musicpack_core::player::engine::Engine;
use musicpack_core::player::types::{PlaybackItem, PlaybackSource, SourceKind};
use musicpack_core::storage::mpak::MpakBackend;
use musicpack_engine::mpak_source::RangeSourceBackend;
use musicpack_engine::{
    DecoderEngine, DecoderEngineConfig, MemorySourceBackend, SniffingDecoderFactory, SourceBackend,
    format_container_source,
};

const RATE: u32 = 44_100;
const BLOCK: usize = 64 * 1024;
const AUDIO: &str = "audio/01.wav";
const ARTWORK: &str = "cover/front.jpg";
const LYRICS: &str = "lyrics/01.txt";
const CONTAINER_URL: &str = "https://library.test/packages/thrasher.mpak";

fn build_wav(frames: usize) -> Vec<u8> {
    let channels = 2u16;
    let block_align = channels * 2;
    let data_len = (frames * channels as usize * 2) as u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * block_align as u32).to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        let v = ((i as f32 * 0.02).sin() * 0.6 * 32767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
        out.extend_from_slice(&((v as f32 * 0.9) as i16).to_le_bytes());
    }
    out
}

/// A package of several members, packed deterministically.
struct Package {
    manifest: Vec<u8>,
    members: Vec<(String, Vec<u8>)>,
}

impl Package {
    fn build() -> Self {
        let audio = build_wav(RATE as usize);
        let artwork = (0..4096u32).map(|i| (i % 251) as u8).collect::<Vec<u8>>();
        let lyrics = b"singing in the dark, no sound at all".to_vec();
        let sha = sha256_hex(&audio);
        let manifest = format!(
            r#"{{"format":"musicpack","version":1,"album":{{"title":"THRASHER","artists":[{{"name":"Brandon Flowers"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"THRASHER","audio":{{"path":"{AUDIO}","sha256":"{sha}"}},"lyrics":[{{"path":"{LYRICS}","kind":"lyrics"}}]}}]}}]}}"#
        )
        .into_bytes();
        Self {
            manifest,
            members: vec![
                (AUDIO.into(), audio),
                (ARTWORK.into(), artwork),
                (LYRICS.into(), lyrics),
            ],
        }
    }

    fn bytes(&self, path: &str) -> &[u8] {
        self.members
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, b)| b.as_slice())
            .unwrap_or_else(|| panic!("no member {path}"))
    }

    fn packed(&self) -> Vec<u8> {
        let members: Vec<PackMember> = self
            .members
            .iter()
            .map(|(path, bytes)| PackMember {
                path: path.clone(),
                sha256_hex: sha256_hex(bytes),
            })
            .collect();
        let source = PackedSource {
            manifest: &self.manifest,
            members: &members,
            content: &self.members,
        };
        let mut out = Vec::new();
        write_mpak(&source, &mut out).expect("pack");
        out
    }
}

struct PackedSource<'a> {
    manifest: &'a [u8],
    members: &'a [PackMember],
    content: &'a [(String, Vec<u8>)],
}

impl PackSource for PackedSource<'_> {
    fn manifest_bytes(&self) -> &[u8] {
        self.manifest
    }
    fn members(&self) -> &[PackMember] {
        self.members
    }
    fn member_size(&self, path: &str) -> Result<u64, musicpack_core::error::Error> {
        Ok(self
            .content
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, b)| b.len())
            .unwrap_or(0) as u64)
    }
    fn read_member(&self, path: &str) -> Result<Box<dyn Read + '_>, musicpack_core::error::Error> {
        Ok(Box::new(std::io::Cursor::new(
            self.content
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, b)| b.clone())
                .unwrap_or_default(),
        )))
    }
}

/// A model of the browser's block-aligned range transport.
///
/// Every request is answered from a 64 KiB block (a miss "fetches" exactly that
/// block), and a request that would cross the block's end is **short** — the
/// property a host is entitled to exercise.
struct Transport {
    url: String,
    bytes: Vec<u8>,
    /// Fetched (base, len) pairs: what actually left the host.
    fetched: RefCell<Vec<(u64, usize)>>,
    /// Every (offset, len) the engine asked for.
    asked: RefCell<Vec<(u64, usize)>>,
}

impl Transport {
    fn new(bytes: Vec<u8>) -> Rc<Self> {
        Self::for_url(CONTAINER_URL, bytes)
    }

    fn for_url(url: &str, bytes: Vec<u8>) -> Rc<Self> {
        Rc::new(Self {
            url: url.to_string(),
            bytes,
            fetched: RefCell::new(Vec::new()),
            asked: RefCell::new(Vec::new()),
        })
    }

    fn read(&self, url: &str, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        assert_eq!(url, self.url, "the member key never reaches the host");
        self.asked.borrow_mut().push((offset, len));
        let base = (offset as usize / BLOCK) * BLOCK;
        let end = (base + BLOCK).min(self.bytes.len());
        self.fetched.borrow_mut().push((base as u64, end - base));
        let from = (offset as usize).min(self.bytes.len()).max(base);
        let to = (from + len).min(end);
        Ok(self.bytes[from..to].to_vec())
    }

    /// Reads issued for `member` while the container was already scanned.
    fn asks_within(&self, span: (u64, u64)) -> bool {
        self.asked
            .borrow()
            .iter()
            .all(|(offset, len)| *offset >= span.0 && offset.saturating_add(*len as u64) <= span.1)
    }
}

/// The key the direct (whole-bytes) reference engine reads the WAV under.
const DIRECT_URL: &str = "memory:direct.wav";

fn item(id: &str, url: &str, byte_size: Option<u64>, kind: SourceKind) -> PlaybackItem {
    PlaybackItem {
        id: id.into(),
        track_id: 1,
        source: PlaybackSource {
            kind,
            url: url.into(),
            byte_size,
        },
        duration_hint_seconds: Some(1.0),
        title: "THRASHER".into(),
        artist: "Brandon Flowers".into(),
        album_title: "THRASHER".into(),
        edition: None,
        artwork_url: None,
        loudness: None,
        album_loudness: None,
        codec: Some("wav".into()),
        mime_type: None,
        extra: Vec::new(),
    }
}

/// Decodes `frames` interleaved stereo frames from an engine built over
/// `backend`, opening `item`.
fn render(
    backend: Rc<dyn musicpack_engine::SourceBackend>,
    item: PlaybackItem,
    frames: usize,
) -> Vec<f32> {
    let engine = DecoderEngine::new(
        Box::new(SniffingDecoderFactory::new(backend)),
        DecoderEngineConfig::default(),
    );
    let engine = Rc::new(RefCell::new(engine));
    engine.borrow_mut().open(&item).expect("open source");
    engine.borrow_mut().play().unwrap();
    let mut out = vec![0.0f32; frames * 2];
    let written = engine.borrow_mut().consume(frames, &mut out);
    assert_eq!(written, frames, "the member decoded to the full request");
    out
}

#[test]
fn range_backed_members_equal_the_complete_bytes() {
    let package = Package::build();
    let container = package.packed();
    let transport = Transport::new(container.clone());
    let calls = Rc::new(Cell::new(0));
    let counter = calls.clone();
    let t = transport.clone();
    let fetch: musicpack_engine::RangeFetcher = Rc::new(move |url, offset, len| {
        counter.set(counter.get() + 1);
        t.read(url, offset, len)
    });
    let backend = RangeSourceBackend::new(fetch);
    let container_size = container.len() as u64;
    let size_of_wav = package.bytes(AUDIO).len() as u64;

    // 1. Every member reads back exactly, through the range transport.
    for (path, expected) in &package.members {
        let mut reader = backend
            .open_source(&PlaybackSource {
                kind: SourceKind::Other("mpak".into()),
                url: format_container_source(CONTAINER_URL, path),
                byte_size: Some(container_size),
            })
            .unwrap_or_else(|e| panic!("open {path}: {e}"));
        let mut got = Vec::new();
        reader.read_to_end(&mut got).expect("read member");
        assert_eq!(
            &got, expected,
            "member {path} differs from the source bytes"
        );
    }

    // 2. The decoded audio is identical to decoding the complete bytes. The
    //    reference engine is handed the WAV directly; the range engine reads
    //    it out of the container.
    let mut memory = MemorySourceBackend::new();
    memory.insert(DIRECT_URL, package.bytes(AUDIO).to_vec());
    let direct = render(
        Rc::new(memory),
        item(
            "direct",
            DIRECT_URL,
            Some(size_of_wav),
            SourceKind::Other("memory".into()),
        ),
        8_192,
    );

    let ranged: Rc<dyn musicpack_engine::SourceBackend> = Rc::new(backend);
    let via_container = render(
        ranged,
        item(
            "t1",
            &format_container_source(CONTAINER_URL, AUDIO),
            Some(container_size),
            SourceKind::Other("mpak".into()),
        ),
        8_192,
    );
    assert_eq!(
        direct, via_container,
        "range-backed decode differs from direct"
    );
    assert!(
        via_container.chunks(2).any(|frame| frame[0].abs() > 0.01),
        "the decoded member is not silence"
    );
    assert!(calls.get() > 1, "the transport really was used in pieces");
}

#[test]
fn a_scanned_container_is_reused_for_its_other_members() {
    let package = Package::build();
    let container = package.packed();
    let transport = Transport::new(container.clone());
    let t = transport.clone();
    let fetch: musicpack_engine::RangeFetcher =
        Rc::new(move |url, offset, len| t.read(url, offset, len));
    let backend = RangeSourceBackend::new(fetch);
    let size = container.len() as u64;
    let source = |path: &str| PlaybackSource {
        kind: SourceKind::Other("mpak".into()),
        url: format_container_source(CONTAINER_URL, path),
        byte_size: Some(size),
    };

    backend.open_source(&source(AUDIO)).expect("first member");
    assert_eq!(backend.cached_containers(), 1);

    // The artwork sits after the audio in the file, so a second open that
    // touched the container framing again would show reads outside its span.
    let member_span = MpakBackend::open(Arc::new(MemorySource::new(container.clone())))
        .expect("scan")
        .reader()
        .members()
        .iter()
        .find(|m| m.path == ARTWORK)
        .map(|m| (m.offset, m.offset + m.length))
        .expect("artwork member");

    transport.asked.borrow_mut().clear();
    let mut reader = backend
        .open_source(&source(ARTWORK))
        .expect("second member");
    let mut artwork = Vec::new();
    reader.read_to_end(&mut artwork).expect("read artwork");
    assert_eq!(artwork, package.bytes(ARTWORK));
    assert_eq!(backend.cached_containers(), 1, "still exactly one scan");
    assert!(
        transport.asks_within(member_span),
        "the second member re-read the container framing instead of using the cache"
    );
}

#[test]
fn a_missing_member_or_container_fails_without_panicking() {
    let package = Package::build();
    let container = package.packed();
    let size = container.len() as u64;
    let t = Transport::new(container);
    let fetch: musicpack_engine::RangeFetcher =
        Rc::new(move |url, offset, len| t.read(url, offset, len));
    let backend = RangeSourceBackend::new(fetch);

    let missing = backend
        .open_source(&PlaybackSource {
            kind: SourceKind::Other("mpak".into()),
            url: format_container_source(CONTAINER_URL, "audio/99.wav"),
            byte_size: Some(size),
        })
        .err()
        .expect("unknown member is an error");
    assert!(missing.0.contains("audio/99.wav"), "{}", missing.0);

    // A malformed key is rejected before any byte is read.
    assert!(
        backend
            .open_source(&PlaybackSource {
                kind: SourceKind::Other("mpak".into()),
                url: "mpak:no-separator".into(),
                byte_size: Some(size),
            })
            .is_err()
    );

    // The scan itself succeeded, so it stays cached even though the member was
    // absent: the cache holds containers, not member lookups.
    assert_eq!(backend.cached_containers(), 1);

    // A failed *scan* is not cached, so a repaired container is picked up on the
    // next open rather than being remembered as broken.
    let broken = "https://library.test/packages/broken.mpak";
    let broken_transport = Transport::for_url(broken, vec![0u8; 512]);
    let t = broken_transport.clone();
    let broken_backend =
        RangeSourceBackend::new(Rc::new(move |url, offset, len| t.read(url, offset, len)));
    let error = broken_backend
        .open_source(&PlaybackSource {
            kind: SourceKind::Other("mpak".into()),
            url: format_container_source(broken, AUDIO),
            byte_size: Some(512),
        })
        .err()
        .expect("garbage is not a container");
    assert!(error.0.contains("cannot scan container"), "{}", error.0);
    assert_eq!(
        broken_backend.cached_containers(),
        0,
        "a failed scan must not be cached"
    );
}

#[test]
fn a_plain_source_is_unaffected_by_the_container_path() {
    let bytes = build_wav(1024);
    let t = Transport::new(bytes.clone());
    let fetch: musicpack_engine::RangeFetcher =
        Rc::new(move |url, offset, len| t.read(url, offset, len));
    let backend = RangeSourceBackend::new(fetch);
    let mut reader = backend
        .open_source(&PlaybackSource {
            kind: SourceKind::HttpRange,
            url: CONTAINER_URL.into(),
            byte_size: Some(bytes.len() as u64),
        })
        .expect("plain source");
    let mut got = Vec::new();
    reader.read_to_end(&mut got).expect("read");
    assert_eq!(got, bytes, "a plain source streams the transport verbatim");
    assert_eq!(backend.cached_containers(), 0, "no scan for a plain source");
}

/// The real-artifact check: a container produced by the authoring pipeline
/// (not this test's writer) must expose its members the same way.
#[test]
fn a_real_container_exposes_its_members() {
    let Ok(path) = std::env::var("MUSICPACK_MPAK_FIXTURE") else {
        eprintln!(
            "skipping the real .mpak check: set MUSICPACK_MPAK_FIXTURE to a container path \
             (the authored fixture is not committed)"
        );
        return;
    };
    let bytes = std::fs::read(&path).expect("read the fixture");
    let size = bytes.len() as u64;
    let t = Transport::new(bytes);
    let fetch: musicpack_engine::RangeFetcher =
        Rc::new(move |url, offset, len| t.read(url, offset, len));
    let backend = RangeSourceBackend::new(fetch);
    let container = MpakBackend::open(Arc::new(MemorySource::new(
        std::fs::read(&path).expect("read the fixture"),
    )))
    .expect("the fixture is a valid container");
    let manifest =
        String::from_utf8(container.manifest_bytes().to_vec()).expect("the manifest is UTF-8");
    assert!(manifest.contains("audio"), "manifest lists audio");

    let members: Vec<String> = container
        .reader()
        .members()
        .iter()
        .map(|m| m.path.clone())
        .collect();
    assert!(!members.is_empty(), "the fixture has members");
    eprintln!(
        "{}: {} bytes, {} members: {}",
        path,
        size,
        members.len(),
        members.join(", ")
    );
    for path in &members {
        let mut reader = backend
            .open_source(&PlaybackSource {
                kind: SourceKind::Other("mpak".into()),
                url: format_container_source(CONTAINER_URL, path),
                byte_size: Some(size),
            })
            .unwrap_or_else(|e| panic!("open {path}: {e}"));
        let mut got = Vec::new();
        reader.read_to_end(&mut got).expect("read member");
        let expected = container
            .read_member(path, usize::MAX)
            .expect("read the same member from the whole file");
        assert_eq!(got.len(), expected.len(), "member {path} length");
        assert_eq!(got, expected, "member {path} bytes");
    }
    // The whole-file comparison above is the invariant; the range-backed read
    // of every member is what makes it true.
    assert_eq!(backend.cached_containers(), 1);
}
