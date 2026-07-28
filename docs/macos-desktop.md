# macOS Desktop Setup

parakit supports Apple Silicon macOS as a terminal-run CLI. Build from source, grant the terminal the required privacy permissions once, then run `parakit` from that terminal.

## Build

Install the native Apple Silicon Metal build using [build.md#install](build.md#install). Build dependencies, source-install library paths, and Rosetta constraints are covered there.

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
2. A real end-to-end paste-transaction smoke test: parakit briefly opens a small, unobtrusive titled window with a text field, pastes a unique sentinel into it through the same production clipboard-swap/paste/`AXValue`-acknowledgement path the daemon uses for real dictation, then verifies the text landed, was Accessibility-acknowledged, and that supported text, HTML, file-list, or image clipboard content was restored. Unsupported clipboard formats are cleared instead of being reconstructed; the format limits are described in [running.md#insertion](running.md#insertion).

For `direct` mode, only the suppressed key-event tap runs (direct typing never touches the clipboard or `AXValue` acknowledgement, so there is no transaction to open a window and test).

Because stage 2 opens a real window and drives real Accessibility APIs, `doctor --deep` on macOS requires an active GUI login session (not a headless or SSH-only session) with Accessibility already granted to the terminal. The window closes itself automatically once the check completes.

## Hotkey

The default macOS push-to-talk hotkey is `Left Control+Space`. This deliberately avoids `Command+Space`, which is normally Spotlight. Press and hold `Left Control+Space` while speaking, then release when done. parakit handles the chord with a CoreGraphics event tap and suppresses the Space key while the exact chord is active. Modified chords such as `Left Control+Shift+Space`, `Control+Option+Space`, or `Command+Space` pass through to macOS and the focused app.

macOS may also use `Control+Space` for input-source switching when multiple input sources are configured. If parakit does not react, or if the input-source switcher appears instead:

1. Open System Settings > Keyboard > Keyboard Shortcuts.
2. Check Input Sources for shortcuts assigned to `Control+Space` or `Control+Option+Space`, and disable or change them.
3. Check other shortcut categories for warning icons; macOS marks conflicting shortcuts there.
4. Restart parakit and rerun `parakit doctor`.

The push-to-talk chord is not configurable yet; macOS uses `Left Control+Space`.

## Background Use

Use the commands in [running.md#background-use](running.md#background-use), launched from the terminal application that holds parakit's privacy permissions.

## Insertion

Paste modes, focus-change handling, clipboard staging, and recovery commands are in [running.md#insertion](running.md#insertion). This section covers the acknowledgement step that is unique to macOS.

After a paste chord is sent, macOS does not restore the previous clipboard contents after a fixed delay. It polls the focused Accessibility element's `AXValue` for evidence that the target consumed the paste, at 40 ms intervals against a 1800 ms deadline, and insertion resolves to one of three tiers:

- **Confirmed** - the element's value showed newly visible transcript-specific evidence before the deadline. The previous clipboard contents are restored, or the transcript is cleared, per the usual clipboard policy. Logged as `outcome: pasted` with `acknowledgement_kind: ax_confirmed`.
- **Unverified** - no pollable Accessibility value was available at all: no focused element was captured, or the field withholds its value, as secure and password fields do by design. After a fixed 1500 ms grace period the paste is treated as likely successful and the clipboard is restored per policy, logged as `outcome: pasted_unverified` with `acknowledgement_kind: unverified_timeout` so the degraded case stays visible in telemetry. The grace period runs before the success cue, so targets that never expose `AXValue` add about 1.5 seconds of perceived completion latency.
- **No evidence** - a pollable value was available but never showed the transcript before the deadline, or the element died mid-poll. Uncertainty must never destroy the transcript, so the previous clipboard is not restored: the transcript stays on the clipboard, parakit plays the error tone, and a notification asks you to press `Cmd+V` to insert it manually. Logged as `outcome: copied_only` with `acknowledgement_kind: no_evidence`.

Paste-failure and paste-fallback notifications (transcript copied, paste blocked, paste temporarily disabled, microphone unavailable/recovered) surface as real Notification Center banners on macOS, sent through `osascript`/`display notification`. If macOS does not show them, check System Settings > Notifications; some macOS versions file `osascript`-originated notifications under "Script Editor" rather than under "parakit".

### What Counts As Evidence

The baseline value is read *before* the chord is posted, not after. Reading it afterward races the target: an app that refreshes its accessibility tree coarsely (terminals especially; ghostty confirms in roughly 300 ms where Safari and Discord confirm in roughly 40 ms) can already have the pasted text in the first post-chord read. From then on the value never changes, so a paste that landed perfectly is indistinguishable from one that was dropped, and the transaction reports no evidence: an error chime and a withheld clipboard restore on a completely successful dictation. Whether that happened came down to timing, which made it look intermittent and arbitrary from the outside.

Bare value growth is not confirmation. A focused element can grow for reasons unrelated to the paste: asynchronous application output, autocomplete, a remote terminal update, or another input source can all change `AXValue` during the confirmation window. Treating any length increase as evidence would restore the previous clipboard even when the transcript never landed, destroying the only remaining copy. Confirmation requires transcript-specific evidence instead.

Matching ignores whitespace on both sides rather than comparing verbatim. A terminal's `AXValue` is its rendered screen, hard-wrapped at the column width and wrapping mid-word, so a pasted transcript comes back with newlines injected at the wrap points. Dropping whitespace makes the comparison independent of that layout.

When the whole transcript cannot be found, a newly visible 32-character leading or trailing window still counts. A terminal scrolls the head of a long paste off the top of the screen and a bounded field truncates the tail, but either end appearing verbatim is real evidence rather than coincidence: a natural-language run of that length is effectively unique against whatever the field held beforehand. Transcripts at or under 32 characters do not use occurrence matching at all, since short text such as `ok` can appear inside unrelated growth such as `token`. They require either an exact baseline-to-current insertion after whitespace normalization or the exact selection and value-length transition expected from the paste.

## Metal Verification

Use verbose doctor output to confirm the Metal build and visible compute device:

```bash
parakit --verbose doctor
```

For release builds, the Metal backend should be in the generated sibling library directory:

```bash
ls target/release/build/parakit-*/out/lib/libggml-metal.dylib
otool -L target/release/build/parakit-*/out/lib/libggml.dylib
otool -l "$(command -v parakit)" | grep -A2 LC_RPATH
otool -s __DATA __ggml_metallib target/release/build/parakit-*/out/lib/libggml-metal.dylib | head
```
