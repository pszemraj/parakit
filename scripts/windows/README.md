# Windows Build Scripts

## 30-Second Version

> [!NOTE]
> Commands in this file are meant to run from the repository root (`parakit\`). If your shell is already in `scripts\windows`, run `cd ..\..` first.

- Normal CPU install: run `scripts\windows\build.bat` and press Enter.
- Recommended Windows GPU install: run `scripts\windows\build.bat --backend vulkan`.
- CUDA install: run `scripts\windows\build.bat --backend cuda` only when you specifically need NVIDIA CUDA.
- Build without replacing the installed command: add `--no-install`.
- Intentionally replace an installed CPU/CUDA/Vulkan backend with another: add `--force`.
- After install, open a new terminal and run `parakit doctor --deep`.

From PowerShell, use `.\scripts\windows\build.ps1 ...` instead of `build.bat`. For debug builds, use `-Profile debug`.

## Why The Script Exists

Windows builds need an installed runnable directory, not only `parakit.exe`. CrispASR and ggml build shared DLLs, and Windows loads those DLLs from the executable directory or `PATH`.

`cargo build` works for development because `build.rs` copies the generated DLLs next to `target\debug\parakit.exe` or `target\release\parakit.exe`.

`cargo install --path .` is different: Cargo installs only `parakit.exe` into Cargo's bin directory. It does not copy the generated CrispASR, ggml, or OpenBLAS DLLs. Use `build.ps1` when you want a normal Windows install.

By default, the build script asks which compute backend to build. CPU is highlighted first, so pressing Enter selects CPU. The script then builds `target\parakit-windows-x86_64-<backend>`, installs it to `%LOCALAPPDATA%\Programs\parakit`, and adds that install directory to the Windows User `PATH`.

The scripts do not edit the system `PATH`, create firewall rules, open TCP ports, create symlinks or junctions, map temporary drive letters, require Windows Developer Mode, or require administrator rights.

All backends install into the same default per-user app directory. The installer refuses to replace an installed `cpu`, `cuda`, or `vulkan` bundle with a different backend unless you pass `--allow-backend-switch` or `--force`. This prevents a later CPU install from silently replacing a GPU install.

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
| `--release` | Builds `target\release` and bundles it. This is the default. |
| `--debug` | Legacy shorthand. PowerShell can consume it before the script sees it; use `-Profile debug` for debug builds. |
| `-Profile release\|debug` | Selects the Cargo profile. Use `-Profile debug` when you need `target\debug`. |
| `--no-submodules` | Does not run `git submodule update --init --recursive`; fails if `vendor\CrispASR` is not already populated. |
| `--no-install` | Builds the repo-local bundle without installing it. |
| `--no-user-path` | Installs without adding the install directory to User `PATH`. |
| `--allow-backend-switch`, `--force` | Allows replacing an installed `cpu`, `cuda`, or `vulkan` bundle with a different backend. This does not skip directory, runtime DLL, or loader checks. |
| `--install-dir DIR` | Installs to `DIR` instead of `%LOCALAPPDATA%\Programs\parakit`. |
| `-h`, `--help` | Prints help. |

The script rejects contradictory backend choices such as `--cuda --vulkan`. Raw Cargo experiments can still enable multiple features, but the Windows bundle path keeps one backend per installed directory.

## BLAS

CPU BLAS is autodetected by default. On Windows, autodetection first uses `PARAKIT_OPENBLAS_ROOT` when set, then an active conda environment's `%CONDA_PREFIX%\Library`, then falls back to non-BLAS native/OpenMP CPU kernels if no bundleable OpenBLAS install is found.

Use script arguments for normal Windows builds:

```bat
scripts\windows\build.bat --backend cpu --blas auto
scripts\windows\build.bat --backend cpu --blas off
scripts\windows\build.bat --backend cpu --blas openblas --openblas-root C:\path\to\OpenBLAS
```

`--openblas-root` is valid with `--blas auto` or `--blas openblas`. OpenBLAS detection requires `cblas.h`, a runtime DLL under `bin\`, and an import library compatible with the active Rust target environment: `.lib` for MSVC or `.dll.a` for GNU.

Advanced CMake overrides still work through the environment. Set both `BLAS_INCLUDE_DIRS` and `BLAS_LIBRARIES` when you need explicit CMake paths; together they take precedence over autodetection and skip OpenBLAS DLL bundling.

When `build.rs` selects a Windows OpenBLAS install, the bundle includes `openblas.dll` plus adjacent known runtime DLLs such as OpenMP, gfortran, GCC, quadmath, and winpthreads libraries when present. `parakit doctor` reports the requested and selected BLAS modes.

## Backend Requirements

Only one compute backend is supported per bundle.

| Backend | Command | Build-time requirements | Runtime expectation |
| --- | --- | --- | --- |
| CPU | `build.ps1 --backend cpu` | Visual Studio C++ tools, CMake, Rust | Generated CrispASR/ggml DLLs are bundled. BLAS is autodetected unless overridden. |
| CUDA | `build.ps1 --backend cuda` | Visual Studio C++ tools, Ninja, NVIDIA CUDA Toolkit with `nvcc`; `CUDA_PATH` may be inferred from `nvcc.exe` on `PATH` | NVIDIA-only. CUDA runtime and cuBLAS DLLs must be found from the installed app directory or `PATH`, unless `--bundle-cuda-dlls` is used. |
| Vulkan | `build.ps1 --backend vulkan` | Visual Studio C++ tools, Ninja, LunarG Vulkan SDK with `glslc`; `VULKAN_SDK` may be autodetected from `C:\VulkanSDK\*` or inferred from `glslc.exe` on `PATH` | Recommended Windows GPU backend for NVIDIA, AMD, and Intel. `vulkan-1.dll` is provided by the installed GPU driver. |

For Windows GPU installs, start with `--backend vulkan` unless you specifically need CUDA. Vulkan is vendor-agnostic, ships as a self-contained parakit bundle, and uses the GPU driver's `vulkan-1.dll` at runtime. CUDA is NVIDIA-only and either needs matching CUDA Toolkit runtime DLLs available from the installed app directory or `PATH`, or a larger bundle built with `--bundle-cuda-dlls`.

CUDA runtime DLL bundling is opt-in because `cublasLt64_*.dll` is large:

```bat
scripts\windows\build.bat --backend cuda --bundle-cuda-dlls
```

CUDA 12.x and 13.x toolkits are supported by the vendored ggml. CUDA 13.x toolkits do not install a display driver as part of the toolkit; install a compatible NVIDIA display driver separately. The default CUDA architecture behavior is ggml's native build for the GPU present on the machine. Override it when needed:

```powershell
$env:PARAKIT_CUDA_ARCHS = "89-real"
.\scripts\windows\build.ps1 --backend cuda
```

`PARAKIT_CUDA_ARCHS` is passed directly to CMake as `CMAKE_CUDA_ARCHITECTURES`; values such as `native`, `89-real`, or semicolon-separated architecture lists are accepted by CMake/CUDA.

GPU builds default `CMAKE_GENERATOR` to `Ninja` when the variable is unset. The script activates an amd64 Visual Studio C++ environment before Cargo runs, then verifies `cl.exe`, `link.exe`, and `ninja.exe`. This is intentional for CUDA: Visual Studio generators select CUDA from versioned MSBuild BuildCustomizations, so stale versioned targets can override the toolkit selected by `nvcc` and `CUDA_PATH`.

If you explicitly set a Visual Studio generator for CUDA, keep the matching CUDA Visual Studio integration installed and ensure the matching versioned variable, such as `CUDA_PATH_V13_2`, resolves. The advanced override is `CMAKE_GENERATOR_TOOLSET=cuda=<toolkit-path>`, but Ninja is the normal bundle path.

When `ccache` is on `PATH`, ggml's fallback CMake build can auto-enable it. The script keeps that supported by setting `CCACHE_DIR`, `CCACHE_TEMPDIR`, and `CCACHE_BASEDIR` to repo-local paths under `target\tmp` unless you already set them. For troubleshooting, set `CCACHE_DISABLE=1` in the build shell to bypass caching without uninstalling ccache.

Vulkan builds can fail in ggml's shader generator when the checkout plus Cargo target path is too deep. If `CARGO_TARGET_DIR` is unset and the repo-local target path would be too deep, the script automatically uses `$env:USERPROFILE\parakit-target`. It does not shorten paths by mapping temporary drive letters. If you set `CARGO_TARGET_DIR` yourself and it is still too deep, the script fails early before CMake starts.

Override the target directory only when you need a different approved location:

```powershell
$env:CARGO_TARGET_DIR = "$env:USERPROFILE\parakit-target"
.\scripts\windows\build.ps1 --backend vulkan --no-install
```

If path shortening does not fix a Vulkan shader-gen failure, capture the exact `glslc` command. A `linking multiple files is not supported yet` message is a separate SDK/ggml issue, not a path-length problem.

## Build Process

`build.ps1` runs this sequence:

1. Parses options. If no backend was specified, opens the backend selector with CPU highlighted.
2. Refuses `CRISPASR_LIB_DIR`, because bundle builds must produce a fresh runtime manifest and colocated DLL staging directory.
3. Validates native Windows, Rust, CMake, and backend-specific toolchains.
4. Checks `vendor\CrispASR`; initializes submodules only when needed and only non-interactively.
5. Applies `--blas` and `--openblas-root` by setting `PARAKIT_BLAS` and `PARAKIT_OPENBLAS_ROOT` for the current process.
6. For GPU builds, configures the CMake generator, activates the MSVC amd64 developer environment when needed, and validates CUDA or Vulkan SDK inputs.
7. Runs `cargo build --locked`, plus `--release` for release builds and `--features cuda` or `--features vulkan` for GPU builds.
8. Creates `target\parakit-windows-x86_64-<backend>`.
9. Copies `parakit-runtime-manifest.json`, every manifest `required_files` entry, `LICENSE`, and `README.md` into the bundle.
10. Unless `--no-install` is set, calls `install.ps1`.

`install.ps1` validates the bundle manifest, checks external CUDA/Vulkan runtime DLLs before replacing an install, refuses unsafe install directories, enforces the backend-switch guard, copies the bundle, runs `parakit --version` as a loader smoke test, and then updates User `PATH` unless `--no-user-path` is set. Direct `install.ps1` calls can use `-AllowBackendSwitch` or `-Force` for intentional backend replacement.

The installer only wipes directories it owns, marked by `.parakit-install`. It refuses a non-empty unmarked destination instead of merging files into it, because stale accelerator DLLs from a foreign directory can change loader behavior.

When intentionally replacing an installed backend, pass `--allow-backend-switch` or `--force` on the build command. Without it, the installer fails before deleting the existing install.

The build script checks whether `vendor\CrispASR` is already populated before touching submodules. If the submodule is present and pinned, the script does not contact GitHub. If it must initialize the submodule, it runs Git non-interactively so firewalled machines fail instead of opening credential prompts. On a firewalled machine, use a checkout or source archive that already includes `vendor\CrispASR`, or pass `--no-submodules` to fail fast instead of trying to initialize it.

## Runtime Manifest

The build writes `parakit-runtime-manifest.json` beside `parakit.exe`. The bundle copies every file in `required_files`, and the installer validates those entries before installing.

The manifest records the selected accelerator and external runtime DLLs. CUDA external DLLs are hard requirements unless they were bundled. Vulkan's `vulkan-1.dll` is driver-managed and must be present through System32 or `PATH`; install or update the NVIDIA, AMD, or Intel GPU driver before installing the Vulkan bundle.

After installing, open a new terminal and run:

```text
parakit doctor --deep
parakit
```

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
