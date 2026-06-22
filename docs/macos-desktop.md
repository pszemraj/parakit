# macOS Desktop Setup

parakit supports Apple Silicon macOS as a terminal-run CLI. Build from source, grant the terminal the required privacy permissions once, then run `parakit` from that terminal.

## Build

Install Xcode command line tools and the build helpers:

```bash
xcode-select --install
brew install cmake autoconf automake libtool pkg-config
```

Build or install the native Apple Silicon binary:

```bash
cargo install --path . --features metal
```

The macOS source install puts `parakit` under `~/.cargo/bin` and uses an rpath into the repository's generated CrispASR/ggml library directory. This matches Linux source installs. Do not delete the repository `target/` tree after installing.

`aarch64-apple-darwin` is the supported macOS target. If `parakit --verbose doctor` reports a Rosetta or non-aarch64 warning, rebuild from a native arm64 terminal.

## Permissions

Grant these in System Settings > Privacy & Security:

- Accessibility: required for the `Left Control+Space` hotkey and synthetic paste/type events.
- Microphone: required for audio capture.

Grant permissions to the terminal application that launches parakit, such as Terminal.app, iTerm2, or Ghostty. This is the recommended source-build flow because the grant attaches to the terminal's stable app identity and survives parakit rebuilds.

Input Monitoring is reported by `doctor` for diagnostics, but it is not a separate required parakit toggle when Accessibility is granted.

First-run flow:

```bash
parakit doctor
parakit
```

If Accessibility is missing, `doctor` can trigger the macOS prompt. After granting the permission, rerun `parakit doctor`. If Microphone is not determined yet, the first capture may trigger the Microphone prompt; rerun parakit after granting it.

## Hotkey

The default macOS push-to-talk hotkey is `Left Control+Space`. This deliberately avoids `Command+Space`, which is normally Spotlight. Press and hold `Left Control+Space` while speaking, then release when done. parakit handles the chord with a CoreGraphics event tap and suppresses the Space key while the exact chord is active. Modified chords such as `Left Control+Shift+Space`, `Control+Option+Space`, or `Command+Space` pass through to macOS and the focused app.

macOS may also use `Control+Space` for input-source switching when multiple input sources are configured. If parakit does not react, or if the input-source switcher appears instead:

1. Open System Settings > Keyboard > Keyboard Shortcuts.
2. Check Input Sources for shortcuts assigned to `Control+Space` or `Control+Option+Space`, and disable or change them.
3. Check other shortcut categories for warning icons; macOS marks conflicting shortcuts there.
4. Restart parakit and rerun `parakit doctor`.

Custom hotkeys are deferred to a future config file. Until then, macOS has one default hotkey.

## Paths

macOS uses the same XDG-style cache layout as Linux:

```text
~/.cache/parakit/models
~/.cache/parakit/run
```

`PARAKIT_MODELS_DIR` still overrides the model directory.

## Background Use

Start parakit from a terminal in the active desktop login:

```bash
parakit doctor && parakit --quiet &
disown
```

Keep stderr in a file:

```bash
mkdir -p "$HOME/.local/state/parakit"
nohup parakit --quiet >/dev/null 2>>"$HOME/.local/state/parakit/parakit.err" &
```

## Insertion

macOS insertion behavior, focus-change handling, and recovery commands are in [running.md#insertion](running.md#insertion).

## Metal Verification

Use verbose doctor output to confirm the Metal build and visible compute device:

```bash
parakit --verbose doctor
```

For release builds, the Metal backend should be in the generated sibling library directory:

```bash
ls target/release/build/parakit-*/out/lib/libggml-metal.dylib
otool -L target/release/build/parakit-*/out/lib/libggml.dylib
otool -l "$HOME/.cargo/bin/parakit" | grep -A2 LC_RPATH
otool -s __DATA __ggml_metallib target/release/build/parakit-*/out/lib/libggml-metal.dylib | head
```

## Troubleshooting

If the hotkey does nothing, grant Accessibility to the terminal, quit and restart parakit, then rerun `parakit doctor`.

If the hotkey worked and later stops after changing privacy settings, restart parakit. The event tap tries to re-enable itself after macOS disables it, but privacy changes can still require a fresh process.

If `--device gpu` reports no GPU on Apple Silicon, run `parakit --verbose doctor`. A Rosetta warning means the process is translated; reinstall from a native arm64 terminal.
