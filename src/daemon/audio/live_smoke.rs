//! Opt-in speaker-to-microphone smoke test using the production capture path.

use super::{AudioCapture, TARGET_RATE};
use crate::daemon::{
    logging::{LogLevel, Logger},
    notifications::Notifier,
    sounds::Sounds,
};
use parakit::inference::{DeviceMode, Engine};
use serde_json::json;
use std::{
    path::PathBuf,
    process::Command,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

fn repository_path(key: &str) -> PathBuf {
    let path = PathBuf::from(std::env::var_os(key).expect(key));
    let root = std::env::current_dir().unwrap().canonicalize().unwrap();
    let path = path.canonicalize().unwrap();
    assert!(path.starts_with(root), "{key} must be inside the checkout");
    path
}

fn capture_for(audio: &AudioCapture, duration: Duration) -> Vec<f32> {
    audio.handle.start_recording().unwrap();
    thread::sleep(duration);
    audio.handle.stop_recording().unwrap()
}

fn metrics(pcm: &[f32]) -> serde_json::Value {
    assert!(!pcm.is_empty());
    assert!(pcm.iter().all(|sample| sample.is_finite()));
    let rms = (pcm.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / pcm.len() as f64).sqrt();
    let tones: Vec<_> = [207.0, 220.0, 330.0, 440.0, 554.0, 659.0]
        .into_iter()
        .map(|hz| {
            let (re, im) = pcm
                .iter()
                .enumerate()
                .fold((0.0, 0.0), |(re, im), (i, sample)| {
                    let phase = std::f64::consts::TAU * hz * i as f64 / f64::from(TARGET_RATE);
                    (
                        re + f64::from(*sample) * phase.cos(),
                        im + f64::from(*sample) * phase.sin(),
                    )
                });
            json!({"hz": hz, "amplitude": 2.0 * re.hypot(im) / pcm.len() as f64})
        })
        .collect();
    json!({"samples": pcm.len(), "sample_rate": TARGET_RATE, "rms": rms, "tones": tones})
}

fn process_usage() -> libc::rusage {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes the supplied rusage on success; it does not
    // retain the pointer, and assume_init is reached only after a successful call.
    assert_eq!(
        unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
        0
    );
    unsafe { usage.assume_init() }
}

fn usage_delta(before: &libc::rusage, after: &libc::rusage) -> serde_json::Value {
    let seconds = |value: libc::timeval| value.tv_sec as f64 + value.tv_usec as f64 / 1_000_000.0;
    json!({
        "scope": "process",
        "user_cpu_seconds": seconds(after.ru_utime) - seconds(before.ru_utime),
        "system_cpu_seconds": seconds(after.ru_stime) - seconds(before.ru_stime),
        "voluntary_context_switches": after.ru_nvcsw - before.ru_nvcsw,
        "involuntary_context_switches": after.ru_nivcsw - before.ru_nivcsw,
    })
}

#[test]
#[ignore = "opens the real macOS microphone; requires microphone permission and an available input device"]
fn microphone_capture_retains_samples_after_idle() {
    let log = Arc::new(Logger::new(LogLevel::Quiet));
    let audio = AudioCapture::open(Arc::clone(&log), Notifier::silent(log)).unwrap();
    let mic = audio.mic_info().expect("opened microphone metadata");
    println!(
        "{}",
        json!({
            "microphone": {
                "name": mic.name,
                "input_rate": mic.input_rate,
                "channels": mic.channels,
                "sample_format": mic.sample_format,
                "resampling": mic.resampling,
                "source_id": mic.source_id,
                "config_note": mic.config_note,
            },
            "model_sample_rate": TARGET_RATE,
        })
    );
    // Warm the always-running macOS stream and its bounded 350 ms pre-roll.
    thread::sleep(Duration::from_secs(1));
    for idle_seconds in [0, 10] {
        for capture_ms in [300, 3_000] {
            let idle_usage = process_usage();
            let idle_at = Instant::now();
            thread::sleep(Duration::from_secs(idle_seconds));
            let idle_elapsed = idle_at.elapsed();
            let after_idle = process_usage();

            let capture_usage = process_usage();
            let start_at = Instant::now();
            audio.handle.start_recording().unwrap();
            let start_latency = start_at.elapsed();
            let held_at = Instant::now();
            thread::sleep(Duration::from_millis(capture_ms));
            let held = held_at.elapsed();
            let stop_at = Instant::now();
            let pcm = audio.handle.stop_recording().unwrap();
            let stop_latency = stop_at.elapsed();
            let total_elapsed = start_at.elapsed();
            let after_capture = process_usage();

            // Allow callback scheduling slack, plus at most 350 ms of pre-roll.
            // These bounds check retained duration, not acoustic speech onset.
            let slack = 0.1_f64.max(held.as_secs_f64() * 0.1);
            let minimum = ((held.as_secs_f64() - slack).max(0.0) * f64::from(TARGET_RATE)) as usize;
            let maximum = ((total_elapsed.as_secs_f64() + 0.35 + 0.1) * f64::from(TARGET_RATE))
                .ceil() as usize;
            let finite = pcm.iter().all(|sample| sample.is_finite());
            println!(
                "{}",
                json!({
                    "idle_requested_seconds": idle_seconds,
                    "idle_elapsed_seconds": idle_elapsed.as_secs_f64(),
                    "idle_usage": usage_delta(&idle_usage, &after_idle),
                    "capture_requested_ms": capture_ms,
                    "held_seconds": held.as_secs_f64(),
                    "start_control_ms": start_latency.as_secs_f64() * 1_000.0,
                    "stop_control_ms": stop_latency.as_secs_f64() * 1_000.0,
                    "capture_usage": usage_delta(&capture_usage, &after_capture),
                    "samples": pcm.len(),
                    "sample_rate": TARGET_RATE,
                    "all_finite": finite,
                    "minimum_samples": minimum,
                    "maximum_samples": maximum,
                })
            );
            assert!(finite, "capture contains nonfinite samples");
            assert!(
                (minimum..=maximum).contains(&pcm.len()),
                "retained duration outside bounds after {idle_seconds}s idle/{capture_ms}ms capture"
            );
        }
    }
}

#[test]
#[ignore = "plays WAVs aloud and uses the real microphone; requires PARAKIT_LIVE_MODEL, WAV, SECOND_WAV, and REPORT"]
fn speaker_wav_through_live_capture() {
    let model = repository_path("PARAKIT_LIVE_MODEL");
    let wav = repository_path("PARAKIT_LIVE_WAV");
    let second = repository_path("PARAKIT_LIVE_SECOND_WAV");
    let report = repository_path("PARAKIT_LIVE_REPORT");
    let device = match std::env::var("PARAKIT_LIVE_DEVICE").as_deref() {
        Ok("gpu") => DeviceMode::Gpu,
        Ok("cpu") => DeviceMode::Cpu,
        _ => panic!("PARAKIT_LIVE_DEVICE must be cpu or gpu"),
    };
    // Set native defaults in the launching environment, before test threads exist.
    let engine = Engine::open(model, 6, device).unwrap();
    let log = Arc::new(Logger::new(LogLevel::Verbose));
    let audio = AudioCapture::open(Arc::clone(&log), Notifier::silent(log)).unwrap();
    let mic = audio.mic_info().unwrap().summary();
    let baseline = metrics(&capture_for(&audio, Duration::from_secs(1)));
    let mut cues = Vec::new();
    if std::env::var_os("PARAKIT_LIVE_CUES").is_some() {
        for (name, enabled) in [
            ("disabled", false),
            ("reload", true),
            ("start", true),
            ("success", true),
            ("error", true),
        ] {
            let sounds = Sounds::new(enabled);
            audio.handle.start_recording().unwrap();
            match name {
                "reload" => sounds.reload(),
                "start" => sounds.start(Arc::new(std::sync::atomic::AtomicBool::new(true))),
                "error" => sounds.error(),
                _ => sounds.success(),
            }
            thread::sleep(Duration::from_secs(1));
            cues.push(
                json!({"cue": name, "capture": metrics(&audio.handle.stop_recording().unwrap())}),
            );
        }
    }
    let mut utterances = Vec::new();
    for path in [&wav, &second, &wav] {
        audio.handle.start_recording().unwrap();
        thread::sleep(Duration::from_millis(300));
        assert!(Command::new("/usr/bin/afplay")
            .arg(path)
            .status()
            .unwrap()
            .success());
        thread::sleep(Duration::from_millis(250));
        let pcm = audio.handle.stop_recording().unwrap();
        let transcript = engine.transcribe(&pcm).unwrap();
        utterances.push(json!({"input": path, "capture": metrics(&pcm), "transcript": transcript}));
    }
    let after = metrics(&capture_for(&audio, Duration::from_secs(1)));
    std::fs::write(
        report,
        serde_json::to_vec_pretty(&json!({
            "mic": mic, "device": device, "baseline": baseline, "cues": cues,
            "utterances": &utterances, "after": after,
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        utterances
            .iter()
            .all(|item| !item["transcript"].as_str().unwrap().trim().is_empty()),
        "WAV playback was not recognized; inspect the capture report"
    );
}
