//! Inference padding, context fallback, and runtime default regressions.

use super::*;
use std::cell::RefCell;
use std::collections::VecDeque;

#[test]
fn short_and_empty_pcm_are_padded_with_silence() {
    let padded = pad_short_pcm(&[0.25, -0.25]);
    assert_eq!(padded.len(), MIN_INFERENCE_SAMPLES);
    assert_eq!(&padded[..2], &[0.25, -0.25]);
    assert!(padded[2..].iter().all(|sample| *sample == 0.0));

    let empty = pad_short_pcm(&[]);
    assert_eq!(empty.len(), MIN_INFERENCE_SAMPLES);
    assert!(empty.iter().all(|sample| *sample == 0.0));
}

#[test]
fn long_pcm_is_borrowed_without_copying() {
    let pcm = vec![0.0; MIN_INFERENCE_SAMPLES];
    assert!(matches!(pad_short_pcm(&pcm), Cow::Borrowed(_)));
}

#[test]
fn copied_context_matches_normalized_runs_without_rejecting_new_wording() {
    for (output, context, copied) in [
        ("", "Earlier text", false),
        ("...", "Earlier text", false),
        ("I'm.", "Earlier, I'm ordering groceries.", true),
        (
            "Refill vitamin D and fish oil.",
            "Remember: refill vitamin D and fish oil.",
            true,
        ),
        (
            "New speech. Will book the ski trip with Lily for March.",
            "will book the ski trip with Lily for March",
            true,
        ),
        (
            "CAFÉ, naïve: 雪 and Émilie!",
            "Earlier: café naïve 雪 and Émilie.",
            true,
        ),
        ("one two three four", "one two three four five", true),
        ("one two three four new", "one two three four five", false),
        (
            "new one two three four five ending",
            "one two three four five",
            true,
        ),
        (
            "the thermostat",
            "another theory about thermodynamics",
            false,
        ),
        (
            "NeoBERT corrected the domain term",
            "We discussed NeoBERT and Common Pile.",
            false,
        ),
    ] {
        assert_eq!(copies_context(output, context), copied, "{output:?}");
    }
}

/// Scripted native session that records every call in order.
#[derive(Default)]
struct FakeSession {
    log: RefCell<Vec<String>>,
    outputs: RefCell<VecDeque<Result<&'static str, &'static str>>>,
    reject_context: bool,
    reject_clear: bool,
}

impl FakeSession {
    fn with_outputs(outputs: &[Result<&'static str, &'static str>]) -> Self {
        Self {
            outputs: RefCell::new(outputs.iter().copied().collect()),
            ..Self::default()
        }
    }

    fn log(&self) -> Vec<String> {
        self.log.borrow().clone()
    }

    fn inferences(&self) -> usize {
        self.log
            .borrow()
            .iter()
            .filter(|call| *call == "infer")
            .count()
    }
}

impl ContextSession for FakeSession {
    fn set_context(&self, text: &str) -> Result<(), String> {
        self.log.borrow_mut().push(format!("set:{text}"));
        let reject = if text.is_empty() {
            self.reject_clear
        } else {
            self.reject_context
        };
        if reject {
            Err("setter failed".into())
        } else {
            Ok(())
        }
    }

    fn infer(&self, _pcm: &[f32]) -> Result<String> {
        self.log.borrow_mut().push("infer".into());
        let output = self
            .outputs
            .borrow_mut()
            .pop_front()
            .expect("unexpected extra inference");
        output.map(str::to_string).map_err(|e| anyhow::anyhow!(e))
    }
}

const PRIOR: &str = "one two three four five six";
const SET_PRIOR: &str = "set:one two three four five six";
const PRIOR_CHARS: usize = 27;

fn scoped(session: &FakeSession) -> Result<(String, ContextUsage)> {
    let pcm = vec![0.0; MIN_INFERENCE_SAMPLES];
    transcribe_scoped(session, true, true, &pcm, Some(PRIOR))
}

fn used(chars: usize, retried: bool) -> ContextUsage {
    ContextUsage { chars, retried }
}

#[test]
fn filtered_context_runs_one_ordinary_inference_without_retry() {
    let long = vec![0.0; MIN_INFERENCE_SAMPLES];
    let short = vec![0.0; MIN_INFERENCE_SAMPLES - 1];
    for (qwen, raw, pcm, context) in [
        (true, true, &long, None),
        (true, true, &long, Some("")),
        (false, true, &long, Some(PRIOR)),
        (true, false, &long, Some(PRIOR)),
        (true, true, &short, Some(PRIOR)),
    ] {
        let case = format!("{qwen} {raw} {} {context:?}", pcm.len());
        let empty = FakeSession::with_outputs(&[Ok("  ")]);
        let result = transcribe_scoped(&empty, qwen, raw, pcm, context).unwrap();
        assert_eq!(result, ("  ".to_string(), used(0, false)), "{case}");
        assert_eq!(empty.inferences(), 1, "{case}");
        let failed = FakeSession::with_outputs(&[Err("native failure")]);
        let error = transcribe_scoped(&failed, qwen, raw, pcm, context).unwrap_err();
        assert_eq!(error.to_string(), "native failure", "{case}");
        let expected: &[&str] = if qwen {
            &["set:", "infer", "set:"]
        } else {
            &["infer"]
        };
        assert_eq!(failed.log(), expected, "{case}");
    }
}

#[test]
fn rejected_context_setter_clears_and_runs_ordinary_inference() {
    let session = FakeSession {
        reject_context: true,
        ..FakeSession::with_outputs(&[Ok("heard")])
    };
    assert_eq!(
        scoped(&session).unwrap(),
        ("heard".to_string(), used(0, false))
    );
    assert_eq!(session.log(), [SET_PRIOR, "set:", "infer", "set:"]);

    let session = FakeSession {
        reject_context: true,
        ..FakeSession::with_outputs(&[Err("native failure")])
    };
    scoped(&session).unwrap_err();
    assert_eq!(session.inferences(), 1);
}

#[test]
fn usable_contextual_output_is_returned_without_retry() {
    let session = FakeSession::with_outputs(&[Ok("new words entirely")]);
    let result = scoped(&session).unwrap();
    assert_eq!(
        result,
        ("new words entirely".to_string(), used(PRIOR_CHARS, false))
    );
    assert_eq!(session.log(), [SET_PRIOR, "infer", "set:"]);
}

#[test]
fn copied_empty_or_failed_contextual_output_retries_once_after_clearing() {
    for first in [
        Ok("Two three four five six."),
        Ok(""),
        Ok(" \n\t"),
        Err("context overran"),
    ] {
        let session = FakeSession::with_outputs(&[first, Ok("heard")]);
        let result = scoped(&session).unwrap();
        assert_eq!(
            result,
            ("heard".to_string(), used(PRIOR_CHARS, true)),
            "{first:?}"
        );
        assert_eq!(
            session.log(),
            [SET_PRIOR, "infer", "set:", "infer"],
            "{first:?}"
        );
    }
}

#[test]
fn failed_retry_reports_both_errors() {
    let session = FakeSession::with_outputs(&[Err("context overran"), Err("still failing")]);
    let error = format!("{:#}", scoped(&session).unwrap_err());
    assert!(error.contains("still failing"), "{error}");
    assert!(error.contains("context overran"), "{error}");
    assert_eq!(session.inferences(), 2);
}

#[test]
fn retry_result_is_final_even_when_empty_or_copied() {
    for last in ["", PRIOR] {
        let session = FakeSession::with_outputs(&[Ok(""), Ok(last)]);
        let result = scoped(&session).unwrap();
        assert_eq!(result, (last.to_string(), used(PRIOR_CHARS, true)));
        assert_eq!(session.inferences(), 2);
    }
}

#[test]
fn failed_clear_never_triggers_a_retry() {
    for output in ["heard", "", PRIOR] {
        let session = FakeSession {
            reject_clear: true,
            ..FakeSession::with_outputs(&[Ok(output)])
        };
        let error = format!("{:#}", scoped(&session).unwrap_err());
        assert!(
            error.contains("clear Qwen context after transcription"),
            "{error}"
        );
        assert_eq!(session.inferences(), 1, "{output:?}");
    }
    let session = FakeSession {
        reject_clear: true,
        ..FakeSession::with_outputs(&[Err("context overran")])
    };
    assert_eq!(scoped(&session).unwrap_err().to_string(), "context overran");
    // Drop repeats the failed clear; the native fault runner expects that final clear.
    assert_eq!(session.log(), [SET_PRIOR, "infer", "set:", "set:"]);
}

#[test]
fn panicking_inference_still_clears_the_context() {
    let session = FakeSession::default();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = scoped(&session);
    }));
    assert!(outcome.is_err());
    assert_eq!(session.log(), [SET_PRIOR, "infer", "set:"]);
}

#[test]
fn device_mode_labels_are_stable() {
    // Exhaustive over ValueEnum::value_variants() with no wildcard arm:
    // adding a DeviceMode variant without a matching label here is a
    // compile error, not a silently-missed test case. Mirrors the
    // value_variants() sweep pattern in config.rs's
    // assert_serde_matches_clap.
    for device in DeviceMode::value_variants() {
        let expected = match device {
            DeviceMode::Auto => "auto",
            DeviceMode::Cpu => "cpu",
            DeviceMode::Gpu => "gpu",
        };
        assert_eq!(device.as_str(), expected, "{device:?}");
    }
}

#[test]
fn segment_text_is_joined_with_single_spaces() {
    let segments = vec![" hello ".to_string(), "world".to_string()];

    assert_eq!(join_segment_text(segments), "hello world");
}

#[test]
fn segment_text_preserves_empty_segment_spacing() {
    let cases: &[(&[&str], &str)] = &[
        (&[], ""),
        (&["", " \t\n"], ""),
        (&["", " hello", "", "\t", "world\n"], "hello world"),
        (&["hello", "", " \t"], "hello "),
        (&[" \u{2003}hello  world\n"], "hello  world"),
    ];
    for &(segments, expected) in cases {
        assert_eq!(
            join_segment_text(segments.iter().map(|text| (*text).to_string())),
            expected,
            "{segments:?}"
        );
    }
}

#[test]
fn missing_model_path_fails_before_crispasr() {
    let err = match Engine::open(
        "target/tmp/definitely-missing-parakit-model.gguf",
        1,
        DeviceMode::Auto,
    ) {
        Ok(_) => panic!("missing model path should fail"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("model path is not a file"));
}

#[test]
fn crispasr_thread_count_accepts_the_c_int_range_only() {
    let max = usize::try_from(i32::MAX).unwrap();
    assert_eq!(crispasr_thread_count(1), Ok(1));
    assert_eq!(crispasr_thread_count(max), Ok(i32::MAX));
    assert!(crispasr_thread_count(max + 1).is_err());
}

#[test]
fn out_of_range_thread_count_is_rejected_in_every_device_mode() {
    let too_many = usize::try_from(i32::MAX).unwrap() + 1;
    for device in DeviceMode::value_variants() {
        let err = match Engine::open(
            "target/tmp/definitely-missing-parakit-model.gguf",
            too_many,
            *device,
        ) {
            Ok(_) => panic!("{device:?}: an out-of-range thread count must fail"),
            Err(err) => err,
        };
        assert!(
            err.to_string()
                .contains("thread count is too large for CrispASR"),
            "{device:?}: {err:#}"
        );
    }
}

#[test]
fn recommended_threads_are_valid_and_scale() {
    for (available, expected) in [
        (0, 1),
        (1, 1),
        (2, 2),
        (3, 2),
        (4, 2),
        (8, 4),
        (12, 6),
        (16, 8),
        (32, 16),
        (64, 32),
    ] {
        assert_eq!(recommended_thread_count(available), expected);
    }
}

#[test]
fn empty_detected_backend_is_rejected() {
    let err = validate_detected_backend("  ".to_string()).expect_err("empty backend fails");

    assert!(err
        .to_string()
        .contains("backend detection returned an empty backend"));
}

#[test]
fn runtime_env_defaults_only_fill_unset_variables() {
    let existing = ["OPENBLAS_NUM_THREADS"];
    let defaults =
        windows_cpu_runtime_env_defaults(8, |key| existing.iter().any(|entry| entry == &key));

    assert_eq!(
        defaults,
        vec![
            RuntimeEnvDefault {
                key: "OMP_NUM_THREADS",
                value: "8".to_string(),
            },
            RuntimeEnvDefault {
                key: "OMP_WAIT_POLICY",
                value: "PASSIVE".to_string(),
            },
        ]
    );
}
