# Troubleshooting

Start with:

```bash
parakit doctor
```

It reports hotkey access, selected microphone, and build flags without loading
the model.

## Hotkey Problems

On Linux, the preferred path is the X11 desktop hotkey backend. Wayland usually
blocks global hotkeys and synthetic input for regular client applications.

Healthy X11 output looks like:

```text
primary:        X11 desktop hotkey
primary status: OK
status:         OK (desktop hotkey backend)
```

If `Ctrl+Space` is unavailable, another desktop shortcut or input method may
own it. Disable that binding and rerun `parakit doctor`.

If parakit must use the low-level evdev fallback, grant input access and start a
new login session:

```bash
sudo usermod -aG input "$USER"
id -nG | tr ' ' '\n' | grep '^input$'
```

Log out and back in, or reboot. Restart tmux and terminals that were open before
the group change. Avoid running parakit with `sudo`; audio, X11, and text
insertion usually belong to the regular desktop user.

If the hotkey stops after lock/unlock, parakit should refresh the X11
registration within a few seconds. If it does not, rerun with `--verbose` and
check whether `Ctrl+Space` became owned by the desktop or input method.

On macOS, grant Accessibility and Input Monitoring permissions to both the
terminal and the built binary.

## Literal Space Appears

The active backend should suppress the literal Space in `Ctrl+Space`. If a
space reaches the focused app:

- confirm only one parakit process is running;
- confirm no desktop/input-method shortcut also handles `Ctrl+Space`;
- retry in foreground mode to inspect errors;
- avoid Wayland sessions.

## Text Does Not Insert

Batch insertion writes the transcript to the clipboard, sends the paste
shortcut, then restores the previous text clipboard when possible. Clipboard
managers may still record the transient transcript.

The default paste mode is terminal-friendly:

```bash
parakit --paste-mode terminal
parakit --paste-mode standard
```

Use `standard` for apps that only accept `Ctrl+V`.

Streaming insertion uses synthetic typing. On Linux, X11 is the supported path.
Wayland usually blocks synthetic key events.

On Windows, security software can flag global hooks plus text insertion.
Whitelist the binary when needed.

## Wrong Microphone

parakit follows the OS default input and avoids monitor/virtual sources when it
can.

On PipeWire/PulseAudio:

```bash
pactl list sources | grep -E 'Description:|Sample Specification:' | grep -v monitor
parakit doctor
```

If the reported microphone is wrong, change the default input in desktop sound
settings or `pavucontrol`, then wait a few seconds or restart parakit.

## Build And Model Issues

Missing CrispASR path dependency:

```text
failed to read vendor\CrispASR\crispasr\Cargo.toml
```

The git submodule is missing. Fix the existing checkout:

```bash
git submodule update --init --recursive
```

On Windows, prefer:

```powershell
pwsh -ExecutionPolicy Bypass -File scripts/install-windows.ps1
```

Shared library loading on Linux:

```bash
ldd target/debug/parakit | grep -E "whisper|ggml"
readelf -d target/debug/parakit | grep -E "RPATH|RUNPATH"
```

The paths should point into `target/debug/build/parakit-*/out/lib`, and
`readelf` should report `RPATH`. More detail is in
[build.md](build.md#runtime-library-paths).

Vulkan failing on `spirv/unified1/spirv.hpp` means `spirv-headers` is missing:

```bash
sudo apt install spirv-headers
cargo build --release --features vulkan
```

Windows builds need generated DLLs next to the executable or on `PATH`; see
[build.md](build.md#windows-dlls).

Model cache commands:

```bash
parakit fetch --force
parakit cache
parakit cache dir
```

With no `-m`, parakit downloads the default Q8_0 model on first run. A custom
`-m <path>` always wins and disables automatic fetch.
