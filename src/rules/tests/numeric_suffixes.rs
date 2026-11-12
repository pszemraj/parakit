//! Numeric prefixes followed by spoken version or decimal components.

use super::*;

#[test]
fn numeric_prefixes_accept_multiword_components() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("Version 2 point twenty five.", "Version 2.25."),
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
            super::super::passes::normalize_numeric_point_suffixes(input).text,
            expected,
            "input {input:?}"
        );
    }
}
