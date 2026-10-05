//! Bounded shutdown handshake: destroy the session before native static teardown.

use super::model_lifecycle::ActivityGate;
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Worker release budget for fatal daemon exits outside the IPC stop path.
const FATAL_EXIT_WORKER_WAIT: Duration = Duration::from_secs(5);

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

    /// Release the worker's native session, then exit after a fatal failure.
    ///
    /// # Arguments
    ///
    /// * `activity` - Existing worker wake channel.
    /// * `code` - Process exit status.
    ///
    /// # Returns
    ///
    /// Never returns; the process exits with `code`.
    pub(crate) fn exit_after_worker(&self, activity: &ActivityGate, code: i32) -> ! {
        terminate_process(
            code,
            self.request_and_wait(activity, FATAL_EXIT_WORKER_WAIT),
        )
    }
}

/// End the process once worker-owned native sessions are released.
///
/// # Arguments
///
/// * `code` - Process exit status.
/// * `graceful` - Whether normal process teardown is safe because the worker
///   finished. Otherwise the process terminates immediately.
///
/// # Returns
///
/// Never returns; the process exits with `code`.
pub(crate) fn terminate_process(code: i32, graceful: bool) -> ! {
    if graceful {
        std::process::exit(code);
    }
    // A wedged worker/insertion must not hang exit, or race C++ static
    // destructors with live native buffers. The OS reclaims process resources.
    unsafe { libc::_exit(code) }
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

    #[test]
    fn hotkey_failure_exits_only_after_worker_resource_release() {
        const CHILD: &str = "PARAKIT_HOTKEY_EXIT_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            struct Session;
            impl Drop for Session {
                fn drop(&mut self) {
                    use std::io::Write;
                    println!("test worker session released");
                    std::io::stdout().flush().unwrap();
                }
            }

            let state = Arc::new(super::super::ipc::SharedState::with_history_limit(1));
            let lifetime = state.shutdown.register();
            let worker_state = Arc::clone(&state);
            std::thread::spawn(move || {
                let session = Session;
                worker_state.activity.changes().recv().unwrap();
                assert!(worker_state.shutdown.requested());
                drop(session);
                drop(lifetime);
            });
            crate::app::finish_hotkey_loop(Err(super::super::hotkey::HotkeyLoopFailed), &state);
            panic!("fatal hotkey failure returned without exiting");
        }

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "daemon::worker_shutdown::tests::hotkey_failure_exits_only_after_worker_resource_release",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stdout).contains("test worker session released"));
        assert!(
            output.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
