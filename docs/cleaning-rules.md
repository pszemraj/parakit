# Cleaning Rules

parakit applies a typed, deterministic cleanup pipeline after ASR and before text insertion. The pipeline combines three implementation types:

- Rust `regex` rules for predictable mechanical substitutions;
- narrowly bounded `fancy-regex` rules where backreferences eliminate duplicated stutter expressions;
- procedural Rust passes for context-sensitive work such as sentence capitalization, acronym collapsing, and structural number formatting.

English number recognition and conversion is delegated to [`text2num`](https://crates.io/crates/text2num). parakit does not maintain its own English number-word table or cardinal parser. Its procedural number passes only add dictation-specific structure around the crate, such as multi-point version notation and compact identifier formatting.

Built-in passes are code in `src/rules/`. Personal, non-generalizable rules can be added without editing code via `config.toml`; see [User Rules](#user-rules) below.

## Profiles

The default `safe` profile preserves discourse and meaning. It performs whitespace and punctuation repair, removes high-confidence filled pauses, collapses conservative stutters, expands selected casual forms, drops a trailing connective left dangling at the end of the transcript, converts number expressions, collapses spaced uppercase-letter runs, and capitalizes true sentence starts without treating decimal, version, domain, email, filename, or dotted-identifier periods as sentence boundaries.

```bash
parakit start --cleaning-profile safe
```

The `aggressive` profile includes every safe pass and adds stylistic deletion of leading discourse markers and broad filler uses of `like`, `you know`, and similar phrases. Those edits can change emphasis or meaning, so they are opt-in.

```bash
parakit start --cleaning-profile aggressive
```

A single terminal period is removed by default because parakit primarily targets messaging-style dictation. Preserve it when the target context expects conventional prose punctuation:

```bash
parakit start --keep-trailing-period
```

These settings also live in `config.toml` as `cleaning.profile` and `cleaning.keep_trailing_period`. Number conversion has the config-only `cleaning.number_threshold` setting. The exact CLI/config merge behavior is in the per-key [config reference](config_reference.toml).

Disable cleaning entirely:

```bash
parakit start --no-cleaning
```

## General Invariants

The default pipeline favors structural rules over product- or vocabulary-specific substitutions.

Spaced acronyms follow one invariant: a run of two or more standalone uppercase ASCII letters separated by exactly one ASCII space is collapsed without separators. For example, `O C R` becomes `OCR`, `R L H F` becomes `RLHF`, and `A B` becomes `AB`. There is no acronym allowlist, so an unfamiliar initialism collapses exactly like a familiar one. Repeated-word stutter handling runs before acronym collapsing, so `I I think` still becomes `I think` rather than `II think`.

Number conversion uses `text2num` with an isolated-number threshold of 4 by default. An isolated value at or above the threshold renders as digits, and an isolated value below it is left exactly as the model produced it: parakit forces it neither to a word nor to a digit. Set `cleaning.number_threshold = 0` to convert every recognized numeric expression instead, including isolated `zero`, `one`, and `two`. Omission and an explicit `0` are not equivalent and produce different `ruleset_id` values. The threshold gates isolated values only, so a comma-delimited listing such as `Zero, one, two, three, four.` still converts every member whatever the threshold is, because `text2num` does not treat listed values as isolated. The same applies to grouped and structural expressions, which may render components below the cutoff numerically. The ambiguous word `second` remains a word when context identifies it as a time unit (`one second`, `per second`, `a split-second`), while genuine ordinals still render numerically. Multi-point versions are parsed component by component through `text2num`; parakit then joins those validated components with periods regardless of the isolated-value threshold. Generic formatting passes compact short uppercase identifiers and split digit groups without maintaining a list of product names.

Safe casual-form normalization expands `gonna` and `gunna` to `going to`, and `wanna` and `wana` to `want to`. The complete tag question `, right?` becomes a period, while ordinary questions such as `Did I turn right?` and `Is that right?` remain unchanged. When the replacement is the dictation's final character, the default terminal-period pass removes it unless `--keep-trailing-period` is set.

Safe dangling-connective removal drops a trailing `so`, `but`, `and`, `or`, or `because` (case-insensitive) when it is the transcript's final word and the text underneath still ends in sentence punctuation, so `The build is green. But` becomes `The build is green.`, which the default terminal-period pass then trims to `The build is green` the same way it trims `, right?`. A trailing `,`, `;`, or `:` left under the removed word is promoted to `.`, a trailing `.`, `!`, or `?` is kept as-is, and the rule chains, so `It works. And so.` loses both dangling words and becomes `It works.`. It needs punctuation to fall back on: `I like cats and dogs and` has no preceding sentence punctuation, so the rule cannot distinguish a stranded conjunction from an unfinished thought and leaves the text alone.

The safe repeated-word rule remains intentionally conservative. A bounded backreference consolidates a small set of high-confidence function-word stutters, while valid or ambiguous repetition such as `that that` and emphatic `no no` survives. The aggressive profile can opt into collapsing the ambiguous set.

## Inspect And Test

Show every pass, its engine, behavior tier, and whether it is active for the selected options:

```bash
parakit rules list
parakit rules list --profile aggressive
parakit rules list --keep-trailing-period
```

Test one input. The output lists each pass that changed the text:

```bash
parakit rules test "um, the the G P T model is gonna work."
parakit rules test "So, it's like, you know, hard." --profile aggressive
```

Disable one or more named passes:

```bash
parakit start --disable-rule casual-gonna
parakit start \
  --disable-rule spaced-acronyms \
  --disable-rule filled-pauses
```

Stutter handling is grouped by shape. Use `stutter-safe-words` for
conservative repeated words, `repeated-prefix-stutter` for repeated letters
or `sh`/`th`/`ch` starts before a matching word, and
`stutter-ambiguous-words` for the aggressive-only `that`, `no`, `can`,
`had`, and `do` set.

## Rule Order

Passes run in the order shown by `parakit rules list`. The output of one pass is the input to the next. Aggressive discourse edits run first. Stutter handling precedes acronym collapsing. `text2num` conversion precedes structural identifier formatting. Whitespace cleanup and capitalization run near the end, followed by the default terminal-period removal.

A pass should be moved only with regression evidence. Reordering can change downstream matches even when every individual expression remains unchanged.

## Choosing An Engine

Use the standard `regex` engine for ordinary substitutions. It remains the default because its matching model is predictable and bounded.

Use `fancy-regex` only when an advanced feature materially improves the implementation. The current uses are bounded backreferences for repeated-token and repeated-prefix stutter patterns. Every `fancy-regex` pass sets an explicit backtrack limit, and a runtime limit failure is not fatal: the cleaner fails open, keeping the original transcript and recording the failure rather than inserting partially transformed text. Do not replace the conservative repeated-word vocabulary with a generic backreference: valid language such as `that that` and emphatic `no no` must survive the safe profile.

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

Increment `CLEANER_VERSION` whenever built-in behavior, rule identity, or
ordering changes so historical logs continue to identify the cleaner that
produced a transcript.

Personal vocabulary belongs in code only when it generalizes to normal dictation. Proper nouns, project-specific shorthand, and private jargon belong in `config.toml` instead. See User Rules below.

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

The key contracts, validation constraints, replacement syntax, and exact
splice points for `first`, `standard`, and `last` are in
[config_reference.toml](config_reference.toml). Use `standard` for ordinary
vocabulary substitutions.

Disable a user rule the same way as a built-in rule:

```bash
parakit start --disable-rule weights-and-biases-to-wandb
```

or in `config.toml`:

```toml
[cleaning]
disabled_rules = ["weights-and-biases-to-wandb"]
```

`parakit rules list` prints user rules in a separate `(user)` section below the built-in rule list.

### Edit Rules Without Rebuilding

User rules are runtime configuration, not compiled into the parakit binary.
They are validated and compiled during process startup, then reused for each
dictation without TOML parsing or regex compilation on the hot path. Editing
a rule does not run Cargo, rebuild Parakit, download a model, or convert model
weights.

Use the edit, validation, and restart workflow in
[configuration.md#validation-and-recovery](configuration.md#validation-and-recovery).

A built-in rule can be replaced without rebuilding by listing its name in
`cleaning.disabled_rules` and adding a differently named `[[rules.user]]`
replacement. Runtime user rules deliberately use the standard Rust `regex`
dialect. Context-sensitive procedural transforms such as number/version
parsing and sentence capitalization remain Rust code unless they expose a
specific config setting such as `cleaning.number_threshold`.

## Regression Workflow

When a cleanup worsens a transcript:

1. Capture the raw ASR output.
2. Reproduce with `parakit rules test "<raw text>"`.
3. Disable candidate passes one at a time with `--disable-rule`.
4. Narrow the pattern or add a more specific rule before the generic one.
5. Add a case to the `rules` unit tests or `tests/cleaning_regressions.rs`.

For historical JSONL replay, use
[quality.md#cleaning-corpus-replay](quality.md#cleaning-corpus-replay).

Avoid disabling broad rule categories to fix a narrow failure. If the failure is personal vocabulary rather than a general defect, add a user rule instead.
