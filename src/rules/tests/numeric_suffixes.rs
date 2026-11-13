//! Numeric prefixes followed by spoken version or decimal components.

use super::*;

#[test]
fn numeric_prefixes_accept_multiword_components() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("Version 2 point twenty five.", "Version 2.25."),
            ("Use V 2.4.6.", "Use v2.4.6."),
            ("Use D 2.4.6.", "Use D2.4.6."),
            ("P ninety nine point nine.", "P99.9."),
            ("SM ninety nine point nine.", "SM99.9."),
            ("Version 2 point zero five.", "Version 2.05."),
            ("Version 2 point one two point four.", "Version 2.12.4."),
            ("Version 2 point twenty-five.", "Version 2.25."),
            ("Version 2 point zero-five.", "Version 2.05."),
            ("Use 20 point zero twenty point zero.", "Use 20.0 20.0."),
            ("Use 20 point five twenty point six.", "Use 20.5 20.6."),
            ("Use 2 point five thousand.", "Use 2.5 thousand."),
            ("Use 2 point twenty five million.", "Use 2.25 million."),
            ("Use 2 point five two thousand.", "Use 2.5 2000."),
            ("Use 2 point five two million.", "Use 2.5 2 million."),
            ("Use 2 point five twenty five thousand.", "Use 2.5 25000."),
        ],
    );
}

#[test]
fn numeric_point_components_stop_at_scales_and_punctuation() {
    for (input, expected) in [
        ("2 point five thousand", "2.5 thousand"),
        ("2 point twenty five million", "2.25 million"),
        ("2 point five, 3 point six", "2.5, 3.6"),
        ("2 point five\n3 point six", "2.5\n3.6"),
        ("2 point\nfive", "2 point\nfive"),
        ("2 point five. 3 point six", "2.5. 3.6"),
    ] {
        assert_eq!(
            numeric_test(
                input,
                DEFAULT_NUMBER_THRESHOLD,
                &[super::super::numbers::NumberKind::Version]
            )
            .text,
            expected,
            "input {input:?}"
        );
    }
    for alias in ["oh", "o", "nought", "zero"] {
        for scale in ["hundred", "thousand", "million", "billion"] {
            let input = format!("zero point {alias} five {scale}");
            assert_eq!(
                numeric_test(
                    &input,
                    DEFAULT_NUMBER_THRESHOLD,
                    &[super::super::numbers::NumberKind::Version]
                )
                .text,
                format!("0.05 {scale}"),
                "input {input:?}"
            );
        }
    }
}

#[test]
fn version_components_do_not_absorb_a_following_size() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("Python three point twelve.", "Python 3.12."),
            ("Capacity 2.5 seven B.", "Capacity 2.5 7B."),
            ("Llama three point one seventy B.", "Llama 3.1 70B."),
            ("Llama 3.1 seventy B.", "Llama 3.1 70B."),
            ("Llama seventy B three point one.", "Llama 70B 3.1."),
            ("Granite thirty B four point one.", "Granite 30B 4.1."),
            ("Mistral seven B v zero point three.", "Mistral 7B v0.3."),
            ("Plan B v two point oh.", "Plan B v2.0."),
            ("About 1 5 M parameters.", "About 1 5M parameters."),
        ],
    );
}

#[test]
fn size_suffixes_preserve_unmatched_scale_values() {
    let cases = [
        ("Use 5 hundred K.", "Use 500K."),
        ("Use five hundred K.", "Use 500K."),
        ("Use two point five million B.", "Use 2.5 million B."),
        ("Use five thousand M.", "Use 5000M."),
        ("Use two point two hundred K.", "Use 2.2 hundred K."),
        ("Use five point one hundred B.", "Use 5.1 hundred B."),
        ("Use 2.2 hundred K.", "Use 2.2 hundred K."),
        ("Use 5.1 hundred B.", "Use 5.1 hundred B."),
        ("Use five hundred billion K.", "Use 500 billion K."),
        ("Use two point five billion M.", "Use 2.5 billion M."),
        ("Use two point five million M.", "Use 2.5M."),
        ("Use three point five thousand K.", "Use 3.5K."),
        ("Use five hundred thousand K.", "Use 500K."),
        ("Use seventy billion B.", "Use 70B."),
        (
            "Use 9007199254740993 hundred K.",
            "Use 900719925474099300K.",
        ),
        (
            "Use 340282366920938463463374607431768211455 hundred K.",
            "Use 340282366920938463463374607431768211455 hundred K.",
        ),
    ];
    let digits = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ];
    for profile in [CleaningProfile::Safe, CleaningProfile::Aggressive] {
        for threshold in [0.0, 4.0, 10.0] {
            let cleaner = build_cleaner_for_test_with_threshold(
                profile,
                false,
                Some(threshold),
                &HashSet::new(),
                &[],
            );
            for (input, expected) in cases {
                assert_eq!(
                    cleaner.clean_text(input),
                    expected,
                    "{profile:?}/{threshold}: {input}"
                );
            }
            for suffix in ["K", "M", "B"] {
                for tenths in 11..100 {
                    let whole = tenths / 10;
                    let fraction = tenths % 10;
                    let expected = format!("Use {whole}.{fraction} hundred {suffix}.");
                    for input in [
                        expected.clone(),
                        format!(
                            "Use {} point {} hundred {suffix}.",
                            digits[whole], digits[fraction]
                        ),
                    ] {
                        assert_eq!(
                            cleaner.clean_text(&input),
                            expected,
                            "{profile:?}/{threshold}: {input}"
                        );
                    }
                }
                for (whole, word) in digits.iter().enumerate().skip(1) {
                    let expected = format!("Use {}{suffix}.", whole * 100);
                    for input in [
                        format!("Use {whole} hundred {suffix}."),
                        format!("Use {word} hundred {suffix}."),
                    ] {
                        assert_eq!(
                            cleaner.clean_text(&input),
                            expected,
                            "{profile:?}/{threshold}: {input}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn spoken_versions_respect_the_identifier_control() {
    let disabled = HashSet::from(["numeric-identifiers".to_string()]);
    let cleaner = build_cleaner_for_test(CleaningProfile::Safe, false, &disabled, &[]);
    assert_eq!(cleaner.clean_text("P ninety nine point nine."), "P 99.9.");
}

#[test]
fn version_components_use_text2num_without_a_custom_number_parser() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("V zero point five point two.", "v0.5.2."),
            ("PyTorch two point thirteen point oh.", "PyTorch 2.13.0."),
            (
                "PyTorch two point seven point oh plus cuda twelve point eight.",
                "PyTorch 2.7.0 plus cuda 12.8.",
            ),
            (
                "PyTorch two point seven point oh and Python three point twelve.",
                "PyTorch 2.7.0 and Python 3.12.",
            ),
            ("Qwen three one point seven B.", "Qwen three 1.7B."),
            ("Qwen three zero point six B.", "Qwen three 0.6B."),
            (
                "Beta one zero point nine, beta two zero point nine five.",
                "Beta one 0.9, beta two 0.95.",
            ),
            ("Python three three point twelve.", "Python three 3.12."),
            ("Opus four five point five.", "Opus 4 5.5."),
            ("Ubuntu two four point oh four.", "Ubuntu two 4.04."),
            // Multiword digit bases remain ambiguous outside IPv4 shapes.
            (
                "Version one two point three point four.",
                "Version one 2.3.4.",
            ),
            ("CUDA one two point eight.", "CUDA one 2.8."),
            ("Zero point oh five million.", "0.05 million."),
            ("Zero point zero five million.", "0.05 million."),
            ("Nought point nought five million.", "0.05 million."),
            ("Two point oh five billion.", "2.05 billion."),
            ("twenty five two point five liter.", "25 2.5 liter."),
            (
                "arXiv twenty four oh one point oh one two three four.",
                "arXiv twenty four oh 1.01234.",
            ),
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
                "One nine two point one six eight point one point one.",
                "192.168.1.1.",
            ),
            (
                "One two seven point zero point zero point one.",
                "127.0.0.1.",
            ),
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
