#!/usr/bin/env python3
"""Reference transcription with NVIDIA NeMo Parakeet.

This is intentionally separate from the Rust daemon. It exists only for
quality A/B checks against parakit's GGUF/CrispASR path.
"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Transcribe WAV files with NVIDIA NeMo Parakeet."
    )
    parser.add_argument(
        "audio",
        nargs="+",
        type=Path,
        help="WAV file(s) to transcribe.",
    )
    parser.add_argument(
        "--model",
        default="nvidia/parakeet-tdt-0.6b-v3",
        help="NeMo/Hugging Face model name.",
    )
    parser.add_argument(
        "--device",
        choices=("auto", "cuda", "cpu"),
        default="auto",
        help="Inference device. auto uses CUDA when torch reports it is available.",
    )
    return parser.parse_args()


def extract_text(result: object) -> str:
    if isinstance(result, tuple) and result:
        result = result[0]
    if isinstance(result, list) and result:
        result = result[0]
    text = getattr(result, "text", None)
    if text is not None:
        return str(text)
    return str(result)


def main() -> None:
    args = parse_args()

    import torch
    import nemo.collections.asr as nemo_asr

    use_cuda = args.device == "cuda" or (
        args.device == "auto" and torch.cuda.is_available()
    )

    model = nemo_asr.models.ASRModel.from_pretrained(model_name=args.model)
    model.eval()
    if use_cuda:
        model = model.cuda()
    else:
        model = model.cpu()

    for audio_path in args.audio:
        started = time.perf_counter()
        result = model.transcribe([str(audio_path)])
        infer_ms = int((time.perf_counter() - started) * 1000)
        print(
            json.dumps(
                {
                    "audio": str(audio_path),
                    "model": args.model,
                    "device": "cuda" if use_cuda else "cpu",
                    "infer_ms": infer_ms,
                    "text": extract_text(result),
                },
                ensure_ascii=False,
            ),
            flush=True,
        )


if __name__ == "__main__":
    main()
