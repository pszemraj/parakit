//! Function-word punctuation repair and filled-pause removal.

use super::engine::TransformResult;
use super::passes::{apply_replacements, protected_token_case, punctuation_is_sentence_boundary};

/// Merge a period or question mark after an eligible function word.
///
/// # Returns
/// The repaired text and number of boundaries removed.
///
/// # Errors
/// Infallible; the return value is a transformation, not a fallible result.
///
/// # Panics
/// Captures come from a fixed pattern and slices use its UTF-8 boundaries.
pub(super) fn merge_sentence_breaks(input: &str) -> TransformResult<'_> {
    let re = regex_static!(
        r"\b(?P<word>[A-Za-z]+)(?P<mark>[.?])[ \t]+",
        "sentence-break pattern"
    );
    let mut edits = Vec::new();
    for captures in re.captures_iter(input) {
        let word = captures.name("word").unwrap();
        if attached_word(input, word.start()) || word.as_str().len() == 1 {
            continue;
        }
        let mark = captures.name("mark").unwrap();
        let end = captures.get(0).unwrap().end();
        let tail = &input[end..];
        let previous = word.as_str().to_ascii_lowercase();
        let merge = if coordinator(&previous) {
            !coordinator_restart(&previous, first_word(tail))
        } else if mark.as_str() == "?" {
            matches!(previous.as_str(), "an" | "your" | "our" | "their") && nominal_start(tail)
        } else {
            previous != "his"
                && continues_phrase(&previous, tail)
                && !(preposition(&previous) && may_be_stranded(&input[..word.start()]))
        };
        if !merge {
            continue;
        }
        let Some((word_end, next)) = continuation(input, end) else {
            continue;
        };
        edits.push((
            mark.start(),
            word_end,
            format!("{}{next}", &input[mark.end()..end]),
        ));
    }
    apply_replacements(input, edits)
}

/// Remove commas after eligible function words.
///
/// # Returns
/// The repaired text and number of commas removed.
///
/// # Errors
/// Infallible; the return value is a transformation, not a fallible result.
///
/// # Panics
/// Captures come from a fixed pattern and slices use its UTF-8 boundaries.
pub(super) fn remove_function_word_commas(input: &str) -> TransformResult<'_> {
    let re = regex_static!(
        r"\b(?P<word>[A-Za-z]+)(?P<mark>,)[ \t]+\b",
        "function-word comma pattern"
    );
    let edits = re.captures_iter(input).filter_map(|captures| {
        let word = captures.name("word").unwrap();
        let mark = captures.name("mark").unwrap();
        let tail = &input[captures.get(0).unwrap().end()..];
        (continues_phrase(word.as_str(), tail)
            && !tail
                .split_whitespace()
                .next()
                .is_some_and(|w| w.ends_with(','))
            && !attached_word(input, word.start())
            && word.as_str().len() != 1)
            .then(|| (mark.start(), mark.end(), ""))
    });
    apply_replacements(input, edits)
}

/// Remove filled pauses while retaining sentence boundaries and likely clause restarts.
///
/// # Returns
/// Text without fillers or their orphaned punctuation, and the removal count.
///
/// # Errors
/// Infallible; the return value is a transformation, not a fallible result.
///
/// # Panics
/// Captures come from a fixed pattern and slices use its UTF-8 boundaries.
pub(super) fn remove_filled_pauses(input: &str) -> TransformResult<'_> {
    let re = regex_static!(
        r"(^|[.!?])(?:\s*\b(?i:u[hm]+|er+m+)\b(?:,?\s+\b(?i:u[hm]+|er+m+)\b)*[.!?]+)+|,?\s*\b(?i:u[hm]+|er+m+)\b\s*,?(?P<terminal>[.!?]+)?",
        "filled-pause pattern"
    );
    let mut matches = re.captures_iter(input).peekable();
    let mut edits = Vec::new();
    let mut pending_terminal: Option<&str> = None;
    while let Some(captures) = matches.next() {
        let whole = captures.get(0).unwrap();
        let mut end = whole.end();
        let mut replacement = format!("{} ", captures.get(1).map_or("", |m| m.as_str()));
        let terminal = captures.name("terminal").map(|m| m.as_str());
        // A chain such as "um? Uh." carries its question to the next real word.
        let pending = pending_terminal.take();
        if let Some(terminal) = pending
            .filter(|s| s.contains(['?', '!']))
            .or(terminal)
            .or(pending)
        {
            let tail = &input[end..];
            let gap = tail.len() - tail.trim_start_matches([' ', '\t']).len();
            let next_start = end + gap;
            if matches
                .peek()
                .is_some_and(|next| next.get(0).unwrap().start() <= next_start)
            {
                pending_terminal = Some(terminal);
            } else {
                let previous = input[..whole.start()]
                    .split_whitespace()
                    .rev()
                    .map(first_word)
                    .find(|word| !word.is_empty() && !re.is_match(word))
                    .unwrap_or("");
                if let Some((word_end, next)) = continuation(input, next_start)
                    .filter(|_| !filler_ends_sentence(previous, terminal, &input[next_start..]))
                {
                    replacement.push_str(&input[end..next_start]);
                    replacement.push_str(&next);
                    end = word_end;
                } else {
                    replacement = format!("{terminal} ");
                }
            }
        }
        edits.push((whole.start(), end, replacement));
    }
    apply_replacements(input, edits)
}

fn first_word(text: &str) -> &str {
    text.split(|c: char| !c.is_alphabetic())
        .next()
        .unwrap_or("")
}

fn coordinator(word: &str) -> bool {
    matches!(word.to_ascii_lowercase().as_str(), "and" | "or" | "but")
}

fn coordinator_restart(previous: &str, next: &str) -> bool {
    coordinator(previous) && coordinator(next) && !previous.eq_ignore_ascii_case(next)
}

fn filler_ends_sentence(previous: &str, terminal: &str, tail: &str) -> bool {
    let next = first_word(tail);
    if coordinator_restart(previous, next) {
        return true;
    }
    if clause_link(previous) {
        return false;
    }
    if starts_clause(tail) {
        return true;
    }
    if determiner(previous) || preposition(previous) {
        return false;
    }
    // A word followed by a determiner can begin an imperative ("Set a goal")
    // or continue the preceding subject ("I um. Need a ride").
    let second = first_word(tail.split_whitespace().nth(1).unwrap_or(""));
    terminal.contains(['?', '!'])
        || determiner(next)
        || next.eq_ignore_ascii_case("let")
        || (determiner(second) && !starts_clause(previous))
}

fn clause_link(word: &str) -> bool {
    coordinator(word)
        || matches!(
            word.to_ascii_lowercase().as_str(),
            "if" | "when"
                | "while"
                | "because"
                | "although"
                | "unless"
                | "whether"
                | "what"
                | "which"
                | "who"
                | "where"
                | "how"
                | "why"
        )
}

/// Recognize subject-led clauses and auxiliary questions, without inferring verb vocabulary.
fn starts_clause(text: &str) -> bool {
    let mut words = text.split_whitespace();
    let first_token = words.next().unwrap_or("");
    let first = first_word(first_token).to_ascii_lowercase();
    let second = first_word(words.next().unwrap_or("")).to_ascii_lowercase();
    let subject = |word: &str| matches!(word, "i" | "we" | "you" | "he" | "she" | "they");
    let auxiliary = |word: &str| {
        matches!(
            word,
            "am" | "is"
                | "are"
                | "was"
                | "were"
                | "have"
                | "has"
                | "had"
                | "do"
                | "does"
                | "did"
                | "can"
                | "could"
                | "will"
                | "would"
                | "shall"
                | "should"
                | "may"
                | "might"
                | "must"
        )
    };
    subject(&first)
        || (matches!(
            first.as_str(),
            "it" | "this" | "that" | "these" | "those" | "there"
        ) && (auxiliary(&second) || first_token.contains(['\'', '’'])))
        || (auxiliary(&first)
            && (subject(&second) || matches!(second.as_str(), "it" | "this" | "that" | "there")))
        || matches!(
            first.as_str(),
            "if" | "unless" | "although" | "while" | "when"
        )
}

/// Bound determiners that expect a following nominal, also used after prepositions.
fn determiner(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "a" | "an" | "the" | "my" | "your" | "our" | "their" | "its" | "his"
    )
}

/// Prepositions repaired only when an explicit determiner phrase follows.
fn preposition(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "to" | "of" | "into" | "from" | "with" | "by"
    )
}

/// Exclude clause starts, asides, and nested determiner or partitive restarts.
fn nominal_start(text: &str) -> bool {
    let word = first_word(text);
    let second = first_word(text.split_whitespace().nth(1).unwrap_or(""));
    let partitive = second.eq_ignore_ascii_case("of")
        && matches!(
            word.to_ascii_lowercase().as_str(),
            "all"
                | "any"
                | "both"
                | "each"
                | "either"
                | "few"
                | "many"
                | "most"
                | "much"
                | "neither"
                | "none"
                | "one"
                | "several"
                | "some"
        );
    !word.is_empty()
        && !partitive
        && !starts_clause(text)
        && !clause_link(word)
        && !determiner(word)
        && !preposition(word)
        && !matches!(
            word.to_ascii_lowercase().as_str(),
            "actually"
                | "well"
                | "no"
                | "ok"
                | "okay"
                | "anyway"
                | "never"
                | "let"
                | "in"
                | "on"
                | "at"
                | "as"
                | "after"
                | "before"
                | "until"
                | "like"
                | "then"
                | "so"
                | "please"
                | "this"
                | "that"
                | "these"
                | "those"
        )
}

/// Require a bound determiner or a preposition with an explicit determiner complement.
fn continues_phrase(previous: &str, text: &str) -> bool {
    if determiner(previous) {
        return nominal_start(text);
    }
    let next = first_word(text);
    if !preposition(previous) || !determiner(next) || starts_clause(text) {
        return false;
    }
    text.split_once(char::is_whitespace)
        .is_some_and(|(_, tail)| nominal_start(tail.trim_start_matches([' ', '\t'])))
}

/// A wh-clause or relative construction can leave its preposition sentence-final.
fn may_be_stranded(prefix: &str) -> bool {
    let start = prefix
        .char_indices()
        .rfind(|&(i, c)| {
            c == '\n'
                || (punctuation_is_sentence_boundary(prefix, i, c)
                    && prefix[i + 1..]
                        .trim_start_matches(['\'', '"', '’', '”', ')', ']', '}'])
                        .starts_with(char::is_whitespace))
        })
        .map_or(0, |(i, _)| i + 1);
    regex_static!(
        r"(?i)\b(?:who|whom|whose|what|which|when|where|why|how|whether|that['’]s|the[ \t]+one)\b",
        "stranded preposition context"
    )
    .is_match(&prefix[start..])
}

fn attached_word(input: &str, start: usize) -> bool {
    input[..start].chars().next_back().is_some_and(|c| {
        c.is_alphanumeric() || matches!(c, '_' | '-' | '\'' | '’' | '.' | '/' | '@')
    })
}

fn continuation(input: &str, start: usize) -> Option<(usize, String)> {
    let re = regex_static!(
        r"^[\p{L}][\p{L}\p{M}'’0-9_.:/@-]*",
        "continuation word pattern"
    );
    let word = re.find(&input[start..])?.as_str().trim_end_matches('.');
    let preserve = word == "I"
        || word.starts_with("I'")
        || word.starts_with("I’")
        || protected_token_case(word)
        || CALENDAR_NAMES.contains(&word.split(['\'', '’', '-']).next().unwrap());
    let mut next = word.to_owned();
    if !preserve {
        let first = word.chars().next().unwrap();
        next.replace_range(..first.len_utf8(), &first.to_lowercase().to_string());
    }
    Some((start + word.len(), next))
}

const CALENDAR_NAMES: &[&str] = &[
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
