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
| Windows | Visual Studio 2022 with the "Desktop development with C++" workload, plus CMake on `PATH`. GPU builds through the Windows scripts also require Ninja. |
| macOS | Apple Silicon with Xcode command line tools plus `cmake autoconf automake libtool pkg-config`. |

CUDA builds need the CUDA Toolkit with `nvcc` on `PATH`.

Vulkan builds on Ubuntu/Debian need:

```bash
sudo apt install libvulkan-dev vulkan-tools glslc spirv-tools spirv-headers mesa-vulkan-drivers
```

`spirv-headers` provides `spirv/unified1/spirv.hpp`; it is not the same package as `spirv-tools`.

## Install

Clone with submodules first:

```text
git submodule update --init --recursive
```

Linux developer install:

```bash
cargo install --path .
```

macOS Apple Silicon install:

```bash
scripts/macos/install.sh --locked
```

The Linux `cargo install --path .` path installs the release binary to Cargo's bin directory, usually `~/.cargo/bin`.

Install behavior:

- Windows `cargo install --path .` copies `parakit.exe` but not the generated CrispASR/ggml DLLs. Use the scripts in [../scripts/windows/README.md](../scripts/windows/README.md) for a normal Windows install.
- macOS uses [../scripts/macos/install.sh](../scripts/macos/install.sh). It copies `parakit` to `<prefix>/bin` and the generated CrispASR/ggml dylibs to `<prefix>/lib/parakit`, then patches the binary rpath to `@executable_path/../lib/parakit`.
- Linux/BSD developer installs currently depend on the generated CrispASR shared libraries under Cargo's build output. Do not delete the repository `target/` tree.
- GitHub auto-generated source archives are unsupported because they do not include the CrispASR submodule. A public release must ship either a source archive with submodules or a binary bundle whose shared libraries are colocated with the executable.

Add `--locked` for CI or reproducibility checks when Cargo must use the exact versions in `Cargo.lock`. Leave it off for normal local installs.

Optional accelerator builds:

```bash
cargo install --path . --features cuda
cargo install --path . --features vulkan
scripts/macos/install.sh --locked        # Apple targets, Metal by default
```

For Windows bundles, build one accelerator backend at a time. A combined CUDA+Vulkan bundle is rejected by the Windows scripts because it would hard-load both accelerator DLL chains while ggml would choose CUDA first anyway.

On macOS, `aarch64-apple-darwin` is the supported target. Build from a native arm64 terminal; an x86_64/Rosetta build may not expose the Metal backend to ggml.

## Windows Bundles

Use the Windows scripts for runnable per-user installs. They copy `parakit.exe` and generated runtime DLLs into one app directory. Backend selection, BLAS arguments, installer, PATH, CUDA, Vulkan, and runtime-manifest behavior are in [../scripts/windows/README.md](../scripts/windows/README.md).

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

Windows OpenBLAS layout and bundling behavior are in [../scripts/windows/README.md#blas](../scripts/windows/README.md#blas).

Ubuntu/Debian OpenBLAS:

```bash
sudo apt install libopenblas-dev
PARAKIT_BLAS=openblas cargo install --path .
```

Explicit `PARAKIT_BLAS` builds print the selected mode, and `parakit doctor` reports the requested and selected modes.

## CrispASR And Backends

The repository vendors [CrispASR](https://github.com/CrispStrobe/CrispASR) as a git submodule. `build.rs` builds it with CMake and installs shared libraries under `target/<profile>/build/parakit-*/out/lib`. Source rebuild requirements are in [dev.md#source-rebuild](dev.md#source-rebuild).

`CRISPASR_LIB_DIR` is for advanced local experiments with an already-built compatible CrispASR tree. The library must match the pinned C ABI, including `crispasr_session_open_with_params`. Bundled builds must also provide compatible ggml libraries with the exported device registry entry points and the pinned device struct prefix used by `parakit doctor` and `--device gpu` preflight.

Feature mapping:

| Cargo feature | CMake option |
| --- | --- |
| `cuda` | `GGML_CUDA=ON` |
| `vulkan` | `GGML_VULKAN=ON` |
| `metal` | `GGML_METAL=ON`, `GGML_METAL_EMBED_LIBRARY=ON` |

CUDA builds also force `GGML_CUDA_NCCL=OFF`.

Metal builds embed the shader source into `libggml-metal.dylib`, so there is no loose `default.metallib` to carry with the binary. The first GPU use still pays normal Metal runtime compilation/warmup cost.

On macOS, `PARAKIT_BLAS=auto` uses Apple Accelerate. `libomp` is optional; ggml's OpenMP path degrades when OpenMP is not available, and Metal handles GPU offload separately.

## Runtime Library Paths

Linux/BSD builds must use transitive `RPATH`, not `RUNPATH`, so `libcrispasr.so` can find sibling `libggml*.so` files.

Verify:

```bash
ldd target/debug/parakit | grep -E "crispasr|ggml"
readelf -d target/debug/parakit | grep -E "RPATH|RUNPATH"
```

The library paths should point into `target/debug/build/parakit-*/out/lib`, and `readelf` should report `RPATH`.

macOS uses `@rpath` and sibling dylib install names. Verify:

```bash
otool -L target/debug/build/parakit-*/out/lib/libggml.dylib
otool -l target/debug/parakit | grep -A2 LC_RPATH
```

For installed macOS builds, verify the binary points at the colocated runtime libraries rather than the repository build tree:

```bash
otool -l "$HOME/.cargo/bin/parakit" | grep -A2 LC_RPATH
ls "$HOME/.cargo/lib/parakit"/libcrispasr.dylib
```

The installed rpath should be `@executable_path/../lib/parakit`.

For Metal builds:

```bash
ls target/debug/build/parakit-*/out/lib/libggml-metal.dylib
otool -s __DATA __ggml_metallib target/debug/build/parakit-*/out/lib/libggml-metal.dylib | head
```
