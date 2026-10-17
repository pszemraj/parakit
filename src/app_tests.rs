//! Native log filtering and startup-configuration resolution regressions.

use super::*;

const GGML_LOG_LEVEL_DEBUG: i32 = 1;
const GGML_LOG_LEVEL_INFO: i32 = 2;
const GGML_LOG_LEVEL_ERROR: i32 = 4;

#[test]
fn source_rebuild_dispatch_preserves_legacy_and_selects_new_checkpoints() {
    fn source(args: &[&str]) -> Result<FetchSource> {
        let cli = Cli::parse_from(["parakit", "fetch"].into_iter().chain(args.iter().copied()));
        let Some(Commands::Fetch(fetch)) = cli.command else {
            panic!("expected fetch");
        };
        resolve_fetch_source(&fetch)
    }
    assert_eq!(
        source(&["--from-source", "--keep-nemo", "--keep-f16"]).unwrap(),
        FetchSource::OfficialNemo {
            keep_nemo: true,
            keep_f16: true,
        }
    );
    assert_eq!(
        source(&["--from-source", "--model-preset", "parakeet-v3"]).unwrap(),
        FetchSource::OfficialNemo {
            keep_nemo: false,
            keep_f16: false,
        }
    );
    assert_eq!(
        source(&[
            "--from-source",
            "--model-preset",
            "parakeet-redux",
            "--checkpoint-dir",
            "snapshot",
            "--keep-f16",
        ])
        .unwrap(),
        FetchSource::PresetSource {
            preset: ModelPreset::ParakeetRedux,
            checkpoint_dir: Some(PathBuf::from("snapshot")),
            keep_f16: true,
        }
    );
    assert!(source(&["--from-source", "--checkpoint-dir", "snapshot"]).is_err());
    assert!(source(&["--from-source", "--model-preset", "parakeet-v2"])
        .unwrap_err()
        .to_string()
        .contains("parakeet-v2 source rebuilding is not available"));
    assert!(source(&[
        "--from-source",
        "--model-preset",
        "parakeet-ultra",
        "--keep-nemo",
    ])
    .is_err());
}

#[test]
fn q6_fetch_selects_local_quantization_and_rejects_conflicting_inputs() {
    for preset in [
        ModelPreset::ParakeetV2,
        ModelPreset::ParakeetV3,
        ModelPreset::Qwen3Asr06b,
        ModelPreset::Qwen3Asr17b,
        ModelPreset::ParakeetUltra,
        ModelPreset::ParakeetRedux,
    ] {
        let cli = Cli::parse_from([
            "parakit",
            "fetch",
            "--model-preset",
            preset.id(),
            "--quantize",
            "q6_k",
        ]);
        let Some(Commands::Fetch(fetch)) = cli.command else {
            panic!("expected fetch")
        };
        assert_eq!(
            resolve_fetch_source(&fetch).unwrap(),
            FetchSource::PresetQ6(preset)
        );
    }
    for args in [
        vec!["parakit", "fetch", "--quantize", "q6_k"],
        vec![
            "parakit",
            "fetch",
            "--model-preset",
            "qwen3-asr-0.6b",
            "--quantize",
            "q4_k",
        ],
        vec![
            "parakit",
            "fetch",
            "--model-preset",
            "qwen3-asr-0.6b",
            "--quantize",
            "q6_k",
            "--from-source",
        ],
    ] {
        assert!(Cli::try_parse_from(args).is_err());
    }
}

#[test]
fn held_daemon_lock_requires_a_responsive_control_endpoint() {
    ensure_existing_daemon_responsive(true).unwrap();
    let error = ensure_existing_daemon_responsive(false).unwrap_err();
    assert!(error
        .to_string()
        .contains("control endpoint is unavailable"));
}

#[test]
fn file_size_format_scales_units() {
    assert_eq!(format_file_size(999_000), "999 KB");
    assert_eq!(format_file_size(999_000_000), "999 MB");
    assert_eq!(format_file_size(1_500_000_000), "1.50 GB");
}

/// One [`native_log_decision`] input/output pair.
struct NativeLogCase {
    label: &'static str,
    level: i32,
    min_level: i32,
    last_allowed: bool,
    expect: NativeLogDecision,
}

/// The 8 assertion points from the four tests this table replaces:
/// suppressing INFO/DEBUG without `--verbose`, keeping WARN/ERROR without
/// `--verbose`, passing everything with `--verbose`, and CONT following
/// whatever `last_allowed` was.
const NATIVE_LOG_CASES: &[NativeLogCase] = &[
    NativeLogCase {
        label: "debug suppressed without verbose",
        level: GGML_LOG_LEVEL_DEBUG,
        min_level: GGML_LOG_LEVEL_WARN,
        last_allowed: true,
        expect: NativeLogDecision {
            allowed: false,
            next_last_allowed: Some(false),
        },
    },
    NativeLogCase {
        label: "info suppressed without verbose",
        level: GGML_LOG_LEVEL_INFO,
        min_level: GGML_LOG_LEVEL_WARN,
        last_allowed: true,
        expect: NativeLogDecision {
            allowed: false,
            next_last_allowed: Some(false),
        },
    },
    NativeLogCase {
        label: "warn kept without verbose",
        level: GGML_LOG_LEVEL_WARN,
        min_level: GGML_LOG_LEVEL_WARN,
        last_allowed: false,
        expect: NativeLogDecision {
            allowed: true,
            next_last_allowed: Some(true),
        },
    },
    NativeLogCase {
        label: "error kept without verbose",
        level: GGML_LOG_LEVEL_ERROR,
        min_level: GGML_LOG_LEVEL_WARN,
        last_allowed: false,
        expect: NativeLogDecision {
            allowed: true,
            next_last_allowed: Some(true),
        },
    },
    NativeLogCase {
        label: "debug passes with verbose",
        level: GGML_LOG_LEVEL_DEBUG,
        min_level: GGML_LOG_LEVEL_NONE,
        last_allowed: false,
        expect: NativeLogDecision {
            allowed: true,
            next_last_allowed: Some(true),
        },
    },
    NativeLogCase {
        label: "info passes with verbose",
        level: GGML_LOG_LEVEL_INFO,
        min_level: GGML_LOG_LEVEL_NONE,
        last_allowed: false,
        expect: NativeLogDecision {
            allowed: true,
            next_last_allowed: Some(true),
        },
    },
    NativeLogCase {
        label: "continuation follows a previously disallowed record",
        level: GGML_LOG_LEVEL_CONT,
        min_level: GGML_LOG_LEVEL_WARN,
        last_allowed: false,
        expect: NativeLogDecision {
            allowed: false,
            next_last_allowed: None,
        },
    },
    NativeLogCase {
        label: "continuation follows a previously allowed record",
        level: GGML_LOG_LEVEL_CONT,
        min_level: GGML_LOG_LEVEL_WARN,
        last_allowed: true,
        expect: NativeLogDecision {
            allowed: true,
            next_last_allowed: None,
        },
    },
];

#[test]
fn native_log_decision_matrix() {
    for case in NATIVE_LOG_CASES {
        assert_eq!(
            native_log_decision(case.level, case.min_level, case.last_allowed),
            case.expect,
            "{}",
            case.label
        );
    }
}

#[test]
fn explicit_gpu_validation_runs_before_default_model_fetch() {
    let start = match Cli::parse_from(["parakit", "start", "--device", "gpu"]).command {
        Some(Commands::Start(start)) => start,
        other => panic!("expected Commands::Start, got {other:?}"),
    };
    let config = ConfigFile::default();
    let fetched_default = std::cell::Cell::new(false);

    let err = resolve_engine_config_with_validator(
        &start,
        &config,
        |_| {
            fetched_default.set(true);
            Ok(PathBuf::from("target/tmp/default-model.gguf"))
        },
        |device_mode| {
            assert_eq!(device_mode, DeviceMode::Gpu);
            anyhow::bail!("gpu unavailable")
        },
    )
    .unwrap_err();

    assert_eq!(err.to_string(), "gpu unavailable");
    assert!(!fetched_default.get());
}

#[test]
fn explicit_qwen_controls_reject_parakeet_before_download() {
    let start = match Cli::parse_from([
        "parakit",
        "start",
        "--model-preset",
        "parakeet-redux",
        "--qwen-language",
        "en",
    ])
    .command
    {
        Some(Commands::Start(start)) => start,
        _ => unreachable!(),
    };
    let error = resolve_engine_config_with_validator(
        &start,
        &ConfigFile::default(),
        |_| panic!("invalid controls must not download"),
        |_| Ok(()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("Qwen controls require"));
}

#[test]
fn worker_settings_preserve_defaults_config_and_cli_precedence() {
    use crate::daemon::inject::PasteMode;

    let defaults = ConfigFile::default();
    let mut configured = ConfigFile::default();
    configured.daemon.model_idle_minutes = Some(2);
    configured.daemon.paste_mode = Some(PasteMode::Direct);
    configured.daemon.keep_transcript_clipboard = Some(true);
    configured.logging.dir = Some(PathBuf::from("target/tmp/config-logs"));

    let cases = [
        (
            &defaults,
            &[][..],
            daemon::model_lifecycle::DEFAULT_MODEL_IDLE_MINUTES,
            crate::config::default_paste_mode(),
            false,
            None,
        ),
        (
            &configured,
            &[][..],
            2,
            PasteMode::Direct,
            true,
            Some("JSONL to target/tmp/config-logs"),
        ),
        (
            &configured,
            &[
                "--model-idle-minutes",
                "0",
                "--paste-mode",
                "standard",
                "--no-keep-transcript-clipboard",
                "--log-dir",
                "target/tmp/cli-logs",
            ][..],
            0,
            PasteMode::Standard,
            false,
            Some("JSONL to target/tmp/cli-logs"),
        ),
        (
            &defaults,
            &["--keep-transcript-clipboard"][..],
            daemon::model_lifecycle::DEFAULT_MODEL_IDLE_MINUTES,
            crate::config::default_paste_mode(),
            true,
            None,
        ),
    ];
    for (config, args, minutes, paste_mode, keep_clipboard, expected_log) in cases {
        let cli = Cli::parse_from(["parakit", "start"].into_iter().chain(args.iter().copied()));
        let Some(Commands::Start(start)) = cli.command else {
            panic!("expected start command");
        };
        let (settings, log_summary) = resolve_worker_settings(&start, config);
        assert_eq!(settings.model_idle_minutes, minutes, "{args:?}");
        assert_eq!(settings.paste_mode, paste_mode, "{args:?}");
        assert_eq!(
            settings.keep_transcript_clipboard, keep_clipboard,
            "{args:?}"
        );
        assert_eq!(
            settings.data_log.is_some(),
            expected_log.is_some(),
            "{args:?}"
        );
        assert_eq!(log_summary.as_deref(), expected_log, "{args:?}");
    }
}

#[test]
fn wire_history_limit_rejects_zero_but_passes_through_none_and_positive_values() {
    // `history --limit 0` must be rejected here, not forwarded to the
    // daemon: an empty result for `limit: Some(0)` would make
    // `print_history` claim history is empty even when transcripts are
    // remembered, the same lie `ensure_history_enabled` exists to avoid.
    assert_eq!(
        wire_history_limit(Some(0)).unwrap_err().to_string(),
        "history --limit must be at least 1"
    );
    assert_eq!(wire_history_limit(None).unwrap(), None);
    assert_eq!(wire_history_limit(Some(5)).unwrap(), Some(5));
}
