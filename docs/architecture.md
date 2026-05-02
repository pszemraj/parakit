# Architecture

parakit keeps the daemon thread-based. There is no async runtime.

## Thread Model

```text
main thread          CLI, hotkey backend, RecordingStarted/RecordingStopped events
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

Empty or near-silent captures are skipped before inference. Short non-silent captures are right-padded with silence before inference instead of being rejected.

Live capture keeps resampler state inside the audio pipeline. Starting a new recording resets that state, and stopping a recording flushes any partial resampler input into the same utterance before the worker sees the PCM buffer. Recording uses a session epoch so stale CPAL callbacks from a stopped stream cannot append into the next utterance.

Streaming mode is currently disabled while the Linux batch path is stabilized.

## Ownership Constraints

- `cpal::Stream` is not reliably `Send`, so the live stream stays on the audio manager thread.
- `rodio::OutputStream` is not reliably `Send`, so the sound stream lives on its own thread.
- `crispasr::Session` is `Send` but not `Sync`, so the worker owns `Engine` directly. Do not wrap it in `Arc<Engine>`.
- Linux `auto` and `desktop` hotkeys register `Ctrl+Space` with X11 through `global-hotkey`; `evdev-proxy` is the explicit experimental evdev/uinput path. Linux text insertion uses X11/XTest and rejects Wayland sessions.
- The active hotkey backend must suppress the literal Space key before it reaches the focused application.

Cross-thread communication uses atomics, mutex-protected buffers, and crossbeam channels.

## Module Map

| Path | Responsibility |
| --- | --- |
| `src/main.rs` | CLI, worker thread, batch PTT simulation helper. |
| `src/daemon/hotkey.rs` | Linux registered hotkey and evdev-proxy backends, non-Linux rdev grab, hotkey state helpers. |
| `src/daemon/audio_manager.rs` | Microphone selection, capture, mono mixdown, resampling, stream restart, shared buffer. |
| `src/fetch.rs` | Hosted [Q8_0 GGUF](https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf) download, source rebuilds, checksum verification. |
| `src/model.rs` | Model names, hosted GGUF naming, cache paths, hosted URLs, and checksum constants. |
| `src/gguf.rs` | Minimal GGUF dtype reader for startup reporting. |
| `src/inference.rs` | [CrispASR](https://github.com/CrispStrobe/CrispASR) session wrapper and short-audio padding. |
| `src/rules.rs` | Built-in transcript cleanup rules. |
| `src/daemon/inject.rs` | Focus guard, clipboard fallback, batch paste/direct insertion. |
| `src/daemon/sounds.rs` | Generated audio cue thread. |
| `src/data_log.rs` | JSONL/TSV transcription logging. |
| `src/audio_file.rs` | WAV decoding, mono mixing, and file resampling for quality tools and PTT simulation. |
| `tools/transcribe-file.rs` | File-based smoke and quality checks. |
| `scripts/transcribe_nemo_parakeet.py` | NeMo reference transcription helper. |

## Failure Policy

Startup failures stop the process when the model, microphone, or hotkey backend cannot be opened.

Runtime failures are reported and the daemon continues when possible: sound cues, log writes, individual transcriptions, and text insertion failures.
