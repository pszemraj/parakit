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
        ("one two three hundreds", "One two three hundreds"),
        (
            "seven hundreds million parameters",
            "Seven hundreds million parameters",
        ),
        (
            "Seven hundreds MILLION billion parameters; six million rows.",
            "Seven hundreds MILLION billion parameters; 6 million rows.",
        ),
        (
            "Seven hundreds, million parameters.",
            "Seven hundreds, 1M parameters.",
        ),
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
            "A few million people, several thousand files, and a couple of hundred rows.",
            "A few million people, several thousand files, and a couple of hundred rows.",
        ),
        (
            "A few hundred thousand dollars, several hundred thousand files, and a few hundred million parameters.",
            "A few hundred thousand dollars, several hundred thousand files, and a few hundred million parameters.",
        ),
        (
            "Half a million people, a quarter of a billion rows, and two and a half million files.",
            "Half a million people, a quarter of a billion rows, and two and a half million files.",
        ),
        (
            "Three quarters of a million people, five halves of a billion rows, and one and three quarters of a million files.",
            "Three quarters of a million people, five halves of a billion rows, and one and three quarters of a million files.",
        ),
        (
            "In the year two thousand a few hundred people came.",
            "In the year 2000 a few hundred people came.",
        ),
        (
            "Few million parameters and a couple billion tokens.",
            "Few million parameters and a couple billion tokens.",
        ),
        (
            "In the early nineteen hundreds, we moved.",
            "In the early nineteen hundreds, we moved.",
        ),
        (
            "About four hundreds of them; FIVE MILLIONS of people.",
            "About four hundreds of them; FIVE MILLIONS of people.",
        ),
        (
            "Twenty-one hundreds, 5 millions, and six records.",
            "Twenty-one hundreds, 5 millions, and 6 records.",
        ),
        (
            "Sixty hundreds and one billion twenty-five millions.",
            "Sixty hundreds and one billion twenty-five millions.",
        ),
        (
            "Keep HUNDREDS of files; add three hundred records and five million rows.",
            "Keep HUNDREDS of files; add 300 records and 5 million rows.",
        ),
        (
            "Six records, five hundreds of forms, and six million rows.",
            "6 records, five hundreds of forms, and 6 million rows.",
        ),
        (
            "Six records and four hundreds of forms.",
            "6 records and four hundreds of forms.",
        ),
        ("Six, hundreds of forms.", "6, hundreds of forms."),
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
            all_numbers.clean_text("one two three hundreds"),
            "One two three hundreds"
        );
        assert_eq!(
            all_numbers.clean_text("seven hundreds million parameters"),
            "Seven hundreds million parameters"
        );
        assert_eq!(
            all_numbers.clean_text("Hundreds of files and three records."),
            "Hundreds of files and 3 records."
        );
        assert_eq!(
            all_numbers.clean_text("A few million people and six million rows."),
            "A few million people and 6 million rows."
        );
        assert_eq!(
            all_numbers.clean_text(
                "Half a million people, quarter of a billion rows, and two and a half million files."
            ),
            "Half a million people, quarter of a billion rows, and two and a half million files."
        );
        assert_eq!(
            all_numbers.clean_text(
                "A few hundred thousand dollars, several hundred thousand files, and a few hundred million parameters."
            ),
            "A few hundred thousand dollars, several hundred thousand files, and a few hundred million parameters."
        );
        assert_eq!(
            all_numbers.clean_text(
                "Three quarters of a million people and one and three quarters of a billion rows."
            ),
            "Three quarters of a million people and one and three quarters of a billion rows."
        );
        assert_eq!(
            all_numbers
                .clean_text("Three hundred thousand dollars and three hundred million parameters."),
            "300000 dollars and 300M parameters."
        );
        assert_eq!(
            all_numbers.clean_text("In the year two thousand a few hundred people came."),
            "In the year 2000 a few hundred people came."
        );
        assert_eq!(
            all_numbers.clean_text("Three records and five millions of rows."),
            "3 records and five millions of rows."
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
            cleaner.clean_text("Four hundreds of files and three hundred records."),
            "Four hundreds of files and three hundred records."
        );
    }
}
