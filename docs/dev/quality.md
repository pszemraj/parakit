# Validation

Build success does not prove transcription quality. Use real user audio, not synthetic TTS.

## Rust Validation Loop

Run the full loop in order before pushing Rust or runtime changes:

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

Use the Windows scripts for CUDA/Vulkan validation; they default to Ninja. Raw `cargo check --workspace --all-targets --all-features` may still enter CMake's Visual Studio generator and fail before Rust typechecking if Visual Studio CUDA BuildCustomizations are stale. A known failure leaves `CudaToolkitDir` empty and reports `CUDA Toolkit directory '' does not exist`.

When that happens, record the exact CUDA/MSBuild error, then validate the Rust all-features surface with an existing bundled library directory:

```powershell
$env:CRISPASR_LIB_DIR = (Resolve-Path 'target\debug\build\parakit-<hash>\out\lib').Path
cargo check --workspace --all-targets --all-features
Remove-Item Env:\CRISPASR_LIB_DIR
```

This fallback does not replace real GPU validation. Also run the CUDA and Vulkan Windows scripts plus simulated-dictation smoke tests against `local-scratch\juniper-voicememo-DO_NOT_DELETE.wav` when touching Windows GPU behavior.

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

For Vulkan, replace the CUDA environment prefix on both commands with `env -u GGML_VK_VISIBLE_DEVICES CUDA_VISIBLE_DEVICES=` or pin a Vulkan device explicitly. Exercise `parakeet-v3`, both Qwen sizes, `parakeet-ultra`, and `parakeet-redux` on the full Juniper memo and silence. For Qwen, also test 29-, 30-, and 31-second crops across its chunk boundary. Silence should produce an empty `Raw:` transcript in the helper and a worker no-speech error, without a successful transcript record. The worker's two idle waits take at least 126 seconds per model/backend pair.

Retain commands, device diagnostics, raw transcripts, exit statuses, timings, and per-process memory samples under the ignored `target/tmp/` tree. For reload calibration, sample host RSS and GPU allocations through startup warmup, full inference, loaded idle, offload, and reload. Compare the increase from the offloaded GPU allocation with the model file size plus the reload allowance; retained driver memory is already unavailable to the free-memory check. Label sampled peaks as observations rather than ceilings. Successful runs with ample free VRAM do not establish out-of-memory recovery.

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

The example does not read daemon configuration. `--model` and `--model-preset` conflict, and Qwen controls require a Qwen GGUF. Token exhaustion without EOS or a failed native chunk must fail the full transcription rather than print an incomplete success.

For latency work, use short real clips around 2s, 5s, 15s, and 25s. Longer quality clips are still useful for catching transcription drift, but they should not drive startup or post-release latency policy. Use `transcribe-file --warmup` for the same model-aware readiness as daemon startup/reload. Qwen uses embedded human speech with automatic language and 512 tokens, then restores the user's options; Parakeet retains synthetic warmup. Repeatable `--warmup-seconds N` flags remain available for Parakeet shape diagnostics and are rejected for Qwen.

## PTT Worker Simulation

The Rust suite also runs the production worker loop with a fake inference engine. It checks timed offload, audio queued during a blocked reload, one transcription and completion, transcript history, and session destruction without a model or a minute-long timeout.

Use the hidden simulation path when you need the daemon worker flow without a live keyboard, microphone, or text insertion:

```bash
mkdir -p target/tmp && : > target/tmp/ptt-empty.toml
PARAKIT_CONFIG_PATH=$PWD/target/tmp/ptt-empty.toml \
XDG_CACHE_HOME=$PWD/target/tmp/ptt-cache \
  cargo run -- start -m path/to/model.gguf --threads 8 --no-cleaning \
  --simulate-ptt-audio local-scratch/juniper-voicememo-DO_NOT_DELETE.wav \
  --model-idle-minutes 1 --simulate-ptt-repeat 2 --simulate-ptt-idle-seconds 61
```

The empty config file keeps your own `config.toml` out of the run; `PARAKIT_CONFIG_PATH` must name a file that exists. Use a real WAV with a known transcript. The pinned Juniper voice memo is 55.36 seconds; its local filename can vary by machine. Compare the readable transcript with task-appropriate tolerance rather than hashing the fixture. The command resamples it to the model rate and sends worker start/stop events with owned PCM. `--no-cleaning` exposes raw output for comparison; omit it to exercise cleanup too. Add the build feature and `--device` for the backend being tested. On PowerShell, create the empty file with `New-Item -ItemType File -Force target/tmp/ptt-empty.toml`, set the environment variables with `$env:NAME = ...`, and use the Windows WAV path.

Simulation bypasses the daemon singleton and opens no control endpoint. It uses the same resolved worker settings as normal startup, but disables insertion, sounds, and desktop notifications. Avoid concurrent inference when measuring timings; simulation does not pause an already-running daemon.

The worker acknowledges each required transcript, checks exact parity across repeats in the same process, and verifies the expected residency after each wait (including the last). This parity check verifies that reloading recreates the same session; it is not a cross-build reference-transcript comparison. Missing transcripts, reload failures, or missed offloads return a failure exit status. An immediate simulated release exercises queuing while reload is busy. Use `--model-idle-minutes 0` to check disabled offloading. For one dictation without an idle wait, omit `--simulate-ptt-repeat` and `--simulate-ptt-idle-seconds`; both options require `--simulate-ptt-audio`.

The [Linux reload measurements](memory.md#linux-reload-measurements) describe observed host and GPU allocation. This is separate from the real-time worker timeout test.

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

Use the official implementation's output as the model reference. Previous Parakit/CrispASR output is a regression baseline, not ground truth; matching it does not establish correct recognition. Label human audio-reviewed transcripts separately, and use the recording to adjudicate what was actually spoken when outputs differ. A full-recording transcript does not establish recognition at the boundaries of a shorter crop.

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

Record raw transcripts, omissions, negation and number/entity disagreements, failures, cold/warm latency, and host/GPU memory. Listen to consequential disagreements and label human-reviewed text separately. Keep downloaded reference weights, dependencies, and generated evidence under ignored scratch directories. Check for active training or other GPU jobs before each hardware run and defer GPU tests while they occupy the device.

Native platform acceptance remains separate: on macOS run both CPU and Metal for all presets; on Windows run the CPU bundle and available GPU backend. On each, check short and long WAV transcription, silence, repeated inference, idle offload/reload, explicit Qwen controls, quiet-mode errors, and real PTT insertion. Record unavailable runs as pending; Linux results do not establish these native platform results.

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

Confirm `start --model-preset NAME` resolves the rebuilt cache artifact. Repeat `fetch --from-source --model-preset NAME` with the completed artifact present and no usable Python converter/network access; it must reuse the cached Q8_0. Then exercise `--force`, intermediate F16 retention/removal, retained downloaded snapshots, and conversion/quantization failures without replacing a completed cached model. Keep generated models and reports in ignored test directories. These are local acceptance checks; they do not add model downloads, conversion, inference, or new jobs to CI.

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

- the platform push-to-talk chord does not leak a literal space;
- holding the platform push-to-talk chord for 3 to 5 seconds produces exactly one recording start and one stop;
- rapid double-presses do not wedge the state machine;
- releasing the push-to-talk modifier before Space stops recording;
- focus changes do not paste into the new target;
- very short captures are padded and transcribed instead of rejected;
- `--quiet` emits no stdout;
- warnings still go to stderr in quiet mode;
- sounds still play in quiet mode unless `--no-sounds` is set;
- logging writes raw and cleaned text without crashing the daemon.

Run daemon/IPC checks sequentially with isolated config, cache/runtime paths, and a model copy. After a configured idle timeout, verify the first PTT still captures and inserts once, then check status and history. Make the test model unavailable while offloaded to verify reload failure, restore it, and confirm the next PTT recovers. Check stop/restart and sleep/wake on the native desktop. For audible cues, idle offload is silent. Hold PTT through reload and listen for the three-note reload cue followed by the usual listening tone when ready. Release during reload and confirm there is no delayed listening tone; a failed reload must not announce readiness. A failure at PTT start plays one two-pulse error cue; if the release retry also fails, it plays a second. Check the normal completion/error cues, the error cue after a capture with no speech, and `--no-sounds` too. Headless tests verify cue ordering and cancellation, but do not validate audible playback, physical input, or insertion. Record unobserved human checks and unavailable backend runs as pending.

For allocation context, see the [Linux reload measurements and ownership notes](memory.md).

### Clipboard Checks

Portable tests cover snapshot format selection, unavailable formats versus real read errors, complete payload comparisons, restoration retry, sentinel recognition, and insertion reports/notifications. Native `doctor --deep` transactions still need an interactive desktop; follow the [Linux](../linux-desktop.md#deep-doctor-check), [macOS](../macos-desktop.md#doctor---deep), or [Windows](../windows-desktop.md#doctor---deep) procedure.

Use controlled text, HTML/text alternatives, images, copied files, and an empty clipboard. Independently read back restored contents: compare HTML and its text alternative, file paths/order, and image dimensions/pixels. Exercise failed cleanup and a newer competing copy where reproducible, checking the macOS diagnostic's [sentinel ownership limits](../macos-desktop.md#doctor---deep). Source checks and SDK-less cross-target compilation do not establish native clipboard serialization or GUI cleanup.

TODO: Reproduce a hung X11 clipboard owner and design bounded snapshot reads that preserve the prior clipboard. Arboard reads formats sequentially and its public API does not distinguish a read timeout from an unavailable format.

TODO: On a native Linux desktop, exercise direct typing and guarded paste with physical modifier/focus changes, key-probe failures, keyboard-map changes, readiness-error reconnection and clipboard-manager handoffs. Closure tests and Mac runs do not establish those X11 results. Windows generation changes and delayed rendering also need native clipboard-manager or RDP verification.

### Ordered audio and loaded Qwen failures

Repeat `transcribe-file --audio` to run short, long, and short inputs on one loaded engine. The hidden worker simulation also accepts repeated `--simulate-ptt-audio` paths; it replays the full ordered sequence per `--simulate-ptt-repeat` cycle, then applies the requested idle interval. This exercises resident context reuse before offload/reload. Its failure path reads history back and rejects replacement of the preceding successful transcript.

The loaded-session fault driver is a local Linux check with an explicit Qwen model:

```bash
python tests/native/run_qwen_faults.py \
  --native-build <isolated-target>/debug/build/parakit-<id>/out/build \
  --binary-dir <isolated-target>/debug \
  --model <qwen-q8.gguf> \
  --audio vendor/CrispASR/bindings/go/samples/jfk.wav \
  --output target/tmp/qwen-native-faults
```

First run `cargo test --test native_correctness_build` in the same isolated target directory to build the readiness-restoration test harness. Use its completed CPU native build directory containing `compile_commands.json`, and a fresh output directory. The driver compiles a separate corrected C API copy and relinks its own test library. Only that library contains fault controls; normal binaries and vendor sources are unchanged. First/middle/last chunk failures and greedy embedding/forward failures run through both file and worker paths. The worker first remembers a successful dictation, then checks history preservation and absence of successful logging/insertion for the failed input. Separate prefix cases inspect actual native English, Cantonese, and Filipino assistant prefills. They force those languages on English fixture audio to inspect prompt construction, not to measure multilingual recognition quality.

The remaining native desktop checklist is in [model-validation.md](model-validation.md).
