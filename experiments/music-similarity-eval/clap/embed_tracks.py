"""Embed the blind playback tracks with laion/larger_clap_music.

Preprocessing follows the checkpoint's own ClapFeatureExtractor config
(preprocessor_config.json):

    sampling_rate   48000 Hz
    feature_size    64 mel bands
    n_fft           1024
    hop_length      480
    frequency range 50 - 14000 Hz
    chunk_length_s  10

The processor's default `truncation` is "rand_trunc", which would draw a random
10 s window per call. That is non-deterministic, so this script fixes the
windowing explicitly: the track is split into consecutive 10 s windows
(480000 samples), each window is embedded independently, and the per-window
embeddings are averaged and re-normalised. `truncation` is therefore never
reached, and the result is deterministic.

Cosine similarity is computed on the final L2-normalised track embedding.
"""

from __future__ import annotations

import hashlib
import json
import sys
import time
from pathlib import Path

import numpy as np
import soundfile as sf
import torch
from transformers import ClapModel, ClapProcessor

MODEL_ID = "laion/larger_clap_music"
REVISION = "a0b4534a14f58e20944452dff00a22a06ce629d1"
SAMPLE_RATE = 48000
WINDOW_SECONDS = 10
WINDOW_SAMPLES = SAMPLE_RATE * WINDOW_SECONDS  # 480000, matches nb_max_samples


def sha256_array(values: np.ndarray) -> str:
    return hashlib.sha256(np.ascontiguousarray(values, dtype=np.float32).tobytes()).hexdigest()


def embed_track(
    model: ClapModel,
    processor: ClapProcessor,
    audio: np.ndarray,
) -> tuple[np.ndarray, int, int]:
    """Mean-pool the per-window audio embeddings, then L2-normalise."""
    window_embeddings = []
    covered = 0
    total_windows = max(1, -(-audio.size // WINDOW_SAMPLES))
    for start in range(0, audio.size, WINDOW_SAMPLES):
        window = audio[start : start + WINDOW_SAMPLES]
        if window.size < WINDOW_SAMPLES:
            # Repeat-pad the final short window; the model requires a fixed
            # 480000-sample input, and repeat padding is the checkpoint's own
            # documented padding mode.
            repeats = -(-WINDOW_SAMPLES // window.size)
            window = np.tile(window, repeats)[:WINDOW_SAMPLES]
        covered += min(WINDOW_SAMPLES, audio.size - start)
        inputs = processor(
            audios=window,
            sampling_rate=SAMPLE_RATE,
            return_tensors="pt",
            truncation="rand_trunc",
        )
        with torch.no_grad():
            features = model.get_audio_features(
                input_features=inputs["input_features"],
                is_longer=inputs.get("is_longer"),
            )
        window_embeddings.append(features.numpy().reshape(-1))
    stacked = np.stack(window_embeddings, axis=0).mean(axis=0)
    norm = float(np.linalg.norm(stacked))
    if norm == 0.0:
        raise ValueError("degenerate zero-norm embedding")
    return (stacked / norm).astype(np.float32), total_windows, covered


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: embed_tracks.py DECODED_DIR OUT_JSON", file=sys.stderr)
        return 2
    decoded_dir = Path(sys.argv[1])
    out_path = Path(sys.argv[2])

    torch.manual_seed(0)
    torch.use_deterministic_algorithms(True, warn_only=True)
    torch.set_num_threads(8)

    print(f"loading {MODEL_ID}@{REVISION[:7]}")
    model = ClapModel.from_pretrained(MODEL_ID, revision=REVISION).eval()
    processor = ClapProcessor.from_pretrained(MODEL_ID, revision=REVISION)
    embedding_dim = int(model.config.projection_dim)
    print(f"projection_dim = {embedding_dim}")

    wavs = sorted(decoded_dir.glob("T*.wav"))
    if not wavs:
        print(f"no decoded inputs in {decoded_dir}", file=sys.stderr)
        return 1

    embeddings: dict[str, np.ndarray] = {}
    tracks = []
    for wav in wavs:
        audio, rate = sf.read(wav, dtype="float32", always_2d=False)
        if audio.ndim > 1:
            audio = audio.mean(axis=1)
        assert rate == SAMPLE_RATE, f"{wav.name} is {rate} Hz"
        began = time.monotonic()
        vector, windows, covered = embed_track(model, processor, audio)
        elapsed = time.monotonic() - began
        embeddings[wav.stem] = vector
        tracks.append(
            {
                "track_id": wav.stem,
                "embedding_dim": embedding_dim,
                "windows": windows,
                "samples_covered": covered,
                "duration_seconds": round(audio.size / SAMPLE_RATE, 6),
                "window_seconds": WINDOW_SECONDS,
                "mean_pooled": True,
                "l2_normalised": True,
                "embedding_sha256": sha256_array(vector),
                "seconds": round(elapsed, 3),
            }
        )
        print(
            f"  {wav.stem}  {windows:>4} windows"
            f"  {audio.size / SAMPLE_RATE:>8.1f}s audio"
            f"  {elapsed:>7.1f}s"
        )

    stacked = np.stack([embeddings[key] for key in sorted(embeddings)], axis=0)
    result = {
        "model_id": MODEL_ID,
        "revision": REVISION,
        "torch_version": torch.__version__,
        "device": "cpu",
        "deterministic": True,
        "random_seed": 0,
        "preprocessing": {
            "sample_rate": SAMPLE_RATE,
            "channels": 1,
            "window_seconds": WINDOW_SECONDS,
            "window_samples": WINDOW_SAMPLES,
            "windowing": "consecutive non-overlapping 10 s windows; mean-pooled then L2-normalised",
            "final_short_window": "repeat-padded to 480000 samples",
            "truncation": "rand_trunc not reached; windowing is explicit",
            "feature_extractor": "ClapFeatureExtractor defaults (64 mel bands, n_fft 1024, hop 480, 50-14000 Hz)",
            "similarity": "cosine on L2-normalised track embedding",
        },
        "embedding_dim": embedding_dim,
        "tracks": tracks,
        "_embeddings": {key: embeddings[key].tolist() for key in sorted(embeddings)},
    }
    out_path.write_text(json.dumps(result, indent=2) + "\n")
    print(f"wrote {out_path} ({len(tracks)} tracks, dim {stacked.shape[1]})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
