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

Replace `<hash>` with the directory from the successful native build; do not
point the fallback at an unrelated or stale library.

## Rust Source Coverage

Install Rust's coverage tools once:

```bash
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov --locked
```

Run the workspace tests with source instrumentation and print the Rust
coverage summary:

```bash
cargo coverage
```

This alias uses the default features and Cargo test-target surface, including the test-enabled `transcribe-file` example. Use `cargo coverage --all-targets` for the remaining Cargo target kinds, and pass explicit feature flags when measuring feature-gated code.

Generate a browsable per-line report when investigating an untested branch:

```bash
cargo coverage-html
```

The aliases are defined in [`.cargo/config.toml`](../../.cargo/config.toml).
By default, `cargo-llvm-cov` keeps instrumented artifacts under
`target/llvm-cov-target`, separate from ordinary builds. Reports cover
parakit's Rust code, not linked CrispASR C/C++ code. They also cover only
code compiled for the current host and feature set, so macOS coverage does
not validate Linux or Windows branches. Use coverage to locate test gaps; do
not treat a percentage as evidence of transcription quality, desktop
integration, or paste safety.

## WAV Quality Target

Use the Rust WAV target to run the same [CrispASR](https://github.com/CrispStrobe/CrispASR) engine without the hotkey daemon:

```bash
cargo run --release --no-default-features --features bundled --example transcribe-file -- \
  --audio clips/example.wav
```

The helper accepts WAV input, uses the same raw `Engine` path as the daemon, and prints raw inference text and timing. It intentionally does not import or apply parakit text-cleaning rules. This command avoids live daemon desktop/audio dependencies while keeping the bundled CrispASR build. Model cache behavior is in [running.md#model-cache](../running.md#model-cache). Pass `--model /path/to/model.gguf` only when comparing a specific custom GGUF. The source lives at [examples/transcribe_file.rs](../../examples/transcribe_file.rs); it is a Cargo example target so it is not installed as an end-user binary.

For latency work, use short real clips around 2s, 5s, 15s, and 25s. Longer quality clips are still useful for catching transcription drift, but they should not drive startup or post-release latency policy. Use repeatable `--warmup-seconds N` flags when calibrating GPU cold-start behavior; each flag runs one synthetic warmup pass before timing the real clip.

## PTT Worker Simulation

The Rust suite also runs the production worker loop with a fake inference engine.
It checks timed offload, audio queued during a blocked reload, one transcription
and completion, transcript history, and session destruction without a model or
a minute-long timeout.

Use the hidden simulation path when you need the daemon worker flow without a live keyboard, microphone, or text insertion:

```bash
PARAKIT_CONFIG_PATH=$PWD/target/tmp/ptt-missing.toml \
XDG_CACHE_HOME=$PWD/target/tmp/ptt-cache \
  cargo run -- start -m path/to/model.gguf --threads 8 --no-cleaning \
  --simulate-ptt-audio local-scratch/juniper-voicememo-DO_NOT_DELETE.wav \
  --model-idle-minutes 1 --simulate-ptt-repeat 2 --simulate-ptt-idle-seconds 61
```

Use a real WAV with a known transcript. The pinned Juniper voice memo is 55.36 seconds; its local filename can vary by machine. Compare the readable transcript with task-appropriate tolerance rather than hashing the fixture. The command resamples it to the model rate and sends worker start/stop events with owned PCM. `--no-cleaning` exposes raw output for comparison; omit it to exercise cleanup too. Add the build feature and `--device` for the backend being tested. On PowerShell, set the environment variables with `$env:NAME = ...` and use the Windows WAV path.

The worker acknowledges each required transcript, checks exact parity across repeats in the same process, and verifies the expected residency after each wait (including the last). This parity check verifies that reloading recreates the same session; it is not a cross-build reference-transcript comparison. Missing transcripts, reload failures, or missed offloads return a failure exit status. An immediate simulated release exercises queuing while reload is busy. Use `--model-idle-minutes 0` to check disabled offloading. For one dictation without an idle wait, omit `--simulate-ptt-repeat` and `--simulate-ptt-idle-seconds`; both options require `--simulate-ptt-audio`.

For ten-cycle memory comparisons and OS-specific measurements, use the [memory harness](memory.md). This is separate from the real-time worker timeout test.

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

1. NeMo or another trusted Parakeet reference implementation.
2. parakit with an F16 GGUF model.
3. parakit with the cached Q8_0 GGUF model.

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

The audit applies built-in rules only. It does not load `config.toml` or `[[rules.user]]`; pass profile, threshold, trailing-period, and disabled-rule choices explicitly. Use `parakit rules test` to validate the currently configured user rules. Number-conversion changes must be evaluated as `text2num` integration and context-formatting changes, not by adding a second local number grammar.

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

Run daemon/IPC checks sequentially with isolated config, cache/runtime paths,
and a model copy. After a configured idle timeout, verify the first PTT still
captures and inserts once, then check status and history. Make the test model
unavailable while offloaded to verify reload failure, restore it, and confirm
the next PTT recovers. Check stop/restart and sleep/wake on the native desktop.
For audible cues, idle offload is silent. Hold PTT through reload and listen for
the three-note reload cue followed by the usual listening tone when ready.
Release during reload and confirm there is no delayed listening tone; a failed
reload must not announce readiness. A failure at PTT start plays one two-pulse
error cue; if the release retry also fails, it plays a second. Check the normal
completion/error cues and `--no-sounds` too. Headless tests verify cue ordering and cancellation, but do
not validate audible playback, physical input, or insertion. Record unobserved
human checks and unavailable backend runs as pending.

For allocation comparisons, use the [memory harness and platform measurements](memory.md).

TODO: Reproduce a hung X11 clipboard owner and design bounded snapshot reads that preserve the prior clipboard. Arboard reads formats sequentially and its public API does not distinguish a read timeout from an unavailable format.
