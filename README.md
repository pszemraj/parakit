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

No `-m` argument is needed for normal use. CPU tuning, accelerator builds, BLAS,
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

## Common Tasks

- Runtime options, logging, model cache commands, paste modes, and custom
  models: [docs/running.md](docs/running.md)
- Linux input permissions, tmux, X11, and `/dev/uinput`:
  [docs/linux-desktop.md](docs/linux-desktop.md)
- Cleanup rule behavior and rule testing:
  [docs/cleaning-rules.md](docs/cleaning-rules.md)
- File transcription and quality checks:
  [docs/quality.md](docs/quality.md)
- Common failures:
  [docs/troubleshooting.md](docs/troubleshooting.md)
- Maintainer source rebuilds from NVIDIA's `.nemo` checkpoint:
  [docs/dev.md#source-rebuild](docs/dev.md#source-rebuild)

## License

MIT. See [`LICENSE`](LICENSE).

`crispasr`, `cpal`, `x11rb`, `rdev`, `enigo`, `rodio`, `rubato`,
`regex`, `clap`, and other dependencies have their own licenses (mostly
MIT/Apache-2.0). The bundled CrispASR library is also MIT-licensed.
