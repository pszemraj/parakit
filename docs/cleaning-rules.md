# Cleaning Rules

parakit applies a deterministic regex cleanup pass after ASR and before text insertion. Built-in rules are code in `src/rules.rs`. Personal, non-generalizable rules can be added without editing code via `config.toml`; see [User Rules](#user-rules) below.

## What Rules Do

The default rules target common dictation artifacts:

- leading filler phrases such as `So,`, `Well,`, `Like,`;
- mid-sentence fillers such as `you know`, `I mean`, and filler `like`;
- repeated word stutters such as `the the`, `I I I`, `did did`;
- partial-word stutters such as `t t t think` and `sh sh should`;
- casual forms such as `cause` to `because`;
- whitespace and punctuation cleanup;
- dropping a single trailing period for short dictation-style utterances.

Show the active rule list:

```bash
parakit --list-rules
```

Test one input:

```bash
parakit --test-rules "So, um, the the cat ran."
```

## Rule Order

Rules run in the order they appear in `DEFAULT_RULES`. The output of one rule is the input to the next.

Specific rules should appear before generic rules. For example, a rule for `it's actually like X` must run before a broader `it's like X` rule.

Whitespace and punctuation cleanup should stay at the end.

## Disabling Rules

Disable one rule:

```bash
parakit --disable-rule lead-so-comma
```

Disable multiple rules:

```bash
parakit \
  --disable-rule lead-so-comma \
  --disable-rule fix-trailing-period
```

Disable the whole cleanup pass:

```bash
parakit --no-cleaning
```

## Adding Rules

Add a `Rule` entry to `DEFAULT_RULES`:

```rust
Rule {
    name: "weights-and-biases-to-wandb",
    description: "Map 'weights and biases' to 'wandb'",
    pattern: r"(?i)\bweights and biases\b",
    replacement: "wandb",
},
```

Rules use Rust's `regex` crate:

- no lookbehind;
- no regex backreferences such as `\1` in the pattern;
- capture replacement uses `$1`, `$2`, and so on;
- use `(?i)` for case-insensitive matches.

Personal vocabulary belongs in code when it is useful for the user, but it should not become a default rule unless it generalizes to normal dictation. Personal vocabulary that should *not* be baked into the binary — proper nouns, jargon, a `'cause`-style substitution you don't want everyone to get — belongs in `config.toml` instead. See User Rules below.

## User Rules

For rules that are personal (your vocabulary, your projects, your typing quirks) rather than generally useful, define them in `config.toml` instead of editing `src/rules.rs`. User rules run alongside the built-in rules in the same cleaning pass.

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

A broken user rule is a hard error at config load time (`parakit config show`, `parakit config edit`, or daemon startup), naming the offending rule:

```text
user rule 'my-rule' has invalid regex: regex parse error: ...
user rule 'filler-um-uh' has the same name as a built-in rule; rename it
duplicate user rule name 'my-rule'
```

## Regression Workflow

When a cleanup worsens a transcript:

1. Capture the raw ASR output.
2. Reproduce with `parakit --test-rules "<raw text>"`.
3. Disable candidate rules one at a time with `--disable-rule`.
4. Narrow the pattern or add a more specific rule before the generic one.
5. Add a unit test in `src/rules.rs`.

Avoid disabling broad rule categories to fix a narrow failure.
