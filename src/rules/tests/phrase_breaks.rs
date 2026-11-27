//! Punctuation repair, capitalization exceptions, and conservative exclusions.

use super::*;

/// A cleaner with both phrase-repair rules disabled by name.
fn cleaner_without_phrase_repairs() -> Cleaner {
    let disabled = ["merge-sentence-breaks", "function-word-commas"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    build_cleaner_for_test(CleaningProfile::Safe, false, &disabled, &[])
}

#[test]
fn sentence_breaks_merge_function_word_continuations() {
    for profile in [CleaningProfile::Safe, CleaningProfile::Aggressive] {
        assert_clean_cases(
            profile,
            &[
                (
                    "It does hard things well and. Fails elsewhere.",
                    "It does hard things well and fails elsewhere.",
                ),
                (
                    "Use our. Install instructions.",
                    "Use our install instructions.",
                ),
                ("Use your? Existing setup.", "Use your existing setup."),
                (
                    "Use their. Settings or. Defaults.",
                    "Use their settings or defaults.",
                ),
                (
                    "Use an. Example and. Our. Template.",
                    "Use an example and our template.",
                ),
            ],
        );
    }
}

#[test]
fn sentence_merges_and_fillers_share_capitalization_exceptions() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("Try it and. I will help.", "Try it and I will help."),
            ("Try it but. I'll check.", "Try it but I'll check."),
            ("Use our. API.", "Use our API."),
            ("Use our. October setup.", "Use our October setup."),
            ("Use our. Friday's notes.", "Use our Friday's notes."),
            ("Use the. West entrance.", "Use the west entrance."),
            ("Use our. macOS setup.", "Use our macOS setup."),
            ("Use our. v1.2 config.", "Use our v1.2 config."),
            (
                "Use our. example.com config.",
                "Use our example.com config.",
            ),
            ("Use my. May report.", "Use my. May report."),
            ("Use our. March report.", "Use our march report."),
            ("Try our um. Wednesday build.", "Try our Wednesday build."),
            ("Use the um. API.", "Use the API."),
            ("Try um. I'll check.", "Try. I'll check."),
        ],
    );
}

#[test]
fn a_kept_filler_mark_leaves_the_text_after_it_untouched() {
    use crate::rules::phrase_breaks::remove_filled_pauses;

    // The pass writes the mark alone, so it neither doubles the spacing after it nor
    // detaches it from a closing quote or bracket. A later pass would hide the doubling.
    let cases = [
        ("Is it ready um? Yes it is.", "Is it ready? Yes it is."),
        ("It works um. Thanks", "It works. Thanks"),
        (
            "He said \u{201c}Are you ready um?\u{201d}",
            "He said \u{201c}Are you ready?\u{201d}",
        ),
        (
            "He said \"Are you ready um.\" Then left.",
            "He said \"Are you ready.\" Then left.",
        ),
        ("(Are you ready um?) Then go.", "(Are you ready?) Then go."),
        ("Is it ready um.\nNext", "Is it ready.\nNext"),
        ("Is it ready um. \tNext", "Is it ready. \tNext"),
        ("Is it ready um? Uh. Yes.", "Is it ready? Yes."),
        ("Is it ready um, uh? Yes.", "Is it ready? Yes."),
        // A mark that the input glues to the next word stays glued.
        ("Is it ready um?Yes.", "Is it ready?Yes."),
    ];
    for (input, expected) in cases {
        assert_eq!(
            remove_filled_pauses(input).text,
            expected,
            "input {input:?}"
        );
    }
    let cleaner = cleaner_keep_period(CleaningProfile::Safe);
    for (input, expected) in [
        (
            "He said \u{201c}Are you ready um?\u{201d}",
            "He said \u{201c}Are you ready?\u{201d}",
        ),
        ("(Are you ready um?) Then go.", "(Are you ready?) Then go."),
        ("It works um. Thanks", "It works. Thanks"),
        ("Is it ready um.\nNext", "Is it ready.\nNext"),
        ("Is it ready um? Uh. Yes.", "Is it ready? Yes."),
    ] {
        let once = cleaner.clean_text(input);
        assert_eq!(once, expected, "input {input:?}");
        assert_eq!(cleaner.clean_text(&once), once, "output {once:?}");
    }
}

#[test]
fn filled_pauses_drop_their_terminal_punctuation() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("In my um. Main config.", "In my main config."),
            (
                "And. I um. Always forget this.",
                "And I always forget this.",
            ),
            (
                "The oven stopped. Um. This was new.",
                "The oven stopped. This was new.",
            ),
            ("In my uh? Main config.", "In my main config."),
            ("In my erm! Main config.", "In my main config."),
            ("In my um. Uh. Main config.", "In my main config."),
            (
                "Read the um. List. Um. Then stop.",
                "Read the list. Then stop.",
            ),
            ("Use um. I think this.", "Use. I think this."),
            ("Done! Um. Next.", "Done! Next."),
            ("Done? Um. Next.", "Done? Next."),
        ],
    );
}

#[test]
fn filler_marks_stay_unless_the_phrase_is_unfinished() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            // The words before the filler end a clause, or the next words start one.
            (
                "The import finished um. We can continue.",
                "The import finished. We can continue.",
            ),
            (
                "I finished the report, um. Let's go to dinner.",
                "I finished the report. Let's go to dinner.",
            ),
            (
                "We shipped it um. Then we went home.",
                "We shipped it. Then we went home.",
            ),
            (
                "We have the tooling um. Set a goal.",
                "We have the tooling. Set a goal.",
            ),
            ("It works um. Thanks.", "It works. Thanks."),
            ("Great um. Water the plants.", "Great. Water the plants."),
            ("Is it ready um? Yes it is.", "Is it ready? Yes it is."),
            (
                "Ship it um! Now run the tests.",
                "Ship it! Now run the tests.",
            ),
            ("What is the um?", "What is the?"),
            (
                "It is the pan we cook with um. The lid broke.",
                "It is the pan we cook with. The lid broke.",
            ),
            // A phrase that one open-class word continues resumes after the pause.
            ("Please um. Review carefully.", "Please review carefully."),
            // An unfinished phrase takes the next word, or a subject after a wh-word.
            (
                "Take the path and um. I will follow.",
                "Take the path and I will follow.",
            ),
            (
                "I wonder if um. They can help.",
                "I wonder if they can help.",
            ),
            ("Wait and um. Stop um. Rest.", "Wait and stop. Rest."),
            ("Go to um. The main config.", "Go to the main config."),
            ("Check what um. It can do.", "Check what it can do."),
            ("What if um. I told you?", "What if I told you?"),
            (
                "It depends on what um? We choose.",
                "It depends on what we choose.",
            ),
            ("I um. Need a ride.", "I need a ride."),
            // Conjunctions that differ keep their boundary.
            (
                "It is stable but. And the rest needs work.",
                "It is stable but. And the rest needs work.",
            ),
            ("Choose this or. Or that.", "Choose this or that."),
        ],
    );
}

#[test]
fn commas_preserve_conjunction_asides_and_stranded_prepositions() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("Use our, setup.", "Use our setup."),
            (
                "He took my, frankly, terrible advice.",
                "He took my, frankly, terrible advice.",
            ),
            (
                "Use their, settings and our, defaults.",
                "Use their settings and our defaults.",
            ),
            (
                "The red one is his, mine is blue.",
                "The red one is his, mine is blue.",
            ),
            (
                "And, as I just realized, it works.",
                "And, as I just realized, it works.",
            ),
            ("Go and, if you like, check.", "Go and, if you like, check."),
            ("Use the, I guess, default.", "Use the, I guess, default."),
            (
                "I know what you're into, but this differs.",
                "I know what you're into, but this differs.",
            ),
            (
                "I know of, but cannot name, a fix.",
                "I know of, but cannot name, a fix.",
            ),
            ("A, then B.", "A, then B."),
        ],
    );
}

#[test]
fn contextual_merges_require_a_nominal_continuation() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            (
                "Return to. The previous task.",
                "Return to the previous task.",
            ),
            ("Use the, larger window.", "Use the larger window."),
            ("Use my. Backup file.", "Use my backup file."),
            // A comma, contraction, or function word shows a restart.
            (
                "I went to the. Well, never mind.",
                "I went to the. Well, never mind.",
            ),
            (
                "I need the. We’ll start over.",
                "I need the. We’ll start over.",
            ),
            (
                "I forgot the. After lunch I remembered.",
                "I forgot the. After lunch I remembered.",
            ),
            (
                "Use my. Some of those are missing.",
                "Use my. Some of those are missing.",
            ),
            (
                "Use the, that was unexpected.",
                "Use the, that was unexpected.",
            ),
            (
                "It came from. This older version.",
                "It came from. This older version.",
            ),
            // A pronoun can end a clause, and a letter can be a label.
            (
                "That jacket is his. Make sure it gets returned.",
                "That jacket is his. Make sure it gets returned.",
            ),
            (
                "Is that jacket his? Tell me.",
                "Is that jacket his? Tell me.",
            ),
            ("Did you say its? Tell me.", "Did you say its? Tell me."),
            (
                "Choose method A. Then proceed.",
                "Choose method A. Then proceed.",
            ),
            (
                "Review item a. Critical behavior follows.",
                "Review item a. Critical behavior follows.",
            ),
            (
                "a. Critical behavior follows.",
                "A. Critical behavior follows.",
            ),
            // A coordinator opens an aside, so its comma stays.
            ("Choose red or, blue.", "Choose red or, blue."),
            ("And, frankly, I disagree.", "And, frankly, I disagree."),
            (
                "And, without warning. It stopped.",
                "And, without warning. It stopped.",
            ),
        ],
    );
}

#[test]
fn phrase_repairs_distinguish_complements_from_ambiguous_boundaries() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            (
                "That's what it is. Then stop.",
                "That's what it is. Then stop.",
            ),
            ("I want to. ", "I want to."),
            ("Do it so. Then stop.", "Do it so. Then stop."),
            (
                "Before we drive to. The northern. Coast they described.",
                "Before we drive to the northern. Coast they described.",
            ),
            (
                "That is. A unedited example.",
                "That is. A unedited example.",
            ),
            ("I will. Add the log.", "I will. Add the log."),
            (
                "Division of, a pile of fruit into, baskets.",
                "Division of a pile of fruit into baskets.",
            ),
            ("One of the, easier bits.", "One of the easier bits."),
            (
                "That's what I know of. Then stop.",
                "That's what I know of. Then stop.",
            ),
            // A wh-clause or relative clause can supply the preposition's object.
            (
                "Where did it come from? I forgot.",
                "Where did it come from? I forgot.",
            ),
            (
                "Look at what they tied it to. The second rope can wait.",
                "Look at what they tied it to. The second rope can wait.",
            ),
            (
                "I know what example.com is linked to. The new address works.",
                "I know what example.com is linked to. The new address works.",
            ),
            (
                "Who called? Return to. The previous task.",
                "Who called? Return to the previous task.",
            ),
            ("First, a. How does it work?", "First, a. How does it work?"),
            (
                "Logical. Pieces and practical. Feedback.",
                "Logical. Pieces and practical. Feedback.",
            ),
        ],
    );
}

#[test]
fn punctuation_repairs_report_counts_and_obey_disabling() {
    let cleaner = cleaner_keep_period(CleaningProfile::Safe);
    let result = cleaner.clean("Use our. Setup and. Defaults, or their, settings.");
    assert_eq!(
        result.text,
        "Use our setup and defaults, or their settings."
    );
    let hits: Vec<_> = result
        .rules_fired
        .iter()
        .map(|hit| (hit.name.as_str(), hit.matches))
        .collect();
    assert_eq!(
        hits,
        [("merge-sentence-breaks", 2), ("function-word-commas", 1)]
    );
    let cleaner = cleaner_without_phrase_repairs();
    assert_eq!(
        cleaner.clean_text("Use our. Setup and their, defaults."),
        "Use our. Setup and their, defaults."
    );
    assert_eq!(
        cleaner.clean_text("In my um. Main config."),
        "In my main config."
    );
}

#[test]
fn punctuation_repair_preserves_unrelated_boundaries_and_is_idempotent() {
    let cleaner = cleaner_keep_period(CleaningProfile::Safe);
    for text in [
        "Use our... Setup.",
        "Use our! Setup.",
        "Use rock-and. Roll.",
        "Use our.\nSetup.",
        "Use our. ",
    ] {
        assert_eq!(cleaner.clean_text(text), text.trim());
    }
    let once = cleaner.clean_text("Use our. Setup and. Our. API, in my um. Main config.");
    assert_eq!(cleaner.clean_text(&once), once);
}

#[test]
fn filler_chain_context_does_not_depend_on_phrase_repairs() {
    let cleaner = cleaner_without_phrase_repairs();
    assert_eq!(
        cleaner.clean_text("Continue and um. Uh. I will follow."),
        "Continue and I will follow."
    );
    assert_eq!(
        cleaner.clean_text("The task ended um. Uh. We can leave."),
        "The task ended. We can leave."
    );
    assert_eq!(
        cleaner.clean_text("Is it ready um? Uh. Yes it is."),
        "Is it ready? Yes it is."
    );
}

#[test]
fn sentence_merges_follow_words_that_cannot_end_a_sentence() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            // Conjunctions introduce a clause or phrase of any kind.
            (
                "We left early because. The roof leaked.",
                "We left early because the roof leaked.",
            ),
            (
                "She went despite. Having a cold.",
                "She went despite having a cold.",
            ),
            (
                "Bring soup or. If you forget, bread.",
                "Bring soup or if you forget, bread.",
            ),
            ("It rained, but. And then.", "It rained, but. And then."),
            // Determiners need an open-class word.
            (
                "Hand me the. Ladder from the garage.",
                "Hand me the ladder from the garage.",
            ),
            ("Hand me the. They are late.", "Hand me the. They are late."),
            (
                "I want the. Sorry, the big one.",
                "I want the. Sorry, the big one.",
            ),
            // A pronoun can end a clause or an inverted question, so it never joins.
            (
                "Then they. Painted the fence.",
                "Then they. Painted the fence.",
            ),
            (
                "Where shall we? Let's start.",
                "Where shall we? Let's start.",
            ),
            (
                "Review phase I. Budget follows.",
                "Review phase I. Budget follows.",
            ),
        ],
    );
}

#[test]
fn sentence_merges_keep_stranded_prepositions() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            (
                "The box holds twice the weight of. Regular crates.",
                "The box holds twice the weight of regular crates.",
            ),
            (
                "They plan to walk to. The station.",
                "They plan to walk to the station.",
            ),
            (
                "This is the model we started with. The loss was high.",
                "This is the model we started with. The loss was high.",
            ),
            (
                "Use the key that he came with. Our spare is lost.",
                "Use the key that he came with. Our spare is lost.",
            ),
            (
                "That leaves two chores to take care of. Please help.",
                "That leaves two chores to take care of. Please help.",
            ),
            (
                "Is this made of? Metal bends.",
                "Is this made of? Metal bends.",
            ),
            // An elided infinitive and adverbial particles can end a clause.
            (
                "They never want to. Rain helps.",
                "They never want to. Rain helps.",
            ),
            ("Hold on. Dinner is ready.", "Hold on. Dinner is ready."),
        ],
    );
}

#[test]
fn merges_judge_a_boundary_by_the_sentence_that_earlier_merges_joined() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            // The wh-word that opens the joined sentence lets `with` end it.
            (
                "Who did we meet the. Team with. Alice.",
                "Who did we meet the team with. Alice.",
            ),
            // Without that context the same words merge.
            (
                "We met the. Team with. Alice.",
                "We met the team with alice.",
            ),
            // A boundary that stays cuts the context.
            (
                "Who left. We met the. Team with. Alice.",
                "Who left. We met the team with alice.",
            ),
        ],
    );
}

#[test]
fn an_indefinite_pronoun_before_an_infinitive_supplies_the_object() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            // The pronoun is already the object of the stranded preposition.
            (
                "There\u{2019}s nothing to be afraid of. The dog is friendly.",
                "There\u{2019}s nothing to be afraid of. The dog is friendly.",
            ),
            (
                "I have something to write with. The pen works.",
                "I have something to write with. The pen works.",
            ),
            (
                "We found nobody to work with. The team left.",
                "We found nobody to work with. The team left.",
            ),
            (
                "She has no one to talk to. The dog helps.",
                "She has no one to talk to. The dog helps.",
            ),
            (
                "I have something to write with um. The pen works.",
                "I have something to write with. The pen works.",
            ),
            // Without an object before the preposition, a following determiner supplies it.
            (
                "They plan to walk to. The station.",
                "They plan to walk to the station.",
            ),
            (
                "I need to talk to. The manager about it.",
                "I need to talk to the manager about it.",
            ),
            // The infinitive `to` is not the final preposition itself.
            (
                "Give something to. The team.",
                "Give something to the team.",
            ),
            ("No one went with. The team.", "No one went with the team."),
        ],
    );
}

#[test]
fn all_caps_tokens_are_abbreviations_not_function_words() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            // An abbreviation that spells a function word never leaves a sentence open.
            (
                "We moved to Portland, OR. The new job starts Monday.",
                "We moved to Portland, OR. The new job starts Monday.",
            ),
            (
                "She was born in Kuala Lumpur, MY. Flights leave daily.",
                "She was born in Kuala Lumpur, MY. Flights leave daily.",
            ),
            (
                "We flew to the US. Then we drove.",
                "We flew to the US. Then we drove.",
            ),
            ("Check WHO um. It can do.", "Check WHO. It can do."),
            // Nor does one start a new sentence after an unfinished phrase.
            ("Talk to the. IT team today.", "Talk to the IT team today."),
            ("Talk to the, IT team today.", "Talk to the IT team today."),
            ("Hand me the. AM radio.", "Hand me the AM radio."),
            ("Fly from. WHO headquarters.", "Fly from WHO headquarters."),
            ("Please um. Review IT budgets.", "Please review IT budgets."),
            // The same words in ordinary case keep working.
            (
                "Take the bus or. Not the train.",
                "Take the bus or not the train.",
            ),
            ("Hand me the. Am I late.", "Hand me the. Am I late."),
            ("Fly from. Who knows where.", "Fly from. Who knows where."),
            ("Talk to the. It team today.", "Talk to the. It team today."),
        ],
    );
}

#[test]
fn only_an_open_relative_clause_merges_across_a_period() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            (
                "Pick apples that are. Slightly green.",
                "Pick apples that are slightly green.",
            ),
            (
                "He sold the boat, which is. A real shame.",
                "He sold the boat, which is a real shame.",
            ),
            ("I know that is. The answer.", "I know that is. The answer."),
            (
                "Bake some loaves. That are easy to slice.",
                "Bake some loaves. That are easy to slice.",
            ),
            (
                "We repaired the old gate. That is enough.",
                "We repaired the old gate. That is enough.",
            ),
            (
                "This works so well. Is anyone free today?",
                "This works so well. Is anyone free today?",
            ),
            ("Great. Is fixed now.", "Great. Is fixed now."),
        ],
    );
}

#[test]
fn function_word_commas_need_an_unfinished_phrase() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("The walls of, cheap houses.", "The walls of cheap houses."),
            (
                "The walls of, the, old house.",
                "The walls of the old house.",
            ),
            (
                "Wait until we, finish lunch.",
                "Wait until we finish lunch.",
            ),
            // A subordinator joins a pronoun subject, but not another clause start.
            (
                "They stayed because, it was raining.",
                "They stayed because it was raining.",
            ),
            (
                "They left because, the roof leaked.",
                "They left because, the roof leaked.",
            ),
            (
                "Because, it was raining hard.",
                "Because, it was raining hard.",
            ),
            (
                "Check if, for example, it rained.",
                "Check if, for example, it rained.",
            ),
            (
                "A fan of, oddly enough, cold soup.",
                "A fan of, oddly enough, cold soup.",
            ),
            ("This kind of, you know.", "This kind of, you know."),
            (
                "That is who I travel with, usually by train.",
                "That is who I travel with, usually by train.",
            ),
            (
                "We all went in, shut the door.",
                "We all went in, shut the door.",
            ),
        ],
    );
}

#[test]
fn commas_pair_around_asides_by_where_the_clause_ends() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            // After a sentence's opening subject pronoun an aside can be a whole phrase.
            (
                "They, tired from the trip, went straight home.",
                "They, tired from the trip, went straight home.",
            ),
            (
                "They, who were tired, went home.",
                "They, who were tired, went home.",
            ),
            (
                "They, tired e.g. from the trip, went home.",
                "They, tired e.g. from the trip, went home.",
            ),
            // A comma before a coordinator, subordinator, or wh-word ends the clause, so
            // nothing beyond it closes the aside.
            (
                "We, went to the store, and bought milk.",
                "We went to the store, and bought milk.",
            ),
            (
                "We, went to the store, but bought milk, eggs, and bread.",
                "We went to the store, but bought milk, eggs, and bread.",
            ),
            (
                "We, went home, because it rained.",
                "We went home, because it rained.",
            ),
            (
                "They, tired from the trip, and hungry, went home.",
                "They tired from the trip, and hungry, went home.",
            ),
            // A later sentence or line does not close it, and a number is not a comma.
            (
                "We, went home. Then, we slept.",
                "We went home. Then, we slept.",
            ),
            (
                "We, went home\nThen, we slept.",
                "We went home\nThen, we slept.",
            ),
            (
                "We, paid 1,000 dollars today.",
                "We paid 1,000 dollars today.",
            ),
            // Elsewhere an aside is a word or two, so only a nearby comma closes it.
            (
                "It was my, frankly, terrible advice.",
                "It was my, frankly, terrible advice.",
            ),
            (
                "Wait until we, run through the tests we have, in order.",
                "Wait until we run through the tests we have, in order.",
            ),
            (
                "On the, PR that's open, for this branch.",
                "On the PR that's open, for this branch.",
            ),
            // A comma already removed by the right-to-left scan cannot close an aside.
            (
                "The walls of, the, old house.",
                "The walls of the old house.",
            ),
        ],
    );
}

#[test]
fn a_sentence_opener_is_read_past_quotes_and_brackets() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            // A subject pronoun that opens a quoted or bracketed sentence still opens an aside.
            (
                "\u{201c}They, tired from the trip, went home.\u{201d}",
                "\u{201c}They, tired from the trip, went home.\u{201d}",
            ),
            (
                "\u{2018}They, tired from the trip, went home.\u{2019}",
                "\u{2018}They, tired from the trip, went home.\u{2019}",
            ),
            (
                "(They, tired from the trip, went home.)",
                "(They, tired from the trip, went home.)",
            ),
            // The clause still ends at a coordinator.
            (
                "\u{201c}We, went to the store, and bought milk.\u{201d}",
                "\u{201c}We went to the store, and bought milk.\u{201d}",
            ),
            // So does a subordinator that opens a quoted sentence, as at the start of a line.
            (
                "\u{201c}Because, it was raining hard.\u{201d}",
                "\u{201c}Because, it was raining hard.\u{201d}",
            ),
            (
                "\u{201c}They stayed because, it was raining.\u{201d}",
                "\u{201c}They stayed because it was raining.\u{201d}",
            ),
            // Unquoted sentences read as before.
            (
                "They, tired from the trip, went home.",
                "They, tired from the trip, went home.",
            ),
            (
                "We, went to the store, and bought milk.",
                "We went to the store, and bought milk.",
            ),
        ],
    );
}

#[test]
fn filler_terminals_stay_when_the_next_word_embeds_a_filler() {
    // The continuation word can hold a filler (`Store-um`), so merging into it would
    // reach into the next filler match.
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            (
                "I went to the um. Store-um thing.",
                "I went to the store- thing.",
            ),
            ("and uh? Path/uh/file.", "And path/ /file."),
            ("with my um. Foo.um bar.", "With my foo. Bar."),
            ("Talk to um, uh. Self-erm test.", "Talk to. Self- test."),
        ],
    );
}

#[test]
fn phrase_passes_survive_fillers_inside_continuation_words() {
    use crate::rules::phrase_breaks::{
        merge_sentence_breaks, remove_filled_pauses, remove_function_word_commas,
    };

    let lefts = [
        "I went to the",
        "and",
        "We",
        "with my",
        "Talk to",
        "Do you want the",
    ];
    let separators = [
        " um.", " uh?", " um,", ".", "?", ",", " um. Uh.", " um, uh.",
    ];
    let rights = [
        "Store-um thing.",
        "Path/uh/file.",
        "Foo.um bar.",
        "X-um",
        "API-um docs.",
        "I'm-uh fine.",
        "Uh-huh, yes.",
    ];
    let cleaners = [
        cleaner_keep_period(CleaningProfile::Safe),
        cleaner_messaging_default(CleaningProfile::Safe),
    ];
    let mut failures = Vec::new();
    for left in lefts {
        for separator in separators {
            for right in rights {
                let input = format!("{left}{separator} {right}");
                // The passes run directly: `Cleaner::clean` fails open on a panic.
                let filled = remove_filled_pauses(&input).text.into_owned();
                let merged = merge_sentence_breaks(&input).text.into_owned();
                let commas = remove_function_word_commas(&input).text.into_owned();
                let chained = {
                    let filled = remove_filled_pauses(&input).text.into_owned();
                    let merged = merge_sentence_breaks(&filled).text.into_owned();
                    remove_function_word_commas(&merged).text.into_owned()
                };
                for cleaner in &cleaners {
                    for text in [&input, &filled, &merged, &commas, &chained] {
                        let once = cleaner.clean(text);
                        let twice = cleaner.clean(&once.text);
                        if once.failure.is_some() || twice.text != once.text {
                            failures.push(format!(
                                "{text:?}: {:?} then {:?} ({:?})",
                                once.text, twice.text, once.failure
                            ));
                        }
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_lone_word_after_a_filler_keeps_the_period_even_without_a_terminal() {
    let cases = [
        ("It works um. Thanks", "It works. Thanks"),
        ("It works um. Thanks.", "It works. Thanks."),
        ("Thanks um. Bye.", "Thanks. Bye."),
        ("Thanks um. Bye", "Thanks. Bye"),
        ("Please um. Review carefully", "Please review carefully"),
        ("Please um. Review carefully.", "Please review carefully."),
        // A determiner still takes a lone word.
        ("In my um. Config", "In my config"),
    ];
    assert_clean_cases(CleaningProfile::Safe, &cases);
    let cleaner = cleaner_keep_period(CleaningProfile::Safe);
    for (_, expected) in cases {
        assert_eq!(cleaner.clean_text(expected), expected);
    }
    let cleaner = cleaner_messaging_default(CleaningProfile::Safe);
    assert_eq!(
        cleaner.clean_text("It works um. Thanks"),
        "It works. Thanks"
    );
}

#[test]
fn auxiliaries_still_start_a_sentence_after_a_filler() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("Wait um. Is it ready", "Wait. Is it ready"),
            ("Please um. Review carefully", "Please review carefully"),
            ("Wait um. Can you hear me", "Wait. Can you hear me"),
        ],
    );
}
