//! Opt-in speaker-to-microphone smoke test using the production capture path.

use super::{AudioCapture, TARGET_RATE};
use crate::daemon::{
    logging::{LogLevel, Logger},
    notifications::Notifier,
    sounds::Sounds,
};
use parakit::inference::{DeviceMode, Engine};
use serde_json::json;
use std::{path::PathBuf, process::Command, sync::Arc, thread, time::Duration};

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
