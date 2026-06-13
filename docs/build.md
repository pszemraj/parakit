# Build

parakit is a Rust 1.87+ binary that links to the vendored [CrispASR](https://github.com/CrispStrobe/CrispASR) submodule. The default build is CPU-only and may use detected CPU BLAS libraries.

Command examples use a POSIX shell unless the surrounding section is Windows-specific. Windows-only commands are shown as `bat` or `powershell`.

## Native Dependencies

Cargo handles Rust packages. System packages are still needed for audio, desktop input, X11/XTest insertion, CMake, and optional accelerator SDKs.

| OS | Packages |
| --- | --- |
| Ubuntu 24.04 | `cmake build-essential libasound2-dev libudev-dev libxtst-dev libxi-dev libx11-dev libxkbcommon-dev libevdev-dev libxdo-dev libgomp1 pkg-config autoconf libtool` |
| Fedora | `cmake gcc-c++ alsa-lib-devel libudev-devel libXtst-devel libXi-devel libX11-devel libxkbcommon-devel libevdev-devel xdotool-devel pkgconf autoconf libtool` |
| Arch | `cmake base-devel alsa-lib libxtst libxi libx11 libxkbcommon libevdev xdotool pkgconf autoconf libtool` |
| Windows | Visual Studio 2022 with the "Desktop development with C++" workload, plus CMake on `PATH`. GPU bundle scripts also require Ninja. |
| macOS | Xcode command line tools plus `cmake autoconf automake libtool pkg-config`. |

CUDA builds need the CUDA Toolkit with `nvcc` on `PATH`.

Vulkan builds on Ubuntu/Debian need:

```bash
sudo apt install libvulkan-dev vulkan-tools glslc spirv-tools spirv-headers mesa-vulkan-drivers
```

`spirv-headers` provides `spirv/unified1/spirv.hpp`; it is not the same package as `spirv-tools`.

## Install

```text
git submodule update --init --recursive
cargo install --path .
```

`cargo install --path .` installs the release binary to Cargo's bin directory, usually `~/.cargo/bin` on Unix-like systems and `%USERPROFILE%\.cargo\bin` on Windows.

Install behavior:

- Windows `cargo install --path .` copies `parakit.exe` but not the generated CrispASR/ggml DLLs. Use the scripts in [../scripts/windows/README.md](../scripts/windows/README.md) for a normal Windows install.
- Unix-like developer installs depend on the generated CrispASR shared libraries under Cargo's build output. Do not delete the repository `target/` tree.
- GitHub auto-generated source archives are unsupported because they do not include the CrispASR submodule. A public release must ship either a source archive with submodules or a binary bundle whose shared libraries are colocated with the executable.

Add `--locked` for CI or reproducibility checks when Cargo must use the exact versions in `Cargo.lock`. Leave it off for normal local installs.

Optional accelerator builds:

```bash
cargo install --path . --features cuda
cargo install --path . --features vulkan
cargo install --path . --features metal  # Apple targets only
```

For Windows bundles, build one accelerator flavor at a time. A combined CUDA+Vulkan bundle is rejected by the Windows scripts because it would hard-load both accelerator DLL chains while ggml would choose CUDA first anyway.

## Windows Bundles

For a per-user Windows CPU install:

```bat
scripts\windows\windows-cpu-build.bat
```

Windows GPU bundle entry points:

```bat
scripts\windows\windows-cuda-build.bat
scripts\windows\windows-vulkan-build.bat
```

The PowerShell equivalent from PowerShell is:

```powershell
Set-ExecutionPolicy -Scope Process Bypass
.\scripts\windows\windows-cpu-build.ps1
.\scripts\windows\windows-cpu-build.ps1 --cuda
.\scripts\windows\windows-cpu-build.ps1 --vulkan
```

Options, install location, PATH behavior, and OpenBLAS bundling are described in [../scripts/windows/README.md](../scripts/windows/README.md).

The Windows scripts do not require Developer Mode. They install by copying files into a per-user directory, not by creating symlinks.

### Windows CUDA

CUDA builds require the NVIDIA CUDA Toolkit on the build machine, with `nvcc` on `PATH` and `CUDA_PATH` pointing at the toolkit root. CUDA 12.x and 13.x toolkits are supported by the vendored ggml. CUDA 13.x toolkits do not install a display driver as part of the toolkit; install a compatible NVIDIA display driver separately.

The Windows CUDA bundle script defaults `CMAKE_GENERATOR` to `Ninja` when no generator is set, then activates an amd64 Visual Studio C++ environment before running Cargo. This avoids a Windows CUDA trap in the Visual Studio generator: CUDA toolkit selection comes from versioned MSBuild BuildCustomizations such as `CUDA 13.1.props`/`.targets`, not from generic `PATH` or `CUDA_PATH`. A stale `CUDA 13.2.targets` file can make MSBuild pick 13.2 even when `nvcc` and `CUDA_PATH` point at 13.1.

The default CUDA architecture behavior is ggml's native build for the GPU present on the machine. Override it when needed:

```powershell
$env:PARAKIT_CUDA_ARCHS = "89-real"
.\scripts\windows\windows-cpu-build.ps1 --cuda
```

`PARAKIT_CUDA_ARCHS` is passed directly to CMake as `CMAKE_CUDA_ARCHITECTURES`; values such as `native`, `89-real`, or semicolon-separated architecture lists are accepted by CMake/CUDA.

On Windows, ggml links the CUDA runtime statically but cuBLAS dynamically. A normal CUDA bundle expects `cublas64_*.dll` and `cublasLt64_*.dll` from `%CUDA_PATH%\bin` or `PATH` at install/run time. For a relocatable bundle:

```bat
scripts\windows\windows-cuda-build.bat --bundle-cuda-dlls
```

That option copies the cuBLAS DLLs into the bundle. The files are large, so this is opt-in. parakit also sets `GGML_CUDA_NCCL=OFF`; the daemon has no multi-GPU collective workload.

If you explicitly set `CMAKE_GENERATOR` to a Visual Studio generator, the matching CUDA Visual Studio integration must be installed and the matching version-specific environment variable such as `CUDA_PATH_V13_1` must resolve. Advanced users can force the VS toolset with `CMAKE_GENERATOR_TOOLSET=cuda=<toolkit-path>`, but Ninja is the supported default for the bundle scripts.

### Windows Vulkan

Vulkan builds require the LunarG Vulkan SDK at build time for headers, `vulkan-1.lib`, and `glslc`. Set `VULKAN_SDK`, or let the Windows script autodetect the newest `C:\VulkanSDK\*` install.

```bat
scripts\windows\windows-vulkan-build.bat
```

The Windows Vulkan bundle script also defaults to Ninja and activates the amd64 Visual Studio C++ environment. ggml-vulkan generates many shader-permutation source files with long names, so deep checkouts can exceed Windows path limits inside Cargo's CMake output tree. If shader generation fails with path or PDB errors, use a short target root:

```powershell
$env:CARGO_TARGET_DIR = "C:\t"
.\scripts\windows\windows-cpu-build.ps1 --vulkan
```

Cloning to a short path such as `C:\src\parakit` and enabling Windows long paths are the other useful mitigations. If the failure is instead a `glslc` message about linking multiple files, capture the failing command line; that is a separate ggml-vulkan shader generation issue and should be handled by using Ninja under a clean build tree or by updating the vendored ggml/CrispASR pin.

The Vulkan bundle has no CUDA-style runtime toolkit dependency. It imports `vulkan-1.dll`, the Khronos loader installed by GPU driver packages into System32. The installer warns if the loader is missing, but does not bundle it.

## CPU Builds

The bundled CMake path enables ggml native CPU code, OpenMP, and CPU repacking. On Linux with GCC or Clang this usually means `-march=native` for the local machine.

Inspect the compiled flags:

```bash
parakit doctor
parakit --verbose doctor
```

Benchmark different thread counts with the daemon-free WAV quality target described in [quality.md#wav-quality-target](quality.md#wav-quality-target):

```bash
cargo run --release --no-default-features --features bundled --example transcribe-file -- \
  --audio path/to/sample.wav --threads 8 --repeat 3
```

## BLAS And MKL

The build defaults to `PARAKIT_BLAS=auto`. If no supported BLAS is detected, parakit uses native ggml CPU kernels. BLAS/MKL can help some matrix paths but adds system-library dependencies.

```bash
PARAKIT_BLAS=openblas cargo install --path .
PARAKIT_BLAS=mkl cargo install --path .
PARAKIT_BLAS=generic cargo install --path .
```

Supported values:

| Value | Behavior |
| --- | --- |
| unset, `auto` | Apple Accelerate on macOS; otherwise MKL if `mkl-sdl.pc` is visible; otherwise Windows OpenBLAS from `PARAKIT_OPENBLAS_ROOT` or `CONDA_PREFIX\Library`; otherwise OpenBLAS if `openblas.pc` or `openblas64.pc` is visible; otherwise off. |
| `off`, `false`, `0` | Native/OpenMP CPU kernels without BLAS. |
| `openblas` | `GGML_BLAS=ON`, `GGML_BLAS_VENDOR=OpenBLAS`. |
| `mkl` | CrispASR `COHERE_MKL=ON`, ggml `Intel10_64lp`. |
| `generic` | `GGML_BLAS=ON`, `GGML_BLAS_VENDOR=Generic`. |
| `accelerate` | Apple Accelerate. Apple targets only. |

On Windows, OpenBLAS detection requires `cblas.h`, a runtime DLL under `bin\`, and an import library compatible with the active Rust target environment: `.lib` for MSVC or `.dll.a` for GNU. Set `PARAKIT_OPENBLAS_ROOT` to the prefix containing `include\`, `lib\`, and `bin\`, or activate a conda environment whose `%CONDA_PREFIX%\Library` has that layout. Set both `BLAS_INCLUDE_DIRS` and `BLAS_LIBRARIES` for explicit CMake paths; together they take precedence over autodetection and skip OpenBLAS DLL bundling.

Ubuntu/Debian OpenBLAS:

```bash
sudo apt install libopenblas-dev
PARAKIT_BLAS=openblas cargo install --path .
```

Explicit `PARAKIT_BLAS` builds print the selected mode, and `parakit doctor` reports the requested and selected modes.

## CrispASR And Backends

The repository vendors [CrispASR](https://github.com/CrispStrobe/CrispASR) as a git submodule. `build.rs` builds it with CMake and installs shared libraries under `target/<profile>/build/parakit-*/out/lib`. Source rebuild requirements are in [dev.md#source-rebuild](dev.md#source-rebuild).

Feature mapping:

| Cargo feature | CMake option |
| --- | --- |
| `cuda` | `GGML_CUDA=ON` |
| `vulkan` | `GGML_VULKAN=ON` |
| `metal` | `GGML_METAL=ON` |

CUDA builds also force `GGML_CUDA_NCCL=OFF`.

## Runtime Library Paths

Linux/BSD builds must use transitive `RPATH`, not `RUNPATH`, so `libcrispasr.so` can find sibling `libggml*.so` files.

Verify:

```bash
ldd target/debug/parakit | grep -E "crispasr|ggml"
readelf -d target/debug/parakit | grep -E "RPATH|RUNPATH"
```

The library paths should point into `target/debug/build/parakit-*/out/lib`, and `readelf` should report `RPATH`.
