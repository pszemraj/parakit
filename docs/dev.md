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

## File Size Exceptions

`src/daemon/audio_manager.rs` is temporarily over the 1k LoC target because it owns one tightly coupled runtime boundary: CPAL stream recovery, the SPSC drain thread, resampler flushing, and recording/pre-roll state. Split it after Windows CPU settles into smaller `audio/stream.rs`, `audio/drain.rs`, and `audio/device.rs` modules without changing behavior.

`src/daemon/inject.rs` is also temporarily over the target while clipboard transaction, X11 paste-chord cleanup, focus snapshots, and smoke-test support settle. Split it into focused clipboard, X11 paste, and focus modules without changing the paste safety contract.

`src/daemon/ipc.rs` is temporarily over the target because it owns both Unix socket IPC and Windows named-pipe IPC, including Windows ACL setup and retry policy tests. Split the Windows named-pipe transport into a dedicated module after the Windows daemon behavior settles.

## Deferred Daemon Safety Work

TODO: Move the default hotkey away from `Ctrl+Space` or make it configurable, then add a Linux `doctor` warning for known IBus `Ctrl+Space` conflicts. Keep the current docs warning until the default/config story changes.

TODO: Add a secondary recording watchdog for missed key-release events from the registered X11 hotkey backend. The existing max-utterance timeout bounds the failure, but a silence-based stop would recover sooner when a backend misses release ordering.

TODO: Revisit Linux/macOS microphone idle policy after Windows CPU validation. Either move them to the same pause/resume default as Windows or keep 350 ms warm pre-roll only with measured idle CPU and first-syllable evidence that justifies the cost.

TODO: Replace fallback microphone device polling with platform event notifications when the audio layer is split: Windows `IMMNotificationClient`, PipeWire/PulseAudio registry events, and macOS `AudioObject` property listeners. Expose stream state, callback drops, and recovery counters through daemon status at the same time.

TODO: Evaluate callback-confirmed recording cues and a short post-roll window. The current cue fires after the start command succeeds, not after the first input callback, and release drains only already-arrived samples plus the resampler tail.

TODO: Upgrade `enigo` from the 0.2 line in a cross-platform validation branch. Linux batch paste uses X11 directly and Windows batch paste uses `SendInput`, but direct mode and macOS paste shortcuts still need real desktop validation after the dependency update.

TODO: In the dedicated macOS work branch, decide whether default model storage should move from `~/Library/Caches/parakit/models/` to `~/Library/Application Support/parakit/models/`. The current cache path is user-scoped and adminless, but a multi-GB model may fit durable app data semantics better than reclaimable cache semantics.

TODO: Add an optional X11 paste inter-key hold only if real target applications miss the current XTest paste chord. The current smoke test covers X11 event delivery; app-specific compatibility should drive any delay so normal paste latency does not grow without evidence.

TODO: Benchmark an opt-in Windows MSVC `/GL` + `/LTCG` build after the CPU daemon is stable. MSVC does not have a direct `/O3`; keep the default at `/O2` unless link-time optimization shows a real transcription-speed win without disruptive build time or packaging side effects.

TODO: Add a Windows PE dependency-walker validation pass for the CPU bundle so release packaging verifies every transitive DLL dependency instead of relying only on build-time known runtime DLL names.

TODO: Run the full Windows BLAS/thread benchmark matrix for CPU builds, including no BLAS, OpenBLAS with controlled OpenMP ownership, and relevant `--threads` values against the pinned voice-memo smoke file.

TODO: Track upstream CrispASR [issue #88](https://github.com/CrispStrobe/CrispASR/issues/88) for Parakeet TDT greedy decode handling of blank + duration-0 predictions. Keep this upstream unless Windows validation shows Parakit needs a temporary diagnostic tail-padding workaround for long dictations before the backend fix lands.

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
