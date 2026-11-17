# Build

Parakit is a Rust application that links to the vendored [CrispASR](https://github.com/CrispStrobe/CrispASR) submodule. The default build is CPU-only and may use detected CPU BLAS libraries.

Command examples use a POSIX shell unless the surrounding section is Windows-specific. Windows-only commands are shown as `bat` or `powershell`.

Install Rust via [rustup](https://rustup.rs) if you do not already have a toolchain; parakit requires Rust 1.87 or newer (`rust-version` in `Cargo.toml`).

## Native Dependencies

Cargo handles Rust packages. System packages are still needed for audio, IBus input, X11/XTest fallback insertion, CMake, and optional accelerator SDKs.

CMake 3.19 or newer is required for bundled builds.

| OS | Packages |
| --- | --- |
| Ubuntu 24.04 | `cmake build-essential libasound2-dev libibus-1.0-dev libxtst-dev libxi-dev libx11-dev libxkbcommon-dev libevdev-dev libxdo-dev libgomp1 pkg-config` |
| Fedora | `cmake gcc-c++ alsa-lib-devel ibus-devel libXtst-devel libXi-devel libX11-devel libxkbcommon-devel libevdev-devel xdotool-devel pkgconf` |
| Arch | `cmake base-devel alsa-lib ibus libxtst libxi libx11 libxkbcommon libevdev xdotool pkgconf` |
| Windows | See the [Windows backend requirements](../scripts/windows/README.md#backend-requirements). |
| macOS | Apple Silicon with Xcode command line tools plus `cmake pkg-config`. |

CUDA builds need the CUDA Toolkit with `nvcc` on `PATH`.

Vulkan builds on Ubuntu/Debian need:

```bash
sudo apt install libvulkan-dev vulkan-tools glslc spirv-tools spirv-headers mesa-vulkan-drivers
```

`spirv-headers` provides `spirv/unified1/spirv.hpp`; it is not the same package as `spirv-tools`.

## Install

On Linux and Apple Silicon macOS, install directly from Git after installing the native dependencies above:

```bash
cargo install --locked --git https://github.com/pszemraj/parakit.git parakit
```

On Apple Silicon, add `--features metal` to enable GPU inference:

```bash
cargo install --locked --git https://github.com/pszemraj/parakit.git parakit --features metal
```

For a local checkout, initialize its submodules before installing:

```bash
git submodule update --init --recursive
cargo install --locked --path .
```

This installs the release build to Cargo's bin directory, usually `~/.cargo/bin`, using the committed `Cargo.lock`. Default Linux features install and update two executables together:

- `parakit`: the CLI and dictation daemon.
- `parakit-ibus`: the input-method helper; see [automatic startup and upgrades](linux-desktop.md#ibus-setup).

Linux builds enable IBus by default, including when adding `--features cuda`. The IBus development package in the [native dependency table](#native-dependencies) is required even when upgrading an existing X11 installation. Follow the [IBus setup](linux-desktop.md#ibus-setup) when choosing native input. To build an X11-only daemon without IBus support:

```bash
cargo install --locked --path . --no-default-features --features bundled,daemon
```

That build retains X11 clipboard paste and direct typing, and cannot provide native GNOME Wayland dictation. Add `--features cuda` or `--features vulkan` when the corresponding accelerator is wanted.

Install behavior:

- [Bundled builds](#crispasr-and-backends) produce executables that do not need the checkout, Cargo Git cache, or native build directory. Custom `--root` installations can be moved together with their `bin` directory.
- Native OS libraries and selected BLAS, OpenMP, GPU runtimes, and drivers remain prerequisites. On Windows, the [bundle procedure](../scripts/windows/README.md) can package optional runtime DLLs.
- Q6_K preparation and source rebuilding require [separately installed tools](dev/README.md#source-rebuild). Hosted Q8_0 downloads and dictation do not use them.
- GitHub auto-generated source archives are unsupported because they do not include the CrispASR submodule.

Optional accelerator builds:

```bash
cargo install --locked --path . --features cuda
cargo install --locked --path . --features vulkan
```

For a local Metal build, use `cargo install --locked --path . --features metal`. On macOS, `aarch64-apple-darwin` is the supported target. Build from a native arm64 terminal; an x86_64/Rosetta build may not expose the Metal backend to ggml.

## Isolated Development Builds

Use `cargo build` with a separate [Cargo target directory](https://doc.rust-lang.org/cargo/reference/build-cache.html) when validating changes alongside an installed daemon. Build both the daemon and WAV helper, then invoke those binaries by path:

```bash
export CARGO_TARGET_DIR="$PWD/target/tmp/validation/cargo"
cargo build --locked --bin parakit --example transcribe-file
"$CARGO_TARGET_DIR/debug/parakit" --version
```

For Linux IBus validation, also build the sibling engine in the same target directory:

```bash
cargo build --locked --bin parakit-ibus
```

For Linux CUDA/Vulkan validation, use a separate GPU directory. This RTX 5090 example requires a CUDA Toolkit that supports architecture 120:

```bash
export CARGO_TARGET_DIR="$PWD/target/tmp/validation/cargo-gpu"
export PARAKIT_CUDA_ARCHS=120-real
cargo build --locked --features cuda,vulkan --bin parakit --bin parakit-ibus --example transcribe-file
```

Use the architecture supported by your GPU and toolkit. Reuse the same directory, feature set, profile, and native build settings for subsequent builds. Changing them can require another native compile; `cargo check` does not produce the runnable binaries. An existing directory built with `--all-features` should keep that feature set when rebuilding; Linux intentionally ignores the unsupported Metal feature.

These builds do not install a binary or restart a running daemon. Verify [runtime library paths](#runtime-library-paths), substituting the isolated binary path for `$(command -v parakit)`, then follow the [Linux GPU procedure](dev/quality.md#linux-cudavulkan) for isolated configuration and model state.

## CPU Builds

The bundled CMake path enables ggml native CPU code, OpenMP, and CPU repacking. On Linux with GCC or Clang this usually means `-march=native` for the local machine.

Inspect the compiled flags:

```bash
parakit --verbose doctor
```

Benchmark different thread counts with the daemon-free [WAV quality target](dev/quality.md#wav-quality-target).

## BLAS And MKL

The build defaults to `PARAKIT_BLAS=auto`. If no supported BLAS is detected, parakit uses native ggml CPU kernels. BLAS/MKL can help some matrix paths but adds system-library dependencies. Values are trimmed and case-insensitive; an explicitly empty value disables BLAS, while an unset value selects `auto`.

```bash
PARAKIT_BLAS=openblas cargo install --locked --path .
PARAKIT_BLAS=mkl cargo install --locked --path .
PARAKIT_BLAS=generic cargo install --locked --path .
```

Supported values:

| Value | Behavior |
| --- | --- |
| unset, `auto` | Apple Accelerate on macOS. Windows searches configured and conventional OpenBLAS prefixes, otherwise disables BLAS. Linux honors configured OpenBLAS prefixes, then tries MKL and OpenBLAS through pkg-config, then checks conventional system prefixes. |
| empty, `off`, `none`, `no`, `false`, `0` | Native/OpenMP CPU kernels without BLAS. |
| `openblas` | `GGML_BLAS=ON`, `GGML_BLAS_VENDOR=OpenBLAS`. |
| `mkl`, `intel`, `intel-mkl` | CrispASR `COHERE_MKL=ON`, ggml `Intel10_64lp`. |
| `generic`, `system`, `blas`, `on`, `yes`, `true`, `1` | `GGML_BLAS=ON`, `GGML_BLAS_VENDOR=Generic`. |
| `accelerate`, `apple` | Apple Accelerate. Apple targets only. |

Windows OpenBLAS layout and bundling behavior are in the [Windows build guide](../scripts/windows/README.md#blas).

Ubuntu/Debian OpenBLAS:

```bash
sudo apt install libopenblas-dev
PARAKIT_BLAS=openblas cargo install --locked --path .
```

Linux and Windows OpenBLAS prefix detection recognizes `PARAKIT_OPENBLAS_ROOT`, `OPENBLAS_ROOT`, `OpenBLAS_ROOT`, `OPENBLAS_HOME`, active Conda environments, `CMAKE_PREFIX_PATH`, and vcpkg installs; Linux also checks `HOMEBREW_PREFIX`. It also checks common Conda and native installation prefixes on Windows and `/usr/local`, `/usr`, `/opt/openblas`, and `/opt/OpenBLAS` on Linux. Explicit `BLAS_INCLUDE_DIRS` plus `BLAS_LIBRARIES` remain the final manual override.

Explicit `PARAKIT_BLAS` builds print the requested and selected modes as a Cargo warning, and `parakit --verbose doctor` reports the selected mode on its `cpu:` line.

## CrispASR And Backends

The repository vendors [CrispASR v0.8.49](https://github.com/CrispStrobe/CrispASR/tree/d94264379dada437aa4ea9163eb34ffe4554422a) at `d94264379dada437aa4ea9163eb34ffe4554422a` as a git submodule. Small manifests under `native/` compile its Rust binding sources unchanged, without upstream build scripts. Parakit's `build.rs` owns the single native build, including Git installs that do not discover local Cargo configuration. It asks CMake's File API for a consumer target's complete archive and runtime dependencies, then bundles the archives into Parakit's Rust library. The standalone IBus helper does not link this library.

The bundled build compiles patched copies of CrispASR's session and Parakeet orchestration sources under Cargo's `OUT_DIR`; it leaves the submodule unchanged. The patches make chunk failures fail the whole utterance, require Qwen decoding to reach EOS, and restrict language-metadata parsing to the output header. Qwen's energy filter also retains recoverable quiet clauses around louder speech. The orchestration copy removes the informational gap-recovery notice while preserving warnings, including the streamed-encoding fallback after an allocation failure. A deferred CMake hook substitutes these two translation units.

`CRISPASR_LIB_DIR` is for advanced local experiments with an already-built compatible CrispASR tree. The library must match the pinned C ABI, including `crispasr_session_open_with_params`. Bundled builds must also provide compatible ggml libraries with the exported device registry entry points used by `parakit doctor` and `--device gpu` preflight. Parakit compiles its device-query adapter against the selected native source headers, including when `CRISPASR_LIB_DIR` skips native compilation; those headers must match the supplied libraries.

External libraries must also supply the Qwen language/token setters, free-form context routing through `CRISPASR_QWEN3_HOTWORDS_AS_CONTEXT=1`, and equivalent native correctness fixes before using Qwen. Selecting an external library skips the bundled source patches; the bundled build is the initial Qwen acceptance target.

`CRISPASR_SRC_DIR` is an optional path to another CrispASR source tree containing `CMakeLists.txt`; by default, the bundled CMake build uses the vendored tree. An alternative tree must remain ABI-compatible with the vendored Rust crates and retain the reviewed source anchors used by the bundled correctness patches. This override changes native sources and headers only, not the Rust path dependencies. It cannot replace a missing submodule because Cargo resolves those path dependencies before `build.rs` runs.

For CUDA builds, `PARAKIT_CUDA_ARCHS` is trimmed and, when nonempty, passed as `CMAKE_CUDA_ARCHITECTURES`; omission or a blank value keeps ggml's native selection. CMake accepts values such as `89-real` and semicolon-separated architecture lists.

Feature mapping:

| Cargo feature | CMake option |
| --- | --- |
| `cuda` | `GGML_CUDA=ON` |
| `vulkan` | `GGML_VULKAN=ON` |
| `metal` | `GGML_METAL=ON`, `GGML_METAL_EMBED_LIBRARY=ON` |

CUDA builds also force `GGML_CUDA_NCCL=OFF`.

Metal builds embed the shader source into the static Metal backend, so there is no loose `default.metallib` to carry with the binary. The first GPU use still pays normal Metal runtime compilation/warmup cost.

On macOS, `libomp` is optional; ggml's OpenMP path degrades when OpenMP is not available, and Metal handles GPU offload separately.

## Runtime Library Paths

Bundled builds must not load CrispASR or ggml from a build directory. On Linux, inspect the installed executable:

```bash
ldd "$(command -v parakit)"
readelf -d "$(command -v parakit)" | grep -E "NEEDED|RPATH|RUNPATH"
```

There should be no CrispASR/ggml shared dependencies or RPATH/RUNPATH entries into the checkout or Cargo build output. System libraries such as OpenBLAS and OpenMP are expected when selected.

On macOS, inspect the installed executable and its loader paths:

```bash
otool -L "$(command -v parakit)"
otool -l "$(command -v parakit)" | grep -A2 LC_RPATH
```

Apple frameworks and separately installed runtimes are expected; build-directory CrispASR/ggml dylibs are not. Explicit external-library builds retain their dynamic-library search paths. See [macOS CPU and Metal validation](dev/model-validation.md#macos-cpu-and-metal) for native checks.
