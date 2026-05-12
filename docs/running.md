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

`--verbose` and `--quiet` are global flags, so they go before `doctor`. On Linux, Wayland sessions fail insertion preflight even when XWayland exposes a `DISPLAY`; use an X11 session.

The daemon checks the hotkey backend, insertion backend, and singleton lock before any model download. If those preflights pass, it opens the microphone, warns when the selected source looks like Bluetooth, downloads the default Q8_0 GGUF if it is not already cached, opens the model, and starts the hotkey loop. Linux backend details are in [linux-desktop.md](linux-desktop.md).

Normal startup:

```text
parakit
  model: parakeet-tdt-0.6b-v3-Q8_0.gguf
  dtype: Q8_0 (745 MB)
  mic:   USB Speech Mic Mono, 48000 Hz mono input -> 16000 Hz mono model, F32
Ready: hold Ctrl+Space to dictate.
```

Use `--verbose` only when debugging startup, backend selection, or latency:

```bash
parakit --verbose
parakit --threads 8 --verbose
```

## Background Use

```bash
parakit --quiet &
disown
```

`--quiet` suppresses normal stdout, including startup lines and transcripts. Errors and warnings still go to stderr.

On Linux, start parakit from a terminal in the current desktop session. Tmux, X11 auth, and evdev details are in [linux-desktop.md](linux-desktop.md).

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
parakit paste-last
parakit copy-last
parakit test-paste "hello from parakit"
```

`paste-last` and `copy-last` keep only the latest transcript in daemon memory. `test-paste` runs clipboard staging, focus checks, paste sanitization, and the paste chord without using the microphone.

## Model Cache

With no `-m`, parakit uses the hosted [Q8_0 GGUF model](https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf). `PARAKIT_MODELS_DIR` overrides the model directory. Without that override, `XDG_CACHE_HOME` is honored on Linux, macOS uses `~/Library/Caches/parakit/models/`, and Windows uses `%LOCALAPPDATA%\parakit\Cache\models\`.

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

The microphone stream stays warm while the daemon is running; a bounded ring buffer feeds a drain thread that keeps 350 ms of pre-roll so the beginning of an utterance is less likely to be clipped.

If the default input changes while parakit is idle, the daemon switches when CPAL reports a changed selected device identity and prints the new microphone unless `--quiet` is set. Idle polling is CPAL-only and does not shell out to `pactl`. On Linux PulseAudio/PipeWire systems, startup, probe, and stream reopen paths use `pactl` only to enrich generic `default` source names for human-readable logs and Bluetooth warnings. If an active stream fails, parakit keeps running and retries.

Bluetooth microphones are allowed, but parakit prints a warning because headset profiles often add latency and reduce speech quality. The warning still goes to stderr in `--quiet` mode.

## Insertion

parakit transcribes once on hotkey release, writes plain text to the system clipboard, then sends the configured paste shortcut. By default it waits for the target to consume the paste and restores the previous clipboard contents when the clipboard API can round-trip them. Current restore support covers text, HTML with a text alternative, copied file lists, and images. Other clipboard MIME formats are not generally restorable through `arboard`; if one was active, parakit leaves the staged transcript on the clipboard instead of overwriting an unknown format with empty text.

On Linux/X11, parakit records the active X11 window when recording starts. If focus clearly changes before insertion, it does not send a paste chord. If focus capture or recheck fails because X11 is transiently unavailable, parakit pastes anyway; the transcript remains available through `parakit paste-last` or `parakit copy-last` either way. Terminal mode strips trailing newlines and blocks multiline terminal paste.

On Windows, parakit records the foreground window at PTT-down, rechecks it before paste, and sends the paste shortcut with `SendInput`. A normal user process cannot inject into an administrator/elevated target application.

Paste modes:

```bash
parakit --paste-mode terminal  # Ctrl+Shift+V on Linux/Windows, Cmd+V on macOS
parakit --paste-mode standard  # Ctrl+V on Linux/Windows, Cmd+V on macOS
parakit --paste-mode direct    # synthetic typing, no clipboard
```

Use `direct` only as an app-compatibility fallback. It is slower and can be less reliable for non-ASCII text. On Linux it still requires an X11 session.

Use `--keep-transcript-clipboard` when you want successful pastes and blocked fallback text to remain on the clipboard. The default is to restore the previous supported clipboard contents.

After repeated paste backend errors, parakit temporarily disables automatic paste and uses the same clipboard/block fallback behavior. It retries automatic paste after a short cooldown instead of requiring a daemon restart.

## Logging And Sounds

Text-only transcription logging:

```bash
parakit --log-dir "$HOME/.parakit/logs"
parakit --log-dir "$HOME/.parakit/logs" --log-format tsv
```

One JSONL or TSV file is written per local day. Records include timestamp, audio seconds, inference milliseconds, raw text, cleaned text, and active rule count. Audio is never logged.

Disable cue tones:

```bash
parakit --no-sounds
```
