"""Fetch and verify the laion/larger_clap_music checkpoint.

Run with the throwaway venv interpreter. Downloads to the default HuggingFace
cache and records a manifest of on-disk sizes plus SHA-256 digests, so the
evaluation can state exactly which bytes it ran against.
"""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

MODEL_ID = "laion/larger_clap_music"
# Published LFS object id for pytorch_model.bin, taken from the HF file tree.
EXPECTED_WEIGHTS_SHA256 = "5c289311f4a030d768af7ffbfdecd01b008aa64824211899a4e59f4f9d154fd1"
REVISION = "a0b4534a14f58e20944452dff00a22a06ce629d1"

REQUIRED_FILES = [
    "config.json",
    "preprocessor_config.json",
    "merges.txt",
    "vocab.json",
    "tokenizer.json",
    "tokenizer_config.json",
    "special_tokens_map.json",
    "pytorch_model.bin",
]


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def main() -> int:
    from huggingface_hub import snapshot_download

    local = Path(
        snapshot_download(
            repo_id=MODEL_ID,
            revision=REVISION,
            allow_patterns=REQUIRED_FILES,
        )
    )
    print(f"checkpoint: {local}")

    missing = [name for name in REQUIRED_FILES if not (local / name).is_file()]
    if missing:
        print("INCOMPLETE: missing files: " + ", ".join(missing))
        return 1

    files = []
    for name in REQUIRED_FILES:
        path = local / name
        files.append(
            {"file": name, "bytes": path.stat().st_size, "sha256": sha256_file(path)}
        )
        print(f"  {name:28} {path.stat().st_size:>12,}  {files[-1]['sha256'][:16]}")

    weights = next(entry for entry in files if entry["file"] == "pytorch_model.bin")
    if weights["sha256"] != EXPECTED_WEIGHTS_SHA256:
        print(
            "CHECKSUM MISMATCH for pytorch_model.bin: "
            f"expected {EXPECTED_WEIGHTS_SHA256}, got {weights['sha256']}"
        )
        return 1
    print("checksum matches the published LFS object id")

    manifest = {
        "model_id": MODEL_ID,
        "revision": REVISION,
        "local_dir": str(local),
        "total_bytes": sum(entry["bytes"] for entry in files),
        "files": files,
    }
    Path("clap-checkpoint-manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n"
    )
    print("wrote clap-checkpoint-manifest.json")
    return 0


if __name__ == "__main__":
    sys.exit(main())
