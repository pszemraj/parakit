//! Model inference through one owned CrispASR session.

use crate::constants::TARGET_RATE;
use crate::crispasr_ext::{crispasr_thread_count, OwnedSession};
use crate::model::validate_model_file;
use crate::qwen::QwenOptions;
use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use std::borrow::Cow;
use std::path::Path;

/// Minimum PCM length sent to CrispASR.
///
/// Very short captures can collapse to too few feature frames for the model
/// pipeline. Right-padding with silence keeps the hotkey behavior predictable
/// without dropping the user's utterance.
const MIN_INFERENCE_SAMPLES: usize = TARGET_RATE as usize;

/// Initialize Parakit's CrispASR defaults without replacing overrides.
///
/// Disables extra pointwise-Q8 and fused-QKV weight copies and enables windowed
/// Qwen audio attention. Library callers
/// can use this initializer at startup or provide these environment variables
/// when launching their process; `Engine::open` does not mutate them. On
/// Windows the defaults are also written to the C runtime's environment table.
///
/// # Safety
///
/// Call before starting any threads and before native model initialization.
/// Native code can read the process environment and caches these controls.
pub unsafe fn initialize_crispasr_runtime_defaults() {
    for (key, value) in [
        ("CRISPASR_FC_PW_Q8", "0"),
        ("CRISPASR_FC_FUSED_QKV", "0"),
        ("CRISP_AUDIO_WINDOWED_ATTN", "1"),
    ] {
        if std::env::var_os(key).is_none() {
            std::env::set_var(key, value);
            #[cfg(target_os = "windows")]
            set_windows_crt_env_default(key, value);
        }
    }
}

/// Mirror a startup default into the Windows C runtime's `getenv` table.
///
/// The C runtime copies the environment before Rust's `main`, and
/// `std::env::set_var` updates only the process environment block. CrispASR
/// reads these controls with `getenv`, which uses the C runtime copy; the
/// MSVC build shares that runtime with CrispASR through `/MD`.
#[cfg(target_os = "windows")]
fn set_windows_crt_env_default(key: &str, value: &str) {
    if let (Ok(key), Ok(value)) = (std::ffi::CString::new(key), std::ffi::CString::new(value)) {
        // SAFETY: Both arguments are NUL-terminated and `_putenv_s` copies
        // them. Callers uphold the single-threaded startup contract above.
        unsafe { libc::putenv_s(key.as_ptr(), value.as_ptr()) };
    }
}

/// Runtime CPU/GPU selection requested by the user.
#[derive(
    Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
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
}

/// Thin wrapper so the rest of the code never touches `crispasr` directly.
///
/// Owned exclusively by the worker thread. The native session is `Send`
/// (we can move it across threads at startup) but not `Sync` (we can't
/// hand out `&Engine` from multiple threads). The architecture respects
/// that: only the worker thread ever calls `transcribe`.
pub struct Engine {
    session: OwnedSession,
    backend: String,
    threads: usize,
    device_mode: DeviceMode,
    qwen: Option<QwenOptions>,
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
    /// Returns an error if the thread count is zero or above `i32::MAX`, the model path is not a file or is not UTF-8, or CrispASR cannot load the model.
    pub fn open<P: AsRef<Path>>(
        model_path: P,
        threads: usize,
        device_mode: DeviceMode,
    ) -> Result<Self> {
        Self::open_with_options(model_path, threads, device_mode, &QwenOptions::default())
    }

    /// Open a GGUF model, applying Qwen controls when its backend is `qwen3`.
    ///
    /// # Arguments
    ///
    /// * `model_path` - GGUF model file to load.
    /// * `threads` - CPU inference thread count requested from CrispASR.
    /// * `device_mode` - CPU/GPU behavior to request at session open.
    /// * `options` - Language and generation controls; ignored for other backends.
    ///
    /// # Returns
    ///
    /// An initialized transcription engine with its session controls applied.
    ///
    /// # Errors
    ///
    /// Returns the model/thread errors of [`Self::open`], or an error when Qwen
    /// controls are invalid or rejected by the native session.
    pub fn open_with_options<P: AsRef<Path>>(
        model_path: P,
        threads: usize,
        device_mode: DeviceMode,
        options: &QwenOptions,
    ) -> Result<Self> {
        if threads == 0 {
            return Err(anyhow::anyhow!("thread count must be at least 1"));
        }
        // Every device mode passes the same checked count, so an out-of-range value fails here instead of wrapping negative.
        let n_threads = crispasr_thread_count(threads).map_err(anyhow::Error::msg)?;
        let path = model_path.as_ref();
        validate_model_file(path)?;
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
        if backend == "qwen3" {
            options.validate().map_err(anyhow::Error::msg)?;
        }
        let session = OwnedSession::open_with_params(
            path_str,
            &backend,
            n_threads,
            device_mode != DeviceMode::Cpu,
        )
        .map_err(|e| anyhow::anyhow!("crispasr open failed: {e}"))?;
        if backend == "qwen3" {
            let language = options.normalized_language().map_err(anyhow::Error::msg)?;
            configure_qwen(
                &session,
                language.as_deref().unwrap_or(""),
                options.max_new_tokens as i32,
            )
            .map_err(anyhow::Error::msg)
            .context("could not configure Qwen inference")?;
        }
        let backend = session.backend();
        let qwen = (backend == "qwen3").then(|| options.clone());
        Ok(Self {
            session,
            backend,
            threads,
            device_mode,
            qwen,
        })
    }

    /// Transcribe Qwen readiness speech with temporary automatic/512 options.
    ///
    /// # Arguments
    /// * `pcm` - Embedded human speech at 16 kHz.
    ///
    /// # Returns
    /// Success after readiness and restoration of the user's options.
    ///
    /// # Errors
    /// Reports inference, configuration, or option-restoration errors.
    pub(crate) fn transcribe_readiness(&self, pcm: &[f32]) -> Result<()> {
        let options = self.qwen.as_ref().context("readiness requires Qwen")?;
        let language = options.normalized_language().map_err(anyhow::Error::msg)?;
        let restore = QwenOptionRestore {
            session: &self.session,
            language: language.unwrap_or_default(),
            budget: options.max_new_tokens as i32,
            active: true,
        };
        let result = configure_qwen(&self.session, "", 512)
            .map_err(anyhow::Error::msg)
            .and_then(|()| self.transcribe(pcm).map(|_| ()));
        restore
            .restore()
            .context("restore Qwen options after readiness")?;
        result
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

    /// Return the resolved Qwen controls, absent for other backends.
    ///
    /// # Returns
    /// The user options restored after every readiness probe.
    pub fn qwen_options(&self) -> Option<&QwenOptions> {
        self.qwen.as_ref()
    }

    /// Read the version reported by the linked native C API.
    ///
    /// # Returns
    /// A version string, or `None` when the native API provides none.
    pub fn native_api_version(&self) -> Option<String> {
        let version = unsafe { crispasr_sys::crispasr_c_api_version() };
        if version.is_null() {
            None
        } else {
            Some(
                unsafe { std::ffi::CStr::from_ptr(version) }
                    .to_string_lossy()
                    .into_owned(),
            )
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
        self.session
            .transcribe(pcm.as_ref())
            .map(join_segment_text)
            .map_err(|e| anyhow::anyhow!("crispasr transcribe failed: {e}"))
    }
}

/// Apply Qwen's language hint before its validated generation budget.
fn configure_qwen(
    session: &OwnedSession,
    language: &str,
    max_new_tokens: i32,
) -> Result<(), String> {
    session.set_source_language(language)?;
    session.set_max_new_tokens(max_new_tokens)
}

struct QwenOptionRestore<'a> {
    session: &'a OwnedSession,
    language: String,
    budget: i32,
    active: bool,
}

impl QwenOptionRestore<'_> {
    fn restore(mut self) -> Result<()> {
        self.active = false;
        configure_qwen(self.session, &self.language, self.budget).map_err(anyhow::Error::msg)
    }
}

impl Drop for QwenOptionRestore<'_> {
    fn drop(&mut self) {
        if self.active {
            let _ = configure_qwen(self.session, &self.language, self.budget);
        }
    }
}

fn join_segment_text(segments: impl IntoIterator<Item = String>) -> String {
    let mut out = String::new();
    for seg in segments {
        if !out.is_empty() && !out.ends_with(' ') {
            out.push(' ');
        }
        out.push_str(seg.trim());
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

/// Largest CPU thread count parakit accepts. CrispASR's session-open entry points take a C `int`, so config load and `--threads` reject larger values before any model is fetched or opened.
pub const MAX_THREADS: usize = std::ffi::c_int::MAX as usize;

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
/// # Arguments
///
/// * `available_threads` - Logical CPUs the process may use; 0 is treated as 1.
///
/// # Returns
///
/// Roughly half the available logical CPUs, with guards for small machines.
/// Explicit `--threads` values bypass this default.
///
/// # Panics
///
/// Does not panic. The only division is by the constant 2.
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
    fn short_and_empty_pcm_are_padded_with_silence() {
        let padded = pad_short_pcm(&[0.25, -0.25]);
        assert_eq!(padded.len(), MIN_INFERENCE_SAMPLES);
        assert_eq!(&padded[..2], &[0.25, -0.25]);
        assert!(padded[2..].iter().all(|sample| *sample == 0.0));

        let empty = pad_short_pcm(&[]);
        assert_eq!(empty.len(), MIN_INFERENCE_SAMPLES);
        assert!(empty.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn long_pcm_is_borrowed_without_copying() {
        let pcm = vec![0.0; MIN_INFERENCE_SAMPLES];
        assert!(matches!(pad_short_pcm(&pcm), Cow::Borrowed(_)));
    }

    #[test]
    fn device_mode_labels_are_stable() {
        // Exhaustive over ValueEnum::value_variants() with no wildcard arm:
        // adding a DeviceMode variant without a matching label here is a
        // compile error, not a silently-missed test case. Mirrors the
        // value_variants() sweep pattern in config.rs's
        // assert_serde_matches_clap.
        for device in DeviceMode::value_variants() {
            let expected = match device {
                DeviceMode::Auto => "auto",
                DeviceMode::Cpu => "cpu",
                DeviceMode::Gpu => "gpu",
            };
            assert_eq!(device.as_str(), expected, "{device:?}");
        }
    }

    #[test]
    fn segment_text_is_joined_with_single_spaces() {
        let segments = vec![" hello ".to_string(), "world".to_string()];

        assert_eq!(join_segment_text(segments), "hello world");
    }

    #[test]
    fn segment_text_preserves_empty_segment_spacing() {
        let cases: &[(&[&str], &str)] = &[
            (&[], ""),
            (&["", " \t\n"], ""),
            (&["", " hello", "", "\t", "world\n"], "hello world"),
            (&["hello", "", " \t"], "hello "),
            (&[" \u{2003}hello  world\n"], "hello  world"),
        ];
        for &(segments, expected) in cases {
            assert_eq!(
                join_segment_text(segments.iter().map(|text| (*text).to_string())),
                expected,
                "{segments:?}"
            );
        }
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
    fn crispasr_thread_count_accepts_the_c_int_range_only() {
        let max = usize::try_from(i32::MAX).unwrap();
        assert_eq!(crispasr_thread_count(1), Ok(1));
        assert_eq!(crispasr_thread_count(max), Ok(i32::MAX));
        assert!(crispasr_thread_count(max + 1).is_err());
    }

    #[test]
    fn out_of_range_thread_count_is_rejected_in_every_device_mode() {
        let too_many = usize::try_from(i32::MAX).unwrap() + 1;
        for device in DeviceMode::value_variants() {
            let err = match Engine::open(
                "target/tmp/definitely-missing-parakit-model.gguf",
                too_many,
                *device,
            ) {
                Ok(_) => panic!("{device:?}: an out-of-range thread count must fail"),
                Err(err) => err,
            };
            assert!(
                err.to_string()
                    .contains("thread count is too large for CrispASR"),
                "{device:?}: {err:#}"
            );
        }
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
