//! Unit tests for live audio selection and recording buffer helpers.

use super::*;
use std::sync::Arc;

#[test]
fn virtual_input_names_are_filtered() {
    assert!(is_virtual_input_name(
        "Monitor of USB Speech Mic Analog Stereo"
    ));
    assert!(is_virtual_input_name("BlackHole 2ch"));
    assert!(is_virtual_input_name("PulseAudio Loopback"));
    assert!(!is_virtual_input_name("USB Speech Mic Mono"));
    assert!(!is_virtual_input_name("Bluetooth Test Headset"));
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
    let handle = AudioHandle {
        buffer: Arc::new(Mutex::new(Vec::new())),
        session_epoch: Arc::new(AtomicU64::new(0)),
        next_session_epoch: Arc::new(AtomicU64::new(0)),
        pipeline: Arc::new(Mutex::new(CapturePipeline::default())),
    };

    handle.start_recording();
    {
        let mut buffer = handle.buffer.lock();
        assert!(buffer.capacity() >= RECORDING_CAPACITY);
        buffer.extend_from_slice(&[0.25, -0.25]);
    }

    let pcm = handle.stop_recording();
    assert_eq!(pcm, vec![0.25, -0.25]);
    assert!(handle.buffer.lock().capacity() >= RECORDING_CAPACITY);
}

#[test]
fn append_recording_samples_honors_hard_cap() {
    let buffer = Mutex::new(vec![0.0; MAX_RECORDING_SAMPLES - 1]);
    let session_epoch = AtomicU64::new(1);
    append_recording_samples(&buffer, &session_epoch, 1, &[1.0, 2.0, 3.0]);

    let buffer = buffer.lock();
    assert_eq!(buffer.len(), MAX_RECORDING_SAMPLES);
    assert_eq!(buffer[MAX_RECORDING_SAMPLES - 1], 1.0);
}

#[test]
fn stale_audio_callback_chunks_are_dropped() {
    let buffer = Mutex::new(Vec::new());
    let session_epoch = AtomicU64::new(1);

    append_recording_samples(&buffer, &session_epoch, 1, &[0.1, 0.2]);
    session_epoch.store(2, Ordering::Release);
    append_recording_samples(&buffer, &session_epoch, 1, &[0.3, 0.4]);

    assert_eq!(buffer.lock().as_slice(), &[0.1, 0.2]);
}

#[test]
fn bluetooth_detection_uses_name_and_source_id_without_virtual_filtering() {
    assert!(!is_virtual_input_name("Bluetooth Test Headset"));
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
fn mic_identity_uses_enhanced_info_fields() {
    let info = MicInfo {
        name: "USB Speech Mic Mono".to_string(),
        input_rate: 48_000,
        channels: 1,
        sample_format: "s24le".to_string(),
        source_id: None,
        resampling: true,
    };

    assert_eq!(
        mic_identity_from_info(
            &info,
            Some("alsa_input.usb-Test_Speech_Mic-00.mono-fallback".to_string())
        ),
        MicIdentity {
            name: "USB Speech Mic Mono".to_string(),
            source_id: Some("alsa_input.usb-Test_Speech_Mic-00.mono-fallback".to_string()),
            input_rate: 48_000,
            channels: 1,
            sample_format: "s24le".to_string(),
        }
    );
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

#[cfg(target_os = "linux")]
#[test]
fn pactl_source_parser_extracts_description_and_rate() {
    let sources = parse_pactl_sources(
        r#"Source #42
    Name: alsa_input.usb-Test_Speech_Mic-00.mono-fallback
    Description: USB Speech Mic Mono
    Sample Specification: s24le 1ch 48000Hz
Source #43
    Name: alsa_output.pci-0000_00.monitor
    Description: Monitor of HDMI Audio
    Sample Specification: s32le 2ch 48000Hz
"#,
    );
    assert_eq!(sources.len(), 2);
    assert_eq!(
        sources[0],
        PactlSourceInfo {
            name: "alsa_input.usb-Test_Speech_Mic-00.mono-fallback".to_string(),
            description: Some("USB Speech Mic Mono".to_string()),
            rate: Some(48_000),
            channels: Some(1),
            sample_format: Some("s24le".to_string()),
        }
    );
}
