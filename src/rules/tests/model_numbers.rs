//! Grouped model numbers and nearby ordinary quantities.

use super::*;

#[test]
fn local_model_references_join_grouped_numbers() {
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
                (
                    "metal GPU and a fucking fifty ninety sitting here",
                    "Metal GPU and a fucking 5090 sitting here",
                ),
                ("GPU and my old forty ninety.", "GPU and my old 4090."),
                ("GPU and a twenty eighty.", "GPU and a 2080."),
                ("GPU and a ten eighty.", "GPU and a 1080."),
                (
                    "GPU and a ten fifty sitting here.",
                    "GPU and a 1050 sitting here.",
                ),
                ("GPU and a twenty fifty.", "GPU and a 2050."),
                (
                    "GPU, and a fucking fifty ninety.",
                    "GPU, and a fucking 5090.",
                ),
                ("GPU fifty ninety.", "GPU 5090."),
                ("gpu fifty ninety.", "Gpu 5090."),
                ("GPU 50 ninety.", "GPU 5090."),
                ("GPU fifty 90.", "GPU 5090."),
                ("GPU fifty oh five.", "GPU 5005."),
                ("GPU fifty zero five.", "GPU 5005."),
                ("GPU fifty 05.", "GPU 5005."),
                ("GPU and a fifty ninety Ti.", "GPU and a 5090 Ti."),
                ("A fifty ninety Ti GPU.", "A 5090 Ti GPU."),
                ("A fifty ninety-Ti GPU.", "A 5090-Ti GPU."),
                ("GPU fifty ninety-Ti.", "GPU 5090-Ti."),
                (
                    "GPU and a fifty ninety XT sitting here.",
                    "GPU and a 5090 XT sitting here.",
                ),
                ("A fifty ninety GPU.", "A 5090 GPU."),
                ("A forty ninety video card.", "A 4090 video card."),
                ("The graphics card fifty eighty.", "The graphics card 5080."),
                ("CPU fifty ninety.", "CPU 5090."),
                ("Processor ninety five thirty.", "Processor 9530."),
                ("Model fifty ninety.", "Model 5090."),
                ("Part number fifty ninety.", "Part number 5090."),
                ("GPU and a new RTX fifty ninety.", "GPU and a new RTX 5090."),
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
fn grouped_model_numbers_preserve_other_number_contexts() {
    for profile in [CleaningProfile::Safe, CleaningProfile::Aggressive] {
        let cleaner = cleaner_keep_period(profile);
        for input in [
            "Meet at ten thirty.",
            "The score was fifty ninety.",
            "Run fifty ninety models.",
            "We ordered fifty ninety GPUs.",
            "We ordered fifty ninety graphics cards.",
            "RTX fifty ninety.",
            "USA fifty ninety.",
            "GPU and a time of nineteen thirty.",
            "GPU and a nineteen thirty appointment.",
            "GPU and the score was fifty ninety.",
            "GPU and a score fifty ninety.",
            "GPU and a fifty ninety minute delay.",
            "GPU and a fifty ninety dollar charge.",
            "GPU and a fifty ninety meter cable.",
            "GPU and a fifty ninety kilograms.",
            "GPU and a fifty ninety discount.",
            "GPU and a fifty ninety running hours.",
            "GPU and a fifty ninety second delay.",
            "GPU fifty ninety second delay.",
            "GPU and a fifty ninety degree temperature.",
            "GPU and a fifty ninety percent discount.",
            "GPU and a fifty ninety K context.",
            "GPU fifty ninety MHz.",
            "GPU fifty ninety GB.",
            "GPU fifty ninety Kg.",
            "GPU fifty ninety USD.",
            "GPU fifty ninety FPS.",
            "GPU fifty ninety RPM.",
            "GPU fifty ninety parameters.",
            "GPU fifty ninety hundredths of a second.",
            "GPU fifty ninety seventy.",
            "GPU thirty fifty ninety.",
            "GPU 50 90 100.",
            "GPU 50 90 one.",
            "GPU 50 90 hundredths of a second.",
            "GPU 50 90 quarters of a dollar.",
            "GPU 50 90-minute delay.",
            "GPU 50 90 FPS.",
            "GPU 50 90 meter cable.",
            "GPU fifty-ninety-cent parts.",
            "GPU. A fifty ninety sitting here.",
            "GPU\nA fifty ninety sitting here.",
            "Llama thirteen seventy B.",
            "SM one twenty.",
            "GPU 50 90.2.",
        ] {
            assert_eq!(cleaner.clean_text(input), input, "{profile:?}: {input}");
        }
        assert_eq!(
            cleaner.clean_text("GPU fifty ninety-minute delay."),
            "GPU 50 ninety-minute delay."
        );
        assert_eq!(cleaner.clean_text("GPU fifty, ninety."), "GPU 50, 90.");
        assert_eq!(cleaner.clean_text("GPU fifty\nninety."), "GPU 50\n90.");
        assert_eq!(cleaner.clean_text("Use F 22 04 bits."), "Use F2204 bits.");
        assert_eq!(
            cleaner.clean_text("Use F 22 04-bit flags."),
            "Use F2204-bit flags."
        );
        assert_eq!(
            cleaner.clean_text("GPU fifty 90-minute delay."),
            "GPU 50 90-minute delay."
        );
        assert_eq!(
            cleaner.clean_text("GPU fifty 90 hundredths of a second."),
            "GPU 50 90 hundredths of a second."
        );
        assert_eq!(cleaner.clean_text("RTX 50 90 cards."), "RTX 5090 cards.");
        assert_eq!(cleaner.clean_text("RTX 50 90-Ti."), "RTX 5090-Ti.");
        assert_eq!(cleaner.clean_text("RTX 40 70-SUPER."), "RTX 4070-SUPER.");
        assert_eq!(cleaner.clean_text("RTX 40 70-super."), "RTX 4070-super.");
        assert_eq!(cleaner.clean_text("GPU 50 ninety FPS."), "GPU 50 90 FPS.");
        assert_eq!(
            cleaner.clean_text("GPU fifty 90 meter cable."),
            "GPU 50 90 meter cable."
        );
        assert_eq!(
            cleaner.clean_text("In Q two of twenty twenty six, Colossus one."),
            "In Q2 of 2026, Colossus one."
        );
    }
}

#[test]
fn model_number_formatting_respects_rule_controls_and_reports_hits() {
    let input = "GPU and a fucking fifty ninety sitting here";
    let disabled = HashSet::from(["spoken-numbers".to_string()]);
    let cleaner = build_cleaner_for_test(CleaningProfile::Safe, false, &disabled, &[]);
    assert_eq!(cleaner.clean_text(input), input);
    let cleaner = cleaner_keep_period(CleaningProfile::Safe);
    let result = cleaner.try_clean(input).unwrap();
    assert_eq!(result.text, "GPU and a fucking 5090 sitting here");
    assert!(result
        .rules_fired
        .iter()
        .any(|hit| hit.name == "spoken-numbers" && hit.matches > 0));
    assert_eq!(cleaner.number_threshold(), 4.0);
}
