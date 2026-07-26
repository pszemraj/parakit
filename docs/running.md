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

Per-device CLI selection is intentionally not exposed. The Parakeet backend currently ignores CrispASR's `gpu_device` field, so pinning remains backend-specific environment configuration:

```bash
CUDA_VISIBLE_DEVICES=0 parakit --device gpu
GGML_VK_VISIBLE_DEVICES=0 parakit --device gpu
```

Use `parakit --verbose doctor` to list the compute devices visible to bundled ggml.
Verbose daemon startup prints the requested mode and expected device, such as
`device=auto -> Vulkan1 - NVIDIA GeForce RTX 4070 Laptop GPU [GPU]`.

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

`parakit stop` uses the local control socket. `pkill parakit` is still a last-resort option if the process is wedged before the socket starts.

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

The daemon keeps a ring buffer of recent transcripts in memory (`daemon.transcript_history` entries, 10 by default). `paste-last` and `copy-last` act on the most recent one by default; pass `N` (1-based, counting back from the most recent) to reach further back, e.g. `parakit paste-last 3` for the third-most-recent transcript. `parakit history` lists what the daemon currently remembers, newest first; `--limit N` caps how many entries print. This history is memory-only: it is never written to disk and is gone as soon as the daemon stops. `test-paste` runs clipboard staging, focus checks, paste sanitization, and the paste chord without using the microphone.

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
  cleaning:   on (12 rules)
  logging:    jsonl to /home/user/.parakit/logs
  history:    3 of 10
```

The detail block reflects the daemon's own state at query time, not the querying process's flags. If the daemon has not finished starting up yet (or predates this feature), `--verbose status` instead prints a single `detail unavailable (daemon starting or older version)` line after the two lines above.

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

If the default input changes while parakit is idle, the daemon switches when CPAL reports a changed selected device identity and prints the new microphone unless `--quiet` is set. Idle polling is CPAL-only and does not shell out to `pactl`. On Linux PulseAudio/PipeWire systems, startup, probe, and stream reopen paths use `pactl` only to enrich generic `default` source names for human-readable logs and Bluetooth warnings. If an active stream fails, parakit keeps running and retries.

Bluetooth microphones are allowed, but parakit prints a warning because headset profiles often add latency and reduce speech quality. The warning still goes to stderr in `--quiet` mode.

## Insertion

parakit transcribes once on hotkey release, stages plain text on the system clipboard, then sends the configured paste shortcut. By default it gives the target and clipboard history tools time to observe the staged text, then restores the previous clipboard contents when the clipboard API can round-trip them. Current restore support covers text, HTML with a text alternative, copied file lists, and images exposed as normal platform image data. Browser-private image packages, WebP-only payloads, and other clipboard MIME formats are not generally restorable through `arboard`; when restore is required, parakit clears the staged transcript instead of leaving sensitive text as the active clipboard.

OS clipboard history or a third-party clipboard manager is useful for recovering a transcript when the target app rejects paste. On Windows, built-in clipboard history is opened with `Win+V` and must be enabled by the user. Clipboard history tools may retain transcript text after parakit restores the active clipboard; disable them if that retention is not acceptable for your workflow.

On Linux/X11, parakit records the active X11 window when recording starts. If focus clearly changes before insertion, it does not send a paste chord, but non-direct modes still stage the transcript before restoring the active clipboard. If focus capture or recheck fails because X11 is transiently unavailable, parakit pastes anyway; the transcript remains available through `parakit paste-last` or `parakit copy-last` either way. Terminal mode strips trailing newlines and blocks multiline terminal paste.

On Windows, parakit records the foreground window at PTT-down, rechecks it before paste, and sends the paste shortcut with `SendInput`. If the foreground target cannot be captured or verified, automatic paste is skipped, but non-direct modes still stage the transcript before restoring the active clipboard. A normal user process cannot inject into an administrator/elevated target application.

On macOS, parakit records the frontmost application's focused Accessibility UI element at PTT-down, rechecks it before paste, and sends `Cmd+V` or direct typing through the Accessibility-controlled desktop input path. Matching is by focused-element identity rather than on-screen window id, so same-app window switches and transient UI churn (palettes, popovers, sheets, z-order changes) no longer false-positive as a focus change; matching falls back to frontmost app identity (pid + bundle identifier) when Accessibility exposes no focused element on either side. If focus cannot be verified as still matching, automatic paste is skipped, but non-direct modes still stage the transcript before restoring the active clipboard.

After the paste chord is sent, macOS additionally confirms whether the target actually consumed it before deciding what to do with the clipboard, instead of blindly restoring the previous clipboard contents after a fixed delay. Insertion resolves to one of three tiers:

- **Confirmed** — the focused Accessibility element's value was observed to show newly visible transcript-specific evidence within about 1.8 seconds of polling. The previous clipboard contents are restored (or the transcript is cleared) per the usual policy.

  Transcript evidence is compared with the value read *before* the paste chord, so text already present in the field does not count as a new insertion. Bare value growth is deliberately not confirmation: asynchronous application or terminal output can grow the field without consuming the paste, and restoring the clipboard on that signal could destroy the only copy of the transcript. Matching ignores whitespace, so a target that re-wraps the text — a terminal reports its rendered screen, hard-wrapped at the column width, and wraps mid-word — still matches. If the whole transcript is not found, a newly visible 32-character leading or trailing window still counts, which covers a terminal scrolling the head of a long paste off screen or a bounded field truncating the tail.
- **Unverified** — no pollable Accessibility value was available at all (no focused element captured, or the field withholds its value, as secure/password fields deliberately do). After a fixed ~1.5 second grace period, the paste is treated as likely successful and the clipboard is restored per policy, but the outcome is logged distinctly (`pasted_unverified`) so the degraded case stays visible in telemetry.
- **No evidence** — a pollable value was available but never showed the transcript before the deadline. Uncertainty must never destroy the transcript: the previous clipboard is deliberately **not** restored, the transcript remains on the clipboard, parakit plays the error tone, and shows a notification asking you to press Cmd+V to insert it manually.

This acknowledgement step only runs after a paste chord has actually been sent; it does not apply to Linux or Windows, or to direct-typing mode, which keep the previous fixed-delay clipboard restore.

Paste modes:

```bash
parakit --paste-mode terminal  # Ctrl+Shift+V on Linux/Windows, Cmd+V on macOS
parakit --paste-mode standard  # Ctrl+V on Linux/Windows, Cmd+V on macOS
parakit --paste-mode direct    # synthetic typing, no clipboard
```

Use `direct` only as an app-compatibility fallback. It is slower and can be less reliable for non-ASCII text. On Linux it still requires an X11 session.

Use `--keep-transcript-clipboard` when you want successful pastes and blocked fallback text to remain as the active clipboard. The default is to restore the previous supported clipboard contents after staging the transcript.

After repeated paste backend errors, parakit temporarily disables automatic paste and uses the same clipboard/block fallback behavior. It retries automatic paste after a short cooldown instead of requiring a daemon restart.

## Logging And Sounds

Text-only transcription logging:

```bash
parakit --log-dir "$HOME/.parakit/logs"
parakit --log-dir "$HOME/.parakit/logs" --log-format tsv
```

One JSONL or TSV file is written per local day. Records include timestamp, audio seconds, inference milliseconds, raw text, cleaned text, active rule count, and `parakit_version`, which is the Cargo package version embedded in the running binary. This version comes from the binary that writes the dictation log; redirected console diagnostics are separate and are never copied into these records. Audio is never logged.

Each transcription record carries how the transcript was cleaned, and is followed by an insertion-outcome record once insertion resolves, so a transcript, the cleanup applied to it, and how it was (or was not) delivered can all be joined:

- **JSONL**: the transcription line gains `parakit_version`, `cleaner_version`, `cleaning_profile`, `ruleset_id`, `drops_trailing_period`, and `rules_fired` (an ordered array of `{"name":...,"matches":...}` objects). `ruleset_id` and `cleaning_failure` are omitted when they are absent. The insertion outcome is a second, independent line: `{"kind":"insertion","ts":...,"ref_id":<N>,"outcome":...,"target_bundle_id":...,"focus_verification":...,"transcript_chars":...,"paste_event_posted":...,"pasteboard_requested":...,"acknowledgement_kind":...,"acknowledgement_ms":...,"clipboard_restored":...,"failure_reason":...}`. `ref_id` matches the transcription line written immediately before it; the transcription line has no `kind` key.
- **TSV**: the transcription row is buffered rather than written immediately. Columns are the six original transcript columns, then `parakit_version`, then six cleaning columns (`cleaner_version`, `cleaning_profile`, `ruleset_id`, `drops_trailing_period`, `rules_fired`, `cleaning_failure`), then ten insertion columns (`outcome`, `target_bundle_id`, `focus_verification`, `transcript_chars`, `paste_event_posted`, `pasteboard_requested`, `acknowledgement_kind`, `acknowledgement_ms`, `clipboard_restored`, `failure_reason`). `rules_fired` is encoded in a single cell as `name:count;name:count`. Missing/`None` values are empty cells; booleans are `true`/`false`. If insertion never resolves within roughly 30 seconds (e.g. the daemon exits mid-insertion), the buffered row is still flushed with its transcript, version, and cleaning columns and empty insertion columns, so the transcript itself is never silently dropped.

Version and cleaning field meanings: `parakit_version` identifies the installed binary release. `cleaner_version` separates procedural behavior revisions within that release, and `ruleset_id` identifies the exact ordered set of enabled passes, including user rules, so two runs with the same id cleaned text the same way; it is absent when cleaning is off. `cleaning_profile` is `safe`, `aggressive`, or `disabled`. `rules_fired` lists only passes that actually changed the text, in application order. `cleaning_failure` is present only when a bounded matcher exceeded its backtrack limit; the cleaner then fails open, keeping the original transcript rather than inserting partially transformed text. See [cleaning-rules.md](cleaning-rules.md).

Field meanings: `outcome` is a coarse result such as `pasted`, `pasted_unverified` (macOS only; paste chord sent but insertion could not be positively confirmed — see Insertion above), `copied_only`, `blocked`, `skipped`, or `error`. `target_bundle_id` is the macOS bundle identifier of the insertion target when known (always empty/`null` on Linux and Windows today). `focus_verification` is `matched`, `changed`, `ax_unsupported` (macOS only; pid and bundle identifier matched but Accessibility focused-element identity could not be compared), `unavailable` (no focus snapshot was captured at PTT-down), or `not_applicable` (insertion never reached a focus check, e.g. sanitizer skip/copy-only paths). `acknowledgement_kind` records how a post-paste-chord insertion was acknowledged: `ax_confirmed` and `no_evidence` (macOS, a pollable Accessibility value was available), `unverified_timeout` (macOS, no pollable value was available), or `not_applicable` (every other path, including Linux/Windows/direct-mode paste and paths that never sent a paste chord). `acknowledgement_ms` is the milliseconds spent on that acknowledgement wait, when applicable. `clipboard_restored` is `true`/`false` when the clipboard was touched at all, and empty when it was not. `pasteboard_requested` is reserved for a future, more precise pasteboard-read-evidence signal and is always empty today. `failure_reason` holds the error message when `outcome` is `error`.

Disable cue tones:

```bash
parakit --no-sounds
```

## Configuration

parakit reads an optional `config.toml` for daemon defaults, cleaning preferences, transcription logging, the Linux hotkey backend, and user-defined cleaning rules. Precedence is **CLI flags > config file > built-in defaults**.

[configuration.md](configuration.md) is the per-key reference: every key's type, default, accepted values, and interactions. This section is the overview.

Default path, following each platform's normal config-directory convention:

```text
Linux/macOS: $XDG_CONFIG_HOME/parakit/config.toml, falling back to ~/.config/parakit/config.toml
Windows:     %APPDATA%\parakit\config.toml
```

`PARAKIT_CONFIG_PATH` overrides the path outright on every platform.

```bash
parakit config path                 # print the resolved config path
parakit config init                 # write a commented template (fails if a file already exists)
parakit config init --force         # overwrite an existing config file
parakit config show                 # print the resolved path and effective merged values (default for bare `parakit config`)
parakit config edit                 # open $VISUAL or $EDITOR, creating the file from the template first if missing
```

A missing config file is equivalent to an empty one: every key falls back to its built-in default. A config file that fails to parse, or that defines an invalid user rule (bad regex, a name colliding with a built-in rule, or a duplicate user rule name), is a hard error that names the config file path — `parakit config show` and `parakit config edit` are the fastest way to find and fix it.

Template excerpt (every key is commented out by default; see `parakit config init`'s output for the full file):

```toml
[daemon]
# device = "auto"          # "auto", "cpu", or "gpu"
# paste_mode = "standard"  # "terminal", "standard", or "direct"
# verbose = false          # a CLI --quiet flag always wins over this
# transcript_history = 10  # transcripts kept in daemon memory; 0 disables paste-last/copy-last/history

[cleaning]
# enabled = true
# profile = "safe"              # "safe" or "aggressive"
# keep_trailing_period = false  # true keeps the period cleanup drops by default
# disabled_rules = ["fix-trailing-period"]

[logging]
# dir = "/home/user/.parakit/logs"
# format = "jsonl"         # "jsonl" or "tsv"

[hotkey]
# backend = "auto"         # Linux only

# [[rules.user]]
# name = "weights-and-biases-to-wandb"
# pattern = "(?i)\\bweights and biases\\b"
# replacement = "wandb"
# position = "standard"    # "first", "standard" (default), or "last"
```

See [cleaning-rules.md](cleaning-rules.md#user-rules) for the full user-defined rule format, position semantics, and validation errors.

`daemon.transcript_history` only affects daemon memory: it is never written to disk, existing history does not survive a restart, and a changed value takes effect the next time the daemon starts (a running daemon keeps the depth it started with).
