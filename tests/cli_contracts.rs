//! Process-level CLI contract tests that cannot be covered through clap's
//! pure parser alone.

use std::path::Path;
use std::process::Command;

#[allow(dead_code)]
mod common;

fn parakit() -> Command {
    Command::new(env!("CARGO_BIN_EXE_parakit"))
}

fn isolated_parakit(root: &Path) -> Command {
    let mut command = parakit();
    command
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("XDG_CACHE_HOME", root.join("cache"));
    command
}

#[test]
fn broken_config_does_not_block_documented_control_and_repair_commands() {
    let root = common::fixture_root("cli-contracts", "broken-config-control");
    std::fs::create_dir_all(&root).expect("fixture root should be created");
    let config = root.join("broken.toml");
    std::fs::write(&config, "not = [valid").expect("broken config fixture should be written");

    let local_commands: &[&[&str]] = &[
        &["fetch", "not-a-repo-or-url"],
        &["cache", "dir"],
        &["cache", "list"],
        &["config", "path"],
        &["config", "init"],
        &["config", "edit"],
    ];
    for args in local_commands {
        let mut process = isolated_parakit(&root);
        process
            .args(*args)
            .env("PARAKIT_CONFIG_PATH", &config)
            .env_remove("VISUAL")
            .env_remove("EDITOR");
        let output = process.output().expect("parakit should run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("failed to parse config") && !stderr.contains("failed to read config"),
            "{args:?} unexpectedly loaded the broken config: {stderr}"
        );
    }

    // Unix daemon endpoints are isolated under XDG_RUNTIME_DIR. The Windows
    // named pipe is keyed directly by the user's SID and has no test-path
    // override, so these commands must not contact it from the test suite.
    #[cfg(unix)]
    for args in [
        &["status"][..],
        &["stop"][..],
        &["copy-last"][..],
        &["history"][..],
        &["test-paste", "test"][..],
    ] {
        let output = isolated_parakit(&root)
            .args(args)
            .env("PARAKIT_CONFIG_PATH", &config)
            .output()
            .expect("parakit should run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("failed to parse config") && !stderr.contains("failed to read config"),
            "{args:?} unexpectedly loaded the broken config: {stderr}"
        );
    }
}

#[test]
fn quiet_path_commands_still_report_resolution_and_parse_errors() {
    let root = common::fixture_root("cli-contracts", "quiet-path-errors");
    std::fs::create_dir_all(&root).expect("fixture root should be created");
    let broken_config = root.join("broken.toml");
    std::fs::write(&broken_config, "not = [valid").expect("write broken config fixture");

    let cases: &[(&[&str], &str, Option<&Path>)] = &[
        (
            &["--quiet", "config", "path"],
            "PARAKIT_CONFIG_PATH is set but empty",
            None,
        ),
        (
            &["--quiet", "cache", "dir"],
            "PARAKIT_MODELS_DIR is set but empty",
            None,
        ),
        (
            &["--quiet", "cache", "list"],
            "PARAKIT_MODELS_DIR is set but empty",
            None,
        ),
        (
            &["--quiet", "config", "show"],
            "failed to parse config file",
            Some(&broken_config),
        ),
        (
            &["--quiet", "config", "show"],
            "PARAKIT_CONFIG_PATH is set but empty",
            None,
        ),
    ];

    for (args, expected_error, config_path) in cases {
        let mut process = isolated_parakit(&root);
        process.args(*args);
        if args[1] == "config" {
            match config_path {
                Some(path) => {
                    process.env("PARAKIT_CONFIG_PATH", path);
                }
                None => {
                    process.env("PARAKIT_CONFIG_PATH", "");
                }
            }
        } else {
            process.env("PARAKIT_MODELS_DIR", "");
        }

        let output = process.output().expect("parakit should run");
        assert!(!output.status.success(), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected_error), "{args:?}: {stderr}");
    }
}

#[test]
fn quiet_rules_list_still_validates_disabled_rule_names() {
    let root = common::fixture_root("cli-contracts", "quiet-rules-validation");
    std::fs::create_dir_all(&root).expect("fixture root should be created");
    let config = root.join("config.toml");
    std::fs::write(&config, "").expect("empty config fixture should be written");

    let output = isolated_parakit(&root)
        .args(["--quiet", "rules", "list", "--disable-rule", "typo"])
        .env("PARAKIT_CONFIG_PATH", &config)
        .output()
        .expect("parakit should run");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no rule named 'typo'"));
}

#[test]
fn cache_list_includes_uppercase_gguf_extensions() {
    let root = common::fixture_root("cli-contracts", "uppercase-gguf-cache");
    let models = root.join("models");
    std::fs::create_dir_all(&models).expect("models directory should be created");
    std::fs::write(models.join("MODEL.GGUF"), b"").expect("model fixture should be written");

    let output = isolated_parakit(&root)
        .args(["cache", "list"])
        .env("PARAKIT_MODELS_DIR", &models)
        .output()
        .expect("parakit should run");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("MODEL.GGUF"), "{stdout}");
    assert!(!stdout.contains("models: none"), "{stdout}");
}

#[cfg(unix)]
#[test]
fn daemon_only_commands_report_an_absent_daemon_cleanly() {
    let root = common::fixture_root("cli-contracts", "absent-daemon");

    let cases: &[(&[&str], i32, &str, &str)] = &[
        (&["status"], 0, "parakit: not running\n", ""),
        (&["stop"], 0, "parakit: not running; nothing to stop\n", ""),
        (
            &["history"],
            1,
            "",
            "parakit: error: daemon is not running; start it with `parakit start` first\n",
        ),
        (
            &["copy-last"],
            1,
            "",
            "parakit: error: daemon is not running; start it with `parakit start` first\n",
        ),
        (
            &["test-paste", "test"],
            1,
            "",
            "parakit: error: daemon is not running; start it with `parakit start` first\n",
        ),
    ];

    for (args, expected_code, expected_stdout, expected_stderr) in cases {
        let output = isolated_parakit(&root)
            .args(*args)
            .output()
            .expect("parakit should run");

        assert_eq!(output.status.code(), Some(*expected_code), "{args:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), *expected_stdout);
        assert_eq!(String::from_utf8_lossy(&output.stderr), *expected_stderr);
    }

    for command in ["status", "stop"] {
        let output = isolated_parakit(&root)
            .args(["--quiet", command])
            .output()
            .expect("parakit should run");

        assert!(output.status.success(), "{command}");
        assert!(output.stdout.is_empty(), "{command}");
        assert!(output.stderr.is_empty(), "{command}");
    }
    assert!(!root.join("runtime").exists());
    assert!(!root.join("cache").exists());
}

#[cfg(unix)]
#[test]
fn stop_waits_for_shutdown_after_the_control_endpoint_disappears() {
    let root = common::fixture_root("cli-contracts", "stopping-daemon");
    #[cfg(target_os = "linux")]
    let runtime = root.join("runtime").join("parakit");
    #[cfg(not(target_os = "linux"))]
    let runtime = root.join("cache").join("parakit").join("run");
    std::fs::create_dir_all(&runtime).unwrap();
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(runtime.join("parakit.lock"))
        .unwrap();
    fs2::FileExt::lock_exclusive(&lock).unwrap();
    let mut stop = isolated_parakit(&root)
        .arg("stop")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(
        stop.try_wait().unwrap().is_none(),
        "stop must wait for the held lock"
    );
    drop(lock);
    let output = stop.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"stopped\n");
    assert!(output.stderr.is_empty());
}

#[cfg(unix)]
#[test]
fn stop_reaches_a_daemon_that_exposes_control_after_the_request_starts() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::time::{Duration, Instant};

    let root = common::fixture_root("cli-contracts", "stop-during-startup");
    #[cfg(target_os = "linux")]
    let runtime = root.join("runtime").join("parakit");
    #[cfg(not(target_os = "linux"))]
    let runtime = root.join("cache").join("parakit").join("run");
    std::fs::create_dir_all(&runtime).unwrap();
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(runtime.join("parakit.lock"))
        .unwrap();
    fs2::FileExt::lock_exclusive(&lock).unwrap();
    let mut stop = isolated_parakit(&root)
        .arg("stop")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_secs(1));
    assert!(stop.try_wait().unwrap().is_none());

    let listener = UnixListener::bind(runtime.join("control.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut delivered = false;
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut request = String::new();
                BufReader::new(&stream).read_line(&mut request).unwrap();
                assert_eq!(request, "\"stop\"\n");
                stream
                    .write_all(b"{\"ok\":{\"message\":\"stopping\"}}\n")
                    .unwrap();
                delivered = true;
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("test control endpoint failed: {error}"),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if delivered {
        // Acknowledgement must end transport retries, not the shutdown wait.
        std::thread::sleep(Duration::from_millis(5050));
        assert!(stop.try_wait().unwrap().is_none());
        assert!(matches!(
            listener.accept(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
        ));
    }
    drop(lock);
    let output = stop.wait_with_output().unwrap();
    assert!(
        delivered,
        "stop never reached the starting daemon: {output:?}"
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"stopped\n");
    assert!(output.stderr.is_empty());
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires an X11 desktop and microphone; opens a passive test daemon without recording"]
fn daemon_control_remains_available_during_model_download() {
    use anyhow::{bail, ensure, Result};
    use std::time::{Duration, Instant};

    let root = common::fixture_root("cli-contracts", "startup-control");
    std::fs::create_dir_all(&root).unwrap();
    let config = root.join("config.toml");
    std::fs::write(&config, "").unwrap();
    let stderr_path = root.join("startup.err");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut daemon = isolated_parakit(&root)
        .args([
            "--verbose",
            "start",
            "--hotkey-backend",
            "x11-listen",
            "--no-sounds",
            "--no-cleaning",
            "--device",
            "cpu",
        ])
        .env("PARAKIT_CONFIG_PATH", &config)
        .env("PARAKIT_MODELS_DIR", root.join("models"))
        .env(
            "HF_ENDPOINT",
            format!("http://{}", listener.local_addr().unwrap()),
        )
        .env_remove("HF_TOKEN")
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(&stderr_path).unwrap())
        .spawn()
        .unwrap();

    let result = (|| -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(15);
        let _download = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
            if let Some(status) = daemon.try_wait()? {
                bail!(
                    "startup exited {status}: {}",
                    std::fs::read_to_string(&stderr_path)?
                );
            }
            ensure!(
                Instant::now() < deadline,
                "startup never requested the model"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        // The local server deliberately withholds its response, exercising the
        // real startup path without downloading a model or entering the hotkey loop.
        for (args, expected) in [
            (&["status"][..], "parakit: starting"),
            (&["start"][..], "parakit: already running"),
            (&["stop"][..], "stopped"),
        ] {
            let output = isolated_parakit(&root).args(args).output()?;
            ensure!(output.status.success(), "{args:?}: {output:?}");
            ensure!(
                String::from_utf8_lossy(&output.stdout).contains(expected),
                "{args:?}: {output:?}"
            );
            ensure!(output.stderr.is_empty(), "{args:?}: {output:?}");
        }
        ensure!(daemon.wait()?.success(), "test daemon failed during stop");
        Ok(())
    })();
    // Clean up only the test-owned process if any assertion or startup check failed.
    let _ = daemon.kill();
    let _ = daemon.wait();
    result.unwrap();
}

#[test]
fn config_show_reports_resolved_defaults() {
    let root = common::fixture_root("cli-contracts", "config-show-defaults");
    std::fs::create_dir_all(&root).expect("fixture root should be created");
    let config = root.join("config.toml");
    std::fs::write(&config, "").expect("empty config fixture should be written");

    let output = isolated_parakit(&root)
        .args(["config", "show"])
        .env("PARAKIT_CONFIG_PATH", &config)
        .output()
        .expect("parakit should run");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for expected in [
        "    model: (default: hosted Q8_0)",
        "    device: (default: auto)",
        "    threads: (default: auto-detected)",
        "    paste_mode: (default: platform)",
        "    keep_transcript_clipboard: false",
        "    sounds: true",
        "    transcript_history: (default: 10)",
        "    enabled: true",
        "    profile: safe",
        "    keep_trailing_period: false",
        "    number_threshold: (default: 4)",
    ] {
        assert!(
            stdout.contains(expected),
            "missing {expected:?} in:\n{stdout}"
        );
    }
}
