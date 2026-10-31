//! Bundled native corrections and opt-in loaded Qwen session checks.

#[path = "../build/native_correctness.rs"]
mod native_correctness;

use std::fs;
use std::path::{Path, PathBuf};

#[test]
#[ignore = "requires PARAKIT_TEST_QWEN_MODEL and the separate warmup-embed fault library"]
fn qwen_readiness_error_restores_user_options() {
    let model = std::env::var("PARAKIT_TEST_QWEN_MODEL").expect("explicit Qwen model");
    let device = qwen_test_device();
    let engine = parakit::inference::Engine::open_with_options(
        model,
        8,
        device,
        &parakit::qwen::QwenOptions {
            language: Some("English".into()),
            max_new_tokens: 1,
        },
    )
    .unwrap();
    assert_eq!(engine.device_mode(), device);
    assert!(parakit::warmup::run(&engine, &[1]).is_err());
    let wav = parakit::audio_file::prepare_wav_for_model(Path::new(
        "vendor/CrispASR/bindings/go/samples/jfk.wav",
    ))
    .unwrap();
    assert!(
        engine.transcribe(&wav.samples[..16_000]).is_err(),
        "the user's one-token budget must be restored after failed readiness"
    );
}

/// Select the native device explicitly requested for a loaded-model test.
fn qwen_test_device() -> parakit::inference::DeviceMode {
    match std::env::var("PARAKIT_TEST_QWEN_DEVICE")
        .unwrap_or_else(|_| "cpu".into())
        .as_str()
    {
        "auto" => parakit::inference::DeviceMode::Auto,
        "cpu" => parakit::inference::DeviceMode::Cpu,
        "gpu" => parakit::inference::DeviceMode::Gpu,
        _ => panic!("PARAKIT_TEST_QWEN_DEVICE must be auto, cpu, or gpu"),
    }
}

/// Load explicit model/audio fixtures and the prior text used by context probes.
fn qwen_context_fixture() -> (
    parakit::inference::Engine,
    parakit::audio_file::PreparedWav,
    String,
) {
    let model = std::env::var("PARAKIT_TEST_QWEN_MODEL").expect("explicit Qwen model");
    let audio = std::env::var_os("PARAKIT_TEST_QWEN_AUDIO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("vendor/CrispASR/bindings/go/samples/jfk.wav"));
    let context = std::env::var("PARAKIT_TEST_QWEN_CONTEXT").unwrap_or_else(|_| {
        "CrispASR, Qwen3-ASR, ParaKit.\nPrevious dictation: café and naïve.".into()
    });
    let engine = parakit::inference::Engine::open_with_options(
        model,
        8,
        qwen_test_device(),
        &parakit::qwen::QwenOptions {
            language: Some("English".into()),
            max_new_tokens: 512,
        },
    )
    .unwrap();
    let audio = parakit::audio_file::prepare_wav_for_model(&audio).unwrap();
    (engine, audio, context)
}

/// Record one actual transcription and its latency for inspection with native traces.
fn context_probe(
    engine: &parakit::inference::Engine,
    pcm: &[f32],
    label: &str,
    context: Option<&str>,
) -> anyhow::Result<String> {
    eprintln!("PARAKIT_TEST_CALL:{label}");
    let started = std::time::Instant::now();
    let result = engine.transcribe_with_context(pcm, context);
    println!(
        "PARAKIT_TEST_RESULT:{}",
        serde_json::json!({
            "case": label,
            "seconds": started.elapsed().as_secs_f64(),
            "raw": result.as_ref().ok(),
            "error": result.as_ref().err().map(ToString::to_string),
        })
    );
    result
}

#[test]
#[ignore = "requires PARAKIT_TEST_QWEN_MODEL and native prompt tracing for routing assertions"]
fn qwen_context_is_scoped_to_each_transcription() {
    let (engine, audio, context) = qwen_context_fixture();
    for (label, prior) in [
        ("ordinary-baseline", None),
        ("context-first", Some(context.as_str())),
        ("ordinary-after-success", None),
        ("context-again", Some(context.as_str())),
        ("invalid-context-fallback", Some("unusable\0context")),
    ] {
        let transcript = context_probe(&engine, &audio.samples, label, prior).unwrap();
        assert!(
            !transcript.trim().is_empty(),
            "{label}: empty speech transcript"
        );
    }
}

#[test]
#[ignore = "requires PARAKIT_TEST_QWEN_MODEL and the context-embed fault library"]
fn qwen_context_error_does_not_affect_next_transcription() {
    let (engine, audio, context) = qwen_context_fixture();
    assert!(context_probe(&engine, &audio.samples, "context-failure", Some(&context)).is_err());
    let transcript =
        context_probe(&engine, &audio.samples, "ordinary-after-failure", None).unwrap();
    assert!(!transcript.trim().is_empty());
}

#[test]
#[ignore = "requires PARAKIT_TEST_QWEN_MODEL and optional audio/context fixtures"]
/// Report native outputs; the daemon rejects silent captures before inference.
fn qwen_context_handles_silent_and_short_audio() {
    let (engine, audio, context) = qwen_context_fixture();
    let silence = vec![0.0; 16_000];
    for (label, prior) in [
        ("silence-ordinary", None),
        ("silence-context", Some(context.as_str())),
    ] {
        context_probe(&engine, &silence, label, prior).unwrap();
    }
    let short = &audio.samples[..audio.samples.len().min(8_000)];
    let ordinary = context_probe(&engine, short, "short-ordinary", None).unwrap();
    let contextual = context_probe(&engine, short, "short-context", Some(&context)).unwrap();
    assert_eq!(
        contextual, ordinary,
        "subsecond audio must use ordinary inference"
    );
}

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

const SOURCE: &str = include_str!("../vendor/CrispASR/src/crispasr_c_api.cpp");
const PARAKEET_SOURCE: &str = include_str!("../vendor/CrispASR/src/parakeet_orchestrate.cpp");

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/tmp/native-correctness-tests")
            .join(format!(
                "{}-{} with spaces",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn pinned_source_uses_the_tested_helpers() {
    let patched = native_correctness::patch_source(SOURCE).unwrap();
    for call in [
        "parakit_asr::transcribe_chunks<crispasr_session_result>",
        "s->backend == \"qwen3\" && !parakit_asr::valid_eos(n_eos, eos_id, vocab)",
        "s->backend == \"qwen3\" && !parakit_asr::decode_completed(dec.tokens, eos_id, vocab, q3_max_new)",
        "parakit_asr::transcript_pieces(std::move(pieces), !raon && assistant_prefill.empty())",
    ] {
        assert_eq!(patched.matches(call).count(), 1, "{call}");
    }
    assert!(!patched.contains("if (!raon && piece == \"language\")"));
    // Other backend implementations are deliberately retained byte-for-byte.
    let qwen_end = "#ifdef CA_HAVE_COHERE\n    if (s->backend == \"cohere\" && s->cohere_ctx)";
    assert!(patched.split_once(qwen_end).unwrap().1 == SOURCE.split_once(qwen_end).unwrap().1);
}

#[test]
fn unknown_or_duplicate_patch_anchors_fail_clearly() {
    let changed = SOURCE.replace(
        "        if (!part)\n            continue;",
        "        if (!part) return nullptr;",
    );
    let error = native_correctness::patch_source(&changed).unwrap_err();
    assert!(error.contains("chunk merger"), "{error}");
    let duplicate = format!("{SOURCE}\n#include \"crispasr_session.h\"\n");
    let error = native_correctness::patch_source(&duplicate).unwrap_err();
    assert!(error.contains("expected once, found 2"), "{error}");
}

#[test]
fn windows_line_endings_apply_the_same_corrections() {
    assert!(
        native_correctness::patch_source(&SOURCE.replace('\n', "\r\n")).unwrap()
            == native_correctness::patch_source(SOURCE).unwrap()
    );
    assert!(
        native_correctness::patch_parakeet_source(&PARAKEET_SOURCE.replace('\n', "\r\n")).unwrap()
            == native_correctness::patch_parakeet_source(PARAKEET_SOURCE).unwrap()
    );
}

#[test]
fn parakeet_recovery_notice_is_removed_without_hiding_warnings() {
    let patched = native_correctness::patch_parakeet_source(PARAKEET_SOURCE).unwrap();
    assert!(!patched.contains("gap-fill recovered %d word(s)"));
    let unchanged_tail =
        "        // Re-sort and rebuild only the segments a word was inserted into.";
    assert!(
        patched.split_once(unchanged_tail).unwrap().1
            == PARAKEET_SOURCE.split_once(unchanged_tail).unwrap().1
    );
    assert!(patched.contains("single-pass encode failed (likely VRAM OOM"));
    assert!(native_correctness::patch_parakeet_source(&patched).is_err());
}

#[test]
fn preparation_writes_only_the_output_copy() {
    let scratch = Scratch::new();
    let source_dir = scratch.0.join("source");
    let input = source_dir.join("src/crispasr_c_api.cpp");
    fs::create_dir_all(input.parent().unwrap()).unwrap();
    fs::write(&input, SOURCE).unwrap();
    let orchestration = source_dir.join("src/parakeet_orchestrate.cpp");
    fs::write(&orchestration, PARAKEET_SOURCE).unwrap();
    let output_dir = scratch.0.join("out");
    let (output, parakeet_output) = native_correctness::prepare(&source_dir, &output_dir).unwrap();
    assert!(output.starts_with(&output_dir));
    assert_eq!(fs::read_to_string(&input).unwrap(), SOURCE);
    assert_eq!(
        fs::read_to_string(&output).unwrap(),
        native_correctness::patch_source(SOURCE).unwrap()
    );
    let old_time = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
    fs::File::options()
        .write(true)
        .open(&output)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(old_time))
        .unwrap();
    native_correctness::prepare(&source_dir, &output_dir).unwrap();
    assert_eq!(fs::metadata(&output).unwrap().modified().unwrap(), old_time);
    assert_eq!(fs::read_to_string(&orchestration).unwrap(), PARAKEET_SOURCE);
    assert!(parakeet_output.starts_with(&output_dir));
    assert_eq!(
        fs::read_to_string(&parakeet_output).unwrap(),
        native_correctness::patch_parakeet_source(PARAKEET_SOURCE).unwrap()
    );
    let revised = format!("// revised source\n{SOURCE}");
    fs::write(&input, &revised).unwrap();
    native_correctness::prepare(&source_dir, &output_dir).unwrap();
    assert!(fs::read_to_string(output)
        .unwrap()
        .starts_with("// revised source\n"));
    assert_eq!(fs::read_to_string(input).unwrap(), revised);
}

fn checked(command: &mut Command) {
    let output = command
        .output()
        .expect("CMake must be available for bundled builds");
    assert!(
        output.status.success(),
        "{command:?}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cmake_hook_compiles_the_copy_and_excludes_the_original() {
    let scratch = Scratch::new();
    let root = &scratch.0;
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.19)\nproject(crispasr LANGUAGES CXX)\nset(CMAKE_CXX_STANDARD 17)\nadd_subdirectory(src)\n",
    )
    .unwrap();
    fs::write(
        root.join("src/CMakeLists.txt"),
        "add_library(crispasr-lib STATIC crispasr_c_api.cpp)\nadd_library(parakeet STATIC parakeet_orchestrate.cpp)\n",
    )
    .unwrap();
    fs::write(
        root.join("src/parakeet_orchestrate.cpp"),
        "#error original must not compile\n",
    )
    .unwrap();
    fs::write(
        root.join("src/crispasr_c_api.cpp"),
        "#error original must not compile\n",
    )
    .unwrap();
    let copy = root.join("patched.cpp");
    fs::write(&copy, "#include \"qwen_correctness.hpp\"\nint answer() { return parakit_asr::decode_completed({5}, 5, 8, 1) ? 42 : 0; }\n").unwrap();
    let parakeet_copy = root.join("patched-parakeet.cpp");
    fs::write(&parakeet_copy, "int parakeet_answer() { return 42; }\n").unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::create_dir_all(root.join("src/core")).unwrap();
    fs::copy(
        manifest.join("vendor/CrispASR/src/core/audio_chunking.h"),
        root.join("src/core/audio_chunking.h"),
    )
    .unwrap();
    checked(
        Command::new("cmake")
            .arg("-S")
            .arg(root)
            .arg("-B")
            .arg(root.join("build"))
            .arg(format!(
                "-DCMAKE_PROJECT_crispasr_INCLUDE={}",
                manifest.join("build/native_correctness.cmake").display()
            ))
            .arg(format!(
                "-DPARAKIT_CRISPASR_C_API_SOURCE={}",
                copy.display()
            ))
            .arg(format!(
                "-DPARAKIT_PARAKEET_SOURCE={}",
                parakeet_copy.display()
            ))
            .arg(format!(
                "-DPARAKIT_NATIVE_CORRECTNESS_INCLUDE={}",
                manifest.join("build/native").display()
            )),
    );
    checked(
        Command::new("cmake")
            .arg("--build")
            .arg(root.join("build"))
            .args(["--config", "Release"]),
    );
    // Reconfiguration must not accidentally compile two copies of the C ABI.
    checked(
        Command::new("cmake")
            .arg("-S")
            .arg(root)
            .arg("-B")
            .arg(root.join("build")),
    );
    checked(
        Command::new("cmake")
            .arg("--build")
            .arg(root.join("build"))
            .args(["--config", "Release"]),
    );
}
