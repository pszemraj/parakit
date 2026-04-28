//! Transcription logging for collecting raw/cleaned cleanup pairs.

use anyhow::{Context, Result};
use chrono::{Local, NaiveDate, SecondsFormat, Utc};
use parking_lot::Mutex;
use serde::Serialize;
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogFormat {
    Jsonl,
    Tsv,
}

impl std::str::FromStr for LogFormat {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "jsonl" | "json" => Ok(Self::Jsonl),
            "tsv" => Ok(Self::Tsv),
            other => Err(anyhow::anyhow!(
                "unknown log format '{other}'. Expected 'jsonl' or 'tsv'"
            )),
        }
    }
}

#[derive(Debug, Serialize)]
struct LogRecord<'a> {
    ts: String,
    audio_secs: f32,
    infer_ms: u128,
    raw: &'a str,
    cleaned: &'a str,
    rules_active: usize,
}

struct LogState {
    date: NaiveDate,
    file: BufWriter<File>,
}

pub struct DataLogger {
    dir: PathBuf,
    format: LogFormat,
    state: Mutex<Option<LogState>>,
}

impl DataLogger {
    pub fn new(dir: PathBuf, format: LogFormat) -> Self {
        Self {
            dir,
            format,
            state: Mutex::new(None),
        }
    }

    pub fn log(
        &self,
        audio_secs: f32,
        infer: Duration,
        raw: &str,
        cleaned: &str,
        rules_active: usize,
    ) {
        if let Err(e) = self.try_log(audio_secs, infer, raw, cleaned, rules_active) {
            eprintln!("parakit: transcription log write failed: {e:#}");
        }
    }

    fn try_log(
        &self,
        audio_secs: f32,
        infer: Duration,
        raw: &str,
        cleaned: &str,
        rules_active: usize,
    ) -> Result<()> {
        let local_date = Local::now().date_naive();
        let ts = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let mut state = self.state.lock();
        if state.as_ref().map(|s| s.date) != Some(local_date) {
            *state = Some(LogState {
                date: local_date,
                file: self.open_for_date(local_date)?,
            });
        }

        let record = LogRecord {
            ts,
            audio_secs,
            infer_ms: infer.as_millis(),
            raw,
            cleaned,
            rules_active,
        };
        let state = state
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("log file state was not initialized"))?;

        match self.format {
            LogFormat::Jsonl => {
                serde_json::to_writer(&mut state.file, &record)
                    .context("failed to serialize jsonl log record")?;
                writeln!(state.file).context("failed to write jsonl newline")?;
            }
            LogFormat::Tsv => {
                writeln!(
                    state.file,
                    "{}\t{:.3}\t{}\t{}\t{}\t{}",
                    record.ts,
                    record.audio_secs,
                    record.infer_ms,
                    sanitize_tsv(record.raw),
                    sanitize_tsv(record.cleaned),
                    record.rules_active
                )
                .context("failed to write tsv log record")?;
            }
        }
        state.file.flush().context("failed to flush log file")?;
        Ok(())
    }

    fn open_for_date(&self, date: NaiveDate) -> Result<BufWriter<File>> {
        create_dir_all(&self.dir)
            .with_context(|| format!("failed to create log dir {}", self.dir.display()))?;
        let path = self.dir.join(file_name(date, self.format));
        let file = append_file(&path)
            .with_context(|| format!("failed to open log file {}", path.display()))?;
        Ok(BufWriter::new(file))
    }
}

fn append_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(Into::into)
}

fn file_name(date: NaiveDate, format: LogFormat) -> String {
    let ext = match format {
        LogFormat::Jsonl => "jsonl",
        LogFormat::Tsv => "tsv",
    };
    format!("parakit-{}.{}", date.format("%Y-%m-%d"), ext)
}

fn sanitize_tsv(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\t' | '\r' | '\n' => ' ',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;

    #[test]
    fn concurrent_jsonl_logging_writes_all_lines() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/tmp/parakit-log-test");
        let _ = fs::remove_dir_all(&dir);
        let logger = Arc::new(DataLogger::new(dir.clone(), LogFormat::Jsonl));

        let mut threads = Vec::new();
        for thread_id in 0..10 {
            let logger = Arc::clone(&logger);
            threads.push(std::thread::spawn(move || {
                for i in 0..100 {
                    logger.log(
                        4.21,
                        Duration::from_millis(187),
                        &format!("raw {thread_id} {i}"),
                        &format!("cleaned {thread_id} {i}"),
                        72,
                    );
                }
            }));
        }

        for thread in threads {
            thread.join().expect("logging thread panicked");
        }

        let date = Local::now().date_naive();
        let path = dir.join(file_name(date, LogFormat::Jsonl));
        let contents = fs::read_to_string(&path).expect("read log file");
        assert_eq!(contents.lines().count(), 1000);
        for line in contents.lines() {
            let value: serde_json::Value = serde_json::from_str(line).expect("valid jsonl");
            assert_eq!(value["rules_active"], 72);
        }
    }

    #[test]
    fn tsv_sanitizes_tabs_and_newlines() {
        assert_eq!(sanitize_tsv("a\tb\nc\rd"), "a b c d");
    }
}
