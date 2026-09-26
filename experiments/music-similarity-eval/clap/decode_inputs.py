"""Decode the blind playback files to canonical 48 kHz mono WAV for CLAP.

Musepack inputs are not readable by libsndfile, so ffmpeg is the single decoder
for every input. Decoding is anonymous: only T### file names are used, never the
symlink targets, and no metadata is read or written.

Canonical form:
  container      WAV (RIFF, 16-bit signed PCM)
  sample rate    48000 Hz  (ClapFeatureExtractor sampling_rate)
  channels       1        (mono downmix)
  resampler      ffmpeg aresample, soxr when built in, else swr (recorded below)

The resampler engine actually used is probed and written into the manifest
rather than assumed, so the preprocessing statement stays truthful.
"""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path

TARGET_RATE = 48000

# ffmpeg is not always built with libsoxr; fall back to the always-present swr
# engine and record which one was used.
RESAMPLER_CANDIDATES = [
    "aresample=resampler=soxr:precision=28",
    "aresample=resampler=swr",
]


def available_resampler() -> str:
    for candidate in RESAMPLER_CANDIDATES:
        probe = subprocess.run(
            [
                "ffmpeg",
                "-v",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                f"sine=f=440:d=0.1:r=44100",
                "-af",
                candidate,
                "-f",
                "null",
                "-",
            ],
            capture_output=True,
            text=True,
        )
        if probe.returncode == 0:
            return candidate
    raise SystemExit("no usable ffmpeg resampler engine found")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def decode(source: Path, destination: Path, resampler: str) -> None:
    subprocess.run(
        [
            "ffmpeg",
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-i",
            str(source),
            "-map_metadata",
            "-1",
            "-vn",
            "-ac",
            "1",
            "-ar",
            str(TARGET_RATE),
            "-af",
            resampler,
            "-c:a",
            "pcm_s16le",
            str(destination),
        ],
        check=True,
    )


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: decode_inputs.py PLAY_DIR OUT_DIR", file=sys.stderr)
        return 2
    play_dir = Path(sys.argv[1])
    out_dir = Path(sys.argv[2])
    out_dir.mkdir(parents=True, exist_ok=True)
    resampler = available_resampler()
    print(f"resampler: {resampler}")

    sources = sorted(play_dir.glob("T*.flac")) + sorted(play_dir.glob("T*.mpc"))
    if not sources:
        print(f"no anonymous T### inputs found in {play_dir}", file=sys.stderr)
        return 1

    entries = []
    for source in sources:
        destination = out_dir / (source.stem + ".wav")
        decode(source, destination, resampler)
        probe = subprocess.run(
            [
                "ffprobe",
                "-v",
                "error",
                "-show_entries",
                "stream=sample_rate,channels,codec_name:format=duration",
                "-of",
                "json",
                str(destination),
            ],
            check=True,
            capture_output=True,
            text=True,
        )
        info = json.loads(probe.stdout)
        stream = info["streams"][0]
        entries.append(
            {
                "track_id": source.stem,
                "input_suffix": source.suffix.lstrip("."),
                "sample_rate": int(stream["sample_rate"]),
                "channels": int(stream["channels"]),
                "codec": stream["codec_name"],
                "duration_seconds": round(float(info["format"]["duration"]), 6),
                "wav_bytes": destination.stat().st_size,
                "wav_sha256": sha256_file(destination),
            }
        )
        print(
            f"  {entries[-1]['track_id']}  {entries[-1]['input_suffix']:>4}"
            f"  {entries[-1]['duration_seconds']:>9.2f}s"
            f"  {entries[-1]['sample_rate']} Hz"
            f"  {entries[-1]['channels']}ch"
        )

    manifest = {
        "decoder": subprocess.run(
            ["ffmpeg", "-version"], capture_output=True, text=True, check=True
        ).stdout.splitlines()[0],
        "target_sample_rate": TARGET_RATE,
        "target_channels": 1,
        "target_codec": "pcm_s16le",
        "resampler": resampler,
        "metadata_stripped": True,
        "track_count": len(entries),
        "tracks": entries,
    }
    (out_dir / "decoded-inputs.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"decoded {len(entries)} tracks -> {out_dir}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
