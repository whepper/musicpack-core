//! MPAK member → `PackageBackend` → `Read` → `audio::open` → engine path.
//!
//! Builds a deterministic container in memory with the writer, then decodes a
//! WAV member through the same adapter. No package knowledge enters the
//! player or the engine state machine: the host wires `MpakBackend` into the
//! engine's `SourceBackend`.

use std::cell::RefCell;
use std::io::Read;
use std::rc::Rc;
use std::sync::Arc;

use musicpack_core::format::checksum::sha256_hex;
use musicpack_core::format::mpak::{MemorySource, PackMember, PackSource, write_mpak};
use musicpack_core::player::engine::Engine;
use musicpack_core::player::types::{PlaybackItem, PlaybackSource, SourceKind};
use musicpack_core::storage::mpak::MpakBackend;
use musicpack_engine::{
    DecoderEngine, DecoderEngineConfig, PackageSourceBackend, SniffingDecoderFactory,
};

const RATE: u32 = 44_100;

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

struct SingleMember {
    manifest: Vec<u8>,
    member: PackMember,
    bytes: Vec<u8>,
}

impl PackSource for SingleMember {
    fn manifest_bytes(&self) -> &[u8] {
        &self.manifest
    }
    fn members(&self) -> &[PackMember] {
        std::slice::from_ref(&self.member)
    }
    fn member_size(&self, _path: &str) -> Result<u64, musicpack_core::error::Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_member(&self, _path: &str) -> Result<Box<dyn Read + '_>, musicpack_core::error::Error> {
        Ok(Box::new(std::io::Cursor::new(self.bytes.clone())))
    }
}

fn build_container() -> Vec<u8> {
    let wav = build_wav(RATE as usize);
    let sha = sha256_hex(&wav);
    let manifest = format!(
        r#"{{"format":"musicpack","version":1,"album":{{"title":"Engine","artists":[{{"name":"A"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"t","audio":{{"path":"audio/01.wav","sha256":"{sha}"}}}}]}}]}}"#
    )
    .into_bytes();
    let source = SingleMember {
        manifest,
        member: PackMember {
            path: "audio/01.wav".into(),
            sha256_hex: sha,
        },
        bytes: wav,
    };
    let mut out = Vec::new();
    write_mpak(&source, &mut out).expect("pack");
    out
}

#[test]
fn decodes_an_mpak_member_through_the_engine() {
    let container = build_container();
    let backend = MpakBackend::open(Arc::new(MemorySource::new(container))).expect("open mpak");
    let engine = DecoderEngine::new(
        Box::new(SniffingDecoderFactory::new(PackageSourceBackend::new(
            Rc::new(backend),
        ))),
        DecoderEngineConfig::default(),
    );
    let engine = Rc::new(RefCell::new(engine));
    let item = PlaybackItem {
        id: "t1".into(),
        track_id: 1,
        source: PlaybackSource {
            kind: SourceKind::Other("mpak".into()),
            url: "audio/01.wav".into(),
            byte_size: None,
        },
        duration_hint_seconds: Some(1.0),
        title: "t".into(),
        artist: "a".into(),
        album_title: "al".into(),
        edition: None,
        artwork_url: None,
        loudness: None,
        album_loudness: None,
        codec: Some("wav".into()),
        mime_type: None,
        extra: Vec::new(),
    };
    let info = engine.borrow_mut().open(&item).expect("open member");
    assert_eq!(info.rate, RATE);
    engine.borrow_mut().play().unwrap();
    let mut buf = vec![0.0f32; 4096];
    engine.borrow_mut().consume(buf.len() / 2, &mut buf);
    assert!(
        buf.iter().any(|s| s.abs() > 0.01),
        "decoded member is non-silent"
    );
    assert!(engine.borrow().rendered_samples() > 0);
}
