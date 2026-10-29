# PR #25 unattended macOS follow-up

With the installed daemon stopped, the remaining unattended lifecycle, microphone/coordinator/worker, and stock-daemon insertion checks passed. This pass established no additional production defect. Commit `e5f7d3c` adds an opt-in native pipeline test; the tested application runtime is unchanged from `303bc7f`.

The host was the same native arm64 Apple M5 MacBook Air with macOS 26.7 used in the [initial report](macos-ibus-validation-2026-10-28.md). All test configuration, models, logs, build output and control sockets remained under `target/`. The installed binary and Parakit configuration were not changed. All isolated daemons and playback processes were stopped after testing.

## Completed checks

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

Raw reports, temporary receiver source, command logs and failed diagnostic trials are in the ignored local directory `target/tmp/pr25-mac/followup/`; these artifacts are not distributed with the checkout. Microphone PCM remained in memory. Logs contain transcripts and signal/timing measurements, not recorded audio. Native clipboard backups stayed in memory and were restored only while the test still owned the current payload.

## Acoustic test boundaries

The new test sends logical PTT press/release transitions to the production coordinator. It does not synthesize physical HID state or weaken the macOS hotkey checks. Output is predecoded and its Rodio stream is opened before the press, avoiding process-launch and output-device startup in the onset interval. Hardware playback onset itself is not timestamped.

The final source was the authorized Juniper memo, trimmed from 0.65 to 5.1 seconds to end at a natural sentence pause, with a level-adjusted playback copy. The original memo was left unchanged. Initial tests using `afplay` and a 12-second clip varied in their recognized words; a main-branch control also varied. The clip ended mid-word, while process startup and the original leading silence could delay speech until after a fast reload. Those failures remain in the evidence. Final calibrated captures matched the reference, but this is not a general recognition-accuracy result.

Pipeline checks always gate the new test. Exact opening/closing-word comparison is reported separately; setting `PARAKIT_LIVE_REQUIRE_MATCHING_EDGES` also makes it a test failure. This keeps acoustic variability visible without treating word equality as deterministic proof of PCM ordering. The existing blocked-reload regressions provide that exact-PCM check.

Default input/output devices and mute state matched the initial readback. Output volume was observed at approximately 0.375 initially and 0.5625 at cleanup; no harness code changed it, and the current level was left alone. Its change time was not observed, so the acoustic trials are not controlled volume or power comparisons.

## Repeat the native worker test

Use the [native isolation setup](model-validation.md#macos-cpu-and-metal), a model and WAV inside the checkout, and a fresh report destination. The report file must exist; its sibling `.logs` directory must not already exist.

```sh
mkdir -p target/tmp/native-worker
printf '{}\n' > target/tmp/native-worker/cpu.json
export PARAKIT_LIVE_MODEL="$PWD/target/tmp/pr25-mac/models/parakeet-tdt-0.6b-v3-Q8_0.gguf"
export PARAKIT_LIVE_WAV="$PWD/target/tmp/pr25-mac/followup/juniper-complete-sentence-playback.wav"
export PARAKIT_LIVE_REPORT="$PWD/target/tmp/native-worker/cpu.json"
export PARAKIT_LIVE_DEVICE=cpu
export PARAKIT_LIVE_CUES=1
cargo test --features metal --bin parakit \
  microphone_coordinator_worker_reloads_after_idle -- \
  --ignored --nocapture --test-threads=1
```

Select `gpu` and a fresh report path for Metal. `PARAKIT_LIVE_SECOND_WAV` optionally adds another loaded/offloaded pair. Use an audible, complete utterance and inspect the recorded transcripts and speech-boundary result. The fixture paths above refer to local ignored artifacts; supply equivalent repository-local files in another checkout.

## Still unverified

- Physical held-key PTT, physical modifier release/focus changes, and native release-before-reload-readiness. Reload finished before release in every final acoustic trial; the report explicitly marks early-release coverage false. Model-free tests continue to cover blocked reload and early release ordering.
- Hardware input changes, microphone unplug/replug, and sleep/wake.
- Actual TextEdit/terminal application behavior, including sliding terminal AX values. The stock-daemon checks used owned AppKit fields; the earlier unsafe TextEdit activation attempt remains unqualified.
- Subjective cue audibility, natural quiet/noisy-room speech, and stock CPU/wakeup or physical stop-to-insert latency measurements.

