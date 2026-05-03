//! parakit - a push-to-talk dictation daemon.
//!
//! Architecture:
//!   - Main thread: parse CLI, set up subsystems, then run the hotkey backend.
//!     The hotkey loop is blocking and runs forever until SIGINT.
//!   - Recording coordinator thread: converts hotkey transitions into audio
//!     start/stop calls and owned PCM worker events.
//!   - Audio manager thread: owns the live cpal stream and follows the default
//!     input device.
//!   - cpal callback thread: mixes mic samples to mono and pushes them into a
//!     bounded SPSC ring for the audio drain thread.
//!   - Worker thread: receives Event messages via crossbeam-channel, runs
//!     transcription off the hotkey thread so input stays responsive.
//!
//! State machine (single-recording-at-a-time invariant):
//!   Idle --[Ctrl+Space down]--> Recording --[Ctrl+Space up]--> Transcribing --> Idle
//!
//! On Linux, `auto` registers Ctrl+Space with the X11 desktop. The evdev/uinput
//! keyboard proxy is explicit and experimental.

mod daemon;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use crossbeam_channel::{bounded, unbounded};
use parakit::audio_file::{read_wav_mono, resample_to_target};
use parakit::data_log::{DataLogger, LogFormat};
use parakit::fetch::{self, FetchOptions, FetchSource};
use parakit::gguf;
use parakit::inference::{default_thread_count, Engine};
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

use crate::daemon::audio::{AudioCapture, TARGET_RATE};
use crate::daemon::hotkey::HotkeyBackend;
use crate::daemon::inject::PasteMode;
use crate::daemon::logging::{BannerInfo, LogLevel, Logger};
use crate::daemon::notifications::Notifier;
use crate::daemon::sounds::Sounds;
use crate::daemon::worker::{spawn_worker, WorkerCtx, WorkerEvent, WORKER_QUEUE_CAPACITY};

// =============================================================================
// CLI
// =============================================================================

#[derive(Parser, Debug)]
#[command(
    name = "parakit",
    version,
    about = "Push-to-talk dictation daemon (Parakeet-TDT via CrispASR).",
    long_about = "Push-to-talk dictation daemon. Hold Ctrl+Space to record, release to transcribe and insert text at the cursor.\n\nDefault mode prints concise status and transcripts. Pass --verbose for diagnostic paths and timings, or --quiet for background daemon mode."
)]
struct Cli {
    /// Subcommand to run instead of the push-to-talk daemon.
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to a GGUF model file. Overrides the cached Q8_0 model.
    #[arg(short = 'm', long, value_name = "PATH")]
    model: Option<PathBuf>,

    /// Quiet mode: suppress stdout. Errors and warnings still go to stderr.
    /// Suitable for backgrounding the daemon.
    #[arg(long, short = 'q')]
    quiet: bool,

    /// Verbose diagnostics: paths, backend details, and timing lines.
    #[arg(long, short = 'v', conflicts_with = "quiet")]
    verbose: bool,

    /// CPU inference threads. Defaults to the OS available parallelism.
    #[arg(long, value_name = "N")]
    threads: Option<NonZeroUsize>,

    /// Batch insertion style. `terminal` uses Ctrl+Shift+V on Linux/Windows;
    /// `direct` types text without touching the clipboard.
    #[arg(long, value_enum, default_value_t = PasteMode::Terminal)]
    paste_mode: PasteMode,

    /// Disable the audio cues (start / success / error tones).
    #[arg(long)]
    no_sounds: bool,

    /// Disable all text cleaning rules (raw transcript inserted as-is).
    #[arg(long)]
    no_cleaning: bool,

    /// Disable a specific rule by name. Repeatable: `--disable-rule a --disable-rule b`.
    #[arg(long, value_name = "NAME")]
    disable_rule: Vec<String>,

    /// Print all available cleaning rules and exit.
    #[arg(long)]
    list_rules: bool,

    /// Test the rule pipeline against a string and exit. No audio capture.
    /// Useful for iterating on rules.
    ///   `parakit --test-rules "So, um, the the cat ran"`
    #[arg(long, value_name = "INPUT")]
    test_rules: Option<String>,

    /// Hidden validation path: send a WAV through the daemon PTT worker without insertion.
    #[arg(long, hide = true, value_name = "WAV")]
    simulate_ptt_audio: Option<PathBuf>,

    /// Linux hotkey backend. `auto` registers Ctrl+Space with the X11 session.
    #[cfg(target_os = "linux")]
    #[arg(long, value_enum, default_value_t = HotkeyBackend::Auto)]
    hotkey_backend: HotkeyBackend,

    /// Directory for transcription logs. One file is written per local day.
    #[arg(long, value_name = "DIR")]
    log_dir: Option<PathBuf>,

    /// Transcription log format. Used only when --log-dir is set.
    #[arg(long, default_value = "jsonl", value_parser = clap::value_parser!(LogFormat))]
    log_format: LogFormat,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Download the default hosted Parakeet Q8_0 GGUF.
    Fetch(FetchCli),
    /// Inspect the parakit model cache.
    Cache(CacheCli),
    /// Check runtime prerequisites without starting. Exits 0 when ready, 1 when blocked.
    Doctor(DoctorCli),
    /// Query a running daemon over the local control socket.
    Status,
    /// Stop a running daemon over the local control socket.
    Stop,
    /// Paste the last transcript remembered by the running daemon.
    PasteLast,
    /// Exercise clipboard staging and paste without recording microphone audio.
    TestPaste(TestPasteCli),
}

#[derive(Args, Debug)]
struct CacheCli {
    /// Cache subcommand. Defaults to `list`.
    #[command(subcommand)]
    command: Option<CacheCommand>,
}

#[derive(Subcommand, Debug)]
enum CacheCommand {
    /// List cached model artifacts.
    List,
    /// Print the model cache directory.
    Dir,
}

#[derive(Args, Debug)]
struct FetchCli {
    /// Ignore cached artifacts and download or rebuild again.
    #[arg(long)]
    force: bool,

    /// Rebuild Q8_0 locally from NVIDIA's official .nemo checkpoint.
    #[arg(long)]
    from_source: bool,

    /// Keep the downloaded 2.4 GB .nemo checkpoint after source rebuild.
    #[arg(long, requires = "from_source")]
    keep_nemo: bool,

    /// Keep the intermediate F16 GGUF after source rebuild.
    #[arg(long, requires = "from_source")]
    keep_f16: bool,
}

#[derive(Args, Debug)]
struct DoctorCli {
    /// Run active smoke tests in addition to passive preflight checks.
    ///
    /// On Linux/X11 this briefly focuses a tiny probe window and verifies the
    /// configured paste shortcut reaches it.
    #[arg(long)]
    deep: bool,
}

#[derive(Args, Debug)]
struct TestPasteCli {
    /// Text to insert through the running daemon's paste path.
    text: String,
}

// =============================================================================
// main
// =============================================================================

fn main() {
    if let Err(err) = run() {
        eprintln!("parakit: error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    #[cfg(target_os = "linux")]
    daemon::alsa::install_error_silencer();

    let cli = Cli::parse();
    let log = Arc::new(Logger::new(log_level(&cli)));
    let notifier = Notifier::new(Arc::clone(&log));
    #[cfg(target_os = "linux")]
    let hotkey_backend = cli.hotkey_backend;
    #[cfg(not(target_os = "linux"))]
    let hotkey_backend = HotkeyBackend::Auto;

    if let Some(command) = &cli.command {
        match command {
            Commands::Fetch(fetch_cli) => {
                fetch::run(FetchOptions {
                    force: fetch_cli.force,
                    quiet: cli.quiet,
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
                    cli.paste_mode,
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
    daemon::session::ensure_x11_session_supported()?;

    let _daemon_lock = daemon::preflight::acquire_singleton_lock()?;

    daemon::preflight::ensure_hotkey_ready(hotkey_backend)?;
    log.verbose(format!(
        "parakit: hotkey preflight passed ({})",
        hotkey_backend.label()
    ));
    daemon::inject::preflight(cli.paste_mode).context("text insertion preflight failed")?;
    log.verbose("parakit: insertion preflight passed");
    let ipc_state = Arc::new(daemon::ipc::SharedState::new());
    #[cfg(unix)]
    let _ipc_server =
        daemon::ipc::spawn_server(Arc::clone(&ipc_state), cli.paste_mode, Arc::clone(&log))
            .context("start daemon control socket")?;
    #[cfg(not(unix))]
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

    let model_path = match cli.model.as_deref() {
        Some(path) => path.to_path_buf(),
        None => fetch::ensure_default_model(cli.quiet)?,
    };
    let model_dtype = model_dtype_label(&model_path);
    let threads = cli
        .threads
        .map(NonZeroUsize::get)
        .unwrap_or_else(default_thread_count);
    let open_started = Instant::now();
    let engine = open_engine(&model_path, threads, cli.verbose)
        .with_context(|| format!("could not open model {}", model_path.display()))?;
    log.verbose(format!(
        "parakit: model opened in {:.0}ms with backend={} threads={}",
        open_started.elapsed().as_secs_f32() * 1000.0,
        engine.backend(),
        engine.threads()
    ));

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
        insertion: format!("batch paste ({})", cli.paste_mode.label()),
        threads: engine.threads(),
        backend: engine.backend().to_string(),
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
        paste_mode: cli.paste_mode,
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
    let cleaner = rules::build_cleaner(cli.no_cleaning, &cli.disable_rule)?.map(Arc::new);
    let data_log = cli
        .log_dir
        .clone()
        .map(|dir| Arc::new(DataLogger::new(dir, cli.log_format)));
    let sounds = Sounds::new(false);

    let mut wav = read_wav_mono(audio_path)?;
    let source_rate = wav.sample_rate;
    wav.samples = resample_to_target(wav.samples, source_rate)?;
    let audio_secs = wav.samples.len() as f32 / TARGET_RATE as f32;

    let model_path = match cli.model.as_deref() {
        Some(path) => path.to_path_buf(),
        None => fetch::ensure_default_model(cli.quiet || !cli.verbose)?,
    };
    let threads = cli
        .threads
        .map(NonZeroUsize::get)
        .unwrap_or_else(default_thread_count);
    let engine = open_engine(&model_path, threads, cli.verbose)
        .with_context(|| format!("could not open model {}", model_path.display()))?;

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
        paste_mode: cli.paste_mode,
        insert_transcripts: false,
        rx,
    });

    let started_at = Instant::now();
    let stopped_at = started_at + Duration::from_secs_f32(audio_secs);
    tx.send(WorkerEvent::RecordingStarted)
        .context("could not send simulated PTT start event")?;
    tx.send(WorkerEvent::RecordingStopped {
        started_at,
        stopped_at,
        pcm: wav.samples,
        focus_at_start: None,
        target_at_start: None,
    })
    .context("could not send simulated PTT stop event")?;
    drop(tx);
    worker
        .join()
        .map_err(|_| anyhow::anyhow!("PTT simulation worker panicked"))?;
    Ok(())
}

fn model_dtype_label(path: &std::path::Path) -> String {
    let dtype = gguf::detect_dtype(path)
        .ok()
        .flatten()
        .unwrap_or_else(|| "unknown".to_string());
    let size = path
        .metadata()
        .ok()
        .map(|meta| format!(" ({:.0} MB)", meta.len() as f64 / 1_000_000.0))
        .unwrap_or_default();
    format!("{dtype}{size}")
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

fn open_engine(path: &Path, threads: usize, verbose: bool) -> Result<Engine> {
    if verbose {
        return Engine::open_with_threads(path, threads);
    }
    with_stderr_suppressed(|| Engine::open_with_threads(path, threads))
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

#[cfg(not(unix))]
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
        let dtype = gguf::detect_dtype(&path)
            .ok()
            .flatten()
            .unwrap_or_else(|| "unknown".to_string());
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
