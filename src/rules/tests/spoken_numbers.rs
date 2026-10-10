//! Cleaning-pipeline tests for spoken-number conversion, thresholds, and version components.

use super::*;

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
fn text2num_renders_signed_numbers() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("The offset is negative five.", "The offset is -5."),
            (
                "The temperature is minus twenty degrees.",
                "The temperature is -20 degrees.",
            ),
            (
                "The bound is non-negative five.",
                "The bound is non-negative 5.",
            ),
        ],
    );
}

#[test]
fn text2num_renders_ordinals_and_spoken_years() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            (
                "It was the twenty-first attempt.",
                "It was the 21st attempt.",
            ),
            ("Use the one hundredth entry.", "Use the 100th entry."),
            (
                "in the fourth quarter million-dollar deals closed",
                "In the 4th quarter million-dollar deals closed",
            ),
            (
                "January fifth thousands of people arrived",
                "January 5th thousands of people arrived",
            ),
            (
                "The release is from two thousand twenty-four.",
                "The release is from 2024.",
            ),
            (
                "The release is from twenty twenty two.",
                "The release is from twenty twenty two.",
            ),
            (
                "It happened in nineteen ninety-nine.",
                "It happened in nineteen ninety-nine.",
            ),
            (
                "Use twenty, twenty two, and ninety nine.",
                "Use 20, 22, and 99.",
            ),
            (
                "Use twenty two records and ninety nine files.",
                "Use 22 records and 99 files.",
            ),
            ("The counts are 20 22.", "The counts are 20 22."),
            ("Use twenty point five twenty point six.", "Use 20.5 20.6."),
            ("Use twenty point zero twenty point zero.", "Use 20.0 20.0."),
            ("Use twenty twenty point zero.", "Use 20 20.0."),
            ("Use twenty point zero twenty.", "Use 20.0 20."),
            ("Meet at ten thirty.", "Meet at ten thirty."),
            ("Meet at five thirty.", "Meet at five thirty."),
            ("Meet at one thirty.", "Meet at one thirty."),
            (
                "My number is five five five twenty twelve.",
                "My number is five five five twenty twelve.",
            ),
            ("Use five, thirty.", "Use 5, 30."),
            (
                "Use five thirty files and six rows.",
                "Use five thirty files and 6 rows.",
            ),
            ("Use two point zero two point zero.", "Use 2.02.0."),
            ("Use two point zero three point zero.", "Use 2.03.0."),
            ("Use two point oh three point five.", "Use 2.03.5."),
            ("Use two point five two point five.", "Use 2.52.5."),
            ("Use two point five three point six.", "Use 2.53.6."),
            ("Use two point zero, three point zero.", "Use 2.0, 3.0."),
            (
                "We need twenty ten-cent stamps.",
                "We need 20 ten-cent stamps.",
            ),
            (
                "We need twenty twenty-two-cent stamps.",
                "We need 20 twenty-two-cent stamps.",
            ),
        ],
    );
}

#[test]
fn large_spoken_magnitudes_use_readable_hybrid_notation() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            (
                "The company is worth three billion dollars.",
                "The company is worth 3 billion dollars.",
            ),
            (
                "The budget is one hundred twenty-five million dollars.",
                "The budget is 125 million dollars.",
            ),
            (
                "The estimate is three point five billion dollars.",
                "The estimate is 3.5 billion dollars.",
            ),
            (
                "The existing estimate is 3.5 billion dollars.",
                "The existing estimate is 3.5 billion dollars.",
            ),
        ],
    );
}

#[test]
fn equal_scales_joined_by_and_remain_separate_quantities() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            (
                "Between five thousand and ten thousand rows.",
                "Between 5000 and 10000 rows.",
            ),
            (
                "Use two point five million and three point six million rows.",
                "Use 2.5 million and 3.6 million rows.",
            ),
            ("Use five hundred and six hundred.", "Use 500 and 600."),
            ("Use one hundred and five thousand.", "Use 105000."),
            ("Use two million and five.", "Use 2000005."),
            (
                "Use one hundred and twenty thousand five hundred.",
                "Use 120500.",
            ),
            (
                "Between two hundred and fifty thousand and three hundred thousand rows.",
                "Between 250000 and 300000 rows.",
            ),
            (
                "Between five thousand and ten thousand five hundred rows.",
                "Between 5000 and 10500 rows.",
            ),
        ],
    );
}

/// Assert each case at the default threshold and at `0` and `10`.
fn assert_clean_at_thresholds(cases: &[(&str, &str)]) {
    let failures: Vec<String> = [None, Some(0.0), Some(10.0)]
        .into_iter()
        .flat_map(|threshold| {
            let cleaner = cleaner_with_number_threshold(threshold);
            cases.iter().filter_map(move |(input, expected)| {
                let actual = cleaner.clean_text(input);
                (actual != *expected).then(|| {
                    format!("{threshold:?} {input:?}: expected {expected:?}, got {actual:?}")
                })
            })
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_scale_takes_the_count_spoken_before_it() {
    // text2num ends a number at the first scale it cannot apply, which left
    // that scale bare: "1502 1500" and "203 100".
    assert_clean_at_thresholds(&[
        (
            "Fifteen hundred and two thousand five hundred.",
            "1500 and 2500.",
        ),
        (
            "It costs fifteen hundred two thousand five hundred.",
            "It costs 1500 2500.",
        ),
        ("Rows two hundred three hundred.", "Rows 200 300."),
        ("Rows five hundred five hundred.", "Rows 500 500."),
        ("Use one thousand two thousand.", "Use 1000 2000."),
        ("Use ten thousand two thousand.", "Use 10000 2000."),
        (
            "Use two hundred thousand three hundred thousand.",
            "Use 200000 300000.",
        ),
        ("Use two hundred five hundred thousand.", "Use 200 500000."),
        (
            "Use five million three million.",
            "Use 5 million 3 million.",
        ),
        (
            "Use twenty-five hundred and two thousand.",
            "Use 2500 and 2000.",
        ),
    ]);
}

#[test]
fn and_after_a_count_starts_another_number() {
    assert_clean_at_thresholds(&[
        (
            "Between two hundred and fifty and three hundred rows.",
            "Between 250 and 300 rows.",
        ),
        (
            "Between two hundred and fifty and three hundred and fifty dollars.",
            "Between 250 and 350 dollars.",
        ),
        (
            "Between twenty and five degrees.",
            "Between 20 and 5 degrees.",
        ),
        ("Use fifty and two hundred.", "Use 50 and 200."),
        (
            "Use one hundred and twenty and five hundred.",
            "Use 120 and 500.",
        ),
        (
            "Use fifty and two point five million.",
            "Use 50 and 2.5 million.",
        ),
        // Without the article, text2num scaled the count itself: 2000, 1500.
        (
            "Somewhere between twenty and hundred.",
            "Somewhere between 20 and 100.",
        ),
        (
            "Between fifteen and hundred rows.",
            "Between 15 and 100 rows.",
        ),
    ]);
}

#[test]
fn ambiguous_or_stacked_scales_keep_their_wording() {
    // Both "100 2300" and "120 300" are grammatical, and "fifteen hundred
    // thousand" may stack into 1500000; none of them is a bare scale.
    assert_clean_at_thresholds(&[
        (
            "Use one hundred twenty three hundred.",
            "Use one hundred twenty three hundred.",
        ),
        (
            "Use two thousand twenty three hundred.",
            "Use two thousand twenty three hundred.",
        ),
        (
            "Use fifteen hundred thousand.",
            "Use fifteen hundred thousand.",
        ),
        (
            "Use twenty five hundred thousand.",
            "Use twenty five hundred thousand.",
        ),
    ]);
}

#[test]
fn scale_coefficient_splits_leave_valid_compounds_alone() {
    assert_clean_at_thresholds(&[
        ("Use one hundred and five thousand.", "Use 105000."),
        (
            "Use one hundred and twenty thousand five hundred.",
            "Use 120500.",
        ),
        (
            "Use one million two hundred and five thousand.",
            "Use 1205000.",
        ),
        ("It was two thousand and five.", "It was 2005."),
        ("Nine thousand four hundred and fifty-three.", "9453."),
        ("Use twenty-three hundred.", "Use 2300."),
        (
            "Between five thousand and ten thousand five hundred rows.",
            "Between 5000 and 10500 rows.",
        ),
        (
            "Between two hundred and fifty thousand and three hundred thousand rows.",
            "Between 250000 and 300000 rows.",
        ),
        (
            "Use two point five million and three point six million rows.",
            "Use 2.5 million and 3.6 million rows.",
        ),
        (
            "Windows ten point zero point nineteen thousand forty five.",
            "Windows 10.0.19045.",
        ),
        ("The year twenty twenty two.", "The year twenty twenty two."),
        ("Use twenty and a half.", "Use 20 and a half."),
        (
            "About two and a half million users.",
            "About two and a half million users.",
        ),
        ("A few hundred.", "A few hundred."),
        ("In the nineteen hundreds.", "In the nineteen hundreds."),
    ]);
}

#[test]
fn large_magnitude_formatting_preserves_compounds_and_numeric_literals() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            (
                "The total is three billion five hundred million.",
                "The total is 3500000000.",
            ),
            (
                "The identifier is 3000000000.",
                "The identifier is 3000000000.",
            ),
        ],
    );
}

#[test]
fn parameter_counts_use_compact_magnitude_suffixes() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            (
                "Use a hundred million up to one billion parameters.",
                "Use 100M up to 1B parameters.",
            ),
            (
                "The model has 125 million parameters.",
                "The model has 125M parameters.",
            ),
            (
                "The model has 1500000000 parameters.",
                "The model has 1.5B parameters.",
            ),
            (
                "The model has 100000000 parameters.",
                "The model has 100M parameters.",
            ),
            (
                "The model has 123456789 parameters.",
                "The model has 123456789 parameters.",
            ),
            (
                "The company is worth three billion dollars.",
                "The company is worth 3 billion dollars.",
            ),
            (
                "The dataset has a million examples.",
                "The dataset has 1 million examples.",
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
    let cleaner = cleaner_with_number_threshold(None);
    assert_eq!(cleaner.number_threshold(), DEFAULT_NUMBER_THRESHOLD);
    for (input, expected) in [
        ("Zero files remain.", "Zero files remain."),
        (
            "One thing and two more ideas.",
            "One thing and two more ideas.",
        ),
        ("There are four folders.", "There are 4 folders."),
        ("There are five folders.", "There are 5 folders."),
        (
            "One file. Three notes. Four folders. Ten tasks.",
            "One file. Three notes. 4 folders. 10 tasks.",
        ),
    ] {
        assert_eq!(cleaner.clean_text(input), expected, "input: {input}");
    }
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
fn fraction_guards_preserve_numerators_without_absorbing_independent_counts() {
    for threshold in [0.0, DEFAULT_NUMBER_THRESHOLD, 5.0] {
        for (input, expected) in [
            ("three-quarters of a million", "three-quarters of a million"),
            ("one-half of a million", "one-half of a million"),
            ("three quarters of a million", "three quarters of a million"),
            ("five thirds of a million", "five thirds of a million"),
            ("four fifths of a million", "four fifths of a million"),
            (
                "ten seconds of a million rows",
                "10 seconds of 1 million rows",
            ),
            (
                "ten SECONDS of a billion rows",
                "10 SECONDS of 1 billion rows",
            ),
            (
                "ten seconds and five thirds of a million",
                "10 seconds and five thirds of a million",
            ),
            ("five-thirds of a million", "five-thirds of a million"),
            (
                "five twenty-firsts of a million",
                "five twenty-firsts of a million",
            ),
            (
                "five and four fifths of a billion",
                "five and four fifths of a billion",
            ),
            ("the first of a million", "the first of a million"),
            ("the fifth of a hundred", "the fifth of a hundred"),
            ("six quarters a million", "six quarters a million"),
            (
                "in the last six quarters a million users joined",
                "in the last six quarters a million users joined",
            ),
            ("six quarters, a million", "6 quarters, 1 million"),
            (
                "between two hundred and half a million",
                "between two hundred and half a million",
            ),
            (
                "we gained two hundred and half a million more",
                "we gained two hundred and half a million more",
            ),
            (
                "In the fourth quarter a million users signed up",
                if threshold > 4.0 {
                    "In the fourth quarter a million users signed up"
                } else {
                    "In the 4th quarter a million users signed up"
                },
            ),
            (
                "In the fourth quarter, a million users signed up",
                if threshold > 4.0 {
                    "In the fourth quarter, 1 million users signed up"
                } else {
                    "In the 4th quarter, 1 million users signed up"
                },
            ),
            (
                "In the second half a million users signed up",
                if threshold > 2.0 {
                    "In the second half a million users signed up"
                } else {
                    "In the 2nd half a million users signed up"
                },
            ),
            (
                "In the second half, a million users signed up",
                if threshold > 2.0 {
                    "In the second half, 1 million users signed up"
                } else {
                    "In the 2nd half, 1 million users signed up"
                },
            ),
            ("a quarter a million rows", "a quarter a million rows"),
            ("one quarter a million rows", "one quarter a million rows"),
            ("one-quarter a million rows", "one-quarter a million rows"),
            ("quarter of a million rows", "quarter of a million rows"),
            ("a half a million rows", "a half a million rows"),
            ("half a million rows", "half a million rows"),
            (
                "in the year two thousand and five half a million people",
                "in the year 2005 half a million people",
            ),
            (
                "between two thousand and half a million",
                "between two thousand and half a million",
            ),
            (
                "both five and half a million parameters are allowed",
                "both five and half a million parameters are allowed",
            ),
            (
                "between about five and half a million parameters",
                "between about five and half a million parameters",
            ),
            (
                "between nearly five and half a million parameters",
                "between nearly five and half a million parameters",
            ),
            (
                "between almost five and half a million parameters",
                "between almost five and half a million parameters",
            ),
            (
                "the models are both about five and a half million parameters",
                "the models are both about five and a half million parameters",
            ),
            (
                "they are both roughly five and a half million parameters",
                "they are both roughly five and a half million parameters",
            ),
            (
                "models are both five and a half million parameters",
                "models are both five and a half million parameters",
            ),
            (
                "we chose between models, then five and a half million parameters",
                "we chose between models, then five and a half million parameters",
            ),
            (
                "I like them both\nfive and a half million parameters fit",
                "I like them both\nfive and a half million parameters fit",
            ),
            (
                "between about\r\nfive and a half million parameters fit",
                "between about\r\nfive and a half million parameters fit",
            ),
            (
                "chapter five half a million words long",
                "chapter 5 half a million words long",
            ),
            (
                "Both five and half a million. Both five and half a million.",
                "Both five and half a million. Both five and half a million.",
            ),
            (
                "Count them; both five and half a million are allowed",
                "Count them; both five and half a million are allowed",
            ),
            (
                "Count them, both about five and half a million are allowed",
                "Count them, both about five and half a million are allowed",
            ),
            (
                "Next option:\nBoth five and half a million",
                "Next option:\nBoth five and half a million",
            ),
            (
                "January fifth thousands of people",
                "January 5th thousands of people",
            ),
            (
                "a million and a half parameters",
                "a million and a half parameters",
            ),
            (
                "two million and a half parameters",
                "two million and a half parameters",
            ),
            (
                "five million and a quarter rows",
                "five million and a quarter rows",
            ),
            ("a hundred and a half rows", "a hundred and a half rows"),
            ("It is a third of a million", "It is a third of a million"),
            ("A fifth of a billion rows", "A fifth of a billion rows"),
            ("one third of a million", "one third of a million"),
            (
                "five and a third of a million",
                "five and a third of a million",
            ),
            (
                "Both five and a third of a million",
                "Both five and a third of a million",
            ),
            (
                "chapter five a third of a million words long",
                "chapter 5 a third of a million words long",
            ),
            (
                "a twenty-first of a million rows",
                "a twenty-first of a million rows",
            ),
            (
                "a twenty first of a million rows",
                "a twenty first of a million rows",
            ),
            (
                "a one hundredth of a billion rows",
                "a one hundredth of a billion rows",
            ),
            (
                "a one-hundredth of a billion rows",
                "a one-hundredth of a billion rows",
            ),
            (
                "five and a twenty first of a million rows",
                "five and a twenty first of a million rows",
            ),
            (
                "chapter five a twenty first of a million words",
                "chapter 5 a twenty first of a million words",
            ),
            ("a group of a million rows", "a group of 1 million rows"),
            (
                "hundreds million and a half rows",
                "hundreds million and a half rows",
            ),
            (
                "year two thousand a million and a half parameters",
                "year 2000 a million and a half parameters",
            ),
            (
                "chapter five a million and a half words",
                "chapter 5 a million and a half words",
            ),
            (
                "nineteen hundreds of a million",
                "nineteen hundreds of 1 million",
            ),
            ("five millions of a billion", "five millions of 1 billion"),
            ("a hundreds of a million", "a hundreds of 1 million"),
            (
                "half a million both five and a half million",
                "half a million both five and a half million",
            ),
            ("two and a half million", "two and a half million"),
            ("one and half a million", "one and half a million"),
            ("one half million", "one half million"),
            ("one quarter of a million", "one quarter of a million"),
            (
                "five and one quarter million",
                "five and one quarter million",
            ),
            (
                "one and three quarters of a million",
                "one and three quarters of a million",
            ),
            (
                "a few hundred-thousand dollars",
                "a few hundred-thousand dollars",
            ),
        ] {
            assert_eq!(
                super::numbers::normalize_spoken_numbers(input, threshold).text,
                expected,
                "{threshold}: {input}"
            );
        }
    }
    assert_eq!(
        super::numbers::normalize_spoken_numbers("chapter five half a million words long", 10.0)
            .text,
        "chapter five half a million words long"
    );
}

#[test]
fn line_breaks_end_spoken_numbers_and_their_guards() {
    for threshold in [0.0, DEFAULT_NUMBER_THRESHOLD, 5.0] {
        for (input, expected) in [
            (
                "seven hundred\n\nthousands of people",
                "700\n\nthousands of people",
            ),
            (
                "a few hundred\nthousands of people",
                "a few hundred\nthousands of people",
            ),
            ("seven hundred\nthousand people", "700\n1000 people"),
            ("twenty\nfive people", "20\n5 people"),
            ("seven hundred\r\nthousand people", "700\r\n1000 people"),
            ("minus\nfive", "minus\n5"),
        ] {
            assert_eq!(
                super::numbers::normalize_spoken_numbers(input, threshold).text,
                expected,
                "{threshold}: {input:?}"
            );
        }
    }
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
            (
                "Windows ten point zero point nineteen thousand forty five.",
                "Windows 10.0.19045.",
            ),
            (
                "Ubuntu twenty two point zero four point three.",
                "Ubuntu 22.04.3.",
            ),
            ("Python three point one two point four.", "Python 3.12.4."),
            (
                "Use one hundred ninety two point one hundred sixty eight point zero point one.",
                "Use 192.168.0.1.",
            ),
            ("CUDA 12 point nine.", "CUDA 12.9."),
            ("B0.1 point two.", "B0.1.2."),
            ("B1 point zero-five point two.", "B1.05.2."),
            ("B1 point oh-five point two.", "B1.05.2."),
            ("Use 20 point zero twenty point zero.", "Use 20.0 20.0."),
            ("V one point zero five point two.", "v1.05.2."),
            ("V one point zero zero five point two.", "v1.005.2."),
            ("V one point oh five point two.", "v1.05.2."),
            ("V one point zero-five point two.", "v1.05.2."),
            ("V two point zero two point zero.", "v2.02.0."),
            ("V two point five two point zero.", "v2.52.0."),
            (
                "Version two point zero three point zero.",
                "Version 2.03.0.",
            ),
            ("V two point zero zero three point zero.", "v2.003.0."),
            (
                "V one point twenty one point one hundred twenty.",
                "v1.21.120.",
            ),
        ],
    );
}
