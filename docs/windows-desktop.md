# Windows Desktop Setup

parakit runs from a Windows terminal and installs as a per-user bundle. Its hotkey and insertion APIs do not require macOS-style Accessibility or Input Monitoring grants. Microphone access still depends on Windows privacy settings.

## Build

Build and install with the [Windows bundle scripts](../scripts/windows/README.md). Shared native dependencies are in [build.md#native-dependencies](build.md#native-dependencies). Native model and resampler checks are in [model validation](dev/model-validation.md#windows-cpu-and-supported-gpu-backends).

## Microphone Permissions

In Windows 11, open Settings > Privacy & security > Microphone and enable microphone access for desktop apps. On Windows 10, use Settings > Privacy > Microphone. See [Microsoft's permission instructions](https://support.microsoft.com/en-us/windows/privacy/turn-on-app-permissions-for-your-microphone-in-windows), then rerun `parakit doctor`.

## Hotkey

The push-to-talk chord is `Ctrl+Space` and is not configurable. parakit registers it with Win32's `RegisterHotKey`, which gives a clean, OS-level conflict signal instead of racing another listener for the same keys.

`parakit doctor` probes the same registration and releases it immediately, so an already-owned chord shows up as a `hotkey` `FAIL` before you start the daemon. If another application already holds `Ctrl+Space` and you start the daemon anyway, `RegisterHotKey` fails and parakit exits right away with a message pointing back at `doctor`.

Some Windows input methods, particularly CJK IMEs, bind `Ctrl+Space` to toggle input mode. If parakit never reacts to the held chord, check for a conflicting IME shortcut before assuming a bug.

Windows has a single hotkey backend. The Linux-only `--hotkey-backend` flag documented in [linux-desktop.md](linux-desktop.md) is not accepted on Windows; the `RegisterHotKey` path is always used.

## Insertion

Parakit compares the recording's foreground window before sending the paste shortcut with `SendInput`. If the target cannot be captured or verified, automatic paste is skipped. A normal user process cannot inject into an administrator/elevated target application. Rejected non-direct insertions still follow the shared [clipboard staging policy](running.md#clipboard-staging-and-restore).

When its background clipboard-history listener is available, Windows uses it to time restoration after the staged write reaches the clipboard; otherwise it uses a timed fallback. Neither path confirms that text landed in the target; see [insertion outcomes](logging.md#insertion-outcomes). [Paste modes](running.md#insertion) select the shortcut or direct typing.

## `doctor --deep`

In `standard` or `terminal` mode, `doctor --deep` opens a visible Win32 edit window, briefly takes focus, stages a sentinel on the clipboard, sends the configured paste chord, reads the text back, and restores supported clipboard content. Windows may use a brief cursor-click fallback to focus the probe, then restores the cursor position afterward. Run it from an unlocked interactive desktop. In `direct` mode, it performs backend preflight only and does not type into the probe.

The probe checks Win32 input delivery and edit-control readback. It stages and restores the clipboard directly, so passing does not establish the daemon's clipboard-history timing or restore gate, or paste behavior in other applications.
