//! Bounded shutdown handshake: destroy the session before native static teardown.

use super::model_lifecycle::ActivityGate;
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Lives from startup until the worker has dropped its engine and other locals.
pub(crate) struct WorkerLifetime {
    _complete: Sender<()>,
}

/// Requests worker exit and observes its lifetime without taking insertion locks.
#[derive(Default)]
pub(crate) struct WorkerShutdown {
    requested: AtomicBool,
    finished: Mutex<Option<Receiver<()>>>,
}

impl WorkerShutdown {
    /// Register before exposing IPC; transfer the returned lifetime to the worker.
    ///
    /// # Returns
    ///
    /// A guard that must outlive the engine, including during failed startup.
    pub(crate) fn register(&self) -> WorkerLifetime {
        let (complete, finished) = bounded(0);
        *self.finished.lock() = Some(finished);
        WorkerLifetime {
            _complete: complete,
        }
    }

    /// Read the stop request before starting another operation or insertion.
    ///
    /// # Returns
    ///
    /// True once IPC requested shutdown.
    pub(crate) fn requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }

    /// Wake the worker and wait within a bounded process-stop budget.
    ///
    /// # Arguments
    ///
    /// * `activity` - Existing worker wake channel, independent of queued audio.
    /// * `timeout` - Maximum wait for startup/inference/insertion to finish.
    ///
    /// # Returns
    ///
    /// True after the worker lifetime ended, or when none was registered.
    pub(crate) fn request_and_wait(&self, activity: &ActivityGate, timeout: Duration) -> bool {
        self.requested.store(true, Ordering::Release);
        activity.wake();
        let finished = self.finished.lock().clone();
        finished.is_none_or(|rx| {
            matches!(
                rx.recv_timeout(timeout),
                Err(RecvTimeoutError::Disconnected)
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn request_wakes_worker_and_waits_for_resources_before_completion() {
        let shutdown = Arc::new(WorkerShutdown::default());
        let activity = ActivityGate::new();
        let lifetime = shutdown.register();
        let worker_shutdown = Arc::clone(&shutdown);
        let worker_activity = Arc::clone(&activity);
        let (released, release_observed) = bounded(1);
        let worker = std::thread::spawn(move || {
            worker_activity.changes().recv().unwrap();
            assert!(worker_shutdown.requested());
            released.send(()).unwrap();
            drop(lifetime);
        });
        assert!(shutdown.request_and_wait(&activity, Duration::from_secs(2)));
        release_observed.try_recv().unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn blocked_worker_times_out_and_already_finished_worker_is_ready() {
        let shutdown = WorkerShutdown::default();
        let activity = ActivityGate::new();
        let lifetime = shutdown.register();
        assert!(!shutdown.request_and_wait(&activity, Duration::ZERO));
        assert!(shutdown.requested());
        drop(lifetime);
        assert!(shutdown.request_and_wait(&activity, Duration::ZERO));
        assert!(WorkerShutdown::default().request_and_wait(&activity, Duration::ZERO));
    }
}
