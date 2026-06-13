//! Application entry point and top-level command dispatch for the `parakit` binary.

use anyhow::{Context, Result};
use clap::Parser;
use crossbeam_channel::{bounded, unbounded};
use parakit::audio_file::{read_wav_mono, resample_to_target};
use parakit::data_log::DataLogger;
use parakit::fetch::{self, FetchOptions, FetchSource};
use parakit::gguf;
use parakit::inference::{default_thread_count, DeviceMode, Engine};
use parakit::model;
use parakit::rules;
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::io::Read;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cli::{CacheCli, CacheCommand, Cli, Commands};
use crate::daemon;
use crate::daemon::audio::{AudioCapture, TARGET_RATE};
#[cfg(not(target_os = "linux"))]
use crate::daemon::hotkey::HotkeyBackend;
use crate::daemon::logging::{BannerInfo, LogLevel, Logger};
use crate::daemon::notifications::Notifier;
use crate::daemon::sounds::Sounds;
use crate::daemon::worker::{spawn_worker, WorkerCtx, WorkerEvent, WORKER_QUEUE_CAPACITY};

const CPU_ENGINE_WARMUP_SECONDS: usize = 1;
const GPU_ENGINE_WARMUP_SECONDS: usize = 60;
const ENGINE_WARMUP_AMPLITUDE: f32 = 0.02;

/// Parse CLI arguments and run the requested command or daemon mode.
///
/// # Returns
///
/// Returns `Ok(())` after the selected command completes or the daemon shuts down.
///
/// # Errors
///
/// Returns an error when CLI command execution, model loading, audio setup, or daemon startup fails.
pub(crate) fn run() -> Result<()> {
    #[cfg(target_os = "linux")]
    daemon::audio::alsa::install_error_silencer();

    let cli = Cli::parse();
    let log = Arc::new(Logger::new(log_level(&cli)));
    let notifier = Notifier::new(Arc::clone(&log));
    #[cfg(target_os = "linux")]
    let hotkey_backend = cli.hotkey_backend;
    #[cfg(not(target_os = "linux"))]
    let hotkey_backend = HotkeyBackend::Auto;
    let paste_mode = cli.effective_paste_mode();

    if let Some(command) = &cli.command {
        match command {
            Commands::Fetch(fetch_cli) => {
                fetch::run(FetchOptions {
                    force: fetch_cli.force,
                    quiet: cli.quiet,
                    verbose: cli.verbose,
                    source: if fetch_cli.from_source {
                        FetchSource::OfficialNemo {
                            keep_nemo: fetch_cli.keep_nemo,
                            keep_f16: fetch_cli.keep_f16,
                        }
                    } else {
                        FetchSource::HostedQ8
                    },
                })?;
                return Ok(());
            }
            Commands::Cache(cache_cli) => {
                run_cache_command(cache_cli, cli.quiet)?;
                return Ok(());
            }
            Commands::Doctor(doctor_cli) => {
                let ok = daemon::preflight::print_doctor(
                    cli.quiet,
                    cli.verbose,
                    paste_mode,
                    doctor_cli.deep,
                    hotkey_backend,
                );
                if ok {
                    return Ok(());
                }
                std::process::exit(1);
            }
            Commands::Status => {
                daemon::ipc::run_client(daemon::ipc::IpcCommand::Status, cli.quiet)?;
                return Ok(());
            }
            Commands::Stop => {
                daemon::ipc::run_client(daemon::ipc::IpcCommand::Stop, cli.quiet)?;
                return Ok(());
            }
            Commands::PasteLast => {
                daemon::ipc::run_client(daemon::ipc::IpcCommand::PasteLast, cli.quiet)?;
                return Ok(());
            }
            Commands::CopyLast => {
                daemon::ipc::run_client(daemon::ipc::IpcCommand::CopyLast, cli.quiet)?;
                return Ok(());
            }
            Commands::TestPaste(test_paste) => {
                daemon::ipc::run_client(
                    daemon::ipc::IpcCommand::TestPaste {
                        text: test_paste.text.clone(),
                    },
                    cli.quiet,
                )?;
                return Ok(());
            }
        }
    }

    // Special command modes: print rules / test rules.
    if cli.list_rules {
        if !cli.quiet {
            rules::print_rule_list();
        }
        return Ok(());
    }
    if let Some(input) = &cli.test_rules {
        let cleaner = rules::build_cleaner(cli.no_cleaning, &cli.disable_rule)?;
        let raw = input.as_str();
        let cleaned = cleaner.as_ref().map(|c| c.clean(raw));
        if !cli.quiet {
            println!("Raw:     {}", raw);
            if let Some(cleaned) = cleaned {
                println!("Clean:   {}", cleaned);
            } else {
                println!("Clean:   <cleaning disabled>");
            }
        }
        return Ok(());
    }
    if let Some(audio_path) = &cli.simulate_ptt_audio {
        return run_ptt_audio_simulation(&cli, Arc::clone(&log), audio_path);
    }

    if let Some(path) = cli.model.as_deref() {
        if !path.is_file() {
            return Err(anyhow::anyhow!(
                "model path is not a file: {}",
                path.display()
            ));
        }
    }

    #[cfg(target_os = "linux")]
    if daemon::wsl::running_under_wsl() {
        log.warn(daemon::wsl::warning());
    }

    #[cfg(target_os = "linux")]
    daemon::session::ensure_x11_session_supported()?;

    let _daemon_lock = daemon::preflight::acquire_singleton_lock()?;

    daemon::preflight::ensure_hotkey_ready(hotkey_backend)?;
    log.verbose(format!(
        "parakit: hotkey preflight passed ({})",
        hotkey_backend.label()
    ));
    daemon::inject::preflight(paste_mode).context("text insertion preflight failed")?;
    log.verbose("parakit: insertion preflight passed");
    let ipc_state = Arc::new(daemon::ipc::SharedState::new());
    #[cfg(any(unix, target_os = "windows"))]
    let _ipc_server = daemon::ipc::spawn_server(
        Arc::clone(&ipc_state),
        paste_mode,
        cli.keep_transcript_clipboard,
        Arc::clone(&log),
    )
    .context("start daemon control socket")?;
    #[cfg(not(any(unix, target_os = "windows")))]
    log.verbose("parakit: local control socket unavailable on this platform");

    let cleaner = rules::build_cleaner(cli.no_cleaning, &cli.disable_rule)?.map(Arc::new);
    let data_log = cli
        .log_dir
        .clone()
        .map(|dir| Arc::new(DataLogger::new(dir, cli.log_format)));

    let sounds = Sounds::new(!cli.no_sounds);

    let capture = AudioCapture::open(Arc::clone(&log), notifier.clone())?;
    let audio = capture.handle.clone();
    let mic_info = capture
        .mic_info()
        .context("audio manager started without reporting a microphone")?;
    warn_about_bluetooth_mic_if_needed(&log, &mic_info);

    let (model_path, engine) = open_cli_engine(&cli, cli.quiet, &log)?;
    let model_dtype = model_dtype_label(&model_path);

    // Banner.
    let model_name = model_file_name(&model_path);
    log.banner(BannerInfo {
        model_name: &model_name,
        model_path: &model_path,
        dtype: &model_dtype,
        mic: &mic_info,
        cleaning: match cleaner.as_deref() {
            Some(c) => format!("on ({} rules)", c.active_rule_count()),
            None => "off".to_string(),
        },
        sounds: if cli.no_sounds { "off" } else { "on" },
        transcription_logging: match &cli.log_dir {
            Some(dir) => format!("{:?} to {}", cli.log_format, dir.display()),
            None => "off".to_string(),
        },
        insertion: format!(
            "batch paste ({}, {})",
            paste_mode.label(),
            if cli.keep_transcript_clipboard {
                "keep transcript clipboard"
            } else {
                "restore clipboard"
            }
        ),
        threads: engine.threads(),
        backend: engine.backend().to_string(),
        device: engine.device_mode().as_str().to_string(),
    });

    // Worker thread takes exclusive ownership of `engine`. `crispasr::Session`
    // is `Send` but not `Sync`, which is fine: only one thread ever calls
    // `transcribe`, and the hotkey path only posts transitions on a channel.
    let (tx, rx) = bounded::<WorkerEvent>(WORKER_QUEUE_CAPACITY);
    let worker = spawn_worker(WorkerCtx {
        engine,
        cleaner,
        data_log,
        sounds: sounds.clone(),
        log: Arc::clone(&log),
        notifier: notifier.clone(),
        state: Arc::clone(&ipc_state),
        paste_mode,
        keep_transcript_clipboard: cli.keep_transcript_clipboard,
        insert_transcripts: true,
        rx,
    });
    let (hotkey_tx, hotkey_rx) = unbounded();
    let coordinator = daemon::recording::spawn_recording_coordinator(hotkey_rx, tx, audio)
        .context("spawn recording coordinator")?;

    // Hotkey grab loop. Blocks forever (until grab returns or process exits).
    ipc_state.set_phase("idle");
    log.ready();

    daemon::hotkey::run_grab_loop(hotkey_tx, hotkey_backend, Arc::clone(&log));

    // Tear down.
    let _ = coordinator.join();
    let _ = worker.join();
    Ok(())
}

/// Warn when the selected microphone appears to be Bluetooth.
///
/// # Arguments
///
/// * `log` - Logger used for the warning.
/// * `mic_info` - Selected microphone metadata.
fn warn_about_bluetooth_mic_if_needed(log: &Logger, mic_info: &daemon::audio::MicInfo) {
    if mic_info.looks_bluetooth() {
        log.warn(format!(
            "selected microphone appears to be Bluetooth ({}); use a wired or local mic if latency or quality is poor",
            mic_info.summary()
        ));
    }
}

fn run_ptt_audio_simulation(cli: &Cli, log: Arc<Logger>, audio_path: &Path) -> Result<()> {
    let paste_mode = cli.effective_paste_mode();
    let cleaner = rules::build_cleaner(cli.no_cleaning, &cli.disable_rule)?.map(Arc::new);
    let data_log = cli
        .log_dir
        .clone()
        .map(|dir| Arc::new(DataLogger::new(dir, cli.log_format)));
    let sounds = Sounds::new(false);

    let load_started = Instant::now();
    let mut wav = read_wav_mono(audio_path)?;
    let load_elapsed = load_started.elapsed();
    let source_rate = wav.sample_rate;
    let source_samples = wav.samples.len();
    let resample_started = Instant::now();
    wav.samples = resample_to_target(wav.samples, source_rate)?;
    let resample_elapsed = resample_started.elapsed();
    let audio_secs = wav.samples.len() as f32 / TARGET_RATE as f32;
    log.verbose(format!(
        "parakit: simulated audio prepared in {:.0}ms (read/downmix {:.0}ms, resample {:.0}ms, source_samples={}, target_samples={})",
        (load_elapsed + resample_elapsed).as_secs_f32() * 1000.0,
        load_elapsed.as_secs_f32() * 1000.0,
        resample_elapsed.as_secs_f32() * 1000.0,
        source_samples,
        wav.samples.len()
    ));

    let (_model_path, engine) = open_cli_engine(cli, cli.quiet || !cli.verbose, &log)?;

    let msg = format!(
        "parakit: simulating PTT from {} ({audio_secs:.2}s, {source_rate} Hz source)",
        audio_path.display()
    );
    log.line(&msg);

    let (tx, rx) = bounded::<WorkerEvent>(WORKER_QUEUE_CAPACITY);
    let worker = spawn_worker(WorkerCtx {
        engine,
        cleaner,
        data_log,
        sounds,
        log,
        notifier: Notifier::new(Arc::new(Logger::new(LogLevel::Quiet))),
        state: Arc::new(daemon::ipc::SharedState::new()),
        paste_mode,
        keep_transcript_clipboard: cli.keep_transcript_clipboard,
        insert_transcripts: false,
        rx,
    });

    let started_at = Instant::now();
    let stopped_at = started_at + Duration::from_secs_f32(audio_secs);
    tx.send(WorkerEvent::Started)
        .context("could not send simulated PTT start event")?;
    tx.send(WorkerEvent::Stopped {
        started_at,
        stopped_at,
        pcm: wav.samples,
        focus_at_start: None,
    })
    .context("could not send simulated PTT stop event")?;
    drop(tx);
    worker
        .join()
        .map_err(|_| anyhow::anyhow!("PTT simulation worker panicked"))?;
    Ok(())
}

fn model_dtype_label(path: &std::path::Path) -> String {
    let dtype = gguf::dtype_label(path);
    let size = path
        .metadata()
        .ok()
        .map(|meta| format!(" ({:.0} MB)", meta.len() as f64 / 1_000_000.0))
        .unwrap_or_default();
    format!("{dtype}{size}")
}

fn open_cli_engine(cli: &Cli, fetch_quiet: bool, log: &Logger) -> Result<(PathBuf, Engine)> {
    let model_path = match cli.model.as_deref() {
        Some(path) => path.to_path_buf(),
        None => fetch::ensure_default_model_with_verbosity(fetch_quiet, cli.verbose)?,
    };
    let threads = cli
        .threads
        .map(NonZeroUsize::get)
        .unwrap_or_else(default_thread_count);
    let device_mode = cli.effective_device_mode();
    validate_device_request(device_mode, log)?;
    let open_started = Instant::now();
    let engine = open_engine(&model_path, threads, device_mode, cli.verbose)
        .with_context(|| format!("could not open model {}", model_path.display()))?;
    log.verbose(format!(
        "parakit: model opened in {:.0}ms with backend={} threads={} device={}",
        open_started.elapsed().as_secs_f32() * 1000.0,
        engine.backend(),
        engine.threads(),
        engine.device_mode().as_str()
    ));
    warm_up_engine(&engine, log)?;
    Ok((model_path, engine))
}

fn log_level(cli: &Cli) -> LogLevel {
    if cli.quiet {
        LogLevel::Quiet
    } else if cli.verbose {
        LogLevel::Verbose
    } else {
        LogLevel::Normal
    }
}

fn model_file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string())
}

fn open_engine(
    path: &Path,
    threads: usize,
    device_mode: DeviceMode,
    verbose: bool,
) -> Result<Engine> {
    if verbose {
        return Engine::open(path, threads, device_mode);
    }
    with_stderr_suppressed(|| Engine::open(path, threads, device_mode))
}

fn validate_device_request(device_mode: DeviceMode, log: &Logger) -> Result<()> {
    if device_mode != DeviceMode::Gpu {
        return Ok(());
    }

    #[cfg(feature = "bundled")]
    {
        if !parakit::gpu::has_gpu_device() {
            anyhow::bail!(
                "--device gpu requested, but ggml reports no GPU or iGPU devices; run `parakit doctor --verbose` for compute diagnostics"
            );
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

fn warm_up_engine(engine: &Engine, log: &Logger) -> Result<()> {
    let started = Instant::now();
    let seconds = engine_warmup_seconds(engine);
    let warmup = engine_warmup_pcm(seconds);
    engine
        .transcribe(&warmup)
        .context("engine warmup transcription failed")?;
    log.verbose(format!(
        "parakit: engine warmup took {:.0}ms ({}s synthetic input)",
        started.elapsed().as_secs_f32() * 1000.0,
        seconds
    ));
    Ok(())
}

fn engine_warmup_seconds(engine: &Engine) -> usize {
    if engine.device_mode() == DeviceMode::Cpu {
        return CPU_ENGINE_WARMUP_SECONDS;
    }

    #[cfg(feature = "bundled")]
    {
        if parakit::gpu::has_gpu_device() {
            return GPU_ENGINE_WARMUP_SECONDS;
        }
    }

    CPU_ENGINE_WARMUP_SECONDS
}

fn engine_warmup_pcm(seconds: usize) -> Vec<f32> {
    let sample_count = TARGET_RATE as usize * seconds;
    (0..sample_count)
        .map(|index| {
            if (index / 80) % 2 == 0 {
                ENGINE_WARMUP_AMPLITUDE
            } else {
                -ENGINE_WARMUP_AMPLITUDE
            }
        })
        .collect()
}

#[cfg(test)]
mod app_tests {
    use super::*;

    #[test]
    fn engine_warmup_pcm_is_representative_and_nonzero() {
        let pcm = engine_warmup_pcm(GPU_ENGINE_WARMUP_SECONDS);
        assert_eq!(pcm.len(), TARGET_RATE as usize * GPU_ENGINE_WARMUP_SECONDS);
        assert!(pcm.iter().any(|sample| *sample > 0.0));
        assert!(pcm.iter().any(|sample| *sample < 0.0));
        assert!(pcm
            .iter()
            .all(|sample| sample.abs() <= ENGINE_WARMUP_AMPLITUDE));
    }
}

#[cfg(unix)]
fn with_stderr_suppressed<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    use std::os::fd::FromRawFd;

    struct RestoreStderr {
        saved: i32,
        drain: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for RestoreStderr {
        fn drop(&mut self) {
            unsafe {
                libc::dup2(self.saved, libc::STDERR_FILENO);
                libc::close(self.saved);
            }
            if let Some(drain) = self.drain.take() {
                let _ = drain.join();
            }
        }
    }

    let mut pipe_fds = [0_i32; 2];
    unsafe {
        if libc::pipe(pipe_fds.as_mut_ptr()) != 0 {
            return f();
        }
    }
    let read_fd = pipe_fds[0];
    let write_fd = pipe_fds[1];
    let saved = unsafe { libc::dup(libc::STDERR_FILENO) };
    if saved < 0 {
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
        return f();
    }
    if unsafe { libc::dup2(write_fd, libc::STDERR_FILENO) } < 0 {
        unsafe {
            libc::close(saved);
            libc::close(read_fd);
            libc::close(write_fd);
        }
        return f();
    }
    unsafe {
        libc::close(write_fd);
    }

    let drain = std::thread::spawn(move || unsafe {
        let mut file = File::from_raw_fd(read_fd);
        let mut buf = [0_u8; 8192];
        while matches!(file.read(&mut buf), Ok(n) if n > 0) {}
    });
    let _restore = RestoreStderr {
        saved,
        drain: Some(drain),
    };
    f()
}

#[cfg(windows)]
const STDERR_FD: libc::c_int = 2;

#[cfg(windows)]
fn with_stderr_suppressed<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    use std::os::windows::io::{FromRawHandle, IntoRawHandle};

    struct RestoreStderr {
        saved_fd: libc::c_int,
        nul_fd: libc::c_int,
    }

    impl Drop for RestoreStderr {
        fn drop(&mut self) {
            unsafe {
                libc::dup2(self.saved_fd, STDERR_FD);
                libc::close(self.saved_fd);
                libc::close(self.nul_fd);
            }
        }
    }

    let Ok(nul_file) = std::fs::OpenOptions::new().write(true).open("NUL") else {
        return f();
    };
    let nul_handle = nul_file.into_raw_handle();
    let nul_fd = unsafe { libc::open_osfhandle(nul_handle as isize, 0) };
    if nul_fd < 0 {
        unsafe {
            drop(std::fs::File::from_raw_handle(nul_handle));
        }
        return f();
    }

    let saved_fd = unsafe { libc::dup(STDERR_FD) };
    if saved_fd < 0 {
        unsafe {
            libc::close(nul_fd);
        }
        return f();
    }

    // MSVCRT _dup2 returns 0 on success, while POSIX dup2 returns the
    // destination fd. Both report failure as -1, so check the failure value.
    if unsafe { libc::dup2(nul_fd, STDERR_FD) } == -1 {
        unsafe {
            libc::close(saved_fd);
            libc::close(nul_fd);
        }
        return f();
    }

    let _restore = RestoreStderr { saved_fd, nul_fd };
    f()
}

#[cfg(all(test, windows))]
mod windows_stdio_tests {
    use super::STDERR_FD;
    use std::os::windows::io::{FromRawHandle, IntoRawHandle};

    struct RestoreStderr {
        saved_fd: libc::c_int,
        nul_fd: libc::c_int,
    }

    impl Drop for RestoreStderr {
        fn drop(&mut self) {
            unsafe {
                libc::dup2(self.saved_fd, STDERR_FD);
                libc::close(self.saved_fd);
                libc::close(self.nul_fd);
            }
        }
    }

    #[test]
    fn windows_crt_dup2_reports_zero_on_success() {
        let nul_file = std::fs::OpenOptions::new()
            .write(true)
            .open("NUL")
            .expect("open NUL");
        let nul_handle = nul_file.into_raw_handle();
        let nul_fd = unsafe { libc::open_osfhandle(nul_handle as isize, 0) };
        if nul_fd < 0 {
            unsafe {
                drop(std::fs::File::from_raw_handle(nul_handle));
            }
            panic!("open_osfhandle failed");
        }

        let saved_fd = unsafe { libc::dup(STDERR_FD) };
        assert!(saved_fd >= 0, "dup stderr failed");
        let _restore = RestoreStderr { saved_fd, nul_fd };

        let result = unsafe { libc::dup2(nul_fd, STDERR_FD) };
        assert_eq!(result, 0);
    }
}

#[cfg(not(any(unix, windows)))]
fn with_stderr_suppressed<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    f()
}

fn run_cache_command(cache: &CacheCli, quiet: bool) -> Result<()> {
    if quiet {
        return Ok(());
    }
    match cache.command.as_ref().unwrap_or(&CacheCommand::List) {
        CacheCommand::Dir => {
            println!("{}", model::models_dir()?.display());
        }
        CacheCommand::List => print_cache_list()?,
    }
    Ok(())
}

fn print_cache_list() -> Result<()> {
    let dir = model::models_dir()?;
    println!("parakit cache");
    println!("  dir: {}", dir.display());
    if !dir.is_dir() {
        println!("  models: none");
        return Ok(());
    }

    let mut entries = std::fs::read_dir(&dir)
        .with_context(|| format!("read cache dir {}", dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "gguf"))
        .collect::<Vec<_>>();
    entries.sort();

    if entries.is_empty() {
        println!("  models: none");
        return Ok(());
    }

    println!("  models:");
    for path in entries {
        let name = model_file_name(&path);
        let dtype = gguf::dtype_label(&path);
        let size = path
            .metadata()
            .map(|meta| format_file_size(meta.len()))
            .unwrap_or_else(|_| "unknown size".to_string());
        let default_marker = if name == model::Q8_FILENAME {
            " default"
        } else {
            ""
        };
        let checksum = if name == model::Q8_FILENAME {
            match parakit::checksum::sha256_file_hex(&path) {
                Ok(hash) if hash == model::HOSTED_Q8_SHA256 => "sha256 ok".to_string(),
                Ok(hash) => format!("sha256 mismatch ({hash})"),
                Err(err) => format!("sha256 unavailable ({err})"),
            }
        } else {
            "sha256 not checked".to_string()
        };
        println!("    {name}{default_marker}: {dtype}, {size}, {checksum}");
    }
    Ok(())
}

fn format_file_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.2} GB", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.0} MB", bytes as f64 / 1_000_000.0)
    } else {
        format!("{} KB", bytes / 1000)
    }
}
