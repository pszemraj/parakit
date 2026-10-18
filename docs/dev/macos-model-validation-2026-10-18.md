# macOS model validation: 2026-10-18

Automated CPU and Metal validation covers all six presets at Q8_0 and Q6_K. Physical microphone, PTT, insertion, cues, sleep/wake, and fresh human listening remain pending. These results do not close the Windows checklist or establish recognition accuracy.

## Build and isolation

- Baseline: `b4b9ab812053fc58138de3a7d2ff9e4c1da0f51d`, PR #21. Reload correction: `a7e3c6cd1b8b9b10e0c459394a3fb17351106090`.
- Host: Apple M5 MacBook Air, 10 cores, 32 GiB unified memory, macOS 26.7 (25G229).
- Toolchain: Rust 1.98.1, CMake 4.4.4; optimized native code and release Rust binaries; six inference threads.
- CrispASR v0.8.42: `3be8bcb4390b8080dc73f807b7abb34dc0aca9c3`; ggml: `c36dab89b662838f0f5d4826c399198c0b90bbfc`.

The full matrix used the baseline. The correction was checked with both test suites, rebuilt CPU/Metal binaries, focused native reload regressions, Qwen corpus recapture, and default v3 memo replay.

Builds, models, configuration, logs, control paths, temporary files and the Metal pipeline cache were isolated inside the checkout. The installed daemon remained running throughout. Loaded-library traces confirmed the matching isolated CrispASR/ggml libraries; the separate CPU build loaded no Metal backend. Graph-dispatch traces confirmed substantial CPU or Metal model graphs for every requested matrix configuration, excluding small device-capability probes.

Formatting, strict rustdoc, workspace/all-target checks, tests, Clippy with warnings denied, CPU/Metal release builds, and the Rust all-features fallback passed. The corrected source passes 471 tests in each default and Metal suite; the separate native fault-library test remains ignored. All five resampler-filtered tests passed.

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

Ten English FLEURS clips were transcribed across all 24 configurations. Dataset reading-text comparisons and raw predictions are retained locally as diagnostic evidence; they were not freshly adjudicated and do not qualify model accuracy. Synthetic noise/quiet controls do not establish real room-noise behavior. Qwen remains experimental, Redux remains testing-only, and Parakeet v3 Q8_0 remains the default.

## Metal reload correction

The old Qwen requirement, one model file plus 2 GiB, was below observed Metal allocation. The pinned mmap loader registers the entire GGUF separately for Qwen's main model and its filtered audio tower. The reload guard now counts both registrations on Metal: twice the selected file size plus 2 GiB. CUDA/Vulkan and Parakeet accounting retain their existing policies. See [memory accounting](memory.md#metal-qwen-reload-accounting).

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

The built-in microphone/device format and existing OS grants were inspected, but capture was not activated. Pending work is the [physical desktop checklist](model-validation.md#physical-desktop-checks), real room/quiet-speech recordings, fresh human listening, and native Windows execution. Physical checks require a separate session that can own the hotkey and insertion path. The user's installed daemon was deliberately preserved for this run.
