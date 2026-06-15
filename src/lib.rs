//! Shared parakit modules used by the daemon and debugging tools.

/// WAV decoding and resampling helpers.
pub mod audio_file;
/// Build-time CrispASR and ggml diagnostics.
pub mod build_info;
/// File checksum helpers.
pub mod checksum;
/// Shared audio and model constants.
pub mod constants;
mod crispasr_ext;
/// Transcription log writer for raw/cleaned text pairs.
pub mod data_log;
/// Model acquisition pipeline for the official Parakeet checkpoint.
pub mod fetch;
mod ffi_util;
/// Minimal GGUF metadata reader for dtype reporting.
pub mod gguf;
/// Bundled ggml device diagnostics.
#[cfg(feature = "bundled")]
pub mod gpu;
/// CrispASR-backed transcription engine.
pub mod inference;
/// Canonical model names and cache paths.
pub mod model;
/// Regex-based transcript cleanup rules.
pub mod rules;
/// Synthetic warmup audio used by daemon and validation tools.
pub mod warmup;
