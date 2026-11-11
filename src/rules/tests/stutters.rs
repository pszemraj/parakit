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
        let result = collapse_repeated_words(input);
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
        let result = collapse_repeated_words(&input);
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
