//! Linux insertion selection shared by recording, IPC, and startup checks.

use super::hotkey::LinuxHotkeyRoute;
use super::ibus::IbusClient;
#[cfg(any(feature = "ibus", test))]
use super::ibus::NativeTarget;
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
    /// Use native insertion with IBus PTT, and X11 with other hotkey backends.
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
    pub(crate) hotkey_route: LinuxHotkeyRoute,
    pub(crate) client: Option<IbusClient>,
}

impl Default for FocusSource {
    fn default() -> Self {
        Self {
            backend: InsertionBackend::Auto,
            mode: PasteMode::Terminal,
            hotkey_route: LinuxHotkeyRoute::RegisteredX11,
            client: None,
        }
    }
}

impl FocusSource {
    /// Select native insertion only when the input method owns push-to-talk.
    ///
    /// A focused native target never changes the transport selected at startup.
    ///
    /// # Returns
    /// True when the configured insertion path belongs to the IBus engine.
    pub(crate) fn uses_native(&self) -> bool {
        self.mode != PasteMode::Direct
            && self.backend != InsertionBackend::X11
            && self.hotkey_route == LinuxHotkeyRoute::Ibus
    }

    /// Reject insertion options that cannot use the selected hotkey transport.
    ///
    /// # Returns
    /// Success when the hotkey, paste mode, and insertion preference agree.
    ///
    /// # Errors
    /// Reports direct/IBus conflicts and native insertion without IBus PTT.
    pub(crate) fn validate_options(&self) -> Result<()> {
        ensure!(
            self.mode != PasteMode::Direct || self.backend != InsertionBackend::Ibus,
            "direct paste mode conflicts with the IBus insertion backend"
        );
        ensure!(
            self.backend != InsertionBackend::Ibus || self.hotkey_route == LinuxHotkeyRoute::Ibus,
            "IBus insertion requires the IBus hotkey backend; use --hotkey-backend ibus"
        );
        Ok(())
    }

    /// Validate transport compatibility without requiring an already focused field.
    ///
    /// # Returns
    /// Success when the selected transport can start in this session.
    ///
    /// # Errors
    /// Reports incompatible direct/IBus modes, missing IBus build support, or
    /// unavailable X11 facilities for a transport that requires them.
    pub(crate) fn preflight(&self) -> Result<()> {
        self.validate_options()?;
        if !self.uses_native() {
            return super::inject::preflight(self.mode);
        }
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
        Ok(())
    }

    /// Capture a target through the insertion transport selected at startup.
    ///
    /// # Returns
    /// A recording target bound to one insertion transport.
    ///
    /// # Errors
    /// Reports X11 focus-query failures when the chosen route is X11.
    pub(crate) fn capture(&self) -> Result<FocusSnapshot> {
        if self.uses_native() {
            let native = self.client.as_ref().and_then(IbusClient::snapshot);
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
    #[cfg(any(feature = "ibus", test))]
    pub(crate) fn capture_native(&self, target: NativeTarget) -> Result<FocusSnapshot> {
        if !self.uses_native() {
            self.capture()
        } else {
            Ok(FocusSnapshot::from_native(
                self.client.clone(),
                Some(target),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insertion_transport_is_selected_by_hotkey_ownership() {
        use super::super::hotkey::HotkeyBackend;
        for (name, hotkey, wayland, backend, mode, native, valid) in [
            (
                "X11 auto keeps the registered shortcut transport",
                HotkeyBackend::Auto,
                false,
                InsertionBackend::Auto,
                PasteMode::Terminal,
                false,
                true,
            ),
            (
                "Wayland auto uses engine-owned PTT",
                HotkeyBackend::Auto,
                true,
                InsertionBackend::Auto,
                PasteMode::Terminal,
                true,
                true,
            ),
            (
                "explicit IBus PTT enables native insertion on X11",
                HotkeyBackend::Ibus,
                false,
                InsertionBackend::Auto,
                PasteMode::Standard,
                true,
                true,
            ),
            (
                "passive X11 listening keeps X11 insertion",
                HotkeyBackend::X11Listen,
                false,
                InsertionBackend::Auto,
                PasteMode::Standard,
                false,
                true,
            ),
            (
                "evdev PTT keeps X11 insertion",
                HotkeyBackend::EvdevProxyExperimental,
                false,
                InsertionBackend::Auto,
                PasteMode::Standard,
                false,
                true,
            ),
            (
                "explicit X11 insertion remains available with engine-owned PTT",
                HotkeyBackend::Ibus,
                false,
                InsertionBackend::X11,
                PasteMode::Standard,
                false,
                true,
            ),
            (
                "direct typing uses the X11 transport",
                HotkeyBackend::Ibus,
                false,
                InsertionBackend::Auto,
                PasteMode::Direct,
                false,
                true,
            ),
            (
                "direct typing conflicts with required native insertion",
                HotkeyBackend::Ibus,
                false,
                InsertionBackend::Ibus,
                PasteMode::Direct,
                false,
                false,
            ),
            (
                "required IBus insertion rejects the default X11 grab",
                HotkeyBackend::Auto,
                false,
                InsertionBackend::Ibus,
                PasteMode::Standard,
                false,
                false,
            ),
            (
                "an explicit X11 grab cannot carry native insertion on Wayland",
                HotkeyBackend::X11GlobalHotkey,
                true,
                InsertionBackend::Ibus,
                PasteMode::Standard,
                false,
                false,
            ),
        ] {
            let source = FocusSource {
                backend,
                mode,
                hotkey_route: hotkey.linux_route_for_session(wayland),
                client: None,
            };
            assert_eq!(source.uses_native(), native, "{name}");
            assert_eq!(source.validate_options().is_ok(), valid, "{name}");
        }
    }

    #[test]
    fn unavailable_engine_target_stays_native_until_clipboard_recovery() {
        let source = FocusSource {
            hotkey_route: LinuxHotkeyRoute::Ibus,
            ..FocusSource::default()
        };
        let snapshot = source.capture().unwrap();
        assert!(snapshot.is_native());
        assert!(matches!(
            snapshot.commit_native("text"),
            super::super::ibus::CommitOutcome::Rejected(_)
        ));
    }

    #[test]
    fn x11_preflight_opens_the_selected_insertion_transport() {
        const CHILD: &str = "PARAKIT_X11_PREFLIGHT_CHILD";
        if std::env::var_os(CHILD).is_some() {
            FocusSource::default()
                .preflight()
                .expect_err("an unavailable X11 server must fail insertion preflight");
            let target = NativeTarget {
                engine_id: 1,
                ..NativeTarget::default()
            };
            assert!(target.eligible());
            assert!(
                FocusSource::default().capture_native(target).is_err(),
                "a native token must not switch the default X11 transport"
            );
            return;
        }
        let output = crate::test_support::exact_test_command("daemon::desktop::linux_insertion::tests::x11_preflight_opens_the_selected_insertion_transport")
            .env(CHILD, "1")
            .env_remove("DISPLAY")
            .env("XDG_SESSION_TYPE", "x11")
            .env_remove("WAYLAND_DISPLAY")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
}
