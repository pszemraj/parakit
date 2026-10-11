//! Keeps the shipped config documentation in step with the config schema.
//!
//! `docs/config_reference.toml` documents every key, `docs/example.toml` is the
//! starter config, and [`TEMPLATE`] is what `parakit config init` writes. The
//! set of accepted keys is derived from the deserializer itself: serde's
//! unknown-key error lists the names a table accepts. The comparisons fail in
//! both directions, so a key added to or removed from the schema cannot go
//! undocumented, and a documented key cannot outlive its removal.

use super::*;
use crate::cli::{Cli, StartCli};
use clap::ValueEnum;
use std::collections::{BTreeMap, BTreeSet};

const REFERENCE: &str = include_str!("../docs/config_reference.toml");
const EXAMPLE: &str = include_str!("../docs/example.toml");
const CONFIGURATION_GUIDE: &str = include_str!("../docs/configuration.md");

/// Key that no table accepts, used to make serde list the accepted names.
const PROBE_KEY: &str = "zz_probe";

/// Tables nested inside [`ConfigFile`], as dotted paths. A trailing `[]` marks
/// an array of tables. This is the only hand-written part of the schema walk;
/// [`accepted_key_paths`] fails when a listed table is not reachable, and a
/// table missing here surfaces as an undocumented leaf key.
const TABLES: &[&str] = &[
    "daemon",
    "cleaning",
    "logging",
    "hotkey",
    "rules",
    "rules.user[]",
];

/// Join a table path and a key name into a dotted key path.
fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}.{name}")
    }
}

/// Build a document whose only content is an unknown key inside `table`.
fn probe_document(table: &str) -> String {
    if table.is_empty() {
        return format!("{PROBE_KEY} = 1\n");
    }
    match table.strip_suffix("[]") {
        Some(array) => format!("[[{array}]]\n{PROBE_KEY} = 1\n"),
        None => format!("[{table}]\n{PROBE_KEY} = 1\n"),
    }
}

/// Return the key names `table` accepts, as listed by serde's unknown-key error.
fn accepted_names(table: &str) -> BTreeSet<String> {
    let error = toml::from_str::<ConfigFile>(&probe_document(table))
        .expect_err("an unknown key must be rejected");
    let message = error.message();
    assert!(
        message.contains(&format!("unknown field `{PROBE_KEY}`")),
        "table `{table}`: probe rejected for another reason: {message}"
    );
    let (_, listed) = message
        .split_once("expected ")
        .unwrap_or_else(|| panic!("table `{table}`: error lists no accepted keys: {message}"));
    listed
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// Return every scalar-or-list key path the program accepts, such as
/// `daemon.threads` or `rules.user[].pattern`.
fn accepted_key_paths() -> BTreeSet<String> {
    fn walk(table: &str, keys: &mut BTreeSet<String>, visited: &mut BTreeSet<String>) {
        visited.insert(table.to_owned());
        for name in accepted_names(table) {
            let path = join(table, &name);
            let nested = TABLES.iter().copied().find(|candidate| {
                *candidate == path || candidate.strip_suffix("[]") == Some(path.as_str())
            });
            match nested {
                Some(child) => walk(child, keys, visited),
                None => {
                    keys.insert(path);
                }
            }
        }
    }

    let mut keys = BTreeSet::new();
    let mut visited = BTreeSet::new();
    walk("", &mut keys, &mut visited);
    for table in TABLES {
        assert!(
            visited.contains(*table),
            "table `{table}` in TABLES is not accepted by the schema"
        );
    }
    keys
}

/// Whether `text` is a TOML key assignment with a plain snake_case key.
fn is_key_assignment(text: &str) -> bool {
    text.split_once(" = ").is_some_and(|(name, _)| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    })
}

/// Whether `text` is a whole-line TOML table or array-of-tables header.
fn is_table_header(text: &str) -> bool {
    !text.contains(' ')
        && ((text.starts_with("[[") && text.ends_with("]]"))
            || (text.starts_with('[') && text.ends_with(']') && !text.starts_with("[[")))
}

/// Return the dotted path a header opens, with `[]` marking an array of tables.
fn header_path(header: &str) -> String {
    match header.strip_prefix("[[") {
        Some(array) => format!("{}[]", array.trim_end_matches("]]")),
        None => header
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned(),
    }
}

/// Return `text` with every commented-out example key and table header
/// uncommented, so the examples can be parsed and loaded.
fn uncomment_examples(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        match line.strip_prefix("# ") {
            Some(rest) if is_key_assignment(rest) || is_table_header(rest) => out.push_str(rest),
            _ => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// Flatten parsed TOML into the key paths used by [`accepted_key_paths`].
fn flatten(value: &toml::Value, path: &str, keys: &mut BTreeSet<String>) {
    match value {
        toml::Value::Table(table) => {
            for (name, child) in table {
                flatten(child, &join(path, name), keys);
            }
        }
        toml::Value::Array(items)
            if !items.is_empty() && items.iter().all(toml::Value::is_table) =>
        {
            for item in items {
                flatten(item, &format!("{path}[]"), keys);
            }
        }
        _ => {
            keys.insert(path.to_owned());
        }
    }
}

/// Return the key paths a TOML text uses, counting commented-out example keys.
fn documented_key_paths(text: &str) -> BTreeSet<String> {
    let table: toml::Table = uncomment_examples(text)
        .parse()
        .expect("text must parse once its examples are uncommented");
    let mut keys = BTreeSet::new();
    flatten(&toml::Value::Table(table), "", &mut keys);
    keys
}

/// Return the key paths a TOML text sets without uncommenting anything.
fn set_key_paths(text: &str) -> BTreeSet<String> {
    let table: toml::Table = text.parse().expect("text must be valid TOML");
    let mut keys = BTreeSet::new();
    flatten(&toml::Value::Table(table), "", &mut keys);
    keys
}

/// One key of the reference and the comment lines directly above it.
struct KeyBlock {
    /// Whether the key line is commented out in the file.
    commented_out: bool,
    /// Comment lines above the key, without the leading `# `.
    comments: Vec<String>,
}

impl KeyBlock {
    fn first_line(&self) -> &str {
        self.comments.first().map_or("", String::as_str)
    }
}

/// Group the keys of a reference file with the comment block above each.
///
/// A blank line or a table header ends a comment block, so a section
/// introduction must be separated from the first key by a blank line.
fn key_blocks(text: &str) -> BTreeMap<String, KeyBlock> {
    let mut blocks = BTreeMap::new();
    let mut table = String::new();
    let mut comments: Vec<String> = Vec::new();
    for line in text.lines() {
        let (commented_out, body) = match line.strip_prefix("# ") {
            Some(rest) if is_key_assignment(rest) || is_table_header(rest) => (true, rest),
            _ => (false, line),
        };
        if is_table_header(body) {
            table = header_path(body);
            comments.clear();
        } else if is_key_assignment(body) {
            let name = body.split_once(" = ").expect("checked above").0;
            let previous = blocks.insert(
                join(&table, name),
                KeyBlock {
                    commented_out,
                    comments: std::mem::take(&mut comments),
                },
            );
            assert!(
                previous.is_none(),
                "`{}` is documented twice",
                join(&table, name)
            );
        } else if let Some(comment) = line.strip_prefix('#') {
            comments.push(comment.trim_start().to_owned());
        } else {
            comments.clear();
        }
    }
    blocks
}

fn write_fixture(name: &str, contents: &str) -> PathBuf {
    let dir = crate::test_support::fixture_root("parakit-config-reference-test", name);
    let path = dir.join("config.toml");
    std::fs::write(&path, contents).expect("write fixture config file");
    path
}

/// Render every effective value of the config, one entry per scalar key,
/// resolved the way the daemon resolves it with no CLI flags.
fn effective_snapshot(config: &ConfigFile) -> BTreeMap<&'static str, String> {
    let start = StartCli::default();
    let cli = Cli {
        command: None,
        quiet: false,
        verbose: false,
    };
    let cleaner = parakit::rules::build_enabled_cleaner(
        start.effective_cleaning_profile(config),
        start.effective_drops_trailing_period(config),
        config.cleaning.number_threshold,
        &start.effective_disabled_rules(config),
        &config.rules.user,
    )
    .expect("a loaded config builds a cleaner");

    let mut snapshot = BTreeMap::new();
    let mut put = |key: &'static str, value: String| {
        snapshot.insert(key, value);
    };
    put(
        "daemon.model",
        format!("{:?}", start.effective_model(config)),
    );
    put(
        "daemon.device",
        format!("{:?}", start.effective_device(config)),
    );
    put(
        "daemon.threads",
        format!("{:?}", start.effective_threads(config)),
    );
    put(
        "daemon.model_idle_minutes",
        start.effective_model_idle_minutes(config).to_string(),
    );
    put(
        "daemon.paste_mode",
        format!("{:?}", start.effective_paste_mode(config)),
    );
    put(
        "daemon.keep_transcript_clipboard",
        start
            .effective_keep_transcript_clipboard(config)
            .to_string(),
    );
    put(
        "daemon.sounds",
        start.effective_sounds_enabled(config).to_string(),
    );
    put("daemon.verbose", cli.effective_verbose(config).to_string());
    put(
        "daemon.transcript_history",
        start.effective_transcript_history(config).to_string(),
    );
    put(
        "cleaning.enabled",
        start.effective_cleaning_enabled(config).to_string(),
    );
    put("cleaning.profile", cleaner.profile().to_string());
    put(
        "cleaning.keep_trailing_period",
        (!cleaner.drops_trailing_period()).to_string(),
    );
    put(
        "cleaning.number_threshold",
        cleaner.number_threshold().to_string(),
    );
    put(
        "cleaning.disabled_rules",
        format!("{:?}", start.effective_disabled_rules(config)),
    );
    put(
        "logging.dir",
        format!("{:?}", start.effective_log_dir(config)),
    );
    #[cfg(target_os = "linux")]
    put(
        "hotkey.backend",
        format!("{:?}", start.effective_hotkey_backend(config)),
    );
    // Off Linux the key is accepted and never read, so no value can differ.
    #[cfg(not(target_os = "linux"))]
    put("hotkey.backend", "ignored".to_owned());
    put("rules.user[]", config.rules.user.len().to_string());
    snapshot
}

#[test]
fn accepted_keys_are_listed_per_table() {
    // A few spot checks keep the derivation itself honest: if the serde error
    // format changed, every other test here would pass vacuously.
    let accepted = accepted_key_paths();
    for key in [
        "daemon.model",
        "daemon.transcript_history",
        "cleaning.disabled_rules",
        "logging.dir",
        "hotkey.backend",
        "rules.user[].name",
        "rules.user[].position",
    ] {
        assert!(accepted.contains(key), "{key} missing from {accepted:?}");
    }
}

#[test]
fn reference_documents_exactly_the_accepted_keys() {
    let accepted = accepted_key_paths();
    let documented = documented_key_paths(REFERENCE);
    let undocumented: Vec<_> = accepted.difference(&documented).collect();
    let unknown: Vec<_> = documented.difference(&accepted).collect();
    assert!(
        undocumented.is_empty() && unknown.is_empty(),
        "docs/config_reference.toml is out of step with the schema\n  accepted but not documented: {undocumented:?}\n  documented but not accepted: {unknown:?}"
    );
}

#[test]
fn template_documents_exactly_the_accepted_keys() {
    let accepted = accepted_key_paths();
    let documented = documented_key_paths(TEMPLATE);
    let undocumented: Vec<_> = accepted.difference(&documented).collect();
    let unknown: Vec<_> = documented.difference(&accepted).collect();
    assert!(
        undocumented.is_empty() && unknown.is_empty(),
        "config::TEMPLATE is out of step with the schema\n  accepted but not in the template: {undocumented:?}\n  in the template but not accepted: {unknown:?}"
    );

    let path = write_fixture("template-uncommented", &uncomment_examples(TEMPLATE));
    load_from_path(&path).expect("every example value in the template must load");
}

#[test]
fn reference_loads_and_every_value_is_the_default() {
    let path = write_fixture("reference", REFERENCE);
    let loaded = load_from_path(&path).expect("docs/config_reference.toml must load as written");
    assert_eq!(
        effective_snapshot(&loaded),
        effective_snapshot(&ConfigFile::default()),
        "a value set in docs/config_reference.toml differs from the program default"
    );
}

#[test]
fn reference_examples_load_once_uncommented() {
    let path = write_fixture("reference-uncommented", &uncomment_examples(REFERENCE));
    let loaded = load_from_path(&path)
        .expect("every commented-out example in docs/config_reference.toml must load");
    assert_eq!(
        loaded.rules.user.len(),
        1,
        "one example [[rules.user]] entry"
    );
}

#[test]
fn effective_snapshot_covers_every_key() {
    let covered: BTreeSet<String> = effective_snapshot(&ConfigFile::default())
        .keys()
        .map(|key| (*key).to_owned())
        .collect();
    let accepted: BTreeSet<String> = accepted_key_paths()
        .into_iter()
        .map(|key| match key.split_once("[].") {
            Some((array, _)) => format!("{array}[]"),
            None => key,
        })
        .collect();
    assert_eq!(
        covered, accepted,
        "effective_snapshot must report one entry per accepted key, so a new key cannot skip the default check"
    );
}

#[test]
fn every_reference_key_has_a_description_and_a_status() {
    let blocks = key_blocks(REFERENCE);
    let documented = documented_key_paths(REFERENCE);
    for key in &documented {
        let block = blocks
            .get(key)
            .unwrap_or_else(|| panic!("{key}: no key line found in docs/config_reference.toml"));
        let first = block.first_line();
        assert!(
            block.comments.len() >= 2,
            "{key}: needs a type line and a description above it"
        );
        let required = first.contains("(required)");
        let optional = first.contains("(optional)");
        if block.commented_out {
            assert!(
                required || optional || first.contains("Default:"),
                "{key}: a commented-out key must lead with (required), (optional), or Default: ({first})"
            );
        } else {
            assert!(
                !required && !optional,
                "{key}: an uncommented key carries its default, not (required) or (optional) ({first})"
            );
        }
        if optional {
            assert!(
                block.comments.iter().any(|line| line.starts_with("Unset:")),
                "{key}: an (optional) key needs an Unset: line"
            );
        }
    }
}

/// Return the strings a user types for every variant of a `ValueEnum`.
fn variant_names<T: ValueEnum>() -> Vec<String> {
    T::value_variants()
        .iter()
        .map(|variant| {
            variant
                .to_possible_value()
                .expect("every variant has a possible value")
                .get_name()
                .to_owned()
        })
        .collect()
}

/// Assert that the type line above `key` quotes every value in `values`.
fn assert_values_listed(key: &str, values: &[String]) {
    let blocks = key_blocks(REFERENCE);
    let type_line = blocks
        .get(key)
        .unwrap_or_else(|| panic!("{key}: not documented"))
        .first_line()
        .to_owned();
    for value in values {
        assert!(
            type_line.contains(&format!("\"{value}\"")),
            "{key}: valid value \"{value}\" is not in its type line: {type_line}"
        );
    }
}

#[test]
fn closed_value_sets_are_fully_enumerated() {
    assert_values_listed("daemon.device", &variant_names::<DeviceMode>());
    assert_values_listed("daemon.paste_mode", &variant_names::<PasteMode>());
    assert_values_listed(
        "cleaning.profile",
        &[CleaningProfile::Safe, CleaningProfile::Aggressive]
            .map(|profile| profile.as_str().to_owned()),
    );
    assert_values_listed(
        "rules.user[].position",
        &[
            parakit::rules::RulePosition::First,
            parakit::rules::RulePosition::Standard,
            parakit::rules::RulePosition::Last,
        ]
        .map(|position| position.as_str().to_owned()),
    );
}

#[cfg(target_os = "linux")]
#[test]
fn hotkey_backend_values_are_fully_enumerated() {
    assert_values_listed("hotkey.backend", &variant_names::<HotkeyBackend>());
}

#[test]
fn example_loads_and_uses_only_documented_keys() {
    let path = write_fixture("example", EXAMPLE);
    load_from_path(&path).expect("docs/example.toml must load as written");

    let documented = documented_key_paths(REFERENCE);
    for key in set_key_paths(EXAMPLE) {
        assert!(
            documented.contains(&key),
            "docs/example.toml sets {key}, which docs/config_reference.toml does not document"
        );
    }
}

#[test]
fn configuration_guide_sample_matches_the_example() {
    let marker = "```toml\n";
    let start = CONFIGURATION_GUIDE
        .find(marker)
        .expect("docs/configuration.md has a toml sample")
        + marker.len();
    let end = start
        + CONFIGURATION_GUIDE[start..]
            .find("```")
            .expect("the toml sample is closed");
    let sample: toml::Table = CONFIGURATION_GUIDE[start..end]
        .parse()
        .expect("the sample in docs/configuration.md is valid TOML");
    let example: toml::Table = EXAMPLE.parse().expect("docs/example.toml is valid TOML");
    assert_eq!(
        sample, example,
        "the first toml sample in docs/configuration.md must equal docs/example.toml"
    );
}
