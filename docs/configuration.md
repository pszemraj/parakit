# Configuration

Parakit reads an optional TOML file for daemon defaults, cleaning behavior,
transcription logging, the Linux hotkey backend, and user-defined cleaning
rules.

The type, default, valid values, interactions, and warnings for each key and
Parakit-owned runtime environment variable are in
[config_reference.toml](config_reference.toml).

## File Location

Parakit resolves one config path:

1. A nonempty `PARAKIT_CONFIG_PATH`, used verbatim on every platform.
2. Linux and macOS:
   `$XDG_CONFIG_HOME/parakit/config.toml`, or
   `$HOME/.config/parakit/config.toml` when `XDG_CONFIG_HOME` is unset or
   empty.
3. Windows: `%APPDATA%\parakit\config.toml`.

Setting `PARAKIT_CONFIG_PATH` to an empty value is an error. Relative override
paths stay relative to the process working directory; Parakit does not expand
`~` or environment variables in paths. On Linux/macOS, path resolution fails
when neither a nonempty `XDG_CONFIG_HOME` nor `HOME` is available.

Print the resolved path without reading the file:

```bash
parakit config path
```

A missing file is valid and selects built-in defaults.

## Create, Inspect, and Edit

```bash
parakit config init
parakit config show
parakit config edit
```

`config init` creates parent directories and writes a fully commented
template. It refuses to replace an existing file. `config init --force`
truncates and overwrites the existing file, so use it only when that is
intentional.

Bare `parakit config` is the same as `parakit config show`. `config show`
loads and validates the file, then prints a defaults-aware summary rather than
the raw file or a round-trippable effective config. It does not merge
unrelated global CLI flags, so, for example, a temporary daemon override is
not reflected there. `--quiet config show` intentionally emits nothing and
returns before loading the file; do not use quiet mode as a validation
command.

`config edit` creates the commented template when the file is missing, then
opens `$VISUAL`, falling back to `$EDITOR` only when `VISUAL` is unset. The
value is launched as one executable name, not parsed as a shell command with
arguments. With neither variable set, the command errors and prints the path
to edit manually. The command remains available when the config is broken and
does not validate the file after the editor closes.

## Precedence and Loading

The usual precedence is:

```text
CLI flag > config value > built-in default
```

Some booleans have only an enabling or disabling CLI flag, disabled rule names
are merged, and a few settings are config-only. Those exact interactions are
documented on each key in
[config_reference.toml](config_reference.toml).

Unknown tables and keys are deliberately ignored for forward compatibility.
That permits an older binary to read a newer config, but it also means a
misspelled key loads without warning and has no effect. Invalid TOML, invalid
known values, and rule-validation failures are hard errors that name the
config path.

The daemon loads the file once during startup and does not watch it. Restart
the daemon to apply edits. Short-lived `--test-rules` and `--list-rules`
processes each load the current file, which makes them useful before a
restart.

## Validation and Recovery

`parakit config show` catches TOML/schema errors, invalid cleaning thresholds,
unknown disabled-rule names, and invalid or conflicting user rules. Daemon
startup additionally checks resources that depend on the real machine, such
as the model file, compute device, insertion backend, hotkey, audio, and
permissions. The log directory is created and checked lazily on the first
transcription write, so `config show` and daemon startup cannot prove it is
writable.

Use this edit loop for cleaning and user-rule changes:

```bash
parakit config edit
parakit config show
parakit --test-rules "A representative dictation to clean."
parakit --list-rules
```

When the result is right, stop and relaunch the daemon with the same startup
command you normally use:

```bash
parakit stop
```

A broken config does not block repair or control commands that do not need
daemon settings: `fetch`, `cache`, `config path`, `config init`, `config edit`,
`status`, `stop`, `paste-last`, `copy-last`, `history`, and `test-paste`.
`doctor`, `--list-rules`, `--test-rules`, PTT simulation, and daemon startup
do load it.
