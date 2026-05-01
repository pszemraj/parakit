# parakit

Local push-to-talk dictation for desktop work. Hold `Ctrl+Space`, speak,
release, and parakit inserts the transcript into the focused application.

parakit runs NVIDIA's Parakeet-TDT-0.6B-v3 locally through the vendored
CrispASR runtime. After startup preflights pass, the first successful daemon
startup downloads the default Q8_0 GGUF model and caches it; normal use does
not need a `-m` model argument.

## Install

Install the native, OS-specific packages needed[^1] as explained in [docs/build.md](docs/build.md), then:

[^1]: these are mostly audio stream handling + monitoring (if shortcut pressed) related

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

If `doctor` finds issues with the setup/build, it will exit 1 and display detials on what is wrong. Otherwise, the following `parakit` starts up the daemon and you're ready to try it:

1. Switch to anywhere else, and put the cursor where you want to dictate into
2. press & hold CTRL+space, say something, then let go of CTRL+space
  - sounds are played to indicate: started listening for dictation, finished listening, or an error
3. observe your dictated text appear at your cursor

More details are in the console outputs. That's it! For info on how to run parakit as a background process & other adv options, see [docs/running.md](docs/running.md)

**gotcha:** Linux insertion requires an X11 session[^2]

[^2]: Wayland sessions are rejected because XTest cannot insert into focused native Wayland applications. This is a common limitation across dictation tools

## Docs

- Build and native dependencies: [docs/build.md](docs/build.md)
- Running, model cache, logging, and paste modes: [docs/running.md](docs/running.md)
- Linux X11, evdev, and `/dev/uinput`: [docs/linux-desktop.md](docs/linux-desktop.md)
- Cleanup rules: [docs/cleaning-rules.md](docs/cleaning-rules.md)
- Troubleshooting: [docs/troubleshooting.md](docs/troubleshooting.md)

## License

MIT. See [LICENSE](LICENSE).
