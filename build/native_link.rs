//! Native linker directives supplied by Parakit instead of crispasr-sys.

use std::env;
use std::path::Path;

/// Link the platform C++ runtime used by the CrispASR native library.
pub(crate) fn emit_cpp_runtime() {
    match env::var("CARGO_CFG_TARGET_OS").unwrap_or_default().as_str() {
        "linux" => println!("cargo:rustc-link-lib=dylib=stdc++"),
        "macos" => println!("cargo:rustc-link-lib=dylib=c++"),
        _ => {}
    }
}

/// Emit the explicit external library directory followed by system prefixes.
///
/// # Arguments
///
/// * `explicit_dir` - Optional `CRISPASR_LIB_DIR` override to search first.
pub(crate) fn emit_external_search_paths(explicit_dir: Option<&Path>) {
    if let Some(dir) = explicit_dir {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }
    for dir in [
        "/opt/homebrew/lib",
        "/usr/local/lib",
        "/usr/lib",
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib/aarch64-linux-gnu",
    ] {
        if Path::new(dir).is_dir() {
            println!("cargo:rustc-link-search=native={dir}");
        }
    }
}
