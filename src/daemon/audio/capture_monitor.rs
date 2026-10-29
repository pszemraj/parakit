//! Background input probes and confirmation of microphone changes.

use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};

use super::device::MicIdentity;

/// One probe at a time, tagged with the stream generation that requested it.
pub(super) struct MicProbe {
    requests: Sender<u64>,
    pub(super) results: Receiver<(u64, Result<MicIdentity>)>,
    pub(super) pending: bool,
}

impl MicProbe {
    /// Run device queries away from recording controls; channel closure stops the worker.
    ///
    /// # Returns
    /// The request handle and generation-tagged result channel.
    ///
    /// # Errors
    /// Reports failure to spawn the probe thread.
    pub(super) fn spawn(
        mut probe: impl FnMut() -> Result<MicIdentity> + Send + 'static,
    ) -> Result<Self> {
        let (requests, receiver) = bounded(1);
        let (sender, results) = bounded(1);
        std::thread::Builder::new()
            .name("parakit-mic-probe".into())
            .spawn(move || {
                while let Ok(generation) = receiver.recv() {
                    if sender.send((generation, probe())).is_err() {
                        break;
                    }
                }
            })
            .context("spawn microphone probe worker")?;
        Ok(Self {
            requests,
            results,
            pending: false,
        })
    }

    /// Queue a probe unless one is already running or awaiting consumption.
    pub(super) fn request(&mut self, generation: u64) {
        if !self.pending {
            self.pending = self.requests.try_send(generation).is_ok();
        }
    }
}

/// Require two matching observations before replacing a healthy stream.
#[derive(Default)]
pub(super) struct MicChanges {
    pending: Option<MicIdentity>,
}

impl MicChanges {
    /// Clear confirmation after a failed probe, recording, or stream replacement.
    pub(super) fn reset(&mut self) {
        self.pending = None;
    }

    /// Confirm changes while treating missing source metadata as unknown.
    ///
    /// # Arguments
    /// * `current` - Identity of the opened stream, including its last known source.
    /// * `next` - Latest successful device probe; its source metadata may be absent.
    ///
    /// # Returns
    /// A changed identity after two consecutive matching observations.
    pub(super) fn observe(
        &mut self,
        current: &MicIdentity,
        next: MicIdentity,
    ) -> Option<MicIdentity> {
        if &next == current || (current.same_device(&next) && next.source_id.is_none()) {
            self.reset();
            return None;
        }
        if self.pending.as_ref() == Some(&next) {
            self.reset();
            Some(next)
        } else {
            self.pending = Some(next);
            None
        }
    }

    /// Whether the next probe should promptly confirm a possible change.
    ///
    /// # Returns
    /// True when one observation awaits confirmation.
    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

#[cfg(test)]
#[path = "capture_monitor_tests.rs"]
mod tests;
