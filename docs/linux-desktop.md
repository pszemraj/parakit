# Linux Desktop Hotkeys

parakit needs a desktop input backend for `Ctrl+Space`.

Default behavior:

- `auto` and `desktop` register `Ctrl+Space` with the X11 session through `global-hotkey`.
- `x11-global-hotkey` forces the same registered X11 backend as `auto`.
- `x11-listen` passively observes X11 key events with `rdev::listen`. It does not grab, suppress, or forward keys, so `Ctrl+Space` can also reach the focused application.
- `evdev-proxy-experimental` uses the experimental evdev/uinput keyboard proxy.
- The default path does not read `/dev/input` and does not require `/dev/uinput`.
- Linux insertion uses X11/XTest and requires an X11 session for every paste mode, including `direct`.
- Wayland sessions are rejected during startup. An XWayland `DISPLAY` is not enough because XTest cannot insert into focused native Wayland applications.

## X11 Sessions

Start Parakit from a terminal opened in the current graphical login. General
foreground and background commands are in
[running.md#background-use](running.md#background-use).

Tmux is fine when the tmux server was started from the current desktop login. A tmux server that survived a GNOME logout/login can keep stale `DISPLAY` or `XAUTHORITY` values from the old session. In that case, `parakit doctor` may report an X11 error such as `Connection refused`.

Fix that by starting a new terminal or tmux server from the current desktop session, then rerun:

```bash
parakit doctor && parakit --quiet &
disown
```

If `doctor` reports that `Ctrl+Space` could not be registered, disable any desktop shortcut, input method, or keyboard remapper that already owns that chord and rerun `parakit doctor`. Ubuntu/GNOME IBus commonly uses `Ctrl+Space` to switch input methods, so check IBus first when the registered backend is intermittent after login, suspend, or an input-method state change.

## Shortcut Conflicts

On GNOME/Ubuntu, check the visible shortcuts first:

1. Open Settings > Keyboard > Keyboard Shortcuts.
2. Check Typing or Input Sources for `Ctrl+Space`.
3. Disable or change anything that uses `Ctrl+Space`, then rerun `parakit doctor`.

Useful command-line checks:

```bash
gsettings get org.gnome.desktop.wm.keybindings switch-input-source
gsettings get org.gnome.desktop.wm.keybindings switch-input-source-backward
gsettings get org.gnome.desktop.input-sources xkb-options
```

If any output mentions `<Control>space`, `<Ctrl>space`, or a left-control toggle, remove that binding in Settings or with your input-method tool. For IBus-specific bindings, run `ibus-setup`, open Keyboard Shortcuts, and remove `Ctrl+Space` from input-method switching before rerunning `parakit doctor`.

## Deep Doctor Check

In `terminal` or `standard` mode, `parakit doctor --deep` creates and focuses a temporary 1x1 X11 window, stages a sentinel through the production guarded clipboard transaction, sends the configured paste chord, and verifies the window observed the V key press and release. It then restores the previous focus and supported clipboard contents and destroys the window. An active X11 desktop is required. In `direct` mode, the command performs backend preflight only and does not synthesize text.

## Focus Guard

The X11 focus guard, fail-open query behavior, and clipboard fallback are described in [running.md#insertion](running.md#insertion). Parakit does not inspect application internals with AT-SPI.

## Passive X11 Listen

The `x11-listen` backend is for debugging hotkey state without registering or grabbing the chord:

```bash
parakit doctor --hotkey-backend x11-listen
parakit start --hotkey-backend x11-listen --quiet &
```

Because this backend is passive, it cannot prevent the literal Space key from reaching the focused application. Use the default registered backend for normal dictation.

## Evdev Proxy

The evdev-proxy experimental backend is for testing the old keyboard proxy path. It grabs a physical keyboard event device, suppresses the `Ctrl+Space` chord, and forwards other key events through `/dev/uinput`.

Only this backend needs at least one readable keyboard event device that exposes both `Ctrl` and `Space`, plus writable `/dev/uinput`. `parakit doctor --hotkey-backend evdev-proxy-experimental` reports unreadable non-keyboard event devices, but they do not block startup when a usable hotkey keyboard candidate is readable.

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
parakit doctor --hotkey-backend evdev-proxy-experimental
```

When `doctor` reports `hotkey OK`, run:

```bash
parakit doctor --hotkey-backend evdev-proxy-experimental && parakit start --hotkey-backend evdev-proxy-experimental --quiet &
disown
```

Avoid running parakit with `sudo`; audio, X11, clipboard, and text insertion belong to the regular desktop user session.
