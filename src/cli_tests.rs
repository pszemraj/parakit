//! Parser, effective-option precedence, and flag-conflict regressions for the CLI.

use super::*;

fn start_from(args: &[&str]) -> StartCli {
    let mut full = vec!["parakit", "start"];
    full.extend_from_slice(args);
    match Cli::parse_from(full).command {
        Some(Commands::Start(start)) => start,
        other => panic!("expected Commands::Start, got {other:?}"),
    }
}

fn cli_from(args: &[&str]) -> Cli {
    let mut full = vec!["parakit"];
    full.extend_from_slice(args);
    Cli::parse_from(full)
}

#[test]
fn bare_parakit_parses_with_no_command() {
    let cli = Cli::parse_from(["parakit"]);
    assert!(cli.command.is_none());
}

#[test]
fn bare_start_parses_to_default_start_cli() {
    match Cli::parse_from(["parakit", "start"]).command {
        Some(Commands::Start(start)) => {
            assert_eq!(start.model, None);
            assert!(!start.no_cleaning);
            assert!(start.disable_rule.is_empty());
        }
        other => panic!("expected Commands::Start, got {other:?}"),
    }
}

#[test]
fn status_accepts_global_quiet_flag() {
    let cli = Cli::parse_from(["parakit", "status", "-q"]);
    assert!(cli.quiet);
    assert!(matches!(cli.command, Some(Commands::Status)));
}

#[test]
fn doctor_accepts_global_verbose_flag() {
    let cli = Cli::parse_from(["parakit", "doctor", "--verbose"]);
    assert!(cli.effective_verbose(&ConfigFile::default()));
    assert!(matches!(cli.command, Some(Commands::Doctor(_))));
}

#[test]
fn global_quiet_and_verbose_still_conflict() {
    let error = Cli::try_parse_from(["parakit", "-q", "-v"])
        .expect_err("global --quiet and --verbose must still conflict");
    assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
}

#[test]
fn rules_defaults_to_list() {
    match Cli::parse_from(["parakit", "rules"]).command {
        Some(Commands::Rules(rules)) => assert!(rules.command.is_none()),
        other => panic!("expected Commands::Rules, got {other:?}"),
    }
}

#[test]
fn rules_test_parses_input_and_profile() {
    match Cli::parse_from(["parakit", "rules", "test", "x", "--profile", "aggressive"]).command {
        Some(Commands::Rules(RulesCli {
            command: Some(RulesCommand::Test { input, args }),
        })) => {
            assert_eq!(input, "x");
            assert_eq!(args.profile, Some(CleaningProfile::Aggressive));
        }
        other => panic!("expected Commands::Rules(Test), got {other:?}"),
    }
}

#[test]
fn rules_trailing_period_pair_conflicts() {
    let error = Cli::try_parse_from([
        "parakit",
        "rules",
        "list",
        "--keep-trailing-period",
        "--no-keep-trailing-period",
    ])
    .expect_err("--keep-trailing-period and --no-keep-trailing-period must conflict");
    assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
}

#[test]
fn effective_device_prefers_cli_then_config_then_default() {
    let mut config = ConfigFile::default();
    assert_eq!(
        start_from(&[]).effective_device(&config),
        DeviceMode::default()
    );

    config.daemon.device = Some(DeviceMode::Gpu);
    assert_eq!(start_from(&[]).effective_device(&config), DeviceMode::Gpu);

    assert_eq!(
        start_from(&["--device", "cpu"]).effective_device(&config),
        DeviceMode::Cpu
    );
}

#[test]
fn effective_paste_mode_prefers_cli_then_config_then_platform_default() {
    let mut config = ConfigFile::default();
    assert_eq!(
        start_from(&[]).effective_paste_mode(&config),
        default_paste_mode()
    );

    config.daemon.paste_mode = Some(PasteMode::Direct);
    assert_eq!(
        start_from(&[]).effective_paste_mode(&config),
        PasteMode::Direct
    );

    assert_eq!(
        start_from(&["--paste-mode", "standard"]).effective_paste_mode(&config),
        PasteMode::Standard
    );
}

#[cfg(target_os = "linux")]
#[test]
fn effective_hotkey_backend_prefers_cli_then_config_then_auto() {
    let mut config = ConfigFile::default();
    assert_eq!(
        start_from(&[]).effective_hotkey_backend(&config),
        HotkeyBackend::Auto
    );

    config.hotkey.backend = Some(HotkeyBackend::X11Listen);
    assert_eq!(
        start_from(&[]).effective_hotkey_backend(&config),
        HotkeyBackend::X11Listen
    );
    assert_eq!(
        start_from(&["--hotkey-backend", "desktop"]).effective_hotkey_backend(&config),
        HotkeyBackend::Desktop
    );

    let doctor = |args: &[&str]| {
        let mut full = vec!["parakit", "doctor"];
        full.extend_from_slice(args);
        match Cli::parse_from(full).command {
            Some(Commands::Doctor(doctor)) => doctor,
            other => panic!("expected Commands::Doctor, got {other:?}"),
        }
    };
    assert_eq!(
        doctor(&[]).effective_hotkey_backend(&config),
        HotkeyBackend::X11Listen
    );
    assert_eq!(
        doctor(&["--hotkey-backend", "desktop"]).effective_hotkey_backend(&config),
        HotkeyBackend::Desktop
    );
}

#[test]
fn doctor_effective_paste_mode_prefers_cli_then_config_then_platform_default() {
    let mut config = ConfigFile::default();
    let doctor_cli = |args: &[&str]| -> DoctorCli {
        let mut full = vec!["parakit", "doctor"];
        full.extend_from_slice(args);
        match Cli::parse_from(full).command {
            Some(Commands::Doctor(doctor)) => doctor,
            other => panic!("expected Commands::Doctor, got {other:?}"),
        }
    };

    assert_eq!(
        doctor_cli(&[]).effective_paste_mode(&config),
        default_paste_mode()
    );

    config.daemon.paste_mode = Some(PasteMode::Direct);
    assert_eq!(
        doctor_cli(&[]).effective_paste_mode(&config),
        PasteMode::Direct,
        "config value must win when no CLI flag is given"
    );
    assert_eq!(
        doctor_cli(&["--paste-mode", "standard"]).effective_paste_mode(&config),
        PasteMode::Standard
    );
}

#[test]
fn effective_model_and_threads_and_log_dir_prefer_cli_then_config() {
    let mut config = ConfigFile::default();

    let start = start_from(&[]);
    assert_eq!(start.effective_model(&config), None);
    assert_eq!(start.effective_threads(&config), None);
    assert_eq!(start.effective_log_dir(&config), None);

    config.daemon.model = Some(PathBuf::from("/config/model.gguf"));
    config.daemon.threads = NonZeroUsize::new(2);
    config.logging.dir = Some(PathBuf::from("/config/logs"));

    let start = start_from(&[]);
    assert_eq!(
        start.effective_model(&config),
        Some(PathBuf::from("/config/model.gguf"))
    );
    assert_eq!(start.effective_threads(&config), NonZeroUsize::new(2));
    assert_eq!(
        start.effective_log_dir(&config),
        Some(PathBuf::from("/config/logs"))
    );

    let start = start_from(&[
        "-m",
        "/cli/model.gguf",
        "--threads",
        "8",
        "--log-dir",
        "/cli/logs",
    ]);
    assert_eq!(
        start.effective_model(&config),
        Some(PathBuf::from("/cli/model.gguf"))
    );
    assert_eq!(start.effective_threads(&config), NonZeroUsize::new(8));
    assert_eq!(
        start.effective_log_dir(&config),
        Some(PathBuf::from("/cli/logs"))
    );
}

/// One positive/negative CLI flag pair that overrides a config value,
/// plus everything [`bool_flag_pairs_override_config_in_both_directions`]
/// needs to drive the shared (a)-(d) template against it.
struct BoolFlagPairCase {
    /// Included in every assertion message so a failure names the pair.
    label: &'static str,
    positive_flag: &'static str,
    negative_flag: &'static str,
    /// Effective value with no flags and no config set.
    default_fallback: bool,
    /// What the positive flag forces the effective value to (the
    /// negative flag forces the opposite). Not always `true`:
    /// `--keep-trailing-period` forces `effective_drops_trailing_period`
    /// to `false`.
    positive_forces: bool,
    /// Write `value` into the config field this pair reads, choosing the
    /// underlying config polarity so that `effective(&config)` (with no
    /// CLI flags) would return `value`.
    set_config_for_effective: fn(&mut ConfigFile, bool),
    effective: fn(&StartCli, &ConfigFile) -> bool,
}

/// Table-driven (a)-(d) coverage for every mutually exclusive
/// positive/negative `start` flag pair that overrides a config value:
///
/// - (a) the positive flag overrides a config value of the opposite polarity
/// - (b) the negative flag overrides a config value of the opposite polarity
/// - (c) with neither flag, the config value applies, and a missing config
///   value falls back to the built-in default
/// - (d) the two flags conflict with each other
#[test]
fn bool_flag_pairs_override_config_in_both_directions() {
    let cases = [
        BoolFlagPairCase {
            label: "sounds",
            positive_flag: "--sounds",
            negative_flag: "--no-sounds",
            default_fallback: true,
            positive_forces: true,
            set_config_for_effective: |config, value| config.daemon.sounds = Some(value),
            effective: StartCli::effective_sounds_enabled,
        },
        BoolFlagPairCase {
            label: "cleaning",
            positive_flag: "--cleaning",
            negative_flag: "--no-cleaning",
            default_fallback: true,
            positive_forces: true,
            set_config_for_effective: |config, value| config.cleaning.enabled = Some(value),
            effective: StartCli::effective_cleaning_enabled,
        },
        BoolFlagPairCase {
            label: "keep_trailing_period",
            positive_flag: "--keep-trailing-period",
            negative_flag: "--no-keep-trailing-period",
            // Dropped by default: messaging-style dictation drops the
            // trailing period unless the user positively asks to keep it.
            default_fallback: true,
            // `--keep-trailing-period` means "do not drop it".
            positive_forces: false,
            set_config_for_effective: |config, value| {
                config.cleaning.keep_trailing_period = Some(!value)
            },
            effective: StartCli::effective_drops_trailing_period,
        },
        BoolFlagPairCase {
            label: "keep_transcript_clipboard",
            positive_flag: "--keep-transcript-clipboard",
            negative_flag: "--no-keep-transcript-clipboard",
            default_fallback: false,
            positive_forces: true,
            set_config_for_effective: |config, value| {
                config.daemon.keep_transcript_clipboard = Some(value)
            },
            effective: StartCli::effective_keep_transcript_clipboard,
        },
    ];

    for case in cases {
        let mut config = ConfigFile::default();

        // (c) No flags: falls through to config, then the built-in default.
        assert_eq!(
            (case.effective)(&start_from(&[]), &config),
            case.default_fallback,
            "{}: built-in default should apply with no flags and no config",
            case.label
        );
        (case.set_config_for_effective)(&mut config, !case.default_fallback);
        assert_eq!(
            (case.effective)(&start_from(&[]), &config),
            !case.default_fallback,
            "{}: config should override the built-in default with no flags",
            case.label
        );

        // (a) Positive flag overrides a config value of the opposite polarity.
        (case.set_config_for_effective)(&mut config, !case.positive_forces);
        assert_eq!(
            (case.effective)(&start_from(&[case.positive_flag]), &config),
            case.positive_forces,
            "{}: {} should force the effective value regardless of config",
            case.label,
            case.positive_flag
        );

        // (b) Negative flag overrides a config value of the opposite polarity.
        (case.set_config_for_effective)(&mut config, case.positive_forces);
        assert_eq!(
            (case.effective)(&start_from(&[case.negative_flag]), &config),
            !case.positive_forces,
            "{}: {} should force the effective value regardless of config",
            case.label,
            case.negative_flag
        );

        // (d) The pair conflicts.
        let error =
            Cli::try_parse_from(["parakit", "start", case.positive_flag, case.negative_flag])
                .expect_err("the flag pair must conflict");
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::ArgumentConflict,
            "{}: {} and {} must conflict",
            case.label,
            case.positive_flag,
            case.negative_flag
        );
    }
}

#[test]
fn effective_cleaning_profile_prefers_cli_then_config_then_safe() {
    let mut config = ConfigFile::default();

    // Safe is the shipped default: broad semantic editing, in particular
    // deleting `like`/`you know`/`I mean`, must stay opt-in.
    assert_eq!(
        start_from(&[]).effective_cleaning_profile(&config),
        CleaningProfile::Safe
    );

    config.cleaning.profile = Some(CleaningProfile::Aggressive);
    assert_eq!(
        start_from(&[]).effective_cleaning_profile(&config),
        CleaningProfile::Aggressive
    );

    assert_eq!(
        start_from(&["--cleaning-profile", "safe"]).effective_cleaning_profile(&config),
        CleaningProfile::Safe
    );
}

#[test]
fn effective_verbose_uses_or_semantics() {
    let mut config = ConfigFile::default();
    assert!(!cli_from(&[]).effective_verbose(&config));

    config.daemon.verbose = Some(true);
    assert!(cli_from(&[]).effective_verbose(&config));

    config.daemon.verbose = Some(false);
    assert!(cli_from(&["--verbose"]).effective_verbose(&config));
}

#[test]
fn effective_verbose_config_true_does_not_override_cli_quiet() {
    // `--quiet` and `--verbose` conflict at the CLI level, but a config
    // `verbose = true` cannot participate in that check. Effective
    // verbosity must still keep every logging layer quiet.
    let mut config = ConfigFile::default();
    config.daemon.verbose = Some(true);
    let cli = cli_from(&["--quiet"]);
    assert!(cli.quiet);
    assert!(!cli.effective_verbose(&config));
}

#[test]
fn effective_disabled_rules_merges_cli_and_config_without_duplicates() {
    let mut config = ConfigFile::default();
    config.cleaning.disabled_rules = vec![
        "fix-trailing-period".to_string(),
        "filled-pauses".to_string(),
    ];

    let start = start_from(&[
        "--disable-rule",
        "filled-pauses",
        "--disable-rule",
        "lead-discourse-comma",
    ]);
    let mut merged = start.effective_disabled_rules(&config);
    merged.sort();
    let mut expected = vec![
        "filled-pauses".to_string(),
        "fix-trailing-period".to_string(),
        "lead-discourse-comma".to_string(),
    ];
    expected.sort();
    assert_eq!(merged, expected);
}

#[test]
fn effective_transcript_history_prefers_config_then_default() {
    let mut config = ConfigFile::default();
    assert_eq!(
        start_from(&[]).effective_transcript_history(&config),
        DEFAULT_TRANSCRIPT_HISTORY
    );

    config.daemon.transcript_history = Some(0);
    assert_eq!(start_from(&[]).effective_transcript_history(&config), 0);

    config.daemon.transcript_history = Some(25);
    assert_eq!(start_from(&[]).effective_transcript_history(&config), 25);
}

/// Build a `Vec<String>` from string literals; shared by the `fetch`
/// coverage tables below, whose rows sometimes need to append an owned
/// value (a generated SHA256 digest) that cannot live in a `'static`
/// slice.
fn strs(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

fn fetch_from(args: &[String]) -> FetchCli {
    let mut full: Vec<String> = vec!["parakit".to_string(), "fetch".to_string()];
    full.extend_from_slice(args);
    match Cli::parse_from(full).command {
        Some(Commands::Fetch(fetch)) => fetch,
        other => panic!("expected Commands::Fetch, got {other:?}"),
    }
}

#[test]
fn bare_fetch_parses_with_no_source() {
    let fetch = fetch_from(&[]);
    assert_eq!(fetch.source, None);
    assert_eq!(fetch.file, None);
    assert_eq!(fetch.sha256, None);
    assert!(!fetch.force);
    assert!(!fetch.from_source);
}

#[test]
fn fetch_parses_a_positional_repo_source() {
    let fetch = fetch_from(&strs(&["cstr/parakeet-tdt-0.6b-v3-GGUF"]));
    assert_eq!(
        fetch.source.as_deref(),
        Some("cstr/parakeet-tdt-0.6b-v3-GGUF")
    );
    assert_eq!(fetch.file, None);
    assert_eq!(fetch.sha256, None);
    assert!(!fetch.force && !fetch.from_source);
}

#[test]
fn fetch_parses_source_with_file_and_sha256() {
    let sha = "a".repeat(64);
    let mut args = strs(&[
        "cstr/parakeet-tdt-0.6b-v3-GGUF",
        "--file",
        "parakeet-tdt-0.6b-v3-q4_k.gguf",
        "--sha256",
    ]);
    args.push(sha.clone());
    let fetch = fetch_from(&args);
    assert_eq!(
        fetch.source.as_deref(),
        Some("cstr/parakeet-tdt-0.6b-v3-GGUF")
    );
    assert_eq!(
        fetch.file.as_deref(),
        Some("parakeet-tdt-0.6b-v3-q4_k.gguf")
    );
    assert_eq!(fetch.sha256, Some(sha));
    assert!(!fetch.force && !fetch.from_source);
}

#[test]
fn fetch_sha256_is_normalized_to_lowercase() {
    let mut args = strs(&["owner/repo", "--sha256"]);
    args.push("A".repeat(64));
    let fetch = fetch_from(&args);
    assert_eq!(fetch.source.as_deref(), Some("owner/repo"));
    assert_eq!(fetch.sha256, Some("a".repeat(64)));
}

struct FetchRejectionCase {
    label: &'static str,
    args: Vec<String>,
    expect_kind: clap::error::ErrorKind,
}

#[test]
fn fetch_rejects_invalid_and_conflicting_argument_combinations() {
    let sha = "a".repeat(64);

    let cases = vec![
        FetchRejectionCase {
            label: "sha256 rejects a non-64-hex value",
            args: strs(&["parakit", "fetch", "owner/repo", "--sha256", "not-hex"]),
            expect_kind: clap::error::ErrorKind::ValueValidation,
        },
        FetchRejectionCase {
            label: "--file requires a source",
            args: strs(&["parakit", "fetch", "--file", "model.gguf"]),
            expect_kind: clap::error::ErrorKind::MissingRequiredArgument,
        },
        FetchRejectionCase {
            label: "--sha256 requires a source",
            args: {
                let mut args = strs(&["parakit", "fetch", "--sha256"]);
                args.push(sha.clone());
                args
            },
            expect_kind: clap::error::ErrorKind::MissingRequiredArgument,
        },
        FetchRejectionCase {
            label: "a positional source conflicts with --from-source",
            args: strs(&["parakit", "fetch", "owner/repo", "--from-source"]),
            expect_kind: clap::error::ErrorKind::ArgumentConflict,
        },
        FetchRejectionCase {
            label: "--file conflicts with --from-source",
            args: strs(&[
                "parakit",
                "fetch",
                "owner/repo",
                "--file",
                "model.gguf",
                "--from-source",
            ]),
            expect_kind: clap::error::ErrorKind::ArgumentConflict,
        },
        FetchRejectionCase {
            label: "--keep-nemo still requires --from-source",
            args: strs(&["parakit", "fetch", "--keep-nemo"]),
            expect_kind: clap::error::ErrorKind::MissingRequiredArgument,
        },
    ];

    for case in cases {
        let error = Cli::try_parse_from(case.args.iter().cloned())
            .expect_err(&format!("{}: expected a parse error", case.label));
        assert_eq!(error.kind(), case.expect_kind, "{}", case.label);
    }
}
#[test]
fn model_idle_minutes_precedence_and_validation() {
    let mut config = ConfigFile::default();
    assert_eq!(start_from(&[]).effective_model_idle_minutes(&config), 10);
    config.daemon.model_idle_minutes = Some(2);
    assert_eq!(start_from(&[]).effective_model_idle_minutes(&config), 2);
    assert_eq!(
        start_from(&["--model-idle-minutes", "0"]).effective_model_idle_minutes(&config),
        0
    );
    assert_eq!(
        start_from(&["--model-idle-minutes", "18446744073709551615"])
            .effective_model_idle_minutes(&config),
        u64::MAX
    );
    for raw in ["-1", "0.5", "18446744073709551616", "forever"] {
        assert!(
            Cli::try_parse_from(["parakit", "start", "--model-idle-minutes", raw]).is_err(),
            "{raw}"
        );
    }
    assert!(Cli::try_parse_from(["parakit", "start", "--simulate-ptt-repeat", "2"]).is_err());
}
