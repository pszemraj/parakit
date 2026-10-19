# Configuration

parakit reads an optional TOML file for daemon defaults, cleaning behavior, transcription logging, the Linux hotkey backend, and user-defined cleaning rules. A missing file is valid and selects built-in defaults.

A small config that keeps the trailing period that cleanup drops by default and leaves spoken single-digit numbers below five as the model produced them:

```toml
[cleaning]
keep_trailing_period = true
number_threshold = 5
```

That is the same config as the commented starter file [example.toml](example.toml), which explains each choice. Save it as `config.toml` at the path printed by `parakit config path`. Every key is optional, and `parakit config init` writes a commented template listing all of them. The [annotated reference](config_reference.toml) lists every key with its type, default, accepted values, interactions, and warnings, and documents parakit-owned environment variables. It is itself a valid config file that holds every default.

> [!IMPORTANT]
> The daemon reads this file once at startup and does not watch it. Restart the daemon after every edit.

## File Location

parakit resolves one config path:

1. `PARAKIT_CONFIG_PATH` when set to a nonempty value, used verbatim on every platform.
2. Linux and macOS: `$XDG_CONFIG_HOME/parakit/config.toml`, or `$HOME/.config/parakit/config.toml` when `XDG_CONFIG_HOME` is unset or empty.
3. Windows: `%APPDATA%\parakit\config.toml`.

Setting `PARAKIT_CONFIG_PATH` to an empty value is an error. So is pointing it at a file that does not exist: `start`, `doctor`, `rules`, and `config show` refuse to run on defaults in that case, while `config path`, `config init`, and `config edit` still work so the file can be created. A missing file at the default location is fine and selects built-in defaults. A relative override path stays relative to the process working directory. All configured paths, including `PARAKIT_CONFIG_PATH`, `daemon.model`, and `logging.dir`, are used literally; parakit does not expand `~` or environment variables inside them. On Linux and macOS, resolution fails with `HOME is unset or empty` when neither `XDG_CONFIG_HOME` nor `HOME` is set to a nonempty value.

## Commands

```bash
parakit config path
parakit config init
parakit config show
parakit config edit
```

`config path` prints the resolved path without reading the file. `config init` creates any missing parent directories and writes the fully commented template. `config show`, which is also what bare `parakit config` runs, loads and validates the file and prints the resolved path plus the effective value of every key. `config edit` writes the template first when the file is missing, then opens the file in `$VISUAL`, or in `$EDITOR` when `VISUAL` is unset, empty, or whitespace-only.

On Linux, `config show` also includes the selected hotkey backend.

### Details

`config init` refuses to replace an existing file. `config init --force` truncates and overwrites it, so use it only when losing the current contents is intended.

`config show` reports the config file merged with built-in defaults. It does not merge CLI flags, so a one-off `parakit start --device cpu` does not appear there, and the output is a summary rather than a round-trippable config. With `daemon.model` set, it reports `model_preset: (not applicable: local model)` and `quantize: (from model file)`.

`--quiet config show` prints nothing but still resolves, loads, and validates the config file, so it can be used as a silent validation command. `--quiet` likewise silences `config path` and the `wrote <path>` line from `config init`, though those commands still perform their normal path resolution and file operations.

`$VISUAL` and `$EDITOR` are split into an executable plus arguments using shell-style quoting, then launched directly with the config path appended. Values such as `EDITOR="code -w"` therefore work without invoking a shell. An empty or whitespace-only value counts as unset, so a blank `VISUAL` falls through to `EDITOR`. With neither variable set to a nonblank value, `config edit` errors and prints the path to edit by hand.

`config edit` stays available when the config is broken, and it does not validate the file after the editor exits. Run `parakit config show` afterward.

## Precedence and Loading

The usual precedence is:

```text
CLI flag > config value > built-in default
```

These daemon and cleaning flags live on `parakit start`. The profile, trailing-period, and disable-rule flags also work the same way on `parakit rules list`/`parakit rules test` for checking a change before restarting the daemon (`cleaning.profile`'s flag is spelled `--profile` there instead of `--cleaning-profile`); `--cleaning`/`--no-cleaning` exist only on `start`.

Boolean flag pairs, merged disabled-rule names, and config-only settings have per-key interactions described in [config_reference.toml](config_reference.toml).

To configure Qwen persistently, select a preset and set the language and token budget:

```toml
[daemon]
model_preset = "qwen3-asr-1.7b"

[qwen]
language = "auto"
max_new_tokens = 512

[cleaning]
enabled = false
```

The example disables English cleaning for multilingual dictation. Available models, Qwen controls, and cleaning guidance are under [model presets](running.md#model-presets); accepted config values are in [config_reference.toml](config_reference.toml).

TOML, unknown-key, invalid-value, and rule-validation errors name the config path.

For `daemon.model_idle_minutes` behavior, see [idle model offload](running.md#idle-model-offload).

## Validation and Recovery

`parakit config show` catches TOML and configuration errors. Daemon startup additionally checks machine-dependent resources such as the model file, compute device, insertion backend, hotkey, audio, and permissions. The log directory is created and checked on the first transcription write, so neither `config show` nor startup can prove it is writable.

Use this edit loop for cleaning and user-rule changes:

```bash
parakit config edit
parakit config show
parakit rules test "A representative dictation to clean."
parakit rules list
```

When the result is right, stop the daemon and start it again with the startup command you normally use:

```bash
parakit stop
parakit start --log-dir "$HOME/.parakit/logs" --quiet & disown
```

A broken config does not block repair or control commands that do not need daemon settings: `fetch`, `cache`, `config path`, `config init`, `config edit`, `status`, `stop`, `copy-last`, `history`, and `test-paste`. `config show`, `doctor`, `rules list`, `rules test`, PTT simulation, and daemon startup do load it.
