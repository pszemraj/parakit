//! Function-word punctuation repair and filled-pause removal.

use super::engine::TransformResult;
use super::passes::{apply_replacements, protected_token_case};

const SENTENCE_WORDS: &[&str] = &[
    "an", "your", "our", "their", "its", "his", "and", "or", "but",
];
const COMMA_WORDS: &[&str] = &["an", "our", "their", "its", "his"];

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
        if !SENTENCE_WORDS.contains(&word.as_str().to_ascii_lowercase().as_str())
            || attached_word(input, word.start())
        {
            continue;
        }
        let mark = captures.name("mark").unwrap();
        let end = captures.get(0).unwrap().end();
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
    let edits = re
        .captures_iter(input)
        .filter_map(|captures| {
            let word = captures.name("word").unwrap();
            let mark = captures.name("mark").unwrap();
            (COMMA_WORDS.contains(&word.as_str().to_ascii_lowercase().as_str())
                && !attached_word(input, word.start()))
            .then(|| (mark.start(), mark.end(), String::new()))
        })
        .collect();
    apply_replacements(input, edits)
}

/// Remove filled pauses, retaining boundaries supplied by preceding sentences.
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
    while let Some(captures) = matches.next() {
        let whole = captures.get(0).unwrap();
        let mut end = whole.end();
        let mut replacement = format!("{} ", captures.get(1).map_or("", |m| m.as_str()));
        if captures.name("terminal").is_some() {
            let tail = &input[end..];
            let gap = tail.len() - tail.trim_start_matches([' ', '\t']).len();
            let next_start = end + gap;
            if matches
                .peek()
                .is_none_or(|next| next.get(0).unwrap().start() > next_start)
            {
                if let Some((word_end, next)) = continuation(input, next_start) {
                    replacement.push_str(&input[end..next_start]);
                    replacement.push_str(&next);
                    end = word_end;
                }
            }
        }
        edits.push((whole.start(), end, replacement));
    }
    apply_replacements(input, edits)
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
        || PROPER_NAMES.contains(&word.split(['\'', '’', '-']).next().unwrap());
    let mut next = word.to_owned();
    if !preserve {
        let first = word.chars().next().unwrap();
        next.replace_range(..first.len_utf8(), &first.to_lowercase().to_string());
    }
    Some((start + word.len(), next))
}

// Names repeatedly capitalized inside historical dictation sentences.
const PROPER_NAMES: &[&str] = &[
    "Abilene",
    "Adam",
    "Adex",
    "Africa",
    "Airbnb",
    "Alan",
    "Alibaba",
    "Allen",
    "Altman",
    "Amazon",
    "America",
    "American",
    "Andrew",
    "Apache",
    "Apple",
    "April",
    "Asian",
    "Astra",
    "Atlanta",
    "August",
    "Austin",
    "Auth",
    "Battersea",
    "Bellman",
    "Ben",
    "Bible",
    "Bitwarden",
    "Bluetooth",
    "Bo",
    "Bob",
    "Bobble",
    "Borat",
    "Boston",
    "Brave",
    "Brazilian",
    "British",
    "Brock",
    "Buckmaster",
    "Bureau",
    "Bushism",
    "California",
    "Calvin",
    "Catholic",
    "Cemetery",
    "Chicago",
    "Chico",
    "China",
    "Chinese",
    "Chomp",
    "Chris",
    "Christ",
    "Chrome",
    "Claude",
    "Clot",
    "Cloudflare",
    "Coke",
    "Colab",
    "Colossus",
    "Coxon",
    "Ctrl",
    "Daddy",
    "Dario",
    "Databricks",
    "David",
    "Deberta",
    "December",
    "Dell",
    "Discord",
    "Disney",
    "Docker",
    "Doritos",
    "Dropbox",
    "Dyson",
    "Eastern",
    "Eats",
    "Einstein",
    "Elon",
    "Emmy",
    "English",
    "Equinox",
    "Ethernet",
    "Eton",
    "Europe",
    "European",
    "Excel",
    "Falcon",
    "Fappy",
    "February",
    "Fi",
    "Ford",
    "Francisco",
    "Franken",
    "Frankenstein",
    "French",
    "Friday",
    "Fuyu",
    "Gathering",
    "Gemini",
    "Gemma",
    "Georgia",
    "German",
    "Ghostie",
    "Gina",
    "Google",
    "Greek",
    "Grock",
    "Gucci",
    "Haskell",
    "Hobbit",
    "Holly",
    "Houston",
    "Hz",
    "Ian",
    "Indian",
    "Italian",
    "Jackson",
    "January",
    "Japan",
    "Jeek",
    "Jensen",
    "Jeppa",
    "Jesus",
    "Jinja",
    "Jira",
    "July",
    "June",
    "Keon",
    "Kimi",
    "Kimmy",
    "Kissinger",
    "Korea",
    "Korean",
    "Kroger",
    "Laguna",
    "Latin",
    "Laura",
    "Lee",
    "Legends",
    "Libra",
    "Libre",
    "Lily",
    "Linux",
    "Lite",
    "Loki",
    "London",
    "Luke",
    "Luna",
    "Luther",
    "Mac",
    "Macs",
    "Madison",
    "Maine",
    "Mamba",
    "Markov",
    "Matt",
    "Megalodon",
    "Metroska",
    "Microsoft",
    "Mike",
    "Mistral",
    "Monday",
    "Moondream",
    "Mr",
    "Nala",
    "Naomi",
    "Nemo",
    "Neobert",
    "Netlify",
    "Nobel",
    "North",
    "November",
    "October",
    "Olmo",
    "Palace",
    "Parakeit",
    "Pareto",
    "Pegasus",
    "Peng",
    "Peter",
    "Philips",
    "Pilates",
    "Pokemon",
    "Python",
    "Quan",
    "Quen",
    "Quentin",
    "Quinn",
    "Raven",
    "Reddit",
    "Reuters",
    "Rufus",
    "Russian",
    "Rust",
    "Safari",
    "Sagitt",
    "Samaritan",
    "Samaritans",
    "San",
    "Saturday",
    "Sauron",
    "Semble",
    "Seoul",
    "September",
    "Shitta",
    "Siri",
    "Sonic",
    "Sora",
    "Spark",
    "Spotify",
    "Steve",
    "Stokes",
    "Sudoku",
    "Sumble",
    "Sunday",
    "Swift",
    "Swiss",
    "Switzerland",
    "Tailscale",
    "Tebow",
    "Terra",
    "Texas",
    "Thanksgiving",
    "Thompson",
    "Thomson",
    "Thursday",
    "Topo",
    "Tory",
    "Treasure",
    "Triton",
    "Trump",
    "Tuesday",
    "Tuesdays",
    "Twitter",
    "Uber",
    "Ubuntu",
    "Ulmo",
    "Unicode",
    "United",
    "Unix",
    "Vim",
    "Vince",
    "Virginia",
    "Vulcan",
    "Vulture",
    "Wallace",
    "Wayland",
    "Waymo",
    "Wednesday",
    "West",
    "Wi",
    "Wikipedia",
    "York",
    "Zell",
    "Zurich",
];
