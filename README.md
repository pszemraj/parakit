# parakit

Push-to-talk dictation for desktop use.

Hold `Ctrl+Space`, speak, release, and parakit types the transcript at the
current cursor. Text is inserted with synthetic keystrokes; the clipboard is
not used.

parakit is backed by [CrispASR](https://github.com/CrispStrobe/CrispASR) and
NVIDIA's [Parakeet-TDT-0.6B-v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3)
model. It includes an optional cleanup pass for filler words, repeated words,
partial stutters, and punctuation spacing.

Repository: [github.com/pszemraj/parakit](https://github.com/pszemraj/parakit)

## Quickstart

```bash
git clone --recurse-submodules https://github.com/pszemraj/parakit.git
cd parakit

cargo install --path . --locked
export PATH="$HOME/.cargo/bin:$PATH"

# One-time Python deps for converting NVIDIA's official .nemo checkpoint.
python -m pip install -r requirements-convert.txt

# Download, convert, and quantize the official model to cached Q8_0 GGUF.
parakit fetch

# Run in the foreground first.
parakit
```

`cargo install --path .` builds the release binary and installs it into
Cargo's binary directory, usually `~/.cargo/bin`. Use
`cargo install --path . --locked --features cuda` for CUDA; replace `cuda`
with `vulkan` or `metal` for those backends. Native dependencies and backend
notes are in [docs/build.md](docs/build.md).

Foreground startup prints the resolved model and waits for the hotkey:

```text
parakit
  model:    /home/user/.cache/parakit/models/parakeet-tdt-0.6b-v3-Q8_0.gguf
  dtype:    Q8_0
  mode:     Batch
  cleaning: on (72 rules)
  sounds:   on
  logging:  off
Ready: hold Ctrl+Space to dictate. Ctrl+C in this terminal to exit.
```

For normal use, run it quietly in the background:

```bash
parakit --quiet &
```

See [docs/running.md](docs/running.md) for background launch, `nohup`,
logging, and process management examples.

## Common Commands

```bash
# Rebuild cached model artifacts from the official checkpoint.
parakit fetch --force
parakit fetch --keep-nemo
parakit fetch --keep-f16

# Run with transcription logging.
parakit --log-dir ~/.parakit/logs
parakit --log-dir ~/.parakit/logs --log-format tsv

# Disable sounds or cleanup.
parakit --no-sounds
parakit --no-cleaning

# Disable one cleanup rule.
parakit --disable-rule lead-so-comma

# Use a custom GGUF instead of the cached Q8_0.
parakit -m /path/to/model.gguf

# Inspect or test cleanup rules without starting the daemon.
parakit --list-rules
parakit --test-rules "So, um, the the cat ran like, you know, fast"
```

The hotkey is fixed at `Ctrl+Space`. The literal space is suppressed before it
reaches the focused application.

## Model Setup

`parakit fetch` owns the canonical model pipeline:

1. download NVIDIA's official `.nemo` checkpoint;
2. convert it to an intermediate F16 GGUF with CrispASR's converter;
3. quantize it to the default Q8_0 GGUF with `crispasr-quantize`.

The final model is cached at:

```text
~/.cache/parakit/models/parakeet-tdt-0.6b-v3-Q8_0.gguf
```

`XDG_CACHE_HOME` is honored on Linux. macOS uses
`~/Library/Caches/parakit/models/`, and Windows uses
`%LOCALAPPDATA%\parakit\Cache\models\`.

The downloaded `.nemo` and intermediate F16 GGUF are deleted after a successful
fetch unless `--keep-nemo` or `--keep-f16` is passed. `-m <path>` always
overrides the cached model.

## Documentation

- [docs/build.md](docs/build.md): dependencies, backend features, bundled
  CrispASR, rpath, and Windows DLL handling.
- [docs/running.md](docs/running.md): foreground/background launch, quiet
  mode, logging, sounds, and runtime modes.
- [docs/architecture.md](docs/architecture.md): thread model, event flow,
  module boundaries, and ownership constraints.
- [docs/cleaning-rules.md](docs/cleaning-rules.md): cleanup rule behavior,
  how to add rules, and how to test regressions.
- [docs/quality.md](docs/quality.md): file transcription helpers and quality
  comparison workflow.
- [docs/troubleshooting.md](docs/troubleshooting.md): common build and
  runtime failures.

## License

MIT. See [`LICENSE`](LICENSE).

`crispasr`, `cpal`, `rdev`, `enigo`, `rodio`, `rubato`, `regex`, `clap`,
and other dependencies have their own licenses (mostly MIT/Apache-2.0).
The bundled CrispASR library is also MIT-licensed.
