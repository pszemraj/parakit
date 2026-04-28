# Running parakit

parakit is a foreground CLI process by default. It becomes a background daemon
when launched with `--quiet` and shell job control.

## Foreground Check

Run the foreground path first after an install or model change:

```bash
parakit
```

Confirm that the model loads, the microphone opens, `Ctrl+Space` records, text
injection works, and errors are visible in the terminal.

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

The default model is resolved from the cache populated by
[`parakit fetch`](../README.md#model-setup):

```bash
parakit fetch
parakit --quiet &
```

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
injection errors, transcription logging write failures, or sound device
warnings. Those still go to stderr.

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
the recommended mode.

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
- success: transcription was injected;
- error: transcription or injection failed.

Disable cues:

```bash
parakit --no-sounds
```
