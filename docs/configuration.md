# Configuration

parakit reads an optional TOML file for daemon defaults, cleaning behavior, transcription logging, Linux insertion and hotkey backends, and user-defined cleaning rules. A missing file is valid and selects built-in defaults.

For example, keep sentence-final periods and raise the number-conversion threshold to five:

```toml
[cleaning]
keep_trailing_period = true
number_threshold = 5
```

Save this [starter config](example.toml) at the path printed by `parakit config path`. The [annotated reference](config_reference.toml) lists the keys, defaults, accepted values, and interactions. Inherited process settings are listed under [environment variables](#environment-variables).

> [!IMPORTANT]
> The daemon reads this file once at startup and does not watch it. Restart the daemon after every edit.

## File Location

parakit resolves one config path:

1. `PARAKIT_CONFIG_PATH` when set to a nonempty value, used verbatim on every platform.
2. Linux and macOS: `$XDG_CONFIG_HOME/parakit/config.toml`, or `$HOME/.config/parakit/config.toml` when `XDG_CONFIG_HOME` is unset or empty.
3. Windows: `%APPDATA%\parakit\config.toml`.

Setting `PARAKIT_CONFIG_PATH` to an empty value is an error. So is pointing it at a file that does not exist: `start`, `doctor`, `rules`, and `config show` refuse to run on defaults in that case, while `config path`, `config init`, and `config edit` still work so the file can be created. A relative override path stays relative to the process working directory. All configured paths, including `PARAKIT_CONFIG_PATH`, `daemon.model`, and `logging.dir`, are used literally; parakit does not expand `~` or environment variables inside them. On Linux and macOS, resolution fails with `HOME is unset or empty` when neither `XDG_CONFIG_HOME` nor `HOME` is set to a nonempty value.

## Commands

```bash
parakit config path
parakit config init
parakit config show
parakit config edit
```

`config path` prints the resolved path without reading the file. `config init` creates any missing parent directories and writes the fully commented template. `config show`, which is also what bare `parakit config` runs, loads and validates the file and prints the resolved path plus the effective value of every key. `config edit` writes the template first when the file is missing, then opens the file with the [configured editor](#editor).

On Linux, `config show` also includes the selected insertion and hotkey backends. See [Linux desktop input](linux-desktop.md#insertion-selection-and-recovery) for transport selection and setup.

### Details

`config init` refuses to replace an existing file. `config init --force` truncates and overwrites it, so use it only when losing the current contents is intended.

`config show` reports the config file merged with built-in defaults. It does not merge CLI flags, so a one-off `parakit start --device cpu` does not appear there, and the output is a summary rather than a round-trippable config. With `daemon.model` set, it reports `model_preset: (not applicable: local model)` and `quantize: (from model file)`.

`--quiet config show` prints nothing but still resolves, loads, and validates the config file, so it can be used as a silent validation command. `--quiet` likewise silences `config path` and the `wrote <path>` line from `config init`, though those commands still perform their normal path resolution and file operations.

### Editor

`config edit` uses `$VISUAL`, falling back to `$EDITOR` when `VISUAL` is unset, empty, or whitespace-only. The value is split into an executable plus arguments using shell-style quoting, then launched directly with the config path appended. `EDITOR="code -w"` therefore works without invoking a shell. If both variables are blank or unset, the command prints the path to edit by hand and exits with an error.

The editor can repair a broken config, but its exit does not validate the file. Follow the [edit and validation loop](#validation-and-recovery).

## Precedence and Loading

The [config reference](config_reference.toml) describes precedence, CLI overrides, and settings available only in the file. Use `parakit start` for daemon flags. The profile, trailing-period, and disable-rule flags also work on `parakit rules list` and `parakit rules test` so you can check cleaning changes before restarting; the profile flag is `--profile` there instead of `--cleaning-profile`.

For Qwen model selection, language hints, and context, see the [Qwen3-ASR guide](models/qwen3-asr.md). Other presets are listed under [model selection](running.md#model-presets).

TOML, unknown-key, invalid-value, and rule-validation errors name the config path.

For `daemon.model_idle_minutes` behavior, see [idle model offload](running.md#idle-model-offload).

## Preserving Repeated Words

Add literal words or phrases to the built-in repetition exceptions. For example, `tomato` preserves deliberate `tomato tomato` dictation:

```toml
[cleaning]
repetition_exceptions = ["something", "hee", "bang", "tomato", "paddle", "chinny chin chin"]
```

Merge the assignment into your existing `[cleaning]` section if you already have one. The [repetition-exception constraints](config_reference.toml) apply to each literal entry.

Other cleaning rules still apply, including the aggressive profile's separate ambiguous-repetition pass. Use `parakit rules test` to check the result before restarting.

Library callers pass `repetition_exceptions: &[String]` immediately after `number_threshold` in `build_cleaner`, `build_enabled_cleaner`, and `validate_configured_rules`. Pass `&[]` to retain the defaults; exceptions are validated and compiled through the same construction path as the other rule inputs.

## Validation and Recovery

`parakit config show` catches TOML and configuration errors. Daemon startup additionally checks machine-dependent resources such as the model file, compute device, insertion backend, hotkey, audio, and permissions. The log directory is created and checked on the first transcription write, so neither `config show` nor startup can prove it is writable.

Use this edit loop for cleaning and user-rule changes:

```bash
parakit config edit
parakit config show
parakit rules test "A representative dictation to clean."
parakit rules list
```

When the result is right, run `parakit stop`, then start it with your usual [foreground or background command](running.md#background-use), including any CLI overrides you normally use.

A broken config does not block repair or control commands that do not need daemon settings: `fetch`, `cache`, `config path`, `config init`, `config edit`, `status`, `stop`, `copy-last`, `history`, `test-paste`, and Linux `ibus` commands. `config show`, `doctor`, `rules list`, `rules test`, PTT simulation, and daemon startup do load it.

## Environment Variables

These are inherited process settings, not TOML keys. Parakit does not load a `.env` file.

For build-time settings, see [building](build.md).

### `PARAKIT_CONFIG_PATH`

Config-file override. See [path selection and missing-file behavior](#file-location).

### `PARAKIT_MODELS_DIR`

Directory that holds downloaded models. Used verbatim when set. Empty is an error. A relative path is relative to the working directory.
The default paths are listed under [model cache](running.md#model-cache).
Read only by commands that touch the model cache: `fetch`, `cache`, and daemon startup when a preset or the default model is selected. It is not consulted when the selected model is an explicit local path.

### `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `HOME`

On Linux and macOS, `XDG_CONFIG_HOME` supplies the [config base](#file-location), while `XDG_CACHE_HOME` supplies the [model cache base](running.md#model-cache) and the runtime fallback below. Relative values are used as given, and empty values count as unset. `HOME` supplies the fallback base when the relevant XDG variable is absent or empty; without either base, path resolution fails with `HOME is unset or empty`. An explicit `PARAKIT_CONFIG_PATH` bypasses config-base resolution.

### `XDG_RUNTIME_DIR`

Linux. Directory whose parakit subdirectory holds the daemon lock file (parakit.lock) and control socket (control.sock). An empty value counts as unset.
Fallback: $XDG_CACHE_HOME/parakit/run, or $HOME/.cache/parakit/run. macOS always uses that fallback. Windows keeps the lock in parakit\run inside Local AppData and talks over a named pipe.
The control commands (`status`, `stop`, `copy-last`, `history`, `test-paste`) and the daemon must resolve the same directory to find each other.
The socket directory must be owned by you, and parakit sets it to mode 0700.

### `VISUAL`

Editor for `parakit config edit`, tried before EDITOR; [selection and command parsing](configuration.md#editor).

### `EDITOR`

Editor fallback for `parakit config edit`; see VISUAL above.

### `DISPLAY`

Linux. Required by X11 hotkeys, clipboard paste, direct typing, and the existing clipboard-copy backend. Native IBus insertion and focused-field PTT do not require it.
Inherit it from the desktop session. An XWayland DISPLAY never enables XTest insertion into native Wayland applications.

### `XDG_SESSION_TYPE`

Linux. A value of "wayland" in any case selects Wayland session handling: automatic hotkeys use IBus and X11 insertion is unavailable.
An empty value counts as present, so it also disables the WAYLAND_DISPLAY fallback below.

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

CUDA builds. Read by the CUDA runtime, not by parakit. A comma-separated list of NVIDIA GPU indices or UUIDs that CUDA may use; an empty value hides all CUDA devices.
Visibility controls affect only their named backend. In bundled builds, daemon.device "auto" falls back to CPU and "gpu" fails at startup only when no GPU remains visible across enabled backends. `parakit --verbose doctor` lists visible devices.
There is no per-device config key or command-line flag, because the Parakeet backend ignores CrispASR's device index.

### `GGML_VK_VISIBLE_DEVICES`

Vulkan builds. Read by ggml's Vulkan backend, not by parakit. A comma-separated list of indices into the Vulkan driver's physical-device list (the GPU0, GPU1 order of `vulkaninfo --summary`) limits which devices Vulkan may use.
The Vulkan0, Vulkan1 names in `parakit --verbose doctor` number only the visible devices, so they can differ from these indices.
An empty value hides all Vulkan devices; an invalid index disables the Vulkan backend. Other backends follow the visibility rule above.

### `HF_ENDPOINT`

Base URL that replaces https://huggingface.co for default/preset model downloads, `parakit fetch <repo>`, and `parakit fetch --from-source`. This includes automatic startup downloads.
A blank value means the default. Trailing slashes are trimmed. It does not apply to `parakit fetch <url>`.

### `HF_TOKEN`

Bearer token sent to the Hugging Face endpoint (HF_ENDPOINT or the default) for those downloads. Blank means none. It is never sent for a direct URL fetch.

### `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY`

Read by the HTTP client library (reqwest's system-proxy support), in upper and lower case, for model downloads. They affect `fetch` and the automatic download only.

### `SSL_CERT_FILE`, `SSL_CERT_DIR`

Read by the TLS library for additional trusted certificates when downloading models.

### `PATH`

`parakit fetch --from-source` searches it for python3 or python. Both source rebuilds and Q6_K preparation require crispasr-quantize here (crispasr-quantize.exe on Windows).
`parakit config edit` relies on it to find the editor program.
