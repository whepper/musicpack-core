//! Shared test support: a deterministic Rust port of the reference
//! repository's v1 conformance corpus generator
//! (`tests/generate_mpack_conformance.py`), including a minimal SHA-256
//! for the corpus digests.
//!
//! The module is compiled into several test binaries, each of which uses a
//! subset of it, so unused-item warnings are expected here.
#![allow(dead_code)]
//!
//! The corpus is the authoritative behavioural spec for manifest-level
//! accept/reject decisions; this port reproduces its bytes exactly. When
//! a Python 3 interpreter and the reference repository are available
//! (see [`reference_dir`]), `tests/conformance_corpus.rs` byte-compares
//! this port's output against the authoritative generator and fails on
//! any divergence.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------
// SHA-256 (FIPS 180-4, compact form for corpus digests)
// ---------------------------------------------------------------------

/// SHA-256 of `data`, lowercase hex.
pub fn sha256_hex(data: &[u8]) -> String {
    let digest = sha256(data);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

// ---------------------------------------------------------------------
// CPython-compatible PRNG (for reference fuzz-lite replay)
// ---------------------------------------------------------------------

/// A minimal MT19937 matching CPython's `random.Random` for integer seeds.
///
/// The reference repository's manifest fuzz-lite mutates a manifest with
/// `random.Random(1000 + k)` and `randrange`; reproducing the exact stream
/// lets the Rust replay use byte-identical mutations. Verified against
/// committed CPython vectors in the tests below.
pub struct PyRandom {
    mt: [u32; 624],
    index: usize,
}

const PY_N: usize = 624;
const PY_M: usize = 397;
const PY_MATRIX_A: u32 = 0x9908_b0df;
const PY_UPPER_MASK: u32 = 0x8000_0000;
const PY_LOWER_MASK: u32 = 0x7fff_ffff;

impl PyRandom {
    /// `random.Random(seed)` for a non-negative integer seed.
    pub fn new(seed: u32) -> Self {
        let mut rng = Self {
            mt: [0; PY_N],
            index: PY_N + 1,
        };
        rng.init_genrand(19650218);
        rng.init_by_array(&[seed]);
        rng
    }

    fn init_genrand(&mut self, s: u32) {
        self.mt[0] = s;
        for i in 1..PY_N {
            let prev = self.mt[i - 1];
            self.mt[i] = 1_812_433_253u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
        }
        self.index = PY_N;
    }

    fn init_by_array(&mut self, key: &[u32]) {
        let mut i = 1usize;
        let mut j = 0usize;
        let mut k = PY_N.max(key.len());
        while k > 0 {
            let prev = self.mt[i - 1];
            self.mt[i] = (self.mt[i] ^ ((prev ^ (prev >> 30)).wrapping_mul(1_664_525)))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= PY_N {
                self.mt[0] = self.mt[PY_N - 1];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
            k -= 1;
        }
        let mut k = PY_N - 1;
        while k > 0 {
            let prev = self.mt[i - 1];
            self.mt[i] = (self.mt[i] ^ ((prev ^ (prev >> 30)).wrapping_mul(1_566_083_941)))
                .wrapping_sub(i as u32);
            i += 1;
            if i >= PY_N {
                self.mt[0] = self.mt[PY_N - 1];
                i = 1;
            }
            k -= 1;
        }
        self.mt[0] = 0x8000_0000;
    }

    fn genrand_uint32(&mut self) -> u32 {
        if self.index >= PY_N {
            for kk in 0..(PY_N - PY_M) {
                let y = (self.mt[kk] & PY_UPPER_MASK) | (self.mt[kk + 1] & PY_LOWER_MASK);
                self.mt[kk] =
                    self.mt[kk + PY_M] ^ (y >> 1) ^ if y & 1 != 0 { PY_MATRIX_A } else { 0 };
            }
            for kk in (PY_N - PY_M)..(PY_N - 1) {
                let y = (self.mt[kk] & PY_UPPER_MASK) | (self.mt[kk + 1] & PY_LOWER_MASK);
                self.mt[kk] =
                    self.mt[kk + PY_M - PY_N] ^ (y >> 1) ^ if y & 1 != 0 { PY_MATRIX_A } else { 0 };
            }
            let y = (self.mt[PY_N - 1] & PY_UPPER_MASK) | (self.mt[0] & PY_LOWER_MASK);
            self.mt[PY_N - 1] =
                self.mt[PY_M - 1] ^ (y >> 1) ^ if y & 1 != 0 { PY_MATRIX_A } else { 0 };
            self.index = 0;
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^= y >> 18;
        y
    }

    /// CPython `getrandbits(k)` for `0 < k <= 32`.
    pub fn getrandbits(&mut self, k: u32) -> u32 {
        debug_assert!((1..=32).contains(&k));
        self.genrand_uint32() >> (32 - k)
    }

    /// CPython `randrange(n)` for `n > 0`.
    pub fn randrange(&mut self, n: u32) -> u32 {
        assert!(n > 0);
        let k = 32 - n.leading_zeros();
        let mut r = self.getrandbits(k);
        while r >= n {
            r = self.getrandbits(k);
        }
        r
    }
}

// ---------------------------------------------------------------------
// Python-style JSON (json.dump / json.dumps with ensure_ascii=False)
// ---------------------------------------------------------------------

/// An ordered JSON value mirroring the Python corpus generator's dicts.
#[derive(Clone)]
pub enum Py {
    /// Python `bool`.
    Bool(bool),
    /// Python `int`.
    Int(i64),
    /// Python `float` (printed like `repr`, e.g. `-12.0`).
    Float(f64),
    /// Python `str`.
    Str(String),
    /// Python `None`.
    #[allow(dead_code)] // part of the value model; unused by current cases
    Null,
    /// Python `list`.
    Arr(Vec<Py>),
    /// Python `dict` (insertion-ordered).
    Obj(Vec<(&'static str, Py)>),
}

impl Py {
    /// Convenience constructor for string values.
    pub fn s(v: impl Into<String>) -> Py {
        Py::Str(v.into())
    }
}

fn escape_python_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_python_value(v: &Py, indent: Option<usize>, depth: usize, out: &mut String) {
    match v {
        Py::Bool(true) => out.push_str("true"),
        Py::Bool(false) => out.push_str("false"),
        Py::Null => out.push_str("null"),
        Py::Int(i) => out.push_str(&i.to_string()),
        Py::Float(f) => {
            // repr() of the floats used in the corpus: always -12.0 / 1.5 /
            // -1.0 style — shortest representation that round-trips, with
            // ".0" for integral values.
            if f.fract() == 0.0 && f.is_finite() && f.abs() < 1e16 {
                out.push_str(&format!("{f:.1}"));
            } else {
                out.push_str(&format!("{f}"));
            }
        }
        Py::Str(s) => escape_python_string(s, out),
        Py::Arr(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                match indent {
                    Some(n) => {
                        out.push('\n');
                        out.push_str(&" ".repeat(n * (depth + 1)));
                    }
                    None if i > 0 => out.push(' '),
                    None => {}
                }
                write_python_value(item, indent, depth + 1, out);
            }
            if let Some(n) = indent {
                out.push('\n');
                out.push_str(&" ".repeat(n * depth));
            }
            out.push(']');
        }
        Py::Obj(members) => {
            if members.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (key, value)) in members.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                match indent {
                    Some(n) => {
                        out.push('\n');
                        out.push_str(&" ".repeat(n * (depth + 1)));
                    }
                    None if i > 0 => out.push(' '),
                    None => {}
                }
                escape_python_string(key, out);
                out.push_str(": ");
                write_python_value(value, indent, depth + 1, out);
            }
            if let Some(n) = indent {
                out.push('\n');
                out.push_str(&" ".repeat(n * depth));
            }
            out.push('}');
        }
    }
}

/// `json.dumps(v)` (compact, ensure_ascii=False).
pub fn dumps(v: &Py) -> String {
    let mut out = String::new();
    write_python_value(v, None, 0, &mut out);
    out
}

/// `json.dump(v, indent=2, ensure_ascii=False)` + trailing newline —
/// exactly how the generator writes `manifest.json`.
pub fn dump_manifest(v: &Py) -> String {
    let mut out = String::new();
    write_python_value(v, Some(2), 0, &mut out);
    out.push('\n');
    out
}

// ---------------------------------------------------------------------
// The corpus generator port
// ---------------------------------------------------------------------

/// One corpus case: a package directory with manifest + files.
pub struct Case {
    /// Case name (`<name>.mpack` directory).
    pub name: &'static str,
    /// Group per the authoritative generator.
    pub group: Group,
    /// The manifest to write (or raw bytes for hand-built manifests).
    pub manifest: ManifestCase,
    /// Package files to write.
    pub files: Vec<(&'static str, &'static [u8])>,
}

/// The three authoritative groups (cases.json).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// `info` and `verify` must succeed.
    Valid,
    /// `info` and `verify` must fail.
    InvalidManifest,
    /// `verify` must fail (`info` unconstrained by the runner).
    InvalidVerify,
}

/// Manifest content for a case.
pub enum ManifestCase {
    /// Generated from the ordered value (json.dump indent=2 style).
    Generated(Py),
    /// Exact bytes (the raw/duplicate-key cases).
    Raw(Vec<u8>),
}

fn base_manifest(path: &str, digest: &str) -> Py {
    Py::Obj(vec![
        ("format", Py::s("musicpack")),
        ("version", Py::Int(1)),
        (
            "album",
            Py::Obj(vec![
                ("title", Py::s("Conformance")),
                (
                    "artists",
                    Py::Arr(vec![Py::Obj(vec![("name", Py::s("Tester"))])]),
                ),
            ]),
        ),
        (
            "media",
            Py::Arr(vec![Py::Obj(vec![
                ("disc", Py::Int(1)),
                (
                    "tracks",
                    Py::Arr(vec![Py::Obj(vec![
                        ("track", Py::Int(1)),
                        ("title", Py::s("One")),
                        (
                            "audio",
                            Py::Obj(vec![("path", Py::s(path)), ("sha256", Py::s(digest))]),
                        ),
                    ])]),
                ),
            ])]),
        ),
    ])
}

fn hash(data: &[u8]) -> String {
    sha256_hex(data)
}

/// Enumerates the entire authoritative corpus (all 72 cases), in the
/// generator's order.
pub fn corpus() -> Vec<Case> {
    let mut cases = Vec::new();
    let one_file = ("audio/01.bin", b"one".as_slice());

    // ---------------- valid ----------------
    cases.push(Case {
        name: "minimal",
        group: Group::Valid,
        manifest: ManifestCase::Generated(base_manifest("audio/01.bin", &hash(b"one"))),
        files: vec![one_file],
    });

    // complete
    let files: Vec<(&'static str, &'static [u8])> = vec![
        ("audio/01.bin", b"one"),
        ("audio/02.bin", b"two"),
        ("audio/03.bin", b"three"),
        ("artwork/front.bin", b"front"),
        ("booklet/booklet.txt", b"booklet"),
        ("lyrics/01.txt", b"lyrics"),
        ("extras/notes.txt", b"notes"),
        ("analysis/external.json", b"{}"),
    ];
    let h = |d: &[u8]| hash(d);
    let complete = Py::Obj(vec![
        ("format", Py::s("musicpack")),
        ("version", Py::Int(1)),
        (
            "album",
            Py::Obj(vec![
                ("title", Py::s("Complete")),
                (
                    "artists",
                    Py::Arr(vec![
                        Py::Obj(vec![("name", Py::s("Alpha")), ("role", Py::s("main"))]),
                        Py::Obj(vec![("name", Py::s("Beta"))]),
                    ]),
                ),
                ("releaseType", Py::s("compilation")),
                ("originalReleaseDate", Py::s("2001-02-03")),
                ("genres", Py::Arr(vec![Py::s("Rock"), Py::s("Pop")])),
            ]),
        ),
        (
            "media",
            Py::Arr(vec![
                Py::Obj(vec![
                    ("disc", Py::Int(1)),
                    ("format", Py::s("Digital")),
                    ("title", Py::s("Main")),
                    (
                        "tracks",
                        Py::Arr(vec![
                            Py::Obj(vec![
                                ("track", Py::Int(1)),
                                ("title", Py::s("One")),
                                (
                                    "artists",
                                    Py::Arr(vec![Py::Obj(vec![("name", Py::s("Alpha"))])]),
                                ),
                                (
                                    "identifiers",
                                    Py::Obj(vec![
                                        ("isrc", Py::s("ISRC")),
                                        ("musicbrainzTrackId", Py::s("track")),
                                        ("musicbrainzRecordingId", Py::s("recording")),
                                    ]),
                                ),
                                (
                                    "source",
                                    Py::Obj(vec![
                                        ("store", Py::s("Store")),
                                        ("trackId", Py::s("one")),
                                    ]),
                                ),
                                (
                                    "sourceAudio",
                                    Py::Obj(vec![("codec", Py::s("flac")), ("md5", Py::s("abc"))]),
                                ),
                                ("duration", Py::Float(1.5)),
                                (
                                    "loudness",
                                    Py::Obj(vec![
                                        ("trackLUFS", Py::Float(-12.0)),
                                        ("truePeakDbTP", Py::Float(-1.0)),
                                    ]),
                                ),
                                (
                                    "audio",
                                    Py::Obj(vec![
                                        ("path", Py::s("audio/01.bin")),
                                        ("sha256", Py::s(h(b"one"))),
                                        ("codec", Py::s("test")),
                                    ]),
                                ),
                            ]),
                            Py::Obj(vec![
                                ("track", Py::Int(2)),
                                ("title", Py::s("Two")),
                                (
                                    "audio",
                                    Py::Obj(vec![
                                        ("path", Py::s("audio/02.bin")),
                                        ("sha256", Py::s(h(b"two"))),
                                    ]),
                                ),
                            ]),
                        ]),
                    ),
                ]),
                Py::Obj(vec![
                    ("disc", Py::Int(2)),
                    ("format", Py::s("CD")),
                    (
                        "tracks",
                        Py::Arr(vec![Py::Obj(vec![
                            ("track", Py::Int(1)),
                            ("title", Py::s("Three")),
                            (
                                "audio",
                                Py::Obj(vec![
                                    ("path", Py::s("audio/03.bin")),
                                    ("sha256", Py::s(h(b"three"))),
                                ]),
                            ),
                        ])]),
                    ),
                ]),
            ]),
        ),
        (
            "release",
            Py::Obj(vec![
                ("releaseDate", Py::s("2020-01-02")),
                ("edition", Py::s("Deluxe")),
                ("country", Py::s("GB")),
                ("label", Py::s("Label")),
                ("catalogueNumber", Py::s("CAT-1")),
                ("notes", Py::s("Notes")),
            ]),
        ),
        (
            "identifiers",
            Py::Obj(vec![
                ("musicbrainzReleaseGroupId", Py::s("group")),
                ("musicbrainzReleaseId", Py::s("release")),
                ("barcode", Py::s("123")),
            ]),
        ),
        (
            "identity",
            Py::Obj(vec![
                ("source", Py::s("local")),
                ("confidence", Py::s("none")),
            ]),
        ),
        (
            "source",
            Py::Obj(vec![
                ("type", Py::s("digital-download")),
                ("store", Py::s("Store")),
                ("sourceId", Py::s("album-id")),
            ]),
        ),
        (
            "artwork",
            Py::Arr(vec![Py::Obj(vec![
                ("role", Py::s("front")),
                ("path", Py::s("artwork/front.bin")),
                ("sha256", Py::s(h(b"front"))),
            ])]),
        ),
        (
            "booklet",
            Py::Arr(vec![Py::Obj(vec![
                ("path", Py::s("booklet/booklet.txt")),
                ("sha256", Py::s(h(b"booklet"))),
            ])]),
        ),
        (
            "lyrics",
            Py::Arr(vec![Py::Obj(vec![
                ("path", Py::s("lyrics/01.txt")),
                ("sha256", Py::s(h(b"lyrics"))),
            ])]),
        ),
        (
            "extras",
            Py::Arr(vec![Py::Obj(vec![
                ("path", Py::s("extras/notes.txt")),
                ("sha256", Py::s(h(b"notes"))),
            ])]),
        ),
        (
            "analysis",
            Py::Arr(vec![Py::Obj(vec![
                ("type", Py::s("future-analysis")),
                ("path", Py::s("analysis/external.json")),
                ("sha256", Py::s(h(b"{}"))),
            ])]),
        ),
        (
            "loudness",
            Py::Obj(vec![
                ("algorithm", Py::s("ITU-R BS.1770-5")),
                ("albumLUFS", Py::Float(-12.0)),
                ("albumTruePeakDbTP", Py::Float(-1.0)),
            ]),
        ),
        (
            "provenance",
            Py::Obj(vec![
                ("tool", Py::s("conformance")),
                ("toolVersion", Py::s("1")),
            ]),
        ),
        ("xFutureField", Py::Obj(vec![("preserved", Py::Bool(true))])),
    ]);
    cases.push(Case {
        name: "complete",
        group: Group::Valid,
        manifest: ManifestCase::Generated(complete),
        files,
    });

    // unicode
    let unicode = Py::Obj(vec![
        ("format", Py::s("musicpack")),
        ("version", Py::Int(1)),
        (
            "album",
            Py::Obj(vec![
                ("title", Py::s("Cafe\u{301}")),
                (
                    "artists",
                    Py::Arr(vec![Py::Obj(vec![("name", Py::s("Beyonce\u{301}"))])]),
                ),
            ]),
        ),
        (
            "media",
            Py::Arr(vec![Py::Obj(vec![
                ("disc", Py::Int(1)),
                (
                    "tracks",
                    Py::Arr(vec![Py::Obj(vec![
                        ("track", Py::Int(1)),
                        ("title", Py::s("One")),
                        (
                            "audio",
                            Py::Obj(vec![
                                ("path", Py::s("audio/01-unicode.bin")),
                                ("sha256", Py::s(hash(b"unicode"))),
                            ]),
                        ),
                    ])]),
                ),
            ])]),
        ),
        (
            "artwork",
            Py::Arr(vec![Py::Obj(vec![
                ("role", Py::s("front")),
                ("path", Py::s("artwork/back.bin")),
                ("sha256", Py::s(hash(b"back"))),
            ])]),
        ),
    ]);
    cases.push(Case {
        name: "unicode",
        group: Group::Valid,
        manifest: ManifestCase::Generated(unicode),
        files: vec![
            ("audio/01-unicode.bin", b"unicode"),
            ("artwork/back.bin", b"back"),
        ],
    });

    // credit-anchors
    let anchors = Py::Obj(vec![
        ("format", Py::s("musicpack")),
        ("version", Py::Int(1)),
        (
            "album",
            Py::Obj(vec![
                ("title", Py::s("Conformance")),
                (
                    "artists",
                    Py::Arr(vec![
                        Py::Obj(vec![
                            ("name", Py::s("Alpha")),
                            ("role", Py::s("main")),
                            ("sortName", Py::s("Alpha")),
                            (
                                "musicbrainzId",
                                Py::s("5441c29d-3602-4898-b1a1-b77fa23b8e50"),
                            ),
                        ]),
                        Py::Obj(vec![("name", Py::s("Beta"))]),
                    ]),
                ),
            ]),
        ),
        (
            "media",
            Py::Arr(vec![Py::Obj(vec![
                ("disc", Py::Int(1)),
                (
                    "tracks",
                    Py::Arr(vec![Py::Obj(vec![
                        ("track", Py::Int(1)),
                        ("title", Py::s("One")),
                        // Base audio first; the artists assignment is
                        // appended after it by the generator.
                        (
                            "audio",
                            Py::Obj(vec![
                                ("path", Py::s("audio/01.bin")),
                                ("sha256", Py::s(hash(b"one"))),
                            ]),
                        ),
                        (
                            "artists",
                            Py::Arr(vec![Py::Obj(vec![
                                ("name", Py::s("Gamma")),
                                (
                                    "musicbrainzId",
                                    Py::s("70b2a40e-8f4d-4c6b-b6ce-8f1e0a6dc3ba"),
                                ),
                            ])]),
                        ),
                    ])]),
                ),
            ])]),
        ),
    ]);
    cases.push(Case {
        name: "credit-anchors",
        group: Group::Valid,
        manifest: ManifestCase::Generated(anchors),
        files: vec![one_file],
    });

    // with-waveform
    let wfm20 = &[0u8; 20];
    let with_waveform = Py::Obj(vec![
        ("format", Py::s("musicpack")),
        ("version", Py::Int(1)),
        (
            "album",
            Py::Obj(vec![
                ("title", Py::s("Conformance")),
                (
                    "artists",
                    Py::Arr(vec![Py::Obj(vec![("name", Py::s("Tester"))])]),
                ),
            ]),
        ),
        (
            "media",
            Py::Arr(vec![Py::Obj(vec![
                ("disc", Py::Int(1)),
                (
                    "tracks",
                    Py::Arr(vec![Py::Obj(vec![
                        ("track", Py::Int(1)),
                        ("title", Py::s("One")),
                        (
                            "audio",
                            Py::Obj(vec![
                                ("path", Py::s("audio/01.bin")),
                                ("sha256", Py::s(hash(b"one"))),
                            ]),
                        ),
                        (
                            "waveform",
                            Py::Obj(vec![
                                ("version", Py::Int(1)),
                                ("path", Py::s("analysis/waveform/01-01.wfm")),
                                ("sha256", Py::s(hash(wfm20))),
                                ("intervalMs", Py::Int(100)),
                                ("encoding", Py::s("peak-rms-u8")),
                                ("floorDb", Py::Int(-60)),
                                ("points", Py::Int(10)),
                            ]),
                        ),
                    ])]),
                ),
            ])]),
        ),
    ]);
    cases.push(Case {
        name: "with-waveform",
        group: Group::Valid,
        manifest: ManifestCase::Generated(with_waveform),
        files: vec![one_file, ("analysis/waveform/01-01.wfm", wfm20)],
    });

    // with-representations
    let reps = Py::Obj(vec![
        ("format", Py::s("musicpack")),
        ("version", Py::Int(1)),
        (
            "album",
            Py::Obj(vec![
                ("title", Py::s("Conformance")),
                (
                    "artists",
                    Py::Arr(vec![Py::Obj(vec![("name", Py::s("Tester"))])]),
                ),
            ]),
        ),
        (
            "media",
            Py::Arr(vec![Py::Obj(vec![
                ("disc", Py::Int(1)),
                (
                    "tracks",
                    Py::Arr(vec![Py::Obj(vec![
                        ("track", Py::Int(1)),
                        ("title", Py::s("One")),
                        (
                            "audio",
                            Py::Obj(vec![
                                ("path", Py::s("audio/01.bin")),
                                ("sha256", Py::s(hash(b"one"))),
                            ]),
                        ),
                        (
                            "representations",
                            Py::Arr(vec![Py::Obj(vec![
                                ("path", Py::s("audio/01-alt.bin")),
                                ("sha256", Py::s(hash(b"alt"))),
                                ("label", Py::s("FLAC 24/96")),
                                ("codec", Py::s("flac")),
                            ])]),
                        ),
                    ])]),
                ),
            ])]),
        ),
    ]);
    cases.push(Case {
        name: "with-representations",
        group: Group::Valid,
        manifest: ManifestCase::Generated(reps),
        files: vec![one_file, ("audio/01-alt.bin", b"alt")],
    });

    // ---------------- invalid_manifest ----------------
    for (name, representations) in [
        (
            "rep-missing-sha",
            vec![Py::Obj(vec![("path", Py::s("audio/01-alt.bin"))])],
        ),
        (
            "rep-dup-path-primary",
            vec![Py::Obj(vec![
                ("path", Py::s("audio/01.bin")),
                ("sha256", Py::s(hash(b"one"))),
            ])],
        ),
        (
            "rep-dup-path-self",
            vec![
                Py::Obj(vec![
                    ("path", Py::s("audio/01-alt.bin")),
                    ("sha256", Py::s(hash(b"alt"))),
                ]),
                Py::Obj(vec![
                    ("path", Py::s("audio/01-alt.bin")),
                    ("sha256", Py::s(hash(b"two"))),
                ]),
            ],
        ),
        (
            "rep-traversal",
            vec![Py::Obj(vec![
                ("path", Py::s("../evil.bin")),
                ("sha256", Py::s(hash(b"x"))),
            ])],
        ),
    ] {
        cases.push(Case {
            name,
            group: Group::InvalidManifest,
            manifest: ManifestCase::Generated(with_representations(representations)),
            files: vec![one_file],
        });
    }

    // malformed-json: a bare "{".
    cases.push(Case {
        name: "malformed-json",
        group: Group::InvalidManifest,
        manifest: ManifestCase::Raw(b"{".to_vec()),
        files: vec![],
    });

    // Simple semantic mutations of the base manifest.
    for (name, mutate) in mutation_cases() {
        cases.push(Case {
            name,
            group: Group::InvalidManifest,
            manifest: ManifestCase::Generated(mutate),
            files: vec![one_file],
        });
    }

    // unsafe-path-0..7
    for (i, path) in [
        "../x", "/tmp/x", "a\\b", "a//b", "a/./b", "a/../b", "", "audio/",
    ]
    .iter()
    .enumerate()
    {
        let name: &'static str = Box::leak(format!("unsafe-path-{i}").into_boxed_str());
        cases.push(Case {
            name,
            group: Group::InvalidManifest,
            manifest: ManifestCase::Generated(base_manifest(path, &hash(b"one"))),
            files: vec![one_file],
        });
    }

    // waveform mutations
    let wfm20 = &[0u8; 20];
    for (name, waveform) in [
        (
            "waveform-bad-version",
            Py::Obj(vec![
                ("version", Py::Int(2)),
                ("path", Py::s("analysis/waveform/01-01.wfm")),
                ("sha256", Py::s("a".repeat(64))),
                ("intervalMs", Py::Int(100)),
                ("encoding", Py::s("peak-rms-u8")),
                ("floorDb", Py::Int(-60)),
                ("points", Py::Int(10)),
            ]),
        ),
        (
            "waveform-bad-encoding",
            Py::Obj(vec![
                ("version", Py::Int(1)),
                ("path", Py::s("analysis/waveform/01-01.wfm")),
                ("sha256", Py::s("a".repeat(64))),
                ("intervalMs", Py::Int(100)),
                ("encoding", Py::s("binary-f32le")),
                ("floorDb", Py::Int(-60)),
                ("points", Py::Int(10)),
            ]),
        ),
        (
            "waveform-bad-interval",
            Py::Obj(vec![
                ("version", Py::Int(1)),
                ("path", Py::s("analysis/waveform/01-01.wfm")),
                ("sha256", Py::s("a".repeat(64))),
                ("intervalMs", Py::Int(50)),
                ("encoding", Py::s("peak-rms-u8")),
                ("floorDb", Py::Int(-60)),
                ("points", Py::Int(10)),
            ]),
        ),
        (
            "waveform-bad-floor",
            Py::Obj(vec![
                ("version", Py::Int(1)),
                ("path", Py::s("analysis/waveform/01-01.wfm")),
                ("sha256", Py::s("a".repeat(64))),
                ("intervalMs", Py::Int(100)),
                ("encoding", Py::s("peak-rms-u8")),
                ("floorDb", Py::Int(-30)),
                ("points", Py::Int(10)),
            ]),
        ),
        (
            "waveform-too-many-points",
            Py::Obj(vec![
                ("version", Py::Int(1)),
                ("path", Py::s("analysis/waveform/01-01.wfm")),
                ("sha256", Py::s("a".repeat(64))),
                ("intervalMs", Py::Int(100)),
                ("encoding", Py::s("peak-rms-u8")),
                ("floorDb", Py::Int(-60)),
                ("points", Py::Int(900000)),
            ]),
        ),
        (
            "waveform-traversal",
            Py::Obj(vec![
                ("version", Py::Int(1)),
                ("path", Py::s("../evil.wfm")),
                ("sha256", Py::s("a".repeat(64))),
                ("intervalMs", Py::Int(100)),
                ("encoding", Py::s("peak-rms-u8")),
                ("floorDb", Py::Int(-60)),
                ("points", Py::Int(10)),
            ]),
        ),
        (
            "waveform-points-mismatch",
            Py::Obj(vec![
                ("version", Py::Int(1)),
                ("path", Py::s("analysis/waveform/01-01.wfm")),
                ("sha256", Py::s("a".repeat(64))),
                ("intervalMs", Py::Int(100)),
                ("encoding", Py::s("peak-rms-u8")),
                ("floorDb", Py::Int(-60)),
                ("points", Py::Int(999)),
            ]),
        ),
    ] {
        cases.push(Case {
            name,
            group: Group::InvalidManifest,
            manifest: ManifestCase::Generated(with_waveform_ref(waveform)),
            files: vec![one_file],
        });
    }

    // missing-*-checksum
    for asset_key in ["artwork", "booklet", "lyrics", "extras", "analysis"] {
        let entry: Py = match asset_key {
            "artwork" => Py::Obj(vec![
                ("role", Py::s("front")),
                ("path", Py::s("artwork/a.bin")),
            ]),
            "analysis" => Py::Obj(vec![
                ("type", Py::s("future")),
                ("path", Py::s("analysis/a.bin")),
            ]),
            other => Py::Obj(vec![(
                "path",
                Py::s(format!("{other}/a.bin").leak() as &str),
            )]),
        };
        let key: &'static str = asset_key;
        cases.push(Case {
            name: Box::leak(format!("missing-{key}-checksum").into_boxed_str()),
            group: Group::InvalidManifest,
            manifest: ManifestCase::Generated(with_asset_array(key, entry)),
            files: vec![one_file],
        });
    }

    // Raw-byte duplicate/trailing cases (byte patterns from the generator).
    let raw = dumps(&base_manifest("audio/01.bin", &hash(b"one")));
    let raw = raw.into_bytes();
    let raw_cases: Vec<(&'static str, Vec<u8>)> = {
        fn replace_n(haystack: &[u8], from: &str, to: &str, max: usize) -> Vec<u8> {
            let from = from.as_bytes();
            let to = to.as_bytes();
            let mut out = Vec::with_capacity(haystack.len());
            let mut i = 0;
            let mut replaced = 0;
            while i < haystack.len() {
                if replaced < max && haystack[i..].starts_with(from) {
                    out.extend_from_slice(to);
                    i += from.len();
                    replaced += 1;
                } else {
                    out.push(haystack[i]);
                    i += 1;
                }
            }
            out
        }
        // Python str.replace(a, b, count): duplicate-sha replaces the
        // FIRST occurrence only; the others replace all (they occur once).
        vec![
            (
                "trailing-json",
                [raw.as_slice(), b" trailing".as_slice()].concat(),
            ),
            (
                "nul-suffix",
                [raw.as_slice(), b"\0suffix".as_slice()].concat(),
            ),
            (
                "duplicate-format",
                replace_n(
                    &raw,
                    "\"format\": \"musicpack\",",
                    "\"format\":\"musicpack\",\"format\":\"musicpack\",",
                    usize::MAX,
                ),
            ),
            (
                "duplicate-version",
                replace_n(
                    &raw,
                    "\"version\": 1,",
                    "\"version\":1,\"version\":1,",
                    usize::MAX,
                ),
            ),
            (
                "duplicate-path",
                replace_n(
                    &raw,
                    "\"path\": \"audio/01.bin\",",
                    "\"path\":\"audio/01.bin\",\"path\":\"audio/01.bin\",",
                    usize::MAX,
                ),
            ),
            (
                "duplicate-sha",
                replace_n(
                    &raw,
                    "\"sha256\":",
                    &format!("\"sha256\":\"{}\",\"sha256\":", "a".repeat(64)),
                    1,
                ),
            ),
            (
                "duplicate-credit-mbid",
                replace_n(
                    &raw,
                    "\"artists\": [{\"name\": \"Tester\"}]",
                    "\"artists\": [{\"name\": \"Tester\", \"musicbrainzId\": \"one\", \"musicbrainzId\": \"two\"}]",
                    usize::MAX,
                ),
            ),
        ]
    };
    for (name, content) in raw_cases {
        cases.push(Case {
            name,
            group: Group::InvalidManifest,
            manifest: ManifestCase::Raw(content),
            files: vec![one_file],
        });
    }

    // ---------------- invalid_verify ----------------
    cases.push(Case {
        name: "missing-asset",
        group: Group::InvalidVerify,
        manifest: ManifestCase::Generated(base_manifest("audio/01.bin", &hash(b"one"))),
        files: vec![],
    });
    cases.push(Case {
        name: "checksum-mismatch",
        group: Group::InvalidVerify,
        manifest: ManifestCase::Generated(base_manifest("audio/01.bin", &hash(b"one"))),
        files: vec![("audio/01.bin", b"changed")],
    });
    for (asset_key, path, data) in [
        ("artwork", "artwork/a.bin", &b"art"[..]),
        ("booklet", "booklet/a.bin", b"book"),
        ("lyrics", "lyrics/a.bin", b"lyric"),
        ("extras", "extras/a.bin", b"extra"),
        ("analysis", "analysis/a.bin", b"analysis"),
    ] {
        let mut entry = vec![("path", Py::s(path)), ("sha256", Py::s(hash(data)))];
        // Python insertion order: role/type are added after the dict is
        // created, so they print last.
        if asset_key == "artwork" {
            entry.push(("role", Py::s("front")));
        }
        if asset_key == "analysis" {
            entry.push(("type", Py::s("future")));
        }
        let manifest = base_manifest_with_extra_asset(asset_key, Py::Obj(entry));
        cases.push(Case {
            name: Box::leak(format!("{asset_key}-checksum-mismatch").into_boxed_str()),
            group: Group::InvalidVerify,
            manifest: ManifestCase::Generated(manifest),
            files: vec![("audio/01.bin", b"one"), (path, b"changed")],
        });
    }
    // symlink-escape is created by the POSIX-only part of the generator;
    // handled by the caller (needs symlink()).
    // waveform-checksum-mismatch
    let waveform_bad = Py::Obj(vec![
        ("format", Py::s("musicpack")),
        ("version", Py::Int(1)),
        (
            "album",
            Py::Obj(vec![
                ("title", Py::s("Conformance")),
                (
                    "artists",
                    Py::Arr(vec![Py::Obj(vec![("name", Py::s("Tester"))])]),
                ),
            ]),
        ),
        (
            "media",
            Py::Arr(vec![Py::Obj(vec![
                ("disc", Py::Int(1)),
                (
                    "tracks",
                    Py::Arr(vec![Py::Obj(vec![
                        ("track", Py::Int(1)),
                        ("title", Py::s("One")),
                        (
                            "audio",
                            Py::Obj(vec![
                                ("path", Py::s("audio/01.bin")),
                                ("sha256", Py::s(hash(b"one"))),
                            ]),
                        ),
                        (
                            "waveform",
                            Py::Obj(vec![
                                ("version", Py::Int(1)),
                                ("path", Py::s("analysis/waveform/01-01.wfm")),
                                ("sha256", Py::s("0".repeat(64))),
                                ("intervalMs", Py::Int(100)),
                                ("encoding", Py::s("peak-rms-u8")),
                                ("floorDb", Py::Int(-60)),
                                ("points", Py::Int(10)),
                            ]),
                        ),
                    ])]),
                ),
            ])]),
        ),
    ]);
    cases.push(Case {
        name: "waveform-checksum-mismatch",
        group: Group::InvalidVerify,
        manifest: ManifestCase::Generated(waveform_bad),
        files: vec![one_file, ("analysis/waveform/01-01.wfm", wfm20)],
    });

    cases
}

// --- helpers mirroring the generator's dict mutations ---

fn minimal_album() -> Py {
    Py::Obj(vec![
        ("title", Py::s("Conformance")),
        (
            "artists",
            Py::Arr(vec![Py::Obj(vec![("name", Py::s("Tester"))])]),
        ),
    ])
}

fn base_with_track_fields(track_fields: Vec<(&'static str, Py)>) -> Py {
    // The generator mutates the *base* manifest whose track already
    // carries the audio object; dict.update() appends new keys after it,
    // except that an updated "audio" replaces the base audio entirely.
    let replaces_audio = track_fields.iter().any(|(k, _)| *k == "audio");
    Py::Obj(vec![
        ("format", Py::s("musicpack")),
        ("version", Py::Int(1)),
        ("album", minimal_album()),
        (
            "media",
            Py::Arr(vec![Py::Obj(vec![
                ("disc", Py::Int(1)),
                (
                    "tracks",
                    Py::Arr(vec![Py::Obj({
                        let mut fields: Vec<(&'static str, Py)> =
                            vec![("track", Py::Int(1)), ("title", Py::s("One"))];
                        if !replaces_audio {
                            fields.push((
                                "audio",
                                Py::Obj(vec![
                                    ("path", Py::s("audio/01.bin")),
                                    ("sha256", Py::s(hash(b"one"))),
                                ]),
                            ));
                        }
                        fields.extend(track_fields);
                        fields
                    })]),
                ),
            ])]),
        ),
    ])
}

/// `track.representations` mutation (valid path+sha + extra keys).
fn with_representations(reps: Vec<Py>) -> Py {
    base_with_track_fields(vec![("representations", Py::Arr(reps))])
}

/// `track.waveform` mutation.
fn with_waveform_ref(waveform: Py) -> Py {
    base_with_track_fields(vec![("waveform", waveform)])
}

/// Root-level asset-array mutations (missing checksums).
fn with_asset_array(key: &str, entry: Py) -> Py {
    Py::Obj(vec![
        ("format", Py::s("musicpack")),
        ("version", Py::Int(1)),
        ("album", minimal_album()),
        ("media", base_media_full()),
        (
            Box::leak(key.to_string().into_boxed_str()),
            Py::Arr(vec![entry]),
        ),
    ])
}

/// invalid_verify asset-checksum cases: base manifest + one extra asset
/// entry whose declared hash matches the ORIGINAL bytes.
fn base_manifest_with_extra_asset(key: &str, entry: Py) -> Py {
    Py::Obj(vec![
        ("format", Py::s("musicpack")),
        ("version", Py::Int(1)),
        ("album", minimal_album()),
        ("media", base_media_full()),
        (
            Box::leak(key.to_string().into_boxed_str()),
            Py::Arr(vec![entry]),
        ),
    ])
}

/// The base manifest's media array (one disc, one track with audio) —
/// the shape every `mutations` case operates on.
fn base_media_full() -> Py {
    Py::Arr(vec![Py::Obj(vec![
        ("disc", Py::Int(1)),
        (
            "tracks",
            Py::Arr(vec![Py::Obj(vec![
                ("track", Py::Int(1)),
                ("title", Py::s("One")),
                (
                    "audio",
                    Py::Obj(vec![
                        ("path", Py::s("audio/01.bin")),
                        ("sha256", Py::s(hash(b"one"))),
                    ]),
                ),
            ])]),
        ),
    ])])
}

/// The 25 simple semantic mutations (`mutations` dict in the generator).
#[allow(clippy::vec_init_then_push)] // a linear port of the Python dict
fn mutation_cases() -> Vec<(&'static str, Py)> {
    let h = |d: &[u8]| hash(d);
    // The 25 semantic mutations of the base manifest, in generator order.
    let mut cases: Vec<(&'static str, Py)> = vec![];

    // wrong-format
    cases.push((
        "wrong-format",
        Py::Obj(vec![
            ("format", Py::s("other")),
            ("version", Py::Int(1)),
            ("album", minimal_album()),
            ("media", base_media_full()),
        ]),
    ));
    // unsupported-version
    cases.push((
        "unsupported-version",
        Py::Obj(vec![
            ("format", Py::s("musicpack")),
            ("version", Py::Int(2)),
            ("album", minimal_album()),
            ("media", base_media_full()),
        ]),
    ));
    // missing-album
    cases.push((
        "missing-album",
        Py::Obj(vec![
            ("format", Py::s("musicpack")),
            ("version", Py::Int(1)),
            ("media", base_media_full()),
        ]),
    ));
    // empty-artists
    cases.push((
        "empty-artists",
        Py::Obj(vec![
            ("format", Py::s("musicpack")),
            ("version", Py::Int(1)),
            (
                "album",
                Py::Obj(vec![
                    ("title", Py::s("Conformance")),
                    ("artists", Py::Arr(vec![])),
                ]),
            ),
            ("media", base_media_full()),
        ]),
    ));
    // bad-release-type
    cases.push((
        "bad-release-type",
        Py::Obj(vec![
            ("format", Py::s("musicpack")),
            ("version", Py::Int(1)),
            (
                "album",
                Py::Obj(vec![
                    ("title", Py::s("Conformance")),
                    (
                        "artists",
                        Py::Arr(vec![Py::Obj(vec![("name", Py::s("Tester"))])]),
                    ),
                    ("releaseType", Py::s("mixtape")),
                ]),
            ),
            ("media", base_media_full()),
        ]),
    ));
    // bad-medium-format (update() appends "format" after "tracks")
    cases.push((
        "bad-medium-format",
        Py::Obj(vec![
            ("format", Py::s("musicpack")),
            ("version", Py::Int(1)),
            ("album", minimal_album()),
            (
                "media",
                Py::Arr(vec![Py::Obj(vec![
                    ("disc", Py::Int(1)),
                    (
                        "tracks",
                        Py::Arr(vec![Py::Obj(vec![
                            ("track", Py::Int(1)),
                            ("title", Py::s("One")),
                            (
                                "audio",
                                Py::Obj(vec![
                                    ("path", Py::s("audio/01.bin")),
                                    ("sha256", Py::s(h(b"one"))),
                                ]),
                            ),
                        ])]),
                    ),
                    ("format", Py::s("DAT")),
                ])]),
            ),
        ]),
    ));
    // duplicate-disc: media[0] appended twice.
    {
        let disc = Py::Obj(vec![
            ("disc", Py::Int(1)),
            (
                "tracks",
                Py::Arr(vec![Py::Obj(vec![
                    ("track", Py::Int(1)),
                    ("title", Py::s("One")),
                    (
                        "audio",
                        Py::Obj(vec![
                            ("path", Py::s("audio/01.bin")),
                            ("sha256", Py::s(h(b"one"))),
                        ]),
                    ),
                ])]),
            ),
        ]);
        cases.push((
            "duplicate-disc",
            Py::Obj(vec![
                ("format", Py::s("musicpack")),
                ("version", Py::Int(1)),
                ("album", minimal_album()),
                ("media", Py::Arr(vec![disc.clone(), disc])),
            ]),
        ));
    }
    // duplicate-track: tracks[0] appended twice.
    {
        let track = Py::Obj(vec![
            ("track", Py::Int(1)),
            ("title", Py::s("One")),
            (
                "audio",
                Py::Obj(vec![
                    ("path", Py::s("audio/01.bin")),
                    ("sha256", Py::s(h(b"one"))),
                ]),
            ),
        ]);
        cases.push((
            "duplicate-track",
            Py::Obj(vec![
                ("format", Py::s("musicpack")),
                ("version", Py::Int(1)),
                ("album", minimal_album()),
                (
                    "media",
                    Py::Arr(vec![Py::Obj(vec![
                        ("disc", Py::Int(1)),
                        ("tracks", Py::Arr(vec![track.clone(), track])),
                    ])]),
                ),
            ]),
        ));
    }
    // bad-checksum-form
    cases.push((
        "bad-checksum-form",
        base_with_track_fields(vec![(
            "audio",
            Py::Obj(vec![
                ("path", Py::s("audio/01.bin")),
                ("sha256", Py::s("A".repeat(64))),
            ]),
        )]),
    ));
    // missing-audio-checksum
    cases.push((
        "missing-audio-checksum",
        base_with_track_fields(vec![(
            "audio",
            Py::Obj(vec![("path", Py::s("audio/01.bin"))]),
        )]),
    ));
    // bad-duration
    cases.push((
        "bad-duration",
        base_with_track_fields(vec![("duration", Py::Int(0))]),
    ));
    // bad-loudness
    cases.push((
        "bad-loudness",
        base_with_track_fields(vec![(
            "loudness",
            Py::Obj(vec![
                ("trackLUFS", Py::Int(-9999)),
                ("truePeakDbTP", Py::Int(0)),
            ]),
        )]),
    ));
    // sonic-without-profile
    cases.push((
        "sonic-without-profile",
        Py::Obj(vec![
            ("format", Py::s("musicpack")),
            ("version", Py::Int(1)),
            ("album", minimal_album()),
            ("media", base_media_full()),
            (
                "analysis",
                Py::Arr(vec![Py::Obj(vec![
                    ("type", Py::s("sonic")),
                    ("path", Py::s("analysis/a")),
                    ("sha256", Py::s(h(b"a"))),
                ])]),
            ),
        ]),
    ));
    // duplicate-asset-path
    cases.push((
        "duplicate-asset-path",
        Py::Obj(vec![
            ("format", Py::s("musicpack")),
            ("version", Py::Int(1)),
            ("album", minimal_album()),
            ("media", base_media_full()),
            (
                "extras",
                Py::Arr(vec![Py::Obj(vec![
                    ("path", Py::s("audio/01.bin")),
                    ("sha256", Py::s(h(b"one"))),
                ])]),
            ),
        ]),
    ));
    // wrong-track-object
    cases.push((
        "wrong-track-object",
        Py::Obj(vec![
            ("format", Py::s("musicpack")),
            ("version", Py::Int(1)),
            ("album", minimal_album()),
            (
                "media",
                Py::Arr(vec![Py::Obj(vec![
                    ("disc", Py::Int(1)),
                    ("tracks", Py::Arr(vec![Py::Int(42)])),
                ])]),
            ),
        ]),
    ));
    // wrong-media-object
    cases.push((
        "wrong-media-object",
        Py::Obj(vec![
            ("format", Py::s("musicpack")),
            ("version", Py::Int(1)),
            ("album", minimal_album()),
            ("media", Py::Arr(vec![Py::Int(42)])),
        ]),
    ));
    // wrong-audio-object
    cases.push((
        "wrong-audio-object",
        base_with_track_fields(vec![("audio", Py::Int(42))]),
    ));
    // wrong-artists-object
    cases.push((
        "wrong-artists-object",
        Py::Obj(vec![
            ("format", Py::s("musicpack")),
            ("version", Py::Int(1)),
            (
                "album",
                Py::Obj(vec![
                    ("title", Py::s("Conformance")),
                    ("artists", Py::Obj(vec![])),
                ]),
            ),
            ("media", base_media_full()),
        ]),
    ));
    // bad-credit-mbid-type
    cases.push((
        "bad-credit-mbid-type",
        Py::Obj(vec![
            ("format", Py::s("musicpack")),
            ("version", Py::Int(1)),
            (
                "album",
                Py::Obj(vec![
                    ("title", Py::s("Conformance")),
                    (
                        "artists",
                        Py::Arr(vec![Py::Obj(vec![
                            ("name", Py::s("Tester")),
                            ("musicbrainzId", Py::Int(123)),
                        ])]),
                    ),
                ]),
            ),
            ("media", base_media_full()),
        ]),
    ));
    // bad-credit-sortname-type
    cases.push((
        "bad-credit-sortname-type",
        Py::Obj(vec![
            ("format", Py::s("musicpack")),
            ("version", Py::Int(1)),
            (
                "album",
                Py::Obj(vec![
                    ("title", Py::s("Conformance")),
                    (
                        "artists",
                        Py::Arr(vec![Py::Obj(vec![
                            ("name", Py::s("Tester")),
                            ("sortName", Py::Int(5)),
                        ])]),
                    ),
                ]),
            ),
            ("media", base_media_full()),
        ]),
    ));
    // bad-track-credit-mbid-type
    cases.push((
        "bad-track-credit-mbid-type",
        base_with_track_fields(vec![(
            "artists",
            Py::Arr(vec![Py::Obj(vec![
                ("name", Py::s("X")),
                ("musicbrainzId", Py::Int(7)),
            ])]),
        )]),
    ));
    // bad-identity-enum
    cases.push((
        "bad-identity-enum",
        Py::Obj(vec![
            ("format", Py::s("musicpack")),
            ("version", Py::Int(1)),
            ("album", minimal_album()),
            ("media", base_media_full()),
            ("identity", Py::Obj(vec![("source", Py::s("bogus"))])),
        ]),
    ));
    // partial-album-loudness
    cases.push((
        "partial-album-loudness",
        Py::Obj(vec![
            ("format", Py::s("musicpack")),
            ("version", Py::Int(1)),
            ("album", minimal_album()),
            ("media", base_media_full()),
            ("loudness", Py::Obj(vec![("albumLUFS", Py::Int(-12))])),
        ]),
    ));
    // fractional-track
    cases.push((
        "fractional-track",
        Py::Obj(vec![
            ("format", Py::s("musicpack")),
            ("version", Py::Int(1)),
            ("album", minimal_album()),
            (
                "media",
                Py::Arr(vec![Py::Obj(vec![
                    ("disc", Py::Int(1)),
                    (
                        "tracks",
                        Py::Arr(vec![Py::Obj(vec![
                            ("track", Py::Float(1.5)),
                            ("title", Py::s("One")),
                            (
                                "audio",
                                Py::Obj(vec![
                                    ("path", Py::s("audio/01.bin")),
                                    ("sha256", Py::s(h(b"one"))),
                                ]),
                            ),
                        ])]),
                    ),
                ])]),
            ),
        ]),
    ));
    // oversized-disc
    cases.push((
        "oversized-disc",
        Py::Obj(vec![
            ("format", Py::s("musicpack")),
            ("version", Py::Int(1)),
            ("album", minimal_album()),
            (
                "media",
                Py::Arr(vec![Py::Obj(vec![
                    ("disc", Py::Int(2147483648)),
                    (
                        "tracks",
                        Py::Arr(vec![Py::Obj(vec![
                            ("track", Py::Int(1)),
                            ("title", Py::s("One")),
                            (
                                "audio",
                                Py::Obj(vec![
                                    ("path", Py::s("audio/01.bin")),
                                    ("sha256", Py::s(h(b"one"))),
                                ]),
                            ),
                        ])]),
                    ),
                ])]),
            ),
        ]),
    ));

    cases
}

// ---------------------------------------------------------------------
// Corpus materialisation + reference-repo discovery
// ---------------------------------------------------------------------

/// Writes the corpus into `root` (creating `<name>.mpack` directories),
/// mirroring the generator's output byte-for-byte. An existing `root` is
/// removed first (the generator does the same).
pub fn write_corpus(root: &Path) -> std::io::Result<Vec<Case>> {
    let _ = std::fs::remove_dir_all(root);
    std::fs::create_dir_all(root)?;
    #[cfg(unix)]
    let mut cases = corpus();
    #[cfg(not(unix))]
    let cases = corpus();
    for case in &cases {
        let dir = root.join(format!("{}.mpack", case.name));
        std::fs::create_dir_all(&dir)?;
        for (path, data) in &case.files {
            let full = dir.join(path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut f = std::fs::File::create(full)?;
            f.write_all(data)?;
        }
        match &case.manifest {
            ManifestCase::Generated(v) => {
                std::fs::write(dir.join("manifest.json"), dump_manifest(v).as_bytes())?;
            }
            ManifestCase::Raw(bytes) => {
                std::fs::write(dir.join("manifest.json"), bytes)?;
            }
        }
    }

    // POSIX-only: the symlink-escape case (the generator skips it on
    // Windows; so do we).
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let outside = root.join("outside.bin");
        std::fs::write(&outside, b"one")?;
        let pkg = root.join("symlink-escape.mpack");
        std::fs::create_dir_all(pkg.join("audio"))?;
        std::fs::write(
            pkg.join("manifest.json"),
            dump_manifest(&base_manifest("audio/01.bin", &hash(b"one"))).as_bytes(),
        )?;
        symlink(&outside, pkg.join("audio/01.bin"))?;
        cases.push(Case {
            name: "symlink-escape",
            group: Group::InvalidVerify,
            manifest: ManifestCase::Raw(Vec::new()), // manifest never read for this case
            files: vec![],
        });
    }

    Ok(cases)
}

/// Locates the reference MusicPack repository, if it is available.
///
/// Resolution order: `$MUSICPACK_REFERENCE_DIR`, else the sibling
/// `../musicpack` relative to this crate. Returns `None` when neither
/// exists — differential layers then skip with a notice instead of
/// failing.
pub fn reference_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("MUSICPACK_REFERENCE_DIR") {
        let p = PathBuf::from(dir);
        if p.is_dir() {
            return Some(p);
        }
        return None;
    }
    let sibling = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../musicpack");
    if sibling.is_dir() {
        Some(sibling)
    } else {
        None
    }
}

/// Path of the reference corpus generator inside the reference checkout.
pub fn reference_generator() -> Option<PathBuf> {
    reference_dir().map(|d| d.join("tests/generate_mpack_conformance.py"))
}

/// Runs `python3 <generator> <out>`; `None` when Python is unavailable.
pub fn run_reference_generator(out: &Path) -> Option<std::io::Result<()>> {
    let generator = reference_generator()?;
    let status = Command::new("python3")
        .arg(generator)
        .arg(out)
        .status()
        .ok()?;
    Some(if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("reference generator failed"))
    })
}

/// Path of the reference `musicpack` CLI, if one has been built.
///
/// (Used by `conformance_differential.rs`; `mod support` is compiled per
/// test binary, so unused-in-this-binary warnings are expected and silenced.)
///
/// Resolution order: `$MUSICPACK_REF_CLI`, else
/// `<reference>/build/core/musicpack/musicpack`.
#[allow(dead_code)]
pub fn reference_cli() -> Option<PathBuf> {
    if let Ok(cli) = std::env::var("MUSICPACK_REF_CLI") {
        let p = PathBuf::from(cli);
        if p.is_file() {
            return Some(p);
        }
        return None;
    }
    let cli = reference_dir()?.join("build/core/musicpack/musicpack");
    if cli.is_file() { Some(cli) } else { None }
}

// ---------------------------------------------------------------------
// MPAK test builders (shared by the container and hostile suites)
// ---------------------------------------------------------------------

use std::io::Read;

use musicpack_core::Error;
use musicpack_core::format::checksum;
use musicpack_core::format::mpak::{
    self, BLOCK_HEADER_LEN, HEADER_LEN, MemorySource, PackMember, PackSource, write_mpak,
};

pub fn container_header() -> Vec<u8> {
    let mut h = vec![0u8; HEADER_LEN];
    h[0..4].copy_from_slice(b"MPAK");
    h[4] = 1;
    h[5] = 0;
    h[6..8].copy_from_slice(&1u16.to_be_bytes()); // INDX_PRESENT
    h
}

pub fn frame(block_type: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let length = payload.len() as u64;
    let mut hdr = [0u8; BLOCK_HEADER_LEN];
    hdr[0..4].copy_from_slice(block_type);
    hdr[4..12].copy_from_slice(&length.to_be_bytes());
    let crc = mpak::crc16_buypass(&hdr[0..12]);
    hdr[12..14].copy_from_slice(&crc.to_be_bytes());
    out.extend_from_slice(&hdr);
    out.extend_from_slice(payload);
    out
}

/// A block with an arbitrary declared length and (optionally) a broken CRC.
pub fn raw_frame(
    block_type: &[u8; 4],
    declared_len: u64,
    payload: &[u8],
    break_crc: bool,
) -> Vec<u8> {
    let mut out = Vec::new();
    let mut hdr = [0u8; BLOCK_HEADER_LEN];
    hdr[0..4].copy_from_slice(block_type);
    hdr[4..12].copy_from_slice(&declared_len.to_be_bytes());
    let mut crc = mpak::crc16_buypass(&hdr[0..12]);
    if break_crc {
        crc ^= 0xFFFF;
    }
    hdr[12..14].copy_from_slice(&crc.to_be_bytes());
    out.extend_from_slice(&hdr);
    out.extend_from_slice(payload);
    out
}

pub fn data_payload(path: &str, member: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&(path.len() as u16).to_be_bytes());
    payload.extend_from_slice(path.as_bytes());
    payload.extend_from_slice(member);
    payload
}

pub fn data_frame(path: &str, member: &[u8]) -> Vec<u8> {
    frame(b"DATA", &data_payload(path, member))
}

/// Big-endian u64 (the wire helper is crate-private).
pub fn be_u64(bytes: &[u8]) -> u64 {
    u64::from_be_bytes(bytes[..8].try_into().expect("8 bytes"))
}

/// A manifest referencing the given members: `audio/*` entries become
/// tracks, everything else becomes an extras entry.
pub fn manifest_for(entries: &[(&str, &[u8])]) -> String {
    let mut tracks = Vec::new();
    let mut extras = Vec::new();
    for (path, bytes) in entries {
        let sha = checksum::sha256_hex(bytes);
        if path.starts_with("audio/") {
            tracks.push(format!(
                r#"{{"track": {n}, "title": "t", "audio": {{"path": "{path}", "sha256": "{sha}"}}}}"#,
                n = tracks.len() + 1
            ));
        } else {
            extras.push(format!(r#"{{"path": "{path}", "sha256": "{sha}"}}"#));
        }
    }
    assert!(!tracks.is_empty(), "test manifests need audio");
    let mut json = format!(
        r#"{{"format":"musicpack","version":1,"album":{{"title":"T","artists":[{{"name":"A"}}]}},"media":[{{"disc":1,"tracks":[{}]}}]"#,
        tracks.join(",")
    );
    if !extras.is_empty() {
        json.push_str(&format!(r#","extras":[{}]"#, extras.join(",")));
    }
    json.push('}');
    json
}

/// A `PackSource` over in-memory members (declared digests computed from
/// the bytes, so the writer's copy-time verification succeeds).
pub struct TestSource {
    pub manifest: Vec<u8>,
    pub members: Vec<PackMember>,
    pub contents: Vec<(String, Vec<u8>)>,
}

impl TestSource {
    pub fn new(manifest: String, members: Vec<(&str, Vec<u8>)>) -> Self {
        let members = members
            .into_iter()
            .map(|(path, bytes)| PackMember {
                path: path.to_string(),
                sha256_hex: checksum::sha256_hex(&bytes),
            })
            .collect();
        Self {
            manifest: manifest.into_bytes(),
            members,
            contents: Vec::new(),
        }
    }

    pub fn with_contents(mut self, contents: Vec<(&str, Vec<u8>)>) -> Self {
        self.contents = contents
            .into_iter()
            .map(|(p, b)| (p.to_string(), b))
            .collect();
        self
    }
}

impl PackSource for TestSource {
    fn manifest_bytes(&self) -> &[u8] {
        &self.manifest
    }

    fn members(&self) -> &[PackMember] {
        &self.members
    }

    fn member_size(&self, path: &str) -> Result<u64, Error> {
        self.contents
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, b)| b.len() as u64)
            .ok_or_else(|| Error::Missing {
                path: path.to_string(),
            })
    }

    fn read_member(&self, path: &str) -> Result<Box<dyn Read + '_>, Error> {
        self.contents
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, b)| Box::new(std::io::Cursor::new(b.clone())) as Box<dyn Read>)
            .ok_or_else(|| Error::Missing {
                path: path.to_string(),
            })
    }
}

/// A valid container for the given members, built with the writer.
pub fn valid_container(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let manifest = manifest_for(entries);
    let members: Vec<(&str, Vec<u8>)> = entries.iter().map(|(p, b)| (*p, b.to_vec())).collect();
    let source = TestSource::new(manifest, members.clone()).with_contents(members);
    let mut out = Vec::new();
    write_mpak(&source, &mut out).expect("writes");
    out
}

pub fn scan(bytes: &[u8], recovery: bool) -> Result<mpak::MpakReader, Error> {
    mpak::scan(&MemorySource::new(bytes.to_vec()), recovery)
}

/// Bypasses `MpakBackend::open`'s manifest requirement for scan-only tests.
pub fn raw_scan(bytes: &[u8]) -> Result<mpak::MpakReader, Error> {
    scan(bytes, false)
}

#[cfg(test)]
mod support_tests {
    use super::*;

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"one"),
            "7692c3ad3540bb803c020b3aee66cd8887123234ea0c6e7143c0add73ff431ed"
        );
    }

    #[test]
    fn py_random_matches_cpython() {
        // Vectors generated with CPython:
        //   rng = random.Random(seed); [rng.randrange(4417), rng.randrange(8)] * 4
        let seed_1001 = [480, 3, 747, 6, 1312, 6, 3067, 6];
        let seed_1002 = [4267, 6, 1809, 2, 1067, 4, 3964, 2];
        let seed_1003 = [4074, 5, 1814, 7, 3659, 7, 3310, 0];
        for (seed, expected) in [(1001u32, seed_1001), (1002, seed_1002), (1003, seed_1003)] {
            let mut rng = PyRandom::new(seed);
            let mut actual = Vec::new();
            for _ in 0..4 {
                actual.push(rng.randrange(4417));
                actual.push(rng.randrange(8));
            }
            assert_eq!(actual, expected, "seed {seed}");
        }
    }

    #[test]
    fn compact_dump_matches_python() {
        let raw = dumps(&base_manifest("audio/01.bin", &hash(b"one")));
        assert_eq!(
            raw,
            format!(
                "{{\"format\": \"musicpack\", \"version\": 1, \"album\": {{\"title\": \"Conformance\", \"artists\": [{{\"name\": \"Tester\"}}]}}, \"media\": [{{\"disc\": 1, \"tracks\": [{{\"track\": 1, \"title\": \"One\", \"audio\": {{\"path\": \"audio/01.bin\", \"sha256\": \"{}\"}}}}]}}]}}",
                hash(b"one")
            )
        );
        assert!(raw.contains("\"artists\": [{\"name\": \"Tester\"}]"));
    }
}
