//! Process-level CLI contract tests that cannot be covered through clap's
//! pure parser alone.

use std::process::Command;

fn parakit() -> Command {
    Command::new(env!("CARGO_BIN_EXE_parakit"))
}

#[test]
fn migration_hint_uses_clap_exit_code_two() {
    let output = parakit()
        .arg("paste-last")
        .output()
        .expect("parakit should run");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("paste-last' was removed"), "{stderr}");
    assert!(stderr.contains("try: parakit copy-last"), "{stderr}");
}

#[test]
fn broken_config_does_not_block_control_commands() {
    let root = std::path::Path::new("target")
        .join("tmp")
        .join("cli-contracts")
        .join(format!("{}-broken-config-control", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("fixture root should be created");
    let config = root.join("broken.toml");
    std::fs::write(&config, "not = [valid").expect("broken config fixture should be written");

    // Status is read-only and safe on a developer machine. On Unix, isolate
    // the daemon endpoint under this fixture as well, which makes Stop safe to
    // cover without contacting a live daemon. The Windows named pipe is keyed
    // directly by the user's SID and has no test-path override, so never send
    // Stop there.
    let commands: &[&str] = if cfg!(unix) {
        &["status", "stop"]
    } else {
        &["status"]
    };

    for command in commands {
        let mut process = parakit();
        process
            .arg(command)
            .env("PARAKIT_CONFIG_PATH", &config)
            .env("XDG_RUNTIME_DIR", root.join("runtime"))
            .env("XDG_CACHE_HOME", root.join("cache"));
        let output = process.output().expect("parakit should run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("failed to parse config")
                && !stderr.contains("failed to read config")
                && !stderr.contains(&config.display().to_string()),
            "{command} unexpectedly loaded the broken config: {stderr}"
        );
    }
}

#[cfg(unix)]
#[test]
fn daemon_only_commands_report_an_absent_daemon_cleanly() {
    let root = std::path::Path::new("target")
        .join("tmp")
        .join("cli-contracts")
        .join(format!("{}-absent-daemon", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);

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
        let output = parakit()
            .args(*args)
            .env("XDG_RUNTIME_DIR", root.join("runtime"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .output()
            .expect("parakit should run");

        assert_eq!(output.status.code(), Some(*expected_code), "{args:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), *expected_stdout);
        assert_eq!(String::from_utf8_lossy(&output.stderr), *expected_stderr);
    }

    for command in ["status", "stop"] {
        let output = parakit()
            .args(["--quiet", command])
            .env("XDG_RUNTIME_DIR", root.join("runtime"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .output()
            .expect("parakit should run");

        assert!(output.status.success(), "{command}");
        assert!(output.stdout.is_empty(), "{command}");
        assert!(output.stderr.is_empty(), "{command}");
    }
}
