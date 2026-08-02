# Running parakit

parakit runs in the foreground by default. Use that mode once after install, then run it quietly in the background for daily use.

## First Run

```text
parakit doctor && parakit
```

`parakit doctor` checks hotkey access, the selected microphone, insertion support, and the daemon singleton lock without downloading or loading the model. An already-running daemon makes readiness fail. Starting again is harmless: `parakit start` prints `parakit: already running` and exits successfully. It exits `0` when startup should proceed and `1` when a blocking issue remains, so it can be used directly in shell conditionals.

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

`parakit stop` uses the local daemon control channel. Use `pkill parakit` as a last resort if the process is wedged before IPC starts.

## Daemon Control

When the daemon is running, these commands talk to it through local per-user IPC. Unix-like systems use a Unix socket under the parakit runtime directory; Windows uses a named pipe. Restart the daemon after upgrading parakit in place (`parakit stop`, then start it again).

```text
parakit status
parakit stop
parakit copy-last [N]
parakit history
parakit test-paste "hello from parakit"
```

`status` and `stop` are safe when no daemon is running. They print `parakit: not running` or `parakit: not running; nothing to stop` and exit successfully. Commands that need daemon state (`history`, `copy-last`, and `test-paste`) instead report that the daemon is not running and point to `parakit start`.

The daemon keeps a ring buffer of recent transcripts in memory (`daemon.transcript_history` entries, 10 by default). `copy-last` acts on the most recent one by default; pass `N` (1-based, counting back from the most recent) to reach further back, e.g. `parakit copy-last 3` for the third-most-recent transcript. `parakit history` lists what the daemon currently remembers, newest first; `--limit N` caps how many entries print. The ring disappears when the daemon stops, so treat it as a same-session convenience. `test-paste` runs clipboard staging, focus checks, paste sanitization, and the paste chord without using the microphone.

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

For locked-down or offline machines, seed the default model by placing `parakeet-tdt-0.6b-v3-Q8_0.gguf` in the directory printed by `parakit cache dir`. Use `PARAKIT_MODELS_DIR` when the approved model location is managed by IT or shared across a build image.

### Other GGUF Builds

`parakit fetch` can also acquire other Parakeet GGUF builds instead of the hosted default, for example the CrispASR author's repo or the widely-used `handy-computer` build:

```bash
parakit fetch cstr/parakeet-tdt-0.6b-v3-GGUF
parakit fetch cstr/parakeet-tdt-0.6b-v3-GGUF --file parakeet-tdt-0.6b-v3-q4_k.gguf
parakit fetch handy-computer/parakeet-tdt-0.6b-v3-gguf@main
parakit fetch https://example.com/models/parakeet-q8.gguf --sha256 <64-hex-sha256>
```

The first positional argument is either a Hugging Face repo (`owner/repo`, optionally `owner/repo@revision`) or a direct `http://`/`https://` URL. Other URL schemes are rejected before any request. For a repo source, `--file` selects a specific `.gguf` file when the repo publishes more than one and none of them is uniquely named `q8_0`; otherwise parakit auto-selects the sole `.gguf` file, or the sole `q8_0` file among several. `--sha256` checks the completed download when you explicitly supply it; parakit does not otherwise hash models or turn an unpinned URL into a persistent checksum policy. Resumed downloads use the server's strong ETag or Last-Modified value through `If-Range`; a partial with no usable validator restarts instead of being appended blindly. `--force` discards any partial and replaces the cached file.

Repo fetches land under a revision-keyed `<cache dir>/hub/<owner>--<repo>--<revision-key>/<file>` directory; URL fetches land under `<cache dir>/url/<url-key>/<file>`, where the key separates exact source URLs that share a filename. Different revisions and URLs therefore cannot overwrite or resume onto one another. Both source classes are separate from the canonical top-level `parakeet-tdt-0.6b-v3-Q8_0.gguf`, and different repos remain separate even when they publish identically named files. `parakit cache list` shows each model's path, GGUF type, and size without reading every model to compute hashes. These fetched models are never picked up automatically: activate one with `parakit start -m <path>` or by setting `daemon.model = "<path>"` in `config.toml`, exactly as with any other custom GGUF.

On a corporate network, set `HF_ENDPOINT` to point every Hugging Face request (including the default hosted download) at an internal mirror, and `HF_TOKEN` to authenticate against it or a gated repo. See [troubleshooting.md](troubleshooting.md#downloads-behind-a-corporate-proxy) for proxy and TLS-interception guidance.

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

`--keep-transcript-clipboard` leaves the transcript as the active clipboard after both successful pastes and blocked fallbacks. The default is to restore the previous supported clipboard contents after staging. On macOS, one case ignores this setting either way: if push-to-talk (or another) modifier key is still physically held down at paste time, sending the chord would post a different shortcut, so parakit skips it and keeps the transcript on the clipboard regardless of the configured policy. This is the quiet path - a "Transcript copied" notification and the success tone, not the error tone - logged as `copied_only`.

Linux clipboard modes use a fixed-delay restore gate. Windows waits for its clipboard-update listener when available and falls back to timing. macOS waits for insertion evidence instead, described below.

The daemon's transcript ring is not durable: it holds only the last `daemon.transcript_history` entries and disappears when the daemon stops. Enable [JSONL logging](logging.md) for parakit-managed durable transcript records. An OS or third-party clipboard history manager provides paste-oriented recovery when a target rejects an insertion; on Windows, built-in clipboard history opens with `Win+V` and must be enabled by the user.

Clipboard history tools do retain transcript text after parakit restores the active clipboard. That is the point, but it also means dictated text outlives the paste; disable the manager if that retention is not acceptable for your workflow.

### Focus Changes

On Linux/X11, parakit records the active X11 window when recording starts. If focus clearly changes before insertion, it does not send a paste chord, but non-direct modes still stage the transcript before restoring the active clipboard. If focus capture or recheck fails because X11 is transiently unavailable, parakit pastes anyway; the transcript remains available through `parakit copy-last` either way.

On Windows, parakit records the foreground window at PTT-down, rechecks it before paste, and sends the paste shortcut with `SendInput`. If the foreground target cannot be captured or verified, automatic paste is skipped, but non-direct modes still stage the transcript before restoring the active clipboard. A normal user process cannot inject into an administrator/elevated target application.

On macOS, parakit records the frontmost application's focused Accessibility UI element at PTT-down, rechecks it before paste, and sends `Cmd+V` or direct typing through the Accessibility-controlled desktop input path. Matching is by focused-element identity rather than on-screen window id, so actual same-app field or window changes are detected without relying on transient window-list churn from palettes, popovers, sheets, or z-order changes. When an application exposes no focused Accessibility element, matching process and bundle identity provide the fallback instead of blocking insertion; a real application or focused-element mismatch still blocks it. Non-direct modes still stage the transcript before restoring the active clipboard.

### Paste Acknowledgement On macOS

After a paste chord is sent, macOS waits for Accessibility evidence before deciding whether to restore the clipboard. Confirmed and safely unverified pastes restore it; losing the target during confirmation or reaching the deadline with no insertion evidence keeps the transcript available. Direct typing and blocked insertions skip this step. The evidence tiers and clipboard decisions are in [macos-desktop.md#insertion](macos-desktop.md#insertion), and their JSONL representation is in [logging.md#insertion-outcomes](logging.md#insertion-outcomes).

If parakit reports that a paste could not be confirmed, inspect the target before pressing `Cmd+V`: the paste may have landed even though Accessibility did not expose it.

## Logging And Sounds

Enable text-only JSONL logging with:

```bash
parakit start --log-dir "$HOME/.parakit/logs"
```

Set the same directory persistently with `logging.dir`. Logging is off by default and stores transcript text as plaintext. File layout, privacy implications, record correlation, and the complete schema are in [logging.md](logging.md).

Disable cue tones:

```bash
parakit start --no-sounds
```

## Configuration

Config file location, commands, precedence, loading, and recovery are in [configuration.md](configuration.md). The type, default, valid values, and interactions for every setting are in [config_reference.toml](config_reference.toml).
