# PR #25 macOS validation

The [October 29 follow-up](macos-ibus-followup-2026-10-29.md) completes the unattended daemon lifecycle, native microphone pipeline and stock-daemon insertion checks after the installed instance was stopped.

The initial automated macOS follow-up for PR #25 passed after correcting a microphone failure/Stop race in `0b9ce64`. During this pass the installed daemon stayed running at the maintainer's request; physical PTT, device-change and application-specific TextEdit/terminal checks remained pending.

## Environment and scope

- PR baseline: `1a63fab`; audio correction: `0b9ce64`.
- Apple M5 MacBook Air, 32 GB, native arm64 macOS 26.7 (25G229).
- Default CPU and Metal builds, plus an optimized Metal-capable executable and matching WAV helper. The release executable links the freshly built native Release libraries through `CRISPASR_LIB_DIR`.
- Parakeet v3 Q8_0; the pinned 55.36-second Juniper memo, SHA256 `d79d7f729b192642b91aef5e1486b97c1cb7f4c34f6ce4a0d45199a9ff23ab5f`. This pass does not repeat the six-model/precision matrix.
- Build output, models, configuration, sockets, logs and Metal pipeline cache were isolated under `target/`. The installed daemon retained its original PID, start time and command. No hotkey daemon was started or stopped for these checks.
- Processes launched by these checks inherited Codex's permission context. Permission results do not establish effective access from the maintainer's terminal.

Raw evidence and temporary harness source are in the ignored local directory `target/tmp/pr25-mac/`; they are not distributed with the checkout. Original clipboard payloads stayed in memory and were not logged. The native microphone check saved no audio.

## Results

| Requirement | Result |
| --- | --- |
| Formatting, strict rustdoc, workspace/all-target checks, tests, strict Clippy and builds | Passed with default and Metal features. Each final test suite passed 521 tests with three opt-in tests ignored. The native microphone test was run separately and passed. |
| Rust all-features surface | Passed with the documented matching-library fallback. Raw native configuration failed at the expected missing CUDA Toolkit; this is not CUDA/Vulkan runtime validation. |
| `doctor --deep` | Standard and terminal modes passed supported-format clipboard transactions; direct mode passed its suppressed key-event probe. Accessibility, Input Monitoring and microphone preflight were granted. |
| Supported clipboard restoration | Eight native cases passed: text, verbatim HTML plus alternative text, distinct image pixels, and ordered file URLs, each with standard and terminal paste modes. Complete supported-payload readback matched. The outer harness also verified restoration of the original native clipboard representations. |
| AX acknowledgement and final focus guards | All 20 owned-AppKit cases passed through the production injector, focus snapshots and clipboard transaction. Cases covered `ab`/`aba`, `tes`/`test`, combining Unicode, existing repeated text, a long wrapped field and selection replacement. Field/window changes at the final guard withheld the chord and left both receivers unchanged. |
| Unverified and newer-copy paths | An explicit absent AX context used the approximately 1.5-second unverified grace, landed the text and restored the previous payload. Synthetic newer HTML with alternative text survived acknowledgement. These cases do not establish behavior for every application's inaccessible AX implementation. |
| Pinned WAV worker and reloads | CPU and Metal each passed three dictations, three timed idle offloads and two reloads. All transcripts matched exactly, with one transcript record and one correlated insertion-skipped record per dictation. |
| Release executable and WAV helper | CPU and Metal worker/helper transcripts matched exactly. Quiet worker runs produced zero stdout and stderr bytes. Ordinary baseline worker runs also had empty stderr. |
| Native Metal execution | A temporary forwarding dispatch shim observed successful model matrix-multiply graphs on the Metal `MTL0` backend during the release worker run. The transcript matched the uninstrumented run. This establishes graph dispatch, not kernel timing or exclusive GPU execution. |
| Live microphone retention | The production capture manager passed 300 ms and three-second captures, both without an added idle wait and after ten seconds idle. All returned finite PCM within the duration bounds. Start/Stop latency, process CPU and context switches were recorded; these are development-profile component observations, not wakeup or power measurements. |
| CLI contracts | Cached fetch/readback passed; quiet rule testing was silent; a missing model exited 1 with empty stdout and the expected file-path error. Mach-O run paths resolved the matching isolated native libraries. |

Owned-window AX acknowledgements took 40-47 ms in the measured matrix; the explicit absent-context cases took about 1500 ms. The harness event pump differs from an ordinary target application, so its CPU use does not measure daemon polling cost. No polling interval or grace period changed.

The existing vendored CrispASR deprecation warnings and `block` 0.1.6 future-compatibility diagnostic remain. Strict package Clippy passed without new findings.

## Microphone race correction

A callback error can arrive after the manager's error check but before it receives Stop. The unavailable-control path previously cleared the recording epoch and returned the buffer while the surviving drain still owned queued PCM and resampler state. Those remaining samples could be lost.

Stop now flushes the surviving drain before returning the recording. When the stream has already been torn down, it still returns the retained buffer directly. New captures remain rejected while a callback error is pending.

`stop_after_callback_error_retains_queued_pcm_and_resampler_tail` uses real ring/drain threads at 16 kHz and 48 kHz and checks exact PCM and final recording state. It failed against the old behavior in an isolated source copy and passed with the correction. The optional native count check can be repeated with:

```sh
cargo test --features metal --bin parakit \
  microphone_capture_retains_samples_after_idle -- \
  --ignored --nocapture --test-threads=1
```

Use the [native isolation setup](model-validation.md#macos-cpu-and-metal) before repeating hardware checks.

## Remaining native checks

- Physical PTT speech onset, held-through-reload and release-before-readiness trials, audible cue ordering, OS input changes and microphone disconnect/reconnect remain pending. The installed daemon was left running as requested; no competing hotkey daemon was launched.
- Actual TextEdit and terminal insertion, rendered terminal scrolling, and field changes during a physical modifier wait remain pending. The TextEdit adapter required the frontmost bundle and a marked disposable document; it refused both attempts because the OS did not report TextEdit as frontmost. It sent no input and the clipboard was restored. The synthetic document was saved under the isolated evidence directory and closed.
- Native forced cleanup-retry failure, application-specific no-AX behavior, notification observation, and stock acknowledgement CPU/wakeup measurements remain unverified. Existing model-free regressions cover the corresponding policies; no timing constants were changed.
- Normal daemon lifecycle/restart checks were not repeated alongside the installed daemon. WAV worker restarts and timed model reloads do not establish that lifecycle or physical desktop behavior.

The [macOS acknowledgement semantics](../macos-desktop.md#what-counts-as-evidence) and [quality procedures](quality.md) describe the acceptance boundaries.
