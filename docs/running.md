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

`--verbose` and `--quiet` are global flags: they work whether they come before or after `doctor`. On Linux, Wayland sessions fail insertion preflight even when XWayland exposes a `DISPLAY`; use an X11 session. On macOS, `doctor` checks Accessibility, Input Monitoring, and Microphone status for the terminal that launched parakit.

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
parakit start --threads 8 --verbose
```

## Device Selection

GPU-capable builds default to `--device auto`. In `auto`, CrispASR asks ggml for the best backend device and falls back to CPU when no GPU backend is available. On hybrid laptops, ggml prefers a discrete GPU over an integrated GPU.

Windows bundle backend selection is covered in [../scripts/windows/README.md#backend-requirements](../scripts/windows/README.md#backend-requirements). At runtime, `--device` controls CPU/GPU use inside the installed build.

```bash
parakit start --device auto
parakit start --device cpu
parakit start --device gpu
```

`--device cpu` opens the session with GPU use disabled. `--device gpu` requests the GPU path and, on bundled builds, fails before model load if ggml reports no discrete or integrated GPU device. On system-library builds without the bundled ggml probe, `--device gpu` continues with a warning because parakit cannot verify device visibility before opening the session.

Per-device CLI selection is not exposed. The Parakeet backend currently ignores CrispASR's `gpu_device` field, so pinning remains backend-specific environment configuration:

```bash
CUDA_VISIBLE_DEVICES=0 parakit start --device gpu
GGML_VK_VISIBLE_DEVICES=0 parakit start --device gpu
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

When the daemon is running, these commands talk to it through local per-user IPC. Unix-like systems use a Unix socket under the parakit runtime directory; Windows uses a named pipe. After upgrading parakit in place, restart the daemon (`parakit stop`, then start it again): `copy-last` uses a wire format that is not compatible across the upgrade until the daemon restarts.

```text
parakit status
parakit stop
parakit copy-last [N]
parakit history
parakit test-paste "hello from parakit"
```

The daemon keeps a ring buffer of recent transcripts in memory (`daemon.transcript_history` entries, 10 by default). `copy-last` acts on the most recent one by default; pass `N` (1-based, counting back from the most recent) to reach further back, e.g. `parakit copy-last 3` for the third-most-recent transcript. `parakit history` lists what the daemon currently remembers, newest first; `--limit N` caps how many entries print. The history ring is never written to disk and disappears when the daemon stops, so treat it as a same-session convenience rather than an archive; a [clipboard history manager](#insertion) is what carries dictations across restarts, and enabled [JSONL logging](#logging-and-sounds) independently persists them. `test-paste` runs clipboard staging, focus checks, paste sanitization, and the paste chord without using the microphone.

Plain `parakit status` output is unchanged and safe for scripts to parse:

```text
$ parakit status
parakit: idle
last transcript: 42 bytes
```

Pass the global `--verbose` flag (before or after the subcommand) to also print a detail block (mic, model, device, uptime, dictation count, and more):

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
  cleaning:   on (safe, 25 rules)
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
parakit start -m /path/to/model.gguf
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
parakit start --paste-mode terminal
parakit start --paste-mode standard
parakit start --paste-mode direct
```

### Clipboard Staging And Restore

`terminal` and `standard` stage plain text on the system clipboard and send the paste shortcut. Both give the target and clipboard history tools time to observe the staged text, then restore the previous clipboard contents when the clipboard API can round-trip them. Restore support covers text, HTML with a text alternative, copied file lists, and images exposed as normal platform image data. Browser-private image packages, WebP-only payloads, and other clipboard MIME formats are not restorable through `arboard`; when restore is required, parakit clears the staged transcript instead of leaving sensitive text as the active clipboard.

`--keep-transcript-clipboard` leaves the transcript as the active clipboard after both successful pastes and blocked fallbacks. The default is to restore the previous supported clipboard contents after staging. On macOS, one case ignores this setting either way: if push-to-talk (or another) modifier key is still physically held down at paste time, sending the chord would post a different shortcut, so parakit skips it and keeps the transcript on the clipboard regardless of the configured policy. This is the quiet path — a "Transcript copied" notification and the success tone, not the error tone — logged as `copied_only`.

Linux clipboard modes use a fixed-delay restore gate. Windows waits for its clipboard-update listener when available and falls back to timing. macOS waits for insertion evidence instead, described below.

Run an OS or third-party clipboard history manager alongside parakit. parakit does not persist dictations on its own: the daemon's in-memory ring holds only the last `daemon.transcript_history` transcripts and is gone when the daemon stops, and [JSONL logging](#logging-and-sounds) writes nothing unless you set a log directory. A clipboard manager is what gives you durable access to something you dictated earlier, and it is also how you recover a transcript when the target application rejects the paste. On Windows, built-in clipboard history is opened with `Win+V` and must be enabled by the user.

Clipboard history tools do retain transcript text after parakit restores the active clipboard. That is the point, but it also means dictated text outlives the paste; disable the manager if that retention is not acceptable for your workflow.

### Focus Changes

On Linux/X11, parakit records the active X11 window when recording starts. If focus clearly changes before insertion, it does not send a paste chord, but non-direct modes still stage the transcript before restoring the active clipboard. If focus capture or recheck fails because X11 is transiently unavailable, parakit pastes anyway; the transcript remains available through `parakit copy-last` either way.

On Windows, parakit records the foreground window at PTT-down, rechecks it before paste, and sends the paste shortcut with `SendInput`. If the foreground target cannot be captured or verified, automatic paste is skipped, but non-direct modes still stage the transcript before restoring the active clipboard. A normal user process cannot inject into an administrator/elevated target application.

On macOS, parakit records the frontmost application's focused Accessibility UI element at PTT-down, rechecks it before paste, and sends `Cmd+V` or direct typing through the Accessibility-controlled desktop input path. Matching is by focused-element identity rather than on-screen window id, so actual same-app field or window changes are detected without relying on transient window-list churn from palettes, popovers, sheets, or z-order changes. Frontmost app identity (pid + bundle identifier) is checked too, but it is not sufficient by itself: if Accessibility exposes no focused element on either side, automatic paste is skipped. Non-direct modes still stage the transcript before restoring the active clipboard.

### Paste Acknowledgement On macOS

After the paste chord is sent, macOS polls the focused element's Accessibility value for evidence that the target consumed it, and only then decides what to do with the clipboard. Several outcomes are possible. When the transcript is seen in the target, the previous clipboard contents are restored (or cleared) per the usual policy. When the target exposes no readable value at all, which is normal for secure and password fields, or no baseline value could be captured to compare later reads against, the paste is treated as successful and the clipboard is restored, at the cost of about 1.5 seconds of added completion latency. When the target instead becomes unobservable partway through polling — its Accessibility object dies and the frontmost application changes, or its focus state can no longer be read at all — the paste is still treated as likely successful, but the clipboard is deliberately *not* restored: the target can never be re-checked, so the transcript is kept as the only remaining copy. The polling deadlines, matching rules, and the reasoning behind each tier are in [macos-desktop.md#insertion](macos-desktop.md#insertion). If restoring the previous clipboard itself fails after a paste that landed or was accepted as unverified-but-restorable, the paste is not retroactively treated as a failure: the same `pasted`/`pasted_unverified` result is logged with `clipboard_restored: false`, and parakit prints a warning that the transcript is likely still on the clipboard.

> [!IMPORTANT]
> One outcome needs you to act. When a readable value never shows the transcript before the deadline, parakit leaves the transcript on the clipboard instead of restoring the previous contents, plays the error tone, and posts a notification. Look at the target before pressing `Cmd+V` yourself: parakit could not confirm the paste, but it may still have landed, and pasting again would insert a second copy.

This step only runs after a paste chord has been sent, so direct typing and blocked insertions skip it.

### Repeated Failures

After repeated paste backend errors, parakit temporarily disables automatic paste and uses the same clipboard/block fallback behavior. It retries automatic paste after a short cooldown instead of requiring a daemon restart.

## Logging And Sounds

Text-only transcription logging:

```bash
parakit start --log-dir "$HOME/.parakit/logs"
```

> [!WARNING]
> Logging stores raw and cleaned transcripts as plaintext, with no retention or size cap. Protect the directory and rotate or delete old files according to the sensitivity of your dictation.

Audio and redirected console output are never included.

Set the same directory persistently through the `logging.dir` config key instead of the flag; see [configuration.md](configuration.md) and [config_reference.toml](config_reference.toml). One append-only `parakit-YYYY-MM-DD.jsonl` file is written per local day. A completed dictation normally writes two independent, synchronously flushed lines: a transcription line as soon as the model and cleaner finish, then a later insertion line once parakit knows what happened to the paste attempt.

```json
{"ts":"2026-07-27T14:02:11.482Z","session_id":"2026-07-27T14:02:10.981234000Z-p4312-l0","record_id":7,"parakit_version":"0.4.0","audio_secs":4.21,"infer_ms":187,"raw":"so the build is green now.","cleaned":"So the build is green now","rules_active":25,"cleaner_version":6,"cleaning_profile":"safe","ruleset_id":"v6-safe-69be753fb69620ef","drops_trailing_period":true,"number_threshold":4.0,"rules_fired":[{"name":"capitalize-sentence-starts","matches":1},{"name":"fix-trailing-period","matches":1}]}
{"kind":"insertion","ts":"2026-07-27T14:02:11.930Z","session_id":"2026-07-27T14:02:10.981234000Z-p4312-l0","ref_id":7,"outcome":"pasted","target_bundle_id":"com.mitchellh.ghostty","focus_verification":"matched","transcript_chars":25,"paste_event_posted":true,"pasteboard_requested":null,"acknowledgement_kind":"ax_confirmed","acknowledgement_ms":312,"clipboard_restored":true,"failure_reason":null}
```

The transcription line is durable before insertion starts, so exiting during insertion can leave one without a corresponding outcome; when both writes succeed, join them by matching the transcription's (`session_id`, `record_id`) pair to the insertion's (`session_id`, `ref_id`) pair, which stays unambiguous across a daemon restart. `outcome` is one of `pasted`, `pasted_unverified`, `copied_only`, `blocked`, `skipped`, or `error`. The full field-by-field reference and what each outcome means are in [logging.md](logging.md).

Disable cue tones:

```bash
parakit start --no-sounds
```

## Configuration

Config file location, commands, precedence, loading, and recovery are in [configuration.md](configuration.md). The type, default, valid values, and interactions for every setting are in [config_reference.toml](config_reference.toml).
