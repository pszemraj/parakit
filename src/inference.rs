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
/// Qwen audio attention and free-form Qwen context. Library callers
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
        ("CRISPASR_QWEN3_HOTWORDS_AS_CONTEXT", "1"),
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

/// Context actually supplied to inference for one completed capture.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ContextUsage {
    /// Unicode characters supplied to the first inference; zero for ordinary inference.
    pub chars: usize,
    /// Whether a contextual error, empty output, or copied output caused a second inference without context.
    pub retried: bool,
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
        self.transcribe_with_context(pcm, None)
            .map(|(text, _)| text)
    }

    /// Transcribe one capture with optional prior text for Qwen3-ASR.
    ///
    /// Call [`initialize_crispasr_runtime_defaults`] at process startup, or set
    /// `CRISPASR_QWEN3_HOTWORDS_AS_CONTEXT=1`, to route the text as raw context.
    /// Without that native setting, context is ignored.
    /// This method does not reject silent PCM; the daemon filters it before inference.
    ///
    /// # Arguments
    /// * `pcm` - Mono PCM at 16 kHz, padded when shorter than one second.
    /// * `context` - Prior dictation text; ignored by other model backends.
    ///
    /// # Returns
    /// Joined transcript text and the context actually supplied. Captures shorter
    /// than one second and unusable context fall back to ordinary inference.
    /// Contextual inference that fails, returns no text, or copies the context is
    /// retried once without context, including on longer captures; the usage keeps
    /// the initial context length. A retry that returns no text is returned as is.
    /// No retry follows a failure to clear the context.
    ///
    /// # Errors
    /// Reports inference failures, an ordinary retry failure (naming the contextual
    /// error when one triggered the retry), or failure to clear the session's context.
    pub fn transcribe_with_context(
        &self,
        pcm: &[f32],
        context: Option<&str>,
    ) -> Result<(String, ContextUsage)> {
        transcribe_scoped(
            &self.session,
            self.qwen.is_some(),
            raw_qwen_context_enabled(),
            pcm,
            context,
        )
    }
}

/// Native session operations behind the per-capture context fallback.
trait ContextSession {
    /// Set the session's raw context; empty text clears it.
    ///
    /// # Returns
    /// Success once the session holds the text.
    ///
    /// # Errors
    /// Reports text the session rejects.
    fn set_context(&self, text: &str) -> Result<(), String>;

    /// Run inference over the session's current context.
    ///
    /// # Returns
    /// The joined transcript text.
    ///
    /// # Errors
    /// Reports a failed native inference.
    fn infer(&self, pcm: &[f32]) -> Result<String>;
}

impl ContextSession for OwnedSession {
    fn set_context(&self, text: &str) -> Result<(), String> {
        self.set_hotwords(text)
    }

    fn infer(&self, pcm: &[f32]) -> Result<String> {
        self.transcribe(pcm)
            .map(join_segment_text)
            .map_err(|e| anyhow::anyhow!("crispasr transcribe failed: {e}"))
    }
}

/// Scope optional context to one inference and retry once without it when it misbehaves.
///
/// Context is used only on a Qwen session with raw context routing enabled and
/// at least [`MIN_INFERENCE_SAMPLES`] of audio. The native error cannot say why a
/// contextual call failed, so any contextual error, whitespace-only output, or
/// copied output is retried once without context. A failed clear leaves the
/// session state uncertain, so it is never followed by a retry.
fn transcribe_scoped<S: ContextSession>(
    session: &S,
    qwen: bool,
    raw_context: bool,
    pcm: &[f32],
    context: Option<&str>,
) -> Result<(String, ContextUsage)> {
    let mut context = context.filter(|text| {
        !text.is_empty() && qwen && raw_context && pcm.len() >= MIN_INFERENCE_SAMPLES
    });
    let restore = qwen.then(|| QwenContextRestore {
        session,
        active: true,
    });
    if restore.is_some() && session.set_context(context.unwrap_or_default()).is_err() {
        context = None;
        session.set_context("").map_err(anyhow::Error::msg)?;
    }
    let pcm = pad_short_pcm(pcm);
    let result = session.infer(pcm.as_ref());
    let cleared = restore.map_or(Ok(()), QwenContextRestore::clear);
    let mut usage = ContextUsage {
        chars: context.map_or(0, |prior| prior.chars().count()),
        retried: false,
    };
    let text = match (result, cleared) {
        (Err(error), Err(clear_error)) => {
            eprintln!("parakit: warning: failed to clear Qwen context: {clear_error:#}");
            return Err(error);
        }
        (Ok(_), Err(error)) => return Err(error.context("clear Qwen context after transcription")),
        (Ok(text), Ok(())) => match context {
            Some(prior) if text.trim().is_empty() || copies_context(&text, prior) => session
                .infer(pcm.as_ref())
                .context("ordinary inference retry after unusable Qwen context")?,
            _ => return Ok((text, usage)),
        },
        (Err(error), Ok(())) if context.is_none() => return Err(error),
        (Err(error), Ok(())) => session.infer(pcm.as_ref()).map_err(|retry| {
            retry.context(format!(
                "ordinary inference retry after Qwen context failed ({error:#})"
            ))
        })?,
    };
    usage.retried = true;
    Ok((text, usage))
}

/// Match the native runtime's environment table and integer flag semantics.
fn raw_qwen_context_enabled() -> bool {
    // Startup defaults precede native threads; inference only reads this value.
    // On Windows this also observes the CRT table used by CrispASR's getenv.
    unsafe {
        let value = libc::getenv(c"CRISPASR_QWEN3_HOTWORDS_AS_CONTEXT".as_ptr());
        !value.is_null() && libc::atoi(value) != 0
    }
}

/// Detect a whole short output or a five-word run copied from prior text.
fn copies_context(output: &str, context: &str) -> bool {
    let words = |text: &str| {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
    };
    let output = words(output);
    let context = words(context);
    let run = output.len().min(5);
    run != 0
        && context.len() >= run
        && output
            .windows(run)
            .any(|candidate| context.windows(run).any(|prior| candidate == prior))
}

/// Clear per-call context on early returns and unwinding.
struct QwenContextRestore<'a, S: ContextSession> {
    session: &'a S,
    active: bool,
}

impl<S: ContextSession> QwenContextRestore<'_, S> {
    fn clear(mut self) -> Result<()> {
        self.session.set_context("").map_err(anyhow::Error::msg)?;
        self.active = false;
        Ok(())
    }
}

impl<S: ContextSession> Drop for QwenContextRestore<'_, S> {
    fn drop(&mut self) {
        if self.active {
            let _ = self.session.set_context("");
        }
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
#[path = "inference_tests.rs"]
mod tests;
