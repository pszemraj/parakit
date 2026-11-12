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
    }
}

#[test]
fn ordinal_lists_remain_ordinal_lists() {
    for threshold in [0.0, 4.0, 10.0] {
        let cleaner = cleaner_with_number_threshold(Some(threshold));
        for (input, expected) in [
            ("She came first second third.", "She came 1st 2nd 3rd."),
            ("She came first, second, third.", "She came 1st, 2nd, 3rd."),
            (
                "The first second third arrived.",
                "The 1st 2nd 3rd arrived.",
            ),
        ] {
            assert_eq!(cleaner.clean_text(input), expected, "threshold {threshold}");
        }
    }
}
