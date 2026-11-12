//! Calendar and identifier formatting without changing ordinary number policy.

use super::*;

#[test]
fn quarters_and_years_bypass_the_threshold_without_changing_prose() {
    let input = "In Q two of twenty twenty six. Of which they're the only occupant, and I'm talking about Colossus one";
    for profile in [CleaningProfile::Safe, CleaningProfile::Aggressive] {
        let cleaner = cleaner_keep_period(profile);
        assert_eq!(cleaner.number_threshold(), 4.0);
        assert_eq!(
            cleaner.clean_text(input),
            "In Q2 of 2026. Of which they're the only occupant, and I'm talking about Colossus one"
        );
        for threshold in [0.0, 4.0, 10.0] {
            let cleaner = build_cleaner_for_test_with_threshold(
                profile,
                false,
                Some(threshold),
                &HashSet::new(),
                &[],
            );
            for (input, expected) in [
                ("In Q one of twenty twenty six.", "In Q1 of 2026."),
                ("In Q two of twenty twenty six.", "In Q2 of 2026."),
                ("In Q three of twenty twenty six.", "In Q3 of 2026."),
                ("In Q four of twenty twenty six.", "In Q4 of 2026."),
                ("Q two twenty twenty six.", "Q2 2026."),
                ("Q2 of nineteen ninety-nine.", "Q2 of 1999."),
                ("Q 2 of twenty twenty six.", "Q2 of 2026."),
                ("Q two of 20 twenty six.", "Q2 of 2026."),
                ("Q two of twenty 26.", "Q2 of 2026."),
                ("Q two of 20 26.", "Q2 of 2026."),
                ("The year 19 06.", "The year 1906."),
                (
                    "During the first quarter of twenty twenty six.",
                    if threshold <= 1.0 {
                        "During the 1st quarter of 2026."
                    } else {
                        "During the first quarter of 2026."
                    },
                ),
                ("The year seventeen seventy-six.", "The year 1776."),
                (
                    "It happened in nineteen ninety-nine.",
                    "It happened in 1999.",
                ),
                (
                    "The release is from twenty twenty two.",
                    "The release is from 2022.",
                ),
                ("Since eighteen forty-two.", "Since 1842."),
                ("The year nineteen oh five.", "The year 1905."),
                ("In Q two of twenty zero six.", "In Q2 of 2006."),
                ("In Q two of twenty o six.", "In Q2 of 2006."),
                ("March fifth, twenty twenty six.", "March 5th, 2026."),
                ("March first twenty twenty six.", "March 1st 2026."),
                ("May five twenty twenty six.", "May 5 2026."),
                ("FY twenty twenty six.", "FY 2026."),
                ("Use B two and SM one.", "Use B2 and SM1."),
                ("Q nine and B zero.", "Q9 and B0."),
            ] {
                assert_eq!(
                    cleaner.clean_text(input),
                    expected,
                    "{profile:?}/{threshold}: {input}"
                );
            }
        }
    }
}

#[test]
fn calendar_context_does_not_reinterpret_unmarked_numeric_runs() {
    for threshold in [0.0, 4.0, 10.0] {
        let cleaner = cleaner_with_number_threshold(Some(threshold));
        for (input, expected) in [
            ("The year 020 26.", "The year 020 26."),
            ("The year 20 026.", "The year 20 026."),
            (
                "The year twenty point zero twenty six.",
                "The year 20.0 26.",
            ),
            (
                "Q two of nineteen ninety nine point zero.",
                "Q2 of 19 99.0.",
            ),
        ] {
            assert_eq!(cleaner.clean_text(input), expected, "{threshold}: {input}");
        }
        for (input, expected) in [
            ("Meet at ten thirty.", "Meet at ten thirty."),
            ("Wait until ten thirty.", "Wait until ten thirty."),
            ("Call at nineteen thirty.", "Call at nineteen thirty."),
            ("Until nineteen thirty p.m.", "Until nineteen thirty p.m."),
            (
                "The score was nineteen ninety nine.",
                "The score was nineteen ninety nine.",
            ),
            (
                "Use twenty twenty six files.",
                "Use twenty twenty six files.",
            ),
            (
                "My number is five five five twenty twelve.",
                "My number is five five five twenty twelve.",
            ),
            ("Llama thirteen seventy B.", "Llama thirteen seventy B."),
            (
                "Use one twenty eight K context.",
                "Use one twenty eight K context.",
            ),
            ("SM one twenty.", "SM one twenty."),
            ("Q two-year term.", "Q two-year term."),
            ("B two thirds of the whole.", "B two thirds of the whole."),
            (
                "B three quarters of a dollar.",
                "B three quarters of a dollar.",
            ),
            (
                "B five hundredths of a second.",
                "B five hundredths of a second.",
            ),
            (
                "We work from nineteen thirty until twenty fifteen.",
                "We work from nineteen thirty until twenty fifteen.",
            ),
            (
                "Until nineteen thirty tonight.",
                "Until nineteen thirty tonight.",
            ),
            (
                "In twenty twenty six K context.",
                "In twenty twenty six K context.",
            ),
            ("Q two\nof twenty twenty six.", "Q2\nof twenty twenty six."),
        ] {
            assert_eq!(cleaner.clean_text(input), expected, "{threshold}: {input}");
        }
    }
    assert_eq!(
        cleaner_with_number_threshold(None).clean_text("A two and I one. Colossus one."),
        "A two and I one. Colossus one."
    );
}

#[test]
fn contextual_numbers_respect_existing_rule_controls() {
    let disabled = HashSet::from(["spoken-numbers".to_string()]);
    let cleaner = build_cleaner_for_test(CleaningProfile::Safe, false, &disabled, &[]);
    assert_eq!(
        cleaner.clean_text("In Q two of twenty twenty six."),
        "In Q two of twenty twenty six."
    );
    let disabled = HashSet::from(["compact-short-identifier".to_string()]);
    let cleaner = build_cleaner_for_test(CleaningProfile::Safe, false, &disabled, &[]);
    assert_eq!(
        cleaner.clean_text("In Q two of twenty twenty six."),
        "In Q 2 of 2026."
    );
}
