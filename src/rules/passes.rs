//! Procedural cleaning passes: acronyms, structural numbers/versions, and
//! sentence capitalization. Spoken cardinal parsing lives in the sibling
//! `numbers` module.
//!
//! These transforms are used as [`crate::rules::engine::RuleKind::Procedural`]
//! entries in [`crate::rules::defaults::DEFAULT_RULES`] because they need
//! context a single regex substitution cannot express (e.g. counting
//! consecutive words, or tracking sentence-boundary state across the whole
//! input).

use std::borrow::Cow;

use super::engine::TransformResult;

/// Expand apostrophe-cause while preserving exactly the spacing already
/// present around standalone forms and inserting one space for attached forms.
///
/// # Returns
///
/// A [`TransformResult`] with each apostrophe-cause form expanded and
/// `matches` set to the number of expansions.
///
/// # Errors
///
/// This function is infallible: it returns [`TransformResult`], not
/// `Result`, and never returns an `Err`.
///
/// # Panics
///
/// Does not panic in practice. The regex is a compile-time literal, and
/// capture zero always exists for each item from `captures_iter`.
pub(crate) fn normalize_apostrophe_cause(input: &str) -> TransformResult<'_> {
    let re = regex_static!(
        r#"(?i)['\u{2019}]cause\b"#,
        "apostrophe-cause regex must compile"
    );

    let replacements = re
        .captures_iter(input)
        .map(|captures| {
            let whole = captures.get(0).expect("capture zero must exist");
            let attached = input[..whole.start()]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric);
            let replacement = if attached { " because" } else { "because" }.to_string();
            (whole.start(), whole.end(), replacement)
        })
        .collect();

    apply_replacements(input, replacements)
}

/// Collapse runs of standalone uppercase letters separated by single ASCII
/// spaces (e.g. `"I B M"` becomes `"IBM"`).
///
/// This enforces a general structural invariant, not a lookup against a set
/// of known acronyms: any run of two or more single-letter, word-bounded
/// uppercase tokens separated by exactly one space collapses, regardless of
/// whether the result is a real acronym. There is deliberately no
/// vocabulary allowlist; this is a maintained design constraint, not an
/// oversight.
///
/// # Returns
///
/// A [`TransformResult`] with matching runs collapsed and `matches` set to
/// the number of runs collapsed.
///
/// # Errors
///
/// This function is infallible: it returns [`TransformResult`], not
/// `Result`, and never returns an `Err`.
///
/// # Panics
///
/// Does not panic in practice. The regex pattern is a compile-time literal
/// validated to compile, which is what the `.expect()` on it documents.
/// Byte-range slicing uses offsets returned by this same regex's matches on
/// `input`, which are always valid UTF-8 boundaries of `input`.
pub(crate) fn normalize_spaced_acronyms(input: &str) -> TransformResult<'_> {
    let re = regex_static!(r"\b[A-Z](?: [A-Z])+\b", "spaced acronym regex must compile");

    let replacements: Vec<(usize, usize, String)> = re
        .find_iter(input)
        .map(|found| {
            let collapsed: String = found
                .as_str()
                .chars()
                .filter(|character| character.is_ascii_uppercase())
                .collect();
            (found.start(), found.end(), collapsed)
        })
        .collect();

    apply_replacements(input, replacements)
}

fn apply_replacements(
    input: &str,
    replacements: Vec<(usize, usize, String)>,
) -> TransformResult<'_> {
    if replacements.is_empty() {
        return unchanged(input);
    }

    let mut output = String::with_capacity(input.len());
    let mut last_end = 0;
    for (start, end, replacement) in &replacements {
        debug_assert!(*start >= last_end);
        output.push_str(&input[last_end..*start]);
        output.push_str(replacement);
        last_end = *end;
    }
    output.push_str(&input[last_end..]);
    TransformResult {
        text: Cow::Owned(output),
        matches: replacements.len(),
    }
}

fn unchanged(input: &str) -> TransformResult<'_> {
    TransformResult {
        text: Cow::Borrowed(input),
        matches: 0,
    }
}

/// Whole words treated as a dangling trailing connective by
/// [`drop_dangling_connective`]. Case-insensitive, matched only as a
/// complete word. Deliberately tight: adding a word here removes it
/// whenever it is the very last word of a transcript, so only genuinely
/// unambiguous connectives belong on this list.
const DANGLING_CONNECTIVES: &[&str] = &["so", "but", "and", "or", "because"];

fn is_dangling_connective(word: &str) -> bool {
    DANGLING_CONNECTIVES
        .iter()
        .any(|candidate| word.eq_ignore_ascii_case(candidate))
}

/// Remove a trailing connective (`so`, `but`, `and`, `or`, `because`) left
/// dangling at the very end of a transcript, chaining through any further
/// trailing connectives, as long as the sentence content underneath still
/// ends with real sentence-ending punctuation.
///
/// Dictation often ends with a stranded conjunction (e.g. `"The build is
/// green. But"`) that connects to nothing and should be dropped, keeping
/// the punctuation that already closed the previous sentence. Scanning
/// backward from the end of the input, this repeatedly strips one trailing
/// connective word at a time -- which is what lets a chain such as `"It
/// works. And so."` fully resolve to `"It works."` -- but only commits once
/// the content underneath the whole chain (a) contains at least one
/// alphanumeric character and (b) ends with one of `. ! ? , ; :`; a bare
/// trailing connective with no reachable prior punctuation (including a
/// transcript that is nothing but a lone connective) is ambiguous and is
/// left untouched. A trailing `,`, `;`, or `:` immediately under the chain
/// is promoted to `.`; a trailing `.`, `!`, or `?` is kept as-is. Any
/// punctuation directly trailing the connective chain itself (e.g. the
/// period in `"so."`) is discarded, since the kept punctuation always comes
/// from what precedes the chain.
///
/// # Returns
///
/// A [`TransformResult`] with the trailing connective(s) removed and
/// `matches` set to the number of connective words dropped.
///
/// # Errors
///
/// This function is infallible: it returns [`TransformResult`], not
/// `Result`, and never returns an `Err`.
///
/// # Panics
///
/// Does not panic in practice. The one `.expect()` is guarded by the
/// immediately preceding emptiness check, which is what it documents. Every
/// byte offset used for slicing comes from `char_indices` on `input` or a
/// prefix of it, or from `len_utf8` of a character read out of that same
/// text, so all of them are valid UTF-8 boundaries of `input`.
pub(crate) fn drop_dangling_connective(input: &str) -> TransformResult<'_> {
    let mut cursor = input.trim_end().len();

    // The connective chain's own trailing punctuation (e.g. the period in
    // "so.") does not influence the kept punctuation, which always comes
    // from what precedes the chain, so it is simply discarded here.
    if let Some(last) = input[..cursor].chars().next_back() {
        if ".!?,;:".contains(last) {
            cursor -= last.len_utf8();
        }
    }

    let mut removed = 0usize;
    loop {
        let head = input[..cursor].trim_end();
        let word_start = head
            .char_indices()
            .rev()
            .find_map(|(index, character)| {
                (!character.is_alphabetic()).then_some(index + character.len_utf8())
            })
            .unwrap_or(0);
        if !is_dangling_connective(&head[word_start..]) {
            break;
        }
        removed += 1;
        cursor = word_start;
    }

    if removed == 0 {
        return unchanged(input);
    }

    let root = input[..cursor].trim_end();
    if root.is_empty() || !root.chars().any(char::is_alphanumeric) {
        return unchanged(input);
    }
    let last = root
        .chars()
        .next_back()
        .expect("root is non-empty, checked above");

    let mut text = String::with_capacity(root.len());
    match last {
        '.' | '!' | '?' => text.push_str(root),
        ',' | ';' | ':' => {
            text.push_str(&root[..root.len() - last.len_utf8()]);
            text.push('.');
        }
        _ => return unchanged(input),
    }

    TransformResult {
        text: Cow::Owned(text),
        matches: removed,
    }
}

/// Capitalize the first letter of each true sentence while leaving
/// non-sentence-start tokens untouched.
///
/// Walks the input character by character, tracking whether the next
/// alphabetic character begins a sentence. A character eligible for
/// capitalization is instead left as-is when
/// [`preserve_sentence_start_token_case`] identifies its token as a URL,
/// domain, email address, filename/extension, or a `v`-prefixed version
/// (via [`preserve_sentence_start_token_case`]'s dot/`@`/scheme checks), so
/// those are never corrupted. Existing mixed-case tokens such as `macOS`,
/// `iOS`, and `eBay` are also preserved. A `.`, `!`, or `?` opens a new
/// sentence only when [`punctuation_is_sentence_boundary`] agrees; that helper
/// keeps URL query/path punctuation, decimals, semvers, domains, ellipses, and
/// known abbreviations/initialisms (e.g. `"e.g."`, `"U.S."`) from being
/// treated as sentence ends, so a protected token is never split into two
/// sentences and its next character is never capitalized.
///
/// # Returns
///
/// A [`TransformResult`] with sentence-initial letters capitalized and
/// `matches` set to the number of letters actually changed from lowercase
/// to uppercase.
///
/// # Errors
///
/// This function is infallible: it returns [`TransformResult`], not
/// `Result`, and never returns an `Err`.
///
/// # Panics
///
/// The copied prefix uses an offset from `input.char_indices()`, which
/// always identifies a valid UTF-8 boundary of `input`.
pub(crate) fn capitalize_sentence_starts(input: &str) -> TransformResult<'_> {
    let mut output: Option<String> = None;
    let mut capitalize_next = true;
    let mut changes = 0;

    for (byte_index, character) in input.char_indices() {
        if capitalize_next {
            if character.is_alphabetic() {
                if preserve_sentence_start_token_case(input, byte_index) {
                    if let Some(output) = &mut output {
                        output.push(character);
                    }
                } else {
                    let uppercase = character.to_uppercase();
                    if !uppercase.clone().eq(std::iter::once(character)) {
                        let output = output.get_or_insert_with(|| {
                            let mut output = String::with_capacity(input.len());
                            output.push_str(&input[..byte_index]);
                            output
                        });
                        output.extend(uppercase);
                    } else if let Some(output) = &mut output {
                        output.push(character);
                    }
                    if character.is_lowercase() {
                        changes += 1;
                    }
                }
                capitalize_next = false;
                continue;
            }
            if let Some(output) = &mut output {
                output.push(character);
            }
            if character.is_numeric() {
                capitalize_next = false;
            }
            continue;
        }

        if let Some(output) = &mut output {
            output.push(character);
        }
        if punctuation_is_sentence_boundary(input, byte_index, character) {
            capitalize_next = true;
        }
    }

    TransformResult {
        text: output.map_or(Cow::Borrowed(input), Cow::Owned),
        matches: changes,
    }
}

fn punctuation_is_sentence_boundary(input: &str, byte_index: usize, character: char) -> bool {
    match character {
        '.' => period_is_sentence_boundary(input, byte_index),
        '!' | '?' => !punctuation_is_inside_protected_token(input, byte_index, character),
        _ => false,
    }
}

fn punctuation_is_inside_protected_token(
    input: &str,
    byte_index: usize,
    punctuation: char,
) -> bool {
    let token_start = input[..byte_index]
        .char_indices()
        .rev()
        .find_map(|(index, character)| {
            character
                .is_whitespace()
                .then_some(index + character.len_utf8())
        })
        .unwrap_or(0);
    let after_punctuation = byte_index + punctuation.len_utf8();
    let token_end = input[after_punctuation..]
        .char_indices()
        .find_map(|(offset, character)| {
            character
                .is_whitespace()
                .then_some(after_punctuation + offset)
        })
        .unwrap_or(input.len());

    input[after_punctuation..token_end]
        .chars()
        .any(char::is_alphanumeric)
        && protected_token_case(&input[token_start..token_end])
}

fn preserve_sentence_start_token_case(input: &str, byte_index: usize) -> bool {
    let tail = &input[byte_index..];
    let token = tail
        .split(|character: char| character.is_whitespace())
        .next()
        .unwrap_or("");
    protected_token_case(token)
}

fn protected_token_case(token: &str) -> bool {
    let token = token.trim_matches(|character: char| {
        matches!(
            character,
            '"' | '\''
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '<'
                | '>'
                | ','
                | ';'
                | ':'
                | '!'
                | '?'
        )
    });
    let lexical = token.trim_end_matches('.');
    let lower = lexical.to_ascii_lowercase();
    let has_internal_uppercase = lexical.chars().skip(1).any(char::is_uppercase);

    has_internal_uppercase
        || lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("www.")
        || lower.contains('@')
        || lower.contains('.')
        || (lower.starts_with('v')
            && lower
                .chars()
                .nth(1)
                .is_some_and(|character| character.is_ascii_digit()))
}

fn period_is_sentence_boundary(input: &str, period_index: usize) -> bool {
    let before = &input[..period_index];
    let after = &input[period_index + '.'.len_utf8()..];
    let previous = before.chars().next_back();
    let next = after.chars().next();

    // Decimals, versions, domains, file extensions, and dotted identifiers have
    // no whitespace around the dot and therefore are not sentence boundaries.
    if previous.is_some_and(|character| character.is_alphanumeric())
        && next.is_some_and(|character| character.is_alphanumeric())
    {
        return false;
    }

    // Treat a run of periods as one ellipsis boundary at its final dot.
    if next == Some('.') {
        return false;
    }
    if previous == Some('.') {
        return true;
    }

    let token_start = before
        .char_indices()
        .rev()
        .find_map(|(index, character)| {
            (!character.is_ascii_alphabetic() && character != '.')
                .then_some(index + character.len_utf8())
        })
        .unwrap_or(0);
    let dotted_token = format!("{}.", &before[token_start..]).to_ascii_lowercase();

    const ABBREVIATIONS: &[&str] = &[
        "a.m.", "dr.", "e.g.", "etc.", "fig.", "i.e.", "jr.", "mr.", "mrs.", "ms.", "no.", "p.m.",
        "prof.", "sr.", "st.", "vs.",
    ];
    if ABBREVIATIONS.contains(&dotted_token.as_str()) || is_initialism(&dotted_token) {
        return false;
    }

    true
}

fn is_initialism(token: &str) -> bool {
    let mut letters = 0;
    for piece in token.split('.') {
        if piece.is_empty() {
            continue;
        }
        if piece.chars().count() != 1
            || !piece
                .chars()
                .all(|character| character.is_ascii_alphabetic())
        {
            return false;
        }
        letters += 1;
    }
    letters >= 2
}
