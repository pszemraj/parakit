//! Paste-target safety inspection.

use super::inject::FocusSnapshot;

/// Target state captured at the recording boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TargetSnapshot {
    #[cfg(target_os = "linux")]
    x11_class: Option<String>,
    #[cfg(target_os = "linux")]
    atspi: Option<AtspiFocus>,
}

impl TargetSnapshot {
    /// Capture paste-target metadata for the currently focused target.
    ///
    /// # Arguments
    ///
    /// * `focus` - X11 focus snapshot captured at the same boundary.
    ///
    /// # Returns
    ///
    /// Target metadata used later to decide whether automatic paste is safe.
    pub(crate) fn capture(focus: Option<&FocusSnapshot>) -> Self {
        capture_target_snapshot_impl(focus)
    }
}

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
/// * `target` - Recording-start paste target metadata.
///
/// # Returns
///
/// A conservative insertion decision.
pub(crate) fn inspect_recording_target(target: Option<&TargetSnapshot>) -> TargetDecision {
    inspect_recording_target_impl(target)
}

#[cfg(target_os = "linux")]
fn capture_target_snapshot_impl(focus: Option<&FocusSnapshot>) -> TargetSnapshot {
    TargetSnapshot {
        x11_class: focus.and_then(FocusSnapshot::wm_class).map(str::to_owned),
        atspi: current_atspi_focus(),
    }
}

#[cfg(not(target_os = "linux"))]
fn capture_target_snapshot_impl(_focus: Option<&FocusSnapshot>) -> TargetSnapshot {
    TargetSnapshot {}
}

#[cfg(target_os = "linux")]
fn inspect_recording_target_impl(target: Option<&TargetSnapshot>) -> TargetDecision {
    let Some(target) = target else {
        return TargetDecision::CopyOnly("recording target unavailable");
    };
    recording_target_decision(target, current_atspi_focus())
}

#[cfg(not(target_os = "linux"))]
fn inspect_recording_target_impl(_target: Option<&TargetSnapshot>) -> TargetDecision {
    TargetDecision::Allow
}

#[cfg(target_os = "linux")]
fn recording_target_decision(
    target: &TargetSnapshot,
    current_atspi: Option<AtspiFocus>,
) -> TargetDecision {
    let decision = target_decision(
        target.x11_class.as_deref(),
        target.atspi.as_ref(),
        current_atspi.as_ref(),
    );
    if target.x11_class.is_none() && decision == TargetDecision::Allow {
        TargetDecision::CopyOnly("target window class unavailable")
    } else {
        decision
    }
}

#[cfg(target_os = "linux")]
fn target_decision(
    x11_class: Option<&str>,
    recording_atspi: Option<&AtspiFocus>,
    current_atspi: Option<&AtspiFocus>,
) -> TargetDecision {
    if recording_atspi.is_some_and(|focus| focus.password)
        || current_atspi.is_some_and(|focus| focus.password)
    {
        return TargetDecision::Block("password field");
    }

    if let Some(class) = x11_class {
        if is_desktop_shell_class(class) {
            return TargetDecision::CopyOnly("desktop shell target");
        }
        if is_file_manager_class(class) {
            return editable_atspi_decision(
                recording_atspi,
                current_atspi,
                "file manager target is not editable",
                "file manager target could not be verified editable",
            );
        }
        if is_terminal_class(class) {
            return TargetDecision::Allow;
        }
        return editable_atspi_decision(
            recording_atspi,
            current_atspi,
            "target is not editable",
            "target accessibility state unavailable",
        );
    }

    TargetDecision::Allow
}

#[cfg(target_os = "linux")]
fn editable_atspi_decision(
    recording_atspi: Option<&AtspiFocus>,
    current_atspi: Option<&AtspiFocus>,
    not_editable_reason: &'static str,
    missing_reason: &'static str,
) -> TargetDecision {
    let Some(current) = current_atspi else {
        return TargetDecision::CopyOnly(missing_reason);
    };
    if !current.editable {
        return TargetDecision::CopyOnly(not_editable_reason);
    }
    if !same_atspi_object(recording_atspi, current) {
        return TargetDecision::CopyOnly("focused accessible changed");
    }
    TargetDecision::Allow
}

#[cfg(target_os = "linux")]
fn same_atspi_object(recording_atspi: Option<&AtspiFocus>, current: &AtspiFocus) -> bool {
    let Some(recording_id) = recording_atspi.and_then(|focus| focus.object.as_ref()) else {
        return false;
    };
    current
        .object
        .as_ref()
        .is_some_and(|current_id| current_id == recording_id)
}

#[cfg(target_os = "linux")]
#[derive(Clone, Debug, Eq, PartialEq)]
struct AtspiFocus {
    object: Option<AtspiObjectId>,
    password: bool,
    editable: bool,
}

#[cfg(target_os = "linux")]
#[derive(Clone, Debug, Eq, PartialEq)]
struct AtspiObjectId {
    name: String,
    path: String,
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
            let object_id = object.name_as_str().map(|name| AtspiObjectId {
                name: name.to_owned(),
                path: object.path_as_str().to_owned(),
            });
            let accessible = object
                .as_accessible_proxy(connection.connection())
                .await
                .ok()?;
            let role = accessible.get_role().await.ok()?;
            let states = accessible.get_state().await.ok()?;
            return Some(AtspiFocus {
                object: object_id,
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
                Some(&atspi_focus(":1.10", "/field", true, false)),
                Some(&atspi_focus(":1.10", "/field", true, false)),
            ),
            TargetDecision::CopyOnly("desktop shell target")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn non_terminal_targets_need_accessibility_state() {
        assert_eq!(
            target_decision(Some("firefox"), None, None),
            TargetDecision::CopyOnly("target accessibility state unavailable")
        );
        assert_eq!(
            target_decision(
                Some("firefox"),
                Some(&atspi_focus(":1.10", "/field", true, false)),
                Some(&atspi_focus(":1.10", "/field", false, false)),
            ),
            TargetDecision::CopyOnly("target is not editable")
        );
        assert_eq!(
            target_decision(
                Some("firefox"),
                Some(&atspi_focus(":1.10", "/field", true, false)),
                Some(&atspi_focus(":1.10", "/field", true, false)),
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
                Some(&atspi_focus(":1.10", "/field", true, true)),
                Some(&atspi_focus(":1.10", "/field", true, false)),
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
                &TargetSnapshot {
                    x11_class: None,
                    atspi: Some(atspi_focus(":1.10", "/field", true, false)),
                },
                Some(atspi_focus(":1.10", "/field", true, false)),
            ),
            TargetDecision::CopyOnly("target window class unavailable")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn password_blocks_even_without_recording_target_class() {
        assert_eq!(
            recording_target_decision(
                &TargetSnapshot {
                    x11_class: None,
                    atspi: Some(atspi_focus(":1.10", "/field", true, true)),
                },
                Some(atspi_focus(":1.10", "/field", true, false)),
            ),
            TargetDecision::Block("password field")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn non_terminal_targets_require_same_accessible_object() {
        assert_eq!(
            target_decision(
                Some("firefox"),
                Some(&atspi_focus(":1.10", "/field-a", true, false)),
                Some(&atspi_focus(":1.10", "/field-b", true, false)),
            ),
            TargetDecision::CopyOnly("focused accessible changed")
        );
        assert_eq!(
            target_decision(
                Some("firefox"),
                None,
                Some(&atspi_focus(":1.10", "/field-a", true, false)),
            ),
            TargetDecision::CopyOnly("focused accessible changed")
        );
    }

    #[cfg(target_os = "linux")]
    fn atspi_focus(name: &str, path: &str, editable: bool, password: bool) -> AtspiFocus {
        AtspiFocus {
            object: Some(atspi_object(name, path)),
            password,
            editable,
        }
    }

    #[cfg(target_os = "linux")]
    fn atspi_object(name: &str, path: &str) -> AtspiObjectId {
        AtspiObjectId {
            name: name.to_string(),
            path: path.to_string(),
        }
    }
}
