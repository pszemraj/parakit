# Architecture

parakit keeps the daemon thread-based. There is no long-running async runtime.

## Thread Model

```text
main thread                  CLI, setup, and blocking hotkey backend
recording coordinator thread hotkey transition -> focus snapshot -> audio start/stop -> PCM handoff
audio manager thread         owns the current cpal::Stream and follows the default mic
cpal callback thread         mixes input to mono and pushes frames into a bounded SPSC ring
audio drain thread           drains ring -> resamples -> updates pre-roll and active recording
worker thread                owns Engine and runs transcribe -> clean -> insert
sound thread                 opens rodio output only while playing cue tones
IPC thread                   handles status, stop, paste-last, copy-last, and test-paste commands
```

## State Machine

```text
Idle
  Ctrl+Space down
Recording
  Ctrl+Space up
Transcribing
  paste, optional clipboard fallback, block, or skip
Idle
```

Empty or near-silent captures are skipped before inference. Short non-silent captures are right-padded with silence before inference instead of being rejected.

Live capture drains callback audio through a bounded single-producer/single-consumer ring buffer. Linux and macOS keep the microphone stream open for 350 ms pre-roll; Windows opens the stream paused and resumes it only while recording so `audiodg.exe` and driver-level processing do not run while idle. Recording uses a session epoch so stale drained samples from a stopped utterance cannot append into the next utterance.

## Ownership Constraints

- `cpal::Stream` is not reliably `Send`, so the live stream stays on the audio manager thread.
- `rodio::OutputStream` is not reliably `Send`, so cue playback lives on its own thread and opens output only for the duration of a cue.
- `crispasr::Session` is `Send` but not `Sync`, so the worker owns `Engine` directly. Do not wrap it in `Arc<Engine>`.
- Hotkey backends emit only logical press/release transitions. They do not call audio, ASR, clipboard, or insertion code.
- Linux `auto`, `desktop`, and `x11-global-hotkey` register `Ctrl+Space` with X11 through `global-hotkey`; `x11-listen` is passive debugging, and `evdev-proxy-experimental` is the explicit experimental evdev/uinput path. Linux text insertion uses X11/XTest and rejects Wayland sessions.
- Windows registers `Ctrl+Space` with `RegisterHotKey`, pastes with `SendInput`, checks the foreground window before insertion, and uses a per-user named pipe for daemon commands.
- Normal dictation hotkey backends must suppress the literal Space key before it reaches the focused application. The passive `x11-listen` backend is for debugging and does not suppress keys.

Cross-thread communication uses atomics, mutex-protected buffers, and crossbeam channels.

## Module Map

| Path | Responsibility |
| --- | --- |
| `src/main.rs` | CLI, daemon setup, and batch PTT simulation helper. |
| `src/daemon/hotkey.rs` | Hotkey backends and hotkey state helpers. |
| `src/daemon/recording.rs` | Hotkey transition coordinator, focus snapshot, audio start/stop, and PCM handoff. |
| `src/daemon/audio_manager.rs` | Microphone selection, live stream ownership, ring-buffer drain, pre-roll, resampling, and restart. |
| `src/daemon/audio_pactl.rs` | Linux `pactl` parsing for startup/reopen microphone display details. |
| `src/daemon/worker.rs` | ASR worker, paste sanitizer, focus guard, clipboard fallback, and insertion circuit breaker. |
| `src/daemon/ipc.rs` | Local control socket for `status`, `stop`, `paste-last`, `copy-last`, and `test-paste`. |
| `src/daemon/windows_{focus,input,paste_smoke,security}.rs` | Windows foreground checks, `SendInput` paste helpers, deep paste smoke test, and privilege diagnostics. |
| `src/daemon/{preflight,session,x11,alsa}.rs` | Startup checks, session events, X11 helpers, and ALSA stderr suppression. |
| `src/daemon/{logging,notifications,sounds}.rs` | Runtime logging, desktop notifications, and generated audio cues. |
| `src/fetch.rs` | Hosted [Q8_0 GGUF](https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf) download, source rebuilds, checksum verification. |
| `src/model.rs` | Model names, hosted GGUF naming, cache paths, hosted URLs, and checksum constants. |
| `src/gguf.rs` | Minimal GGUF dtype reader for startup reporting. |
| `src/inference.rs` | [CrispASR](https://github.com/CrispStrobe/CrispASR) session wrapper and short-audio padding. |
| `src/rules.rs` | Built-in transcript cleanup rules. |
| `src/daemon/inject.rs` | Clipboard transaction, X11/XTest paste chord, direct insertion, and clipboard restore. |
| `src/data_log.rs` | JSONL/TSV transcription logging. |
| `src/audio_file.rs` | WAV decoding, mono mixing, and file resampling for quality tools and PTT simulation. |
| `tools/transcribe-file.rs` | File-based smoke and quality checks. |
| `scripts/transcribe_nemo_parakeet.py` | NeMo reference transcription helper. |

## Failure Policy

Startup failures stop the process when the model, microphone, or hotkey backend cannot be opened.

Runtime failures are reported and the daemon continues when possible: sound cues, log writes, individual transcriptions, and text insertion failures.
