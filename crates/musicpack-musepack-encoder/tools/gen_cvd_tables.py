#!/usr/bin/env python3
"""Generate `src/psy/cvd_tables.rs` from the reference `cvd.c` literals.

Run from the crate root (`crates/musicpack-musepack-encoder`) with the
reference checkout path as the first argument:

    python3 tools/gen_cvd_tables.py /path/to/musicpack

The `Puls`/`CosWin` arrays are literal `static const float` data; they are
emitted as IEEE-754 bit patterns so the Rust constants are exact.
Run `cargo fmt` after generating.
"""
import re
import struct
import sys

if len(sys.argv) != 2:
    raise SystemExit("usage: gen_cvd_tables.py <reference-repo>")
REF = sys.argv[1] + "/codec/libmpcpsy/cvd.c"
OUT = "src/psy/cvd_tables.rs"
src = open(REF).read()


def block(name):
    i = src.index(name)
    j = src.index("{", i)
    depth = 0
    for k in range(j, len(src)):
        if src[k] == "{":
            depth += 1
        elif src[k] == "}":
            depth -= 1
            if depth == 0:
                break
    return src[j:k + 1]


puls = [float(x[:-1]) for x in re.findall(r"-?\d+\.\d+f", block("Puls ["))]
coswin = [float(x[:-1]) for x in re.findall(r"\d+\.\d+f", block("CosWin ["))]
assert len(puls) == 9, len(puls)
assert len(coswin) == 256, len(coswin)


def bits(v):
    # The integer value of the little-endian IEEE-754 encoding, so the
    # `f32::from_bits` literal below reproduces the exact reference constant.
    return f"0x{struct.unpack('<I', struct.pack('<f', v))[0]:08X}"


out = []
out.append("//! Literal CVD source tables (`cvd.c`), copied faithfully as IEEE-754 bit\n")
out.append("//! patterns so the values are exactly the reference `float` constants.\n\n")
out.append("/// `Puls[9]`, the cepstral pulse shape used by `CEP_Analyse2048`.\n")
out.append("pub(crate) const PULS: [f32; 9] = [\n")
for i in range(0, 9, 4):
    out.append("    " + ", ".join(f"f32::from_bits({bits(v)})" for v in puls[i:i + 4]) + ",\n")
out.append("];\n\n")
out.append("/// `CosWin[256]`, the cepstral cos-rolloff window.\n")
out.append("pub(crate) const COS_WIN: [f32; 256] = [\n")
for i in range(0, 256, 4):
    out.append("    " + ", ".join(f"f32::from_bits({bits(v)})" for v in coswin[i:i + 4]) + ",\n")
out.append("];\n")
open(OUT, "w").write("".join(out))
print("wrote", OUT, len("".join(out)))
