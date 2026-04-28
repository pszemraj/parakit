//! Inference wrapper around a `crispasr::Session`.

use anyhow::{Context, Result};
use std::path::Path;

#[derive(Clone, Copy, Debug)]
pub enum Mode {
    Batch,
    Streaming { chunk_secs: f32 },
}

impl Mode {
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
    pub fn open<P: AsRef<Path>>(model_path: P) -> Result<Self> {
        let path_str = model_path
            .as_ref()
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("model path is not valid UTF-8"))?;
        let session = crispasr::Session::open(path_str)
            .map_err(|e| anyhow::anyhow!("crispasr open failed: {e}"))
            .with_context(|| format!("failed to open model {}", path_str))?;
        Ok(Self { session })
    }

    pub fn transcribe(&self, pcm: &[f32]) -> Result<String> {
        let segments = self
            .session
            .transcribe(pcm)
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
