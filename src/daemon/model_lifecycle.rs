//! Model residency and activity admission shared by capture, worker, and IPC.

use anyhow::Result;
use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Default uninterrupted idle interval before releasing the inference session.
pub(crate) const DEFAULT_MODEL_IDLE_MINUTES: u64 = 10;

/// Convert whole minutes to a timeout; zero disables it.
///
/// # Returns
///
/// A timeout saturating at the maximum seconds, or none when disabled.
/// Unrepresentable channel deadlines simply never expire.
pub(crate) fn idle_timeout(minutes: u64) -> Option<Duration> {
    (minutes != 0).then(|| Duration::from_secs(minutes.saturating_mul(60)))
}

/// Residency is independent of the recording/transcribing daemon phase.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Residency {
    /// The session has passed its readiness warmup.
    Loaded,
    /// The worker is opening and warming the session.
    Loading,
    /// No session is owned by the worker.
    Offloaded,
}

impl Residency {
    /// Stable status and diagnostic label.
    ///
    /// # Returns
    ///
    /// The lowercase residency label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Loaded => "loaded",
            Self::Loading => "loading",
            Self::Offloaded => "offloaded",
        }
    }
}

/// Additive model detail in the status protocol.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ModelStatus {
    /// Current session residency.
    pub(crate) residency: Residency,
    /// Effective configured timeout; zero means disabled.
    pub(crate) idle_minutes: u64,
    /// Most recent reload failure, cleared by a successful reload.
    pub(crate) last_error: Option<String>,
}

struct Activity {
    ready: bool,
    count: usize,
    idle_since: Instant,
}

/// Serializes the idle offload decision with admission of recording/insertion work.
///
/// The lock covers bookkeeping and session removal, never native destruction,
/// loading, capture, inference, or paste. A token follows audio through the queue.
pub(crate) struct ActivityGate {
    inner: Mutex<Activity>,
    changed_tx: Sender<()>,
    changed_rx: Receiver<()>,
}

impl ActivityGate {
    /// Create a gate whose idle timer is suspended until startup is ready.
    ///
    /// # Returns
    ///
    /// A shared gate with no admitted activity.
    pub(crate) fn new() -> Arc<Self> {
        let (changed_tx, changed_rx) = bounded(1);
        Arc::new(Self {
            inner: Mutex::new(Activity {
                ready: false,
                count: 0,
                idle_since: Instant::now(),
            }),
            changed_tx,
            changed_rx,
        })
    }

    /// Start the idle clock after all startup readiness checks finish.
    pub(crate) fn ready(&self) {
        let mut activity = self.inner.lock();
        activity.ready = true;
        activity.idle_since = Instant::now();
        let _ = self.changed_tx.try_send(());
    }

    /// Admit activity before starting capture or waiting for the insertion lock.
    ///
    /// # Returns
    ///
    /// A token that blocks offload until dropped.
    pub(crate) fn begin(self: &Arc<Self>) -> ActivityGuard {
        self.inner.lock().count += 1;
        let _ = self.changed_tx.try_send(());
        ActivityGuard(Arc::clone(self))
    }

    /// Wake the worker when admission or completion changes its deadline.
    ///
    /// # Returns
    ///
    /// The bounded, coalescing activity-change receiver.
    pub(crate) fn changes(&self) -> &Receiver<()> {
        &self.changed_rx
    }

    /// Wake an idle worker for a shutdown request without admitting activity.
    pub(crate) fn wake(&self) {
        let _ = self.changed_tx.try_send(());
    }

    /// Remaining idle interval, or no deadline while busy/not ready/disabled.
    ///
    /// # Returns
    ///
    /// The remaining duration, including zero when already due.
    pub(crate) fn remaining(&self, timeout: Option<Duration>) -> Option<Duration> {
        self.remaining_at(timeout, Instant::now())
    }

    fn remaining_at(&self, timeout: Option<Duration>, now: Instant) -> Option<Duration> {
        let activity = self.inner.lock();
        let timeout = timeout?;
        (activity.ready && activity.count == 0)
            .then(|| timeout.saturating_sub(now.saturating_duration_since(activity.idle_since)))
    }

    fn unload_if_idle(&self, timeout: Duration, unload: impl FnOnce()) -> bool {
        let activity = self.inner.lock();
        if !activity.ready || activity.count != 0 || activity.idle_since.elapsed() < timeout {
            return false;
        }
        // Commit session removal before admitting new work. Native destruction
        // runs after unlocking so a new recording can start immediately.
        unload();
        true
    }
}

/// Keeps the model resident until the last queued/active consumer finishes.
pub(crate) struct ActivityGuard(Arc<ActivityGate>);

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        let mut activity = self.0.inner.lock();
        activity.count -= 1;
        if activity.count == 0 {
            activity.idle_since = Instant::now();
        }
        let _ = self.0.changed_tx.try_send(());
    }
}

/// Single-worker session ownership; generic only to test destruction and failure.
pub(crate) struct ModelSlot<T> {
    engine: Option<T>,
    status: ModelStatus,
}

impl<T> ModelSlot<T> {
    /// Wrap an already warmed startup session.
    ///
    /// # Arguments
    ///
    /// * `engine` - Session transferred exclusively to the worker.
    /// * `idle_minutes` - Effective setting for status reporting.
    ///
    /// # Returns
    ///
    /// A loaded model slot.
    pub(crate) fn new(engine: T, idle_minutes: u64) -> Self {
        Self {
            engine: Some(engine),
            status: ModelStatus {
                residency: Residency::Loaded,
                idle_minutes,
                last_error: None,
            },
        }
    }

    /// Return the live engine or the failure that prevented the current reload.
    ///
    /// # Returns
    ///
    /// A worker-local borrow of the ready session.
    ///
    /// # Errors
    ///
    /// Returns the saved reload error when the session is unavailable.
    pub(crate) fn engine(&self) -> Result<&T> {
        self.engine.as_ref().ok_or_else(|| {
            anyhow::anyhow!(self.status.last_error.clone().unwrap_or_else(|| {
                "model is offloaded; recording start did not load it".to_string()
            }))
        })
    }

    /// Snapshot without loading the model or changing the idle timer.
    ///
    /// # Returns
    ///
    /// The residency, timeout, and most recent reload error.
    pub(crate) fn status(&self) -> ModelStatus {
        self.status.clone()
    }

    /// Reopen on PTT start, publishing loading and success/failure states.
    ///
    /// # Arguments
    ///
    /// * `load` - Opens and warms a replacement session if needed.
    /// * `publish` - Receives loading and terminal state snapshots.
    pub(crate) fn ensure_loaded(
        &mut self,
        load: impl FnOnce() -> Result<T>,
        publish: impl Fn(ModelStatus),
    ) {
        if self.engine.is_some() {
            return;
        }
        self.status.residency = Residency::Loading;
        publish(self.status());
        match load() {
            Ok(engine) => {
                self.engine = Some(engine);
                self.status.residency = Residency::Loaded;
                self.status.last_error = None;
            }
            Err(err) => {
                self.status.residency = Residency::Offloaded;
                self.status.last_error = Some(format!("model reload failed: {err:#}"));
            }
        }
        publish(self.status());
    }

    /// Remove an idle session under the activity gate, then destroy it unlocked.
    ///
    /// # Arguments
    ///
    /// * `gate` - Serializes session removal with recording/insertion admission.
    /// * `timeout` - Idle interval, or none to keep the session resident.
    ///
    /// # Returns
    ///
    /// True only when this call destroyed a resident session.
    pub(crate) fn offload(&mut self, gate: &ActivityGate, timeout: Option<Duration>) -> bool {
        let Some(timeout) = timeout.filter(|_| self.engine.is_some()) else {
            return false;
        };
        let mut retired = None;
        if !gate.unload_if_idle(timeout, || retired = self.engine.take()) {
            return false;
        }
        // The worker finishes destruction before consuming a queued start/reload,
        // while the recording coordinator can capture and release during teardown.
        drop(retired);
        self.status.residency = Residency::Offloaded;
        true
    }

    /// Return whether the worker needs an idle deadline.
    ///
    /// # Returns
    ///
    /// True when a session is resident.
    pub(crate) fn is_loaded(&self) -> bool {
        self.engine.is_some()
    }
}

#[cfg(test)]
#[path = "model_lifecycle_tests.rs"]
mod tests;
