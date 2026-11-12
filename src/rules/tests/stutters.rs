//! Whole-word restart policy and its interactions with the full cleaner.

use super::*;
use crate::rules::stutters::collapse_repeated_words;

#[test]
fn whole_words_keep_the_first_spelling_and_count_runs() {
    for (input, expected, matches) in [
        ("Where where WHERE", "Where", 1),
        ("I'm I'm and it's IT'S", "I'm and it's", 2),
        ("I’m I’M", "I’m", 1),
        ("what\t what\twhat", "what", 1),
        ("café CAFÉ", "café", 1),
        ("hello hello, hello hello.", "hello, hello.", 2),
        ("is is useful", "is useful", 1),
        ("a a test", "a test", 1),
        ("the\nthe", "the\nthe", 0),
        ("where, where", "where, where", 0),
        ("what—what", "what—what", 0),
        ("very, very", "very, very", 0),
        ("where\u{a0}where", "where\u{a0}where", 0),
        ("123 123", "123 123", 0),
    ] {
        let result = collapse_repeated_words(input, &[]);
        assert_eq!(result.text, expected, "{input}");
        assert_eq!(result.matches, matches, "{input}");
    }
}

#[test]
fn deliberate_repeats_and_number_words_are_preserved() {
    for word in [
        "five",
        "twenty",
        "hundred",
        "million",
        "first",
        "zeroth",
        "trillion",
        "oh",
        "nil",
        "that",
        "had",
        "do",
        "can",
        "no",
        "plus",
        "dash",
        "equals",
        "slash",
        "dot",
        "star",
        "underscore",
        "colon",
        "pipe",
        "blah",
        "yada",
        "ha",
        "heh",
        "la",
        "bye",
        "yeah",
        "very",
        "really",
        "much",
        "many",
        "far",
        "B",
        "x",
        "é",
    ] {
        let input = format!("{word} {word} {word}");
        let result = collapse_repeated_words(&input, &[]);
        assert_eq!(result.text, input, "{word}");
        assert_eq!(result.matches, 0, "{word}");
    }
}

#[test]
fn general_stutters_work_before_number_and_acronym_formatting() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            (
                "So that's where where this comment will be used.",
                "So that's where this comment will be used.",
            ),
            (
                "I'm I'm ready and it's it's useful.",
                "I'm ready and it's useful.",
            ),
            ("we i i think", "We I think"),
            ("sh sh should and th-th-the test", "Should and the test"),
            (
                "C plus plus and dash dash verbose",
                "C plus plus and dash dash verbose",
            ),
            ("five five five", "Five five five"),
            ("B B C", "BBC"),
            ("the\nthe test", "The\nthe test"),
        ],
    );
    assert_clean_cases(
        CleaningProfile::Aggressive,
        &[
            ("that that can can work", "That can work"),
            ("no, no and very very", "No, no and very very"),
            ("had\nhad", "Had\nhad"),
        ],
    );
}

#[test]
fn general_rule_remains_disableable_and_reports_runs() {
    let input = "where where where and how how";
    let cleaner = cleaner_keep_period(CleaningProfile::Safe);
    let result = cleaner.try_clean(input).unwrap();
    assert_eq!(result.text, "Where and how");
    assert_eq!(
        result
            .rules_fired
            .iter()
            .find(|hit| hit.name == "stutter-safe-words")
            .unwrap()
            .matches,
        2
    );
    let disabled = HashSet::from(["stutter-safe-words".to_string()]);
    let cleaner = build_cleaner_for_test(CleaningProfile::Safe, false, &disabled, &[]);
    assert_eq!(cleaner.clean_text(input), "Where where where and how how");
}

#[test]
fn identical_words_can_have_distinct_grammatical_roles() {
    for profile in [CleaningProfile::Safe, CleaningProfile::Aggressive] {
        assert_clean_cases(
            profile,
            &[
                (
                    "You don't get to control control T and control W.",
                    "You don't get to control control T and control W.",
                ),
                (
                    "You can control control control T.",
                    "You can control control T.",
                ),
                (
                    "Calling something that isn't AGI AGI changes its label.",
                    "Calling something that isn't AGI AGI changes its label.",
                ),
                (
                    "Call something that is art art.",
                    "Call something that is art art.",
                ),
                (
                    "Don't label something that is not science science.",
                    "Don't label something that is not science science.",
                ),
                (
                    "Call someone who is a doctor doctor.",
                    "Call someone who is a doctor doctor.",
                ),
                (
                    "Calling something that isn't AGI AGI AGI changes its label.",
                    "Calling something that isn't AGI AGI changes its label.",
                ),
                ("Press control control T.", "Press control T."),
                (
                    "We call something that is is is not not a a label.",
                    "We call something that is not a label.",
                ),
                (
                    "You can control control the cursor.",
                    "You can control the cursor.",
                ),
                (
                    "We discussed calling it. This isn't AGI AGI.",
                    "We discussed calling it. This isn't AGI.",
                ),
                (
                    "We discussed calling it; this isn't AGI AGI.",
                    "We discussed calling it; this isn't AGI.",
                ),
                (
                    "Calling it now.\nThat isn't AGI AGI.",
                    "Calling it now.\nThat isn't AGI.",
                ),
            ],
        );
    }
}

#[test]
fn configured_repetition_exceptions_preserve_words_and_literal_phrases() {
    let entries = [
        "something",
        "hee",
        "bang",
        "tomato",
        "paddle",
        "chinny chin chin",
    ]
    .map(str::to_string);
    for profile in [CleaningProfile::Safe, CleaningProfile::Aggressive] {
        let mut cleaner = cleaner_keep_period(profile);
        cleaner.set_repetition_exceptions(&entries).unwrap();
        for input in [
            "Something something, selling tools.",
            "Hee hee, do you get it?",
            "Bang bang.",
            "Tomato tomato.",
            "Use the paddle paddle model.",
            "By my chinny chin chin.",
        ] {
            assert_eq!(cleaner.clean_text(input), input, "{profile:?}: {input}");
        }
        assert_eq!(
            cleaner.clean_text("Bang BANG where where."),
            "Bang BANG where."
        );
        assert_eq!(cleaner.clean_text("My chin chin hurts."), "My chin hurts.");
        assert_eq!(
            cleaner.clean_text("My chinny\tchin\tchin."),
            "My chinny\tchin\tchin."
        );
        assert_eq!(
            cleaner.clean_text("My chinny chin, chin chin."),
            "My chinny chin, chin."
        );
        assert_eq!(
            cleaner.clean_text("My chinny\nchin chin."),
            "My chinny\nchin."
        );
        assert_eq!(
            cleaner.clean_text("My chinny chin chin chin."),
            "My chinny chin chin."
        );
        let result = cleaner.try_clean("Bang bang where where.").unwrap();
        assert!(result
            .rules_fired
            .iter()
            .any(|hit| hit.name == "stutter-safe-words" && hit.matches == 1));
    }
}

#[test]
fn repetition_exceptions_have_canonical_identity_and_respect_disabled_rules() {
    let mut cleaner = cleaner_keep_period(CleaningProfile::Safe);
    let baseline = cleaner.ruleset_id().to_string();
    cleaner
        .set_repetition_exceptions(&["BANG".into(), "chinny  chin\tchin".into(), "bang".into()])
        .unwrap();
    let configured = cleaner.ruleset_id().to_string();
    assert_ne!(baseline, configured);
    cleaner
        .set_repetition_exceptions(&["chinny chin chin".into(), "bang".into()])
        .unwrap();
    assert_eq!(cleaner.ruleset_id(), configured);
    cleaner.set_repetition_exceptions(&[]).unwrap();
    assert_eq!(cleaner.ruleset_id(), baseline);
    assert_eq!(cleaner.clean_text("Bang bang."), "Bang.");

    let disabled = HashSet::from(["stutter-safe-words".to_string()]);
    let mut cleaner = build_cleaner_for_test(CleaningProfile::Safe, false, &disabled, &[]);
    let disabled_id = cleaner.ruleset_id().to_string();
    cleaner.set_repetition_exceptions(&["bang".into()]).unwrap();
    assert_eq!(cleaner.ruleset_id(), disabled_id);
    assert_eq!(
        cleaner.clean_text("Bang bang where where."),
        "Bang bang where where."
    );
    for invalid in [
        "",
        " ",
        "word, word",
        "word.*",
        "word\nword",
        "word\u{a0}word",
    ] {
        assert!(
            super::super::validate_repetition_exceptions(&[invalid.into()]).is_err(),
            "{invalid:?}"
        );
        assert!(
            cleaner
                .set_repetition_exceptions(&[invalid.into()])
                .is_err(),
            "{invalid:?}"
        );
    }
}

#[test]
fn repetition_exceptions_keep_contractions_whole_and_do_not_match_substrings() {
    let mut cleaner = cleaner_keep_period(CleaningProfile::Safe);
    cleaner
        .set_repetition_exceptions(&["I'm".into(), "café".into(), "cat".into()])
        .unwrap();
    assert_eq!(
        cleaner.clean_text("I'm I'm visiting café CAFÉ."),
        "I'm I'm visiting café CAFÉ."
    );
    assert_eq!(
        cleaner.clean_text("Cat cat and catalog catalog."),
        "Cat cat and catalog."
    );
    cleaner
        .set_repetition_exceptions(&["alpha beta".into(), "beta beta gamma".into()])
        .unwrap();
    assert_eq!(
        cleaner.clean_text("Alpha beta beta gamma."),
        "Alpha beta beta gamma."
    );
    cleaner
        .set_repetition_exceptions(&["chinny chin chin".into(), "alpha alpha beta".into()])
        .unwrap();
    assert_eq!(cleaner.clean_text("O'Chinny chin chin."), "O'Chinny chin.");
    assert_eq!(cleaner.clean_text("O’Chinny chin chin."), "O’Chinny chin.");
    assert_eq!(cleaner.clean_text("Alpha alpha beta's."), "Alpha beta's.");
}
