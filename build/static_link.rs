//! Translate CMake's resolved consumer dependencies into Rust native linkage.

use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Request the consumer codemodel before CMake configures the native build.
///
/// # Arguments
/// * `out_dir` - Cargo's native build output directory.
///
/// # Returns
/// `Ok(())` after the query is written.
///
/// # Errors
/// Returns filesystem errors while creating the File API query.
pub(crate) fn request(out_dir: &Path) -> Result<(), String> {
    let query = out_dir.join("build/.cmake/api/v1/query");
    fs::create_dir_all(&query).map_err(|err| err.to_string())?;
    fs::write(query.join("codemodel-v2"), "").map_err(|err| err.to_string())
}

/// Emit the resolved static archives and their external runtime dependencies.
///
/// Static archives are bundled into Parakit's Rust library, so its consumers
/// inherit them while the standalone IBus executable does not.
///
/// # Arguments
/// * `out_dir` - CMake installation and build output directory.
/// * `windows` - Whether CMake uses Windows command-line quoting.
///
/// # Returns
/// `Ok(())` after emitting the native linkage directives.
///
/// # Errors
/// Returns unreadable or unsupported CMake consumer-link information.
///
/// # Panics
/// Does not panic for valid filesystem entries and CMake library paths.
pub(crate) fn emit(out_dir: &Path, windows: bool) -> Result<(), String> {
    let build = out_dir.join("build");
    let reply = build.join(".cmake/api/v1/reply");
    let mut indexes = fs::read_dir(&reply)
        .map_err(|err| err.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("index-")
        })
        .collect::<Vec<_>>();
    indexes.sort();
    let index = read_json(indexes.last().ok_or("CMake File API index missing")?)?;
    let model_file = index["reply"]["codemodel-v2"]["jsonFile"]
        .as_str()
        .ok_or("CMake codemodel reply missing")?;
    let model = read_json(&reply.join(model_file))?;
    let configurations = model["configurations"]
        .as_array()
        .ok_or("CMake configurations missing")?;
    let configuration = configurations
        .iter()
        .find(|config| config["name"] == "Release")
        .or_else(|| configurations.iter().find(|config| config["name"] == ""))
        .ok_or("CMake Release configuration missing")?;
    let target = configuration["targets"]
        .as_array()
        .and_then(|targets| {
            targets
                .iter()
                .find(|target| target["name"] == "parakit-native-link")
        })
        .ok_or("CMake Parakit consumer target missing")?;
    let target = read_json(
        &reply.join(
            target["jsonFile"]
                .as_str()
                .ok_or("CMake target file missing")?,
        ),
    )?;
    let fragments = target["link"]["commandFragments"]
        .as_array()
        .ok_or("CMake consumer link fragments missing")?;
    let mut directives = Vec::new();
    if windows {
        // Cargo adds OUT_DIR link-search paths to PATH for run/test. The
        // staged external DLLs live beside the runtime manifest in bin/.
        directives.push(format!(
            "rustc-link-search=native={}",
            out_dir.join("bin").display()
        ));
    }
    for fragment in fragments {
        let role = fragment["role"].as_str().unwrap_or("");
        // CMake's driver optimization/debug flags are not library usage
        // requirements. Rust selects these for its own final link.
        if role == "flags" {
            continue;
        }
        let fragment = fragment["fragment"]
            .as_str()
            .ok_or("CMake link fragment missing")?;
        let words = split_fragment(fragment, windows)?;
        directives.extend(translate(&words, role, &build)?);
    }
    let runtime =
        fs::read_to_string(build.join("parakit-cxx-runtime.txt")).map_err(|err| err.to_string())?;
    for library in runtime.lines().filter(|line| !line.is_empty()) {
        directives.extend(translate(&[library.to_string()], "libraries", &build)?);
    }
    let mut seen = HashSet::new();
    for directive in directives {
        // Rust bundles each archive into one rlib. Repeated archives from
        // CMake's cycle-resolution ordering must not bundle objects twice.
        if seen.insert(directive.clone()) {
            println!("cargo:{directive}");
        }
    }
    Ok(())
}

fn read_json(path: &Path) -> Result<Value, String> {
    let contents = fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
    serde_json::from_slice(&contents).map_err(|err| format!("{}: {err}", path.display()))
}

fn translate(words: &[String], role: &str, build: &Path) -> Result<Vec<String>, String> {
    let mut output = Vec::new();
    let mut words = words.iter();
    while let Some(word) = words.next() {
        if word == "-framework" {
            let name = words.next().ok_or("missing framework name")?;
            output.push(format!("rustc-link-lib=framework={name}"));
        } else if word == "-L" || word == "-F" {
            let path = words.next().ok_or("missing library search directory")?;
            let kind = if word == "-F" { "framework" } else { "native" };
            output.push(format!(
                "rustc-link-search={kind}={}",
                absolute(build, path).display()
            ));
        } else if let Some(path) = word
            .strip_prefix("-L")
            .or_else(|| word.strip_prefix("/LIBPATH:"))
        {
            output.push(format!(
                "rustc-link-search=native={}",
                absolute(build, path).display()
            ));
        } else if let Some(path) = word.strip_prefix("-F") {
            output.push(format!(
                "rustc-link-search=framework={}",
                absolute(build, path).display()
            ));
        } else if let Some(name) = word.strip_prefix("-l") {
            output.push(format!("rustc-link-lib={name}"));
        } else if word == "-pthread" {
            output.push("rustc-link-lib=pthread".into());
        } else if let Some(paths) = word.strip_prefix("-Wl,-rpath,") {
            // Toolkit runtimes may be outside the system loader's defaults.
            // Driver stubs and native build products are never runtime paths.
            for path in paths.split(':') {
                let path = Path::new(path);
                if path.is_absolute() && !path.starts_with(build) && !path.ends_with("stubs") {
                    output.push(format!("rustc-link-arg=-Wl,-rpath,{}", path.display()));
                }
            }
        } else if matches!(word.as_str(), "-Wl,--start-group" | "-Wl,--end-group") {
            // Archive cycles disappear when their objects share one Rust rlib.
        } else if role == "libraries" {
            let path = absolute(build, word);
            let filename = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("invalid library path")?;
            let (name, kind) = if let Some(name) = filename.strip_suffix(".dll.a") {
                (name.strip_prefix("lib").unwrap_or(name), "dylib")
            } else if let Some(name) = filename.strip_suffix(".a") {
                (name.strip_prefix("lib").unwrap_or(name), "static")
            } else if let Some(name) = filename.strip_suffix(".lib") {
                (
                    name,
                    if path.starts_with(build) && path.is_file() {
                        "static"
                    } else {
                        "dylib"
                    },
                )
            } else if let Some((name, _)) = filename.split_once(".so") {
                (name.strip_prefix("lib").unwrap_or(name), "dylib")
            } else if let Some(name) = filename
                .strip_suffix(".dylib")
                .or_else(|| filename.strip_suffix(".tbd"))
            {
                (name.strip_prefix("lib").unwrap_or(name), "dylib")
            } else if !word.contains(['/', '\\']) && !word.starts_with('-') {
                output.push(format!("rustc-link-lib={word}"));
                continue;
            } else {
                return Err(format!("unsupported CMake library fragment: {word}"));
            };
            output.push(format!(
                "rustc-link-search=native={}",
                path.parent().unwrap().display()
            ));
            output.push(format!("rustc-link-lib={kind}={name}"));
        } else {
            return Err(format!("unsupported CMake {role} fragment: {word}"));
        }
    }
    Ok(output)
}

fn absolute(build: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        build.join(path)
    }
}

fn split_fragment(fragment: &str, windows: bool) -> Result<Vec<String>, String> {
    if !windows {
        return shlex::split(fragment).ok_or_else(|| format!("invalid CMake quoting: {fragment}"));
    }
    // CMake emits Windows paths with forward slashes. Backslashes before
    // quotes still follow the native even/odd escaping rule.
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut slashes = 0;
    for character in fragment.chars().chain(std::iter::once(' ')) {
        if character == '\\' {
            slashes += 1;
            continue;
        }
        if character == '"' {
            word.extend(std::iter::repeat_n('\\', slashes / 2));
            if slashes % 2 == 1 {
                word.push('"');
            } else {
                quoted = !quoted;
            }
        } else {
            word.extend(std::iter::repeat_n('\\', slashes));
            if character.is_whitespace() && !quoted {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            } else {
                word.push(character);
            }
        }
        slashes = 0;
    }
    if quoted {
        return Err(format!("invalid CMake quoting: {fragment}"));
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_archive_order_and_frameworks_with_spaces() {
        let words = split_fragment(
            "'src/libcrispasr.a' 'models with spaces/libparakeet.a' -framework Accelerate -lm",
            false,
        )
        .unwrap();
        let result = translate(&words, "libraries", Path::new("/native/build")).unwrap();
        assert_eq!(
            result,
            [
                "rustc-link-search=native=/native/build/src",
                "rustc-link-lib=static=crispasr",
                "rustc-link-search=native=/native/build/models with spaces",
                "rustc-link-lib=static=parakeet",
                "rustc-link-lib=framework=Accelerate",
                "rustc-link-lib=m",
            ]
        );
    }

    #[test]
    fn windows_shell_quoting_retains_library_paths() {
        assert_eq!(
            split_fragment(
                r#"/LIBPATH:"C:\Program Files\CUDA\lib" "src/Release/crispasr.lib" kernel32.lib"#,
                true
            )
            .unwrap(),
            [
                r"/LIBPATH:C:\Program Files\CUDA\lib",
                "src/Release/crispasr.lib",
                "kernel32.lib",
            ]
        );
        assert!(split_fragment("\"unterminated", true).is_err());
    }

    #[test]
    fn keeps_external_runtime_rpath_without_build_or_driver_stubs() {
        let words = vec!["-Wl,-rpath,/opt/cuda/lib:/opt/cuda/stubs:/native/build/lib".to_string()];
        assert_eq!(
            translate(&words, "libraries", Path::new("/native/build")).unwrap(),
            ["rustc-link-arg=-Wl,-rpath,/opt/cuda/lib",]
        );
    }
}
