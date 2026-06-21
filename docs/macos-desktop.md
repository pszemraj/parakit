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
scripts/macos/install.sh --locked
```

The macOS installer builds from source, installs `parakit` under `~/.cargo/bin`, copies the generated CrispASR/ggml dylibs under `~/.cargo/lib/parakit`, and patches the installed binary to use `@executable_path/../lib/parakit`. The installed binary does not depend on the repository `target/` tree.

`aarch64-apple-darwin` is the supported macOS target. If `parakit --verbose doctor` reports a Rosetta or non-aarch64 warning, rebuild from a native arm64 terminal.

## Permissions

Grant these in System Settings > Privacy & Security:

- Accessibility: required for the `Ctrl+Space` hotkey and synthetic paste/type events.
- Microphone: required for audio capture.

Grant permissions to the terminal application that launches parakit, such as Terminal.app, iTerm2, or Ghostty. This is the recommended source-build flow because the grant attaches to the terminal's stable app identity and survives parakit rebuilds.

Input Monitoring is reported by `doctor` for diagnostics, but it is not a separate required parakit toggle when Accessibility is granted.

First-run flow:

```bash
parakit doctor
parakit
```

If Accessibility is missing, `doctor` can trigger the macOS prompt. After granting the permission, rerun `parakit doctor`. If Microphone is not determined yet, the first capture may trigger the Microphone prompt; rerun parakit after granting it.

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

## Focus Guard

On macOS, parakit records the frontmost app at push-to-talk down using `NSWorkspace`. Before insertion, it checks that the same bundle identifier and process id are still frontmost. If focus changed or cannot be verified, automatic paste is skipped and the transcript remains recoverable through clipboard fallback and daemon commands.

## Metal Verification

Use verbose doctor output to confirm the Metal build and visible compute device:

```bash
parakit --verbose doctor
```

For release builds, the Metal backend should be in the generated sibling library directory:

```bash
ls "$HOME/.cargo/lib/parakit"/libggml-metal.dylib
otool -L "$HOME/.cargo/lib/parakit/libggml.dylib"
otool -l "$HOME/.cargo/bin/parakit" | grep -A2 LC_RPATH
otool -s __DATA __ggml_metallib "$HOME/.cargo/lib/parakit/libggml-metal.dylib" | head
```

## Troubleshooting

If the hotkey does nothing, grant Accessibility to the terminal, quit and restart parakit, then rerun `parakit doctor`.

If the hotkey worked and later stops after changing privacy settings or after a macOS input-event issue, restart parakit. Restarting recreates the event tap.

If `--device gpu` reports no GPU on Apple Silicon, run `parakit --verbose doctor`. A Rosetta warning means the process is translated; reinstall from a native arm64 terminal.
