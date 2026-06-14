//! Local extensions over the pinned CrispASR Rust bindings.
//!
//! The vendored C API exports `crispasr_session_open_with_params`, but the
//! `crispasr-sys` Rust bindings have not exposed it yet. This module keeps the
//! raw ABI use narrow so parakit can choose CPU or GPU at session open without
//! modifying the submodule.

use crate::ffi_util::c_string_lossy;
use crispasr::{SessionSegment, SessionWord};
use std::ffi::CString;
use std::os::raw::{c_char, c_float, c_int};

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

extern "C" {
    fn crispasr_session_open_with_params(
        model_path: *const c_char,
        backend_name: *const c_char,
        params: *const CrispAsrOpenParamsV1,
    ) -> *mut crispasr_sys::CrispasrSession;
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
    /// * `threads` - CPU thread count for CrispASR.
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
        threads: usize,
        use_gpu: bool,
    ) -> Result<Self, String> {
        let path = CString::new(model_path).map_err(|err| format!("invalid path: {err}"))?;
        let backend =
            CString::new(backend).map_err(|err| format!("invalid backend name: {err}"))?;
        let n_threads = c_int::try_from(threads)
            .map_err(|_| format!("thread count is too large for CrispASR: {threads}"))?;
        let params = CrispAsrOpenParamsV1 {
            abi_version: 2,
            n_threads,
            use_gpu: c_int::from(use_gpu),
            verbosity: 0,
            flash_attn: 1,
            n_gpu_layers: -1,
            reserved: [0; 6],
        };
        let handle =
            unsafe { crispasr_session_open_with_params(path.as_ptr(), backend.as_ptr(), &params) };
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

    /// Transcribe 16 kHz mono PCM through the raw session.
    ///
    /// # Returns
    ///
    /// CrispASR session segments.
    ///
    /// # Errors
    ///
    /// Returns a string error if the sample count exceeds the C ABI range or
    /// CrispASR returns a null result handle.
    ///
    /// # Panics
    ///
    /// Panics only if allocating the returned segment or word vectors fails.
    pub(crate) fn transcribe(&self, pcm: &[f32]) -> Result<Vec<SessionSegment>, String> {
        if pcm.is_empty() {
            return Ok(Vec::new());
        }
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
                let text = c_string_lossy(crispasr_sys::crispasr_session_result_segment_text(
                    result.0,
                    segment_index,
                ))
                .trim()
                .to_string();
                let start =
                    crispasr_sys::crispasr_session_result_segment_t0(result.0, segment_index)
                        as f64
                        / 100.0;
                let end = crispasr_sys::crispasr_session_result_segment_t1(result.0, segment_index)
                    as f64
                    / 100.0;

                let n_words =
                    crispasr_sys::crispasr_session_result_n_words(result.0, segment_index);
                let n_words = n_words.max(0);
                let mut words = Vec::with_capacity(usize::try_from(n_words).unwrap_or(0));
                for word_index in 0..n_words {
                    let confidence = crispasr_sys::crispasr_session_result_word_p(
                        result.0,
                        segment_index,
                        word_index,
                    );
                    words.push(SessionWord {
                        text: c_string_lossy(crispasr_sys::crispasr_session_result_word_text(
                            result.0,
                            segment_index,
                            word_index,
                        )),
                        start: crispasr_sys::crispasr_session_result_word_t0(
                            result.0,
                            segment_index,
                            word_index,
                        ) as f64
                            / 100.0,
                        end: crispasr_sys::crispasr_session_result_word_t1(
                            result.0,
                            segment_index,
                            word_index,
                        ) as f64
                            / 100.0,
                        confidence: if confidence < 0.0 { 1.0 } else { confidence },
                    });
                }
                segments.push(SessionSegment {
                    text,
                    start,
                    end,
                    words,
                });
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
