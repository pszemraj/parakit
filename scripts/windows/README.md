# Windows Build Scripts

Windows builds need an installed runnable directory, not only `parakit.exe`. CrispASR and ggml build shared DLLs, and Windows loads those DLLs from the executable directory or `PATH`.

`cargo build` works for development because `build.rs` copies the generated DLLs next to `target\debug\parakit.exe` or `target\release\parakit.exe`.

`cargo install --path .` is different: Cargo installs only `parakit.exe` into Cargo's bin directory. It does not copy the generated CrispASR, ggml, or OpenBLAS DLLs. Use one of these scripts when you want a normal Windows install.

By default, the scripts build `target\parakit-windows-x86_64-cpu`, install it to `%LOCALAPPDATA%\Programs\parakit`, and add that install directory to the Windows User `PATH`. They do not edit the system `PATH` and do not require administrator rights.

The installer is intentionally per-user. It refuses system locations such as `C:\Windows` and `C:\Program Files\...`; those paths require admin rights on normal Windows systems and are the wrong default for a developer or corporate laptop.

## Build

```bat
scripts\windows\windows-cpu-build.bat
```

The batch file is a wrapper around the PowerShell implementation. Both entry points accept the same options; run either one with `--help` for the supported flags.

PowerShell equivalent from PowerShell:

```powershell
Set-ExecutionPolicy -Scope Process Bypass
.\scripts\windows\windows-cpu-build.ps1
```

## Runtime Manifest

The build writes `parakit-runtime-manifest.json` beside `parakit.exe`. The bundle copies every file in `required_files`, and the installer validates those entries before installing. With an active conda environment, `PARAKIT_BLAS=auto` detects OpenBLAS from `%CONDA_PREFIX%\Library`; selected OpenBLAS bundles include `openblas.dll` plus adjacent known runtime DLLs such as OpenMP, gfortran, GCC, quadmath, and winpthreads libraries when present.

After installing, open a new terminal and run:

```text
parakit doctor --deep
parakit
```

The installer updates persistent User `PATH`; it cannot rewrite already-running parent shells.

For development-only bundle checks without installing:

```bat
scripts\windows\windows-cpu-build.bat --no-install
```
