//! Function-word punctuation repair and filled-pause removal.
//!
//! A mark is removed only when function words show that its sentence continues.
//! The word lists are closed grammatical classes; where they leave a mark
//! undecided, as with any pair of open-class words, the mark stays.
//! A token in all capitals (`OR`, `IT`, `US`) is an abbreviation, never a function word.

use std::borrow::Cow;
use std::ops::Range;

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
/// Start a question when first in a sentence, and otherwise start something new.
const AUXILIARIES: &[&str] = &[
    "am", "is", "are", "was", "were", "do", "does", "did", "have", "has", "had", "can", "could",
    "will", "would", "shall", "should", "may", "might", "must",
];
/// Other function words: pronouns, demonstratives, quantifiers, conjunctions, and
/// prepositions. One after an unfinished phrase starts something new.
const RESTARTS: &[&str] = &[
    "you", "it", "me", "him", "her", "us", "them", "this", "that", "these", "those", "there",
    "here", "some", "any", "all", "each", "every", "either", "neither", "both", "none", "few",
    "many", "most", "much", "several", "not", "so", "nor", "yet", "while", "until", "since",
    "than", "as", "like", "in", "on", "at", "by", "after", "before", "about", "through", "during",
    "between", "over", "under", "versus",
];

/// Closing quotes and brackets, which can follow the terminal that ends a sentence.
const CLOSERS: [char; 7] = ['\'', '"', '’', '”', ')', ']', '}'];

/// Merge periods and question marks inside a phrase or relative clause.
///
/// A question mark that a merge removes moves to the end of the merged sentence when
/// that sentence opens as a question. A boundary that an earlier merge of the pass removed
/// no longer ends the sentence, so it does not cut the context of later decisions.
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
    let mut edits = Vec::new();
    let mut merged: Option<Merge> = None;
    for captures in re.captures_iter(input) {
        let mark = captures.get(1).unwrap();
        let end = captures.get(0).unwrap().end();
        let left_start = sentence_start(input, mark.start(), merged.as_ref());
        let left = &input[left_start..mark.start()];
        let right = &input[end..];
        let merge = match mark.as_str() {
            "." if !punctuation_is_sentence_boundary(input, mark.start(), '.') => false,
            "." if open_relative(left) => true,
            // A single letter can be a label, and a pronoun can end an inverted question.
            mark => {
                let word = last_word(left);
                word.len() > 1 && !in_class(word, SUBJECTS) && joins(left, mark, right)
            }
        };
        if !merge {
            continue;
        }
        let Some((word_end, next)) = continuation(input, end) else {
            continue;
        };
        let gap = &input[mark.end()..end];
        merged = Some(Merge {
            removed: mark.range(),
            sentence_start: left_start,
        });
        edits.push(Edit {
            start: mark.start(),
            end: word_end,
            text: format!("{gap}{next}"),
            carry: mark.as_str() == "?" && opens_question(left),
        });
    }
    apply_edits(input, edits)
}

/// Remove commas inside a phrase unless a later comma of the sentence closes an aside.
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
        let initial = from_first_letter(left) == word;
        // Conjunctions often open a clause with an aside. A subordinator that does not
        // start its sentence still joins a clause that starts with a pronoun.
        let opener = in_class(word, COORDINATORS)
            || (in_class(word, SUBORDINATORS) && (initial || !subject(right)));
        // Between a sentence's opening subject pronoun and its verb an aside can be a
        // whole phrase (`They, tired from the trip, went home`), so any comma that ends
        // it before the clause does closes one. Inside a noun or prepositional phrase an
        // aside is a word or two (`my, frankly, terrible advice`), so its closing comma
        // must be among the next two tokens.
        let aside = if initial && in_class(word, SUBJECTS) {
            closes_aside(&text, captures.get(0).unwrap().end())
        } else {
            right
                .split_whitespace()
                .take(2)
                .any(|token| token.ends_with(','))
        };
        if !opener && joins(left, ",", right) && !aside {
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
    let mut edits: Vec<Edit> = Vec::new();
    let mut pending_terminal: Option<&str> = None;
    let mut merged: Option<Merge> = None;
    let mut left = "";
    let mut left_start = 0;
    let mut chain_end = None;
    let mut chain_removed = 0;
    while let Some(captures) = matches.next() {
        let whole = captures.get(0).unwrap();
        let mut end = whole.end();
        let mut carry = false;
        let mut replacement = format!("{} ", captures.get(1).map_or("", |m| m.as_str()));
        let terminal = captures.name("terminal").map(|m| m.as_str());
        // A chain such as "um? Uh." carries its context and question to the next real word.
        if chain_end != Some(whole.start()) {
            left_start = sentence_start(input, whole.start(), merged.as_ref());
            left = &input[left_start..whole.start()];
            // The first alternative keeps the boundary that precedes its fillers.
            chain_removed = whole.start() + captures.get(1).map_or(0, |m| m.len());
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
                            || (in_class(last_word(left), WH_WORDS) && subject(right))
                            || (!terminal.contains(['?', '!']) && resumes(right)))
                })
            {
                replacement.push_str(&input[end..next_start]);
                replacement.push_str(&next);
                end = word_end;
                carry = terminal.contains('?') && opens_question(left);
                merged = Some(Merge {
                    removed: chain_removed..whole.end(),
                    sentence_start: left_start,
                });
            } else {
                // The kept mark follows the word before the chain, so the spaces that
                // earlier fillers of the chain left would only detach it.
                let mut boundary = whole.start();
                for earlier in edits.iter_mut().rev() {
                    if earlier.end != boundary || !earlier.text.trim().is_empty() {
                        break;
                    }
                    earlier.text.clear();
                    boundary = earlier.start;
                }
                // The match ends at the mark, so the text after it keeps its own spacing.
                replacement = terminal.to_owned();
            }
        }
        edits.push(Edit {
            start: whole.start(),
            end,
            text: replacement,
            carry,
        });
    }
    apply_edits(input, edits)
}

/// A replacement of `start..end`. With `carry`, the replacement ends a merge that removed
/// a question mark, which moves to the end of the sentence the merge builds.
struct Edit {
    start: usize,
    end: usize,
    text: String,
    carry: bool,
}

/// Apply ordered, non-overlapping edits, then carry the question marks they removed.
fn apply_edits(input: &str, edits: Vec<Edit>) -> TransformResult<'_> {
    let mut carries = Vec::new();
    let mut shift = 0isize;
    for edit in &edits {
        let start = edit.start.checked_add_signed(shift).unwrap();
        if edit.carry {
            carries.push(start + edit.text.len());
        }
        shift += edit.text.len() as isize - (edit.end - edit.start) as isize;
    }
    let mut result = apply_replacements(
        input,
        edits
            .iter()
            .map(|edit| (edit.start, edit.end, edit.text.as_str())),
    );
    if !carries.is_empty() {
        let text = result.text.to_mut();
        // Later marks first: an appended mark never moves an earlier offset.
        for &from in carries.iter().rev() {
            carry_question(text, from);
        }
    }
    result
}

/// Turn the period that ends the sentence at `from` into a question mark. A sentence that
/// ends in `?` or `!`, or in an ellipsis, stays; one without a terminal gets `?` after its
/// last word, inside any quote or bracket that closes it.
fn carry_question(text: &mut String, from: usize) {
    let ends = |rest: &str| {
        let rest = rest.trim_start_matches(CLOSERS);
        rest.is_empty() || rest.starts_with(char::is_whitespace)
    };
    let insert_at =
        |text: &str, to: usize| from + text[from..to].trim_end().trim_end_matches(CLOSERS).len();
    for (i, c) in text[from..].char_indices().map(|(i, c)| (from + i, c)) {
        if c == '\n' {
            text.insert(insert_at(text, i), '?');
            return;
        }
        let rest = &text[i + c.len_utf8()..];
        // An ellipsis ends the sentence as it is, whether typed as dots or as `…`.
        if c == '…' && ends(rest) {
            return;
        }
        if matches!(c, '.' | '?' | '!')
            && punctuation_is_sentence_boundary(text, i, c)
            && ends(rest)
        {
            if c == '.' && !text[..i].ends_with('.') {
                text.replace_range(i..=i, "?");
            }
            return;
        }
    }
    text.insert(insert_at(text, text.len()), '?');
}

/// Whether the sentence that `left` begins opens as an inverted question: an auxiliary or
/// its negative contraction followed by a subject (`Do you`, `Shouldn't we`, `Is the`), a
/// wh-word followed by an auxiliary (`Where did`, `Why didn't`), or a wh-word with an
/// attached `'s`, `'re`, `'ll`, or `'d` (`What's`). A wh-word also opens statements such
/// as `What I need is`, so any other wh-word does not count.
fn opens_question(left: &str) -> bool {
    let text = from_first_letter(left);
    let opener = word_with_apostrophes(text);
    if let Some((auxiliary, negated)) = auxiliary(opener) {
        return subject_follows(auxiliary, negated, &text[opener.len()..]);
    }
    let opener = first_word(text);
    if !in_class(opener, WH_WORDS) {
        return false;
    }
    let rest = &text[opener.len()..];
    let contraction = rest
        .strip_prefix(['\'', '’'])
        .is_some_and(|suffix| in_class(first_word(suffix), &["s", "re", "ll", "d"]));
    contraction
        || auxiliary(word_with_apostrophes(
            rest.trim_start_matches(char::is_whitespace),
        ))
        .is_some()
}

/// The auxiliary that `token` spells, and whether it is negated: a word of the class, or
/// its negative contraction, where `ca`, `wo`, and `sha` stand for the irregular stems of
/// `can't`, `won't`, and `shan't`.
fn auxiliary(token: &str) -> Option<(&str, bool)> {
    let negated = token
        .strip_suffix(['t', 'T'])
        .and_then(|stem| stem.strip_suffix(['\'', '’']))
        .and_then(|stem| stem.strip_suffix(['n', 'N']));
    let word = negated.unwrap_or(token);
    let irregular = [("ca", "can"), ("wo", "will"), ("sha", "shall")]
        .into_iter()
        .find(|&(stem, _)| negated.is_some() && in_class(word, &[stem]));
    let word = irregular.map_or(word, |(_, name)| name);
    in_class(word, AUXILIARIES).then_some((word, negated.is_some()))
}

/// Whether `text` starts the subject of an inverted question opened by `auxiliary`: a
/// subject pronoun, an article or possessive, a demonstrative, or `there`. Affirmative
/// base-form `do` and `have` also open imperatives (`Do the dishes`, `Have it ready`), so
/// after them only a pronoun that cannot be an object counts. A negated one never opens
/// an imperative with a subject, and `Don't forget` fails the subject check anyway.
fn subject_follows(auxiliary: &str, negated: bool, text: &str) -> bool {
    let text = text.trim_start_matches(char::is_whitespace);
    let word = first_word(text);
    if !negated && in_class(auxiliary, &["do", "have"]) {
        return in_class(word, SUBJECTS) || in_class(word, &["you"]);
    }
    subject(text)
        || in_class(word, DETERMINERS)
        || in_class(word, &["this", "that", "these", "those", "there"])
}

/// The word at the start of `text`, with the apostrophes inside it (`Shouldn’t`).
fn word_with_apostrophes(text: &str) -> &str {
    let end = text
        .find(|c: char| !(c.is_alphabetic() || matches!(c, '\'' | '’')))
        .unwrap_or(text.len());
    &text[..end]
}

/// The sentence `left` begins, without the quotes, brackets, or digits that precede its
/// first letter, so that an opening word is read the same inside `“They, ...”`.
fn from_first_letter(left: &str) -> &str {
    left.trim_start_matches(|c: char| !c.is_alphabetic())
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

/// The latest merge of a pass: the input span it removed, and where the sentence that it
/// joined began.
struct Merge {
    removed: Range<usize>,
    sentence_start: usize,
}

/// Where the sentence that ends at `end` begins. A boundary that `merge` removed does not
/// end a sentence, so the sentence extends back to the start of the one the merge joined.
/// Each merge records the start it found this way, so consecutive merges chain.
fn sentence_start(input: &str, end: usize, merge: Option<&Merge>) -> usize {
    let start = last_sentence_start(&input[..end]);
    match merge {
        // The boundary is the byte before the start of the sentence it cut.
        Some(merge) if start > 0 && merge.removed.contains(&(start - 1)) => merge.sentence_start,
        _ => start,
    }
}

/// Whether the first comma after `from` closes an aside that began before it. A comma
/// before a coordinator, a subordinator, or a wh-word ends the clause instead, so nothing
/// beyond it can close the aside, and neither can a sentence end or a line break.
fn closes_aside(text: &str, from: usize) -> bool {
    for (i, c) in text[from..].char_indices().map(|(i, c)| (from + i, c)) {
        if ends_sentence(text, i, c) {
            return false;
        }
        let rest = &text[i + c.len_utf8()..];
        if c == ',' && (rest.is_empty() || rest.starts_with(char::is_whitespace)) {
            let next = first_word(rest.trim_start());
            return ![COORDINATORS, SUBORDINATORS, WH_WORDS]
                .iter()
                .any(|class| in_class(next, class));
        }
    }
    false
}

/// Whether the character `c` at byte `i` of `text` ends a sentence: a line break, or a
/// terminal that is a sentence boundary and is followed, past any closing quotes or
/// brackets, by whitespace.
fn ends_sentence(text: &str, i: usize, c: char) -> bool {
    c == '\n'
        || (punctuation_is_sentence_boundary(text, i, c)
            && text[i + c.len_utf8()..]
                .trim_start_matches(CLOSERS)
                .starts_with(char::is_whitespace))
}

/// The text after the last sentence boundary.
fn last_sentence(text: &str) -> &str {
    &text[last_sentence_start(text)..]
}

/// The offset after the last sentence boundary of `text`.
fn last_sentence_start(text: &str) -> usize {
    text.char_indices()
        .rfind(|&(i, c)| ends_sentence(text, i, c))
        .map_or(0, |(i, _)| i + 1)
}

/// Whether the last word of `left` is unfinished and `right` can complete it.
fn joins(left: &str, mark: &str, right: &str) -> bool {
    let word = last_word(left);
    let next = first_word(right);
    if word == "A" {
        return false; // A label or grade.
    }
    if in_class(next, COORDINATORS) {
        return next.eq_ignore_ascii_case(word);
    }
    if in_class(word, COORDINATORS) || in_class(word, SUBORDINATORS) {
        return true;
    }
    if in_class(word, DETERMINERS) {
        return continues_phrase(right);
    }
    if mark.contains('?') {
        return false;
    }
    if in_class(word, SUBJECTS) {
        return continues_phrase(right);
    }
    if !in_class(word, PREPOSITIONS) || object_precedes(left) {
        return false;
    }
    // An infinitive can be elided after `to` and can strand other prepositions, as in
    // `to take care of`; only a determiner shows that their object follows.
    let infinitive = left
        .split_whitespace()
        .rev()
        .take(4)
        .any(|word| in_class(word, &["to"]));
    in_class(next, DETERMINERS) || (!infinitive && continues_phrase(right))
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
            AUXILIARIES,
            RESTARTS,
        ]
        .iter()
        .any(|class| in_class(word, class))
}

/// Whether `text` resumes a paused phrase rather than starting a sentence. A lone word,
/// whether it ends its sentence or the text, a contraction such as `Let's`, or a clause
/// shape such as `Set a goal` starts one.
fn resumes(text: &str) -> bool {
    let mut tokens = text.split_whitespace();
    let first = tokens.next().unwrap_or("");
    let second = tokens.next().unwrap_or("");
    continues_phrase(text)
        && !second.is_empty()
        && !first.ends_with(['.', '?', '!'])
        && !first.contains(['\'', '’'])
        && !subject(second)
        && !in_class(first_word(second), DETERMINERS)
}

/// Whether a wh-clause or relative clause (`the team we spoke with`) can supply the
/// object of the preposition that ends `left`.
fn object_precedes(left: &str) -> bool {
    let words: Vec<&str> = left.split_whitespace().collect();
    words
        .iter()
        .any(|word| in_class(first_word(word), WH_WORDS))
        || words
            .windows(2)
            .rfind(|pair| subject(pair[1]))
            .is_some_and(|pair| in_class(pair[0], &["that"]) || continues_phrase(pair[0]))
}

/// Whether `word` belongs to the closed word class `class`. A token of two or more
/// letters that are all uppercase (`OR`, `IT`, `US`, `WHO`) is an abbreviation, not the
/// function word it spells, so it belongs to no class.
fn in_class(word: &str, class: &[&str]) -> bool {
    let mut letters = word.chars().filter(|c| c.is_alphabetic());
    let abbreviation = letters.clone().nth(1).is_some() && letters.all(char::is_uppercase);
    !abbreviation && is_any(word, class)
}

/// Whether `text` starts with a pronoun that can be a subject.
fn subject(text: &str) -> bool {
    let word = first_word(text);
    in_class(word, SUBJECTS) || in_class(word, &["you", "it"])
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
