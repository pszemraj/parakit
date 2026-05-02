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
    if linux_session_type_is_wayland(session_type.as_deref()) {
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| "<unset>".to_string());
        let wayland_display =
            std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "<unset>".to_string());
        bail!(
            "Linux desktop automation is unsupported on Wayland sessions. Use an X11 session. XDG_SESSION_TYPE={}, DISPLAY={display}, WAYLAND_DISPLAY={wayland_display}",
            session_type.as_deref().unwrap_or("<unset>")
        );
    }

    if std::env::var_os("DISPLAY").is_none() {
        bail!(
            "Linux desktop automation requires an X11 DISPLAY. Start parakit from the active X11 desktop session."
        );
    }

    Ok(())
}

/// Ensure the current Linux desktop session can receive synthetic insertion.
///
/// # Returns
///
/// `Ok(())` when text insertion should be allowed to initialize.
///
/// # Errors
///
/// Returns an error for Wayland sessions because the Linux insertion backend
/// uses X11/XTest and cannot insert into focused native Wayland applications.
pub(crate) fn ensure_text_insertion_supported() -> Result<()> {
    ensure_x11_session_supported()
}

fn linux_session_type_is_wayland(session_type: Option<&str>) -> bool {
    session_type.is_some_and(|value| value.eq_ignore_ascii_case("wayland"))
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
}
