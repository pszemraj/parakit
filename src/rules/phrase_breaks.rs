//! Function-word punctuation repair and filled-pause removal.
//!
//! A mark is removed only when function words show that its sentence continues.
//! The word lists are closed grammatical classes; where they leave a mark
//! undecided, as with any pair of open-class words, the mark stays.

use std::borrow::Cow;

use super::engine::TransformResult;
use super::passes::{
    apply_replacements, is_any, protected_token_case, punctuation_is_sentence_boundary,
};

// Words that leave a sentence unfinished, by what they need next.

/// Introduce a word, phrase, or clause.
const COORDINATORS: &[&str] = &["and", "or", "but"];
/// Introduce a clause or, for `despite`, a noun phrase.
const SUBORDINATORS: &[&str] = &["because", "if", "whether", "although", "unless", "despite"];
/// Introduce a noun phrase. `its` and `his` can also stand alone.
const DETERMINERS: &[&str] = &["a", "an", "the", "my", "your", "our", "their"];
/// Nominative pronouns, which introduce a verb phrase.
const SUBJECTS: &[&str] = &["i", "we", "he", "she", "they"];
/// Introduce a noun phrase unless stranded. Adverbial particles are excluded.
const PREPOSITIONS: &[&str] = &["to", "of", "into", "from", "with", "for", "without"];

/// Start a question or relative clause.
const WH_WORDS: &[&str] = &[
    "who", "whom", "whose", "which", "what", "when", "where", "why", "how",
];
/// Other function words: pronouns, demonstratives, quantifiers, auxiliaries,
/// conjunctions, and prepositions. One after an unfinished phrase starts something new.
const RESTARTS: &[&str] = &[
    "you", "it", "me", "him", "her", "us", "them", "this", "that", "these", "those", "there",
    "here", "some", "any", "all", "each", "every", "either", "neither", "both", "none", "few",
    "many", "most", "much", "several", "am", "is", "are", "was", "were", "have", "has", "had",
    "do", "does", "did", "can", "could", "will", "would", "shall", "should", "may", "might",
    "must", "not", "so", "nor", "yet", "while", "until", "since", "than", "as", "like", "in", "on",
    "at", "by", "after", "before", "about", "through", "during", "between", "over", "under",
    "versus",
];

/// Merge periods and question marks inside a phrase or relative clause.
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
    let re = regex_static!(r"\w([.?])[ \t]+", "sentence-break pattern");
    let edits = re.captures_iter(input).filter_map(|captures| {
        let mark = captures.get(1).unwrap();
        let end = captures.get(0).unwrap().end();
        let left = last_sentence(&input[..mark.start()]);
        let right = &input[end..];
        let merge = match mark.as_str() {
            "." if !punctuation_is_sentence_boundary(input, mark.start(), '.') => false,
            "." if open_relative(left) => true,
            // A single letter can be a label, and a pronoun can end an inverted question.
            mark => {
                let word = last_word(left);
                word.len() > 1 && !is_any(word, SUBJECTS) && joins(left, mark, right)
            }
        };
        if !merge {
            return None;
        }
        let (word_end, next) = continuation(input, end)?;
        let gap = &input[mark.end()..end];
        Some((mark.start(), word_end, format!("{gap}{next}")))
    });
    apply_replacements(input, edits)
}

/// Remove commas inside a phrase unless another soon closes an aside.
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
        r"\b(?P<word>[A-Za-z]{2,})(?P<mark>,)[ \t]+\b",
        "function-word comma pattern"
    );
    let mut text = Cow::Borrowed(input);
    let mut matches = 0;
    // Right to left, so a removed comma cannot close an earlier aside.
    let candidates: Vec<_> = re.captures_iter(input).collect();
    for captures in candidates.iter().rev() {
        let word = captures.name("word").unwrap().as_str();
        let mark = captures.name("mark").unwrap();
        let left = last_sentence(&input[..mark.start()]);
        let right = &text[captures.get(0).unwrap().end()..];
        // Conjunctions often open a clause with an aside. A subordinator that does not
        // start its sentence still joins a clause that starts with a pronoun.
        let opener = is_any(word, COORDINATORS)
            || (is_any(word, SUBORDINATORS) && (left.trim() == word || !subject(right)));
        let aside = right
            .split_whitespace()
            .take(2)
            .any(|token| token.ends_with(','));
        if !opener && !aside && joins(left, ",", right) {
            text.to_mut().remove(mark.start());
            matches += 1;
        }
    }
    TransformResult { text, matches }
}

/// Remove filled pauses, keeping a mark before words that can start a sentence.
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
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut pending_terminal: Option<&str> = None;
    let mut left = "";
    let mut chain_end = None;
    while let Some(captures) = matches.next() {
        let whole = captures.get(0).unwrap();
        let mut end = whole.end();
        let mut replacement = format!("{} ", captures.get(1).map_or("", |m| m.as_str()));
        let terminal = captures.name("terminal").map(|m| m.as_str());
        // A chain such as "um? Uh." carries its context and question to the next real word.
        if chain_end != Some(whole.start()) {
            left = last_sentence(&input[..whole.start()]);
        }
        chain_end = Some(whole.end());
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
            } else if let Some((word_end, next)) =
                continuation(input, next_start).filter(|&(word_end, _)| {
                    // The word can embed a filler (`Store-um`); an edit must not reach into it.
                    let clear = matches
                        .peek()
                        .is_none_or(|next| next.get(0).unwrap().start() >= word_end);
                    let right = &input[next_start..];
                    clear
                        && (joins(left, terminal, right)
                            || (is_any(last_word(left), WH_WORDS) && subject(right))
                            || (!terminal.contains(['?', '!']) && resumes(right)))
                })
            {
                replacement.push_str(&input[end..next_start]);
                replacement.push_str(&next);
                end = word_end;
            } else {
                // The kept mark follows the word before the chain, so the spaces that
                // earlier fillers of the chain left would only detach it.
                let mut boundary = whole.start();
                for earlier in edits.iter_mut().rev() {
                    if earlier.1 != boundary || !earlier.2.trim().is_empty() {
                        break;
                    }
                    earlier.2.clear();
                    boundary = earlier.0;
                }
                replacement = format!("{terminal} ");
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

/// The word that ends `text`, empty when it belongs to a compound or identifier.
fn last_word(text: &str) -> &str {
    let rest = text.trim_end_matches(char::is_alphabetic);
    let attached = rest.ends_with(|c: char| {
        c.is_alphanumeric() || matches!(c, '_' | '-' | '\'' | '’' | '.' | '/' | '@')
    });
    if attached {
        ""
    } else {
        &text[rest.len()..]
    }
}

/// The text after the last sentence boundary.
fn last_sentence(text: &str) -> &str {
    let start = text
        .char_indices()
        .rfind(|&(i, c)| {
            c == '\n'
                || (punctuation_is_sentence_boundary(text, i, c)
                    && text[i + 1..]
                        .trim_start_matches(['\'', '"', '’', '”', ')', ']', '}'])
                        .starts_with(char::is_whitespace))
        })
        .map_or(0, |(i, _)| i + 1);
    &text[start..]
}

/// Whether the last word of `left` is unfinished and `right` can complete it.
fn joins(left: &str, mark: &str, right: &str) -> bool {
    let word = last_word(left);
    let next = first_word(right);
    if word == "A" {
        return false; // A label or grade.
    }
    if is_any(next, COORDINATORS) {
        return next.eq_ignore_ascii_case(word);
    }
    if is_any(word, COORDINATORS) || is_any(word, SUBORDINATORS) {
        return true;
    }
    if is_any(word, DETERMINERS) {
        return continues_phrase(right);
    }
    if mark.contains('?') {
        return false;
    }
    if is_any(word, SUBJECTS) {
        return continues_phrase(right);
    }
    if !is_any(word, PREPOSITIONS) || object_precedes(left) {
        return false;
    }
    // An infinitive can be elided after `to` and can strand other prepositions, as in
    // `to take care of`; only a determiner shows that their object follows.
    let infinitive = left
        .split_whitespace()
        .rev()
        .take(4)
        .any(|word| is_any(word, &["to"]));
    is_any(next, DETERMINERS) || (!infinitive && continues_phrase(right))
}

/// Whether `text` starts with an open-class word that no comma sets off.
fn continues_phrase(text: &str) -> bool {
    let token = text.split_whitespace().next().unwrap_or("");
    let word = first_word(token);
    let contraction = regex_static!(
        r"(?i)(?:n['’]t|['’](?:re|ve|ll|d))\b",
        "contraction pattern"
    );
    !word.is_empty()
        && !token.ends_with(',')
        && !contraction.is_match(token)
        && ![
            COORDINATORS,
            SUBORDINATORS,
            DETERMINERS,
            SUBJECTS,
            PREPOSITIONS,
            WH_WORDS,
            RESTARTS,
        ]
        .iter()
        .any(|class| is_any(word, class))
}

/// Whether `text` resumes a paused phrase rather than starting a sentence. A lone word,
/// a contraction such as `Let's`, or a clause shape such as `Set a goal` starts one.
fn resumes(text: &str) -> bool {
    let mut tokens = text.split_whitespace();
    let first = tokens.next().unwrap_or("");
    let second = tokens.next().unwrap_or("");
    continues_phrase(text)
        && !first.ends_with(['.', '?', '!'])
        && !first.contains(['\'', '’'])
        && !subject(second)
        && !is_any(first_word(second), DETERMINERS)
}

/// Whether a wh-clause or relative clause (`the team we spoke with`) can supply the
/// object of the preposition that ends `left`.
fn object_precedes(left: &str) -> bool {
    let words: Vec<&str> = left.split_whitespace().collect();
    words.iter().any(|word| is_any(first_word(word), WH_WORDS))
        || words
            .windows(2)
            .rfind(|pair| subject(pair[1]))
            .is_some_and(|pair| is_any(pair[0], &["that"]) || continues_phrase(pair[0]))
}

/// Whether `text` starts with a pronoun that can be a subject.
fn subject(text: &str) -> bool {
    let word = first_word(text);
    is_any(word, SUBJECTS) || is_any(word, &["you", "it"])
}

/// Whether `left` ends inside a relative clause, as in `the ones that are. Pending`.
fn open_relative(left: &str) -> bool {
    regex_static!(
        r"(?i)\b(?:which[ \t]+(?:is|are|was|were)|that[ \t]+(?:are|were))$",
        "open relative clause pattern"
    )
    .is_match(left)
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
    "April",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
