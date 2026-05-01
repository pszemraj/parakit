# Troubleshooting

Start with diagnostics:

```bash
parakit doctor
parakit --verbose doctor
parakit doctor --deep
```

`doctor` does not load the model. It exits `0` when the daemon should start and
`1` when a blocking issue remains, so `parakit doctor && parakit` is the safe
launch pattern. Normal launch behavior is covered in [running.md](running.md).

## Hotkey Problems

Linux backend and permission setup is in
[linux-desktop.md](linux-desktop.md).

If `Ctrl+Space` is unavailable, another desktop shortcut, input method, or
keyboard remapper may own it. Disable that binding and rerun `parakit doctor`.

If `doctor` reports `Connection refused` for X11 after a logout/login, start a
new terminal or tmux server from the current desktop session.

On macOS, grant Accessibility and Input Monitoring permissions to both the
terminal and the built binary.

## Literal Space Appears

The active backend should suppress the literal Space in `Ctrl+Space`. If a
space reaches the focused app:

- confirm only one parakit process is running;
- confirm no desktop/input-method shortcut also handles `Ctrl+Space`;
- retry in foreground mode to inspect errors;
- use an X11 session for Linux insertion.

## Text Does Not Insert

Batch insertion writes the transcript to the clipboard, sends the paste
shortcut, then restores the previous text clipboard when possible. See
[running.md#insertion](running.md#insertion) for paste modes.

Run `parakit doctor --deep` for an active insertion smoke test. Use `standard`
for apps that only accept `Ctrl+V`; use `direct` only when an app refuses
clipboard paste entirely. Streaming mode is disabled while batch dictation is
stabilized.

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
