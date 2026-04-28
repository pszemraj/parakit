//! Builds (or locates) the CrispASR C library and tells the linker where it is.
//!
//! Default behavior (with the `bundled` feature on, which is the default):
//!   - Vendored source is at `vendor/CrispASR/`. If empty, attempt to init the
//!     git submodule. If that also fails, error out with clear instructions.
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
//!   - `--no-default-features`              : the user takes responsibility.
//!   - `CRISPASR_LIB_DIR=/path/to/libdir`   : link-search path override.
//!   - `CRISPASR_SRC_DIR=/path/to/source`   : use this checkout instead of the vendored source.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=CRISPASR_LIB_DIR");
    println!("cargo:rerun-if-env-changed=CRISPASR_SRC_DIR");
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
        // Skip tests. Build examples because CrispASR's quantizer lives there.
        .define("WHISPER_BUILD_TESTS", "OFF")
        // CrispASR's GGUF requantizer lives under examples/. We build the
        // examples tree so `crispasr-quantize` is available to `parakit fetch`.
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
    } else {
        panic!(
            "expected install dir to contain lib/ or lib64/, got {}",
            install_dir.display()
        );
    };

    create_crispasr_alias(&final_lib_dir);

    println!("cargo:rustc-link-search=native={}", final_lib_dir.display());

    // Windows DLLs land in bin/, not lib/. Add it for completeness.
    let bin_dir = install_dir.join("bin");
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
            "cargo:warning=crispasr-quantize was not installed at {}; `parakit fetch` will look on PATH",
            quantize_bin.display()
        );
    }

    // 6. Bake the lib path into the binary's rpath so we don't need
    //    LD_LIBRARY_PATH / DYLD_FALLBACK_LIBRARY_PATH at runtime.
    //    No-op on Windows — see README for the DLL placement guidance.
    emit_rpath(&final_lib_dir);

    // 7. Re-export the install path. Useful for `cargo run` on macOS
    //    (where DYLD_LIBRARY_PATH is hostile) and for downstream tooling
    //    that wants to copy the dylib into a release artifact.
    println!(
        "cargo:rustc-env=CRISPASR_INSTALL_DIR={}",
        install_dir.display()
    );
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

fn exe_name(name: &str) -> String {
    if env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
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
           4. Use a system-installed library and skip the build entirely:\n\
                cargo build --no-default-features\n\
              (or set CRISPASR_LIB_DIR=/path/to/libdir to override the search path)\n"
    );
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

/// Ensure `lib_dir` contains `libcrispasr.{so,dylib,dll}` as an alias
/// for the canonical `libwhisper.*` produced by CrispASR's CMake.
///
/// On Unix we use a relative symlink so the install dir is relocatable.
/// On Windows we copy the DLL because creating symlinks requires
/// administrator privileges by default.
///
/// Idempotent — if the alias already exists and points at libwhisper,
/// we leave it alone. If it exists but points somewhere wrong, we recreate.
fn create_crispasr_alias(lib_dir: &Path) {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    let (whisper_name, alias_name) = match target_os.as_str() {
        "macos" | "ios" => ("libwhisper.dylib", "libcrispasr.dylib"),
        "windows" => ("whisper.dll", "crispasr.dll"),
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
