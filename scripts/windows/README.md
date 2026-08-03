# Windows Build Scripts

## 30-Second Version

> [!NOTE]
> Commands in this file are meant to run from the repository root (`parakit\`). If your shell is already in `scripts\windows`, run `cd ..\..` first.

- Normal CPU install: run `scripts\windows\build.bat` and press Enter.
- Recommended Windows GPU install: run `scripts\windows\build.bat --backend vulkan`.
- CUDA install: run `scripts\windows\build.bat --backend cuda` only when you specifically need NVIDIA CUDA.
- Build without replacing the installed command: add `--no-install`.
- After install, open a new terminal and run `parakit doctor --deep`.

From PowerShell, use `.\scripts\windows\build.ps1 ...` instead of `build.bat`. For debug builds, use `-Profile debug`.

## Why The Script Exists

Windows builds need an installed runnable directory, not only `parakit.exe`. CrispASR and ggml build shared DLLs, and Windows loads those DLLs from the executable directory or `PATH`.

`cargo build` works for development because `build.rs` copies the generated DLLs next to `parakit.exe` in the active Cargo target directory. That is normally `target\debug` or `target\release`, but `CARGO_TARGET_DIR` can move it.

`cargo install --path .` is different: Cargo installs only `parakit.exe` into Cargo's bin directory. It does not copy the generated CrispASR, ggml, or OpenBLAS DLLs. Use `build.ps1` when you want a normal Windows install.

The script creates `target\parakit-windows-x86_64-<backend>`, installs it to `%LOCALAPPDATA%\Programs\parakit`, and adds that install directory to the Windows User `PATH`.

The scripts do not edit the system `PATH`, create firewall rules, open TCP ports, create symlinks or junctions, map temporary drive letters, require Windows Developer Mode, or require administrator rights.

All backends install into the same default per-user app directory. Installing a different backend replaces the previous bundle and removes its stale DLLs.

The installer is intentionally per-user. It refuses system locations such as `C:\Windows` and `C:\Program Files\...`; those paths require admin rights on normal Windows systems and are the wrong default for a developer or corporate laptop.

## Backend Selection

If you omit `--backend`, the script opens a keyboard menu. It accepts Up/Down, `1`/`2`/`3`, and Enter. CPU starts highlighted, so pressing Enter selects CPU.

`build.bat` is a `cmd.exe` wrapper around the PowerShell implementation. From PowerShell, call the implementation directly:

```powershell
.\scripts\windows\build.ps1
.\scripts\windows\build.ps1 --backend vulkan
```

Run `.\scripts\windows\build.ps1 --help` for the script's live help.

## Build Options

| Option | Behavior |
| --- | --- |
| `--backend cpu\|cuda\|vulkan` | Selects the compute backend. If omitted, the script opens the interactive selector. |
| `--cpu` | Alias for `--backend cpu`. |
| `--cuda` | Alias for `--backend cuda`. Requires the NVIDIA CUDA Toolkit on this machine. |
| `--vulkan` | Alias for `--backend vulkan`. Requires the LunarG Vulkan SDK and `glslc`. |
| `--blas auto\|off\|openblas\|mkl\|generic` | Sets `PARAKIT_BLAS` for this build. Omit it for normal autodetection. |
| `--openblas-root DIR` | Sets `PARAKIT_OPENBLAS_ROOT` for this build. `DIR` must contain a Windows OpenBLAS layout with `include\`, `lib\`, and `bin\`. |
| `--bundle-cuda-dlls` | CUDA only: copies `cudart64_*.dll`, `cublas64_*.dll`, and `cublasLt64_*.dll` into the bundle. |
| `--release` | Builds the Cargo `release` profile and bundles it. This is the default. |
| `--debug` | Legacy shorthand. PowerShell can consume it before the script sees it; use `-Profile debug` for debug builds. |
| `-Profile release\|debug` | Selects the Cargo profile. Bundle builds keep each backend under `<CARGO_TARGET_DIR>\<backend>` so cached CPU, CUDA, and Vulkan artifacts cannot overwrite one another. |
| `--no-submodules` | Does not run `git submodule update --init --recursive`; fails if `vendor\CrispASR` is not already populated. |
| `--no-install` | Builds the repo-local bundle without installing it. Install-only options have no effect. |
| `--no-user-path` | Installs without adding the install directory to User `PATH`. |
| `--install-dir DIR` | Installs to `DIR` instead of `%LOCALAPPDATA%\Programs\parakit`. |
| `-h`, `--help` | Prints help. |

The script rejects contradictory backend choices such as `--cuda --vulkan`. Raw Cargo experiments can still enable multiple features, but the Windows bundle path keeps one backend per installed directory.

## BLAS

CPU BLAS is autodetected by default using the cross-platform search order in [the build guide](../../docs/build.md#blas-and-mkl). On Windows, a usable OpenBLAS installation must include `cblas.h`, a runtime DLL under `bin\`, and a target-compatible import library under `lib\`: `.lib` for MSVC or `.dll.a` for GNU. The build falls back to native/OpenMP CPU kernels when no compatible installation is found.

Use script arguments for normal Windows builds:

```bat
scripts\windows\build.bat --backend cpu --blas auto
scripts\windows\build.bat --backend cpu --blas off
scripts\windows\build.bat --backend cpu --blas openblas --openblas-root C:\path\to\OpenBLAS
```

`--openblas-root` is used with `--blas auto` or `--blas openblas` and ignored by other BLAS modes. Advanced CMake path overrides are documented in the build guide; explicit library paths skip automatic OpenBLAS DLL bundling.

When `build.rs` selects a Windows OpenBLAS install, the bundle includes `openblas.dll` plus adjacent known runtime DLLs such as OpenMP, gfortran, GCC, quadmath, and winpthreads libraries when present. `parakit doctor` reports the requested and selected BLAS modes.

## Backend Requirements

Only one compute backend is supported per bundle.

| Backend | Command | Build-time requirements | Runtime expectation |
| --- | --- | --- | --- |
| CPU | `build.ps1 --backend cpu` | Visual Studio C++ tools, CMake, Rust | Generated CrispASR/ggml DLLs are bundled. BLAS is autodetected unless overridden. |
| CUDA | `build.ps1 --backend cuda` | Visual Studio C++ tools, Ninja, NVIDIA CUDA Toolkit with `nvcc`; `CUDA_PATH` may be inferred from `nvcc.exe` on `PATH` | NVIDIA-only. CUDA runtime and cuBLAS DLLs must be found from the installed app directory or `PATH`, unless `--bundle-cuda-dlls` is used. |
| Vulkan | `build.ps1 --backend vulkan` | Visual Studio C++ tools, Ninja, LunarG Vulkan SDK with `glslc`; `VULKAN_SDK` may be autodetected from `C:\VulkanSDK\*` or inferred from `glslc.exe` on `PATH` | Recommended Windows GPU backend for NVIDIA, AMD, and Intel. `vulkan-1.dll` is provided by the installed GPU driver. |

CUDA runtime DLL bundling is opt-in because `cublasLt64_*.dll` is large:

```bat
scripts\windows\build.bat --backend cuda --bundle-cuda-dlls
```

CUDA 12.x and 13.x toolkits are supported by the vendored ggml. CUDA 13.x toolkits do not install a display driver as part of the toolkit; install a compatible NVIDIA display driver separately. The default CUDA architecture behavior is ggml's native build for the GPU present on the machine; the cross-platform `PARAKIT_CUDA_ARCHS` override is in [the build guide](../../docs/build.md#crispasr-and-backends).

GPU builds use `CMAKE_GENERATOR=Ninja`. The script restores an inherited generator afterward, activates an amd64 Visual Studio C++ environment before Cargo runs, and verifies `cl.exe`, `link.exe`, and `ninja.exe`. This avoids stale versioned CUDA MSBuild BuildCustomizations selecting a toolkit other than `nvcc` and `CUDA_PATH`.

Windows MSVC bundle builds set `GGML_CCACHE=OFF`. A `ccache` executable on `PATH` is not used by this build path.

Vulkan builds can fail in ggml's shader generator when the checkout plus Cargo target path is too deep. If `CARGO_TARGET_DIR` is unset and the repo-local target path would be too deep, the script automatically uses `$env:USERPROFILE\parakit-target` as the base and builds under its `vulkan` child. It does not shorten paths by mapping temporary drive letters. If you set `CARGO_TARGET_DIR` yourself and it is still too deep, the script fails early before CMake starts.

Override the target directory only when you need a different approved location:

```powershell
$env:CARGO_TARGET_DIR = "$env:USERPROFILE\parakit-target"
.\scripts\windows\build.ps1 --backend vulkan --no-install
```

If path shortening does not fix a Vulkan shader-gen failure, capture the exact `glslc` command. A `linking multiple files is not supported yet` message is a separate SDK/ggml issue, not a path-length problem.

## Build Process

`build.ps1` runs this sequence:

1. Ignores `CRISPASR_LIB_DIR` for the bundled build so it produces a fresh runtime manifest and colocated DLL staging directory, then restores the inherited value.
2. Checks `vendor\CrispASR`, accepting any populated checkout and initializing submodules non-interactively only when needed.
3. Configures BLAS and the selected accelerator, including the MSVC amd64 environment and Ninja for GPU builds.
4. Gives the backend its own Cargo target directory, then runs `cargo build` with the requested profile and backend feature.
5. Creates `target\parakit-windows-x86_64-<backend>` and copies the runtime manifest, its required files, `LICENSE`, and `README.md`.
6. Calls `install.ps1` unless `--no-install` is set.

The default `%LOCALAPPDATA%\Programs\parakit` directory is dedicated to parakit and is replaced on each install. Custom install directories are wiped only when marked by `.parakit-install`; an unmarked non-empty custom destination is refused instead of merged, because stale accelerator DLLs can change loader behavior.

The build script checks whether `vendor\CrispASR` is already populated before touching submodules. If the checkout is present, the script does not contact GitHub or require the repository-pinned revision. If it must initialize the submodule, it runs Git non-interactively so firewalled machines fail instead of opening credential prompts. On a firewalled machine, use a checkout or source archive that already includes `vendor\CrispASR`, or pass `--no-submodules` to fail fast instead of trying to initialize it.

## Runtime Manifest

The build writes `parakit-runtime-manifest.json` beside `parakit.exe`. The bundle copies every file in `required_files`, and the installer validates those entries before installing.

`install.ps1` checks external CUDA/Vulkan runtime DLLs, refuses unsafe or non-owned custom destinations, copies the bundle, runs `parakit --version` as a loader smoke test, and updates User `PATH` unless `--no-user-path` is set.

The manifest records the selected accelerator and external runtime DLLs. CUDA external DLLs are hard requirements unless they were bundled. Vulkan's `vulkan-1.dll` is driver-managed and must be present through System32 or `PATH`; install or update the NVIDIA, AMD, or Intel GPU driver before installing the Vulkan bundle.

After installing, open a new terminal and run:

```text
parakit doctor --deep
parakit
```

What `doctor --deep` actually does on Windows is described in [windows-desktop.md#doctor---deep](../../docs/windows-desktop.md#doctor---deep).

The installer runs `parakit --version` after copying files. That checks Windows loader resolution without touching the hotkey, microphone, daemon lock, model cache, or clipboard. If Windows reports `0xC0000135`, the installer translates it to a missing-runtime-DLL message before PATH updates.

The installer updates persistent User `PATH`; it does not broadcast an environment change to already-running applications. Open a new terminal after install.

If Group Policy blocks User `PATH` writes, the install still succeeds and prints a warning. Run `%LOCALAPPDATA%\Programs\parakit\parakit.exe` directly, add the directory through your approved endpoint-management path, or rerun with `--no-user-path` when PATH changes are not allowed.

Model downloads use the platform certificate roots and system proxy settings. This is required on corporate Windows networks where TLS inspection or an HTTP proxy is configured through the OS.

For development-only bundle checks without installing:

```bat
scripts\windows\build.bat --backend cpu --no-install
scripts\windows\build.bat --backend cuda --no-install
scripts\windows\build.bat --backend vulkan --no-install
scripts\windows\build.bat -Profile debug --backend cpu --no-install
```
