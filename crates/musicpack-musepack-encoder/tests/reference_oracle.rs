//! Opt-in live oracle: reproduce the frozen reference hashes with the C
//! reference encoder.
//!
//! The crate itself never depends on the C encoder. This test only runs when
//! the caller points it at a reference binary and a generated corpus, so the
//! normal `cargo test` suite stays hermetic and keeps working after the legacy
//! encoder is deleted:
//!
//! ```sh
//! # In the C reference repository:
//! python3 tests/generate_encoder_corpus.py /tmp/enc-corpus
//! # In this repository:
//! MUSICPACK_MPCENC=/path/to/mpcenc \
//! MUSICPACK_ENCODER_CORPUS=/tmp/enc-corpus \
//!   cargo test -p musicpack-musepack-encoder --test reference_oracle -- --nocapture
//! ```
//!
//! It runs the reference encoder over every manifest entry and requires the
//! whole-file SHA-256 to match the frozen manifest exactly. It never rewrites
//! the manifest and never tolerates a mismatch.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

/// Removes its directory when dropped, so a failing run leaves nothing behind.
struct TempDir(PathBuf);

impl TempDir {
    fn create() -> Self {
        let path = std::env::temp_dir().join(format!(
            "musicpack-musepack-encoder-oracle-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create oracle temp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn manifest_path() -> PathBuf {
    match std::env::var_os("MUSICPACK_ENCODER_MANIFEST") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/encoder_reference_manifest.txt"),
    }
}

/// Loads `<input> <quality> <sha256>` rows, grouped by input name.
fn load_manifest() -> BTreeMap<String, BTreeMap<u8, String>> {
    let path = manifest_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read manifest {}: {error}", path.display()));
    let mut vectors: BTreeMap<String, BTreeMap<u8, String>> = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let input = fields.next().expect("input name").to_owned();
        let quality: u8 = fields.next().expect("quality").parse().expect("quality");
        let hash = fields.next().expect("hash").to_owned();
        vectors.entry(input).or_default().insert(quality, hash);
    }
    vectors
}

#[test]
fn reference_encoder_reproduces_the_frozen_manifest() {
    let (Some(mpcenc), Some(corpus)) = (
        std::env::var_os("MUSICPACK_MPCENC"),
        std::env::var_os("MUSICPACK_ENCODER_CORPUS"),
    ) else {
        eprintln!(
            "skipping live reference oracle: set MUSICPACK_MPCENC and MUSICPACK_ENCODER_CORPUS"
        );
        return;
    };
    let mpcenc = PathBuf::from(mpcenc);
    let corpus = PathBuf::from(corpus);
    assert!(
        mpcenc.is_file(),
        "MUSICPACK_MPCENC is not a file: {}",
        mpcenc.display()
    );
    assert!(
        corpus.is_dir(),
        "MUSICPACK_ENCODER_CORPUS is not a directory: {}",
        corpus.display()
    );

    let manifest = load_manifest();
    let temp = TempDir::create();

    let mut checked = 0usize;
    let mut failures = Vec::new();
    for (input, by_quality) in &manifest {
        let wav = corpus.join(input);
        if !wav.is_file() {
            failures.push(format!(
                "{input}: input missing from corpus {}",
                corpus.display()
            ));
            continue;
        }
        for (quality, expected) in by_quality {
            let out = temp.path().join(format!("{input}.q{quality}.mpc"));
            let status = Command::new(&mpcenc)
                .arg("--silent")
                .arg("--overwrite")
                .arg("--quality")
                .arg(quality.to_string())
                .arg(&wav)
                .arg(&out)
                .status()
                .unwrap_or_else(|error| panic!("failed to run reference encoder: {error}"));
            if !status.success() {
                failures.push(format!(
                    "{input} q{quality}: reference encoder exited {status}"
                ));
                continue;
            }
            let produced = std::fs::read(&out)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", out.display()));
            let got = sha256_hex(&produced);
            if &got != expected {
                failures.push(format!(
                    "{input} q{quality}: expected {expected}, got {got}"
                ));
            }
            checked += 1;
        }
    }

    assert!(
        failures.is_empty(),
        "reference oracle mismatches ({} of {checked}):\n{}",
        failures.len(),
        failures.join("\n")
    );
    eprintln!("reference oracle: {checked} vectors reproduced exactly");
}
