# parakit

Local push-to-talk dictation for desktop workflows. Hold `Ctrl+Space`, speak,
release, and parakit inserts the transcript into the focused application.

parakit runs NVIDIA's
[Parakeet-TDT-0.6B-v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3)
locally through [CrispASR](https://github.com/CrispStrobe/CrispASR). The first
daemon run downloads the default hosted Q8_0 GGUF model and caches it.

Core behavior:

- fixed `Ctrl+Space` push-to-talk;
- automatic first-run model download;
- CPU by default, with CUDA, Vulkan, and Metal build options;
- terminal-friendly paste mode by default;
- follows the current default input microphone;
- optional cleanup rules and JSONL/TSV text logging.

Repository: [github.com/pszemraj/parakit](https://github.com/pszemraj/parakit)

## Install

Install the native dependencies in [docs/build.md](docs/build.md), then clone
and build:

```bash
git clone --recurse-submodules https://github.com/pszemraj/parakit.git
cd parakit
git submodule update --init --recursive
cargo install --path .
```

For local CPU installs with available BLAS libraries, use:

```bash
PARAKIT_BLAS=auto cargo install --path .
```

Windows PowerShell:

```powershell
pwsh -ExecutionPolicy Bypass -File scripts/install-windows.ps1
```

Cargo installs the binary into Cargo's bin directory, usually `~/.cargo/bin` on
Linux/macOS or `%USERPROFILE%\.cargo\bin` on Windows. Make sure that directory
is on `PATH`.

Linux/macOS:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

Check the environment and start the daemon:

```bash
parakit doctor && parakit
```

No `-m` argument is needed for normal use.

More dependency lists, `PARAKIT_BLAS` options, backend SDK notes, rpath details,
and Windows DLL handling are covered in [docs/build.md](docs/build.md).

## Run

For daily use, start it from the current desktop session and detach it:

```bash
parakit --quiet &
disown
```

See [docs/running.md](docs/running.md) for foreground output, background
launch, model cache, logging, microphone selection, paste modes, sounds, and
disabled streaming mode.

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

Maintainer source rebuilds from NVIDIA's `.nemo` checkpoint are in
[docs/dev.md#source-rebuild](docs/dev.md#source-rebuild).

## Documentation

- [docs/build.md](docs/build.md): dependencies, backend features, CrispASR,
  rpath, BLAS, and Windows DLL handling.
- [docs/running.md](docs/running.md): launch, quiet mode, model cache,
  logging, paste modes, microphones, and runtime modes.
- [docs/linux-desktop.md](docs/linux-desktop.md): Linux hotkey backends,
  desktop session auth, tmux, and evdev setup.
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

`crispasr`, `cpal`, `x11rb`, `rdev`, `enigo`, `rodio`, `rubato`,
`regex`, `clap`, and other dependencies have their own licenses (mostly
MIT/Apache-2.0). The bundled CrispASR library is also MIT-licensed.
