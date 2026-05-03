//! Unit tests for live audio selection and recording buffer helpers.

use super::*;

#[test]
fn input_name_classifiers_are_stable() {
    for (name, virtual_input, bluetooth) in [
        ("Monitor of USB Speech Mic Analog Stereo", true, false),
        ("BlackHole 2ch", true, false),
        ("PulseAudio Loopback", true, false),
        ("USB Speech Mic Mono", false, false),
        ("Bluetooth Test Headset", false, true),
        (
            "bluez_input.00_11_22_33_44_55.headset-head-unit",
            false,
            true,
        ),
        ("WH-1000XM4 Hands-Free AG Audio", false, true),
        ("AirPods Pro", false, true),
        ("Pixel Buds Pro", false, true),
    ] {
        assert_eq!(is_virtual_input_name(name), virtual_input, "{name}");
        assert_eq!(is_bluetooth_input_name(name), bluetooth, "{name}");
    }
}

#[test]
fn mic_summary_reports_input_and_model_rates() {
    let mic = MicInfo {
        name: "USB Speech Mic Mono".to_string(),
        input_rate: 48_000,
        channels: 1,
        sample_format: "F32".to_string(),
        source_id: None,
        resampling: true,
    };
    assert_eq!(
        mic.summary(),
        "USB Speech Mic Mono, 48000 Hz input -> 16000 Hz model, mono, F32"
    );
}

#[test]
fn audio_handle_reuses_recording_capacity_after_stop() {
    let handle = AudioHandle::test_handle();

    handle.start_recording();
    {
        let mut state = handle.state.lock();
        assert!(state.buffer.capacity() >= RECORDING_CAPACITY);
        state.buffer.extend_from_slice(&[0.25, -0.25]);
    }

    let pcm = handle.stop_recording();
    assert_eq!(pcm, vec![0.25, -0.25]);
    assert!(handle.state.lock().buffer.capacity() >= RECORDING_CAPACITY);
}

#[test]
fn append_processed_samples_honors_hard_cap() {
    let state = Mutex::new(CaptureState {
        buffer: vec![0.0; MAX_RECORDING_SAMPLES - 1],
        pre_roll: VecDeque::new(),
    });
    let session_epoch = AtomicU64::new(1);
    append_processed_samples(&state, &session_epoch, &[1.0, 2.0, 3.0]);

    let state = state.lock();
    assert_eq!(state.buffer.len(), MAX_RECORDING_SAMPLES);
    assert_eq!(state.buffer[MAX_RECORDING_SAMPLES - 1], 1.0);
}

#[test]
fn append_processed_samples_respects_epoch_boundaries() {
    assert_epoch_boundary(
        "stale recording chunk",
        1,
        0,
        Some(1),
        &[0.3, 0.4],
        &[0.1, 0.2],
        &[],
    );
    assert_epoch_boundary(
        "active recording does not refresh pre-roll",
        0,
        1,
        None,
        &[0.8, 0.9],
        &[0.8, 0.9],
        &[0.1, 0.2],
    );
    assert_epoch_boundary(
        "idle chunk dropped after recording starts",
        0,
        1,
        Some(0),
        &[0.3, 0.4],
        &[],
        &[0.1, 0.2],
    );
}

fn assert_epoch_boundary(
    name: &str,
    initial_epoch: u64,
    next_epoch: u64,
    observed_epoch: Option<u64>,
    next_samples: &[f32],
    expected_buffer: &[f32],
    expected_pre_roll: &[f32],
) {
    let state = Mutex::new(CaptureState::new());
    let session_epoch = AtomicU64::new(initial_epoch);
    append_processed_samples(&state, &session_epoch, &[0.1, 0.2]);
    session_epoch.store(next_epoch, Ordering::Release);
    match observed_epoch {
        Some(observed) => {
            append_processed_samples_observed(&state, &session_epoch, observed, next_samples)
        }
        None => append_processed_samples(&state, &session_epoch, next_samples),
    }

    let state = state.lock();
    assert_eq!(state.buffer, expected_buffer, "{name} buffer");
    assert_eq!(pre_roll_vec(&state), expected_pre_roll, "{name} pre-roll");
}

fn pre_roll_vec(state: &CaptureState) -> Vec<f32> {
    state.pre_roll.iter().copied().collect()
}

#[test]
fn stop_recording_without_drain_takes_buffered_samples() {
    let handle = AudioHandle::test_handle();

    handle.start_recording();
    append_processed_samples(&handle.state, &handle.session_epoch, &[0.7]);

    let pcm = handle.stop_recording();
    assert_eq!(pcm, vec![0.7]);
}

#[test]
fn audio_drain_drop_stops_thread() {
    let ring = HeapRb::<f32>::new(8);
    let (_producer, consumer) = ring.split();
    let (wake_tx, wake_rx) = bounded::<()>(1);
    let (_control_tx, control_rx) = bounded::<AudioControl>(1);
    let alive = Arc::new(AtomicBool::new(true));
    let thread = spawn_audio_drain(
        consumer,
        wake_rx,
        control_rx,
        Arc::new(Mutex::new(CaptureState::new())),
        Arc::new(AtomicU64::new(0)),
        CapturePipeline::default(),
        Arc::clone(&alive),
    )
    .expect("audio drain should spawn");

    drop(AudioDrain {
        alive,
        wake: wake_tx,
        thread: Some(thread),
    });
}

#[test]
fn stop_recording_flushes_resampler_tail_and_keeps_next_pre_roll_clean() {
    let ring = HeapRb::<f32>::new(48_000);
    let (mut producer, consumer) = ring.split();
    let (wake_tx, wake_rx) = bounded::<()>(1);
    let (control_tx, control_rx) = bounded::<AudioControl>(4);
    let state = Arc::new(Mutex::new(CaptureState::new()));
    let session_epoch = Arc::new(AtomicU64::new(0));
    let alive = Arc::new(AtomicBool::new(true));
    let thread = spawn_audio_drain(
        consumer,
        wake_rx,
        control_rx,
        Arc::clone(&state),
        Arc::clone(&session_epoch),
        CapturePipeline {
            resampler: make_resampler(48_000).expect("resampler"),
        },
        Arc::clone(&alive),
    )
    .expect("audio drain should spawn");
    let handle = AudioHandle {
        state,
        session_epoch,
        next_session_epoch: Arc::new(AtomicU64::new(0)),
        control: Arc::new(Mutex::new(Some(control_tx))),
    };

    handle.start_recording();
    let short_tail = vec![0.25; 100];
    let pushed = producer.push_slice(&short_tail);
    assert_eq!(pushed, short_tail.len());
    let _ = wake_tx.try_send(());

    let first = handle.stop_recording();
    assert!(
        !first.is_empty(),
        "stop must flush the partial resampler chunk"
    );

    handle.start_recording();
    let second = handle.stop_recording();
    assert!(
        second.is_empty(),
        "flushed recording tail must not seed the next pre-roll"
    );

    drop(AudioDrain {
        alive,
        wake: wake_tx,
        thread: Some(thread),
    });
}

#[test]
fn start_recording_consumes_pre_roll() {
    let handle = AudioHandle::test_handle();
    append_processed_samples(&handle.state, &handle.session_epoch, &[0.1, 0.2, 0.3]);

    handle.start_recording();
    let first = handle.stop_recording();
    handle.start_recording();
    let second = handle.stop_recording();

    assert_eq!(first, vec![0.1, 0.2, 0.3]);
    assert!(second.is_empty());
}

#[test]
fn bluetooth_input_names_are_detected_but_not_virtual() {
    let by_name = MicInfo {
        name: "Bluetooth Test Headset".to_string(),
        input_rate: 16_000,
        channels: 1,
        sample_format: "F32".to_string(),
        source_id: None,
        resampling: false,
    };
    assert!(by_name.looks_bluetooth());

    let by_source = MicInfo {
        name: "Wireless Test Headset".to_string(),
        input_rate: 16_000,
        channels: 1,
        sample_format: "F32".to_string(),
        source_id: Some("bluez_input.00_11_22_33_44_55.headset-head-unit".to_string()),
        resampling: false,
    };
    assert!(by_source.looks_bluetooth());
}

#[test]
fn source_aware_identity_detects_default_source_switch() {
    let raw = MicIdentity {
        name: "default".to_string(),
        source_id: None,
        input_rate: 48_000,
        channels: 2,
        sample_format: "F32".to_string(),
    };

    let first = source_aware_mic_identity(
        raw.clone(),
        Some("alsa_input.usb-First_Mic-00.mono-fallback".to_string()),
    );
    let second = source_aware_mic_identity(
        raw,
        Some("alsa_input.usb-Second_Mic-00.mono-fallback".to_string()),
    );

    assert_ne!(first, second);
    assert_eq!(first.name, "default");
    assert_eq!(first.input_rate, 48_000);
}

#[test]
fn resampler_flushes_and_resets_tail_between_recordings() {
    let mut pipeline = CapturePipeline {
        resampler: make_resampler(48_000).expect("resampler"),
    };
    let mut out = Vec::new();
    let mut scratch = Vec::new();
    let input = vec![0.1; 100];

    assert!(pipeline.process(&input, &mut scratch).is_empty());
    pipeline.finish_recording(&mut out);
    assert!(!out.is_empty());
    assert!(pipeline.resampler.as_ref().unwrap().scratch.is_empty());

    let flushed_len = out.len();
    pipeline.reset_recording();
    pipeline.finish_recording(&mut out);
    assert_eq!(out.len(), flushed_len);
}

#[test]
fn resampler_reuses_chunk_buffers_during_process_and_flush() {
    let mut resampler = make_resampler(48_000)
        .expect("resampler creation should succeed")
        .expect("48 kHz input should need resampling");
    let input_capacity = resampler.input_buf[0].capacity();
    let output_capacity = resampler.output_buf[0].capacity();
    let mut out = Vec::new();
    let input = vec![0.1; resampler.chunk_size * 3 + 100];

    resampler.process(&input, &mut out);
    resampler.flush_recording(&mut out);

    assert!(!out.is_empty());
    assert_eq!(resampler.input_buf[0].capacity(), input_capacity);
    assert_eq!(resampler.output_buf[0].capacity(), output_capacity);
    assert!(resampler.scratch.is_empty());
}
