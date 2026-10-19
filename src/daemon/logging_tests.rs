//! The normal/quiet/verbose boundary for everyday dictation output.

use super::*;

#[test]
fn everyday_output_shows_comparison_but_reserves_timings_for_verbose() {
    const LEVEL: &str = "PARAKIT_LOGGING_OUTPUT_TEST_LEVEL";
    if let Ok(level) = std::env::var(LEVEL) {
        let level = match level.as_str() {
            "quiet" => LogLevel::Quiet,
            "verbose" => LogLevel::Verbose,
            _ => LogLevel::Normal,
        };
        let log = Logger::new(level);
        log.ready();
        log.transcribing(1.0, 2.0);
        log.transcript("um hello", "Hello", Duration::from_millis(1250));
        log.transcript("unchanged", "unchanged", Duration::from_millis(750));
        return;
    }
    let test = "daemon::logging::feedback_tests::everyday_output_shows_comparison_but_reserves_timings_for_verbose";
    for level in ["quiet", "normal", "verbose"] {
        let output = crate::test_support::exact_test_command(test)
            .env(LEVEL, level)
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success() && output.stderr.is_empty(),
            "{output:?}"
        );
        let text = String::from_utf8(output.stdout).unwrap();
        let visible = usize::from(level != "quiet");
        assert_eq!(
            text.matches("Clean:").count(),
            2 * visible,
            "{level}: {text}"
        );
        assert_eq!(text.matches("Raw:").count(), visible, "{level}: {text}");
        assert_eq!(
            text.contains("Raw:    um hello"),
            level != "quiet",
            "{level}: {text}"
        );
        assert_eq!(
            text.contains("Clean:  Hello"),
            level != "quiet",
            "{level}: {text}"
        );
        assert_eq!(
            text.contains("Clean: unchanged"),
            level != "quiet",
            "{level}: {text}"
        );
        assert_eq!(
            text.contains("1250ms"),
            level == "verbose",
            "{level}: {text}"
        );
        assert_eq!(
            text.contains("750ms"),
            level == "verbose",
            "{level}: {text}"
        );
        assert_eq!(
            text.contains("wall)"),
            level == "verbose",
            "{level}: {text}"
        );
        assert_eq!(
            text.contains("Ctrl+C to stop"),
            level != "quiet",
            "{level}: {text}"
        );
    }
}
