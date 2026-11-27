//! Question marks that a merge removes: where the carried mark goes, and when it is dropped.

use super::*;

/// Cleans each case with the default dictation settings, which drop one final period, and
/// checks that the output is stable.
pub(super) fn assert_messaging_cases(cases: &[(&str, &str)]) {
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

#[test]
fn an_auxiliary_and_a_subject_open_an_inverted_question() {
    assert_messaging_cases(&[
        // A subject pronoun, an article or possessive, a demonstrative, or `there` follows.
        ("Do you want tea or? Coffee.", "Do you want tea or coffee?"),
        ("Is the? Build ready.", "Is the build ready?"),
        ("Did the? Tests pass.", "Did the tests pass?"),
        ("Is that the? Plan.", "Is that the plan?"),
        ("Is there an? Answer.", "Is there an answer?"),
        ("May I take the? Last one.", "May I take the last one?"),
        ("Does it have the? Thing.", "Does it have the thing?"),
        // A negative contraction of the auxiliary, with either apostrophe.
        (
            "Shouldn\u{2019}t we take the? Train.",
            "Shouldn\u{2019}t we take the train?",
        ),
        (
            "Shouldn't we take the? Train.",
            "Shouldn't we take the train?",
        ),
        ("Can't we take the? Train.", "Can't we take the train?"),
        ("Won't you try the? Soup.", "Won't you try the soup?"),
        ("Shan't we take the? Train.", "Shan't we take the train?"),
        ("Don't you want the? Tea.", "Don't you want the tea?"),
        // A wh-word followed by a contracted auxiliary needs no subject check.
        ("Why didn't the? Build run.", "Why didn't the build run?"),
        ("What's the? Plan.", "What's the plan?"),
        // The filler path shares the rule.
        (
            "Shouldn\u{2019}t we take the um? Train.",
            "Shouldn\u{2019}t we take the train?",
        ),
    ]);
}

#[test]
fn imperatives_names_and_open_class_subjects_do_not_open_a_question() {
    assert_messaging_cases(&[
        // Base-form `do` and `have` open imperatives, so only a subject pronoun follows them.
        ("Do the? Dishes before bed.", "Do the dishes before bed"),
        ("Do this before the? Meeting.", "Do this before the meeting"),
        (
            "Have it ready by the? Morning.",
            "Have it ready by the morning",
        ),
        ("Don't forget the? Charger.", "Don't forget the charger"),
        ("Do the um? Dishes before bed.", "Do the dishes before bed"),
        // A name that spells an auxiliary opens a statement.
        ("Will said the? Build failed.", "Will said the build failed"),
        // An open-class subject is not one that the closed classes recognize.
        ("Is Peter coming or? Not.", "Is Peter coming or not"),
        ("Does Peter want the? Soup.", "Does Peter want the soup"),
        // An all-caps token is an abbreviation, not an auxiliary.
        ("DON'T we take the? Train.", "DON'T we take the train"),
        // An auxiliary that no subject follows is not an inverted question.
        ("Can't take the? Train.", "Can't take the train"),
    ]);
}

#[test]
fn a_negated_do_or_have_takes_the_full_subject_set() {
    assert_messaging_cases(&[
        // Only the affirmative base forms open imperatives, so a negated one takes any subject.
        (
            "Haven\u{2019}t the? Tests finished yet.",
            "Haven\u{2019}t the tests finished yet?",
        ),
        (
            "Haven't the? Tests finished yet.",
            "Haven't the tests finished yet?",
        ),
        ("Don't the? Kids need coats.", "Don't the kids need coats?"),
        ("Haven't you the? Time.", "Haven't you the time?"),
        // A negative imperative takes a verb, which is not a subject.
        ("Don't forget the? Charger.", "Don't forget the charger"),
        // The affirmative forms keep the restriction.
        ("Do the? Dishes before bed.", "Do the dishes before bed"),
        (
            "Have it ready by the? Morning.",
            "Have it ready by the morning",
        ),
        ("Have you the? Time.", "Have you the time?"),
    ]);
}

#[test]
fn an_ellipsis_ends_a_question_as_it_is() {
    assert_messaging_cases(&[
        // The ASCII and the Unicode ellipsis end the sentence, so no mark is carried.
        (
            "Do you want tea or? Coffee... I can make some.",
            "Do you want tea or coffee... I can make some",
        ),
        (
            "Do you want tea or? Coffee\u{2026} I can make some.",
            "Do you want tea or coffee\u{2026} I can make some",
        ),
        (
            "Do you want tea or? Coffee...",
            "Do you want tea or coffee...",
        ),
        (
            "Do you want tea or? Coffee\u{2026}",
            "Do you want tea or coffee\u{2026}",
        ),
        (
            "Do you want tea or? Coffee\u{2026}\u{201d} she asked.",
            "Do you want tea or coffee\u{2026}\u{201d} she asked",
        ),
        (
            "Do you want tea or? Coffee\u{2026}\nMore text.",
            "Do you want tea or coffee\u{2026}\nMore text",
        ),
        // An ellipsis inside a sentence does not end it.
        (
            "Do you want tea or? Coffee\u{2026}and tea. Thanks.",
            "Do you want tea or coffee\u{2026}and tea? Thanks",
        ),
    ]);
}

#[test]
fn a_carried_question_mark_goes_inside_only_the_closers_that_enclose_the_question() {
    assert_messaging_cases(&[
        // A closer whose opener is not before the first letter ends a phrase inside it.
        (
            "Do you want tea or? Coffee from \u{201c}Central Perk\u{201d}",
            "Do you want tea or coffee from \u{201c}Central Perk\u{201d}?",
        ),
        (
            "\u{201c}Do you want tea or? Coffee from \u{201c}Central Perk\u{201d}\u{201d}",
            "\u{201c}Do you want tea or coffee from \u{201c}Central Perk\u{201d}?\u{201d}",
        ),
        (
            "\u{201c}Do you want tea or? Coffee from \u{201c}Central Perk\u{201d}\u{201d}\nMore text.",
            "\u{201c}Do you want tea or coffee from \u{201c}Central Perk\u{201d}?\u{201d}\nMore text",
        ),
        (
            "Do you want tea or? Coffee from \"Central Perk\"",
            "Do you want tea or coffee from \"Central Perk\"?",
        ),
        // A plural possessive is not a closing quote.
        (
            "Do you want the dogs or? The cats\u{2019}",
            "Do you want the dogs or the cats\u{2019}?",
        ),
        // The closer that the continuation word absorbs still encloses the question.
        (
            "\u{2018}Do you want tea or? Coffee\u{2019}",
            "\u{2018}Do you want tea or coffee?\u{2019}",
        ),
        (
            "\u{2018}Do you want tea or? Coffee\u{2019}\nMore text.",
            "\u{2018}Do you want tea or coffee?\u{2019}\nMore text",
        ),
        // Openers pair from the outside in, each with its own closer.
        (
            "\u{201c}Do you want tea or? Coffee\u{201d}",
            "\u{201c}Do you want tea or coffee?\u{201d}",
        ),
        (
            "(Do you want tea or? Coffee)",
            "(Do you want tea or coffee?)",
        ),
        (
            "\"Do you want tea or? Coffee\"",
            "\"Do you want tea or coffee?\"",
        ),
        (
            "'Do you want tea or? Coffee'",
            "'Do you want tea or coffee?'",
        ),
        (
            "\u{201c}\u{2018}Do you want tea or? Coffee\u{2019}\u{201d}",
            "\u{201c}\u{2018}Do you want tea or coffee?\u{2019}\u{201d}",
        ),
        (
            "\u{201c}(Do you want tea or? Coffee)\u{201d}",
            "\u{201c}(Do you want tea or coffee?)\u{201d}",
        ),
        // A closer that does not match its opener stays before the mark.
        (
            "\u{201c}Do you want tea or? Coffee)\u{201d}",
            "\u{201c}Do you want tea or coffee)?\u{201d}",
        ),
        // Without closers the mark ends the text or the line.
        ("Do you want tea or? Coffee", "Do you want tea or coffee?"),
    ]);
}
