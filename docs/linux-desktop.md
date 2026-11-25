# Linux Desktop Input

Hold `Ctrl+Space` to dictate. See the [Qwen context shortcuts](models/qwen3-asr.md#prior-dictation-context) for dictation that uses recent transcripts.

- On X11, `auto` and `desktop` use the registered global shortcut.
- In a Wayland session, they select focused-field capture through IBus. On GNOME Wayland, select the Parakit input source and focus an editable field to dictate.
- `--hotkey-backend ibus` explicitly selects focused-field capture on either session type.

## IBus Setup

For native IBus input on GNOME Wayland or with `--hotkey-backend ibus`, [install Parakit](build.md#install) and register its IBus component once. The default X11 shortcut and paste workflow needs no IBus registration or input-source change.

```bash
sudo "$(command -v parakit)" ibus install
```

Log out and back in, then add and select **Parakit** in your desktop's input-source settings. Keep your other input sources for ordinary input.

The registration uses the US keyboard layout by default. To choose an XKB layout and variant:

```bash
sudo "$(command -v parakit)" ibus install --layout de --variant nodeadkeys
```

IBus uses the selected layout for ordinary typing, including Compose and dead keys. The install command writes the component XML to `/usr/share/ibus/component/parakit.xml` for GNOME to discover. Add or remove the source through the desktop settings.

IBus launches the sibling `parakit-ibus` helper automatically. Keep it beside `parakit` when relocating the binaries. Ordinary typing continues through the helper while the daemon is stopped or loading its model.

Before starting the dictation daemon, inspect registration and startup readiness:

```bash
parakit ibus status
parakit --verbose doctor
```

Once the daemon is running, inspect it with:

```bash
parakit --verbose status
```

`ibus status` reports the registration file, build support, connection, engine discovery, and active engine. To verify delivery, focus a disposable field in the application and check that it receives the test text. See [`test-paste`](running.md#daemon-control).

Upgrade `parakit` and `parakit-ibus` together and restart the daemon. If `Ctrl+Space` passes through after a protocol upgrade, log out and back in so IBus loads the new helper. Re-run `ibus install` when migrating from a component that launched `parakit ibus engine` or changing the executable path or layout.

To remove the integration, select another input source, remove Parakit from the desktop's source list, then run the uninstall command.

```bash
sudo "$(command -v parakit)" ibus uninstall
```

Log out and back in afterward. Run the dictation daemon as your desktop user.

## Chromium On Wayland

If Chrome or VS Code does not accept native text input, try explicit Wayland and input-method options:

```bash
google-chrome --ozone-platform=wayland --enable-wayland-ime --wayland-text-input-version=3
code --ozone-platform=wayland --enable-wayland-ime --wayland-text-input-version=3
```

Close existing application instances before changing their launch options. Chromium documents the [Ozone platform selection](https://chromium.googlesource.com/chromium/src/+/main/docs/ozone_overview.md#linux-desktop) and defines the [Wayland IME and text-input version switches](https://github.com/chromium/chromium/blob/main/ui/ozone/public/ozone_switches.cc). Application defaults can differ.

## Insertion Selection And Recovery

`--insertion-backend auto` follows the hotkey route. IBus capture uses native commits and other hotkey routes use X11. To use native delivery under X11, select `--hotkey-backend ibus` because a global X11 grab can invalidate the input-method target. `--insertion-backend ibus` requires IBus capture.

`--insertion-backend x11` and `--paste-mode direct` need a genuine X11 session. Direct mode selects X11 under automatic transport selection and conflicts with forced IBus. On Wayland, recovery copies use the XWayland clipboard when available. IBus capture and delivery work without `DISPLAY`. See [paste modes](running.md#insertion) and [`daemon.insertion_backend`](config_reference.toml).

## X11 Sessions

Run Parakit from a terminal or tmux server started in the current graphical login. A tmux server that survived a logout can keep stale `DISPLAY` or `XAUTHORITY` values and make `parakit doctor` report `Connection refused`. Start a new terminal or tmux server and rerun the check. For registration failures or intermittent shortcuts, see [shortcut conflicts](#shortcut-conflicts).

## Shortcut Conflicts

On GNOME/Ubuntu, check these settings.

1. Open Settings > Keyboard > Keyboard Shortcuts.
2. Check Typing or Input Sources for `Ctrl+Space` and, for Qwen context, `Ctrl+Alt+Space`.
3. Disable or change a conflicting binding, then rerun `parakit --verbose doctor` to inspect both registrations on X11. See [Qwen shortcut availability](models/qwen3-asr.md#prior-dictation-context) for context-only conflicts.

You can also inspect the configured bindings from a terminal.

```bash
gsettings get org.gnome.desktop.wm.keybindings switch-input-source
gsettings get org.gnome.desktop.wm.keybindings switch-input-source-backward
gsettings get org.gnome.desktop.input-sources xkb-options
```

If any output mentions `<Control>space`, `<Ctrl>space`, or a left-control toggle, remove that binding in Settings or with your input-method tool. For IBus-specific bindings, run `ibus-setup`, open Keyboard Shortcuts, and remove `Ctrl+Space` from input-method switching before rerunning `parakit doctor`.

## Deep Doctor Check

With X11 clipboard insertion, `parakit doctor --deep` briefly focuses a temporary window to verify it observes the paste chord's V press and release. It applies the [clipboard restore policy](running.md#clipboard-staging-and-restore) without reading back the restored contents. Direct typing and IBus perform preflight only.

## Focus Guard

Native IBus requires a known focused field and blocks password, PIN, or private input. It checks the engine and focus generation again at submission. Focus loss, field or source changes, engine resets, and reconnects can invalidate delivery even when GNOME reuses the input-context ID. See [clipboard recovery](running.md#clipboard-staging-and-restore) and [insertion outcomes](logging.md#insertion-outcomes).

IBus can miss Space's release during an input-source switch. If the next shortcut does not start recording, release it fully and press again. Waiting for that release prevents held-key repeats from starting another dictation.

The X11 route checks the active window. A detected focus change blocks paste, while a transient focus-query failure allows the paste to proceed. Parakit does not inspect fields with AT-SPI.

Direct typing waits up to two seconds for held modifiers, then rechecks focus and modifiers before each character. An unreadable modifier state blocks typing. A detected change can stop a transcript partway through. Tabs, newlines, and other control characters could navigate fields or submit text, so the check rejects them before typing. Use [daemon history](running.md#daemon-control) to recover the text.

## Passive X11 Listen

The `x11-listen` backend is for debugging hotkey state without registering or grabbing the chord:

```bash
parakit doctor --hotkey-backend x11-listen
parakit start --hotkey-backend x11-listen --quiet &
```

The focused application receives the literal Space key when you dictate with this backend. Use the default registered shortcut for normal dictation.

## Evdev Proxy

The experimental evdev proxy grabs a physical keyboard event device, suppresses the dictation chords' Space events, and forwards other key events through `/dev/uinput`.

The proxy needs a readable keyboard event device with both `Ctrl` and `Space` and writable `/dev/uinput`. Only this backend needs them; the default backends do not read `/dev/input` or require `/dev/uinput`. Doctor can report unreadable non-keyboard devices while a usable keyboard still lets startup proceed.

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

When `doctor` reports `hotkey OK`, run `parakit start --hotkey-backend evdev-proxy-experimental` as your desktop user.
