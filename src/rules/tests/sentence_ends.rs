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

#[test]
fn an_abbreviation_ends_a_sentence_before_a_new_clause() {
    assert_messaging_cases(&[
        // Forward: the question ends at the abbreviation, and the mark follows its period.
        (
            "Can you get the? Report by 5 p.m. I can wait.",
            "Can you get the report by 5 p.m.? I can wait",
        ),
        (
            "Can you get the? Report by 5 p.m. Is that ok.",
            "Can you get the report by 5 p.m.? Is that ok",
        ),
        (
            "Do you want tea or? Coffee at 5 p.m. I can make some.",
            "Do you want tea or coffee at 5 p.m.? I can make some",
        ),
        // Backward: the sentence after the abbreviation is read from its own start.
        (
            "We left at 5 p.m. Can we take the? Bus.",
            "We left at 5 p.m. Can we take the bus?",
        ),
        // A name, a day, or any other word goes on, and a title never ends a sentence.
        (
            "Can you ask Mr. Smith about the? Report.",
            "Can you ask Mr. Smith about the report?",
        ),
        (
            "Can we meet at 5 p.m. tomorrow or? Later.",
            "Can we meet at 5 p.m. tomorrow or later?",
        ),
        (
            "Can you ask Mr. Will about the? Report.",
            "Can you ask Mr. Will about the report?",
        ),
        (
            "Do you want the? Report from Dr. Who is here.",
            "Do you want the report from Dr. Who is here?",
        ),
        // A lowercase word after the abbreviation continues the sentence.
        (
            "Can you get the? Report by 5 p.m. and send it.",
            "Can you get the report by 5 p.m. and send it?",
        ),
    ]);
}
