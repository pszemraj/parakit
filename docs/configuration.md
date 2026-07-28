# Configuration

parakit reads an optional TOML file for daemon defaults, cleaning behavior, transcription logging, the Linux hotkey backend, and user-defined cleaning rules. A missing file is valid and selects built-in defaults.

A small config that pins the thread count, keeps the trailing period that cleanup drops by default, leaves spoken numbers below five as the model produced them, and turns on transcription logging:

```toml
[daemon]
threads = 8
paste_mode = "standard"
transcript_history = 20

[cleaning]
profile = "safe"
keep_trailing_period = true
number_threshold = 5

[logging]
dir = "/home/user/.parakit/logs"
```

Save that as `config.toml` at the path printed by `parakit config path`. Every key is optional, and `parakit config init` writes a commented template listing all of them. The type, default, valid values, interactions, and warnings for each key and each parakit-owned runtime environment variable are in [config_reference.toml](config_reference.toml).

> [!IMPORTANT]
> The daemon reads this file once at startup and does not watch it. Restart the daemon after every edit.

## File Location

parakit resolves one config path:

1. `PARAKIT_CONFIG_PATH` when set to a nonempty value, used verbatim on every platform.
2. Linux and macOS: `$XDG_CONFIG_HOME/parakit/config.toml`, or `$HOME/.config/parakit/config.toml` when `XDG_CONFIG_HOME` is unset or empty.
3. Windows: `%APPDATA%\parakit\config.toml`.

Setting `PARAKIT_CONFIG_PATH` to an empty value is an error. A relative override path stays relative to the process working directory, and parakit does not expand `~` or environment variables inside a path. On Linux and macOS, resolution fails when neither a nonempty `XDG_CONFIG_HOME` nor `HOME` is available.

Print the resolved path without reading the file:

```bash
parakit config path
```

## Commands

```bash
parakit config path
parakit config init
parakit config show
parakit config edit
```

`config init` creates any missing parent directories and writes the fully commented template. `config show`, which is also what bare `parakit config` runs, loads and validates the file and prints the resolved path plus the effective value of every key. `config edit` writes the template first when the file is missing, then opens the file in `$VISUAL`, or in `$EDITOR` when `VISUAL` is unset.

`config show` output for the example config above:

```text
$ parakit config show
parakit config
  path: /home/user/.config/parakit/config.toml
  exists: true
  daemon:
    model: (default: hosted Q8_0)
    device: (default: auto)
    threads: 8
    paste_mode: standard
    keep_transcript_clipboard: false
    sounds: true
    verbose: false
    transcript_history: 20
  cleaning:
    enabled: true
    profile: safe
    keep_trailing_period: true
    number_threshold: 5
    disabled_rules: []
  logging:
    dir: /home/user/.parakit/logs
  rules:
    user rules: 0
```

A key you did not set prints its built-in default in parentheses. On Linux the output has an extra `hotkey:` section with the selected `backend`. Each user rule adds a `name (position)` line under the `user rules` count.

### Details

`config init` refuses to replace an existing file. `config init --force` truncates and overwrites it, so use it only when losing the current contents is intended.

`config show` reports the config file merged with built-in defaults. It does not merge CLI flags, so a one-off `parakit --device cpu` does not appear there, and the output is a summary rather than a round-trippable config.

`--quiet config show` prints nothing and returns before it loads the file, so quiet mode is not a validation command. `--quiet` likewise silences `config path` and the `wrote <path>` line from `config init`, though `init` still writes the file.

`$VISUAL` and `$EDITOR` are launched as a single executable name with the config path as the only argument, not parsed as a shell command, so a value carrying arguments such as `EDITOR="code -w"` fails to launch. With neither variable set, `config edit` errors and prints the path to edit by hand.

`config edit` stays available when the config is broken, and it does not validate the file after the editor exits. Run `parakit config show` afterward.

## Precedence and Loading

The usual precedence is:

```text
CLI flag > config value > built-in default
```

Some booleans have only an enabling or a disabling CLI flag, disabled rule names from the CLI and the config are merged, and a few settings are config-only. Those exact interactions are documented on each key in [config_reference.toml](config_reference.toml).

Unknown tables and keys are ignored for forward compatibility, which lets an older binary read a newer config. Invalid TOML, invalid known values, and rule-validation failures are hard errors that name the config path.

> [!WARNING]
> Because unknown keys are ignored, a misspelled key loads without any warning and silently has no effect. Run `parakit config show` after editing to confirm a setting actually took.

Short-lived `--test-rules` and `--list-rules` processes each load the current file, which makes them useful for checking a change before restarting the daemon.

## Validation and Recovery

`parakit config show` catches TOML and schema errors, invalid cleaning thresholds, unknown disabled-rule names, and invalid or conflicting user rules. Daemon startup additionally checks resources that depend on the real machine, such as the model file, compute device, insertion backend, hotkey, audio, and permissions. The log directory is created and checked lazily on the first transcription write, so neither `config show` nor daemon startup can prove it is writable.

Use this edit loop for cleaning and user-rule changes:

```bash
parakit config edit
parakit config show
parakit --test-rules "A representative dictation to clean."
parakit --list-rules
```

When the result is right, stop the daemon and start it again with the startup command you normally use:

```bash
parakit stop
parakit --quiet &
```

A broken config does not block repair or control commands that do not need daemon settings: `fetch`, `cache`, `config path`, `config init`, `config edit`, `status`, `stop`, `paste-last`, `copy-last`, `history`, and `test-paste`. `config show`, `doctor`, `--list-rules`, `--test-rules`, PTT simulation, and daemon startup do load it.
