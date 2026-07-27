# Running parakit

parakit runs in the foreground by default. Use that mode once after install, then run it quietly in the background for daily use.

## First Run

```text
parakit doctor && parakit
```

`parakit doctor` checks hotkey access, the selected microphone, insertion support, and the daemon singleton lock without downloading or loading the model. An already-running daemon makes readiness fail; use `parakit status` or `parakit stop` before starting another copy. It exits `0` when startup should proceed and `1` when a blocking issue remains, so it can be used directly in shell conditionals.

Useful variants:

```text
parakit --verbose doctor
parakit --quiet doctor
parakit doctor --deep
```

`--verbose` and `--quiet` are global flags, so they go before `doctor`. On Linux, Wayland sessions fail insertion preflight even when XWayland exposes a `DISPLAY`; use an X11 session. On macOS, `doctor` checks Accessibility, Input Monitoring, and Microphone status for the terminal that launched parakit.

The daemon checks the hotkey backend, insertion backend, and singleton lock before any model download. If those preflights pass, it opens the microphone, warns when the selected source looks like Bluetooth, downloads the default Q8_0 GGUF if it is not already cached, opens the model, and starts the hotkey loop. Linux backend details are in [linux-desktop.md](linux-desktop.md).

Normal startup:

```text
parakit
  model: parakeet-tdt-0.6b-v3-Q8_0.gguf
  dtype: Q8_0 (745 MB)
  mic:   USB Speech Mic Mono, 48000 Hz mono input -> 16000 Hz mono model, F32
Ready: hold Ctrl+Space to dictate.
```

On macOS the ready line says `Left Control+Space`; on Linux and Windows it says `Ctrl+Space`.

Use `--verbose` only when debugging startup, backend selection, or latency:

```bash
parakit --verbose
parakit --threads 8 --verbose
```

## Device Selection

GPU-capable builds default to `--device auto`. In `auto`, CrispASR asks ggml for the best backend device and falls back to CPU when no GPU backend is available. On hybrid laptops, ggml prefers a discrete GPU over an integrated GPU.

Windows bundle backend selection is covered in [../scripts/windows/README.md#backend-requirements](../scripts/windows/README.md#backend-requirements). At runtime, `--device` controls CPU/GPU use inside the installed build.

```bash
parakit --device auto
parakit --device cpu
parakit --device gpu
```

`--device cpu` opens the session with GPU use disabled. `--device gpu` requests the GPU path and, on bundled builds, fails before model load if ggml reports no discrete or integrated GPU device. On system-library builds without the bundled ggml probe, `--device gpu` continues with a warning because parakit cannot verify device visibility before opening the session.

Per-device CLI selection is not exposed. The Parakeet backend currently ignores CrispASR's `gpu_device` field, so pinning remains backend-specific environment configuration:

```bash
CUDA_VISIBLE_DEVICES=0 parakit --device gpu
GGML_VK_VISIBLE_DEVICES=0 parakit --device gpu
```

Use `parakit --verbose doctor` to list the compute devices visible to bundled ggml. Verbose daemon startup prints the requested mode and expected device, such as `device=auto -> Vulkan1 - NVIDIA GeForce RTX 4070 Laptop GPU [GPU]`.

## Background Use

```bash
parakit --quiet &
disown
```

`--quiet` suppresses normal stdout, including startup lines and transcripts. Errors and warnings still go to stderr.

On Linux, start parakit from a terminal in the current desktop session. Tmux, X11 auth, and evdev details are in [linux-desktop.md](linux-desktop.md).

On macOS, start parakit from the terminal app that has Accessibility, Input Monitoring, and Microphone permission. Permission details are in [macos-desktop.md](macos-desktop.md).

Keep stderr in a file:

```bash
mkdir -p "$HOME/.local/state/parakit"
nohup parakit --quiet >/dev/null 2>>"$HOME/.local/state/parakit/parakit.err" &
```

Stop it:

```bash
parakit stop
```

`parakit stop` uses the local control socket. Use `pkill parakit` as a last resort if the process is wedged before the socket starts.

## Control Socket

When the daemon is running, these commands talk to it through local per-user IPC. Unix-like systems use a Unix socket under the parakit runtime directory; Windows uses a named pipe.

```text
parakit status
parakit stop
parakit paste-last [N]
parakit copy-last [N]
parakit history
parakit test-paste "hello from parakit"
```

The daemon keeps a ring buffer of recent transcripts in memory (`daemon.transcript_history` entries, 10 by default). `paste-last` and `copy-last` act on the most recent one by default; pass `N` (1-based, counting back from the most recent) to reach further back, e.g. `parakit paste-last 3` for the third-most-recent transcript. `parakit history` lists what the daemon currently remembers, newest first; `--limit N` caps how many entries print. The history ring is never written to disk and disappears when the daemon stops, but enabled [JSONL logging](#logging-and-sounds) independently persists transcripts. `test-paste` runs clipboard staging, focus checks, paste sanitization, and the paste chord without using the microphone.

Plain `parakit status` output is unchanged and safe for scripts to parse:

```text
$ parakit status
parakit: idle
last transcript: 42 bytes
```

Pass the global `--verbose` flag before the subcommand to also print a detail block (mic, model, device, uptime, dictation count, and more):

```text
$ parakit --verbose status
parakit: idle
last transcript: 42 bytes
  pid:        41213
  uptime:     1h 23m
  dictations: 17
  mic:        USB Speech Mic Mono, 48000 Hz mono input -> 16000 Hz mono model, F32
  model:      parakeet-tdt-0.6b-v3-Q8_0.gguf (Q8_0 (745 MB))
  device:     cpu (CPU, 8 threads)
  paste mode: standard
  sounds:     on
  cleaning:   on (24 rules)
  logging:    JSONL to /home/user/.parakit/logs
  history:    3 of 10
  hotkey:     auto
```

The detail block reflects the daemon's own state at query time, not the querying process's flags. The `hotkey` line is Linux-only. `daemon.verbose = true` does not expand `parakit status`; pass the querying command's global `--verbose` flag as shown. If the daemon has not finished starting up yet (or predates this feature), `--verbose status` instead prints a single `detail unavailable (daemon starting or older version)` line after the two lines above.

## Model Cache

With no `-m`, parakit uses the hosted [Q8_0 GGUF model](https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf). `PARAKIT_MODELS_DIR` overrides the model directory. Without that override, `XDG_CACHE_HOME` is honored on Linux and macOS, both fall back to `~/.cache/parakit/models/`, and Windows uses `%LOCALAPPDATA%\parakit\Cache\models\`.

Useful commands:

```bash
parakit fetch --force
parakit cache
parakit cache list
parakit cache dir
parakit -m /path/to/model.gguf
```

`-m <path>` always wins and disables automatic fetch.

For locked-down or offline machines, seed the default model by placing `parakeet-tdt-0.6b-v3-Q8_0.gguf` in the directory printed by `parakit cache dir`. On the next startup, parakit verifies the compiled-in SHA256 and writes the cache manifest. Use `PARAKIT_MODELS_DIR` when the approved model location is managed by IT or shared across a build image.

## Microphone

parakit follows the OS default input device and avoids monitor/loopback/virtual sources unless no better input is available. When CPAL reports a mono stream with the same sample rate and sample format as the default stream, parakit opens the mono stream. Otherwise it opens the default stream and downmixes multi-channel input to mono before resampling and before model inference. The model input is always 16 kHz mono PCM.

On Linux and macOS, the microphone stream stays warm while the daemon is running; a bounded ring buffer feeds a drain thread that keeps 350 ms of pre-roll so the beginning of an utterance is less likely to be clipped. On Windows, parakit pauses the input stream while idle so `audiodg.exe` and driver-level microphone processing do not burn CPU when no recording is active. Recording start/stop is event-driven; idle device-change polling runs once per second.

One continuously held recording is force-stopped and handed to the worker after 270 seconds. This bounds a missed hotkey-release event without discarding the captured audio.

If the default input changes while parakit is idle, the daemon switches when CPAL reports a changed selected device identity and prints the new microphone unless `--quiet` is set. Idle polling is CPAL-only and does not shell out to `pactl`. On Linux PulseAudio/PipeWire systems, startup, probe, and stream reopen paths use `pactl` only to enrich generic `default` source names for human-readable logs and Bluetooth warnings. If an active stream fails, parakit keeps running and retries.

Bluetooth microphones are allowed, but parakit prints a warning because headset profiles often add latency and reduce speech quality. The warning still goes to stderr in `--quiet` mode.

## Insertion

parakit transcribes once on hotkey release and hands the text to one of three insertion modes.

| Mode | What it sends | When to pick it |
| --- | --- | --- |
| `terminal` | `Ctrl+Shift+V` on Linux and Windows, `Cmd+V` on macOS | Terminal emulators, where `Ctrl+V` is not paste. The default on Linux. Trailing newlines are stripped and multiline transcripts are not auto-pasted. |
| `standard` | `Ctrl+V` on Linux and Windows, `Cmd+V` on macOS | GUI applications. The default on macOS and Windows. |
| `direct` | Synthetic typing through the platform keyboard API; the clipboard is never touched | App-compatibility fallback when neither chord reaches the target. Slower and less reliable for non-ASCII text. On Linux it still requires an X11 session. |

```bash
parakit --paste-mode terminal
parakit --paste-mode standard
parakit --paste-mode direct
```

### Clipboard Staging And Restore

`terminal` and `standard` stage plain text on the system clipboard and send the paste shortcut. Both give the target and clipboard history tools time to observe the staged text, then restore the previous clipboard contents when the clipboard API can round-trip them. Restore support covers text, HTML with a text alternative, copied file lists, and images exposed as normal platform image data. Browser-private image packages, WebP-only payloads, and other clipboard MIME formats are not restorable through `arboard`; when restore is required, parakit clears the staged transcript instead of leaving sensitive text as the active clipboard.

`--keep-transcript-clipboard` leaves the transcript as the active clipboard after both successful pastes and blocked fallbacks. The default is to restore the previous supported clipboard contents after staging.

Linux clipboard modes use a fixed-delay restore gate. Windows waits for its clipboard-update listener when available and falls back to timing. macOS waits for insertion evidence instead, described below.

OS clipboard history or a third-party clipboard manager is useful for recovering a transcript when the target app rejects paste. On Windows, built-in clipboard history is opened with `Win+V` and must be enabled by the user. Clipboard history tools may retain transcript text after parakit restores the active clipboard; disable them if that retention is not acceptable for your workflow.

### Focus Changes

On Linux/X11, parakit records the active X11 window when recording starts. If focus clearly changes before insertion, it does not send a paste chord, but non-direct modes still stage the transcript before restoring the active clipboard. If focus capture or recheck fails because X11 is transiently unavailable, parakit pastes anyway; the transcript remains available through `parakit paste-last` or `parakit copy-last` either way.

On Windows, parakit records the foreground window at PTT-down, rechecks it before paste, and sends the paste shortcut with `SendInput`. If the foreground target cannot be captured or verified, automatic paste is skipped, but non-direct modes still stage the transcript before restoring the active clipboard. A normal user process cannot inject into an administrator/elevated target application.

On macOS, parakit records the frontmost application's focused Accessibility UI element at PTT-down, rechecks it before paste, and sends `Cmd+V` or direct typing through the Accessibility-controlled desktop input path. Matching is by focused-element identity rather than on-screen window id, so same-app window switches and transient UI churn (palettes, popovers, sheets, z-order changes) do not false-positive as a focus change; matching falls back to frontmost app identity (pid + bundle identifier) when Accessibility exposes no focused element on either side. If focus cannot be verified as still matching, automatic paste is skipped, but non-direct modes still stage the transcript before restoring the active clipboard.

### Paste Acknowledgement On macOS

After the paste chord is sent, macOS polls the focused element's Accessibility value for evidence that the target consumed it, and only then decides what to do with the clipboard. Three outcomes are possible. When the transcript is seen in the target, the previous clipboard contents are restored (or cleared) per the usual policy. When the target exposes no readable value at all, which is normal for secure and password fields, the paste is treated as successful and the clipboard is restored, at the cost of about 1.5 seconds of added completion latency. The polling deadlines, matching rules, and the reasoning behind them are in [macos-desktop.md#insertion](macos-desktop.md#insertion).

> [!IMPORTANT]
> The third outcome needs you to act. When a readable value never shows the transcript, parakit leaves the transcript on the clipboard instead of restoring the previous contents, plays the error tone, and posts a notification. Look at the target before pressing `Cmd+V` yourself: parakit could not confirm the paste, but it may still have landed, and pasting again would insert a second copy.

This step only runs after a paste chord has been sent, so direct typing and blocked insertions skip it.

### Repeated Failures

After repeated paste backend errors, parakit temporarily disables automatic paste and uses the same clipboard/block fallback behavior. It retries automatic paste after a short cooldown instead of requiring a daemon restart.

## Logging And Sounds

Text-only transcription logging:

```bash
parakit --log-dir "$HOME/.parakit/logs"
```

> [!WARNING]
> Logging stores raw and cleaned transcripts as plaintext, with no retention or size cap. Protect the directory and rotate or delete old files according to the sensitivity of your dictation.

Audio and redirected console output are never included.

One append-only `parakit-YYYY-MM-DD.jsonl` file is written per local day. Every line is an independent JSON object and is flushed synchronously before the worker continues. A completed dictation normally produces two lines, a transcription line and a later insertion line:

```json
{"ts":"2026-07-27T14:02:11.482Z","parakit_version":"0.4.0","audio_secs":4.21,"infer_ms":187,"raw":"so the build is green now.","cleaned":"So the build is green now","rules_active":24,"cleaner_version":5,"cleaning_profile":"safe","ruleset_id":"v5-safe-9c1f2ab40d7e6538","drops_trailing_period":true,"number_threshold":null,"rules_fired":[{"name":"capitalize-sentence-starts","matches":1},{"name":"fix-trailing-period","matches":1}]}
{"kind":"insertion","ts":"2026-07-27T14:02:11.930Z","ref_id":7,"outcome":"pasted","target_bundle_id":"com.mitchellh.ghostty","focus_verification":"matched","transcript_chars":25,"paste_event_posted":true,"pasteboard_requested":null,"acknowledgement_kind":"ax_confirmed","acknowledgement_ms":312,"clipboard_restored":true,"failure_reason":null}
```

The transcription line is durable before insertion starts, so exiting during insertion can leave one transcription line without a corresponding outcome. When both writes succeed, the insertion line belongs to the immediately preceding transcription line. `ref_id` is a process-local sequence that starts at zero after each daemon restart; the transcription line has no matching ID, so do not use `ref_id` alone to join records across a file.

Transcription line fields, in serialization order:

| Field | Type | Meaning |
| --- | --- | --- |
| `ts` | string | UTC RFC 3339 timestamp with milliseconds. |
| `parakit_version` | string | Cargo package version of the binary that wrote the record. |
| `audio_secs` | number | Length of the recorded utterance. |
| `infer_ms` | integer | Model inference time in milliseconds. |
| `raw` | string | Transcript as returned by the model. |
| `cleaned` | string | Transcript after cleaning passes. |
| `rules_active` | integer | Enabled pass count after profile and disable filtering. |
| `cleaner_version` | integer | Procedural cleaner behavior revision. |
| `cleaning_profile` | string | `safe`, `aggressive`, or `disabled`. |
| `ruleset_id` | string | Identifier of the ordered enabled pass set and configurable cleaning behavior, including user rules and a non-default number threshold. Omitted when cleaning is disabled. |
| `drops_trailing_period` | boolean | Whether the messaging-style terminal-period pass was enabled. |
| `number_threshold` | number or null | Minimum isolated value rendered as digits; `null` is the default convert-all policy. |
| `rules_fired` | array | Passes that changed text, in application order, as `{"name":...,"matches":...}` objects. |
| `cleaning_failure` | string | Error text from a bounded matcher that exceeded its limit; the cleaner then keeps the original transcript instead of inserting a partial transformation. Omitted when cleaning succeeded. |

`ruleset_id` and `cleaning_failure` are the only omittable fields on this line. Everything else is always present, and `number_threshold` serializes as `null` rather than disappearing. Pass semantics are in [cleaning-rules.md](cleaning-rules.md).

Insertion line fields, in serialization order:

| Field | Type | Meaning |
| --- | --- | --- |
| `kind` | string | Always `insertion`; the transcription line carries no `kind`. |
| `ts` | string | UTC RFC 3339 timestamp with milliseconds. |
| `ref_id` | integer | Process-local sequence number of the transcription record this outcome belongs to. |
| `outcome` | string | `pasted`, `pasted_unverified`, `copied_only`, `blocked`, `skipped`, or `error`. |
| `target_bundle_id` | string or null | macOS target bundle identifier when known; `null` on Linux and Windows. |
| `focus_verification` | string | `matched`, `changed`, `ax_unsupported`, `unavailable`, or `not_applicable`. The last value also covers a live focus-recheck error, not only paths that skipped the check. |
| `transcript_chars` | integer | Character count of the transcript offered for insertion. |
| `paste_event_posted` | boolean | Whether a paste chord or type event was sent. |
| `pasteboard_requested` | null | Reserved for a clipboard read-back signal; always `null` today. |
| `acknowledgement_kind` | string | `ax_confirmed`, `no_evidence`, `unverified_timeout`, or `not_applicable`. |
| `acknowledgement_ms` | integer or null | Milliseconds spent waiting for acknowledgement; `null` when no wait occurred. |
| `clipboard_restored` | boolean or null | Whether the previous clipboard was restored; `null` when the result is unknown or inapplicable. |
| `failure_reason` | string or null | Error text when `outcome` is `error`. |

No field on the insertion line is omitted; absent values serialize as `null`. An `error` record is assembled without a completed insertion report, so `clipboard_restored` can be `null` even though the clipboard was touched, and `paste_event_posted` is `false` whether or not an event was attempted.

Disable cue tones:

```bash
parakit --no-sounds
```

## Configuration

Config file location, commands, precedence, loading, and recovery are in [configuration.md](configuration.md). The type, default, valid values, and interactions for every setting are in [config_reference.toml](config_reference.toml).
