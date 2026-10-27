//! Native Linux input-method readiness and delivery-scope diagnostics.

use super::super::desktop::ibus::{self, IbusProbe};
use super::super::desktop::linux_insertion::{FocusSource, InsertionBackend};
use super::super::inject::PasteMode;
use super::HotkeyReport;
use anyhow::{ensure, Result};

/// Report whether focused-field IBus PTT can wait for source activation.
///
/// # Returns
/// Registration, connection, selected-source, and PTT-scope readiness.
pub(super) fn hotkey_report() -> HotkeyReport {
    report_for_probe(&ibus::probe_status())
}

fn report_for_probe(probe: &IbusProbe) -> HotkeyReport {
    let blocking = !probe.compiled || !probe.connected || !probe.engine_listed;
    let status = if !probe.compiled {
        "IBus support omitted from this build"
    } else if !probe.connected {
        "IBus private bus unavailable"
    } else if !probe.engine_listed {
        "Parakit input source is not registered"
    } else if probe.active_engine.as_deref() == Some("parakit") {
        "focused-field IBus Ctrl+Space ready"
    } else {
        "focused-field IBus Ctrl+Space waiting for Parakit source selection"
    };
    let details = format!(
        "parakit doctor\n  IBus compiled:   {}\n  IBus connection: {}\n  registration:    {}\n  input source:    {}\n  PTT scope:       focused editable fields; global Wayland shortcuts are not implemented\n  shortcut:        Ctrl+Space must not be reserved for input-source switching\n  setup:           parakit ibus status; select Parakit in desktop Input Sources\n",
        probe.compiled,
        probe.connected,
        if probe.engine_listed { "listed" } else { "missing" },
        probe.active_engine.as_deref().unwrap_or("unavailable"),
    );
    HotkeyReport {
        blocking,
        status: status.to_owned(),
        summary: format!("{status}. Run parakit ibus status and follow the Linux input-source setup instructions."),
        details,
    }
}

/// Whether diagnostics should inspect native resources rather than X11 paste.
///
/// # Returns
/// True when the configured transport and current source select native insertion.
pub(super) fn uses_native(source: &FocusSource) -> bool {
    source.mode != PasteMode::Direct
        && source.backend != InsertionBackend::X11
        && (source.backend == InsertionBackend::Ibus
            || super::super::session::is_wayland()
            || source
                .client
                .as_ref()
                .is_some_and(|client| client.snapshot().is_some())
            || ibus::probe_status().active_engine.as_deref() == Some("parakit"))
}

/// Validate registration and connection without committing into a focused app.
///
/// # Returns
/// Success when native build support, the bus, and component registration exist.
///
/// # Errors
/// Reports missing build support, an unavailable bus, or absent registration.
pub(super) fn insertion_check() -> Result<()> {
    let probe = ibus::probe_status();
    ensure!(probe.compiled, "this build has no IBus support");
    ensure!(probe.connected, "IBus private bus is unavailable");
    ensure!(probe.engine_listed, "Parakit input source is not registered; run parakit ibus install and refresh the desktop session");
    Ok(())
}

/// Explain actual source state without equating submission with reception.
///
/// # Arguments
/// * `source` - Configured insertion transport and optional daemon connection.
/// * `deep` - Whether to state the passive scope of native deep checks.
///
/// # Returns
/// Diagnostic lines describing source state and native submission limits.
pub(super) fn insertion_details(source: &FocusSource, deep: bool) -> String {
    let probe = ibus::probe_status();
    let status = source.client.as_ref().map(|client| client.status());
    format!(
        "  transport:      {} (IBus native)\n  source:         {}\n  helper:         {}\n  native target:  {}\n  delivery:       CommitText submission has no application-reception acknowledgement{}\n",
        source.backend.label(),
        probe.active_engine.as_deref().unwrap_or("unavailable"),
        status.as_ref().map_or_else(|| if probe.active_engine.as_deref() == Some("parakit") { "active source" } else { "not selected" }, |status| if status.helper_connected { "connected" } else { "unavailable" }),
        status.as_ref().map_or("validated by the daemon at delivery", |status| status.detail.as_str()),
        if deep { "; deep checks are passive and do not prove text reception" } else { "" },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_but_unselected_engine_can_wait_for_activation() {
        let report = report_for_probe(&IbusProbe {
            compiled: true,
            connected: true,
            engine_listed: true,
            active_engine: Some("xkb:us::eng".to_owned()),
        });
        assert!(!report.blocking);
        assert!(report.status.contains("waiting"));
        assert!(report.details.contains("focused editable fields"));
    }

    #[test]
    fn native_ptt_requires_build_bus_and_registration() {
        for (compiled, connected, engine_listed) in [
            (false, false, false),
            (true, false, false),
            (true, true, false),
        ] {
            assert!(
                report_for_probe(&IbusProbe {
                    compiled,
                    connected,
                    engine_listed,
                    active_engine: None
                })
                .blocking
            );
        }
    }
}
