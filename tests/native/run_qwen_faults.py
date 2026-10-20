#!/usr/bin/env python3
"""Exercise loaded Qwen session failures with a separate test-only native library.

Requires a completed Linux or macOS bundled build with compile_commands.json. The source
copy, object, library and results are written beneath --output. No vendor source,
normal build object or installed library is changed. Run explicitly; this is not
part of CI or the default Rust suite.
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import subprocess
import sys
import wave
from pathlib import Path

HOOKS = r'''
#include <cstring>
#include <cstdlib>
static thread_local bool parakit_test_user_audio = false;
static bool parakit_test_mode(const char* mode) {
    const char* selected = std::getenv("PARAKIT_TEST_QWEN_FAILURE");
    if (selected && std::strcmp(selected, "warmup-embed") == 0 &&
        !parakit_test_user_audio && std::strcmp(mode, "embed") == 0)
        return true;
    return parakit_test_user_audio && selected && std::strcmp(selected, mode) == 0;
}
static float* parakit_test_embed(qwen3_asr_context* ctx, const int32_t* ids, int n) {
    if (parakit_test_mode("embed")) {
        fprintf(stderr, "PARAKIT_TEST: embed failure\n");
        return nullptr;
    }
    return qwen3_asr_embed_tokens(ctx, ids, n);
}
static float* parakit_test_forward(qwen3_asr_context* ctx, const float* values,
                                  int n, int past, int* out_n, int* out_vocab) {
    if (parakit_test_mode("forward")) {
        fprintf(stderr, "PARAKIT_TEST: forward failure\n");
        return nullptr;
    }
    return qwen3_asr_run_llm_kv(ctx, values, n, past, out_n, out_vocab);
}
'''


def replace_once(source: str, old: str, new: str) -> str:
    """Replace one known source location in the test copy."""
    if source.count(old) != 1:
        raise RuntimeError(f"test injection expected one source location: {old[:80]!r}")
    return source.replace(old, new, 1)


def build_library(build: Path, output: Path) -> Path:
    """Compile a test copy of the actual corrected C API and relink its library."""
    entries = json.loads((build / "compile_commands.json").read_text(encoding="utf-8"))
    entry, = [item for item in entries if item["file"].endswith("native-correctness/crispasr_c_api.cpp")]
    source = Path(entry["file"]).read_text(encoding="utf-8")
    source = replace_once(source, '#include "qwen3_asr.h"', '#include "qwen3_asr.h"\n' + HOOKS)
    source = replace_once(source, '    const std::string lang = (language && *language) ? language : "en";',
                          '    parakit_test_user_audio = n_samples != 11 * 16000;\n'
                          '    const std::string lang = (language && *language) ? language : "en";')
    source = replace_once(source, "    return parakit_asr::transcribe_chunks<crispasr_session_result>(",
                          "    size_t parakit_test_chunk = 0;\n"
                          "    return parakit_asr::transcribe_chunks<crispasr_session_result>(")
    source = replace_once(source, "            return transcribe_single(s, pcm + b, (int)(e - b), language);", r'''
            ++parakit_test_chunk;
            const char* mode = std::getenv("PARAKIT_TEST_QWEN_FAILURE");
            if (mode && ((std::strcmp(mode, "chunk-first") == 0 && parakit_test_chunk == 1) ||
                         (std::strcmp(mode, "chunk-middle") == 0 && parakit_test_chunk == ranges.size() / 2 + 1) ||
                         (std::strcmp(mode, "chunk-last") == 0 && parakit_test_chunk == ranges.size()))) {
                fprintf(stderr, "PARAKIT_TEST: chunk failure %zu/%zu\n", parakit_test_chunk, ranges.size());
                return (crispasr_session_result*)nullptr;
            }
            return transcribe_single(s, pcm + b, (int)(e - b), language);''')
    source = replace_once(source, "qwen3_asr_embed_tokens, qwen3_asr_run_llm_kv, dec_cfg",
                          "parakit_test_embed, parakit_test_forward, dec_cfg")
    source = replace_once(source, "        int32_t* raw_ids = qwen3_asr_tokenize(s->qwen3_ctx, text.c_str(), &n_prompt);",
                          '        fprintf(stderr, "PARAKIT_TEST_PREFILL:%s\\n", assistant_prefill.c_str());\n'
                          "        int32_t* raw_ids = qwen3_asr_tokenize(s->qwen3_ctx, text.c_str(), &n_prompt);")
    cpp = output / "crispasr_c_api.cpp"
    cpp.write_text(source, encoding="utf-8")
    obj = output / "crispasr_c_api.cpp.o"
    compile_args = shlex.split(entry["command"])
    compile_args[compile_args.index("-o") + 1] = str(obj)
    compile_args[compile_args.index("-c") + 1] = str(cpp)
    for flag in ("-MF", "-MT"):
        if flag in compile_args:
            compile_args[compile_args.index(flag) + 1] = str(output / "faults.d" if flag == "-MF" else obj)
    subprocess.run(compile_args, cwd=entry["directory"], check=True)
    link_args = shlex.split((build / "src/CMakeFiles/crispasr-lib.dir/link.txt").read_text(encoding="utf-8"))
    link_args = [str(obj) if arg.endswith("crispasr_c_api.cpp.o") else arg for arg in link_args]
    original_library = Path(link_args[link_args.index("-o") + 1]).name
    library = output / (original_library if sys.platform == "darwin" else "libcrispasr-faults.so")
    link_args[link_args.index("-o") + 1] = str(library)
    subprocess.run(link_args, cwd=build / "src", check=True)
    if sys.platform == "darwin":
        # dyld resolves the executable's existing install name from this
        # directory. Keep every version alias local to the fault library.
        for installed in (build.parent / "lib").glob("libcrispasr*.dylib"):
            alias = output / installed.name
            if alias != library:
                alias.symlink_to(library.name)
    (output / "build-commands.json").write_text(json.dumps({
        "compile": compile_args, "compile_cwd": entry["directory"],
        "link": link_args, "link_cwd": str(build / "src"),
    }, indent=2) + "\n", encoding="utf-8")
    return library


def run_case(label: str, command: list[str], env: dict[str, str], output: Path) -> tuple[int, str, str]:
    """Capture one explicitly requested loaded-session case."""
    with (output / f"{label}.out").open("w", encoding="utf-8") as stdout, (output / f"{label}.err").open("w", encoding="utf-8") as stderr:
        result = subprocess.run(command, env=env, stdout=stdout, stderr=stderr)
    with (output / "commands.jsonl").open("a", encoding="utf-8") as records:
        records.write(json.dumps({"case": label, "command": command, "status": result.returncode}) + "\n")
    return result.returncode, (output / f"{label}.out").read_text(encoding="utf-8"), (output / f"{label}.err").read_text(encoding="utf-8")


def execute(native_build: Path, binary_dir: Path, model: Path, audio_path: Path, output: Path, device: str = "cpu") -> None:
    """Run five fault modes through both file and production worker paths."""
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with wave.open(str(audio_path), "rb") as audio:
        params = audio.getparams()
        if params.nchannels != 1 or params.sampwidth != 2 or params.framerate != 16000:
            raise ValueError("--audio must be mono PCM16 at 16 kHz")
        frames = audio.readframes(params.nframes)
    long_audio = output / "repeated-speech-90s.wav"
    with wave.open(str(long_audio), "wb") as audio:
        audio.setparams(params)
        audio.writeframes((frames * (90 * 16000 * 2 // len(frames) + 1))[:90 * 16000 * 2])
    library = build_library(native_build.resolve(), output)
    binary = binary_dir.resolve()
    speech = Path("vendor/CrispASR/bindings/go/samples/jfk.wav").resolve()
    native_libraries = str(native_build.resolve().parent / "lib")
    loader = ({"DYLD_LIBRARY_PATH": f"{output}{os.pathsep}{native_libraries}"}
              if sys.platform == "darwin" else
              {"LD_PRELOAD": str(library), "LD_LIBRARY_PATH": native_libraries})
    env = dict(os.environ, **loader,
               PARAKIT_CONFIG_PATH=str(output / "config.toml"),
               XDG_CACHE_HOME=str(output / "cache"), XDG_RUNTIME_DIR=str(Path("target/tmp/nmf-run").resolve()),
               OMP_NUM_THREADS="8", OPENBLAS_NUM_THREADS="8")
    Path(env["XDG_RUNTIME_DIR"]).mkdir(exist_ok=True)
    (output / "config.toml").write_text("# Isolated native correctness tests.\n", encoding="utf-8")
    common = ["--model", str(model.resolve()), "--device", device, "--threads", "8", "--qwen-language", "en"]
    for mode in ("chunk-first", "chunk-middle", "chunk-last", "embed", "forward"):
        mode_env = dict(env, PARAKIT_TEST_QWEN_FAILURE=mode)
        for surface in ("file", "worker"):
            label = f"{mode}-{surface}"
            logs = output / f"{label}-jsonl"
            if surface == "file":
                command = [str(binary / "examples/transcribe-file"), *common, "--warmup", "--audio", str(long_audio)]
            else:
                command = [str(binary / "parakit"), "--verbose", "start", *common, "--no-cleaning", "--no-sounds", "--model-idle-minutes", "0", "--log-dir", str(logs), "--simulate-ptt-audio", str(speech), "--simulate-ptt-audio", str(long_audio)]
            status, stdout, stderr = run_case(label, command, mode_env, output)
            if status != 1 or "PARAKIT_TEST:" not in stderr:
                raise RuntimeError(f"{label}: expected injected native failure, got {status}: {stderr[-500:]}")
            if surface == "file" and "Raw:" in stdout:
                raise RuntimeError(f"{label}: failed utterance returned a transcript")
            if surface == "worker":
                rows = [json.loads(line) for path in logs.glob("*.jsonl") for line in path.read_text(encoding="utf-8").splitlines()]
                if len([row for row in rows if "raw" in row]) != 1 or len(rows) != 2:
                    raise RuntimeError(f"{label}: failed input produced successful logging/insertion")
                if "failed simulated dictation left history unchanged" not in stdout:
                    raise RuntimeError(f"{label}: missing history read-back")
            print(f"passed {label}", flush=True)
    for language, canonical in (("en", "English"), ("yue", "Cantonese"), ("fil", "Filipino")):
        command = [str(binary / "examples/transcribe-file"), "--model", str(model.resolve()), "--device", device, "--threads", "8", "--warmup", "--audio", str(speech), "--qwen-language", language]
        status, _, stderr = run_case(f"prefix-{language}", command, env, output)
        if status or f"PARAKIT_TEST_PREFILL:language {canonical}<asr_text>" not in stderr:
            raise RuntimeError(f"native prefix check failed for {language} (exit {status})")
        print(f"passed native prefix {language}", flush=True)
    harness = max((path for path in (binary / "deps").glob("native_correctness_build-*")
                   if path.is_file() and os.access(path, os.X_OK)), key=lambda path: path.stat().st_mtime)
    status, stdout, stderr = run_case("warmup-restoration", [str(harness), "--exact", "qwen_readiness_error_restores_user_options", "--ignored", "--nocapture"],
                                     dict(env, PARAKIT_TEST_QWEN_FAILURE="warmup-embed", PARAKIT_TEST_QWEN_MODEL=str(model.resolve()), PARAKIT_TEST_QWEN_DEVICE=device), output)
    if status or "1 passed" not in stdout or "generated 1 of 1 tokens" not in stderr or "PARAKIT_TEST_PREFILL:language English<asr_text>" not in stderr:
        raise RuntimeError(f"failed readiness did not restore actual native user controls: {stderr[-1000:]}")
    print("passed failed readiness option restoration", flush=True)


def main() -> int:
    """Parse explicit model/build inputs and run loaded native correctness checks."""
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.ArgumentDefaultsHelpFormatter)
    parser.add_argument("--native-build", type=Path, required=True, help="CMake build directory from the bundled Cargo build")
    parser.add_argument("--binary-dir", type=Path, required=True, help="directory containing parakit and examples/transcribe-file")
    parser.add_argument("--model", type=Path, required=True, help="Qwen GGUF to load")
    parser.add_argument("--audio", type=Path, required=True, help="human speech WAV used to construct a 90-second failure fixture")
    parser.add_argument("--output", type=Path, required=True, help="ignored repository-local results/build directory; use a fresh directory")
    parser.add_argument("--device", choices=("cpu", "gpu"), default="cpu", help="native device to exercise; GPU requires a matching bundled build")
    args = parser.parse_args()
    try:
        execute(args.native_build, args.binary_dir, args.model, args.audio, args.output, args.device)
    except (OSError, ValueError, wave.Error, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
