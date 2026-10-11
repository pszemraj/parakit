//! Focus owner captured when recording begins, and its drift checks before insertion.

#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::Result;
#[cfg(target_os = "linux")]
use x11rb::protocol::xproto::ConnectionExt as _;
#[cfg(target_os = "linux")]
use x11rb::rust_connection::RustConnection;

#[cfg(target_os = "macos")]
use crate::daemon::desktop::clipboard_restore::PasteTargetValue;
#[cfg(target_os = "windows")]
use crate::daemon::desktop::windows_focus::WindowsFocusSnapshot;
#[cfg(target_os = "linux")]
use crate::daemon::desktop::x11;
use crate::daemon::desktop::FocusVerification;

/// Focus owner captured when recording begins.
pub(crate) struct FocusSnapshot {
    #[cfg(target_os = "linux")]
    input_focus: Option<u32>,
    #[cfg(target_os = "linux")]
    active_window: Option<u32>,
    #[cfg(target_os = "linux")]
    linux_connection: RustConnection,
    #[cfg(target_os = "linux")]
    linux_root: u32,
    #[cfg(target_os = "windows")]
    windows: WindowsFocusSnapshot,
    #[cfg(target_os = "macos")]
    macos: crate::daemon::macos::MacOsFocusSnapshot,
}

impl FocusSnapshot {
    /// Capture the current focus owner for later drift checks.
    ///
    /// # Returns
    ///
    /// A focus snapshot that can be compared before insertion.
    ///
    /// # Errors
    ///
    /// Returns an error when the platform focus cannot be read or has no
    /// concrete target window.
    pub(crate) fn capture() -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            let (conn, screen_num) = RustConnection::connect(None)
                .context("could not connect to X11 while capturing recording focus")?;
            let root = x11::root_window(&conn, screen_num)
                .context("could not read X11 root window for focus snapshot")?;
            let focus = linux_current_input_focus(&conn)?;
            let input_focus = linux_focus_is_insertable(focus).then_some(focus);
            let active_window = x11::active_window(&conn, root)
                .context("could not read X11 active window for focus snapshot")?;
            if input_focus.is_none() && active_window.is_none() {
                anyhow::bail!("X11 focus is not an insertable application window");
            }
            Ok(Self {
                input_focus,
                active_window,
                linux_connection: conn,
                linux_root: root,
            })
        }

        #[cfg(target_os = "windows")]
        {
            Ok(Self {
                windows: WindowsFocusSnapshot::capture()?,
            })
        }

        #[cfg(target_os = "macos")]
        {
            Ok(Self {
                macos: crate::daemon::macos::MacOsFocusSnapshot::capture()?,
            })
        }
    }

    /// Compare the current focus against this snapshot with one platform read.
    ///
    /// # Returns
    ///
    /// A verification value carrying both the telemetry label and insertion
    /// decision for the same live focus observation.
    ///
    /// # Errors
    ///
    /// Returns an error when the current focus cannot be read.
    pub(crate) fn verify_current(&self) -> Result<FocusVerification> {
        #[cfg(target_os = "linux")]
        {
            if let Some(expected) = self.active_window {
                if let Some(current) = x11::active_window(&self.linux_connection, self.linux_root)
                    .context("could not query the current X11 active window")?
                {
                    return Ok(FocusVerification::from_matches(current == expected));
                }
            }

            let Some(expected) = self.input_focus else {
                anyhow::bail!(
                    "X11 active window is unavailable and no input focus fallback exists"
                );
            };
            Ok(FocusVerification::from_matches(
                linux_current_input_focus(&self.linux_connection)
                    .context("could not query the current X11 focus")?
                    == expected,
            ))
        }

        #[cfg(target_os = "windows")]
        {
            self.windows
                .matches_current()
                .map(FocusVerification::from_matches)
        }

        #[cfg(target_os = "macos")]
        {
            Ok(self.macos.verify_current())
        }
    }

    /// Return the bundle identifier of the captured insertion target, when
    /// the platform focus snapshot carries one.
    ///
    /// # Returns
    ///
    /// `Some` bundle identifier on macOS when the frontmost application
    /// reported one; `None` on platforms whose focus snapshot has no
    /// application bundle identifier concept.
    #[cfg(target_os = "macos")]
    pub(crate) fn target_bundle_id(&self) -> Option<&str> {
        self.macos.bundle_id()
    }

    /// Return the bundle identifier of the captured insertion target, when
    /// the platform focus snapshot carries one.
    ///
    /// # Returns
    ///
    /// Always `None`: Linux and Windows focus snapshots do not carry an
    /// application bundle identifier today.
    #[cfg(not(target_os = "macos"))]
    pub(crate) fn target_bundle_id(&self) -> Option<&str> {
        None
    }

    /// Return the focused Accessibility element captured for this snapshot,
    /// when one is available for post-paste `AXValue` acknowledgement
    /// polling.
    ///
    /// # Returns
    ///
    /// `Some` when this snapshot captured a focused Accessibility element
    /// (see [`crate::daemon::macos::MacOsFocusSnapshot::ax_element`] for the
    /// cases where it did not, e.g. capture-time Accessibility failure or an
    /// application that exposes no focused element).
    #[cfg(target_os = "macos")]
    pub(crate) fn macos_ax_element(&self) -> Option<&crate::daemon::macos::AxElementSnapshot> {
        self.macos.ax_element()
    }

    /// Reacquire and read the current focused macOS Accessibility value.
    ///
    /// # Arguments
    ///
    /// * `max_utf16_units` - Maximum number of UTF-16 code units copied from
    ///   the complete value.
    ///
    /// # Returns
    ///
    /// The current bounded value and whether the live Accessibility object
    /// is identical to the originally captured one.
    ///
    /// # Errors
    ///
    /// Returns `Err(())` when the frontmost application changed or its focus
    /// state cannot be read.
    #[cfg(target_os = "macos")]
    pub(crate) fn macos_poll_current_value(
        &self,
        max_utf16_units: usize,
    ) -> Result<Option<(PasteTargetValue, bool)>, ()> {
        self.macos.poll_current_value(max_utf16_units)
    }

    /// Return the pid of the frontmost application this snapshot captured.
    ///
    /// # Returns
    ///
    /// The process id macOS reported as frontmost at capture time.
    #[cfg(target_os = "macos")]
    pub(crate) fn macos_pid(&self) -> libc::pid_t {
        self.macos.pid()
    }
}

#[cfg(target_os = "linux")]
fn linux_focus_is_insertable(focus: u32) -> bool {
    focus != x11rb::NONE && focus != u32::from(x11rb::protocol::xproto::InputFocus::POINTER_ROOT)
}

/// Read the window that currently holds X11 input focus.
///
/// # Arguments
///
/// * `conn` - Open X11 connection to query.
///
/// # Returns
///
/// The focused window id, which may be `NONE` or `POINTER_ROOT`.
///
/// # Errors
///
/// Returns an error when the focus request or its reply fails.
#[cfg(target_os = "linux")]
pub(super) fn linux_current_input_focus(conn: &RustConnection) -> Result<u32> {
    Ok(conn
        .get_input_focus()
        .context("could not request current X11 input focus")?
        .reply()
        .context("could not read current X11 input focus")?
        .focus)
}
