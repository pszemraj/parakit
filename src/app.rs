//! Application entry point and top-level command dispatch for the `parakit` binary.

use anyhow::{Context, Result};
use clap::Parser;
use crossbeam_channel::{bounded, unbounded};

mod config_command;
mod simulation;
use config_command::run_config_command;
use parakit::data_log::DataLogger;
use parakit::fetch::{self, FetchOptions, FetchSource};
use parakit::gguf;
use parakit::inference::{default_thread_count, DeviceMode, Engine};
use parakit::model;
use parakit::rules;
use simulation::run_ptt_audio_simulation;
use std::ffi::{c_char, c_void, CStr};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cli::{
    CacheCli, CacheCommand, Cli, Commands, DoctorCli, RulesArgs, RulesCli, RulesCommand, StartCli,
};
use crate::config::{self, ConfigFile};
use crate::daemon;
use crate::daemon::audio::AudioCapture;
use crate::daemon::engine_runtime::{validate_device_request, EngineRecipe};
#[cfg(not(target_os = "linux"))]
use crate::daemon::hotkey::HotkeyBackend;
use crate::daemon::logging::{BannerInfo, LogLevel, Logger};
use crate::daemon::notifications::Notifier;
use crate::daemon::sounds::Sounds;
use crate::daemon::worker::{spawn_worker, WorkerCtx, WorkerEvent, WORKER_QUEUE_CAPACITY};

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
    // `config::load()` call). `doctor`, `rules`, and the daemon bootstrap
    // path re-run this once config is available so a config
    // `daemon.verbose = true` also takes effect.
    configure_native_logging(cli.verbose);

    // `fetch`, `cache`, `config`, `status`, `stop`, `copy-last`, `history`,
    // and `test-paste` must keep working even when the user's config file is
    // broken (missing, bad TOML, invalid user rule), so none of these
    // branches touch `config::load()`. `doctor` and `rules` are the
    // dispatch-block exceptions: they load config themselves below because
    // they report/apply the same effective values the daemon would use.
    match &cli.command {
        Some(Commands::Fetch(fetch_cli)) => {
            let source = if fetch_cli.from_source {
                FetchSource::OfficialNemo {
                    keep_nemo: fetch_cli.keep_nemo,
                    keep_f16: fetch_cli.keep_f16,
                }
            } else {
                fetch::source_from_cli(
                    fetch_cli.source.clone(),
                    fetch_cli.file.clone(),
                    fetch_cli.sha256.clone(),
                )?
            };
            fetch::run(FetchOptions {
                force: fetch_cli.force,
                quiet: cli.quiet,
                verbose: cli.verbose,
                source,
            })?;
            Ok(())
        }
        Some(Commands::Cache(cache_cli)) => run_cache_command(cache_cli, cli.quiet),
        Some(Commands::Config(config_cli)) => run_config_command(config_cli, cli.quiet),
        Some(Commands::Doctor(doctor_cli)) => run_doctor(&cli, doctor_cli),
        Some(Commands::Rules(rules_cli)) => run_rules_command(&cli, rules_cli),
        Some(Commands::Status) => {
            daemon::ipc::run_client(daemon::ipc::IpcCommand::Status, cli.quiet, cli.verbose)
        }
        Some(Commands::Stop) => {
            daemon::ipc::run_client(daemon::ipc::IpcCommand::Stop, cli.quiet, cli.verbose)
        }
        Some(Commands::CopyLast(history_ref)) => {
            let index = wire_history_index(history_ref.index)?;
            daemon::ipc::run_client(
                daemon::ipc::IpcCommand::CopyLast { index },
                cli.quiet,
                cli.verbose,
            )
        }
        Some(Commands::History(history_cli)) => {
            let limit = wire_history_limit(history_cli.limit)?;
            daemon::ipc::run_client(
                daemon::ipc::IpcCommand::History { limit },
                cli.quiet,
                cli.verbose,
            )
        }
        Some(Commands::TestPaste(test_paste)) => daemon::ipc::run_client(
            daemon::ipc::IpcCommand::TestPaste {
                text: test_paste.text.clone(),
            },
            cli.quiet,
            cli.verbose,
        ),
        Some(Commands::Start(start)) => run_daemon(&cli, start),
        None => run_daemon(&cli, &StartCli::default()),
    }
}

/// Run the `doctor` preflight checks.
///
/// Loads config itself (unlike most other subcommands) because it reports
/// the same effective hotkey/paste-mode values the daemon would use.
///
/// # Errors
///
/// Returns an error when the config file fails to load or parse.
fn run_doctor(cli: &Cli, doctor_cli: &DoctorCli) -> Result<()> {
    let config = config::load()?;
    configure_native_logging(cli.effective_verbose(&config));
    let paste_mode = doctor_cli.effective_paste_mode(&config);
    #[cfg(target_os = "linux")]
    let hotkey_backend = doctor_cli.effective_hotkey_backend(&config);
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

/// Run `rules list` or `rules test`, defaulting to `list` when no rules
/// subcommand is given.
///
/// Loads config itself (unlike most other subcommands) for the same reason
/// as `doctor`: it needs `cleaning.profile`/`cleaning.disabled_rules`
/// fallbacks and `[[rules.user]]` entries.
///
/// # Errors
///
/// Returns an error when the config file fails to load or parse, or when an
/// unknown rule name is disabled or a user rule is invalid.
fn run_rules_command(cli: &Cli, rules_cli: &RulesCli) -> Result<()> {
    let config = config::load()?;
    configure_native_logging(cli.effective_verbose(&config));
    let default_command = RulesCommand::List(RulesArgs::default());
    match rules_cli.command.as_ref().unwrap_or(&default_command) {
        RulesCommand::List(args) => {
            rules::print_rule_list(
                args.effective_cleaning_profile(&config),
                args.effective_drops_trailing_period(&config),
                &args.effective_disabled_rules(&config),
                &config.rules.user,
                cli.quiet,
            )?;
            Ok(())
        }
        RulesCommand::Test { input, args } => {
            let cleaner = build_rules_cleaner(args, &config)?;
            let raw = input.as_str();
            let cleaned = cleaner.clean(raw);
            if !cli.quiet {
                println!("Raw:     {}", raw);
                println!("Clean:   {}", cleaned.text);
                if let Some(failure) = &cleaned.failure {
                    eprintln!("parakit: cleaning failed, raw text kept: {failure}");
                } else if !cleaned.rules_fired.is_empty() {
                    let fired: Vec<String> = cleaned
                        .rules_fired
                        .iter()
                        .map(|hit| format!("{}x{}", hit.name, hit.matches))
                        .collect();
                    println!("Rules:   {}", fired.join(", "));
                }
            }
            Ok(())
        }
    }
}

/// Run the push-to-talk daemon: the full startup sequence when no
/// subcommand is given, or when `start` is given explicitly.
///
/// # Errors
///
/// Returns an error when config loading, model loading, audio setup, or
/// daemon startup fails.
fn run_daemon(cli: &Cli, start: &StartCli) -> Result<()> {
    // The hidden audio simulation is a one-shot worker validation, not a
    // daemon instance, so it intentionally does not participate in the
    // singleton lock.
    let _daemon_lock = if start.simulate_ptt_audio.is_none() {
        match daemon::preflight::acquire_singleton_lock() {
            Ok(lock) => Some(lock),
            Err(err) if err.is::<daemon::preflight::DaemonAlreadyRunning>() => {
                ensure_existing_daemon_responsive(daemon::ipc::daemon_responsive()?)?;
                if !cli.quiet {
                    println!("parakit: already running");
                }
                return Ok(());
            }
            Err(err) => return Err(err),
        }
    } else {
        None
    };

    // Beyond this point: `--simulate-ptt-audio` and full daemon bootstrap.
    // All of these merge CLI flags with the config file, loaded once here.
    let config = config::load()?;
    let verbose = cli.effective_verbose(&config);
    configure_native_logging(verbose);
    let log = Arc::new(Logger::new(log_level(cli, &config)));
    #[cfg(target_os = "linux")]
    let hotkey_backend = start.effective_hotkey_backend(&config);
    #[cfg(not(target_os = "linux"))]
    let hotkey_backend = HotkeyBackend::Auto;
    let paste_mode = start.effective_paste_mode(&config);

    if let Some(audio_path) = &start.simulate_ptt_audio {
        return run_ptt_audio_simulation(cli, start, &config, Arc::clone(&log), audio_path);
    }

    if let Some(path) = start.effective_model(&config) {
        model::validate_model_file(&path)?;
    }

    #[cfg(target_os = "linux")]
    if daemon::wsl::running_under_wsl() {
        log.warn(daemon::wsl::warning());
    }

    #[cfg(target_os = "linux")]
    daemon::session::ensure_x11_session_supported()?;

    daemon::preflight::ensure_hotkey_ready(hotkey_backend)?;
    log.verbose(format!(
        "parakit: hotkey preflight passed ({})",
        hotkey_backend.label()
    ));
    daemon::inject::preflight(paste_mode).context("text insertion preflight failed")?;
    log.verbose("parakit: insertion preflight passed");

    let ipc_state = Arc::new(daemon::ipc::SharedState::with_history_limit(
        start.effective_transcript_history(&config),
    ));
    // Register before the engine so reverse local-drop order releases the
    // native session before this guard on every failed-startup path.
    let worker_lifetime = ipc_state.shutdown.register();
    let notifier = Notifier::new(Arc::clone(&log));
    let keep_transcript_clipboard = start.effective_keep_transcript_clipboard(&config);
    let log_dir = start.effective_log_dir(&config);
    let data_log = log_dir.clone().map(|dir| Arc::new(DataLogger::new(dir)));
    #[cfg(any(unix, target_os = "windows"))]
    let _ipc_server = daemon::ipc::spawn_server(
        Arc::clone(&ipc_state),
        paste_mode,
        keep_transcript_clipboard,
        Arc::clone(&log),
        notifier.clone(),
    )
    .context("start daemon control socket")?;

    let cleaner = build_cli_cleaner(start, &config)?.map(Arc::new);
    let sounds_enabled = start.effective_sounds_enabled(&config);
    let sounds = Sounds::new(sounds_enabled);

    let capture = AudioCapture::open(Arc::clone(&log), notifier.clone())?;
    let audio = capture.handle.clone();
    let mic_info = capture
        .mic_info()
        .context("audio manager started without reporting a microphone")?;
    warn_about_bluetooth_mic_if_needed(&log, &mic_info);

    // Keep control available during model download and loading, including stop.
    let OpenedEngine {
        model_path,
        engine,
        device_summary,
        recipe,
    } = open_cli_engine(start, &config, verbose, cli.quiet, &log)?;
    let model_dtype = model_dtype_label(&model_path);

    // Banner.
    let model_name = model_file_name(&model_path);
    let cleaning_summary = match cleaner.as_deref() {
        Some(c) => format!(
            "on ({}, {} rules{})",
            c.profile(),
            c.active_rule_count(),
            if c.drops_trailing_period() {
                ""
            } else {
                ", keeps trailing period"
            }
        ),
        None => "off".to_string(),
    };
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
            Some(dir) => format!("JSONL to {}", dir.display()),
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
            .map(|dir| format!("JSONL to {}", dir.display())),
        hotkey_backend_label,
    });

    // Worker thread takes exclusive ownership of `engine`. `crispasr::Session`
    // is `Send` but not `Sync`, which is fine: only one thread ever calls
    // `transcribe`, and the hotkey path only posts transitions on a channel.
    let (tx, rx) = bounded::<WorkerEvent>(WORKER_QUEUE_CAPACITY);
    let worker = spawn_worker(WorkerCtx {
        engine,
        recipe,
        model_idle_minutes: start.effective_model_idle_minutes(&config),
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
        lifetime: worker_lifetime,
    });
    let (hotkey_tx, hotkey_rx) = unbounded();
    let coordinator = match daemon::recording::spawn_recording_coordinator(
        hotkey_rx,
        tx,
        audio,
        Arc::clone(&log),
        Arc::clone(&ipc_state.activity),
    )
    .context("spawn recording coordinator")
    {
        Ok(coordinator) => coordinator,
        Err(err) => {
            log.error(&format!("{err:#}"));
            ipc_state.shutdown.exit_after_worker(&ipc_state.activity, 1);
        }
    };

    // Hotkey grab loop. Blocks forever (until grab returns or process exits).
    ipc_state.set_phase("idle");
    ipc_state.activity.ready();
    log.ready();

    finish_hotkey_loop(
        daemon::hotkey::run_grab_loop(hotkey_tx, hotkey_backend, Arc::clone(&log)),
        &ipc_state,
    );

    // Tear down.
    let _ = coordinator.join();
    let _ = worker.join();
    Ok(())
}

fn ensure_existing_daemon_responsive(responsive: bool) -> Result<()> {
    anyhow::ensure!(
        responsive,
        "daemon singleton lock is held, but its control endpoint is unavailable; retry shortly or inspect the parakit process"
    );
    Ok(())
}

/// Release worker-owned native resources before exiting on a hotkey failure.
///
/// # Arguments
///
/// * `result` - Outcome of the blocking hotkey loop.
/// * `state` - Shared daemon state used to coordinate worker shutdown.
pub(crate) fn finish_hotkey_loop(
    result: Result<(), daemon::hotkey::HotkeyLoopFailed>,
    state: &daemon::ipc::SharedState,
) {
    if result.is_err() {
        state.shutdown.exit_after_worker(&state.activity, 2);
    }
}

/// Convert the CLI's 1-based `copy-last` index into the wire protocol's
/// 0-based index.
///
/// # Returns
///
/// `0` (most recent) when `index` is `None`, otherwise `index - 1`.
///
/// # Errors
///
/// Returns an error when `index` is `Some(0)`: transcript numbering starts
/// at 1.
fn wire_history_index(index: Option<usize>) -> Result<usize> {
    match index {
        None => Ok(0),
        Some(0) => anyhow::bail!("transcript index starts at 1; 1 is the most recent"),
        Some(n) => Ok(n - 1),
    }
}

/// Reject `history --limit 0` before it reaches the daemon.
///
/// Without this check, `--limit 0` sails through to `history_snapshot` and
/// comes back as an empty list, which `print_history` then reports as "no
/// transcripts remembered in this daemon session" — a lie when the daemon
/// does remember transcripts and the caller simply asked for zero of them.
///
/// # Arguments
///
/// * `limit` - Value of `--limit` as parsed from the CLI.
///
/// # Returns
///
/// `limit` unchanged: `None`, or `Some(n)` with `n >= 1`.
///
/// # Errors
///
/// Returns an error when `limit` is `Some(0)`.
fn wire_history_limit(limit: Option<usize>) -> Result<Option<usize>> {
    match limit {
        Some(0) => anyhow::bail!("history --limit must be at least 1"),
        other => Ok(other),
    }
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

fn build_cli_cleaner(start: &StartCli, config: &ConfigFile) -> Result<Option<rules::Cleaner>> {
    rules::build_cleaner(
        !start.effective_cleaning_enabled(config),
        start.effective_cleaning_profile(config),
        start.effective_drops_trailing_period(config),
        config.cleaning.number_threshold,
        &start.effective_disabled_rules(config),
        &config.rules.user,
    )
}

/// Build the cleaner for `rules list`/`rules test`. Unlike [`build_cli_cleaner`],
/// cleaning is never disabled here: there is no `--cleaning`/`--no-cleaning`
/// under `rules`, since testing or listing rules with cleaning off is
/// meaningless.
fn build_rules_cleaner(args: &RulesArgs, config: &ConfigFile) -> Result<rules::Cleaner> {
    rules::build_enabled_cleaner(
        args.effective_cleaning_profile(config),
        args.effective_drops_trailing_period(config),
        config.cleaning.number_threshold,
        &args.effective_disabled_rules(config),
        &config.rules.user,
    )
}

fn model_dtype_label(path: &std::path::Path) -> String {
    let dtype = gguf::dtype_label(path);
    let size = path
        .metadata()
        .ok()
        .map(|meta| format!(" ({})", format_file_size(meta.len())))
        .unwrap_or_default();
    format!("{dtype}{size}")
}

fn open_cli_engine(
    start: &StartCli,
    config: &ConfigFile,
    verbose: bool,
    fetch_quiet: bool,
    log: &Logger,
) -> Result<OpenedEngine> {
    let engine_config = resolve_engine_config(
        start,
        config,
        || fetch::ensure_default_model_with_verbosity(fetch_quiet, verbose),
        log,
    )?;
    let model_path = engine_config.model_path;
    let recipe = EngineRecipe {
        model_path: std::path::absolute(&model_path)?,
        threads: engine_config.threads,
        device_mode: engine_config.device_mode,
        verbose,
    };
    let (engine, device_summary) = recipe.open(log)?;
    Ok(OpenedEngine {
        model_path,
        engine,
        device_summary,
        recipe,
    })
}

struct OpenedEngine {
    recipe: EngineRecipe,
    model_path: PathBuf,
    engine: Engine,
    device_summary: String,
}

#[derive(Debug)]
struct EngineConfig {
    model_path: PathBuf,
    threads: usize,
    device_mode: DeviceMode,
}

fn resolve_engine_config<F>(
    start: &StartCli,
    config: &ConfigFile,
    fetch_default_model: F,
    log: &Logger,
) -> Result<EngineConfig>
where
    F: FnOnce() -> Result<PathBuf>,
{
    resolve_engine_config_with_validator(start, config, fetch_default_model, |device_mode| {
        validate_device_request(device_mode, log)
    })
}

fn resolve_engine_config_with_validator<F, V>(
    start: &StartCli,
    config: &ConfigFile,
    fetch_default_model: F,
    validate_device: V,
) -> Result<EngineConfig>
where
    F: FnOnce() -> Result<PathBuf>,
    V: FnOnce(DeviceMode) -> Result<()>,
{
    let device_mode = start.effective_device(config);
    validate_device(device_mode)?;
    let model_path = match start.effective_model(config) {
        Some(path) => path,
        None => fetch_default_model()?,
    };
    let threads = start
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

fn run_cache_command(cache: &CacheCli, quiet: bool) -> Result<()> {
    match cache.command.as_ref().unwrap_or(&CacheCommand::List) {
        CacheCommand::Dir => {
            let dir = model::models_dir()?;
            if !quiet {
                println!("{}", dir.display());
            }
        }
        CacheCommand::List => print_cache_list(quiet)?,
    }
    Ok(())
}

fn print_cache_list(quiet: bool) -> Result<()> {
    let dir = model::models_dir()?;
    if quiet {
        return Ok(());
    }
    println!("parakit cache");
    println!("  dir: {}", dir.display());
    if !dir.is_dir() {
        println!("  models: none");
        return Ok(());
    }

    let top_level = list_gguf_files(&dir);
    let mut extra = list_nested_gguf_files(&dir.join("hub"));
    extra.extend(list_nested_gguf_files(&dir.join("url")));
    extra.sort();

    if top_level.is_empty() && extra.is_empty() {
        println!("  models: none");
        return Ok(());
    }

    println!("  models:");
    for path in &top_level {
        print_cache_entry(&dir, path, &model_file_name(path));
    }
    for path in &extra {
        print_cache_entry(&dir, path, &relative_cache_display(&dir, path));
    }
    Ok(())
}

/// List `.gguf` files directly inside `dir`, sorted.
fn list_gguf_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = read
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
        })
        .collect();
    entries.sort();
    entries
}

/// List `.gguf` files one level below `parent` (`parent/*/*.gguf`), sorted.
/// Used for revision-keyed Hub entries and URL-keyed direct downloads.
fn list_nested_gguf_files(parent: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = read
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .flat_map(|sub| list_gguf_files(&sub))
        .collect();
    entries.sort();
    entries
}

/// Display a `hub/…`/`url/…` cache entry by its path relative to `dir`,
/// with `/` separators regardless of platform.
fn relative_cache_display(dir: &Path, path: &Path) -> String {
    path.strip_prefix(dir)
        .map(model::slash_joined)
        .unwrap_or_else(|_| model_file_name(path))
}

fn print_cache_entry(dir: &Path, path: &Path, display_name: &str) {
    let dtype = gguf::dtype_label(path);
    let size = path
        .metadata()
        .map(|meta| format_file_size(meta.len()))
        .unwrap_or_else(|_| "unknown size".to_string());
    let is_default_q8 = model_file_name(path) == model::Q8_FILENAME && path.parent() == Some(dir);
    let default_marker = if is_default_q8 { " default" } else { "" };
    println!("    {display_name}{default_marker}: {dtype}, {size}");
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

#[cfg(test)]
#[path = "app_tests.rs"]
mod app_tests;
