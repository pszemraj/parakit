//! Windows bundle-target isolation and installer guard regressions.

#[cfg(windows)]
#[allow(dead_code)]
mod common;

#[cfg(windows)]
fn quote_powershell_path(path: &std::path::Path) -> String {
    path.display().to_string().replace('\'', "''")
}

#[cfg(windows)]
fn run_powershell(script: &str) -> std::process::Output {
    std::process::Command::new("powershell")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "RemoteSigned",
            "-Command",
            script,
        ])
        .output()
        .expect("PowerShell script should run")
}

#[cfg(windows)]
fn bundle_target_root(base: &std::path::Path, backend: &str) -> std::path::PathBuf {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let toolchains = repo.join("scripts/windows/toolchains.ps1");
    let script = format!(
        ". '{}'; $repo = '{}'; $Backend = '{}'; $env:CARGO_TARGET_DIR = '{}'; Set-BundleCargoTargetDir; Write-Output (Get-CargoTargetRoot)",
        quote_powershell_path(&toolchains),
        quote_powershell_path(repo),
        backend,
        quote_powershell_path(base),
    );
    let output = run_powershell(&script);
    assert!(
        output.status.success(),
        "target resolution failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("PowerShell output should be UTF-8");
    std::path::PathBuf::from(
        stdout
            .lines()
            .rfind(|line| !line.trim().is_empty())
            .expect("target resolution should print a path")
            .trim(),
    )
}

#[cfg(windows)]
#[test]
fn bundle_targets_are_isolated_by_backend() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let base = repo.join(common::fixture_root("windows-bundle-guard", "target-root"));

    for backend in ["cpu", "cuda", "vulkan"] {
        assert_eq!(bundle_target_root(&base, backend), base.join(backend));
    }
}

#[cfg(windows)]
#[test]
fn gpu_builds_replace_an_inherited_cmake_generator_with_ninja() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let common = repo.join("scripts/windows/common.ps1");
    let toolchains = repo.join("scripts/windows/toolchains.ps1");
    let script = format!(
        ". '{}'; . '{}'; function Ensure-MsvcBuildEnvironment {{}}; function Ensure-NinjaAvailable {{}}; $Backend = 'vulkan'; $env:CMAKE_GENERATOR = 'Visual Studio 17 2022'; Configure-GpuBuildGenerator; if ($env:CMAKE_GENERATOR -ne 'Ninja') {{ throw 'GPU generator was not forced to Ninja' }}",
        quote_powershell_path(&common),
        quote_powershell_path(&toolchains),
    );
    let output = run_powershell(&script);

    assert!(
        output.status.success(),
        "GPU generator setup failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(windows)]
#[test]
fn cuda_path_bin_is_added_before_nvcc_validation() {
    use std::fs;

    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = common::fixture_root("windows-bundle-guard", "cuda-path");
    let bin = root.join("bin");
    fs::create_dir_all(&bin).expect("CUDA bin fixture should be created");
    fs::write(bin.join("nvcc.exe"), b"").expect("nvcc fixture should be written");

    let common = repo.join("scripts/windows/common.ps1");
    let toolchains = repo.join("scripts/windows/toolchains.ps1");
    let script = format!(
        ". '{}'; . '{}'; $env:CUDA_PATH = '{}'; $env:Path = ''; Assert-CudaBuildReady; if (-not ($env:Path.Split(';') -contains '{}')) {{ throw 'CUDA bin was not added to PATH' }}",
        quote_powershell_path(&common),
        quote_powershell_path(&toolchains),
        quote_powershell_path(&root),
        quote_powershell_path(&bin),
    );
    let output = run_powershell(&script);

    assert!(
        output.status.success(),
        "CUDA_PATH setup failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(windows)]
#[test]
fn installer_refuses_unmarked_custom_destination() {
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
            "Refusing to install into existing non-empty custom directory without .parakit-install marker"
        ),
        "unexpected installer output: {output_text}"
    );
    assert!(!install.join(".parakit-install").exists());
    assert_eq!(
        fs::read(install.join("ggml-cuda.dll")).expect("stale dll should remain untouched"),
        b"stale"
    );
}

#[cfg(windows)]
#[test]
fn installer_replaces_unmarked_default_bundle_and_removes_stale_backend_files() {
    use std::fs;
    use std::process::Command;

    let root = common::fixture_root("windows-bundle-guard", "unmarked-default-switch");
    let local_app_data = root.join("local-app-data");
    let bundle = root.join("bundle");
    let install = local_app_data.join("Programs").join("parakit");
    fs::create_dir_all(&bundle).expect("bundle dir should be created");
    fs::create_dir_all(&install).expect("install dir should be created");

    let parakit = std::path::Path::new(env!("CARGO_BIN_EXE_parakit"));
    fs::copy(parakit, bundle.join("parakit.exe")).expect("test binary should enter bundle");
    fs::copy(parakit, install.join("parakit.exe")).expect("test binary should enter install");
    fs::write(
        bundle.join("parakit-runtime-manifest.json"),
        r#"{"required_files":["parakit.exe"],"accelerator":"vulkan"}"#,
    )
    .expect("incoming manifest should be written");
    fs::write(
        install.join("parakit-runtime-manifest.json"),
        r#"{"required_files":["parakit.exe","ggml-cuda.dll"],"accelerator":"cuda"}"#,
    )
    .expect("installed manifest should be written");
    fs::write(install.join("ggml-cuda.dll"), b"stale")
        .expect("stale backend dll should be written");

    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("powershell")
        .current_dir(repo)
        .env("LOCALAPPDATA", &local_app_data)
        .args(["-NoProfile", "-ExecutionPolicy", "RemoteSigned", "-File"])
        .arg(repo.join("scripts/windows/install.ps1"))
        .arg("-BundleDir")
        .arg(&bundle)
        .arg("-InstallDir")
        .arg(&install)
        .arg("-NoUserPath")
        .output()
        .expect("install.ps1 should run");

    let output_text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "default bundle switch should succeed: {output_text}"
    );
    assert!(install.join(".parakit-install").is_file());
    assert!(!install.join("ggml-cuda.dll").exists());
    let manifest = fs::read_to_string(install.join("parakit-runtime-manifest.json"))
        .expect("installed manifest should be readable");
    assert!(manifest.contains(r#""accelerator":"vulkan""#));
}
