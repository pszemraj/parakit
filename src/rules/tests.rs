//! Unit tests for cleaning rules, ordering, profiles, and user-rule validation.

use std::collections::HashSet;

use super::passes::capitalize_sentence_starts;
use super::*;

fn build_cleaner_for_test(
    profile: CleaningProfile,
    drop_trailing_period: bool,
    disabled: &HashSet<String>,
    user_rules: &[UserRule],
) -> Cleaner {
    Cleaner::new(profile, drop_trailing_period, None, disabled, user_rules)
        .expect("rules must compile")
}

fn cleaner_with_number_threshold(threshold: Option<f64>) -> Cleaner {
    Cleaner::new(
        CleaningProfile::Safe,
        false,
        threshold,
        &HashSet::new(),
        &[],
    )
    .expect("rules must compile")
}

fn cleaner_keep_period(profile: CleaningProfile) -> Cleaner {
    build_cleaner_for_test(profile, false, &HashSet::new(), &[])
}

fn cleaner_messaging_default(profile: CleaningProfile) -> Cleaner {
    build_cleaner_for_test(profile, true, &HashSet::new(), &[])
}

fn assert_clean_cases(profile: CleaningProfile, cases: &[(&str, &str)]) {
    let cleaner = cleaner_keep_period(profile);
    for (input, expected) in cases {
        assert_eq!(cleaner.clean_text(input), *expected, "input: {input}");
    }
}

fn user_rule(name: &str, pattern: &str, replacement: &str, position: RulePosition) -> UserRule {
    UserRule {
        name: name.to_string(),
        description: None,
        pattern: pattern.to_string(),
        replacement: replacement.to_string(),
        position,
    }
}

#[test]
fn safe_profile_preserves_semantic_discourse_markers() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("So, I think this works.", "So, I think this works."),
            ("You know the rules.", "You know the rules."),
            ("You know what I mean?", "You know what I mean?"),
            ("It's like pulling teeth.", "It's like pulling teeth."),
            (
                "It is like 50 percent more expensive.",
                "It is like 50 percent more expensive.",
            ),
            (
                "Files, like Microsoft Word files.",
                "Files, like Microsoft Word files.",
            ),
        ],
    );
}

#[test]
fn aggressive_profile_retains_opt_in_stylistic_cleanup() {
    assert_clean_cases(
        CleaningProfile::Aggressive,
        &[
            ("So, I think this works.", "I think this works."),
            ("It's like, you know, hard.", "It's hard."),
            ("As in like tuple.", "As in tuple."),
            ("It is basically like broken.", "It is basically broken."),
            ("So, well, I mean, this works.", "This works."),
            ("You know what I mean, this works.", "This works."),
            ("So that works.", "That works."),
            ("Well the system works.", "The system works."),
            ("Like actually this works.", "Actually this works."),
            ("It is, I mean, difficult.", "It is difficult."),
            ("It is, I don't know, difficult.", "It is difficult."),
            ("Not like, I mean, impossible.", "Not impossible."),
            ("It is actually like broken.", "It is actually broken."),
            ("And actually like broken.", "And actually broken."),
        ],
    );
}

#[test]
fn you_know_requires_full_comma_delimiting() {
    assert_clean_cases(
        CleaningProfile::Aggressive,
        &[
            ("You know, this works.", "This works."),
            ("You know what I mean?", "You know what I mean?"),
            (
                "You know what the report says.",
                "You know what the report says.",
            ),
            ("It is, you know, difficult.", "It is difficult."),
        ],
    );
}

#[test]
fn filled_pauses_are_removed_without_matching_er_acronym() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("I, um, think this works.", "I think this works."),
            ("uh, hello there.", "Hello there."),
            ("I, erm, agree.", "I agree."),
            ("The ER team arrived.", "The ER team arrived."),
        ],
    );
}

#[test]
fn fancy_regex_collapses_only_safe_repeated_words_by_default() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("the the the cat", "The cat"),
            ("I I I think", "I think"),
            ("we we ran", "We ran"),
            ("did did happen", "Did happen"),
            ("has has changed", "Has changed"),
            ("I know that that is valid.", "I know that that is valid."),
            (
                "No no no, we are not doing that.",
                "No no no, we are not doing that.",
            ),
            ("Can can we split this?", "Can can we split this?"),
            ("We should be be better.", "We should be be better."),
        ],
    );
}

#[test]
fn aggressive_profile_can_collapse_ambiguous_repetitions() {
    assert_clean_cases(
        CleaningProfile::Aggressive,
        &[
            ("that that works", "That works"),
            ("no no no problem", "No problem"),
        ],
    );
}

#[test]
fn generic_prefix_stutter_rule_reports_all_matches_under_one_name() {
    let result = cleaner_keep_period(CleaningProfile::Safe)
        .try_clean("we sh sh should and d d definitely change")
        .unwrap();

    assert_eq!(result.text, "We should and definitely change");
    assert_eq!(
        result
            .rules_fired
            .iter()
            .find(|hit| hit.name == "repeated-prefix-stutter"),
        Some(&RuleHit {
            name: "repeated-prefix-stutter".to_string(),
            matches: 2,
        })
    );
}

#[test]
fn high_confidence_casual_forms_are_expanded() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("I'm gonna test it.", "I'm going to test it."),
            ("I'm gunna test it.", "I'm going to test it."),
            ("I wanna test it.", "I want to test it."),
            ("I wana test it.", "I want to test it."),
            ("It's kinda odd.", "It's kind of odd."),
            ("Gimme a second.", "Give me a second."),
            ("I left 'cause it was late.", "I left because it was late."),
        ],
    );
}

#[test]
fn complete_right_tag_questions_become_periods() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            (
                "The model can discover it, right?",
                "The model can discover it.",
            ),
            (
                "(In either case, it should be discoverable by the model, right?)",
                "(In either case, it should be discoverable by the model.)",
            ),
            ("Turn right?", "Turn right?"),
            ("Is that right?", "Is that right?"),
        ],
    );
}

#[test]
fn spaced_uppercase_letters_collapse_by_invariant() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("Use the O C R model.", "Use the OCR model."),
            ("This is an L L M code agent.", "This is an LLM code agent."),
            ("R L is a subset of S F T.", "RL is a subset of SFT."),
            ("Use the G G H C L I.", "Use the GGHCLI."),
            ("Let's A B test this.", "Let's AB test this."),
            (
                "A continue. B I also need this.",
                "A continue. BI also need this.",
            ),
            ("I I think this works.", "I think this works."),
        ],
    );
}

#[test]
fn text2num_handles_documented_cardinals_groups_and_decimals() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("Five files and four folders.", "5 files and 4 folders."),
            ("Nine thousand four hundred and fifty-three.", "9453."),
            (
                "The value is three point one four one five.",
                "The value is 3.1415.",
            ),
            (
                // A comma-delimited group converts as a whole regardless of
                // the default threshold; text2num does not treat these
                // members as "isolated" values.
                "Groups like one, two, three are digits.",
                "Groups like 1, 2, 3 are digits.",
            ),
            ("Zero, one, two, three, four.", "0, 1, 2, 3, 4."),
            (
                // Unlike the comma-delimited group above, the trailing
                // "three" here is an isolated value below the default
                // threshold of 4, so it is left as text2num produced it.
                "When it asks you to press 0, 1, 2, or three.",
                "When it asks you to press 0, 1, 2, or three.",
            ),
        ],
    );
}

#[test]
fn text2num_leaves_values_below_the_default_threshold_untouched() {
    // With `number_threshold` unset, the effective threshold is
    // `DEFAULT_NUMBER_THRESHOLD` (4.0): isolated values strictly below it
    // are left exactly as produced (not forced to words, not forced to
    // digits), and values at or above it are digitized.
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("Zero files remain.", "Zero files remain."),
            (
                "One thing and two more ideas.",
                "One thing and two more ideas.",
            ),
            ("There are four folders.", "There are 4 folders."),
            ("There are five folders.", "There are 5 folders."),
        ],
    );
}

#[test]
fn spoken_number_threshold_preserves_only_values_strictly_below_it() {
    let cleaner = cleaner_with_number_threshold(Some(5.0));
    assert_eq!(
        cleaner.clean_text("One file. Four folders. Five notes. Six tasks."),
        "One file. Four folders. 5 notes. 6 tasks."
    );
    assert_eq!(cleaner.number_threshold(), 5.0);
}

#[test]
fn unset_number_threshold_resolves_to_the_default_and_digitizes_at_or_above_it() {
    let cleaner = cleaner_with_number_threshold(None);
    assert_eq!(
        cleaner.clean_text("One file. Three notes. Four folders. Ten tasks."),
        "One file. Three notes. 4 folders. 10 tasks."
    );
    assert_eq!(cleaner.number_threshold(), DEFAULT_NUMBER_THRESHOLD);
}

#[test]
fn explicit_zero_number_threshold_is_the_opt_out_that_converts_every_value() {
    let cleaner = cleaner_with_number_threshold(Some(0.0));
    assert_eq!(
        cleaner.clean_text("Zero files. One folder. Four notes."),
        "0 files. 1 folder. 4 notes."
    );
    assert_eq!(cleaner.number_threshold(), 0.0);
}

#[test]
fn unset_and_explicit_zero_number_thresholds_now_produce_different_ruleset_ids() {
    // Before the default changed, an explicit `Some(0.0)` collapsed into
    // `None` in `Cleaner::assemble`, so the two shared a ruleset id. Now
    // that `Some(0.0)` is the explicit opt-out from the default of 4.0
    // instead of a synonym for "unset", they must fingerprint differently.
    let omitted = cleaner_with_number_threshold(None);
    let zero = cleaner_with_number_threshold(Some(0.0));
    assert_ne!(omitted.ruleset_id(), zero.ruleset_id());
}

#[test]
fn invalid_number_thresholds_are_rejected_even_when_cleaning_is_disabled() {
    for threshold in [Some(-1.0), Some(f64::NAN), Some(f64::INFINITY)] {
        let err =
            build_cleaner(true, CleaningProfile::Safe, false, threshold, &[], &[]).unwrap_err();
        assert!(
            err.to_string()
                .contains("number threshold must be a finite value"),
            "threshold {threshold:?}: {err:#}"
        );
    }
}

#[test]
fn number_threshold_does_not_override_structural_version_conversion() {
    let cleaner = cleaner_with_number_threshold(Some(10.0));
    assert_eq!(
        cleaner.clean_text("V zero point five point two. Four files."),
        "v0.5.2. Four files."
    );
}

#[test]
fn text2num_preserves_second_when_it_is_a_time_unit() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("Give me a second.", "Give me a second."),
            // "one" is an isolated value below the default threshold of 4,
            // so it is left as-is; this case only used to convert back when
            // the default converted every value.
            ("Wait one second.", "Wait one second."),
            (
                "Process five tokens per second.",
                "Process 5 tokens per second.",
            ),
            (
                "It was a split-second choice.",
                "It was a split-second choice.",
            ),
            // The ordinal "second" (2nd) is also below the default
            // threshold of 4, so it is left as-is.
            ("Open the second file.", "Open the second file."),
            ("This is the twenty-second file.", "This is the 22nd file."),
        ],
    );
}

#[test]
fn version_components_use_text2num_without_a_custom_number_parser() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("V zero point five point two.", "v0.5.2."),
            ("PyTorch two point thirteen point oh.", "PyTorch 2.13.0."),
            ("CUDA 12 point nine.", "CUDA 12.9."),
            ("B0.1 point two.", "B0.1.2."),
        ],
    );
}

#[test]
fn identifier_formatting_uses_structural_rules_not_vocabulary_lists() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("RTX 50 90 release.", "RTX 5090 release."),
            ("S M 1 20 support.", "SM120 support."),
            ("Use F 32 and H 100.", "Use F32 and H100."),
            ("About 3 50 K lines.", "About 350K lines."),
            ("GPT 5 5 stays split.", "GPT 5 5 stays split."),
            ("I 5 stays separate.", "I 5 stays separate."),
        ],
    );
}

#[test]
fn capitalization_protects_decimals_versions_and_dotted_tokens() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("gpt 5.5 was tested.", "Gpt 5.5 was tested."),
            (
                "versions 3.5 and 3.6 already work.",
                "Versions 3.5 and 3.6 already work.",
            ),
            (
                "use dataset.filter and agents.md.",
                "Use dataset.filter and agents.md.",
            ),
            ("version 0.5.2 was released.", "Version 0.5.2 was released."),
            (
                "open claude.ai or gmail.com.",
                "Open claude.ai or gmail.com.",
            ),
            ("email test@gmail.com now.", "Email test@gmail.com now."),
            (
                "visit https://example.com?token=abc and https://example.com/a!b.",
                "Visit https://example.com?token=abc and https://example.com/a!b.",
            ),
            (
                "visit https://example.com? then continue.",
                "Visit https://example.com? Then continue.",
            ),
            (
                "i.e. this remains lowercase.",
                "i.e. this remains lowercase.",
            ),
            ("it worked. then it stopped.", "It worked. Then it stopped."),
            (
                "it worked! then it stopped? yes.",
                "It worked! Then it stopped? Yes.",
            ),
        ],
    );
}

#[test]
fn capitalization_stops_after_a_numeric_sentence_start() {
    assert_eq!(
        cleaner_keep_period(CleaningProfile::Safe).clean_text("version. 2 changes are pending."),
        "Version. 2 changes are pending."
    );
}

#[test]
fn trace_reports_ordered_rule_hits_and_counts() {
    let result = cleaner_keep_period(CleaningProfile::Safe)
        .try_clean("um, the the G P T model is gonna work.")
        .unwrap();
    assert_eq!(result.text, "The GPT model is going to work.");
    assert!(result.failure.is_none());
    // `filled-pauses`' match on "um," does not consume the space that
    // followed the comma in the input, so its single-space replacement
    // leaves a doubled space behind; `fix-collapse-spaces` cleans it up a
    // few rules later.
    assert_eq!(
        result.rules_fired,
        vec![
            RuleHit {
                name: "filled-pauses".to_string(),
                matches: 1,
            },
            RuleHit {
                name: "stutter-safe-words".to_string(),
                matches: 1,
            },
            RuleHit {
                name: "spaced-acronyms".to_string(),
                matches: 1,
            },
            RuleHit {
                name: "casual-gonna".to_string(),
                matches: 1,
            },
            RuleHit {
                name: "fix-collapse-spaces".to_string(),
                matches: 1,
            },
            RuleHit {
                name: "fix-trim".to_string(),
                matches: 1,
            },
            RuleHit {
                name: "capitalize-sentence-starts".to_string(),
                matches: 1,
            },
        ]
    );
}

#[test]
fn cleaner_is_idempotent() {
    let cleaner = cleaner_messaging_default(CleaningProfile::Safe);
    let once = cleaner.clean_text("um, the the G P T model is gonna work.");
    assert_eq!(cleaner.clean_text(&once), once);
}

#[test]
fn disabled_rules_are_removed_from_pipeline_and_ruleset() {
    let baseline = cleaner_keep_period(CleaningProfile::Safe);
    let disabled = HashSet::from(["filled-pauses".to_string()]);
    let without_filler = build_cleaner_for_test(CleaningProfile::Safe, false, &disabled, &[]);
    assert_eq!(
        without_filler.clean_text("hello, um, world"),
        "Hello, um, world"
    );
    assert_ne!(baseline.ruleset_id(), without_filler.ruleset_id());
}

#[test]
fn ruleset_id_is_stable_and_option_sensitive() {
    let first = cleaner_keep_period(CleaningProfile::Safe);
    let second = cleaner_keep_period(CleaningProfile::Safe);
    let aggressive = cleaner_keep_period(CleaningProfile::Aggressive);
    let messaging = cleaner_messaging_default(CleaningProfile::Safe);
    assert_eq!(first.ruleset_id(), second.ruleset_id());
    assert_ne!(first.ruleset_id(), aggressive.ruleset_id());
    assert_ne!(first.ruleset_id(), messaging.ruleset_id());

    let threshold = cleaner_with_number_threshold(Some(5.0));
    assert_ne!(first.ruleset_id(), threshold.ruleset_id());
}

#[test]
fn disabled_spoken_number_rule_ignores_threshold_in_ruleset_id() {
    let disabled = HashSet::from(["spoken-numbers".to_string()]);
    let all = Cleaner::new(CleaningProfile::Safe, false, None, &disabled, &[]).unwrap();
    let threshold = Cleaner::new(CleaningProfile::Safe, false, Some(5.0), &disabled, &[]).unwrap();
    assert_eq!(all.ruleset_id(), threshold.ruleset_id());
}

#[test]
fn clean_result_remains_unchanged_when_no_pass_fires() {
    let result = cleaner_keep_period(CleaningProfile::Safe)
        .try_clean("Already clean.")
        .unwrap();
    assert_eq!(result.text, "Already clean.");
    assert!(result.rules_fired.is_empty());
    assert!(result.failure.is_none());
}

#[test]
fn lead_so_removed() {
    // Leading so-removal is unconditional in the pre-merge
    // engine but Aggressive-gated in the merged engine; use the Aggressive
    // profile to preserve this test's intent. The pre-merge engine also
    // dropped a trailing period unconditionally, so `drop_trailing_period`
    // must be enabled here too.
    let cleaner = cleaner_messaging_default(CleaningProfile::Aggressive);
    assert_eq!(
        cleaner.clean_text("So, I think this works."),
        "I think this works"
    );
    assert_eq!(
        cleaner.clean_text("So I think this works."),
        "I think this works"
    );
}

#[test]
fn um_uh_removed() {
    let cleaner = cleaner_keep_period(CleaningProfile::Safe);
    assert_eq!(
        cleaner.clean_text("I, um, think this works"),
        "I think this works"
    );
    assert_eq!(cleaner.clean_text("uh, hello there"), "Hello there");
}

#[test]
fn repeated_prefix_stutters_are_removed() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("we sh sh should go", "We should go"),
            ("we sh-sh-should go", "We should go"),
            ("we th th think so", "We think so"),
            ("we ch ch change it", "We change it"),
            ("we sh sh shutdown cleanly", "We shutdown cleanly"),
            ("it happened b b because", "It happened because"),
            ("it is d d definitely ready", "It is definitely ready"),
            ("we m m make it", "We make it"),
            ("we are s s sure", "We are sure"),
        ],
    );
}

#[test]
fn cause_becomes_because() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("'cause it was late", "Because it was late"),
            ("that's'cause it works", "That's because it works"),
            ("I left 'cause it was late", "I left because it was late"),
        ],
    );
}

#[test]
fn whitespace_cleanup() {
    let cleaner = cleaner_messaging_default(CleaningProfile::Safe);
    assert_eq!(
        cleaner.clean_text("hello   world ,  foo ."),
        "Hello world, foo"
    );
}

#[test]
fn user_rule_first_position_runs_before_builtins() {
    // A `First` user rule expands "xyz" to "So, hello" *before* any
    // built-in rule runs, so the built-in `lead-discourse-comma` rule (which only
    // fires at sentence start, and only under the Aggressive profile in the
    // merged engine) still strips the "So," it produced.
    let rules = vec![user_rule(
        "expand-xyz",
        r"(?i)^xyz$",
        "So, hello",
        RulePosition::First,
    )];
    let cleaner =
        build_cleaner_for_test(CleaningProfile::Aggressive, false, &HashSet::new(), &rules);
    assert_eq!(cleaner.clean_text("xyz"), "Hello");
}

#[test]
fn user_rule_standard_position_runs_before_cleanup_group() {
    // A `Standard` user rule introduces messy whitespace and a stray space
    // before a comma; it must run before the built-in `fix-collapse-spaces`
    // / `fix-space-before-punct` cleanup rules (both Safe, unconditional)
    // for the output to come out clean.
    let rules = vec![user_rule(
        "expand-brb",
        r"(?i)\bbrb\b",
        "be right   back ,",
        RulePosition::Standard,
    )];
    let cleaner = build_cleaner_for_test(CleaningProfile::Safe, false, &HashSet::new(), &rules);
    assert_eq!(cleaner.clean_text("brb"), "Be right back,");
}

#[test]
fn user_rule_standard_position_stays_before_cleanup_when_boundary_is_disabled() {
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

#[test]
fn user_rule_last_position_runs_after_builtins() {
    // A `Last` user rule appends a trailing period *after* the built-in
    // `fix-trailing-period` rule (enabled here via `drop_trailing_period =
    // true`) has already run, so the period this rule adds is not stripped.
    //
    // Unlike the pre-merge engine, `capitalize-sentence-starts` is itself a
    // built-in rule that also runs before the `Last` group in the merged
    // engine (see the module-level doc comment), so the user rule's literal
    // replacement text is not recapitalized afterward: the result is
    // "done." rather than the pre-merge engine's "Done.".
    let rules = vec![user_rule(
        "add-trailing-period",
        r"(?i)^done$",
        "done.",
        RulePosition::Last,
    )];
    let cleaner = build_cleaner_for_test(CleaningProfile::Safe, true, &HashSet::new(), &rules);
    assert_eq!(cleaner.clean_text("done"), "done.");
}

#[test]
fn user_rule_name_colliding_with_builtin_is_an_error() {
    let rules = vec![user_rule(
        "filled-pauses",
        r"(?i)nope",
        "x",
        RulePosition::Standard,
    )];
    let err =
        Cleaner::new(CleaningProfile::Safe, false, None, &HashSet::new(), &rules).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("filled-pauses"), "message: {msg}");
    assert!(msg.contains("rename"), "message: {msg}");
}

#[test]
fn duplicate_user_rule_names_are_an_error() {
    let rules = vec![
        user_rule("custom-a", r"(?i)a", "A", RulePosition::Standard),
        user_rule("custom-a", r"(?i)b", "B", RulePosition::Standard),
    ];
    let err =
        Cleaner::new(CleaningProfile::Safe, false, None, &HashSet::new(), &rules).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("duplicate"), "message: {msg}");
    assert!(msg.contains("custom-a"), "message: {msg}");
}

#[test]
fn empty_user_rule_name_is_rejected() {
    let rules = vec![user_rule("", r"(?i)hi", "hello", RulePosition::Standard)];
    let err =
        Cleaner::new(CleaningProfile::Safe, false, None, &HashSet::new(), &rules).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("empty name"), "message: {msg}");
}

#[test]
fn whitespace_only_user_rule_name_is_rejected() {
    let rules = vec![user_rule("   ", r"(?i)hi", "hello", RulePosition::Standard)];
    let err =
        Cleaner::new(CleaningProfile::Safe, false, None, &HashSet::new(), &rules).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("empty name"), "message: {msg}");
}

#[test]
fn user_rule_name_with_surrounding_whitespace_is_rejected() {
    let rules = vec![user_rule(
        " custom-hello ",
        r"(?i)hi",
        "hello",
        RulePosition::Standard,
    )];
    let err =
        Cleaner::new(CleaningProfile::Safe, false, None, &HashSet::new(), &rules).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("leading or trailing whitespace"),
        "message: {msg}"
    );
}

#[test]
fn disabled_cleaning_still_validates_user_rule_names() {
    let rules = vec![user_rule(
        " custom-hello ",
        r"(?i)hi",
        "hello",
        RulePosition::Standard,
    )];

    let err = build_cleaner(true, CleaningProfile::Safe, false, None, &[], &rules).unwrap_err();

    assert!(
        err.to_string().contains("leading or trailing whitespace"),
        "message: {err:#}"
    );
}

#[test]
fn empty_user_rule_pattern_is_rejected() {
    let rules = vec![user_rule(
        "custom-empty-pattern",
        "",
        "x",
        RulePosition::Standard,
    )];
    let err =
        Cleaner::new(CleaningProfile::Safe, false, None, &HashSet::new(), &rules).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("custom-empty-pattern"), "message: {msg}");
    assert!(msg.contains("empty pattern"), "message: {msg}");
}

#[test]
fn whitespace_only_user_rule_pattern_is_accepted_as_a_literal_pattern() {
    // Only the literal empty string is rejected. A whitespace-only pattern
    // is a normal (if unusual) regex that matches a literal space, not the
    // "matches everywhere" footgun an empty pattern is, so it must not be
    // rejected by `validate_user_rules`.
    let rules = vec![user_rule("space-rule", " ", "_", RulePosition::Standard)];
    let cleaner = build_cleaner_for_test(CleaningProfile::Safe, false, &HashSet::new(), &rules);
    assert_eq!(cleaner.clean_text("a b"), "A_b");
}

#[test]
fn invalid_user_rule_regex_names_the_rule() {
    let rules = vec![user_rule(
        "bad-regex",
        "(unclosed",
        "x",
        RulePosition::Standard,
    )];
    let err =
        Cleaner::new(CleaningProfile::Safe, false, None, &HashSet::new(), &rules).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("user rule 'bad-regex' has invalid regex"),
        "message: {msg}"
    );
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
fn build_cleaner_never_compiles_regex_for_a_disabled_user_rule() {
    // A user rule named in `disabled_rules` is filtered out in
    // `push_user_rules` before `compile_user_regex` is ever called on it, so
    // a disabled rule may carry an invalid regex without failing
    // `build_cleaner` -- the same entry point `config::validate_config`
    // calls with the real `cleaning.disabled_rules` list instead of an
    // empty slice. This is the documented way to "park" a `[[rules.user]]`
    // entry whose pattern does not compile yet.
    let rules = vec![user_rule(
        "broken-regex",
        "(unclosed",
        "x",
        RulePosition::Standard,
    )];
    let disabled = vec!["broken-regex".to_string()];
    let result = build_cleaner(false, CleaningProfile::Safe, false, None, &disabled, &rules);
    assert!(
        result.is_ok(),
        "a disabled user rule's invalid regex must not be compiled: {result:?}"
    );
}

#[test]
fn sentence_starts_are_capitalized_after_cleaning() {
    // Uses leading "So," stripping, which is Aggressive-gated in the merged
    // engine.
    let cleaner = cleaner_keep_period(CleaningProfile::Aggressive);
    assert_eq!(
        cleaner.clean_text("So, the cat ran. then it slept. \"then it woke.\""),
        "The cat ran. Then it slept. \"Then it woke.\""
    );
}

#[test]
fn capitalization_is_idempotent_for_existing_sentence_case() {
    let cleaner = cleaner_keep_period(CleaningProfile::Safe);
    let input = "Already capitalized. Still capitalized!";
    assert_eq!(cleaner.clean_text(input), input);
}

#[test]
fn capitalization_handles_leading_whitespace() {
    assert_eq!(
        capitalize_sentence_starts("   hello there").text,
        "   Hello there"
    );
}

#[test]
fn single_letter_stutter() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("t t t think", "Think"),
            ("I w w w want this", "I want this"),
            ("s s s sure", "Sure"),
        ],
    );
}

#[test]
fn assert_rule_name_exists_works() {
    assert!(assert_rule_name_exists("filled-pauses", &[]).is_ok());
    assert!(assert_rule_name_exists("does-not-exist", &[]).is_err());
    let rules = vec![user_rule(
        "custom-hello",
        r"(?i)hello",
        "HI",
        RulePosition::Standard,
    )];
    assert!(assert_rule_name_exists("custom-hello", &rules).is_ok());
}

#[test]
fn rendered_rule_list_reports_enabled_state_for_user_rules() {
    let rules = vec![
        UserRule {
            description: Some("enabled description".to_string()),
            ..user_rule("custom-enabled", r"(?i)hello", "HI", RulePosition::Standard)
        },
        UserRule {
            description: Some("disabled description".to_string()),
            ..user_rule(
                "custom-disabled",
                r"(?i)goodbye",
                "BYE",
                RulePosition::Standard,
            )
        },
    ];
    let disabled = HashSet::from(["custom-disabled"]);

    let rendered = render_rule_list(CleaningProfile::Safe, true, &disabled, &rules);

    assert!(rendered.contains("name                              enabled  description (user)"));
    assert!(rendered.contains("custom-enabled                    yes      enabled description"));
    assert!(rendered.contains("custom-disabled                   no       disabled description"));
}

#[test]
fn final_period_removal_is_off_by_default_and_can_be_enabled() {
    let kept = cleaner_keep_period(CleaningProfile::Safe);
    assert_eq!(kept.clean_text("Keep this period."), "Keep this period.");
    assert!(!kept.drops_trailing_period());

    let dropped = cleaner_messaging_default(CleaningProfile::Safe);
    assert_eq!(dropped.clean_text("Drop this period."), "Drop this period");
    assert!(dropped.drops_trailing_period());
}

#[test]
fn dangling_connective_after_a_period_is_dropped() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("The build is green. But", "The build is green."),
            ("I pushed the fix. So", "I pushed the fix."),
        ],
    );
}

#[test]
fn dangling_connective_promotes_a_trailing_comma_to_a_period() {
    assert_clean_cases(CleaningProfile::Safe, &[("It works, but", "It works.")]);
}

#[test]
fn chained_dangling_connectives_are_all_removed_in_one_pass() {
    // Each application strips one trailing connective; applying that
    // repeatedly resolves a whole chain, so "And so." fully reduces down to
    // the real sentence underneath.
    assert_clean_cases(CleaningProfile::Safe, &[("It works. And so.", "It works.")]);
}

#[test]
fn dangling_connective_rule_leaves_real_endings_and_mid_sentence_uses_untouched() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            // A real sentence ending in a word that is not on the
            // connective list is left alone.
            ("It works well.", "It works well."),
            // "and" mid-sentence, not at the very end, is never touched.
            (
                "The cat sat on the mat and the dog ran",
                "The cat sat on the mat and the dog ran",
            ),
        ],
    );
}

#[test]
fn lone_dangling_connective_with_no_preceding_content_is_left_alone() {
    assert_clean_cases(CleaningProfile::Safe, &[("So", "So")]);
}

#[test]
fn dangling_connective_removal_interacts_with_trailing_period_removal() {
    // The rule always leaves the sentence-final period behind; whether that
    // period itself survives depends only on the independent, default-on
    // `fix-trailing-period` messaging rule, exercised here both off and on.
    let kept = cleaner_keep_period(CleaningProfile::Safe);
    assert_eq!(
        kept.clean_text("The build is green. But"),
        "The build is green."
    );

    let messaging = cleaner_messaging_default(CleaningProfile::Safe);
    assert_eq!(
        messaging.clean_text("The build is green. But"),
        "The build is green"
    );
}

#[test]
fn dangling_connective_rule_can_be_disabled_by_name() {
    let disabled = HashSet::from(["drop-dangling-connective".to_string()]);
    let cleaner = build_cleaner_for_test(CleaningProfile::Safe, false, &disabled, &[]);
    assert_eq!(
        cleaner.clean_text("The build is green. But"),
        "The build is green. But"
    );
}

#[test]
fn fancy_regex_failure_fails_open_to_the_original_transcript() {
    // A backtrack limit of zero fails on the very first backtrack step
    // (`fancy_regex::vm` errors as soon as `backtrack_count > limit`), so
    // any input requiring more than a single deterministic pass through the
    // `stutter-safe-words` pattern's unanchored search must error.
    let cleaner =
        Cleaner::new_with_backtrack_limit(CleaningProfile::Safe, false, &HashSet::new(), &[], 0)
            .expect("rule patterns must still compile with a zero backtrack limit");

    let input = "the the the cat sat on the the mat";
    let try_clean_err = cleaner
        .try_clean(input)
        .expect_err("a backtrack limit of 0 must exhaust during matching");
    assert!(!try_clean_err.to_string().is_empty());

    // ...while `clean` must never propagate or panic: it fails open to the
    // exact original transcript and records the failure.
    let result = cleaner.clean(input);
    assert_eq!(result.text, input);
    assert!(result.rules_fired.is_empty());
    assert!(result.failure.is_some());
}
