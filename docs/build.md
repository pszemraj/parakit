# Build

Parakit requires Rust 1.87 or newer and builds for CPU by default. Get Rust from [rustup](https://rustup.rs).

## Native Dependencies

Bundled builds require CMake 3.19 or newer and the packages for your platform.

| OS | Packages |
| --- | --- |
| Ubuntu 24.04 | `cmake build-essential libasound2-dev libibus-1.0-dev libxtst-dev libxi-dev libx11-dev libxkbcommon-dev libevdev-dev libxdo-dev libgomp1 pkg-config` |
| Fedora | `cmake gcc-c++ alsa-lib-devel ibus-devel libXtst-devel libXi-devel libX11-devel libxkbcommon-devel libevdev-devel libxdo-devel pkgconf-pkg-config` |
| Arch | `cmake base-devel alsa-lib ibus libxtst libxi libx11 libxkbcommon libevdev xdotool pkgconf` |
| Windows | See the [Windows backend requirements](../scripts/windows/README.md#backend-requirements). |
| macOS | Apple Silicon with Xcode command line tools plus `cmake pkg-config`. Install `libomp` if you want OpenMP support. |

CUDA builds need the CUDA Toolkit with `nvcc` on `PATH`.

For Vulkan on Ubuntu/Debian, install the SDK, shader tools, and driver packages. `spirv-headers` supplies `spirv/unified1/spirv.hpp`.

```bash
sudo apt install libvulkan-dev vulkan-tools glslc spirv-tools spirv-headers mesa-vulkan-drivers
```

## Install

After installing the native dependencies, use the [Git install commands](../README.md#install) on Linux or macOS. Build from a native arm64 terminal on macOS with target `aarch64-apple-darwin`. Rosetta can hide the Metal backend from ggml.

If you already have a checkout, initialize the submodule and install from it. Use a Git checkout or an archive that includes `vendor/CrispASR` because GitHub's generated source archives omit submodules.

```bash
git submodule update --init --recursive
cargo install --locked --path .
```

Add the same accelerator feature flag for a local GPU build. Cargo installs the release build to its bin directory, usually `~/.cargo/bin`. Default Linux builds install both `parakit` and `parakit-ibus` and require the IBus development package even when you use X11. See [IBus setup and upgrades](linux-desktop.md#ibus-setup).

For an X11-only build, select the daemon and bundled runtime features explicitly. You can also enable CUDA or Vulkan.

```bash
cargo install --locked --path . --no-default-features --features bundled,daemon
```

Move bundled executables with their `bin` directory, including installs created with `--root`. Native OS libraries, selected BLAS and OpenMP libraries, and GPU runtimes and drivers must be available on the destination. See [Windows runtime bundling](../scripts/windows/README.md#runtime-manifest) and [model preparation tools](dev/README.md#source-rebuild).

## Isolated Development Builds

Use a separate Cargo target directory when validating changes alongside an installed daemon. Invoke the test binaries by path.

```bash
export CARGO_TARGET_DIR="$PWD/target/tmp/validation/cargo"
cargo build --locked --bin parakit --example transcribe-file
"$CARGO_TARGET_DIR/debug/parakit" --version
```

For Linux IBus validation, also build the helper in the same directory.

```bash
cargo build --locked --bin parakit-ibus
```

Keep accelerator builds in a separate target directory. This RTX 5090 example requires a CUDA Toolkit that supports architecture 120.

```bash
export CARGO_TARGET_DIR="$PWD/target/tmp/validation/cargo-gpu"
export PARAKIT_CUDA_ARCHS=120-real
cargo build --locked --features cuda,vulkan --bin parakit --bin parakit-ibus --example transcribe-file
```

Set the architecture for your GPU and reuse the same profile, feature set, and native settings in each target directory to avoid recompiling C/C++. Substitute the isolated executable when checking [runtime library paths](#runtime-library-paths). See [Linux GPU validation](dev/quality.md#linux-cudavulkan) for isolated runtime state.

## CPU Builds

The bundled build enables ggml native CPU code, OpenMP when available, and CPU repacking. With GCC or Clang on Linux, `-march=native` specializes code for the build machine's CPU. Check the flags before moving a binary to a different machine.

```bash
parakit --verbose doctor
```

Benchmark different thread counts with the daemon-free [WAV quality target](dev/quality.md#wav-quality-target).

## BLAS And MKL

`PARAKIT_BLAS` selects a build-time CPU matrix library. Values are trimmed and case-insensitive.

```bash
PARAKIT_BLAS=mkl cargo install --locked --path .
PARAKIT_BLAS=generic cargo install --locked --path .
```

Supported values:

| Value | Behavior |
| --- | --- |
| unset, `auto` | macOS uses Accelerate. Windows checks OpenBLAS prefixes. Linux checks configured OpenBLAS prefixes, then pkg-config for MKL and OpenBLAS, then conventional OpenBLAS prefixes. Linux and Windows use native ggml kernels if no compatible library is found. |
| empty, `off`, `none`, `no`, `false`, `0` | Native/OpenMP CPU kernels without BLAS. |
| `openblas` | `GGML_BLAS=ON`, `GGML_BLAS_VENDOR=OpenBLAS`. |
| `mkl`, `intel`, `intel-mkl` | CrispASR `COHERE_MKL=ON`, ggml `Intel10_64lp`. |
| `generic`, `system`, `blas`, `on`, `yes`, `true`, `1` | `GGML_BLAS=ON`, `GGML_BLAS_VENDOR=Generic`. |
| `accelerate`, `apple` | Apple Accelerate. Apple targets only. |

Windows OpenBLAS layout and bundling behavior are in the [Windows build guide](../scripts/windows/README.md#blas).

Choose OpenBLAS on Ubuntu/Debian with the development package installed.

```bash
sudo apt install libopenblas-dev
PARAKIT_BLAS=openblas cargo install --locked --path .
```

Set `PARAKIT_OPENBLAS_ROOT` to point at a custom OpenBLAS prefix. Linux and Windows also check `OPENBLAS_ROOT`, `OpenBLAS_ROOT`, `OPENBLAS_HOME`, `CMAKE_PREFIX_PATH`, `CONDA_PREFIX`, and vcpkg prefixes. Linux checks `HOMEBREW_PREFIX` and standard system prefixes. Set both `BLAS_INCLUDE_DIRS` and `BLAS_LIBRARIES` to supply paths manually. See [the list of conventional prefixes](../build/openblas_roots.rs).

When `PARAKIT_BLAS` is set, Cargo reports the requested and selected modes. The `cpu:` line in `parakit --verbose doctor` reports the selected mode.

## CrispASR And Backends

The bundled build links CrispASR and ggml statically, so the executable can run outside the checkout or Cargo build directory. Parakit's [build script](../build.rs) obtains archives and runtime dependencies from CMake's File API. The [native manifests](../native) compile the Rust bindings from the submodule. The IBus helper builds separately from the inference runtime. See the pinned [CrispASR v0.8.49 source](https://github.com/CrispStrobe/CrispASR/tree/d94264379dada437aa4ea9163eb34ffe4554422a).

The [native build hook](../build/native_correctness.cmake) substitutes patched session and orchestration sources generated under Cargo's `OUT_DIR`. The patches reject partial utterances on chunk failure, require Qwen to reach EOS, restrict language-metadata parsing to the output header, and retain recoverable quiet clauses around louder speech. Gap recovery stays quiet while native warnings such as the streamed-encoding fallback remain visible.

`CRISPASR_LIB_DIR` selects prebuilt shared libraries and skips the bundled source patches. Match the pinned C ABI, including `crispasr_session_open_with_params`. With the `bundled` feature, supply matching ggml headers and libraries exporting the device registry used by `doctor` and GPU preflight. For Qwen, the external library also needs the language and token setters, equivalent correctness fixes, and free-form context routing through `CRISPASR_QWEN3_HOTWORDS_AS_CONTEXT=1`.

`CRISPASR_SRC_DIR` selects a native source tree containing `CMakeLists.txt`. It must match the pinned ABI and provide the source anchors used by the corrections. The Rust bindings still come from the submodule.

For CUDA builds, set `PARAKIT_CUDA_ARCHS` to override ggml's native architecture selection. It passes a nonblank value to `CMAKE_CUDA_ARCHITECTURES` and accepts values such as `89-real` or semicolon-separated lists.

Cargo features set the corresponding CMake options.

| Cargo feature | CMake option |
| --- | --- |
| `cuda` | `GGML_CUDA=ON`, `GGML_CUDA_NCCL=OFF` |
| `vulkan` | `GGML_VULKAN=ON` |
| `metal` | `GGML_METAL=ON`, `GGML_METAL_EMBED_LIBRARY=ON` |

Metal builds embed shader source in the binary. Metal compiles it at runtime on first GPU use.

## Runtime Library Paths

On Linux, inspect the installed executable's shared dependencies and loader paths.

```bash
ldd "$(command -v parakit)"
readelf -d "$(command -v parakit)" | grep -E "NEEDED|RPATH|RUNPATH"
```

On macOS, use `otool`.

```bash
otool -L "$(command -v parakit)"
otool -l "$(command -v parakit)" | grep -A2 LC_RPATH
```

For a bundled build, expect system libraries, Apple frameworks on macOS, and selected external runtimes. CrispASR/ggml shared libraries and paths into the checkout or Cargo output indicate an unexpected dependency. Builds using `CRISPASR_LIB_DIR` retain the external-library search path.
