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

#[cfg(target_os = "linux")]
pub(crate) use super::super::linux_insertion::FocusSource;

/// Platform focus provider for the existing non-Linux insertion path.
#[cfg(not(target_os = "linux"))]
#[derive(Clone, Default)]
pub(crate) struct FocusSource {}

#[cfg(not(target_os = "linux"))]
impl FocusSource {
    /// Capture the platform's current insertion target.
    ///
    /// # Returns
    /// The current platform focus snapshot.
    ///
    /// # Errors
    /// Reports a platform focus query failure.
    pub(crate) fn capture(&self) -> Result<FocusSnapshot> {
        FocusSnapshot::capture()
    }
}

#[cfg(target_os = "linux")]
enum LinuxFocus {
    X11 {
        input_focus: Option<u32>,
        active_window: Option<u32>,
        connection: Box<RustConnection>,
        root: u32,
        active_window_atom: u32,
    },
    Native {
        client: Option<super::super::ibus::IbusClient>,
        target: Option<super::super::ibus::NativeTarget>,
    },
}

/// Focus owner captured when recording begins.
pub(crate) struct FocusSnapshot {
    #[cfg(target_os = "linux")]
    linux: LinuxFocus,
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
            let active_window_atom = x11::active_window_atom(&conn)?;
            let active_window = x11::active_window(&conn, root, active_window_atom)
                .context("could not read X11 active window for focus snapshot")?;
            if input_focus.is_none() && active_window.is_none() {
                anyhow::bail!("X11 focus is not an insertable application window");
            }
            Ok(Self {
                linux: LinuxFocus::X11 {
                    input_focus,
                    active_window,
                    connection: Box::new(conn),
                    root,
                    active_window_atom,
                },
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
            let (input_focus, active_window, connection, root, active_window_atom) =
                match &self.linux {
                    LinuxFocus::X11 {
                        input_focus,
                        active_window,
                        connection,
                        root,
                        active_window_atom,
                    } => (
                        input_focus,
                        active_window,
                        connection,
                        root,
                        active_window_atom,
                    ),
                    LinuxFocus::Native { client, target } => {
                        return Ok(FocusVerification::from_matches(
                            target.is_some()
                                && client.as_ref().and_then(|client| client.snapshot()) == *target,
                        ))
                    }
                };
            if let Some(expected) = *active_window {
                if let Some(current) = x11::active_window(connection, *root, *active_window_atom)
                    .context("could not query the current X11 active window")?
                {
                    return Ok(FocusVerification::from_matches(current == expected));
                }
            }

            let Some(expected) = *input_focus else {
                anyhow::bail!(
                    "X11 active window is unavailable and no input focus fallback exists"
                );
            };
            Ok(FocusVerification::from_matches(
                linux_current_input_focus(connection)
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

    /// Capture a native token without requiring an X11 connection.
    ///
    /// # Arguments
    /// * `client` - Connection that owns this recording's native route, when available.
    /// * `target` - Original target token; absence prevents native delivery.
    ///
    /// # Returns
    /// A native snapshot that cannot fall back to X11 insertion.
    #[cfg(target_os = "linux")]
    pub(crate) fn from_native(
        client: Option<super::super::ibus::IbusClient>,
        target: Option<super::super::ibus::NativeTarget>,
    ) -> Self {
        Self {
            linux: LinuxFocus::Native { client, target },
        }
    }

    /// Whether this capture is bound to native input-method delivery.
    ///
    /// # Returns
    /// True for a native capture, including one with an unavailable target.
    #[cfg(target_os = "linux")]
    pub(crate) fn is_native(&self) -> bool {
        matches!(self.linux, LinuxFocus::Native { .. })
    }

    /// Whether known native content metadata permits starting a dictation.
    ///
    /// # Returns
    /// False when the captured field is known to be private.
    #[cfg(target_os = "linux")]
    pub(crate) fn permits_dictation(&self) -> bool {
        !matches!(&self.linux, LinuxFocus::Native { target: Some(target), .. } if !target.eligible())
    }

    /// Submit only through the native owner captured for this recording.
    ///
    /// # Returns
    /// Definite rejection, submission evidence, or uncertain delivery.
    #[cfg(target_os = "linux")]
    pub(crate) fn commit_native(&self, text: &str) -> super::super::ibus::CommitOutcome {
        use super::super::ibus::CommitOutcome;
        match &self.linux {
            LinuxFocus::Native {
                client: Some(client),
                target: Some(target),
            } => client.commit(*target, text),
            _ => CommitOutcome::Rejected("no eligible IBus target was captured".to_string()),
        }
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
