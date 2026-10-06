//! Regression coverage for the standalone memory profiler example.

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

    let peak = host_interval_peak(&host, sampled, true);

    assert_eq!(peak["rss_bytes"], 120);
    assert_eq!(peak["source"], "linux_VmHWM");
    assert_eq!(peak["samples"], 4);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_interval_peak_falls_back_when_kernel_reset_is_unavailable() {
    let host = json!({"status_bytes": {"VmRSS": 40, "VmHWM": 120}});
    let sampled = json!({"rss_bytes": 80, "samples": 4});

    let peak = host_interval_peak(&host, sampled, false);

    assert_eq!(peak["rss_bytes"], 80);
    assert_eq!(peak["source"], "sampled");
}

#[test]
fn command_timeout_kills_a_slow_child() {
    const CHILD: &str = "PARAKIT_PROFILE_TIMEOUT_TEST_CHILD";
    const MARKER: &str = "PARAKIT_PROFILE_TIMEOUT_TEST_MARKER";
    const PID_MARKER: &str = "PARAKIT_PROFILE_TIMEOUT_TEST_PID";
    if std::env::var_os(CHILD).is_some() {
        fs::write(
            std::env::var_os(PID_MARKER).unwrap(),
            std::process::id().to_string(),
        )
        .unwrap();
        thread::sleep(Duration::from_millis(500));
        fs::write(std::env::var_os(MARKER).unwrap(), b"completed").unwrap();
        return;
    }

    let capture = command_test_dir("timeout");
    let marker = capture.join("completed");
    let pid_marker = capture.join("pid");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "tests::command_timeout_kills_a_slow_child",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env(MARKER, &marker)
        .env(PID_MARKER, &pid_marker);
    let started = Instant::now();
    let outcome =
        command_output_with_timeout(&mut command, Duration::from_millis(100), &capture).unwrap();

    assert!(matches!(outcome, TimedOutput::TimedOut));
    assert!(started.elapsed() < Duration::from_secs(1));
    thread::sleep(Duration::from_millis(300));
    assert!(
        !marker.exists(),
        "timed-out child survived long enough to finish"
    );
    #[cfg(target_os = "linux")]
    {
        let pid = fs::read_to_string(&pid_marker).expect("child should record its pid");
        assert!(
            !Path::new("/proc").join(pid.trim()).exists(),
            "timed-out child was killed but not reaped"
        );
    }
    assert!(
        fs::read_dir(&capture).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")),
        "temporary command captures were not removed"
    );
    fs::remove_dir_all(capture).unwrap();
}

#[test]
fn command_capture_does_not_deadlock_on_large_output() {
    const CHILD: &str = "PARAKIT_PROFILE_LARGE_OUTPUT_CHILD";
    if std::env::var_os(CHILD).is_some() {
        io::stdout().write_all(&vec![b'x'; 300_000]).unwrap();
        return;
    }

    let capture = command_test_dir("large-output");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "tests::command_capture_does_not_deadlock_on_large_output",
            "--nocapture",
        ])
        .env(CHILD, "1");

    let outcome =
        command_output_with_timeout(&mut command, Duration::from_secs(1), &capture).unwrap();
    let TimedOutput::Completed(output) = outcome else {
        panic!("large completed output was misclassified as a timeout");
    };
    assert!(output.status.success());
    assert!(output.stdout.len() >= 300_000);
    fs::remove_dir_all(capture).unwrap();
}

#[test]
#[allow(clippy::zombie_processes)] // The descendant must outlive its parent to exercise inherited output handles.
fn descendant_holding_output_does_not_extend_parent_timeout() {
    const ROLE: &str = "PARAKIT_PROFILE_DESCENDANT_ROLE";
    match std::env::var(ROLE).as_deref() {
        Ok("grandchild") => {
            thread::sleep(Duration::from_millis(500));
            return;
        }
        Ok("parent") => {
            let _child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::descendant_holding_output_does_not_extend_parent_timeout",
                    "--nocapture",
                ])
                .env(ROLE, "grandchild")
                .spawn()
                .unwrap();
            return;
        }
        _ => {}
    }

    let capture = command_test_dir("descendant");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "tests::descendant_holding_output_does_not_extend_parent_timeout",
            "--nocapture",
        ])
        .env(ROLE, "parent");
    let started = Instant::now();
    let outcome =
        command_output_with_timeout(&mut command, Duration::from_secs(1), &capture).unwrap();

    assert!(matches!(outcome, TimedOutput::Completed(_)));
    assert!(started.elapsed() < Duration::from_millis(400));
    fs::remove_dir_all(capture).unwrap();
}

fn command_test_dir(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/tmp/profile-command-tests")
        .join(format!("{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}
