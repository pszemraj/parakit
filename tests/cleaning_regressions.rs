//! Corpus-derived regression cases for semantic-preserving cleanup.

use parakit::rules::{build_cleaner, CleaningProfile};

/// Clean `input` with terminal-period removal disabled, so each case asserts
/// on the transformation under test rather than on the messaging-style
/// trailing-period default. `drops_trailing_period` has its own coverage in
/// `trailing_period_default_and_opt_out`.
fn clean(profile: CleaningProfile, input: &str) -> String {
    build_cleaner(false, profile, false, None, &[], &[])
        .expect("cleaner should compile")
        .expect("cleaning should be enabled")
        .clean_text(input)
}

/// Clean `input` with the shipped defaults: safe profile, terminal period
/// dropped.
fn clean_with_defaults(input: &str) -> String {
    build_cleaner(false, CleaningProfile::Safe, true, None, &[], &[])
        .expect("cleaner should compile")
        .expect("cleaning should be enabled")
        .clean_text(input)
}

#[test]
fn safe_profile_preserves_comparison_and_hedging() {
    let cases = [
        "It was like preventable.",
        "It's like I don't necessarily know.",
        "What is like what do they mean?",
        "Files, like Microsoft Word files, should work.",
        "It's like pulling teeth.",
        "It's like 50 or 60% more expensive.",
    ];
    for input in cases {
        assert_eq!(clean(CleaningProfile::Safe, input), input);
    }
}

#[test]
fn safe_profile_preserves_semantic_you_know_phrases() {
    let cases = [
        "You know the rules.",
        "You know what the report says.",
        "You know what to do.",
        "You know Jira?",
        "You know what I mean?",
    ];
    for input in cases {
        assert_eq!(clean(CleaningProfile::Safe, input), input);
    }
}

#[test]
fn safe_profile_preserves_valid_duplicate_words() {
    let cases = [
        "I know that that is probably unlikely.",
        "The model that that blog references is public.",
        "Make sure that that happens.",
        "No no no, we're not doing that.",
        "No no, I mean general review.",
        "Can can be a noun in this example.",
    ];
    for input in cases {
        assert_eq!(clean(CleaningProfile::Safe, input), input);
    }
}

#[test]
fn capitalization_protects_real_technical_tokens() {
    let cases = [
        ("gpt 5.5 was tested.", "Gpt 5.5 was tested."),
        ("use 3.5 and 3.6 already.", "Use 3.5 and 3.6 already."),
        ("call dataset.filter next.", "Call dataset.filter next."),
        ("open claude.ai now.", "Open claude.ai now."),
        // "first" (1) is an isolated value below the default number
        // threshold of 4, so it stays as a word; this case is about
        // `agents.md` staying lowercase, not number conversion.
        ("edit agents.md first.", "Edit agents.md first."),
        ("email test@gmail.com now.", "Email test@gmail.com now."),
        (
            // Likewise "one" (1) stays as a word below the default
            // threshold; this case is about `i.e.` staying lowercase.
            "this is i.e. still one sentence.",
            "This is i.e. still one sentence.",
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(clean(CleaningProfile::Safe, input), expected);
    }
}

#[test]
fn safe_profile_applies_high_confidence_normalization() {
    let cases = [
        (
            "um, the the G P T model is gonna work.",
            "The GPT model is going to work.",
        ),
        (
            "the G P T model is gunna work.",
            "The GPT model is going to work.",
        ),
        ("I wanna run an A B test.", "I want to run an AB test."),
        ("I wana run an A B test.", "I want to run an AB test."),
        ("send S T D out to the T T Y.", "Send STD out to the TTY."),
        ("gimme the G G UF file.", "Give me the GG UF file."),
        (
            "I kinda think sh sh should change.",
            "I kind of think should change.",
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(clean(CleaningProfile::Safe, input), expected);
    }
}

#[test]
fn safe_profile_number_conversion_respects_the_default_threshold() {
    // Under the default (unset) number threshold of 4, the trailing "three"
    // is an isolated value below it and is left as a word...
    assert_eq!(
        clean(
            CleaningProfile::Safe,
            "When it asks you to press 0, 1, 2, or three."
        ),
        "When it asks you to press 0, 1, 2, or three."
    );
    // ...but a comma-delimited listing converts as a whole regardless of
    // the threshold, since text2num does not treat its members as
    // "isolated" values.
    assert_eq!(
        clean(CleaningProfile::Safe, "Zero, one, two, three, four."),
        "0, 1, 2, 3, 4."
    );
}

#[test]
fn safe_profile_drops_complete_right_tag_questions() {
    let input = "Okay, well could you iterate on that to make it a little bit more generic? I don't want to have to think through all that on the trim rule (In either case, it should be discoverable by the model, right?)";
    let expected = "Okay, well could you iterate on that to make it a little bit more generic? I don't want to have to think through all that on the trim rule (In either case, it should be discoverable by the model.)";
    assert_eq!(clean(CleaningProfile::Safe, input), expected);

    assert_eq!(
        clean(CleaningProfile::Safe, "Did I turn right?"),
        "Did I turn right?"
    );
}

#[test]
fn trailing_period_default_and_opt_out() {
    // Messaging-style default: exactly one terminal period is dropped.
    assert_eq!(clean_with_defaults("the cat ran."), "The cat ran");
    // Only the terminal one. Interior sentence punctuation is untouched.
    assert_eq!(
        clean_with_defaults("the cat ran. the dog slept."),
        "The cat ran. The dog slept"
    );
    // Other terminal punctuation is never dropped.
    assert_eq!(clean_with_defaults("did the cat run?"), "Did the cat run?");

    // --keep-trailing-period / cleaning.keep_trailing_period opts back in.
    assert_eq!(clean(CleaningProfile::Safe, "the cat ran."), "The cat ran.");
}

#[test]
fn spaced_acronyms_collapse_without_a_vocabulary_allowlist() {
    // No allowlist is consulted: the invariant is purely structural, so
    // project-specific and nonsense initialisms behave identically.
    let cases = [
        ("the L L M wrote it", "The LLM wrote it"),
        ("load the G G U F file", "Load the GGUF file"),
        ("R L H F is expensive", "RLHF is expensive"),
        ("A I keeps improving", "AI keeps improving"),
        ("ship the Q R S T U V build", "Ship the QRSTUV build"),
    ];
    for (input, expected) in cases {
        assert_eq!(clean_with_defaults(input), expected, "input: {input}");
    }
}

#[test]
fn duplicate_words_resolve_before_acronym_reconstruction() {
    // If acronym collapse ran first, "I I" would become "II".
    assert_eq!(clean_with_defaults("I I think so"), "I think so");
    assert_eq!(
        clean_with_defaults("I I think the L L M is fine"),
        "I think the LLM is fine"
    );
}

#[test]
fn aggressive_profile_retains_explicit_legacy_semantic_edits() {
    assert_eq!(
        clean(CleaningProfile::Aggressive, "It's like pulling teeth."),
        "It's pulling teeth."
    );
    assert_eq!(
        clean(CleaningProfile::Aggressive, "So, um, the the cat ran."),
        "The cat ran."
    );
}
