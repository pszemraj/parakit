# Windows Build Scripts

Windows builds need an installed runnable directory, not only `parakit.exe`. CrispASR and ggml build shared DLLs, and Windows loads those DLLs from the executable directory or `PATH`.

`cargo build` works for development because `build.rs` copies the generated DLLs next to `target\debug\parakit.exe` or `target\release\parakit.exe`.

`cargo install --path .` is different: Cargo installs only `parakit.exe` into Cargo's bin directory. It does not copy the generated CrispASR, ggml, or OpenBLAS DLLs. Use one of these scripts when you want a normal Windows install.

By default, the CPU script builds `target\parakit-windows-x86_64-cpu`, installs it to `%LOCALAPPDATA%\Programs\parakit`, and adds that install directory to the Windows User `PATH`. GPU scripts build sibling bundle directories with `-cuda` or `-vulkan` suffixes. They do not edit the system `PATH`, create firewall rules, open TCP ports, or require administrator rights.

Running a different flavor script installs that flavor into the same default per-user app directory. The installer replaces the previous marked parakit install, so switching from CUDA to Vulkan or back is just another script run.

The installer is intentionally per-user. It refuses system locations such as `C:\Windows` and `C:\Program Files\...`; those paths require admin rights on normal Windows systems and are the wrong default for a developer or corporate laptop.

The scripts do not create symlinks, junctions, or temporary drive-letter mappings, and do not require Windows Developer Mode.

## Build

```bat
scripts\windows\windows-cpu-build.bat
scripts\windows\windows-cuda-build.bat
scripts\windows\windows-vulkan-build.bat
```

The batch files are wrappers around the PowerShell implementation. All entry points accept the same options; run any of them with `--help` for the supported flags.

For Windows GPU installs, start with `windows-vulkan-build.bat` unless you
specifically need CUDA. Vulkan is vendor-agnostic, ships as a self-contained
parakit bundle, and uses the GPU driver's `vulkan-1.dll` at runtime. CUDA is
NVIDIA-only and either needs matching CUDA Toolkit runtime DLLs available
through `%CUDA_PATH%\bin`, `%CUDA_PATH%\bin\x64`, or `PATH`, or a larger bundle
built with `--bundle-cuda-dlls`.

PowerShell equivalent from PowerShell:

```powershell
.\scripts\windows\windows-cpu-build.ps1
.\scripts\windows\windows-cpu-build.ps1 --cuda
.\scripts\windows\windows-cpu-build.ps1 --vulkan
```

The build script checks whether `vendor\CrispASR` is already populated before touching submodules. If the submodule is present and pinned, the script does not contact GitHub. If it must initialize the submodule, it runs Git non-interactively so firewalled machines fail instead of opening credential prompts. On a firewalled machine, use a checkout or source archive that already includes `vendor\CrispASR`, or pass `--no-submodules` to fail fast instead of trying to initialize it.

## Bundle Flavors

Only one accelerator flavor is supported per bundle.

| Flavor | Command | Build-time requirements | Runtime expectation |
| --- | --- | --- | --- |
| CPU | `windows-cpu-build.bat` | Visual Studio C++ tools, CMake, Rust | Generated CrispASR/ggml DLLs are bundled. |
| CUDA | `windows-cuda-build.bat` | Visual Studio C++ tools, Ninja, NVIDIA CUDA Toolkit with `nvcc`; `CUDA_PATH` may be inferred from `nvcc.exe` on `PATH` | NVIDIA-only. CUDA runtime and cuBLAS DLLs must be found through `%CUDA_PATH%\bin`, `%CUDA_PATH%\bin\x64`, or `PATH`, unless `--bundle-cuda-dlls` is used. |
| Vulkan | `windows-vulkan-build.bat` | Visual Studio C++ tools, Ninja, LunarG Vulkan SDK with `glslc`; `VULKAN_SDK` may be autodetected from `C:\VulkanSDK\*` or inferred from `glslc.exe` on `PATH` | Recommended Windows GPU flavor for NVIDIA, AMD, and Intel. `vulkan-1.dll` is provided by the installed GPU driver. |

CUDA runtime DLL bundling is opt-in because `cublasLt64_*.dll` is large:

```bat
scripts\windows\windows-cuda-build.bat --bundle-cuda-dlls
```

The scripts reject `--cuda --vulkan`; raw Cargo experiments can still enable multiple features, but the Windows bundle path keeps one accelerator per installed directory.

CUDA 12.x and 13.x toolkits are supported by the vendored ggml. CUDA 13.x toolkits do not install a display driver as part of the toolkit; install a compatible NVIDIA display driver separately. The default CUDA architecture behavior is ggml's native build for the GPU present on the machine. Override it when needed:

```powershell
$env:PARAKIT_CUDA_ARCHS = "89-real"
.\scripts\windows\windows-cpu-build.ps1 --cuda
```

`PARAKIT_CUDA_ARCHS` is passed directly to CMake as `CMAKE_CUDA_ARCHITECTURES`; values such as `native`, `89-real`, or semicolon-separated architecture lists are accepted by CMake/CUDA.

GPU builds default `CMAKE_GENERATOR` to `Ninja` when the variable is unset. The script activates an amd64 Visual Studio C++ environment before Cargo runs, then verifies `cl.exe`, `link.exe`, and `ninja.exe`. This is intentional for CUDA: Visual Studio generators select CUDA from versioned MSBuild BuildCustomizations, so stale files such as `CUDA 13.2.targets` can override a shell where `nvcc` and `CUDA_PATH` point at 13.1.

If you explicitly set a Visual Studio generator for CUDA, keep the matching CUDA Visual Studio integration installed and ensure the matching variable such as `CUDA_PATH_V13_1` resolves. The advanced override is `CMAKE_GENERATOR_TOOLSET=cuda=<toolkit-path>`, but Ninja is the normal bundle path.

When `ccache` is on `PATH`, ggml's fallback CMake build can auto-enable it. The script keeps that supported by setting `CCACHE_DIR`, `CCACHE_TEMPDIR`, and `CCACHE_BASEDIR` to repo-local paths under `target\tmp` unless you already set them. For troubleshooting, set `CCACHE_DISABLE=1` in the build shell to bypass caching without uninstalling ccache.

Vulkan builds can fail in ggml's shader generator when the checkout plus Cargo target path is too deep. If `CARGO_TARGET_DIR` is unset and the repo-local target path would be too deep, the script automatically uses `$env:USERPROFILE\parakit-target`. It does not shorten paths by mapping temporary drive letters. If you set `CARGO_TARGET_DIR` yourself and it is still too deep, the script fails early before CMake starts.

Override the target directory only when you need a different approved location:

```powershell
$env:CARGO_TARGET_DIR = "$env:USERPROFILE\parakit-target"
.\scripts\windows\windows-vulkan-build.bat --no-install
```

If path shortening does not fix a Vulkan shader-gen failure, capture the exact `glslc` command. A `linking multiple files is not supported yet` message is a separate SDK/ggml issue, not a path-length problem.

## Runtime Manifest

The build writes `parakit-runtime-manifest.json` beside `parakit.exe`. The bundle copies every file in `required_files`, and the installer validates those entries before installing. OpenBLAS selection and manual path overrides are in [../../docs/build.md#blas-and-mkl](../../docs/build.md#blas-and-mkl). When `build.rs` selects a Windows OpenBLAS install, the bundle includes `openblas.dll` plus adjacent known runtime DLLs such as OpenMP, gfortran, GCC, quadmath, and winpthreads libraries when present.

The manifest also records the selected accelerator and external runtime DLLs. CUDA external DLLs are hard requirements unless they were bundled. Vulkan's `vulkan-1.dll` is driver-managed and must be present through System32 or `PATH`; install or update the NVIDIA, AMD, or Intel GPU driver before installing the Vulkan bundle.

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
scripts\windows\windows-cpu-build.bat --no-install
scripts\windows\windows-cuda-build.bat --no-install
scripts\windows\windows-vulkan-build.bat --no-install
```
