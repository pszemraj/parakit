# Troubleshooting

Start with diagnostics. `doctor` does not load the model.

```bash
parakit doctor
parakit --verbose doctor
parakit doctor --deep
```

It exits `0` when startup should proceed and `1` when a blocking issue remains. Launch behavior is in [running.md](running.md).

If a daemon is already running, use the control socket before starting another copy:

```bash
parakit status
parakit stop
```

## Hotkey Problems

Linux X11 session and backend setup are in [linux-desktop.md](linux-desktop.md). The default backend registers `Ctrl+Space` with X11 and does not need `/dev/input` or `/dev/uinput`. If `Ctrl+Space` is unavailable, another desktop shortcut, input method, or keyboard remapper may own it. Disable that binding and rerun `parakit doctor`.

On macOS, grant Accessibility and Input Monitoring permissions to both the terminal and the built binary.

## Literal Space Appears

The active backend should suppress the literal Space in `Ctrl+Space`. If a space reaches the focused app:

- confirm only one parakit process is running;
- confirm no desktop/input-method shortcut also handles `Ctrl+Space`;
- if you selected `evdev-proxy`, confirm `/dev/uinput` is writable and the input device can be grabbed;
- retry in foreground mode to inspect errors;
- use an X11 session for Linux insertion.

## Text Does Not Insert

Paste modes, focus-change behavior, target safety checks, and copy-only fallback are described in [running.md#insertion](running.md#insertion).

Run `parakit doctor --deep` for an active insertion smoke test. On Linux, use an X11 session; Wayland details are in [linux-desktop.md](linux-desktop.md). Use `standard` for apps that only accept `Ctrl+V`; use `direct` only when an app refuses clipboard paste entirely.

On Windows, paste shortcuts are sent with `SendInput`. Windows blocks synthetic input into higher-integrity processes, so a normal parakit process cannot paste into an administrator/elevated target application. Security software can also flag global hooks plus text insertion; whitelist the binary when needed.

If paste is blocked but the transcript was copied, focus the intended field and run:

```bash
parakit paste-last
```

## Wrong Microphone

Microphone selection behavior is described in [running.md#microphone](running.md#microphone).

On PipeWire/PulseAudio:

```bash
pactl get-default-source
pactl list sources | grep -E 'Description:|Sample Specification:' | grep -v monitor
parakit doctor
```

If the reported microphone is wrong, change the default input in desktop sound settings or `pavucontrol`, then wait a few seconds and rerun `parakit doctor`. Restart parakit if the audio server itself is not reporting the new default source.

Bluetooth microphone policy is in [running.md#microphone](running.md#microphone).

## Build And Model Issues

Missing [CrispASR](https://github.com/CrispStrobe/CrispASR) path dependency:

```text
failed to read vendor\CrispASR\crispasr\Cargo.toml
```

The git submodule is missing. Fix the existing checkout:

```bash
git submodule update --init --recursive
```

On Windows, initialize the submodule before building:

```powershell
git submodule update --init --recursive
cargo build --release
```

For shared library loading failures on Linux, check [build.md#runtime-library-paths](build.md#runtime-library-paths).

Vulkan failing on `spirv/unified1/spirv.hpp` means `spirv-headers` is missing. Install it and rebuild with the Vulkan feature:

```bash
sudo apt install spirv-headers
cargo build --release --features vulkan
```

Windows builds need generated DLLs next to the executable or on `PATH`; see [build.md#windows-dlls](build.md#windows-dlls).

Model cache behavior and commands are in [running.md#model-cache](running.md#model-cache).
