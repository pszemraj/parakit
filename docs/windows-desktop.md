# Windows Desktop Setup

## Build

Build with the [Windows bundle scripts](../scripts/windows/README.md) or [install an existing bundle](../scripts/windows/README.md#install-an-existing-bundle). Run the daemon from a native Windows terminal for desktop input. See [first run](running.md#first-run).

## Microphone Permissions

In Windows 11, open Settings > Privacy & security > Microphone and enable microphone access for desktop apps. On Windows 10, use Settings > Privacy > Microphone. See [Microsoft's permission instructions](https://support.microsoft.com/en-us/windows/privacy/turn-on-app-permissions-for-your-microphone-in-windows), then rerun `parakit doctor`.

## Hotkey

Parakit registers the fixed `Ctrl+Space` chord through Win32's `RegisterHotKey`. If another app owns it, `parakit doctor` reports `hotkey FAIL` and the daemon cannot start. Doctor immediately releases the shortcuts it probes. Its verbose output reports conflicts on the optional Qwen chord while leaving ordinary dictation available. See [Qwen context shortcuts and AltGr](models/qwen3-asr.md#prior-dictation-context).

If dictation does not start when you hold the chord, check your IME for a conflicting input-mode shortcut.

Windows has a single hotkey backend, so the Linux-only `--hotkey-backend` and `--insertion-backend` flags are not accepted.

## Insertion

Parakit checks the foreground window before sending input with `SendInput`. If the target cannot be captured or verified, automatic insertion is skipped. Run the target app at the same privilege level as Parakit so Windows permits input delivery. See the [clipboard staging policy](running.md#clipboard-staging-and-restore).

Parakit uses a Windows clipboard-update listener to time restoration, with a timed fallback when it cannot observe the update. This gives clipboard history time to see the staged write. Check the target app to confirm insertion. See [paste modes](running.md#insertion) and [insertion outcomes](logging.md#insertion-outcomes).

## `doctor --deep`

Run `parakit doctor --deep` from an unlocked interactive desktop. In `standard` or `terminal` mode it briefly focuses a visible Win32 edit window, sends the configured chord, and reads back the pasted text. It preserves newer clipboard copies during cleanup. If it has to click the window to focus it, the cursor returns to its original position. In `direct` mode the command performs preflight only.

For clipboard-history timing, receiving-app behavior, and native inference checks, see [Windows validation](dev/model-validation.md#windows-cpu-and-supported-gpu-backends).
