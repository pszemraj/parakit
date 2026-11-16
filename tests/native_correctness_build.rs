//! Bundled native correction source patches and CMake integration checks.

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
fn native_decoder_faults_and_correctness_helpers_execute() {
    let scratch = Scratch::new();
    fs::write(
        scratch.0.join("CMakeLists.txt"),
        r#"cmake_minimum_required(VERSION 3.19)
project(native_correctness LANGUAGES CXX)
set(CMAKE_CXX_STANDARD 17)
add_executable(qwen-correctness "${PARAKIT_ROOT}/tests/native/qwen_correctness.cpp")
target_include_directories(qwen-correctness PRIVATE "${PARAKIT_ROOT}/build/native" "${PARAKIT_ROOT}/vendor/CrispASR/src")
if(MSVC)
    target_compile_options(qwen-correctness PRIVATE /utf-8)
endif()
enable_testing()
add_test(NAME qwen-correctness COMMAND qwen-correctness)
"#,
    )
    .unwrap();
    let build = scratch.0.join("build");
    checked(
        Command::new("cmake")
            .arg("-S")
            .arg(&scratch.0)
            .arg("-B")
            .arg(&build)
            .arg(format!("-DPARAKIT_ROOT={}", env!("CARGO_MANIFEST_DIR"))),
    );
    checked(
        Command::new("cmake")
            .arg("--build")
            .arg(&build)
            .args(["--config", "Release"]),
    );
    checked(Command::new("ctest").current_dir(&build).args([
        "--build-config",
        "Release",
        "--output-on-failure",
    ]));
}

#[test]
#[ignore = "rebuilds the native CPU library and injects faults into the repo-local Qwen model named by PARAKIT_LIVE_MODEL"]
fn loaded_qwen_session_faults_discard_partial_text_and_recover() {
    // Pass normal absolute paths to CMake; Windows verbatim paths produced by
    // canonicalize become invalid source paths in the MSVC generator.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let model =
        std::path::absolute(std::env::var_os("PARAKIT_LIVE_MODEL").expect("PARAKIT_LIVE_MODEL"))
            .unwrap();
    assert!(
        model
            .canonicalize()
            .unwrap()
            .starts_with(manifest.canonicalize().unwrap()),
        "model must be inside the checkout"
    );
    let scratch = Scratch::new();
    let (api, parakeet) =
        native_correctness::prepare(&manifest.join("vendor/CrispASR"), &scratch.0).unwrap();
    let source = fs::read_to_string(&api).unwrap();
    let source = source
        .replace("qwen3_asr_embed_tokens", "parakit_test::embed")
        .replace("qwen3_asr_run_llm_kv", "parakit_test::forward")
        .replace("qwen3_asr_tokenize", "parakit_test::tokenize");
    let call = "            return transcribe_single(s, pcm + b, (int)(e - b), language);";
    assert_eq!(source.matches(call).count(), 1);
    let source = source.replace(call, "            parakit_test::begin_chunk();\n            auto* part = transcribe_single(s, pcm + b, (int)(e - b), language);\n            parakit_test::end_chunk(part && !part->segments.empty() && !part->segments[0].text.empty());\n            return part;");
    fs::write(&api, format!("{LOADED_FAULT_DECLARATIONS}\n{source}")).unwrap();
    fs::write(
        scratch.0.join("project-hook.cmake"),
        r#"include("${PARAKIT_ROOT}/build/native_correctness.cmake")
# The pinned project has four targets whose input paths assume a top-level
# build. Repair just those targets after declaration, keeping vendor read-only.
function(parakit_test_subproject_paths)
    foreach(target crispasr_c2pa_native crispasr-llama-core index_echo webrtc-vad)
        foreach(property SOURCES INCLUDE_DIRECTORIES INTERFACE_INCLUDE_DIRECTORIES)
            get_target_property(paths ${target} ${property})
            if(paths)
                string(REPLACE "${CMAKE_SOURCE_DIR}/" "${PROJECT_SOURCE_DIR}/" paths "${paths}")
                set_property(TARGET ${target} PROPERTY ${property} "${paths}")
            endif()
        endforeach()
    endforeach()
    # Their original globs searched the wrapper root and found no sources.
    file(GLOB models "${PROJECT_SOURCE_DIR}/examples/talk-llama/models/*.cpp")
    target_sources(crispasr-llama-core PRIVATE ${models})
    file(GLOB_RECURSE webrtc "${PROJECT_SOURCE_DIR}/third_party/webrtc/*.c")
    target_sources(webrtc-vad PRIVATE ${webrtc})
endfunction()
cmake_language(DEFER CALL parakit_test_subproject_paths)
"#,
    )
    .unwrap();
    fs::write(scratch.0.join("CMakeLists.txt"), r#"cmake_minimum_required(VERSION 3.19)
project(parakit_loaded_faults LANGUAGES C CXX)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_MSVC_RUNTIME_LIBRARY MultiThreadedDLL)
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(GGML_NATIVE ON CACHE BOOL "" FORCE)
set(GGML_BACKEND_DL OFF CACHE BOOL "" FORCE)
set(GGML_OPENMP ON CACHE BOOL "" FORCE)
set(GGML_BLAS OFF CACHE BOOL "" FORCE)
set(GGML_CUDA OFF CACHE BOOL "" FORCE)
set(GGML_VULKAN OFF CACHE BOOL "" FORCE)
set(GGML_METAL OFF CACHE BOOL "" FORCE)
set(CRISPASR_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(CRISPASR_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(CRISPASR_BUILD_SERVER OFF CACHE BOOL "" FORCE)
set(CRISPASR_OPUS OFF CACHE BOOL "" FORCE)
set(CRISPASR_AMR OFF CACHE BOOL "" FORCE)
set(CRISPASR_MEL_BLAS OFF CACHE BOOL "" FORCE)
set(CMAKE_PROJECT_crispasr_INCLUDE "${CMAKE_CURRENT_SOURCE_DIR}/project-hook.cmake")
add_subdirectory("${PARAKIT_ROOT}/vendor/CrispASR" crispasr)
add_executable(qwen-loaded-faults "${PARAKIT_ROOT}/tests/native/qwen_correctness.cpp")
target_compile_definitions(qwen-loaded-faults PRIVATE PARAKIT_LOADED_QWEN_FAULTS)
target_include_directories(qwen-loaded-faults PRIVATE "${PARAKIT_ROOT}/build/native" "${PARAKIT_ROOT}/vendor/CrispASR/src")
target_link_libraries(qwen-loaded-faults PRIVATE crispasr-lib)
if(MSVC)
    target_compile_options(qwen-loaded-faults PRIVATE /utf-8)
endif()
enable_testing()
add_test(NAME qwen-loaded-faults COMMAND qwen-loaded-faults "${PARAKIT_MODEL}" "${PARAKIT_ROOT}/vendor/CrispASR/bindings/go/samples/jfk.wav")
"#).unwrap();
    let build = scratch.0.join("build");
    checked(
        Command::new("cmake")
            .arg("-S")
            .arg(&scratch.0)
            .arg("-B")
            .arg(&build)
            .arg(format!("-DPARAKIT_ROOT={}", manifest.display()))
            .arg(format!("-DPARAKIT_MODEL={}", model.display()))
            .arg(format!("-DPARAKIT_CRISPASR_C_API_SOURCE={}", api.display()))
            .arg(format!("-DPARAKIT_PARAKEET_SOURCE={}", parakeet.display()))
            .arg(format!(
                "-DPARAKIT_NATIVE_CORRECTNESS_INCLUDE={}",
                manifest.join("build/native").display()
            ))
            .arg("-DCMAKE_BUILD_TYPE=Release"),
    );
    checked(Command::new("cmake").arg("--build").arg(&build).args([
        "--config",
        "Release",
        "--target",
        "qwen-loaded-faults",
        "--parallel",
        "4",
    ]));
    checked(
        Command::new("ctest")
            .current_dir(&build)
            .args(["--build-config", "Release", "--output-on-failure"])
            .env("CRISPASR_FC_PW_Q8", "0")
            .env("CRISPASR_FC_FUSED_QKV", "0")
            .env("CRISP_AUDIO_WINDOWED_ATTN", "1"),
    );
}

const LOADED_FAULT_DECLARATIONS: &str = r#"#include "qwen3_asr.h"
namespace parakit_test {
float* embed(qwen3_asr_context*, const int32_t*, int);
float* forward(qwen3_asr_context*, const float*, int, int, int*, int*);
int32_t* tokenize(qwen3_asr_context*, const char*, int*);
void begin_chunk();
void end_chunk(bool);
}
"#;

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
