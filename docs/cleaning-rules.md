# Cleaning Rules

parakit applies a deterministic regex cleanup pass after ASR and before text insertion. Rules are code in `src/rules.rs`; there is no rule config file.

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

Personal vocabulary belongs in code when it is useful for the user, but it should not become a default rule unless it generalizes to normal dictation.

## Regression Workflow

When a cleanup worsens a transcript:

1. Capture the raw ASR output.
2. Reproduce with `parakit --test-rules "<raw text>"`.
3. Disable candidate rules one at a time with `--disable-rule`.
4. Narrow the pattern or add a more specific rule before the generic one.
5. Add a unit test in `src/rules.rs`.

Avoid disabling broad rule categories to fix a narrow failure.
