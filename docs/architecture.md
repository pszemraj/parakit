# Architecture

parakit keeps the daemon thread-based. There is no async runtime.

## Thread Model

```text
main thread          CLI, hotkey backend, Start/Stop/StreamChunk events
audio manager thread owns the current cpal::Stream and follows the default mic
cpal callback thread mixes, resamples, and appends samples while recording
worker thread        owns Engine and runs transcribe -> clean -> insert
sound thread         owns rodio::OutputStream and plays cue tones
```

## State Machine

```text
Idle
  Ctrl+Space down
Recording
  Ctrl+Space up
Transcribing
  worker finishes
Idle
```

Very short captures are right-padded with silence before inference instead of
being rejected.

In streaming mode, a ticker sends `StreamChunk` events while recording. The
final stop event transcribes only the unconsumed tail.

## Ownership Constraints

- `cpal::Stream` is not reliably `Send`, so the live stream stays on the audio
  manager thread.
- `rodio::OutputStream` is not reliably `Send`, so the sound stream lives on
  its own thread.
- `crispasr::Session` is `Send` but not `Sync`, so the worker owns `Engine`
  directly. Do not wrap it in `Arc<Engine>`.
- Linux `auto` uses the evdev backend first when all input devices are readable;
  otherwise it uses an X11 desktop hotkey registration.
- The X11 hotkey backend refreshes while idle and fails over instead of staying
  alive with a dead shortcut after desktop session churn.
- The active hotkey backend must suppress the literal Space key before it
  reaches the focused application.

Cross-thread communication uses atomics, mutex-protected buffers, and
crossbeam channels.

## Module Map

| Path | Responsibility |
| --- | --- |
| `src/main.rs` | CLI, worker thread, streaming ticker. |
| `src/daemon/hotkey.rs` | X11 desktop hotkey, rdev fallback, hotkey state helpers. |
| `src/daemon/audio_manager.rs` | Microphone selection, capture, mono mixdown, resampling, stream restart, shared buffer. |
| `src/fetch.rs` | Hosted Q8_0 download, source rebuilds, checksum verification. |
| `src/model.rs` | Model names, hosted GGUF naming, cache paths, hosted URLs, and checksum constants. |
| `src/gguf.rs` | Minimal GGUF dtype reader for startup reporting. |
| `src/inference.rs` | CrispASR session wrapper and short-audio padding. |
| `src/rules.rs` | Built-in transcript cleanup rules. |
| `src/daemon/inject.rs` | Batch paste/direct insertion and streaming synthetic typing. |
| `src/daemon/sounds.rs` | Generated audio cue thread. |
| `src/data_log.rs` | JSONL/TSV transcription logging. |
| `examples/transcribe-file.rs` | File-based smoke and quality checks. |
| `scripts/transcribe_nemo_parakeet.py` | NeMo reference transcription helper. |

## Failure Policy

Startup failures stop the process when the model, microphone, or hotkey backend
cannot be opened.

Runtime failures are reported and the daemon continues when possible: sound
cues, log writes, individual transcriptions, and text insertion failures.
