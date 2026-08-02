# Development Notes

Source-coverage setup and report commands are in
[quality.md#rust-source-coverage](quality.md#rust-source-coverage).

## Model Artifacts

End-user download, cache, and model-override behavior is in
[running.md#model-cache](../running.md#model-cache).

Hosted release files:

| File | Role |
| --- | --- |
| `parakeet-tdt-0.6b-v3-Q8_0.gguf` | Default user artifact. |
| `parakeet-tdt-0.6b-v3-F16.gguf` | Source GGUF kept for maintainers and future re-quantization work. |

The CLI has no quant selector. Q8_0 is the default hosted model. Runtime model precedence is in [running.md#model-cache](../running.md#model-cache). Avoid unrelated names, nested directories, or model-card-only links for release artifacts.

The Parakeet converter, loader, and `crispasr-quantize` path are built around F16/F32 tensors. Treat BF16 as future work until it has explicit support and validation.

## Source Rebuild

Maintainers can rebuild from NVIDIA's `.nemo` checkpoint:

```bash
python -m pip install -r scripts/requirements-convert.txt
parakit fetch --from-source --keep-f16 --keep-nemo
```

That path downloads [NVIDIA's official `.nemo`](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3), converts it with `vendor/CrispASR/models/convert-parakeet-to-gguf.py`, and quantizes the intermediate GGUF with `crispasr-quantize`.

On Windows, the hosted Q8_0 path is the normal model setup. `fetch --from-source` requires a compatible `crispasr-quantize.exe` on `PATH` because bundled CPU builds skip the CrispASR examples tree under MSVC.

After rebuilding a release artifact, upload F16 and Q8_0 to the hosted repo.

GPU build and runtime checks are in [quality.md#gpu-feature-validation](quality.md#gpu-feature-validation).

## File Size Exceptions

Current Rust files over the approximate 1k LoC target:

| Path | Reason and split boundary |
| --- | --- |
| `src/daemon/audio/capture.rs` | Owns the coupled CPAL stream-recovery, SPSC drain, resampling, and pre-roll boundary. Split into stream, drain, and device modules when their ownership boundaries are stable. |
| `src/daemon/desktop/inject.rs` | Owns the cross-platform clipboard transaction and insertion contract. Split clipboard, X11 paste, and focus code without changing paste safety. |
| `src/daemon/ipc.rs` | Contains both Unix-socket and Windows named-pipe transports plus their policy tests. Extract the Windows transport after its behavior settles. |
| `src/daemon/macos/diagnostics.rs` | Owns the AppKit probe window and the two-stage macOS deep insertion check. Extract the probe-window harness if either diagnostic stage grows. |
| `src/daemon/worker.rs` | Coordinates ASR, cleaning, logging, recovery history, and insertion. Extract stable policy helpers without splitting the end-to-end worker state machine. |
| `src/app.rs` | Holds top-level command dispatch and daemon bootstrap. Extract command handlers when a stable subsystem boundary appears. |
| `src/cli.rs` | Keeps clap declarations, effective-option precedence, and parser tests together. Split declarations from resolution policy after the new command surface settles. |
| `src/daemon/desktop/hotkey.rs` | Is only slightly over the target and already delegates macOS code. Extract Linux backend implementations if it grows further. |
| `src/daemon/desktop/inject_tests.rs` | Keeps the clipboard and insertion transaction regression matrix together. Split by transaction phase when shared fixtures no longer dominate. |
| `src/daemon/macos/pasteboard.rs` | Owns the macOS paste-acknowledgement evidence policy: baseline capture, confirmation polling, transcript matching, and their regression tests, which dominate the count. Extract `TranscriptMatcher` and its tests into a sibling module if the evidence rules grow further. |
| `src/rules/tests.rs` | Keeps the cleaning pipeline regression matrix beside shared rule fixtures. Split by rule group when the shared setup no longer dominates the file. |

## Deferred Runtime Work

TODO: Config does not yet support remapping the actual PTT chord (e.g. a custom macOS fallback of right Command alone, or right Command plus right Option). Keep the default chords as-is until that lands: Linux/Windows use `Ctrl+Space`, and macOS uses `Left Control+Space`.

TODO: Keep the direct platform-hotkey path available when configurable chords land. On macOS, extend the existing CoreGraphics event-tap backend so it owns the selected chord, suppresses only that chord, and handles tap-disabled callbacks. On Linux, keep the registered X11 backend as the normal path and use the evdev/uinput proxy only when users explicitly accept the lower-level permission tradeoff. On Windows, `RegisterHotKey` already gives a clear conflict/error boundary.

TODO: Add a Linux `doctor` warning for known IBus `Ctrl+Space` conflicts, or close this if configurable hotkeys make the warning unnecessary. Keep the current Linux docs warning until the default/config story changes.

TODO: Remove the [Unix source-install dependency on the repository `target/` library tree](../build.md#install) in a dedicated follow-up PR. First try to work upstream with CrispASR for static/manual linking support; if that is not viable, revisit full vendoring or an aggregate static-link strategy in parakit. Do not patch the CrispASR submodule locally for this.

TODO: Add a secondary recording watchdog for missed key-release events from the registered X11 hotkey backend. The existing max-utterance timeout bounds the failure, but a silence-based stop would recover sooner when a backend misses release ordering.

TODO: Revisit Linux/macOS microphone idle policy. Either move them to the same pause/resume default as Windows or keep 350 ms warm pre-roll only with measured idle CPU and first-syllable evidence that justifies the cost.

TODO: Replace fallback microphone device polling with platform event notifications when the audio layer is split: Windows `IMMNotificationClient`, PipeWire/PulseAudio registry events, and macOS `AudioObject` property listeners. Expose stream state, callback drops, and recovery counters through daemon status at the same time.

TODO: Evaluate callback-confirmed recording cues and a short post-roll window. The current cue fires after the start command succeeds, not after the first input callback, and release drains only already-arrived samples plus the resampler tail.

TODO: Upgrade `enigo` from the 0.2 line in a cross-platform validation branch. Linux batch paste uses X11 directly, Windows batch paste uses `SendInput`, and macOS batch paste uses CoreGraphics Cmd+V events that `doctor --deep` can smoke-test. Keep the dependency update focused on direct typing unless a real desktop paste regression points back to the fallback path.

TODO: Revisit model durability semantics before packaged releases. Source builds use the XDG-style `~/.cache/parakit/models/` path on Linux and macOS; a future bundle may want a less reclaimable app-data location.

TODO: Add an optional X11 paste inter-key hold only if real target applications miss the current XTest paste chord. The current smoke test covers X11 event delivery; app-specific compatibility should drive any delay so normal paste latency does not grow without evidence.

TODO: Benchmark an opt-in Windows MSVC `/GL` + `/LTCG` build after the CPU daemon is stable. MSVC does not have a direct `/O3`; keep the default at `/O2` unless link-time optimization shows a real transcription-speed win without disruptive build time or packaging side effects.

TODO: Add a Windows PE dependency-walker validation pass for the CPU bundle so release packaging verifies every transitive DLL dependency instead of relying only on build-time known runtime DLL names.

TODO: Run the full Windows BLAS/thread benchmark matrix for CPU builds, including no BLAS, OpenBLAS with controlled OpenMP ownership, and relevant `--threads` values against the pinned voice-memo smoke file. This is separate from upstream CrispASR issue #88 and remains open after the v0.6.6 pin.

TODO: Re-run the problematic long dictation from CrispASR [issue #88](https://github.com/CrispStrobe/CrispASR/issues/88) against the pinned v0.6.6 backend. The pin includes the upstream NeMo parity fix for blank plus duration-0 TDT decode retries, and the upstream issue is closed. Remove this local note only after the original reproducer confirms that tail speech survives; add a temporary Parakit diagnostic workaround only if it still fails.

TODO: Add native Linux and Windows CI coverage for target-gated desktop transports, input backends, and their tests.

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
