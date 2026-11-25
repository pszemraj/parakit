# Windows Build Scripts

## Backend Selection

Run from the repository root on native Windows. Pick CPU for a build without accelerator SDKs, Vulkan for GPU inference, or CUDA for NVIDIA CUDA specifically. See [backend requirements](#backend-requirements).

```bat
scripts\windows\build.bat --backend cpu
scripts\windows\build.bat --backend vulkan
scripts\windows\build.bat --backend cuda
```

Without `--backend`, the menu accepts Up/Down, `1`/`2`/`3`, and Enter. Press Enter to select the highlighted CPU option. In PowerShell, substitute `.\scripts\windows\build.ps1` for the batch wrapper.

The script assembles the executable and runtime DLLs in `target\parakit-windows-x86_64-<backend>`. It installs into `%LOCALAPPDATA%\Programs\parakit` and adds that directory to User `PATH` by default. Add `--no-install` to stop after bundling. After installing, open a new terminal and follow [first run](../../docs/running.md#first-run).

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
| `-Profile release\|debug` | Selects the Cargo profile. Each backend gets a separate build directory under `CARGO_TARGET_DIR` or the repository's `target`. |
| `--no-submodules` | Does not run `git submodule update --init --recursive`; fails if `vendor\CrispASR` is not already populated. |
| `--no-install` | Builds the repo-local bundle without installing it. Install-only options have no effect. |
| `--no-user-path` | Installs without adding the install directory to User `PATH`. |
| `--install-dir DIR` | Installs to `DIR` instead of `%LOCALAPPDATA%\Programs\parakit`. |
| `-h`, `--help` | Prints help. |

Choose one backend per bundle.

## BLAS

An OpenBLAS prefix needs `cblas.h` under `include\` or `include\openblas\`, a runtime DLL under `bin\`, and an import library under `lib\` (`.lib` for MSVC or `.dll.a` for GNU). See the [BLAS search order](../../docs/build.md#blas-and-mkl).

Set the prefix explicitly when OpenBLAS is installed in a custom directory.

```bat
scripts\windows\build.bat --backend cpu --blas openblas --openblas-root "C:\path\to\OpenBLAS"
```

The bundle includes the detected OpenBLAS DLL and adjacent runtime DLLs such as OpenMP, gfortran, GCC, quadmath, and winpthreads. A manual `BLAS_INCLUDE_DIRS` and `BLAS_LIBRARIES` override skips automatic DLL bundling.

## Backend Requirements

Install [Rust and CMake](../../docs/build.md#native-dependencies) and Visual Studio 2022 with the "Desktop development with C++" workload. Put CMake on `PATH`. GPU builds also need Ninja, which can come from Visual Studio's CMake tools.

| Backend | Build requirements | Runtime requirements |
| --- | --- | --- |
| CPU | Base toolchain | Any selected [BLAS runtime](#blas) |
| CUDA | NVIDIA CUDA 12.x or 13.x Toolkit with `nvcc`. The script can infer `CUDA_PATH` from `nvcc.exe` on `PATH`. | A compatible NVIDIA display driver and the CUDA runtime and cuBLAS DLLs in the app directory or on `PATH`. The CUDA 13.x toolkit requires a separate display-driver install. |
| Vulkan | LunarG Vulkan SDK with `glslc`. The script checks `VULKAN_SDK`, `C:\VulkanSDK\*`, and `glslc.exe` on `PATH`. | A compatible NVIDIA, AMD, or Intel driver with `vulkan-1.dll` in System32 or on `PATH`. |

Bundle CUDA runtime DLLs when the destination lacks them. This can substantially enlarge the bundle because of `cublasLt64_*.dll`.

```bat
scripts\windows\build.bat --backend cuda --bundle-cuda-dlls
```

For CUDA architecture selection, see [`PARAKIT_CUDA_ARCHS`](../../docs/build.md#crispasr-and-backends). For additional model preparation tools, see [source rebuild](../../docs/dev/README.md#source-rebuild).

GPU builds select Ninja and reuse or activate an amd64 Visual Studio developer environment before Cargo runs. Ninja lets `nvcc` and `CUDA_PATH` select the toolkit without relying on versioned MSBuild CUDA customizations. The script restores the inherited generator afterward.

The bundled MSVC build sets `GGML_CCACHE=OFF`.

For a deep checkout, the Vulkan build falls back to `$env:USERPROFILE\parakit-target\vulkan` to keep shader generator paths short. A custom `CARGO_TARGET_DIR` that exceeds the limit fails before CMake starts.

Use a short base path if you need to set it explicitly.

```powershell
$env:CARGO_TARGET_DIR = "$env:USERPROFILE\parakit-target"
.\scripts\windows\build.ps1 --backend vulkan --no-install
```

If the shader generator reports `linking multiple files is not supported yet`, capture the exact `glslc` command to diagnose the SDK or ggml.

## Build Process

`build.ps1` invokes `cargo build --locked` with the selected profile and accelerator feature. It clears `CRISPASR_LIB_DIR` during the build so the app links the bundled runtime, then restores the inherited value. The app directory contains the [manifest and runtime files](#runtime-manifest), `LICENSE`, and `README.md`.

The script uses the populated CrispASR checkout as-is, including a different local revision. Otherwise it initializes the submodule non-interactively. On a firewalled machine, start with a checkout or archive containing `vendor\CrispASR` and use `--no-submodules` to skip the Git attempt.

## Install An Existing Bundle

Keep the full bundle together when copying it to another machine. Stop a running daemon with `parakit stop`, then run the installer from a checkout containing the scripts.

```powershell
.\scripts\windows\install.ps1 -BundleDir .\target\parakit-windows-x86_64-cpu -InstallDir "$env:LOCALAPPDATA\Programs\parakit"
```

Substitute the copied bundle's path. The destination only needs its [runtime dependencies](#backend-requirements). Add `-NoUserPath` to skip the User `PATH` update.

Install as your desktop user in a per-user directory. The installer refuses system paths and non-empty custom destinations without a `.parakit-install` marker. Reinstalling replaces the entire app directory so old accelerator DLLs are removed.

## Runtime Manifest

Cargo writes `parakit-runtime-manifest.json` and bundled DLLs under `OUT_DIR/bin`. The bundle script locates the output via Cargo's JSON build messages and copies the executable and runtime files listed in `required_files`. The manifest identifies the accelerator and external DLLs expected on the destination.

`install.ps1` checks required files and external DLLs, then runs `parakit --version` in a staging directory and again at the installed path. If the replacement fails, it restores the previous directory or reports the backup path if rollback fails. A `0xC0000135` result reports a missing DLL before any `PATH` update.

For loader errors, check the [backend requirements](#backend-requirements). Once the binary launches, inspect [compute devices](../../docs/running.md#device-selection) and run the [Windows deep insertion check](../../docs/windows-desktop.md#doctor---deep).

If Group Policy blocks User `PATH` writes, the installed app remains usable. Run `parakit.exe` directly from the app directory or add it through your endpoint-management tool.

For TLS interception and proxy setup, see [download troubleshooting](../../docs/troubleshooting.md#downloads-behind-a-corporate-proxy).
