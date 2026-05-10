# Windows Build Scripts

Windows builds need an installed runnable directory, not only `parakit.exe`. CrispASR and ggml build shared DLLs, and Windows loads those DLLs from the executable directory or `PATH`.

`cargo build` works for development because `build.rs` copies the generated DLLs next to `target\debug\parakit.exe` or `target\release\parakit.exe`.

`cargo install --path .` is different: Cargo installs only `parakit.exe` into Cargo's bin directory. It does not copy the generated CrispASR, ggml, or OpenBLAS DLLs. Use one of these scripts when you want a normal Windows install.

By default, the scripts build `target\parakit-windows-x86_64-cpu`, install it to `%LOCALAPPDATA%\Programs\parakit`, and add that install directory to the Windows User `PATH`. They do not edit the system `PATH` and do not require administrator rights. Open a new terminal after install before running `parakit` by name.

The installer is intentionally per-user. It refuses system locations such as `C:\Windows` and `C:\Program Files\...`; those paths require admin rights on normal Windows systems and are the wrong default for a developer or corporate laptop.

## Build

```bat
scripts\windows\windows-cpu-build.bat
```

The batch file is a wrapper around the PowerShell implementation. Both entry points accept the same options. Useful options are `--help`, `--debug`, `--no-install`, `--no-user-path`, and `--install-dir`.

PowerShell equivalent from PowerShell:

```powershell
Set-ExecutionPolicy -Scope Process Bypass
.\scripts\windows\windows-cpu-build.ps1
```

## OpenBLAS

With an active conda environment, `PARAKIT_BLAS=auto` detects OpenBLAS from `%CONDA_PREFIX%\Library`. The bundle includes `openblas.dll` when that backend is selected.

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

The daemon uses `RegisterHotKey` for `Ctrl+Space`, Windows clipboard staging, `SendInput` for the paste chord, a foreground-window focus guard, and a per-user named pipe for `status`, `stop`, `paste-last`, `copy-last`, and `test-paste`.
