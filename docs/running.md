# Running parakit

parakit runs in the foreground by default. Use that mode once after install, then run it quietly in the background for daily use.

## First Run

```text
parakit doctor && parakit
```

`parakit doctor` checks hotkey access, the selected microphone, insertion support, and the daemon singleton lock without downloading or loading the model. It exits `0` when startup should proceed and `1` when a blocking issue remains. An already-running daemon makes readiness fail. Starting again is harmless: when the running daemon answers, `parakit start` prints `parakit: already running` and exits successfully.

Useful variants:

```text
parakit --verbose doctor
parakit --quiet doctor
parakit doctor --deep
```

`--verbose` and `--quiet` are global flags: they work whether they come before or after `doctor`. Follow the desktop setup for [Linux](linux-desktop.md), [macOS](macos-desktop.md), or [Windows](windows-desktop.md).

The daemon checks the hotkey backend, insertion backend, and singleton lock before any model download. If those preflights pass, it starts the control endpoint, opens the microphone, warns when the selected source looks like Bluetooth, downloads the default Q8_0 GGUF if it is not already cached, opens the model, and starts the hotkey loop. `status`, `stop`, and a second `start` remain available while the model downloads or loads.

Normal startup:

```text
parakit: loading model...
parakit
  model: parakeet-tdt-0.6b-v3-Q8_0.gguf
  dtype: Q8_0 (745 MB)
  mic:   USB Speech Mic Mono
Ready: hold Ctrl+Space to dictate. Ctrl+C to stop.
```

On macOS the ready line says `Left Control+Space`; on Linux and Windows it says `Ctrl+Space`. Loading and conversion show an activity indicator in a terminal. Normal dictation shows the raw and cleaned transcripts when cleaning changes the text. Timings, audio formats, and backend details require `--verbose`.

Use `--verbose` only when debugging startup, backend selection, or latency:

```bash
parakit --verbose
parakit start --threads 8 --verbose
```

## Device Selection

GPU-capable builds default to `--device auto`. In `auto`, CrispASR asks ggml for the best backend device and falls back to CPU when no GPU backend is available. On hybrid laptops, ggml prefers a discrete GPU over an integrated GPU.

Windows bundle backend selection is covered in [../scripts/windows/README.md#backend-requirements](../scripts/windows/README.md#backend-requirements). At runtime, `--device` controls CPU/GPU use inside the installed build.

```bash
parakit start --device auto
parakit start --device cpu
parakit start --device gpu
```

`--device cpu` opens the session with GPU use disabled. `--device gpu` requests the GPU path and, on bundled builds, fails before model load if ggml reports no discrete or integrated GPU device. On system-library builds without the bundled ggml probe, `--device gpu` continues with a warning because parakit cannot verify device visibility before opening the session.

Per-device selection has no flag or config key because the Parakeet backend ignores CrispASR's `gpu_device` field. Pin a device with the backend's environment variable, whose format is in [config_reference.toml](config_reference.toml):

```bash
CUDA_VISIBLE_DEVICES=0 parakit start --device gpu
GGML_VK_VISIBLE_DEVICES=0 parakit start --device gpu
```

Use `parakit --verbose doctor` to list the compute devices visible to bundled ggml. Verbose daemon startup prints the requested mode and expected device, such as `device=auto -> Vulkan1 - NVIDIA GeForce RTX 4070 Laptop GPU [GPU] (7012 MiB free / 8188 MiB total)`.

## Background Use

```bash
parakit start --log-dir "$HOME/.parakit/logs" --quiet & disown
```

`--quiet` suppresses normal stdout, including startup lines and transcripts. Errors, warnings, and the [Q6 preparation notice](#model-presets) still go to stderr. Use `--verbose` for native loader diagnostics, including details hidden while the model opens at startup.

On Linux, start parakit from a terminal in the current desktop session. Tmux, X11 auth, and evdev details are in [linux-desktop.md](linux-desktop.md).

On macOS, use the terminal app configured under [permissions](macos-desktop.md#permissions).

Keep stderr in a file:

```bash
mkdir -p "$HOME/.local/state/parakit"
nohup parakit --quiet >/dev/null 2>>"$HOME/.local/state/parakit/parakit.err" &
```

Stop it:

```bash
parakit stop
```

`parakit stop` uses the local daemon control channel, gives in-flight worker and insertion cleanup up to five seconds, and confirms that the process singleton lock was released before printing `stopped`. The client allows about six seconds for that final confirmation, including when the endpoint has already disappeared during shutdown. If startup holds the lock before exposing control, the client first allows a separate bounded discovery window, then gives an acknowledged stop its full cleanup budget. If a timeout persists, identify the daemon with `pgrep -af parakit` on Unix-like systems or `Get-Process parakit` on Windows, then end that specific process.

## Daemon Control

When the daemon is running, these commands talk to it through local per-user IPC. Unix-like systems use a Unix socket under the parakit runtime directory; Windows uses a named pipe. Restart the daemon after upgrading parakit in place (`parakit stop`, then start it again).

```text
parakit status
parakit stop
parakit copy-last [N]
parakit history
parakit history --plain
parakit history --text N
parakit test-paste "hello from parakit"
```

When the control endpoint is absent and the singleton lock is free, `status` and `stop` print `parakit: not running` or `parakit: not running; nothing to stop` and exit successfully. If the endpoint is absent while the lock remains held, `status` reports that the daemon may still be starting or stopping; `stop` waits within its remaining deadline, sending the request if startup exposes the endpoint and confirming that the lock releases. Commands that need daemon state (`history`, `copy-last`, and `test-paste`) instead report that the daemon is not running and point to `parakit start`.

The daemon keeps a ring buffer of recent transcripts in memory (`daemon.transcript_history` entries, 10 by default). `copy-last` acts on the most recent one by default; pass `N` (1-based, counting back from the most recent) to reach further back, e.g. `parakit copy-last 3` for the third-most-recent transcript. The ring disappears when the daemon stops, so treat it as a same-session convenience.

In a terminal, `parakit history` opens a picker with the newest transcript selected. Up/Down selects an entry and shows its full text; PageUp/PageDown scrolls long previews. Enter copies the selected entry and closes the picker after success. Escape or Ctrl+C closes it without copying. A copy failure stays visible in the picker. New dictations cannot silently change which entry is selected; an entry evicted from the ring or lost in a daemon restart reports that it is no longer available.

Piped or redirected `history` output stays a plain newest-first list. Use `--plain` to request that list in a terminal, `--limit N` to limit the entries shown, or `--text N` to print one full transcript without clipboard access. `--quiet` never opens the picker. Full-text previews and output respect the control protocol's 64 KiB response limit; oversized text reports an error without truncation, but Enter can still copy the selected picker entry.

Explicit copying uses the existing platform clipboard backend. On GNOME Wayland, it can use XWayland clipboard access even though IBus dictation itself needs no X11 connection. Without that clipboard access, full-text previews and `history --text N` remain available within the response limit.

`test-paste` runs the configured insertion path, including content policy and target checks, without recording microphone audio. The native IBus route submits a commit; the legacy clipboard route stages text and sends its paste chord.

Plain `parakit status` output is unchanged and safe for scripts to parse:

```text
$ parakit status
parakit: idle
last transcript: 42 bytes
```

Pass the global `--verbose` flag (before or after the subcommand) to also print a detail block (mic, model, device, uptime, dictation count, and more):

```text
$ parakit --verbose status
parakit: idle
last transcript: 42 bytes
  pid:        41213
  uptime:     1h 23m
  dictations: 17
  mic:        USB Speech Mic Mono, 48000 Hz mono input -> 16000 Hz mono model, F32
  model:      parakeet-tdt-0.6b-v3-Q8_0.gguf (Q8_0 (745 MB))
  residency:  loaded
  model idle: 10 minutes
  device:     cpu (parakeet, 8 threads)
  paste mode: standard
  sounds:     on
  cleaning:   on (safe, 29 rules)
  logging:    JSONL to /home/user/.parakit/logs
  history:    3 of 10
  hotkey:     auto
```

The detail block reflects the daemon's own state at query time, not the querying process's flags. The `hotkey` line is Linux-only. `daemon.verbose = true` does not expand `parakit status`; pass the querying command's global `--verbose` flag as shown. Before startup details are available, it prints `detail unavailable (daemon starting)` after the two lines above.

## Idle Model Offload

The model loads and warms at startup, then releases its inference session after ten uninterrupted idle minutes. Set `[daemon] model_idle_minutes = N` in the config, or run `parakit start --model-idle-minutes N`. The flag overrides the config. Nonnegative whole minutes are required; `0` keeps the model resident.

The idle interval uses monotonic elapsed time, not calendar time. Whether system sleep counts toward it depends on the OS and Rust's [clock implementation](https://doc.rust-lang.org/std/time/struct.Instant.html). On clocks that pause during suspend, the remaining idle interval continues after wake; sleeping for ten minutes does not itself make the model eligible for offload.

Recording, queued dictations, transcription, and insertion prevent offloading. The interval restarts when all work finishes, including silent captures and failures. Hotkeys, microphone policy, IPC, and transcript history remain available. Reading status or browsing history does not postpone offloading; copying a transcript counts as insertion activity.

Press PTT normally after an idle period. A three-note cue announces model reloading, followed by the normal listening tone once the model is ready if PTT is still held. Audio records throughout reload, so early speech is preserved. An early release queues that audio until the model is ready and suppresses the delayed listening tone. Successful dictation retains its normal completion cue. Reload uses the resolved local file without downloading it again, preserving the thread count and [device policy](#device-selection). If reopening at PTT start fails, parakit plays its two-pulse error cue and retries once when PTT is released so a temporary failure can still preserve the dictation. If that retry also fails, it plays the error cue again and reports the dictation discarded without insertion; the next PTT retries automatically. `--no-sounds` disables these cues.

Other programs can fill the GPU while the model is offloaded. Before a bundled build reloads on a GPU, parakit checks free memory for the model and its working space. Qwen on Metal requires twice the model file size plus 2 GiB, accounting for its main-model and audio-tower mappings. Other Qwen GPU backends require one model file size plus 2 GiB; other models require one file size plus 1 GiB. If there is insufficient room, `auto` reloads on CPU until the next offload and says so in a warning and a desktop notification; `--device gpu` fails the reload with the free and needed amounts.

This check does not reserve memory or bound the workspace for long audio. Later allocations can still exhaust memory and abort the daemon; `model_idle_minutes = 0` avoids repeated offload/reload but does not prevent out-of-memory failures. The [historical pressure measurements](dev/memory.md#reload-under-gpu-memory-pressure) explain the original Parakeet allowance and its limits; they are not current-runtime memory ceilings.

Qwen's workspace allowance follows [Linux CUDA/Vulkan measurements](dev/memory.md#qwen-gpu-reload-measurements); its Metal mapping requirement follows [native Metal measurements](dev/memory.md#metal-qwen-reload-accounting). Tested inputs include the Juniper memo, chunk boundaries, and 270 seconds of repeated speech at token budgets 512 and 4096. Margins depend on the artifact; other hardware and native Windows behavior remain uncalibrated. A model's download size is not its runtime memory requirement.

Parakeet startup warms one second of synthetic audio on CPU, or five seconds on GPU; reload uses one second. Qwen startup and reload instead transcribe an embedded 11-second human speech sample with automatic language and a fixed 512-token budget. Readiness text is discarded and the user's language and token budget are restored before dictation. A deliberately tiny user budget can therefore fail the actual transcription without preventing readiness. Longer dictations allocate larger buffers as needed; the first use of a new shape can also incur backend compilation work. [Linux reload measurements](dev/memory.md#linux-reload-measurements) show host and GPU memory for the earlier synthetic policy.

`parakit --verbose status` reports `loaded`, `loading`, or `offloaded`, the effective timeout, and the last reload error until a successful reload clears it. Residency is separate from the recording/transcribing phase. Queries never load the model. During startup, residency can be unavailable until the worker publishes its initial state.

Session destruction releases owned CPU/GPU allocations, but process and driver caches can remain. See the [memory measurements and allocation notes](dev/memory.md); GPU inference does not imply zero host memory.

## Model Cache

Without a CLI or configured model selection, parakit uses the hosted [Parakeet v3 Q8_0 GGUF model](https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf). `PARAKIT_MODELS_DIR` overrides the model directory. Without that override, `XDG_CACHE_HOME` is honored on Linux and macOS, both fall back to `~/.cache/parakit/models/`, and Windows uses `%LOCALAPPDATA%\parakit\Cache\models\`.

Useful commands:

```bash
parakit fetch --force
parakit cache
parakit cache list
parakit cache dir
parakit start -m /path/to/model.gguf
```

`-m <path>` disables automatic fetch. It conflicts with `--model-preset` in the same command. Either CLI selection overrides both configured model selectors; `daemon.model` and `daemon.model_preset` conflict with each other even when a CLI selection is supplied.

For locked-down or offline machines, seed the default model by placing `parakeet-tdt-0.6b-v3-Q8_0.gguf` in the directory printed by `parakit cache dir`. Use `PARAKIT_MODELS_DIR` when the approved model location is managed by IT or shared across a build image.

### Model Presets

Choose a hosted Q8_0 model explicitly. Qwen presets are experimental; Redux is retained for testing and comparison only:

```bash
parakit fetch --model-preset qwen3-asr-1.7b
parakit start --model-preset qwen3-asr-1.7b --no-cleaning
parakit start --model-preset parakeet-ultra
```

`start` handles acquisition automatically. You do not need to run `fetch` first. To generate Q6_K on first use and reuse it on later starts:

```bash
parakit start --model-preset parakeet-ultra --quantize q6_k \
  --log-dir "$HOME/.parakit/logs" --quiet & disown
```

Use `parakeet-v2` for English-only v2, `parakeet-v3` for the original default, or either Qwen name in the table below. Without `--model-preset`, `--quantize q6_k` prepares v3. Preparing a missing Q6_K prints a notice to stderr even with `--quiet`, since a larger download and local quantization may be needed. It reuses an explicitly fetched F16 when available; otherwise it downloads a private F16 intermediate. That private file is removed after success and kept after failure for retry. Explicitly fetched F16 files are preserved, including during `--force` rebuilds, which download a fresh private intermediate. Wait for readiness before dictating. Cached Q6_K starts work offline and idle reload reuses the resolved file.

Interactive downloads show a progress bar with bytes downloaded, transfer speed, and ETA. If the server omits the file size, a byte counter is shown instead. Progress bars are hidden with `--quiet` or when stderr is redirected. Quantization is a separate stage announced after the download. URLs and native converter output require `--verbose`; failed tools report a short diagnostic. An explicit `fetch` confirms the cached model and prints its startup command, while daemon startup continues directly to loading.

To keep the same startup command each time, set these fields in the file printed by `parakit config path`:

```toml
[daemon]
model_preset = "parakeet-ultra"
quantize = "q6_k"
```

Then use `parakit start --log-dir "$HOME/.parakit/logs" --quiet & disown`. Install a build containing these options first, and restart an existing daemon with `parakit stop` before starting it with the new settings. Another `start` does not change a running daemon; explicit model/precision flags print a restart notice in that case. `--quantize q8_0` overrides a configured Q6_K choice, and `-m` loads a custom file at its existing precision. The WAV helper accepts the same `--quantize` flag with preset selection.

| Preset | Hosted repository | Exact artifact |
| --- | --- | --- |
| `parakeet-v2` (English-only) | [cstr/parakeet-tdt-0.6b-v2-GGUF](https://huggingface.co/cstr/parakeet-tdt-0.6b-v2-GGUF) | `parakeet-tdt-0.6b-v2-q8_0.gguf` |
| `parakeet-v3` | [pszemraj/parakeet-tdt-0.6b-v3-gguf](https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf) | `parakeet-tdt-0.6b-v3-Q8_0.gguf` |
| `qwen3-asr-0.6b` | [cstr/qwen3-asr-0.6b-GGUF](https://huggingface.co/cstr/qwen3-asr-0.6b-GGUF) | `qwen3-asr-0.6b-q8_0.gguf` |
| `qwen3-asr-1.7b` | [cstr/qwen3-asr-1.7b-GGUF](https://huggingface.co/cstr/qwen3-asr-1.7b-GGUF) | `qwen3-asr-1.7b-q8_0.gguf` |
| `parakeet-ultra` | [cstr/parakeet-ultra-GGUF](https://huggingface.co/cstr/parakeet-ultra-GGUF) | `parakeet-ultra-q8_0.gguf` |
| `parakeet-redux` | [cstr/parakeet-redux-GGUF](https://huggingface.co/cstr/parakeet-redux-GGUF) | `parakeet-redux-q8_0.gguf` |

**Redux is for testing and checkpoint comparisons, not normal dictation.** Its reconstructed weights use the ordinary Parakeet backend without packed ternary acceleration or Moondream's VAD head. Photon's 178 MB footprint, [published CPU results](https://huggingface.co/moondream/parakeet-redux#performance), and pause-based segmentation therefore do not describe Parakit. In our [CPU comparison](dev/quality.md#parakeet-cpu-comparison), Redux was slower than Ultra and used similar RAM.

Presets use the exact file above and check the local cache before contacting Hugging Face. Once fetched, they work offline. All presets except v3 select fixed provider commits from the model catalog; `--force` downloads that selected revision again. Parakeet v3 keeps its existing revision policy and top-level cache path; the other presets use the revision-specific Hub cache layout described below. Previously downloaded files remain usable with `-m`; changing a preset revision selects a separate cache destination. `fetch --model-preset` does not read daemon configuration. It accepts `--force` and conflicts with a custom source, `--file`, and `--sha256`.

Qwen, Moondream, and NVIDIA v3 checkpoints can also be [rebuilt from source](dev/README.md#source-rebuild). Source rebuilds use the preset's normal Q8_0 cache destination; v2 has no source-rebuild path.

To prepare Q6_K without starting the daemon:

```bash
parakit fetch --model-preset qwen3-asr-1.7b --quantize q6_k
parakit start -m /path/printed/by/fetch/qwen3-asr-1.7b-q6_k.gguf --device gpu
```

This accepts every preset above and uses its provider revision with `crispasr-quantize`; no Python environment or upload is needed. `--force` rebuilds Q6_K. Qwen retains a Q8_0-or-higher audio encoder, while Parakeet quantizes eligible encoder and decoder matrices to Q6_K; unsupported tensors keep higher precision. Validate recognition on your own recordings before relying on a new quantization.

`fetch --quantize q6_k` conflicts with `--from-source`. GGUF Q6_K differs from [MLX's affine 6-bit quantization](https://ml-explore.github.io/mlx/build/html/python/_autosummary/mlx.core.quantize.html), so evaluate them separately. Windows requires a compatible external `crispasr-quantize.exe` on `PATH`; see [source rebuild](dev/README.md#source-rebuild).

Qwen defaults to automatic language identification and a maximum of 512 generated tokens per native chunk. Use `--qwen-language en` (or `English`) to force a supported language, and `--qwen-max-new-tokens N` to set a budget from 1 to 4096. An incomplete decode fails the dictation rather than returning partial text. Qwen splits long audio into native chunks of at most 30 seconds, resetting decoder context for each chunk. Windowed audio attention is enabled by default; set `CRISP_AUDIO_WINDOWED_ATTN` in the launching environment to override it for native comparisons. The [Qwen validation procedure](dev/quality.md#qwen-and-moondream-comparisons) covers the comparison. The published Qwen ASR input capability of 1,200 seconds does not change Parakit's 270-second capture limit or imply continuous linguistic context across these cuts. Explicit Qwen flags require a Qwen model; persistent `[qwen]` settings are inactive for Parakeet. The [configuration reference](config_reference.toml) lists all supported language codes and names, plus the environment override.

Cleaning remains enabled by default and includes English rules. For multilingual dictation, use `--no-cleaning`, or set `[cleaning] enabled = false`; both bypass built-in and user cleaning rules.

The [Qwen team](https://huggingface.co/Qwen/Qwen3-ASR-0.6B) publishes Qwen3-ASR under [Apache 2.0](https://www.apache.org/licenses/LICENSE-2.0). [Moondream Ultra](https://huggingface.co/moondream/parakeet-ultra) and [Redux](https://huggingface.co/moondream/parakeet-redux) derive from NVIDIA Parakeet v3 and use [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/); the linked cstr repositories provide the GGUF conversions.

### Other GGUF Builds

`parakit fetch` can also acquire custom GGUF builds instead of a preset, for example the CrispASR author's Parakeet repo or the widely-used `handy-computer` build:

```bash
parakit fetch cstr/parakeet-tdt-0.6b-v3-GGUF
parakit fetch cstr/parakeet-tdt-0.6b-v3-GGUF --file parakeet-tdt-0.6b-v3-q4_k.gguf
parakit fetch handy-computer/parakeet-tdt-0.6b-v3-gguf@main
parakit fetch https://example.com/models/parakeet-q8.gguf --sha256 <64-hex-sha256>
```

The first positional argument is either a Hugging Face repo (`owner/repo`, optionally `owner/repo@revision`) or a direct `http://`/`https://` URL. Other URL schemes are rejected before any request. For a repo source, `--file` selects a specific `.gguf` file when the repo publishes more than one and none of them is uniquely named `q8_0`; otherwise parakit auto-selects the sole `.gguf` file, or the sole `q8_0` file among several. `--sha256` checks the completed download when you explicitly supply it; parakit does not otherwise hash models or turn an unpinned URL into a persistent checksum policy. Resumed downloads use the server's strong ETag or Last-Modified value through `If-Range`; a partial with no usable validator restarts instead of being appended blindly. `--force` discards any partial and replaces the cached file.

Repo fetches land under `<cache dir>/hub/<owner>--<repo>--<key>/<file>`, where the key hashes the repo, revision, and file path; URL fetches land under `<cache dir>/url/<url-key>/<file>`, where the key separates exact source URLs that share a filename. Different revisions and URLs therefore cannot overwrite or resume onto one another. Both source classes are separate from the canonical top-level `parakeet-tdt-0.6b-v3-Q8_0.gguf`, and different repos remain separate even when they publish identically named files. `parakit cache list` shows each model's path, GGUF type, and size without reading every model to compute hashes. Custom fetches are never picked up automatically: activate one with `parakit start -m <path>` or by setting `daemon.model = "<path>"` in `config.toml`, exactly as with any other custom GGUF. A preset selects its matching cached artifact.

For mirrors, authentication, proxies, and TLS interception, see [corporate-network downloads](troubleshooting.md#downloads-behind-a-corporate-proxy).

## Microphone

parakit follows the OS default input device and avoids monitor/loopback/virtual sources unless no better input is available. When CPAL reports a mono stream with the same sample rate and sample format as the default stream, parakit opens the mono stream. Otherwise it opens the default stream and downmixes multi-channel input to mono before resampling and before model inference. The model input is always 16 kHz mono PCM.

Microphone capture and WAV helpers share [native resampling](dev/quality.md#qwen-and-moondream-comparisons) on every platform and model when the input is not already 16 kHz. They drain the final filter tail to the recording's nominal sample count and reset between recordings. No Python preprocessing step or additional startup option is needed.

On Linux and macOS, the microphone stream stays warm while the daemon is running; a bounded ring buffer feeds a drain thread that keeps 350 ms of pre-roll so the beginning of an utterance is less likely to be clipped. On Windows, parakit pauses the input stream while idle so `audiodg.exe` and driver-level microphone processing do not burn CPU when no recording is active. Recording start/stop is event-driven; idle device-change polling runs once per second.

One continuously held recording is force-stopped and handed to the worker after 270 seconds. This bounds a missed hotkey-release event without discarding the captured audio. It follows the usual transcription path and normal completion cue.

If the default input changes while parakit is idle, the daemon switches to it and prints the new microphone unless `--quiet` is set. Each idle poll reads CPAL's selected input; on Linux PulseAudio/PipeWire systems, when that input is the OS default, it also runs `pactl get-default-source` so a changed default source is detected behind CPAL's generic `default` device. Startup, probe, and stream reopen also read `pactl list sources` to name the source in logs and detect Bluetooth. If an active stream fails, parakit keeps running and retries.

Bluetooth microphones are allowed, but parakit prints a warning because headset profiles often add latency and reduce speech quality. The warning still goes to stderr in `--quiet` mode.

## Insertion

Parakit transcribes once on hotkey release. Linux supports [IBus and X11 insertion transports](linux-desktop.md#insertion-selection-and-recovery); other platforms use their desktop input APIs.

All modes convert CRLF and CR to LF, then drop NUL and other ASCII control characters except tab and LF. A whitespace-only result follows the [skipped outcome](logging.md#insertion-outcomes).

Paste mode controls transcript content and the shortcut used by the clipboard route:

| Mode | What it sends | When to pick it |
| --- | --- | --- |
| `terminal` | Native IBus commit on Linux when eligible; otherwise `Ctrl+Shift+V` on Linux/Windows or `Cmd+V` on macOS | The Linux default. Trailing newlines are stripped and embedded newlines are blocked. Native terminal commits also block tabs because they lack clipboard bracketed-paste semantics. |
| `standard` | Native IBus commit on Linux when eligible; otherwise `Ctrl+V` on Linux/Windows or `Cmd+V` on macOS | GUI applications and multiline text. The default on macOS and Windows. |
| `direct` | Synthetic typing through the platform keyboard API; the clipboard is never touched | App-compatibility fallback when neither chord reaches the target. Slower and less reliable for non-ASCII text. On Linux it still requires an X11 session. |

```bash
parakit start --paste-mode terminal
parakit start --paste-mode standard
parakit start --paste-mode direct
```

### Clipboard Staging And Restore

The clipboard route in `terminal` and `standard` modes stages plain text and sends the paste shortcut. Both give the target and clipboard history tools time to observe the staged text, then restore the previous clipboard contents when the clipboard API can round-trip them. Restore support covers text, HTML with a text alternative, copied file lists, and images exposed as normal platform image data. Browser-private image packages, WebP-only payloads, and other clipboard MIME formats are not restorable through `arboard`; when restore is required, parakit clears the staged transcript instead of leaving sensitive text as the active clipboard.

Successful native IBus delivery and direct typing skip this transaction. `--keep-transcript-clipboard` has no effect on them. Rejected or uncertain native delivery attempts a clipboard-only recovery copy, retaining the transcript regardless of this setting; it never sends another paste or commit.

Delivering the dictation takes priority over restoring the previous clipboard. If the previous clipboard cannot be read, or changes while parakit saves it, parakit still stages and pastes the transcript and leaves it as the active clipboard for that dictation; a clipboard history manager keeps the previous contents.

After staging, parakit checks that the staged text and native clipboard state still match. A replacement or unreadable clipboard before the final focus check causes one re-stage of the transcript, which remains active after paste. Parakit rechecks focus after that write. On macOS and Windows, a clipboard write observed during this final check cancels the paste and preserves the newer copy; X11 reports only the selection owner, so a clipboard manager taking ownership of the transcript there does not cancel it. Newer copies made after dispatch are also preserved during restoration. The transcript remains in daemon history when history is enabled. These checks cannot lock the clipboard until the receiving application consumes it.

`--keep-transcript-clipboard` leaves the transcript as the active clipboard after both successful pastes and blocked fallbacks. The default is to restore the previous supported clipboard contents after staging. On macOS and Linux, one case ignores this setting either way: if push-to-talk (or another) modifier key is still held after a bounded wait of up to two seconds, sending the chord could change the shortcut or interrupt a capture, so parakit skips it and keeps the transcript on the clipboard regardless of the configured policy. This path prints a warning, sends a "Transcript copied" notification, and plays the success tone; it is logged as `copied_only`.

Linux clipboard modes use a fixed-delay restore gate. Platform-specific timing is covered under [Windows insertion](windows-desktop.md#insertion) and [macOS acknowledgement](macos-desktop.md#insertion).

Use [daemon history](#daemon-control) for same-session recovery or [JSONL logging](logging.md) for durable transcript records. Clipboard history managers also retain staged transcripts after parakit restores the active clipboard; on Windows, enable the built-in history and open it with `Win+V`. Disable the manager if dictated text must not outlive the paste.

### Focus Changes

Parakit captures focus at PTT-down and checks it before insertion. Target identity and failure behavior depend on the platform: [Linux IBus and X11](linux-desktop.md#focus-guard), [Windows](windows-desktop.md#insertion), or [macOS](macos-desktop.md#insertion). Clipboard handling after a rejected non-direct insertion follows the [staging and restore policy](#clipboard-staging-and-restore).

### Paste Acknowledgement On macOS

See [macOS insertion evidence](macos-desktop.md#insertion), [logged outcomes](logging.md#insertion-outcomes), and [recovery after an unconfirmed paste](troubleshooting.md#macos-paste-could-not-be-confirmed).

## Logging And Sounds

Enable text-only JSONL logging with:

```bash
parakit start --log-dir "$HOME/.parakit/logs"
```

Set the same directory persistently with `logging.dir`. File layout, privacy implications, record correlation, and the complete schema are in [logging.md](logging.md).

A capture with no detectable speech inserts nothing, so it plays the two-pulse error cue instead of the completion cue. Disable cue tones:

```bash
parakit start --no-sounds
```

## Configuration

Config file location, commands, precedence, loading, and recovery are in [configuration.md](configuration.md). The type, default, valid values, and interactions for every setting are in [config_reference.toml](config_reference.toml).
