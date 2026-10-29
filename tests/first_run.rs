//! Fresh-profile acquisition and configuration behavior through the real CLI.

use std::path::Path;
use std::process::Command;

#[allow(dead_code)]
mod common;

fn fresh_parakit(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_parakit"));
    command
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("PARAKIT_MODELS_DIR", root.join("models"))
        .env_remove("PARAKIT_CONFIG_PATH")
        .env_remove("HF_TOKEN")
        .env("HF_ENDPOINT", "http://127.0.0.1:1");
    #[cfg(target_os = "windows")]
    {
        std::fs::write(root.join("config.toml"), "").unwrap();
        command.env("PARAKIT_CONFIG_PATH", root.join("config.toml"));
    }
    command
}

#[test]
fn cached_fetch_confirms_success_and_prints_an_exact_start_command_offline() {
    let root = common::fixture_root_with_files(
        "first-run",
        "cached-fetch",
        &["models/parakeet-tdt-0.6b-v3-Q8_0.gguf"],
    );
    let ultra = parakit::fetch::preset_cache_path(
        &root.join("models"),
        parakit::model::ModelPreset::ParakeetUltra,
    );
    common::touch(&ultra);
    for _ in 0..2 {
        let output = fresh_parakit(&root).arg("fetch").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        assert_eq!(text.lines().count(), 2, "{text}");
        assert!(text.contains("model ready: parakeet-ultra-q8_0.gguf"));
        assert!(text.contains("parakit start --model-preset parakeet-ultra --quantize q8_0"));
        assert!(!text.contains("downloading") && !text.contains("config.toml"));
    }
    let quiet = fresh_parakit(&root)
        .args(["fetch", "--quiet"])
        .output()
        .unwrap();
    assert!(quiet.status.success() && quiet.stdout.is_empty() && quiet.stderr.is_empty());
    let rebuilt = fresh_parakit(&root)
        .args(["fetch", "--from-source"])
        .output()
        .unwrap();
    assert!(
        rebuilt.status.success() && rebuilt.stderr.is_empty(),
        "{rebuilt:?}"
    );
    assert!(String::from_utf8_lossy(&rebuilt.stdout)
        .contains("parakit start --model-preset parakeet-ultra --quantize q8_0"));
    let legacy = fresh_parakit(&root)
        .args(["fetch", "--model-preset", "parakeet-v3"])
        .output()
        .unwrap();
    assert!(
        legacy.status.success() && legacy.stderr.is_empty(),
        "{legacy:?}"
    );
    assert!(String::from_utf8_lossy(&legacy.stdout)
        .contains("parakit start --model-preset parakeet-v3 --quantize q8_0"));
    let cache = fresh_parakit(&root)
        .args(["cache", "list"])
        .output()
        .unwrap();
    assert!(
        cache.status.success() && cache.stderr.is_empty(),
        "{cache:?}"
    );
    let cache_text = String::from_utf8_lossy(&cache.stdout);
    assert!(
        cache_text.contains("parakeet-ultra-q8_0.gguf default:"),
        "{cache_text}"
    );
    assert!(
        !cache_text.contains("parakeet-tdt-0.6b-v3-Q8_0.gguf default:"),
        "{cache_text}"
    );
}

#[cfg(not(target_os = "windows"))]
#[test]
fn missing_default_config_uses_defaults_until_the_user_creates_one() {
    let root = common::fixture_root_with_files("first-run", "absent-config", &[]);
    for args in [&["config", "show"][..], &["cache", "list"]] {
        let output = fresh_parakit(&root).args(args).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        assert!(!root.join("config/parakit/config.toml").exists());
        let text = String::from_utf8(output.stdout).unwrap();
        if args[0] == "config" {
            assert!(text.contains("exists: false") && text.contains("default: hosted Q8_0"));
        } else {
            assert!(text.contains("models: none"));
        }
    }
    let init = fresh_parakit(&root)
        .args(["config", "init"])
        .output()
        .unwrap();
    assert!(init.status.success(), "{init:?}");
    let shown = fresh_parakit(&root)
        .args(["config", "show"])
        .output()
        .unwrap();
    assert!(shown.status.success(), "{shown:?}");
    assert!(String::from_utf8_lossy(&shown.stdout).contains("exists: true"));
}
