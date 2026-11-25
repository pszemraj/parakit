# Configuration

Save this TOML at the path printed by `parakit config path` to keep sentence-final periods and leave ordinary numbers below five spelled out.

```toml
[cleaning]
keep_trailing_period = true
number_threshold = 5
```

Download the [starter config](example.toml) or browse the [available TOML settings](config_reference.toml) and [environment variables](#environment-variables). To fix a word parakit often transcribes incorrectly, see [word replacements](cleaning-rules.md#word-replacements).

## File Location

parakit resolves one config path:

1. `PARAKIT_CONFIG_PATH` when set to a nonempty value, used verbatim on every platform.
2. Linux and macOS: `$XDG_CONFIG_HOME/parakit/config.toml`, or `$HOME/.config/parakit/config.toml` when `XDG_CONFIG_HOME` is unset or empty.
3. Windows: `%APPDATA%\parakit\config.toml`.

An absent file at the default location uses the built-in defaults. An empty `PARAKIT_CONFIG_PATH` prevents path resolution. When it names a missing file, commands that load the config fail until you create the file or unset the override. Use `parakit config init` to create it. The path conventions in the [config reference](config_reference.toml) also apply to this override.

## Commands

| Command | Result |
| --- | --- |
| `parakit config path` | Print the selected path without reading the file. |
| `parakit config init` | Create the parent directories and a template. Pass `--force` to overwrite an existing file. |
| `parakit config show` | Validate the file and display resolved values. Bare `parakit config` does the same. |
| `parakit config edit` | Open the file with the [configured editor](#editor), creating the template first if needed. |

`config show` prints a summary of file settings merged with built-in defaults. Use `parakit --verbose status` to inspect the [running daemon's settings](running.md#daemon-control). On Linux, the config summary lists the configured [insertion and hotkey backends](linux-desktop.md#insertion-selection-and-recovery). With `daemon.model` set, it reports the preset as inapplicable and precision as `(from model file)`.

Use `parakit --quiet config show` to validate without stdout. The flag also suppresses normal stdout from `config path` and `config init`.

### Editor

`config edit` uses `$VISUAL`, falling back to `$EDITOR` when `VISUAL` is unset, empty, or whitespace-only. The value is split into an executable plus arguments using shell-style quoting, then launched directly with the config path appended. `EDITOR="code -w"` therefore works without invoking a shell. If both variables are blank or unset, the command prints the path to edit by hand and exits with an error.

## Precedence and Loading

See the [config reference](config_reference.toml) for precedence, validation errors, and the CLI overrides for each key. Empty CLI paths (`-m ""` and `--log-dir ""`) fail during argument parsing.

## Preserving Repeated Words

Add `repetition_exceptions` to your `[cleaning]` table to retain deliberate repeats such as `tomato tomato`.

```toml
[cleaning]
repetition_exceptions = ["tomato", "paddle", "chinny chin chin"]
```

See the [config reference](config_reference.toml) for what counts as a literal exception. The [aggressive repeated-word pass](cleaning-rules.md#casual-forms-and-stutters) runs independently.

## Validation and Recovery

The daemon reads the file once at startup. Use `config show` to check each edit. When changing cleanup rules, also preview a representative dictation and check which rules are active.

```bash
parakit config edit
parakit config show
parakit rules test "A representative dictation to clean."
parakit rules list
```

Run `parakit stop` and start it again with your usual [command and flags](running.md#background-use). Startup checks the model file, device, desktop input, microphone, and permissions. [Logging](logging.md) checks its output directory when a record is ready to write.

`config show`, `doctor`, `rules`, daemon startup, and PTT simulation load the file. You can repair invalid TOML with `config edit` and stop a running daemon with `parakit stop`. The other config and daemon control commands, `fetch`, `cache`, and Linux `ibus` commands run independently of the file contents.

## Environment Variables

Set these variables in the launching environment. If you store them in an environment file such as `.env`, load it before starting parakit. See [building](build.md) for build-time settings.

### `PARAKIT_CONFIG_PATH`

Config-file override. See [path selection and missing-file behavior](#file-location).

### `PARAKIT_MODELS_DIR`

See [model cache paths](running.md#model-cache).

### `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `HOME`

On Linux and macOS, `XDG_CONFIG_HOME` selects the [config base](#file-location). `XDG_CACHE_HOME` selects the base for the [model cache](running.md#model-cache) and the runtime fallback below. Relative values are used as given. If the relevant XDG variable is empty or unset, parakit uses `HOME` and reports `HOME is unset or empty` when that is unavailable.

### `XDG_RUNTIME_DIR`

On Linux, `$XDG_RUNTIME_DIR/parakit` holds `parakit.lock` and `control.sock`. If the variable is unset or empty, parakit uses `$XDG_CACHE_HOME/parakit/run` or `$HOME/.cache/parakit/run`. macOS uses the cache fallback. Windows keeps the lock in `parakit\run` inside Local AppData and uses a named pipe for control.

On Unix, the socket directory must belong to you. Parakit sets its mode to 0700. A `path must be shorter than SUN_LEN` error means the full socket pathname is too long. Use a shorter runtime directory on Linux or a shorter cache root on macOS. Start the daemon and run [control commands](running.md#daemon-control) with the same path settings.

### `VISUAL`

First choice for `parakit config edit`. See [editor selection](#editor).

### `EDITOR`

Fallback for `parakit config edit`. See [editor selection](#editor).

### `DISPLAY`

Linux. Required by X11 hotkeys, clipboard paste, direct typing, and the existing clipboard-copy backend. Native IBus insertion and focused-field PTT do not require it.
Inherit it from the desktop session. An XWayland DISPLAY never enables XTest insertion into native Wayland applications.

### `XDG_SESSION_TYPE`

Linux. A case-insensitive `wayland` selects Wayland session handling. See [Linux desktop input](linux-desktop.md). An empty value counts as present for the `WAYLAND_DISPLAY` check.

### `WAYLAND_DISPLAY`

Linux. Consulted only when XDG_SESSION_TYPE is absent. A nonempty value is then treated as a Wayland session.

### `XAUTHORITY`

Linux. Only reported by `parakit --verbose doctor` and by startup failure messages. parakit makes no decision from it. The X11 libraries read it themselves.

### `USER`

Linux. Only fills in the user name in the permission fix shown for the evdev-proxy-experimental backend. It has no other effect.

### `OMP_NUM_THREADS`, `OPENBLAS_NUM_THREADS`, `OMP_WAIT_POLICY`

Windows. Each model open sets these to the effective daemon.threads count, 1, and PASSIVE respectively, unless already present, including an empty string. Linux and macOS leave them unchanged.

### Native Inference Controls

These controls have no TOML keys or CLI flags. Parakit preserves existing values, including empty strings. Set overrides in the launching environment and restart after changes.

Every command defaults `CRISPASR_FC_PW_Q8` and `CRISPASR_FC_FUSED_QKV` to `0` at process start. The native library caches these two controls after the first read.

#### `CRISPASR_FC_PW_Q8`

Controls whether native model loading repacks the F16 convolution pointwise weights of each layer into an extra Q8_0 copy.
The library treats a value starting with 0 as off, any other nonempty value as on, and an empty or absent value as automatic, which repacks only a model that is already quantized.

#### `CRISPASR_FC_FUSED_QKV`

Controls whether native model loading concatenates each layer's attention query, key, and value weights into an extra fused copy.
The library treats a value starting with 0 as off, and any other value, including an empty string or an absent variable, as on.

#### `CRISP_AUDIO_WINDOWED_ATTN`

Native Qwen audio-encoder attention control, defaulted to `1` before initialization.
Unset: windowed attention using the model's inference windows. Parakeet models do not use this control.
A value parsed as a nonzero integer enables windowing; 0, an empty value or a nonnumeric value keeps the native dense path for models without windowed-attention metadata. Models with explicit windowed-attention metadata retain that behavior.
Use an override for [reference comparisons](dev/quality.md#qwen-and-moondream-comparisons).

#### `CRISPASR_QWEN3_HOTWORDS_AS_CONTEXT`

Defaults to `1` at startup so Qwen session hotwords carry free-form context.
A nonzero integer enables raw context. Zero, empty, or nonnumeric values make Parakit transcribe without prior context.

#### `CRISPASR_QWEN3ASR_QUANT_AUDIO`

Native Qwen quantizer override. A nonempty value not starting with 0 allows audio-encoder tensors to use the target quantization below the normal Q8_0 floor.
`parakit fetch --from-source` and Q6_K preparation (`fetch` or automatic startup) set it to 0 for the quantizer child process, preserving the native audio-encoder precision policy regardless of the launching environment. It does not change the parent environment or runtime inference.

### `CUDA_VISIBLE_DEVICES`

The CUDA runtime accepts a comma-separated list of NVIDIA GPU indices or UUIDs. An empty value hides all CUDA devices. Each visibility variable affects only its own backend. See [device selection](running.md#device-selection) for how parakit uses visible devices.

### `GGML_VK_VISIBLE_DEVICES`

The Vulkan backend accepts a comma-separated list of physical-device indices in the GPU0, GPU1 order of `vulkaninfo --summary`. An empty value hides all Vulkan devices. An invalid index disables the Vulkan backend. The Vulkan0, Vulkan1 names in `parakit --verbose doctor` number only the visible devices, so they can differ from driver indices.

### `HF_ENDPOINT`

Base URL that replaces https://huggingface.co for default/preset model downloads, `parakit fetch <repo>`, and `parakit fetch --from-source`. This includes automatic startup downloads.
A blank value means the default. Trailing slashes are trimmed. It does not apply to `parakit fetch <url>`.

### `HF_TOKEN`

Bearer token sent to the Hugging Face endpoint (HF_ENDPOINT or the default) for those downloads. Blank means none. It is never sent for a direct URL fetch.

### `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY`

Read by the HTTP client library (reqwest's system-proxy support), in upper and lower case, for model downloads. They affect `fetch` and the automatic download only.

### `SSL_CERT_FILE`, `SSL_CERT_DIR`

`SSL_CERT_FILE` names a PEM certificate bundle. `SSL_CERT_DIR` names certificate directories separated by `:` on Unix or `;` on Windows. When either is set, the TLS library loads roots only from those locations instead of the operating system trust store. Without them, it uses the native store. See [rustls-native-certs](https://docs.rs/rustls-native-certs/0.8.3/rustls_native_certs/fn.load_native_certs.html).

### `PATH`

`parakit fetch --from-source` searches it for `python3` or `python`, the model's converter (`convert-parakeet-to-gguf.py` or `convert-qwen3-asr-to-gguf.py`), and `download_asr_checkpoint.py` when downloading an original Hugging Face checkpoint. Both source rebuilds and Q6_K preparation require `crispasr-quantize` here (`crispasr-quantize.exe` on Windows).
`parakit config edit` relies on it to find the editor program.
