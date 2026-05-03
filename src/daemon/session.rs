//! Desktop session compatibility checks.

use anyhow::{bail, Result};

/// Ensure the current Linux desktop session is an X11 session.
///
/// # Returns
///
/// `Ok(())` when Linux desktop hotkeys and insertion should be allowed to
/// initialize.
///
/// # Errors
///
/// Returns an error for Wayland sessions or when no X11 `DISPLAY` is present.
pub(crate) fn ensure_x11_session_supported() -> Result<()> {
    let session_type = std::env::var("XDG_SESSION_TYPE").ok();
    let wayland_display = std::env::var("WAYLAND_DISPLAY").ok();
    if linux_session_looks_wayland(session_type.as_deref(), wayland_display.as_deref()) {
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| "<unset>".to_string());
        bail!(
            "Linux desktop automation is unsupported on Wayland sessions. Use an X11 session. XDG_SESSION_TYPE={}, DISPLAY={display}, WAYLAND_DISPLAY={wayland_display}",
            session_type.as_deref().unwrap_or("<unset>"),
            wayland_display = wayland_display.as_deref().unwrap_or("<unset>")
        );
    }

    if std::env::var_os("DISPLAY").is_none() {
        bail!(
            "Linux desktop automation requires an X11 DISPLAY. Start parakit from the active X11 desktop session."
        );
    }

    Ok(())
}

fn linux_session_type_is_wayland(session_type: Option<&str>) -> bool {
    session_type.is_some_and(|value| value.eq_ignore_ascii_case("wayland"))
}

fn linux_session_looks_wayland(session_type: Option<&str>, wayland_display: Option<&str>) -> bool {
    if linux_session_type_is_wayland(session_type) {
        return true;
    }
    session_type.is_none() && wayland_display.is_some_and(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wayland_session_is_rejected_case_insensitively() {
        assert!(linux_session_type_is_wayland(Some("wayland")));
        assert!(linux_session_type_is_wayland(Some("Wayland")));
    }

    #[test]
    fn non_wayland_sessions_are_allowed() {
        assert!(!linux_session_type_is_wayland(Some("x11")));
        assert!(!linux_session_type_is_wayland(Some("tty")));
        assert!(!linux_session_type_is_wayland(None));
    }

    #[test]
    fn unset_session_with_wayland_display_is_rejected() {
        assert!(linux_session_looks_wayland(None, Some("wayland-0")));
        assert!(!linux_session_looks_wayland(Some("x11"), Some("wayland-0")));
        assert!(!linux_session_looks_wayland(None, None));
    }
}
