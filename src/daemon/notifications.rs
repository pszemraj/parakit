//! Desktop notifications for actionable daemon fallbacks.

#[cfg(target_os = "macos")]
use anyhow::Context;
#[cfg(test)]
use std::sync::Mutex;
use std::sync::{mpsc, Arc};
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

const NOTIFICATION_QUEUE_CAPACITY: usize = 16;
#[cfg(target_os = "macos")]
const NOTIFICATION_TIMEOUT: Duration = Duration::from_secs(3);

#[cfg(test)]
type RecordedNotifications = Arc<Mutex<Vec<(String, String)>>>;

use super::{audio::MicInfo, logging::Logger};

/// Thin notification wrapper with stderr logging fallback.
#[derive(Clone)]
pub(crate) struct Notifier {
    log: Arc<Logger>,
    delivery: NotificationDelivery,
}

#[derive(Clone)]
enum NotificationDelivery {
    Desktop(Arc<DesktopDelivery>),
    Silent,
    #[cfg(test)]
    Recording(RecordedNotifications),
}

struct DesktopDelivery {
    sender: mpsc::SyncSender<NotificationMessage>,
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
        let delivery = start_desktop_delivery(&log)
            .map_or(NotificationDelivery::Silent, |sender| {
                NotificationDelivery::Desktop(Arc::new(DesktopDelivery { sender }))
            });
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
            delivery: NotificationDelivery::Silent,
        }
    }

    #[cfg(test)]
    /// Build a notifier that records messages for assertions without desktop I/O.
    ///
    /// # Returns
    ///
    /// The notifier and its shared recorded-message buffer.
    pub(crate) fn recording(log: Arc<Logger>) -> (Self, RecordedNotifications) {
        let messages = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                log,
                delivery: NotificationDelivery::Recording(Arc::clone(&messages)),
            },
            messages,
        )
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
    pub(crate) fn insertion_failed(&self) {
        self.show(
            "Insertion failed",
            "Check the target before retrying. If transcript history is enabled, recover the dictation with parakit copy-last.",
        );
    }

    /// Notify that an offloaded model could not be reopened at PTT start.
    ///
    /// # Arguments
    ///
    /// * `error` - Saved model reload error.
    pub(crate) fn model_unavailable(&self, error: impl AsRef<str>, recording_active: bool) {
        let recovery = if recording_active {
            "Recording continues; reload will retry when push-to-talk is released."
        } else {
            "Push-to-talk is already released; reload will retry before transcription."
        };
        self.show(
            "Model unavailable",
            format!("{}. {recovery}", error.as_ref()),
        );
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
        match &self.delivery {
            NotificationDelivery::Silent => {}
            #[cfg(test)]
            NotificationDelivery::Recording(messages) => {
                messages
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push((summary.to_owned(), body.as_ref().to_owned()));
            }
            NotificationDelivery::Desktop(delivery) => {
                let message = NotificationMessage {
                    summary: summary.to_owned(),
                    body: body.as_ref().to_owned(),
                };
                if let Err(error) = delivery.sender.try_send(message) {
                    self.log.verbose(format!(
                        "parakit: desktop notification queue unavailable: {error}"
                    ));
                }
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
/// Notification delivery runs on the notifier's detached delivery thread, so
/// a slow or unavailable notification server cannot delay worker cues,
/// transcript retention, or insertion completion.
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
    let deadline = Instant::now() + NOTIFICATION_TIMEOUT;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .context("could not query osascript notification status")?
        {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("osascript notification timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
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
mod recording_tests {
    use super::*;
    use crate::daemon::logging::{LogLevel, Logger};

    #[test]
    fn unconfirmed_paste_has_a_truthful_distinct_title() {
        let (notifier, messages) = Notifier::recording(Arc::new(Logger::new(LogLevel::Quiet)));

        notifier.paste_unconfirmed("Inspect the target before retrying.");

        assert_eq!(
            messages.lock().unwrap().as_slice(),
            &[(
                "Paste unconfirmed".to_string(),
                "Inspect the target before retrying.".to_string()
            )]
        );
    }

    #[test]
    fn insertion_failure_points_to_conditional_history_recovery() {
        let (notifier, messages) = Notifier::recording(Arc::new(Logger::new(LogLevel::Quiet)));

        notifier.insertion_failed();

        let messages = messages.lock().unwrap();
        assert_eq!(messages.len(), 1);
        let message = &messages[0];
        assert_eq!(message.0, "Insertion failed");
        assert!(message.1.contains("If transcript history is enabled"));
        assert!(message.1.contains("parakit copy-last"));
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
