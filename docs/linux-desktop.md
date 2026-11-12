# Linux Desktop Input

Parakit supports native IBus commits and X11 clipboard paste or direct typing. Choose a transport under [insertion selection](#insertion-selection-and-recovery).

Push-to-talk uses `Ctrl+Space`; Qwen3-ASR also supports `Ctrl+Left Alt+Space` for [prior-dictation context](models/qwen3-asr.md#prior-dictation-context):

- On X11, `auto` and `desktop` use the registered global shortcut.
- On GNOME Wayland, they use the Parakit IBus engine. Push-to-talk works while that source is selected and an editable field has focus. This is focused-field support, not a global Wayland shortcut.
- `--hotkey-backend ibus` explicitly selects focused-field capture on either session type.
- The default paths do not read `/dev/input` or require `/dev/uinput`.

An XWayland `DISPLAY` does not make XTest insertion usable in native Wayland applications. The native IBus path does not require `DISPLAY`; explicit X11 paste and direct typing remain limited to genuine X11 sessions.

## IBus Setup

For native IBus input on GNOME Wayland or with `--hotkey-backend ibus`, [install Parakit](build.md#install) and register its IBus component once. The default X11 shortcut and paste workflow needs no IBus registration or input-source change.

```bash
sudo "$(command -v parakit)" ibus install
```

Log out and back in, then add **Parakit** in your desktop's input-source settings and select it. Keep your other input sources. A running IBus process alone is insufficient: Parakit must be the selected source for native insertion. It does not insert through a different active input method, such as Pinyin.

The registration uses the US keyboard layout by default. To choose an XKB layout and variant:

```bash
sudo "$(command -v parakit)" ibus install --layout de --variant nodeadkeys
```

The selected layout supplies ordinary typing; IBus handles Compose and dead keys. The command writes only Parakit's component XML, normally `/usr/share/ibus/component/parakit.xml`. It does not change input-source lists or shortcuts. A user-local component XML file is not sufficient for normal GNOME discovery.

Start and stop the dictation daemon with `parakit` as usual. IBus launches and manages `parakit-ibus` automatically; you never need to run the helper manually or add it to login/autostart commands. Keep it beside `parakit` when moving the installed binaries. The helper handles ordinary keyboard input independently of the speech daemon, so typing continues while dictation is stopped or its model is loading.

Before starting the dictation daemon, inspect registration and startup readiness:

```bash
parakit ibus status
parakit --verbose doctor
```

Once the daemon is running, inspect it with:

```bash
parakit --verbose status
```

`ibus status` reports the registration file, build support, connection, engine discovery, and active engine. Registration does not prove that the current application accepts native commits. Run `parakit test-paste "hello from parakit"` in a disposable terminal, then inspect the inserted prompt text and [insertion outcome](logging.md#insertion-outcomes).

After an upgrade, install matching `parakit` and `parakit-ibus` binaries and restart the daemon. Log out and back in to reload the helper when its protocol changes. Re-run `ibus install` when upgrading from a component that launched `parakit ibus engine`, or if the executable path or keyboard layout changed. To remove the integration, first select another input source, remove Parakit from the desktop's source list, then run:

```bash
sudo "$(command -v parakit)" ibus uninstall
```

Log out and back in afterward. Only registration commands need administrator privileges; run the dictation daemon as your desktop user.

## Chromium On Wayland

If Chrome or VS Code does not accept native text input, try explicit Wayland and input-method options:

```bash
google-chrome --ozone-platform=wayland --enable-wayland-ime --wayland-text-input-version=3
code --ozone-platform=wayland --enable-wayland-ime --wayland-text-input-version=3
```

Close existing application instances before changing their launch options. Chromium documents the [Ozone platform selection](https://chromium.googlesource.com/chromium/src/+/main/docs/ozone_overview.md#linux-desktop) and defines the [Wayland IME and text-input version switches](https://github.com/chromium/chromium/blob/main/ui/ozone/public/ozone_switches.cc). Application defaults can differ.

## Insertion Selection And Recovery

`--insertion-backend ibus` requires engine-owned IBus push-to-talk. `--insertion-backend x11` selects X11 clipboard paste or direct typing. The default `auto` follows the hotkey route: native delivery for IBus hotkeys and X11 for all others, including the evdev proxy. To use native delivery on X11, select `--hotkey-backend ibus`; a registered X11 grab can invalidate an input-method target. Set `daemon.insertion_backend` to keep the transport choice across starts. Explicit `direct` mode retains X11 typing with automatic transport selection and conflicts with forced IBus insertion.

Clipboard recovery needs XWayland clipboard access; native IBus submission itself does not. [Paste modes](running.md#insertion) control terminal tabs and newlines; use [daemon history](running.md#daemon-control) for blocked text and explicit copying.

## X11 Sessions

Start Parakit from a terminal opened in the current graphical login. Use the [background commands](running.md#background-use); `x11-global-hotkey` forces registered global capture.

Tmux is fine when the tmux server was started from the current desktop login. A tmux server that survived a GNOME logout/login can keep stale `DISPLAY` or `XAUTHORITY` values from the old session. In that case, `parakit doctor` may report an X11 error such as `Connection refused`.

Start a new terminal or tmux server from the current desktop session, rerun `parakit doctor`, then use the [background commands](running.md#background-use).

For registration failures or an intermittent shortcut after login, suspend, or an input-method change, follow [shortcut conflicts](#shortcut-conflicts).

## Shortcut Conflicts

On GNOME/Ubuntu, check the visible shortcuts first:

1. Open Settings > Keyboard > Keyboard Shortcuts.
2. Check Typing or Input Sources for `Ctrl+Space` and, for Qwen context, `Ctrl+Alt+Space`.
3. Disable or change a conflicting binding, then rerun `parakit --verbose doctor` to inspect both registrations on X11. See [Qwen shortcut availability](models/qwen3-asr.md#prior-dictation-context) for context-only conflicts.

Useful command-line checks:

```bash
gsettings get org.gnome.desktop.wm.keybindings switch-input-source
gsettings get org.gnome.desktop.wm.keybindings switch-input-source-backward
gsettings get org.gnome.desktop.input-sources xkb-options
```

If any output mentions `<Control>space`, `<Ctrl>space`, or a left-control toggle, remove that binding in Settings or with your input-method tool. For IBus-specific bindings, run `ibus-setup`, open Keyboard Shortcuts, and remove `Ctrl+Space` from input-method switching before rerunning `parakit doctor`.

## Deep Doctor Check

For X11 clipboard insertion in `terminal` or `standard` mode, `parakit doctor --deep` creates and focuses a temporary 1x1 X11 window, stages a sentinel through the production guarded clipboard transaction, sends the configured paste chord, and verifies the window observed the V key press and release. The transaction applies the normal clipboard restore policy; the check does not verify the restored clipboard contents. It restores the previous focus and destroys the window. An active X11 desktop is required. Direct mode and the IBus route perform backend checks without synthesizing text into the focused application.

## Focus Guard

Native IBus requires an eligible focused field and blocks fields identified as password, PIN, or private input. It keeps the recording target's engine and focus generation, then checks them immediately before submission. Focus loss, field or source changes, relevant resets, engine destruction, and reconnects invalidate pending insertion even when GNOME reuses the same input-context ID. Unknown native focus blocks delivery. Rejected or uncertain delivery follows the shared [clipboard recovery policy](running.md#clipboard-staging-and-restore); inspect [insertion outcomes](logging.md#insertion-outcomes) before retrying. Use [daemon history](running.md#daemon-control) when clipboard access is unavailable.

IBus can miss Space's release during an input-source switch. If the next shortcut does not start recording, release it fully and press again. Waiting for that release prevents held-key repeats from starting another dictation.

The X11 route compares the active X11 window. A clear focus change prevents the paste chord; non-direct modes follow the shared clipboard policy. If focus capture or recheck fails because X11 is transiently unavailable, parakit pastes anyway. The transcript remains available through daemon history when enabled. Parakit does not inspect application internals with AT-SPI.

Batch paste follows the shared [modifier wait and clipboard-retention behavior](running.md#clipboard-staging-and-restore).

Direct typing waits up to two seconds for held modifiers to be released, then rechecks focus and modifiers before each character. An unreadable modifier state blocks typing; a detected change stops the remaining text, so part of a transcript may already have been inserted. Tabs, newlines, and other control characters are rejected before typing because synthetic keys could navigate fields or submit text. Direct mode never copies a blocked transcript to the clipboard; use daemon history for recovery.

## Passive X11 Listen

The `x11-listen` backend is for debugging hotkey state without registering or grabbing the chord:

```bash
parakit doctor --hotkey-backend x11-listen
parakit start --hotkey-backend x11-listen --quiet &
```

Because this backend is passive, it cannot prevent the literal Space key from reaching the focused application. Use the default registered backend for normal dictation.

## Evdev Proxy

The experimental evdev proxy grabs a physical keyboard event device, suppresses the dictation chords' Space events, and forwards other key events through `/dev/uinput`.

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
