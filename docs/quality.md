# Quality Checks

Build success does not prove transcription quality. Use real user audio, not
synthetic TTS.

## File-Based parakit Transcription

Use the Rust example to run the same CrispASR engine and cleanup pipeline
without the hotkey daemon:

```bash
cargo run --example transcribe-file -- \
  --audio clips/example.wav
```

The helper accepts WAV input, uses the same `Engine` path as the daemon, applies
cleanup unless disabled, and prints raw and cleaned text. It uses the cached
Q8_0 model by default. Pass `--model /path/to/model.gguf` only when comparing a
specific custom GGUF.

## PTT Worker Simulation

Use the hidden simulation path when you need the daemon worker flow without a
live keyboard, microphone, or text insertion:

```bash
cargo run -- --paste-mode direct \
  --simulate-ptt-audio target/tmp/ptt-audio/Sitrep_2602_20_0145_first60.wav
```

It reads a WAV, resamples it to the model rate, sends
`RecordingStarted`/`RecordingStopped` events with owned PCM, runs inference and
cleanup, and prints the transcript. It does not test evdev capture or paste
insertion.

## NeMo Reference Helper

The Python helper runs NVIDIA NeMo's official Parakeet path:

```bash
python scripts/transcribe_nemo_parakeet.py clips/example.wav
```

Install details are in [scripts/README.md](../scripts/README.md). The script
has hard imports for PyTorch and NeMo so a broken reference environment fails
immediately.

## Recommended A/B Procedure

Use 5 to 10 real clips that cover:

- short and long utterances;
- clean and noisy audio;
- technical vocabulary;
- normal dictation pace;
- at least one clip with hesitations or restarts.

Compare against:

1. NeMo, parakeet-mlx, or another trusted Parakeet reference implementation.
2. parakit with an F16 GGUF model.
3. parakit with the canonical cached Q8_0 GGUF model.

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

If F16 differs materially from the reference, inspect CrispASR's Parakeet
preprocessor first. If F16 matches but Q8_0 is worse, suspect the hosted
artifact or quantizer path.

Parakeet-v3 auto-detects language. It has no parakit language flag today, so
non-English or code-switched dictation should be checked against real clips
before relying on it. Accents, noise, proper nouns, and technical vocabulary can
change language detection behavior.

## Runtime Smoke Checks

Run the daemon in foreground mode and exercise real applications:

- terminal;
- browser text fields;
- editor buffers;
- chat applications;
- rich-text editors.

Check:

- `Ctrl+Space` does not leak a literal space;
- rapid double-presses do not wedge the state machine;
- Ctrl release before Space stops recording;
- very short captures are padded and transcribed instead of rejected;
- `--quiet` emits no stdout;
- sounds still play in quiet mode unless `--no-sounds` is set;
- logging writes raw and cleaned text without crashing the daemon.

For a long-running check:

```bash
ps -o pid,rss,vsz -p "$(pgrep parakit)"
```

RSS should settle near model size plus runtime overhead.
