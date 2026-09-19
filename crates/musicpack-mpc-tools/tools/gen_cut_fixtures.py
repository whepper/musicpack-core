#!/usr/bin/env python3
"""Generate mpccut differential fixtures (Phase 15J).

Runs the reference C `mpccut` binary over committed encoder-fixture streams
and stores the cut outputs plus a manifest. The Rust test replays the same
cuts and must reproduce the bytes. This script is run deliberately by a
developer; no Rust test invokes C.

Run from the crate root (`crates/musicpack-mpc-tools`):

    python3 tools/gen_cut_fixtures.py <mpccut> <encoder-data-dir>

SPDX-License-Identifier: LGPL-2.1-or-later
"""
import hashlib
import os
import subprocess
import sys

if len(sys.argv) != 3:
    raise SystemExit("usage: gen_cut_fixtures.py <mpccut> <encoder-data-dir>")
MPCCUT = sys.argv[1]
SRCDIR = sys.argv[2]
OUTDIR = "tests/data/cut"
os.makedirs(OUTDIR, exist_ok=True)

# (name, input, start, end) — end 0 means end-of-stream.
CASES = [
    ("short-full", "q5-44100-short.mpc", 0, 0),
    ("short-head", "q5-44100-short.mpc", 0, 500),
    ("short-mid", "q5-44100-short.mpc", 100, 900),
    ("short-tail", "q5-44100-short.mpc", 900, 0),
    ("multi-full", "q5-44100-multiblock.mpc", 0, 0),
    ("multi-head", "q5-44100-multiblock.mpc", 0, 20000),
    ("multi-mid", "q5-44100-multiblock.mpc", 30000, 60000),
    ("multi-tail", "q5-44100-multiblock.mpc", 80000, 0),
    ("multi-aligned", "q5-44100-multiblock.mpc", 0, 73728),
    ("multi-lastblock", "q5-44100-multiblock.mpc", 73728, 0),
    ("multi-unaligned", "q5-44100-multiblock.mpc", 12345, 54321),
    ("q7-transient-mid", "q7-44100-transient.mpc", 5000, 15000),
]


def main():
    manifest = ["# name input start end bytes sha256"]
    for name, inp, start, end in CASES:
        out = os.path.join(OUTDIR, name + ".mpc")
        subprocess.run(
            [MPCCUT, "-s", str(start), "-e", str(end),
             os.path.join(SRCDIR, inp), out],
            check=True,
        )
        data = open(out, "rb").read()
        sha = hashlib.sha256(data).hexdigest()
        manifest.append(f"{name} {inp} {start} {end} {len(data)} {sha}")
        print(f"{name}: {len(data)} bytes {sha[:12]}")
    open(os.path.join(OUTDIR, "manifest.txt"), "w").write("\n".join(manifest) + "\n")


main()
