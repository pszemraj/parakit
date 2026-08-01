# macOS Desktop Setup

parakit supports Apple Silicon macOS as a terminal-run CLI. Build from source, grant the terminal the required privacy permissions once, then run `parakit` from that terminal.

## Build

Install the native Apple Silicon Metal build using [build.md#install](build.md#install). Build dependencies, source-install library paths, and Rosetta constraints are covered there.

## Permissions

Grant these in System Settings > Privacy & Security:

- [Accessibility](https://support.apple.com/guide/mac-help/allow-accessibility-apps-to-access-your-mac-mh43185/mac): required for the `Left Control+Space` hotkey and synthetic paste/type events.
- [Input Monitoring](https://support.apple.com/guide/mac-help/control-access-to-input-monitoring-on-mac-mchl4cedafb6/mac): required for the CoreGraphics event tap that observes `Left Control+Space`.
- [Microphone](https://support.apple.com/guide/mac-help/control-access-to-the-microphone-on-mac-mchla1b1e1fe/mac): required for audio capture.

Grant permissions to the terminal application that launches parakit, such as Terminal.app, iTerm2, or Ghostty. This is the recommended source-build flow because the grant attaches to the terminal's stable app identity and survives parakit rebuilds.

First-run flow:

```bash
parakit doctor
parakit
```

If Accessibility is missing, `doctor` can trigger the macOS prompt. Input Monitoring must be granted manually in System Settings. After changing either permission, restart parakit and rerun `parakit doctor`. If Microphone is not determined yet, the first capture may trigger the Microphone prompt; rerun parakit after granting it.

### Recovering From a Denied Prompt

> [!TIP]
> macOS remembers a denied or dismissed permission request and may not show the prompt again automatically. You are not locked out: quit parakit, open the relevant Privacy & Security pane linked above, and enable the terminal application that launches parakit. In the Accessibility pane, use the Add button if the terminal is not listed. Then restart the terminal application and rerun `parakit doctor`.

If the entry is stuck or you specifically want macOS to ask again, Apple documents [`tccutil reset <service> [bundle-id]`](https://developer.apple.com/documentation/xcode/resetting-access-to-protected-resources-in-macos). Reset only the failed permission. For Terminal.app:

```bash
tccutil reset Accessibility com.apple.Terminal
tccutil reset ListenEvent com.apple.Terminal
tccutil reset Microphone com.apple.Terminal
```

`ListenEvent` is the TCC service behind Input Monitoring. These commands remove Terminal.app's saved decision; they do not grant access. Run parakit again to trigger the applicable prompt, then grant the permission and restart the terminal application. Do not use `sudo`, which broadens the reset beyond the current user. For iTerm2, Ghostty, or another launcher, substitute that application's bundle identifier.

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

After a paste chord is sent, macOS does not restore the previous clipboard contents after a fixed delay. It polls the focused Accessibility element's `AXValue` for evidence that the target consumed the paste, at 40 ms intervals against a roughly 1.8 s deadline. That deadline is a soft target: the poll loop only checks it between reads, not inside one, so a single slow, blocking `AXValue` read can carry the wait past it before the loop notices. Each Accessibility request has its own 250 ms messaging timeout, which bounds how long any one read can block, so an unresponsive target cannot hold the worker for macOS's longer system default. Insertion resolves to one of four tiers:

- **Confirmed** - the element's value showed newly visible transcript-specific evidence before the deadline. The previous clipboard contents are restored, or the transcript is cleared, per the usual clipboard policy. Logged as `outcome: pasted` with `acknowledgement_kind: ax_confirmed`.
- **Unverified** - either no pollable Accessibility value was available at all (no focused element was captured, or the field withholds its value, as secure and password fields do by design; `acknowledgement_kind: unverified_timeout`), a pollable element existed but no baseline value could be read before or immediately after the chord (`acknowledgement_kind: unverified_no_baseline`), or a transcript under 12 normalized characters reached the deadline without its required exact insertion delta (`acknowledgement_kind: unverified_short_transcript`). The first two paths use a fixed 1500 ms grace period; the short-transcript path uses the full confirmation window. The paste is treated as likely successful and the clipboard is restored per policy, logged as `outcome: pasted_unverified` so the degraded case stays visible in telemetry.
- **Unverified, focus lost** - the captured Accessibility element died mid-poll (routine, e.g. WebKit replacing an editable field's object) and the fallback re-read then found the frontmost application had changed, or its focus state could no longer be read at all, before any evidence appeared. The chord was posted into a verified-focused target and very likely landed, but that target can never be re-observed to check, so unlike the other `Unverified` cases above, the previous clipboard is *not* restored: the transcript is kept as the only remaining copy. Logged as `outcome: pasted_unverified` with `acknowledgement_kind: unverified_focus_lost` and `clipboard_restored: false`.
- **No evidence** - a pollable value was available but never showed the transcript before the deadline, whether the originally captured element stayed alive the whole time or was reacquired from the same still-focused frontmost application after dying mid-poll. Uncertainty must never destroy the transcript, so the previous clipboard is not restored: the transcript stays on the clipboard, parakit plays the error tone, and a notification asks you to press `Cmd+V` to insert it manually. Logged as `outcome: copied_only` with `acknowledgement_kind: no_evidence`.

Paste-failure and paste-fallback notifications surface as real Notification Center banners sent through `osascript`/`display notification`. If macOS does not show them, check System Settings > Notifications; some versions file these banners under "Script Editor" rather than under "parakit". The user-visible messages and recovery steps are in [troubleshooting.md#macos-paste-could-not-be-confirmed](troubleshooting.md#macos-paste-could-not-be-confirmed).

### What Counts As Evidence

The baseline value is read *before* the chord is posted, not after. Reading the baseline after the chord would race the target: an app that refreshes its accessibility tree coarsely (terminals especially; ghostty confirms in roughly 300 ms where Safari and Discord confirm in roughly 40 ms) can already have the pasted text in the first post-chord read. From then on the value never changes, so a paste that landed perfectly would be indistinguishable from one that was dropped, and the transaction would report no evidence: an error chime and a withheld clipboard restore on a completely successful dictation. That outcome would hinge entirely on how fast the target happened to refresh its accessibility tree, which is exactly what would make it look intermittent and arbitrary from the outside. Reading the baseline first sidesteps the race instead of chasing its symptoms.

Bare value growth is not confirmation. A focused element can grow for reasons unrelated to the paste: asynchronous application output, autocomplete, a remote terminal update, or another input source can all change `AXValue` during the confirmation window. Treating any length increase as evidence would restore the previous clipboard even when the transcript never landed, destroying the only remaining copy. Confirmation requires transcript-specific evidence instead.

Matching ignores whitespace on both sides rather than comparing verbatim. A terminal's `AXValue` is its rendered screen, hard-wrapped at the column width and wrapping mid-word, so a pasted transcript comes back with newlines injected at the wrap points. Dropping whitespace makes the comparison independent of that layout.

In `standard` mode, when the whole transcript cannot be found, a newly visible 32-character leading or trailing window can count after the target's total value length has grown. Either end appearing as a new occurrence rather than merely already being present is transcript-specific evidence against a stable GUI value. `terminal` mode does not use occurrence-count evidence: a terminal exposes a sliding rendered `AXValue`, so old identical text can scroll into view without the new paste landing. An exact normalized insertion delta still confirms hard-wrapped terminal text.

Very short transcripts, under 12 normalized characters, do not use occurrence or selection-geometry matching: text that short, such as `ok`, can collide with unrelated target activity. They require an exact baseline-to-current insertion after whitespace normalization. If unrelated churn defeats that exact delta, the result is `unverified_short_transcript`: parakit restores the previous clipboard without the error banner that could prompt a duplicate manual paste. At and above the floor, selection geometry can confirm a reformatted value. In `standard` mode, a whole-transcript or distinctive-window occurrence-count increase can also confirm when the target's total value grew; `terminal` mode disables that evidence because its rendered value slides.

## Metal Verification

Use verbose doctor output to confirm the Metal build and visible compute device:

```bash
parakit doctor --verbose
```

For release builds, the Metal backend should be in the generated sibling library directory:

```bash
ls target/release/build/parakit-*/out/lib/libggml-metal.dylib
otool -L target/release/build/parakit-*/out/lib/libggml.dylib
otool -l "$(command -v parakit)" | grep -A2 LC_RPATH
otool -s __DATA __ggml_metallib target/release/build/parakit-*/out/lib/libggml-metal.dylib | head
```
