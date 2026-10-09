//! Deterministic residency/admission tests without an inference model.

use super::*;
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Session(Arc<AtomicUsize>);

impl Drop for Session {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn slot() -> (ModelSlot<Session>, Arc<AtomicUsize>) {
    let drops = Arc::new(AtomicUsize::new(0));
    (ModelSlot::new(Session(Arc::clone(&drops)), 10), drops)
}

#[test]
fn timeout_defaults_disable_and_overflow() {
    assert_eq!(DEFAULT_MODEL_IDLE_MINUTES, 10);
    assert_eq!(idle_timeout(10).unwrap(), Some(Duration::from_secs(600)));
    assert_eq!(idle_timeout(0).unwrap(), None);
    assert!(idle_timeout(u64::MAX).is_err());
}

#[test]
fn startup_and_disabled_timeout_never_offload() {
    let gate = ActivityGate::new();
    let (mut slot, drops) = slot();
    assert!(!slot.offload(&gate, Some(Duration::ZERO)));
    gate.ready();
    assert!(!slot.offload(&gate, None));
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert_eq!(gate.remaining(None), None);
}

#[test]
fn all_active_leases_must_finish_before_unload_and_restart_deadline() {
    let gate = ActivityGate::new();
    gate.ready();
    let (mut slot, drops) = slot();
    let recording = gate.begin();
    let insertion = gate.begin();
    assert_eq!(gate.remaining(Some(Duration::ZERO)), None);
    assert!(!slot.offload(&gate, Some(Duration::ZERO)));
    drop(recording);
    assert!(!slot.offload(&gate, Some(Duration::ZERO)));
    let completed_after = Instant::now();
    drop(insertion);
    assert!(gate.inner.lock().idle_since >= completed_after);
    assert!(!slot.offload(&gate, Some(Duration::from_secs(60))));
    assert!(slot.offload(&gate, Some(Duration::ZERO)));
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(!slot.offload(&gate, Some(Duration::ZERO)));
}

#[test]
fn queued_audio_and_disconnected_queue_keep_then_release_activity() {
    let gate = ActivityGate::new();
    gate.ready();
    let (mut slot, _) = slot();
    let (tx, rx) = bounded(1);
    tx.send(gate.begin()).unwrap();
    assert!(!slot.offload(&gate, Some(Duration::ZERO)));
    let processing = rx.recv().unwrap();
    assert!(!slot.offload(&gate, Some(Duration::ZERO)));
    drop(processing);
    tx.send(gate.begin()).unwrap();
    drop(rx);
    drop(tx);
    assert!(slot.offload(&gate, Some(Duration::ZERO)));
}

#[test]
fn status_reads_do_not_postpone_expiration() {
    let gate = ActivityGate::new();
    gate.ready();
    let (mut slot, _) = slot();
    let started = gate.inner.lock().idle_since;
    for _ in 0..5 {
        assert_eq!(slot.status().residency, Residency::Loaded);
        assert_eq!(
            gate.remaining_at(
                Some(Duration::from_secs(10)),
                started + Duration::from_secs(9)
            ),
            Some(Duration::from_secs(1))
        );
    }
    assert_eq!(gate.inner.lock().idle_since, started);
    assert!(slot.offload(&gate, Some(Duration::ZERO)));
}

#[test]
fn reload_failure_is_visible_and_next_start_retries_once() {
    let gate = ActivityGate::new();
    gate.ready();
    let (mut slot, drops) = slot();
    assert!(slot.offload(&gate, Some(Duration::ZERO)));
    let states = RefCell::new(Vec::new());
    let publish = |status: ModelStatus| states.borrow_mut().push(status.residency);
    slot.ensure_loaded(|| anyhow::bail!("GPU unavailable"), publish);
    assert!(!slot.is_loaded());
    assert!(slot
        .status()
        .last_error
        .unwrap()
        .contains("GPU unavailable"));
    assert!(slot.engine().is_err());
    let recording = gate.begin();
    slot.ensure_loaded(|| Ok(Session(Arc::clone(&drops))), publish);
    assert!(slot.engine().is_ok());
    assert_eq!(slot.status().last_error, None);
    assert_eq!(
        *states.borrow(),
        [
            Residency::Loading,
            Residency::Offloaded,
            Residency::Loading,
            Residency::Loaded
        ]
    );
    slot.ensure_loaded(|| panic!("resident session must not be reopened"), |_| {});
    assert!(!slot.offload(&gate, Some(Duration::ZERO)));
    drop(recording);
    assert!(slot.offload(&gate, Some(Duration::ZERO)));
    assert_eq!(drops.load(Ordering::SeqCst), 2);
}

#[test]
fn idle_decision_serializes_activity_admission() {
    let gate = ActivityGate::new();
    gate.ready();
    let (entered_tx, entered_rx) = bounded(1);
    let (release_tx, release_rx) = bounded(1);
    let unload_gate = Arc::clone(&gate);
    let unload = std::thread::spawn(move || {
        unload_gate.unload_if_idle(Duration::ZERO, || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        })
    });
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(gate.inner.try_lock().is_none());
    let (attempt_tx, attempt_rx) = bounded(1);
    let (admit_tx, admit_rx) = bounded(1);
    let recorder = std::thread::spawn(move || {
        attempt_tx.send(()).unwrap();
        admit_tx.send(gate.begin()).unwrap();
    });
    attempt_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(admit_rx.try_recv().is_err());
    release_tx.send(()).unwrap();
    let lease = admit_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(unload.join().unwrap());
    recorder.join().unwrap();
    drop(lease);
}

#[test]
fn loading_does_not_hold_activity_mutex() {
    let gate = ActivityGate::new();
    gate.ready();
    let (mut slot, drops) = slot();
    slot.offload(&gate, Some(Duration::ZERO));
    let recording = gate.begin();
    slot.ensure_loaded(
        || {
            let _second_recording = gate.begin();
            Ok(Session(Arc::clone(&drops)))
        },
        |_| {},
    );
    drop(recording);
    assert!(slot.is_loaded());
}
