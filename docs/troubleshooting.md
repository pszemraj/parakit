# Troubleshooting

Start with diagnostics:

```bash
parakit doctor
parakit doctor --deep
```

`doctor` does not load the model. `--deep` adds an active insertion smoke test.
Normal launch behavior is covered in [running.md](running.md).

## Hotkey Problems

On Linux, `auto` uses evdev when all input devices are readable; otherwise it
uses the X11 desktop hotkey backend. Wayland usually blocks this class of
desktop automation.

Healthy X11 output without evdev access looks like:

```text
desktop:        X11 desktop hotkey
desktop status: OK
evdev:          rdev grab (partial permissions)
status:         OK (desktop hotkey backend)
```

If `Ctrl+Space` is unavailable, another desktop shortcut or input method may
own it. Disable that binding and rerun `parakit doctor`.

If `doctor` reports `Connection refused` for X11 after a logout/login, restart
the terminal or tmux server from the current desktop session. Use the evdev
backend when the daemon must survive desktop session churn; see
[linux-desktop.md](linux-desktop.md).

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
shortcut, then restores the previous text clipboard when possible. See
[running.md#insertion](running.md#insertion) for paste modes.

Useful checks:

```bash
parakit doctor --deep
parakit --paste-mode terminal
parakit --paste-mode standard
parakit --paste-mode direct
```

Use `standard` for apps that only accept `Ctrl+V`; use `direct` only when an
app refuses clipboard paste entirely. Streaming insertion uses synthetic typing
and is not the recommended path for reliability testing.

On Windows, paste shortcuts are sent with `SendInput`. Windows blocks synthetic
input into higher-integrity processes, so a normal parakit process cannot paste
into an administrator/elevated target application. Security software can also
flag global hooks plus text insertion; whitelist the binary when needed.

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
