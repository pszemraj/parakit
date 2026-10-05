//! Linux direct typing guards shared with focused regression tests.

use anyhow::{bail, Result};
use std::time::Duration;

/// Type printable text after waiting for modifiers, then recheck each character.
///
/// # Arguments
///
/// * `text` - Text to type without control characters.
/// * `timeout` - Initial physical-modifier release budget.
/// * `modifiers_held` - Live modifier query; errors prevent further typing.
/// * `before_character` - Focus guard run before each character.
/// * `type_character` - Existing platform character insertion backend.
///
/// # Returns
///
/// Success after every character has been posted.
///
/// # Errors
///
/// Rejects controls before posting input. Modifier query failures, held modifiers,
/// focus changes, and key failures stop insertion, including partway through text.
pub(super) fn type_text_guarded(
    text: &str,
    timeout: Duration,
    mut modifiers_held: impl FnMut() -> Result<bool>,
    mut before_character: impl FnMut() -> Result<bool>,
    mut type_character: impl FnMut(char) -> Result<()>,
) -> Result<()> {
    if text.chars().any(char::is_control) {
        bail!("Linux direct insertion cannot type control characters, including tabs and newlines");
    }
    if !super::wait_for_x11_modifier_release(timeout, &mut modifiers_held)? {
        bail!("direct insertion blocked because physical modifiers remained held");
    }
    for character in text.chars() {
        if !before_character()? {
            bail!("direct insertion stopped because focus changed");
        }
        if modifiers_held()? {
            bail!("direct insertion stopped because a physical modifier is held");
        }
        type_character(character)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, collections::VecDeque};

    #[test]
    fn rejects_controls_before_any_probe_or_input() {
        for text in ["a\tb", "a\nb", "a\rb", "a\0b", "a\u{7f}b", "a\u{85}b"] {
            let result = type_text_guarded(
                text,
                Duration::ZERO,
                || panic!("control text must not query modifiers"),
                || panic!("control text must not query focus"),
                |_| panic!("control text must not post input"),
            );
            assert!(result.is_err(), "{text:?}");
        }
    }

    #[test]
    fn held_modifiers_block_before_focus_or_input() {
        let result = type_text_guarded(
            "a",
            Duration::ZERO,
            || Ok(true),
            || panic!("focus must be checked after modifier readiness"),
            |_| panic!("held modifiers must not post input"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn unreadable_modifiers_fail_closed() {
        let result = type_text_guarded(
            "a",
            Duration::ZERO,
            || bail!("X11 query failed"),
            || panic!("unreadable modifiers must block"),
            |_| panic!("unreadable modifiers must not post input"),
        );
        assert!(result.unwrap_err().to_string().contains("X11 query failed"));
    }

    #[test]
    fn checks_focus_after_modifier_release_and_types_unicode() {
        let ready = Cell::new(false);
        let mut polls = 0;
        let mut typed = String::new();
        type_text_guarded(
            "é日🚀",
            Duration::from_millis(100),
            || {
                polls += 1;
                let held = polls == 1;
                ready.set(!held);
                Ok(held)
            },
            || {
                assert!(ready.get());
                Ok(true)
            },
            |character| {
                typed.push(character);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(typed, "é日🚀");
    }

    #[test]
    fn focus_changes_stop_before_typing_into_another_target() {
        let mut focus = VecDeque::from([true, false]);
        let mut typed = String::new();
        let result = type_text_guarded(
            "ab",
            Duration::ZERO,
            || Ok(false),
            || Ok(focus.pop_front().unwrap()),
            |character| {
                typed.push(character);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(typed, "a");
    }

    #[test]
    fn a_new_modifier_stops_remaining_characters() {
        let mut modifiers = VecDeque::from([false, false, true]);
        let mut typed = String::new();
        let result = type_text_guarded(
            "ab",
            Duration::ZERO,
            || Ok(modifiers.pop_front().unwrap()),
            || Ok(true),
            |character| {
                typed.push(character);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(typed, "a");
    }

    #[test]
    fn a_failed_modifier_recheck_stops_remaining_characters() {
        let mut queries = 0;
        let mut typed = String::new();
        let result = type_text_guarded(
            "ab",
            Duration::ZERO,
            || {
                queries += 1;
                if queries == 3 {
                    bail!("X11 query failed");
                }
                Ok(false)
            },
            || Ok(true),
            |character| {
                typed.push(character);
                Ok(())
            },
        );
        assert!(result.unwrap_err().to_string().contains("X11 query failed"));
        assert_eq!(typed, "a");
    }
}
