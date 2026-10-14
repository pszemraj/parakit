//! Build-copy and CMake source-substitution tests for bundled native corrections.

#[path = "../build/native_correctness.rs"]
mod native_correctness;

use std::fs;
use std::path::{Path, PathBuf};
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
    fs::File::open(&output)
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
        "cmake_minimum_required(VERSION 3.19)\nproject(crispasr LANGUAGES CXX)\nadd_subdirectory(src)\n",
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
