# macOS model validation: 2026-10-18

Automated CPU and Metal validation covers all six presets at Q8_0 and Q6_K. Follow-up checks exercise real microphone capture and native paste separately. Physical PTT timing, system sleep/wake, and fresh human listening remain pending. These results do not close the Windows checklist or establish recognition accuracy.

## Build and isolation

- Baseline: `b4b9ab812053fc58138de3a7d2ff9e4c1da0f51d`, PR #21. Reload correction: `a7e3c6cd1b8b9b10e0c459394a3fb17351106090`.
- Host: Apple M5 MacBook Air, 10 cores, 32 GiB unified memory, macOS 26.7 (25G229).
- Toolchain: Rust 1.98.1, CMake 4.4.4; optimized native code and release Rust binaries; six inference threads.
- CrispASR v0.8.42: `3be8bcb4390b8080dc73f807b7abb34dc0aca9c3`; ggml: `c36dab89b662838f0f5d4826c399198c0b90bbfc`.

The full matrix used the baseline. The correction was checked with both test suites, rebuilt CPU/Metal binaries, focused native reload regressions, Qwen corpus recapture, and default v3 memo replay.

Builds, models, configuration, logs, control paths, temporary files and the Metal pipeline cache were isolated inside the checkout. The installed daemon remained running throughout the initial matrix. The follow-up below used private daemons after the user stopped that instance. Loaded-library traces confirmed the matching isolated CrispASR/ggml libraries; the separate CPU build loaded no Metal backend. Graph-dispatch traces confirmed substantial CPU or Metal model graphs for every requested matrix configuration, excluding small device-capability probes.

Formatting, strict rustdoc, workspace/all-target checks, tests, Clippy with warnings denied, CPU/Metal release builds, and the Rust all-features fallback passed. The corrected source passes 471 tests in each default and Metal suite; native tests are ignored by the default suite and run explicitly in the follow-up below. All five resampler-filtered tests passed.

Raw all-features validation failed at the documented absent CUDA Toolkit; `CRISPASR_LIB_DIR` pointing to the built Metal libraries passed the Rust surface check. Vendored deprecated-wrapper warnings and the existing `block` future-incompatibility diagnostic remain. Optional OpenMP was unavailable; CPU inference used Apple Accelerate. No vendor, CI, installed binary or README changes were needed.

## Automated behavior

| Preset | CPU Q8_0 / Q6_K | Metal Q8_0 / Q6_K |
| --- | --- | --- |
| parakeet-v2 | Pass / Pass | Pass / Pass |
| parakeet-v3 | Pass / Pass | Pass / Pass |
| parakeet-ultra | Pass / Pass | Pass / Pass |
| parakeet-redux | Pass / Pass | Pass / Pass |
| qwen3-asr-0.6b | Pass / Pass | Pass / Pass |
| qwen3-asr-1.7b | Pass / Pass | Pass / Pass |

Each configuration ran repeated WAV-helper inference, the production simulated-PTT worker, silence/noise controls, and a real timed offload/reload with a one-minute idle setting and 63-second waits. Readback checked transcripts, successful JSONL records, selected preset/precision/device, skipped insertion, resident/offloaded state, and history preservation on failure. File/worker output and repeated/native-stereo output matched within each configuration.

Inputs included the public fixture archive, constructed 44.1 kHz mono and 48 kHz stereo/full-block/partial-block controls, and the pinned 55.36-second native 48 kHz mono memo. The memo SHA256 was `d79d7f729b192642b91aef5e1486b97c1cb7f4c34f6ce4a0d45199a9ff23ab5f`. Duplicated stereo channels and an appended partial block exercised native-rate downmix/tail handling. The 92-word v3 memo baseline matched at both precisions on CPU and Metal after case/punctuation normalization, including the final default-preset replay. Constructed resampling controls are not independent recordings or microphone callback validation.

Both Qwen sizes and precisions on CPU/Metal additionally passed a short/270-second/short inference sequence with forced English and a 4096-token budget, option-preserving idle reload, 29/30/31-second boundaries, forced German and Chinese, and speech containing the word language. Budget 1 completed readiness but failed actual dictation without successful history/JSONL output. Default auto/512 and four controlled quiet/noise-tail diagnostics were checked. Repeated JFK speech stresses context and memory; it is not natural long-form quality evidence.

Cold acquisition covered all twelve artifacts, including F16-to-Q6_K quantization. Cached Q6_K fetch, helper and worker startup succeeded for all presets with network access denied, PATH empty, and local F16 files and quantizers hidden. Two presets regenerated Q6_K at worker startup from retained F16; regenerated hashes matched. Saved preset/precision and Qwen options survived two fresh worker processes with unchanged configuration files. This does not substitute for physical daemon restart and insertion tests.

Quiet-mode worker checks produced empty stdout/stderr while recording successful JSONL state. Silence and synthetic noise produced no successful transcript or insertion and preserved the previous history entry. Missing-model and CLI/rules smoke checks also passed.

Ten English FLEURS clips were transcribed across all 24 configurations. Dataset reading-text comparisons and raw predictions are retained locally as diagnostic evidence; they were not freshly adjudicated and do not qualify model accuracy. Synthetic noise/quiet controls do not establish real room-noise behavior. At the time of these runs, Qwen was experimental, Redux was testing-only, and Parakeet v3 Q8_0 was the default.

## Metal reload correction

The old Qwen requirement, one model file plus 2 GiB, was below observed Metal allocation. The corrected [reload requirement](../running.md#idle-model-offload) accounts for the loader's [two Metal buffer views](memory.md#metal-qwen-reload-accounting). CUDA/Vulkan and Parakeet policies were unchanged.

The table summarizes profiled Qwen GPU short/long/reload runs, including the final large-model memo replay. Metal samples were taken at nominal 50 ms; host RSS was sampled separately at nominal 200 ms. These accounting domains overlap on unified memory and must not be added. Peaks are sampled observations, not allocation bounds.

| Qwen | File MiB | Peak Metal MiB | Revised requirement MiB | Margin MiB | Sampled RSS MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| 0.6B Q8_0 | 960.2 | 3231.5 | 3968.3 | 736.9 | 1305.9 |
| 0.6B Q6_K | 786.6 | 2857.2 | 3621.2 | 764.0 | 1107.6 |
| 1.7B Q8_0 | 2390.6 | 6267.0 | 6829.2 | 562.2 | 2867.8 |
| 1.7B Q6_K | 1921.4 | 5220.0 | 5890.7 | 670.7 | 2347.5 |

Qwen Metal allocation returned below 1 MiB after measured timed offloads. The new requirement covers the observed profiled short, long-input and reload peaks for these artifacts; it does not reserve memory or guarantee safety for other inputs, overrides or hardware.

A test-only native callback wrapper reported 5 GiB free without filling memory. The baseline accepted a large Qwen GPU reload under that report. The correction moved automatic reload to CPU with options retained and CPU graph execution confirmed; explicit GPU reload failed with history unchanged. A smaller Qwen Q6_K model still reloaded on Metal under the same report. With the real ample budget, large Qwen reload and memo replay passed; the default v3 quiet memo check also passed. This tests the policy branches, not actual memory pressure or allocation-failure recovery.

## Evidence and remaining checks

Ignored evidence is under `target/tmp/new-models/macos/`: command/exit records in `commands.jsonl`, state readback in `audit.json`, transcripts and sampled allocations beside each case, `execution-summary.json`, `memory-summary.json`, fixture hashes/attribution, and `toolchain-fixed/`. Private audio/transcripts are not included in this report. A pre-open failure caused by an early observer revision was retained separately and superseded by a passing run with corrected instrumentation; it was not a model failure. An output-filename collision in the temporary corpus recorder was corrected, and all eight affected Qwen cases were recaptured with distinct evidence files.

## Native follow-up

The native fault driver at this revision supported macOS library loading and explicit CPU/GPU selection. All 112 commands passed across both Qwen sizes, both precisions, and CPU/Metal: first/middle/last chunk failures and greedy embedding/forward failures on file and worker paths, three native language prefixes, and failed-readiness restoration. All eight readiness failures restored the actual native English hint and one-token budget. Failed worker dictations preserved the prior successful history and produced no successful transcript/insertion record. The normal libraries and vendor sources were not modified by fault injection.

Original-checkpoint rebuilding covers Qwen 0.6B/1.7B, Ultra, Redux, and the legacy Parakeet v3 `.nemo` path. Each cold rebuild produced F16 and Q8_0 artifacts in an isolated cache. Both precisions were exercised on CPU/Metal through the WAV helper and simulated production worker, including short speech, a negation near a 31-second boundary, and silence. File and worker transcripts matched within all 20 artifact/device cases; all positive transcripts preserved the public fixture's negation. Named-preset startup selected the rebuilt Q8_0, and completed source fetches reused it with network denied and PATH empty. These controls do not replace official-framework or fresh human quality comparisons.

A forced rebuild from the downloaded local Qwen 0.6B snapshot preserved every supplied input file. Injected converter and quantizer failures preserved the completed Q8_0 hash and removed partial outputs. The original quantizer was restored. With Q8_0 temporarily absent, retained F16 regenerated the same Q8_0 hash with network denied and PATH empty. A later cached fetch removed F16 when retention was not requested. Unsupported v2 source conversion failed as expected. All six transaction cases passed.

Converter dependencies were installed into an ignored repository directory using the existing Python interpreter; its environment was unchanged. Qwen's converter emitted a NumPy `log(0)` warning from the unused logarithmic branch of its zero-Hz mel calculation. The four resulting F16/Q8_0 mel tensors were finite and exactly equal to the Librosa Slaney filterbank after matching layout. No vendor patch was needed.

An opt-in hardware smoke test opens the production `AudioCapture`, records speaker WAV playback through CPAL and the shared Rubato resampler, and passes the resulting 16-kHz PCM directly to `Engine`. The built-in input opened at 48 kHz mono F32. Only text and signal measurements are retained; microphone PCM stays in memory. This drives capture start/stop directly and does not exercise the hotkey/coordinator/worker as one continuous flow.

Twelve hardware configurations covered Qwen 0.6B/1.7B and Parakeet v3 at both precisions on CPU/Metal. Each captured a sentence containing a negation, seven files, and the word language; a distinct one-word WAV; then the original sentence again. All 24 sentence captures matched their scripted text. Of 12 one-word captures, Qwen 1.7B Q6_K/CPU returned empty once; the same WAV transcribed as "Seven" twice through direct file inference. The following sentence remained correct. The acoustic test therefore passed 11 of 12 configurations, with 35 of 36 semantic controls matched. This short-word miss remains an observed recognition limitation, not a fixed application defect.

The initial public WAV averaged -66 dB and was too quiet for speaker playback at the unchanged system volume. A gain-adjusted copy produced inconsistent acoustic recognition; louder synthetic WAV controls were used for the matrix. A quieter Qwen 1.7B Q8_0/CPU one-word miss passed after matching playback levels. These failed attempts are retained. No system volume or input settings were changed. Native reload/start/success/error cue requests also produced measurable microphone energy near their expected frequencies; human assessment of cue audibility remains pending.

A separate real daemon pasted a microphone-recognized sentence through `test-paste`. The localhost browser recorded exactly one native paste event and the expected textarea value. `doctor --deep` passed real native transactions for text, HTML with its text alternative, ordered file lists, and an empty clipboard. The existing image clipboard survived the browser paste with identical dimensions and RGBA pixels; the test then restored its original PNG/TIFF representations byte-for-byte. Clipboard backups remained private and were removed after cleanup.

A normal quiet daemon loaded saved Parakeet v3 Q6_K/CPU settings, reported its sound override, offloaded after one minute, stopped through IPC, and restarted with a new PID and unchanged config bytes. The restarted in-memory history was empty; this case did not first populate it with a live dictation. Stdout and stderr stayed empty across both daemon lifetimes. The pinned memo again passed the production simulated worker with no stderr.

Follow-up command records, native fault traces, acoustic signal/transcript reports, source-build hashes and transaction results are under `target/tmp/new-models/macos-followup/`. The full default and Metal suites each passed 471 tests, with two opt-in native tests ignored in routine runs. Strict rustdoc, formatting, all-target checks, Clippy, build, and the all-features fallback passed again.

Cleanup removed another 25.7 GiB of downloaded checkpoints, source-build model copies, converter packages, and disposable build/fault objects. All twelve existing quantized models and the usable release runtimes remain; their hashes and the older installed-library hashes were verified. Checkpoint provenance and the cleanup manifest are retained with the evidence. Test daemons, the localhost server, and the owned browser tab were stopped.

The [physical desktop checklist](model-validation.md#physical-desktop-checks) remains partly open. Browser/native automation delivered application keys but did not register held Control/Space in macOS HID or combined-session key-state probes, so physical PTT timing, modifier release, and release during reload remain unverified. The hotkey was not weakened to accept a synthetic substitute. System sleep/wake was not attempted without a reliable wake path. Fresh human listening and natural room/quiet speech remain pending. Acoustic playback controls do not establish dictation accuracy in those conditions. These October 18 runs did not cover Windows; the later [focused Windows results](model-validation.md#focused-windows-results-2026-10-19) have separate scope and limits.
