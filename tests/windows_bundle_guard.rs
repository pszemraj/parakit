//! Windows installer ownership and stale-file guard regressions.

#[cfg(windows)]
#[allow(dead_code)]
mod common;

#[cfg(windows)]
#[test]
fn installer_refuses_non_empty_unmarked_destination() {
    use std::fs;
    use std::process::Command;

    let root = common::fixture_root("windows-bundle-guard", "unmarked-non-empty");
    let bundle = root.join("bundle");
    let install = root.join("install");
    fs::create_dir_all(&bundle).expect("bundle dir should be created");
    fs::create_dir_all(&install).expect("install dir should be created");

    fs::write(
        bundle.join("parakit-runtime-manifest.json"),
        r#"{"required_files":["parakit.exe"],"accelerator":"cpu"}"#,
    )
    .expect("manifest should be written");
    fs::write(bundle.join("parakit.exe"), b"").expect("dummy exe should be written");
    fs::write(install.join("ggml-cuda.dll"), b"stale").expect("stale dll should be written");

    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("powershell")
        .current_dir(repo)
        .args(["-NoProfile", "-ExecutionPolicy", "RemoteSigned", "-File"])
        .arg(repo.join("scripts/windows/install.ps1"))
        .arg("-BundleDir")
        .arg(&bundle)
        .arg("-InstallDir")
        .arg(&install)
        .arg("-NoUserPath")
        .output()
        .expect("install.ps1 should run");

    assert!(!output.status.success());
    let output_text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output_text.contains(
            "Refusing to install into existing non-empty directory without .parakit-install marker"
        ),
        "unexpected installer output: {output_text}"
    );
    assert!(!install.join(".parakit-install").exists());
    assert_eq!(
        fs::read(install.join("ggml-cuda.dll")).expect("stale dll should remain untouched"),
        b"stale"
    );
}
