# Cleaning Rules

parakit applies a typed, deterministic cleanup pipeline after ASR and before text insertion. The pipeline combines three implementation types:

- Rust `regex` rules for predictable mechanical substitutions;
- narrowly bounded `fancy-regex` rules where backreferences eliminate duplicated stutter expressions;
- procedural Rust passes for context-sensitive work such as sentence capitalization, acronym collapsing, and structural number formatting.

English number recognition and conversion is delegated to [`text2num`](https://crates.io/crates/text2num). parakit does not maintain its own English number-word table or cardinal parser. Its procedural number passes only add dictation-specific structure around the crate, such as multi-point version notation and compact identifier formatting.

Built-in passes are code in `src/rules/`. Personal, non-generalizable rules can be added without editing code via `config.toml`; see [User Rules](#user-rules) below.

## Profiles

The default `safe` profile preserves discourse and meaning. It performs whitespace and punctuation repair, removes high-confidence filled pauses, collapses conservative stutters, expands selected casual forms, converts number expressions, collapses spaced uppercase-letter runs, and capitalizes true sentence starts without treating decimal, version, domain, email, filename, or dotted-identifier periods as sentence boundaries.

```bash
parakit --cleaning-profile safe
```

The `aggressive` profile includes every safe pass and adds stylistic deletion of leading discourse markers and broad filler uses of `like`, `you know`, and similar phrases. Those edits can change emphasis or meaning, so they are opt-in.

```bash
parakit --cleaning-profile aggressive
```

A single terminal period is removed by default because parakit primarily targets messaging-style dictation. Preserve it when the target context expects conventional prose punctuation:

```bash
parakit --keep-trailing-period
```

These settings also live in `config.toml` as `cleaning.profile` and `cleaning.keep_trailing_period`; the CLI flags override them. Number conversion has the config-only `cleaning.number_threshold` setting. See [configuration.md](configuration.md).

Disable cleaning entirely:

```bash
parakit --no-cleaning
```

## General Invariants

The default pipeline favors structural rules over product- or vocabulary-specific substitutions.

Spaced acronyms follow one invariant: a run of two or more standalone uppercase ASCII letters separated by exactly one ASCII space is collapsed without separators. For example, `O C R` becomes `OCR`, `R L H F` becomes `RLHF`, and `A B` becomes `AB`. There is no acronym allowlist, so an unfamiliar initialism collapses exactly like a familiar one. Repeated-word stutter handling runs before acronym collapsing, so `I I think` still becomes `I think` rather than `II think`.

Number conversion uses `text2num` with an isolated-number threshold of zero by default, so every recognized numeric expression becomes digits, including isolated values such as `zero`, `one`, and `three`. Set `cleaning.number_threshold = 5`, for example, to leave isolated values strictly below five as words while rendering five and larger values as digits. Omission and an explicit zero are equivalent. Because this is `text2num`'s isolated-value threshold, grouped or structural expressions may still render components below the cutoff numerically. The ambiguous word `second` remains a word when context identifies it as a time unit (`one second`, `per second`, `a split-second`), while genuine ordinals still render numerically. Multi-point versions are parsed component by component through `text2num`; parakit then joins those validated components with periods regardless of the isolated-value threshold. Generic formatting passes compact short uppercase identifiers and split digit groups without maintaining a list of product names.

The safe repeated-word rule remains intentionally conservative. A bounded backreference consolidates a small set of high-confidence function-word stutters, while valid or ambiguous repetition such as `that that` and emphatic `no no` survives. The aggressive profile can opt into collapsing the ambiguous set.

## Inspect And Test

Show every pass, its engine, behavior tier, and whether it is active for the selected options:

```bash
parakit --list-rules
parakit --cleaning-profile aggressive --list-rules
parakit --keep-trailing-period --list-rules
```

Test one input. The output lists each pass that changed the text:

```bash
parakit --test-rules "um, the the G P T model is gonna work."
parakit --cleaning-profile aggressive --test-rules "So, it's like, you know, hard."
```

Disable one or more named passes:

```bash
parakit --disable-rule casual-gonna
parakit \
  --disable-rule spaced-acronyms \
  --disable-rule filler-um-uh
```

Per-word stutter names are consolidated. Use `stutter-safe-words` for conservative default handling and `stutter-ambiguous-words` for the aggressive-only `that`, `no`, `can`, `had`, and `do` set.

## Rule Order

Passes run in the order shown by `--list-rules`. The output of one pass is the input to the next. Aggressive discourse edits run first. Stutter handling precedes acronym collapsing. `text2num` conversion precedes structural identifier formatting. Whitespace cleanup and capitalization run near the end, followed by the default terminal-period removal.

A pass should be moved only with regression evidence. Reordering can change downstream matches even when every individual expression remains unchanged.

## Choosing An Engine

Use the standard `regex` engine for ordinary substitutions. It remains the default because its matching model is predictable and bounded.

Use `fancy-regex` only when an advanced feature materially improves the implementation. The current uses are bounded backreferences for repeated-token and single-letter stutter patterns. Every `fancy-regex` pass sets an explicit backtrack limit, and a runtime limit failure is not fatal: the cleaner fails open, keeping the original transcript and recording the failure rather than inserting partially transformed text. Do not replace the conservative stutter vocabulary with a generic repeated-word backreference: valid language such as `that that` and emphatic `no no` must survive the safe profile.

Use a maintained domain crate when the task is already a well-defined parsing problem. English number grammar belongs to `text2num`, not to a growing local word table.

Use a procedural pass when correctness depends on token classes or longer context. Sentence capitalization, spaced-letter collapsing, multi-point version assembly, and identifier formatting are procedural because they enforce structural invariants that are clearer in Rust than in a monolithic expression.

Every pass must have a stable name, explicit activation tier, tests, and rule-hit attribution. Reducing source declarations is useful only when observability and precision are preserved.

## Adding A Built-In Rule

Add an entry to `DEFAULT_RULES` in `src/rules/defaults.rs` using the `regex_rule!`, `fancy_rule!`, or `procedural_rule!` macro, and pick an activation tier: `Activation::Safe` for mechanical or high-confidence work, `Activation::Aggressive` for anything that can change meaning.

Rules use Rust's `regex` crate dialect:

- no lookbehind;
- no regex backreferences such as `\1` in a standard `regex` pattern;
- capture replacement uses `$1`, `$2`, and so on;
- use `(?i)` for case-insensitive matches.

Personal vocabulary belongs in code only when it generalizes to normal dictation. Personal vocabulary that should *not* be baked into the binary — proper nouns, jargon, a `'cause`-style substitution you don't want everyone to get — belongs in `config.toml` instead. See User Rules below.

## User Rules

For rules that are personal (your vocabulary, your projects, your typing quirks) rather than generally useful, define them in `config.toml` instead of editing `src/rules/`. User rules run alongside the built-in rules in the same cleaning pass, and are never filtered by the profile since you asked for them explicitly.

Run `parakit config init` to create a config file with a commented template, then add a `[[rules.user]]` table per rule:

```toml
[[rules.user]]
name = "weights-and-biases-to-wandb"
description = "Map 'weights and biases' to 'wandb'"   # optional
pattern = "(?i)\\bweights and biases\\b"
replacement = "wandb"
position = "standard"   # "first", "standard" (default), or "last"
```

- `name` must be unique: it must not collide with a built-in rule name or another user rule name.
- `pattern` and `replacement` use the same Rust `regex` crate dialect as built-in rules (no lookbehind, no backreferences, `$1`/`$2`/... for captures, `(?i)` for case-insensitive).
- `position` controls where the rule is spliced into the built-in rule list:
  - `first` — runs before every built-in rule, including leading-filler removal. Use this when your rule needs to see the rawest possible ASR output.
  - `standard` (default) — runs with the bulk of the built-in rules, before the final whitespace/punctuation cleanup group. Right choice for most vocabulary substitutions.
  - `last` — runs after every built-in rule, including whitespace/punctuation cleanup. Use this for rules that should have the final word, such as appending punctuation.

Disable a user rule the same way as a built-in rule:

```bash
parakit --disable-rule weights-and-biases-to-wandb
```

or in `config.toml`:

```toml
[cleaning]
disabled_rules = ["weights-and-biases-to-wandb"]
```

`parakit --list-rules` prints user rules in a separate `(user)` section below the built-in rule list.

### Validation Errors

A broken user rule is a hard error at config load time (`parakit config show` or daemon startup), naming the offending rule:

```text
user rule #1 has an empty name
user rule 'my-rule' has an empty pattern
user rule 'my-rule' has invalid regex: regex parse error: ...
user rule 'filler-um-uh' has the same name as a built-in rule; rename it
duplicate user rule name 'my-rule'
```

An unknown name in `cleaning.disabled_rules` fails the same way, with `no rule named '<name>'`. `parakit config edit` never loads the file, so it stays available to repair a config that no longer parses.

A user rule named in `disabled_rules` is never compiled, so parking a rule with a known-bad pattern there is a supported way to keep a draft in the file without breaking startup.

## Log Schema And Ruleset Identity

Transcription records include:

- `parakit_version`, the Cargo package version embedded in the running binary;
- `cleaner_version`;
- `cleaning_profile`, one of `safe`, `aggressive`, or `disabled`;
- `ruleset_id`, derived from the ordered enabled pass set, user rules, and configurable behavioral inputs;
- `drops_trailing_period`;
- `number_threshold`, the configured isolated-number cutoff or `null` when all recognized numbers are converted;
- `rules_active`;
- `rules_fired`, an ordered array of `{name, matches}` objects;
- `cleaning_failure`, present only when a bounded matcher failed and the cleaner fell back to the original transcript.

When cleaning is disabled the profile is recorded as `disabled`, no ruleset ID is written, the active count is zero, and `rules_fired` is empty. Historical records that contain only `rules_active` remain valid input for the audit tool, but that count alone cannot identify behavior.

When a procedural pass changes behavior without changing its name, increment `CLEANER_VERSION`. That changes the ruleset ID and prevents different implementations from being grouped as the same cleaner.

## Corpus Regression Workflow

Replay one JSONL file or a directory tree with the audit example. The audit uses the same messaging-style terminal-period behavior as the daemon unless `--keep-trailing-period` is supplied. Pass `--number-threshold VALUE` to audit a non-default number policy. Insertion-outcome lines are skipped automatically.

```bash
cargo run --no-default-features --features bundled --example audit-cleaning -- \
  "$HOME/.parakit/logs" \
  --profile safe \
  --output target/cleaning-safe.json
```

Inspect the highest-frequency passes in `rules_fired`, then read the `examples` array, which holds transcripts where the replay disagrees with the historical `cleaned` value.

## Regression Workflow

When a cleanup worsens a transcript:

1. Capture the raw ASR output.
2. Reproduce with `parakit --test-rules "<raw text>"`.
3. Disable candidate passes one at a time with `--disable-rule`.
4. Narrow the pattern or add a more specific rule before the generic one.
5. Add a case to the `rules` unit tests or `tests/cleaning_regressions.rs`.

Avoid disabling broad rule categories to fix a narrow failure. If the failure is personal vocabulary rather than a general defect, add a user rule instead.
