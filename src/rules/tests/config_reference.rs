//! The built-in rule inventory in `docs/config_reference.toml`.
//!
//! `cleaning.disabled_rules` accepts exactly the built-in rule names, so the
//! reference lists them. This keeps that list, and each rule's tier, equal to
//! the rule table that `parakit rules list` prints.

use super::DEFAULT_RULES;

const REFERENCE: &str = include_str!("../../../docs/config_reference.toml");

/// Parse inventory lines of the form `#   name (tier): description`.
///
/// The three-space indent after `# ` separates inventory entries from prose.
fn documented_inventory() -> Vec<(String, String)> {
    REFERENCE
        .lines()
        .filter_map(|line| {
            let entry = line.strip_prefix("#   ")?;
            if entry.starts_with(' ') {
                return None;
            }
            let (name, rest) = entry.split_once(" (")?;
            let (tier, _) = rest.split_once("): ")?;
            Some((name.to_owned(), tier.to_owned()))
        })
        .collect()
}

#[test]
fn reference_lists_every_builtin_rule_with_its_tier_in_table_order() {
    let table: Vec<(String, String)> = DEFAULT_RULES
        .iter()
        .map(|rule| (rule.name.to_owned(), rule.activation.label().to_owned()))
        .collect();
    assert_eq!(
        documented_inventory(),
        table,
        "the rule inventory in docs/config_reference.toml must match DEFAULT_RULES in order, with the tier label `parakit rules list` prints"
    );
}
