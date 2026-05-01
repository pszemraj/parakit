# Linux Desktop Hotkeys

parakit needs a desktop input backend for `Ctrl+Space`.

Default behavior:

- `auto` uses the Linux evdev keyboard grab backend.
- The old X11 desktop hotkey backend is disabled in the Linux-stable path.
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
parakit doctor && parakit --quiet &
disown
```

## Evdev Backend

The evdev backend needs readable `/dev/input/event*` devices and writable
`/dev/uinput`. Use it when you want parakit to keep working across
lock/logout/session restarts:

```bash
sudo usermod -aG input "$USER"
```

Log out completely and log back in, or reboot. Then verify:

```bash
id -nG | tr ' ' '\n' | grep '^input$'
parakit doctor
```

When `doctor` reports `hotkey OK`, run:

```bash
parakit doctor && parakit --hotkey-backend evdev --quiet &
disown
```

Avoid running parakit with `sudo`; audio, X11, clipboard, and text insertion
belong to the regular desktop user session.
