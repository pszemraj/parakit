//! Paste-target safety inspection.

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

/// Inspect the currently focused target before staging transcript text.
///
/// # Returns
///
/// A conservative insertion decision.
pub(crate) fn inspect_current_target() -> TargetDecision {
    inspect_current_target_impl()
}

#[cfg(target_os = "linux")]
fn inspect_current_target_impl() -> TargetDecision {
    let x11_class = current_x11_wm_class().ok().flatten();
    let atspi = current_atspi_focus();
    target_decision(x11_class.as_deref(), atspi)
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
    }

    TargetDecision::Allow
}

#[cfg(not(target_os = "linux"))]
fn inspect_current_target_impl() -> TargetDecision {
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

#[cfg(target_os = "linux")]
fn current_x11_wm_class() -> anyhow::Result<Option<String>> {
    use anyhow::Context as _;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};
    use x11rb::rust_connection::RustConnection;

    let (conn, screen_num) = RustConnection::connect(None).context("could not connect to X11")?;
    let root = super::x11::root_window(&conn, screen_num)?;
    let mut window = conn
        .get_input_focus()
        .context("could not request X11 input focus")?
        .reply()
        .context("could not read X11 input focus")?
        .focus;
    if window == x11rb::NONE
        || window == u32::from(x11rb::protocol::xproto::InputFocus::POINTER_ROOT)
    {
        return Ok(None);
    }

    let wm_class = conn
        .intern_atom(false, b"WM_CLASS")
        .context("could not request X11 WM_CLASS atom")?
        .reply()
        .context("could not read X11 WM_CLASS atom")?
        .atom;

    for _ in 0..32 {
        let reply = conn
            .get_property(false, window, wm_class, AtomEnum::STRING, 0, 1024)
            .context("could not request X11 WM_CLASS")?
            .reply()
            .context("could not read X11 WM_CLASS")?;
        if let Some(class) = parse_wm_class(&reply.value) {
            return Ok(Some(class));
        }
        if window == root {
            break;
        }
        let tree = conn
            .query_tree(window)
            .context("could not request X11 window tree")?
            .reply()
            .context("could not read X11 window tree")?;
        if tree.parent == x11rb::NONE || tree.parent == window {
            break;
        }
        window = tree.parent;
    }

    Ok(None)
}

#[cfg(target_os = "linux")]
fn parse_wm_class(value: &[u8]) -> Option<String> {
    let mut parts = value
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .filter_map(|part| std::str::from_utf8(part).ok());
    parts.next_back().map(str::to_string)
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
            parse_wm_class(b"org.gnome.Nautilus\0org.gnome.Nautilus\0"),
            Some("org.gnome.Nautilus".to_string())
        );
    }
}
