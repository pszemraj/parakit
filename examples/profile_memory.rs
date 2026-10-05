//! Repeated real Engine sessions with in-process native memory measurements.
//!
//! This measures the inference allocation lifecycle, not hotkeys or insertion.
//! A background sampler tracks interval peaks while the main thread opens,
//! exercises, and drops the real Engine session.

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use parakit::audio_file::prepare_wav_for_model;
use parakit::inference::{DeviceMode, Engine};
use parakit::warmup;
use serde_json::{json, Map, Value};
use std::env;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SAMPLE_INTERVAL: Duration = Duration::from_millis(20);
const NVIDIA_SAMPLE_INTERVAL: Duration = Duration::from_millis(50);
const NVIDIA_QUERY_TIMEOUT: Duration = Duration::from_secs(2);
const NVIDIA_CHECKPOINT_TIMEOUT: Duration = Duration::from_secs(5);
const OFFLOAD_SETTLE: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum ReloadWarmup {
    #[default]
    Production,
    Startup,
    OneSecond,
    None,
}

impl ReloadWarmup {
    fn sequence(self, device: DeviceMode, has_gpu: bool, cycle: usize) -> &'static [usize] {
        // Preserve startup initialization so comparisons isolate same-process reloads.
        match (cycle, self) {
            (1, _) | (_, Self::Startup) => warmup::engine_warmup_seconds(device, has_gpu),
            (_, Self::Production) => warmup::reload_warmup_seconds(),
            (_, Self::OneSecond) => &[1],
            (_, Self::None) => &[],
        }
    }
}

#[derive(Debug, Parser)]
#[command(about = "Measure retained and idle-offloaded inference memory")]
struct Cli {
    /// New directory for metadata and memory readings.
    #[arg(long)]
    output: PathBuf,
    /// Local GGUF file; this tool never downloads a model.
    #[arg(long)]
    model: PathBuf,
    /// Real WAV used for the short and full transcription checkpoints.
    #[arg(long)]
    audio: PathBuf,
    /// Requested inference device.
    #[arg(long, value_enum, default_value = "auto")]
    device: DeviceMode,
    /// CPU inference threads.
    #[arg(long, default_value = "8")]
    threads: NonZeroUsize,
    /// Number of open/transcribe/close cycles.
    #[arg(long, default_value = "10")]
    cycles: NonZeroUsize,
    /// Measure the resident baseline instead of closing between cycles.
    #[arg(long)]
    keep_loaded: bool,
    /// Save macOS `vmmap -summary` output at every checkpoint.
    #[arg(long)]
    vmmap: bool,
    /// Sample per-process NVIDIA allocations at checkpoints and during operations.
    #[arg(long)]
    nvidia: bool,
    /// Experimental reload warmup; the first session always uses startup policy.
    #[arg(
        long,
        value_enum,
        default_value = "production",
        conflicts_with = "keep_loaded"
    )]
    reload_warmup: ReloadWarmup,
    /// Measure a full recording as the first real inference after every open.
    #[arg(long)]
    full_first: bool,
}

struct Profiler {
    output: PathBuf,
    metrics: BufWriter<File>,
    sampler: PeakSampler,
    nvidia_sampler: Option<NvidiaPeakSampler>,
    vmmap: bool,
}

impl Profiler {
    fn new(cli: &Cli) -> Result<Self> {
        validate_input(&cli.model, "model")?;
        validate_input(&cli.audio, "audio")?;
        if cli.output.exists() {
            bail!("output directory already exists: {}", cli.output.display());
        }
        fs::create_dir_all(&cli.output)
            .with_context(|| format!("create output directory {}", cli.output.display()))?;

        let metadata = json!({
            "command": env::args().collect::<Vec<_>>(),
            "platform": platform_identity(),
            "started_unix": unix_time()?,
            "model": file_identity(&cli.model)?,
            "audio": file_identity(&cli.audio)?,
            "commit": git_text(&["rev-parse", "HEAD"]),
            "crispasr": git_text(&["submodule", "status", "vendor/CrispASR"]),
            "working_tree": git_text(&["status", "--short"]),
            "measurement_scope": "self",
            "sampling_interval_seconds": SAMPLE_INTERVAL.as_secs_f64(),
            "offload_settle_seconds": OFFLOAD_SETTLE.as_secs_f64(),
        });
        let mut encoded = serde_json::to_vec_pretty(&metadata)?;
        encoded.push(b'\n');
        fs::write(cli.output.join("metadata.json"), encoded)?;

        Ok(Self {
            output: cli.output.clone(),
            metrics: BufWriter::new(File::create(cli.output.join("metrics.jsonl"))?),
            sampler: PeakSampler::start(SAMPLE_INTERVAL),
            nvidia_sampler: cli.nvidia.then(NvidiaPeakSampler::start),
            vmmap: cli.vmmap,
        })
    }

    fn checkpoint(&mut self, cli: &Cli, cycle: usize, phase: &str, elapsed_ms: f64) -> Result<()> {
        let sampled_at = Instant::now();
        let host = host_memory()?;
        self.sampler.observe_checkpoint(&host, sampled_at)?;
        let peak = host_interval_peak(&host, self.sampler.snapshot()?);
        let mut record = json!({
            "pid": std::process::id(),
            "cycle": cycle,
            "phase": phase,
            "elapsed_ms": elapsed_ms,
            "device": cli.device.as_str(),
            "threads": cli.threads.get(),
            "keep_loaded": cli.keep_loaded,
            "reload_warmup": format!("{:?}", cli.reload_warmup),
            "full_first": cli.full_first,
            "sampled_unix": unix_time()?,
            "host": host,
            "host_interval_peak": peak,
        });
        let object = record
            .as_object_mut()
            .context("checkpoint record should be a JSON object")?;
        if let Some(sampler) = &self.nvidia_sampler {
            let (reading, peak) = sampler.checkpoint()?;
            object.insert("nvidia".into(), reading);
            object.insert("nvidia_interval_peak".into(), peak);
        }
        if self.vmmap && cfg!(target_os = "macos") {
            object.insert("vmmap".into(), self.save_vmmap(cycle, phase)?);
        }

        serde_json::to_writer(&mut self.metrics, &record)?;
        writeln!(self.metrics)?;
        self.metrics.flush()?;
        println!("{record}");
        io::stdout().flush()?;
        reset_host_interval_peak()?;
        self.sampler.reset()?;
        Ok(())
    }

    fn save_vmmap(&self, cycle: usize, phase: &str) -> Result<Value> {
        let name = format!("{cycle:02}-{phase}.vmmap.txt");
        let summary = Command::new("vmmap")
            .args(["-summary", &std::process::id().to_string()])
            .output()
            .context("run vmmap")?;
        let mut output = summary.stdout;
        output.extend_from_slice(&summary.stderr);
        fs::write(self.output.join(&name), output)?;
        Ok(json!({"file": name, "exit_code": summary.status.code()}))
    }
}

fn validate_input(path: &Path, label: &str) -> Result<()> {
    if !path.is_file() {
        bail!("{label} path is not a file: {}", path.display());
    }
    Ok(())
}

fn file_identity(path: &Path) -> Result<Value> {
    Ok(json!({
        "path": path.canonicalize()?.to_string_lossy(),
        "bytes": path.metadata()?.len(),
    }))
}

fn git_text(args: &[&str]) -> String {
    Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_default()
}

fn platform_identity() -> String {
    let command = if cfg!(windows) {
        Command::new("cmd").args(["/C", "ver"]).output()
    } else {
        Command::new("uname").arg("-a").output()
    };
    command
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("{} {}", env::consts::OS, env::consts::ARCH))
}

fn unix_time() -> Result<f64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before UNIX epoch")?
        .as_secs_f64())
}

#[derive(Debug)]
struct PeakState {
    started: Instant,
    interval: Duration,
    metric: Option<&'static str>,
    peak: u64,
    samples: u64,
}

impl PeakState {
    fn new(started: Instant, interval: Duration) -> Self {
        Self {
            started,
            interval,
            metric: None,
            peak: 0,
            samples: 0,
        }
    }

    fn observe(&mut self, metric: &'static str, value: u64, sampled_at: Instant) {
        if sampled_at < self.started {
            return;
        }
        if self.metric != Some(metric) {
            self.metric = Some(metric);
            self.peak = 0;
            self.samples = 0;
        }
        self.peak = self.peak.max(value);
        self.samples += 1;
    }

    fn snapshot(&self) -> Value {
        let mut snapshot = Map::new();
        if let Some(metric) = self.metric {
            snapshot.insert(metric.into(), json!(self.peak));
        }
        snapshot.insert("samples".into(), json!(self.samples));
        snapshot.insert(
            "interval_seconds".into(),
            json!(self.interval.as_secs_f64()),
        );
        Value::Object(snapshot)
    }

    fn reset_at(&mut self, started: Instant) {
        self.started = started;
        self.metric = None;
        self.peak = 0;
        self.samples = 0;
    }
}

struct PeakSampler {
    state: Arc<Mutex<PeakState>>,
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl PeakSampler {
    fn start(interval: Duration) -> Self {
        let state = Arc::new(Mutex::new(PeakState::new(Instant::now(), interval)));
        let thread_state = Arc::clone(&state);
        let (stop, stopped) = mpsc::channel();
        let thread = thread::spawn(move || loop {
            let sampled_at = Instant::now();
            if let Ok((metric, value)) = resident_sample() {
                if let Ok(mut state) = thread_state.lock() {
                    state.observe(metric, value, sampled_at);
                }
            }
            match stopped.recv_timeout(interval) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        });
        Self {
            state,
            stop,
            thread: Some(thread),
        }
    }

    fn observe_checkpoint(&self, host: &Value, sampled_at: Instant) -> Result<()> {
        let (metric, value) =
            resident_metric(host).context("host reading has no resident value")?;
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("peak sampler lock poisoned"))?
            .observe(metric, value, sampled_at);
        Ok(())
    }

    fn snapshot(&self) -> Result<Value> {
        Ok(self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("peak sampler lock poisoned"))?
            .snapshot())
    }

    fn reset(&self) -> Result<()> {
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("peak sampler lock poisoned"))?
            .reset_at(Instant::now());
        Ok(())
    }
}

impl Drop for PeakSampler {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn resident_metric(host: &Value) -> Option<(&'static str, u64)> {
    if let Some(value) = host.pointer("/status_bytes/VmRSS").and_then(Value::as_u64) {
        return Some(("rss_bytes", value));
    }
    if let Some(value) = host.get("working_set_bytes").and_then(Value::as_u64) {
        return Some(("working_set_bytes", value));
    }
    host.get("rss_bytes")
        .and_then(Value::as_u64)
        .map(|value| ("rss_bytes", value))
}

fn host_interval_peak(host: &Value, mut sampled: Value) -> Value {
    #[cfg(target_os = "linux")]
    if let Some(kernel_peak) = host.pointer("/status_bytes/VmHWM").and_then(Value::as_u64) {
        if let Some(object) = sampled.as_object_mut() {
            object.insert("rss_bytes".into(), json!(kernel_peak));
            object.insert("source".into(), json!("linux_VmHWM"));
        }
    }
    sampled
}

#[cfg(target_os = "linux")]
fn reset_host_interval_peak() -> Result<()> {
    fs::write("/proc/self/clear_refs", b"5\n").context("reset Linux VmHWM")
}

#[cfg(not(target_os = "linux"))]
fn reset_host_interval_peak() -> Result<()> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn resident_sample() -> Result<(&'static str, u64)> {
    let status = fs::read_to_string("/proc/self/status")?;
    let value = proc_kib_field(&status, "VmRSS").context("VmRSS missing from /proc/self/status")?;
    Ok(("rss_bytes", value))
}

#[cfg(target_os = "windows")]
fn resident_sample() -> Result<(&'static str, u64)> {
    Ok((
        "working_set_bytes",
        windows_memory()?["working_set_bytes"].as_u64().unwrap_or(0),
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn resident_sample() -> Result<(&'static str, u64)> {
    Ok(("rss_bytes", ps_rss()?))
}

#[cfg(target_os = "linux")]
fn host_memory() -> Result<Value> {
    Ok(json!({
        "smaps_rollup_bytes": proc_fields(&fs::read_to_string("/proc/self/smaps_rollup")?)?,
        "status_bytes": proc_fields(&fs::read_to_string("/proc/self/status")?)?,
    }))
}

#[cfg(target_os = "windows")]
fn host_memory() -> Result<Value> {
    windows_memory()
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn host_memory() -> Result<Value> {
    Ok(json!({"rss_bytes": ps_rss()?}))
}

#[cfg(target_os = "windows")]
fn windows_memory() -> Result<Value> {
    use std::mem::size_of;
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;

    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            std::ptr::addr_of_mut!(counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
    }
    .context("read current process memory")?;
    Ok(json!({
        "working_set_bytes": counters.WorkingSetSize,
        "peak_working_set_bytes": counters.PeakWorkingSetSize,
        "private_commit_bytes": counters.PrivateUsage,
    }))
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn ps_rss() -> Result<u64> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .context("run ps")?;
    if !output.status.success() {
        bail!("ps failed while reading process memory");
    }
    let kib: u64 = String::from_utf8_lossy(&output.stdout).trim().parse()?;
    Ok(kib * 1024)
}

#[cfg(any(target_os = "linux", test))]
fn proc_fields(input: &str) -> Result<Map<String, Value>> {
    let mut fields = Map::new();
    for line in input.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        if let [name, value, "kB"] = parts.as_slice() {
            let bytes = value
                .parse::<u64>()?
                .checked_mul(1024)
                .context("proc memory value overflow")?;
            fields.insert(name.trim_end_matches(':').to_owned(), json!(bytes));
        }
    }
    Ok(fields)
}

#[cfg(any(target_os = "linux", test))]
fn proc_kib_field(input: &str, wanted: &str) -> Option<u64> {
    input.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name != wanted {
            return None;
        }
        let mut parts = value.split_whitespace();
        let kib = parts.next()?.parse::<u64>().ok()?;
        (parts.next() == Some("kB")).then(|| kib * 1024)
    })
}

#[derive(Debug, PartialEq)]
struct NvidiaProcessRow {
    used_gpu_mib: Option<u64>,
    reported: String,
}

#[derive(Debug)]
struct NvidiaMemory {
    available: bool,
    process_rows: Vec<NvidiaProcessRow>,
    process_rows_complete: bool,
    error: Option<String>,
}

impl NvidiaMemory {
    fn unavailable(error: Option<String>) -> Self {
        Self {
            available: false,
            process_rows: Vec::new(),
            process_rows_complete: false,
            error,
        }
    }

    fn total_mib(&self) -> Option<u64> {
        if !self.available || !self.process_rows_complete || self.process_rows.is_empty() {
            return None;
        }
        self.process_rows
            .iter()
            .try_fold(0_u64, |total, row| total.checked_add(row.used_gpu_mib?))
    }

    fn to_json(&self) -> Value {
        let process_rows: Vec<_> = self
            .process_rows
            .iter()
            .map(|row| {
                json!({
                    "used_gpu_MiB": row.used_gpu_mib,
                    "reported": row.reported,
                })
            })
            .collect();
        json!({
            "available": self.available,
            "process_rows": process_rows,
            "process_rows_complete": self.process_rows_complete,
            "error": self.error,
        })
    }
}

fn parse_nvidia_process_rows(input: &str, pid: u32) -> (Vec<NvidiaProcessRow>, bool) {
    let wanted = pid.to_string();
    let mut rows = Vec::new();
    let mut matching_process = false;
    let mut complete = true;

    for line in input.lines() {
        let Some((label, value)) = line.split_once(':') else {
            continue;
        };
        match label.trim() {
            "Process ID" => {
                if matching_process {
                    complete = false;
                }
                matching_process = value.trim() == wanted;
            }
            "Used GPU Memory" if matching_process => {
                let reported = value.trim().to_owned();
                let used_gpu_mib = reported
                    .strip_suffix(" MiB")
                    .and_then(|value| value.parse().ok());
                rows.push(NvidiaProcessRow {
                    used_gpu_mib,
                    reported,
                });
                matching_process = false;
            }
            _ => {}
        }
    }
    if matching_process {
        complete = false;
    }
    (rows, complete)
}

enum TimedOutput {
    Completed(Output),
    TimedOut,
}

fn command_output_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> io::Result<TimedOutput> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait()?.is_some() {
            return child.wait_with_output().map(TimedOutput::Completed);
        }
        let now = Instant::now();
        if now >= deadline {
            let _ = child.kill();
            let _ = child.wait_with_output();
            return Ok(TimedOutput::TimedOut);
        }
        thread::sleep((deadline - now).min(Duration::from_millis(10)));
    }
}

fn nvidia_memory() -> NvidiaMemory {
    let mut command = Command::new("nvidia-smi");
    command.args(["-q", "-d", "PIDS"]);
    let output = match command_output_with_timeout(&mut command, NVIDIA_QUERY_TIMEOUT) {
        Ok(TimedOutput::Completed(output)) => output,
        Ok(TimedOutput::TimedOut) => {
            return NvidiaMemory::unavailable(Some(format!(
                "nvidia-smi timed out after {:.0}s",
                NVIDIA_QUERY_TIMEOUT.as_secs_f64()
            )));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return NvidiaMemory::unavailable(None);
        }
        Err(error) => return NvidiaMemory::unavailable(Some(error.to_string())),
    };
    let available = output.status.success();
    let (process_rows, process_rows_complete) =
        parse_nvidia_process_rows(&String::from_utf8_lossy(&output.stdout), std::process::id());
    NvidiaMemory {
        available,
        process_rows,
        process_rows_complete,
        error: (!output.stderr.is_empty())
            .then(|| String::from_utf8_lossy(&output.stderr).trim().to_owned()),
    }
}

struct NvidiaPeakSampler {
    commands: mpsc::Sender<NvidiaSamplerCommand>,
    thread: Option<JoinHandle<()>>,
}

enum NvidiaSamplerCommand {
    Checkpoint(mpsc::SyncSender<(Value, Value)>),
    Stop,
}

impl NvidiaPeakSampler {
    fn start() -> Self {
        let mut state = PeakState::new(Instant::now(), NVIDIA_SAMPLE_INTERVAL);
        let (commands, requests) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut next_sample = Instant::now();
            loop {
                let wait = next_sample.saturating_duration_since(Instant::now());
                match requests.recv_timeout(wait) {
                    Ok(NvidiaSamplerCommand::Checkpoint(reply)) => {
                        let sampled_at = Instant::now();
                        let reading = nvidia_memory();
                        if let Some(value) = reading.total_mib() {
                            state.observe("used_gpu_MiB", value, sampled_at);
                        }
                        let snapshot = state.snapshot();
                        state.reset_at(Instant::now());
                        let _ = reply.send((reading.to_json(), snapshot));
                        next_sample = Instant::now() + NVIDIA_SAMPLE_INTERVAL;
                    }
                    Ok(NvidiaSamplerCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let sampled_at = Instant::now();
                        let reading = nvidia_memory();
                        if let Some(value) = reading.total_mib() {
                            state.observe("used_gpu_MiB", value, sampled_at);
                        }
                        next_sample += NVIDIA_SAMPLE_INTERVAL;
                        if next_sample <= Instant::now() {
                            // A slow vendor query must yield to queued checkpoint
                            // requests before beginning another bounded query.
                            next_sample = Instant::now() + Duration::from_millis(1);
                        }
                    }
                }
            }
        });
        Self {
            commands,
            thread: Some(thread),
        }
    }

    fn checkpoint(&self) -> Result<(Value, Value)> {
        let (reply, result) = mpsc::sync_channel(1);
        self.commands
            .send(NvidiaSamplerCommand::Checkpoint(reply))
            .context("NVIDIA sampler stopped before checkpoint")?;
        result
            .recv_timeout(NVIDIA_CHECKPOINT_TIMEOUT)
            .context("NVIDIA sampler checkpoint timed out")
    }
}

impl Drop for NvidiaPeakSampler {
    fn drop(&mut self) {
        let _ = self.commands.send(NvidiaSamplerCommand::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let wav = prepare_wav_for_model(&cli.audio)?;
    let mut profiler = Profiler::new(&cli)?;
    profiler.checkpoint(&cli, 0, "before_load", 0.0)?;
    #[cfg(feature = "bundled")]
    if cli.device == DeviceMode::Gpu && !parakit::gpu::has_gpu_device() {
        bail!("GPU requested, but no GPU is available");
    }
    let mut engine = None;
    for cycle in 1..=cli.cycles.get() {
        if engine.is_none() {
            let started = Instant::now();
            let opened = Engine::open(&cli.model, cli.threads.get(), cli.device)?;
            profiler.checkpoint(
                &cli,
                cycle,
                "after_load",
                started.elapsed().as_secs_f64() * 1000.0,
            )?;
            #[cfg(feature = "bundled")]
            let has_gpu = cli.device != DeviceMode::Cpu && parakit::gpu::has_gpu_device();
            #[cfg(not(feature = "bundled"))]
            let has_gpu = false;
            let started = Instant::now();
            for seconds in cli.reload_warmup.sequence(cli.device, has_gpu, cycle) {
                opened.transcribe(&warmup::synthetic_pcm(*seconds))?;
            }
            profiler.checkpoint(
                &cli,
                cycle,
                "after_warmup",
                started.elapsed().as_secs_f64() * 1000.0,
            )?;
            engine = Some(opened);
        }
        let session = engine.as_ref().context("session should be loaded")?;
        let short = &wav.samples[..wav.samples.len().min(2 * 16000)];
        let mut clips = [
            ("after_short", short),
            ("after_full", wav.samples.as_slice()),
        ];
        if cli.full_first {
            clips.reverse();
        }
        for (phase, pcm) in clips {
            let started = Instant::now();
            let text = session.transcribe(pcm)?;
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            if text.trim().is_empty() {
                bail!("empty {phase} transcript at cycle {cycle}");
            }
            profiler.checkpoint(&cli, cycle, phase, elapsed_ms)?;
        }
        let started = Instant::now();
        if !cli.keep_loaded {
            drop(engine.take());
        }
        let close_elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        if !cli.keep_loaded {
            thread::sleep(OFFLOAD_SETTLE);
        }
        profiler.checkpoint(
            &cli,
            cycle,
            if cli.keep_loaded {
                "retained"
            } else {
                "offloaded"
            },
            close_elapsed_ms,
        )?;
    }
    drop(engine.take());
    profiler.checkpoint(&cli, cli.cycles.get(), "closed", 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_peak_reset_rejects_late_samples() {
        let start = Instant::now();
        let mut state = PeakState::new(start, SAMPLE_INTERVAL);
        state.observe("rss_bytes", 100, start);
        state.observe("rss_bytes", 40, start + Duration::from_millis(1));
        assert_eq!(state.snapshot()["rss_bytes"], 100);

        let next = start + Duration::from_secs(1);
        state.reset_at(next);
        state.observe("rss_bytes", 200, start + Duration::from_millis(2));
        state.observe("rss_bytes", 30, next);
        assert_eq!(state.snapshot()["rss_bytes"], 30);
        assert_eq!(state.snapshot()["samples"], 1);
    }

    #[test]
    fn interval_peak_preserves_windows_metric_name() {
        let start = Instant::now();
        let mut state = PeakState::new(start, SAMPLE_INTERVAL);
        state.observe("working_set_bytes", 50, start);
        assert_eq!(state.snapshot()["working_set_bytes"], 50);
        assert!(state.snapshot().get("rss_bytes").is_none());
    }

    #[test]
    fn nvidia_rows_sum_current_process_across_devices() {
        let input = r#"
        Process ID                        : 41
            Used GPU Memory               : 120 MiB
        Process ID                        : 99
            Used GPU Memory               : 999 MiB
        Process ID                        : 41
            Used GPU Memory               : 23 MiB
        "#;
        let (rows, complete) = parse_nvidia_process_rows(input, 41);
        let reading = NvidiaMemory {
            available: true,
            process_rows: rows,
            process_rows_complete: complete,
            error: None,
        };
        assert_eq!(reading.total_mib(), Some(143));
    }

    #[test]
    fn nvidia_unavailable_rows_never_become_zero() {
        for input in [
            "",
            "Process ID : 99\n    Used GPU Memory : 7 MiB",
            "Process ID : 41\n    Used GPU Memory : N/A",
            "Process ID : 41\n    Used GPU Memory : 7 MiB\nProcess ID : 41",
        ] {
            let (rows, complete) = parse_nvidia_process_rows(input, 41);
            let reading = NvidiaMemory {
                available: true,
                process_rows: rows,
                process_rows_complete: complete,
                error: None,
            };
            assert_eq!(reading.total_mib(), None, "input: {input:?}");
        }
        assert_eq!(
            NvidiaMemory::unavailable(Some("failed".into())).total_mib(),
            None
        );

        let (rows, complete) =
            parse_nvidia_process_rows("Process ID : 41\nUsed GPU Memory : 0 MiB", 41);
        let zero = NvidiaMemory {
            available: true,
            process_rows: rows,
            process_rows_complete: complete,
            error: None,
        };
        assert_eq!(zero.total_mib(), Some(0));
    }

    #[test]
    fn nvidia_interval_peak_keeps_transient_and_its_own_interval() {
        let start = Instant::now();
        let mut state = PeakState::new(start, NVIDIA_SAMPLE_INTERVAL);
        state.observe("used_gpu_MiB", 200, start);
        state.observe("used_gpu_MiB", 80, start + Duration::from_millis(1));
        assert_eq!(state.snapshot()["used_gpu_MiB"], 200);
        assert_eq!(
            state.snapshot()["interval_seconds"],
            NVIDIA_SAMPLE_INTERVAL.as_secs_f64()
        );

        let next = start + Duration::from_secs(1);
        state.reset_at(next);
        state.observe("used_gpu_MiB", 300, start + Duration::from_millis(2));
        state.observe("used_gpu_MiB", 70, next);
        assert_eq!(state.snapshot()["used_gpu_MiB"], 70);
        assert_eq!(state.snapshot()["samples"], 1);
    }

    #[test]
    fn proc_memory_fields_are_converted_to_bytes() {
        let input = "VmRSS:\t12 kB\nThreads:\t4\nPss: 3 kB\n";
        let fields = proc_fields(input).unwrap();
        assert_eq!(fields["VmRSS"], 12 * 1024);
        assert_eq!(fields["Pss"], 3 * 1024);
        assert!(!fields.contains_key("Threads"));
        assert_eq!(proc_kib_field(input, "VmRSS"), Some(12 * 1024));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_interval_peak_uses_kernel_high_water_mark() {
        let host = json!({"status_bytes": {"VmRSS": 40, "VmHWM": 120}});
        let sampled = json!({
            "rss_bytes": 80,
            "samples": 4,
            "interval_seconds": SAMPLE_INTERVAL.as_secs_f64(),
        });

        let peak = host_interval_peak(&host, sampled);

        assert_eq!(peak["rss_bytes"], 120);
        assert_eq!(peak["source"], "linux_VmHWM");
        assert_eq!(peak["samples"], 4);
    }

    #[test]
    fn command_timeout_kills_a_slow_child() {
        const CHILD: &str = "PARAKIT_PROFILE_TIMEOUT_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            thread::sleep(Duration::from_millis(250));
            return;
        }

        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "tests::command_timeout_kills_a_slow_child",
                "--nocapture",
            ])
            .env(CHILD, "1");
        let started = Instant::now();
        let outcome = command_output_with_timeout(&mut command, Duration::from_millis(20)).unwrap();

        assert!(matches!(outcome, TimedOutput::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
