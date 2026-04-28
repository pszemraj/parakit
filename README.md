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

## Status

| Platform | Status |
| --- | --- |
| Linux X11 | Supported. |
| Linux Wayland | Not supported for the daemon. Most compositors block global key grabs from regular clients. |
| Windows | Supported, with DLL placement requirements after build. |
| macOS | Expected to work with Accessibility and Input Monitoring permissions. |

See [docs/troubleshooting.md](docs/troubleshooting.md) for platform-specific
notes.

## Quickstart

```bash
git clone --recurse-submodules https://github.com/pszemraj/parakit.git
cd parakit

# Choose one backend.
cargo build --release --features cuda
cargo build --release --features vulkan
cargo build --release --features metal
cargo build --release

# Download a GGUF model.
huggingface-cli download CrispStrobe/parakeet-tdt-0.6b-v3-gguf \
  --include "*Q5_K_M.gguf" \
  --local-dir ./models

# Run in the foreground first.
./target/release/parakit -m models/parakeet-tdt-0.6b-v3-Q5_K_M.gguf
```

Expected foreground output is plain status text:

```text
parakit
  model:    models/parakeet-tdt-0.6b-v3-Q5_K_M.gguf
  mode:     Batch
  cleaning: on (72 rules)
  sounds:   on
  logging:  off
  audio:    48000 Hz hardware (resampling), 16000 Hz target
Ready: hold Ctrl+Space to dictate. Ctrl+C in this terminal to exit.
Recording...
Transcribing (3.42s audio, 3.61s wall)...
Raw:    So, um, the the cat sat on the mat.
Clean:  The cat sat on the mat  (212ms)
```

For normal use, run it quietly in the background:

```bash
./target/release/parakit -m "$PWD/models/parakeet-tdt-0.6b-v3-Q5_K_M.gguf" --quiet &
```

See [docs/running.md](docs/running.md) for background launch, `nohup`,
logging, and process management examples.

## Common Commands

```bash
# Run with transcription logging.
parakit -m models/parakeet-tdt-0.6b-v3.gguf --log-dir ~/.parakit/logs
parakit -m models/parakeet-tdt-0.6b-v3.gguf --log-dir ~/.parakit/logs --log-format tsv

# Disable sounds or cleanup.
parakit -m models/parakeet-tdt-0.6b-v3.gguf --no-sounds
parakit -m models/parakeet-tdt-0.6b-v3.gguf --no-cleaning

# Disable one cleanup rule.
parakit -m models/parakeet-tdt-0.6b-v3.gguf --disable-rule lead-so-comma

# Inspect or test cleanup rules without starting the daemon.
parakit --list-rules
parakit --test-rules "So, um, the the cat ran like, you know, fast"
```

The hotkey is fixed at `Ctrl+Space`. The literal space is suppressed so it
does not reach the focused application.

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
