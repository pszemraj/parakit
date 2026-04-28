# Troubleshooting

## `rdev::grab` Fails

Run the preflight check first:

```bash
parakit doctor
```

On Linux, the daemon requires X11. Wayland compositors generally block global
key interception and synthetic input from regular client applications.

Check the session type:

```bash
echo "$XDG_SESSION_TYPE"
```

Use an X11 session for the daemon.

If the session is X11 and `parakit doctor` reports `permission denied` on
`/dev/input/event*`, the hotkey backend cannot read low-level input devices.
Add the desktop user to the `input` group, then start a completely new login
session:

```bash
sudo usermod -aG input "$USER"
```

Log out and back in, or reboot. A terminal or tmux server that was already
running before the group change keeps the old group list, so restart tmux and
launch parakit from a fresh shell. Verify the new session:

```bash
id -nG | tr ' ' '\n' | grep '^input$'
```

Do not run parakit with `sudo` as a normal workaround. The keyboard hook might
open, but audio, X11, and synthetic typing are owned by the regular desktop
session and can fail in different ways.

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

On Linux, run the dynamic-linking checks in
[build.md](build.md#runtime-library-paths).

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
work. The full Vulkan dependency set is in
[build.md](build.md#native-dependencies).

## Windows DLL Loading

After a Windows build, make generated DLLs findable as described in
[build.md](build.md#windows-dlls).

## Model Cache Problems

With no `-m` path, parakit downloads the default model on first run and stores
it in the cache described in [Model Setup](../README.md#model-setup):

```bash
parakit --quiet &
```

Use `parakit fetch --force` to redownload the hosted Q8_0 GGUF after a failed
or interrupted fetch.

If you pass `-m <path>`, that custom model path always wins. Relative custom
paths are resolved from the shell's current working directory at launch time.
