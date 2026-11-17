# Windows Build Scripts

## 30-Second Version

> [!NOTE]
> Commands in this file are meant to run from the repository root (`parakit\`). If your shell is already in `scripts\windows`, run `cd ..\..` first.

- Normal CPU install: run `scripts\windows\build.bat` and press Enter.
- Recommended Windows GPU install: run `scripts\windows\build.bat --backend vulkan`.
- CUDA install: run `scripts\windows\build.bat --backend cuda` only when you specifically need NVIDIA CUDA.
- Build without replacing the installed command: add `--no-install`.
- After install, open a new terminal and follow [first run](../../docs/running.md#first-run).

`build.bat` is a `cmd.exe` wrapper around the PowerShell implementation. From PowerShell, use `.\scripts\windows\build.ps1 ...`; pass `--help` for live options or `-Profile debug` for debug builds.

## Why The Script Exists

`cargo install` installs executables but does not copy external runtime DLLs. The bundle script configures the native toolchain and backend, links CrispASR/ggml statically, packages optional BLAS/CUDA runtime DLLs, and installs an app directory.

The script creates `target\parakit-windows-x86_64-<backend>`, installs it to `%LOCALAPPDATA%\Programs\parakit`, and adds that install directory to the Windows User `PATH`.

Installation uses the User `PATH` and requires neither administrator rights nor Windows Developer Mode. It refuses system locations such as `C:\Windows` and `C:\Program Files\...`. The scripts leave the system `PATH` unchanged and use ordinary directories, without symlinks, junctions, drive mappings, firewall rules, or TCP listeners.

## Backend Selection

If you omit `--backend`, the script opens a keyboard menu. It accepts Up/Down, `1`/`2`/`3`, and Enter. CPU starts highlighted, so pressing Enter selects CPU.

## Build Options

| Option | Behavior |
| --- | --- |
| `--backend cpu\|cuda\|vulkan` | Selects the compute backend; omission uses the [interactive selector](#backend-selection). |
| `--cpu` | Alias for `--backend cpu`. |
| `--cuda` | Alias for `--backend cuda`. |
| `--vulkan` | Alias for `--backend vulkan`. |
| `--blas auto\|off\|openblas\|mkl\|generic` | Sets `PARAKIT_BLAS` for this build. Omit it for normal autodetection. |
| `--openblas-root DIR` | Sets `PARAKIT_OPENBLAS_ROOT` with `--blas auto` or `--blas openblas`; ignored by other modes. See the [OpenBLAS layout](#blas). |
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

Advanced CMake path overrides are documented in the build guide; explicit library paths skip automatic OpenBLAS DLL bundling.

When `build.rs` selects a Windows OpenBLAS install, the bundle includes its OpenBLAS runtime DLL plus adjacent known runtime DLLs such as OpenMP, gfortran, GCC, quadmath, and winpthreads libraries when present.

## Backend Requirements

All backends need Rust, CMake on `PATH`, and Visual Studio 2022 with the "Desktop development with C++" workload. GPU builds also need Ninja.

| Backend | Command | Build-time requirements | Runtime expectation |
| --- | --- | --- | --- |
| CPU | `build.ps1 --backend cpu` | No additional SDK. | CrispASR/ggml are linked statically. Optional BLAS runtimes are bundled. |
| CUDA | `build.ps1 --backend cuda` | NVIDIA CUDA Toolkit with `nvcc`; `CUDA_PATH` may be inferred from `nvcc.exe` on `PATH` | NVIDIA-only. CUDA runtime and cuBLAS DLLs must be found from the installed app directory or `PATH`, unless `--bundle-cuda-dlls` is used. |
| Vulkan | `build.ps1 --backend vulkan` | LunarG Vulkan SDK with `glslc`; `VULKAN_SDK` may be autodetected from `C:\VulkanSDK\*` or inferred from `glslc.exe` on `PATH` | Recommended Windows GPU backend for NVIDIA, AMD, and Intel. Install a compatible GPU driver; its `vulkan-1.dll` must be present through System32 or `PATH`. |

Hosted Q8_0 inference does not need a local quantizer. Windows `fetch --quantize q6_k` and `fetch --from-source` require a compatible external `crispasr-quantize.exe` on `PATH`; source conversion also has Python requirements. See [source rebuild](../../docs/dev/README.md#source-rebuild).

CUDA runtime DLL bundling is opt-in because `cublasLt64_*.dll` is large:

```bat
scripts\windows\build.bat --backend cuda --bundle-cuda-dlls
```

CUDA 12.x and 13.x toolkits are supported by the vendored ggml. CUDA 13.x toolkits do not install a display driver as part of the toolkit; install a compatible NVIDIA display driver separately. The default CUDA architecture behavior is ggml's native build for the GPU present on the machine; the cross-platform `PARAKIT_CUDA_ARCHS` override is in [the build guide](../../docs/build.md#crispasr-and-backends).

GPU builds use `CMAKE_GENERATOR=Ninja`. The script restores an inherited generator afterward, reuses an already-active Visual Studio developer environment or activates an amd64 one before Cargo runs, and verifies `cl.exe`, `link.exe`, and `ninja.exe`. This avoids stale versioned CUDA MSBuild BuildCustomizations selecting a toolkit other than `nvcc` and `CUDA_PATH`.

Windows MSVC bundle builds set `GGML_CCACHE=OFF`. A `ccache` executable on `PATH` is not used by this build path.

Vulkan builds can fail in ggml's shader generator when the checkout plus Cargo target path is too deep. If `CARGO_TARGET_DIR` is unset and the repo-local target path would be too deep, the script automatically uses `$env:USERPROFILE\parakit-target` as the base and builds under its `vulkan` child. It does not shorten paths by mapping temporary drive letters. If you set `CARGO_TARGET_DIR` yourself and it is still too deep, the script fails early before CMake starts.

Override the target directory only when you need a different approved location:

```powershell
$env:CARGO_TARGET_DIR = "$env:USERPROFILE\parakit-target"
.\scripts\windows\build.ps1 --backend vulkan --no-install
```

If path shortening does not fix a Vulkan shader-gen failure, capture the exact `glslc` command. A `linking multiple files is not supported yet` message is a separate SDK/ggml issue, not a path-length problem.

## Build Process

`build.ps1` selects the backend's Cargo target directory, checks Vulkan path depth, prepares submodules, BLAS, and the selected accelerator toolchain, then runs `cargo build` with the requested profile and backend feature. It ignores `CRISPASR_LIB_DIR` during the bundled build and restores the inherited value afterward. The resulting bundle contains the executable, runtime manifest and required files, `LICENSE`, and `README.md`; installation calls `install.ps1` unless `--no-install` is set.

The build script checks whether `vendor\CrispASR` is already populated before touching submodules. If the checkout is present, the script does not contact GitHub or require the repository-pinned revision. If it must initialize the submodule, it runs Git non-interactively so firewalled machines fail instead of opening credential prompts. On a firewalled machine, use a checkout or source archive that already includes `vendor\CrispASR`, or pass `--no-submodules` to fail fast instead of trying to initialize it.

## Install An Existing Bundle

Keep the complete bundle directory together when copying it to another Windows machine. From a checkout containing the installer scripts, install that directory with:

```powershell
.\scripts\windows\install.ps1 -BundleDir .\target\parakit-windows-x86_64-cpu -InstallDir "$env:LOCALAPPDATA\Programs\parakit"
```

Use the copied bundle's actual path and backend; the target needs the [runtime dependencies](#backend-requirements), without the build toolchain. Add `-NoUserPath` to leave User `PATH` unchanged. Stop a running daemon with `parakit stop` before replacing its installation, then open a new terminal for [first run](../../docs/running.md#first-run).

The default install directory is dedicated to parakit. An existing non-empty custom destination must carry `.parakit-install`; otherwise installation is refused. Bundles replace the directory rather than merging files, so stale accelerator DLLs cannot remain.

## Runtime Manifest

Cargo's build script writes `parakit-runtime-manifest.json` and external runtime DLLs under `OUT_DIR/bin`. The bundle script discovers that directory from Cargo's JSON build messages, then copies the executable and every file in `required_files` into the app directory. The installer validates those entries before installing. Native CrispASR/ggml archives are linked into the executable and are not bundle files.

`install.ps1` checks external CUDA/Vulkan runtime DLLs, copies the bundle into a sibling staging directory, and runs `parakit --version` there before replacing an existing installation. It keeps the old directory as a backup until a second loader check at the installed path passes. On failure, it restores the backup; if rollback itself fails, the error reports the backup's location.

The manifest records the selected accelerator and external runtime DLLs required by each [backend](#backend-requirements).

For desktop checks after installation, follow [first run](../../docs/running.md#first-run) and the [Windows deep insertion check](../../docs/windows-desktop.md#doctor---deep).

The version smoke test does not touch the hotkey, microphone, daemon lock, model cache, or clipboard. If Windows reports `0xC0000135`, the installer translates it to a missing-runtime-DLL message before PATH updates.

For loader failures, check the [backend requirements](#backend-requirements). Once the executable starts, run `parakit doctor --verbose` to inspect visible compute devices and `parakit start --device cpu` to check CPU inference. If startup is slow rather than broken, backend warmup is expected; `--verbose` shows timing.

If Group Policy blocks User `PATH` writes, the install still succeeds and prints a warning. Run `%LOCALAPPDATA%\Programs\parakit\parakit.exe` directly, add the directory through your approved endpoint-management path, or rerun with `--no-user-path` when PATH changes are not allowed.

For TLS interception and proxy setup, see [download troubleshooting](../../docs/troubleshooting.md#downloads-behind-a-corporate-proxy).
