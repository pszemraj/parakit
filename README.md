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

cargo install --path .
export PATH="$HOME/.cargo/bin:$PATH"

parakit doctor

# First run checks the hotkey backend, then downloads the default Q8_0 GGUF.
parakit
```

`cargo install --path .` builds the release binary and installs it into
Cargo's binary directory, usually `~/.cargo/bin`. Use
`cargo install --path . --features cuda` for CUDA; replace `cuda` with
`vulkan` or `metal` for those backends. Native dependencies and backend notes
are in [docs/build.md](docs/build.md).

Foreground startup prints the model file, precision, and selected microphone:

```text
parakit
  model: parakeet-tdt-0.6b-v3-Q8_0.gguf
  dtype: Q8_0 (745 MB)
  mic:   RODE NT-USB+ Mono, 48000 Hz input -> 16000 Hz model, mono, F32
Ready: hold Ctrl+Space to dictate.
```

For normal use, run it quietly in the background:

```bash
parakit --quiet &
```

See [docs/running.md](docs/running.md) for background launch, `nohup`,
logging, and process management examples.

## Common Commands

```bash
# Download the hosted default model again.
parakit fetch --force

# Inspect the model cache.
parakit cache
parakit cache dir

# Show diagnostic startup paths, backend details, and timings.
parakit --verbose
parakit --threads 8 --verbose

# Rebuild Q8_0 locally from NVIDIA's official checkpoint.
python -m pip install -r requirements-convert.txt
parakit fetch --from-source
parakit fetch --from-source --keep-nemo
parakit fetch --from-source --keep-f16

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
reaches the focused application. On Linux/X11, parakit uses a desktop hotkey
registration first, so ordinary X11 sessions do not need `/dev/input` access.
The low-level evdev backend is only a fallback; it requires explicit input
device permissions.

## Model Setup

When no `-m` path is supplied, startup ensures the default Q8_0 model exists
and then loads it. The default model is downloaded from:

```text
https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf
```

The downloaded GGUF is SHA256-verified before it is accepted.
The same hosted repository can also hold F16 and other quantized GGUF files;
the current startup default remains Q8_0.

The final model is cached at:

```text
~/.cache/parakit/models/parakeet-tdt-0.6b-v3-Q8_0.gguf
```

`XDG_CACHE_HOME` is honored on Linux. macOS uses
`~/Library/Caches/parakit/models/`, and Windows uses
`%LOCALAPPDATA%\parakit\Cache\models\`.

The downloaded `.nemo` and intermediate F16 GGUF are deleted after a successful
source rebuild unless `--keep-nemo` or `--keep-f16` is passed. `-m <path>`
always overrides the cached model and disables automatic fetch.

Use `parakit cache` to list cached GGUF files, dtypes, sizes, and the default
Q8_0 checksum status. Use `parakit cache dir` for scripts that need the cache
directory.

`parakit fetch --from-source` is the reproducible maintainer path: download
NVIDIA's official `.nemo`, convert it to GGUF with CrispASR's Python converter,
then quantize it to Q8_0 with `crispasr-quantize`. It requires the Python
packages in `requirements-convert.txt`.

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
- [docs/dev.md](docs/dev.md): model artifact policy, source rebuild notes, and
  project TODOs.

## License

MIT. See [`LICENSE`](LICENSE).

`crispasr`, `cpal`, `global-hotkey`, `rdev`, `enigo`, `rodio`, `rubato`,
`regex`, `clap`, and other dependencies have their own licenses (mostly
MIT/Apache-2.0). The bundled CrispASR library is also MIT-licensed.
