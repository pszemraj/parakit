# Windows Build Scripts

Windows builds need an installed runnable directory, not only `parakit.exe`. CrispASR and ggml build shared DLLs, and Windows loads those DLLs from the executable directory or `PATH`.

`cargo build` works for development because `build.rs` copies the generated DLLs next to `target\debug\parakit.exe` or `target\release\parakit.exe`.

`cargo install --path .` is different: Cargo installs only `parakit.exe` into Cargo's bin directory. It does not copy the generated CrispASR, ggml, or OpenBLAS DLLs. Use one of these scripts when you want a normal Windows install.

By default, the scripts build `target\parakit-windows-x86_64-cpu`, install it to `%LOCALAPPDATA%\Programs\parakit`, and add that install directory to the Windows User `PATH`. They do not edit the system `PATH` and do not require administrator rights.

The installer is intentionally per-user. It refuses system locations such as `C:\Windows` and `C:\Program Files\...`; those paths require admin rights on normal Windows systems and are the wrong default for a developer or corporate laptop.

The scripts do not create symlinks and do not require Windows Developer Mode.

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

The build script checks whether `vendor\CrispASR` is already populated before touching submodules. If the submodule is present and pinned, the script does not contact GitHub. On a firewalled machine, use a checkout or source archive that already includes `vendor\CrispASR`, or pass `--no-submodules` to fail fast instead of trying to initialize it.

## Runtime Manifest

The build writes `parakit-runtime-manifest.json` beside `parakit.exe`. The bundle copies every file in `required_files`, and the installer validates those entries before installing. With an active conda environment, `PARAKIT_BLAS=auto` detects OpenBLAS from `%CONDA_PREFIX%\Library`; selected OpenBLAS bundles include `openblas.dll` plus adjacent known runtime DLLs such as OpenMP, gfortran, GCC, quadmath, and winpthreads libraries when present.

After installing, open a new terminal and run:

```text
parakit doctor --deep
parakit
```

The installer updates persistent User `PATH`; it cannot rewrite already-running parent shells.

If Group Policy blocks User `PATH` writes, the install still succeeds and prints a warning. Run `%LOCALAPPDATA%\Programs\parakit\parakit.exe` directly, add the directory through your approved endpoint-management path, or rerun with `--no-user-path` when PATH changes are not allowed.

Model downloads use the platform certificate roots and system proxy settings. This is required on corporate Windows networks where TLS inspection or an HTTP proxy is configured through the OS.

For development-only bundle checks without installing:

```bat
scripts\windows\windows-cpu-build.bat --no-install
```
