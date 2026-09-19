//! MPAK v1 container tests: reader, writer, backend, malformed input, and
//! the security invariants (CRC-before-length, checked arithmetic, bounded
//! allocation, guaranteed scan progress).
//!
//! Everything here is portable (in-memory sources), so it runs on every CI
//! platform. Filesystem-backed compatibility (the committed reference
//! container, byte-identical packing) lives in `tests/mpak_compat.rs`.

mod support;

use std::io::Read;
use std::sync::Arc;

use musicpack_core::Error;
use musicpack_core::format::checksum;
use musicpack_core::format::mpak::{
    self, BLOCK_HEADER_LEN, ByteSource, HEADER_LEN, MemorySource, PackMember, PackSource,
    write_mpak,
};
use musicpack_core::storage::PackageBackend;
use musicpack_core::storage::mpak::{MpakBackend, verify_mpak};

use support::{
    TestSource, be_u64, container_header, data_frame, data_payload, frame, manifest_for, raw_frame,
    raw_scan, scan, valid_container,
};

// ---------------------------------------------------------------------
// builders
// ---------------------------------------------------------------------

// ---------------------------------------------------------------------
// reader
// ---------------------------------------------------------------------

#[test]
fn valid_minimal_container_scans() {
    let bytes = valid_container(&[("audio/01.bin", b"one")]);
    let reader = raw_scan(&bytes).expect("scans");
    assert_eq!(reader.members().len(), 1);
    assert_eq!(reader.members()[0].path, "audio/01.bin");
    assert_eq!(reader.members()[0].length, 3);
    assert_eq!(reader.manifest_count(), 1);
    assert!(reader.indx_present() && reader.indx_valid());
    assert!(reader.tail_present() && !reader.tail_malformed());
    assert_eq!(reader.duplicate_members(), 0);
    assert!(!reader.resynced());
}

#[test]
fn valid_multi_member_container_scans() {
    let bytes = valid_container(&[
        ("audio/01.bin", b"one"),
        ("audio/02.bin", b"two"),
        ("artwork/front.jpg", b"art"),
        ("extras/notes.txt", b"notes"),
    ]);
    let reader = raw_scan(&bytes).expect("scans");
    assert_eq!(
        reader
            .members()
            .iter()
            .map(|m| m.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "audio/01.bin",
            "audio/02.bin",
            "artwork/front.jpg",
            "extras/notes.txt"
        ]
    );
    assert!(reader.member("audio/02.bin").is_some());
    assert!(reader.member("nope").is_none());
}

#[test]
fn empty_container_is_structurally_valid() {
    // Header only: no blocks at all.
    let bytes = container_header();
    let reader = raw_scan(&bytes).expect("scans");
    assert_eq!(reader.members().len(), 0);
    assert_eq!(reader.manifest_count(), 0);
    // Opening as a package requires a MANF block.
    let opened = MpakBackend::open(Arc::new(MemorySource::new(bytes)));
    assert!(matches!(opened, Err(Error::Missing { .. })));
}

#[test]
fn truncated_header_is_rejected() {
    for len in [0usize, 1, 4, 15] {
        let bytes = vec![b'M'; len];
        assert!(raw_scan(&bytes).is_err(), "len {len}");
    }
}

#[test]
fn bad_magic_is_rejected() {
    let mut bytes = container_header();
    bytes[0] = b'X';
    assert!(matches!(raw_scan(&bytes), Err(Error::Invalid { .. })));
}

#[test]
fn unsupported_major_is_a_version_error() {
    let mut bytes = container_header();
    bytes[4] = 2;
    assert!(matches!(raw_scan(&bytes), Err(Error::Version { .. })));
}

#[test]
fn unknown_and_private_blocks_are_skipped_by_length() {
    let mut bytes = container_header();
    bytes.extend_from_slice(&frame(b"XFUT", b"unknown public block"));
    bytes.extend_from_slice(&frame(b"priv", b"private block"));
    bytes.extend_from_slice(&data_frame("audio/01.bin", b"one"));
    let reader = raw_scan(&bytes).expect("scans");
    assert_eq!(reader.members().len(), 1);
    assert_eq!(reader.members()[0].path, "audio/01.bin");
}

#[test]
fn truncated_frame_resynchronizes() {
    // A valid DATA header claiming 100 bytes with only 3 present.
    let mut bytes = container_header();
    bytes.extend_from_slice(&raw_frame(b"DATA", 100, b"abc", false));
    let reader = raw_scan(&bytes).expect("scans");
    assert_eq!(
        reader.members().len(),
        0,
        "truncated member is not consumed"
    );
    assert!(reader.resynced());
}

#[test]
fn corrupt_crc_resynchronizes() {
    let mut bytes = container_header();
    bytes.extend_from_slice(&raw_frame(b"DATA", 5, b"abcd", true));
    bytes.extend_from_slice(&data_frame("audio/01.bin", b"one"));
    let reader = raw_scan(&bytes).expect("scans");
    assert!(reader.resynced(), "a bad CRC must trigger resync");
    assert_eq!(reader.members().len(), 1);
    assert_eq!(reader.members()[0].path, "audio/01.bin");
}

#[test]
fn corrupt_crc_length_is_never_trusted() {
    // A huge declared length with a broken CRC must not be consumed: the
    // scan resynchronizes byte by byte and still finds the real member.
    let mut bytes = container_header();
    bytes.extend_from_slice(&raw_frame(b"DATA", u64::MAX, b"junk", true));
    bytes.extend_from_slice(&data_frame("audio/01.bin", b"one"));
    let reader = raw_scan(&bytes).expect("scans");
    assert!(reader.resynced());
    assert_eq!(reader.members().len(), 1);
}

#[test]
fn valid_crc_but_out_of_bounds_length_resynchronizes() {
    // CRC validates, length is ≤ 2^63−1 but the payload runs past EOF.
    let mut bytes = container_header();
    bytes.extend_from_slice(&raw_frame(b"DATA", 1 << 40, b"abc", false));
    let reader = raw_scan(&bytes).expect("scans");
    assert!(reader.resynced());
    assert_eq!(reader.members().len(), 0);
}

#[test]
fn enormous_declared_length_is_rejected_before_bounds() {
    // length > 2^63−1 with a *valid* CRC: rejected by the wire maximum.
    for declared in [(1u64 << 63), u64::MAX] {
        let mut bytes = container_header();
        bytes.extend_from_slice(&raw_frame(b"DATA", declared, b"", false));
        let reader = raw_scan(&bytes).expect("scans");
        assert!(
            reader.resynced(),
            "declared {declared:#x} must not be trusted"
        );
        assert_eq!(reader.members().len(), 0);
    }
}

#[test]
fn malformed_member_name_is_fatal_in_normal_mode() {
    // A structurally valid DATA block whose path violates the canonical
    // rules (traversal) must fail the normal load.
    let mut bytes = container_header();
    bytes.extend_from_slice(&data_frame("../evil", b"x"));
    assert!(matches!(raw_scan(&bytes), Err(Error::Path(_))));

    // ... and is a counted skip in recovery mode.
    let reader = scan(&bytes, true).expect("recovery scans");
    assert_eq!(reader.skipped_members(), 1);
    assert_eq!(reader.members().len(), 0);
}

#[test]
fn zero_length_member_is_valid() {
    let mut bytes = container_header();
    bytes.extend_from_slice(&data_frame("audio/empty.bin", b""));
    let reader = raw_scan(&bytes).expect("scans");
    assert_eq!(reader.members().len(), 1);
    assert_eq!(reader.members()[0].length, 0);
}

#[test]
fn boundary_sized_member_reads_back() {
    let big = vec![0x5a; 70_000];
    let bytes = valid_container(&[("audio/big.bin", &big)]);
    let reader = raw_scan(&bytes).expect("scans");
    assert_eq!(reader.members()[0].length, 70_000);
    let backend = MpakBackend::open(Arc::new(MemorySource::new(bytes))).expect("opens");
    let extracted = backend.read_member("audio/big.bin", 1 << 20).unwrap();
    assert_eq!(extracted, big);
}

#[test]
fn duplicate_members_keep_the_first_and_are_counted() {
    let mut bytes = container_header();
    bytes.extend_from_slice(&data_frame("audio/01.bin", b"first"));
    bytes.extend_from_slice(&data_frame("audio/01.bin", b"second"));
    let reader = raw_scan(&bytes).expect("scans");
    assert_eq!(reader.members().len(), 1);
    assert_eq!(reader.members()[0].length, 5); // "first"
    assert_eq!(reader.duplicate_members(), 1);
    assert_eq!(reader.duplicate_example(), Some("audio/01.bin"));
}

#[test]
fn pathological_input_terminates_without_members() {
    // 0xFF everywhere: no valid framing, scan must resync through and stop.
    let mut bytes = container_header();
    bytes.extend_from_slice(&vec![0xFFu8; 8192]);
    let reader = raw_scan(&bytes).expect("scans");
    assert_eq!(reader.members().len(), 0);
    assert!(reader.resynced());
}

#[test]
fn indx_is_discarded_when_it_disagrees_with_the_scan() {
    // Hand-build: a valid DATA member plus an INDX whose offset is wrong.
    let member = b"one";
    let sha = checksum::sha256_hex_to_bytes(&checksum::sha256_hex(member)).unwrap();
    let path = "audio/01.bin";
    let mut data_block = Vec::new();
    let payload = data_payload(path, member);
    data_block.extend_from_slice(&frame(b"DATA", &payload));
    let data_start = HEADER_LEN as u64;

    let mut indx_payload = Vec::new();
    indx_payload.extend_from_slice(&1u32.to_be_bytes());
    indx_payload.extend_from_slice(&(path.len() as u16).to_be_bytes());
    indx_payload.extend_from_slice(path.as_bytes());
    // Deliberately wrong offset (lie).
    indx_payload.extend_from_slice(&(data_start + 999).to_be_bytes());
    indx_payload.extend_from_slice(&(member.len() as u64).to_be_bytes());
    indx_payload.extend_from_slice(&sha);
    let indx_block = frame(b"INDX", &indx_payload);

    let mut bytes = container_header();
    bytes.extend_from_slice(&indx_block);
    bytes.extend_from_slice(&data_block);
    let reader = raw_scan(&bytes).expect("scans");
    assert!(reader.indx_present());
    assert!(!reader.indx_valid(), "a lying INDX must be discarded");
    assert!(!reader.indx_duplicate());
    assert_eq!(reader.members().len(), 1);
}

#[test]
fn duplicate_indx_path_is_flagged() {
    let path = "audio/01.bin";
    let sha = [0u8; 32];
    let mut indx_payload = Vec::new();
    indx_payload.extend_from_slice(&2u32.to_be_bytes());
    for _ in 0..2 {
        indx_payload.extend_from_slice(&(path.len() as u16).to_be_bytes());
        indx_payload.extend_from_slice(path.as_bytes());
        indx_payload.extend_from_slice(&0u64.to_be_bytes());
        indx_payload.extend_from_slice(&3u64.to_be_bytes());
        indx_payload.extend_from_slice(&sha);
    }
    let mut bytes = container_header();
    bytes.extend_from_slice(&frame(b"INDX", &indx_payload));
    bytes.extend_from_slice(&data_frame(path, b"one"));
    let reader = raw_scan(&bytes).expect("scans");
    assert!(!reader.indx_valid());
    assert!(reader.indx_duplicate());
}

#[test]
fn indx_entry_overflow_is_rejected() {
    // offset = u64::MAX, length = 1 → offset+length overflows.
    let path = "audio/01.bin";
    let mut indx_payload = Vec::new();
    indx_payload.extend_from_slice(&1u32.to_be_bytes());
    indx_payload.extend_from_slice(&(path.len() as u16).to_be_bytes());
    indx_payload.extend_from_slice(path.as_bytes());
    indx_payload.extend_from_slice(&u64::MAX.to_be_bytes());
    indx_payload.extend_from_slice(&1u64.to_be_bytes());
    indx_payload.extend_from_slice(&[0u8; 32]);
    let mut bytes = container_header();
    bytes.extend_from_slice(&frame(b"INDX", &indx_payload));
    bytes.extend_from_slice(&data_frame(path, b"one"));
    let reader = raw_scan(&bytes).expect("scans");
    assert!(
        !reader.indx_valid(),
        "overflowing INDX entry must be rejected"
    );
}

#[test]
fn recovery_skips_damaged_preamble_members() {
    let mut bytes = container_header();
    bytes.extend_from_slice(&data_frame("audio/good.bin", b"one"));
    // Structurally fine block, invalid path preamble.
    bytes.extend_from_slice(&data_frame("../bad", b"two"));
    bytes.extend_from_slice(&data_frame("audio/good2.bin", b"three"));
    let reader = scan(&bytes, true).expect("recovery scan");
    assert_eq!(reader.members().len(), 2);
    assert_eq!(reader.skipped_members(), 1);
}

// ---------------------------------------------------------------------
// writer
// ---------------------------------------------------------------------

#[test]
fn writer_is_deterministic() {
    let entries: &[(&str, &[u8])] = &[("audio/01.bin", b"one"), ("artwork/a.jpg", b"art")];
    let a = valid_container(entries);
    let b = valid_container(entries);
    assert_eq!(a, b, "separate writes must be byte-identical");
}

#[test]
fn writer_orders_data_by_canonical_pack_order() {
    // Two tracks (with a representation and a waveform) plus package
    // assets: DATA order must be all audio, all representations, all
    // waveforms, then artwork/extras — the reference writer's grouping.
    let contents: Vec<(&str, Vec<u8>)> = vec![
        ("audio/01.bin", b"a1".to_vec()),
        ("audio/02.bin", b"a2".to_vec()),
        ("rep/01.flac", b"r1".to_vec()),
        ("wf/01.wfm", b"w1".to_vec()),
        ("art/f.jpg", b"f1".to_vec()),
        ("ex/n.txt", b"n1".to_vec()),
    ];
    let hex = |p: &str| checksum::sha256_hex(&contents.iter().find(|(q, _)| *q == p).unwrap().1);
    let manifest = format!(
        r#"{{"format":"musicpack","version":1,"album":{{"title":"T","artists":[{{"name":"A"}}]}},"media":[{{"disc":1,"tracks":[
          {{"track":1,"title":"a","audio":{{"path":"audio/01.bin","sha256":"{a1}"}},
           "representations":[{{"path":"rep/01.flac","sha256":"{r1}"}}],
           "waveform":{{"version":1,"path":"wf/01.wfm","sha256":"{w1}","intervalMs":100,"encoding":"peak-rms-u8","floorDb":-60,"points":0}}}},
          {{"track":2,"title":"b","audio":{{"path":"audio/02.bin","sha256":"{a2}"}}}}
        ]}}],"artwork":[{{"role":"front","path":"art/f.jpg","sha256":"{f1}"}}],"extras":[{{"path":"ex/n.txt","sha256":"{n1}"}}]}}"#,
        a1 = hex("audio/01.bin"),
        a2 = hex("audio/02.bin"),
        r1 = hex("rep/01.flac"),
        w1 = hex("wf/01.wfm"),
        f1 = hex("art/f.jpg"),
        n1 = hex("ex/n.txt"),
    );
    // Members are supplied in the same canonical order the writer expects.
    let members: Vec<(&str, Vec<u8>)> = vec![
        ("audio/01.bin", contents[0].1.clone()),
        ("audio/02.bin", contents[1].1.clone()),
        ("rep/01.flac", contents[2].1.clone()),
        ("wf/01.wfm", contents[3].1.clone()),
        ("art/f.jpg", contents[4].1.clone()),
        ("ex/n.txt", contents[5].1.clone()),
    ];
    let source = TestSource::new(manifest, members.clone()).with_contents(members);
    let mut bytes = Vec::new();
    write_mpak(&source, &mut bytes).expect("writes");
    let reader = raw_scan(&bytes).expect("scans");
    assert_eq!(
        reader
            .members()
            .iter()
            .map(|m| m.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "audio/01.bin",
            "audio/02.bin",
            "rep/01.flac",
            "wf/01.wfm",
            "art/f.jpg",
            "ex/n.txt"
        ]
    );
}

#[test]
fn writer_indx_is_sorted_by_path_bytes() {
    let entries: &[(&str, &[u8])] = &[
        ("extras/z.txt", b"z"),
        ("audio/b.bin", b"b"),
        ("audio/a.bin", b"a"),
    ];
    let bytes = valid_container(entries);
    // INDX is the first block after the header; parse its entry paths.
    let indx_payload_len = be_u64(&bytes[HEADER_LEN + 4..HEADER_LEN + 12]);
    let payload = &bytes
        [HEADER_LEN + BLOCK_HEADER_LEN..HEADER_LEN + BLOCK_HEADER_LEN + indx_payload_len as usize];
    let count = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
    let mut paths = Vec::new();
    let mut p = 4;
    for _ in 0..count {
        let len = u16::from_be_bytes([payload[p], payload[p + 1]]) as usize;
        p += 2;
        paths.push(String::from_utf8(payload[p..p + len].to_vec()).unwrap());
        p += len + 48;
    }
    assert_eq!(paths, vec!["audio/a.bin", "audio/b.bin", "extras/z.txt"]);
}

#[test]
fn writer_tail_is_self_consistent() {
    let entries: &[(&str, &[u8])] = &[("audio/01.bin", b"one"), ("extras/x", b"x")];
    let bytes = valid_container(entries);
    let reader = raw_scan(&bytes).expect("scans");
    assert!(reader.tail_present() && !reader.tail_malformed());
    assert_eq!(reader.tail_total_size(), bytes.len() as u64);
    assert_eq!(reader.tail_objects(), 2);
    assert_eq!(reader.tail_indx_offset(), HEADER_LEN as u64);
    // Digest over every preceding byte.
    let prefix = &bytes[..reader.tail_offset() as usize];
    assert_eq!(&checksum::sha256(prefix), reader.tail_digest());
}

#[test]
fn writer_embeds_the_manifest_bytes_exactly() {
    let manifest = manifest_for(&[("audio/01.bin", b"one")]);
    let source = TestSource::new(manifest.clone(), vec![("audio/01.bin", b"one".to_vec())])
        .with_contents(vec![("audio/01.bin", b"one".to_vec())]);
    let mut bytes = Vec::new();
    write_mpak(&source, &mut bytes).unwrap();
    let reader = raw_scan(&bytes).expect("scans");
    assert_eq!(reader.manifest_bytes(), Some(manifest.as_bytes()));
}

#[test]
fn writer_rejects_invalid_inputs() {
    let manifest = manifest_for(&[("audio/01.bin", b"one")]);
    let contents = vec![("audio/01.bin", b"one".to_vec())];

    // Bad digest form.
    let mut bad = TestSource::new(manifest.clone(), vec![("audio/01.bin", b"one".to_vec())])
        .with_contents(contents.clone());
    bad.members[0].sha256_hex = "XYZ".into();
    assert!(write_mpak(&bad, &mut Vec::new()).is_err());

    // Duplicate member path.
    let mut dup = TestSource::new(manifest.clone(), vec![]).with_contents(contents.clone());
    dup.members = vec![
        PackMember {
            path: "audio/01.bin".into(),
            sha256_hex: checksum::sha256_hex(b"one"),
        },
        PackMember {
            path: "audio/01.bin".into(),
            sha256_hex: checksum::sha256_hex(b"one"),
        },
    ];
    assert!(write_mpak(&dup, &mut Vec::new()).is_err());

    // Content does not match the declared digest → checksum error.
    let mismatch = TestSource::new(manifest.clone(), vec![("audio/01.bin", b"one".to_vec())])
        .with_contents(vec![("audio/01.bin", b"tampered".to_vec())]);
    assert!(matches!(
        write_mpak(&mismatch, &mut Vec::new()),
        Err(Error::Checksum { .. })
    ));

    // Declared size differs from the streamed content.
    struct WrongSize<'a>(&'a TestSource);
    impl PackSource for WrongSize<'_> {
        fn manifest_bytes(&self) -> &[u8] {
            self.0.manifest_bytes()
        }
        fn members(&self) -> &[PackMember] {
            self.0.members()
        }
        fn member_size(&self, _path: &str) -> Result<u64, Error> {
            Ok(999)
        }
        fn read_member(&self, path: &str) -> Result<Box<dyn Read + '_>, Error> {
            self.0.read_member(path)
        }
    }
    let good =
        TestSource::new(manifest, vec![("audio/01.bin", b"one".to_vec())]).with_contents(contents);
    assert!(matches!(
        write_mpak(&WrongSize(&good), &mut Vec::new()),
        Err(Error::Checksum { .. })
    ));
}

#[test]
fn writer_rejects_oversized_members() {
    struct Oversized;
    impl PackSource for Oversized {
        fn manifest_bytes(&self) -> &[u8] {
            b"{}"
        }
        fn members(&self) -> &[PackMember] {
            static ONCE: std::sync::OnceLock<Vec<PackMember>> = std::sync::OnceLock::new();
            ONCE.get_or_init(|| {
                vec![PackMember {
                    path: "audio/01.bin".into(),
                    sha256_hex: "0".repeat(64),
                }]
            })
        }
        fn member_size(&self, _path: &str) -> Result<u64, Error> {
            Ok(musicpack_core::limits::MAX_FILE_BYTES + 1)
        }
        fn read_member(&self, _path: &str) -> Result<Box<dyn Read + '_>, Error> {
            Ok(Box::new(std::io::empty()))
        }
    }
    assert!(matches!(
        write_mpak(&Oversized, &mut Vec::new()),
        Err(Error::Invalid { .. })
    ));
}

// ---------------------------------------------------------------------
// backend + verification integration
// ---------------------------------------------------------------------

#[test]
fn backend_verifies_containers_through_the_shared_verifier() {
    let entries: [(&str, &[u8]); 3] = [
        ("audio/01.bin", b"one"),
        ("artwork/front.jpg", b"art"),
        ("extras/notes.txt", b"notes"),
    ];
    let bytes = valid_container(&entries);
    let report = verify_mpak(Arc::new(MemorySource::new(bytes))).expect("opens");
    assert!(
        report.is_ok(),
        "clean container must verify: {:?}",
        report
            .findings()
            .iter()
            .map(|f| f.message.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(report.errors(), 0);
}

#[test]
fn container_tamper_is_a_checksum_error() {
    let mut bytes = valid_container(&[("audio/01.bin", b"one")]);
    // Flip a byte inside the member payload (last byte before TAIL is not
    // it, so locate the member through the reader).
    let reader = raw_scan(&bytes).expect("scans");
    let member = reader.member("audio/01.bin").unwrap();
    bytes[member.offset as usize] ^= 0xFF;
    let report = verify_mpak(Arc::new(MemorySource::new(bytes))).expect("opens");
    assert!(!report.is_ok());
    assert!(
        report
            .findings()
            .iter()
            .any(|f| f.message.contains("checksum mismatch")),
        "{:?}",
        report.findings()
    );
}

#[test]
fn missing_indx_and_tail_warn() {
    // Strip INDX and TAIL: a minimal DATA+MANF container.
    let mut bytes = container_header();
    bytes[6..8].copy_from_slice(&0u16.to_be_bytes()); // clear flag
    bytes.extend_from_slice(&data_frame("audio/01.bin", b"one"));
    // MANF after DATA is fine (readers scan).
    bytes.extend_from_slice(&frame(
        b"MANF",
        manifest_for(&[("audio/01.bin", b"one")]).as_bytes(),
    ));
    let report = verify_mpak(Arc::new(MemorySource::new(bytes))).expect("opens");
    assert!(report.is_ok(), "{:?}", report.findings());
    let messages: Vec<&str> = report
        .findings()
        .iter()
        .map(|f| f.message.as_str())
        .collect();
    assert!(messages.contains(&"index: missing INDX; sequential scan used"));
    assert!(messages.contains(&"completeness unproven (no TAIL)"));
}

#[test]
fn duplicate_member_is_a_container_error() {
    let mut bytes = container_header();
    bytes.extend_from_slice(&data_frame("audio/01.bin", b"one"));
    bytes.extend_from_slice(&data_frame("audio/01.bin", b"one"));
    bytes.extend_from_slice(&frame(
        b"MANF",
        manifest_for(&[("audio/01.bin", b"one")]).as_bytes(),
    ));
    let report = verify_mpak(Arc::new(MemorySource::new(bytes))).expect("opens");
    assert!(!report.is_ok());
    assert!(
        report
            .findings()
            .iter()
            .any(|f| f.message == "duplicate object path 'audio/01.bin'"),
        "{:?}",
        report.findings()
    );
}

#[test]
fn index_hash_mismatch_is_an_error() {
    // Build a clean container, then rewrite its INDX entry hash to zeroes
    // while keeping it structurally consistent (reconciliation only
    // compares paths/offsets/lengths, so the index stays valid).
    let mut bytes = valid_container(&[("audio/01.bin", b"one")]);
    let payload_start = HEADER_LEN + BLOCK_HEADER_LEN;
    // Layout: count(4) + path_len(2) + path + offset(8) + length(8) + sha(32)
    let mut p = payload_start + 4;
    let path_len = u16::from_be_bytes([bytes[p], bytes[p + 1]]) as usize;
    p += 2 + path_len + 16;
    for b in &mut bytes[p..p + 32] {
        *b = 0;
    }
    // Recompute the INDX block CRC so the block still frames correctly.
    let crc = mpak::crc16_buypass(&bytes[HEADER_LEN..HEADER_LEN + 12]);
    bytes[HEADER_LEN + 12..HEADER_LEN + 14].copy_from_slice(&crc.to_be_bytes());

    let report = verify_mpak(Arc::new(MemorySource::new(bytes))).expect("opens");
    assert!(!report.is_ok());
    assert!(
        report
            .findings()
            .iter()
            .any(|f| f.message == "index: checksum mismatch 'audio/01.bin'"),
        "{:?}",
        report.findings()
    );
}

#[test]
fn container_tail_mismatch_is_an_error() {
    let mut bytes = valid_container(&[("audio/01.bin", b"one")]);
    // Corrupt the TAIL total_size field, fixing the block CRC.
    let reader = raw_scan(&bytes).expect("scans");
    let tail = reader.tail_offset() as usize;
    let payload = tail + BLOCK_HEADER_LEN;
    bytes[payload] ^= 0xFF;
    let crc = mpak::crc16_buypass(&bytes[tail..tail + 12]);
    bytes[tail + 12..tail + 14].copy_from_slice(&crc.to_be_bytes());

    let report = verify_mpak(Arc::new(MemorySource::new(bytes))).expect("opens");
    assert!(!report.is_ok());
    assert!(
        report
            .findings()
            .iter()
            .any(|f| f.message.starts_with("tail: total size mismatch")),
        "{:?}",
        report.findings()
    );
}

#[test]
fn minor_and_reserved_header_fields_warn_only() {
    let mut bytes = valid_container(&[("audio/01.bin", b"one")]);
    // Strip TAIL: its digest covers the header, so header edits would
    // (correctly) become a digest mismatch error. The reference's own
    // version test uses a tail-less container for the same reason.
    let reader = raw_scan(&bytes).expect("scans");
    bytes.truncate(reader.tail_offset() as usize);
    bytes[5] = 9; // minor
    bytes[8] = 1; // reserved nonzero
    let report = verify_mpak(Arc::new(MemorySource::new(bytes))).expect("opens");
    assert!(report.is_ok(), "{:?}", report.findings());
    let messages: Vec<&str> = report
        .findings()
        .iter()
        .map(|f| f.message.as_str())
        .collect();
    assert!(messages.iter().any(|m| m.contains("newer minor version 9")));
    assert!(messages.iter().any(|m| m.contains("reserved")));
    assert!(messages.contains(&"completeness unproven (no TAIL)"));
}

#[test]
fn backend_member_lookup_and_bounds() {
    let bytes = valid_container(&[("audio/01.bin", b"one"), ("extras/x", b"xx")]);
    let backend = MpakBackend::open(Arc::new(MemorySource::new(bytes))).expect("opens");

    // Missing member.
    assert!(backend.open_asset("audio/999.bin").is_err());

    // read_member respects its bound.
    assert!(backend.read_member("audio/01.bin", 2).is_err());
    assert_eq!(backend.read_member("audio/01.bin", 3).unwrap(), b"one");

    // Streamed reads return exactly the member (not a byte more).
    let mut reader = backend.open_asset("extras/x").unwrap().reader;
    let mut out = Vec::new();
    reader.read_to_end(&mut out).unwrap();
    assert_eq!(out, b"xx");
}

#[test]
fn memory_source_bounds_are_enforced() {
    let source = MemorySource::new(vec![1, 2, 3]);
    let mut buf = [0u8; 3];
    assert!(source.read_at(0, &mut buf).is_ok());
    assert!(source.read_at(1, &mut buf).is_err());
    assert!(source.read_at(u64::MAX, &mut [0u8; 1]).is_err());
    assert_eq!(source.size(), 3);
}
