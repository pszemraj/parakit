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
- Runtime focus and insertion behavior is in [running.md#insertion](../running.md#insertion);
  backend setup is in [linux-desktop.md](../linux-desktop.md),
  [macos-desktop.md](../macos-desktop.md), and
  [the Windows guide](../../scripts/windows/README.md).
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
| `src/fetch/` | Hosted [Q8_0 GGUF](https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf) download, Hugging Face repo and direct-URL sources, source rebuilds, checksum verification, and the acquisition manifest. |
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

## Intentional Duplication

A repo-wide deduplication audit (2026-07, the `feat/ux` PR) consolidated every
duplicate with a clean extraction path. The look-alikes below were evaluated
and deliberately left separate. Do not merge them without revisiting the
reasoning here and at the cited sites.

- `daemon/macos/diagnostics.rs` `DoctorClipboardSnapshot` vs
  `daemon/desktop/inject.rs` `ClipboardSnapshot`: `doctor --deep` keeps an
  independent capture/restore oracle so it verifies the production clipboard
  path instead of trusting it. Sharing the implementation would let a bug
  pass its own verification.
- `daemon/macos/pasteboard.rs` `BoundedNormalizedValue` vs
  `daemon/macos/focus.rs` `cfstring_to_bounded_value`: same bounded
  head-plus-tail shape, different unit systems (normalized chars with
  whitespace stripping for layout-independent matching vs raw UTF-16 units at
  the FFI boundary). Cross-referenced in comments at both sites.
- `daemon/stderr.rs` unix vs windows suppressors: a pipe plus drain thread vs
  a `NUL` redirect. Only the guard structure is similar, not the mechanism.
- `daemon/ipc.rs` unix vs windows `handle_client`: the ~15 glue lines differ
  in three real ways (stream timeouts, warning wording, the
  `schedule_exit_after_response` argument); the business logic is already
  shared via `client_command_outcome`. A transport trait was evaluated and
  rejected as net-negative.
- `src/test_support.rs` vs `tests/common/mod.rs` `fixture_root`: unit-test vs
  integration-test crate boundary; they cannot share code and differ by
  design (eager creation and nanosecond suffix vs caller-driven layout).
- `rules/passes.rs` and `rules/defaults.rs` A/I-ambiguity exclusion: one
  invariant encoded twice over non-overlapping input shapes; cross-referenced
  in comments at both sites.
- `rules/engine.rs` `RuleKind::engine` vs `CompiledTransform::engine`: a
  three-arm match on a definition type vs a compiled type; a shared trait
  costs more than the duplication.
- `fetch::FetchSource::HubRepo` stores a joined `owner/repo` string that
  `fetch/hub.rs` re-splits: the joined form is the display/manifest format,
  and the defensive re-split guards public construction of the enum.
- `gguf.rs` `COMMON_DTYPE_NAMES` is shared across the `general.file_type`
  and tensor-type code spaces on purpose; verified against the vendored
  `ggml/include/ggml.h` and commented at the site.
- X11 `X11KeySink` vs Windows `InputSender` test seams: the same injectable
  design pattern over incompatible OS APIs; only one side compiles per
  target.
- The three `start_recording` rejection tests in
  `daemon/audio/capture_tests.rs`: a table-driven merge was attempted and
  reverted — each case needs different live-channel scaffolding to control
  drop timing, and the table cost more than the shared assertion lines.
- The `effective_*` precedence tests in `src/cli.rs`: a single
  cli-over-config-over-default mega-table spanning every getter was
  evaluated and rejected — the getters return unrelated types, so a shared
  table needs an expectation enum plus per-type accessors that cost more
  than the per-getter tests they would replace.
- The `classify_source`/`source_from_cli` tests in `src/fetch/hub.rs`, the
  fetch-parse tests in `src/cli.rs`, and the paste-fallback test in
  `src/daemon/worker.rs`: case-table conversions were applied and reverted —
  each table roughly doubled the lines while adding no new coverage and no
  meaningful compile-time enforcement (`SourceKind` has two variants), so
  direct asserts are shorter and equally legible. A table here must earn
  its scaffolding with an exhaustive expectation map over a real enum, new
  case-points, or five-plus rows.

Deferred follow-ups:

- TODO: validate the consolidated Windows-only helpers
  (`daemon/desktop/windows_{clipboard_history,paste_smoke,focus}.rs`,
  commit `f52e3ff`) on Windows CI; they cannot be compiled or tested from a
  macOS host.
- TODO: fold the `cfg(target_os = "linux")` test clusters (the XTest paste
  trio in `daemon/desktop/inject_tests.rs`, the backend alias tests in
  `daemon/desktop/hotkey_tests.rs`) into their adjacent case tables. They
  do not compile on a macOS host and there is no Linux CI, so a test edit
  there is currently unverifiable anywhere.
- TODO: same for the Windows-gated test modules
  (`daemon/desktop/windows_input.rs`,
  `daemon/desktop/windows_clipboard_history.rs`), for the same reason.
