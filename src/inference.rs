//! Inference wrapper around a `crispasr::Session`.

use crate::constants::TARGET_RATE;
use anyhow::{Context, Result};
use std::borrow::Cow;
use std::path::Path;

/// Minimum PCM length sent to CrispASR.
///
/// Very short captures can collapse to too few feature frames for the model
/// pipeline. Right-padding with silence keeps the hotkey behavior predictable
/// without dropping the user's utterance.
const MIN_INFERENCE_SAMPLES: usize = TARGET_RATE as usize;

/// Transcription mode used by the daemon.
#[derive(Clone, Copy, Debug)]
pub enum Mode {
    /// Transcribe the full utterance after the hotkey is released.
    Batch,
    /// Emit partial transcripts while recording.
    Streaming { chunk_secs: f32 },
}

impl Mode {
    /// Parse a CLI mode string.
    ///
    /// # Returns
    ///
    /// The parsed transcription mode.
    ///
    /// # Errors
    ///
    /// Returns an error for unknown modes or invalid streaming chunk sizes.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "batch" => Ok(Mode::Batch),
            "streaming" => Ok(Mode::Streaming { chunk_secs: 4.0 }),
            other if other.starts_with("streaming:") => {
                let secs: f32 = other.trim_start_matches("streaming:").parse()?;
                Ok(Mode::Streaming { chunk_secs: secs })
            }
            other => Err(anyhow::anyhow!(
                "unknown mode '{other}'. Expected 'batch' or 'streaming' or 'streaming:<seconds>'"
            )),
        }
    }
}

/// Thin wrapper so the rest of the code never touches `crispasr` directly.
///
/// Owned exclusively by the worker thread. `crispasr::Session` is `Send`
/// (we can move it across threads at startup) but not `Sync` (we can't
/// hand out `&Engine` from multiple threads). The architecture respects
/// that: only the worker thread ever calls `transcribe`.
pub struct Engine {
    session: crispasr::Session,
}

impl Engine {
    /// Open a GGUF model through CrispASR.
    ///
    /// # Returns
    ///
    /// An initialized transcription engine.
    ///
    /// # Errors
    ///
    /// Returns an error if the model path is not a file, is not UTF-8, or
    /// CrispASR cannot load the model.
    pub fn open<P: AsRef<Path>>(model_path: P) -> Result<Self> {
        let path = model_path.as_ref();
        if !path.is_file() {
            return Err(anyhow::anyhow!(
                "model path is not a file: {}",
                path.display()
            ));
        }
        let path_str = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("model path is not valid UTF-8"))?;
        let session = crispasr::Session::open(path_str)
            .map_err(|e| anyhow::anyhow!("crispasr open failed: {e}"))
            .with_context(|| format!("failed to open model {}", path_str))?;
        Ok(Self { session })
    }

    /// Transcribe 16 kHz mono PCM samples.
    ///
    /// # Returns
    ///
    /// The concatenated transcript text from all returned segments.
    ///
    /// # Errors
    ///
    /// Returns an error if CrispASR rejects the audio or inference fails.
    pub fn transcribe(&self, pcm: &[f32]) -> Result<String> {
        let pcm = pad_short_pcm(pcm);
        let segments = self
            .session
            .transcribe(pcm.as_ref())
            .map_err(|e| anyhow::anyhow!("crispasr transcribe failed: {e}"))?;
        let mut out = String::new();
        for seg in segments {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
            out.push_str(seg.text.trim());
        }
        Ok(out)
    }
}

fn pad_short_pcm(pcm: &[f32]) -> Cow<'_, [f32]> {
    if pcm.len() >= MIN_INFERENCE_SAMPLES {
        return Cow::Borrowed(pcm);
    }

    let mut padded = Vec::with_capacity(MIN_INFERENCE_SAMPLES);
    padded.extend_from_slice(pcm);
    padded.resize(MIN_INFERENCE_SAMPLES, 0.0);
    Cow::Owned(padded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_pcm_is_padded_with_silence() {
        let padded = pad_short_pcm(&[0.25, -0.25]);
        assert_eq!(padded.len(), MIN_INFERENCE_SAMPLES);
        assert_eq!(&padded[..2], &[0.25, -0.25]);
        assert!(padded[2..].iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn long_pcm_is_borrowed_without_copying() {
        let pcm = vec![0.0; MIN_INFERENCE_SAMPLES];
        assert!(matches!(pad_short_pcm(&pcm), Cow::Borrowed(_)));
    }

    #[test]
    fn missing_model_path_fails_before_crispasr() {
        let err = match Engine::open("target/tmp/definitely-missing-parakit-model.gguf") {
            Ok(_) => panic!("missing model path should fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("model path is not a file"));
    }
}
