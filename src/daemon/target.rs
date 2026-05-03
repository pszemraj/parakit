//! Paste-target safety inspection.

use super::inject::FocusSnapshot;

/// Decision returned by target safety inspection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TargetDecision {
    /// Continue with normal paste handling.
    Allow,
    /// Copy the transcript, but do not synthesize a paste chord.
    CopyOnly(&'static str),
    /// Do not paste or copy the transcript.
    Block(&'static str),
}

/// Inspect the target captured at recording start.
///
/// # Arguments
///
/// * `focus` - Recording-start focus snapshot that already passed the drift
///   guard.
///
/// # Returns
///
/// A conservative insertion decision.
pub(crate) fn inspect_recording_target(focus: Option<&FocusSnapshot>) -> TargetDecision {
    inspect_recording_target_impl(focus)
}

#[cfg(target_os = "linux")]
fn inspect_recording_target_impl(focus: Option<&FocusSnapshot>) -> TargetDecision {
    let atspi = current_atspi_focus();
    recording_target_decision(focus.and_then(FocusSnapshot::wm_class), atspi)
}

#[cfg(not(target_os = "linux"))]
fn inspect_recording_target_impl(_focus: Option<&FocusSnapshot>) -> TargetDecision {
    TargetDecision::Allow
}

#[cfg(target_os = "linux")]
fn recording_target_decision(x11_class: Option<&str>, atspi: Option<AtspiFocus>) -> TargetDecision {
    let decision = target_decision(x11_class, atspi);
    if x11_class.is_none() && decision == TargetDecision::Allow {
        TargetDecision::CopyOnly("target window class unavailable")
    } else {
        decision
    }
}

#[cfg(target_os = "linux")]
fn target_decision(x11_class: Option<&str>, atspi: Option<AtspiFocus>) -> TargetDecision {
    if let Some(focus) = atspi {
        if focus.password {
            return TargetDecision::Block("password field");
        }
    }

    if let Some(class) = x11_class {
        if is_desktop_shell_class(class) {
            return TargetDecision::CopyOnly("desktop shell target");
        }
        if is_file_manager_class(class) {
            return match atspi {
                Some(focus) if focus.editable => TargetDecision::Allow,
                Some(_) => TargetDecision::CopyOnly("file manager target is not editable"),
                None => {
                    TargetDecision::CopyOnly("file manager target could not be verified editable")
                }
            };
        }
        if is_terminal_class(class) {
            return TargetDecision::Allow;
        }
        return match atspi {
            Some(focus) if focus.editable => TargetDecision::Allow,
            Some(_) => TargetDecision::CopyOnly("target is not editable"),
            None => TargetDecision::CopyOnly("target accessibility state unavailable"),
        };
    }

    TargetDecision::Allow
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AtspiFocus {
    password: bool,
    editable: bool,
}

#[cfg(target_os = "linux")]
fn current_atspi_focus() -> Option<AtspiFocus> {
    futures_lite::future::block_on(async {
        use atspi::proxy::accessible::ObjectRefExt as _;
        use atspi::proxy::collection::CollectionProxy;
        use atspi::{
            AccessibilityConnection, MatchType, ObjectMatchRule, Role, SortOrder, State, StateSet,
        };

        let connection = AccessibilityConnection::new().await.ok()?;
        let collection = CollectionProxy::new(connection.connection()).await.ok()?;

        let mut rule = ObjectMatchRule::default();
        rule.states = StateSet::from(State::Focused);
        rule.states_mt = MatchType::All;

        let matches = collection
            .get_matches(rule, SortOrder::Canonical, 8, true)
            .await
            .ok()?;

        for object in matches {
            if object.is_null() {
                continue;
            }
            let accessible = object
                .as_accessible_proxy(connection.connection())
                .await
                .ok()?;
            let role = accessible.get_role().await.ok()?;
            let states = accessible.get_state().await.ok()?;
            return Some(AtspiFocus {
                password: role == Role::PasswordText,
                editable: states.contains(State::Editable),
            });
        }
        None
    })
}

fn is_file_manager_class(class: &str) -> bool {
    let lower = class.to_lowercase();
    [
        "org.gnome.nautilus",
        "nautilus",
        "org.kde.dolphin",
        "dolphin",
        "thunar",
        "nemo",
        "pcmanfm",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

fn is_desktop_shell_class(class: &str) -> bool {
    let lower = class.to_lowercase();
    ["gnome-shell", "org.gnome.shell", "plasmashell", "xfdesktop"]
        .iter()
        .any(|pattern| lower.contains(pattern))
}

fn is_terminal_class(class: &str) -> bool {
    let lower = class.to_lowercase();
    [
        "gnome-terminal",
        "org.gnome.terminal",
        "org.gnome.console",
        "kgx",
        "kitty",
        "alacritty",
        "foot",
        "urxvt",
        "xterm",
        "st-256color",
        "org.kde.konsole",
        "wezterm",
        "tilix",
        "terminator",
        "ptyxis",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_manager_classes_are_copy_only_candidates() {
        assert!(is_file_manager_class("org.gnome.Nautilus"));
        assert!(is_file_manager_class("Dolphin"));
        assert!(!is_file_manager_class("Code"));
    }

    #[test]
    fn shell_classes_are_copy_only_candidates() {
        assert!(is_desktop_shell_class("gnome-shell"));
        assert!(is_desktop_shell_class("plasmashell"));
        assert!(!is_desktop_shell_class("kitty"));
    }

    #[test]
    fn terminal_classes_are_known_safe_without_atspi() {
        assert!(is_terminal_class("kitty"));
        assert!(is_terminal_class("org.gnome.Console"));
        assert!(is_terminal_class("Alacritty"));
        assert!(!is_terminal_class("Code"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn shell_class_stays_copy_only_even_with_atspi_focus() {
        assert_eq!(
            target_decision(
                Some("gnome-shell"),
                Some(AtspiFocus {
                    password: false,
                    editable: true,
                }),
            ),
            TargetDecision::CopyOnly("desktop shell target")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn non_terminal_targets_need_accessibility_state() {
        assert_eq!(
            target_decision(Some("firefox"), None),
            TargetDecision::CopyOnly("target accessibility state unavailable")
        );
        assert_eq!(
            target_decision(
                Some("firefox"),
                Some(AtspiFocus {
                    password: false,
                    editable: false,
                }),
            ),
            TargetDecision::CopyOnly("target is not editable")
        );
        assert_eq!(
            target_decision(
                Some("firefox"),
                Some(AtspiFocus {
                    password: false,
                    editable: true,
                }),
            ),
            TargetDecision::Allow
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn password_atspi_focus_blocks_before_copy_only_class_checks() {
        assert_eq!(
            target_decision(
                Some("gnome-shell"),
                Some(AtspiFocus {
                    password: true,
                    editable: true,
                }),
            ),
            TargetDecision::Block("password field")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn wm_class_parser_uses_class_half() {
        assert_eq!(
            crate::daemon::x11::parse_wm_class(b"org.gnome.Nautilus\0org.gnome.Nautilus\0"),
            Some("org.gnome.Nautilus".to_string())
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn missing_recording_target_class_is_copy_only() {
        assert_eq!(
            recording_target_decision(
                None,
                Some(AtspiFocus {
                    password: false,
                    editable: true,
                }),
            ),
            TargetDecision::CopyOnly("target window class unavailable")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn password_blocks_even_without_recording_target_class() {
        assert_eq!(
            recording_target_decision(
                None,
                Some(AtspiFocus {
                    password: true,
                    editable: true,
                }),
            ),
            TargetDecision::Block("password field")
        );
    }
}
