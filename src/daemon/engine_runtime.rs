//! Shared startup/reload recipe; model resolution and downloads stay at startup.

use super::logging::Logger;
use anyhow::{Context, Result};
use parakit::inference::{DeviceMode, Engine};
use parakit::warmup::{self, engine_warmup_seconds};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Immutable session parameters resolved at daemon startup.
#[derive(Clone, Debug)]
pub(crate) struct EngineRecipe {
    /// Resolved model path, reused without fetching during recording.
    pub(crate) model_path: PathBuf,
    /// Requested CPU thread count.
    pub(crate) threads: usize,
    /// Requested device policy, revalidated on every reload.
    pub(crate) device_mode: DeviceMode,
    /// Whether native informational diagnostics should be emitted.
    pub(crate) verbose: bool,
}

impl EngineRecipe {
    /// Open and warm the model for initial startup readiness.
    ///
    /// # Returns
    ///
    /// The ready session and its device summary.
    ///
    /// # Errors
    ///
    /// Reports missing devices, model errors, and failed readiness inference.
    pub(crate) fn open(&self, log: &Logger) -> Result<(Engine, String)> {
        self.open_with_policy(log, LoadPolicy::Startup)
    }

    /// Reopen the resolved model and verify readiness with a short inference.
    ///
    /// # Returns
    ///
    /// The ready session and its device summary.
    ///
    /// # Errors
    ///
    /// Reports missing devices, model errors, and failed readiness inference.
    pub(crate) fn reload(&self, log: &Logger) -> Result<(Engine, String)> {
        self.open_with_policy(log, LoadPolicy::Reload)
    }

    fn open_with_policy(&self, log: &Logger, policy: LoadPolicy) -> Result<(Engine, String)> {
        // Startup is preflighted before a missing default model can trigger a
        // download. Reload has no resolver boundary, so it validates here.
        let mut device_mode = self.device_mode;
        if policy.requires_device_preflight() {
            validate_device_request(self.device_mode, log)?;
            device_mode = self.reload_device_mode(log)?;
        }
        let started = Instant::now();
        let engine = open_engine(
            &self.model_path,
            self.threads,
            device_mode,
            self.verbose,
            policy,
        )
        .with_context(|| format!("could not open model {}", self.model_path.display()))?;
        let (device_summary, has_gpu) = resolve_runtime_device(engine.device_mode());
        log.verbose(format!(
            "parakit: model opened in {:.0}ms with backend={} threads={} device={}",
            started.elapsed().as_secs_f32() * 1000.0,
            engine.backend(),
            engine.threads(),
            device_summary
        ));
        warm_up_engine(
            &engine,
            policy.warmup_seconds(engine.device_mode(), has_gpu),
            log,
        )?;
        Ok((engine, device_summary))
    }

    /// Choose the reload device from the GPU memory free right now.
    ///
    /// Other programs may fill the GPU while the model is offloaded, and ggml
    /// aborts the process when a session cannot grow its GPU buffers. `auto`
    /// therefore reloads on CPU until the next offload when the GPU lacks room
    /// for the weights plus a typical dictation's workspace.
    ///
    /// # Returns
    ///
    /// The device mode to open this session with.
    ///
    /// # Errors
    ///
    /// Reports `--device gpu` when the GPU lacks that room.
    fn reload_device_mode(&self, log: &Logger) -> Result<DeviceMode> {
        #[cfg(feature = "bundled")]
        {
            if self.device_mode == DeviceMode::Cpu {
                return Ok(DeviceMode::Cpu);
            }
            let devices = parakit::gpu::devices();
            let free_bytes = parakit::gpu::preferred_gpu_device_in(&devices)
                .filter(|device| device.total_bytes > 0)
                .map(|device| device.free_bytes as u64);
            let needed_bytes = std::fs::metadata(&self.model_path)
                .map_or(0, |metadata| metadata.len())
                + RELOAD_GPU_WORKSPACE_BYTES;
            let free_mib = free_bytes.unwrap_or_default() / 1_048_576;
            let needed_mib = needed_bytes / 1_048_576;
            match reload_device_for_free_memory(self.device_mode, free_bytes, needed_bytes) {
                Some(mode) if mode != self.device_mode => {
                    log.warn(format!(
                        "only {free_mib} MiB GPU memory free (about {needed_mib} MiB needed); reloading the model on CPU until the next offload"
                    ));
                    Ok(mode)
                }
                Some(mode) => Ok(mode),
                None => anyhow::bail!(
                    "only {free_mib} MiB GPU memory free; --device gpu needs about {needed_mib} MiB to reload the model. Free GPU memory, or set model_idle_minutes = 0 to keep the model resident"
                ),
            }
        }

        #[cfg(not(feature = "bundled"))]
        {
            let _ = log;
            Ok(self.device_mode)
        }
    }
}

/// GPU memory to keep free beyond the model weights when reloading: the
/// compute buffer and scratch pool of a dictation up to about a minute long.
#[cfg(feature = "bundled")]
const RELOAD_GPU_WORKSPACE_BYTES: u64 = 1 << 30;

/// Decide the reload device from free GPU memory.
///
/// # Arguments
///
/// * `requested` - Configured device policy.
/// * `free_bytes` - Free memory on the preferred GPU, when known.
/// * `needed_bytes` - Weights plus reload workspace.
///
/// # Returns
///
/// The device to reload on, or `None` when `--device gpu` lacks room.
#[cfg(feature = "bundled")]
fn reload_device_for_free_memory(
    requested: DeviceMode,
    free_bytes: Option<u64>,
    needed_bytes: u64,
) -> Option<DeviceMode> {
    match (requested, free_bytes) {
        (DeviceMode::Cpu, _) | (_, None) => Some(requested),
        (_, Some(free)) if free >= needed_bytes => Some(requested),
        (DeviceMode::Auto, Some(_)) => Some(DeviceMode::Cpu),
        (DeviceMode::Gpu, Some(_)) => None,
    }
}

#[derive(Clone, Copy)]
enum LoadPolicy {
    Startup,
    Reload,
}

impl LoadPolicy {
    fn requires_device_preflight(self) -> bool {
        matches!(self, Self::Reload)
    }

    fn warmup_seconds(self, device: DeviceMode, has_gpu: bool) -> &'static [usize] {
        match self {
            Self::Startup => engine_warmup_seconds(device, has_gpu),
            Self::Reload => warmup::reload_warmup_seconds(),
        }
    }
}

fn open_engine(
    path: &Path,
    threads: usize,
    device_mode: DeviceMode,
    verbose: bool,
    policy: LoadPolicy,
) -> Result<Engine> {
    if verbose {
        return Engine::open(path, threads, device_mode);
    }
    match policy {
        // Quiet startup discards native loader output; a mistaken model file
        // is reported once through the returned error, not low-level GGUF lines.
        LoadPolicy::Startup => {
            super::stderr::with_stderr_suppressed(|| Engine::open(path, threads, device_mode))
        }
        // Reload runs beside capture, cues, and IPC, so their errors must survive.
        LoadPolicy::Reload => Engine::open(path, threads, device_mode),
    }
}

/// Validate the explicit GPU requirement before loading model weights.
///
/// # Arguments
///
/// * `device_mode` - Requested CPU/automatic/GPU policy.
/// * `log` - Logger for builds without device probing.
///
/// # Returns
///
/// Success when the request is supported or cannot be preflighted.
///
/// # Errors
///
/// Reports an unavailable explicitly requested GPU in bundled builds.
pub(crate) fn validate_device_request(device_mode: DeviceMode, log: &Logger) -> Result<()> {
    if device_mode != DeviceMode::Gpu {
        return Ok(());
    }

    #[cfg(feature = "bundled")]
    {
        if !parakit::gpu::has_gpu_device() {
            let message = "--device gpu requested, but ggml reports no GPU or iGPU devices; run `parakit --verbose doctor` for compute diagnostics";
            #[cfg(target_os = "macos")]
            let message = crate::daemon::macos::no_gpu_hint()
                .map_or_else(|| message.to_string(), |hint| format!("{message}; {hint}"));
            anyhow::bail!(message);
        }
    }

    #[cfg(not(feature = "bundled"))]
    {
        log.warn(
            "--device gpu requested, but this build does not include the bundled ggml device probe; continuing without GPU preflight",
        );
    }

    let _ = log;
    Ok(())
}

fn resolve_runtime_device(device_mode: DeviceMode) -> (String, bool) {
    if device_mode == DeviceMode::Cpu {
        return (DeviceMode::Cpu.as_str().to_string(), false);
    }

    #[cfg(feature = "bundled")]
    {
        let devices = parakit::gpu::devices();
        let preferred = parakit::gpu::preferred_gpu_device_in(&devices);
        let summary = match preferred {
            Some(device) => format!("{} -> {}", device_mode.as_str(), device.diagnostic_line()),
            None if device_mode == DeviceMode::Auto => {
                "auto -> CPU fallback (no GPU/iGPU visible)".to_string()
            }
            None => "gpu -> unavailable (no GPU/iGPU visible)".to_string(),
        };
        (summary, preferred.is_some())
    }

    #[cfg(not(feature = "bundled"))]
    {
        (
            format!("{} (device probe unavailable)", device_mode.as_str()),
            false,
        )
    }
}

fn warm_up_engine(engine: &Engine, sequence: &[usize], log: &Logger) -> Result<()> {
    let started = Instant::now();
    for seconds in sequence {
        let warmup = warmup::synthetic_pcm(*seconds);
        engine
            .transcribe(&warmup)
            .context("engine warmup transcription failed")?;
    }
    log.verbose(format!(
        "parakit: engine warmup took {:.0}ms ({} synthetic input)",
        started.elapsed().as_secs_f32() * 1000.0,
        format_warmup_sequence(sequence)
    ));
    Ok(())
}

fn format_warmup_sequence(sequence: &[usize]) -> String {
    sequence
        .iter()
        .map(|seconds| format!("{seconds}s"))
        .collect::<Vec<_>>()
        .join(" + ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "bundled")]
    #[test]
    fn reload_falls_back_to_cpu_only_when_the_gpu_lacks_room() {
        const NEED: u64 = 2_000;
        for (requested, free, expected) in [
            (DeviceMode::Auto, Some(NEED), Some(DeviceMode::Auto)),
            (DeviceMode::Auto, Some(NEED - 1), Some(DeviceMode::Cpu)),
            (DeviceMode::Auto, None, Some(DeviceMode::Auto)),
            (DeviceMode::Gpu, Some(NEED), Some(DeviceMode::Gpu)),
            (DeviceMode::Gpu, Some(NEED - 1), None),
            (DeviceMode::Gpu, None, Some(DeviceMode::Gpu)),
            (DeviceMode::Cpu, Some(0), Some(DeviceMode::Cpu)),
        ] {
            assert_eq!(
                reload_device_for_free_memory(requested, free, NEED),
                expected,
                "{requested:?} with {free:?} free"
            );
        }
    }

    #[test]
    fn warmup_policy_uses_gpu_sequence_only_for_a_visible_gpu() {
        for mode in [DeviceMode::Cpu, DeviceMode::Auto, DeviceMode::Gpu] {
            for has_gpu in [false, true] {
                let startup: &[usize] = if mode != DeviceMode::Cpu && has_gpu {
                    &[5]
                } else {
                    &[1]
                };
                assert_eq!(LoadPolicy::Startup.warmup_seconds(mode, has_gpu), startup);
                assert_eq!(LoadPolicy::Reload.warmup_seconds(mode, has_gpu), &[1]);
            }
        }
    }

    #[test]
    fn only_reload_performs_device_preflight_at_open() {
        assert!(!LoadPolicy::Startup.requires_device_preflight());
        assert!(LoadPolicy::Reload.requires_device_preflight());
    }

    #[test]
    fn warmup_sequence_format_is_stable() {
        assert_eq!(format_warmup_sequence(&[5, 30]), "5s + 30s");
    }

    #[test]
    fn cpu_device_summary_is_plain() {
        let (summary, has_gpu) = resolve_runtime_device(DeviceMode::Cpu);
        assert_eq!(summary, "cpu");
        assert!(!has_gpu);
    }
}
