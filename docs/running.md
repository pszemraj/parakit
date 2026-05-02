# Running parakit

parakit runs in the foreground by default. Use that mode once after install, then run it quietly in the background for daily use.

## First Run

```bash
parakit doctor && parakit
```

`parakit doctor` checks hotkey access, the selected microphone, insertion support, and daemon lock state without downloading or loading the model. It exits `0` when startup should proceed and `1` when a blocking issue remains, so it can be used directly in shell conditionals.

Useful variants:

```bash
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
  mic:   USB Speech Mic Mono, 48000 Hz input -> 16000 Hz model, mono, F32
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
pkill parakit
```

## Model Cache

With no `-m`, parakit uses the hosted [Q8_0 GGUF model](https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf). `XDG_CACHE_HOME` is honored on Linux. macOS uses `~/Library/Caches/parakit/models/`; Windows uses `%LOCALAPPDATA%\parakit\Cache\models\`.

Useful commands:

```bash
parakit fetch --force
parakit cache
parakit cache list
parakit cache dir
parakit -m /path/to/model.gguf
```

`-m <path>` always wins and disables automatic fetch.

## Microphone

parakit follows the OS default input device and avoids monitor/loopback/virtual sources unless no better input is available.

If the default input changes while parakit is idle, the daemon switches and prints the new microphone unless `--quiet` is set. Idle polling is CPAL-only and does not shell out to `pactl`. On Linux PulseAudio/PipeWire systems, startup, probe, and stream reopen paths use `pactl` only to enrich generic `default` source names for human-readable logs and Bluetooth warnings. If an active stream fails, parakit keeps running and retries.

Bluetooth microphones are allowed, but parakit prints a warning because headset profiles often add latency and reduce speech quality. The warning still goes to stderr in `--quiet` mode.

## Insertion

parakit transcribes once on hotkey release, writes the transcript to the system clipboard, sends the configured paste shortcut, then restores the previous text clipboard when possible. Clipboard managers may still keep the transient transcript in history.

On Linux/X11, parakit records the focused X11 window when recording starts. If focus changes before release, it copies the transcript to the clipboard and does not paste into the new target.

Paste modes:

```bash
parakit --paste-mode terminal  # Ctrl+Shift+V on Linux/Windows, Cmd+V on macOS
parakit --paste-mode standard  # Ctrl+V on Linux/Windows, Cmd+V on macOS
parakit --paste-mode direct    # synthetic typing, no clipboard
```

Use `direct` only as an app-compatibility fallback. It is slower and can be less reliable for non-ASCII text. On Linux it still requires an X11 session.

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
