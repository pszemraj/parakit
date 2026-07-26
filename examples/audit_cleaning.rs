//! Replay the transcript cleaner over one JSONL file or a directory tree.
//!
//! This target is for regression analysis of real dictation logs. It does not
//! load a speech model or require desktop/audio features.

use anyhow::{bail, Context, Result};
use clap::Parser;
use parakit::rules::{build_cleaner, CleaningProfile, RuleHit, CLEANER_VERSION};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(about = "Replay parakit cleaning over historical JSONL dictation logs")]
struct Cli {
    /// JSONL file or directory containing JSONL files.
    #[arg(value_name = "PATH")]
    input: PathBuf,

    /// Cleanup behavior tier.
    #[arg(
        long,
        value_name = "PROFILE",
        default_value = "safe",
        value_parser = clap::value_parser!(CleaningProfile)
    )]
    profile: CleaningProfile,

    /// Keep terminal periods instead of applying the messaging-style default.
    #[arg(long)]
    keep_trailing_period: bool,

    /// Minimum isolated numeric value converted to digits. Omit to convert all.
    #[arg(long, value_name = "VALUE")]
    number_threshold: Option<f64>,

    /// Disable a pass by name. Repeatable.
    #[arg(long, value_name = "NAME")]
    disable_rule: Vec<String>,

    /// Maximum historical-difference examples retained in the report.
    #[arg(long, default_value_t = 40)]
    max_examples: usize,

    /// Write the JSON report here instead of stdout.
    #[arg(long, value_name = "PATH")]
    output: Option<PathBuf>,
}

#[derive(Default)]
struct MutableRuleStats {
    transcripts: usize,
    matches: usize,
}

#[derive(Debug, Serialize)]
struct RuleStats<'a> {
    name: &'a str,
    transcripts: usize,
    matches: usize,
}

#[derive(Debug, Serialize)]
struct DifferenceExample {
    source: String,
    line: usize,
    raw: String,
    historical_cleaned: String,
    replay_cleaned: String,
    rules_fired: Vec<RuleHit>,
}

#[derive(Debug, Serialize)]
struct AuditReport<'a> {
    report_schema: u32,
    cleaner_version: u32,
    profile: &'a str,
    drop_trailing_period: bool,
    number_threshold: Option<f64>,
    ruleset_id: &'a str,
    rules_active: usize,
    files: usize,
    records: usize,
    changed_from_raw: usize,
    unchanged_from_raw: usize,
    matches_historical_cleaned: usize,
    differs_from_historical_cleaned: usize,
    missing_historical_cleaned: usize,
    rules_fired: Vec<RuleStats<'a>>,
    examples: Vec<DifferenceExample>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("audit-cleaning: error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let cleaner = build_cleaner(
        false,
        cli.profile,
        !cli.keep_trailing_period,
        cli.number_threshold,
        &cli.disable_rule,
        &[],
    )?
    .expect("cleaning is explicitly enabled for the audit target");

    let mut files = Vec::new();
    collect_jsonl_files(&cli.input, &mut files)?;
    files.sort();
    if files.is_empty() {
        bail!("no .jsonl files found under {}", cli.input.display());
    }

    let mut records = 0;
    let mut changed_from_raw = 0;
    let mut matches_historical_cleaned = 0;
    let mut differs_from_historical_cleaned = 0;
    let mut missing_historical_cleaned = 0;
    let mut rule_stats: BTreeMap<String, MutableRuleStats> = BTreeMap::new();
    let mut examples = Vec::new();

    for path in &files {
        let file = File::open(path)
            .with_context(|| format!("failed to open JSONL file {}", path.display()))?;
        for (zero_based_line, line) in BufReader::new(file).lines().enumerate() {
            let line_number = zero_based_line + 1;
            let line = line
                .with_context(|| format!("failed to read {} line {line_number}", path.display()))?;
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(&line).with_context(|| {
                format!("invalid JSON at {} line {line_number}", path.display())
            })?;
            // A transcription log holds two independently parseable lines per
            // utterance. Only the first carries `raw`/`cleaned`; the second is
            // the correlated insertion outcome and has nothing to replay.
            if value.get("kind").and_then(Value::as_str) == Some("insertion") {
                continue;
            }
            let raw = value.get("raw").and_then(Value::as_str).with_context(|| {
                format!(
                    "missing string field 'raw' at {} line {line_number}",
                    path.display()
                )
            })?;
            let historical = value.get("cleaned").and_then(Value::as_str);
            let result = cleaner.try_clean(raw).with_context(|| {
                format!("cleaning failed at {} line {line_number}", path.display())
            })?;

            records += 1;
            if result.text != raw {
                changed_from_raw += 1;
            }
            for hit in &result.rules_fired {
                let stats = rule_stats.entry(hit.name.clone()).or_default();
                stats.transcripts += 1;
                stats.matches += hit.matches;
            }

            match historical {
                Some(historical) if historical == result.text => {
                    matches_historical_cleaned += 1;
                }
                Some(historical) => {
                    differs_from_historical_cleaned += 1;
                    if examples.len() < cli.max_examples {
                        examples.push(DifferenceExample {
                            source: path.display().to_string(),
                            line: line_number,
                            raw: raw.to_string(),
                            historical_cleaned: historical.to_string(),
                            replay_cleaned: result.text,
                            rules_fired: result.rules_fired,
                        });
                    }
                }
                None => {
                    missing_historical_cleaned += 1;
                }
            }
        }
    }

    let rules_fired = rule_stats
        .iter()
        .map(|(name, stats)| RuleStats {
            name: name.as_str(),
            transcripts: stats.transcripts,
            matches: stats.matches,
        })
        .collect();
    let report = AuditReport {
        report_schema: 2,
        cleaner_version: CLEANER_VERSION,
        profile: cleaner.profile().as_str(),
        drop_trailing_period: cleaner.drops_trailing_period(),
        number_threshold: cleaner.number_threshold(),
        ruleset_id: cleaner.ruleset_id(),
        rules_active: cleaner.active_rule_count(),
        files: files.len(),
        records,
        changed_from_raw,
        unchanged_from_raw: records - changed_from_raw,
        matches_historical_cleaned,
        differs_from_historical_cleaned,
        missing_historical_cleaned,
        rules_fired,
        examples,
    };

    match cli.output {
        Some(path) => {
            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            let mut file = File::create(&path)
                .with_context(|| format!("failed to create {}", path.display()))?;
            serde_json::to_writer_pretty(&mut file, &report)
                .context("failed to serialize audit report")?;
            writeln!(file).context("failed to terminate audit report with a newline")?;
        }
        None => {
            let stdout = std::io::stdout();
            let mut locked = stdout.lock();
            serde_json::to_writer_pretty(&mut locked, &report)
                .context("failed to serialize audit report")?;
            writeln!(locked).context("failed to terminate audit report with a newline")?;
        }
    }

    Ok(())
}

fn collect_jsonl_files(path: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    if path.is_file() {
        if path
            .extension()
            .is_some_and(|extension| extension == OsStr::new("jsonl"))
        {
            output.push(path.to_path_buf());
        }
        return Ok(());
    }
    if !path.is_dir() {
        bail!(
            "input path does not exist or is not readable: {}",
            path.display()
        );
    }

    for entry in std::fs::read_dir(path)
        .with_context(|| format!("failed to read directory {}", path.display()))?
    {
        let entry =
            entry.with_context(|| format!("failed to read entry under {}", path.display()))?;
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", entry.path().display()))?;
        if file_type.is_symlink() {
            continue;
        }
        collect_jsonl_files(&entry.path(), output)?;
    }
    Ok(())
}
