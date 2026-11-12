# Configuration

parakit reads an optional TOML file for daemon defaults, cleaning behavior, transcription logging, Linux insertion and hotkey backends, and user-defined cleaning rules. A missing file is valid and selects built-in defaults.

For example, keep sentence-final periods and raise the isolated-number cutoff to five:

```toml
[cleaning]
keep_trailing_period = true
number_threshold = 5
```

Save this [starter config](example.toml) at the path printed by `parakit config path`. The [annotated reference](config_reference.toml) lists every key, default, accepted value, interaction, and environment variable.

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

Add literal words or phrases to the built-in repetition exceptions. For example, `bang` extends the [supported spoken-symbol words](cleaning-rules.md#casual-forms-and-stutters), and `tomato` preserves deliberate `tomato tomato` dictation:

```toml
[cleaning]
repetition_exceptions = ["something", "hee", "bang", "tomato", "paddle", "chinny chin chin"]
```

Merge the assignment into your existing `[cleaning]` section if you already have one. Matching ignores case and accepts spaces or tabs between words, without crossing punctuation or line breaks. Contractions such as `it's` are valid words. Empty entries, other punctuation, newlines, and regex syntax are rejected.

These additional exceptions affect only `stutter-safe-words` under both profiles. Other cleaning rules still apply, including the aggressive profile's separate ambiguous-repetition pass. This setting has no CLI flag; `parakit rules test` uses it from the config file.

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
