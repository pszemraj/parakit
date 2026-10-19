# Development Notes

Source-coverage setup and report commands are in [quality.md#rust-source-coverage](quality.md#rust-source-coverage).

## Model Artifacts

End-user model selection, downloads, cache behavior, and quantization are in [running.md#model-cache](../running.md#model-cache). F16 and official-runtime comparison procedures are in [quality.md#qwen-and-moondream-comparisons](quality.md#qwen-and-moondream-comparisons).

Maintainer release files:

| File | Role |
| --- | --- |
| `parakeet-tdt-0.6b-v3-Q8_0.gguf` | Default user artifact. |
| `parakeet-tdt-0.6b-v3-F16.gguf` | Source GGUF kept for maintainers and future re-quantization work. |

Avoid unrelated names, nested directories, or model-card-only links for release artifacts.

The Parakeet converter, loader, and `crispasr-quantize` path are built around F16/F32 tensors. Treat BF16 as future work until it has explicit support and validation.

## Source Rebuild

Install the conversion dependencies once in the Python environment used by `fetch`:

```bash
python -m pip install -r scripts/requirements-convert.txt
```

With no preset, or with `--model-preset parakeet-v3`, rebuild from NVIDIA's `.nemo` checkpoint:

```bash
parakit fetch --from-source --keep-f16 --keep-nemo
```

That path downloads [NVIDIA's official `.nemo`](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3), converts it with `vendor/CrispASR/models/convert-parakeet-to-gguf.py`, and quantizes the intermediate GGUF with `crispasr-quantize`.

Rebuild a Qwen or Moondream preset from its original Hugging Face checkpoint:

```bash
parakit fetch --from-source --model-preset qwen3-asr-1.7b --keep-f16
```

| Preset | Original checkpoint |
| --- | --- |
| `qwen3-asr-0.6b` | [Qwen/Qwen3-ASR-0.6B](https://huggingface.co/Qwen/Qwen3-ASR-0.6B) |
| `qwen3-asr-1.7b` | [Qwen/Qwen3-ASR-1.7B](https://huggingface.co/Qwen/Qwen3-ASR-1.7B) |
| `parakeet-ultra` | [moondream/parakeet-ultra](https://huggingface.co/moondream/parakeet-ultra) |
| `parakeet-redux` | [moondream/parakeet-redux](https://huggingface.co/moondream/parakeet-redux) |

The pipeline downloads and retains the original snapshot under `<model cache>/checkpoints/<preset-id>/<source-commit>/`, converts it to F16 with the vendored model converter, then runs the built `crispasr-quantize` with `q8_0`. `--keep-f16` retains the intermediate GGUF beside the final Q8_0; without it the intermediate is removed once Q8_0 is ready, including on a completed Q8_0 cache hit. `--keep-nemo` applies only to the original Parakeet v3 path.

For an existing original Hugging Face snapshot, add `--checkpoint-dir`:

```bash
parakit fetch --from-source --model-preset qwen3-asr-1.7b \
  --checkpoint-dir /path/to/Qwen3-ASR-1.7B --keep-f16
```

This directory must contain the original checkpoint and tokenizer/configuration files, not a GGUF. It requires `--from-source` with a Qwen or Moondream preset. The pipeline never removes the supplied directory.

A completed Q8_0 artifact in the preset cache is reused before checking Python dependencies or accessing the network. `--keep-f16` does not create a missing F16 intermediate on that cache hit. If only F16 is cached, it is quantized without re-reading the checkpoint, including a supplied `--checkpoint-dir`. Add `--force` to repeat conversion and quantization; for downloaded checkpoints this also refreshes the selected source commit. Qwen and Moondream presets record fixed provider and source commits in the model catalog, so refresh does not silently select a newer upstream revision. A new catalog revision uses a separate checkpoint directory, preventing stale shards from another revision entering conversion. Rebuilt Q8_0 goes to the same destination as the hosted preset: `parakit start --model-preset NAME` selects it without changing daemon configuration or the application default. Source rebuilding produces Q8_0; the [cached Q6_K workflow](../running.md#model-presets) uses provider F16 instead.

Redux conversion reconstructs its packed ternary weights into ordinary F16 weights before Q8_0 quantization. The resulting GGUF does not retain Photon's 178 MB representation, ternary acceleration, or Moondream VAD head. Building the artifact also does not establish transcription quality; follow the [conversion acceptance checks](quality.md#checkpoint-conversion-acceptance).

On Windows, the hosted Q8_0 path is the normal model setup. `fetch --from-source` requires a compatible `crispasr-quantize.exe` on `PATH` because bundled CPU builds skip the CrispASR examples tree under MSVC.

GPU build and runtime checks are in [quality.md#gpu-feature-validation](quality.md#gpu-feature-validation).

## Deferred Runtime Work

TODO: Config does not yet support remapping the actual PTT chord (e.g. a custom macOS fallback of right Command alone, or right Command plus right Option). Keep the default chords as-is until that lands: Linux/Windows use `Ctrl+Space`, and macOS uses `Left Control+Space`.

TODO: Keep the direct platform-hotkey path available when configurable chords land. On macOS, extend the existing CoreGraphics event-tap backend so it owns the selected chord, suppresses only that chord, and handles tap-disabled callbacks. On Linux, keep the registered X11 backend as the normal path and use the evdev/uinput proxy only when users explicitly accept the lower-level permission tradeoff. On Windows, `RegisterHotKey` already gives a clear conflict/error boundary.

TODO: Add a Linux `doctor` warning for known IBus `Ctrl+Space` conflicts, or close this if configurable hotkeys make the warning unnecessary. Keep the current Linux docs warning until the default/config story changes.

TODO: Remove the [Unix source-install dependency on the repository `target/` library tree](../build.md#install) in a dedicated follow-up PR. First try to work upstream with CrispASR for static/manual linking support; if that is not viable, revisit full vendoring or an aggregate static-link strategy in parakit. Do not patch the CrispASR submodule locally for this.

TODO: Revisit Linux/macOS microphone idle policy. Either move them to the same pause/resume default as Windows or keep 350 ms warm pre-roll only with measured idle CPU and first-syllable evidence that justifies the cost.

TODO: Replace fallback microphone device polling with platform event notifications when the audio layer is split: Windows `IMMNotificationClient`, PipeWire/PulseAudio registry events, and macOS `AudioObject` property listeners. Expose stream state, callback drops, and recovery counters through daemon status at the same time.

TODO: Evaluate callback-confirmed recording cues and a short post-roll window. The current cue fires after the start command succeeds, not after the first input callback, and release drains only already-arrived samples plus the resampler tail.

TODO: Upgrade `enigo` from the 0.2 line in a cross-platform validation branch. Linux batch paste uses X11 directly, Windows batch paste uses `SendInput`, and macOS batch paste uses CoreGraphics Cmd+V events that `doctor --deep` can smoke-test. Keep the dependency update focused on direct typing unless a real desktop paste regression points back to the fallback path.

TODO: Revisit model durability semantics before packaged releases. Source builds use the XDG-style `~/.cache/parakit/models/` path on Linux and macOS; a future bundle may want a less reclaimable app-data location.

TODO: Add an optional X11 paste inter-key hold only if real target applications miss the current XTest paste chord. The current smoke test covers X11 event delivery; app-specific compatibility should drive any delay so normal paste latency does not grow without evidence.

TODO: Benchmark an opt-in Windows MSVC `/GL` + `/LTCG` build after the CPU daemon is stable. MSVC does not have a direct `/O3`; keep the default at `/O2` unless link-time optimization shows a real transcription-speed win without disruptive build time or packaging side effects.

TODO: Add a Windows PE dependency-walker validation pass for the CPU bundle so release packaging verifies every transitive DLL dependency instead of relying only on build-time known runtime DLL names.

TODO: Run the full Windows BLAS/thread benchmark matrix for CPU builds, including no BLAS, OpenBLAS with controlled OpenMP ownership, and relevant `--threads` values against the pinned voice-memo smoke file.

TODO: Complete the remaining [native model and physical desktop checks](model-validation.md), including target-gated Linux and Windows transports and input backends. CI remains limited to static checks unless its scope and cost are separately agreed.

## Updating [CrispASR](https://github.com/CrispStrobe/CrispASR)

Use the [dependency update procedure](crispasr-upgrade.md#updating-crispasr) to keep native sources, Rust bindings, and the lockfile aligned.
