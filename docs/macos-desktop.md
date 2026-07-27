# macOS Desktop Setup

parakit supports Apple Silicon macOS as a terminal-run CLI. Build from source, grant the terminal the required privacy permissions once, then run `parakit` from that terminal.

## Build

Install the native Apple Silicon Metal build using
[build.md#install](build.md#install). Build dependencies, source-install
library paths, and Rosetta constraints are covered there.

## Permissions

Grant these in System Settings > Privacy & Security:

- Accessibility: required for the `Left Control+Space` hotkey and synthetic paste/type events.
- Input Monitoring: required for the CoreGraphics event tap that observes `Left Control+Space`.
- Microphone: required for audio capture.

Grant permissions to the terminal application that launches parakit, such as Terminal.app, iTerm2, or Ghostty. This is the recommended source-build flow because the grant attaches to the terminal's stable app identity and survives parakit rebuilds.

First-run flow:

```bash
parakit doctor
parakit
```

If Accessibility is missing, `doctor` can trigger the macOS prompt. Input Monitoring must be granted manually in System Settings. After changing either permission, restart parakit and rerun `parakit doctor`. If Microphone is not determined yet, the first capture may trigger the Microphone prompt; rerun parakit after granting it.

### `doctor --deep`

`parakit doctor --deep` runs a deeper insertion smoke test than plain `doctor`. For the `standard`/`terminal` paste modes it runs two stages and reports which one failed:

1. A suppressed Cmd+V event-tap smoke test: parakit posts a synthetic paste chord and confirms, via a CoreGraphics event tap that suppresses the chord before it reaches any app, that the chord was actually posted.
2. A real end-to-end paste-transaction smoke test: parakit briefly opens a small, unobtrusive titled window with a text field, pastes a unique sentinel into it through the same production clipboard-swap/paste/`AXValue`-acknowledgement path the daemon uses for real dictation, then verifies the text landed, was Accessibility-acknowledged, and that the clipboard was restored to whatever it held before the test.

For `direct` mode, only the suppressed key-event tap runs (direct typing never touches the clipboard or `AXValue` acknowledgement, so there is no transaction to open a window and test).

Because stage 2 opens a real window and drives real Accessibility APIs, `doctor --deep` on macOS requires an active GUI login session (not a headless or SSH-only session) with Accessibility already granted to the terminal. The window closes itself automatically once the check completes.

## Hotkey

The default macOS push-to-talk hotkey is `Left Control+Space`. This deliberately avoids `Command+Space`, which is normally Spotlight. Press and hold `Left Control+Space` while speaking, then release when done. parakit handles the chord with a CoreGraphics event tap and suppresses the Space key while the exact chord is active. Modified chords such as `Left Control+Shift+Space`, `Control+Option+Space`, or `Command+Space` pass through to macOS and the focused app.

macOS may also use `Control+Space` for input-source switching when multiple input sources are configured. If parakit does not react, or if the input-source switcher appears instead:

1. Open System Settings > Keyboard > Keyboard Shortcuts.
2. Check Input Sources for shortcuts assigned to `Control+Space` or `Control+Option+Space`, and disable or change them.
3. Check other shortcut categories for warning icons; macOS marks conflicting shortcuts there.
4. Restart parakit and rerun `parakit doctor`.

Custom hotkeys are deferred to a future config file. Until then, macOS has one default hotkey.

## Background Use

Use the commands in [running.md#background-use](running.md#background-use),
launched from the terminal application that holds Parakit's privacy
permissions.

## Insertion

macOS insertion behavior, focus-change handling, and recovery commands are in [running.md#insertion](running.md#insertion).

Paste-failure and paste-fallback notifications (transcript copied, paste blocked, paste temporarily disabled, microphone unavailable/recovered) surface as real Notification Center banners on macOS, sent through `osascript`/`display notification`. If macOS does not show them, check System Settings > Notifications; some macOS versions file `osascript`-originated notifications under "Script Editor" rather than under "parakit".

## Metal Verification

Use verbose doctor output to confirm the Metal build and visible compute device:

```bash
parakit --verbose doctor
```

For release builds, the Metal backend should be in the generated sibling library directory:

```bash
ls target/release/build/parakit-*/out/lib/libggml-metal.dylib
otool -L target/release/build/parakit-*/out/lib/libggml.dylib
otool -l "$HOME/.cargo/bin/parakit" | grep -A2 LC_RPATH
otool -s __DATA __ggml_metallib target/release/build/parakit-*/out/lib/libggml-metal.dylib | head
```
