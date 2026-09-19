//! Compatibility tests for canonical JSON number formatting.
//!
//! `tests/data/g8_c_reference.txt` is a committed table of
//! `<hex f64 bits> <canonical output>` vectors produced by the real C
//! reference (`tools/g8_probe.c` runs the actual `snprintf` calls used by
//! `json_number` in `core/libmusicpack/src/manifest.c`). Regeneration
//! instructions are in that file's header. These vectors are the empirical
//! proof that the Rust port reproduces C `%.8g` behaviour byte-for-byte.

use std::fs;

use musicpack_core::format::number::{format_g, format_json_number};

fn load_vectors() -> Vec<(u64, String)> {
    let text = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/g8_c_reference.txt"
    ))
    .expect("vector table present");
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let (bits, out) = line.split_once(' ').expect("two columns");
            (
                u64::from_str_radix(bits, 16).expect("hex bits"),
                out.to_string(),
            )
        })
        .collect()
}

#[test]
fn json_number_matches_the_c_reference() {
    let vectors = load_vectors();
    assert!(vectors.len() > 3000, "expected the full vector table");
    let mut failures = Vec::new();
    for (bits, expected) in &vectors {
        let actual = format_json_number(f64::from_bits(*bits));
        if &actual != expected {
            failures.push(format!(
                "bits {bits:016x}: expected {expected:?}, got {actual:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} vectors diverge from the C reference:\n{}",
        failures.len(),
        vectors.len(),
        failures
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn format_g_matches_the_c_reference() {
    // The probe's integral-branch outputs must also be reachable through
    // format_g alone when the value is non-integral; for integral values
    // the two differ by design (format_g is %.8g only). Verify the %g
    // subset: all vectors whose value is non-integral or outside the
    // ±9.2e18 gate must match format_g directly.
    let vectors = load_vectors();
    let mut checked = 0;
    let mut failures = Vec::new();
    for (bits, expected) in &vectors {
        let v = f64::from_bits(*bits);
        #[allow(clippy::manual_range_contains)] // mirrors the C condition
        let hits_json_number_integer_branch = v.fract() == 0.0 && v >= -9.2e18 && v <= 9.2e18;
        if hits_json_number_integer_branch {
            continue;
        }
        checked += 1;
        let actual = format_g(v);
        if &actual != expected {
            failures.push(format!(
                "bits {bits:016x}: expected {expected:?}, got {actual:?}"
            ));
        }
    }
    assert!(checked > 1000, "expected a large %g subset");
    assert!(
        failures.is_empty(),
        "{} of {} %g vectors diverge:\n{}",
        failures.len(),
        checked,
        failures
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn representative_values() {
    // Documented behaviours, readable without the vector table.
    assert_eq!(format_json_number(-12.0), "-12"); // integral, not "-12.0"
    assert_eq!(format_json_number(1.5), "1.5");
    assert_eq!(format_json_number(-7.1902902), "-7.1902902");
    assert_eq!(format_json_number(1e-5), "1e-05");
    assert_eq!(format_json_number(99999999.0), "99999999"); // integral → integer branch
    assert_eq!(format_json_number(123456789.5), "1.2345679e+08");
    assert_eq!(format_json_number(5e-324), "4.9406565e-324");
}
