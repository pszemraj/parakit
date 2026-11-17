//! Configuration parsing, defaults, and load-time validation tests.

use super::*;

#[test]
fn model_alternatives_and_qwen_controls_are_validated_on_load() {
    let path = write_fixture("qwen", "[daemon]\nmodel_preset = 'qwen3-asr-0.6b'\n[qwen]\nlanguage = 'yue'\nmax_new_tokens = 4096\n[cleaning]\nenabled = false\n");
    let config = load_from_path(&path).unwrap();
    assert_eq!(config.daemon.model_preset, Some(ModelPreset::Qwen3Asr06b));
    assert_eq!(
        config.qwen.options().normalized_language().unwrap(),
        Some("Cantonese".to_owned())
    );
    assert!(!config.cleaning.enabled());
    for text in [
        "[daemon]\nmodel = 'a'\nmodel_preset = 'parakeet-v3'\n",
        "[daemon]\nmodel_preset = 'unknown'\n",
        "[daemon]\nmodel = 'a'\nquantize = 'q6_k'\n",
        "[daemon]\nquantize = 'q4_k'\n",
        "[qwen]\nlanguage = 'unsupported'\n",
        "[qwen]\nmax_new_tokens = 0\n",
        "[qwen]\nmax_new_tokens = 4097\n",
    ] {
        let path = write_fixture("bad-qwen", text);
        assert!(load_from_path(&path).is_err(), "{text}");
    }
}

fn write_fixture(name: &str, contents: &str) -> PathBuf {
    crate::test_support::write_config_fixture("parakit-config-test", name, contents)
}

#[test]
fn config_defaults_from_missing_empty_or_template_file() {
    // Missing, empty, and commented-template files select built-in defaults.
    let cases: [(&str, &str, Option<&str>); 3] = [
        ("missing file", "missing", None),
        ("empty file", "empty", Some("")),
        (
            "template (entirely commented out, must still parse as valid TOML)",
            "template",
            Some(TEMPLATE),
        ),
    ];

    for (label, slug, contents) in cases {
        let path = match contents {
            None => {
                let dir = crate::test_support::fixture_root("parakit-config-test", slug);
                dir.join("does-not-exist.toml")
            }
            Some(contents) => write_fixture(slug, contents),
        };
        let config = load_from_path(&path)
            .unwrap_or_else(|e| panic!("{label}: config file should parse: {e:#}"));

        assert!(config.daemon.model.is_none(), "{label}: daemon.model");
        assert!(config.daemon.device.is_none(), "{label}: daemon.device");
        assert!(config.rules.user.is_empty(), "{label}: rules.user");
        assert!(
            config.cleaning.disabled_rules.is_empty(),
            "{label}: cleaning.disabled_rules"
        );
        assert!(
            config.cleaning.enabled.is_none(),
            "{label}: cleaning.enabled"
        );
        assert!(
            config.cleaning.number_threshold.is_none(),
            "{label}: cleaning.number_threshold"
        );
        assert!(
            config.cleaning.repetition_exceptions.is_empty(),
            "{label}: cleaning.repetition_exceptions"
        );
    }
}

#[test]
fn fully_populated_config_parses_every_field() {
    let toml = r#"
[daemon]
model = "/models/custom.gguf"
device = "gpu"
threads = 4
paste_mode = "direct"
keep_transcript_clipboard = true
sounds = false
verbose = true
transcript_history = 25

[cleaning]
enabled = false
profile = "aggressive"
keep_trailing_period = true
number_threshold = 5
repetition_exceptions = ["paddle", "chinny chin chin"]
disabled_rules = ["fix-trailing-period", "filled-pauses"]

[logging]
dir = "/logs"

[[rules.user]]
name = "custom-hello"
description = "greet"
pattern = "(?i)hi"
replacement = "hello"
position = "first"
"#;
    let path = write_fixture("full", toml);
    let config = load_from_path(&path).expect("fully populated config should parse");

    assert_eq!(
        config.daemon.model,
        Some(PathBuf::from("/models/custom.gguf"))
    );
    assert_eq!(config.daemon.device, Some(DeviceMode::Gpu));
    assert_eq!(config.daemon.threads, NonZeroUsize::new(4));
    assert_eq!(config.daemon.paste_mode, Some(PasteMode::Direct));
    assert_eq!(config.daemon.keep_transcript_clipboard, Some(true));
    assert_eq!(config.daemon.sounds, Some(false));
    assert_eq!(config.daemon.verbose, Some(true));
    assert_eq!(config.daemon.transcript_history, Some(25));

    assert_eq!(config.cleaning.enabled, Some(false));
    assert_eq!(config.cleaning.profile, Some(CleaningProfile::Aggressive));
    assert_eq!(config.cleaning.keep_trailing_period, Some(true));
    assert_eq!(config.cleaning.number_threshold, Some(5.0));
    assert_eq!(
        config.cleaning.repetition_exceptions,
        ["paddle", "chinny chin chin"]
    );
    assert_eq!(
        config.cleaning.disabled_rules,
        vec![
            "fix-trailing-period".to_string(),
            "filled-pauses".to_string()
        ]
    );

    assert_eq!(config.logging.dir, Some(PathBuf::from("/logs")));

    assert_eq!(config.rules.user.len(), 1);
    assert_eq!(config.rules.user[0].name, "custom-hello");
    assert_eq!(config.rules.user[0].description.as_deref(), Some("greet"));
    assert_eq!(config.rules.user[0].pattern, "(?i)hi");
    assert_eq!(config.rules.user[0].replacement, "hello");
    assert_eq!(
        config.rules.user[0].position,
        parakit::rules::RulePosition::First
    );
}

#[test]
fn transcript_history_above_transport_safe_limit_is_rejected() {
    let toml = format!(
        "[daemon]\ntranscript_history = {}\n",
        crate::daemon::ipc::MAX_TRANSCRIPT_HISTORY + 1
    );
    let path = write_fixture("history-limit", &toml);
    let err = load_from_path(&path).expect_err("oversized transcript history must fail");
    let message = format!("{err:#}");

    assert!(
        message.contains(&format!(
            "daemon.transcript_history must be between 0 and {}",
            crate::daemon::ipc::MAX_TRANSCRIPT_HISTORY
        )),
        "{message}"
    );
    assert!(message.contains(&path.display().to_string()), "{message}");
}

#[test]
fn empty_model_and_log_dir_paths_are_rejected() {
    for (slug, toml, key) in [
        ("empty-model", "[daemon]\nmodel = \"\"\n", "daemon.model"),
        ("empty-log-dir", "[logging]\ndir = \"\"\n", "logging.dir"),
    ] {
        let path = write_fixture(slug, toml);
        let err = load_from_path(&path).expect_err("an empty path must fail config load");
        let message = format!("{err:#}");
        assert!(
            message.contains(&format!("{key} is set but empty")),
            "{message}"
        );
        assert!(message.contains(&path.display().to_string()), "{message}");
    }
}

#[test]
fn thread_count_above_the_c_int_range_is_rejected_at_load() {
    let max = write_fixture("threads-max", "[daemon]\nthreads = 2147483647\n");
    assert_eq!(
        load_from_path(&max).unwrap().daemon.threads,
        NonZeroUsize::new(2_147_483_647)
    );

    let path = write_fixture("threads-too-many", "[daemon]\nthreads = 2147483648\n");
    let message = format!("{:#}", load_from_path(&path).unwrap_err());
    assert!(
        message.contains("daemon.threads must be between 1 and 2147483647"),
        "{message}"
    );
}

#[test]
fn parse_error_includes_file_path() {
    let path = write_fixture("bad-toml", "not = [valid");
    let err = load_from_path(&path).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains(&path.display().to_string()), "message: {msg}");
}

#[test]
fn user_rule_validation_errors_include_failure_and_config_path() {
    for (name, toml, expected_error) in [
        (
            "misspelled-user-rule-position",
            r#"
[[rules.user]]
name = "custom-hello"
pattern = "(?i)hi"
replacement = "hello"
positon = "last"
"#,
            "unknown field `positon`",
        ),
        (
            "bad-regex",
            r#"
[[rules.user]]
name = "bad"
pattern = "(unclosed"
replacement = "x"
"#,
            "invalid regex",
        ),
        (
            "disabled-user-rule-bad-regex",
            r#"
[cleaning]
disabled_rules = ["broken"]

[[rules.user]]
name = "broken"
pattern = "(unclosed"
replacement = "x"
"#,
            "invalid regex",
        ),
    ] {
        let path = write_fixture(name, toml);
        let err = load_from_path(&path).expect_err(name);
        let message = format!("{err:#}");
        assert!(message.contains(expected_error), "{name}: {message}");
        assert!(
            message.contains(&path.display().to_string()),
            "{name}: config path missing from {message}"
        );
    }
}

#[test]
fn disabled_rules_naming_a_user_rule_is_accepted_at_load() {
    let toml = r#"
[cleaning]
disabled_rules = ["custom-hello"]

[[rules.user]]
name = "custom-hello"
pattern = "(?i)hi"
replacement = "hello"
"#;
    let path = write_fixture("disabled-rules-user-rule", toml);
    let config = load_from_path(&path).expect("disabling a user rule by name should validate");
    assert_eq!(
        config.cleaning.disabled_rules,
        vec!["custom-hello".to_string()]
    );
}

#[test]
fn unknown_config_keys_are_rejected() {
    for (name, toml, key) in [
        ("unknown-top-level", "typo = true", "typo"),
        ("unknown-section-key", "[daemon]\npositon = true", "positon"),
    ] {
        let path = write_fixture(name, toml);
        let err = load_from_path(&path).expect_err("unknown key must fail parsing");
        assert!(format!("{err:#}").contains(key));
    }
}

#[test]
fn config_path_env_override_wins() {
    let dir = crate::test_support::fixture_root("parakit-config-test", "env-override");
    let path = dir.join("custom-config.toml");
    let resolved = config_path_with_override(Some(path.clone().into_os_string()));
    assert_eq!(resolved.expect("env override should resolve"), path);
}

#[test]
fn missing_config_path_env_override_is_an_error() {
    let dir = crate::test_support::fixture_root("parakit-config-test", "env-override-missing");
    let missing = dir.join("typo-config.toml");
    let err = load_with_override(Some(missing.clone().into_os_string()))
        .expect_err("a missing override file must not silently give defaults");
    let message = format!("{err:#}");
    assert!(message.contains("PARAKIT_CONFIG_PATH names"), "{message}");
    assert!(
        message.contains(&missing.display().to_string()),
        "{message}"
    );

    let existing = write_fixture("env-override-existing", "[daemon]\nthreads = 3\n");
    let config = load_with_override(Some(existing.into_os_string()))
        .expect("an existing override file loads");
    assert_eq!(config.daemon.threads, NonZeroUsize::new(3));
}

#[test]
fn empty_config_path_env_override_is_rejected_without_global_env_mutation() {
    let err = config_path_with_override(Some(std::ffi::OsString::new())).unwrap_err();
    assert!(
        err.to_string().contains("is set but empty"),
        "message: {err:#}"
    );
}

/// Assert that every variant of a `clap::ValueEnum` serializes (via
/// serde) to exactly the string clap accepts on the command line. This
/// is the invariant `ConfigFile` parsing depends on: a value copied
/// verbatim from `--flag <value>` into `config.toml` must parse back to
/// the same enum variant.
fn assert_serde_matches_clap<T>()
where
    T: clap::ValueEnum + serde::Serialize + std::fmt::Debug,
{
    for variant in T::value_variants() {
        let clap_name = variant
            .to_possible_value()
            .expect("value_variants() entries must have a possible value")
            .get_name()
            .to_string();
        let json = serde_json::to_string(variant).expect("serde serialization must succeed");
        assert_eq!(
            json,
            format!("\"{clap_name}\""),
            "variant {variant:?}: serde form must match clap value string"
        );
    }
}

#[test]
fn enum_serde_matches_clap_value_strings() {
    assert_serde_matches_clap::<PasteMode>();
    assert_serde_matches_clap::<DeviceMode>();
    assert_serde_matches_clap::<CleaningProfile>();
    assert_serde_matches_clap::<parakit::rules::RulePosition>();
    #[cfg(target_os = "linux")]
    assert_serde_matches_clap::<HotkeyBackend>();
}

#[cfg(not(target_os = "linux"))]
#[test]
fn linux_hotkey_backend_is_accepted_and_ignored_on_other_platforms() {
    let path = write_fixture(
        "ignored-linux-hotkey-backend",
        "[hotkey]\nbackend = \"x11-listen\"\n",
    );
    let config = load_from_path(&path).expect("Linux hotkey config should remain portable");

    assert_eq!(config.hotkey._backend.as_deref(), Some("x11-listen"));
}
