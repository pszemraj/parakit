# macOS Desktop Setup

parakit supports Apple Silicon macOS as a terminal-run CLI. Build from source, grant the launching app the required privacy permissions, then run `parakit` from that app.

## Build

Install the native Apple Silicon Metal build using [build.md#install](build.md#install). Build dependencies, source-install library paths, and Rosetta constraints are covered there.

## Permissions

Grant these in System Settings > Privacy & Security:

- [Accessibility](https://support.apple.com/guide/mac-help/allow-accessibility-apps-to-access-your-mac-mh43185/mac): required for the `Left Control+Space` hotkey and synthetic paste/type events.
- [Microphone](https://support.apple.com/guide/mac-help/control-access-to-the-microphone-on-mac-mchla1b1e1fe/mac): required for audio capture.

Parakit uses a modifying CoreGraphics event tap to suppress the hotkey. [Apple distinguishes this Accessibility-controlled tap from listen-only taps](https://developer.apple.com/videos/play/wwdc2019/701/), which require Input Monitoring. `doctor` reports the separate Input Monitoring status for diagnostics; it does not require that grant.

For ordinary terminal launches, macOS attributes permission requests to the terminal application, such as Terminal.app, iTerm2, or Ghostty. Run `doctor` from the same app you use to launch parakit; results from another terminal, an editor, or a coding agent do not establish that launcher's access.

First-run flow:

```bash
parakit doctor
parakit
```

If Accessibility is missing, `doctor` can trigger the macOS prompt. After granting it, restart parakit and rerun `parakit doctor`. If Microphone is not determined yet, the first capture may trigger its prompt; rerun parakit after granting it.

If Accessibility is already enabled but `doctor` reports denied, save terminal work, fully quit and reopen the launching app, then rerun `parakit doctor`. An app updated on disk can still have an older process running; the enabled Settings entry does not establish effective access for that process. Try this before resetting permissions.

### Recovering From a Denied Prompt

> [!TIP]
> macOS remembers a denied or dismissed permission request and may not show the prompt again automatically. You are not locked out: quit parakit, open the relevant Privacy & Security pane linked above, and enable the terminal application that launches parakit. In the Accessibility pane, use the Add button if the terminal is not listed. Then restart the terminal application and rerun `parakit doctor`.

If the entry is stuck or you specifically want macOS to ask again, Apple documents [`tccutil reset <service> [bundle-id]`](https://developer.apple.com/documentation/xcode/resetting-access-to-protected-resources-in-macos). Reset only the failed permission. For Terminal.app:

```bash
tccutil reset Accessibility com.apple.Terminal
tccutil reset Microphone com.apple.Terminal
```

These commands remove Terminal.app's saved decision; they do not grant access. Run parakit again to trigger the applicable prompt, then grant the permission and restart the terminal application. Do not use `sudo`, which broadens the reset beyond the current user. For iTerm2, Ghostty, or another launcher, substitute that application's bundle identifier.

### `doctor --deep`

`parakit doctor --deep` runs a deeper insertion smoke test than plain `doctor`. For the `standard`/`terminal` paste modes it runs two stages and reports which one failed:

1. A suppressed Cmd+V event-tap smoke test: parakit posts a synthetic paste chord and confirms, via a CoreGraphics event tap that suppresses the chord before it reaches any app, that the chord was actually posted.
2. A real end-to-end paste-transaction smoke test: parakit briefly opens a small titled window with a text view, pastes a unique sentinel through the production clipboard-swap/paste/`AXValue` path, then reads the text back and verifies clipboard restoration under the [shared policy](running.md#clipboard-staging-and-restore). Accessibility acknowledgement is required when the probe exposes a focused AX element; otherwise an unverified outcome is accepted only if the sentinel still lands.

For `direct` mode, only a suppressed key-event tap runs, and it passes once a synthetic key-down is observed (direct typing posts unicode key-down events without a matching key-up, and never touches the clipboard or `AXValue` acknowledgement, so there is no transaction to open a window and test).

The check reads back the complete restored clipboard payload and fails on read errors; an empty file list does not hide another format. On failure, cleanup retries only while its plain-text sentinel is current, preserving newer copies, including HTML with the same text.

Because stage 2 opens a real window and drives real Accessibility APIs, `doctor --deep` on macOS requires an active GUI login session (not a headless or SSH-only session) with Accessibility already granted to the terminal. The window closes itself automatically once the check completes.

## Hotkey

The macOS push-to-talk hotkey is `Left Control+Space`. With a Qwen3-ASR model, `Left Control+Left Option+Space` requests [prior-dictation context](models/qwen3-asr.md#prior-dictation-context). Press and hold the shortcut while speaking, then release Control or Space when done. parakit handles these chords with a CoreGraphics event tap and suppresses their Space events. Other modified chords, including `Left Control+Shift+Space` and `Command+Space`, pass through to macOS and the focused app. The context shortcut also passes through with non-Qwen models.

macOS may also use `Control+Space` for input-source switching when multiple input sources are configured. If parakit does not react, or if the input-source switcher appears instead:

1. Open System Settings > Keyboard > Keyboard Shortcuts.
2. Check Input Sources for shortcuts assigned to `Control+Space` or `Control+Option+Space`, and disable or change them.
3. Check other shortcut categories for warning icons; macOS marks conflicting shortcuts there.
4. Restart parakit and rerun `parakit doctor`.

## Background Use

Use the commands in [running.md#background-use](running.md#background-use), launched from the terminal application that holds parakit's privacy permissions.

## Insertion

Parakit matches the frontmost application's focused Accessibility element rather than its on-screen window ID. Same-app field or window changes block insertion without treating palettes, popovers, sheets, or z-order changes as a different target. If an application exposes no focused Accessibility element, matching process and bundle identity provide the fallback; a real application or focused-element mismatch still blocks insertion. Rejected non-direct insertions follow the shared [clipboard staging policy](running.md#clipboard-staging-and-restore). [Paste modes](running.md#insertion) select `Cmd+V` or direct typing through the Accessibility-controlled input path.

After a paste chord is sent, macOS does not restore the previous clipboard contents after a fixed delay. It polls the focused Accessibility element's `AXValue` for evidence that the target consumed the paste, at 40 ms intervals against a roughly 1.8 s deadline. That deadline is a soft target: the poll loop only checks it between reads, not inside one, so a single slow, blocking `AXValue` read can carry the wait past it before the loop notices. Each Accessibility request has its own 250 ms messaging timeout, which bounds how long any one read can block, so an unresponsive target cannot hold the worker for macOS's longer system default. The [insertion outcomes](logging.md#insertion-outcomes) describe acknowledgement states and clipboard decisions. When no focused element or baseline is available after the fallback read, a 1500 ms grace period is used; otherwise polling uses the full confirmation window. Direct typing and insertions rejected before posting a paste chord do not run acknowledgement.

Paste-failure and paste-fallback notifications surface as real Notification Center banners sent through `osascript`/`display notification`. If macOS does not show them, check System Settings > Notifications; some versions file these banners under "Script Editor" rather than under "parakit". The user-visible messages and recovery steps are in [troubleshooting.md#macos-paste-could-not-be-confirmed](troubleshooting.md#macos-paste-could-not-be-confirmed).

### What Counts As Evidence

Parakit tries to read a baseline before posting the chord. If that read is unavailable, it attempts a post-chord baseline, which may already include a fast paste. Bare value growth is insufficient: application output, autocomplete, or another input source can change `AXValue` without the transcript arriving. Confirmation requires newly visible transcript-specific evidence.

Matching ignores whitespace on both sides rather than comparing verbatim. A terminal's `AXValue` is its rendered screen, hard-wrapped at the column width and wrapping mid-word, so a pasted transcript comes back with newlines injected at the wrap points. Dropping whitespace makes the comparison independent of that layout.

Transcripts under 12 normalized characters require an exact baseline-to-current insertion delta. Longer transcripts can also use selection geometry to confirm a reformatted value. In `standard` mode, an increased occurrence count for the whole transcript or a 32-character leading/trailing window can confirm when the value grew. `terminal` mode disables occurrence-count evidence because old text can scroll into its sliding rendered value; exact normalized deltas still confirm hard-wrapped text.

## Metal Verification

The shared library and run-path checks are in [build.md#runtime-library-paths](build.md#runtime-library-paths). To confirm Metal library presence and device visibility:

```bash
parakit doctor --verbose
ls target/release/build/parakit-*/out/lib/libggml-metal.dylib
otool -s __DATA __ggml_metallib target/release/build/parakit-*/out/lib/libggml-metal.dylib | head
```

Build success and device visibility alone do not establish inference acceptance. The [macOS validation report](dev/macos-model-validation-2026-10-18.md) records CPU/Metal model coverage and remaining physical checks. Use the [model-validation checklist](dev/model-validation.md#macos-cpu-and-metal) and [quality procedures](dev/quality.md#qwen-and-moondream-comparisons) when repeating those checks.
