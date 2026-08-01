//! Desktop hotkey, focus, session, and insertion helpers.

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("parakit desktop mode supports Linux, macOS, and Windows only");

/// One live comparison between a captured focus owner and the current target.
///
/// The label and insertion decision deliberately live on the same value so
/// telemetry cannot describe a different platform read from the one that
/// authorized (or blocked) insertion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FocusVerification {
    /// The current insertion target matches the recording target.
    Matched,
    /// The current insertion target differs from the recording target.
    Changed,
    /// The application identity matched, but macOS Accessibility could not
    /// expose focused-element identity on one or both reads.
    #[cfg(target_os = "macos")]
    AxUnsupported,
}

impl FocusVerification {
    /// Return the stable telemetry label for this comparison.
    ///
    /// # Returns
    ///
    /// `"matched"`, `"changed"`, or `"ax_unsupported"` on macOS.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Matched => "matched",
            Self::Changed => "changed",
            #[cfg(target_os = "macos")]
            Self::AxUnsupported => "ax_unsupported",
        }
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    const fn from_matches(matches: bool) -> Self {
        if matches {
            Self::Matched
        } else {
            Self::Changed
        }
    }
}

/// Clipboard restore timing and paste-acknowledgement policy. `pub(crate)`
/// so the macOS Accessibility acknowledgement override in
/// `daemon::macos::pasteboard` (a sibling of `daemon::desktop`) can name
/// [`clipboard_restore::PasteConfirmation`] and
/// [`clipboard_restore::PasteConfirmationContext`].
pub(crate) mod clipboard_restore;
/// Global push-to-talk hotkey registration and event handling.
pub(crate) mod hotkey;
/// Text insertion into the currently focused desktop target.
pub(crate) mod inject;

#[cfg(target_os = "linux")]
/// Linux desktop session detection helpers.
pub(crate) mod session;
#[cfg(target_os = "linux")]
/// Windows Subsystem for Linux environment detection.
pub(crate) mod wsl;
#[cfg(target_os = "linux")]
/// X11 focus and hotkey integration.
pub(crate) mod x11;

#[cfg(target_os = "windows")]
/// Windows clipboard-history listener helpers.
pub(crate) mod windows_clipboard_history;
#[cfg(target_os = "windows")]
/// Windows foreground-window detection helpers.
pub(crate) mod windows_focus;
#[cfg(target_os = "windows")]
/// Windows keyboard and clipboard insertion helpers.
pub(crate) mod windows_input;
#[cfg(target_os = "windows")]
/// Windows paste-path smoke checks.
pub(crate) mod windows_paste_smoke;
#[cfg(target_os = "windows")]
/// Windows security and process-context helpers.
pub(crate) mod windows_security;
