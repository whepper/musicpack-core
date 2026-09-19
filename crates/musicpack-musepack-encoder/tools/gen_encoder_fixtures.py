#!/usr/bin/env python3
"""Generate whole-encoder reference fixtures (Phase 15G).

Writes deterministic integer-only 16-bit stereo WAVs, runs the reference
`mpcenc` binary on them, and stores the resulting `.mpc` streams plus a
manifest. The Rust test regenerates the same PCM and must reproduce the bytes.

Run from the crate root with the reference binary path:

    python3 tools/gen_encoder_fixtures.py <mpcenc> <reference-repo>

This is temporary migration tooling; it is not built by Cargo and no Rust test
invokes the C encoder.
"""
import hashlib
import os
import struct
import subprocess
import sys
import wave

if len(sys.argv) != 3:
    raise SystemExit("usage: gen_encoder_fixtures.py <mpcenc> <reference-repo>")
MPCENC = sys.argv[1]
OUTDIR = "tests/data/encoder"
os.makedirs(OUTDIR, exist_ok=True)


def lcg(state):
    return (state * 1664525 + 1013904223) & 0xFFFFFFFF


def signed16(state):
    return ((state >> 16) & 0xFFFF) - 32768


def gen(kind, frames, seed_l=0x12345678, seed_r=0x9ABCDEF0):
    sl, sr = seed_l, seed_r
    out = bytearray()
    for i in range(frames):
        if kind == "silence":
            l = r = 0
        elif kind == "impulse":
            l = r = 30000 if i == 0 else 0
        elif kind == "constant":
            l = r = 12000
        elif kind == "alternating":
            l = r = 8000 if i % 2 == 0 else -8000
        elif kind == "ramp":
            l = r = ((i % 512) - 256) * 60
        elif kind == "noise":
            sl = lcg(sl); l = signed16(sl)
            sr = lcg(sr); r = signed16(sr)
        elif kind == "left_only":
            sl = lcg(sl); l = signed16(sl); r = 0
        elif kind == "right_only":
            sr = lcg(sr); r = signed16(sr); l = 0
        elif kind == "phase_invert":
            sl = lcg(sl); l = signed16(sl); r = -l
        elif kind == "transient":
            v = 0
            if i == 100:
                v = 30000
            elif i == 5000:
                v = -20000
            elif i == 20000:
                v = 25000
            l = r = v
        else:
            raise SystemExit("unknown kind " + kind)
        out += struct.pack("<hh", l, r)
    return bytes(out)


CASES = [
    # (name, quality, rate, kind, frames)
    ("q5-44100-silence", 5, 44100, "silence", 5000),
    ("q5-44100-impulse", 5, 44100, "impulse", 5000),
    ("q5-44100-constant", 5, 44100, "constant", 5000),
    ("q5-44100-alternating", 5, 44100, "alternating", 5000),
    ("q5-44100-ramp", 5, 44100, "ramp", 5000),
    ("q5-44100-noise", 5, 44100, "noise", 5000),
    ("q5-44100-left_only", 5, 44100, "left_only", 5000),
    ("q5-44100-right_only", 5, 44100, "right_only", 5000),
    ("q5-44100-phase_invert", 5, 44100, "phase_invert", 5000),
    ("q5-44100-transient", 5, 44100, "transient", 25000),
    ("q5-44100-short", 5, 44100, "noise", 1000),
    ("q5-44100-oneframe", 5, 44100, "noise", 1152),
    ("q5-44100-multiblock", 5, 44100, "noise", 73728 + 20000),
    ("q4-44100-noise", 4, 44100, "noise", 5000),
    ("q6-44100-noise", 6, 44100, "noise", 5000),
    ("q7-44100-noise", 7, 44100, "noise", 5000),
    ("q4-44100-transient", 4, 44100, "transient", 25000),
    ("q7-44100-transient", 7, 44100, "transient", 25000),
    ("q5-48000-noise", 5, 48000, "noise", 5000),
    ("q5-37800-noise", 5, 37800, "noise", 5000),
    ("q5-32000-noise", 5, 32000, "noise", 5000),
]


def write_wav(path, rate, pcm):
    with wave.open(path, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(pcm)


def main():
    manifest = ["# name quality rate kind frames bytes sha256"]
    # The strict whole-encoder compatibility target is the scalar-forced C
    # reference (scalar encoder + scalar psychoacoustic kernels). These flags
    # are passed explicitly so fixture generation never depends on the build's
    # default SIMD dispatch. Do not remove them.
    scalar_flags = ["--impl", "scalar", "--psy-impl", "scalar"]
    for name, qual, rate, kind, frames in CASES:
        pcm = gen(kind, frames)
        wav = "/tmp/mp15g.wav"
        mpc = os.path.join(OUTDIR, name + ".mpc")
        write_wav(wav, rate, pcm)
        subprocess.run(
            [MPCENC, "--silent", "--overwrite", *scalar_flags, "--quality", str(qual), wav, mpc],
            check=True,
        )
        data = open(mpc, "rb").read()
        sha = hashlib.sha256(data).hexdigest()
        manifest.append(f"{name} {qual} {rate} {kind} {frames} {len(data)} {sha}")
        print(f"{name}: {len(data)} bytes {sha[:12]}")
    open(os.path.join(OUTDIR, "manifest.txt"), "w").write("\n".join(manifest) + "\n")


main()
