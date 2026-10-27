# Linux Desktop Input

Parakit prefers IBus text insertion on Linux. When its input source is selected and the focused field is eligible, the transcript arrives through IBus without changing the clipboard or sending a paste shortcut. On an X11 session, the default `auto` insertion backend retains clipboard paste as the fallback when native insertion is unavailable before submission.

Push-to-talk remains `Ctrl+Space`:

- On X11, `auto` and `desktop` use the registered global shortcut, preserving the existing workflow.
- On GNOME Wayland, they use the Parakit IBus engine. Push-to-talk works while that source is selected and an editable field has focus. This is focused-field support, not a global Wayland shortcut.
- `--hotkey-backend ibus` explicitly selects focused-field capture on either session type.
- The default paths do not read `/dev/input` or require `/dev/uinput`.

An XWayland `DISPLAY` does not make XTest insertion usable in native Wayland applications. The native IBus path does not require `DISPLAY`; explicit X11 paste and direct typing remain limited to genuine X11 sessions.

## IBus Setup

Default Linux builds include the `ibus` feature and require the [native build dependencies](build.md#native-dependencies). Install Parakit, then register its component once:

```bash
cargo install --locked --path .
sudo "$(command -v parakit)" ibus install
```

Log out and back in, then add **Parakit** in your desktop's input-source settings and select it. Keep your other input sources. A running IBus process alone is insufficient: Parakit must be the selected source for native insertion. It does not insert through a different active input method, such as Pinyin.

The registration uses the US keyboard layout by default. To choose an XKB layout and variant:

```bash
sudo "$(command -v parakit)" ibus install --layout de --variant nodeadkeys
```

The selected layout supplies ordinary typing; IBus handles Compose and dead keys. The command writes only Parakit's component XML, normally `/usr/share/ibus/component/parakit.xml`. It does not change input-source lists or shortcuts. A user-local component XML file is not sufficient for normal GNOME discovery.

Start and stop the dictation daemon as usual. Existing login/autostart commands stay the same. IBus launches and manages its lightweight engine automatically; ordinary keyboard input continues while the dictation daemon is stopped or preparing its model.

Inspect setup and runtime readiness:

```bash
parakit ibus status
parakit --verbose doctor
parakit --verbose status
```

`ibus status` reports the registration file, build support, connection, engine discovery, and active engine. Registration does not prove that the current application accepts native commits. Run `parakit test-paste "hello from parakit"` in a disposable terminal, then inspect the inserted prompt text and [insertion outcome](logging.md#insertion-outcomes).

After an upgrade, restart the daemon. Re-run `ibus install` if the executable path or keyboard layout changed; log out and back in to reload the engine. To remove the integration, first select another input source, remove Parakit from the desktop's source list, then run:

```bash
sudo "$(command -v parakit)" ibus uninstall
```

Log out and back in afterward. Only registration commands need administrator privileges; run the dictation daemon as your desktop user.

## Validated GNOME Wayland Setup

Native reception was verified on GNOME Shell 46.0 with IBus 1.5.29-rc2, with `DISPLAY`, `GTK_IM_MODULE`, and `QT_IM_MODULE` unset and no XWayland server. GTK used GNOME's Wayland text-input path. The results below cover Unicode, combining accents, and emoji:

| Application | Verified reception |
| --- | --- |
| GNOME Text Editor 46.3 / GTK 4.14.5 | Saved document contents |
| Chrome 156.0.8078.47 | Textarea contents and trusted input event |
| VS Code 1.143.1 | Saved editor buffer |
| Ghostty 1.3.1 | Canonical terminal input |
| Ghostty 1.3.1 with tmux 3.4 | Raw terminal input inside tmux |

Chrome and VS Code were launched with explicit Wayland and input-method options:

```bash
google-chrome --ozone-platform=wayland --enable-wayland-ime --wayland-text-input-version=3
code --ozone-platform=wayland --enable-wayland-ime --wayland-text-input-version=3
```

Close existing application instances before changing their launch options. Chromium documents the [Ozone platform selection](https://chromium.googlesource.com/chromium/src/+/main/docs/ozone_overview.md#linux-desktop) and defines the [Wayland IME and text-input version switches](https://github.com/chromium/chromium/blob/main/ui/ozone/public/ozone_switches.cc). These are the tested options for the versions above; application defaults can differ.

Focused GTK fields also verified selection replacement, rejection after switching fields in the same window, and `Ctrl+Space` with repeats and either release order. Ordinary typing and German `de` / `nodeadkeys` layout with Compose worked while the dictation daemon was absent. The production Rust worker delivered into GTK and Chrome with `DISPLAY` unset; Chrome's existing clipboard text remained unchanged even with transcript clipboard retention enabled.

The production worker also verified automatic native IBus delivery into a GTK X11 field on an isolated X11 server, with its clipboard unchanged. With no native target, automatic selection delivered the exact text through the existing X11 clipboard paste path.

Helper and client restart/reconnect behavior was checked separately through isolated IBus contexts. Those protocol checks do not establish reception in every application during a restart. The application matrix above covers native Wayland; XWayland reception and an actual interactive coding-assistant UI remain unverified. Terminal checks used input capture programs, without executing dictated text as commands.

## Insertion Selection And Recovery

`--insertion-backend ibus` requires native delivery. `--insertion-backend x11` selects the existing X11 clipboard/direct route. The default `auto` prefers IBus, with X11 fallback only where supported. Set `daemon.insertion_backend` to keep that choice across starts.

Native target state is captured at recording start and checked again immediately before submission. Focus, field, source, reset, engine, or connection changes can invalidate it. Parakit then keeps the transcript in enabled daemon history instead of sending it to a new target. It never falls back after target invalidation or uncertain native delivery, because the text may already have arrived.

IBus reports submission, not application reception. A `submitted` outcome means the native commit was sent. Inspect the target before retrying an `uncertain` outcome. Native delivery leaves the clipboard unchanged, including when `--keep-transcript-clipboard` is configured.

Terminal mode remains the default. Native terminal commits reject tabs and embedded newlines because they do not provide clipboard bracketed-paste semantics. Use [daemon history](running.md#daemon-control) for blocked text. Explicit copying uses the existing clipboard backend, which works through XWayland on normal GNOME Wayland sessions. Without that clipboard connection, the picker still previews text and `parakit history --text N` prints it in full.

## X11 Sessions

Start Parakit from a terminal opened in the current graphical login. General foreground and background commands are in [running.md#background-use](running.md#background-use). `x11-global-hotkey` forces registered global capture. The legacy insertion route can be selected with `--insertion-backend x11`.

Tmux is fine when the tmux server was started from the current desktop login. A tmux server that survived a GNOME logout/login can keep stale `DISPLAY` or `XAUTHORITY` values from the old session. In that case, `parakit doctor` may report an X11 error such as `Connection refused`.

Start a new terminal or tmux server from the current desktop session, rerun `parakit doctor`, then use the [background commands](running.md#background-use).

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

For X11 clipboard insertion in `terminal` or `standard` mode, `parakit doctor --deep` creates and focuses a temporary 1x1 X11 window, stages a sentinel through the production guarded clipboard transaction, sends the configured paste chord, and verifies the window observed the V key press and release. It then restores the previous focus and supported clipboard contents and destroys the window. An active X11 desktop is required. Direct mode and the IBus route perform backend checks without synthesizing text into the focused application.

## Focus Guard

The legacy X11 focus guard, fail-open query behavior, and clipboard fallback are described under [insertion](running.md#insertion). Native IBus delivery requires a valid native target and does not inherit that fail-open behavior. Parakit does not inspect application internals with AT-SPI.

Batch paste follows the shared [modifier wait and clipboard-retention behavior](running.md#clipboard-staging-and-restore).

Direct typing also waits for modifiers, then rechecks focus and modifiers before each character. An unreadable modifier state blocks typing; a detected change stops the remaining text, so part of a transcript may already have been inserted. Tabs, newlines, and other control characters are rejected before typing because synthetic keys could navigate fields or submit text. Direct mode never copies a blocked transcript to the clipboard; use daemon history for recovery.

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

When `doctor` reports `hotkey OK`, use the [background commands](running.md#background-use), adding `--hotkey-backend evdev-proxy-experimental` to `parakit start`.

Avoid running parakit with `sudo`; audio, X11, clipboard, and text insertion belong to the regular desktop user session.
