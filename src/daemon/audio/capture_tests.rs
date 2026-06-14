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
        config_note: None,
    };
    assert_eq!(
        mic.summary(),
        "USB Speech Mic Mono, 48000 Hz mono input -> 16000 Hz mono model, F32"
    );
}

#[test]
fn mic_summary_makes_downmix_explicit() {
    let mic = MicInfo {
        name: "Microphone Array".to_string(),
        input_rate: 48_000,
        channels: 4,
        sample_format: "F32".to_string(),
        source_id: None,
        resampling: true,
        config_note: None,
    };

    assert_eq!(
        mic.summary(),
        "Microphone Array, 48000 Hz 4ch input -> 16000 Hz mono model, F32"
    );
    assert_eq!(
        mic.detail_lines(),
        vec![
            "model input: 16000 Hz mono PCM".to_string(),
            "capture path: CPAL opened 4ch; callback downmixes to mono before resampling"
                .to_string(),
        ]
    );
}

#[test]
fn preferred_input_config_uses_mono_at_default_rate_and_format() {
    let default = stream_config_range(4, 16_000, 48_000, SampleFormat::F32)
        .with_sample_rate(cpal::SampleRate(48_000));

    let selected = preferred_mono_config_from_ranges(
        &default,
        [
            stream_config_range(2, 16_000, 48_000, SampleFormat::F32),
            stream_config_range(1, 16_000, 48_000, SampleFormat::F32),
        ]
        .iter(),
    )
    .expect("mono config should be selected");

    assert_eq!(selected.channels(), 1);
    assert_eq!(selected.sample_rate().0, 48_000);
    assert_eq!(selected.sample_format(), SampleFormat::F32);
}

#[test]
fn preferred_input_config_keeps_default_when_mono_changes_rate_or_format() {
    let default = stream_config_range(4, 48_000, 48_000, SampleFormat::F32)
        .with_sample_rate(cpal::SampleRate(48_000));

    let selected = preferred_mono_config_from_ranges(
        &default,
        [
            stream_config_range(1, 16_000, 16_000, SampleFormat::F32),
            stream_config_range(1, 48_000, 48_000, SampleFormat::I16),
        ]
        .iter(),
    );

    assert!(selected.is_none());
}

#[test]
fn lower_cost_config_note_reports_intentional_mono_non_selection() {
    let default = stream_config_range(4, 48_000, 48_000, SampleFormat::F32)
        .with_sample_rate(cpal::SampleRate(48_000));
    let ranges = [
        stream_config_range(1, 48_000, 48_000, SampleFormat::I16),
        stream_config_range(1, 16_000, 16_000, SampleFormat::F32),
    ];

    assert_eq!(
        lower_cost_mono_config_note(&default, &ranges),
        Some(
            "same-rate mono is available as I16, but not selected because it changes sample format"
                .to_string()
        )
    );
}

#[test]
fn lower_cost_config_note_reports_target_rate_mono_when_rate_would_change() {
    let default = stream_config_range(4, 48_000, 48_000, SampleFormat::F32)
        .with_sample_rate(cpal::SampleRate(48_000));
    let ranges = [stream_config_range(1, 16_000, 16_000, SampleFormat::F32)];

    assert_eq!(
        lower_cost_mono_config_note(&default, &ranges),
        Some(
            "16000 Hz mono is available as F32, but not selected because the current policy preserves the OS default sample rate"
                .to_string()
        )
    );
}

fn stream_config_range(
    channels: u16,
    min_rate: u32,
    max_rate: u32,
    sample_format: SampleFormat,
) -> cpal::SupportedStreamConfigRange {
    cpal::SupportedStreamConfigRange::new(
        channels,
        cpal::SampleRate(min_rate),
        cpal::SampleRate(max_rate),
        cpal::SupportedBufferSize::Unknown,
        sample_format,
    )
}

#[test]
fn audio_handle_reuses_recording_capacity_after_stop() {
    let handle = AudioHandle::test_handle();

    handle.start_recording().expect("recording should start");
    {
        let mut state = handle.state.lock();
        assert!(state.buffer.capacity() >= RECORDING_CAPACITY);
        state.buffer.extend_from_slice(&[0.25, -0.25]);
    }

    let pcm = handle.stop_recording().expect("recording should stop");
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

fn audio_handle_with_control(
    control_tx: Sender<AudioControl>,
    epoch: u64,
    buffer: Vec<f32>,
) -> AudioHandle {
    AudioHandle {
        state: Arc::new(Mutex::new(CaptureState {
            buffer,
            pre_roll: VecDeque::new(),
        })),
        session_epoch: Arc::new(AtomicU64::new(epoch)),
        next_session_epoch: Arc::new(AtomicU64::new(epoch)),
        control: Arc::new(Mutex::new(Some(control_tx))),
    }
}

#[test]
fn stop_recording_without_drain_takes_buffered_samples() {
    let handle = AudioHandle::test_handle();

    handle.start_recording().expect("recording should start");
    append_processed_samples(&handle.state, &handle.session_epoch, &[0.7]);

    let pcm = handle.stop_recording().expect("recording should stop");
    assert_eq!(pcm, vec![0.7]);
}

#[test]
fn sent_audio_control_start_timeout_does_not_fallback_to_direct_state() {
    let (control_tx, _control_rx) = bounded::<AudioControl>(1);
    let handle = audio_handle_with_control(control_tx, 0, Vec::new());

    let err = handle
        .start_recording()
        .expect_err("sent-but-unacked Start should fail");

    assert!(format!("{err:#}").contains("accepted Start"));
    assert_eq!(handle.session_epoch.load(Ordering::Acquire), 0);
    assert!(handle.state.lock().buffer.is_empty());
}

#[test]
fn full_audio_control_queue_does_not_block_or_fallback() {
    let (control_tx, _control_rx) = bounded::<AudioControl>(1);
    let (ack_tx, _ack_rx) = bounded(1);
    control_tx
        .try_send(AudioControl::Stop { ack: ack_tx })
        .expect("preload control queue");
    let handle = audio_handle_with_control(control_tx, 0, Vec::new());

    let err = handle
        .start_recording()
        .expect_err("full control queue should fail without blocking");

    assert!(format!("{err:#}").contains("control queue is full"));
    assert_eq!(handle.session_epoch.load(Ordering::Acquire), 0);
    assert!(handle.state.lock().buffer.is_empty());
}

#[test]
fn stop_recording_control_failure_resets_recording_state() {
    let (control_tx, _control_rx) = bounded::<AudioControl>(1);
    let (ack_tx, _ack_rx) = bounded(1);
    control_tx
        .try_send(AudioControl::Stop { ack: ack_tx })
        .expect("preload control queue");
    let handle = audio_handle_with_control(control_tx, 7, vec![0.4, -0.4]);

    let err = handle
        .stop_recording()
        .expect_err("stop failure should be reported after local reset");

    assert!(format!("{err:#}").contains("control queue is full"));
    assert_eq!(handle.session_epoch.load(Ordering::Acquire), 0);
    assert!(handle.state.lock().buffer.is_empty());
}

#[test]
fn audio_drain_drop_stops_thread() {
    let ring = HeapRb::<f32>::new(8);
    let (_producer, consumer) = ring.split();
    let (wake_tx, wake_rx) = bounded::<()>(1);
    let (_control_tx, control_rx) = bounded::<DrainControl>(1);
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
    let (control_tx, control_rx) = bounded::<DrainControl>(4);
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
    send_drain_start(&control_tx, 1, true);
    let short_tail = vec![0.25; 100];
    let pushed = producer.push_slice(&short_tail);
    assert_eq!(pushed, short_tail.len());
    let _ = wake_tx.try_send(());

    let first = send_drain_stop(&control_tx);
    assert!(
        !first.is_empty(),
        "stop must flush the partial resampler chunk"
    );

    send_drain_start(&control_tx, 2, true);
    let second = send_drain_stop(&control_tx);
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
fn cold_start_discards_idle_pre_roll() {
    let ring = HeapRb::<f32>::new(48_000);
    let (mut producer, consumer) = ring.split();
    let (wake_tx, wake_rx) = bounded::<()>(1);
    let (control_tx, control_rx) = bounded::<DrainControl>(4);
    let state = Arc::new(Mutex::new(CaptureState::new()));
    let session_epoch = Arc::new(AtomicU64::new(0));
    let alive = Arc::new(AtomicBool::new(true));
    let thread = spawn_audio_drain(
        consumer,
        wake_rx,
        control_rx,
        state,
        Arc::clone(&session_epoch),
        CapturePipeline::default(),
        Arc::clone(&alive),
    )
    .expect("audio drain should spawn");

    let stale_pre_roll = [0.1, 0.2, 0.3];
    assert_eq!(producer.push_slice(&stale_pre_roll), stale_pre_roll.len());
    let _ = wake_tx.try_send(());

    send_drain_start(&control_tx, 1, false);
    let pcm = send_drain_stop(&control_tx);

    assert!(
        pcm.is_empty(),
        "cold Windows starts should not reuse stale idle pre-roll"
    );
    assert_eq!(session_epoch.load(Ordering::Acquire), 0);

    drop(AudioDrain {
        alive,
        wake: wake_tx,
        thread: Some(thread),
    });
}

fn send_drain_start(control_tx: &Sender<DrainControl>, epoch: u64, include_pre_roll: bool) {
    let (ack_tx, ack_rx) = bounded(1);
    control_tx
        .send(DrainControl::Start {
            epoch,
            include_pre_roll,
            ack: ack_tx,
        })
        .expect("send drain start");
    ack_rx.recv().expect("drain start ack");
}

fn send_drain_stop(control_tx: &Sender<DrainControl>) -> Vec<f32> {
    let (ack_tx, ack_rx) = bounded(1);
    control_tx
        .send(DrainControl::Stop { ack: ack_tx })
        .expect("send drain stop");
    ack_rx.recv().expect("drain stop ack")
}

#[test]
fn start_recording_consumes_pre_roll() {
    let handle = AudioHandle::test_handle();
    append_processed_samples(&handle.state, &handle.session_epoch, &[0.1, 0.2, 0.3]);

    handle.start_recording().expect("recording should start");
    let first = handle.stop_recording().expect("recording should stop");
    handle.start_recording().expect("recording should start");
    let second = handle.stop_recording().expect("recording should stop");

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
        config_note: None,
    };
    assert!(by_name.looks_bluetooth());

    let by_source = MicInfo {
        name: "Wireless Test Headset".to_string(),
        input_rate: 16_000,
        channels: 1,
        sample_format: "F32".to_string(),
        source_id: Some("bluez_input.00_11_22_33_44_55.headset-head-unit".to_string()),
        resampling: false,
        config_note: None,
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
