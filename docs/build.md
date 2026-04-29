# Build

parakit is a Rust 1.87+ binary that links to the vendored CrispASR submodule.
The default build is CPU-only and local-machine optimized.

## Native Dependencies

Cargo handles Rust packages. System packages are still needed for audio,
keyboard hooks, text insertion, CMake, and optional accelerator SDKs.

| OS | Packages |
| --- | --- |
| Ubuntu 24.04 | `cmake build-essential libasound2-dev libudev-dev libxtst-dev libxdo-dev libxi-dev libx11-dev libevdev-dev libgomp1 pkg-config autoconf libtool` |
| Fedora | `cmake gcc-c++ alsa-lib-devel libudev-devel libXtst-devel libxdo-devel libXi-devel libX11-devel libevdev-devel pkgconf autoconf libtool` |
| Arch | `cmake base-devel alsa-lib libxtst xdotool libxi libx11 libevdev pkgconf autoconf libtool` |
| Windows | Visual Studio 2022 with the "Desktop development with C++" workload. |
| macOS | Xcode command line tools plus `cmake autoconf automake libtool pkg-config`. |

CUDA builds need the CUDA Toolkit with `nvcc` on `PATH`.

Vulkan builds on Ubuntu/Debian need:

```bash
sudo apt install libvulkan-dev vulkan-tools glslc spirv-tools spirv-headers mesa-vulkan-drivers
```

`spirv-headers` provides `spirv/unified1/spirv.hpp`; it is not the same package
as `spirv-tools`.

## Install Commands

```bash
cargo install --path .
PARAKIT_BLAS=auto cargo install --path .
cargo install --path . --features cuda
cargo install --path . --features vulkan
cargo install --path . --features metal
```

`cargo install --path .` installs the release binary to Cargo's bin directory,
usually `~/.cargo/bin`.

Add `--locked` for CI or reproducibility checks when Cargo must use the exact
versions in `Cargo.lock`. Leave it off for normal local installs.

## CPU Builds

The bundled CMake path enables ggml native CPU code, OpenMP, and CPU repacking.
On Linux with GCC or Clang this usually means `-march=native` for the local
machine.

Inspect the compiled flags:

```bash
parakit doctor
parakit --verbose
```

Benchmark different thread counts without the daemon:

```bash
cargo run --release --example transcribe-file -- --audio path/to/sample.wav --threads 8 --repeat 3
```

## BLAS And MKL

Native ggml kernels are the default. BLAS/MKL can help some matrix paths but
adds system-library dependencies.

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
| `auto` | Apple Accelerate on macOS; otherwise MKL if `mkl-sdl.pc` is visible; otherwise OpenBLAS if `openblas.pc` or `openblas64.pc` is visible; otherwise off. |
| `openblas` | `GGML_BLAS=ON`, `GGML_BLAS_VENDOR=OpenBLAS`. |
| `mkl` | CrispASR `COHERE_MKL=ON`, ggml `Intel10_64lp`. |
| `generic` | `GGML_BLAS=ON`, `GGML_BLAS_VENDOR=Generic`. |
| `accelerate` | Apple Accelerate. Apple targets only. |

Ubuntu/Debian OpenBLAS:

```bash
sudo apt install libopenblas-dev
PARAKIT_BLAS=openblas cargo install --path .
```

The selected mode is printed during explicit BLAS builds and later shown by
`parakit doctor`.

## CrispASR And Backends

The repository vendors CrispASR as a git submodule. `build.rs` builds it with
CMake, installs shared libraries under
`target/<profile>/build/parakit-*/out/lib`, and builds `crispasr-quantize` for
`parakit fetch --from-source`.

Feature mapping:

| Cargo feature | CMake option |
| --- | --- |
| `cuda` | `GGML_CUDA=ON` |
| `vulkan` | `GGML_VULKAN=ON` |
| `metal` | `GGML_METAL=ON` |

## Runtime Library Paths

Linux/BSD builds must use transitive `RPATH`, not `RUNPATH`, so
`libwhisper.so` can find sibling `libggml*.so` files.

Verify:

```bash
ldd target/debug/parakit | grep -E "whisper|ggml"
readelf -d target/debug/parakit | grep -E "RPATH|RUNPATH"
```

The library paths should point into `target/debug/build/parakit-*/out/lib`, and
`readelf` should report `RPATH`.

## Windows DLLs

Windows has no rpath. After building, copy generated DLLs next to the binary or
put the generated `out\lib` directory on `PATH`:

```powershell
cargo build --release
copy target\release\build\parakit-*\out\lib\*.dll target\release\
```

Security software may flag global hooks plus text insertion. Whitelist the
binary if needed.

## Updating CrispASR

Keep submodule updates separate from parakit code changes:

```bash
cd vendor/CrispASR
git fetch
git checkout <tag-or-commit>
cd ../..
git add vendor/CrispASR
cargo build
```
