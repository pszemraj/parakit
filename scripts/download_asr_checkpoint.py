#!/usr/bin/env python3
# /// script
# requires-python = ">=3.10"
# dependencies = ["huggingface-hub"]
# ///
"""Download original ASR weights and tokenizer files for local GGUF conversion.

HF_ENDPOINT selects a Hub mirror and HF_TOKEN supplies optional authentication.
Downloaded checkpoint files and Hub resume metadata remain in the destination.
"""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

PATTERNS = [
    "config.json", "*.safetensors", "*.safetensors.index.json", "tokenizer.json",
    "tokenizer_config.json", "vocab.json", "merges.txt", "ternary.json",
    "preprocessor_config.json", "generation_config.json", "chat_template.json",
    "special_tokens_map.json",
]


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    """Parse the original checkpoint repository and download destination."""
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.ArgumentDefaultsHelpFormatter
    )
    parser.add_argument("repo", help="original Hugging Face model repository")
    parser.add_argument("destination", type=Path, help="checkpoint cache directory")
    parser.add_argument("--force", action="store_true", help="download checkpoint files again")
    return parser.parse_args(argv)


def download_checkpoint(repo: str, destination: Path, force: bool) -> None:
    """Download only original model weights, configuration and tokenizer files."""
    from huggingface_hub import snapshot_download

    endpoint = os.environ.get("HF_ENDPOINT", "").strip().rstrip("/") or "https://huggingface.co"
    token = os.environ.get("HF_TOKEN", "").strip() or False
    snapshot_download(
        repo_id=repo, local_dir=destination, allow_patterns=PATTERNS,
        endpoint=endpoint, token=token, force_download=force,
    )


def main(argv: list[str] | None = None) -> int:
    """Download a checkpoint or report a readable failure."""
    args = parse_args(argv)
    try:
        download_checkpoint(args.repo, args.destination, args.force)
    except KeyboardInterrupt:
        return 130
    except Exception as error:
        message = " ".join(str(error).splitlines())
        print(f"error: checkpoint download failed: {message}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
