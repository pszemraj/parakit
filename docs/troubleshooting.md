# Troubleshooting

## `rdev::grab` Fails

On Linux, the daemon requires X11. Wayland compositors generally prevent global
key interception and synthetic input from regular client applications.

Check the session type:

```bash
echo "$XDG_SESSION_TYPE"
```

Use an X11 session for the daemon.

On macOS, grant Accessibility and Input Monitoring permissions to both the
terminal and the built binary.

## Literal Space Appears In The Target App

parakit uses `rdev::grab`, not `rdev::listen`, so `Ctrl+Space` should suppress
the literal Space event. If a Space appears:

- confirm another process is not also handling the same hotkey;
- confirm the daemon is the process receiving keyboard events;
- retry in foreground mode to inspect errors;
- avoid Wayland sessions.

## Text Does Not Inject

Injection uses Enigo synthetic typing, not clipboard paste.

On Linux, X11 is the supported path. Wayland usually blocks this.

On macOS, check Accessibility and Input Monitoring permissions.

On Windows, security software can flag the binary because global hooks and
synthetic typing resemble keylogger behavior. Whitelist the binary when needed.

## Shared Libraries Cannot Be Found

On Linux, inspect dynamic linking:

```bash
ldd target/debug/parakit | grep -E "whisper|ggml"
readelf -d target/debug/parakit | grep -E "RPATH|RUNPATH"
```

Expected:

- dependencies resolve under `target/debug/build/parakit-*/out/lib`;
- `readelf` reports `RPATH`, not only `RUNPATH`.

If this regresses, inspect `build.rs::emit_rpath` and confirm
`--disable-new-dtags` is still emitted for Linux/BSD builds.

## Vulkan Build Fails On `spirv/unified1/spirv.hpp`

The Vulkan backend can find `glslc` and `vulkan.pc` while still missing SPIR-V
headers. The failing line usually looks like:

```text
fatal error: spirv/unified1/spirv.hpp: No such file or directory
```

Install the distro package or SDK component that provides SPIR-V headers. On
Ubuntu/Debian, the missing package is usually `spirv-headers`:

```bash
sudo nala install spirv-headers
```

Then retry:

```bash
cargo build --release --features vulkan
```

Until those headers are available, default builds and CUDA builds can still
work.

## Windows DLL Loading

After a Windows build, copy generated DLLs next to the executable:

```powershell
copy target\release\build\parakit-*\out\lib\*.dll target\release\
```

or add that `out\lib` directory to `PATH`.

## Model Path Problems

Use absolute model paths for background launches:

```bash
parakit -m "$HOME/.local/share/parakit/models/parakeet-tdt-0.6b-v3-Q5_K_M.gguf" --quiet &
```

Relative paths are resolved from the shell's current working directory at
launch time.
