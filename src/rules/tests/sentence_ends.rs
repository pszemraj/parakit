//! Where a sentence ends: one rule for the context before a mark and the scan after it.

use super::question_carry::assert_messaging_cases;

#[test]
fn an_ellipsis_ends_a_sentence_for_the_context_before_a_mark() {
    assert_messaging_cases(&[
        // The ASCII and the Unicode ellipsis both cut the context, so a statement that
        // follows one never becomes a question.
        (
            "Can we stop... I want the? Blue one.",
            "Can we stop... I want the blue one",
        ),
        (
            "Can we stop\u{2026} I want the? Blue one.",
            "Can we stop\u{2026} I want the blue one",
        ),
        (
            "Can we stop\u{2026}\u{201d} I want the? Blue one.",
            "Can we stop\u{2026}\u{201d} I want the blue one",
        ),
        // A question that follows one is read from its own start.
        (
            "I stopped... do you want tea or? Coffee. Thanks.",
            "I stopped... Do you want tea or coffee? Thanks",
        ),
        (
            "I stopped\u{2026} do you want tea or? Coffee. Thanks.",
            "I stopped\u{2026} do you want tea or coffee? Thanks",
        ),
        // An ellipsis that a word follows ends nothing.
        (
            "Can we take the? Bus\u{2026}and go home.",
            "Can we take the bus\u{2026}and go home?",
        ),
    ]);
}
