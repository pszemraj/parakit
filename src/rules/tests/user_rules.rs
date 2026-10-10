//! Cleaning-pipeline tests for user-rule ordering, ruleset ids, and validation errors.

use super::*;

#[test]
fn user_rule_ruleset_id_is_position_sensitive_and_description_insensitive() {
    let first_rule = UserRule {
        description: Some("first description".to_string()),
        ..user_rule("custom-hello", "hello", "hi", RulePosition::First)
    };
    let same_behavior = UserRule {
        description: Some("rewritten description".to_string()),
        ..first_rule.clone()
    };
    let last_rule = UserRule {
        position: RulePosition::Last,
        ..first_rule.clone()
    };

    let first =
        build_cleaner_for_test(CleaningProfile::Safe, false, &HashSet::new(), &[first_rule]);
    let described_differently = build_cleaner_for_test(
        CleaningProfile::Safe,
        false,
        &HashSet::new(),
        &[same_behavior],
    );
    let last = build_cleaner_for_test(CleaningProfile::Safe, false, &HashSet::new(), &[last_rule]);

    assert_eq!(first.ruleset_id(), described_differently.ruleset_id());
    assert_ne!(first.ruleset_id(), last.ruleset_id());
}

/// One row of the [`RulePosition`] Level-3 exhaustive driver: the cleaner
/// config and single user rule that pins down where that position splices
/// relative to the built-in rule list, plus the input/output pair that
/// proves it.
struct PositionCase {
    profile: CleaningProfile,
    drop_trailing_period: bool,
    rule: UserRule,
    input: &'static str,
    expected: &'static str,
}

fn position_case(position: RulePosition) -> PositionCase {
    match position {
        // adding a RulePosition variant fails compilation here until its
        // ordering expectation is stated
        RulePosition::First => PositionCase {
            profile: CleaningProfile::Aggressive,
            drop_trailing_period: false,
            // A `First` user rule expands "xyz" to "So, hello" *before* any
            // built-in rule runs, so the built-in `lead-discourse-comma` rule
            // (which only fires at sentence start, and only under the
            // Aggressive profile in the merged engine) still strips the
            // "So," it produced.
            rule: user_rule("expand-xyz", r"(?i)^xyz$", "So, hello", RulePosition::First),
            input: "xyz",
            expected: "Hello",
        },
        RulePosition::Standard => PositionCase {
            profile: CleaningProfile::Safe,
            drop_trailing_period: false,
            // A `Standard` user rule introduces messy whitespace and a stray
            // space before a comma; it must run before the built-in
            // `fix-collapse-spaces` / `fix-space-before-punct` cleanup rules
            // (both Safe, unconditional) for the output to come out clean.
            rule: user_rule(
                "expand-brb",
                r"(?i)\bbrb\b",
                "be right   back ,",
                RulePosition::Standard,
            ),
            input: "brb",
            expected: "Be right back,",
        },
        RulePosition::Last => PositionCase {
            profile: CleaningProfile::Safe,
            drop_trailing_period: true,
            // A `Last` user rule appends a trailing period *after* the
            // built-in `fix-trailing-period` rule (enabled here via
            // `drop_trailing_period = true`) has already run, so the period
            // this rule adds is not stripped.
            //
            // Unlike the pre-merge engine, `capitalize-sentence-starts` is
            // itself a built-in rule that also runs before the `Last` group
            // in the merged engine (see the module-level doc comment), so
            // the user rule's literal replacement text is not recapitalized
            // afterward: the result is "done." rather than the pre-merge
            // engine's "Done.".
            rule: user_rule(
                "add-trailing-period",
                r"(?i)^done$",
                "done.",
                RulePosition::Last,
            ),
            input: "done",
            expected: "done.",
        },
    }
}

#[test]
fn user_rule_positions_are_all_ordered_correctly() {
    let failures: Vec<String> = [
        RulePosition::First,
        RulePosition::Standard,
        RulePosition::Last,
    ]
    .into_iter()
    .filter_map(|position| {
        let case = position_case(position);
        let cleaner = build_cleaner_for_test(
            case.profile,
            case.drop_trailing_period,
            &HashSet::new(),
            std::slice::from_ref(&case.rule),
        );
        let actual = cleaner.clean_text(case.input);
        (actual != case.expected).then(|| {
            format!(
                "{position:?}: input {:?}: expected {:?}, got {actual:?}",
                case.input, case.expected
            )
        })
    })
    .collect();

    assert!(
        failures.is_empty(),
        "{} case(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn user_rule_standard_position_stays_before_cleanup_when_boundary_is_disabled() {
    // Extra `Standard` boundary point on the disabled-rules axis, not
    // covered by the exhaustive per-position matrix above.
    let rules = vec![user_rule(
        "expand-brb",
        r"^brb$",
        "be right back",
        RulePosition::Standard,
    )];
    let disabled = HashSet::from(["fix-space-before-punct".to_string()]);
    let cleaner = build_cleaner_for_test(CleaningProfile::Safe, false, &disabled, &rules);

    assert_eq!(cleaner.clean_text("brb"), "Be right back");
}

/// One row of the user-rule validation-error matrix: a set of `(name,
/// pattern)` pairs (replacement is always `"x"`) fed to the cleaner, and the
/// substrings that must all appear in the resulting error message.
struct RejectCase {
    name: &'static str,
    /// (rule name, pattern) pairs handed to the cleaner; replacement is always "x".
    rules: &'static [(&'static str, &'static str)],
    /// Route through `build_cleaner(true, ...)` (cleaning disabled) instead of `Cleaner::new`.
    cleaning_disabled: bool,
    expect_substrings: &'static [&'static str],
}

#[test]
fn user_rule_validation_rejections() {
    let cases = [
        RejectCase {
            name: "name colliding with a builtin is an error",
            rules: &[("filled-pauses", r"(?i)nope")],
            cleaning_disabled: false,
            expect_substrings: &["filled-pauses", "rename"],
        },
        RejectCase {
            name: "duplicate user rule names are an error",
            rules: &[("custom-a", r"(?i)a"), ("custom-a", r"(?i)b")],
            cleaning_disabled: false,
            expect_substrings: &["duplicate", "custom-a"],
        },
        RejectCase {
            name: "empty user rule name is rejected",
            rules: &[("", r"(?i)hi")],
            cleaning_disabled: false,
            expect_substrings: &["empty name"],
        },
        RejectCase {
            name: "whitespace-only user rule name is rejected",
            rules: &[("   ", r"(?i)hi")],
            cleaning_disabled: false,
            expect_substrings: &["empty name"],
        },
        RejectCase {
            name: "user rule name with surrounding whitespace is rejected",
            rules: &[(" custom-hello ", r"(?i)hi")],
            cleaning_disabled: false,
            expect_substrings: &["leading or trailing whitespace"],
        },
        RejectCase {
            name: "disabled cleaning still validates user rule names",
            rules: &[(" custom-hello ", r"(?i)hi")],
            cleaning_disabled: true,
            expect_substrings: &["leading or trailing whitespace"],
        },
        RejectCase {
            name: "empty user rule pattern is rejected",
            rules: &[("custom-empty-pattern", "")],
            cleaning_disabled: false,
            expect_substrings: &["custom-empty-pattern", "empty pattern"],
        },
        RejectCase {
            name: "invalid user rule regex names the rule",
            rules: &[("bad-regex", "(unclosed")],
            cleaning_disabled: false,
            expect_substrings: &["user rule 'bad-regex' has invalid regex"],
        },
    ];

    let failures: Vec<String> = cases
        .iter()
        .flat_map(|case| {
            let rules: Vec<UserRule> = case
                .rules
                .iter()
                .map(|(name, pattern)| user_rule(name, pattern, "x", RulePosition::Standard))
                .collect();
            let err = if case.cleaning_disabled {
                build_cleaner(true, CleaningProfile::Safe, false, None, &[], &rules).unwrap_err()
            } else {
                build_enabled_cleaner(CleaningProfile::Safe, false, None, &[], &rules).unwrap_err()
            };
            let msg = err.to_string();
            case.expect_substrings
                .iter()
                .filter(|expected| !msg.contains(**expected))
                .map(|expected| {
                    format!(
                        "{}: expected message to contain {expected:?}, got {msg:?}",
                        case.name
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        failures.is_empty(),
        "{} case(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn whitespace_only_user_rule_pattern_is_accepted_as_a_literal_pattern() {
    // Only a missing literal pattern value is rejected. A whitespace-only
    // pattern is a normal (if unusual) regex that matches a literal space.
    let rules = vec![user_rule("space-rule", " ", "_", RulePosition::Standard)];
    let cleaner = build_cleaner_for_test(CleaningProfile::Safe, false, &HashSet::new(), &rules);
    assert_eq!(cleaner.clean_text("a b"), "A_b");
}

#[test]
fn valid_zero_width_user_regexes_are_not_treated_as_missing_patterns() {
    for (name, pattern) in [("group", "()"), ("repeat", "a*"), ("boundary", r"\b")] {
        let rule = user_rule(name, pattern, "x", RulePosition::Standard);
        build_enabled_cleaner(CleaningProfile::Safe, false, None, &[], &[rule])
            .unwrap_or_else(|err| panic!("{pattern:?} is a valid configured regex: {err:#}"));
    }
}

#[test]
fn disabled_user_rule_skips_compilation_and_application() {
    let mut disabled = HashSet::new();
    disabled.insert("custom-hello".to_string());
    let rules = vec![user_rule(
        "custom-hello",
        r"(?i)hello",
        "HI",
        RulePosition::Standard,
    )];
    let cleaner = build_cleaner_for_test(CleaningProfile::Safe, false, &disabled, &rules);
    assert_eq!(cleaner.clean_text("hello world"), "Hello world");
}

#[test]
fn build_cleaner_validates_regex_for_a_disabled_user_rule() {
    let rules = vec![user_rule(
        "broken-regex",
        "(unclosed",
        "x",
        RulePosition::Standard,
    )];
    let disabled = vec!["broken-regex".to_string()];
    let result = build_cleaner(false, CleaningProfile::Safe, false, None, &disabled, &rules);
    let err = result.expect_err("disabled rules still need valid configuration");
    assert!(format!("{err:#}").contains("invalid regex"));
}
