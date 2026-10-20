//! Shared parakit modules used by the daemon and debugging tools.

/// WAV decoding and resampling helpers.
pub mod audio_file;
/// Build-time CrispASR and ggml diagnostics.
pub mod build_info;
/// Digest helpers for explicit verification, cache keys, and fingerprints.
pub mod checksum;
/// Shared audio and model constants.
pub mod constants;
mod crispasr_ext;
/// Transcription log writer for raw/cleaned text pairs.
pub mod data_log;
/// Hosted model acquisition and original-checkpoint conversion/quantization.
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
/// Command output and temporary activity indicators.
pub mod output;
/// Qwen3-ASR language hints and generation controls.
pub mod qwen;
/// Regex-based transcript cleanup rules.
pub mod rules;
#[cfg(test)]
mod test_support;
/// Synthetic warmup audio used by daemon and validation tools.
pub mod warmup;
