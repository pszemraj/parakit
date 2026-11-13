//! Full-cleaner tests for numeric literals, fractions, and time units.

use super::*;

#[test]
fn existing_numeric_magnitudes_are_not_reparsed() {
    for threshold in [0.0, 4.0, 10.0] {
        let cleaner = cleaner_with_number_threshold(Some(threshold));
        for input in [
            "Existing 5 hundred people.",
            "Existing 5 thousand people.",
            "Existing 5.5 hundred people.",
            "Existing 5 million people.",
            "Existing 5 billion people.",
        ] {
            assert_eq!(cleaner.clean_text(input), input, "threshold {threshold}");
        }
        assert_eq!(
            cleaner.clean_text("Five hundred people arrived."),
            "500 people arrived."
        );
    }
}

#[test]
fn plural_fractions_remain_worded_before_any_noun() {
    for threshold in [0.0, 4.0, 10.0] {
        let cleaner = cleaner_with_number_threshold(Some(threshold));
        for input in [
            "Five hundredths of a second.",
            "Three thousandths of a gram.",
            "Twenty thirds of the whole.",
            "Two thirds of a dollar.",
            "Three quarters of a dollar.",
            "Three-quarters of a dollar.",
            "Five hundredths of a million dollars.",
            "Two fifths.",
            "Nine tenths done.",
        ] {
            assert_eq!(cleaner.clean_text(input), input, "threshold {threshold}");
        }
        assert_eq!(
            cleaner.clean_text("The five hundredth customer arrived."),
            "The 500th customer arrived."
        );
        assert_eq!(
            cleaner.clean_text("The twenty-first of May."),
            "The 21st of May."
        );
        let elapsed = if threshold > 5.0 { "five" } else { "5" };
        assert_eq!(
            cleaner.clean_text("Wait five seconds of audio."),
            format!("Wait {elapsed} seconds of audio.")
        );
    }
}

#[test]
fn singular_fractions_remain_worded_without_hiding_ordinal_dates() {
    for threshold in [0.0, 4.0, 10.0] {
        let cleaner = cleaner_with_number_threshold(Some(threshold));
        for input in [
            "A fifth of a dollar.",
            "One fifth of the total.",
            "One fifth.",
            "A fifth.",
            "One twenty-first.",
            "One hundredth of a second.",
            "A twenty-first of the total.",
            "One twenty-first of the total.",
            "A fifth of a million dollars.",
        ] {
            assert_eq!(cleaner.clean_text(input), input, "threshold {threshold}");
        }
        let fifth = if threshold > 5.0 { "fifth" } else { "5th" };
        assert_eq!(
            cleaner.clean_text("The fifth of May."),
            format!("The {fifth} of May.")
        );
        assert_eq!(
            cleaner.clean_text("The twenty-first of May."),
            "The 21st of May."
        );
        let one = if threshold > 1.0 { "one" } else { "1" };
        assert_eq!(
            cleaner.clean_text("Wait one second of audio."),
            format!("Wait {one} second of audio.")
        );
    }
}

#[test]
fn ordinal_before_second_does_not_convert_the_time_unit() {
    for threshold in [0.0, 4.0, 10.0] {
        let cleaner = cleaner_with_number_threshold(Some(threshold));
        let fifth = if threshold > 5.0 { "fifth" } else { "5th" };
        assert_eq!(
            cleaner.clean_text("The fifth second of audio."),
            format!("The {fifth} second of audio.")
        );
        assert_eq!(
            cleaner.clean_text("Fifth second of audio."),
            format!(
                "{} second of audio.",
                if threshold > 5.0 { "Fifth" } else { "5th" }
            )
        );
        assert_eq!(
            cleaner.clean_text("The fifth second was silent."),
            format!("The {fifth} second was silent.")
        );
        assert_eq!(
            cleaner.clean_text("The fifty third second was silent."),
            "The 53rd second was silent."
        );
        assert_eq!(
            cleaner.clean_text("It was a split-second choice."),
            "It was a split-second choice."
        );
        assert_eq!(
            cleaner.clean_text("The twenty-second file."),
            "The 22nd file."
        );
        assert_eq!(
            cleaner.clean_text("The thirty second timeout."),
            "The 30 second timeout."
        );
        let fifth = if threshold > 5.0 { "fifth" } else { "5th" };
        assert_eq!(
            cleaner.clean_text("A fifth customer."),
            format!("A {fifth} customer.")
        );
    }
    let input = "Wait thirty second timeout. ".repeat(512);
    assert_eq!(
        normalize_spoken_numbers(&input, DEFAULT_NUMBER_THRESHOLD).text,
        "Wait 30 second timeout. ".repeat(512)
    );
}

#[test]
fn ordinal_lists_remain_ordinal_lists() {
    for threshold in [0.0, 4.0, 10.0] {
        let cleaner = cleaner_with_number_threshold(Some(threshold));
        let first = if threshold <= 1.0 { "1st" } else { "first" };
        let second = if threshold <= 2.0 { "2nd" } else { "second" };
        let third = if threshold <= 3.0 { "3rd" } else { "third" };
        for (input, expected) in [
            (
                "She came first second third.",
                format!("She came {first} {second} {third}."),
            ),
            (
                "She came first, second, third.",
                format!("She came {first}, {second}, {third}."),
            ),
            (
                "The first second third arrived.",
                format!("The {first} {second} {third} arrived."),
            ),
        ] {
            assert_eq!(cleaner.clean_text(input), expected, "threshold {threshold}");
        }
    }
}

#[test]
fn identifier_stems_join_only_numbers_already_rendered_as_digits() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("OS twenty seven.", "OS 27."),
            ("FP thirty two.", "FP 32."),
            ("BM twenty five.", "BM 25."),
            ("B two.", "B two."),
            ("F thirty two.", "F32."),
            ("O seven.", "O7."),
            ("SM thirty two.", "SM32."),
            ("OK five minutes ago.", "OK 5 minutes ago."),
            ("B 2 weeks.", "B2 weeks."),
            ("C 1 more time.", "C1 more time."),
            ("D 3 times.", "D3 times."),
            ("Gen Z 2 years.", "Gen Z2 years."),
            ("B two weeks.", "B two weeks."),
            ("C one more time.", "C one more time."),
            ("D three times.", "D three times."),
            ("Gen Z two years.", "Gen Z two years."),
            ("SM eight nine.", "SM 8 9."),
            ("Arc B five eighty.", "Arc B 5 80."),
            ("R five nine five.", "R 5 9 5."),
            ("P 99.9 five.", "P 99.9 5."),
            ("SM 8.2 nine.", "SM 8.2 9."),
        ],
    );
}

#[test]
fn prose_point_does_not_hide_an_independent_number() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            (
                "At this point ten GPUs were idle.",
                "At this point 10 GPUs were idle.",
            ),
            (
                "At this point five jobs finished.",
                "At this point 5 jobs finished.",
            ),
            (
                "At this point twenty five jobs finished.",
                "At this point 25 jobs finished.",
            ),
            ("Use point nine five.", "Use point nine five."),
            ("Use point oh five.", "Use point oh five."),
            ("Use point zero five.", "Use point zero five."),
            ("Use nought point five.", "Use 0.5."),
        ],
    );
}

#[test]
fn articles_and_bare_scale_words_stay_worded() {
    for threshold in [0.0, 4.0, 10.0] {
        let cleaner = cleaner_with_number_threshold(Some(threshold));
        for input in [
            "A million examples.",
            "A billion rows.",
            "A thousand files.",
            "A hundred entries.",
            "Use thousand examples.",
            "Use million examples.",
            "Several thousand examples.",
        ] {
            assert_eq!(cleaner.clean_text(input), input, "{threshold}: {input}");
        }
        assert_eq!(
            cleaner.clean_text("One million examples."),
            "1 million examples."
        );
        assert_eq!(cleaner.clean_text("Five thousand files."), "5000 files.");
    }
}

#[test]
fn typed_renderers_work_without_general_spoken_number_rendering() {
    use super::super::numbers::NumberKind;

    for (input, kind, expected) in [
        (
            "Python three point twelve.",
            NumberKind::Version,
            "Python 3.12.",
        ),
        ("In twenty twenty five.", NumberKind::Year, "In 2025."),
        ("F 32.", NumberKind::Identifier, "F32."),
        ("Capacity 7 B.", NumberKind::Size, "Capacity 7B."),
    ] {
        assert_eq!(
            numeric_test(input, 10.0, &[kind]).text,
            expected,
            "{kind:?}: {input}"
        );
    }
}

#[test]
fn adjacent_spoken_runs_preserve_every_word_when_any_count_is_below_threshold() {
    for threshold in [0.0, 4.0, 10.0] {
        let cleaner = cleaner_with_number_threshold(Some(threshold));
        for (input, digits) in [
            ("Use three twenty billion.", "Use 3 20 billion."),
            ("Use three fifty K.", "Use 3 50K."),
            ("Use one twenty eight K.", "Use 1 28K."),
            ("SM one twenty.", "SM 1 20."),
            ("Use two thirty.", "Use 2 30."),
            ("Use sixteen three eighty four.", "Use 16 3 84."),
        ] {
            assert_eq!(
                cleaner.clean_text(input),
                if threshold == 0.0 { digits } else { input },
                "{threshold}: {input}"
            );
        }
        assert_eq!(cleaner.clean_text("Use fifty ninety."), "Use 50 90.");
        assert_eq!(
            cleaner.clean_text("Use five thirty."),
            if threshold <= 5.0 {
                "Use 5 30."
            } else {
                "Use five thirty."
            }
        );
    }
}
