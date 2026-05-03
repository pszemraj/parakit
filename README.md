<!--
Agent notice: do not edit this README unless the user has explicitly asked for
or approved the exact README.md change you are about to make, literally and
verbatim. If that explicit approval is not present, do not touch this file.
-->

# parakit

> A [daemon](https://en.wikipedia.org/wiki/Daemon_(computing))-style[^1] local push-to-talk dictation **kit** powered by NVIDIA **Para**keet.

[^1]: Despite the daemon inspo, not everything in the design here tracks standard daemons such as `systemd`, therefore this repo is not using the `d` suffix naming convention to avoid confusion.

Hold `Ctrl+Space`, speak, release, and the transcript is inserted into the focused application.

Transcription runs locally via [NVIDIA Parakeet-TDT-0.6B-v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3), pre-quantized to [Q8_0 by default](https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf) through the vendored [CrispASR](https://github.com/CrispStrobe/CrispASR) runtime.

## Install

Install the native, OS-specific packages needed[^2] as explained in [docs/build.md](docs/build.md), then:

[^2]: These are mostly for audio streaming, hotkey monitoring, and text insertion.

```bash
git clone --recurse-submodules https://github.com/pszemraj/parakit.git
cd parakit
# if BLAS installed, add PARAKIT_BLAS=auto to below command
cargo install --path .
```

Make sure Cargo's bin directory is on `PATH`:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

## First Run

Run the `doctor` subcommand to preflight check, and start up the daemon:

```bash
parakit doctor && parakit
```

If `doctor` finds issues with the setup/build, it will exit 1 and print details on what's wrong. Otherwise, the following `parakit` command starts up the daemon. try it:

1. Switch to another app or text field, and put the cursor where you want text inserted.
2. Press and hold `Ctrl+Space`, say something, then release.
   - Sounds indicate start, stop, or error states.
3. Watch the dictated text appear at your cursor.

For background mode, paste options, and other runtime-related options see [docs/running.md](docs/running.md).

Linux currently requires an X11 session for desktop hotkeys and text insertion; see [docs/linux-desktop.md](docs/linux-desktop.md).

## Docs

- Build and native dependencies: [docs/build.md](docs/build.md)
- Running, control socket, model cache, logging, and paste modes: [docs/running.md](docs/running.md)
- Linux X11 and experimental evdev-proxy setup: [docs/linux-desktop.md](docs/linux-desktop.md)
- Cleanup rules: [docs/cleaning-rules.md](docs/cleaning-rules.md)
- Validation and quality checks: [docs/quality.md](docs/quality.md)
- Architecture and platform work: [docs/architecture.md](docs/architecture.md)
- Maintainer notes: [docs/dev.md](docs/dev.md)
- Python helpers: [scripts/README.md](scripts/README.md)
- Troubleshooting: [docs/troubleshooting.md](docs/troubleshooting.md)

## License

Project code is MIT. See [LICENSE](LICENSE). Downloaded model files keep their upstream licenses; Parakeet-TDT-0.6B-v3 weights are CC-BY-4.0.
