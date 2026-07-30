//! Procedural cleaning passes: acronyms, structural numbers/versions, and
//! sentence capitalization. Spoken cardinal parsing lives in the sibling
//! `numbers` module.
//!
//! These transforms are used as [`crate::rules::engine::RuleKind::Procedural`]
//! entries in [`crate::rules::defaults::DEFAULT_RULES`] because they need
//! context a single regex substitution cannot express (e.g. counting
//! consecutive words, or tracking sentence-boundary state across the whole
//! input).

use regex::Regex;
use std::fmt::Write as _;
use std::sync::OnceLock;
use text2num::{text2digits, Language};

use super::engine::TransformResult;

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
pub(crate) fn normalize_spaced_acronyms(input: &str) -> TransformResult {
    static RUN: OnceLock<Regex> = OnceLock::new();
    let re = RUN.get_or_init(|| {
        Regex::new(r"\b[A-Z](?: [A-Z])+\b").expect("spaced acronym regex must compile")
    });

    let mut output = String::with_capacity(input.len());
    let mut last_end = 0;
    let mut matches = 0;
    for found in re.find_iter(input) {
        output.push_str(&input[last_end..found.start()]);
        output.extend(
            found
                .as_str()
                .chars()
                .filter(|character| character.is_ascii_uppercase()),
        );
        last_end = found.end();
        matches += 1;
    }

    if matches == 0 {
        return unchanged(input);
    }
    output.push_str(&input[last_end..]);
    TransformResult {
        text: output,
        matches,
    }
}

#[derive(Clone, Copy, Debug)]
struct WordSpan {
    start: usize,
    end: usize,
}

/// Render multi-point spoken version numbers (e.g. `"one point two point
/// three"`) as dotted numeric versions (`"1.2.3"`), using `text2num` to
/// convert every component.
///
/// Scans runs of whitespace-separated words for a window of 5 to 16 words
/// containing at least two case-insensitive `"point"` tokens, splits the
/// window on those tokens, and converts each resulting phrase to digits
/// with `text2num`. A candidate is only replaced when every component
/// converts to a purely numeric string and at least three components
/// result, so ordinary uses of the word "point" are left alone.
///
/// # Returns
///
/// A [`TransformResult`] with matching spoken versions rendered as dotted
/// numeric strings and `matches` set to the number of versions rendered.
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
/// Slicing uses offsets returned by this same regex's matches on `input`,
/// which are always valid UTF-8 boundaries of `input`.
pub(crate) fn normalize_spoken_versions(input: &str) -> TransformResult {
    static WORD: OnceLock<Regex> = OnceLock::new();
    let word_re = WORD.get_or_init(|| {
        Regex::new(r"[A-Za-z]+(?:-[A-Za-z]+)*").expect("word span regex must compile")
    });
    let words: Vec<WordSpan> = word_re
        .find_iter(input)
        .map(|found| WordSpan {
            start: found.start(),
            end: found.end(),
        })
        .collect();
    if words.len() < 5 {
        return unchanged(input);
    }

    let mut replacements = Vec::new();
    let mut group_start = 0;
    while group_start < words.len() {
        let mut group_end = group_start + 1;
        while group_end < words.len()
            && input[words[group_end - 1].end..words[group_end].start]
                .chars()
                .all(char::is_whitespace)
        {
            group_end += 1;
        }

        let mut cursor = group_start;
        while cursor < group_end {
            let Some((relative_start, relative_end, replacement)) =
                find_spoken_version_candidate(input, &words[cursor..group_end])
            else {
                break;
            };
            let start_index = cursor + relative_start;
            let end_index = cursor + relative_end;
            replacements.push((
                words[start_index].start,
                words[end_index - 1].end,
                replacement,
            ));
            cursor = end_index;
        }
        group_start = group_end;
    }

    apply_replacements(input, replacements)
}

fn find_spoken_version_candidate(
    input: &str,
    words: &[WordSpan],
) -> Option<(usize, usize, String)> {
    const MIN_VERSION_WORDS: usize = 5;
    const MAX_VERSION_WORDS: usize = 16;

    for start in 0..words.len() {
        let maximum_end = words.len().min(start + MAX_VERSION_WORDS);
        if maximum_end.saturating_sub(start) < MIN_VERSION_WORDS {
            break;
        }
        for end in ((start + MIN_VERSION_WORDS)..=maximum_end).rev() {
            let candidate = &words[start..end];
            let point_count = candidate
                .iter()
                .filter(|span| input[span.start..span.end].eq_ignore_ascii_case("point"))
                .count();
            if point_count < 2 {
                continue;
            }
            if let Some(replacement) = render_spoken_version_candidate(input, candidate) {
                return Some((start, end, replacement));
            }
        }
    }
    None
}

fn render_spoken_version_candidate(input: &str, words: &[WordSpan]) -> Option<String> {
    let mut components = Vec::new();
    let mut component_start = 0;

    for (index, span) in words.iter().enumerate() {
        if !input[span.start..span.end].eq_ignore_ascii_case("point") {
            continue;
        }
        if component_start == index {
            return None;
        }
        components.push(parse_number_component(
            input,
            &words[component_start..index],
        )?);
        component_start = index + 1;
    }
    if component_start == words.len() {
        return None;
    }
    components.push(parse_number_component(input, &words[component_start..])?);
    (components.len() >= 3).then(|| components.join("."))
}

fn parse_number_component(input: &str, words: &[WordSpan]) -> Option<String> {
    let first = words.first()?;
    let last = words.last()?;
    let phrase = canonicalize_number_aliases(&input[first.start..last.end]);
    let language = Language::english();
    let rendered = text2digits(&phrase, &language).ok()?;
    rendered
        .chars()
        .all(|character| character.is_ascii_digit())
        .then_some(rendered)
}

fn canonicalize_number_aliases(input: &str) -> String {
    static OH: OnceLock<Regex> = OnceLock::new();
    let re = OH.get_or_init(|| Regex::new(r"(?i)\boh\b").expect("oh alias regex must compile"));
    re.replace_all(input, "zero").into_owned()
}

/// Append a spoken point-component to an existing numeric token (e.g. `"12
/// point three"` becomes `"12.3"`), using `text2num` to convert the spoken
/// component.
///
/// Repeatedly matches a numeric prefix followed by `"point"` and a spoken
/// word component, converting the component to digits with `text2num` and
/// appending it to the prefix with a `.` separator; a non-numeric
/// conversion leaves that occurrence unchanged. Runs up to 8 passes so a
/// chain such as `"12 point three point five"` is fully resolved, since
/// each pass can only extend the numeric prefix by one component.
///
/// # Returns
///
/// A [`TransformResult`] with matching point-suffixes appended and
/// `matches` set to the total number of appends across all passes.
///
/// # Errors
///
/// This function is infallible: it returns [`TransformResult`], not
/// `Result`, and never returns an `Err`.
///
/// # Panics
///
/// Does not panic in practice. The regex pattern is a compile-time literal
/// validated to compile. Its two capture groups are both required (not
/// optional) by the pattern, so `captures.get(1)`/`captures.get(2)` are
/// always `Some` for any match; `write!` to a `String` cannot fail. Slicing
/// uses offsets returned by this same regex's matches on `text`, which are
/// always valid UTF-8 boundaries of `text`.
pub(crate) fn normalize_numeric_point_suffixes(input: &str) -> TransformResult {
    static POINT_SUFFIX: OnceLock<Regex> = OnceLock::new();
    let re = POINT_SUFFIX.get_or_init(|| {
        Regex::new(r"(?i)(\d+(?:\.\d+)*)[ \t]+point[ \t]+([A-Za-z]+(?:-[A-Za-z]+)?)\b")
            .expect("numeric point suffix regex must compile")
    });

    let language = Language::english();
    let mut text = input.to_string();
    let mut total_matches = 0;

    for _ in 0..8 {
        let mut output = String::with_capacity(text.len());
        let mut last_end = 0;
        let mut pass_matches = 0;

        for captures in re.captures_iter(&text) {
            let found = captures.get(0).expect("full point-suffix match");
            let numeric = captures.get(1).expect("numeric point prefix").as_str();
            let component = canonicalize_number_aliases(
                captures.get(2).expect("spoken point component").as_str(),
            );
            let Ok(rendered) = text2digits(&component, &language) else {
                continue;
            };
            if !rendered.chars().all(|character| character.is_ascii_digit()) {
                continue;
            }

            output.push_str(&text[last_end..found.start()]);
            write!(output, "{numeric}.{rendered}").expect("writing to String cannot fail");
            last_end = found.end();
            pass_matches += 1;
        }

        if pass_matches == 0 {
            break;
        }
        output.push_str(&text[last_end..]);
        text = output;
        total_matches += pass_matches;
    }

    TransformResult {
        text,
        matches: total_matches,
    }
}

/// Join split digit groups onto a structurally recognizable uppercase
/// identifier prefix.
///
/// Matches an uppercase-led alphanumeric token followed by two or more
/// space-separated 1-2 digit groups. Two shapes are recognized:
/// * A short prefix (1-2 characters, excluding the standalone words `"A"`
///   and `"I"`) has all following digit groups joined directly onto it
///   with no space (e.g. `"F 22 04"` becomes `"F2204"`).
/// * A longer prefix (3+ characters) followed by exactly two 2-digit
///   groups keeps one space before the joined digits (e.g. `"ABC 12 34"`
///   becomes `"ABC 1234"`).
///
/// Any other digit-group shape matched by the pattern (e.g. a long prefix
/// with more than two groups) is left unchanged.
///
/// # Returns
///
/// A [`TransformResult`] with matching identifier groups joined and
/// `matches` set to the number of identifiers joined.
///
/// # Errors
///
/// This function is infallible: it returns [`TransformResult`], not
/// `Result`, and never returns an `Err`.
///
/// # Panics
///
/// Does not panic in practice. The regex pattern is a compile-time literal
/// validated to compile. Every match contains the pattern's required
/// `[A-Z][A-Z0-9]*` prefix token, so `parts.next()` is always `Some`.
/// Slicing uses offsets returned by this same regex's matches on `input`,
/// which are always valid UTF-8 boundaries of `input`.
pub(crate) fn normalize_numeric_identifier_groups(input: &str) -> TransformResult {
    static GROUPS: OnceLock<Regex> = OnceLock::new();
    let re = GROUPS.get_or_init(|| {
        Regex::new(r"\b[A-Z][A-Z0-9]*\b(?:[ \t]+\d{1,2}){2,}\b")
            .expect("numeric identifier group regex must compile")
    });

    let mut output = String::with_capacity(input.len());
    let mut last_end = 0;
    let mut matches = 0;

    for found in re.find_iter(input) {
        let mut parts = found.as_str().split_ascii_whitespace();
        let prefix = parts.next().expect("identifier match has a prefix");
        let groups: Vec<&str> = parts.collect();
        let short_prefix = prefix.len() <= 2 && !matches!(prefix, "A" | "I");
        let four_digit_model =
            prefix.len() >= 3 && groups.len() == 2 && groups.iter().all(|group| group.len() == 2);
        if !short_prefix && !four_digit_model {
            continue;
        }

        output.push_str(&input[last_end..found.start()]);
        output.push_str(prefix);
        if four_digit_model {
            output.push(' ');
        }
        for group in groups {
            output.push_str(group);
        }
        last_end = found.end();
        matches += 1;
    }

    if matches == 0 {
        return unchanged(input);
    }
    output.push_str(&input[last_end..]);
    TransformResult {
        text: output,
        matches,
    }
}

/// Join a numeric value (optionally with additional space-separated digit
/// groups) to a trailing uppercase `K`/`M`/`B` magnitude suffix by removing
/// the whitespace between them (e.g. `"1 5 M"` becomes `"15M"`).
///
/// # Returns
///
/// A [`TransformResult`] with matching numeric/suffix pairs joined and
/// `matches` set to the number of pairs joined.
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
/// Slicing uses offsets returned by this same regex's matches on `input`,
/// which are always valid UTF-8 boundaries of `input`.
pub(crate) fn normalize_magnitude_suffixes(input: &str) -> TransformResult {
    static MAGNITUDE: OnceLock<Regex> = OnceLock::new();
    let re = MAGNITUDE.get_or_init(|| {
        Regex::new(r"\b\d+(?:\.\d+)?(?:[ \t]+\d+)*[ \t]+[KMB]\b")
            .expect("numeric magnitude regex must compile")
    });

    let mut output = String::with_capacity(input.len());
    let mut last_end = 0;
    let mut matches = 0;
    for found in re.find_iter(input) {
        output.push_str(&input[last_end..found.start()]);
        output.extend(
            found
                .as_str()
                .chars()
                .filter(|character| !character.is_ascii_whitespace()),
        );
        last_end = found.end();
        matches += 1;
    }

    if matches == 0 {
        return unchanged(input);
    }
    output.push_str(&input[last_end..]);
    TransformResult {
        text: output,
        matches,
    }
}

fn apply_replacements(input: &str, replacements: Vec<(usize, usize, String)>) -> TransformResult {
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
        text: output,
        matches: replacements.len(),
    }
}

fn unchanged(input: &str) -> TransformResult {
    TransformResult {
        text: input.to_string(),
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
pub(crate) fn drop_dangling_connective(input: &str) -> TransformResult {
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
        text,
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
/// those are never corrupted. A `.`, `!`, or `?` opens a new sentence only
/// when [`punctuation_is_sentence_boundary`] agrees; that helper keeps URL
/// query/path punctuation, decimals, semvers, domains, ellipses, and known
/// abbreviations/initialisms (e.g. `"e.g."`, `"U.S."`) from being treated
/// as sentence ends, so a protected token is never split into two sentences
/// and its next character is never capitalized.
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
pub(crate) fn capitalize_sentence_starts(input: &str) -> TransformResult {
    let mut output = String::with_capacity(input.len());
    let mut capitalize_next = true;
    let mut changes = 0;

    for (byte_index, character) in input.char_indices() {
        if capitalize_next {
            if character.is_alphabetic() {
                if preserve_sentence_start_token_case(input, byte_index) {
                    output.push(character);
                } else {
                    for upper in character.to_uppercase() {
                        output.push(upper);
                    }
                    if character.is_lowercase() {
                        changes += 1;
                    }
                }
                capitalize_next = false;
                continue;
            }
            output.push(character);
            if character.is_numeric() {
                capitalize_next = false;
            }
            continue;
        }

        output.push(character);
        if punctuation_is_sentence_boundary(input, byte_index, character) {
            capitalize_next = true;
        }
    }

    TransformResult {
        text: output,
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

    lower.starts_with("http://")
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
