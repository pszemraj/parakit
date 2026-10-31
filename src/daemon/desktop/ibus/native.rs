//! Thread-owned libibus bridge and safe daemon-facing handles.

use super::{CommitOutcome, HotkeyTransition, IbusProbe, IbusStatus, NativeTarget, Sender};
use crate::daemon::recording::DictationMode;
use anyhow::{Context, Result};
use parking_lot::Mutex;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Instant;

type EventCallback = unsafe extern "C" fn(c_int, c_int, c_int, *const NativeTarget, *mut c_void);

unsafe extern "C" {
    fn parakit_ibus_client_new(callback: EventCallback, user: *mut c_void) -> *mut c_void;
    fn parakit_ibus_client_run(client: *mut c_void);
    fn parakit_ibus_client_configure(
        client: *mut c_void,
        ready: c_int,
        ptt: c_int,
        context_enabled: c_int,
    );
    fn parakit_ibus_client_commit(
        client: *mut c_void,
        target: *const NativeTarget,
        text: *const c_char,
    ) -> c_int;
    fn parakit_ibus_client_stop(client: *mut c_void);
    fn parakit_ibus_client_free(client: *mut c_void);
    fn parakit_ibus_probe(listed: *mut c_int, active: *mut *mut c_char) -> c_int;
    fn parakit_ibus_string_free(text: *mut c_char);
}

#[derive(Clone, Copy)]
struct NativeHandle(NonNull<c_void>);

// Native entry points queue operations onto the owning GLib context. Destruction
// occurs only after its thread has exited and all cloned handles are dropped.
unsafe impl Send for NativeHandle {}
unsafe impl Sync for NativeHandle {}

impl NativeHandle {
    fn run(self) {
        unsafe { parakit_ibus_client_run(self.0.as_ptr()) }
    }
}

#[derive(Default)]
struct Snapshot {
    connected: bool,
    helper_connected: bool,
    target: Option<NativeTarget>,
}

struct State {
    snapshot: Mutex<Snapshot>,
    ready: AtomicBool,
    context_enabled: AtomicBool,
    sender: Mutex<Option<Sender<HotkeyTransition>>>,
    started: Mutex<Option<mpsc::SyncSender<()>>>,
}

struct Inner {
    handle: NativeHandle,
    state: Arc<State>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        unsafe { parakit_ibus_client_stop(self.handle.0.as_ptr()) }
        if let Some(thread) = self.thread.get_mut().take() {
            let _ = thread.join();
        }
        unsafe { parakit_ibus_client_free(self.handle.0.as_ptr()) }
    }
}

/// Shared connection to the independently managed Parakit input method.
#[derive(Clone)]
pub(crate) struct IbusClient(Arc<Inner>);

impl IbusClient {
    /// Start a private-bus listener without changing the selected input source.
    ///
    /// # Returns
    /// A clonable client listening for the selected Parakit input method.
    ///
    /// # Errors
    /// Returns an error if the listener thread cannot start.
    pub(crate) fn start() -> Result<Self> {
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let state = Arc::new(State {
            snapshot: Mutex::new(Snapshot::default()),
            ready: AtomicBool::new(false),
            context_enabled: AtomicBool::new(false),
            sender: Mutex::new(None),
            started: Mutex::new(Some(started_tx)),
        });
        let user = Arc::into_raw(Arc::clone(&state)) as usize;
        let pointer = unsafe { parakit_ibus_client_new(event, user as *mut c_void) };
        let handle = NativeHandle(NonNull::new(pointer).context("allocate IBus client")?);
        let thread = match thread::Builder::new()
            .name("parakit-ibus".into())
            .spawn(move || {
                handle.run();
                // The native loop no longer invokes callbacks after it returns.
                unsafe { drop(Arc::from_raw(user as *const State)) }
            }) {
            Ok(thread) => thread,
            Err(error) => {
                unsafe {
                    parakit_ibus_client_free(handle.0.as_ptr());
                    drop(Arc::from_raw(user as *const State));
                }
                return Err(error).context("spawn IBus client thread");
            }
        };
        let client = Self(Arc::new(Inner {
            handle,
            state,
            thread: Mutex::new(Some(thread)),
        }));
        started_rx.recv().context("initialize IBus client thread")?;
        Ok(client)
    }

    /// Return the cached native focus, including blocked private-field metadata.
    ///
    /// # Returns
    /// The latest attached target, or `None` while no native focus is available.
    pub(crate) fn snapshot(&self) -> Option<NativeTarget> {
        self.0.state.snapshot.lock().target
    }

    /// Publish daemon admission readiness and invalidate earlier target tokens.
    pub(crate) fn set_ready(&self, ready: bool) {
        self.0.state.ready.store(ready, Ordering::Release);
        self.configure();
    }

    /// Enable focused-field PTT only when the native backend owns the trigger.
    ///
    /// # Arguments
    /// * `sender` - Coordinator channel when IBus owns the trigger.
    /// * `context_enabled` - Enable Ctrl+Alt+Space for Qwen context.
    pub(crate) fn set_ptt_sender(
        &self,
        sender: Option<Sender<HotkeyTransition>>,
        context_enabled: bool,
    ) {
        *self.0.state.sender.lock() = sender;
        self.0
            .state
            .context_enabled
            .store(context_enabled, Ordering::Release);
        self.configure();
    }

    fn configure(&self) {
        let ready = self.0.state.ready.load(Ordering::Acquire);
        let ptt = ready && self.0.state.sender.lock().is_some();
        unsafe {
            parakit_ibus_client_configure(
                self.0.handle.0.as_ptr(),
                ready.into(),
                ptt.into(),
                self.0.state.context_enabled.load(Ordering::Acquire).into(),
            );
        }
    }

    /// Attempt one validated native submission, without retry or fallback.
    ///
    /// # Arguments
    /// * `target` - Native identity captured when the recording began.
    /// * `text` - Sanitized UTF-8 transcript to submit.
    ///
    /// # Returns
    /// Definite rejection, submission evidence, or uncertainty after a lost reply.
    pub(crate) fn commit(&self, target: NativeTarget, text: &str) -> CommitOutcome {
        if !target.eligible() {
            return CommitOutcome::Rejected("native target does not permit dictation".into());
        }
        let text = match CString::new(text) {
            Ok(text) => text,
            Err(_) => return CommitOutcome::Rejected("native text contains a NUL byte".into()),
        };
        match unsafe {
            parakit_ibus_client_commit(self.0.handle.0.as_ptr(), &target, text.as_ptr())
        } {
            1 => CommitOutcome::Submitted,
            0 => CommitOutcome::Rejected("native target changed or is unavailable".into()),
            _ => CommitOutcome::Uncertain(
                "IBus submission reply was lost; check the target before retrying".into(),
            ),
        }
    }

    /// Describe current native availability without selecting an input source.
    ///
    /// # Returns
    /// Cached connection, target and daemon readiness details.
    pub(crate) fn status(&self) -> IbusStatus {
        let snapshot = self.0.state.snapshot.lock();
        let ready = self.0.state.ready.load(Ordering::Acquire)
            && snapshot.target.is_some_and(NativeTarget::eligible);
        let detail = if !snapshot.connected {
            "IBus daemon is unavailable"
        } else if !snapshot.helper_connected {
            "Parakit input source is not active"
        } else if !self.0.state.ready.load(Ordering::Acquire) {
            "daemon is preparing"
        } else if snapshot.target.is_some_and(|target| !target.eligible()) {
            "focused field is private"
        } else if !ready {
            "no focused Parakit input context"
        } else {
            "native input context is ready"
        };
        IbusStatus {
            connected: snapshot.connected,
            helper_connected: snapshot.helper_connected,
            ready,
            detail: detail.into(),
        }
    }
}

unsafe extern "C" fn event(
    kind: c_int,
    connected: c_int,
    helper: c_int,
    target: *const NativeTarget,
    user: *mut c_void,
) {
    // The callback's Arc reference outlives the complete native event loop.
    let state = unsafe { &*(user as *const State) };
    let target = unsafe { target.as_ref() }
        .copied()
        .filter(|target| target.engine_id != 0);
    {
        let mut snapshot = state.snapshot.lock();
        snapshot.connected = connected != 0;
        snapshot.helper_connected = helper != 0;
        if kind == 0 {
            snapshot.target = target;
        }
    }
    if let Some(started) = state.started.lock().take() {
        let _ = started.send(());
    }
    let sender = state.sender.lock().clone();
    if let Some(sender) = sender {
        let transition = match kind {
            1 | 4 if state.ready.load(Ordering::Acquire) => {
                target.map(|target| HotkeyTransition::NativePressed {
                    at: Instant::now(),
                    mode: if kind == 4 {
                        DictationMode::WithContext
                    } else {
                        DictationMode::Plain
                    },
                    target,
                })
            }
            2 => Some(HotkeyTransition::Released { at: Instant::now() }),
            _ => None,
        };
        if let Some(transition) = transition {
            let _ = sender.send(transition);
        }
    }
}

/// Probe IBus registration and selection without attaching to the helper.
///
/// # Returns
/// Bus reachability, registry presence, and the reported active engine.
pub(crate) fn probe_status() -> IbusProbe {
    let mut listed = 0;
    let mut active = std::ptr::null_mut();
    let connected = unsafe { parakit_ibus_probe(&mut listed, &mut active) } != 0;
    let active_engine = if active.is_null() {
        None
    } else {
        let name = unsafe { CStr::from_ptr(active) }
            .to_string_lossy()
            .into_owned();
        unsafe { parakit_ibus_string_free(active) };
        Some(name)
    };
    IbusProbe {
        compiled: true,
        connected,
        engine_listed: listed != 0,
        active_engine,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_press_preserves_context_intent_and_readiness() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let state = State {
            snapshot: Mutex::new(Snapshot::default()),
            ready: AtomicBool::new(true),
            context_enabled: AtomicBool::new(true),
            sender: Mutex::new(Some(sender)),
            started: Mutex::new(None),
        };
        let target = NativeTarget {
            engine_id: 1,
            ..NativeTarget::default()
        };
        for (kind, expected) in [(1, DictationMode::Plain), (4, DictationMode::WithContext)] {
            unsafe {
                event(
                    kind,
                    1,
                    1,
                    &target,
                    (&state as *const State).cast_mut().cast(),
                );
            }
            assert!(matches!(receiver.try_recv().unwrap(),
                HotkeyTransition::NativePressed { mode, target: captured, .. }
                if mode == expected && captured == target));
        }
        state.ready.store(false, Ordering::Release);
        unsafe {
            event(4, 1, 1, &target, (&state as *const State).cast_mut().cast());
        }
        assert!(receiver.try_recv().is_err());
    }
}
