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

Any run of two or more standalone uppercase ASCII letters separated by exactly one ASCII space collapses without separators: `O C R` becomes `OCR`. There is no acronym allowlist. [Stutter handling runs first](#rule-order), so `I I think` becomes `I think`.

### Numbers

English cardinal and ordinal recognition uses [`text2num`](https://crates.io/crates/text2num). Quantities, versions, years, identifiers, and sizes share one tokenization and classification pass; numeric output is never reparsed by a later numeric category. Their [controls](#numeric-controls-and-migration) are independent, with shared boundaries and protections.

#### Threshold And Adjacent Numbers

`cleaning.number_threshold` defaults to 4. Every cardinal and ordinal value at or above it renders as digits; smaller values retain the model's wording, so `zero` through `three` and `first` through `third` stay as produced. A threshold of `100` also leaves `fifty` as a word. At `0`, every eligible value converts, including a leading zero (`zero one` becomes `01`); the [protected quantities](#preserved-quantities-and-times) below still remain words.

Adjacent parsed cardinal counts connected by spaces or tabs keep their separators when all meet the threshold: `GPU fifty ninety` becomes `GPU 50 90`. If any count is below it, the entire run retains its wording, including a final size suffix: `one twenty eight K` stays words at the default and becomes `1 28K` at `0`. Ordinal modifiers remain separate (`first five hundred rows` becomes `first 500 rows`). This protection also gates compaction when `spoken-numbers` is disabled: `three 50 K` stays separate at `4` but becomes `three 50K` at `0`.

Punctuation separates values; lists and nearby labels never bypass the threshold. `Nought` is recognized as an unambiguous zero under the ordinary numeric rules. Unknown runs containing the contextual zero aliases `oh` or `o`, such as `fifty oh five`, remain intact unless they form a recognized year or version. Unsupported scientific notation (`three e minus four`) also stays words. [Years](#calendar-years) and [versions](#versions-and-identifiers) use their own rendering rules.

#### Magnitudes And Parameter Counts

An explicit quantity ending in one `million` or `billion` scale keeps the scale word (`three billion` becomes `3 billion`). Bare scales, with or without an article, remain words. A compound with an explicit remainder can consume its article: `a thousand five hundred` becomes `1500`.

A decimal coefficient keeps its scale words and spoken precision (`three point zero million` becomes `3.0 million`), and a count after those words starts another number. Unambiguous non-decimal compounds with multiple scales use their full-digit value. Existing digit-plus-scale forms such as `5 hundred` retain their value and wording instead of being reparsed as two numbers.

After a scale word, `and` separates quantities when the next has the same largest scale (`five thousand and ten thousand five hundred` becomes `5000 and 10500`); otherwise it continues the number (`one hundred and five thousand` becomes `105000`). After a count, it always separates: `twenty and five` becomes `20 and 5`.

A scale takes the immediately preceding count as its coefficient (`two hundred three hundred` becomes `200 300`). Ambiguous coefficient boundaries or stacked scales remain words, as in `one hundred twenty three hundred` and `fifteen hundred thousand`.

`size-suffixes` compacts written counts and explicit `thousand`/`million`/`billion` quantities to exact `K`/`M`/`B` notation; `trillion` retains its wording. An explicit uppercase suffix binds only the last eligible count (`2.5 seven B` becomes `2.5 7B`), including its scale (`a hundred K` becomes `100K`). Independent counts never concatenate, protected mixed runs stay intact, and digit-by-digit decimals remain one expression (`two point five seven B` becomes `2.57B`). Disable this control for the general quantity rendering.

Spoken cardinal counts require `spoken-numbers`: disabling it keeps `a seven B model` as words, while existing `7 B` can still become `7B`. Decimals rendered by an enabled `spoken-versions` pass also remain eligible.

A matching scale and suffix drop the redundant scale (`2.5 million M` becomes `2.5M`). Mismatched scales fold only for integer coefficients when every scale is smaller than the suffix (`5 hundred K` becomes `500K`). Decimal coefficients or a scale at least as large as a mismatching suffix stay spaced, preserving `2.2 hundred K` and `2.5 million B`.

#### Preserved Quantities And Times

At every threshold and in both profiles, indefinite quantities preserve wording, capitalization, and adjacent punctuation. These include plural magnitudes (`tens` through `trillions`), qualified scale compounds such as `a few hundred-thousand`, and fractions with numerators, articles, `of`, or a mixed-number `and`: `nineteen hundreds`, `half a million`, `a third of a million`, and `two and a half million` stay words. A zero coefficient before a scale also remains words, including any additive remainder.

Fractions before ordinary nouns are protected too (`five hundredths of a second`), while genuine ordinals follow the threshold (`the five hundredth customer` becomes `the 500th customer`).

A separate exact quantity before an indefinite phrase still follows the threshold (`year two thousand a few hundred` becomes `year 2000 a few hundred`). A count joined to a fraction by `and` stays with it, including after `both` or `between`. Without punctuation, a following article-plus-scale phrase can belong to the fraction even after an ordinal; a comma separates it: `six quarters a million` stays intact, but `six quarters, a million` becomes `6 quarters, a million`. Other enabled cleanup still applies.

`Second` defaults to a time unit after a number, including before a noun (`the thirty second timeout` becomes `the 30 second timeout`). `One second`, `the fifth second`, `per second`, `split-second`, and elapsed plural `seconds` retain the unit. A compound ordinal is selected by a following `of` or supported calendar word (`day`, `week`, `month`, `year`, `decade`, `century`, `millennium`, or their plurals), preceding month, or phrase-ending punctuation/end: `May twenty second` becomes `May 22nd`, and `the twenty second century` becomes `the 22nd century`. Before the entire numeric phrase, `every`, `each`, `per`, `a`, or `an` selects a duration even at phrase end, unless a stronger `of`, calendar-word, or month cue applies. A definite article alone does not select the ordinal. Genuine ordinals, including lists, follow the threshold. Clock expressions have no separate threshold bypass: `five o'clock` becomes `5 o'clock`, while `two o'clock` stays words at the default.

#### Calendar Years

`Eighteen`, `nineteen`, or `twenty` followed by a two-digit count, or by a zero alias and a digit, renders as one year without cue words: `twenty twenty six` becomes `2026`, and `nineteen oh five` becomes `1905`.

A pair needs at least one spoken component; existing `20 22` stays separate. Pairs inside longer adjacent numeric runs also stay separate. A preceding ordinal day or joined identifier starts a separate run.

A following whole-word time or measurement unit, case-insensitive unit abbreviation (`GB`, `ms`, `Hz`), uppercase `AM`, case-insensitive `pm`, dotted `a.m.`/`p.m.`, or uppercase `K`/`M`/`B` blocks joining; ordinary adjacent-count handling applies instead. Single-letter units, unrelated uppercase abbreviations (`TV`), and prose words (`in`, `a`, `am`, `us`) do not block it. Each pair is classified independently, so `in twenty twenty five and twenty twenty` becomes `in 2025 and 2020`.

Date days, identifier counts, and scale-based years use the ordinary threshold: `April first, twenty twenty four` becomes `April first, 2024`, while `two thousand twenty two` becomes `2022` at the default.

#### Versions And Identifiers

Multi-point versions render component by component with periods, regardless of the prose threshold or a version cue: `Ubuntu twenty two point zero four point three` becomes `Ubuntu 22.04.3`. Digit-by-digit components retain their digits, including leading zeros; `two point zero three point zero` becomes `2.03.0`, and a single decimal such as `three point one four one five` becomes `3.1415`. Punctuation separates decimal values (`two point zero, three point zero` becomes `2.0, 3.0`).

A base spoken digit by digit is joined only in an IPv4-shaped run with at least three `point` separators, such as `one nine two point one six eight point one point one` becoming `192.168.1.1`. Other version bases use ordinary cardinal parsing, preserving the boundary between adjacent independent counts.

Numeric prefixes use the same component parsing (`Version 2 point zero five` becomes `Version 2.05`). A following scale can start a separate coefficient (`2 point five two thousand` becomes `2.5 2000`). Standalone `V` or `v` joins a dotted version as lowercase `v`.

`numeric-identifiers` joins an uppercase single letter other than `A` or `I`, or `SM`, to one complete numeric run. That run may be existing digits or output eligible for `spoken-numbers` or `spoken-versions`; there is no threshold bypass. At the default, `Q five` becomes `Q5`, `Q two` stays words, and `SM eight nine` becomes `SM 8 9`. Other two-letter stems do not join (`FP thirty two` becomes `FP 32`). A letter already bound to another conversion, such as a size suffix, is not reused. With `spoken-numbers` disabled, existing `Q 2` can still become `Q2`.

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

Safe casual-form normalization expands `gonna`/`gunna` to `going to`, `gotta` to `got to`, `wanna`/`wana` to `want to`, `kinda` to `kind of`, and `gimme` to `give me`. The complete tag question `, right?` becomes a period; ordinary questions such as `Is that right?` stay unchanged. The [terminal-period setting](#profiles) applies to the replacement.

Dangling-connective removal drops final `so`, `but`, `and`, `or`, or `because` case-insensitively only when punctuation remains underneath. It preserves `.`, `!`, and `?`, promotes `,`, `;`, or `:` to `.`, and can remove a chain (`It works. And so.` becomes `It works.` before terminal-period handling). Without that boundary, `I like cats and dogs and` stays unchanged because the unfinished thought is ambiguous.

`stutter-safe-words` collapses adjacent identical words case-insensitively, keeping the first spelling (`where where` becomes `where`). A pronoun restarted as its contraction keeps the suffix (`it it's` becomes `it's`). Only spaces and tabs join a run; punctuation and line breaks preserve repetitions. Numbers, spelling letters other than `I`/`a`, and the built-in exceptions remain intact. Exceptions include common idioms and intensifiers (`hee`, `bang`, `poo`, `very`) and the symbol words `plus`, `dash`, `equals`, `slash`, `dot`, `star`, `underscore`, `colon`, and `pipe`. Other symbol names follow ordinary repetition handling. The complete list lives beside the scanner in `src/rules/stutters.rs`.

Recognized grammatical contexts also preserve distinct uses of the same word, such as `get to control control T` and `calling something that isn't science science`. Add your own words or literal phrases with [`cleaning.repetition_exceptions`](configuration.md#preserving-repeated-words); these supplement the built-in list.

Repeated `I` and `a` retain their existing treatment, including uppercase-I normalization. Repeated `is` collapses in normal mode. The aggressive profile additionally collapses `that`, `had`, `do`, `can`, and `no`; both profiles preserve punctuation-delimited forms such as `No, no`. Add a repetition exception, use punctuation, or disable the rule to retain other deliberate repetitions such as `tomato tomato`.

## Inspect And Test

`rules list` shows every pass, its engine, tier, and activation for the selected options. `rules test` applies the configured threshold and reports each pass that changed its input; listing does not apply that threshold to text.

```bash
parakit rules list
parakit rules list --profile aggressive --keep-trailing-period
parakit rules test "um, the the G P T model is gonna work."
parakit rules test "So, it's like, you know, hard." --profile aggressive
```

The profile flag is `--profile` here and `--cleaning-profile` on `start`. Trailing-period and repeatable disable-rule flags work on both commands. To disable passes for a daemon run:

```bash
parakit start \
  --disable-rule spaced-acronyms \
  --disable-rule filled-pauses
```

Set the same names persistently with [`cleaning.disabled_rules`](config_reference.toml). Both rules commands inspect or apply cleaning even when `cleaning.enabled = false`.

Stutter handling is grouped by shape. Use `stutter-i` for repeated first-person pronouns, `stutter-safe-words` for whole-word restarts, `contracted-have-restart` for false starts such as `I've I already have`, `repeated-prefix-stutter` for repeated letters or `sh`/`th`/`ch` starts before a matching word, and `stutter-ambiguous-words` for the aggressive-only set.

## Rule Order

Built-ins run in the order shown by `rules list`; user rules occupy their configured slots even though the command prints them together in a trailing `(user)` section. Each pass receives the previous pass's output. Aggressive discourse edits and filled-pause removal run first, then partial-word restarts, whole-word collapse, acronyms, and numbers. Thus `sh sh should` becomes `should`, and removing a pause in `twenty uh twenty layers` can create the year `2020`. The [numeric group](#numeric-controls-and-migration) reports hits in metadata order. Whitespace cleanup and capitalization run near the end, followed by terminal-period removal.

A pass should be moved only with regression evidence. Reordering can change downstream matches even when every individual expression remains unchanged.

## Choosing An Engine

Use the standard `regex` engine for ordinary substitutions. It remains the default because its matching model is predictable and bounded.

Use `fancy-regex` only when an advanced feature materially improves the implementation, as in the [bounded stutter rules](#casual-forms-and-stutters). Every `fancy-regex` pass sets an explicit backtrack limit. If matching exceeds it, the cleaner keeps the original transcript and records the failure instead of inserting partially transformed text.

Use a procedural pass when correctness depends on token classes or longer context. Sentence capitalization, spaced-letter collapsing, multi-point version assembly, and identifier formatting are procedural because they enforce structural invariants that are clearer in Rust than in a monolithic expression.

Every pass must have a stable name, explicit activation tier, tests, and rule-hit attribution. Reducing source declarations is useful only when observability and precision are preserved.

## Adding A Built-In Rule

Add an entry to `DEFAULT_RULES` in `src/rules/defaults.rs` using the `regex_rule!`, `fancy_rule!`, or `procedural_rule!` macro, and pick an activation tier: `Activation::Safe` for mechanical or high-confidence work, `Activation::Aggressive` for anything that can change meaning.

The five numeric categories use raw `Rule` literals with `RuleKind::Numbers(NumberKind)`. Keep them contiguous so the engine can invoke their shared numeric core once with the enabled categories.

Standard patterns and replacements use the same [Rust regex syntax documented for user rules](config_reference.toml); use the bounded `fancy-regex` path only when advanced matching is required.

Increment `CLEANER_VERSION` whenever built-in behavior, rule identity, or ordering changes so historical logs continue to identify the cleaner that produced a transcript.

## User Rules

For personal vocabulary or typing quirks, define rules in `config.toml` instead of editing Rust. User rules run alongside the built-ins and are not filtered by the profile.

Run `parakit config init` to create a config file with a commented template, then add a `[[rules.user]]` table per rule:

```toml
[[rules.user]]
name = "weights-and-biases-to-wandb"
pattern = "(?i)\\bweights and biases\\b"
replacement = "wandb"
```

The [reference](config_reference.toml) defines optional descriptions, validation, replacement syntax, and the exact splice points for `first`, `standard`, and `last`. The default `standard` position fits ordinary vocabulary substitutions.

Disable user rules with the same [named-pass controls](#inspect-and-test) as built-in rules.

### Edit Rules Without Rebuilding

Follow the [edit, validation, and restart workflow](configuration.md#validation-and-recovery). User rules need no Cargo build, model download, or conversion.

A built-in rule can be replaced without rebuilding by listing its name in `cleaning.disabled_rules` and adding a differently named `[[rules.user]]` replacement. Runtime user rules deliberately use the standard Rust `regex` dialect. Context-sensitive procedural transforms such as number/version parsing and sentence capitalization remain Rust code unless they expose a specific config setting such as `cleaning.number_threshold`.

## Regression Workflow

When a cleanup worsens a transcript:

1. Capture the raw ASR output.
2. Reproduce with `parakit rules test "<raw text>"`.
3. Disable candidate passes one at a time with `--disable-rule`.
4. Fix the parsing or matching rule, checking related inputs that should remain unchanged.
5. Add a case to the `rules` unit tests or `tests/cleaning_regressions.rs`.

For historical JSONL replay, use [dev/quality.md#cleaning-corpus-replay](dev/quality.md#cleaning-corpus-replay).

Avoid disabling broad rule categories to fix a narrow failure. If the failure is personal vocabulary rather than a general defect, add a user rule instead.
