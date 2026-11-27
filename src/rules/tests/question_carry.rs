//! Question marks that a merge removes: where the carried mark goes, and when it is dropped.

use super::*;

/// Cleans each case with the default dictation settings, which drop one final period, and
/// checks that the output is stable.
fn assert_messaging_cases(cases: &[(&str, &str)]) {
    let cleaner = cleaner_messaging_default(CleaningProfile::Safe);
    for (input, expected) in cases {
        assert_eq!(cleaner.clean_text(input), *expected, "input {input:?}");
        assert_eq!(
            cleaner.clean_text(expected),
            *expected,
            "output {expected:?}"
        );
    }
}

#[test]
fn a_merged_question_mark_moves_to_the_end_of_a_question() {
    assert_messaging_cases(&[
        (
            "Do you want tea or? Coffee. I can make some.",
            "Do you want tea or coffee? I can make some",
        ),
        (
            "Can you send me the? Report by Friday.",
            "Can you send me the report by Friday?",
        ),
        ("Do you want tea or? Coffee", "Do you want tea or coffee?"),
        ("Do you want the um? Red one.", "Do you want the red one?"),
        ("Which is the uh? Best option.", "Which is the best option?"),
        (
            "Do you want the um? Uh. Red one.",
            "Do you want the red one?",
        ),
        // A wh-word followed by an auxiliary or a contraction opens an inverted question.
        ("What is the? Plan.", "What is the plan?"),
        ("What's the? Plan.", "What's the plan?"),
        ("What\u{2019}s the? Plan.", "What\u{2019}s the plan?"),
        ("Where's the? Remote.", "Where's the remote?"),
        ("Where did the? Cable go.", "Where did the cable go?"),
        ("How do we fix the? Parser.", "How do we fix the parser?"),
        // Other openers keep dropping the mark, as rising intonation also marks statements.
        ("I went to the? Store.", "I went to the store"),
        ("Is it ready um? Yes it is.", "Is it ready? Yes it is"),
    ]);
}

#[test]
fn a_wh_word_that_opens_a_statement_does_not_carry_the_mark() {
    assert_messaging_cases(&[
        (
            "What I need is the um? Red one. Then we go.",
            "What I need is the red one. Then we go",
        ),
        (
            "How we did it was with the? Old script.",
            "How we did it was with the old script",
        ),
        (
            "Where it broke is in the? Parser module, I think.",
            "Where it broke is in the parser module, I think",
        ),
        // A subject question has no auxiliary after the wh-word, so it drops the mark.
        ("Who broke the? Build.", "Who broke the build"),
    ]);
}

#[test]
fn a_carried_question_mark_respects_where_the_sentence_ends() {
    assert_messaging_cases(&[
        // An existing terminal other than a period stays.
        (
            "Do you want the um? Red one or the blue one?",
            "Do you want the red one or the blue one?",
        ),
        ("Do you want tea or? Coffee!", "Do you want tea or coffee!"),
        (
            "Do you want tea or? Coffee...",
            "Do you want tea or coffee...",
        ),
        // A boundary that is itself merged hands the mark to the next one.
        (
            "Do you want tea or? Coffee and. Milk. Thanks.",
            "Do you want tea or coffee and milk? Thanks",
        ),
        (
            "Do you want the um? Red one and the? Blue one.",
            "Do you want the red one and the blue one?",
        ),
        // Two marks that carry to one terminal.
        (
            "Is it the? Red one and. Is it the? Blue one.",
            "Is it the red one and is it the blue one?",
        ),
        // Closing quotes and brackets follow the mark, and a line ends the sentence.
        (
            "Do you want tea or? Coffee.\" she asked.",
            "Do you want tea or coffee?\" She asked",
        ),
        (
            "Do you want tea or? Coffee.) Thanks.",
            "Do you want tea or coffee?) Thanks",
        ),
        (
            "Do you want tea or? Coffee\nThanks.",
            "Do you want tea or coffee?\nThanks",
        ),
        // Dotted tokens and abbreviations are not sentence ends.
        (
            "Do you want the? Report from example.com or Mr. Smith. Thanks.",
            "Do you want the report from example.com or Mr. Smith? Thanks",
        ),
    ]);
}

#[test]
fn a_carried_question_mark_goes_inside_the_closers_that_end_the_sentence() {
    assert_messaging_cases(&[
        // Without a terminal, the mark follows the last word and precedes quotes and brackets.
        (
            "\u{201c}Do you want tea or? Coffee\u{201d}",
            "\u{201c}Do you want tea or coffee?\u{201d}",
        ),
        (
            "\"Do you want tea or? Coffee\"",
            "\"Do you want tea or coffee?\"",
        ),
        (
            "(Do you want tea or? Coffee)",
            "(Do you want tea or coffee?)",
        ),
        (
            "\u{201c}(Do you want tea or? Coffee)\u{201d}",
            "\u{201c}(Do you want tea or coffee?)\u{201d}",
        ),
        (
            "\u{201c}Do you want tea or? Coffee\u{201d}\nMore text.",
            "\u{201c}Do you want tea or coffee?\u{201d}\nMore text",
        ),
        (
            "(Do you want tea or? Coffee)\nMore text.",
            "(Do you want tea or coffee?)\nMore text",
        ),
        (
            "\u{201c}Do you want the um? Red one\u{201d}",
            "\u{201c}Do you want the red one?\u{201d}",
        ),
        // A terminal inside the closers is the one that changes.
        (
            "\u{201c}Do you want tea or? Coffee.\u{201d} I can make some.",
            "\u{201c}Do you want tea or coffee?\u{201d} I can make some",
        ),
        // Text without closers still ends in the mark.
        ("Do you want tea or? Coffee", "Do you want tea or coffee?"),
        (
            "Do you want tea or? Coffee\nMore text.",
            "Do you want tea or coffee?\nMore text",
        ),
    ]);
}

#[test]
fn a_boundary_merged_earlier_in_the_pass_does_not_cut_the_question() {
    assert_messaging_cases(&[
        // Each merge sees the sentence that the earlier merges joined.
        (
            "Can we take the. Train or? Bus. We could drive.",
            "Can we take the train or bus? We could drive",
        ),
        (
            "Can we take the um. Train or um? Bus. We could drive.",
            "Can we take the train or bus? We could drive",
        ),
        (
            "Can we take the um. Train or? Bus. We could drive.",
            "Can we take the train or bus? We could drive",
        ),
        // A filler chain that follows a merge still reaches back to the start.
        (
            "Can we take the um. Train or um? Uh. Bus. We could drive.",
            "Can we take the train or bus? We could drive",
        ),
        // Three merges in one sentence, in each pass and across both.
        (
            "Can we take the. Train or? Bus and. Taxi. We could drive.",
            "Can we take the train or bus and taxi? We could drive",
        ),
        (
            "Can we take the um. Train or um. Bus and um? Taxi. We could drive.",
            "Can we take the train or bus and taxi? We could drive",
        ),
        // A boundary that stays still starts the sentence.
        ("Can we go. I took the? Bus.", "Can we go. I took the bus"),
        (
            "Can we go. I took the. Train or? Bus.",
            "Can we go. I took the train or bus",
        ),
        (
            "Can we take the. Train or? Bus. Is it far? We could drive.",
            "Can we take the train or bus? Is it far? We could drive",
        ),
    ]);
}
