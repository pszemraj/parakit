//! Century pairs form years without calendar or product context cues.

use super::*;

#[test]
fn century_pairs_form_years_without_context_or_threshold_bypasses() {
    for profile in [CleaningProfile::Safe, CleaningProfile::Aggressive] {
        for threshold in [0.0, 4.0, 10.0] {
            let cleaner = build_cleaner_for_test_with_threshold(
                profile,
                false,
                Some(threshold),
                &HashSet::new(),
                &[],
            );
            for (input, expected) in [
                ("Twenty twenty six.", "2026."),
                ("Record twenty twenty six.", "Record 2026."),
                ("Nineteen ninety-nine.", "1999."),
                ("Eighteen seventy six.", "1876."),
                ("Nineteen oh five.", "1905."),
                ("Twenty o six.", "2006."),
                ("Twenty oh zero.", "2000."),
                ("Use 20 twenty six.", "Use 2026."),
                ("Use twenty 26.", "Use 2026."),
                ("Use 20 26.", "Use 20 26."),
                ("Use 19 oh 5.", "Use 1905."),
                ("Use 18 76.", "Use 18 76."),
                (
                    "In twenty twenty five and twenty twenty.",
                    "In 2025 and 2020.",
                ),
                (
                    "From nineteen thirty until twenty fifteen.",
                    "From 1930 until 2015.",
                ),
                ("Use twenty twenty six files.", "Use 2026 files."),
                ("In twenty twenty six in May.", "In 2026 in May."),
                (
                    "Born in nineteen ninety one in Ohio.",
                    "Born in 1991 in Ohio.",
                ),
                (
                    "In twenty twenty six a new era began.",
                    "In 2026 a new era began.",
                ),
                ("In twenty twenty six am I ready?", "In 2026 am I ready?"),
                ("In twenty twenty six us too.", "In 2026 us too."),
                ("May twenty sixth twenty twenty six.", "May 26th 2026."),
                (
                    "Q four twenty twenty five.",
                    if threshold <= 4.0 {
                        "Q4 2025."
                    } else {
                        "Q four 2025."
                    },
                ),
                ("Q4 twenty twenty five.", "Q4 2025."),
                ("Twenty uh twenty layers.", "20 20 layers."),
                ("The year seventeen seventy-six.", "The year 17 76."),
                ("The year 19 06.", "The year 19 06."),
                ("The year 020 26.", "The year 020 26."),
                ("The year 20 026.", "The year 20 026."),
                ("In twenty\nthirty.", "In 20\n30."),
                (
                    "The year twenty point zero twenty six.",
                    "The year 20.0 26.",
                ),
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
fn following_time_and_measurement_units_block_year_joining() {
    let cleaner = cleaner_keep_period(CleaningProfile::Safe);
    for (input, expected) in [
        ("In twenty thirty minutes.", "In 20 30 minutes."),
        ("Use twenty thirty weeks.", "Use 20 30 weeks."),
        ("Use twenty thirty months.", "Use 20 30 months."),
        ("Use twenty thirty years.", "Use 20 30 years."),
        ("Use twenty thirty meters.", "Use 20 30 meters."),
        ("Use twenty thirty kilograms.", "Use 20 30 kilograms."),
        ("Until nineteen thirty PM.", "Until 19 30 PM."),
        ("Until nineteen thirty p.m.", "Until 19 30 p.m."),
        ("Until nineteen thirty AM.", "Until 19 30 AM."),
        ("Use twenty twenty six GB.", "Use 20 26 GB."),
        ("Use twenty twenty six MB.", "Use 20 26 MB."),
        ("Use twenty twenty six Hz.", "Use 20 26 Hz."),
        ("Use twenty twenty six gigabytes.", "Use 20 26 gigabytes."),
        ("Use twenty twenty six percent.", "Use 20 26 percent."),
        ("Use twenty thirty K.", "Use 20 30K."),
        ("Use twenty thirty M.", "Use 20 30M."),
        ("Use twenty thirty B.", "Use 20 30B."),
        (
            "A twenty thirty-minute interval.",
            "A 20 30-minute interval.",
        ),
    ] {
        assert_eq!(cleaner.clean_text(input), expected, "{input}");
    }
}

#[test]
fn dates_and_identifier_words_follow_the_number_threshold() {
    for threshold in [0.0, 4.0, 10.0] {
        let cleaner = cleaner_with_number_threshold(Some(threshold));
        for (input, expected) in [
            (
                "Q two of twenty twenty six.",
                if threshold <= 2.0 {
                    "Q2 of 2026."
                } else {
                    "Q two of 2026."
                },
            ),
            ("Q five.", if threshold <= 5.0 { "Q5." } else { "Q five." }),
            (
                "April first, twenty twenty four.",
                if threshold <= 1.0 {
                    "April 1st, 2024."
                } else {
                    "April first, 2024."
                },
            ),
            ("May twenty sixth.", "May 26th."),
            (
                "Use B two and SM one.",
                if threshold <= 1.0 {
                    "Use B2 and SM1."
                } else {
                    "Use B two and SM one."
                },
            ),
            (
                "Wait five o'clock.",
                if threshold <= 5.0 {
                    "Wait 5 o'clock."
                } else {
                    "Wait five o'clock."
                },
            ),
            (
                "Wait two o'clock.",
                if threshold <= 2.0 {
                    "Wait 2 o'clock."
                } else {
                    "Wait two o'clock."
                },
            ),
        ] {
            assert_eq!(cleaner.clean_text(input), expected, "{threshold}: {input}");
        }
    }
}

#[test]
fn calendar_and_identifier_renderers_have_independent_controls() {
    let disabled = HashSet::from(["spoken-numbers".to_string()]);
    let cleaner = build_cleaner_for_test(CleaningProfile::Safe, false, &disabled, &[]);
    assert_eq!(
        cleaner.clean_text("Q two of twenty twenty six."),
        "Q two of 2026."
    );
    assert_eq!(
        cleaner.clean_text("Q five of twenty twenty six."),
        "Q five of 2026."
    );
    assert_eq!(
        cleaner.clean_text("Q 5 of twenty twenty six."),
        "Q5 of 2026."
    );

    let disabled = HashSet::from(["numeric-identifiers".to_string()]);
    let cleaner = build_cleaner_for_test(CleaningProfile::Safe, false, &disabled, &[]);
    assert_eq!(
        cleaner.clean_text("Q five of twenty twenty six."),
        "Q 5 of 2026."
    );

    let disabled = HashSet::from(["calendar-years".to_string()]);
    let cleaner = build_cleaner_for_test(CleaningProfile::Safe, false, &disabled, &[]);
    assert_eq!(
        cleaner.clean_text("Q five of twenty twenty six."),
        "Q5 of 20 26."
    );
    assert_eq!(
        cleaner.clean_text("Q four twenty twenty five."),
        "Q 4 20 25."
    );
}
