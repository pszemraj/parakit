# macOS Desktop Setup

## Build

Install from a native Apple Silicon terminal. See [installation and Metal builds](build.md#install).

## Permissions

In System Settings > Privacy & Security, grant the launching terminal app access to Accessibility for hotkeys and synthetic events, and to Microphone for capture. See Apple's [Accessibility](https://support.apple.com/guide/mac-help/allow-accessibility-apps-to-access-your-mac-mh43185/mac) and [Microphone](https://support.apple.com/guide/mac-help/control-access-to-the-microphone-on-mac-mchla1b1e1fe/mac) instructions.

Accessibility grants the modifying CoreGraphics event tap used for the hotkey. Doctor reports Input Monitoring for diagnostics only. See [Apple's explanation of event-tap permissions](https://developer.apple.com/videos/play/wwdc2019/701/).

Check readiness in the same terminal app you use to launch Parakit.

```bash
parakit doctor
```

`doctor` can trigger the Accessibility prompt. The first capture can trigger the Microphone prompt. Restart Parakit after granting access, then continue with the [first run](running.md#first-run).

### Recovering From a Denied Prompt

macOS remembers denied or dismissed requests. Enable the terminal app in the relevant pane, using Add in the Accessibility pane if necessary. If Settings already shows the permission on, save your work and fully quit and reopen the terminal app before running `parakit doctor` again. This also reloads an app updated on disk while its older process stayed open.

If access is still denied, reset the saved decision for the affected service as your desktop user. These examples target Terminal.app. Substitute your launching app's bundle identifier for other terminals. See [Apple's reset command](https://developer.apple.com/documentation/xcode/resetting-access-to-protected-resources-in-macos).

```bash
tccutil reset Accessibility com.apple.Terminal
tccutil reset Microphone com.apple.Terminal
```

Run Parakit again to trigger the prompt, grant permission, and restart the terminal app.

### `doctor --deep`

Run `parakit doctor --deep` from an active GUI login with Accessibility already granted. In `standard` and `terminal` modes it runs two checks.

1. A CoreGraphics event tap observes and suppresses a synthetic Cmd+V chord.
2. A temporary window receives a unique paste, reads the text back, and verifies the complete restored clipboard payload. The check requires Accessibility acknowledgement when the probe exposes a focused element. On failure, cleanup preserves newer copies, including HTML with the same plain text.

In `direct` mode the suppressed tap only checks for a Unicode key-down, with no probe window.

## Hotkey

Hold `Left Control+Space` while speaking, then release Control or Space to transcribe. The event tap suppresses Space for active dictation shortcuts and passes through other chords such as `Command+Space` for Spotlight. The Qwen context chord also passes through with non-Qwen models. The Linux-only `--hotkey-backend` and `--insertion-backend` flags are not accepted. See [Qwen context shortcuts](models/qwen3-asr.md#prior-dictation-context).

If the shortcut opens the input-source switcher or Parakit does not respond, check for a conflict.

1. Open System Settings > Keyboard > Keyboard Shortcuts.
2. Check Input Sources for shortcuts assigned to `Control+Space` or `Control+Option+Space`, and disable or change them.
3. Check other shortcut categories for warning icons; macOS marks conflicting shortcuts there.
4. Restart parakit and rerun `parakit doctor`.

## Insertion

Parakit matches the frontmost application's focused Accessibility element. A field change within the same app blocks insertion, while palettes, popovers, sheets, and window ordering matter only when they change the focused element. When the app exposes no focused element, Parakit checks process and bundle identity. See [paste modes](running.md#insertion) and the [clipboard staging policy](running.md#clipboard-staging-and-restore).

After sending the paste chord, Parakit polls the focused element's `AXValue` every 40 ms for up to 1.8 seconds, with a 250 ms timeout per Accessibility request. A read in progress can extend that wait. When there is no focused element or usable baseline, it waits 1.5 seconds before applying the clipboard policy. See [insertion outcomes](logging.md#insertion-outcomes).

Paste failure and fallback messages appear as Notification Center banners. If they are missing, look in System Settings > Notifications under Parakit or Script Editor. See [paste recovery](troubleshooting.md#macos-paste-could-not-be-confirmed).

### What Counts As Evidence

Confirmation compares the accessible text before and after paste. If the initial read fails, Parakit tries a baseline after sending the chord, which can miss a fast paste. Matching ignores whitespace so terminal line wraps do not change the comparison.

Transcripts under 12 normalized characters require an exact insertion delta. For longer transcripts, selection geometry can confirm reformatted text when the focused element matches. In `standard` mode, growth plus a new occurrence of the transcript or a 32-character edge can also confirm insertion. Terminal scrollback can reveal older occurrences, so `terminal` mode excludes that evidence.

## Metal Verification

Inspect the embedded shader section and device visibility after a [Metal build](build.md#crispasr-and-backends).

```bash
parakit doctor --verbose
otool -s __DATA __ggml_metallib "$(command -v parakit)" | head
```

For runtime verification, see [loader paths](build.md#runtime-library-paths) and [macOS CPU and Metal validation](dev/model-validation.md#macos-cpu-and-metal).
