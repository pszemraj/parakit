# PR #25 macOS validation, October 28-29

The initial automated checks passed after correcting a microphone failure/Stop race in `0b9ce64`. The October 29 follow-up completed unattended daemon lifecycle, native microphone pipeline and stock-daemon insertion checks. It established no additional production defect. Commit `e5f7d3c` adds the opt-in native pipeline test; its tested application runtime is unchanged from `303bc7f`.

## Environment and scope

- PR baseline: `1a63fab`; audio correction: `0b9ce64`.
- Apple M5 MacBook Air, 32 GB, native arm64 macOS 26.7 (25G229).
- Default CPU and Metal builds, plus an optimized Metal-capable executable and matching WAV helper. The release executable links the freshly built native Release libraries through `CRISPASR_LIB_DIR`.
- Parakeet v3 Q8_0; the pinned 55.36-second Juniper memo, SHA256 `d79d7f729b192642b91aef5e1486b97c1cb7f4c34f6ce4a0d45199a9ff23ab5f`. This pass does not repeat the six-model/precision matrix.
- Build output, models, configuration, sockets, logs and Metal pipeline cache were isolated under `target/`. During the initial pass, the installed daemon retained its original PID, start time and command; no hotkey daemon was started or stopped. The October 29 follow-up ran after the installed instance was stopped. The installed binary and Parakit configuration were not changed. All isolated daemons and playback processes were stopped afterward.
- Processes launched by these checks inherited Codex's permission context. Permission results do not establish effective access from the maintainer's terminal; run `parakit doctor` from the intended launcher to verify it separately.

Raw evidence and temporary harness source are in ignored `target/tmp/pr25-mac/`, with follow-up reports, command logs and failed diagnostic trials under its `followup/` directory; these artifacts are not distributed with the checkout. Microphone PCM and native clipboard backups stayed in memory. Logs contain transcripts and signal/timing measurements, not audio or clipboard payloads. Clipboard backups were restored only while the test still owned the current payload.

## Initial results, October 28

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

The correction made Stop flush the surviving drain before returning the recording. When the stream had already been torn down, it returned the retained buffer directly. New captures remained rejected while a callback error was pending.

`stop_after_callback_error_retains_queued_pcm_and_resampler_tail` uses real ring/drain threads at 16 kHz and 48 kHz and checks exact PCM and final recording state. It failed against the old behavior in an isolated source copy and passed with the correction. The optional native count check can be repeated with:

```sh
cargo test --features metal --bin parakit \
  microphone_capture_retains_samples_after_idle -- \
  --ignored --nocapture --test-threads=1
```

Use the [native isolation setup](model-validation.md#macos-cpu-and-metal) before repeating hardware checks.

## Follow-up results, October 29

| Check | Result |
| --- | --- |
| Normal CLI startup and idle offload | Stock CPU and Metal daemons reached ready state, served status/history queries, and reported `offloaded` after a real one-minute idle interval. Status queries did not reload the model. |
| Duplicate startup | A second start retained the original PID and settings. Supplying different startup options produced the expected restart guidance and left the running daemon unchanged. |
| Stop and restart | Normal `stop` exited successfully and removed the socket. Restart without runtime overrides applied the same effective configuration; the config file and `config show` output matched byte-for-byte. History started empty, as intended. |
| Stock daemon insertion | All 27 owned-AppKit receiver cases passed across CPU/Metal and standard/terminal modes. They used the normal `parakit test-paste` client, with exact text and AX readbacks, CLI outcome checks and clipboard restoration. Cases covered overlaps, combining Unicode, repeated text, a long wrapped field and selection replacement. Three mode/backend runs also inserted a transcript obtained from the real microphone pipeline. |
| Real microphone pipeline | CPU and Metal each passed loaded-model and post-offload captures through production CPAL capture, Rubato resampling, the recording coordinator and the ASR worker. Capture was observed during model loading. Each capture produced exactly one history entry, preserving the older entry. All four final transcripts were `Um so actually to add to the previous one.` |
| Model reload | Each native pipeline test waited for an actual one-minute offload, began capture from `offloaded`, and finished loaded without a model error or an active recording. |
| Audio cues | The existing acoustic smoke test passed. Microphone measurements showed elevated energy at the reload, start, success and error cue frequencies compared with the baseline. This establishes emission; subjective sound quality was not assessed. |
| Rust validation | Formatting, strict rustdoc, workspace/all-target checks, strict Clippy, default/Metal builds and both test suites passed. Each routine suite passed 521 tests with four opt-in tests ignored. The native worker tests and existing acoustic smoke were run explicitly. |
| All-features and pinned WAV | The Rust all-features check passed against the matching native Metal libraries, using the documented fallback for the absent CUDA Toolkit. The rebuilt daemon's pinned Juniper worker run passed with zero quiet stdout/stderr bytes. |

### Acoustic test boundaries

The new test sends logical PTT press/release transitions to the production coordinator. It does not synthesize physical HID state or weaken the macOS hotkey checks. Output is predecoded and its Rodio stream is opened before the press, avoiding process-launch and output-device startup in the onset interval. Hardware playback onset itself is not timestamped.

The final source was the authorized Juniper memo, trimmed from 0.65 to 5.1 seconds to end at a natural sentence pause, with a level-adjusted playback copy. The original memo was left unchanged. Initial tests using `afplay` and a 12-second clip varied in their recognized words; a main-branch control also varied. The clip ended mid-word, while process startup and the original leading silence could delay speech until after a fast reload. Those failures remain in the evidence. Final calibrated captures matched the reference, but this is not a general recognition-accuracy result.

Pipeline checks always gate the new test. Exact opening/closing-word comparison is reported separately; setting `PARAKIT_LIVE_REQUIRE_MATCHING_EDGES` also makes it a test failure. This keeps acoustic variability visible without treating word equality as deterministic proof of PCM ordering. The existing blocked-reload regressions provide that exact-PCM check.

Default input/output devices and mute state matched the initial readback. Output volume was observed at approximately 0.375 initially and 0.5625 at cleanup; no harness code changed it, and the current level was left alone. Its change time was not observed, so the acoustic trials are not controlled volume or power comparisons.

Follow the [native microphone worker procedure](quality.md#native-microphone-worker) to repeat the test. The [macOS acknowledgement semantics](../macos-desktop.md#what-counts-as-evidence) describe insertion evidence.

## Remaining native checks after the follow-up

- Physical held-key PTT, speech onset, physical modifier release/focus changes, and native release-before-reload-readiness remain unverified. Reload finished before release in every final acoustic trial; the report explicitly marks early-release coverage false. Model-free tests cover blocked reload and early release ordering.
- Hardware input changes, microphone unplug/replug, and sleep/wake remain pending.
- Actual TextEdit/terminal behavior, rendered terminal scrolling, and field changes during a physical modifier wait remain pending. The TextEdit adapter required the frontmost bundle and a marked disposable document; it refused both initial attempts because the OS did not report TextEdit as frontmost. It sent no input and the clipboard was restored. The synthetic document was saved under the isolated evidence directory and closed. The stock-daemon insertion checks used owned AppKit fields.
- Native forced cleanup-retry failure, application-specific no-AX behavior, notification observation, and stock acknowledgement CPU/wakeup measurements remain unverified. Existing model-free regressions cover the corresponding policies; no timing constants were changed.
- Subjective cue audibility and ordering, natural quiet/noisy-room speech, and physical stop-to-insert latency remain unverified.
