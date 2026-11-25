# Running parakit

The daemon runs in the foreground by default. Use `parakit start` when passing daemon flags, or choose [background mode](#background-use) for daily dictation.

## First Run

```bash
parakit doctor
```

`parakit doctor` checks hotkey access, the microphone, insertion support, and whether a daemon already holds the startup lock. It exits `0` when checks pass and `1` when a check blocks startup. It skips model path validation, downloads, and loading. Run it from the desktop session set up for [Linux](linux-desktop.md), [macOS](macos-desktop.md), or [Windows](windows-desktop.md).

```bash
parakit --verbose doctor
parakit doctor --deep
```

`--verbose` and `--quiet` work before or after the command. `--quiet doctor` runs the same checks with the normal report suppressed. The deep insertion check depends on the desktop backend.

Startup takes the lock and checks the hotkey and insertion backends before opening the microphone or acquiring a model. Once its control endpoint is up, the daemon accepts `status` and `stop` while it prepares and loads the model. The ready line appears after the model warms.

Start with `parakit` once checks pass. The ready line shows the shortcut for your platform (`Ctrl+Space` on Linux and Windows, `Left Control+Space` on macOS). Hold the shortcut to speak and release to insert. Foreground dictation shows the raw and cleaned transcripts when cleaning changes the text. Use `--verbose` for audio formats, diagnostics, and timings.

```bash
parakit --verbose
parakit start --threads 8 --verbose
```

`daemon.verbose` saves the startup and doctor preference. See [configuration options](config_reference.toml) for its scope and precedence.

Inference uses the [configured or computed thread count](config_reference.toml). CPU quotas and affinity can reduce the available count, and an explicit `--threads` value can oversubscribe the machine. The [Windows runtime defaults](configuration.md#environment-variables) also use this count.

A completed transcription in verbose mode reports `release_to_worker`, `infer`, `clean`, `insert`, `worker`, and `total` in milliseconds. `release_to_worker` covers audio finalization and queue wait. `worker` covers reload, inference, cleaning, logging, and insertion. `total` spans capture stop to worker completion, including blocked or copied-only insertion. The last two timings end at worker completion, regardless of when the receiving application displays the text.

## Device Selection

GPU-capable builds default to `--device auto`. In `auto`, CrispASR asks ggml for the best backend device and falls back to CPU when no GPU backend is available. On hybrid laptops, ggml prefers a discrete GPU over an integrated GPU.

Select a [backend when building](build.md#crispasr-and-backends). At runtime, `--device` controls CPU/GPU use inside the installed build.

```bash
parakit start --device auto
parakit start --device cpu
parakit start --device gpu
```

`--device cpu` opens the session with GPU use disabled. `--device gpu` requests the GPU path and, on bundled builds, fails before model load if ggml reports no discrete or integrated GPU device. On system-library builds without the bundled ggml probe, `--device gpu` continues with a warning because parakit cannot verify device visibility before opening the session.

The Parakeet backend ignores CrispASR's device index. To select a device, limit visibility with [`CUDA_VISIBLE_DEVICES`](configuration.md#cuda_visible_devices) or [`GGML_VK_VISIBLE_DEVICES`](configuration.md#ggml_vk_visible_devices).

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

Launch from the [current Linux desktop session](linux-desktop.md#x11-sessions) or the [macOS terminal with parakit's permissions](macos-desktop.md#permissions).

To retain startup errors and warnings after closing the terminal, redirect stderr.

```bash
mkdir -p "$HOME/.local/state/parakit"
nohup parakit --quiet >/dev/null 2>>"$HOME/.local/state/parakit/parakit.err" &
```

Stop it with [`parakit stop`](#daemon-control).

## Daemon Control

Control commands use the running daemon's [per-user socket or named pipe](configuration.md#xdg_runtime_dir).

```text
parakit status
parakit --verbose status
parakit stop
parakit copy-last [N]
parakit history
parakit history --plain
parakit history --text N
parakit test-paste "hello from parakit"
```

`parakit status` reports the current phase, last transcript size, package versions, and cleaner revisions. `parakit --version` identifies the CLI binary launched by your shell. Installing a new binary leaves the existing daemon running its original code. If either reported value differs or the daemon version is `unreported`, run `parakit stop` and start again with your usual options.

Use `parakit --verbose status` to inspect the running process, uptime, dictation count, microphone, model filename, compute device, and effective settings. Preset and Qwen details appear when applicable. Linux also reports its hotkey and insertion backends. During startup these details may be unavailable. See [idle offload](#idle-model-offload) for residency and reload diagnostics.

`parakit stop` allows up to five seconds for worker and insertion cleanup and reports success once the startup lock is released. If the endpoint is unavailable while the lock is held, `status` reports a daemon that may be starting or stopping. `stop` looks for the endpoint during startup and waits for the lock during shutdown. If it times out, identify the daemon with `pgrep -af parakit` on Unix-like systems or `Get-Process parakit` on Windows and end that process.

With the endpoint absent and the lock free, `status` and `stop` report that parakit is not running and exit successfully. `history`, `copy-last`, and `test-paste` require a running daemon. If `start` finds a responsive daemon already running, it exits successfully and reports any startup options it could not apply. To change those options, stop the daemon and start it again.

History holds recent transcripts from the current daemon session, including those whose insertion was blocked or failed. Transcripts that sanitize to whitespace are discarded. `copy-last` copies the newest transcript. For example, `parakit copy-last 3` copies the third-newest. Configure the capacity or turn history off with `daemon.transcript_history` in the [configuration reference](config_reference.toml).

In a terminal, `parakit history` opens a picker with the newest transcript selected. Use Up/Down to select an entry and PageUp/PageDown to scroll its preview. Enter copies the selected entry and closes the picker after success. Escape or Ctrl+C closes it without copying. Copy failures stay visible in the picker. If the daemon restarts or evicts the selected transcript, the picker reports that the original entry is unavailable.

Piped or redirected `history` output is a plain newest-first list. Use `--plain` for that list in a terminal, `--limit N` to cap the number of entries, or `--text N` to print one full transcript without clipboard access. `--quiet` also bypasses the picker. Full-text previews and output have a 64 KiB protocol limit. If text exceeds the limit, the command reports an error without truncation, and Enter can still copy the selected picker entry.

On GNOME Wayland, explicit copying needs XWayland clipboard access. Use the preview or `history --text N` when clipboard access is unavailable.

`test-paste` submits text through the configured [insertion path](#insertion) without opening a microphone capture. It exercises content policy and target checks as well as native IBus commit or clipboard staging and paste.

## Idle Model Offload

The model releases its inference session after the configured idle interval. Set `daemon.model_idle_minutes` or pass `parakit start --model-idle-minutes N`. See the [configuration reference](config_reference.toml) for defaults and bounds.

The idle interval uses monotonic elapsed time. On systems where that clock pauses during suspend, the remaining interval continues after wake. An interval too large for the deadline representation keeps the model resident. [Rust clock behavior](https://doc.rust-lang.org/std/time/struct.Instant.html)

The interval starts when all pending recordings, transcriptions, and insertions finish, including silent captures and failures. Hotkeys, microphone policy, and control commands work throughout offload. Copying a transcript counts as insertion activity and restarts the interval. Viewing status or history leaves the interval running.

After offload, press PTT as usual. A three-note cue announces the reload, and recording begins while the model opens. When it is ready, parakit plays the listening tone if PTT is still held. If you released early, parakit queues the captured audio and skips the listening tone. Reload uses the same local file, thread count, and [device policy](#device-selection).

If opening fails at PTT start, parakit plays its two-pulse error cue and retries on release. If the retry fails, it plays the error cue again and reports the discarded dictation. The next PTT attempts another reload.

Before a bundled build reloads on a GPU, parakit checks free memory for the model and its working space. Qwen on Metal needs twice the model file size plus 2 GiB to account for the main model and audio tower. Qwen on other GPU backends needs one file size plus 2 GiB. Other models need one file size plus 1 GiB. If memory is insufficient, `auto` warns, sends a desktop notification, and reloads on CPU until the next offload. `--device gpu` reports the free and required amounts and fails the reload.

This check uses a snapshot of available memory. Later allocations can exhaust memory and abort the daemon. Set `model_idle_minutes = 0` to keep the session resident if reload delays are unwelcome or another workload could claim the GPU memory while the model is offloaded.

Parakeet startup warms one second of synthetic audio on CPU or five seconds on GPU. Reload uses one second. Qwen startup and reload transcribe an embedded 11-second speech sample with automatic language and a fixed 512-token budget. Parakit discards that text and restores the user's language and token budget before dictation. A small configured budget can fail the dictation even when readiness succeeds. Longer dictations can allocate larger buffers and trigger backend compilation on the first use of a new shape.

`parakit --verbose status` reports `loaded`, `loading`, or `offloaded`, the effective timeout, and the last reload error until a successful reload clears it. Residency is separate from the recording/transcribing phase. Queries never load the model. During startup, residency can be unavailable until the worker publishes its initial state.

For allocations that persist after offload, see [memory accounting](dev/memory.md#allocation-ownership).

## Model Cache

With no model selection, parakit uses the Parakeet Ultra Q8_0 preset. Startup acquires missing models automatically. Use `fetch` to download without starting the daemon. The command prints the cached path and startup command.

```bash
parakit fetch
parakit fetch --force
parakit cache list
parakit cache dir
parakit start -m /path/to/model.gguf
```

`parakit cache` is shorthand for `parakit cache list`. The list shows each model's path, GGUF type, and size, and labels the default Ultra Q8_0 artifact. `parakit cache dir` prints the cache root.

Set `PARAKIT_MODELS_DIR` to override the root for `fetch`, `cache`, and preset startup. Set it to a nonempty, literal path. Relative paths resolve against the working directory. With the override unset, Linux and macOS use `$XDG_CACHE_HOME/parakit/models/` or `~/.cache/parakit/models/`, and Windows uses `%LOCALAPPDATA%\parakit\Cache\models\`.

The cache override does not affect an explicit model path. For `-m` or `daemon.model`, startup checks that the path names a regular file, including a symlink to a file, before checking hotkeys or opening the microphone. It resolves an absolute path for idle reloads. The path must be valid UTF-8, and the model loader validates the contents. See [model selection and precedence](config_reference.toml).

For offline use, fetch the preset on a connected machine and copy the directory printed by `parakit cache dir` to the new machine's cache. Preserve the directory layout. Alternatively, pass an approved GGUF path with `-m`.

### Model Presets

Start with `parakit start --model-preset NAME` or set `daemon.model_preset`. To prepare a preset for later, run `parakit fetch --model-preset NAME`. `fetch` uses the arguments passed to it and ignores the daemon's model settings. It accepts `--force` for presets. Use `--file` and `--sha256` with [custom sources](#other-gguf-builds).

| Preset | Hosted repository | Exact Q8_0 artifact |
| --- | --- | --- |
| `parakeet-v2` (English-only) | [cstr/parakeet-tdt-0.6b-v2-GGUF](https://huggingface.co/cstr/parakeet-tdt-0.6b-v2-GGUF) | `parakeet-tdt-0.6b-v2-q8_0.gguf` |
| `parakeet-v3` | [pszemraj/parakeet-tdt-0.6b-v3-gguf](https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf) | `parakeet-tdt-0.6b-v3-Q8_0.gguf` |
| `qwen3-asr-0.6b` | [cstr/qwen3-asr-0.6b-GGUF](https://huggingface.co/cstr/qwen3-asr-0.6b-GGUF) | `qwen3-asr-0.6b-q8_0.gguf` |
| `qwen3-asr-1.7b` | [cstr/qwen3-asr-1.7b-GGUF](https://huggingface.co/cstr/qwen3-asr-1.7b-GGUF) | `qwen3-asr-1.7b-q8_0.gguf` |
| `parakeet-ultra` (default) | [cstr/parakeet-ultra-GGUF](https://huggingface.co/cstr/parakeet-ultra-GGUF) | `parakeet-ultra-q8_0.gguf` |
| `parakeet-redux` | [cstr/parakeet-redux-GGUF](https://huggingface.co/cstr/parakeet-redux-GGUF) | `parakeet-redux-q8_0.gguf` |

The experimental [Qwen3-ASR presets](models/qwen3-asr.md) add language controls and prior-dictation context. Redux is for checkpoint comparison. It uses the ordinary Parakeet backend with reconstructed weights, without packed ternary acceleration or Moondream's VAD head. Photon's 178 MB footprint, CPU performance, and pause-based segmentation therefore apply only to the original implementation. To compare presets on your machine, use the [CPU comparison procedure](dev/quality.md#parakeet-cpu-comparison).

Parakeet v3 follows the provider's `main` revision and stores Q8_0 at the cache root. The other Q8_0 presets use pinned provider commits and [revision-specific cache paths](#other-gguf-builds). `fetch --force` downloads the catalog's selected revision again.

For Q6_K, choose a preset and add `--quantize q6_k`. Start directly or prepare the same cached artifact in advance.

```bash
parakit fetch --model-preset qwen3-asr-1.7b --quantize q6_k
parakit start --model-preset qwen3-asr-1.7b --quantize q6_k --device gpu
```

To prepare Q6_K, put a compatible `crispasr-quantize` on `PATH` (`crispasr-quantize.exe` on Windows). Preparation uses the provider revision's F16 GGUF and prints a notice even in quiet mode. It reuses a user-fetched F16 or downloads its own intermediate, removing that file on success and retaining it on failure for retry. `fetch --force` rebuilds Q6_K from a fresh intermediate and preserves user-fetched F16 files. Cached Q6_K can start offline.

Qwen Q6_K retains a Q8_0-or-higher audio encoder. Parakeet quantizes eligible encoder and decoder matrices to Q6_K. Unsupported tensors keep higher precision. The WAV helper accepts the same precision flag with a preset. For original checkpoints, use the separate [source rebuild workflow](dev/README.md#source-rebuild), which produces Q8_0.

During interactive download, the transfer display shows speed and ETA when size is known, or bytes transferred when it is unknown. Quiet mode and redirected stderr hide the progress bar. Quantization is announced separately. Use `--verbose` for source URLs and native converter output.

Moondream's [Ultra](https://huggingface.co/moondream/parakeet-ultra) and [Redux](https://huggingface.co/moondream/parakeet-redux) derive from NVIDIA Parakeet v3 and use [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). The cstr repositories above provide the GGUF conversions.

### Other GGUF Builds

Download a custom GGUF by repository, exact filename, revision, or URL.

```bash
parakit fetch cstr/parakeet-tdt-0.6b-v3-GGUF
parakit fetch cstr/parakeet-tdt-0.6b-v3-GGUF --file parakeet-tdt-0.6b-v3-q4_k.gguf
parakit fetch handy-computer/parakeet-tdt-0.6b-v3-gguf@main
parakit fetch https://example.com/models/parakeet-q8.gguf --sha256 '<64-hex-sha256>'
```

Pass a Hugging Face repo (`owner/repo` or `owner/repo@revision`) or a direct HTTP or HTTPS URL. For a repo, automatic selection ignores `.imatrix.gguf` files and chooses the sole remaining GGUF, or the sole file whose name contains `q8_0` in any case. If several candidates remain, pass the exact name with `--file`. An explicit filename can also select a calibration file.

Supply `--sha256` to check the file against an expected hash. Downloads resume when the server can validate the partial file using a strong ETag or Last-Modified value. A partial with no usable validator restarts. `--force` discards any partial and replaces the cached file.

Repo fetches store files under `<cache dir>/hub/<owner>--<repo>--<key>/<file>`. The key separates each repo, revision, and file path. URL fetches use `<cache dir>/url/<url-key>/<file>` and key on the exact source URL. To use a custom fetch, pass its printed path to `parakit start -m <path>` or set `daemon.model`.

For mirrors, authentication, proxies, and TLS interception, see [corporate-network downloads](troubleshooting.md#downloads-behind-a-corporate-proxy).

## Microphone

parakit follows the OS default input device and avoids monitor, loopback, and virtual sources when a better input is available. It prefers mono at the default sample rate and format. Multichannel input is downmixed before resampling. The model receives 16 kHz mono PCM.

Microphone capture and WAV helpers share the same native resampler for input at other sample rates. [Resampling checks](dev/quality.md#qwen-and-moondream-comparisons)

On Linux and macOS, the microphone stream stays warm while the daemon is running and keeps 350 ms of pre-roll to reduce clipped beginnings. On Windows, parakit pauses the input stream while idle to reduce CPU use by `audiodg.exe` and the microphone driver. Device probes run separately from recording controls, normally every five idle seconds.

One continuously held recording is force-stopped and handed to the worker after 270 seconds. This bounds a missed hotkey-release event without discarding the captured audio. It follows the usual transcription path and normal completion cue.

When the default input changes while idle, a second probe confirms it before parakit switches and prints the new microphone. Quiet mode suppresses that line. Linux tracks the source behind CPAL's generic `default` device through PulseAudio or PipeWire metadata and ignores failed metadata queries. Successful reopens are spaced by at least ten seconds, and capture can continue on a healthy stream during that interval. After a stream failure, parakit backs off retries without blocking recording controls. New captures report the missing input, and stopping a capture returns audio already recorded. Notifications are limited, and parakit announces recovery after a stable interval.

Bluetooth microphones are allowed, but parakit prints a warning because headset profiles often add latency and reduce speech quality. The warning still goes to stderr in `--quiet` mode.

## Insertion

Parakit initiates transcription on hotkey release. Linux selects an [IBus or X11 insertion transport](linux-desktop.md#insertion-selection-and-recovery), and other platforms use their desktop input APIs.

All modes convert CRLF and CR to LF, then drop NUL and other ASCII control characters except tab and LF. A whitespace-only result follows the [skipped outcome](logging.md#insertion-outcomes).

Paste mode controls transcript content and the shortcut used by the clipboard route.

| Mode | What it sends | When to pick it |
| --- | --- | --- |
| `terminal` | Native commit on IBus. Clipboard paste uses `Ctrl+Shift+V` on Linux and Windows, or `Cmd+V` on macOS. | Strips trailing newlines and blocks embedded newlines. Native terminal commits also block tabs because they lack clipboard bracketed-paste semantics. |
| `standard` | Native commit on IBus. Clipboard paste uses `Ctrl+V` on Linux and Windows, or `Cmd+V` on macOS. | GUI applications and multiline text. |
| `direct` | Synthetic typing through the platform keyboard API. Linux uses X11. | When clipboard chords fail to reach the target. Direct typing is slower and less reliable for non-ASCII text. |

See [configuration options](config_reference.toml) for platform defaults.

```bash
parakit start --paste-mode terminal
parakit start --paste-mode standard
parakit start --paste-mode direct
```

### Clipboard Staging And Restore

The clipboard route in `terminal` and `standard` modes stages plain text and sends the paste shortcut. Both give the target and clipboard history tools time to observe the staged text, then restore the previous clipboard contents when the clipboard API can round-trip them. Restore support covers text, HTML with a text alternative, copied file lists, and images exposed as normal platform image data. Browser-private image packages, WebP-only payloads, and other clipboard MIME formats are not restorable through `arboard`; when restore is required, parakit clears the staged transcript instead of leaving sensitive text as the active clipboard.

Successful native IBus delivery and direct typing skip this transaction. `--keep-transcript-clipboard` has no effect on them. Rejected or uncertain native delivery attempts a clipboard-only recovery copy, retaining the transcript regardless of this setting; it never sends another paste or commit.

If the previous clipboard cannot be read or changes while parakit saves it, the daemon stages and pastes the transcript and leaves it active. A clipboard history manager may retain the earlier copy.

After staging, parakit checks that the staged text and native clipboard state still match. A replacement or unreadable clipboard before the final focus check causes one re-stage of the transcript, which remains active after paste. Parakit rechecks focus after that write. On macOS and Windows, a clipboard write observed during this final check cancels the paste and preserves the newer copy; X11 reports only the selection owner, so a clipboard manager taking ownership of the transcript there does not cancel it. Newer copies made after dispatch are also preserved during restoration. The transcript remains in daemon history when history is enabled. These checks cannot lock the clipboard until the receiving application consumes it.

`--keep-transcript-clipboard` keeps the transcript active after successful paste or blocked fallback. On macOS and Linux, parakit waits up to two seconds for held modifiers to release before sending the shortcut. If a modifier stays held, parakit skips the paste and keeps the transcript on the clipboard regardless of the setting. This logs `copied_only`, prints a warning, sends a "Transcript copied" notification, and plays the success tone.

Linux clipboard modes use a fixed-delay restore gate. Platform-specific timing is covered under [Windows insertion](windows-desktop.md#insertion) and [macOS acknowledgement](macos-desktop.md#insertion); see [macOS recovery](troubleshooting.md#macos-paste-could-not-be-confirmed) for unconfirmed pastes.

Use [daemon history](#daemon-control) for same-session recovery or [JSONL logging](logging.md) for durable transcript records. Clipboard history managers also retain staged transcripts after parakit restores the active clipboard; on Windows, enable the built-in history and open it with `Win+V`. Disable the manager if dictated text must not outlive the paste.

### Focus Changes

Parakit captures focus at PTT-down and checks it before insertion. See the platform checks for [Linux IBus and X11](linux-desktop.md#focus-guard), [Windows](windows-desktop.md#insertion), and [macOS](macos-desktop.md#insertion).

## Logging And Sounds

For durable transcript records, [enable JSONL logging](logging.md#enable-logging).

Sound cues mark listening, success, errors, and [model reload](#idle-model-offload). Silent captures, failed reloads or transcriptions, and [alerting insertion outcomes](logging.md#insertion-outcomes) play the two-pulse error cue. Parakit plays the listening cue only while the key is held. It drops cues when the playback queue fills and reports playback errors to stderr.

Use `--no-sounds` to turn off cue tones independently of quiet mode.

```bash
parakit start --no-sounds
```

## Configuration

To keep options across restarts, [edit the config file](configuration.md#validation-and-recovery). Look up accepted keys and values in the [configuration reference](config_reference.toml).
