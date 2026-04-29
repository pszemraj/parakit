//! Builds (or locates) the CrispASR C library and tells the linker where it is.
//!
//! Default behavior (with the `bundled` feature on, which is the default):
//!   - Vendored source is at `vendor/CrispASR/`. Cargo must see that submodule
//!     before dependency resolution because the `crispasr` Rust crate is a path
//!     dependency.
//!   - Configure & build via the `cmake` crate. Output is cached in
//!     `OUT_DIR` so subsequent `cargo build` invocations are incremental.
//!   - Backend selection (`cuda` / `metal` / `vulkan`) is driven by parakit's
//!     own cargo features, not by `crispasr-sys` (which is a pure shim and
//!     doesn't compile anything).
//!   - Emits a `rustc-link-search` so `crispasr-sys`'s `link-lib=crispasr`
//!     resolves to our just-built library.
//!   - Emits an rpath on Unix so the binary finds the dylib at runtime
//!     without needing LD_LIBRARY_PATH.
//!
//! Escape hatches (any one of these skips the bundled build):
//!   - `--no-default-features`              : the user takes responsibility for
//!     providing libcrispasr; add `--features daemon` when building the daemon.
//!   - `CRISPASR_LIB_DIR=/path/to/libdir`   : link-search path override.
//!   - `CRISPASR_SRC_DIR=/path/to/source`   : use this checkout instead of the vendored source.
//!
//! These still require Cargo to load the `crispasr` Rust path dependency unless
//! the manifest is changed, so a missing submodule must be fixed before Cargo
//! can start this script.

use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=CRISPASR_LIB_DIR");
    println!("cargo:rerun-if-env-changed=CRISPASR_SRC_DIR");
    println!("cargo:rerun-if-env-changed=PARAKIT_BLAS");
    println!("cargo:rerun-if-changed=build.rs");

    // 1. Honor an explicit lib-dir override regardless of feature flags.
    //    crispasr-sys reads CRISPASR_LIB_DIR too — we add to its search path
    //    here so `cargo build` works without the user re-exporting the var.
    if let Ok(dir) = env::var("CRISPASR_LIB_DIR") {
        println!("cargo:rustc-link-search=native={dir}");
        emit_rpath(Path::new(&dir));
        return;
    }

    // 2. If the user disabled the `bundled` feature, do nothing.
    //    crispasr-sys's own build.rs will probe /usr/local/lib, /opt/homebrew/lib, etc.
    if env::var("CARGO_FEATURE_BUNDLED").is_err() {
        return;
    }

    // 3. Locate the CrispASR source tree.
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src_dir = locate_source(&manifest_dir);
    println!(
        "cargo:rerun-if-changed={}/CMakeLists.txt",
        src_dir.display()
    );

    // 4. Run cmake. The `cmake` crate handles incremental builds, MSVC
    //    detection on Windows, generator selection, parallelism, and
    //    install-step plumbing.
    let mut cfg = cmake::Config::new(&src_dir);
    cfg.profile("Release")
        .define("BUILD_SHARED_LIBS", "ON")
        // parakit is normally built from source for the machine it runs on.
        // Make the CPU policy explicit instead of relying on ggml defaults.
        .define("GGML_NATIVE", "ON")
        .define("GGML_OPENMP", "ON")
        .define("GGML_CPU_REPACK", "ON")
        // Skip tests. Build examples because CrispASR's quantizer lives there.
        .define("WHISPER_BUILD_TESTS", "OFF")
        // CrispASR's GGUF requantizer lives under examples/. We build the
        // examples tree so source rebuilds can invoke `crispasr-quantize`.
        .define("WHISPER_BUILD_EXAMPLES", "ON")
        .define("GGML_BUILD_TESTS", "OFF")
        .define("GGML_BUILD_EXAMPLES", "OFF")
        // Bake an `$ORIGIN` (Linux/BSD) or `@loader_path` (macOS) rpath into
        // the installed shared libraries so each library finds its siblings
        // (libwhisper.so → libggml.so → libggml-cpu.so) without LD_LIBRARY_PATH.
        // Without this, libwhisper.so's transitive deps (libggml*.so) fail to
        // resolve at load time even when the binary's own rpath points at
        // the right directory — Linux's DT_RUNPATH doesn't apply transitively.
        .define("CMAKE_INSTALL_RPATH", install_rpath_token())
        .define("CMAKE_BUILD_WITH_INSTALL_RPATH", "ON")
        .define("CMAKE_INSTALL_RPATH_USE_LINK_PATH", "ON");

    let cuda_enabled = cargo_feature("cuda");
    let metal_enabled = cargo_feature("metal");
    let vulkan_enabled = cargo_feature("vulkan");

    configure_blas(&mut cfg);
    cfg.define("GGML_CUDA", if cuda_enabled { "ON" } else { "OFF" });
    cfg.define("GGML_VULKAN", if vulkan_enabled { "ON" } else { "OFF" });
    if metal_enabled {
        if target_is_apple() {
            cfg.define("GGML_METAL", "ON");
        } else if cuda_enabled || vulkan_enabled {
            cfg.define("GGML_METAL", "OFF");
            println!(
                "cargo:warning=ignoring unsupported metal feature on non-Apple target during multi-backend build"
            );
        } else {
            panic!("the metal feature is only supported on Apple targets");
        }
    } else {
        cfg.define("GGML_METAL", "OFF");
    }

    let install_dir = cfg.build();
    emit_build_report(&install_dir);

    // 5. CrispASR's CMake builds `libwhisper.{so,dylib,dll}` as the umbrella
    //    shared library — every backend (parakeet, voxtral, qwen3, ...) is a
    //    static lib that gets statically linked INTO libwhisper. The build
    //    also creates a `libcrispasr.{so,dylib}` symlink pointing at libwhisper
    //    inside the build directory, but that alias is *not* installed.
    //
    //    `crispasr-sys` defaults to looking for `libcrispasr` (it accepts
    //    `CRISPASR_LIB_NAME=whisper` as an override but we can't set env vars
    //    for sibling build scripts). Cleanest solution: recreate the alias
    //    inside install_dir/lib so the canonical link name resolves.
    let lib_dir = install_dir.join("lib");
    let lib_dir_alt = install_dir.join("lib64");
    let final_lib_dir = if lib_dir.is_dir() {
        lib_dir
    } else if lib_dir_alt.is_dir() {
        lib_dir_alt
    } else if target_is_windows() {
        std::fs::create_dir_all(&lib_dir).unwrap_or_else(|err| {
            panic!(
                "failed to create Windows import-library dir {}: {err}",
                lib_dir.display()
            )
        });
        lib_dir
    } else {
        panic!(
            "expected install dir to contain lib/ or lib64/, got {}",
            install_dir.display()
        );
    };

    let bin_dir = install_dir.join("bin");
    if target_is_windows() {
        prepare_windows_artifacts(&install_dir, &final_lib_dir, &bin_dir);
    } else {
        create_crispasr_alias(&final_lib_dir);
    }

    println!("cargo:rustc-link-search=native={}", final_lib_dir.display());

    // Windows DLLs land in bin/, not lib/. Add it for completeness.
    if bin_dir.is_dir() {
        println!("cargo:rustc-link-search=native={}", bin_dir.display());
    }

    let quantize_bin = bin_dir.join(exe_name("crispasr-quantize"));
    if quantize_bin.is_file() {
        println!(
            "cargo:rustc-env=CRISPASR_QUANTIZE_BIN={}",
            quantize_bin.display()
        );
    } else {
        println!(
            "cargo:warning=crispasr-quantize was not installed at {}; source rebuilds will look on PATH",
            quantize_bin.display()
        );
    }

    // 6. Bake the lib path into the binary's rpath so we don't need
    //    LD_LIBRARY_PATH / DYLD_FALLBACK_LIBRARY_PATH at runtime.
    //    No-op on Windows; DLLs are copied into the profile dir above and the
    //    Windows installer script copies them next to the installed exe.
    emit_rpath(&final_lib_dir);

    // 7. Re-export the install path. Useful for `cargo run` on macOS
    //    (where DYLD_LIBRARY_PATH is hostile) and for downstream tooling
    //    that wants to copy the dylib into a release artifact.
    println!(
        "cargo:rustc-env=CRISPASR_INSTALL_DIR={}",
        install_dir.display()
    );
}

fn emit_build_report(install_dir: &Path) {
    let build_dir = install_dir.join("build");
    let cache_path = build_dir.join("CMakeCache.txt");
    let cache = read_cmake_cache(&cache_path);
    let cpu_flags = read_cpu_flags(&build_dir);

    emit_env_from_cache(&cache, "CMAKE_BUILD_TYPE", "PARAKIT_BUILD_CMAKE_BUILD_TYPE");
    for key in [
        "GGML_NATIVE",
        "GGML_OPENMP",
        "GGML_CPU_REPACK",
        "GGML_BLAS",
        "GGML_BLAS_VENDOR",
        "COHERE_MKL",
        "GGML_CUDA",
        "GGML_VULKAN",
        "GGML_METAL",
        "CMAKE_CUDA_ARCHITECTURES",
    ] {
        let env_key = format!("PARAKIT_BUILD_{key}");
        emit_env_from_cache(&cache, key, &env_key);
    }

    if let Some(flags) = cpu_flags {
        println!("cargo:rustc-env=PARAKIT_BUILD_CPU_FLAGS={flags}");
    } else {
        println!(
            "cargo:warning=could not read ggml CPU flags from {}",
            build_dir.display()
        );
    }
}

fn configure_blas(cfg: &mut cmake::Config) {
    let blas = BlasConfig::from_env();
    cfg.define("GGML_BLAS", if blas.enabled { "ON" } else { "OFF" });
    cfg.define("COHERE_MKL", if blas.cohere_mkl { "ON" } else { "OFF" });
    if let Some(vendor) = blas.vendor {
        cfg.define("GGML_BLAS_VENDOR", vendor);
        if blas.cohere_mkl {
            cfg.define("BLA_VENDOR", vendor);
        }
    }

    println!(
        "cargo:rustc-env=PARAKIT_BUILD_BLAS_REQUEST={}",
        blas.requested
    );
    println!(
        "cargo:rustc-env=PARAKIT_BUILD_BLAS_SELECTED={}",
        blas.selected
    );
    if blas.explicit {
        println!(
            "cargo:warning=parakit build: PARAKIT_BLAS={} selected {}",
            blas.requested, blas.selected
        );
    }
}

struct BlasConfig {
    requested: String,
    selected: &'static str,
    enabled: bool,
    vendor: Option<&'static str>,
    cohere_mkl: bool,
    explicit: bool,
}

impl BlasConfig {
    fn from_env() -> Self {
        let raw = env::var("PARAKIT_BLAS").unwrap_or_else(|_| "off".to_string());
        let requested = raw.trim().to_ascii_lowercase();
        let explicit = env::var("PARAKIT_BLAS").is_ok();
        match requested.as_str() {
            "" | "0" | "false" | "no" | "none" | "off" => Self::off(raw, explicit),
            "auto" => Self::auto(raw),
            "mkl" | "intel" | "intel-mkl" => Self::mkl(raw, explicit),
            "openblas" => Self::openblas(raw, explicit),
            "accelerate" | "apple" => Self::accelerate(raw, explicit),
            "1" | "true" | "yes" | "on" | "blas" | "generic" | "system" => {
                Self::generic(raw, explicit)
            }
            other => panic!(
                "unsupported PARAKIT_BLAS={other}. Use off, auto, openblas, mkl, accelerate, or generic."
            ),
        }
    }

    fn auto(raw: String) -> Self {
        if target_is_apple() {
            return Self::accelerate(raw, true);
        }
        if pkg_config_exists("mkl-sdl") {
            return Self::mkl(raw, true);
        }
        if pkg_config_exists("openblas") || pkg_config_exists("openblas64") {
            return Self::openblas(raw, true);
        }
        println!(
            "cargo:warning=parakit build: PARAKIT_BLAS=auto found no MKL/OpenBLAS pkg-config metadata; building without BLAS"
        );
        Self::off(raw, true)
    }

    fn off(requested: String, explicit: bool) -> Self {
        Self {
            requested,
            selected: "off",
            enabled: false,
            vendor: None,
            cohere_mkl: false,
            explicit,
        }
    }

    fn generic(requested: String, explicit: bool) -> Self {
        Self {
            requested,
            selected: "generic",
            enabled: true,
            vendor: Some("Generic"),
            cohere_mkl: false,
            explicit,
        }
    }

    fn openblas(requested: String, explicit: bool) -> Self {
        Self {
            requested,
            selected: "openblas",
            enabled: true,
            vendor: Some("OpenBLAS"),
            cohere_mkl: false,
            explicit,
        }
    }

    fn mkl(requested: String, explicit: bool) -> Self {
        Self {
            requested,
            selected: "mkl",
            enabled: true,
            vendor: Some("Intel10_64lp"),
            cohere_mkl: true,
            explicit,
        }
    }

    fn accelerate(requested: String, explicit: bool) -> Self {
        if !target_is_apple() {
            panic!("PARAKIT_BLAS=accelerate is only supported on Apple targets");
        }
        Self {
            requested,
            selected: "accelerate",
            enabled: true,
            vendor: Some("Apple"),
            cohere_mkl: false,
            explicit,
        }
    }
}

fn pkg_config_exists(package: &str) -> bool {
    Command::new("pkg-config")
        .args(["--exists", package])
        .status()
        .is_ok_and(|status| status.success())
}

fn read_cmake_cache(path: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        println!(
            "cargo:warning=could not read CMake cache at {}; build diagnostics will be sparse",
            path.display()
        );
        return BTreeMap::new();
    };

    let mut values = BTreeMap::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        let Some((key_with_type, value)) = line.split_once('=') else {
            continue;
        };
        let key = key_with_type
            .split_once(':')
            .map_or(key_with_type, |(key, _)| key);
        values.insert(key.to_string(), value.to_string());
    }
    values
}

fn read_cpu_flags(build_dir: &Path) -> Option<String> {
    let flags_path = build_dir.join("ggml/src/CMakeFiles/ggml-cpu.dir/flags.make");
    let text = std::fs::read_to_string(flags_path).ok()?;
    let mut cxx_flags = None;
    let mut c_flags = None;
    for line in text.lines() {
        if let Some((key, value)) = line.split_once(" = ") {
            match key {
                "CXX_FLAGS" => cxx_flags = Some(value),
                "C_FLAGS" => c_flags = Some(value),
                _ => {}
            }
        }
    }

    let flags = cxx_flags.or(c_flags)?;
    Some(summarize_cpu_flags(flags))
}

fn summarize_cpu_flags(flags: &str) -> String {
    let interesting = [
        "-O3",
        "-march=native",
        "-fopenmp",
        "-mavx512bf16",
        "-mavx512vnni",
        "-mavx512f",
        "-mavx2",
        "-mfma",
        "-mf16c",
        "-mbmi2",
        "-mavx",
        "-msse4.2",
        "/arch:AVX512",
        "/arch:AVX2",
        "/arch:AVX",
        "/arch:SSE4.2",
    ];
    let mut found = Vec::new();
    for flag in interesting {
        if flags.split_whitespace().any(|part| part == flag) {
            found.push(flag);
        }
    }
    if found.is_empty() {
        "none detected".to_string()
    } else {
        found.join(" ")
    }
}

fn emit_env_from_cache(cache: &BTreeMap<String, String>, cache_key: &str, env_key: &str) {
    if let Some(value) = cache.get(cache_key) {
        println!("cargo:rustc-env={env_key}={value}");
    }
}

/// Returns true if the named cargo feature is enabled.
fn cargo_feature(name: &str) -> bool {
    env::var(format!(
        "CARGO_FEATURE_{}",
        name.to_uppercase().replace('-', "_")
    ))
    .is_ok()
}

fn target_is_apple() -> bool {
    matches!(
        env::var("CARGO_CFG_TARGET_OS").unwrap_or_default().as_str(),
        "macos" | "ios"
    )
}

fn target_is_windows() -> bool {
    env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows"
}

fn exe_name(name: &str) -> String {
    if target_is_windows() {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Find the CrispASR source. Order:
///   1. CRISPASR_SRC_DIR (explicit override)
///   2. vendor/CrispASR (git submodule)
///   3. Try `git submodule update --init --recursive vendor/CrispASR` and retry
///   4. Fail with actionable error
///
/// In normal builds Cargo resolves the `crispasr` path dependency before this
/// function can run, so a completely missing submodule still has to be fixed
/// with `git submodule update --init --recursive` or `scripts/install-windows.ps1`.
fn locate_source(manifest_dir: &Path) -> PathBuf {
    if let Ok(d) = env::var("CRISPASR_SRC_DIR") {
        let p = PathBuf::from(d);
        if p.join("CMakeLists.txt").is_file() {
            return p;
        }
        panic!(
            "CRISPASR_SRC_DIR={} does not contain CMakeLists.txt",
            p.display()
        );
    }

    let vendored = manifest_dir.join("vendor/CrispASR");
    if vendored.join("CMakeLists.txt").is_file() {
        return vendored;
    }

    // Try to init the submodule.
    eprintln!(
        "parakit build.rs: vendor/CrispASR is empty, running `git submodule update --init`..."
    );
    let status = Command::new("git")
        .args([
            "submodule",
            "update",
            "--init",
            "--recursive",
            "vendor/CrispASR",
        ])
        .current_dir(manifest_dir)
        .status();

    if matches!(status, Ok(s) if s.success()) && vendored.join("CMakeLists.txt").is_file() {
        return vendored;
    }

    panic!(
        "\n\
         parakit build.rs: cannot find CrispASR source.\n\
         \n\
         Pick one of:\n\
           1. Initialize the submodule:\n\
                git submodule update --init --recursive\n\
           2. Vendor a checkout manually:\n\
                git clone https://github.com/CrispStrobe/CrispASR vendor/CrispASR\n\
           3. Point at an existing checkout:\n\
                CRISPASR_SRC_DIR=/path/to/CrispASR cargo build\n\
           4. Use a system-installed library and skip the bundled build:\n\
                cargo build --no-default-features --features daemon\n\
              (or set CRISPASR_LIB_DIR=/path/to/libdir to override the search path)\n"
    );
}

fn prepare_windows_artifacts(install_dir: &Path, lib_dir: &Path, bin_dir: &Path) {
    std::fs::create_dir_all(lib_dir).unwrap_or_else(|err| {
        panic!(
            "failed to create Windows import-library dir {}: {err}",
            lib_dir.display()
        )
    });
    std::fs::create_dir_all(bin_dir).unwrap_or_else(|err| {
        panic!(
            "failed to create Windows runtime DLL dir {}: {err}",
            bin_dir.display()
        )
    });

    copy_windows_runtime_dlls(install_dir, bin_dir);

    let (whisper_import, crispasr_import) = windows_import_library_names();
    copy_named_artifact(install_dir, whisper_import, lib_dir);

    let whisper_import_path = lib_dir.join(whisper_import);
    let crispasr_import_path = lib_dir.join(crispasr_import);
    let _ = std::fs::remove_file(&crispasr_import_path);
    std::fs::copy(&whisper_import_path, &crispasr_import_path).unwrap_or_else(|err| {
        panic!(
            "failed to create {} from {}: {err}",
            crispasr_import_path.display(),
            whisper_import_path.display()
        )
    });

    copy_named_artifact(install_dir, "whisper.dll", bin_dir);
    let whisper_dll = bin_dir.join("whisper.dll");
    let crispasr_dll = bin_dir.join("crispasr.dll");
    let _ = std::fs::remove_file(&crispasr_dll);
    std::fs::copy(&whisper_dll, &crispasr_dll).unwrap_or_else(|err| {
        panic!(
            "failed to create {} from {}: {err}",
            crispasr_dll.display(),
            whisper_dll.display()
        )
    });

    copy_runtime_dlls_to_profile_dir(bin_dir);
}

fn windows_import_library_names() -> (&'static str, &'static str) {
    if env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default() == "gnu" {
        ("libwhisper.dll.a", "libcrispasr.dll.a")
    } else {
        ("whisper.lib", "crispasr.lib")
    }
}

fn copy_windows_runtime_dlls(install_dir: &Path, bin_dir: &Path) {
    let mut dlls = Vec::new();
    collect_files_with_extension(&install_dir.join("build"), "dll", &mut dlls);
    collect_files_with_extension(bin_dir, "dll", &mut dlls);
    dlls.sort();
    dlls.dedup();

    for dll in dlls {
        let Some(name) = dll.file_name() else {
            continue;
        };
        let dest = bin_dir.join(name);
        if dll != dest {
            std::fs::copy(&dll, &dest).unwrap_or_else(|err| {
                panic!(
                    "failed to copy Windows runtime DLL {} to {}: {err}",
                    dll.display(),
                    dest.display()
                )
            });
        }
    }
}

fn copy_named_artifact(install_dir: &Path, file_name: &str, dest_dir: &Path) {
    if dest_dir.join(file_name).is_file() {
        return;
    }

    let mut matches = Vec::new();
    collect_files_named(install_dir, file_name, &mut matches);
    matches.sort();

    let Some(src) = matches.into_iter().next() else {
        panic!(
            "CrispASR build did not produce {file_name}. \
             Check the Windows CMake output with `cargo build -vv`."
        );
    };

    std::fs::copy(&src, dest_dir.join(file_name)).unwrap_or_else(|err| {
        panic!(
            "failed to copy {} to {}: {err}",
            src.display(),
            dest_dir.display()
        )
    });
}

fn copy_runtime_dlls_to_profile_dir(bin_dir: &Path) {
    let Some(profile_dir) = cargo_profile_dir() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(bin_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("dll"))
        {
            let Some(name) = path.file_name() else {
                continue;
            };
            let dest = profile_dir.join(name);
            std::fs::copy(&path, &dest).unwrap_or_else(|err| {
                panic!(
                    "failed to copy runtime DLL {} to {}: {err}",
                    path.display(),
                    dest.display()
                )
            });
        }
    }
}

fn cargo_profile_dir() -> Option<PathBuf> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").ok()?);
    let build_dir = out_dir.parent()?.parent()?;
    if build_dir.file_name()? != "build" {
        return None;
    }
    build_dir.parent().map(Path::to_path_buf)
}

fn collect_files_named(root: &Path, file_name: &str, out: &mut Vec<PathBuf>) {
    collect_files(root, &mut |path| {
        if path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(file_name))
        {
            out.push(path.to_path_buf());
        }
    });
}

fn collect_files_with_extension(root: &Path, extension: &str, out: &mut Vec<PathBuf>) {
    collect_files(root, &mut |path| {
        if path
            .extension()
            .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case(extension))
        {
            out.push(path.to_path_buf());
        }
    });
}

fn collect_files(root: &Path, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, visit);
        } else if path.is_file() {
            visit(&path);
        }
    }
}

/// Tell the linker to bake `dir` into the binary's rpath.
/// On Linux/BSD we ALSO emit `--disable-new-dtags` so the resulting
/// DT_RPATH (rather than DT_RUNPATH) applies transitively to the
/// binary's transitive shared-library dependencies. This is belt-and-
/// suspenders insurance — `CMAKE_INSTALL_RPATH=$ORIGIN` should already
/// make libwhisper.so find its own ggml siblings, but `--disable-new-dtags`
/// keeps things working even on systems where the cmake rpath setting
/// gets stripped.
///
/// macOS: @rpath/install_name resolution is naturally transitive via the
/// dyld machinery; rpath alone is sufficient.
/// Windows: rpath is a Unix concept; DLL resolution happens differently
/// (PATH or alongside the .exe). We document this in the README.
fn emit_rpath(dir: &Path) {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target_os.as_str() {
        "linux" | "freebsd" | "netbsd" | "openbsd" | "dragonfly" => {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
            println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
        }
        "macos" | "ios" => {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
        }
        _ => {
            // Windows / WASM / other — no-op.
        }
    }
}

/// Returns the right "$ORIGIN-style" token for `CMAKE_INSTALL_RPATH`.
/// Linux/BSD: `$ORIGIN` — the dynamic linker substitutes it with the
/// directory of the loading binary at runtime.
/// macOS: `@loader_path` — same idea, dyld syntax.
fn install_rpath_token() -> &'static str {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target_os.as_str() {
        "macos" | "ios" => "@loader_path",
        _ => "$ORIGIN",
    }
}

/// Ensure `lib_dir` contains `libcrispasr.{so,dylib}` as an alias
/// for the canonical `libwhisper.*` produced by CrispASR's CMake.
///
/// On Unix we use a relative symlink so the install dir is relocatable.
/// Windows uses [`prepare_windows_artifacts`] instead because MSVC needs an
/// import library at link time and DLLs at runtime.
///
/// Idempotent: the alias is recreated every time so stale aliases do not
/// survive a backend or submodule change.
fn create_crispasr_alias(lib_dir: &Path) {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    let (whisper_name, alias_name) = match target_os.as_str() {
        "macos" | "ios" => ("libwhisper.dylib", "libcrispasr.dylib"),
        _ => ("libwhisper.so", "libcrispasr.so"),
    };

    let whisper_path = lib_dir.join(whisper_name);
    let alias_path = lib_dir.join(alias_name);

    if !whisper_path.exists() {
        panic!(
            "CrispASR build did not produce {}. \
             cmake build may have failed silently — \
             check `cargo build -vv` output.",
            whisper_path.display()
        );
    }

    // Recreate the alias each time. Cheap, and avoids stale-link issues
    // if the user changed something in vendor/CrispASR.
    let _ = std::fs::remove_file(&alias_path);

    let result = match target_os.as_str() {
        "windows" => std::fs::copy(&whisper_path, &alias_path).map(|_| ()),
        _ => {
            #[cfg(unix)]
            {
                // Relative symlink keeps the install dir relocatable.
                std::os::unix::fs::symlink(whisper_name, &alias_path)
            }
            #[cfg(not(unix))]
            {
                std::fs::copy(&whisper_path, &alias_path).map(|_| ())
            }
        }
    };

    if let Err(e) = result {
        panic!(
            "failed to create {} -> {} alias in {}: {e}",
            alias_name,
            whisper_name,
            lib_dir.display()
        );
    }
}
