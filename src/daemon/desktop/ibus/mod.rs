//! IBus-owned input method and the daemon's native insertion connection.

use crate::daemon::recording::HotkeyTransition;
use crossbeam_channel::Sender;

#[cfg(feature = "ibus")]
mod native;
#[cfg(feature = "ibus")]
pub(crate) use native::{probe_status, IbusClient};

/// Input-method target bound to one helper, attachment, engine and focus lifetime.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct NativeTarget {
    /// Fresh helper process identity.
    pub(crate) helper_epoch: u64,
    /// Daemon attachment identity, invalidated by readiness changes.
    pub(crate) attachment_epoch: u64,
    /// Engine instance within the helper process.
    pub(crate) engine_id: u64,
    /// Input focus and reset generation within the engine.
    pub(crate) focus_generation: u64,
    /// IBus input purpose, including password and PIN purposes.
    pub(crate) purpose: u32,
    /// IBus content hints, including private input.
    pub(crate) hints: u32,
}

impl NativeTarget {
    /// Return whether content metadata permits dictation into this field.
    ///
    /// # Returns
    /// False for absent targets, passwords, PINs, and private fields.
    pub(crate) fn eligible(self) -> bool {
        self.engine_id != 0 && self.purpose != 8 && self.purpose != 9 && self.hints & (1 << 11) == 0
    }
}

/// Native submission evidence; uncertain delivery must never trigger another insertion.
#[derive(Debug, Eq, PartialEq)]
#[cfg_attr(
    not(feature = "ibus"),
    allow(
        dead_code,
        reason = "the opt-out client cannot submit, while callers retain the same delivery outcome contract"
    )
)]
pub(crate) enum CommitOutcome {
    /// No native submission occurred.
    Rejected(String),
    /// The helper emitted CommitText; application reception is unacknowledged.
    Submitted,
    /// The helper may have submitted before its reply was lost.
    Uncertain(String),
}

/// Current daemon connection and helper readiness, without desktop mutations.
#[derive(Clone, Debug)]
pub(crate) struct IbusStatus {
    /// The daemon is connected to IBus's private bus.
    pub(crate) connected: bool,
    /// The installed component currently owns its IBus name.
    pub(crate) helper_connected: bool,
    /// The current native target is ready for dictation.
    pub(crate) ready: bool,
    /// Human-readable connection or target state.
    pub(crate) detail: String,
}

/// Read-only registry and active-source probe used by installation diagnostics.
#[derive(Clone, Debug)]
pub(crate) struct IbusProbe {
    /// This build includes IBus integration.
    pub(crate) compiled: bool,
    /// The current IBus bus is reachable.
    pub(crate) connected: bool,
    /// The registry lists the Parakit input source.
    pub(crate) engine_listed: bool,
    /// Name of the globally selected engine, when reported by IBus.
    pub(crate) active_engine: Option<String>,
}

#[cfg(not(feature = "ibus"))]
/// Feature-disabled client retaining the Linux backend-selection interface.
#[derive(Clone)]
pub(crate) struct IbusClient;

#[cfg(not(feature = "ibus"))]
impl IbusClient {
    /// Return a disabled connection, allowing explicit X11 builds to start.
    ///
    /// # Returns
    /// An inert native client.
    ///
    /// # Errors
    /// This feature-disabled constructor does not fail.
    pub(crate) fn start() -> anyhow::Result<Self> {
        Ok(Self)
    }

    /// Return no native target when IBus was omitted from the build.
    ///
    /// # Returns
    /// Always `None`.
    pub(crate) fn snapshot(&self) -> Option<NativeTarget> {
        None
    }

    /// Ignore native readiness for a feature-disabled build.
    pub(crate) fn set_ready(&self, _ready: bool) {}

    /// Ignore the native PTT channel for a feature-disabled build.
    pub(crate) fn set_ptt_sender(&self, _sender: Option<Sender<HotkeyTransition>>) {}

    /// Reject before submission when the native backend is not compiled.
    ///
    /// # Arguments
    /// * `_target` - Unused native target.
    /// * `_text` - Unused insertion text.
    ///
    /// # Returns
    /// A definite rejection without desktop interaction.
    pub(crate) fn commit(&self, _target: NativeTarget, _text: &str) -> CommitOutcome {
        CommitOutcome::Rejected("IBus support is disabled in this build".into())
    }

    /// Report the compile-time opt-out without opening desktop resources.
    ///
    /// # Returns
    /// Disabled native status.
    pub(crate) fn status(&self) -> IbusStatus {
        IbusStatus {
            connected: false,
            helper_connected: false,
            ready: false,
            detail: "IBus support is disabled in this build".into(),
        }
    }
}

#[cfg(not(feature = "ibus"))]
/// Report the compile-time opt-out without probing desktop resources.
///
/// # Returns
/// A disabled registration probe.
pub(crate) fn probe_status() -> IbusProbe {
    IbusProbe {
        compiled: false,
        connected: false,
        engine_listed: false,
        active_engine: None,
    }
}

#[cfg(test)]
mod tests {
    use super::NativeTarget;

    #[test]
    fn private_fields_remain_identifiable_but_ineligible() {
        let normal = NativeTarget {
            engine_id: 1,
            ..NativeTarget::default()
        };
        assert!(normal.eligible());
        for purpose in [8, 9] {
            assert!(!NativeTarget { purpose, ..normal }.eligible());
        }
        assert!(!NativeTarget {
            hints: 1 << 11,
            ..normal
        }
        .eligible());
        assert!(!NativeTarget::default().eligible());
    }

    #[test]
    fn native_target_ffi_layout_has_no_padding() {
        assert_eq!(std::mem::size_of::<NativeTarget>(), 40);
        assert_eq!(std::mem::align_of::<NativeTarget>(), 8);
    }
}
