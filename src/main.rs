//! parakit - a push-to-talk dictation daemon.
//!
//! Architecture:
//!   - Main thread: parse CLI, set up subsystems, then run the rdev grab loop.
//!     The grab loop is blocking and runs forever until SIGINT.
//!   - cpal audio thread: continuously captures from the mic, appends to a
//!     shared buffer when `recording` is true.
//!   - Worker thread: receives Event messages via crossbeam-channel, runs
//!     transcription off the hotkey thread so input stays responsive.
//!
//! State machine (single-recording-at-a-time invariant):
//!   Idle --[Ctrl+Space down]--> Recording --[Ctrl+Space up]--> Transcribing --> Idle
//!
//! In streaming mode there's an additional periodic Tick that sends partial
//! chunks to the worker while recording is active.
//!
//! NOTE: rdev::grab on Linux requires X11 (Wayland blocks synthetic input
//! interception). On macOS it requires Accessibility permission. Windows
//! works out of the box.

mod daemon;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use crossbeam_channel::{bounded, Receiver, Sender};
use parakit::data_log::{DataLogger, LogFormat};
use parakit::fetch::{self, FetchOptions};
use parakit::gguf;
use parakit::inference::{Engine, Mode};
use parakit::model;
use parakit::rules::{self, Cleaner};
use rdev::{Event, EventType, Key};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::daemon::audio::{AudioCapture, AudioHandle, TARGET_RATE};
use crate::daemon::inject::Injector;
use crate::daemon::sounds::Sounds;

// =============================================================================
// CLI
// =============================================================================

#[derive(Parser, Debug)]
#[command(
    name = "parakit",
    version,
    about = "Push-to-talk dictation daemon (Parakeet-TDT via CrispASR).",
    long_about = "Push-to-talk dictation daemon. Hold Ctrl+Space to record, release to transcribe and inject text at the cursor.\n\nDefault mode is verbose (prints raw + cleaned text). Pass --quiet for daemon mode."
)]
struct Cli {
    /// Subcommand to run instead of the push-to-talk daemon.
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to a GGUF model file. Overrides the cached Q8_0 model.
    #[arg(short = 'm', long, value_name = "PATH")]
    model: Option<PathBuf>,

    /// Inference mode. `batch` (default) records all audio then transcribes once.
    /// `streaming` transcribes chunks during recording (experimental, finicky).
    /// `streaming:N` sets chunk seconds (default 4.0).
    #[arg(long, default_value = "batch")]
    mode: String,

    /// Quiet mode: suppress stdout. Errors and warnings still go to stderr.
    /// Suitable for backgrounding the daemon.
    #[arg(long, short = 'q')]
    quiet: bool,

    /// Disable the audio cues (start / success / error tones).
    #[arg(long)]
    no_sounds: bool,

    /// Disable all text cleaning rules (raw transcript injected as-is).
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

    /// Override the push-to-talk hotkey. Currently only `ctrl+space` is supported.
    /// (Left as a flag for future expansion; changing it requires editing the
    /// keymap in main.)
    #[arg(long, default_value = "ctrl+space", hide = true)]
    hotkey: String,

    /// Directory for transcription logs. One file is written per local day.
    #[arg(long, value_name = "DIR")]
    log_dir: Option<PathBuf>,

    /// Transcription log format. Used only when --log-dir is set.
    #[arg(long, default_value = "jsonl", value_parser = clap::value_parser!(LogFormat))]
    log_format: LogFormat,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Download, convert, and quantize the official Parakeet model to Q8_0.
    Fetch(FetchCli),
}

#[derive(Args, Debug)]
struct FetchCli {
    /// Ignore cached artifacts and rebuild every step.
    #[arg(long)]
    force: bool,

    /// Keep the downloaded 2.4 GB .nemo checkpoint after Q8_0 is produced.
    #[arg(long)]
    keep_nemo: bool,

    /// Keep the intermediate F16 GGUF after Q8_0 is produced.
    #[arg(long)]
    keep_f16: bool,
}

// =============================================================================
// Events sent to the worker thread
// =============================================================================

enum Event_ {
    /// Hotkey pressed: start a new recording session.
    Start,
    /// Hotkey released: finalize the recording, transcribe, type.
    Stop,
    /// Streaming mode only: a chunk boundary was reached. Snapshot the buffer
    /// from `consumed_samples` and transcribe that slice.
    StreamChunk,
    /// Shutdown.
    Quit,
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
    let cli = Cli::parse();

    if let Some(Commands::Fetch(fetch_cli)) = &cli.command {
        fetch::run(FetchOptions {
            force: fetch_cli.force,
            keep_nemo: fetch_cli.keep_nemo,
            keep_f16: fetch_cli.keep_f16,
        })?;
        return Ok(());
    }

    // Special command modes: print rules / test rules.
    if cli.list_rules {
        if !cli.quiet {
            rules::print_rule_list();
        }
        return Ok(());
    }
    if let Some(input) = &cli.test_rules {
        let disabled: HashSet<String> = cli.disable_rule.iter().cloned().collect();
        for name in &cli.disable_rule {
            rules::assert_rule_name_exists(name)?;
        }
        let cleaner = if cli.no_cleaning {
            None
        } else {
            Some(Cleaner::new(&disabled)?)
        };
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

    let mode = Mode::parse(&cli.mode)?;
    let disabled: HashSet<String> = cli.disable_rule.iter().cloned().collect();
    for name in &cli.disable_rule {
        rules::assert_rule_name_exists(name)?;
    }
    let cleaner = if cli.no_cleaning {
        None
    } else {
        Some(Arc::new(Cleaner::new(&disabled)?))
    };
    let rules_active = cleaner.as_deref().map_or(0, Cleaner::len);
    let data_log = cli
        .log_dir
        .clone()
        .map(|dir| Arc::new(DataLogger::new(dir, cli.log_format)));

    let sounds = Sounds::new(!cli.no_sounds);
    let model_path = model::resolve_model_path(cli.model.as_deref())?;
    let model_dtype = model_dtype_label(&model_path);
    let engine = Engine::open(&model_path)
        .with_context(|| format!("could not open model {}", model_path.display()))?;
    let capture = AudioCapture::open()?;
    let audio = capture.handle.clone();
    let log = Arc::new(Logger::new(!cli.quiet));

    // Banner.
    log.banner(
        &cli,
        &model_path,
        &model_dtype,
        &mode,
        cleaner.as_deref(),
        &capture,
    );

    // Worker thread takes exclusive ownership of `engine`. `crispasr::Session`
    // is `Send` but not `Sync`, which is fine: only one thread ever calls
    // `transcribe`, and the grab callback / streaming ticker only post
    // events on a channel.
    let (tx, rx) = bounded::<Event_>(64);
    let worker = spawn_worker(WorkerCtx {
        engine,
        audio: audio.clone(),
        cleaner: cleaner.clone(),
        data_log: data_log.clone(),
        rules_active,
        sounds: sounds.clone(),
        log: Arc::clone(&log),
        mode,
        rx,
    });

    // Streaming chunk timer (if applicable).
    let streaming_alive = Arc::new(AtomicBool::new(true));
    let streaming_thread = if let Mode::Streaming { chunk_secs } = mode {
        Some(spawn_streaming_ticker(
            tx.clone(),
            audio.clone(),
            Arc::clone(&streaming_alive),
            chunk_secs,
        ))
    } else {
        None
    };

    // SIGINT handler - set Quit on Ctrl+C in the terminal.
    let tx_sig = tx.clone();
    let streaming_alive_sig = Arc::clone(&streaming_alive);
    ctrlc_handler(move || {
        let _ = tx_sig.send(Event_::Quit);
        streaming_alive_sig.store(false, Ordering::SeqCst);
    });

    // Hotkey grab loop. Blocks forever (until grab returns or process exits).
    log.line("Ready: hold Ctrl+Space to dictate. Ctrl+C in this terminal to exit.");

    run_grab_loop(tx, audio);

    // Tear down.
    if let Some(t) = streaming_thread {
        streaming_alive.store(false, Ordering::SeqCst);
        let _ = t.join();
    }
    let _ = worker.join();
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

// =============================================================================
// Hotkey grab
// =============================================================================
//
// rdev::grab gives us a callback per event with the option to suppress
// passthrough by returning None. We track Ctrl modifier state and Space
// edges. When Ctrl is held and Space is pressed, we start; when Space is
// released (regardless of Ctrl), we stop.
//
// Static state is unfortunately required because rdev::grab takes a
// `Fn(Event) -> Option<Event>` and runs it across multiple thread contexts
// in the listener implementation. We use atomics + a shared sender.

use once_cell::sync::OnceCell;
static GRAB_TX: OnceCell<Sender<Event_>> = OnceCell::new();
static GRAB_AUDIO: OnceCell<AudioHandle> = OnceCell::new();
static CTRL_HELD: AtomicBool = AtomicBool::new(false);
static SPACE_HELD: AtomicBool = AtomicBool::new(false);

fn run_grab_loop(tx: Sender<Event_>, audio: AudioHandle) {
    let _ = GRAB_TX.set(tx);
    let _ = GRAB_AUDIO.set(audio);

    if let Err(e) = rdev::grab(grab_callback) {
        eprintln!(
            "parakit: rdev::grab failed: {:?}\n\
             On Linux, this requires X11 (not Wayland) and may need\n\
             your user added to the `input` group:\n  sudo usermod -aG input $USER\n\
             On macOS, grant Accessibility + Input Monitoring permissions.\n\
             On Windows, just rerun.",
            e
        );
        std::process::exit(2);
    }
}

fn grab_callback(event: Event) -> Option<Event> {
    match event.event_type {
        EventType::KeyPress(Key::ControlLeft) | EventType::KeyPress(Key::ControlRight) => {
            CTRL_HELD.store(true, Ordering::SeqCst);
            Some(event)
        }
        EventType::KeyRelease(Key::ControlLeft) | EventType::KeyRelease(Key::ControlRight) => {
            CTRL_HELD.store(false, Ordering::SeqCst);
            // If user released Ctrl while still holding Space, end the recording.
            if SPACE_HELD.swap(false, Ordering::SeqCst) {
                if let Some(tx) = GRAB_TX.get() {
                    let _ = tx.send(Event_::Stop);
                }
                return None;
            }
            Some(event)
        }
        EventType::KeyPress(Key::Space) => {
            if CTRL_HELD.load(Ordering::SeqCst) {
                if !SPACE_HELD.swap(true, Ordering::SeqCst) {
                    if let Some(audio) = GRAB_AUDIO.get() {
                        audio.start_recording();
                    }
                    if let Some(tx) = GRAB_TX.get() {
                        let _ = tx.send(Event_::Start);
                    }
                }
                return None; // suppress so the literal space doesn't reach apps
            }
            Some(event)
        }
        EventType::KeyRelease(Key::Space) => {
            if SPACE_HELD.swap(false, Ordering::SeqCst) {
                if let Some(tx) = GRAB_TX.get() {
                    let _ = tx.send(Event_::Stop);
                }
                return None;
            }
            Some(event)
        }
        _ => Some(event),
    }
}

// =============================================================================
// Worker thread
// =============================================================================

struct WorkerCtx {
    engine: Engine,
    audio: AudioHandle,
    cleaner: Option<Arc<Cleaner>>,
    data_log: Option<Arc<DataLogger>>,
    rules_active: usize,
    sounds: Sounds,
    log: Arc<Logger>,
    mode: Mode,
    rx: Receiver<Event_>,
}

fn spawn_worker(ctx: WorkerCtx) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || worker_loop(ctx))
}

fn worker_loop(ctx: WorkerCtx) {
    let WorkerCtx {
        engine,
        audio,
        cleaner,
        data_log,
        rules_active,
        sounds,
        log,
        mode,
        rx,
    } = ctx;

    let mut consumed_samples: usize = 0; // for streaming
    let mut recording_started_at: Option<Instant> = None;

    while let Ok(ev) = rx.recv() {
        match ev {
            Event_::Start => {
                consumed_samples = 0;
                recording_started_at = Some(Instant::now());
                sounds.start();
                log.line("parakit: recording...");
            }
            Event_::StreamChunk => {
                if let Mode::Streaming { .. } = mode {
                    let chunk = audio.snapshot_from(consumed_samples);
                    if !chunk.is_empty() {
                        consumed_samples += chunk.len();
                        let infer_started = Instant::now();
                        match engine.transcribe(&chunk) {
                            Ok(raw) if !raw.trim().is_empty() => {
                                let infer_elapsed = infer_started.elapsed();
                                let cleaned = match &cleaner {
                                    Some(c) => c.clean(&raw),
                                    None => raw.clone(),
                                };
                                if let Some(data_log) = &data_log {
                                    let chunk_secs = chunk.len() as f32 / TARGET_RATE as f32;
                                    data_log.log(
                                        chunk_secs,
                                        infer_elapsed,
                                        &raw,
                                        &cleaned,
                                        rules_active,
                                    );
                                }
                                log.streaming_partial(&raw, &cleaned);
                                if let Err(e) = type_text(&cleaned) {
                                    log.error(&format!("type failed: {e:#}"));
                                    sounds.error();
                                }
                            }
                            Ok(_) => {}
                            Err(e) => {
                                log.error(&format!("transcribe (chunk) failed: {e:#}"));
                            }
                        }
                    }
                }
            }
            Event_::Stop => {
                let dur_audio = recording_started_at
                    .take()
                    .map(|t| t.elapsed())
                    .unwrap_or(Duration::ZERO);

                let pcm = audio.stop_recording();

                // In streaming mode we may have already injected most of the
                // audio. Only transcribe the unconsumed tail.
                let to_transcribe: &[f32] = match mode {
                    Mode::Streaming { .. } => &pcm[consumed_samples.min(pcm.len())..],
                    Mode::Batch => &pcm,
                };

                let secs = pcm.len() as f32 / TARGET_RATE as f32;
                log.line(&format!(
                    "parakit: transcribing ({:.2}s audio, {:.2}s wall)...",
                    secs,
                    dur_audio.as_secs_f32()
                ));

                let infer_started = Instant::now();
                match engine.transcribe(to_transcribe) {
                    Ok(raw) if !raw.trim().is_empty() => {
                        let infer_elapsed = infer_started.elapsed();
                        let cleaned = match &cleaner {
                            Some(c) => c.clean(&raw),
                            None => raw.clone(),
                        };
                        if let Some(data_log) = &data_log {
                            data_log.log(secs, infer_elapsed, &raw, &cleaned, rules_active);
                        }
                        log.transcript(&raw, &cleaned, infer_elapsed);
                        match type_text(&cleaned) {
                            Ok(_) => sounds.success(),
                            Err(e) => {
                                log.error(&format!("type failed: {e:#}"));
                                sounds.error();
                            }
                        }
                    }
                    Ok(_) => {
                        log.line("parakit: no speech detected");
                        sounds.success();
                    }
                    Err(e) => {
                        log.error(&format!("transcribe failed: {e:#}"));
                        sounds.error();
                    }
                }
            }
            Event_::Quit => break,
        }
    }
}

fn type_text(text: &str) -> Result<()> {
    let mut injector = Injector::new()?;
    injector.type_text(text)
}

// =============================================================================
// Streaming ticker
// =============================================================================

fn spawn_streaming_ticker(
    tx: Sender<Event_>,
    audio: AudioHandle,
    alive: Arc<AtomicBool>,
    chunk_secs: f32,
) -> std::thread::JoinHandle<()> {
    let chunk_samples = (chunk_secs * TARGET_RATE as f32) as usize;
    std::thread::spawn(move || {
        let mut last_len = 0usize;
        while alive.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(250));
            let cur = audio.len();
            if cur >= last_len + chunk_samples {
                last_len = cur;
                let _ = tx.send(Event_::StreamChunk);
            }
            if cur < last_len {
                last_len = 0;
            }
        }
    })
}

// =============================================================================
// SIGINT
// =============================================================================

fn ctrlc_handler<F: FnMut() + Send + 'static>(mut f: F) {
    // Minimal handler: spawn a thread that polls a flag set by signal-hook,
    // or just use the Rust stdlib if available. We'll use ctrlc-equivalent
    // via a thread that catches Ctrl+C.
    //
    // To avoid adding the `ctrlc` crate, we use a simple SIGINT handler via
    // signal-hook would still be a dep. Instead, the rdev::grab loop holds
    // the main thread; the user can also send SIGTERM. For a more robust
    // shutdown in production, add a `ctrlc` dep.
    let _ = std::thread::Builder::new()
        .name("parakit-sigwait".into())
        .spawn(move || {
            // Best-effort: just sleep forever. The OS will tear down the
            // process on Ctrl+C since we don't install a handler.
            // If you need cleaner shutdown, add the `ctrlc` crate and call it
            // here.
            loop {
                std::thread::sleep(Duration::from_secs(3600));
                f(); // never reached in this minimal impl
            }
        });
}

// =============================================================================
// Logging
// =============================================================================

struct Logger {
    verbose: bool,
}

impl Logger {
    fn new(verbose: bool) -> Self {
        Self { verbose }
    }

    fn line(&self, msg: &str) {
        if self.verbose {
            println!("{}", msg);
        }
    }

    fn error(&self, msg: &str) {
        // Errors always go to stderr regardless of --quiet.
        eprintln!("parakit: error: {msg}");
    }

    fn transcript(&self, raw: &str, cleaned: &str, infer: Duration) {
        if !self.verbose {
            return;
        }
        if raw == cleaned {
            println!("Text: {}  ({:.0}ms)", cleaned, infer.as_secs_f32() * 1000.0);
        } else {
            println!("Raw:    {raw}");
            println!(
                "Clean:  {}  ({:.0}ms)",
                cleaned,
                infer.as_secs_f32() * 1000.0
            );
        }
    }

    fn streaming_partial(&self, raw: &str, cleaned: &str) {
        if !self.verbose {
            return;
        }
        let raw = raw.trim();
        let cleaned = cleaned.trim();
        if raw == cleaned {
            println!("+  {}", cleaned);
        } else {
            println!("+  {}  =>  {}", raw, cleaned);
        }
    }

    fn banner(
        &self,
        cli: &Cli,
        model_path: &std::path::Path,
        model_dtype: &str,
        mode: &Mode,
        cleaner: Option<&Cleaner>,
        capture: &AudioCapture,
    ) {
        if !self.verbose {
            return;
        }
        println!("parakit");
        println!("  model:    {}", model_path.display());
        println!("  dtype:    {}", model_dtype);
        println!("  mode:     {:?}", mode);
        println!(
            "  cleaning: {}",
            match cleaner {
                Some(c) => format!("on ({} rules)", c.len()),
                None => "off".to_string(),
            }
        );
        println!("  sounds:   {}", if cli.no_sounds { "off" } else { "on" });
        println!(
            "  logging:  {}",
            match &cli.log_dir {
                Some(dir) => format!("{:?} to {}", cli.log_format, dir.display()),
                None => "off".to_string(),
            }
        );
        println!(
            "  audio:    {} Hz hardware{}, {} Hz target",
            capture.hw_rate,
            if capture.resampling {
                " (resampling)"
            } else {
                ""
            },
            TARGET_RATE
        );
    }
}
