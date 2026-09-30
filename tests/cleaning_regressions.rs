//! Public-facade smoke coverage for the shipped cleaning pipeline.

use parakit::rules::{build_cleaner, CleaningProfile};

#[test]
fn public_builder_runs_the_shipped_default_pipeline() {
    let cleaner = build_cleaner(false, CleaningProfile::Safe, true, None, &[], &[])
        .expect("cleaner should compile")
        .expect("cleaning should be enabled");

    assert_eq!(
        cleaner.clean_text("um, the the G P T model is gonna work."),
        "The GPT model is going to work"
    );
}

#[test]
fn plural_magnitudes_preserve_words_in_both_profiles_and_thresholds() {
    let cases = [
        (
            "Hundreds! Thousands? MILLIONS; billions.",
            "Hundreds! Thousands? MILLIONS; billions.",
        ),
        ("Hundreds of files.", "Hundreds of files."),
        (
            "There are thousands of people.",
            "There are thousands of people.",
        ),
        (
            "Millions, billions, and trillions remain.",
            "Millions, billions, and trillions remain.",
        ),
        (
            "Tens of thousands and hundreds of millions.",
            "Tens of thousands and hundreds of millions.",
        ),
        (
            "Keep HUNDREDS of files; add three hundred records and five million rows.",
            "Keep HUNDREDS of files; add 300 records and 5 million rows.",
        ),
        (
            "Use the second batch of hundreds after one second.",
            "Use the second batch of hundreds after one second.",
        ),
        (
            "Use version one point two point three across thousands of files.",
            "Use version 1.2.3 across thousands of files.",
        ),
    ];
    for profile in [CleaningProfile::Safe, CleaningProfile::Aggressive] {
        for threshold in [None, Some(5.0)] {
            let cleaner = build_cleaner(false, profile, false, threshold, &[], &[])
                .unwrap()
                .unwrap();
            for (input, expected) in cases {
                assert_eq!(
                    cleaner.clean_text(input),
                    expected,
                    "{profile:?} {threshold:?}: {input}"
                );
            }
        }
        let all_numbers = build_cleaner(false, profile, false, Some(0.0), &[], &[])
            .unwrap()
            .unwrap();
        assert_eq!(
            all_numbers.clean_text("Hundreds of files and three records."),
            "Hundreds of files and 3 records."
        );
        assert_eq!(
            all_numbers.clean_text("Use the second batch of hundreds after one second."),
            "Use the 2nd batch of hundreds after 1 second."
        );
    }
}

#[test]
fn disabling_spoken_numbers_preserves_both_plural_and_exact_quantities() {
    for profile in [CleaningProfile::Safe, CleaningProfile::Aggressive] {
        let cleaner = build_cleaner(
            false,
            profile,
            false,
            Some(0.0),
            &["spoken-numbers".into()],
            &[],
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            cleaner.clean_text("Hundreds of files and three hundred records."),
            "Hundreds of files and three hundred records."
        );
    }
}
