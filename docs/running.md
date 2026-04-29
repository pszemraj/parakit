# Running parakit

parakit is a foreground CLI process by default. It becomes a background daemon
when launched with `--quiet` and shell job control.

## Foreground Check

Run the foreground path first after an install or model change:

```bash
parakit doctor
parakit
```

`parakit doctor` checks the active hotkey backend and reports the microphone
parakit would use. It does not download or load a model. On Linux/X11 it probes
desktop hotkey registration first and reports evdev access only as a fallback.
It also reports the compiled ggml CPU/backend flags so CPU performance issues
can be debugged without rebuilding.
On the first real run, parakit downloads the default Q8_0 GGUF into the model
cache before opening the microphone. Confirm that the model loads,
`Ctrl+Space` records, text insertion works, and errors are visible in the
terminal.

Normal startup is concise:

```text
parakit
  model: parakeet-tdt-0.6b-v3-Q8_0.gguf
  dtype: Q8_0 (745 MB)
  mic:   RODE NT-USB+ Mono, 48000 Hz input -> 16000 Hz model, mono, F32
Ready: hold Ctrl+Space to dictate.
```

Use `--verbose` when debugging startup, backend selection, or latency:

```bash
parakit --verbose
parakit --threads 8 --verbose
```

Verbose mode includes full paths, CrispASR backend, thread count, and timing
lines for inference, cleanup, insertion, and total post-release latency. It
also prints the build flags reported by the bundled CrispASR/ggml build.

## Background Launch

Quiet mode suppresses stdout. Errors and warnings still go to stderr.

```bash
parakit --quiet &
```

Install the binary onto `PATH` from the repository:

```bash
cargo install --path .
```

Cargo installs to `~/.cargo/bin` by default. Add it to `PATH` if needed:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

If the default model is not cached yet, `parakit --quiet &` downloads it before
starting the daemon. No stdout is printed in quiet mode; download and startup
errors still go to stderr.

Detach it from the current shell:

```bash
parakit --quiet &
disown
```

Keep stderr in a file with `nohup`:

```bash
mkdir -p "$HOME/.local/state/parakit"
nohup parakit --quiet >/dev/null 2>>"$HOME/.local/state/parakit/parakit.err" &
```

Stop the daemon:

```bash
pkill parakit
```

or:

```bash
pgrep parakit
kill <pid>
```

## Quiet Mode

`--quiet` is intended for normal background use.

It suppresses startup status, ready messages, transcript output, streaming
partials, `--list-rules`, and `--test-rules`.

It does not suppress startup errors, model load errors, audio device errors,
insertion errors, transcription logging write failures, or sound device
warnings. Those still go to stderr.

## Microphone Selection

parakit follows the operating system default input device. Monitor, loopback,
virtual, null, dummy, BlackHole, Soundflower, and similar sources are avoided
unless no physical-looking input is available.

If the default input changes while parakit is running, the daemon switches when
idle and prints the new microphone in normal mode. If the active stream fails
or a device disappears, parakit keeps running and retries with the best
available input.

The microphone line reports the opened input stream rate and the 16 kHz model
target. A 48 kHz USB microphone should therefore look like:

```text
mic:   RODE NT-USB+ Mono, 48000 Hz input -> 16000 Hz model, mono, F32
```

## Transcription Logging

`--log-dir` records text pairs for later cleanup-model training:

```bash
parakit --log-dir "$HOME/.parakit/logs"
parakit --log-dir "$HOME/.parakit/logs" --log-format tsv
```

One file is written per local day:

```text
parakit-YYYY-MM-DD.jsonl
parakit-YYYY-MM-DD.tsv
```

JSONL schema:

```json
{"ts":"2026-04-27T15:32:11.842Z","audio_secs":4.21,"infer_ms":187,"raw":"...","cleaned":"...","rules_active":72}
```

Logging is synchronous and flushed per record. Failures are reported to stderr
but never abort transcription. Audio is not logged.

## Modes

Batch mode is the default:

```bash
parakit --mode batch
```

It records the full utterance and transcribes once on hotkey release. This is
the recommended mode. Batch insertion uses the system clipboard and then sends
the configured paste shortcut. parakit restores the previous clipboard when the
previous contents were text; non-text clipboard contents can be replaced.

The default paste shortcut is terminal-friendly:

```bash
parakit --paste-mode terminal  # default: Ctrl+Shift+V on Linux/Windows, Cmd+V on macOS
parakit --paste-mode standard  # Ctrl+V on Linux/Windows, Cmd+V on macOS
```

macOS uses `Cmd+V` for both modes because terminal paste is normally exposed
through the application paste shortcut there.

Streaming mode sends chunks while the hotkey is still held:

```bash
parakit --mode streaming
parakit --mode streaming:2.5
```

Streaming reduces perceived latency but can split words at chunk boundaries.
Parakeet-TDT is primarily an offline model, so batch mode should be used for
quality checks.

## Sounds

parakit generates short cue tones in-process:

- start: recording began;
- success: transcription was inserted;
- error: transcription or insertion failed.

Disable cues:

```bash
parakit --no-sounds
```
