//! Desktop notifications for actionable daemon fallbacks.

#[cfg(target_os = "macos")]
use anyhow::Context;
use std::sync::{mpsc, Arc};
#[cfg(target_os = "macos")]
use std::time::Duration;

const NOTIFICATION_QUEUE_CAPACITY: usize = 16;
#[cfg(target_os = "macos")]
const NOTIFICATION_TIMEOUT: Duration = Duration::from_secs(3);

#[cfg(target_os = "macos")]
use super::subprocess::wait_with_timeout;
use super::{audio::MicInfo, logging::Logger};

/// Thin notification wrapper with stderr logging fallback.
#[derive(Clone)]
pub(crate) struct Notifier {
    log: Arc<Logger>,
    delivery: Option<mpsc::SyncSender<NotificationMessage>>,
}

struct NotificationMessage {
    summary: String,
    body: String,
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
        let delivery = start_desktop_delivery(&log);
        Self { log, delivery }
    }

    /// Build a notifier that deliberately emits no desktop messages.
    ///
    /// # Returns
    ///
    /// A notifier that ignores all messages.
    pub(crate) fn silent(log: Arc<Logger>) -> Self {
        Self {
            log,
            delivery: None,
        }
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

    /// Notify that a posted paste could not be confirmed.
    pub(crate) fn paste_unconfirmed(&self, reason: impl AsRef<str>) {
        self.show("Paste unconfirmed", reason.as_ref());
    }

    /// Notify that insertion aborted with an operational error.
    pub(crate) fn insertion_failed(&self, typed_chars: Option<usize>) {
        self.show("Insertion failed", insertion_failure_body(typed_chars));
    }

    /// Notify that an offloaded model could not be reopened at PTT start.
    ///
    /// # Arguments
    ///
    /// * `error` - Saved model reload error.
    pub(crate) fn model_unavailable(&self, error: impl AsRef<str>) {
        self.show("Model unavailable", model_unavailable_body(error.as_ref()));
    }

    /// Notify that both model reload attempts failed and the capture was discarded.
    ///
    /// # Arguments
    ///
    /// * `error` - Saved model reload error.
    pub(crate) fn dictation_discarded(&self, error: impl AsRef<str>) {
        self.show(
            "Dictation discarded",
            format!(
                "{}. The model could not be reloaded; the next push-to-talk will retry.",
                error.as_ref()
            ),
        );
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
        if let Some(delivery) = &self.delivery {
            let message = NotificationMessage {
                summary: summary.to_owned(),
                body: body.as_ref().to_owned(),
            };
            if let Err(error) = delivery.try_send(message) {
                self.log.verbose(format!(
                    "parakit: desktop notification queue unavailable: {error}"
                ));
            }
        }
    }
}

fn start_desktop_delivery(log: &Arc<Logger>) -> Option<mpsc::SyncSender<NotificationMessage>> {
    let (sender, receiver) = mpsc::sync_channel(NOTIFICATION_QUEUE_CAPACITY);
    let worker_log = Arc::clone(log);
    match std::thread::Builder::new()
        .name("parakit-notification".into())
        .spawn(move || deliver_notifications(receiver, &worker_log, show_notification))
    {
        Ok(_) => Some(sender),
        Err(error) => {
            log.verbose(format!(
                "parakit: could not start desktop notification worker: {error}"
            ));
            None
        }
    }
}

fn deliver_notifications(
    receiver: mpsc::Receiver<NotificationMessage>,
    log: &Logger,
    mut deliver: impl FnMut(&str, &str) -> anyhow::Result<()>,
) {
    while let Ok(message) = receiver.recv() {
        if let Err(error) = deliver(&message.summary, &message.body) {
            log.verbose(format!("parakit: desktop notification failed: {error:#}"));
        }
    }
}

fn insertion_failure_body(typed_chars: Option<usize>) -> String {
    match typed_chars {
        Some(chars) => format!(
            "Direct typing stopped after {chars} characters. Check the target before retrying."
        ),
        None => "Check the target before retrying.".to_string(),
    }
}

fn model_unavailable_body(error: &str) -> String {
    format!("{error}. Reload will retry before transcription.")
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
/// Delivery runs on the notifier thread, and the subprocess is bounded so one
/// unavailable helper cannot prevent later notifications indefinitely.
#[cfg(target_os = "macos")]
fn show_notification(summary: &str, body: &str) -> anyhow::Result<()> {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        applescript_quote(body),
        applescript_quote(summary)
    );
    let mut child = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .spawn()
        .context("could not spawn osascript for desktop notification")?;
    let status = wait_with_timeout(&mut child, NOTIFICATION_TIMEOUT)
        .context("could not query osascript notification status")?
        .context("osascript notification timed out")?;
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

#[cfg(test)]
mod message_tests {
    use super::*;
    use crate::daemon::logging::LogLevel;

    #[test]
    fn insertion_failure_reports_direct_typing_progress_without_history_advice() {
        assert_eq!(
            insertion_failure_body(Some(3)),
            "Direct typing stopped after 3 characters. Check the target before retrying."
        );
        assert_eq!(
            insertion_failure_body(None),
            "Check the target before retrying."
        );
    }

    #[test]
    fn model_reload_notice_uses_one_retry_message() {
        assert_eq!(
            model_unavailable_body("reload failed"),
            "reload failed. Reload will retry before transcription."
        );
    }

    #[test]
    fn queued_notifications_are_delivered_in_order() {
        let (sender, receiver) = mpsc::sync_channel(3);
        for summary in ["first", "second", "third"] {
            sender
                .send(NotificationMessage {
                    summary: summary.to_string(),
                    body: String::new(),
                })
                .unwrap();
        }
        drop(sender);

        let log = Logger::new(LogLevel::Quiet);
        let mut delivered = Vec::new();
        deliver_notifications(receiver, &log, |summary, _| {
            delivered.push(summary.to_string());
            Ok(())
        });

        assert_eq!(delivered, ["first", "second", "third"]);
    }
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
