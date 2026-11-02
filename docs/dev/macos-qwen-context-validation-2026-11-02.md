# PR #28 macOS validation, November 2

This pass tests `abe0c044cbb28385a1d45086a7ee81a98b865ad1`, with the committed CrispASR v0.8.49 revision `d94264379dada437aa4ea9163eb34ffe4554422a`. It covers the automated Mac requirements in [PR #28](https://github.com/pszemraj/parakit/pull/28#issuecomment-6564780267). Physical shortcut use and fresh human recognition checks remain separate.

## Environment and isolation

- Apple M5 MacBook Air, 32 GB, native arm64 macOS 26.7 (25G229), Metal 4.
- Default CPU and Metal-capable development builds, using optimized native Release libraries. GPU runs select Metal explicitly; native traces show the Apple M5 and model buffer allocations.
- All builds, models, configuration, sockets, logs, scratch files and Metal pipeline archives are under `target/`. No personal daemon was running at the initial check, and the installed binary and configuration were not changed. All validation processes were stopped afterward.
- The authorized Juniper memo is unchanged, SHA256 `d79d7f729b192642b91aef5e1486b97c1cb7f4c34f6ce4a0d45199a9ff23ab5f`. Public and constructed fixtures come from `local-scratch/laptop-fixtures.zip`, retaining its attribution README.

Raw command logs and temporary runners are in ignored `target/tmp/pr28/`; these artifacts are local to this checkout. Microphone PCM stays in memory; reports retain text and signal/timing measurements.

## Results

| Check | Result |
| --- | --- |
| M1: native builds and tests | Default and Metal suites each passed 530 tests, with seven opt-in tests ignored in those routine runs. The model/hardware cases were run separately. Package formatting, strict rustdoc, workspace/all-target checks, strict Clippy and builds passed. macOS modifier tests ran natively. |
| Rust all-features surface | Passed using the documented matching-library fallback. Raw native configuration failed with `CUDA Toolkit not found`; this does not qualify CUDA or Vulkan execution. |
| M2: Juniper transcription | Parakeet Ultra (the application default), Qwen 0.6B Q8_0 and original Parakeet v3 Q8_0 each passed CPU/Metal helper-versus-worker transcript parity. All six quiet worker runs emitted zero stdout/stderr bytes. Original v3 matched the pinned reference case-insensitively. |
| M3: loaded native Qwen faults | All 24 driver cases passed on Metal and all 24 on CPU. Checks traced exact chunked context and language prompts, success/error cleanup, copied-context retry, ordinary fallback, short/silent input, worker history, and matching verbose/JSONL telemetry. Each run used a private instrumented native library; production libraries and vendor sources were unchanged. |
| Resampling | Qwen CPU/Metal helper and worker results matched on the public short clip and its 44.1 kHz mono, 48 kHz stereo, full-block and partial-block variants. These are constructed conversion controls, not independent native-rate microphone recordings. |
| Simulated worker reload | Qwen CPU/Metal each passed two captures with timed idle offloads and a reload. Transcripts were unchanged, with forced-English options retained. |
| Native diagnostics | `doctor --deep` passed standard and terminal guarded paste transactions and the direct-mode suppressed event probe. Accessibility, Input Monitoring and microphone preflight were granted in this launch context. |
| Real microphone retention | The production CoreAudio/Rubato path retained finite PCM within duration bounds for 300 ms and three-second captures, both immediately and after ten seconds idle. |
| Acoustic control | The existing speaker/microphone test passed with Parakeet Ultra on Metal, recognizing first/second/first public phrases. Cue-frequency energy was present for reload, start, success and error tones. This measures emission, not subjective audibility. |
| Real microphone and Qwen worker | CPU/Metal each passed loaded and post-offload captures through CoreAudio, Rubato, the recording coordinator and the worker. Capture was observed during reload; history grew once per capture and preserved its previous entry. Both transcripts and opening/closing words matched within each run. Insertion was disabled. |
| Normal daemon lifecycle | Two Qwen Metal start/status/history/duplicate-start/stop cycles passed. Status observed a genuine idle offload without reloading it. Restart reused the saved preset, Qwen options and history limit; configuration and `config show` matched byte-for-byte. History started empty, the original PID survived duplicate start, and each stop removed the socket and exited cleanly. |
| CLI and cache | Normal model acquisition and cached readback passed. Configuration output exposes all context defaults; quiet rule testing is silent; missing-model startup exits 1 with empty stdout and the expected path error. Normal changed-transcript output retained both Raw/Clean lines and matched rules-CLI cleaning; Parakeet logs omitted Qwen context fields. |

The existing vendored wrapper deprecations, its unused `mut` warning, and the `block` 0.1.6 future-compatibility diagnostic remain dependency warnings. Strict package Clippy passed; vendor sources were not patched to silence them.

The first acoustic control produced empty transcripts: the source fixtures peaked below 0.006 full scale, and captured speech did not rise above background. The unchanged test passed using repository-local playback copies scaled to a 0.7 peak. Source hashes, gains and both reports are retained; original WAVs and system volume were unchanged.

## Reproduction and remaining checks

Use the [isolated native setup](model-validation.md#macos-cpu-and-metal), [loaded Qwen driver](quality.md#ordered-audio-and-loaded-qwen-failures), [worker simulation](quality.md#ptt-worker-simulation), and [native microphone procedure](quality.md#native-microphone-worker). Use a fresh output directory for each fault-driver or microphone report run.

PR items M4-M8 still need physical held-key dictation: both shortcuts, TextEdit Space suppression, Option release while speaking, rejected right-side modifiers, and context disabled or a non-Qwen model. The macOS tap samples physical HID modifier state; synthetic keys and WAV simulation do not establish these behaviors. M9 needs two configured input sources and observation of the macOS shortcut conflict.

Reload finished before release in both acoustic worker runs, so release before native microphone reload readiness remains unobserved. Physical sleep/wake, microphone device changes, natural quiet/noisy-room speech, subjective cue audibility, and physical stop-to-insert latency remain unverified. Permissions must also be checked from the intended terminal or launcher. This focused pass does not repeat the six-preset/two-precision matrix or establish Windows behavior or a general context accuracy benefit.
