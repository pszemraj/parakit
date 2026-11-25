# Maintainer Workflows

## Model Artifacts

Upload GGUFs at the repository root under their [preset artifact names](../running.md#model-presets). Keep F16 source GGUFs, including `parakeet-tdt-0.6b-v3-F16.gguf`, for future re-quantization. Validate artifacts against the original runtimes. [Model comparisons](quality.md#recommended-ab-procedure)

The Parakeet converter, loader, and `crispasr-quantize` path are built around F16/F32 tensors. Treat BF16 as future work until it has explicit support and validation.

## Source Rebuild

Hosted Q8_0 downloads use the Parakit executable alone. Install `crispasr-quantize` (`crispasr-quantize.exe` on Windows) on `PATH` to prepare Q6_K or rebuild from source. A source rebuild also searches `PATH` for `python3` or `python`, the converter for the selected model, and `download_asr_checkpoint.py` when downloading Qwen or Moondream checkpoints. Parakit passes the scripts to Python, so they need only be readable.

Build the quantizer from the pinned submodule and install the tools into a directory on `PATH`.

```bash
cmake -S vendor/CrispASR -B target/conversion-tools \
  -DBUILD_SHARED_LIBS=OFF -DCRISPASR_BUILD_TESTS=OFF \
  -DCRISPASR_BUILD_EXAMPLES=ON -DCRISPASR_BUILD_SERVER=OFF
cmake --build target/conversion-tools --config Release --target crispasr-quantize --parallel
tool_prefix="$HOME/.local"
mkdir -p "$tool_prefix/bin"
cp target/conversion-tools/bin/crispasr-quantize "$tool_prefix/bin/"
cp vendor/CrispASR/models/convert-parakeet-to-gguf.py \
  vendor/CrispASR/models/convert-qwen3-asr-to-gguf.py \
  scripts/download_asr_checkpoint.py "$tool_prefix/bin/"
export PATH="$tool_prefix/bin:$PATH"
```

On Windows, copy `crispasr-quantize.exe` and the Python scripts into a directory on `PATH`. With a multi-config generator, look under `bin/Release/` for the executable. Use the native prerequisites for the selected CrispASR build. The installed tools work independently of the checkout.

Install the conversion dependencies in the Python environment used by `fetch`.

```bash
python -m pip install -r scripts/requirements-convert.txt
```

Select a preset to rebuild from its original checkpoint.

```bash
# Default Ultra, from Moondream.
parakit fetch --from-source --keep-f16
# Original NVIDIA .nemo checkpoint.
parakit fetch --from-source --model-preset parakeet-v3 --keep-f16 --keep-nemo
# Original Hugging Face Qwen checkpoint.
parakit fetch --from-source --model-preset qwen3-asr-1.7b --keep-f16
```

`parakeet-v2` has no source-rebuild path; use its hosted Q8_0 artifact or `--quantize q6_k` instead.

The v3 path downloads [NVIDIA's official `.nemo`](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3), converts it with `convert-parakeet-to-gguf.py`, then quantizes it with `crispasr-quantize`. Other source presets use these Hugging Face checkpoints:

| Preset | Original checkpoint |
| --- | --- |
| `qwen3-asr-0.6b` | [Qwen/Qwen3-ASR-0.6B](https://huggingface.co/Qwen/Qwen3-ASR-0.6B) |
| `qwen3-asr-1.7b` | [Qwen/Qwen3-ASR-1.7B](https://huggingface.co/Qwen/Qwen3-ASR-1.7B) |
| `parakeet-ultra` | [moondream/parakeet-ultra](https://huggingface.co/moondream/parakeet-ultra) |
| `parakeet-redux` | [moondream/parakeet-redux](https://huggingface.co/moondream/parakeet-redux) |

The pipeline downloads and retains the original snapshot under `<model cache>/checkpoints/<preset-id>/<source-commit>/`, converts it to F16 with the installed model converter, then runs `crispasr-quantize` with `q8_0`. `--keep-f16` retains the intermediate GGUF beside the final Q8_0; without it the intermediate is removed once Q8_0 is ready, including on a completed Q8_0 cache hit. `--keep-nemo` applies only to the original Parakeet v3 path.

For an existing original Hugging Face snapshot, add `--checkpoint-dir`:

```bash
parakit fetch --from-source --model-preset qwen3-asr-1.7b \
  --checkpoint-dir /path/to/Qwen3-ASR-1.7B --keep-f16
```

Supply the original weights, tokenizer, and configuration files. `--checkpoint-dir` requires `--from-source` with a Qwen or Moondream preset and leaves the supplied directory intact.

A completed Q8_0 cache hit bypasses the Python dependency check and network access. `--keep-f16` preserves an F16 intermediate if one already exists. If only F16 is cached, Parakit quantizes it directly. Use `--force` to repeat conversion and quantization from the supplied snapshot or the pinned source revision. Each pinned Qwen or Moondream source revision gets its own checkpoint directory. The rebuilt Q8_0 occupies the hosted preset's cache path and is selected by `parakit start --model-preset NAME`. [Q6_K preparation](../running.md#model-presets)

Redux conversion reconstructs its packed ternary weights into F16 before Q8_0 quantization. See the [Redux limitations](../running.md#model-presets) and [conversion acceptance checks](quality.md#checkpoint-conversion-acceptance).

Missing tools report their filename and installation guidance before starting conversion.

## Deferred Runtime Work

TODO: Config does not yet support remapping the actual PTT chords (e.g. right Command alone, or right Command plus right Option on macOS). Keep the [documented platform shortcuts](../running.md) until that lands.

TODO: Keep the direct platform-hotkey path available when configurable chords land. On macOS, extend the existing CoreGraphics event-tap backend so it owns the selected chord, suppresses only that chord, and handles tap-disabled callbacks. On Linux, preserve the [session-specific backends](../linux-desktop.md) and keep the evdev/uinput proxy opt-in. On Windows, `RegisterHotKey` already gives a clear conflict/error boundary.

TODO: Detect configured Linux input-source shortcut conflicts automatically; diagnostics currently provide a [manual-check advisory](../linux-desktop.md#shortcut-conflicts).

TODO: Revisit Linux/macOS microphone idle policy. Either move them to the same pause/resume default as Windows or keep 350 ms warm pre-roll only with measured idle CPU and first-syllable evidence that justifies the cost.

TODO: Replace fallback microphone device polling with platform event notifications when the audio layer is split: Windows `IMMNotificationClient`, PipeWire/PulseAudio registry events, and macOS `AudioObject` property listeners. Expose stream state, callback drops, and recovery counters through daemon status at the same time.

TODO: Evaluate callback-confirmed recording cues and a short post-roll window. The current cue fires after the start command succeeds, not after the first input callback, and release drains only already-arrived samples plus the resampler tail.

TODO: Upgrade `enigo` from the 0.2 line in a cross-platform validation branch. Linux uses IBus native insertion or X11 batch paste, Windows batch paste uses `SendInput`, and macOS batch paste uses CoreGraphics Cmd+V events that `doctor --deep` can smoke-test. Keep the dependency update focused on direct typing unless a real desktop paste regression points back to the fallback path.

TODO: Revisit model durability semantics before packaged releases. Source builds use the XDG-style `~/.cache/parakit/models/` path on Linux and macOS; a future bundle may want a less reclaimable app-data location.

TODO: Add an optional X11 paste inter-key hold only if real target applications miss the current XTest paste chord. The current smoke test covers X11 event delivery; app-specific compatibility should drive any delay so normal paste latency does not grow without evidence.

TODO: Benchmark an opt-in Windows MSVC `/GL` + `/LTCG` build after the CPU daemon is stable. MSVC does not have a direct `/O3`; keep the default at `/O2` unless link-time optimization shows a real transcription-speed win without disruptive build time or packaging side effects.

TODO: Add a Windows PE dependency-walker validation pass for the CPU bundle so release packaging verifies every transitive DLL dependency instead of relying only on build-time known runtime DLL names.

TODO: Run the full Windows BLAS/thread benchmark matrix for CPU builds, including no BLAS, OpenBLAS with controlled OpenMP ownership, and relevant `--threads` values against the pinned voice-memo smoke file.

TODO: Complete the remaining [native model and physical desktop checks](model-validation.md), including target-gated Linux and Windows transports and input backends. CI remains limited to static checks unless its scope and cost are separately agreed.

## Updating CrispASR

Update the native source, its nested submodules, both local binding manifests, and `Cargo.lock` together. Keep the binding package versions aligned with upstream. Review changes to the session ABI, model setters, ggml device registry, loader, and platform-specific linkage before changing the pin.

```bash
git submodule update --init --recursive
cd vendor/CrispASR
git fetch --tags
git checkout '<tag-or-commit>'
git submodule update --init --recursive
cd ../..
cargo check
git add vendor/CrispASR native/crispasr/Cargo.toml native/crispasr-sys/Cargo.toml Cargo.lock
cargo check --locked
```

The first `cargo check` refreshes `Cargo.lock` and the locked check verifies it. Review the source anchors in `build/native_correctness.rs` against the new native source. Keep each correction until upstream provides equivalent behavior. Build the new native library under the [ABI and header requirements](../build.md#crispasr-and-backends) and run the [Rust validation loop](quality.md#rust-validation-loop).

Check the PCM input contract, segment text, runtime defaults, and model preprocessing against the new pin. [Native model validation](model-validation.md)
