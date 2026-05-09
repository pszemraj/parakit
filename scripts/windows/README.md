# Windows Build Scripts

Windows builds need a runnable directory, not only `parakit.exe`. CrispASR and ggml build shared DLLs, and Windows loads those DLLs from the executable directory or `PATH`.

`cargo build` works for development because `build.rs` copies the generated DLLs next to `target\debug\parakit.exe` or `target\release\parakit.exe`.

`cargo install --path .` is different: Cargo installs only `parakit.exe` into Cargo's bin directory. It does not copy the generated CrispASR, ggml, or OpenBLAS DLLs. Use one of these scripts when you want `target\parakit-windows-x86_64-cpu`, a runnable Windows app directory.

## Build

```bat
scripts\windows\windows-cpu-build.bat
```

The batch script checks for the Rust MSVC toolchain, Git, CMake, and submodules. Useful options are `--help`, `--skip-doctor`, and `--debug`.

PowerShell equivalent:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/windows/windows-cpu-build.ps1
```

## OpenBLAS

With an active conda environment, `PARAKIT_BLAS=auto` detects OpenBLAS from `%CONDA_PREFIX%\Library`. The bundle includes `openblas.dll` when that backend is selected.

After bundling:

```bat
target\parakit-windows-x86_64-cpu\parakit.exe doctor --deep
target\parakit-windows-x86_64-cpu\parakit.exe
```

The daemon uses `RegisterHotKey` for `Ctrl+Space`, Windows clipboard staging, `SendInput` for the paste chord, a foreground-window focus guard, and a per-user named pipe for `status`, `stop`, `paste-last`, and `test-paste`.
