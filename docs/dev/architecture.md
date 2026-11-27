# Architecture

The daemon runs on dedicated threads.

## Thread Model

```text
main thread                  CLI, setup, and blocking hotkey backend
recording coordinator thread hotkey transition -> focus snapshot -> audio start/stop -> PCM handoff
audio manager thread         owns the current cpal::Stream and follows the default mic
microphone probe thread      queries the selected input without blocking recording controls
cpal callback thread         mixes input to mono and pushes frames into a bounded SPSC ring
audio drain thread           drains ring -> resamples -> updates pre-roll and active recording
worker thread                owns optional Engine, reload/offload, transcribe -> clean -> insert
optional sound thread        opens rodio output only while playing cue tones
notification thread          delivers desktop notifications from a bounded queue
IPC listener thread          accepts local control connections
IPC client threads           handle status, stop, copy-last, history, and test-paste
Linux IBus client thread     watches the helper and sends guarded native commits through GDBus
Windows clipboard thread     observes clipboard-history acknowledgement when available
```

On Linux, IBus manages `parakit-ibus` separately from the daemon. Its GLib event loop owns the factory, `IBusEngineSimple` instances, target generations, and commit callbacks. The executable links only the input-method bridge and its system libraries.

## State Machine

```text
Idle
  PTT down
Recording
  PTT up
Transcribing
  native submission, paste, optional clipboard fallback, block, or skip
Idle
```

The worker skips empty and effectively silent captures before inference. The `Engine` right-pads short audio and uses ordinary transcription below one second, including calls that request Qwen context.

The coordinator carries the press-time dictation mode to the worker in the stopped-PCM event. Before contextual Qwen inference, the worker selects eligible transcripts from in-memory history in chronological order. `Engine` applies them for one call and clears the context after success or failure. [Qwen shortcuts and settings](../models/qwen3-asr.md#prior-dictation-context)

Model residency has a separate `loaded -> offloaded -> loading -> loaded` lifecycle. Startup remains eager; only the worker destroys or reopens the session. A shared activity gate admits recording before audio starts, and its lease follows PCM through queued transcription and insertion. IPC insertion takes the same gate before waiting for the insertion mutex. Expiry removes the session from the worker slot under the gate, then destroys it after unlocking. A new PTT can capture during destruction; the worker finishes teardown before reloading. Loading also leaves the gate unlocked. The last lease release restarts the idle deadline, including failure and silence paths. Status/history reads take no lease and never load the model.

The reload recipe retains the model selection, thread count, device policy, and Qwen options. Failures leave the slot offloaded with an error. Without `--verbose`, startup discards process-wide stderr during model opening. Reload filters only Parakeet's two informational loader lines, leaving native errors and concurrent diagnostics visible. [Idle offload and retry](../running.md#idle-model-offload)

IPC stop wakes the worker and waits for its engine to be destroyed before running native process teardown. The lifetime guard is registered before IPC startup and transferred with the engine. Stop shares a bounded budget with insertion completion, without holding the insertion mutex while waiting for the worker. If the budget expires, immediate process termination avoids racing C++ static destructors with live native buffers. A failed hotkey backend returns to the daemon owner, which uses the same handshake before exiting with status 2.

Live capture drains callback audio through a bounded single-producer/single-consumer ring buffer. Recording uses a session epoch so stale drained samples from a stopped utterance cannot append into the next utterance. Platform stream and pre-roll policies are in [microphone behavior](../running.md#microphone).

## Ownership Constraints

- `cpal::Stream` is not reliably `Send`, so the live stream stays on the audio manager thread.
- `rodio::OutputStream` is not reliably `Send`, so cue playback lives on its own thread and opens output only for the duration of a cue.
- Parakit's `OwnedSession` is `Send` but not `Sync`, so the worker owns `Engine` directly. Do not wrap it in `Arc<Engine>`.
- Hotkey backends emit only logical press/release transitions. They do not call audio, ASR, clipboard, or insertion code.
- Normal dictation hotkey backends suppress literal Space. The passive `x11-listen` backend observes it for debugging. [Linux hotkey backends](../linux-desktop.md)

Startup and WAV simulation share resolved settings through `WorkerSettings`. `WorkerCtx` supplies the resources specific to each mode. Simulation uses a silent notifier, disabled sounds and insertion, and a private in-memory history. Startup exposes IPC before cleaner and model initialization. The banner reads the same `DaemonInfo` as IPC status. Optional `parakit_version` and `cleaner_version` fields on the IPC status response allow a new client to read older daemon responses. [Worker simulation](quality.md#ptt-worker-simulation)

The daemon and `transcribe-file` call `initialize_crispasr_runtime_defaults` before creating threads or opening a model. Library callers can initialize at the same point or set the equivalent environment variables before native code starts. [Native inference controls](../configuration.md#native-inference-controls)

The Linux daemon attaches to the helper over IBus's private bus and advertises recording readiness. Input callbacks emit directed PTT signals without waiting for daemon work. At the actual PTT press, the engine captures a token containing helper/attachment lifetime, engine identity, and focus generation. The coordinator carries it unchanged through capture and inference. Validation and `commit_text` run together on the helper's owning loop. See [focus changes](../running.md#focus-changes) for delivery policy.

## Cleaner Rules

Add a built-in at the intended position in [`DEFAULT_RULES`](../../src/rules/defaults.rs). Use `regex_rule!` for ordinary substitutions, `fancy_rule!` for bounded advanced matching such as backreferences, and `procedural_rule!` for token or syntax rules. A backtrack-limit failure retains the raw transcript and records a cleaning failure. [Cleaning metadata](../logging.md#transcription-record-fields)

Give each rule a stable name, activation, regression coverage, and per-rule hit attribution. Use `Activation::Safe` for high-confidence mechanical edits and `Activation::Aggressive` for edits that can change meaning. Keep the five `RuleKind::Numbers` entries adjacent so the engine can process enabled numeric categories in one scan. Increment `CLEANER_VERSION` when behavior, identity, or order changes. Check downstream matches with a corpus replay before moving a pass. [Rule order](../cleaning-rules.md#rule-order) and [corpus replay](quality.md#cleaning-corpus-replay)

`build_cleaner`, `build_enabled_cleaner`, and `validate_configured_rules` share input validation. Pass an empty `repetition_exceptions` slice to use only built-in exceptions. Configured whole-word replacements compile into named rules that match the same final text and retain separate hit counts. [Replacement configuration](../cleaning-rules.md#word-replacements)

## Module Map

| Path | Responsibility |
| --- | --- |
| `src/{main,cli,app}.rs` | Binary entrypoint, CLI definitions and precedence merge, command dispatch, daemon setup, and batch PTT simulation helper. |
| `src/output.rs` | Command output on stdout that ends quietly when its reader closes the pipe. |
| `src/config.rs` | `config.toml` path resolution, parsing, validation, and template. |
| `src/daemon/desktop/hotkey.rs`, `src/daemon/desktop/hotkey/{linux,macos}.rs` | Hotkey backends and hotkey state helpers. |
| `src/daemon/hotkey_help.rs` | Shared user-facing hotkey remediation text. |
| `src/daemon/recording.rs` | Hotkey transition coordinator, focus snapshot, audio start/stop, and PCM handoff. |
| `src/daemon/audio/capture.rs`, `src/daemon/audio/capture_{device,drain,monitor,stream}.rs` | Capture handle and pre-roll, microphone selection, ring-buffer drain and resampling, background device probes and change confirmation, and live stream ownership and restart. |
| `src/daemon/audio/pactl.rs` | Linux `pactl` queries for default-source identity polling and startup/reopen microphone display details. |
| `src/daemon/worker.rs` | Worker events, model lifecycle, ASR, cleanup, and completion. |
| `src/daemon/worker/insertion.rs` | Paste sanitizer, focus guard, clipboard fallback, and insertion reporting. |
| `src/daemon/{model_lifecycle,engine_runtime}.rs` | Activity admission, session residency, and the shared open/warmup recipe. |
| `src/daemon/worker_shutdown.rs` | Worker lifetime and bounded IPC shutdown handshake. |
| `src/app/simulation.rs` | Acknowledged WAV dictations and configured idle/reload validation. |
| `src/app/config_command.rs` | `parakit config` subcommands: show, path, init, and edit. |
| `src/app/cache_command.rs` | `parakit cache` listing and directory commands. |
| `src/ibus_command.rs`, `src/ibus_main.rs` | Linux IBus registration, removal, read-only setup status, and the standalone engine executable. |
| `src/daemon/desktop/ibus/` | Native IBus engine/factory, daemon GDBus client, target lifetimes, and submission evidence. |
| `src/daemon/desktop/linux_insertion.rs` | Linux insertion-backend selection and recording target capture independent of clipboard paste mode. |
| `src/daemon/ipc.rs`, `src/daemon/ipc/{client,unix_socket,windows_pipe}.rs` | Local IPC for `status`, `stop`, `copy-last`, `history`, and `test-paste`: shared protocol and in-memory transcript ring buffer, the control client, and the Unix-socket and Windows named-pipe transports. |
| `src/daemon/ipc/{history,history_picker}.rs` | Prior-context selection, full-text lookup/copy by daemon-session and entry reference, and the terminal picker. |
| `src/daemon/desktop/windows_{clipboard_history,focus,input,paste_smoke,security}.rs` | Windows clipboard-history acknowledgement, foreground checks, `SendInput` helpers, deep paste smoke test, and privilege diagnostics. |
| `src/daemon/{preflight,preflight_ibus,audio/alsa,desktop/session,desktop/wsl,desktop/x11,desktop/inject_smoke}.rs`, `src/daemon/audio/alsa_silence.c`, `src/daemon/macos.rs`, `src/daemon/macos/{permissions,focus,insertion_cgevent,cgevent_ffi,pasteboard,diagnostics}.rs` | Startup checks, macOS TCC/focus/insertion/acknowledgement helpers and event-tap FFI, the macOS and X11 deep paste smoke tests, ALSA stderr suppression, X11/Wayland session and WSL checks, and X11 helpers. |
| `src/daemon/{logging,notifications,sounds}.rs` | Runtime logging, desktop notifications, and generated audio cues. |
| `src/daemon/{stderr,subprocess}.rs` | Native stderr suppression and the reload loader-line filter, and bounded waits for desktop helper commands. |
| `src/fetch/` | Preset Q8_0 downloads, cached Q6_K generation, Hugging Face repo and direct-URL sources, source rebuilds, and explicit `--sha256` checks. |
| `src/model.rs` | Presets, provider revisions, model selection, quantization choices, and cache paths. |
| `src/qwen.rs` | Qwen source-language normalization and generation-budget validation. |
| `src/gguf.rs` | Minimal GGUF dtype reader for startup reporting. |
| `src/{build_info,gpu,warmup,ffi_util}.rs`, `src/ggml_device_shim.cpp` | Build diagnostics, bundled ggml device listing through the C++ device shim, Qwen speech and Parakeet synthetic readiness probes, and local FFI helpers. |
| `src/inference.rs`, `src/crispasr_ext.rs` | [CrispASR](https://github.com/CrispStrobe/CrispASR) session ownership, startup defaults (explicit overrides retained), scoped Qwen context, short-audio padding, and raw-ABI CPU/GPU session opening. See the [Qwen comparison guide](quality.md#qwen-and-moondream-comparisons) for windowed audio attention. |
| `src/{lib,checksum,constants,test_support}.rs` | Library crate root, digest helpers, shared audio and inference constants, and unit-test fixtures. |
| `src/rules/{engine,defaults,passes,user}.rs` | Transcript cleanup pipeline, profiles, built-in rule table, procedural passes, and user rules from `config.toml`. |
| `src/rules/{numbers,number_guards,number_labels,number_quantities,number_versions,number_words}.rs` | Shared numeric tokenization and rendering, quantity extents and connected bounds, protected quantities, calendar years, versions, and number-word/unit vocabulary. |
| `src/rules/stutters.rs` | Adjacent repeated-word collapsing and configured repetition exceptions. |
| `src/rules/phrase_breaks.rs` | Filled pauses and function-word punctuation repair. |
| `src/daemon/desktop/{inject,focus_snapshot,clipboard_store,paste_transaction,clipboard_guard,x11_paste,direct,clipboard_restore}.rs` | Insertion contract and injector, focus snapshots, clipboard backend and snapshots, the guarded paste transaction and change detection, X11/XTest paste chord, guarded direct insertion, and restore timing. |
| `src/data_log.rs` | JSONL transcription and insertion-outcome logging. |
| `src/audio_file.rs` | WAV decoding and mono mixing; shared Rubato target-rate resampler used by file input and live capture on every platform. See [microphone behavior](../running.md#microphone). |
| `examples/transcribe_file.rs` | Raw file-based inference smoke and quality checks. |
| `examples/audit_cleaning.rs` | Built-in cleaner replay over historical JSONL logs. |
| `scripts/transcribe_nemo_parakeet.py` | NeMo reference transcription helper. |
| `build.rs`, `build/{native_link,static_link,openblas_roots,unix_openblas,windows_openblas}.rs` | CrispASR CMake orchestration, accelerator selection, static archive linkage and external-library runtime paths, C++ runtime linkage and device-shim compilation, and cross-platform BLAS discovery. |
| `build/ibus.rs` | Linux IBus C bridge compilation and target-specific system linkage. |
| `build/native_correctness.{rs,cmake}`, `build/native/qwen_correctness.hpp` | [Bundled native corrections](../build.md#crispasr-and-backends) compiled from generated source copies. |
| `build/windows_{artifacts,cuda,manifest}.rs`, `scripts/windows/{build,common,toolchains,install}.ps1` | Windows toolchain discovery, runtime-manifest generation, bundle assembly, and per-user installation. |

## Failure Policy

Any failed required startup or preflight step stops the process. This includes config and cleaner validation, session and singleton checks, insertion and local IPC setup, and opening the model, microphone, or hotkey backend.

Runtime failures are reported and the daemon continues when possible: sound cues, log writes, individual transcriptions, and text insertion failures.

Native delivery and uncertainty are described under [insertion outcomes](../logging.md#insertion-outcomes).

`ClipboardSnapshot` in [`clipboard_store.rs`](../../src/daemon/desktop/clipboard_store.rs) owns capture, restoration, and exact payload comparison. Production restoration consumes the saved value and checks ownership through `StagedClipboard`. Diagnostic restoration borrows the snapshot for cleanup retries. [macOS deep diagnostics](../macos-desktop.md#doctor---deep)

`PasteReport` describes transaction results and acknowledgement telemetry. `InsertReport` adds worker policy, direct-typing progress, and degraded-outcome diagnostics. Staging uses one `From<StageOutcome>` conversion into paste telemetry; the worker retains separate notifications, history, and alert decisions. The serialized fields and outcome meanings are in [logging](../logging.md#insertion-record-fields).
