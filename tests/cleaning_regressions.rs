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
            "A few hundred-thousand dollars, several hundred-million files, and a couple of hundred-billion tokens.",
            "A few hundred-thousand dollars, several hundred-million files, and a couple of hundred-billion tokens.",
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
            "One quarter of a million dollars, one half million parameters, and five and one quarter million tokens.",
            "One quarter of a million dollars, one half million parameters, and five and one quarter million tokens.",
        ),
        (
            "In the year two thousand a few hundred people came.",
            "In the year 2000 a few hundred people came.",
        ),
        (
            "In the year two thousand half a million people came.",
            "In the year 2000 half a million people came.",
        ),
        (
            "In the year two thousand and five half a million people came.",
            "In the year 2005 half a million people came.",
        ),
        (
            "Between two thousand and half a million people came.",
            "Between two thousand and half a million people came.",
        ),
        (
            "Both five and half a million parameters are allowed.",
            "Both five and half a million parameters are allowed.",
        ),
        (
            "Between about five and half a million parameters are allowed.",
            "Between about five and half a million parameters are allowed.",
        ),
        (
            "Between nearly five and half a million parameters are allowed.",
            "Between nearly five and half a million parameters are allowed.",
        ),
        (
            "Between almost five and half a million parameters are allowed.",
            "Between almost five and half a million parameters are allowed.",
        ),
        (
            "The models are both about five and a half million parameters.",
            "The models are both about five and a half million parameters.",
        ),
        (
            "Chapter five half a million words long.",
            "Chapter 5 half a million words long.",
        ),
        (
            "In the year two thousand quarter of a million people came.",
            "In the year 2000 quarter of a million people came.",
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
        for (input, expected) in [
            (
                "In the year two thousand and five half a million people came.",
                "In the year 2005 half a million people came.",
            ),
            (
                "Between two thousand and half a million people came.",
                "Between two thousand and half a million people came.",
            ),
            (
                "Chapter five half a million words long.",
                "Chapter 5 half a million words long.",
            ),
            (
                "A few hundred-thousand dollars.",
                "A few hundred-thousand dollars.",
            ),
        ] {
            assert_eq!(
                all_numbers.clean_text(input),
                expected,
                "{profile:?}: {input}"
            );
        }
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
                .clean_text("One quarter million dollars and one and one half billion parameters."),
            "One quarter million dollars and one and one half billion parameters."
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
            all_numbers.clean_text("In the year two thousand half a million people came."),
            "In the year 2000 half a million people came."
        );
        assert_eq!(
            all_numbers.clean_text("In the year two thousand quarter of a million people came."),
            "In the year 2000 quarter of a million people came."
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
fn ambiguous_quantities_remain_words_across_profiles_and_thresholds() {
    let cases = [
        (
            "Five thirds of a million rows.",
            "Five thirds of a million rows.",
        ),
        (
            "Four fifths of a million rows.",
            "Four fifths of a million rows.",
        ),
        (
            "The fifth of a hundred children.",
            "The fifth of a hundred children.",
        ),
        (
            "Six quarters a million users joined.",
            "Six quarters a million users joined.",
        ),
        (
            "Between two hundred and half a million.",
            "Between two hundred and half a million.",
        ),
        (
            "We gained two hundred and half a million more.",
            "We gained two hundred and half a million more.",
        ),
        (
            "The release was in twenty twenty two.",
            "The release was in twenty twenty two.",
        ),
        (
            "It happened in nineteen ninety-nine.",
            "It happened in nineteen ninety-nine.",
        ),
        (
            "Six quarters, a million users joined.",
            "6 quarters, 1 million users joined.",
        ),
        (
            "Use twenty, twenty two, and ninety nine.",
            "Use 20, 22, and 99.",
        ),
        (
            "The release was in two thousand twenty two.",
            "The release was in 2022.",
        ),
        ("Use twenty point five twenty point six.", "Use 20.5 20.6."),
        ("Use twenty point zero twenty point zero.", "Use 20.0 20.0."),
        ("Meet at ten thirty.", "Meet at ten thirty."),
        ("Meet at five thirty.", "Meet at five thirty."),
        ("Meet at one thirty.", "Meet at one thirty."),
        (
            "My number is five five five twenty twelve.",
            "My number is five five five twenty twelve.",
        ),
        ("Use two point zero two point zero.", "Use 2.0 2.0."),
        ("Use two point zero three point zero.", "Use 2.0 3.0."),
        ("Use two point oh three point five.", "Use 2.0 3.5."),
        ("Use two point five two point five.", "Use 2.5 2.5."),
        ("Use two point five three point six.", "Use 2.5 3.6."),
        ("Use two point zero zero three point zero.", "Use 2.00 3.0."),
        (
            "The value is three point one four one five.",
            "The value is 3.1415.",
        ),
        ("V two point zero three point zero.", "v2.03.0."),
        ("V two point five two point zero.", "v2.52.0."),
        (
            "Version two point zero zero three point zero.",
            "Version 2.003.0.",
        ),
        ("B1 point zero-three point zero.", "B1.03.0."),
        (
            "Ten seconds of a million rows.",
            "10 seconds of 1 million rows.",
        ),
        (
            "We need twenty ten-cent stamps.",
            "We need 20 ten-cent stamps.",
        ),
        (
            "We need twenty twenty-two-cent stamps.",
            "We need 20 twenty-two-cent stamps.",
        ),
    ];
    for profile in [CleaningProfile::Safe, CleaningProfile::Aggressive] {
        for threshold in [0.0, 4.0, 5.0] {
            let cleaner = build_cleaner(false, profile, false, Some(threshold), &[], &[])
                .unwrap()
                .unwrap();
            for (input, expected) in cases {
                assert_eq!(
                    cleaner.clean_text(input),
                    expected,
                    "{profile:?} {threshold}: {input}"
                );
            }
        }
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
