//! Rust-only regression tests against the committed SV8 container fixtures.
//!
//! The fixtures in `tests/data/container-*.txt` were extracted from reference
//! `.mpc` output once (see `tests/data/README.md` and
//! `tests/data/make_container_fixtures.py`) and are now frozen. These tests
//! never invoke the C encoder, so they keep working after it is deleted.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use musicpack_musepack_encoder::blocks::{
    EncoderInfo, GainInfo, StreamInfo, encoder_info_payload, gain_info_payload,
    seek_offset_payload, seek_table_payload, stream_info_payload,
};
use musicpack_musepack_encoder::sv8::{
    BlockKey, encode_size, encode_size_self_including, write_block,
};

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

fn fixture_paths() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(data_dir())
        .expect("tests/data directory")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_str()?;
            (name.starts_with("container-") && name.ends_with(".txt")).then_some(path)
        })
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no container fixtures found");
    paths
}

#[derive(Debug)]
struct Block {
    key: BlockKey,
    offset: usize,
    size: Vec<u8>,
    crc: Option<u32>,
    payload_len: usize,
    payload: Option<Vec<u8>>,
}

#[derive(Debug)]
struct Fixture {
    name: String,
    seek_ref: u64,
    seek_ptr: Option<u64>,
    seek_table_offset: Option<u64>,
    seek_pos: Option<u32>,
    seek_pwr: Option<u32>,
    seek_entries: Vec<u64>,
    stream_info: BTreeMap<String, i64>,
    encoder_info: Option<BTreeMap<String, i64>>,
    blocks: Vec<Block>,
}

fn hex(bytes: &str) -> Vec<u8> {
    assert!(bytes.len() % 2 == 0, "odd-length hex: {bytes}");
    (0..bytes.len() / 2)
        .map(|i| u8::from_str_radix(&bytes[i * 2..i * 2 + 2], 16).expect("hex"))
        .collect()
}

fn key_of(name: &str) -> BlockKey {
    let bytes = name.as_bytes();
    BlockKey::new([bytes[0], bytes[1]]).expect("fixture block key")
}

fn parse(path: &Path) -> Fixture {
    let text = std::fs::read_to_string(path).expect("read fixture");
    let mut fx = Fixture {
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        seek_ref: 0,
        seek_ptr: None,
        seek_table_offset: None,
        seek_pos: None,
        seek_pwr: None,
        seek_entries: Vec::new(),
        stream_info: BTreeMap::new(),
        encoder_info: None,
        blocks: Vec::new(),
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (head, rest) = line.split_once(' ').unwrap_or((line, ""));
        match head {
            "seek_ref" => fx.seek_ref = rest.parse().unwrap(),
            "seek_ptr" => fx.seek_ptr = Some(rest.parse().unwrap()),
            "seek_table_offset" => fx.seek_table_offset = Some(rest.parse().unwrap()),
            "seek_pos" => fx.seek_pos = Some(rest.parse().unwrap()),
            "seek_pwr" => fx.seek_pwr = Some(rest.parse().unwrap()),
            "seek_entries" => {
                fx.seek_entries = rest
                    .split_whitespace()
                    .map(|e| e.parse().unwrap())
                    .collect()
            }
            "stream_info" => {
                fx.stream_info = parse_fields(rest);
            }
            "encoder_info" => {
                fx.encoder_info = Some(parse_fields(rest));
            }
            "block" => {
                let mut fields = rest.split(' ');
                let key = key_of(fields.next().expect("block key"));
                let mut map = BTreeMap::new();
                for field in fields {
                    let (k, v) = field.split_once('=').expect("key=value");
                    map.insert(k.to_owned(), v.to_owned());
                }
                fx.blocks.push(Block {
                    key,
                    offset: map["offset"].parse().unwrap(),
                    size: hex(&map["size"]),
                    crc: match map["crc"].as_str() {
                        "-" => None,
                        value => Some(u32::from_str_radix(value, 16).unwrap()),
                    },
                    payload_len: map["payload_len"].parse().unwrap(),
                    payload: (map["payload"] != "-").then(|| hex(&map["payload"])),
                });
            }
            other => panic!("unknown fixture line `{other}` in {}", path.display()),
        }
    }
    fx
}

fn parse_fields(text: &str) -> BTreeMap<String, i64> {
    text.split_whitespace()
        .map(|field| {
            let (k, v) = field.split_once('=').expect("key=value");
            (k.to_owned(), v.parse().unwrap())
        })
        .collect()
}

fn decode_size(bytes: &[u8]) -> u64 {
    let mut value = 0u64;
    for &byte in bytes {
        value = value * 128 + u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return value;
        }
    }
    panic!("unterminated size field")
}

fn stream_info_of(fx: &Fixture) -> StreamInfo {
    let f = &fx.stream_info;
    StreamInfo {
        samples: f["samples"] as u64,
        beg_silence: f["skip"] as u64,
        sample_rate: f["sample_rate"] as u32,
        max_band: f["max_band"] as u32,
        channels: f["channels"] as u32,
        ms: f["ms"] != 0,
        frames_per_block_pwr: f["block_pwr"] as u32,
    }
}

#[test]
fn every_fixture_parses_and_covers_the_container_blocks() {
    let mut total = 0usize;
    for path in fixture_paths() {
        let fx = parse(&path);
        assert!(fx.seek_ptr.is_some(), "{}: missing seek_ptr", fx.name);
        assert!(
            fx.blocks.iter().any(|b| b.key == BlockKey::SH),
            "{}",
            fx.name
        );
        assert!(
            fx.blocks.iter().any(|b| b.key == BlockKey::RG),
            "{}",
            fx.name
        );
        assert!(
            fx.blocks.iter().any(|b| b.key == BlockKey::EI),
            "{}",
            fx.name
        );
        assert!(
            fx.blocks.iter().any(|b| b.key == BlockKey::SO),
            "{}",
            fx.name
        );
        assert!(
            fx.blocks.iter().any(|b| b.key == BlockKey::AP),
            "{}",
            fx.name
        );
        assert!(
            fx.blocks.iter().any(|b| b.key == BlockKey::ST),
            "{}",
            fx.name
        );
        assert!(
            fx.blocks.iter().any(|b| b.key == BlockKey::SE),
            "{}",
            fx.name
        );
        total += fx.blocks.len();
    }
    assert!(total > 0, "fixtures contain no blocks");
}

#[test]
fn block_offsets_and_encoded_sizes_are_contiguous() {
    for path in fixture_paths() {
        let fx = parse(&path);
        assert_eq!(
            fx.blocks[0].offset, 4,
            "{}: first block follows MPCK",
            fx.name
        );
        for pair in fx.blocks.windows(2) {
            let declared = decode_size(&pair[0].size);
            assert_eq!(
                pair[0].offset as u64 + declared,
                pair[1].offset as u64,
                "{}: block {} size disagrees with the next offset",
                fx.name,
                String::from_utf8_lossy(&pair[0].key.as_bytes())
            );
        }
    }
}

#[test]
fn container_block_bytes_match_the_reference() {
    for path in fixture_paths() {
        let fx = parse(&path);
        for block in &fx.blocks {
            let Some(payload) = &block.payload else {
                continue; // AP payloads are not committed
            };
            let mut out = Vec::new();
            write_block(&mut out, block.key, payload, block.crc.is_some());

            let mut expected = Vec::new();
            expected.extend_from_slice(&block.key.as_bytes());
            expected.extend_from_slice(&block.size);
            if let Some(crc) = block.crc {
                expected.extend_from_slice(&crc.to_be_bytes());
            }
            expected.extend_from_slice(payload);

            assert_eq!(
                out,
                expected,
                "{}: block {} framing/payload differs",
                fx.name,
                String::from_utf8_lossy(&block.key.as_bytes())
            );
        }
    }
}

#[test]
fn audio_packet_size_fields_match_the_reference() {
    for path in fixture_paths() {
        let fx = parse(&path);
        for block in &fx.blocks {
            if block.key != BlockKey::AP {
                continue;
            }
            let encoded = encode_size_self_including(block.payload_len as u64 + 2);
            assert_eq!(
                encoded.as_bytes(),
                block.size.as_slice(),
                "{}: AP size field for {} payload bytes",
                fx.name,
                block.payload_len
            );
        }
    }
}

#[test]
fn stream_info_payloads_match_the_reference() {
    for path in fixture_paths() {
        let fx = parse(&path);
        let sh = fx.blocks.iter().find(|b| b.key == BlockKey::SH).unwrap();
        let payload = stream_info_payload(&stream_info_of(&fx)).unwrap();
        assert_eq!(
            Some(&payload),
            sh.payload.as_ref(),
            "{}: SH payload differs",
            fx.name
        );
    }
}

#[test]
fn encoder_info_payloads_match_the_reference() {
    for path in fixture_paths() {
        let fx = parse(&path);
        let Some(fields) = &fx.encoder_info else {
            continue;
        };
        let ei = fx.blocks.iter().find(|b| b.key == BlockKey::EI).unwrap();
        let payload = encoder_info_payload(&EncoderInfo {
            profile: fields["profile"] as f32,
            pns: fields["pns"] != 0,
            major: fields["major"] as u32,
            minor: fields["minor"] as u32,
            build: fields["build"] as u32,
        })
        .unwrap();
        assert_eq!(
            Some(&payload),
            ei.payload.as_ref(),
            "{}: EI payload differs",
            fx.name
        );
    }
}

#[test]
fn gain_info_payloads_match_the_reference() {
    for path in fixture_paths() {
        let fx = parse(&path);
        let rg = fx.blocks.iter().find(|b| b.key == BlockKey::RG).unwrap();
        // The reference encoder writes an all-zero replay-gain block.
        assert_eq!(
            Some(&gain_info_payload(&GainInfo::default())),
            rg.payload.as_ref(),
            "{}: RG payload differs",
            fx.name
        );
    }
}

#[test]
fn seek_offset_reservation_and_patch_match_the_reference() {
    for path in fixture_paths() {
        let fx = parse(&path);
        let so = fx.blocks.iter().find(|b| b.key == BlockKey::SO).unwrap();
        let payload = so.payload.as_ref().expect("SO payload");

        assert_eq!(
            seek_offset_payload().len(),
            5,
            "{}: SO reserves 40 bits",
            fx.name
        );
        let patch = encode_size(fx.seek_table_offset.unwrap() - fx.seek_ptr.unwrap());
        assert_eq!(
            patch.as_bytes(),
            &payload[..patch.len()],
            "{}: SO patch differs",
            fx.name
        );
        assert!(
            payload[patch.len()..].iter().all(|&b| b == 0),
            "{}: SO residual reservation is not zero",
            fx.name
        );
    }
}

#[test]
fn seek_table_payloads_match_the_reference() {
    for path in fixture_paths() {
        let fx = parse(&path);
        let st = fx.blocks.iter().find(|b| b.key == BlockKey::ST).unwrap();
        let payload = seek_table_payload(
            fx.seek_ref,
            fx.seek_pos.unwrap(),
            fx.seek_pwr.unwrap(),
            &fx.seek_entries,
        )
        .unwrap();
        assert_eq!(
            Some(&payload),
            st.payload.as_ref(),
            "{}: ST payload differs (seek_pos {}, seek_pwr {})",
            fx.name,
            fx.seek_pos.unwrap(),
            fx.seek_pwr.unwrap()
        );
    }
}

#[test]
fn seek_entries_agree_with_the_reference_absolute_offsets() {
    for path in fixture_paths() {
        let fx = parse(&path);
        let ap_offsets: Vec<u64> = fx
            .blocks
            .iter()
            .filter(|b| b.key == BlockKey::AP)
            .map(|b| b.offset as u64)
            .collect();
        let pwr = fx.seek_pwr.unwrap();
        let expected: Vec<u64> = ap_offsets
            .iter()
            .enumerate()
            .filter(|(index, _)| index % (1 << pwr) == 0)
            .map(|(_, &offset)| offset - fx.seek_ref)
            .collect();
        assert_eq!(
            fx.seek_entries, expected,
            "{}: recorded seek entries do not match AP offsets",
            fx.name
        );
        assert_eq!(expected.len(), fx.seek_pos.unwrap() as usize, "{}", fx.name);
    }
}
