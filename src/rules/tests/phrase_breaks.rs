//! Punctuation repair, capitalization exceptions, and conservative exclusions.

use super::*;

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
            ("Use our. Mac.", "Use our Mac."),
            ("Use our. Linux setup.", "Use our Linux setup."),
            ("Ask our. Peter.", "Ask our Peter."),
            ("Use our. Peter's setup.", "Use our Peter's setup."),
            ("Use our. Mac-based setup.", "Use our Mac-based setup."),
            ("Use our. macOS setup.", "Use our macOS setup."),
            ("Use our. v1.2 config.", "Use our v1.2 config."),
            (
                "Use our. example.com config.",
                "Use our example.com config.",
            ),
            ("Ask um. Peter.", "Ask Peter."),
            ("Use um. API.", "Use API."),
            ("Use um. Mac.", "Use Mac."),
            ("Try um. I'll check.", "Try. I'll check."),
        ],
    );
}

#[test]
fn filled_pauses_drop_their_terminal_punctuation() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            ("In my um. Main config.", "In my main config."),
            (
                "And. I um. Still only see this.",
                "And I still only see this.",
            ),
            (
                "Review came back. Um. This was local.",
                "Review came back. This was local.",
            ),
            ("In my uh? Main config.", "In my main config."),
            ("In my erm! Main config.", "In my main config."),
            ("In my um. Uh. Main config.", "In my main config."),
            ("Read um. This. Um. Then stop.", "Read this. Then stop."),
            ("Use um. I think this.", "Use. I think this."),
            ("Done! Um. Next.", "Done! Next."),
            ("Done? Um. Next.", "Done? Next."),
        ],
    );
}

#[test]
fn punctuation_preserves_clause_restarts_and_completed_questions() {
    assert_clean_cases(
        CleaningProfile::Safe,
        &[
            (
                "The import finished um. We can continue.",
                "The import finished. We can continue.",
            ),
            (
                "The process stopped uh? Can it restart?",
                "The process stopped? Can it restart?",
            ),
            (
                "Take the path and um. I will follow.",
                "Take the path and I will follow.",
            ),
            (
                "I wonder if um. They can help.",
                "I wonder if they can help.",
            ),
            (
                "It is stable but. And the rest needs work.",
                "It is stable but. And the rest needs work.",
            ),
            ("Choose this or. Or that.", "Choose this or that."),
            ("Check what um. It can do.", "Check what it can do."),
            (
                "The build failed um. It's recoverable.",
                "The build failed. It's recoverable.",
            ),
            ("What is the um?", "What is the?"),
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
                "Use their, settings and his, defaults.",
                "Use their settings and his defaults.",
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
            ("The list of, a few choices.", "The list of a few choices."),
            ("Use the, larger window.", "Use the larger window."),
            ("Use my. Backup file.", "Use my backup file."),
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
                "Choose method A. Then proceed.",
                "Choose method A. Then proceed.",
            ),
            (
                "It came from. This older version.",
                "It came from. This older version.",
            ),
            (
                "It came from. This is the source.",
                "It came from. This is the source.",
            ),
            ("Choose red or, blue.", "Choose red or, blue."),
            ("And, frankly, I disagree.", "And, frankly, I disagree."),
            ("Wait or, perhaps, proceed.", "Wait or, perhaps, proceed."),
            (
                "And, without warning. It stopped.",
                "And, without warning. It stopped.",
            ),
            (
                "We stopped and, without warning, it restarted.",
                "We stopped and, without warning, it restarted.",
            ),
            (
                "Review item a. Critical behavior follows.",
                "Review item a. Critical behavior follows.",
            ),
            (
                "a. Critical behavior follows.",
                "A. Critical behavior follows.",
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
                "Before I return to. The original. Topic I was discussing.",
                "Before I return to the original. Topic I was discussing.",
            ),
            (
                "That is. A unedited example.",
                "That is. A unedited example.",
            ),
            ("I will. Add the log.", "I will. Add the log."),
            (
                "Segmentation of, a stream of text into, phrases.",
                "Segmentation of a stream of text into, phrases.",
            ),
            ("One of the, easier bits.", "One of the easier bits."),
            (
                "That's what I know of. Then stop.",
                "That's what I know of. Then stop.",
            ),
            (
                "Where did it come from? I forgot.",
                "Where did it come from? I forgot.",
            ),
            ("I'm standing by. Call me.", "I'm standing by. Call me."),
            (
                "Which one do you agree with? I forgot.",
                "Which one do you agree with? I forgot.",
            ),
            ("Use method A. Then stop.", "Use method A. Then stop."),
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
    let disabled = PHRASE_BREAK_RULES
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    let cleaner = build_cleaner_for_test(CleaningProfile::Safe, false, &disabled, &[]);
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
    let disabled = PHRASE_BREAK_RULES
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    let cleaner = build_cleaner_for_test(CleaningProfile::Safe, false, &disabled, &[]);
    assert_eq!(
        cleaner.clean_text("Continue and um. Uh. I will follow."),
        "Continue and I will follow."
    );
    assert_eq!(
        cleaner.clean_text("The task ended um. Uh. We can leave."),
        "The task ended. We can leave."
    );
}
