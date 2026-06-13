# Windows Build Scripts

Windows builds need an installed runnable directory, not only `parakit.exe`. CrispASR and ggml build shared DLLs, and Windows loads those DLLs from the executable directory or `PATH`.

`cargo build` works for development because `build.rs` copies the generated DLLs next to `target\debug\parakit.exe` or `target\release\parakit.exe`.

`cargo install --path .` is different: Cargo installs only `parakit.exe` into Cargo's bin directory. It does not copy the generated CrispASR, ggml, or OpenBLAS DLLs. Use one of these scripts when you want a normal Windows install.

By default, the CPU script builds `target\parakit-windows-x86_64-cpu`, installs it to `%LOCALAPPDATA%\Programs\parakit`, and adds that install directory to the Windows User `PATH`. GPU scripts build sibling bundle directories with `-cuda` or `-vulkan` suffixes. They do not edit the system `PATH` and do not require administrator rights.

The installer is intentionally per-user. It refuses system locations such as `C:\Windows` and `C:\Program Files\...`; those paths require admin rights on normal Windows systems and are the wrong default for a developer or corporate laptop.

The scripts do not create symlinks and do not require Windows Developer Mode.

## Build

```bat
scripts\windows\windows-cpu-build.bat
scripts\windows\windows-cuda-build.bat
scripts\windows\windows-vulkan-build.bat
```

The batch files are wrappers around the PowerShell implementation. All entry points accept the same options; run any of them with `--help` for the supported flags.

PowerShell equivalent from PowerShell:

```powershell
Set-ExecutionPolicy -Scope Process Bypass
.\scripts\windows\windows-cpu-build.ps1
.\scripts\windows\windows-cpu-build.ps1 --cuda
.\scripts\windows\windows-cpu-build.ps1 --vulkan
```

The build script checks whether `vendor\CrispASR` is already populated before touching submodules. If the submodule is present and pinned, the script does not contact GitHub. On a firewalled machine, use a checkout or source archive that already includes `vendor\CrispASR`, or pass `--no-submodules` to fail fast instead of trying to initialize it.

## Bundle Flavors

Only one accelerator flavor is supported per bundle.

| Flavor | Command | Build-time requirements | Runtime expectation |
| --- | --- | --- | --- |
| CPU | `windows-cpu-build.bat` | Visual Studio C++ tools, CMake, Rust | Generated CrispASR/ggml DLLs are bundled. |
| CUDA | `windows-cuda-build.bat` | Visual Studio C++ tools, Ninja, NVIDIA CUDA Toolkit with `nvcc` and `CUDA_PATH` | cuBLAS DLLs must be found through `%CUDA_PATH%\bin` or `PATH`, unless `--bundle-cuda-dlls` is used. |
| Vulkan | `windows-vulkan-build.bat` | Visual Studio C++ tools, Ninja, LunarG Vulkan SDK with `glslc`; `VULKAN_SDK` may be autodetected from `C:\VulkanSDK\*` | `vulkan-1.dll` is provided by the installed GPU driver. |

CUDA cuBLAS bundling is opt-in because `cublasLt64_*.dll` is large:

```bat
scripts\windows\windows-cuda-build.bat --bundle-cuda-dlls
```

The scripts reject `--cuda --vulkan`; raw Cargo experiments can still enable multiple features, but the Windows bundle path keeps one accelerator per installed directory.

GPU builds default `CMAKE_GENERATOR` to `Ninja` when the variable is unset. The script activates an amd64 Visual Studio C++ environment before Cargo runs, then verifies `cl.exe`, `link.exe`, and `ninja.exe`. This is intentional for CUDA: Visual Studio generators select CUDA from versioned MSBuild BuildCustomizations, so stale files such as `CUDA 13.2.targets` can override a shell where `nvcc` and `CUDA_PATH` point at 13.1.

If you explicitly set a Visual Studio generator for CUDA, keep the matching CUDA Visual Studio integration installed and ensure the matching variable such as `CUDA_PATH_V13_1` resolves. The advanced override is `CMAKE_GENERATOR_TOOLSET=cuda=<toolkit-path>`, but Ninja is the normal bundle path.

Vulkan builds can fail in ggml's shader generator when the checkout plus Cargo target path is too deep. If the script warns about long paths, use a short target root:

```powershell
$env:CARGO_TARGET_DIR = "C:\t"
.\scripts\windows\windows-vulkan-build.bat --no-install
```

If path shortening does not fix a Vulkan shader-gen failure, capture the exact `glslc` command. A `linking multiple files is not supported yet` message is a separate SDK/ggml issue, not a path-length problem.

## Runtime Manifest

The build writes `parakit-runtime-manifest.json` beside `parakit.exe`. The bundle copies every file in `required_files`, and the installer validates those entries before installing. OpenBLAS selection and manual path overrides are in [../../docs/build.md#blas-and-mkl](../../docs/build.md#blas-and-mkl). When `build.rs` selects a Windows OpenBLAS install, the bundle includes `openblas.dll` plus adjacent known runtime DLLs such as OpenMP, gfortran, GCC, quadmath, and winpthreads libraries when present.

The manifest also records the selected accelerator and external runtime DLLs. CUDA external DLLs are hard requirements unless they were bundled. Vulkan's `vulkan-1.dll` is driver-managed; the installer warns if it cannot find the loader.

After installing, open a new terminal and run:

```text
parakit doctor --deep
parakit
```

The installer runs `parakit --quiet doctor` after copying files. If Windows reports `0xC0000135`, the installer translates it to a missing-runtime-DLL message before PATH updates.

The installer updates persistent User `PATH`; it cannot rewrite already-running parent shells.

If Group Policy blocks User `PATH` writes, the install still succeeds and prints a warning. Run `%LOCALAPPDATA%\Programs\parakit\parakit.exe` directly, add the directory through your approved endpoint-management path, or rerun with `--no-user-path` when PATH changes are not allowed.

Model downloads use the platform certificate roots and system proxy settings. This is required on corporate Windows networks where TLS inspection or an HTTP proxy is configured through the OS.

For development-only bundle checks without installing:

```bat
scripts\windows\windows-cpu-build.bat --no-install
scripts\windows\windows-cuda-build.bat --no-install
scripts\windows\windows-vulkan-build.bat --no-install
```
