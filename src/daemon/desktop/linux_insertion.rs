//! Linux insertion selection shared by recording, IPC, and startup checks.

use super::ibus::{IbusClient, NativeTarget};
use super::inject::{FocusSnapshot, PasteMode};
use anyhow::{ensure, Result};

/// User-selected Linux insertion transport, independent of content policy.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    PartialEq,
    clap::ValueEnum,
    serde::Deserialize,
    serde::Serialize,
)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum InsertionBackend {
    /// Prefer the active input method, retaining X11 when it is unavailable.
    #[default]
    Auto,
    /// Require Parakit's active IBus input method.
    Ibus,
    /// Use the existing X11 clipboard or direct-typing transport.
    X11,
}

impl InsertionBackend {
    /// Return the stable CLI and diagnostic label.
    ///
    /// # Returns
    /// The lowercase transport name.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Ibus => "ibus",
            Self::X11 => "x11",
        }
    }
}

/// Capture insertion targets using the resolved transport and live IBus client.
#[derive(Clone)]
pub(crate) struct FocusSource {
    pub(crate) backend: InsertionBackend,
    pub(crate) mode: PasteMode,
    pub(crate) client: Option<IbusClient>,
}

impl Default for FocusSource {
    fn default() -> Self {
        Self {
            backend: InsertionBackend::Auto,
            mode: PasteMode::Terminal,
            client: None,
        }
    }
}

impl FocusSource {
    /// Validate transport compatibility without requiring an already focused field.
    ///
    /// # Returns
    /// Success when the selected transport can start in this session.
    ///
    /// # Errors
    /// Reports incompatible direct/IBus modes, missing IBus build support, or
    /// unavailable X11 facilities for a transport that requires them.
    pub(crate) fn preflight(&self) -> Result<()> {
        ensure!(
            self.mode != PasteMode::Direct || self.backend != InsertionBackend::Ibus,
            "direct paste mode conflicts with the IBus insertion backend"
        );
        if self.mode == PasteMode::Direct || self.backend == InsertionBackend::X11 {
            return super::inject::preflight(self.mode);
        }
        if self.backend == InsertionBackend::Ibus || super::session::is_wayland() {
            ensure!(
                cfg!(feature = "ibus"),
                "this build has no IBus support; rebuild with the ibus feature"
            );
            let client = self
                .client
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("IBus connection could not be initialized"))?;
            ensure!(
                client.status().connected,
                "IBus is not connected; run parakit ibus status"
            );
            ensure!(super::ibus::probe_status().engine_listed, "Parakit's IBus component is not registered; run parakit ibus install and follow the input-source setup instructions");
            return Ok(());
        }
        // Auto on a real X11 session can operate before the input source is selected.
        super::session::ensure_x11_session_supported()
    }

    /// Capture a native target when available, otherwise the supported X11 target.
    ///
    /// # Returns
    /// A recording target bound to one insertion transport.
    ///
    /// # Errors
    /// Reports X11 focus-query failures when the chosen route is X11.
    pub(crate) fn capture(&self) -> Result<FocusSnapshot> {
        let native = self.client.as_ref().and_then(IbusClient::snapshot);
        if self.mode != PasteMode::Direct
            && self.backend != InsertionBackend::X11
            && (native.is_some()
                || self.backend == InsertionBackend::Ibus
                || super::session::is_wayland())
        {
            return Ok(FocusSnapshot::from_native(self.client.clone(), native));
        }
        FocusSnapshot::capture()
    }

    /// Keep the token captured by the engine's actual PTT press callback.
    ///
    /// # Returns
    /// The original native target, or the explicitly selected X11 target.
    ///
    /// # Errors
    /// Reports an X11 focus query failure when the user explicitly chose X11.
    pub(crate) fn capture_native(&self, target: NativeTarget) -> Result<FocusSnapshot> {
        if self.backend == InsertionBackend::X11 || self.mode == PasteMode::Direct {
            self.capture()
        } else {
            Ok(FocusSnapshot::from_native(
                self.client.clone(),
                Some(target),
            ))
        }
    }
}
