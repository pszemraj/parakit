# parakit

Local push-to-talk dictation for desktop work. Hold `Ctrl+Space`, speak, release, and parakit inserts the transcript into the focused application.

parakit runs [NVIDIA Parakeet-TDT-0.6B-v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3) locally through the vendored [CrispASR](https://github.com/CrispStrobe/CrispASR) runtime. The default model is a [Q8_0 GGUF build hosted at pszemraj/parakeet-tdt-0.6b-v3-gguf](https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf).

## Install

Install the native packages in [docs/build.md](docs/build.md), then:

```bash
git clone --recurse-submodules https://github.com/pszemraj/parakit.git
cd parakit
cargo install --path .
```

Make sure Cargo's bin directory is on `PATH`:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

## First Run

Run the doctor subcommand and start up the daemon.

```bash
parakit doctor && parakit
```

If `doctor` finds issues with the setup/build, it will exit 1 and display details on what is wrong. Otherwise, the following `parakit` starts up the daemon and you're ready to try it:

1. Switch to another app or text field, and put the cursor where you want text inserted.
2. Press and hold `Ctrl+Space`, say something, then release.
   - Sounds indicate start, stop, or error states.
3. Watch the dictated text appear at your cursor.

For background mode, model cache paths, logging, and paste options, see [docs/running.md](docs/running.md).

Linux currently requires an X11 session for the default hotkey and insertion path; see [docs/linux-desktop.md](docs/linux-desktop.md).

## Bluetooth Microphones

Bluetooth microphones and Bluetooth headset microphones are allowed, but they are a poor fit for push-to-talk dictation. Headset profiles often add buffering, clip the beginning of speech, change sample rates or routes unexpectedly, and apply aggressive gain or noise processing. Bluetooth output is fine; Bluetooth input is not recommended.

## Docs

- Build and native dependencies: [docs/build.md](docs/build.md)
- Running, model cache, logging, and paste modes: [docs/running.md](docs/running.md)
- Linux X11 and experimental evdev-proxy setup: [docs/linux-desktop.md](docs/linux-desktop.md)
- Cleanup rules: [docs/cleaning-rules.md](docs/cleaning-rules.md)
- Troubleshooting: [docs/troubleshooting.md](docs/troubleshooting.md)

## License

MIT. See [LICENSE](LICENSE).
