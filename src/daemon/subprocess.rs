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
/// * `poll_interval` - Maximum delay between child-status checks.
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
    poll_interval: Duration,
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
            Ok(None) => thread::sleep(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(poll_interval),
            ),
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

    #[test]
    fn timeout_kills_and_reaps_child() {
        const CHILD: &str = "PARAKIT_SUBPROCESS_TIMEOUT_CHILD";
        if std::env::var_os(CHILD).is_some() {
            thread::sleep(Duration::from_millis(700));
            return;
        }

        for poll_interval in [Duration::from_millis(5), Duration::from_millis(10)] {
            let mut child = crate::test_support::exact_test_command(
                "daemon::subprocess::tests::timeout_kills_and_reaps_child",
            )
            .env(CHILD, "1")
            .spawn()
            .unwrap();
            let started = Instant::now();

            assert!(
                wait_with_timeout(&mut child, Duration::from_millis(100), poll_interval)
                    .unwrap()
                    .is_none()
            );
            assert!(started.elapsed() < Duration::from_millis(500));
            assert!(child.try_wait().unwrap().is_some());
        }
    }
}
