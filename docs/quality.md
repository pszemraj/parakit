# Validation

Build success does not prove transcription quality. Use real user audio, not synthetic TTS.

## WAV Quality Target

Use the Rust WAV target to run the same [CrispASR](https://github.com/CrispStrobe/CrispASR) engine without the hotkey daemon:

```bash
cargo run --no-default-features --features bundled --example transcribe-file -- \
  --audio clips/example.wav
```

The helper accepts WAV input, uses the same raw `Engine` path as the daemon, and prints raw inference text and timing. It intentionally does not import or apply parakit text-cleaning rules. This command avoids live daemon desktop/audio dependencies while keeping the bundled CrispASR build. Model cache behavior is in [running.md#model-cache](running.md#model-cache). Pass `--model /path/to/model.gguf` only when comparing a specific custom GGUF. The source lives at `examples/transcribe_file.rs`; it is a Cargo example target so it is not installed as an end-user binary.

## PTT Worker Simulation

Use the hidden simulation path when you need the daemon worker flow without a live keyboard, microphone, or text insertion:

```bash
cargo run -- \
  --simulate-ptt-audio target/tmp/ptt-audio/example.wav
```

Use a real WAV with a known transcript. The command resamples it to the model rate, sends `RecordingStarted`/`RecordingStopped` events with owned PCM, runs inference and cleanup, and prints the transcript. It does not test registered hotkeys, evdev-proxy capture, or paste insertion.

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

## Runtime Smoke Checks

Run the daemon in foreground mode and exercise real applications:

- terminal;
- browser text fields;
- editor buffers;
- chat applications;
- rich-text editors.

Check:

- `Ctrl+Space` does not leak a literal space;
- holding `Ctrl+Space` for 3 to 5 seconds produces exactly one recording start and one stop;
- rapid double-presses do not wedge the state machine;
- Ctrl release before Space stops recording;
- focus changes do not paste into the new target;
- very short captures are padded and transcribed instead of rejected;
- `--quiet` emits no stdout;
- warnings still go to stderr in quiet mode;
- sounds still play in quiet mode unless `--no-sounds` is set;
- logging writes raw and cleaned text without crashing the daemon.

For a long-running check:

```bash
ps -o pid,rss,vsz -p "$(pgrep parakit)"
```

RSS should settle near model size plus runtime overhead.
