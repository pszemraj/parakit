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
optional sound thread        opens rodio output only while playing cue tones
IPC listener thread          accepts local control connections
IPC client threads           handle status, stop, copy-last, history, and test-paste
Windows clipboard thread     observes clipboard-history acknowledgement when available
```

## State Machine

```text
Idle
  PTT down
Recording
  PTT up
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
- Runtime focus and insertion behavior is in [running.md#insertion](running.md#insertion);
  backend setup is in [linux-desktop.md](linux-desktop.md),
  [macos-desktop.md](macos-desktop.md), and
  [the Windows guide](../scripts/windows/README.md).
- Normal dictation hotkey backends must suppress the literal Space key before it reaches the focused application. The passive `x11-listen` backend is for debugging and does not suppress keys.

Cross-thread communication uses atomics, mutex-protected buffers, and crossbeam channels.

## Module Map

| Path | Responsibility |
| --- | --- |
| `src/{main,cli,app}.rs` | Binary entrypoint, CLI definitions and precedence merge, daemon setup, and batch PTT simulation helper. |
| `src/config.rs` | `config.toml` path resolution, parsing, validation, template, and config subcommands. |
| `src/daemon/desktop/hotkey.rs`, `src/daemon/desktop/hotkey/macos.rs` | Hotkey backends and hotkey state helpers. |
| `src/daemon/hotkey_help.rs` | Shared user-facing hotkey remediation text. |
| `src/daemon/recording.rs` | Hotkey transition coordinator, focus snapshot, audio start/stop, and PCM handoff. |
| `src/daemon/audio/capture.rs` | Microphone selection, live stream ownership, ring-buffer drain, pre-roll, resampling, and restart. |
| `src/daemon/audio/pactl.rs` | Linux `pactl` parsing for startup/reopen microphone display details. |
| `src/daemon/worker.rs` | ASR worker, paste sanitizer, focus guard, clipboard fallback, and insertion circuit breaker. |
| `src/daemon/ipc.rs` | Local control socket for `status`, `stop`, `copy-last`, `history`, and `test-paste`; serves an in-memory transcript ring buffer. |
| `src/daemon/desktop/windows_{clipboard_history,focus,input,paste_smoke,security}.rs` | Windows clipboard-history acknowledgement, foreground checks, `SendInput` helpers, deep paste smoke test, and privilege diagnostics. |
| `src/daemon/{preflight,audio/alsa,desktop/session,desktop/x11}.rs`, `src/daemon/macos.rs`, `src/daemon/macos/{permissions,focus,insertion_cgevent,pasteboard,diagnostics}.rs` | Startup checks, macOS TCC/focus/insertion/acknowledgement helpers, the deep paste-transaction smoke test, ALSA stderr suppression, session events, and X11 helpers. |
| `src/daemon/{logging,notifications,sounds}.rs` | Runtime logging, desktop notifications, and generated audio cues. |
| `src/fetch.rs` | Hosted [Q8_0 GGUF](https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf) download, source rebuilds, checksum verification. |
| `src/model.rs` | Model names, hosted GGUF naming, cache paths, hosted URLs, and checksum constants. |
| `src/gguf.rs` | Minimal GGUF dtype reader for startup reporting. |
| `src/{build_info,gpu,warmup,ffi_util}.rs` | Build diagnostics, bundled ggml device listing, synthetic warmup PCM, and local FFI helpers. |
| `src/inference.rs`, `src/crispasr_ext.rs` | [CrispASR](https://github.com/CrispStrobe/CrispASR) session ownership wrapper and short-audio padding. |
| `src/rules/` | Transcript cleanup pipeline: profiles, the built-in rule table, procedural passes, and user rules from `config.toml`. |
| `src/daemon/desktop/{inject,clipboard_restore}.rs` | Clipboard transaction, X11/XTest paste chord, direct insertion, and restore timing. |
| `src/data_log.rs` | JSONL transcription and insertion-outcome logging. |
| `src/audio_file.rs` | WAV decoding, mono mixing, and file resampling for quality tools and PTT simulation. |
| `examples/transcribe_file.rs` | Raw file-based inference smoke and quality checks. |
| `scripts/transcribe_nemo_parakeet.py` | NeMo reference transcription helper. |

## Failure Policy

Any failed required startup or preflight step stops the process. This includes
config and cleaner validation, session and singleton checks, insertion and
control-socket setup, and opening the model, microphone, or hotkey backend.

Runtime failures are reported and the daemon continues when possible: sound cues, log writes, individual transcriptions, and text insertion failures.
