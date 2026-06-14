# Troubleshooting

Start with diagnostics. Launch behavior and exit codes are in [running.md#first-run](running.md#first-run). `doctor` does not load the model.

```text
parakit doctor
parakit --verbose doctor
parakit doctor --deep
```

If a daemon is already running, use the control socket before starting another copy:

```text
parakit status
parakit stop
```

## Hotkey Problems

Linux X11 session and backend setup are in [linux-desktop.md](linux-desktop.md). The default backend registers `Ctrl+Space` with X11 and does not need `/dev/input` or `/dev/uinput`. If `Ctrl+Space` is unavailable, another desktop shortcut, input method, or keyboard remapper may own it. IBus uses `Ctrl+Space` by default on many Ubuntu/GNOME installs. Disable the conflicting binding and rerun `parakit doctor`.

On macOS, grant Accessibility and Input Monitoring permissions to both the terminal and the built binary.

WSL is not the native Windows daemon path. Validate Windows hotkeys, focus checks, and paste behavior from native Windows PowerShell with the Windows bundle.

## Literal Space Appears

The active backend should suppress the literal Space in `Ctrl+Space`. If a space reaches the focused app:

- confirm only one parakit process is running;
- confirm no desktop/input-method shortcut also handles `Ctrl+Space`;
- if you selected `evdev-proxy-experimental`, confirm `/dev/uinput` is writable and the input device can be grabbed;
- retry in foreground mode to inspect errors;
- use an X11 session for Linux insertion.

## Text Does Not Insert

Paste modes, focus-change behavior, paste sanitization, and clipboard fallback behavior are described in [running.md#insertion](running.md#insertion).

Run `parakit doctor --deep` for an active insertion smoke test. On Linux, use an X11 session; Wayland details are in [linux-desktop.md](linux-desktop.md). Use `standard` for apps that only accept `Ctrl+V`; use `direct` only when an app refuses clipboard paste entirely.

Windows elevated-target behavior is covered in [running.md#insertion](running.md#insertion).

In non-direct paste modes, parakit stages blocked or failed transcripts on the clipboard before restoring the active clipboard. Check OS clipboard history, such as `Win+V` on Windows, or your clipboard manager before using the recovery commands below.

If paste is blocked, focus the intended field and run:

```text
parakit paste-last
```

To avoid sending a paste chord, copy the last transcript instead:

```text
parakit copy-last
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

For shared library loading failures on Linux, check [build.md#runtime-library-paths](build.md#runtime-library-paths).

Vulkan failing on `spirv/unified1/spirv.hpp` means `spirv-headers` is missing. Install it and rebuild with the Vulkan feature:

```bash
sudo apt install spirv-headers
cargo build --release --features vulkan
```

Windows builds need generated DLLs next to the executable; use the Windows scripts in [../scripts/windows/README.md](../scripts/windows/README.md).

Model cache behavior and commands are in [running.md#model-cache](running.md#model-cache).

## Windows GPU Builds

If a GPU bundle fails to start with `0xC0000135` or `STATUS_DLL_NOT_FOUND`, Windows could not resolve a load-time DLL. Bundle requirements, CUDA runtime bundling, Vulkan loader behavior, and installer checks are in [../scripts/windows/README.md#runtime-manifest](../scripts/windows/README.md#runtime-manifest).

For CUDA bundles, install the CUDA Toolkit that matches the build so its runtime DLL directory is available, add that directory to `PATH`, or rebuild with:

```bat
scripts\windows\build.bat --backend cuda --bundle-cuda-dlls
```

For Vulkan bundles, install or update the NVIDIA, AMD, or Intel GPU driver if the installer reports that `vulkan-1.dll` is missing. Use the CPU bundle on machines without a Vulkan-capable driver.

If `parakit --device gpu` fails before model load, run:

```text
parakit --verbose doctor
```

The `compute:` block lists devices visible to bundled ggml. A GPU build with no GPU or iGPU listed usually means the driver is missing, too old for the CUDA toolkit/driver ABI, or not exposing Vulkan on that machine. Device selection behavior is in [running.md#device-selection](running.md#device-selection).

The daemon intentionally warms the backend at startup. Use `--verbose` to see warmup duration. A cold backend can still make an unusually long first dictation slower.
