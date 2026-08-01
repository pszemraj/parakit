# Windows Desktop Setup

parakit supports Windows as a terminal-run CLI, built from source and installed as a per-user bundle by the repo's build scripts. Unlike macOS, Windows needs no privacy-permission grants: `RegisterHotKey` and `SendInput` are ordinary APIs available to any non-elevated process. Daemon behavior - hotkey, insertion, microphone - matches Linux and macOS except where noted below.

## Build

Build and install with the [Windows bundle scripts](../scripts/windows/README.md); a bare `cargo install --path .` does not copy the generated CrispASR/ggml DLLs beside `parakit.exe`. Shared native dependencies and backend controls are in [build.md](build.md).

## Hotkey

The default push-to-talk chord is `Ctrl+Space`, the same default as Linux. parakit registers it with Win32's `RegisterHotKey`, which gives a clean, OS-level conflict signal instead of racing another listener for the same keys.

`parakit doctor` probes the same registration and releases it immediately, so an already-owned chord shows up as a `hotkey` `FAIL` before you start the daemon. If another application already holds `Ctrl+Space` and you start the daemon anyway, `RegisterHotKey` fails and parakit exits right away with a message pointing back at `doctor`.

Some Windows input methods, particularly CJK IMEs, bind `Ctrl+Space` to toggle input mode. If parakit never reacts to the held chord, check for a conflicting IME shortcut before assuming a bug.

Windows has a single hotkey backend. The Linux-only `--hotkey-backend` flag documented in [linux-desktop.md](linux-desktop.md) is not accepted on Windows; the `RegisterHotKey` path is always used.

The chord is not configurable yet; Windows uses `Ctrl+Space`.

## Insertion

Paste modes (`terminal`, `standard`, `direct`), clipboard staging, restore policy, and focus-change handling are the same system documented in [running.md#insertion](running.md#insertion); this section only covers what differs on Windows.

Batch paste sends the paste chord (`Ctrl+Shift+V` for `terminal`, `Ctrl+V` for `standard`) as a single batched `SendInput` call. `direct` mode types through `enigo`'s Unicode `SendInput` path instead of touching the clipboard.

Windows has no post-paste confirmation step like macOS's Accessibility polling. A background clipboard-history listener - a hidden message-only window that watches for clipboard-update notifications - lets parakit wait for its own staged write to actually reach the clipboard before restoring the previous contents, instead of guessing with a fixed delay. That listener only times the clipboard restore; it never confirms that the paste landed in the focused application. Because there is no landing signal, every Windows insertion record's `acknowledgement_kind` is `not_applicable`, whether or not clipboard-history observation was available for that paste. Full field semantics are in [logging.md](logging.md).

Foreground-window capture, the pre-paste focus recheck, and the restriction against injecting into an elevated target application are described in [running.md#focus-changes](running.md#focus-changes).

## `doctor --deep`

In `standard` or `terminal` mode, `doctor --deep` opens a visible Win32 edit window, briefly takes focus, stages a sentinel on the clipboard, sends the configured paste chord, reads the text back, and restores supported clipboard content. Windows may use a brief cursor-click fallback to focus the probe, then restores the cursor position afterward. Run it from an unlocked interactive desktop. In `direct` mode, it performs backend preflight only and does not type into the probe.
