//! Unit tests for live audio selection and recording buffer helpers.

use super::*;
use std::thread;
use std::time::Duration;

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
fn stale_audio_chunks_are_dropped_from_recording_and_pre_roll() {
    let state = Mutex::new(CaptureState::new());
    let session_epoch = AtomicU64::new(1);

    append_processed_samples(&state, &session_epoch, &[0.1, 0.2]);
    session_epoch.store(0, Ordering::Release);
    append_processed_samples_observed(&state, &session_epoch, 1, &[0.3, 0.4]);

    let state = state.lock();
    assert_eq!(state.buffer.as_slice(), &[0.1, 0.2]);
    assert!(state.pre_roll.is_empty());
}

#[test]
fn active_recording_samples_do_not_refresh_pre_roll() {
    let state = Mutex::new(CaptureState::new());
    let session_epoch = AtomicU64::new(0);

    append_processed_samples(&state, &session_epoch, &[0.1, 0.2]);
    session_epoch.store(1, Ordering::Release);
    append_processed_samples(&state, &session_epoch, &[0.8, 0.9]);

    let state = state.lock();
    assert_eq!(state.buffer.as_slice(), &[0.8, 0.9]);
    assert_eq!(
        state.pre_roll.iter().copied().collect::<Vec<_>>(),
        &[0.1, 0.2]
    );
}

#[test]
fn idle_observed_chunks_are_dropped_if_recording_starts_before_append() {
    let state = Mutex::new(CaptureState::new());
    let session_epoch = AtomicU64::new(0);

    append_processed_samples(&state, &session_epoch, &[0.1, 0.2]);
    session_epoch.store(1, Ordering::Release);
    append_processed_samples_observed(&state, &session_epoch, 0, &[0.3, 0.4]);

    let state = state.lock();
    assert!(state.buffer.is_empty());
    assert_eq!(
        state.pre_roll.iter().copied().collect::<Vec<_>>(),
        &[0.1, 0.2]
    );
}

#[test]
fn stop_recording_keeps_final_chunk_already_inside_pipeline() {
    let handle = AudioHandle::test_handle();

    handle.start_recording();
    let stopper = {
        let handle = handle.clone();
        thread::spawn(move || handle.stop_recording())
    };

    thread::sleep(Duration::from_millis(5));
    append_processed_samples(&handle.state, &handle.session_epoch, &[0.7]);

    let pcm = stopper.join().expect("stop thread should not panic");
    assert_eq!(pcm, vec![0.7]);
}

#[test]
fn audio_drain_drop_stops_thread() {
    let ring = HeapRb::<f32>::new(8);
    let (_producer, consumer) = ring.split();
    let (wake_tx, wake_rx) = bounded::<()>(1);
    let alive = Arc::new(AtomicBool::new(true));
    let thread = spawn_audio_drain(
        consumer,
        wake_rx,
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
