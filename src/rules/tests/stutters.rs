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
        ("It it's fine", "It's fine", 1),
        ("it it'll work", "it'll work", 1),
        ("he he's ready", "he's ready", 1),
        ("she she’ll join", "she’ll join", 1),
        ("you you're ready", "you're ready", 1),
        ("we we're ready", "we're ready", 1),
        ("they they're ready", "they're ready", 1),
        ("IT it it's fine", "IT's fine", 1),
        ("It\t it's fine", "It's fine", 1),
        ("that that's fine", "that that's fine", 0),
        ("there there's hope", "there there's hope", 0),
        ("it\nit's fine", "it\nit's fine", 0),
        ("it, it's fine", "it, it's fine", 0),
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
        "hee",
        "bang",
        "poo",
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
            ("It it's fine", "It's fine"),
            ("he he's and you you're ready", "He's and you're ready"),
            ("sh sh should and th-th-the test", "Should and the test"),
            (
                "C plus plus and dash dash verbose",
                "C plus plus and dash dash verbose",
            ),
            ("five five five", "5 5 5"),
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
fn year_fillers_keep_boundaries_without_reordering_stutters() {
    for profile in [CleaningProfile::Safe, CleaningProfile::Aggressive] {
        assert_clean_cases(
            profile,
            &[
                ("twenty uh twenty layers", "20 20 layers"),
                ("twenty uh twenty.", "20 20."),
                ("twenty uh um twenty layers", "20 20 layers"),
                ("twenty uh, um twenty layers", "20 20 layers"),
                ("20 uh twenty layers", "20 20 layers"),
                ("nineteen uh nineteen layers", "19 19 layers"),
                ("19 uh 19 layers", "19 19 layers"),
                ("eighteen uh eighteen layers", "18 18 layers"),
                ("one uh hundred", "100"),
                ("twenty uh five layers", "25 layers"),
                ("twenty twenty uh minutes", "20 20 minutes"),
                ("the um the test", "The test"),
                ("I uh I think", "I think"),
                ("T T testing", "Testing"),
                ("B B build", "Build"),
                ("f f four", "4"),
                ("f uh f four", "4"),
                ("S S seven", "7"),
                ("G uh P U", "GPU"),
                ("G uh P uh U", "GPU"),
                ("and uh and", "And"),
                ("point uh point", "Point"),
            ],
        );
    }
    let result = cleaner_keep_period(CleaningProfile::Safe)
        .try_clean("um, twenty uh twenty layers")
        .unwrap();
    let hits = &result.rules_fired;
    assert_eq!(result.text, "20 20 layers");
    assert_eq!(hits[0].name, "filled-pauses");
    assert_eq!(hits[0].matches, 2);
    assert_eq!(
        hits.iter()
            .filter(|hit| hit.name == "filled-pauses")
            .count(),
        1
    );

    let disabled = HashSet::from([
        "spoken-numbers".to_string(),
        "spoken-versions".to_string(),
        "calendar-years".to_string(),
        "numeric-identifiers".to_string(),
        "size-suffixes".to_string(),
    ]);
    let cleaner = build_cleaner_for_test(CleaningProfile::Safe, false, &disabled, &[]);
    assert_eq!(
        cleaner.clean_text("twenty uh twenty layers"),
        "Twenty twenty layers"
    );
    let disabled = HashSet::from(["filled-pauses".to_string()]);
    let cleaner = build_cleaner_for_test(CleaningProfile::Safe, false, &disabled, &[]);
    assert_eq!(
        cleaner.clean_text("twenty uh twenty layers"),
        "20 uh 20 layers"
    );

    let cleaner =
        cleaner_with_repetition_exceptions(CleaningProfile::Safe, &["model model four".into()]);
    assert_eq!(cleaner.clean_text("model model four"), "Model model 4");
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
        let cleaner = cleaner_with_repetition_exceptions(profile, &entries);
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
    let cleaner = cleaner_keep_period(CleaningProfile::Safe);
    let baseline = cleaner.ruleset_id().to_string();
    let configured = cleaner_with_repetition_exceptions(
        CleaningProfile::Safe,
        &["BANG".into(), "chinny  chin\tchin".into(), "bang".into()],
    );
    assert_ne!(baseline, configured.ruleset_id());
    let canonical = cleaner_with_repetition_exceptions(
        CleaningProfile::Safe,
        &["chinny chin chin".into(), "bang".into()],
    );
    assert_eq!(canonical.ruleset_id(), configured.ruleset_id());
    assert_eq!(
        canonical.clean_text("Bang bang chinny chin chin where where."),
        configured.clean_text("Bang bang chinny chin chin where where.")
    );
    let empty = cleaner_with_repetition_exceptions(CleaningProfile::Safe, &[]);
    assert_eq!(empty.ruleset_id(), baseline);
    assert_eq!(cleaner.clean_text("Bang bang."), "Bang bang.");

    let disabled = ["stutter-safe-words".to_string()];
    let disabled_cleaner =
        build_enabled_cleaner(CleaningProfile::Safe, false, None, &[], &disabled, &[]).unwrap();
    let cleaner = build_enabled_cleaner(
        CleaningProfile::Safe,
        false,
        None,
        &["bang".into()],
        &disabled,
        &[],
    )
    .unwrap();
    assert_eq!(cleaner.ruleset_id(), disabled_cleaner.ruleset_id());
    assert_eq!(
        cleaner.clean_text("Bang bang where where."),
        "Bang bang where where."
    );
}

#[test]
fn builders_validate_repetition_exceptions_even_when_cleaning_or_the_rule_is_disabled() {
    for invalid in [
        "",
        " ",
        "word, word",
        "word.*",
        "word\nword",
        "word\u{a0}word",
    ] {
        let exceptions = [invalid.into()];
        for disabled in [vec![], vec!["stutter-safe-words".into()]] {
            assert!(
                validate_configured_rules(None, &exceptions, &disabled, &[]).is_err(),
                "{invalid:?}"
            );
            for no_cleaning in [false, true] {
                let err = build_cleaner(
                    no_cleaning,
                    CleaningProfile::Safe,
                    false,
                    None,
                    &exceptions,
                    &disabled,
                    &[],
                )
                .unwrap_err();
                assert!(
                    format!("{err:#}").contains("cleaning.repetition_exceptions"),
                    "{invalid:?}: {err:#}"
                );
            }
        }
    }
}

#[test]
fn repetition_exceptions_keep_contractions_whole_and_do_not_match_substrings() {
    let cleaner = cleaner_with_repetition_exceptions(
        CleaningProfile::Safe,
        &["I'm".into(), "café".into(), "cat".into()],
    );
    assert_eq!(
        cleaner.clean_text("I'm I'm visiting café CAFÉ."),
        "I'm I'm visiting café CAFÉ."
    );
    assert_eq!(
        cleaner.clean_text("Cat cat and catalog catalog."),
        "Cat cat and catalog."
    );
    let cleaner = cleaner_with_repetition_exceptions(
        CleaningProfile::Safe,
        &["alpha beta".into(), "beta beta gamma".into()],
    );
    assert_eq!(
        cleaner.clean_text("Alpha beta beta gamma."),
        "Alpha beta beta gamma."
    );
    let cleaner = cleaner_with_repetition_exceptions(
        CleaningProfile::Safe,
        &["chinny chin chin".into(), "alpha alpha beta".into()],
    );
    assert_eq!(cleaner.clean_text("O'Chinny chin chin."), "O'Chinny chin.");
    assert_eq!(cleaner.clean_text("O’Chinny chin chin."), "O’Chinny chin.");
    assert_eq!(cleaner.clean_text("Alpha alpha beta's."), "Alpha beta's.");
}

fn cleaner_with_repetition_exceptions(
    profile: CleaningProfile,
    repetition_exceptions: &[String],
) -> Cleaner {
    build_enabled_cleaner(profile, false, None, repetition_exceptions, &[], &[])
        .expect("repetition exceptions must compile")
}
