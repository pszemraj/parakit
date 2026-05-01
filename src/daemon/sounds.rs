//! Programmatic audio cues for the daemon.
//!
//! Three tones are generated on the fly:
//!   - Start  : low ding (A4 = 440 Hz, ~80 ms)
//!   - Stop   : high ding (E5 = 659 Hz, ~80 ms) — successful transcription
//!   - Error  : two-pulse low buzz (A3 = 220 Hz, ~110 ms each)
//!
//! Implementation note: rodio's `OutputStream` is `!Send`, so we run a
//! dedicated sound thread that owns the stream and listens on a channel.
//! [`Sounds`] is then a thin `Send + Sync` wrapper around the channel
//! sender, so the hotkey and worker threads can both poke it.

use crossbeam_channel::{bounded, Sender};
use rodio::source::Source;
use rodio::{OutputStream, Sink};
use std::time::Duration;

#[derive(Clone, Copy)]
enum Cue {
    Start,
    Success,
    Error,
}

/// Public handle. Cheap to clone (it's just a channel sender).
#[derive(Clone)]
pub struct Sounds {
    tx: Option<Sender<Cue>>,
}

impl Sounds {
    /// Start the sound cue thread when sound cues are enabled.
    ///
    /// # Panics
    ///
    /// Panics if the cue thread cannot be spawned.
    ///
    /// # Returns
    ///
    /// A cloneable handle for sending non-blocking cue requests.
    pub fn new(enabled: bool) -> Self {
        if !enabled {
            return Self { tx: None };
        }

        let (tx, rx) = bounded::<Cue>(8);
        std::thread::Builder::new()
            .name("parakit-sounds".into())
            .spawn(move || {
                // Owning the stream on this thread keeps cpal/rodio happy.
                let (_stream, handle) = match OutputStream::try_default() {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!(
                            "parakit: could not open audio output for cues: {e:?}\n\
                             (Sound cues will be disabled. Pass --no-sounds to silence this warning.)"
                        );
                        return;
                    }
                };

                while let Ok(cue) = rx.recv() {
                    let result = (|| -> Result<(), String> {
                        let sink =
                            Sink::try_new(&handle).map_err(|e| format!("sink: {e:?}"))?;
                        match cue {
                            Cue::Start => {
                                sink.append(sine_with_envelope(
                                    440.0,
                                    Duration::from_millis(80),
                                    0.6,
                                ));
                            }
                            Cue::Success => {
                                sink.append(sine_with_envelope(
                                    659.0,
                                    Duration::from_millis(80),
                                    0.6,
                                ));
                            }
                            Cue::Error => {
                                sink.append(sine_with_envelope(
                                    220.0,
                                    Duration::from_millis(110),
                                    0.7,
                                ));
                                sink.append(sine_with_envelope(
                                    207.0, // a touch flat for clear "wrong" feel
                                    Duration::from_millis(110),
                                    0.7,
                                ));
                            }
                        }
                        // Block this thread until the cue finishes so the
                        // next cue doesn't start mid-tone.
                        sink.sleep_until_end();
                        Ok(())
                    })();
                    if let Err(e) = result {
                        eprintln!("parakit: sound cue dropped: {e}");
                    }
                }
            })
            .expect("failed to spawn parakit-sounds thread");

        Self { tx: Some(tx) }
    }

    /// Play the recording-start cue.
    pub fn start(&self) {
        self.send(Cue::Start);
    }

    /// Play the successful-transcription cue.
    pub fn success(&self) {
        self.send(Cue::Success);
    }

    /// Play the error cue.
    pub fn error(&self) {
        self.send(Cue::Error);
    }

    fn send(&self, cue: Cue) {
        if let Some(tx) = &self.tx {
            // Non-blocking — drop the cue if the channel is full or the
            // sound thread has died. Audio cues are non-essential.
            let _ = tx.try_send(cue);
        }
    }
}

/// Sine wave with a short attack and release to avoid clicks.
fn sine_with_envelope(freq: f32, dur: Duration, vol: f32) -> impl Source<Item = f32> {
    let sample_rate = 44_100u32;
    let total_samples = (sample_rate as f32 * dur.as_secs_f32()) as usize;
    let attack_samples = (sample_rate as f32 * 0.005) as usize; // 5 ms
    let release_samples = (sample_rate as f32 * 0.020) as usize; // 20 ms
    let two_pi_f = 2.0 * std::f32::consts::PI * freq;

    let samples: Vec<f32> = (0..total_samples)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            let mut v = (two_pi_f * t).sin() * vol;
            if i < attack_samples {
                v *= i as f32 / attack_samples as f32;
            }
            let from_end = total_samples.saturating_sub(i);
            if from_end < release_samples {
                v *= from_end as f32 / release_samples as f32;
            }
            v
        })
        .collect();

    rodio::buffer::SamplesBuffer::new(1, sample_rate, samples)
}
