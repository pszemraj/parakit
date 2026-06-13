//! Inference wrapper around a `crispasr::Session`.

use crate::constants::TARGET_RATE;
use crate::crispasr_ext::OwnedSession;
use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use crispasr::SessionSegment;
use std::borrow::Cow;
use std::path::Path;

/// Minimum PCM length sent to CrispASR.
///
/// Very short captures can collapse to too few feature frames for the model
/// pipeline. Right-padding with silence keeps the hotkey behavior predictable
/// without dropping the user's utterance.
const MIN_INFERENCE_SAMPLES: usize = TARGET_RATE as usize;

/// Runtime CPU/GPU selection requested by the user.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum DeviceMode {
    /// Keep CrispASR's default: use the best GPU when one is available,
    /// otherwise fall back to CPU.
    #[default]
    Auto,
    /// Force CrispASR to open the session without a GPU backend.
    Cpu,
    /// Require the GPU path. Device availability is validated by callers
    /// that can use the bundled ggml probe.
    Gpu,
}

impl DeviceMode {
    /// Stable CLI/log label.
    ///
    /// # Returns
    ///
    /// `auto`, `cpu`, or `gpu`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
        }
    }

    fn use_gpu_override(self) -> Option<bool> {
        match self {
            Self::Auto => None,
            Self::Cpu => Some(false),
            Self::Gpu => Some(true),
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
    session: EngineSession,
    backend: String,
    threads: usize,
    device_mode: DeviceMode,
}

enum EngineSession {
    Auto(crispasr::Session),
    WithParams(OwnedSession),
}

impl EngineSession {
    fn backend(&self) -> String {
        match self {
            Self::Auto(session) => session.backend(),
            Self::WithParams(session) => session.backend(),
        }
    }

    fn transcribe(&self, pcm: &[f32]) -> Result<Vec<SessionSegment>, String> {
        match self {
            Self::Auto(session) => session.transcribe(pcm),
            Self::WithParams(session) => session.transcribe(pcm),
        }
    }
}

impl Engine {
    /// Open a GGUF model with a requested CPU thread count and device mode.
    ///
    /// # Arguments
    ///
    /// * `model_path` - GGUF model file to load.
    /// * `threads` - CPU inference thread count requested from CrispASR.
    /// * `device_mode` - CPU/GPU behavior to request at session open.
    ///
    /// # Returns
    ///
    /// An initialized transcription engine.
    ///
    /// # Errors
    ///
    /// Returns an error if the model path is not a file, is not UTF-8, the
    /// thread count is zero, or CrispASR cannot load the model.
    pub fn open<P: AsRef<Path>>(
        model_path: P,
        threads: usize,
        device_mode: DeviceMode,
    ) -> Result<Self> {
        if threads == 0 {
            return Err(anyhow::anyhow!("thread count must be at least 1"));
        }
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

        #[cfg(target_os = "windows")]
        apply_windows_cpu_runtime_env_defaults(threads);

        let detected_backend = crispasr::Session::detect_backend(path_str)
            .map_err(|e| anyhow::anyhow!("crispasr backend detection failed: {e}"))
            .with_context(|| format!("failed to detect backend for model {}", path_str))?;
        let backend = validate_detected_backend(detected_backend)
            .with_context(|| format!("failed to detect backend for model {}", path_str))?;
        let session = match device_mode.use_gpu_override() {
            Some(use_gpu) => EngineSession::WithParams(
                OwnedSession::open_with_params(path_str, &backend, threads, use_gpu)
                    .map_err(|e| anyhow::anyhow!("crispasr open failed: {e}"))?,
            ),
            None => EngineSession::Auto(
                crispasr::Session::open_with_backend(path_str, &backend, threads as i32)
                    .map_err(|e| anyhow::anyhow!("crispasr open failed: {e}"))?,
            ),
        };
        let backend = session.backend();
        Ok(Self {
            session,
            backend,
            threads,
            device_mode,
        })
    }

    /// Return the CrispASR backend used for this model.
    ///
    /// # Returns
    ///
    /// A backend label such as `parakeet`, or `unknown` if CrispASR did not
    /// report one.
    pub fn backend(&self) -> &str {
        if self.backend.is_empty() {
            "unknown"
        } else {
            &self.backend
        }
    }

    /// Return the requested inference thread count.
    ///
    /// # Returns
    ///
    /// The thread count passed to CrispASR when opening the session.
    pub fn threads(&self) -> usize {
        self.threads
    }

    /// Return the requested runtime device mode.
    ///
    /// # Returns
    ///
    /// The device mode passed when opening the engine.
    pub fn device_mode(&self) -> DeviceMode {
        self.device_mode
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
        Ok(join_segment_text(segments))
    }
}

fn join_segment_text(segments: Vec<SessionSegment>) -> String {
    let mut out = String::new();
    for seg in segments {
        if !out.is_empty() && !out.ends_with(' ') {
            out.push(' ');
        }
        out.push_str(seg.text.trim());
    }
    out
}

fn validate_detected_backend(backend: String) -> Result<String> {
    let backend = backend.trim();
    if backend.is_empty() {
        bail!("crispasr backend detection returned an empty backend");
    }
    Ok(backend.to_string())
}

/// Return the default CPU thread count for inference.
///
/// # Returns
///
/// A conservative count based on OS parallelism, falling back to two.
pub fn default_thread_count() -> usize {
    let available = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4);
    recommended_thread_count(available)
}

/// Convert available logical parallelism into an interactive-daemon default.
///
/// # Returns
///
/// Roughly half the available logical CPUs, with guards for small machines.
/// Explicit `--threads` values bypass this default.
///
/// # Panics
///
/// This function does not panic because the clamp bounds are fixed and valid.
pub fn recommended_thread_count(available_threads: usize) -> usize {
    let available_threads = available_threads.max(1);
    if available_threads == 1 {
        return 1;
    }
    (available_threads / 2).max(2)
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

#[cfg(target_os = "windows")]
fn apply_windows_cpu_runtime_env_defaults(threads: usize) {
    for default in windows_cpu_runtime_env_defaults(threads, |key| std::env::var_os(key).is_some())
    {
        std::env::set_var(default.key, default.value);
    }
}

#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Eq, PartialEq)]
struct RuntimeEnvDefault {
    key: &'static str,
    value: String,
}

#[cfg(any(target_os = "windows", test))]
fn windows_cpu_runtime_env_defaults(
    threads: usize,
    mut is_set: impl FnMut(&str) -> bool,
) -> Vec<RuntimeEnvDefault> {
    [
        RuntimeEnvDefault {
            key: "OMP_NUM_THREADS",
            value: threads.to_string(),
        },
        RuntimeEnvDefault {
            key: "OPENBLAS_NUM_THREADS",
            value: "1".to_string(),
        },
        RuntimeEnvDefault {
            key: "OMP_WAIT_POLICY",
            value: "PASSIVE".to_string(),
        },
    ]
    .into_iter()
    .filter(|default| !is_set(default.key))
    .collect()
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
    fn device_mode_labels_are_stable() {
        assert_eq!(DeviceMode::Auto.as_str(), "auto");
        assert_eq!(DeviceMode::Cpu.as_str(), "cpu");
        assert_eq!(DeviceMode::Gpu.as_str(), "gpu");
    }

    #[test]
    fn segment_text_is_joined_with_single_spaces() {
        let segments = vec![
            SessionSegment {
                text: " hello ".to_string(),
                start: 0.0,
                end: 0.5,
                words: Vec::new(),
            },
            SessionSegment {
                text: "world".to_string(),
                start: 0.5,
                end: 1.0,
                words: Vec::new(),
            },
        ];

        assert_eq!(join_segment_text(segments), "hello world");
    }

    #[test]
    fn missing_model_path_fails_before_crispasr() {
        let err = match Engine::open(
            "target/tmp/definitely-missing-parakit-model.gguf",
            1,
            DeviceMode::Auto,
        ) {
            Ok(_) => panic!("missing model path should fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("model path is not a file"));
    }

    #[test]
    fn recommended_threads_are_valid_and_scale() {
        for (available, expected) in [
            (0, 1),
            (1, 1),
            (2, 2),
            (3, 2),
            (4, 2),
            (8, 4),
            (12, 6),
            (16, 8),
            (32, 16),
            (64, 32),
        ] {
            assert_eq!(recommended_thread_count(available), expected);
        }
    }

    #[test]
    fn empty_detected_backend_is_rejected() {
        let err = validate_detected_backend("  ".to_string()).expect_err("empty backend fails");

        assert!(err
            .to_string()
            .contains("backend detection returned an empty backend"));
    }

    #[test]
    fn runtime_env_defaults_only_fill_unset_variables() {
        let existing = ["OPENBLAS_NUM_THREADS"];
        let defaults =
            windows_cpu_runtime_env_defaults(8, |key| existing.iter().any(|entry| entry == &key));

        assert_eq!(
            defaults,
            vec![
                RuntimeEnvDefault {
                    key: "OMP_NUM_THREADS",
                    value: "8".to_string(),
                },
                RuntimeEnvDefault {
                    key: "OMP_WAIT_POLICY",
                    value: "PASSIVE".to_string(),
                },
            ]
        );
    }
}
