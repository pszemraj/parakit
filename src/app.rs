//! Application entry point and top-level command dispatch for the `parakit` binary.

use anyhow::{Context, Result};
use clap::Parser;
use crossbeam_channel::{bounded, unbounded};
use parakit::audio_file::prepare_wav_for_model;
use parakit::data_log::DataLogger;
use parakit::fetch::{self, FetchOptions, FetchSource};
use parakit::gguf;
use parakit::inference::{default_thread_count, DeviceMode, Engine};
use parakit::model;
use parakit::rules;
use parakit::warmup;
use std::ffi::{c_char, c_void, CStr};
use std::io::Write as _;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cli::{CacheCli, CacheCommand, Cli, Commands, ConfigCli, ConfigCommand};
use crate::config::{self, ConfigFile};
use crate::daemon;
use crate::daemon::audio::AudioCapture;
#[cfg(not(target_os = "linux"))]
use crate::daemon::hotkey::HotkeyBackend;
use crate::daemon::logging::{BannerInfo, LogLevel, Logger};
use crate::daemon::notifications::Notifier;
use crate::daemon::sounds::Sounds;
use crate::daemon::worker::{spawn_worker, WorkerCtx, WorkerEvent, WORKER_QUEUE_CAPACITY};

const CPU_ENGINE_WARMUP_SECONDS: &[usize] = &[1];
// The daemon hard-stops held recordings at MAX_UTTERANCE_SECONDS, but warming
// that full 270s shape would make every launch pay worst-case compute. This is
// a realistic-latency policy: cover short dictations and normal 2-25s
// dictations with margin, accepting a one-time backend stall for unusual longer
// cold-cache captures.
const GPU_ENGINE_WARMUP_SECONDS: &[usize] = &[5, 30];
const GGML_LOG_LEVEL_NONE: i32 = 0;
const GGML_LOG_LEVEL_WARN: i32 = 3;
const GGML_LOG_LEVEL_CONT: i32 = 5;
static NATIVE_LOG_MIN_LEVEL: AtomicI32 = AtomicI32::new(GGML_LOG_LEVEL_WARN);
static NATIVE_LOG_LAST_ALLOWED: AtomicBool = AtomicBool::new(false);

type CrispAsrLogCallback = Option<extern "C" fn(i32, *const c_char, *mut c_void)>;

extern "C" {
    fn whisper_log_set(log_callback: CrispAsrLogCallback, user_data: *mut c_void);
}

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
    // Unconditional, CLI-only pass: keeps native ggml log filtering behavior
    // unchanged for the commands below that must not depend on config
    // parsing (see the load-order comment above the post-dispatch
    // `config::load()` call). `doctor` and the post-dispatch daemon
    // bootstrap path re-run this once config is available so a config
    // `daemon.verbose = true` also takes effect.
    configure_native_logging(cli.verbose);

    // `fetch`, `cache`, `config`, `status`, `stop`, `paste-last`,
    // `copy-last`, and `test-paste` must keep working even when the user's
    // config file is broken (missing, bad TOML, invalid user rule), so none
    // of these branches touch `config::load()`. `doctor` is the one
    // dispatch-block exception: it loads config itself below because it
    // reports the same effective hotkey/paste-mode values the daemon would
    // use.
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
            Commands::Config(config_cli) => {
                run_config_command(config_cli, cli.quiet)?;
                return Ok(());
            }
            Commands::Doctor(doctor_cli) => {
                let config = config::load()?;
                configure_native_logging(cli.effective_verbose(&config));
                let paste_mode = cli.effective_paste_mode(&config);
                #[cfg(target_os = "linux")]
                let hotkey_backend = cli.effective_hotkey_backend(&config);
                #[cfg(not(target_os = "linux"))]
                let hotkey_backend = HotkeyBackend::Auto;
                let ok = daemon::preflight::print_doctor(
                    cli.quiet,
                    cli.effective_verbose(&config),
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
                daemon::ipc::run_client(daemon::ipc::IpcCommand::Status, cli.quiet, cli.verbose)?;
                return Ok(());
            }
            Commands::Stop => {
                daemon::ipc::run_client(daemon::ipc::IpcCommand::Stop, cli.quiet, cli.verbose)?;
                return Ok(());
            }
            Commands::PasteLast => {
                daemon::ipc::run_client(
                    daemon::ipc::IpcCommand::PasteLast,
                    cli.quiet,
                    cli.verbose,
                )?;
                return Ok(());
            }
            Commands::CopyLast => {
                daemon::ipc::run_client(daemon::ipc::IpcCommand::CopyLast, cli.quiet, cli.verbose)?;
                return Ok(());
            }
            Commands::TestPaste(test_paste) => {
                daemon::ipc::run_client(
                    daemon::ipc::IpcCommand::TestPaste {
                        text: test_paste.text.clone(),
                    },
                    cli.quiet,
                    cli.verbose,
                )?;
                return Ok(());
            }
        }
    }

    // Beyond this point: `--list-rules`, `--test-rules`,
    // `--simulate-ptt-audio`, and full daemon bootstrap. All of these merge
    // CLI flags with the config file, loaded once here.
    let config = config::load()?;
    configure_native_logging(cli.effective_verbose(&config));
    let log = Arc::new(Logger::new(log_level(&cli, &config)));
    let notifier = Notifier::new(Arc::clone(&log));
    #[cfg(target_os = "linux")]
    let hotkey_backend = cli.effective_hotkey_backend(&config);
    #[cfg(not(target_os = "linux"))]
    let hotkey_backend = HotkeyBackend::Auto;
    let paste_mode = cli.effective_paste_mode(&config);

    // Special command modes: print rules / test rules.
    if cli.list_rules {
        if !cli.quiet {
            rules::print_rule_list(&config.rules.user);
        }
        return Ok(());
    }
    if let Some(input) = &cli.test_rules {
        let cleaner = rules::build_cleaner(
            !cli.effective_cleaning_enabled(&config),
            &cli.effective_disabled_rules(&config),
            &config.rules.user,
        )?;
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
        return run_ptt_audio_simulation(&cli, &config, Arc::clone(&log), audio_path);
    }

    if let Some(path) = cli.effective_model(&config) {
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
    let keep_transcript_clipboard = cli.effective_keep_transcript_clipboard(&config);
    #[cfg(any(unix, target_os = "windows"))]
    let _ipc_server = daemon::ipc::spawn_server(
        Arc::clone(&ipc_state),
        paste_mode,
        keep_transcript_clipboard,
        Arc::clone(&log),
    )
    .context("start daemon control socket")?;
    #[cfg(not(any(unix, target_os = "windows")))]
    log.verbose("parakit: local control socket unavailable on this platform");

    let cleaner = rules::build_cleaner(
        !cli.effective_cleaning_enabled(&config),
        &cli.effective_disabled_rules(&config),
        &config.rules.user,
    )?
    .map(Arc::new);
    let log_dir = cli.effective_log_dir(&config);
    let log_format = cli.effective_log_format(&config);
    let data_log = log_dir
        .clone()
        .map(|dir| Arc::new(DataLogger::new(dir, log_format)));

    let sounds_enabled = cli.effective_sounds_enabled(&config);
    let sounds = Sounds::new(sounds_enabled);

    let capture = AudioCapture::open(Arc::clone(&log), notifier.clone())?;
    let audio = capture.handle.clone();
    let mic_info = capture
        .mic_info()
        .context("audio manager started without reporting a microphone")?;
    warn_about_bluetooth_mic_if_needed(&log, &mic_info);

    let (model_path, engine) = open_cli_engine(&cli, &config, cli.quiet, &log)?;
    let model_dtype = model_dtype_label(&model_path);

    // Banner.
    let model_name = model_file_name(&model_path);
    let cleaning_summary = match cleaner.as_deref() {
        Some(c) => format!("on ({} rules)", c.active_rule_count()),
        None => "off".to_string(),
    };
    let device_summary = resolved_device_summary(engine.device_mode());
    let backend_label = engine.backend().to_string();
    let engine_threads = engine.threads();
    log.banner(BannerInfo {
        model_name: &model_name,
        model_path: &model_path,
        dtype: &model_dtype,
        mic: &mic_info,
        cleaning: cleaning_summary.clone(),
        sounds: if sounds_enabled { "on" } else { "off" },
        transcription_logging: match &log_dir {
            Some(dir) => format!("{log_format:?} to {}", dir.display()),
            None => "off".to_string(),
        },
        insertion: format!(
            "batch paste ({}, {})",
            paste_mode.label(),
            if keep_transcript_clipboard {
                "keep transcript clipboard"
            } else {
                "restore clipboard"
            }
        ),
        threads: engine_threads,
        backend: backend_label.clone(),
        device: device_summary.clone(),
    });

    // Status detail: mirrors the banner fields above so `parakit --verbose
    // status` can report the same effective values without re-deriving them.
    #[cfg(target_os = "linux")]
    let hotkey_backend_label = Some(hotkey_backend.label());
    #[cfg(not(target_os = "linux"))]
    let hotkey_backend_label = None;
    ipc_state.set_info(daemon::ipc::DaemonInfo {
        pid: std::process::id(),
        model_name,
        dtype: model_dtype,
        mic_summary: mic_info.summary(),
        backend: backend_label,
        device: device_summary,
        threads: engine_threads,
        paste_mode: paste_mode.label(),
        cleaning_summary,
        sounds_on: sounds_enabled,
        log_summary: log_dir
            .as_ref()
            .map(|dir| format!("{log_format:?} to {}", dir.display())),
        hotkey_backend_label,
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
        keep_transcript_clipboard,
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

fn configure_native_logging(verbose: bool) {
    let min_level = if verbose {
        GGML_LOG_LEVEL_NONE
    } else {
        GGML_LOG_LEVEL_WARN
    };
    NATIVE_LOG_MIN_LEVEL.store(min_level, Ordering::Relaxed);
    NATIVE_LOG_LAST_ALLOWED.store(false, Ordering::Relaxed);
    unsafe {
        // CrispASR/ggml exposes one process-global logger on every supported
        // platform. Install it unconditionally so non-verbose daemon output
        // keeps native INFO/DEBUG chatter out of stderr while preserving WARN/ERROR.
        whisper_log_set(Some(parakit_native_log_callback), std::ptr::null_mut());
    }
}

extern "C" fn parakit_native_log_callback(
    level: i32,
    text: *const c_char,
    _user_data: *mut c_void,
) {
    if text.is_null() {
        return;
    }
    let min_level = NATIVE_LOG_MIN_LEVEL.load(Ordering::Relaxed);
    let last_allowed = NATIVE_LOG_LAST_ALLOWED.load(Ordering::Relaxed);
    let decision = native_log_decision(level, min_level, last_allowed);
    if let Some(next_last_allowed) = decision.next_last_allowed {
        NATIVE_LOG_LAST_ALLOWED.store(next_last_allowed, Ordering::Relaxed);
    }
    if !decision.allowed {
        return;
    }
    let bytes = unsafe { CStr::from_ptr(text) }.to_bytes();
    let _ = std::io::Write::write_all(&mut std::io::stderr().lock(), bytes);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NativeLogDecision {
    allowed: bool,
    next_last_allowed: Option<bool>,
}

fn native_log_decision(level: i32, min_level: i32, last_allowed: bool) -> NativeLogDecision {
    if level == GGML_LOG_LEVEL_CONT {
        return NativeLogDecision {
            allowed: last_allowed,
            next_last_allowed: None,
        };
    }

    let allowed = level >= min_level;
    NativeLogDecision {
        allowed,
        next_last_allowed: Some(allowed),
    }
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

fn run_ptt_audio_simulation(
    cli: &Cli,
    config: &ConfigFile,
    log: Arc<Logger>,
    audio_path: &Path,
) -> Result<()> {
    let paste_mode = cli.effective_paste_mode(config);
    let cleaner = rules::build_cleaner(
        !cli.effective_cleaning_enabled(config),
        &cli.effective_disabled_rules(config),
        &config.rules.user,
    )?
    .map(Arc::new);
    let data_log = cli
        .effective_log_dir(config)
        .map(|dir| Arc::new(DataLogger::new(dir, cli.effective_log_format(config))));
    let sounds = Sounds::new(false);

    let prepare_started = Instant::now();
    let wav = prepare_wav_for_model(audio_path)?;
    let prepare_elapsed = prepare_started.elapsed();
    let audio_secs = wav.audio_secs();
    log.verbose(format!(
        "parakit: simulated audio prepared in {:.0}ms (source_rate={} Hz, source_samples={}, target_samples={})",
        prepare_elapsed.as_secs_f32() * 1000.0,
        wav.source_rate,
        wav.source_samples,
        wav.samples.len()
    ));

    let (_model_path, engine) = open_cli_engine(
        cli,
        config,
        cli.quiet || !cli.effective_verbose(config),
        &log,
    )?;

    let msg = format!(
        "parakit: simulating PTT from {} ({audio_secs:.2}s, {source_rate} Hz source)",
        audio_path.display(),
        source_rate = wav.source_rate
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
        keep_transcript_clipboard: cli.effective_keep_transcript_clipboard(config),
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

fn open_cli_engine(
    cli: &Cli,
    config: &ConfigFile,
    fetch_quiet: bool,
    log: &Logger,
) -> Result<(PathBuf, Engine)> {
    let verbose = cli.effective_verbose(config);
    let engine_config = resolve_engine_config(
        cli,
        config,
        || fetch::ensure_default_model_with_verbosity(fetch_quiet, verbose),
        log,
    )?;
    let model_path = engine_config.model_path;
    let open_started = Instant::now();
    let engine = open_engine(
        &model_path,
        engine_config.threads,
        engine_config.device_mode,
        verbose,
    )
    .with_context(|| format!("could not open model {}", model_path.display()))?;
    let device_summary = resolved_device_summary(engine.device_mode());
    log.verbose(format!(
        "parakit: model opened in {:.0}ms with backend={} threads={} device={}",
        open_started.elapsed().as_secs_f32() * 1000.0,
        engine.backend(),
        engine.threads(),
        device_summary
    ));
    // Warmup is a startup readiness check, not only a latency hint: it runs
    // the same transcribe path the first real dictation would use.
    warm_up_engine(&engine, log)?;
    Ok((model_path, engine))
}

#[derive(Debug)]
struct EngineConfig {
    model_path: PathBuf,
    threads: usize,
    device_mode: DeviceMode,
}

fn resolve_engine_config<F>(
    cli: &Cli,
    config: &ConfigFile,
    fetch_default_model: F,
    log: &Logger,
) -> Result<EngineConfig>
where
    F: FnOnce() -> Result<PathBuf>,
{
    resolve_engine_config_with_validator(cli, config, fetch_default_model, |device_mode| {
        validate_device_request(device_mode, log)
    })
}

fn resolve_engine_config_with_validator<F, V>(
    cli: &Cli,
    config: &ConfigFile,
    fetch_default_model: F,
    validate_device: V,
) -> Result<EngineConfig>
where
    F: FnOnce() -> Result<PathBuf>,
    V: FnOnce(DeviceMode) -> Result<()>,
{
    let device_mode = cli.effective_device(config);
    validate_device(device_mode)?;
    let model_path = match cli.effective_model(config) {
        Some(path) => path,
        None => fetch_default_model()?,
    };
    let threads = cli
        .effective_threads(config)
        .map(NonZeroUsize::get)
        .unwrap_or_else(default_thread_count);
    Ok(EngineConfig {
        model_path,
        threads,
        device_mode,
    })
}

fn log_level(cli: &Cli, config: &ConfigFile) -> LogLevel {
    if cli.quiet {
        LogLevel::Quiet
    } else if cli.effective_verbose(config) {
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
    daemon::stderr::with_stderr_suppressed(|| Engine::open(path, threads, device_mode))
}

fn validate_device_request(device_mode: DeviceMode, log: &Logger) -> Result<()> {
    if device_mode != DeviceMode::Gpu {
        return Ok(());
    }

    #[cfg(feature = "bundled")]
    {
        if !parakit::gpu::has_gpu_device() {
            let mut message = "--device gpu requested, but ggml reports no GPU or iGPU devices; run `parakit --verbose doctor` for compute diagnostics".to_string();
            #[cfg(target_os = "macos")]
            if let Some(hint) = daemon::macos::no_gpu_hint() {
                message.push_str("; ");
                message.push_str(hint);
            }
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

fn resolved_device_summary(device_mode: DeviceMode) -> String {
    if device_mode == DeviceMode::Cpu {
        return DeviceMode::Cpu.as_str().to_string();
    }

    #[cfg(feature = "bundled")]
    {
        match parakit::gpu::preferred_gpu_device() {
            Some(device) => format!("{} -> {}", device_mode.as_str(), device.diagnostic_line()),
            None if device_mode == DeviceMode::Auto => {
                "auto -> CPU fallback (no GPU/iGPU visible)".to_string()
            }
            None => "gpu -> unavailable (no GPU/iGPU visible)".to_string(),
        }
    }

    #[cfg(not(feature = "bundled"))]
    {
        format!("{} (device probe unavailable)", device_mode.as_str())
    }
}

fn warm_up_engine(engine: &Engine, log: &Logger) -> Result<()> {
    let started = Instant::now();
    let sequence = engine_warmup_seconds(engine);
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

fn engine_warmup_seconds(engine: &Engine) -> &'static [usize] {
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

fn format_warmup_sequence(sequence: &[usize]) -> String {
    sequence
        .iter()
        .map(|seconds| format!("{seconds}s"))
        .collect::<Vec<_>>()
        .join(" + ")
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

fn run_config_command(config_cli: &ConfigCli, quiet: bool) -> Result<()> {
    match config_cli.command.as_ref().unwrap_or(&ConfigCommand::Show) {
        ConfigCommand::Path => {
            if !quiet {
                println!("{}", config::config_path()?.display());
            }
        }
        ConfigCommand::Init { force } => init_config_file(*force, quiet)?,
        ConfigCommand::Show => print_config_show(quiet)?,
        ConfigCommand::Edit => edit_config_file()?,
    }
    Ok(())
}

/// Write the commented config template to the resolved config path.
///
/// # Errors
///
/// Returns an error if the config directory cannot be created, or if the
/// file already exists and `force` is `false`.
fn init_config_file(force: bool, quiet: bool) -> Result<()> {
    let path = config::config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let mut open_options = std::fs::OpenOptions::new();
    open_options.write(true);
    if force {
        open_options.create(true).truncate(true);
    } else {
        open_options.create_new(true);
    }
    let mut file = open_options.open(&path).with_context(|| {
        format!(
            "failed to create config file {} (use --force to overwrite an existing file)",
            path.display()
        )
    })?;
    file.write_all(config::TEMPLATE.as_bytes())
        .with_context(|| format!("failed to write config file {}", path.display()))?;

    if !quiet {
        println!("wrote {}", path.display());
    }
    Ok(())
}

/// Print the resolved config path and effective merged values.
///
/// # Errors
///
/// Returns an error if the config path cannot be resolved or the config
/// file exists but fails to parse or validate.
fn print_config_show(quiet: bool) -> Result<()> {
    if quiet {
        return Ok(());
    }

    let path = config::config_path()?;
    let config = config::load()?;

    println!("parakit config");
    println!("  path: {}", path.display());
    println!("  exists: {}", path.is_file());
    println!("  daemon:");
    println!(
        "    model: {}",
        config
            .daemon
            .model
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(default: hosted Q8_0)".to_string())
    );
    println!(
        "    device: {}",
        config
            .daemon
            .device
            .map(|d| d.as_str().to_string())
            .unwrap_or_else(|| format!("(default: {})", DeviceMode::default().as_str()))
    );
    println!(
        "    threads: {}",
        config
            .daemon
            .threads
            .map(|t| t.to_string())
            .unwrap_or_else(|| "(default: auto-detected)".to_string())
    );
    println!(
        "    paste_mode: {}",
        config
            .daemon
            .paste_mode
            .map(|m| m.label().to_string())
            .unwrap_or_else(|| "(default: platform)".to_string())
    );
    println!(
        "    keep_transcript_clipboard: {}",
        config.daemon.keep_transcript_clipboard.unwrap_or(false)
    );
    println!("    sounds: {}", config.daemon.sounds.unwrap_or(true));
    println!("    verbose: {}", config.daemon.verbose.unwrap_or(false));
    println!("  cleaning:");
    println!("    enabled: {}", config.cleaning.enabled.unwrap_or(true));
    println!("    disabled_rules: {:?}", config.cleaning.disabled_rules);
    println!("  logging:");
    println!(
        "    dir: {}",
        config
            .logging
            .dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(disabled)".to_string())
    );
    println!(
        "    format: {}",
        config
            .logging
            .format
            .map(|f| format!("{f:?}").to_lowercase())
            .unwrap_or_else(|| "jsonl".to_string())
    );
    #[cfg(target_os = "linux")]
    {
        println!("  hotkey:");
        println!(
            "    backend: {}",
            config
                .hotkey
                .backend
                .map(|b| b.label().to_string())
                .unwrap_or_else(|| "(default: auto)".to_string())
        );
    }
    println!("  rules:");
    println!("    user rules: {}", config.rules.user.len());
    for user_rule in &config.rules.user {
        println!("      {} ({})", user_rule.name, user_rule.position.as_str());
    }
    Ok(())
}

/// Open the config file in `$VISUAL` or `$EDITOR`, creating it from the
/// template first if it does not exist yet.
///
/// # Errors
///
/// Returns an error if the config path cannot be resolved, the template
/// cannot be written when the file is missing, neither `$VISUAL` nor
/// `$EDITOR` is set, the editor cannot be launched, or the editor exits
/// with a non-zero status.
fn edit_config_file() -> Result<()> {
    let path = config::config_path()?;
    if !path.is_file() {
        init_config_file(false, true)?;
    }

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .map_err(|_| {
            anyhow::anyhow!(
                "no editor configured: set $VISUAL or $EDITOR, or edit {} directly",
                path.display()
            )
        })?;

    let status = std::process::Command::new(&editor)
        .arg(&path)
        .status()
        .with_context(|| format!("failed to launch editor '{editor}'"))?;
    if !status.success() {
        anyhow::bail!("editor '{editor}' exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod app_tests {
    use super::*;

    const GGML_LOG_LEVEL_DEBUG: i32 = 1;
    const GGML_LOG_LEVEL_INFO: i32 = 2;
    const GGML_LOG_LEVEL_ERROR: i32 = 4;

    #[test]
    fn gpu_warmup_policy_is_realistic_not_worst_case() {
        assert_eq!(crate::daemon::recording::MAX_UTTERANCE_SECONDS, 270);
        assert_eq!(GPU_ENGINE_WARMUP_SECONDS, &[5, 30]);
        assert!(GPU_ENGINE_WARMUP_SECONDS
            .iter()
            .all(|seconds| *seconds < crate::daemon::recording::MAX_UTTERANCE_SECONDS as usize));
    }

    #[test]
    fn warmup_sequence_format_is_stable() {
        assert_eq!(format_warmup_sequence(&[5, 30]), "5s + 30s");
    }

    #[test]
    fn cpu_device_summary_is_plain() {
        assert_eq!(resolved_device_summary(DeviceMode::Cpu), "cpu");
    }

    #[test]
    fn native_log_filter_suppresses_info_and_debug_without_verbose() {
        assert_eq!(
            native_log_decision(GGML_LOG_LEVEL_DEBUG, GGML_LOG_LEVEL_WARN, true),
            NativeLogDecision {
                allowed: false,
                next_last_allowed: Some(false),
            }
        );
        assert_eq!(
            native_log_decision(GGML_LOG_LEVEL_INFO, GGML_LOG_LEVEL_WARN, true),
            NativeLogDecision {
                allowed: false,
                next_last_allowed: Some(false),
            }
        );
    }

    #[test]
    fn native_log_filter_keeps_warnings_and_errors_without_verbose() {
        assert_eq!(
            native_log_decision(GGML_LOG_LEVEL_WARN, GGML_LOG_LEVEL_WARN, false),
            NativeLogDecision {
                allowed: true,
                next_last_allowed: Some(true),
            }
        );
        assert_eq!(
            native_log_decision(GGML_LOG_LEVEL_ERROR, GGML_LOG_LEVEL_WARN, false),
            NativeLogDecision {
                allowed: true,
                next_last_allowed: Some(true),
            }
        );
    }

    #[test]
    fn native_log_filter_passes_everything_with_verbose() {
        assert_eq!(
            native_log_decision(GGML_LOG_LEVEL_DEBUG, GGML_LOG_LEVEL_NONE, false),
            NativeLogDecision {
                allowed: true,
                next_last_allowed: Some(true),
            }
        );
        assert_eq!(
            native_log_decision(GGML_LOG_LEVEL_INFO, GGML_LOG_LEVEL_NONE, false),
            NativeLogDecision {
                allowed: true,
                next_last_allowed: Some(true),
            }
        );
    }

    #[test]
    fn native_log_continuation_follows_previous_allowed_record() {
        assert_eq!(
            native_log_decision(GGML_LOG_LEVEL_CONT, GGML_LOG_LEVEL_WARN, false),
            NativeLogDecision {
                allowed: false,
                next_last_allowed: None,
            }
        );
        assert_eq!(
            native_log_decision(GGML_LOG_LEVEL_CONT, GGML_LOG_LEVEL_WARN, true),
            NativeLogDecision {
                allowed: true,
                next_last_allowed: None,
            }
        );
    }

    #[test]
    fn explicit_gpu_validation_runs_before_default_model_fetch() {
        let cli = Cli::parse_from(["parakit", "--device", "gpu"]);
        let config = ConfigFile::default();
        let fetched_default = std::cell::Cell::new(false);

        let err = resolve_engine_config_with_validator(
            &cli,
            &config,
            || {
                fetched_default.set(true);
                Ok(PathBuf::from("target/tmp/default-model.gguf"))
            },
            |device_mode| {
                assert_eq!(device_mode, DeviceMode::Gpu);
                anyhow::bail!("gpu unavailable")
            },
        )
        .unwrap_err();

        assert_eq!(err.to_string(), "gpu unavailable");
        assert!(!fetched_default.get());
    }
}
