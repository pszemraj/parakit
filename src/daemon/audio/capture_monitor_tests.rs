//! Device-change confirmation and bounded background probe regressions.

use super::*;
use std::time::Duration;

fn identity(source: Option<&str>) -> MicIdentity {
    MicIdentity {
        name: "default".into(),
        source_id: source.map(str::to_owned),
        input_rate: 48_000,
        channels: 1,
        sample_format: "F32".into(),
    }
}

#[test]
fn missing_source_metadata_never_replaces_a_healthy_stream() {
    let current = identity(Some("A"));
    let mut changes = MicChanges::default();
    for source in [None, None, Some("A"), Some("B"), None, Some("A")] {
        assert_eq!(changes.observe(&current, identity(source)), None);
    }
    assert!(!changes.is_pending());
    assert_eq!(current.source_id.as_deref(), Some("A"));
}

#[test]
fn confirmed_default_source_change_survives_a_shared_cpal_alias() {
    let current = identity(Some("A"));
    let changed = identity(Some("B"));
    let mut changes = MicChanges::default();
    assert_eq!(changes.observe(&current, changed.clone()), None);
    assert_eq!(changes.observe(&current, changed.clone()), Some(changed));
    assert!(!changes.is_pending());
}

#[test]
fn interrupted_confirmation_requires_fresh_observations() {
    let current = identity(Some("A"));
    let changed = identity(Some("B"));
    let mut changes = MicChanges::default();
    assert_eq!(changes.observe(&current, changed.clone()), None);
    changes.reset();
    assert_eq!(changes.observe(&current, changed.clone()), None);
    assert_eq!(changes.observe(&current, current.clone()), None);
    for source in ["B", "C", "B", "C", "A"] {
        assert_eq!(changes.observe(&current, identity(Some(source))), None);
    }
}

#[test]
fn unknown_initial_source_is_confirmed_without_reopen_loops() {
    let initial = identity(None);
    let known = identity(Some("A"));
    let mut changes = MicChanges::default();
    assert_eq!(changes.observe(&initial, known.clone()), None);
    let confirmed = changes.observe(&initial, known.clone()).unwrap();
    let mut reopened = identity(None);
    reopened.retain_source(&confirmed);
    for _ in 0..4 {
        assert_eq!(changes.observe(&reopened, known.clone()), None);
        assert_eq!(changes.observe(&reopened, identity(None)), None);
    }
    assert_eq!(changes.observe(&reopened, identity(Some("B"))), None);
    assert_eq!(
        changes.observe(&reopened, identity(Some("B"))),
        Some(identity(Some("B")))
    );
}

#[test]
fn unknown_metadata_does_not_hide_a_changed_cpal_device_or_format() {
    let current = identity(Some("A"));
    for changed in [
        MicIdentity {
            name: "USB mic".into(),
            ..identity(None)
        },
        MicIdentity {
            input_rate: 44_100,
            ..identity(None)
        },
        MicIdentity {
            channels: 2,
            ..identity(None)
        },
        MicIdentity {
            sample_format: "I16".into(),
            ..identity(None)
        },
    ] {
        let mut changes = MicChanges::default();
        assert_eq!(changes.observe(&current, changed.clone()), None);
        assert_eq!(changes.observe(&current, changed.clone()), Some(changed));
    }
}

#[test]
fn reopening_prefers_fresh_metadata_and_never_copies_it_between_devices() {
    let confirmed = identity(Some("A"));
    let mut fresh = identity(Some("B"));
    fresh.retain_source(&confirmed);
    assert_eq!(fresh.source_id.as_deref(), Some("B"));
    let mut other = MicIdentity {
        name: "USB mic".into(),
        ..identity(None)
    };
    other.retain_source(&confirmed);
    assert_eq!(other.source_id, None);
}

#[test]
fn stalled_probe_does_not_block_requests_or_queue_duplicate_work() {
    let (entered_tx, entered_rx) = bounded(1);
    let (release_tx, release_rx) = bounded(1);
    let mut probe = MicProbe::spawn(move || {
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        Ok(identity(Some("A")))
    })
    .unwrap();
    probe.request(7);
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    for _ in 0..100 {
        probe.request(8);
    }
    assert!(probe.results.is_empty());
    release_tx.send(()).unwrap();
    let (generation, result) = probe.results.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(generation, 7);
    assert_eq!(result.unwrap(), identity(Some("A")));
    assert_eq!(probe.requests.len(), 0);
    probe.pending = false;
    probe.request(9);
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    release_tx.send(()).unwrap();
    assert_eq!(
        probe
            .results
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .0,
        9
    );
    drop(probe);
    assert!(matches!(
        entered_rx.recv_timeout(Duration::from_secs(1)),
        Err(crossbeam_channel::RecvTimeoutError::Disconnected)
    ));
}
