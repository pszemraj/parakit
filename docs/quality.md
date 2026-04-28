# Quality Checks

Build success does not prove transcription quality. parakit exists because ASR
preprocessing details matter, so quality checks should use real audio from the
target user.

## File-Based parakit Transcription

Use the Rust file helper to run the same CrispASR engine and cleanup pipeline
without the hotkey daemon:

```bash
cargo run --example transcribe-file -- \
  --model models/parakeet-tdt-0.6b-v3.gguf \
  --audio clips/example.wav
```

The helper accepts WAV input, mixes to mono, resamples to 16 kHz, runs
`Engine::transcribe`, applies cleanup unless disabled, and prints raw and
cleaned text.

## NeMo Reference Helper

The Python helper runs NVIDIA NeMo's official Parakeet path:

```bash
python scripts/transcribe_nemo_parakeet.py clips/example.wav
```

Install details are in [scripts/README.md](../scripts/README.md). The script
has hard imports for PyTorch and NeMo so a broken reference environment fails
immediately.

## Recommended A/B Procedure

Use 5 to 10 real clips:

- short and long utterances;
- clean and noisy audio;
- technical vocabulary;
- normal dictation pace;
- at least one clip with hesitations or restarts.

Compare:

1. NeMo or another trusted Parakeet reference implementation.
2. parakit with an F16 GGUF model.
3. parakit with the intended quantized model, such as Q5_K_M.

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
preprocessor first. If F16 matches but Q5_K_M is worse, prefer F16 for daily
use.

## Runtime Smoke Checks

After quality parity is established, run the daemon in foreground mode and
exercise real applications:

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
