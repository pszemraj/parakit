//! Desktop hotkey, focus, session, and insertion helpers.

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("parakit desktop mode supports Linux, macOS, and Windows only");

/// Build modifier-down, paste-key down/up, and reverse modifier-up steps.
#[cfg(any(target_os = "linux", target_os = "windows", test))]
fn paste_chord_steps<K: Copy>(control: K, shift: Option<K>, paste: K) -> Vec<(K, bool)> {
    let mut keys = vec![control];
    keys.extend(shift);
    keys.push(paste);
    let mut steps: Vec<_> = keys.iter().copied().map(|key| (key, true)).collect();
    steps.extend(keys.into_iter().rev().map(|key| (key, false)));
    steps
}

/// Return accepted downs without matching ups, in reverse press order.
#[cfg(any(target_os = "linux", target_os = "windows", test))]
fn unreleased_keys<K: Copy + Eq>(steps: impl IntoIterator<Item = (K, bool)>) -> Vec<K> {
    let mut pressed = Vec::new();
    for (key, press) in steps {
        if press {
            pressed.push(key);
        } else if let Some(index) = pressed.iter().rposition(|pressed| *pressed == key) {
            pressed.remove(index);
        }
    }
    pressed.reverse();
    pressed
}

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
/// Native Linux input-method engine and its daemon connection.
pub(crate) mod ibus;

#[cfg(target_os = "linux")]
/// Linux transport selection and recording-target capture.
pub(crate) mod linux_insertion;

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
/// Windows security and process-context helpers.
pub(crate) mod windows_security;

#[cfg(test)]
mod tests {
    use super::{paste_chord_steps, unreleased_keys};

    #[test]
    fn paste_chord_cleanup_matches_every_accepted_prefix() {
        let standard = [(1, true), (3, true), (3, false), (1, false)];
        let terminal = [
            (1, true),
            (2, true),
            (3, true),
            (3, false),
            (2, false),
            (1, false),
        ];
        let standard_cleanup: &[&[u8]] = &[&[], &[1], &[3, 1], &[1], &[]];
        let terminal_cleanup: &[&[u8]] = &[&[], &[1], &[2, 1], &[3, 2, 1], &[2, 1], &[1], &[]];
        for (shift, expected_steps, expected_cleanup) in [
            (None, standard.as_slice(), standard_cleanup),
            (Some(2), terminal.as_slice(), terminal_cleanup),
        ] {
            let steps = paste_chord_steps(1, shift, 3);
            assert_eq!(steps, expected_steps);
            for (accepted, expected) in expected_cleanup.iter().enumerate() {
                assert_eq!(
                    unreleased_keys(steps.iter().copied().take(accepted)),
                    *expected,
                    "shift {shift:?}, accepted {accepted}"
                );
            }
        }
    }

    #[test]
    fn cleanup_preserves_repeated_downs_and_ignores_unmatched_ups() {
        assert_eq!(
            unreleased_keys([(9, false), (1, true), (1, true), (2, true), (1, false)]),
            [2, 1]
        );
    }
}
