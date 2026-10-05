//! Linux direct typing guards shared with focused regression tests.

use anyhow::{anyhow, Result};
use std::error::Error;
use std::fmt;
use std::time::Duration;

/// Direct typing stopped after posting only part of the requested text.
#[derive(Debug)]
pub(crate) struct DirectTypingFailure {
    typed_chars: usize,
    total_chars: usize,
    blocked: bool,
    cause: anyhow::Error,
}

impl DirectTypingFailure {
    fn blocked(typed_chars: usize, total_chars: usize, cause: anyhow::Error) -> Self {
        Self {
            typed_chars,
            total_chars,
            blocked: true,
            cause,
        }
    }

    fn operational(typed_chars: usize, total_chars: usize, cause: anyhow::Error) -> Self {
        Self {
            typed_chars,
            total_chars,
            blocked: false,
            cause,
        }
    }

    /// Number of characters successfully emitted before typing stopped.
    ///
    /// # Returns
    ///
    /// Completed Unicode scalar values.
    pub(crate) fn typed_chars(&self) -> usize {
        self.typed_chars
    }

    /// Total characters requested for the insertion attempt.
    ///
    /// # Returns
    ///
    /// Requested Unicode scalar values.
    pub(crate) fn total_chars(&self) -> usize {
        self.total_chars
    }

    /// Human-readable cause reported by the failed guard or backend call.
    ///
    /// # Returns
    ///
    /// The complete formatted error chain.
    pub(crate) fn reason(&self) -> String {
        format!("{:#}", self.cause)
    }

    /// Whether a deliberate focus/modifier/input guard stopped insertion.
    ///
    /// # Returns
    ///
    /// `true` for an intentional safety block, or `false` for an operational
    /// backend failure.
    pub(crate) fn is_blocked(&self) -> bool {
        self.blocked
    }
}

impl fmt::Display for DirectTypingFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "direct insertion stopped after {} of {} characters: {:#}",
            self.typed_chars, self.total_chars, self.cause
        )
    }
}

impl Error for DirectTypingFailure {}

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
/// The number of characters posted after every character succeeds.
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
) -> std::result::Result<usize, DirectTypingFailure> {
    let total_chars = text.chars().count();
    if text.chars().any(char::is_control) {
        return Err(DirectTypingFailure::blocked(
            0,
            total_chars,
            anyhow!(
                "Linux direct insertion cannot type control characters, including tabs and newlines"
            ),
        ));
    }
    let modifiers_released = super::wait_for_x11_modifier_release(timeout, &mut modifiers_held)
        .map_err(|cause| DirectTypingFailure::operational(0, total_chars, cause))?;
    if !modifiers_released {
        return Err(DirectTypingFailure::blocked(
            0,
            total_chars,
            anyhow!("direct insertion blocked because physical modifiers remained held"),
        ));
    }
    let mut typed_chars = 0;
    for character in text.chars() {
        let focus_matches = before_character()
            .map_err(|cause| DirectTypingFailure::operational(typed_chars, total_chars, cause))?;
        if !focus_matches {
            return Err(DirectTypingFailure::blocked(
                typed_chars,
                total_chars,
                anyhow!("direct insertion stopped because focus changed"),
            ));
        }
        let modifier_held = modifiers_held()
            .map_err(|cause| DirectTypingFailure::operational(typed_chars, total_chars, cause))?;
        if modifier_held {
            return Err(DirectTypingFailure::blocked(
                typed_chars,
                total_chars,
                anyhow!("direct insertion stopped because a physical modifier is held"),
            ));
        }
        type_character(character)
            .map_err(|cause| DirectTypingFailure::operational(typed_chars, total_chars, cause))?;
        typed_chars += 1;
    }
    Ok(typed_chars)
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
            assert!(result.unwrap_err().is_blocked(), "{text:?}");
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
        assert!(result.unwrap_err().is_blocked());
    }

    #[test]
    fn unreadable_modifiers_fail_closed() {
        let result = type_text_guarded(
            "a",
            Duration::ZERO,
            || anyhow::bail!("X11 query failed"),
            || panic!("unreadable modifiers must block"),
            |_| panic!("unreadable modifiers must not post input"),
        );
        let failure = result.unwrap_err();
        assert!(!failure.is_blocked());
        assert!(failure.to_string().contains("X11 query failed"));
    }

    #[test]
    fn checks_focus_after_modifier_release_and_types_unicode() {
        let ready = Cell::new(false);
        let mut polls = 0;
        let mut typed = String::new();
        let typed_chars = type_text_guarded(
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
        assert_eq!(typed_chars, 3);
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
        let failure = result.unwrap_err();
        assert_eq!(failure.typed_chars(), 1);
        assert_eq!(failure.total_chars(), 2);
        assert!(failure.is_blocked());
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
        let failure = result.unwrap_err();
        assert_eq!(failure.typed_chars(), 1);
        assert_eq!(failure.total_chars(), 2);
        assert!(failure.is_blocked());
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
                    anyhow::bail!("X11 query failed");
                }
                Ok(false)
            },
            || Ok(true),
            |character| {
                typed.push(character);
                Ok(())
            },
        );
        let failure = result.unwrap_err();
        assert_eq!(failure.typed_chars(), 1);
        assert_eq!(failure.total_chars(), 2);
        assert!(!failure.is_blocked());
        assert!(failure.to_string().contains("X11 query failed"));
        assert_eq!(typed, "a");
    }

    #[test]
    fn focus_query_and_backend_failures_preserve_unicode_progress() {
        for fail_focus in [true, false] {
            let mut typed = String::new();
            let mut focus_checks = 0;
            let result = type_text_guarded(
                "é日🚀",
                Duration::ZERO,
                || Ok(false),
                || {
                    focus_checks += 1;
                    if fail_focus && focus_checks == 2 {
                        anyhow::bail!("focus query failed");
                    }
                    Ok(true)
                },
                |character| {
                    if !fail_focus && character == '日' {
                        anyhow::bail!("typing backend failed");
                    }
                    typed.push(character);
                    Ok(())
                },
            );
            let failure = result.unwrap_err();
            assert_eq!(failure.typed_chars(), 1);
            assert_eq!(failure.total_chars(), 3);
            assert!(!failure.is_blocked());
            assert_eq!(typed, "é");
            assert!(failure.reason().contains(if fail_focus {
                "focus query failed"
            } else {
                "typing backend failed"
            }));
        }
    }
}
