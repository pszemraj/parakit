//! Bounded waits for short-lived desktop helper commands.

use std::io;
use std::process::{Child, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

/// Wait for a child until `timeout`, killing and reaping it on expiry.
///
/// # Arguments
///
/// * `child` - Spawned process to wait for.
/// * `timeout` - Maximum time to allow the child to run.
///
/// # Returns
///
/// The exit status when the child completes or `None` after a timeout.
///
/// # Errors
///
/// Returns an I/O error if querying or reaping the child fails.
pub(super) fn wait_with_timeout(
    child: &mut Child,
    timeout: Duration,
) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(Some(status)),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                child.wait()?;
                return Ok(None);
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn timeout_kills_and_reaps_child() {
        const CHILD: &str = "PARAKIT_SUBPROCESS_TIMEOUT_CHILD";
        if std::env::var_os(CHILD).is_some() {
            thread::sleep(Duration::from_millis(700));
            return;
        }

        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "daemon::subprocess::tests::timeout_kills_and_reaps_child",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .spawn()
            .unwrap();
        let started = Instant::now();

        assert!(wait_with_timeout(&mut child, Duration::from_millis(100))
            .unwrap()
            .is_none());
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(child.try_wait().unwrap().is_some());
    }
}
