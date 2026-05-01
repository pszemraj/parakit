# parakit

Local push-to-talk dictation for desktop work. Hold `Ctrl+Space`, speak,
release, and parakit inserts the transcript into the focused application.

parakit runs NVIDIA's Parakeet-TDT-0.6B-v3 locally through the vendored
CrispASR runtime. After startup preflights pass, the first successful daemon
startup downloads the default Q8_0 GGUF model and caches it; normal use does
not need a `-m` model argument.

## Install

Install the native packages for your OS from [docs/build.md](docs/build.md), then:

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

```bash
parakit doctor && parakit
```

Linux insertion requires an X11 session. Wayland sessions are rejected because
XTest cannot insert into focused native Wayland applications.

## Docs

- Build and native dependencies: [docs/build.md](docs/build.md)
- Running, model cache, logging, and paste modes: [docs/running.md](docs/running.md)
- Linux X11, evdev, and `/dev/uinput`: [docs/linux-desktop.md](docs/linux-desktop.md)
- Cleanup rules: [docs/cleaning-rules.md](docs/cleaning-rules.md)
- Troubleshooting: [docs/troubleshooting.md](docs/troubleshooting.md)

## License

MIT. See [LICENSE](LICENSE).
