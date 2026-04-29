# Troubleshooting

## Hotkey Does Not Start

Run the preflight check first:

```bash
parakit doctor
```

`parakit doctor` also reports the microphone selected for capture. If the
hotkey status is OK but audio is unavailable, fix the desktop/audio-server
input device before starting the daemon.

On Linux, the preferred path is an X11 desktop hotkey registration. This avoids
direct `/dev/input/event*` access and is the path ordinary GNOME/KDE/X11 users
should get.

A healthy X11 setup looks like:

```text
primary:        X11 desktop hotkey
primary status: OK
fallback:       rdev evdev grab
status:         OK (desktop hotkey backend)
```

If `primary status` says the shortcut is unavailable, another desktop shortcut
or input method may already own `Ctrl+Space`. Disable that binding and rerun
`parakit doctor`.

Check the session type:

```bash
echo "$XDG_SESSION_TYPE"
```

Use an X11 session when possible. Wayland compositors generally block global
hotkeys and synthetic input from regular client applications unless the
compositor exposes its own shortcut mechanism.

If the X11 backend is unavailable or you need the low-level fallback,
`rdev::grab` must read evdev devices. Add the desktop user to the `input`
group, then start a completely new login session:

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

The X11 desktop backend and the evdev fallback both register an intercepting
hotkey, so `Ctrl+Space` should suppress the literal Space event. If a Space
appears:

- confirm another process is not also handling the same hotkey;
- confirm the daemon is the process receiving keyboard events;
- retry in foreground mode to inspect errors;
- avoid Wayland sessions.

## Text Does Not Insert

Batch insertion writes the transcript to the system clipboard and sends the
configured paste shortcut. parakit restores the previous clipboard when the
previous contents were text.

The default `--paste-mode terminal` sends `Ctrl+Shift+V` on Linux and Windows,
which matches terminal emulators. Use `--paste-mode standard` for apps that
only accept `Ctrl+V`.

Clipboard managers may still record the transient transcript before parakit
restores the previous text clipboard.

Streaming partial insertion uses Enigo synthetic typing. On Linux, X11 is the
supported path. Wayland usually blocks synthetic key events.

On macOS, check Accessibility and Input Monitoring permissions.

On Windows, security software can flag the binary because global hooks and
synthetic typing resemble keylogger behavior. Whitelist the binary when needed.

## Wrong Microphone Or Sample Rate

parakit uses the OS default input when it is usable and physical-looking. It
avoids monitor and virtual sources unless no better input exists.

List the audio server's current sources on PipeWire/PulseAudio systems:

```bash
pactl list sources | grep -E 'Description:|Sample Specification:' | grep -v monitor
```

Then run:

```bash
parakit doctor
```

The reported microphone line should match the desired input. If it does not,
change the default input in the desktop sound settings or with `pavucontrol`,
then wait a few seconds or restart parakit.

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
it in the cache described in [running.md](running.md#model-cache):

```bash
parakit --quiet &
```

Use `parakit fetch --force` to redownload the hosted Q8_0 GGUF after a failed
or interrupted fetch.

Use `parakit cache` to inspect cached GGUF files, sizes, dtypes, and the Q8_0
checksum. Use `parakit cache dir` to print only the cache directory.

If you pass `-m <path>`, that custom model path always wins. Relative custom
paths are resolved from the shell's current working directory at launch time.
