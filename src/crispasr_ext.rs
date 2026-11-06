//! Local extensions over the pinned CrispASR Rust bindings.
//!
//! The safe wrapper does not expose parameterized session open. This module
//! keeps the raw ABI use narrow so parakit can choose CPU or GPU at session
//! open without modifying the submodule.

use crate::ffi_util::c_string_lossy;
use std::ffi::CString;
use std::os::raw::{c_float, c_int};

#[repr(C)]
struct CrispAsrOpenParamsV1 {
    abi_version: c_int,
    n_threads: c_int,
    use_gpu: c_int,
    verbosity: c_int,
    flash_attn: c_int,
    n_gpu_layers: c_int,
    reserved: [c_int; 6],
}

/// Convert a thread count to the C `int` that every CrispASR session-open entry point takes.
///
/// # Arguments
///
/// * `threads` - Requested CPU thread count.
///
/// # Returns
///
/// `threads` as a C `int`.
///
/// # Errors
///
/// Returns an error when `threads` exceeds `c_int::MAX`, so an out-of-range count is rejected instead of wrapping to a negative value.
pub(crate) fn crispasr_thread_count(threads: usize) -> Result<c_int, String> {
    c_int::try_from(threads)
        .map_err(|_| format!("thread count is too large for CrispASR: {threads}"))
}

/// Raw CrispASR session opened through `crispasr_session_open_with_params`.
pub(crate) struct OwnedSession {
    handle: *mut crispasr_sys::CrispasrSession,
}

// Not `Sync`: the engine owns and uses the session on one worker thread.
unsafe impl Send for OwnedSession {}

impl OwnedSession {
    /// Open a CrispASR session with explicit CPU/GPU parameters.
    ///
    /// # Arguments
    ///
    /// * `model_path` - GGUF model path passed to CrispASR.
    /// * `backend` - Backend label detected from the GGUF metadata.
    /// * `n_threads` - CPU thread count for CrispASR, from [`crispasr_thread_count`].
    /// * `use_gpu` - Whether CrispASR should enable GPU use during open.
    ///
    /// # Returns
    ///
    /// An owned raw session handle.
    ///
    /// # Errors
    ///
    /// Returns a string error if arguments cannot be represented for the C ABI
    /// or CrispASR returns a null session handle.
    pub(crate) fn open_with_params(
        model_path: &str,
        backend: &str,
        n_threads: c_int,
        use_gpu: bool,
    ) -> Result<Self, String> {
        let path = CString::new(model_path).map_err(|err| format!("invalid path: {err}"))?;
        let backend =
            CString::new(backend).map_err(|err| format!("invalid backend name: {err}"))?;
        let params = CrispAsrOpenParamsV1 {
            abi_version: 2,
            n_threads,
            use_gpu: c_int::from(use_gpu),
            verbosity: 0,
            flash_attn: 1,
            n_gpu_layers: -1,
            reserved: [0; 6],
        };
        let handle = unsafe {
            crispasr_sys::crispasr_session_open_with_params(
                path.as_ptr(),
                backend.as_ptr(),
                std::ptr::from_ref(&params).cast(),
            )
        };
        if handle.is_null() {
            let available = crispasr::Session::available_backends().join(",");
            return Err(format!(
                "Failed to open {model_path:?}. Library was built with: [{available}]"
            ));
        }

        Ok(Self { handle })
    }

    /// Return the backend label reported by CrispASR.
    ///
    /// # Returns
    ///
    /// A backend name such as `parakeet`, or an empty string if CrispASR
    /// returned a null backend pointer.
    pub(crate) fn backend(&self) -> String {
        let ptr = unsafe { crispasr_sys::crispasr_session_backend(self.handle) };
        c_string_lossy(ptr)
    }

    /// Set or clear the source-language hint, checking the native result.
    ///
    /// # Arguments
    ///
    /// * `language` - Canonical language name, or an empty string to clear the hint.
    ///
    /// # Returns
    ///
    /// Success after the native session accepts the hint.
    ///
    /// # Errors
    ///
    /// Returns an error for an interior NUL or a native setter failure.
    pub(crate) fn set_source_language(&self, language: &str) -> Result<(), String> {
        let language = CString::new(language).map_err(|err| err.to_string())?;
        let rc = unsafe {
            crispasr_sys::crispasr_session_set_source_language(self.handle, language.as_ptr())
        };
        if rc != 0 {
            return Err(format!("set_source_language failed (rc={rc})"));
        }
        Ok(())
    }

    /// Set the validated generated-token budget, checking the native result.
    ///
    /// # Arguments
    ///
    /// * `max_new_tokens` - Budget already validated against Qwen's accepted range.
    ///
    /// # Returns
    ///
    /// Success after the native session accepts the budget.
    ///
    /// # Errors
    ///
    /// Returns an error when the native setter fails.
    pub(crate) fn set_max_new_tokens(&self, max_new_tokens: c_int) -> Result<(), String> {
        let rc = unsafe {
            crispasr_sys::crispasr_session_set_max_new_tokens(self.handle, max_new_tokens)
        };
        if rc != 0 {
            return Err(format!("set_max_new_tokens failed (rc={rc})"));
        }
        Ok(())
    }

    /// Set Qwen context text, or clear it with an empty string.
    ///
    /// # Arguments
    /// * `text` - UTF-8 text retained by the native session until replaced.
    ///
    /// # Returns
    /// Success after the native session accepts the text.
    ///
    /// # Errors
    /// Returns an error for an interior NUL or a native setter failure.
    pub(crate) fn set_hotwords(&self, text: &str) -> Result<(), String> {
        let text = CString::new(text).map_err(|err| err.to_string())?;
        let rc =
            unsafe { crispasr_sys::crispasr_session_set_hotwords(self.handle, text.as_ptr(), 1.0) };
        if rc != 0 {
            return Err(format!("set_hotwords failed (rc={rc})"));
        }
        Ok(())
    }

    /// Transcribe 16 kHz mono PCM through the raw session.
    ///
    /// # Returns
    ///
    /// Copied text from each CrispASR session segment.
    ///
    /// # Errors
    ///
    /// Returns a string error if the sample count exceeds the C ABI range or
    /// CrispASR returns a null result handle.
    ///
    /// # Panics
    ///
    /// Panics only if allocating the returned segment texts fails.
    pub(crate) fn transcribe(&self, pcm: &[f32]) -> Result<Vec<String>, String> {
        let n_samples = c_int::try_from(pcm.len())
            .map_err(|_| format!("audio buffer too large for CrispASR: {} samples", pcm.len()))?;
        let result = unsafe {
            crispasr_sys::crispasr_session_transcribe(
                self.handle,
                pcm.as_ptr().cast::<c_float>(),
                n_samples,
            )
        };
        if result.is_null() {
            return Err(format!(
                "crispasr_session_transcribe failed for backend {:?}",
                self.backend()
            ));
        }

        let result = OwnedResult(result);
        let mut segments = Vec::new();
        unsafe {
            let n_segments = crispasr_sys::crispasr_session_result_n_segments(result.0);
            for segment_index in 0..n_segments {
                segments.push(c_string_lossy(
                    crispasr_sys::crispasr_session_result_segment_text(result.0, segment_index),
                ));
            }
        }

        Ok(segments)
    }
}

impl Drop for OwnedSession {
    fn drop(&mut self) {
        unsafe {
            crispasr_sys::crispasr_session_close(self.handle);
        }
    }
}

struct OwnedResult(*mut crispasr_sys::CrispasrSessionResult);

impl Drop for OwnedResult {
    fn drop(&mut self) {
        unsafe {
            crispasr_sys::crispasr_session_result_free(self.0);
        }
    }
}
