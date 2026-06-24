# Development Notes

## Model Artifacts

End-user startup uses the hosted [Q8_0 GGUF](https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf).

The binary downloads `parakeet-tdt-0.6b-v3-Q8_0.gguf`, verifies the compiled-in SHA256, writes it to the platform model cache, and starts the daemon after startup preflights pass. The default `parakit` command must not require Python, NeMo, PyTorch, or manual model setup.

`-m <path>` is the escape hatch for local experiments and always disables automatic model fetch.

Hosted release files:

| File | Role |
| --- | --- |
| `parakeet-tdt-0.6b-v3-Q8_0.gguf` | Default user artifact. |
| `parakeet-tdt-0.6b-v3-F16.gguf` | Source GGUF kept for maintainers and future re-quantization work. |

The CLI has no quant selector. Q8_0 is the default hosted model, and `-m <path>` is the only supported model override. Avoid unrelated names, nested directories, or model-card-only links for release artifacts.

The Parakeet converter, loader, and `crispasr-quantize` path are built around F16/F32 tensors. Treat BF16 as future work until it has explicit support and validation.

## Source Rebuild

Maintainers can rebuild from NVIDIA's `.nemo` checkpoint:

```bash
python -m pip install -r scripts/requirements-convert.txt
parakit fetch --from-source --keep-f16 --keep-nemo
```

That path downloads [NVIDIA's official `.nemo`](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3), converts it with `vendor/CrispASR/models/convert-parakeet-to-gguf.py`, and quantizes the intermediate GGUF with `crispasr-quantize`.

On Windows, the hosted Q8_0 path is the normal model setup. `fetch --from-source` requires a compatible `crispasr-quantize.exe` on `PATH` because bundled CPU builds skip the CrispASR examples tree under MSVC.

After rebuilding a release artifact, upload F16 and Q8_0 to the hosted repo and update `HOSTED_Q8_SHA256` in `src/model.rs` if the Q8_0 bytes changed.

## Windows GPU Validation

Use the Windows scripts for CUDA/Vulkan validation; they default to Ninja. Raw `cargo check --workspace --all-targets --all-features` may still enter CMake's Visual Studio generator and fail before Rust typechecking if Visual Studio CUDA BuildCustomizations are stale. Known local failure: versioned CUDA targets leave `CudaToolkitDir` empty and emit `CUDA Toolkit directory '' does not exist`.

When that happens, record the exact CUDA/MSBuild error, then validate the Rust all-features surface with an existing bundled lib directory:

```powershell
$env:CRISPASR_LIB_DIR = (Resolve-Path 'target\debug\build\parakit-<hash>\out\lib').Path
cargo check --workspace --all-targets --all-features
Remove-Item Env:\CRISPASR_LIB_DIR
```

This fallback does not replace real GPU validation. Also run the CUDA and Vulkan Windows scripts plus simulated-dictation smoke tests against `local-scratch\Juniper_St_NE_5.wav` when touching Windows GPU behavior.

On macOS, raw `--all-features` also enables CUDA and can fail in CMake before Rust typechecking when the CUDA Toolkit is not installed. Use the same `CRISPASR_LIB_DIR` fallback to validate the Rust all-features surface; validate Metal with the native macOS build and `doctor`.

## File Size Exceptions

`src/daemon/audio/capture.rs` is temporarily over the 1k LoC target because it owns one tightly coupled runtime boundary: CPAL stream recovery, the SPSC drain thread, resampler flushing, and recording/pre-roll state. Split it after Windows CPU settles into smaller `audio/stream.rs`, `audio/drain.rs`, and `audio/device.rs` modules without changing behavior.

`src/daemon/desktop/inject.rs` is also temporarily over the target while clipboard transaction, X11 paste-chord cleanup, focus snapshots, and smoke-test support settle. Split it into focused clipboard, X11 paste, and focus modules without changing the paste safety contract.

`src/daemon/ipc.rs` is temporarily over the target because it owns both Unix socket IPC and Windows named-pipe IPC, including Windows ACL setup and retry policy tests. Split the Windows named-pipe transport into a dedicated module after the Windows daemon behavior settles.

## Deferred Daemon Safety Work

TODO: Add a small user config file, likely `~/.cache/parakit/config.toml`, for configurable hotkeys and other local daemon preferences. Candidate macOS fallbacks to evaluate there are right Command alone and right Command plus right Option. Keep the default behavior simple until config exists: Linux/Windows use `Ctrl+Space`, and macOS uses `Left Control+Space`.

TODO: Keep the direct platform-hotkey path available when configurable chords land. On macOS, extend the existing CoreGraphics event-tap backend so it owns the selected chord, suppresses only that chord, and handles tap-disabled callbacks. On Linux, keep the registered X11 backend as the normal path and use the evdev/uinput proxy only when users explicitly accept the lower-level permission tradeoff. On Windows, `RegisterHotKey` already gives a clear conflict/error boundary.

TODO: Add a Linux `doctor` warning for known IBus `Ctrl+Space` conflicts, or close this if configurable hotkeys make the warning unnecessary. Keep the current Linux docs warning until the default/config story changes.

TODO: Remove the Unix source-install dependency on the repository `target/` library tree in a dedicated follow-up PR. First try to work upstream with CrispASR for static/manual linking support; if that is not viable, revisit full vendoring or an aggregate static-link strategy in parakit. Do not patch the CrispASR submodule locally for this.

TODO: Add a secondary recording watchdog for missed key-release events from the registered X11 hotkey backend. The existing max-utterance timeout bounds the failure, but a silence-based stop would recover sooner when a backend misses release ordering.

TODO: Revisit Linux/macOS microphone idle policy after Windows CPU validation. Either move them to the same pause/resume default as Windows or keep 350 ms warm pre-roll only with measured idle CPU and first-syllable evidence that justifies the cost.

TODO: Replace fallback microphone device polling with platform event notifications when the audio layer is split: Windows `IMMNotificationClient`, PipeWire/PulseAudio registry events, and macOS `AudioObject` property listeners. Expose stream state, callback drops, and recovery counters through daemon status at the same time.

TODO: Evaluate callback-confirmed recording cues and a short post-roll window. The current cue fires after the start command succeeds, not after the first input callback, and release drains only already-arrived samples plus the resampler tail.

TODO: Upgrade `enigo` from the 0.2 line in a cross-platform validation branch. Linux batch paste uses X11 directly, Windows batch paste uses `SendInput`, and macOS batch paste uses CoreGraphics Cmd+V events that `doctor --deep` can smoke-test. Keep the dependency update focused on direct typing unless a real desktop paste regression points back to the fallback path.

TODO: Revisit model durability semantics before packaged releases. Source builds use the XDG-style `~/.cache/parakit/models/` path on Linux and macOS; a future bundle may want a less reclaimable app-data location.

TODO: Add an optional X11 paste inter-key hold only if real target applications miss the current XTest paste chord. The current smoke test covers X11 event delivery; app-specific compatibility should drive any delay so normal paste latency does not grow without evidence.

TODO: Benchmark an opt-in Windows MSVC `/GL` + `/LTCG` build after the CPU daemon is stable. MSVC does not have a direct `/O3`; keep the default at `/O2` unless link-time optimization shows a real transcription-speed win without disruptive build time or packaging side effects.

TODO: Add a Windows PE dependency-walker validation pass for the CPU bundle so release packaging verifies every transitive DLL dependency instead of relying only on build-time known runtime DLL names.

TODO: Run the full Windows BLAS/thread benchmark matrix for CPU builds, including no BLAS, OpenBLAS with controlled OpenMP ownership, and relevant `--threads` values against the pinned voice-memo smoke file. This is separate from upstream CrispASR issue #88 and remains open after the v0.6.6 pin.

TODO: Track upstream CrispASR [issue #88](https://github.com/CrispStrobe/CrispASR/issues/88) through long-dictation validation. The pinned v0.6.6 submodule includes the upstream NeMo parity fix for blank + duration-0 TDT decode retries, but the issue remains open because the maintainer does not expect that greedy-path parity change alone to explain tail truncation. This is not merge-blocking while Windows PTT smoke remains healthy; close the Parakit note only after rerunning the problematic long dictation against the pinned backend, or add a temporary Parakit diagnostic workaround only if the reproducer still drops tail speech.

## Updating [CrispASR](https://github.com/CrispStrobe/CrispASR)

Keep submodule updates separate from parakit code changes:

```bash
cd vendor/CrispASR
git fetch
git checkout <tag-or-commit>
cd ../..
git add vendor/CrispASR
cargo build
```
