//! The committed reference fixture set for the similarity document v1
//! specification, plus the human-readable manifest and dump renderers.
//!
//! ## Status: SPIKE. Nothing here is production.
//!
//! Every fixture is synthetic. No audio, no model, no library, no server, no
//! filesystem path, no artist/album/track name, no username. A similarity
//! document contains none of those by construction, and the fixtures were
//! chosen to keep it that way: the only "names" in play are two invented
//! profile ids used to compute two invented fingerprints.
//!
//! The fixture *vectors* are not embeddings. They are unit vectors with values
//! chosen to be exact in both binary32 and binary16, so a reviewer can check
//! the bytes by hand and so the f16 encoding fixture is bit-exact rather than
//! approximately right.
//!
//! See [`super::docfmt`] for the codec and `FORMAT_SPEC.md` §15 for the fixture
//! manifest grammar.

use super::docfmt::{
    self, Admission, Document, ENCODING_F16LE, ENCODING_F32LE, ENTRY_BYTES,
    FLAG_RESERVED_ALBUM_AGGREGATE, FORMAT_MAJOR, FORMAT_MINOR, HEADER_BYTES, Member, ProfileFields,
    STATUS_FAILED, STATUS_INSUFFICIENT_AUDIO, STATUS_OK, STATUS_UNSUPPORTED, TABLE_OFFSET,
    element_size, encoding_name, fingerprint_hex, profile_fingerprint, sha256_hex, status_name,
};

/// File extension for the committed fixtures. The path inside a package is the
/// writer's choice; the format is path-agnostic.
pub const FIXTURE_EXTENSION: &str = "msim";

/// Name of the fixture manifest inside the fixture directory.
pub const MANIFEST_NAME: &str = "MANIFEST.txt";

/// Directory name, relative to the experiment root.
pub const FIXTURE_DIRECTORY: &str = "fixtures/similarity-doc";

// ---------------------------------------------------------------------------
// Synthetic profiles
// ---------------------------------------------------------------------------

/// Profile A: the reference profile for the fixtures. Four dimensions, f32
/// elements, no model, no runtime.
///
/// `profile_id` is invented for this spike. It is not a supported MusicPack
/// profile, names no model family, and is not a claim that a shipped profile
/// will look like this.
pub fn profile_a() -> ProfileFields {
    ProfileFields {
        profile_id: "musicpack-similarity-fixture-v1".to_string(),
        model_family: "synthetic-fixture".to_string(),
        model_variant: "none".to_string(),
        model_sha256: None,
        preprocessing_version: "synthetic-linear-v1".to_string(),
        patch_hop: 32,
        pooling: "mean-of-l2-unit-then-l2".to_string(),
        normalization: "l2".to_string(),
        metric: "cosine".to_string(),
        dimensions: 4,
        output_encoding: "f32le".to_string(),
        album_aggregation: None,
        runtime_id: None,
        runtime_version: None,
        numeric_policy: "scalar-no-contraction".to_string(),
    }
}

/// Profile B: identical to A except that its output encoding is binary16.
///
/// It exists to pin two things at once: that `f16le` is implementable from this
/// specification, and that a different output encoding is a *different profile*
/// with a *different fingerprint* — so a half-precision document can never land
/// in the same partition as a full-precision one.
pub fn profile_b() -> ProfileFields {
    ProfileFields {
        profile_id: "musicpack-similarity-fixture-f16-v1".to_string(),
        output_encoding: "f16le".to_string(),
        ..profile_a()
    }
}

/// Profile A's fingerprint.
pub fn fingerprint_a() -> [u8; 32] {
    profile_fingerprint(&profile_a())
}

/// Profile B's fingerprint.
pub fn fingerprint_b() -> [u8; 32] {
    profile_fingerprint(&profile_b())
}

// ---------------------------------------------------------------------------
// Valid documents
// ---------------------------------------------------------------------------

fn member(disc: u32, track: u32, status: u8, vector: Option<Vec<f32>>) -> Member {
    Member {
        disc,
        track,
        status,
        vector,
    }
}

fn document(fingerprint: [u8; 32], encoding: u8, members: Vec<Member>) -> Document {
    Document {
        major: FORMAT_MAJOR,
        minor: FORMAT_MINOR,
        fingerprint,
        dimensions: 4,
        encoding,
        flags: 0,
        members,
    }
}

/// The minimal document: one member, one vector, nothing else in play.
pub fn minimal_ok() -> Vec<u8> {
    docfmt::encode(&document(
        fingerprint_a(),
        ENCODING_F32LE,
        vec![member(1, 1, STATUS_OK, Some(vec![1.0, 0.0, 0.0, 0.0]))],
    ))
    .expect("minimal document is valid")
}

/// Four members covering every defined status, across two discs.
pub fn multi_track() -> Vec<u8> {
    docfmt::encode(&document(
        fingerprint_a(),
        ENCODING_F32LE,
        vec![
            member(1, 1, STATUS_OK, Some(vec![0.5, 0.5, 0.5, 0.5])),
            member(1, 2, STATUS_INSUFFICIENT_AUDIO, None),
            member(1, 3, STATUS_UNSUPPORTED, None),
            member(2, 1, STATUS_FAILED, None),
        ],
    ))
    .expect("multi-track document is valid")
}

/// Two members with vectors; the determinism fixture.
pub fn determinism_ok() -> Vec<u8> {
    docfmt::encode(&document(
        fingerprint_a(),
        ENCODING_F32LE,
        vec![
            member(1, 1, STATUS_OK, Some(vec![0.0, 0.0, 0.0, 1.0])),
            member(1, 2, STATUS_OK, Some(vec![0.5, 0.5, 0.5, 0.5])),
        ],
    ))
    .expect("determinism document is valid")
}

/// The same logical content as [`determinism_ok`] in binary16.
///
/// Every element is exactly representable, so this fixture pins the encoding
/// and the rounding rule without pretending to say anything about whether f16
/// is acceptable (it is not yet measured; see the spec §7.3).
pub fn f16_encoding() -> Vec<u8> {
    docfmt::encode(&document(
        fingerprint_b(),
        ENCODING_F16LE,
        vec![
            member(1, 1, STATUS_OK, Some(vec![0.0, 0.0, 0.0, 1.0])),
            member(1, 2, STATUS_OK, Some(vec![0.5, 0.5, 0.5, 0.5])),
        ],
    ))
    .expect("f16 document is valid")
}

/// A structurally valid document whose `status = ok` vector is all zeros.
///
/// **This is the trap fixture.** The format cannot distinguish it from a real
/// result: an all-zero vector is a legal sequence of four finite values. What
/// it demonstrates is that the *substitution* is impossible — to store a zero
/// vector you must also claim `status = ok`, and a consumer that knows the
/// profile normalizes to L2 can detect the lie. See the spec §6.2.
pub fn all_zero_vector() -> Vec<u8> {
    docfmt::encode(&document(
        fingerprint_a(),
        ENCODING_F32LE,
        vec![member(1, 1, STATUS_OK, Some(vec![0.0, 0.0, 0.0, 0.0]))],
    ))
    .expect("all-zero vector is structurally valid")
}

// ---------------------------------------------------------------------------
// Byte patching, for the invalid fixtures
// ---------------------------------------------------------------------------

/// Return `bytes` with one byte replaced. Panics on a bad offset, which can
/// only be a bug in a fixture definition.
fn patch(mut bytes: Vec<u8>, offset: usize, value: u8) -> Vec<u8> {
    assert!(offset < bytes.len(), "patch offset {offset} out of range");
    bytes[offset] = value;
    bytes
}

/// Overwrite one track-table entry's `(disc, track)` in place, leaving its
/// status, its reserved bytes and the vector region untouched.
fn write_member(bytes: &mut [u8], index: usize, disc: u32, track: u32) {
    let base = TABLE_OFFSET + index * ENTRY_BYTES;
    assert!(
        base + ENTRY_BYTES <= bytes.len(),
        "entry {index} is out of range"
    );
    bytes[base..base + 4].copy_from_slice(&disc.to_be_bytes());
    bytes[base + 4..base + 8].copy_from_slice(&track.to_be_bytes());
}

/// Offsets used by the invalid fixtures, named so the definitions read like the
/// specification table they come from.
mod off {
    /// `magic[2]`
    pub const MAGIC_2: usize = 2;
    /// `format_major` low byte (big-endian: 0x0001 in a valid document)
    pub const MAJOR: usize = 5;
    /// `format_minor` low byte
    pub const MINOR: usize = 7;
    /// `profile_fingerprint[0]`
    pub const FINGERPRINT_0: usize = 8;
    /// `dimensions` low byte
    pub const DIMENSIONS: usize = 41;
    /// `vector_encoding`
    pub const ENCODING: usize = 42;
    /// `flags`
    pub const FLAGS: usize = 43;
    /// `track_count`
    pub const TRACK_COUNT: usize = 44;
    /// `reserved[0]`
    pub const HEADER_RESERVED_0: usize = 48;
    /// `entry[0].status`
    pub const ENTRY_0_STATUS: usize = super::docfmt::TABLE_OFFSET + 8;
    /// `entry[0].reserved[0]`
    pub const ENTRY_0_RESERVED_0: usize = super::docfmt::TABLE_OFFSET + 9;
    /// The first element of the first vector, in a one-member f32 document.
    pub const FIRST_VECTOR_ELEMENT: usize =
        super::docfmt::TABLE_OFFSET + super::docfmt::ENTRY_BYTES;
}

// ---------------------------------------------------------------------------
// The fixture set
// ---------------------------------------------------------------------------

/// One committed fixture.
#[derive(Debug, Clone)]
pub struct Fixture {
    /// File name inside the fixture directory.
    pub name: String,
    /// One line, in the manifest's own words, saying what it pins.
    pub purpose: String,
    /// The exact committed bytes.
    pub bytes: Vec<u8>,
}

/// The complete committed fixture set.
///
/// Order is the order of the manifest, which is the order of the
/// specification: the valid documents first, then one fixture per validation
/// rule.
pub fn fixtures() -> Vec<Fixture> {
    let minimal = minimal_ok();
    let determinism = determinism_ok();
    let mut out: Vec<Fixture> = Vec::new();
    let mut push = |name: &str, purpose: &str, bytes: Vec<u8>| {
        out.push(Fixture {
            name: format!("{name}.{FIXTURE_EXTENSION}"),
            purpose: purpose.to_string(),
            bytes,
        })
    };

    // -- valid ------------------------------------------------------------
    push(
        "minimal-ok",
        "minimal valid document: one member, one f32 vector, no album aggregate",
        minimal.clone(),
    );
    push(
        "multi-track",
        "four members over two discs covering all four status values",
        multi_track(),
    );
    push(
        "determinism-ok",
        "two members with vectors; the fixture the determinism test re-encodes",
        determinism.clone(),
    );
    push(
        "f16-encoding",
        "profile B: same logical vectors as determinism-ok in binary16",
        f16_encoding(),
    );
    push(
        "all-zero-vector",
        "TRAP: status=ok with an all-zero vector. format-valid, profile-invalid",
        all_zero_vector(),
    );

    // -- magic and version ------------------------------------------------
    push(
        "bad-magic",
        "magic[2] is not 'I'",
        patch(minimal.clone(), off::MAGIC_2, b'X'),
    );
    push(
        "unsupported-major",
        "format_major=2",
        patch(minimal.clone(), off::MAJOR, 0x02),
    );
    push(
        "unsupported-minor",
        "format_minor=1, which this specification does not define",
        patch(minimal.clone(), off::MINOR, 0x01),
    );

    // -- profile identity -------------------------------------------------
    push(
        "zero-fingerprint",
        "profile_fingerprint is 32 zero bytes",
        {
            let mut bytes = minimal.clone();
            for byte in bytes[off::FINGERPRINT_0..off::FINGERPRINT_0 + 32].iter_mut() {
                *byte = 0;
            }
            bytes
        },
    );

    // -- dimensions and encoding ------------------------------------------
    push(
        "dimensions-zero",
        "dimensions=0",
        patch(minimal.clone(), off::DIMENSIONS, 0x00),
    );
    push(
        "dimensions-too-large",
        "dimensions=4097, one over the maximum",
        {
            let mut bytes = minimal.clone();
            bytes[off::DIMENSIONS - 1] = 0x10;
            bytes[off::DIMENSIONS] = 0x01;
            bytes
        },
    );
    push(
        "unsupported-encoding",
        "vector_encoding=3, which is not assigned",
        patch(minimal.clone(), off::ENCODING, 0x03),
    );

    // -- reserved ---------------------------------------------------------
    push(
        "reserved-flag-set",
        "flags bit 0 set: the reserved album-aggregate bit is not defined in v1.0",
        patch(minimal.clone(), off::FLAGS, FLAG_RESERVED_ALBUM_AGGREGATE),
    );
    push(
        "header-reserved-nonzero",
        "the first byte of the 16-byte header reserved region is non-zero",
        patch(minimal.clone(), off::HEADER_RESERVED_0, 0x01),
    );
    push(
        "entry-reserved-nonzero",
        "the first reserved byte of entry[0] is non-zero",
        patch(minimal.clone(), off::ENTRY_0_RESERVED_0, 0x01),
    );

    // -- counts -----------------------------------------------------------
    push("track-count-zero", "track_count=0", {
        let mut bytes = minimal.clone();
        bytes[off::TRACK_COUNT..off::TRACK_COUNT + 4].copy_from_slice(&0u32.to_be_bytes());
        bytes
    });
    push(
        "track-count-too-large",
        "track_count=16385, one over MAX_DISCS*MAX_TRACKS_PER_DISC",
        {
            let mut bytes = minimal.clone();
            bytes[off::TRACK_COUNT..off::TRACK_COUNT + 4]
                .copy_from_slice(&(docfmt::MAX_TRACKS + 1).to_be_bytes());
            bytes
        },
    );

    // -- track table ------------------------------------------------------
    push(
        "unknown-status",
        "entry[0].status=4, one past the last defined value",
        patch(minimal.clone(), off::ENTRY_0_STATUS, 0x04),
    );
    push(
        "invalid-member",
        "entry[0].disc=0, which no manifest track can have",
        {
            let mut bytes = minimal.clone();
            write_member(&mut bytes, 0, 0, 1);
            bytes
        },
    );
    push(
        "unsorted-table",
        "entry[0] is (2,1) and entry[1] is (1,2): the table is not ascending",
        {
            let mut bytes = determinism.clone();
            // Canonical order would be (1,1) then (1,2). Write (2,1) first.
            write_member(&mut bytes, 0, 2, 1);
            write_member(&mut bytes, 1, 1, 2);
            bytes
        },
    );
    push(
        "duplicate-member",
        "entry[1] repeats entry[0]'s (disc, track)",
        {
            let mut bytes = determinism.clone();
            write_member(&mut bytes, 1, 1, 1);
            bytes
        },
    );

    // -- size -------------------------------------------------------------
    push(
        "truncated-header",
        "40 bytes: the header is not complete",
        minimal[..40].to_vec(),
    );
    push(
        "truncated-table",
        "header plus six bytes of a twelve-byte entry",
        minimal[..HEADER_BYTES + 6].to_vec(),
    );
    push(
        "truncated",
        "one f32 element short of the declared layout",
        minimal[..minimal.len() - 4].to_vec(),
    );
    push("trailing-bytes", "one byte after the declared layout", {
        let mut bytes = minimal.clone();
        bytes.push(0x00);
        bytes
    });

    // -- vector contents --------------------------------------------------
    push(
        "non-finite-vector",
        "the first element of the only vector is a binary32 NaN",
        {
            let mut bytes = minimal.clone();
            let at = off::FIRST_VECTOR_ELEMENT;
            bytes[at..at + 4].copy_from_slice(&f32::NAN.to_le_bytes());
            bytes
        },
    );

    out
}

// ---------------------------------------------------------------------------
// Lenient field rendering, for the manifest
// ---------------------------------------------------------------------------

/// The header fields as a `key=value` line, read without validating.
///
/// Used only to describe fixtures that are *meant* to be invalid: the manifest
/// has to be able to say what a broken document claims, which is precisely what
/// a strict parser refuses to interpret.
fn lenient_header(bytes: &[u8]) -> Option<String> {
    if bytes.len() < HEADER_BYTES {
        return None;
    }
    let major = u16::from_be_bytes([bytes[4], bytes[5]]);
    let minor = u16::from_be_bytes([bytes[6], bytes[7]]);
    let dimensions = u16::from_be_bytes([bytes[40], bytes[41]]);
    let track_count = u32::from_be_bytes([bytes[44], bytes[45], bytes[46], bytes[47]]);
    let encoding = match encoding_name(bytes[42]) {
        Some(name) => name.to_string(),
        None => format!("undefined:{}", bytes[42]),
    };
    Some(format!(
        "major={major} minor={minor} dimensions={dimensions} encoding={encoding} track_count={track_count} flags=0x{:02x}",
        bytes[43]
    ))
}

/// The track table as `entry<N>=…` lines, read without validating.
///
/// Stops at the first entry the bytes do not reach and then reports how many
/// were declared, so a document claiming 16385 members produces three lines
/// instead of 16385.
fn lenient_table(bytes: &[u8], encoding: u8, dimensions: u16, track_count: u32) -> Vec<String> {
    let mut out = Vec::new();
    let element = element_size(encoding).unwrap_or(0);
    for index in 0..track_count {
        let base = TABLE_OFFSET + index as usize * ENTRY_BYTES;
        if base + ENTRY_BYTES > bytes.len() {
            out.push(format!(
                "table_note=track_count declares {track_count} entries; the bytes end at {}",
                bytes.len()
            ));
            break;
        }
        let disc = u32::from_be_bytes([
            bytes[base],
            bytes[base + 1],
            bytes[base + 2],
            bytes[base + 3],
        ]);
        let track = u32::from_be_bytes([
            bytes[base + 4],
            bytes[base + 5],
            bytes[base + 6],
            bytes[base + 7],
        ]);
        let status = bytes[base + 8];
        let status_text = status_name(status)
            .map(|name| name.to_string())
            .unwrap_or_else(|| format!("undefined:{status}"));
        let start = TABLE_OFFSET
            + track_count as usize * ENTRY_BYTES
            + index as usize * dimensions as usize * element;
        let end = start + dimensions as usize * element;
        let vector = if status == STATUS_OK && element == 0 {
            "vector=unreadable (no decodable encoding, or zero dimensions)".to_string()
        } else if status == STATUS_OK && end <= bytes.len() {
            let values: Vec<String> = (0..dimensions as usize)
                .map(|position| {
                    let at = start + position * element;
                    match encoding {
                        ENCODING_F32LE => format!(
                            "{}",
                            f32::from_le_bytes([
                                bytes[at],
                                bytes[at + 1],
                                bytes[at + 2],
                                bytes[at + 3]
                            ])
                        ),
                        _ => format!(
                            "{}",
                            docfmt::f16_bits_to_f32(u16::from_le_bytes([bytes[at], bytes[at + 1]]))
                        ),
                    }
                })
                .collect();
            format!("vector=[{}]", values.join(", "))
        } else {
            "vector=none".to_string()
        };
        out.push(format!(
            "entry{index}=disc={disc} track={track} status={status_text} {vector}"
        ));
    }
    out
}

/// The fingerprint as lowercase hex, or `absent` when the header is too short.
///
/// Read from the bytes rather than from the fixture's declared profile, so the
/// manifest reports what the file actually says.
fn lenient_fingerprint(bytes: &[u8]) -> String {
    if bytes.len() < HEADER_BYTES {
        return "absent".to_string();
    }
    let mut fingerprint = [0u8; 32];
    fingerprint.copy_from_slice(&bytes[8..40]);
    docfmt::fingerprint_hex(&fingerprint)
}

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

/// Render the fixture manifest.
///
/// The grammar is deliberately trivial so that it can be checked by a test and
/// read by a reviewer: `#` comments, an unindented fixture name, then indented
/// `key=value` lines. It is a harness convention, not part of the document
/// format.
pub fn manifest() -> String {
    let mut out = String::new();
    out.push_str("# MusicPack similarity document v1 — reference fixture manifest\n");
    out.push_str("#\n");
    out.push_str("# Generated from the reference codec in `src/docfmt.rs`; regenerate with\n");
    out.push_str("# `cargo run --manifest-path experiments/music-similarity-eval/Cargo.toml --bin docfmt -- emit fixtures/similarity-doc`\n");
    out.push_str("# and prove it unchanged with `cargo test --manifest-path experiments/music-similarity-eval/Cargo.toml docfmt`.\n");
    out.push_str("# The committed bytes are the contract; this file describes them.\n");
    out.push_str("#\n");
    out.push_str("# Every fixture is synthetic. No audio, model, library path, filesystem path,\n");
    out.push_str("# artist, album, track title or username appears in this directory or in any\n");
    out.push_str("# byte of any fixture.\n");
    out.push_str("#\n");
    out.push_str("# profile_id musicpack-similarity-fixture-v1 (profile A)\n");
    out.push_str(&format!(
        "# profile_fingerprint {}\n",
        fingerprint_hex(&fingerprint_a())
    ));
    out.push_str("# profile_id musicpack-similarity-fixture-f16-v1 (profile B)\n");
    out.push_str(&format!(
        "# profile_fingerprint {}\n",
        fingerprint_hex(&fingerprint_b())
    ));
    out.push_str("#\n");
    out.push_str("# keys: bytes, sha256, profile_fingerprint, header, entry<N>, result,\n");
    out.push_str("#       package_effect\n");
    out.push_str("# result is `valid` or `invalid code=<code> detail=<detail>`. detail contains\n");
    out.push_str("# spaces; the code is the stable part.\n");
    out.push_str(
        "# package_effect is `none` for every fixture: a document finding never changes\n",
    );
    out.push_str("# whether the *package* verifies (FORMAT_SPEC.md §13).\n");
    out.push('\n');

    for fixture in fixtures() {
        out.push_str(&format!("{}\n", fixture.name));
        out.push_str(&format!("  purpose={}\n", fixture.purpose));
        out.push_str(&format!("  bytes={}\n", fixture.bytes.len()));
        out.push_str(&format!("  sha256={}\n", sha256_hex(&fixture.bytes)));
        out.push_str(&format!(
            "  profile_fingerprint={}\n",
            lenient_fingerprint(&fixture.bytes)
        ));
        match docfmt::validate(&fixture.bytes) {
            Ok(document) => {
                out.push_str(&format!("  header={}\n", header_summary(&document)));
                for line in lenient_table(
                    &fixture.bytes,
                    document.encoding,
                    document.dimensions as u16,
                    document.members.len() as u32,
                ) {
                    out.push_str(&format!("  {line}\n"));
                }
                out.push_str("  result=valid\n");
            }
            Err(error) => {
                if let Some(header) = lenient_header(&fixture.bytes) {
                    out.push_str(&format!("  header={header}\n"));
                    if let Some(track_count) = track_count_of(&fixture.bytes) {
                        for line in lenient_table(
                            &fixture.bytes,
                            fixture.bytes.get(42).copied().unwrap_or(0),
                            u16::from_be_bytes([fixture.bytes[40], fixture.bytes[41]]),
                            track_count,
                        ) {
                            out.push_str(&format!("  {line}\n"));
                        }
                    }
                }
                out.push_str(&format!(
                    "  result=invalid code={} detail={}\n",
                    error.code, error.detail
                ));
            }
        }
        out.push_str("  package_effect=none\n");
        out.push('\n');
    }
    out
}

fn header_summary(document: &Document) -> String {
    format!(
        "major={} minor={} dimensions={} encoding={} track_count={} flags=0x{:02x}",
        document.major,
        document.minor,
        document.dimensions,
        encoding_name(document.encoding).unwrap_or("undefined"),
        document.members.len(),
        document.flags
    )
}

fn track_count_of(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < HEADER_BYTES {
        return None;
    }
    Some(u32::from_be_bytes([
        bytes[44], bytes[45], bytes[46], bytes[47],
    ]))
}

// ---------------------------------------------------------------------------
// Dump
// ---------------------------------------------------------------------------

/// Render an annotated hex dump of one document, for review.
///
/// The dump is derived from the same offsets the validator uses, so it cannot
/// quietly disagree with it about the layout.
pub fn dump(name: &str, bytes: &[u8]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{name}: {} bytes\nsha256: {}\n\n",
        bytes.len(),
        sha256_hex(bytes)
    ));
    if bytes.len() < HEADER_BYTES {
        out.push_str(&format!(
            "header: INCOMPLETE ({} of {HEADER_BYTES} bytes)\n\n",
            bytes.len()
        ));
        hex_block(&mut out, bytes, 0);
        return out;
    }
    let field = |out: &mut String, at: usize, len: usize, label: &str, value: String| {
        let slice = &bytes[at..at + len];
        out.push_str(&format!(
            "  {at:>5}  {:<23}  {label:<24}  {value}\n",
            docfmt::hex_bytes(slice),
        ));
    };
    out.push_str("header (64 bytes)\n");
    field(
        &mut out,
        0,
        4,
        "magic",
        format!("{:?}", std::str::from_utf8(&bytes[0..4]).unwrap_or("?")),
    );
    field(
        &mut out,
        4,
        2,
        "format_major",
        u16::from_be_bytes([bytes[4], bytes[5]]).to_string(),
    );
    field(
        &mut out,
        6,
        2,
        "format_minor",
        u16::from_be_bytes([bytes[6], bytes[7]]).to_string(),
    );
    field(
        &mut out,
        8,
        32,
        "profile_fingerprint",
        docfmt::hex_bytes(&bytes[8..40]),
    );
    field(
        &mut out,
        40,
        2,
        "dimensions",
        u16::from_be_bytes([bytes[40], bytes[41]]).to_string(),
    );
    field(
        &mut out,
        42,
        1,
        "vector_encoding",
        format!(
            "{} ({})",
            bytes[42],
            encoding_name(bytes[42]).unwrap_or("unassigned")
        ),
    );
    field(
        &mut out,
        43,
        1,
        "flags",
        format!(
            "0x{:02x}{}",
            bytes[43],
            if bytes[43] == 0 {
                " (reserved: all zero)"
            } else {
                " (RESERVED BIT SET)"
            }
        ),
    );
    field(
        &mut out,
        44,
        4,
        "track_count",
        u32::from_be_bytes([bytes[44], bytes[45], bytes[46], bytes[47]]).to_string(),
    );
    field(
        &mut out,
        48,
        16,
        "reserved",
        if bytes[48..64].iter().all(|b| *b == 0) {
            "all zero".to_string()
        } else {
            docfmt::hex_bytes(&bytes[48..64])
        },
    );

    let track_count = u32::from_be_bytes([bytes[44], bytes[45], bytes[46], bytes[47]]) as usize;
    let dimensions = u16::from_be_bytes([bytes[40], bytes[41]]) as usize;
    let element = element_size(bytes[42]).unwrap_or(0);
    let table_end = TABLE_OFFSET + track_count * ENTRY_BYTES;
    out.push_str(&format!(
        "\ntrack table ({track_count} entries x {ENTRY_BYTES} bytes)\n"
    ));
    for index in 0..track_count {
        let base = TABLE_OFFSET + index * ENTRY_BYTES;
        if base + ENTRY_BYTES > bytes.len() {
            out.push_str(&format!(
                "  entry[{index}] absent: file ends at {}\n",
                bytes.len()
            ));
            break;
        }
        let disc = u32::from_be_bytes([
            bytes[base],
            bytes[base + 1],
            bytes[base + 2],
            bytes[base + 3],
        ]);
        let track = u32::from_be_bytes([
            bytes[base + 4],
            bytes[base + 5],
            bytes[base + 6],
            bytes[base + 7],
        ]);
        out.push_str(&format!(
            "  entry[{index}] disc={disc} track={track} status={} reserved={}\n",
            bytes[base + 8],
            docfmt::hex_bytes(&bytes[base + 9..base + ENTRY_BYTES])
        ));
    }
    out.push_str(&format!("\nvector region (starts at {table_end})\n"));
    if table_end > bytes.len() {
        out.push_str(&format!("  absent: file ends at {}\n", bytes.len()));
    } else {
        for index in 0..track_count {
            let base = TABLE_OFFSET + index * ENTRY_BYTES;
            if base + ENTRY_BYTES > bytes.len() {
                break;
            }
            let status = bytes[base + 8];
            let start = table_end + index * dimensions * element;
            let end = start + dimensions * element;
            if status != STATUS_OK {
                out.push_str(&format!(
                    "  vector[{index}] none (status={})\n",
                    status_name(status).unwrap_or("undefined")
                ));
                continue;
            }
            if end > bytes.len() {
                out.push_str(&format!(
                    "  vector[{index}] truncated: needs bytes {start}..{end}, file has {}\n",
                    bytes.len()
                ));
                continue;
            }
            let values: Vec<String> = (0..dimensions)
                .map(|position| {
                    let at = start + position * element;
                    match bytes[42] {
                        ENCODING_F32LE => format!("0x{}", docfmt::hex_bytes(&bytes[at..at + 4])),
                        _ => format!("0x{}", docfmt::hex_bytes(&bytes[at..at + 2])),
                    }
                })
                .collect();
            out.push_str(&format!("  vector[{index}] {}\n", values.join(" ")));
        }
        if bytes.len() > expected_total(bytes) {
            out.push_str(&format!(
                "  trailing bytes: {} beyond the declared layout\n",
                bytes.len() - expected_total(bytes)
            ));
        } else if bytes.len() < expected_total(bytes) {
            out.push_str(&format!(
                "  missing bytes: {} short of the declared layout\n",
                expected_total(bytes) - bytes.len()
            ));
        }
    }

    out.push_str("\nvalidation\n");
    match docfmt::validate(bytes) {
        Ok(document) => {
            out.push_str("  document structure: valid\n");
            out.push_str(&format!(
                "  admission vs profile A: {}\n",
                match docfmt::admit(&document, &fingerprint_a()) {
                    Admission::Admitted => "admitted",
                    Admission::ProfileMismatch => "profile_mismatch (expected for profile B)",
                }
            ));
            out.push_str(&format!(
                "  admission vs profile B: {}\n",
                match docfmt::admit(&document, &fingerprint_b()) {
                    Admission::Admitted => "admitted",
                    Admission::ProfileMismatch => "profile_mismatch (expected for profile A)",
                }
            ));
        }
        Err(error) => {
            out.push_str(&format!("  document structure: invalid\n  {}\n", error));
        }
    }
    out
}

fn expected_total(bytes: &[u8]) -> usize {
    let track_count = u32::from_be_bytes([bytes[44], bytes[45], bytes[46], bytes[47]]) as usize;
    let dimensions = u16::from_be_bytes([bytes[40], bytes[41]]) as usize;
    let element = element_size(bytes[42]).unwrap_or(0);
    let contributors = (0..track_count)
        .filter(|index| {
            let base = TABLE_OFFSET + index * ENTRY_BYTES;
            base + ENTRY_BYTES <= bytes.len() && bytes[base + 8] == STATUS_OK
        })
        .count();
    TABLE_OFFSET + track_count * ENTRY_BYTES + contributors * dimensions * element
}

fn hex_block(out: &mut String, bytes: &[u8], from: usize) {
    for (index, chunk) in bytes[from..].chunks(16).enumerate() {
        out.push_str(&format!(
            "  {:>5}  {:<47}\n",
            from + index * 16,
            docfmt::hex_bytes(chunk)
        ));
    }
}
