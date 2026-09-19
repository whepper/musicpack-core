//! mpccut differential oracle (Phase 15J): the Rust `cut` must reproduce the
//! reference C `mpccut` outputs byte-for-byte.
//!
//! Fixtures were generated once by `tools/gen_cut_fixtures.py`; this test
//! never invokes C.

use std::path::{Path, PathBuf};

use musicpack_mpc_tools::cut::cut;

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/cut")
}

fn encoder_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../musicpack-musepack-encoder/tests/data/encoder")
}

#[test]
fn cut_matches_reference_mpccut() {
    let manifest = std::fs::read_to_string(data_dir().join("manifest.txt")).expect("manifest");
    let mut cases = 0usize;
    let mut bytes_total = 0usize;
    for line in manifest.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        let (name, input, start, end) = (f[0], f[1], f[2].parse().unwrap(), f[3].parse().unwrap());
        let src = std::fs::read(encoder_dir().join(input)).unwrap();
        let actual = cut(&src, start, end).unwrap_or_else(|e| panic!("{name}: cut failed: {e}"));
        let expected = std::fs::read(data_dir().join(format!("{name}.mpc"))).unwrap();
        assert_eq!(
            actual,
            expected,
            "{name}: first divergence at byte {}",
            actual
                .iter()
                .zip(expected.iter())
                .position(|(a, b)| a != b)
                .unwrap_or(actual.len().min(expected.len()))
        );
        cases += 1;
        bytes_total += expected.len();
        eprintln!("ok {name} ({} bytes)", expected.len());
    }
    assert!(cases > 0, "no cut fixtures found");
    eprintln!("mpccut compat: {cases} cases, {bytes_total} bytes matched");
}
