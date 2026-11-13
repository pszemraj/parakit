# Cleaning Rules

parakit applies a typed, deterministic cleanup pipeline after ASR and before text insertion. Built-in passes use the [engines described below](#choosing-an-engine).

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

A single terminal period is removed by default because parakit primarily targets messaging-style dictation. A final `?`, `!`, or ellipsis is kept. Preserve the period when the target context expects conventional prose punctuation:

```bash
parakit start --keep-trailing-period
```

These settings also live in `config.toml` as `cleaning.profile` and `cleaning.keep_trailing_period`. Number conversion has the config-only `cleaning.number_threshold` setting. The exact CLI/config merge behavior is in the per-key [config reference](config_reference.toml).

Cleaning includes English rules. For multilingual dictation, disable built-in and user cleaning together with `[cleaning] enabled = false` or:

```bash
parakit start --no-cleaning
```

## General Invariants

The default pipeline favors structural rules over product- or vocabulary-specific substitutions.

### Acronyms

Spaced acronyms follow one invariant: a run of two or more standalone uppercase ASCII letters separated by exactly one ASCII space is collapsed without separators. For example, `O C R` becomes `OCR`, `R L H F` becomes `RLHF`, and `A B` becomes `AB`. There is no acronym allowlist, so an unfamiliar initialism collapses exactly like a familiar one. Repeated-word stutter handling runs before acronym collapsing, so `I I think` still becomes `I think` rather than `II think`.

### Numbers

English cardinal and ordinal recognition uses [`text2num`](https://crates.io/crates/text2num). After stutter handling and acronym collapsing, the numeric core tokenizes its input once, classifies its parsed spans, and renders accepted edits without parsing earlier numeric output. Quantities, versions, calendar years, identifiers, and sizes share those token boundaries. Filled pauses are removed before stutter handling, so `twenty uh twenty layers` can become `2020 layers` under the calendar-year rule. Fractions and indefinite quantities retain their wording; a shared unit vocabulary prevents measurements from being joined as years.

#### Threshold And Adjacent Numbers

The number threshold is 4 by default and applies to every cardinal and ordinal value. A value at or above it renders as digits; one below it retains the model's wording. At the default, `zero` through `three` and `first` through `third` stay as produced. A threshold of `100`, for example, also leaves `fifty` as a word. Set `cleaning.number_threshold = 0` to convert every eligible value. Plural magnitudes, fractions, bare scale words, and the time unit `second` remain protected at `0`. The threshold also gates mixed-run compaction when `spoken-numbers` is disabled: `three 50 K` stays separate at `4` but becomes `three 50K` at `0`. Any enabled numeric pass includes the threshold in `ruleset_id`; with all numeric passes disabled, the threshold is irrelevant.

Adjacent parsed counts keep their separators when they all meet the threshold: `fifty ninety` becomes `50 90`, `five thirty` becomes `5 30`, and `five five five twenty twelve` becomes `5 5 5 20 12`. Nearby labels do not combine them: `GPU fifty ninety` becomes `GPU 50 90`.

Leading zeros are part of a single parsed number: at threshold `0`, `zero one` becomes `01`.

If a run of directly adjacent spoken cardinal counts contains any value below the threshold, the entire run retains its original wording, including a final size suffix. Only spaces or tabs connect such a run. At the default, `three twenty billion`, `three fifty K`, `one twenty eight K`, `SM one twenty`, `two thirty`, `Llama three seventy B`, and `sixteen three eighty four` stay as spoken. At threshold `0`, these runs render each parsed count separately instead of joining their values: `one twenty eight K` becomes `1 28K`. Ordinal modifiers remain separate: `first five hundred rows` becomes `first 500 rows` at the default.

Punctuation separates independent values, and lists do not bypass the threshold: `zero, one, two, three, four` becomes `zero, one, two, three, 4` at the default. Unknown runs containing the contextual zero aliases `oh`, `o`, or `nought`, such as `fifty oh five`, stay intact unless they form a recognized year or version. Unsupported scientific notation such as `three e minus four` also retains its wording. The dedicated year and version structures below retain their separate rendering rules.

#### Magnitudes And Parameter Counts

For readability, a recognized quantity ending in one explicit `million` or `billion` scale keeps that word with a numeric coefficient: `three billion` becomes `3 billion`, `one million` becomes `1 million`, and `three point five billion` becomes `3.5 billion`. Bare scales stay words, including `million`, `a million`, `a thousand`, and `a hundred`. A compound with an explicit remainder can consume its article: `a thousand five hundred` becomes `1500`.

A decimal coefficient keeps the scale words after it and its spoken precision: `two point five thousand` becomes `2.5 thousand`, `one point five hundred` becomes `1.5 hundred`, and `three point zero million` becomes `3.0 million`. A count after those scale words starts another number: `two point five million three point six million` becomes `2.5 million 3.6 million`. Multi-scale compounds such as `three billion five hundred million` retain the unambiguous full-digit rendering.

Existing digit-plus-scale forms retain their value and wording: `5 hundred`, `5 thousand`, `5 million`, and `5 billion` are not reparsed as two separate numbers.

An `and` after a scale word separates two quantities when the one that follows has that same largest scale: `between five thousand and ten thousand five hundred` becomes `between 5000 and 10500`, and `two point five million and three point six million` becomes `2.5 million and 3.6 million`. Otherwise it continues the number: `one hundred and five thousand` becomes `105000`, and `one hundred and twenty thousand five hundred` becomes `120500`. An `and` after a count never continues it, so `between two hundred and fifty and three hundred` becomes `between 250 and 300`, and `twenty and five` becomes `20 and 5`.

A scale word takes the count spoken directly before it as its coefficient instead of rendering as a bare `100` or `1000`: `two hundred three hundred` becomes `200 300`, and `fifteen hundred and two thousand five hundred` becomes `1500 and 2500`. When more than one count could be that coefficient, or the scales could stack, the words stay: `one hundred twenty three hundred` (`100 2300` or `120 300`) and `fifteen hundred thousand` remain words.

Parameter-count compaction supports written numeric counts and explicit quantities using `thousand`, `million`, or `billion`, with exact `K`/`M`/`B` notation. The word `trillion` retains its wording. Disable `size-suffixes` to retain the general quantity rendering instead.

An explicit uppercase `K`, `M`, or `B` binds only the last parsed count, provided spoken conversion is eligible: `2.5 seven B` becomes `2.5 7B`, and `thirteen seventy B` becomes `13 70B`. A scale word directly before the suffix also renders as a size: `a hundred K` becomes `100K`. It does not concatenate independent numbers or partly convert a protected mixed run such as `three seventy B`. A digit-by-digit decimal remains one expression: `two point five seven B` becomes `2.57B`. Existing numeric values follow the same boundary rule, so `2.5 7 B` becomes `2.5 7B`.

A scale word can be dropped only when it agrees with the suffix. Otherwise the full quantity precedes the suffix: `5 hundred K` becomes `500K`, and `two point five million B` becomes `2500000B`.

#### Preserved Quantities And Times

Indefinite quantities remain words. This includes plural magnitudes such as `hundreds`, `thousands`, `millions`, and `billions` (also `tens` and `trillions`); qualified compounds such as `a few hundred thousand` and `a few hundred-thousand`; and fractions such as `half a million`, `quarter of a billion`, `three quarters of a million`, `a third of a million`, `a million and a half`, and `two and a half million`. A directly preceding number-word phrase remains part of a plural magnitude or actual fraction numerator, so `nineteen hundreds`, `five millions`, `one half million`, `five thirds of a million`, and `four fifths of a million` retain their wording, capitalization, and adjacent punctuation at every threshold and in both profiles.

Fractions before ordinary nouns are protected too: `five hundredths of a second`, `three quarters of a dollar`, `a fifth of a dollar`, and `one hundredth of a second` keep their wording. Singular ordinals remain numeric where the threshold permits, such as `the five hundredth customer` becoming `the 500th customer` and `the fifth of May` becoming `the 5th of May`.

A separate exact quantity before an indefinite phrase still follows the threshold: at the default, `year two thousand a few hundred` becomes `year 2000 a few hundred`, and `year two thousand and five half a million` becomes `year 2005 half a million`. The same applies to a separate count: `chapter five half a million words long` becomes `chapter 5 half a million words long`. Exact quantities also convert normally at the default threshold: `three hundred records` becomes `300 records`, and `five million rows` becomes `5 million rows`. Other enabled cleanup rules, including sentence capitalization and trailing-period handling, continue to apply.

Ambiguous quantities keep their wording. A count joined to a fraction by `and` stays with it even after `both` or `between`: `between two hundred and half a million` remains words. Without punctuation, `half a million` and `quarter a million` remain protected even after an ordinal, and `six quarters a million` stays intact. A comma separates an independent count: `six quarters, a million` becomes `6 quarters, a million`, retaining the bare article-plus-scale phrase.

The ambiguous word `second` defaults to a time unit after a number: `a thirty second timeout` becomes `a 30 second timeout`, while `one second`, `the fifth second`, `per second`, and `a split-second` retain the unit word. For compound ordinals, a following `of` or calendar division, preceding month, or the end of a phrase identifies the ordinal reading: `the twenty second of May` becomes `the 22nd of May`, `May twenty second` becomes `May 22nd`, and `the twenty second century` becomes `the 22nd century`. A definite article alone does not change a duration: `the thirty second timeout` remains `the 30 second timeout`. Other genuine ordinals follow the threshold, including in lists. Plural elapsed `seconds` also stays a time unit: `ten seconds of a million rows` becomes `10 seconds of a million rows` at the default. Clock expressions have no separate preservation rule: `five o'clock` becomes `5 o'clock`, while `two o'clock` keeps its wording at the default threshold.

#### Calendar Years

`Eighteen`, `nineteen`, or `twenty` followed by a two-digit count, or by `oh`/`o` and a digit, renders as one year without surrounding cue words: `twenty twenty six` becomes `2026`, `nineteen oh five` becomes `1905`, and `Lord, twenty twenty six` becomes `Lord, 2026`.

A pair needs at least one spoken component; mixed word/digit forms can join, while existing `20 22` stays separate. A pair inside a longer adjacent numeric run also stays separate, so `five five five twenty twelve` becomes `5 5 5 20 12` at the default threshold. A preceding ordinal day or joined identifier is separate from that run.

A following whole-word time or measurement unit, case-insensitive unit abbreviation such as `GB`, `ms`, or `Hz`, uppercase `AM`, case-insensitive `pm`, dotted `a.m.`/`p.m.`, or uppercase `K`/`M`/`B` blocks year joining. The ordinary adjacent-count policy then applies: `in twenty thirty minutes` becomes `in 20 30 minutes`. Single-letter units and unrelated uppercase abbreviations such as `TV` do not block a year; neither do prose words such as `in`, `a`, `am`, and `us`. `in twenty twenty five and twenty twenty` becomes `in 2025 and 2020` because each pair independently has the year shape. This also means `from nineteen thirty until twenty fifteen` becomes `from 1930 until 2015`.

Date days follow the ordinary threshold: `April first, twenty twenty four` becomes `April first, 2024`, and `May twenty sixth` becomes `May 26th`. At the default, `Q two of twenty twenty six` becomes `Q two of 2026`. An explicit scale-based year such as `two thousand twenty two` is an ordinary quantity and becomes `2022` at the default threshold.

#### Versions And Identifiers

Multi-point versions are parsed component by component through `text2num` and rendered with periods regardless of the prose threshold or an explicit version cue. `Ubuntu twenty two point zero four point three` becomes `Ubuntu 22.04.3`, and `Python three point one two point four` becomes `Python 3.12.4`. Without punctuation, `two point zero three point zero` becomes `2.03.0`; a comma separates decimal values (`two point zero, three point zero` becomes `2.0, 3.0`). Single decimals spoken digit by digit retain their digits (`three point one four one five` becomes `3.1415`).

A base spoken digit by digit is joined only in an IPv4-shaped run with at least three `point` separators, such as `one nine two point one six eight point one point one` becoming `192.168.1.1`. Other version bases use ordinary cardinal parsing, preserving the boundary between adjacent independent counts.

The same component parsing applies to numeric prefixes: `Version 2 point twenty five` becomes `Version 2.25`, and `Version 2 point zero five` becomes `Version 2.05`. A following scale can start a separate coefficient (`2 point five two thousand` becomes `2.5 2000`). A standalone `V` or `v` before a dotted version is joined as a lowercase `v` prefix.

`numeric-identifiers` joins an uppercase single letter other than `A` or `I`, or the stem `SM`, to a number already written as digits or rendered by `spoken-numbers` or `spoken-versions`. It grants no threshold bypass: at the default, `Q five` becomes `Q5` and `F thirty two` becomes `F32`, while `Q two`, `plan B two weeks`, and `vitamin D three times a day` keep their spoken counts. Other two-letter stems do not join, so `FP thirty two` becomes `FP 32`. The number must be the whole run: `SM eight nine` becomes `SM 8 9`. The mixed-run protection also keeps `SM one twenty` intact at the default threshold.

This compaction can be used independently on existing numeric text: with `spoken-numbers` disabled, `numeric-identifiers` can still turn `Q 2` into `Q2`.

#### Numeric Controls And Migration

The five numeric controls are independently disabled through `cleaning.disabled_rules` or `--disable-rule`. They share one classification/rendering group and report separate rule hits:

| Control | Rendering |
| --- | --- |
| `spoken-numbers` | Ordinary cardinal and ordinal quantities, using `cleaning.number_threshold`. |
| `spoken-versions` | Spoken decimal components, dotted versions, numeric point suffixes, and `v` prefixes. |
| `calendar-years` | Century pairs beginning with 18, 19, or 20, except before a unit or AM/PM/size marker. |
| `numeric-identifiers` | A single uppercase letter other than A/I, or SM, joined to an existing or threshold-rendered number. |
| `size-suffixes` | Explicit `K`/`M`/`B` suffixes and compact parameter counts. |

The former eight numeric passes have been replaced by these five controls. Update existing disabled-rule settings using this mapping; removed names have no aliases and fail configuration validation:

| Previous control | Current control |
| --- | --- |
| `spoken-numbers` | `spoken-numbers` for quantities; `spoken-versions` for decimals. Disable the new `calendar-years` control too to retain separate century-pair counts. |
| `spoken-version-numbers`, `numeric-point-suffixes`, `compact-version-prefix` | `spoken-versions` |
| `compact-short-identifier` | `numeric-identifiers` |
| `numeric-identifier-groups` | Removed; adjacent digit groups remain separate. |
| `compact-parameter-counts`, `numeric-magnitude-suffixes` | `size-suffixes` |

Disabling `spoken-numbers` leaves the other four categories available. After updating the config, run `parakit config show`, inspect `parakit rules list`, and check representative text with `parakit rules test`. Restart the daemon to load the new settings.

### Casual Forms And Stutters

Safe casual-form normalization expands `gonna` and `gunna` to `going to`, `gotta` to `got to`, `wanna` and `wana` to `want to`, `kinda` to `kind of`, and `gimme` to `give me`. The complete tag question `, right?` becomes a period, while ordinary questions such as `Did I turn right?` and `Is that right?` remain unchanged. When the replacement is the dictation's final character, the default terminal-period pass removes it unless `--keep-trailing-period` is set.

Safe dangling-connective removal drops a trailing `so`, `but`, `and`, `or`, or `because` (case-insensitive) when it is the transcript's final word and the text underneath still ends in sentence punctuation, so `The build is green. But` becomes `The build is green.`, which the default terminal-period pass then trims to `The build is green` the same way it trims `, right?`. A trailing `,`, `;`, or `:` left under the removed word is promoted to `.`, a trailing `.`, `!`, or `?` is kept as-is, and the rule chains, so `It works. And so.` loses both dangling words and becomes `It works.`. It needs punctuation to fall back on: `I like cats and dogs and` has no preceding sentence punctuation, so the rule cannot distinguish a stranded conjunction from an unfinished thought and leaves the text alone.

The safe `stutter-safe-words` pass runs before acronym collapsing and numeric rendering. It collapses adjacent identical words, ignoring case and keeping the first word's spelling: `where where` becomes `where`, and `I'm I'm` becomes `I'm`. A pronoun restarted as its own contraction joins to the contraction suffix: `it it's` becomes `it's`. Only spaces and tabs join a run; punctuation and line breaks preserve the repetition. Numbers, spelling letters other than `I` and `a`, and the built-in repetition exceptions are protected. The supported spoken-symbol words are `plus`, `dash`, `equals`, `slash`, `dot`, `star`, `underscore`, `colon`, and `pipe`; other symbol names follow the ordinary repeated-word rule. The built-ins also preserve common repetition idioms and intensifiers, including `hee`, `bang`, and `poo`. For example, `five five five` renders as `5 5 5` without losing a repetition. `C plus plus` and `very very` stay intact. The short exception list lives beside the scanner in `src/rules/stutters.rs`.

Recognized grammatical contexts also preserve distinct uses of the same word, such as `get to control control T` and `calling something that isn't science science`. Add your own words or literal phrases with [`cleaning.repetition_exceptions`](configuration.md#preserving-repeated-words); these supplement the built-in list.

Repeated `I` and `a` retain their existing treatment, including uppercase-I normalization. Repeated `is` collapses in normal mode. The aggressive profile additionally collapses `that`, `had`, `do`, `can`, and `no`; both profiles preserve punctuation-delimited forms such as `No, no`. Add a repetition exception, use punctuation, or disable the rule to retain other deliberate repetitions such as `tomato tomato`.

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

The same names can be set persistently:

```toml
[cleaning]
disabled_rules = ["filled-pauses"]
```

Stutter handling is grouped by shape. Use `stutter-i` for repeated first-person pronouns, `stutter-safe-words` for whole-word restarts, `contracted-have-restart` for false starts such as `I've I already have`, `repeated-prefix-stutter` for repeated letters or `sh`/`th`/`ch` starts before a matching word, and `stutter-ambiguous-words` for the aggressive-only set.

## Rule Order

Built-in passes run in the order shown by the built-in section of `parakit rules list`. User rules run in their configured position slot even though the command prints them together in a trailing `(user)` section. The output of one pass is the input to the next. Aggressive discourse edits run first. Partial-word restarts precede whole-word collapse so `sh sh should` becomes `should`. [Stutter handling](#casual-forms-and-stutters) precedes acronym collapsing. The [numeric group](#numeric-controls-and-migration) reports hits in metadata order. Whitespace cleanup and capitalization run near the end, followed by the default terminal-period removal.

A pass should be moved only with regression evidence. Reordering can change downstream matches even when every individual expression remains unchanged.

## Choosing An Engine

Use the standard `regex` engine for ordinary substitutions. It remains the default because its matching model is predictable and bounded.

Use `fancy-regex` only when an advanced feature materially improves the implementation, as in the [bounded stutter rules](#casual-forms-and-stutters). Every `fancy-regex` pass sets an explicit backtrack limit. If matching exceeds it, the cleaner keeps the original transcript and records the failure instead of inserting partially transformed text.

Use a procedural pass when correctness depends on token classes or longer context. Sentence capitalization, spaced-letter collapsing, multi-point version assembly, and identifier formatting are procedural because they enforce structural invariants that are clearer in Rust than in a monolithic expression.

Every pass must have a stable name, explicit activation tier, tests, and rule-hit attribution. Reducing source declarations is useful only when observability and precision are preserved.

## Adding A Built-In Rule

Add an entry to `DEFAULT_RULES` in `src/rules/defaults.rs` using the `regex_rule!`, `fancy_rule!`, or `procedural_rule!` macro, and pick an activation tier: `Activation::Safe` for mechanical or high-confidence work, `Activation::Aggressive` for anything that can change meaning.

The five numeric categories use raw `Rule` literals with `RuleKind::Numbers(NumberKind)`. Keep them contiguous so the engine can invoke their shared numeric core once with the enabled categories.

Rules use Rust's `regex` crate dialect:

- no lookaround;
- no regex backreferences such as `\1` in a standard `regex` pattern;
- capture replacement uses `$1`, `$2`, and so on;
- use `(?i)` for case-insensitive matches.

Increment `CLEANER_VERSION` whenever built-in behavior, rule identity, or ordering changes so historical logs continue to identify the cleaner that produced a transcript.

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

The key contracts, validation constraints, replacement syntax, and exact splice points for `first`, `standard`, and `last` are in [config_reference.toml](config_reference.toml). Use `standard` for ordinary vocabulary substitutions.

Disable user rules with the same [named-pass controls](#inspect-and-test) as built-in rules.

### Edit Rules Without Rebuilding

User rules are runtime configuration, not compiled into the parakit binary. Follow the [edit, validation, and restart workflow](configuration.md#validation-and-recovery); no Cargo build, model download, or conversion is needed.

A built-in rule can be replaced without rebuilding by listing its name in `cleaning.disabled_rules` and adding a differently named `[[rules.user]]` replacement. Runtime user rules deliberately use the standard Rust `regex` dialect. Context-sensitive procedural transforms such as number/version parsing and sentence capitalization remain Rust code unless they expose a specific config setting such as `cleaning.number_threshold`.

## Regression Workflow

When a cleanup worsens a transcript:

1. Capture the raw ASR output.
2. Reproduce with `parakit rules test "<raw text>"`.
3. Disable candidate passes one at a time with `--disable-rule`.
4. Narrow the pattern or add a more specific rule before the generic one.
5. Add a case to the `rules` unit tests or `tests/cleaning_regressions.rs`.

For historical JSONL replay, use [dev/quality.md#cleaning-corpus-replay](dev/quality.md#cleaning-corpus-replay).

Avoid disabling broad rule categories to fix a narrow failure. If the failure is personal vocabulary rather than a general defect, add a user rule instead.
