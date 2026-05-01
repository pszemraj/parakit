# Running parakit

parakit runs in the foreground by default. Use that mode once after install,
then run it quietly in the background for daily use.

## First Run

```bash
parakit doctor && parakit
```

`parakit doctor` checks hotkey access, the selected microphone, insertion
support, and daemon lock state without downloading or loading the model. It
exits `0` when startup should proceed and `1` when a blocking issue remains, so
it can be used directly in shell conditionals.

Useful variants:

```bash
parakit --verbose doctor
parakit --quiet doctor
parakit doctor --deep
```

`--verbose` and `--quiet` are global flags, so they go before `doctor`.

The first real `parakit` run downloads the default Q8_0 GGUF if it is not
already cached, then opens the microphone and hotkey backend.

Normal startup:

```text
parakit
  model: parakeet-tdt-0.6b-v3-Q8_0.gguf
  dtype: Q8_0 (745 MB)
  mic:   RODE NT-USB+ Mono, 48000 Hz input -> 16000 Hz model, mono, F32
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

`--quiet` suppresses normal stdout, including startup lines and transcripts.
Errors and warnings still go to stderr.

On Linux, start parakit from a terminal in the current desktop session. Tmux,
X11 auth, and evdev details are in [linux-desktop.md](linux-desktop.md).

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

With no `-m`, parakit uses the hosted Q8_0 model from
<https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf>. `XDG_CACHE_HOME`
is honored on Linux. macOS uses `~/Library/Caches/parakit/models/`; Windows
uses `%LOCALAPPDATA%\parakit\Cache\models\`.

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

parakit follows the OS default input device and avoids monitor/loopback/virtual
sources unless no better input is available.

If the default input changes while parakit is idle, the daemon switches and
prints the new microphone unless `--quiet` is set. If an active stream fails,
parakit keeps running and retries.

## Insertion

Batch mode is the default and recommended mode:

```bash
parakit --mode batch
```

It transcribes once on hotkey release, writes the transcript to the system
clipboard, sends the configured paste shortcut, then restores the previous text
clipboard when possible. Clipboard managers may still keep the transient
transcript in history.

Paste modes:

```bash
parakit --paste-mode terminal  # Ctrl+Shift+V on Linux/Windows, Cmd+V on macOS
parakit --paste-mode standard  # Ctrl+V on Linux/Windows, Cmd+V on macOS
parakit --paste-mode direct    # synthetic typing, no clipboard
```

Use `direct` only as an app-compatibility fallback. It is slower and can be
less reliable for non-ASCII text.

Streaming mode is temporarily disabled while Linux batch dictation is being
stabilized. Use batch mode for quality checks.

## Logging And Sounds

Text-only transcription logging:

```bash
parakit --log-dir "$HOME/.parakit/logs"
parakit --log-dir "$HOME/.parakit/logs" --log-format tsv
```

One JSONL or TSV file is written per local day. Records include timestamp,
audio seconds, inference milliseconds, raw text, cleaned text, and active rule
count. Audio is never logged.

Disable cue tones:

```bash
parakit --no-sounds
```
