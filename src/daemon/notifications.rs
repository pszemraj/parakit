//! Desktop notifications for actionable daemon fallbacks.

#[cfg(target_os = "macos")]
use anyhow::Context;
use std::sync::Arc;

use super::{audio::MicInfo, logging::Logger};

/// Thin notification wrapper with stderr logging fallback.
#[derive(Clone)]
pub(crate) struct Notifier {
    log: Arc<Logger>,
}

impl Notifier {
    /// Build a notifier.
    ///
    /// # Arguments
    ///
    /// * `log` - Logger used when the desktop notification backend fails.
    ///
    /// # Returns
    ///
    /// A notifier that falls back to verbose logging when notifications fail.
    pub(crate) fn new(log: Arc<Logger>) -> Self {
        Self { log }
    }

    /// Notify that a transcript was copied without sending a paste chord.
    ///
    /// # Arguments
    ///
    /// * `reason` - Short user-facing reason for leaving transcript text on the clipboard.
    pub(crate) fn transcript_copied(&self, reason: impl AsRef<str>) {
        self.show("Transcript copied", reason.as_ref());
    }

    /// Notify that a transcript was blocked before clipboard staging.
    ///
    /// # Arguments
    ///
    /// * `reason` - Short reason for the block.
    pub(crate) fn paste_blocked(&self, reason: impl AsRef<str>) {
        self.show("Paste blocked", reason.as_ref());
    }

    /// Notify that microphone capture failed and the daemon is trying to reopen it.
    ///
    /// # Arguments
    ///
    /// * `error` - Stream or device error summary.
    pub(crate) fn microphone_unavailable(&self, error: impl AsRef<str>) {
        self.show(
            "Microphone unavailable",
            format!("Audio capture failed: {}. Retrying.", error.as_ref()),
        );
    }

    /// Notify that microphone capture recovered after a failure.
    ///
    /// # Arguments
    ///
    /// * `mic` - Reopened microphone summary.
    pub(crate) fn microphone_recovered(&self, mic: &MicInfo) {
        self.show("Microphone recovered", mic.summary());
    }

    fn show(&self, summary: &str, body: impl AsRef<str>) {
        if let Err(err) = show_notification(summary, body.as_ref()) {
            self.log
                .verbose(format!("parakit: desktop notification failed: {err:#}"));
        }
    }
}

#[cfg(target_os = "linux")]
fn show_notification(summary: &str, body: &str) -> anyhow::Result<()> {
    notify_rust::Notification::new()
        .appname("parakit")
        .summary(summary)
        .body(body)
        .show()?;
    Ok(())
}

/// Show a macOS Notification Center banner through `osascript`.
///
/// This runs `osascript` synchronously. `display notification` returns
/// quickly (it does not wait for user interaction), and this call already
/// happens on the worker thread after insertion has resolved, so blocking
/// briefly here does not add to dictation latency; a synchronous call also
/// keeps failures visible to the caller for the existing verbose-log
/// fallback instead of silently dropping them in a detached thread.
#[cfg(target_os = "macos")]
fn show_notification(summary: &str, body: &str) -> anyhow::Result<()> {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        applescript_quote(body),
        applescript_quote(summary)
    );
    let status = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .status()
        .context("could not spawn osascript for desktop notification")?;
    anyhow::ensure!(status.success(), "osascript exited with {status}");
    Ok(())
}

#[cfg(target_os = "windows")]
fn show_notification(_summary: &str, _body: &str) -> anyhow::Result<()> {
    Ok(())
}

/// Escape a string for embedding in a double-quoted AppleScript string
/// literal.
///
/// Backslash, double-quote, line feed, and carriage return are escaped so
/// untrusted notification text cannot terminate the source line or string
/// literal. Every other character, including non-ASCII text, passes through
/// unchanged.
///
/// # Arguments
///
/// * `s` - Raw text to embed inside a `"..."` AppleScript string literal.
///
/// # Returns
///
/// `s` with AppleScript source-significant characters escaped. The caller
/// is responsible for wrapping the result in the surrounding double quotes.
#[cfg(target_os = "macos")]
fn applescript_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn applescript_quote_matrix() {
        // (name, input, expected escaped form)
        let cases = [
            ("escapes double quotes", r#"say "hi""#, r#"say \"hi\""#),
            ("escapes backslashes", r"C:\Users\me", r"C:\\Users\\me"),
            (
                "escapes mixed quotes and backslashes",
                r#"mixed \ and " chars"#,
                r#"mixed \\ and \" chars"#,
            ),
            (
                "passes through unicode",
                "héllo wörld 你好",
                "héllo wörld 你好",
            ),
            (
                "escapes line endings",
                "first\nsecond\rthird\r\nfourth",
                r"first\nsecond\rthird\r\nfourth",
            ),
            (
                "leaves plain text untouched",
                "Transcript copied",
                "Transcript copied",
            ),
        ];

        let failures: Vec<String> = cases
            .iter()
            .filter_map(|&(name, input, expect)| {
                let actual = applescript_quote(input);
                (actual != expect).then(|| format!("{name}: expected {expect:?}, got {actual:?}"))
            })
            .collect();
        assert!(
            failures.is_empty(),
            "{} case(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
