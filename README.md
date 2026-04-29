# parakit

Local push-to-talk dictation for desktop workflows.

Hold `Ctrl+Space`, speak, release, and parakit inserts the transcript into the
focused application. It runs locally with
[CrispASR](https://github.com/CrispStrobe/CrispASR) and NVIDIA's
[Parakeet-TDT-0.6B-v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3)
model.

Core behavior:

- fixed `Ctrl+Space` push-to-talk hotkey;
- automatic first-run model download for the default Q8_0 GGUF;
- CPU by default, with CUDA, Vulkan, and Metal build options;
- terminal-friendly paste mode by default;
- follows the current default input microphone;
- optional cleanup rules for filler words, repeated words, partial stutters,
  capitalization, and punctuation spacing;
- optional JSONL/TSV transcription logging of `(raw, cleaned)` text pairs.

Repository: [github.com/pszemraj/parakit](https://github.com/pszemraj/parakit)

## Install

Install native build dependencies first. On Ubuntu/Debian:

```bash
sudo apt install cmake build-essential pkg-config libasound2-dev libudev-dev \
  libxtst-dev libxdo-dev libxi-dev libx11-dev libevdev-dev libgomp1 \
  autoconf libtool
```

Then build and install:

```bash
git clone --recurse-submodules https://github.com/pszemraj/parakit.git
cd parakit

cargo install --path .
```

For many local CPU installs, this is a better build command because it enables
BLAS when MKL/OpenBLAS/Accelerate is available and falls back to the native ggml
CPU kernels when it is not:

```bash
PARAKIT_BLAS=auto cargo install --path .
```

Cargo installs the binary into Cargo's bin directory, usually `~/.cargo/bin`.
Make sure that directory is on `PATH`:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

Check the environment and start the daemon:

```bash
parakit doctor
parakit
```

The first `parakit` run downloads the default Q8_0 GGUF model if it is not
already cached. No `-m` argument is needed for normal use.

Backend-specific builds:

```bash
cargo install --path . --features cuda
cargo install --path . --features vulkan
cargo install --path . --features metal
```

More dependency lists, `PARAKIT_BLAS` options, backend SDK notes, rpath details,
and Windows DLL handling are covered in [docs/build.md](docs/build.md).

## Run

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

See [docs/running.md](docs/running.md) for background launch, model cache,
logging, microphone selection, paste modes, sounds, and streaming mode.

## Commands

```bash
# Download or redownload the hosted default model.
parakit fetch
parakit fetch --force

# Inspect the model cache.
parakit cache
parakit cache dir

# Show diagnostic startup paths, backend details, and timings.
parakit --verbose
parakit --threads 8 --verbose

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

Maintainer source rebuilds from NVIDIA's `.nemo` checkpoint are described in
[docs/dev.md](docs/dev.md#source-rebuild).

## Documentation

- [docs/build.md](docs/build.md): dependencies, backend features, bundled
  CrispASR, rpath, and Windows DLL handling.
- [docs/running.md](docs/running.md): foreground/background launch, quiet
  mode, model cache, logging, paste modes, and runtime modes.
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
