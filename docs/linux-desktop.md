# Linux Desktop Hotkeys

parakit needs a desktop input backend for `Ctrl+Space`.

Default behavior:

- `auto` and `desktop` register `Ctrl+Space` with the X11 session through `global-hotkey`.
- `x11-global-hotkey` forces the same registered X11 backend as `auto`.
- `x11-listen` passively observes X11 key events with `rdev::listen`. It does not grab, suppress, or forward keys, so `Ctrl+Space` can also reach the focused application.
- `evdev-proxy-experimental` uses the experimental evdev/uinput keyboard proxy. `evdev-proxy` is a compatibility alias.
- The default path does not read `/dev/input` and does not require `/dev/uinput`.
- Linux insertion uses X11/XTest and requires an X11 session for every paste mode, including `direct`.
- Wayland sessions are rejected during startup. An XWayland `DISPLAY` is not enough because XTest cannot insert into focused native Wayland applications.

## X11 Sessions

Start parakit from a terminal opened in the current graphical login:

```bash
parakit --quiet &
disown
```

Tmux is fine when the tmux server was started from the current desktop login. A tmux server that survived a GNOME logout/login can keep stale `DISPLAY` or `XAUTHORITY` values from the old session. In that case, `parakit doctor` may report an X11 error such as `Connection refused`.

Fix that by starting a new terminal or tmux server from the current desktop session, then rerun:

```bash
parakit doctor && parakit --quiet &
disown
```

If `doctor` reports that `Ctrl+Space` could not be registered, disable any desktop shortcut, input method, or keyboard remapper that already owns that chord and rerun `parakit doctor`.

## Target Safety

On X11, parakit compares the focused window captured at PTT-down with the focused window at paste time. It also uses AT-SPI when available to block password fields and to distinguish editable file-manager fields from file-manager body views. If AT-SPI is unavailable, known file-manager windows fall back to copy-only instead of paste.

## Passive X11 Listen

The `x11-listen` backend is for debugging hotkey state without registering or grabbing the chord:

```bash
parakit --hotkey-backend x11-listen doctor
parakit --hotkey-backend x11-listen --quiet &
```

Because this backend is passive, it cannot prevent the literal Space key from reaching the focused application. Use the default registered backend for normal dictation.

## Evdev Proxy

The evdev-proxy experimental backend is for testing the old keyboard proxy path. It grabs a physical keyboard event device, suppresses the `Ctrl+Space` chord, and forwards other key events through `/dev/uinput`.

Only this backend needs at least one readable keyboard event device that exposes both `Ctrl` and `Space`, plus writable `/dev/uinput`. `parakit doctor --hotkey-backend evdev-proxy` reports unreadable non-keyboard event devices, but they do not block startup when a usable hotkey keyboard candidate is readable.

```bash
sudo usermod -aG input "$USER"
```

Many distros also need a udev rule for `/dev/uinput`:

```bash
printf 'KERNEL=="uinput", GROUP="input", MODE="0660", OPTIONS+="static_node=uinput"\n' | \
  sudo tee /etc/udev/rules.d/70-uinput.rules
sudo modprobe uinput
sudo udevadm control --reload-rules
sudo udevadm trigger /dev/uinput
```

Log out completely and log back in, or reboot. Then verify:

```bash
id -nG | tr ' ' '\n' | grep '^input$'
test -w /dev/uinput
parakit --hotkey-backend evdev-proxy doctor
```

When `doctor` reports `hotkey OK`, run:

```bash
parakit --hotkey-backend evdev-proxy doctor && parakit --hotkey-backend evdev-proxy --quiet &
disown
```

Avoid running parakit with `sudo`; audio, X11, clipboard, and text insertion belong to the regular desktop user session.
