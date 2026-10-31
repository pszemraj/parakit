# Validation

Build success does not prove transcription quality. Use real user audio, not synthetic TTS.

## Rust Validation Loop

Run the full loop in order before committing Rust or runtime changes:

```bash
cargo fmt --package parakit
rustdoc-checker . --exclude target,vendor,.claude,local-scratch --strict
cargo check --workspace --all-targets
cargo test
cargo clippy --workspace --all-targets -- -D warnings
cargo build
cargo check --workspace --all-targets --all-features
```

The all-features check can request accelerator toolchains that the current host does not have. Follow [GPU Feature Validation](#gpu-feature-validation) for the host-specific fallback, then run the native GPU build and runtime checks on each supported platform.

## GPU Feature Validation

Use the [Windows scripts](../../scripts/windows/README.md) for native CUDA/Vulkan builds. Raw `cargo check --workspace --all-targets --all-features` may still enter CMake's Visual Studio generator and fail before Rust typechecking if Visual Studio CUDA BuildCustomizations are stale. A known failure leaves `CudaToolkitDir` empty and reports `CUDA Toolkit directory '' does not exist`.

When that happens, record the exact CUDA/MSBuild error, then validate the Rust all-features surface with an existing bundled library directory:

```powershell
$env:CRISPASR_LIB_DIR = (Resolve-Path 'target\debug\build\parakit-<hash>\out\lib').Path
cargo check --workspace --all-targets --all-features
Remove-Item Env:\CRISPASR_LIB_DIR
```

This fallback does not replace the [native Windows build and inference checks](model-validation.md#windows-cpu-and-supported-gpu-backends).

On macOS, raw `--all-features` also enables CUDA and can fail in CMake before Rust typechecking when the CUDA Toolkit is not installed. Use the same `CRISPASR_LIB_DIR` fallback to validate the Rust all-features surface; validate Metal with the native macOS build and `doctor`.

```bash
CRISPASR_LIB_DIR="$PWD/target/debug/build/parakit-<hash>/out/lib" \
  cargo check --workspace --all-targets --all-features
```

Replace `<hash>` with the directory from the successful native build; do not point the fallback at an unrelated or stale library.

### Linux CUDA/Vulkan

Check `nvidia-smi` for active compute jobs before loading models. Build the daemon and helper in an [isolated target directory](../build.md#isolated-development-builds); do not install the validation binary over the running daemon. The commands below assume `CARGO_TARGET_DIR` still names that GPU build.

```bash
mkdir -p target/tmp/validation
printf '# Isolated validation configuration.\n' > target/tmp/validation/config.toml
export PARAKIT_CONFIG_PATH="$PWD/target/tmp/validation/config.toml"
export XDG_CACHE_HOME="$PWD/target/tmp/validation/cache"
export PARAKIT_MODELS_DIR="$XDG_CACHE_HOME/parakit/models"
```

Fetch the selected preset into this isolated cache first, or reuse an existing test cache by setting `PARAKIT_MODELS_DIR` to it. Check backend visibility before inference:

```bash
CUDA_VISIBLE_DEVICES=0 GGML_VK_VISIBLE_DEVICES= \
  "$CARGO_TARGET_DIR/debug/parakit" --verbose doctor
env -u GGML_VK_VISIBLE_DEVICES CUDA_VISIBLE_DEVICES= \
  "$CARGO_TARGET_DIR/debug/parakit" --verbose doctor
```

The first command exposes CUDA and hides Vulkan; the second hides CUDA and lets Vulkan select a discrete GPU. Check the device name in the native diagnostics. Use `GGML_VK_VISIBLE_DEVICES` to pin a Vulkan device when necessary, as described in [the environment reference](../config_reference.toml). `--device gpu` alone does not distinguish CUDA from Vulkan in a build containing both.

A verbose Vulkan run in a dual-backend build can report `no CUDA-capable device is detected` because CUDA was deliberately hidden. Confirm that the selected device is Vulkan on the intended GPU.

Run the built [WAV helper](#wav-quality-target) and [real worker simulation](#ptt-worker-simulation) directly, preserving the GPU build's feature set:

```bash
CUDA_VISIBLE_DEVICES=0 GGML_VK_VISIBLE_DEVICES= \
  "$CARGO_TARGET_DIR/debug/examples/transcribe-file" \
  --model-preset qwen3-asr-0.6b --device gpu --threads 8 \
  --audio local-scratch/juniper-voicememo-DO_NOT_DELETE.wav \
  --warmup --repeat 2
CUDA_VISIBLE_DEVICES=0 GGML_VK_VISIBLE_DEVICES= \
  "$CARGO_TARGET_DIR/debug/parakit" --verbose start \
  --model-preset qwen3-asr-0.6b --device gpu --threads 8 --no-cleaning \
  --simulate-ptt-audio local-scratch/juniper-voicememo-DO_NOT_DELETE.wav \
  --model-idle-minutes 1 --simulate-ptt-repeat 2 --simulate-ptt-idle-seconds 63 \
  --log-dir "$PWD/target/tmp/validation/logs"
```

For Vulkan, replace the CUDA environment prefix on both commands with `env -u GGML_VK_VISIBLE_DEVICES CUDA_VISIBLE_DEVICES=` or pin a Vulkan device explicitly. Apply these commands to the matrix and acceptance cases in [native model validation](model-validation.md). The worker's two idle waits take at least 126 seconds per model/backend pair.

Retain commands, device diagnostics, raw transcripts, exit statuses, timings, and per-process memory samples under the ignored `target/tmp/` tree. Follow the [reload measurement checklist](model-validation.md#qwen-options-long-input-and-reload); the earlier [Linux measurements](memory.md#qwen-gpu-reload-measurements) record baseline conditions and limits.

## Rust Source Coverage

Install Rust's coverage tools once:

```bash
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov --locked
```

Run the workspace tests with source instrumentation and print the Rust coverage summary:

```bash
cargo coverage
```

This alias uses the default features and Cargo test-target surface, including the test-enabled `transcribe-file` example. Use `cargo coverage --all-targets` for the remaining Cargo target kinds, and pass explicit feature flags when measuring feature-gated code.

Generate a browsable per-line report when investigating an untested branch:

```bash
cargo coverage-html
```

The aliases are defined in [`.cargo/config.toml`](../../.cargo/config.toml). By default, `cargo-llvm-cov` keeps instrumented artifacts under `target/llvm-cov-target`, separate from ordinary builds. Reports cover parakit's Rust code, not linked CrispASR C/C++ code. They also cover only code compiled for the current host and feature set, so macOS coverage does not validate Linux or Windows branches. Use coverage to locate test gaps; do not treat a percentage as evidence of transcription quality, desktop integration, or paste safety.

## WAV Quality Target

Use the Rust WAV target to run the same [CrispASR](https://github.com/CrispStrobe/CrispASR) engine without the hotkey daemon:

```bash
cargo run --release --no-default-features --features bundled --example transcribe-file -- \
  --audio clips/example.wav
```

The helper accepts WAV input, uses the same raw `Engine` path as the daemon, and prints raw inference text and timing. It intentionally does not import or apply parakit text-cleaning rules. This command avoids live daemon desktop/audio dependencies while keeping the bundled CrispASR build. Model cache behavior is in [running.md#model-cache](../running.md#model-cache). Pass `--model /path/to/model.gguf` only when comparing a specific custom GGUF. The source lives at [examples/transcribe_file.rs](../../examples/transcribe_file.rs); it is a Cargo example target so it is not installed as an end-user binary.

Use `--model-preset` for any [hosted preset](../running.md#model-presets), including both Qwen sizes and the Moondream models:

```bash
cargo run --release --no-default-features --features bundled --example transcribe-file -- \
  --audio clips/example.wav --model-preset qwen3-asr-1.7b \
  --qwen-language auto --qwen-max-new-tokens 512
```

The example does not read daemon configuration. Model selectors, Qwen controls, and incomplete-decode behavior follow [model presets](../running.md#model-presets).

For latency work, use short real clips around 2s, 5s, 15s, and 25s. Longer quality clips help catch transcription drift, but should not drive startup or post-release latency policy. Use `transcribe-file --warmup` to apply the [daemon readiness policy](../running.md#idle-model-offload). Repeatable `--warmup-seconds N` flags remain available for Parakeet shape diagnostics and are rejected for Qwen.

## PTT Worker Simulation

The Rust suite runs the production recording coordinator and worker with a test audio manager and fake inference engine. It checks exact PCM captured before and during blocked reload, release before or after readiness, one transcription and completion, transcript history, and session destruction without a model or a minute-long timeout. These checks do not exercise native microphone startup or physical hotkeys; use the [physical desktop checks](model-validation.md#physical-desktop-checks) for those boundaries.

Use the hidden simulation path when you need the daemon worker flow without a live keyboard, microphone, or text insertion:

```bash
mkdir -p target/tmp && : > target/tmp/ptt-empty.toml
PARAKIT_CONFIG_PATH=$PWD/target/tmp/ptt-empty.toml \
XDG_CACHE_HOME=$PWD/target/tmp/ptt-cache \
  cargo run -- start -m path/to/model.gguf --threads 8 --no-cleaning \
  --simulate-ptt-audio local-scratch/juniper-voicememo-DO_NOT_DELETE.wav \
  --model-idle-minutes 1 --simulate-ptt-repeat 2 --simulate-ptt-idle-seconds 61
```

The empty config file keeps your own `config.toml` out of the run; `PARAKIT_CONFIG_PATH` must name a file that exists. Use a real WAV with a known transcript. The pinned Juniper voice memo is 55.36 seconds; its local filename can vary by machine. Compare raw output with the readable reference transcript using task-appropriate tolerance. The command resamples it to the model rate and sends worker start/stop events with owned PCM. `--no-cleaning` exposes raw output for comparison; omit it to exercise cleanup too. Add the build feature and `--device` for the backend being tested. On PowerShell, create the empty file with `New-Item -ItemType File -Force target/tmp/ptt-empty.toml`, set the environment variables with `$env:NAME = ...`, and use the Windows WAV path.

Simulation bypasses the daemon singleton and opens no control endpoint. It uses the same resolved worker settings as normal startup, but disables insertion, sounds, and desktop notifications. Avoid concurrent inference when measuring timings; simulation does not pause an already-running daemon.

The worker acknowledges each required transcript and verifies the expected residency after each wait (including the last). With history enabled and context disabled, repeated transcripts must match within the same process; this verifies reload behavior rather than recognition quality across builds. Context-enabled repetitions can differ as prior history changes. Missing transcripts, reload failures, or missed offloads return a failure exit status. An immediate simulated release exercises queuing while reload is busy. Use `--model-idle-minutes 0` to check disabled offloading. For one dictation without an idle wait, omit `--simulate-ptt-repeat` and `--simulate-ptt-idle-seconds`; both options require `--simulate-ptt-audio`.

## NeMo Reference Helper

The Python helper runs NVIDIA NeMo's official [Parakeet-TDT-0.6B-v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3) path:

```bash
python scripts/transcribe_nemo_parakeet.py clips/example.wav
```

Use a separate Python environment. NeMo and PyTorch are heavy dependencies and are not needed for the Rust daemon:

```bash
python -m venv target/tmp/.venv-nemo
source target/tmp/.venv-nemo/bin/activate
python -m pip install --upgrade pip
python -m pip install -r scripts/requirements-nemo.txt
```

For CUDA, install the PyTorch build appropriate for the machine first if the default wheel resolver does not pick the right one. The script has hard imports for PyTorch and NeMo so a broken reference environment fails immediately.

## Recommended A/B Procedure

For NVIDIA v3 validation, select `--model-preset parakeet-v3` and use its matching F16 and Q8_0 artifacts. The default Parakeet Ultra model uses post-trained weights; follow [Qwen and Moondream comparisons](#qwen-and-moondream-comparisons) for its original-checkpoint reference.

Use 5 to 10 real clips that cover:

- short and long utterances;
- clean and noisy audio;
- technical vocabulary;
- normal dictation pace;
- at least one clip with hesitations or restarts.

Compare against:

1. NVIDIA's official Parakeet-TDT-0.6B-v3 through NeMo on the same WAV inputs.
2. parakit with an F16 GGUF model.
3. parakit with the cached Q8_0 GGUF model.

Compare raw transcripts, checking omissions, negations, numbers, and entities. Use the official implementation's output as the model reference. Previous Parakit/CrispASR output is a regression baseline, not ground truth; matching it does not establish correct recognition. Label human audio-reviewed transcripts separately, and use the recording to adjudicate what was actually spoken when outputs differ. A full-recording transcript does not establish recognition at the boundaries of a shorter crop.

Acceptable differences:

- punctuation;
- capitalization;
- whitespace;
- one-off cleanup differences that preserve meaning.

Not acceptable:

- missing words;
- wrong technical terms;
- repeated substitutions across clips;
- many wrong words in a short utterance.

If F16 differs materially from the reference, inspect CrispASR's Parakeet preprocessor first. If F16 matches but Q8_0 is worse, suspect the hosted artifact or quantizer path.

Parakeet-v3 auto-detects language. It has no parakit language flag today, so non-English or code-switched dictation should be checked against real clips before relying on it. Accents, noise, proper nouns, and technical vocabulary can change language detection behavior.

### Qwen And Moondream Comparisons

Start Qwen comparisons with 1.7B: run the same known-bad recordings through Parakeet v3, Qwen 1.7B F16, and Qwen 1.7B Q8_0, then repeat for 0.6B. Fetch the exact F16 files with the existing custom fetch path:

```bash
parakit fetch cstr/qwen3-asr-1.7b-GGUF --file qwen3-asr-1.7b-f16.gguf
parakit fetch cstr/qwen3-asr-0.6b-GGUF --file qwen3-asr-0.6b.gguf
```

Use the resulting printed model paths with `transcribe-file --model`. Compare against the [official Qwen implementation](https://huggingface.co/Qwen/Qwen3-ASR-0.6B#python-package-usage) as a separate reference. Keep forced language and token budget the same across comparisons; automatic detection and forced English are distinct cases. Include short speech, silence, the pinned Juniper recording, and speech spanning a 30-second Qwen chunk boundary. Use raw output (`--no-cleaning` in daemon simulation), then verify repeated inference and idle offload/reload preserve the options and transcript.

The legacy `qwen-asr` 0.0.6 eager/SDPA audio encoder computes window boundaries but does not apply its window mask. CrispASR's dense audio attention matches that path, so agreement between these implementations can reproduce shared attention behavior. Parakit enables windowed Qwen audio attention by default; explicit environment overrides are retained. [Qwen PR #103](https://github.com/QwenLM/Qwen3-ASR/pull/103) passes the mask to the encoder layers. The native Qwen implementation in Transformers 5.13.0 instead splits eager/SDPA attention into separate audio windows; use the official [`-hf` checkpoints](https://huggingface.co/Qwen/Qwen3-ASR-1.7B-hf) for that reference.

To investigate a consequential disagreement, compare F16 with dense and windowed audio attention using CrispASR's existing environment switch. Disable its repetition repair in both diagnostic arms so the generated output remains observable:

```bash
CRISPASR_NGRAM_LOOPFIX_OFF=1 CRISP_AUDIO_WINDOWED_ATTN=0 \
  "$CARGO_TARGET_DIR/debug/examples/transcribe-file" \
  --model path/to/qwen3-asr-1.7b-f16.gguf --device gpu \
  --audio clips/example-16k.wav --warmup --repeat 2 \
  --qwen-language auto --qwen-max-new-tokens 512
CRISPASR_NGRAM_LOOPFIX_OFF=1 CRISP_AUDIO_WINDOWED_ATTN=1 \
  "$CARGO_TARGET_DIR/debug/examples/transcribe-file" \
  --model path/to/qwen3-asr-1.7b-f16.gguf --device gpu \
  --audio clips/example-16k.wav --warmup --repeat 2 \
  --qwen-language auto --qwen-max-new-tokens 512
```

Use the same build and backend in both arms. Prepare one mono 16-kHz PCM input and give each implementation the same sample ranges. Native long-input chunks use energy-dependent boundaries below the 30-second limit; transcribing the whole recording in Python changes the decoder context. Record actual native chunk ranges when diagnosing a difference, then feed those exact slices to the reference. Include clips around the audio encoder's 8-second window boundary as well as the long-input chunk boundary.

For preprocessing comparisons, also start from the original recording and capture the output of the production WAV and microphone resamplers. Record decoded sample counts, channel mixing, scaling, filter settings, alignment and flushed tails before comparing that PCM with SciPy or the demo's librosa path. A delay reported by a resampler is not sufficient evidence to trim samples; measure alignment on the resulting waveforms.

Librosa's `soxr_hq` path calls SoXR at HQ quality with energy scaling disabled and fixes the final length to the ceiling of the sample-rate ratio. A native comparison can use SoXR directly with float32 mono input, matching quality and final-length handling; streaming comparisons must also signal end of input and drain the output. Parakit's file and microphone paths instead share a 256-tap Rubato sinc resampler with 256 oversampling, .95 cutoff and linear interpolation. Both drain the final filter tail to the recording's nominal sample count and reset between recordings. This is a separate native resampler, not an exact SoXR implementation, and the attention environment switch does not change it.

Compare float32 mel features on identical PCM before changing the encoder. If generated tokens differ, inspect encoder embeddings and the first differing logits under the same preceding tokens. F16 weights do not imply identical intermediate precision or GPU kernels, and a small numeric difference can change later words after greedy decoding diverges. Keep native F16, native quantized and Python BF16 results separate.

Keep context, system text and hotwords empty, with batch size one and greedy decoding. Record the reference versions and checkpoint revisions, model dtype, the audio encoder's effective attention implementation and mask or per-window call shapes, and raw generated tokens through EOS or token exhaustion. For web-demo comparisons, also record the uploaded file's sample rate, decoded PCM and resampling method; a separately prepared 16-kHz waveform can produce different recognition. A requested attention setting alone does not establish execution. Score complete outputs separately from failed or budget-exhausted decodes, and listen to polarity and omission disagreements before choosing a default. Check quiet speech, noisy tails, silence and repeated long input for regressions in both modes.

For [Ultra](https://huggingface.co/moondream/parakeet-ultra) and [Redux](https://huggingface.co/moondream/parakeet-redux), compare the Q8_0 presets with the original checkpoints through CrispASR's existing Hugging Face reference helpers and a short independent Photon run. Photon's packed Redux implementation and VAD segmentation differ from the GGUF runtime; record those conditions rather than attributing their speed or memory figures to parakit.

Record failures, cold/warm latency, and host/GPU memory. Keep downloaded reference weights, dependencies, and generated evidence under ignored scratch directories. Check for active training or other GPU jobs before each hardware run and defer GPU tests while they occupy the device.

### Parakeet CPU Comparison

An acceptance run on October 14, 2026 compared the three Parakeet Q8_0 presets on an AMD Ryzen 7 7700X, using eight inference and OpenMP/BLAS threads, OpenBLAS, and the CPU-only bundled build. The native C++ library was optimized; the Rust file helper was a debug build. Each model transcribed the same 55.36-second Juniper memo twice after loading.

| Preset | First inference | Repeat inference | Peak process RSS |
| --- | ---: | ---: | ---: |
| `parakeet-v3` | 5.60 s | 5.33 s | 1.56 GiB |
| `parakeet-ultra` | 5.00 s | 4.60 s | 1.49 GiB |
| `parakeet-redux` | 6.12 s | 5.66 s | 1.49 GiB |

Inference times exclude model loading. RSS includes loading and both passes; it is not the model's download size. Filesystem caches were not flushed, and other host processes were present. These two runs on one recording do not establish a general speed ranking or transcription-quality advantage. Redux did not provide a CPU speed or memory advantage over Ultra in this check.

Use the [WAV quality target](#wav-quality-target) with `--device cpu --repeat 3` and each `--model-preset` to compare your own CPU and recordings. Check raw-text fidelity alongside timing, especially omissions, negations, numbers, and names.

### Checkpoint Conversion Acceptance

Use the [source-rebuild commands](README.md#source-rebuild) with an isolated model cache and `--keep-f16 --force` to exercise original checkpoint download, F16 conversion, and Q8_0 quantization for both Qwen sizes and both Moondream models. Also run one rebuild from a local original snapshot with `--checkpoint-dir`; verify that the supplied snapshot remains after success and failure. Existing Parakeet v3 `.nemo` rebuilding must continue to work.

For each rebuilt model, open the retained F16 and resulting Q8_0 artifacts through `transcribe-file` and the actual daemon worker. Compare both against the original implementation on the same audio using the quality procedure above. A successful converter or quantizer exit alone is insufficient: check consequential omissions, negations, numbers/entities, silence, and chunk-boundary speech. Treat any existing runtime quality limitation as unresolved until these comparisons establish otherwise.

Check the [source-rebuild cache and retention behavior](README.md#source-rebuild) with the converter/network unavailable, `--force`, and injected conversion/quantization failures. Failed rebuilds must preserve the completed cached model. Confirm named-preset startup selects the completed artifact. Keep generated models and reports in ignored test directories.

## Cleaning Corpus Replay

Use the audit example to measure a cleanup change over historical JSONL dictation logs without loading a speech model:

```bash
cargo run --no-default-features --features bundled --example audit-cleaning -- \
  "$HOME/.parakit/logs" \
  --profile safe \
  --output target/cleaning-safe.json
```

Run it again with `--profile aggressive` when changing an aggressive-only pass. Compare record counts, changed-from-raw counts, agreement with historical output, per-pass activation counts, and every retained difference example. A lower rule count or a larger changed-text count is not a quality metric; preservation of intended meaning and per-pass precision are.

The audit removes one terminal period by default, matching daemon behavior. Add `--keep-trailing-period` when comparing prose-oriented output separately, and use `--number-threshold VALUE` to replay a non-default isolated-number threshold.

The audit applies built-in rules only. It does not load `config.toml` or `[[rules.user]]`; pass profile, threshold, trailing-period, and disabled-rule choices explicitly. Use `parakit rules test` to validate the currently configured user rules.

Number-rule invariants and personal vocabulary handling are in [cleaning rules](../cleaning-rules.md#numbers). Cover both preservation and nearby exact counts in the existing regression matrix.

## Runtime Smoke Checks

Run the daemon in foreground mode and exercise real applications:

- terminal;
- browser text fields;
- editor buffers;
- chat applications;
- rich-text editors.

Check:

- both Qwen [dictation shortcuts](../qwen3-asr.md#prior-dictation-context) suppress literal Space; the extra shortcut stays inactive for Parakeet;
- holding each shortcut for 3 to 5 seconds produces exactly one recording start and one stop;
- rapid double-presses do not wedge the state machine;
- releasing Control before Space, and Space before Control, each stops recording once;
- releasing Alt or Option during contextual recording preserves the press-time mode and recording;
- an occupied optional context shortcut warns without disabling the original shortcut on registered backends;
- enabling `qwen.context_on_default_hotkey` and restarting applies context to the original shortcut; disabling it and restarting restores ordinary dictation;
- focus changes do not paste into the new target;
- short speech is padded and transcribed, with ordinary inference below one second; silence leaves history and the receiving application unchanged;
- `--quiet` emits no stdout;
- warnings still go to stderr in quiet mode;
- sounds still play in quiet mode unless `--no-sounds` is set;
- logging writes raw and cleaned text without crashing the daemon.

Run daemon/IPC checks sequentially with isolated config, cache/runtime paths, and a model copy. After [idle offload](../running.md#idle-model-offload), hold PTT through reload, speak, and release: early speech must be retained, readiness must precede the listening cue, and insertion must occur once. Release PTT immediately during a separate reload: the queued capture must complete once without a late listening cue. Restore a missing model before the next PTT, then check status and history. Check stop/restart and sleep/wake on the native desktop. Verify [sound cues](../running.md#logging-and-sounds) on failed readiness and silence, and with `--no-sounds`. Headless tests verify cue ordering and cancellation; audible playback, physical input, and insertion require native observation. Mark unobserved checks and unavailable backends pending.

On macOS, an opt-in acoustic control plays two WAVs through the speakers and records them through the production CPAL/Rubato microphone path. It checks three successive captures (first WAV, second WAV, first WAV), retains only text and signal measurements, and can measure the real speaker cues. This does not synthesize or validate the physical global hotkey. Use public speech WAVs, an active desktop with microphone permission, and repository-local model, WAV, and report paths. The report file must already exist:

```bash
mkdir -p target/tmp/live-audio
touch target/tmp/live-audio/capture.json
PARAKIT_LIVE_MODEL=target/tmp/models/model.gguf \
PARAKIT_LIVE_DEVICE=gpu \
PARAKIT_LIVE_WAV=target/tmp/first.wav \
PARAKIT_LIVE_SECOND_WAV=target/tmp/second.wav \
PARAKIT_LIVE_REPORT=target/tmp/live-audio/capture.json \
PARAKIT_LIVE_CUES=1 \
CRISPASR_FC_PW_Q8=0 CRISPASR_FC_FUSED_QKV=0 CRISP_AUDIO_WINDOWED_ATTN=1 \
  cargo test --release --features metal --bin parakit \
  daemon::audio::live_smoke::speaker_wav_through_live_capture -- --ignored --nocapture
```

Use `PARAKIT_LIVE_DEVICE=cpu` for CPU. Omit `PARAKIT_LIVE_CUES` to skip cue playback. Inspect the report even on failure: silent input, insufficient acoustic volume, and recognition failure are different outcomes. Compare the transcripts with the WAVs and inspect the following quiet capture for leftover samples. This test does not save microphone audio.

### Native Microphone Worker

Use the [native isolation setup](model-validation.md#macos-cpu-and-metal) and a fresh report destination. Place a model and an audible, complete-sentence WAV at the paths below, or edit the two fixture variables. The report file must exist; its sibling `.logs` directory must not already exist.

```sh
mkdir -p target/tmp/native-worker
printf '{}\n' > target/tmp/native-worker/cpu.json
export PARAKIT_LIVE_MODEL="$PWD/target/tmp/native-worker/model.gguf"
export PARAKIT_LIVE_WAV="$PWD/target/tmp/native-worker/speech.wav"
export PARAKIT_LIVE_REPORT="$PWD/target/tmp/native-worker/cpu.json"
export PARAKIT_LIVE_DEVICE=cpu
export PARAKIT_LIVE_CUES=1
cargo test --features metal --bin parakit \
  microphone_coordinator_worker_reloads_after_idle -- \
  --ignored --nocapture --test-threads=1
```

Select `gpu` and a fresh report path for Metal. `PARAKIT_LIVE_SECOND_WAV` optionally adds another loaded/offloaded pair. Inspect the recorded transcripts and speech-boundary result.

For allocation context, see the [Linux reload measurements and ownership notes](memory.md).

### Clipboard Checks

Portable tests cover snapshot format selection, unavailable formats versus real read errors, complete payload comparisons, restoration retry, sentinel recognition, and insertion reports/notifications. Native `doctor --deep` transactions still need an interactive desktop; follow the [Linux](../linux-desktop.md#deep-doctor-check), [macOS](../macos-desktop.md#doctor---deep), or [Windows](../windows-desktop.md#doctor---deep) procedure.

Use controlled text, HTML/text alternatives, images, copied files, and an empty clipboard. Independently read back restored contents: compare HTML and its text alternative, file paths/order, and image dimensions/pixels. Exercise failed cleanup and a newer competing copy where reproducible, checking the macOS diagnostic's [sentinel ownership limits](../macos-desktop.md#doctor---deep). Source checks and SDK-less cross-target compilation do not establish native clipboard serialization or GUI cleanup.

TODO: Reproduce a hung X11 clipboard owner and design bounded snapshot reads that preserve the prior clipboard. Arboard reads formats sequentially and its public API does not distinguish a read timeout from an unavailable format.

TODO: On a native Linux desktop, exercise direct typing and guarded paste with physical modifier/focus changes, key-probe failures, keyboard-map changes, readiness-error reconnection and clipboard-manager handoffs. Closure tests and Mac runs do not establish those X11 results. Windows generation changes and delayed rendering also need native clipboard-manager or RDP verification.

### Linux IBus and X11

Prepare an isolated GNOME Wayland session using the [Linux input-source setup](../linux-desktop.md#ibus-setup), with a disposable text field focused. Confirm the receiving application uses native Wayland, unset `DISPLAY`, and record actual field contents and clipboard readback. Test GTK, Chrome, VS Code, and supported Ghostty/tmux configurations separately; record XWayland results as a different route.

Use private D-Bus/IBus and XDG state. Register components with the binary under test's `ibus install --component-dir target/tmp/ibus-components` and point the private IBus process's `IBUS_COMPONENT_PATH` at that directory. Exercise installation, upgrade, and removal there; compare unrelated input sources and shortcuts before and after.

The opt-in worker checks use controlled inference with production insertion. Run the native check in that private IBus session and the X11 check separately on an isolated X11 display:

```bash
env -u DISPLAY cargo test --bin parakit daemon::worker::tests::live_native_worker_inserts_without_x11 -- --ignored --exact
cargo test --bin parakit daemon::worker::tests::live_x11_worker_pastes_into_focused_field -- --ignored --exact
```

Independently verify Unicode and selection replacement in the receiving application; a submitted commit or passing worker assertion does not prove reception. Check clipboard preservation on native delivery with retention enabled too. Exercise the [focus and delivery rules](../running.md#focus-changes) during recording, inference, and submission, including helper, daemon, and IBus restarts. Check both PTT release orders, repeats, lost releases, terminal tabs/newlines, and password/PIN fields. Ordinary typing, Compose/dead keys, and a non-US layout/variant must remain usable with the daemon unavailable.

### History picker

Use an isolated daemon with non-sensitive transcripts and exercise the [history commands](../running.md#daemon-control) in a terminal, a pipe, plain mode, quiet mode, and full-text mode. Check long wrapped previews, scrolling, resizing, cancellation, and terminal restoration after successful copies and errors.

While one entry is selected, add dictations and confirm preview and clipboard copy still refer to that entry. Check disabled/empty history, eviction, daemon restart, and oversized full-text responses without truncation or substitution. An oversized preview must still allow copying the selected entry with Enter. Force a clipboard failure, verify the picker remains open, retry, and independently read the actual clipboard through its supported platform route.

### Ordered audio and loaded Qwen failures

Repeat `transcribe-file --audio` to run ordered inputs on one loaded engine. [Worker simulation](#ptt-worker-simulation) accepts repeated `--simulate-ptt-audio` paths; it replays the full sequence per cycle before the idle interval. Its failure path reads history back and rejects replacement of the preceding successful transcript. See the [native Qwen sequence](model-validation.md#qwen-options-long-input-and-reload) for the short/long/short acceptance case.

The loaded-session fault driver runs on Linux and macOS with an explicit Qwen model:

```bash
python tests/native/run_qwen_faults.py \
  --native-build <isolated-target>/debug/build/parakit-<id>/out/build \
  --binary-dir <isolated-target>/debug \
  --model <qwen-q8.gguf> \
  --device cpu \
  --audio vendor/CrispASR/bindings/go/samples/jfk.wav \
  --output target/tmp/qwen-native-faults
```

First run `cargo test --test native_correctness_build` in the same isolated target directory to build the loaded-session test harness. Use the matching completed native build directory containing `compile_commands.json`, and a fresh output directory. CPU is the default; for a GPU run, build both binaries and the harness with the matching accelerator feature, then pass `--device gpu`. On macOS the driver preserves the dylib basename and version aliases and sets `DYLD_LIBRARY_PATH` to load its private fault library. It compiles a separate corrected C API copy and relinks its own test library; normal binaries and vendor sources are unchanged.

First/middle/last chunk failures and greedy embedding/forward failures run through both file and worker paths. The worker first remembers a successful dictation, then checks history preservation and absence of successful logging/insertion for the failed input. Separate prefix cases inspect actual native English, Cantonese, and Filipino assistant prefills. They force those languages on English fixture audio to inspect prompt construction, not multilingual recognition quality.

Context cases trace exact native prompt bytes and session setters. They check raw system text and forced-English prefills across chunks, contextual/ordinary/contextual calls on one session, unusable-context fallback, and clearing after successful or failed inference. Worker cases verify that the next capture receives the previous stored transcript and silence reaches no contextual inference or successful history entry. `PARAKIT_TEST_QWEN_AUDIO` and `PARAKIT_TEST_QWEN_CONTEXT` override the session context probes' WAV and prior text; the driver otherwise uses the constructed long fixture and a small vocabulary sample. The Rust context probes print a `PARAKIT_TEST_RESULT` JSON record per call with raw text, elapsed inference seconds, and any error. Compare these outputs with the recording and a readable reference; prompt-routing checks alone do not establish quality. The separate ignored `qwen_context_handles_silent_and_short_audio` test reports direct-engine silence output and checks subsecond ordinary fallback; direct calls do not use the daemon's silence filter.
