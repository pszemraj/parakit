# Configuration Reference

The per-key contract for parakit's `config.toml` and for every environment
variable parakit reads. Each key below states its type, its default, the values
it accepts, and how it interacts with the rest of the file.

[running.md](running.md) and [cleaning-rules.md](cleaning-rules.md) carry the
narrative introductions and worked examples. Neither is required to use this
page.

## File Location

Resolved in this order:

1. `$PARAKIT_CONFIG_PATH`, used verbatim on every platform. Set but empty is a
   hard error: `PARAKIT_CONFIG_PATH is set but empty`.
2. Linux and macOS: `$XDG_CONFIG_HOME/parakit/config.toml`, falling back to
   `$HOME/.config/parakit/config.toml` when `XDG_CONFIG_HOME` is unset or empty.
   macOS uses this XDG path, not `~/Library/Application Support`.
3. Windows: `%APPDATA%\parakit\config.toml`.

`parakit config path` prints the resolved path without reading the file.

## Contracts That Apply To Every Key

**Everything is optional.** A missing file, a missing section, and a missing key
are all equivalent to the built-in default. No key is required, and there is no
key whose absence is an error.

**Precedence is CLI flag > this file > built-in default,** with five exceptions
that are marked on the keys themselves. `sounds` and `cleaning.enabled` have
only a negating flag, which can force the value off but never on.
`keep_transcript_clipboard` and `cleaning.keep_trailing_period` have only an
affirming flag, which can force the value on but never off. `verbose` is also
affirming, while `--quiet` suppresses both CLI- and config-requested verbose
output.

**Misspelled key names are silently ignored.** parakit deliberately does not set
`deny_unknown_fields`, so a config written for a newer version still loads on an
older one. The cost is that `pastemode = "direct"` or `[hotkeys]` parses fine,
does nothing, and produces no warning. Check spelling against this page.

**Invalid *values* are hard errors,** unlike invalid key names. A bad enum
string or a malformed table aborts with `failed to parse config file <path>`.

**Nothing is re-read while the daemon runs.** The file is loaded once per
process at startup. Editing it has no effect on a running daemon — stop it and
relaunch. This applies to every key in every section; it is not repeated below.

**`parakit config show` reports the file merged with built-in defaults, not
your CLI flags.** It never sees command-line arguments, so `parakit --device gpu
config show` still reports the configured or default device.

**Validation happens at different times for different keys,** which decides
whether a mistake surfaces immediately or at next daemon start:

| Checked when the file loads | Checked only when the daemon starts |
| --- | --- |
| TOML syntax; all enum values; `cleaning.number_threshold` (finite and nonnegative); `[[rules.user]]` name (non-empty, no surrounding whitespace), pattern (non-empty), regex validity, name collisions, duplicate names; `cleaning.disabled_rules` names | `daemon.model` path existence; `logging.dir` writability; hotkey backend availability |

`parakit config show` runs the first column and fails on a broken file. Every
other `parakit config` subcommand — `path`, `init`, `edit` — works regardless,
so `parakit config edit` is always available to repair a file that no longer
parses.

---

## `[daemon]`

```toml
[daemon]

# Path to a GGUF model file. Default: unset, which downloads and caches the
# hosted Q8_0 model on first run. No `~` expansion — write the path in full.
# Existence is not checked until daemon start, so a typo survives `config show`.
model = <path>

# Compute device. Default: "auto".
# "gpu" aborts at startup when no GPU is detected; "auto" lets the engine
# choose and takes a different, engine-managed code path than "cpu"/"gpu".
# Recommend leaving unset unless you are diagnosing a backend problem.
device = "auto" | "cpu" | "gpu"

# CPU inference threads. Must be >= 1; `0` is rejected at parse time.
# Default: computed — half the logical CPUs, floored at 2 (1 on single-core).
# An explicit value bypasses that heuristic and is not clamped, so it is
# possible to oversubscribe the machine. On Windows this value also seeds
# OMP_NUM_THREADS unless that variable is already set.
threads = <int >= 1>

# Insertion style. Default: computed per platform — "terminal" on Linux,
# "standard" elsewhere.
#   "terminal"  Ctrl+Shift+V (Cmd+V on macOS). Strips trailing newlines,
#               refuses multi-line text outright, and caps at 2000 characters.
#   "standard"  Ctrl+V (Cmd+V on macOS). Caps at 20000 characters.
#   "direct"    Types the text; never touches the clipboard.
# Over any cap parakit copies instead of pasting rather than erroring, so an
# unexpectedly long or multi-line dictation lands on the clipboard silently.
# "direct" is an app-compatibility fallback: slower, less reliable for
# non-ASCII text, and it makes keep_transcript_clipboard inert (see below).
paste_mode = "terminal" | "standard" | "direct"

# Leave the dictated text on the clipboard instead of restoring the previous
# contents. Default: false.
# Has no effect when paste_mode = "direct", which never writes the clipboard.
# `--keep-transcript-clipboard` forces this on; no flag can force it off
# against `true` here.
keep_transcript_clipboard = <bool>

# Play the start / success / error cue tones. Default: true.
# `--no-sounds` forces this off; no flag can force it on against `false` here.
sounds = <bool>

# Verbose diagnostics: paths, backend details, timing lines. Default: false.
# Also raises the native inference library's log level.
# `--quiet` overrides this silently — no warning that the file asked otherwise.
# Commands that intentionally bypass config loading ignore this key. In
# particular, use `parakit --verbose status` to request the status detail block;
# `verbose = true` here does not make plain `parakit status` verbose.
verbose = <bool>

# Transcripts kept in daemon memory for `paste-last`, `copy-last`, and
# `history`. Default: 10. `0` disables all three commands, which then report
# that history is disabled. No CLI flag exists; this key is the only control.
# Memory only: never written to disk, discarded when the daemon stops.
transcript_history = <int >= 0>
```

---

## `[cleaning]`

```toml
[cleaning]

# Run the text-cleaning pipeline. Default: true.
# `--no-cleaning` forces this off; no flag can force it on against `false`
# here. Turning cleaning off also disables every [[rules.user]] rule.
enabled = <bool>

# Cleanup behavior tier. Default: "safe". One of "safe" | "aggressive".
# `safe` is mechanical cleanup plus high-confidence normalization only.
# `aggressive` adds deletion of discourse markers such as filler `like`,
# `you know`, and `I mean`, which can change emphasis or meaning.
# Overridden by `--cleaning-profile`. Ignored when `enabled = false`.
# Never applies to [[rules.user]] rules, which always run when enabled.
profile = <string>

# Keep the single terminal period. Default: false (the period is dropped).
# Removal is the default because parakit targets messaging-style dictation.
# `--keep-trailing-period` forces this on; no flag can force it off against
# `true` here. Equivalent to disabling the `fix-trailing-period` rule, but
# stated as an intent rather than a rule name, so it survives rule renames.
# Ignored when `enabled = false`.
keep_trailing_period = <bool>

# Minimum isolated numeric value rendered as digits. Default: unset, which
# converts every recognized number; `0` has the same meaning.
# Values strictly below the threshold stay as words, while the threshold
# itself and larger values become digits. For example, `5` preserves isolated
# `zero` through `four` and converts `five` and above.
# Must be finite and >= 0. No CLI override exists.
# This is text2num's isolated-number threshold. Structured expressions such as
# multi-point versions are still normalized by their dedicated passes.
number_threshold = <number >= 0>

# Rule names to skip. Default: [] (no rules skipped).
# Accepts any built-in or user rule name; `parakit --list-rules` prints the
# authoritative set, including user rules. Merged as a union with repeated
# `--disable-rule` flags rather than replaced by them.
# An unknown name here is caught when the file loads: `config show` and
# daemon startup both fail with `no rule named '<name>'`.
# Naming a user rule here skips compiling it, so this is also the way to
# park a [[rules.user]] entry whose regex does not compile.
disabled_rules = [<string>, ...]
```

---

## `[logging]`

Transcription logs contain the raw and cleaned transcript text in plain text,
plus the identifier of the app that was pasted into. Audio is never written.

```toml
[logging]

# Directory for transcription logs, one file per local day. Default: unset,
# which disables transcription logging entirely.
# Created lazily on first write, not at startup. If it cannot be created or
# written, parakit prints one line to stderr per dictation and carries on —
# logging never blocks or fails a dictation.
# There is no rotation, retention, or size cap: files grow until you delete
# them. Recommend leaving unset unless you are actively investigating
# transcription quality.
dir = <path>

# Log format. Default: "jsonl". Read only when `dir` is set.
# Note `--log-format json` is accepted on the command line but `"json"` is
# NOT valid here; the config file takes "jsonl" or "tsv" only.
format = "jsonl" | "tsv"
```

---

## `[hotkey]`

Linux only. On macOS and Windows this section does not exist in the schema, so
it falls under the silently-ignored-unknown-key rule: a `[hotkey]` table there
parses, does nothing, and warns about nothing. The backend is always `auto` off
Linux, and `--hotkey-backend` is not a flag those builds accept.

All backends except `evdev-proxy-experimental` require an X11 session and
refuse to start on Wayland.

```toml
[hotkey]

# Hotkey capture backend. Default: "auto".
#   "auto", "desktop", "x11-global-hotkey"
#           Register Ctrl+Space as a global hotkey. These three are the same
#           code path today. Fails when another program already owns
#           Ctrl+Space — a GNOME input-source shortcut is the usual culprit.
#   "x11-listen"
#           Passively observe key events instead of registering a hotkey.
#           Does not grab or suppress input. Use when something else
#           legitimately owns Ctrl+Space.
#   "evdev-proxy-experimental"
#           Experimental. Grabs keyboard devices and forwards input through
#           uinput. The only backend that works without X11. Needs read
#           access to /dev/input/event* (usually the `input` group) and a
#           writable /dev/uinput (usually a udev rule). Do not work around
#           those with sudo — audio, clipboard, and insertion belong to the
#           desktop user.
# The CLI additionally accepts `evdev-proxy` as an alias; the config file
# does not.
backend = "auto" | "desktop" | "x11-global-hotkey" | "x11-listen" | "evdev-proxy-experimental"
```

---

## `[[rules.user]]`

An array of tables — repeat the `[[rules.user]]` header once per rule. Rules
are applied in the order they appear within each position group.

Unlike every other section, `name`, `pattern`, and `replacement` are
**required** for a table to parse. All three are validated when the file loads,
so a mistake here blocks daemon start, `parakit config show`, `--list-rules`,
and `--test-rules` — but not `status`, `stop`, `paste-last`, `copy-last`,
`history`, or `config edit`.

> **Uncomment the `[[rules.user]]` header, not just the keys under it.** TOML
> attaches bare keys to the table opened above them, which in the shipped
> template is `[hotkey]`. Because unknown keys are ignored, a rule written that
> way silently never registers.

```toml
[[rules.user]]

# Rule identifier, required. Must not be empty or whitespace-only
# (`user rule #<n> has an empty name`) and must not have leading or trailing
# whitespace. Must not match a built-in rule name (`user rule '<name>' has the
# same name as a built-in rule; rename it`) or another user rule
# (`duplicate user rule name '<name>'`).
# Shown by `--list-rules` and usable in cleaning.disabled_rules.
name = <string>

# Human-readable note, optional. Default: none. Shown by `--list-rules`.
description = <string>

# Rust `regex` crate pattern, required. Must not be the empty string
# (`user rule '<name>' has an empty pattern`) — an empty regex matches at
# every position, so replacement would be spliced between every character.
# No lookahead, no lookbehind, no backreferences. Invalid patterns fail with
# `user rule '<name>' has invalid regex: <detail>`, except when the rule is
# listed in cleaning.disabled_rules, in which case it is never compiled.
pattern = <string>

# Replacement text, required. Supports `$1` / `${name}` capture references and
# `$$` for a literal `$`. Never validated: a reference to a group that does
# not exist expands to nothing, silently dropping text from the transcript.
replacement = <string>

# Where this rule is spliced into the built-in pipeline. Default: "standard".
#   "first"     Before every built-in rule. Sees raw transcription output;
#               its result is then reprocessed by all of them.
#   "standard"  After the content rules (filler, stutter, contractions),
#               before the final whitespace and punctuation cleanup group
#               (fix-space-before-punct, fix-collapse-spaces, fix-trim,
#               fix-leading-comma, fix-trailing-period). Replacement text does
#               not need tidy spacing — the cleanup group fixes it afterward.
#   "last"      After every built-in rule. Nothing normalizes the output, so
#               stray spacing or a trailing period you introduce here survives.
# Sentence-start capitalization always runs after every rule regardless of
# position, so no position can prevent its output from being capitalized.
# Recommend leaving unset unless the rule depends on the raw text ("first") or
# must survive whitespace cleanup ("last").
position = "first" | "standard" | "last"
```

---

## Environment Variables

parakit reads these; it does not read a `.env` file.

| Variable | Effect |
| --- | --- |
| `PARAKIT_CONFIG_PATH` | Overrides the config file path outright, all platforms. Set but empty is an error. |
| `PARAKIT_MODELS_DIR` | Overrides the model cache directory. Set but empty is an error. Default: `$XDG_CACHE_HOME/parakit/models` on Unix, falling back to `$HOME/.cache/parakit/models`; `%LOCALAPPDATA%\parakit\Cache\models` on Windows. |
| `XDG_CONFIG_HOME` | Base for the default config path on Linux and macOS. Ignored when empty. |
| `XDG_CACHE_HOME` | Base for the default model cache on Linux and macOS. Ignored when empty. |
| `HOME` | Required on Unix when the `XDG_*` variable for a given path is unset. |
| `VISUAL`, `EDITOR` | Editor for `parakit config edit`, `VISUAL` first. With neither set the command errors and prints the path to edit by hand. |
| `DISPLAY`, `XDG_SESSION_TYPE`, `WAYLAND_DISPLAY`, `XAUTHORITY`, `XDG_RUNTIME_DIR`, `USER` | Read on Linux to detect the session type and to build hotkey remediation messages. Not parakit settings — they come from your desktop session. |

On Windows, parakit *sets* `OMP_NUM_THREADS` (to the effective thread count),
`OPENBLAS_NUM_THREADS` (to `1`), and `OMP_WAIT_POLICY` (to `PASSIVE`) for its
own process, but only for variables you have not already set.
