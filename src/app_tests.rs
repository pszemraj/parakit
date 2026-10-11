//! Native log filtering and engine-config resolution regressions.

use super::*;

const GGML_LOG_LEVEL_DEBUG: i32 = 1;
const GGML_LOG_LEVEL_INFO: i32 = 2;
const GGML_LOG_LEVEL_ERROR: i32 = 4;

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
        || {
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
