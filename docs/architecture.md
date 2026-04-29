# Architecture

parakit keeps the daemon thread-based. There is no async runtime.

## Thread Model

```text
main thread
  parses CLI
  runs the hotkey backend
  sends Start/Stop/StreamChunk events

audio manager thread
  owns the current cpal::Stream
  follows the OS default input device
  reopens capture when the active device changes or fails

cpal callback thread
  receives microphone samples
  mixes to mono
  resamples to 16 kHz when needed
  appends samples while recording is active

worker thread
  owns Engine
  runs transcribe -> clean -> inject
  writes optional transcription logs
  sends sound cues

sound thread
  owns rodio::OutputStream
  plays generated cue tones from a channel
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
being rejected. This keeps behavior consistent with the file-transcription
helper and avoids treating quick utterances as a separate error class.

In streaming mode, a ticker sends `StreamChunk` events while recording. The
final stop event transcribes only the unconsumed tail.

## Ownership Constraints

The layout is driven by platform and library constraints:

- `cpal::Stream` is not reliably `Send`, so the live stream stays on the audio
  manager thread that created it.
- `rodio::OutputStream` is not reliably `Send`, so the sound stream lives on
  its own thread.
- `crispasr::Session` is `Send` but not `Sync`, so the worker owns `Engine`
  directly. Do not wrap it in `Arc<Engine>`.
- Linux/X11 uses a desktop hotkey registration first. The low-level
  `rdev::grab` backend remains available as a fallback when evdev input access
  is explicitly granted.
- The active hotkey backend must suppress the literal Space key before it
  reaches the focused application.

Cross-thread communication uses atomics, mutex-protected buffers, and
crossbeam channels.

## Module Map

| Module | Responsibility |
| --- | --- |
| `src/main.rs` | CLI, hotkey state machine, worker thread, streaming ticker. |
| `src/daemon/audio_manager.rs` | Microphone selection, capture, mono mixdown, resampling, stream restart, shared buffer. |
| `src/fetch.rs` | Hosted Q8_0 download, source rebuilds, checksum verification, and manifest handling. |
| `src/model.rs` | Model names, hosted GGUF naming, cache paths, hosted URLs, and checksum constants. |
| `src/gguf.rs` | Minimal GGUF dtype reader for startup reporting. |
| `src/inference.rs` | CrispASR session wrapper and short-audio padding. |
| `src/rules.rs` | Built-in transcript cleanup rules. |
| `src/daemon/inject.rs` | Synthetic typing through Enigo. |
| `src/daemon/sounds.rs` | Generated audio cue thread. |
| `src/data_log.rs` | JSONL/TSV transcription logging. |
| `examples/transcribe-file.rs` | File-based smoke and quality checks. |
| `scripts/transcribe_nemo_parakeet.py` | NeMo reference transcription helper. |

## Failure Policy

Critical startup failures return an error and stop the process:

- model cannot be opened;
- microphone cannot be opened;
- hotkey grab cannot be installed.

Runtime failures are reported and the daemon continues when possible:

- a sound cue cannot play;
- a log record cannot be written;
- one transcription fails;
- one text injection fails.
