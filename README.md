# parakit

Local push-to-talk dictation for desktop work. Hold `Ctrl+Space`, speak,
release, and parakit inserts the transcript into the focused application.

parakit runs NVIDIA's Parakeet-TDT-0.6B-v3 locally through the vendored
CrispASR runtime. The first daemon run downloads the default Q8_0 GGUF model
and caches it; normal use does not need a `-m` model argument.

## Install

Install the native packages for your OS from [docs/build.md](docs/build.md),
then build from source:

```bash
git clone --recurse-submodules https://github.com/pszemraj/parakit.git
cd parakit
git submodule update --init --recursive
cargo install --path .
```

Make sure Cargo's bin directory is on `PATH`:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

Windows support is experimental. Build requirements and DLL notes are in
[docs/build.md](docs/build.md).

## First Run

```bash
parakit doctor && parakit
```

For daily background use, start it from the current desktop session:

```bash
parakit --quiet &
disown
```

## Learn More

- Build and native dependencies: [docs/build.md](docs/build.md)
- Running, model cache, logging, and paste modes: [docs/running.md](docs/running.md)
- Linux X11, evdev, and `/dev/uinput`: [docs/linux-desktop.md](docs/linux-desktop.md)
- Cleanup rules: [docs/cleaning-rules.md](docs/cleaning-rules.md)
- Troubleshooting: [docs/troubleshooting.md](docs/troubleshooting.md)

## License

MIT. See [LICENSE](LICENSE).
