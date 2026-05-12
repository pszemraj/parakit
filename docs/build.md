# Build

parakit is a Rust 1.87+ binary that links to the vendored [CrispASR](https://github.com/CrispStrobe/CrispASR) submodule. The default build is CPU-only and local-machine optimized.

Command examples use a POSIX shell unless the surrounding section is Windows-specific. Windows-only commands are shown as `bat` or `powershell`.

## Native Dependencies

Cargo handles Rust packages. System packages are still needed for audio, desktop input, X11/XTest insertion, CMake, and optional accelerator SDKs.

| OS | Packages |
| --- | --- |
| Ubuntu 24.04 | `cmake build-essential libasound2-dev libudev-dev libxtst-dev libxi-dev libx11-dev libxkbcommon-dev libevdev-dev libgomp1 pkg-config autoconf libtool` |
| Fedora | `cmake gcc-c++ alsa-lib-devel libudev-devel libXtst-devel libXi-devel libX11-devel libxkbcommon-devel libevdev-devel pkgconf autoconf libtool` |
| Arch | `cmake base-devel alsa-lib libxtst libxi libx11 libxkbcommon libevdev pkgconf autoconf libtool` |
| Windows | Visual Studio 2022 with the "Desktop development with C++" workload, plus CMake on `PATH`. |
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
PARAKIT_BLAS=auto cargo install --path .
cargo install --path . --features cuda
cargo install --path . --features vulkan
cargo install --path . --features metal  # Apple targets only
```

## Windows Bundles

For a per-user Windows CPU install:

```bat
scripts\windows\windows-cpu-build.bat
```

The PowerShell equivalent from PowerShell is:

```powershell
Set-ExecutionPolicy -Scope Process Bypass
.\scripts\windows\windows-cpu-build.ps1
```

Options, install location, PATH behavior, and OpenBLAS bundling are described in [../scripts/windows/README.md](../scripts/windows/README.md).

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

Native ggml kernels are the default. BLAS/MKL can help some matrix paths but adds system-library dependencies.

```bash
PARAKIT_BLAS=auto cargo install --path .
PARAKIT_BLAS=openblas cargo install --path .
PARAKIT_BLAS=mkl cargo install --path .
PARAKIT_BLAS=generic cargo install --path .
```

Supported values:

| Value | Behavior |
| --- | --- |
| unset, `off` | Native/OpenMP CPU kernels without BLAS. |
| `auto` | Apple Accelerate on macOS; otherwise MKL if `mkl-sdl.pc` is visible; otherwise Windows conda OpenBLAS from `CONDA_PREFIX\Library`; otherwise OpenBLAS if `openblas.pc` or `openblas64.pc` is visible; otherwise off. |
| `openblas` | `GGML_BLAS=ON`, `GGML_BLAS_VENDOR=OpenBLAS`. |
| `mkl` | CrispASR `COHERE_MKL=ON`, ggml `Intel10_64lp`. |
| `generic` | `GGML_BLAS=ON`, `GGML_BLAS_VENDOR=Generic`. |
| `accelerate` | Apple Accelerate. Apple targets only. |

Ubuntu/Debian OpenBLAS:

```bash
sudo apt install libopenblas-dev
PARAKIT_BLAS=openblas cargo install --path .
```

Explicit BLAS builds print the selected mode, and `parakit doctor` reports it.

## CrispASR And Backends

The repository vendors [CrispASR](https://github.com/CrispStrobe/CrispASR) as a git submodule. `build.rs` builds it with CMake and installs shared libraries under `target/<profile>/build/parakit-*/out/lib`. Source rebuild requirements are in [dev.md#source-rebuild](dev.md#source-rebuild).

Feature mapping:

| Cargo feature | CMake option |
| --- | --- |
| `cuda` | `GGML_CUDA=ON` |
| `vulkan` | `GGML_VULKAN=ON` |
| `metal` | `GGML_METAL=ON` |

## Runtime Library Paths

Linux/BSD builds must use transitive `RPATH`, not `RUNPATH`, so `libwhisper.so` can find sibling `libggml*.so` files.

Verify:

```bash
ldd target/debug/parakit | grep -E "whisper|ggml"
readelf -d target/debug/parakit | grep -E "RPATH|RUNPATH"
```

The library paths should point into `target/debug/build/parakit-*/out/lib`, and `readelf` should report `RPATH`.
