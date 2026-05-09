# Windows Build Scripts

Windows builds need an installed runnable directory, not only `parakit.exe`. CrispASR and ggml build shared DLLs, and Windows loads those DLLs from the executable directory or `PATH`.

`cargo build` works for development because `build.rs` copies the generated DLLs next to `target\debug\parakit.exe` or `target\release\parakit.exe`.

`cargo install --path .` is different: Cargo installs only `parakit.exe` into Cargo's bin directory. It does not copy the generated CrispASR, ggml, or OpenBLAS DLLs. Use one of these scripts when you want a normal Windows install.

By default, the scripts build `target\parakit-windows-x86_64-cpu`, install it to `%LOCALAPPDATA%\Programs\parakit`, and add that install directory to the Windows User `PATH`. They do not edit the system `PATH` and do not require administrator rights.

## Build

```bat
scripts\windows\windows-cpu-build.bat
```

The batch script checks for the Rust MSVC toolchain, Git, CMake, and submodules. Useful options are `--help`, `--skip-doctor`, and `--debug`.

PowerShell equivalent from PowerShell:

```powershell
Set-ExecutionPolicy -Scope Process Bypass
.\scripts\windows\windows-cpu-build.ps1
```

## OpenBLAS

With an active conda environment, `PARAKIT_BLAS=auto` detects OpenBLAS from `%CONDA_PREFIX%\Library`. The bundle includes `openblas.dll` when that backend is selected.

After installing from Command Prompt with the batch script, the same Command Prompt can run:

```text
parakit doctor --deep
parakit
```

After installing from PowerShell, or after launching the batch script from PowerShell, open a new terminal before running `parakit` by name. The installer updates persistent User `PATH`; it cannot rewrite an already-running parent shell's environment.

For development-only bundle checks without installing:

```bat
scripts\windows\windows-cpu-build.bat --no-install
```

The daemon uses `RegisterHotKey` for `Ctrl+Space`, Windows clipboard staging, `SendInput` for the paste chord, a foreground-window focus guard, and a per-user named pipe for `status`, `stop`, `paste-last`, and `test-paste`.
