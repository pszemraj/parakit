# Linux Desktop Hotkeys

parakit needs a desktop input backend for `Ctrl+Space`.

Default behavior:

- `auto` uses the evdev backend when all `/dev/input/event*` devices are
  readable.
- Otherwise it uses the X11 desktop hotkey backend.
- Wayland usually blocks desktop global hotkeys and synthetic input for regular
  applications.

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
parakit doctor
parakit --quiet &
disown
```

## Evdev Backend

The evdev backend is more stable across desktop session churn, but it needs
read access to all input event devices. Use it when you want parakit to keep
working across lock/logout/session restarts:

```bash
sudo usermod -aG input "$USER"
```

Log out completely and log back in, or reboot. Then verify:

```bash
id -nG | tr ' ' '\n' | grep '^input$'
parakit doctor
```

When `doctor` reports `evdev: rdev grab (ready)`, run:

```bash
parakit --hotkey-backend evdev --quiet &
disown
```

Avoid running parakit with `sudo`; audio, X11, clipboard, and text insertion
belong to the regular desktop user session.
