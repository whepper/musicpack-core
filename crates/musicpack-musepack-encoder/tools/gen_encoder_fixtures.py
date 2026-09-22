#!/usr/bin/env python3
"""Generate whole-encoder reference fixtures (Phase 15G, extended J.1).

Writes deterministic integer-only 16-bit WAVs, runs the reference `mpcenc`
binary on them, and stores the resulting `.mpc` streams plus manifests. The
Rust tests regenerate the same PCM and must reproduce the bytes.

Run from the crate root with the reference binary path:

    python3 tools/gen_encoder_fixtures.py <mpcenc> <reference-repo>

Two manifests are written:

* `manifest.txt`      — the original Phase 15G corpus (21 stereo cases),
                        unchanged in composition.
* `matrix_manifest.txt` — the J.1 integer-parity matrix: every integer
                        quality `0..=10` x every SV8 rate with `noise` and
                        `transient` signals, mono at q5 x 4 rates, plus
                        long multi-`AP`-block cases for the rate dimension
                        and the quality extremes.

Existing `.mpc` files are **never rewritten**: a fixture that already exists
is kept as-is and only hashed for the manifest. This keeps the original
frozen compatibility evidence byte-stable when the tool is re-run to add
cases.

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

RATES = [44100, 48000, 37800, 32000]


def lcg(state):
    return (state * 1664525 + 1013904223) & 0xFFFFFFFF


def signed16(state):
    return ((state >> 16) & 0xFFFF) - 32768


def gen(kind, frames, channels=2, seed_l=0x12345678, seed_r=0x9ABCDEF0):
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
        if channels == 1:
            # Mono: a single deterministic stream (the left/LCG stream).
            out += struct.pack("<h", l)
        else:
            out += struct.pack("<hh", l, r)
    return bytes(out)


CASES = [
    # (name, quality, rate, kind, frames) — original Phase 15G corpus,
    # frozen: these rows and files must not change.
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


def matrix_cases():
    """J.1 integer-parity matrix rows: (name, quality, rate, kind, frames,
    channels).

    * `noise` (5000 frames) and `transient` (25000 frames) for every integer
      quality `0..=10` x every SV8 rate = 44 + 44 stereo rows. Rows whose
      names already exist in the original corpus reuse those frozen files.
    * mono `noise` at q5 x four rates (mono differential coverage).
    * long multi-`AP`-block cases for the non-44.1k rate dimension and the
      quality extremes (q0/q10 @ 44100).
    """
    rows = []
    for q in range(11):
        for rate in RATES:
            rows.append((f"q{q}-{rate}-noise", q, rate, "noise", 5000, 2))
    for q in range(11):
        for rate in RATES:
            rows.append((f"q{q}-{rate}-transient", q, rate, "transient", 25000, 2))
    for rate in RATES:
        rows.append((f"mono-q5-{rate}-noise", 5, rate, "noise", 5000, 1))
    for q, rate in [(0, 44100), (10, 44100), (5, 48000), (5, 37800), (5, 32000)]:
        rows.append((f"q{q}-{rate}-multiblock", q, rate, "noise", 93728, 2))
    return rows


def fractional_cases():
    """J.2 fractional-parity rows: (name, quality, rate, kind, frames,
    channels).

    Deliberately sparse (27 rows, additive — never regenerates existing
    fixtures): the four representative fractional interiors across all four
    SV8 rates on `noise`, the same interiors on the deterministic periodic
    `ramp` signal at 44.1 kHz (the tonal input — stream equality on one
    input is never treated as the table oracle), the f32 parse-merge
    boundaries around5/6, arbitrary-precision values, and q9.9999 for the
    near-clip inequality. Table-level parity is covered separately by the
    committed `tests/data/psy/psy_q<qual>-<rate>.txt` oracles.
    """
    rows = []
    for q in (4.25, 5.5, 6.5, 8.5):
        for rate in RATES:
            rows.append((f"q{q}-{rate}-noise", q, rate, "noise", 5000, 2))
    for q in (4.25, 5.5, 6.5, 8.5):
        rows.append((f"q{q}-44100-ramp", q, 44100, "ramp", 5000, 2))
    for q in (4.9999999, 5.0000001, 5.9999999, 6.0000001):
        rows.append((f"q{q}-44100-noise", q, 44100, "noise", 5000, 2))
    for q in (4.2501, 6.0000005):
        rows.append((f"q{q}-44100-noise", q, 44100, "noise", 5000, 2))
    rows.append(("q9.9999-44100-noise", 9.9999, 44100, "noise", 5000, 2))
    return rows


def write_wav(path, rate, pcm, channels):
    with wave.open(path, "wb") as w:
        w.setnchannels(channels)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(pcm)


def encode(name, qual, rate, kind, frames, channels):
    mpc = os.path.join(OUTDIR, name + ".mpc")
    if os.path.exists(mpc):
        # Frozen-evidence guard: never rewrite an existing fixture; only
        # hash it for the manifest.
        data = open(mpc, "rb").read()
        print(f"{name}: kept existing ({len(data)} bytes)")
        return data
    pcm = gen(kind, frames, channels)
    wav = "/tmp/mp15g.wav"
    write_wav(wav, rate, pcm, channels)
    subprocess.run(
        [MPCENC, "--silent", "--overwrite", *SCALAR_FLAGS, "--quality", str(qual), wav, mpc],
        check=True,
    )
    data = open(mpc, "rb").read()
    print(f"{name}: {len(data)} bytes {hashlib.sha256(data).hexdigest()[:12]}")
    return data


def main():
    # The strict whole-encoder compatibility target is the scalar-forced C
    # reference (scalar encoder + scalar psychoacoustic kernels). These flags
    # are passed explicitly so fixture generation never depends on the build's
    # default SIMD dispatch. Do not remove them.
    global SCALAR_FLAGS
    SCALAR_FLAGS = ["--impl", "scalar", "--psy-impl", "scalar"]

    # Generate (or reuse) every unique fixture referenced by any manifest.
    seen = set()
    for name, qual, rate, kind, frames, channels in (
        [(n, q, r, k, f, 2) for (n, q, r, k, f) in CASES]
        + matrix_cases()
        + fractional_cases()
    ):
        if name in seen:
            continue
        seen.add(name)
        encode(name, qual, rate, kind, frames, channels)

    # Original corpus manifest (composition unchanged).
    manifest = ["# name quality rate kind frames bytes sha256"]
    for name, qual, rate, kind, frames in CASES:
        data = open(os.path.join(OUTDIR, name + ".mpc"), "rb").read()
        sha = hashlib.sha256(data).hexdigest()
        manifest.append(f"{name} {qual} {rate} {kind} {frames} {len(data)} {sha}")
    open(os.path.join(OUTDIR, "manifest.txt"), "w").write("\n".join(manifest) + "\n")

    # J.1 integer-parity matrix manifest (adds a channels column).
    matrix = ["# name quality rate kind frames channels bytes sha256"]
    for name, qual, rate, kind, frames, channels in matrix_cases():
        data = open(os.path.join(OUTDIR, name + ".mpc"), "rb").read()
        sha = hashlib.sha256(data).hexdigest()
        matrix.append(f"{name} {qual} {rate} {kind} {frames} {channels} {len(data)} {sha}")
    open(os.path.join(OUTDIR, "matrix_manifest.txt"), "w").write("\n".join(matrix) + "\n")

    # J.2 fractional-parity manifest (additive; integer manifests untouched).
    frac = ["# name quality rate kind frames channels bytes sha256"]
    for name, qual, rate, kind, frames, channels in fractional_cases():
        data = open(os.path.join(OUTDIR, name + ".mpc"), "rb").read()
        sha = hashlib.sha256(data).hexdigest()
        frac.append(f"{name} {qual} {rate} {kind} {frames} {channels} {len(data)} {sha}")
    open(os.path.join(OUTDIR, "fractional_manifest.txt"), "w").write("\n".join(frac) + "\n")
    print(f"wrote manifest.txt ({len(CASES)} cases), matrix_manifest.txt "
          f"({len(matrix_cases())} rows) and fractional_manifest.txt "
          f"({len(fractional_cases())} rows)")


main()
