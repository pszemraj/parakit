# Linux Desktop Hotkeys

parakit needs a desktop input backend for `Ctrl+Space`.

Default behavior:

- `auto` and `evdev` use the Linux evdev keyboard grab backend.
- `desktop` exits with an error on Linux.
- Hotkey capture reads evdev devices, so it is not an X11 global shortcut.
- Linux paste insertion uses X11/XTest. Use an X11 session for the full
  dictate-and-insert workflow.
- Wayland hotkey capture may work with evdev permissions, but insertion is
  compositor-dependent and not a supported Linux path today.

## X11 Sessions

Start parakit from a terminal opened in the current graphical login:

```bash
parakit --quiet &
disown
```

Tmux is fine when the tmux server was started from the current desktop login. A
tmux server that survived a GNOME logout/login can keep stale `DISPLAY` or
`XAUTHORITY` values from the old session. In that case, `parakit doctor` may
report an X11 error such as `Connection refused`.

Fix that by starting a new terminal or tmux server from the current desktop
session, then rerun:

```bash
parakit doctor && parakit --quiet &
disown
```

## Evdev Backend

The evdev backend needs readable `/dev/input/event*` devices and writable
`/dev/uinput`.

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
parakit doctor
```

When `doctor` reports `hotkey OK`, run:

```bash
parakit doctor && parakit --hotkey-backend evdev --quiet &
disown
```

Avoid running parakit with `sudo`; audio, X11, clipboard, and text insertion
belong to the regular desktop user session.
