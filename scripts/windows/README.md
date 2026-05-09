# Windows Build Scripts

Windows builds need an installed runnable directory, not only `parakit.exe`. CrispASR and ggml build shared DLLs, and Windows loads those DLLs from the executable directory or `PATH`.

`cargo build` works for development because `build.rs` copies the generated DLLs next to `target\debug\parakit.exe` or `target\release\parakit.exe`.

`cargo install --path .` is different: Cargo installs only `parakit.exe` into Cargo's bin directory. It does not copy the generated CrispASR, ggml, or OpenBLAS DLLs. Use one of these scripts when you want a normal Windows install.

By default, the scripts build `target\parakit-windows-x86_64-cpu`, install it to `%LOCALAPPDATA%\Programs\parakit`, add that install directory to the Windows User `PATH`, and also prepend it to `PATH` for the current terminal. They do not edit the system `PATH` and do not require administrator rights.

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

After installing, open a new terminal or use the same terminal that ran the script:

```bat
parakit doctor --deep
parakit
```

If you launch the PowerShell script with `powershell -File ...` from another shell, use a new terminal afterward. The script updates the persistent User `PATH`, but a child process cannot rewrite its parent shell's current environment.

For development-only bundle checks without installing:

```bat
scripts\windows\windows-cpu-build.bat --no-install
```

The daemon uses `RegisterHotKey` for `Ctrl+Space`, Windows clipboard staging, `SendInput` for the paste chord, a foreground-window focus guard, and a per-user named pipe for `status`, `stop`, `paste-last`, and `test-paste`.
