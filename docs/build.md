# Build

parakit is a Rust 1.87+ binary that links to CrispASR. With the default
`bundled` feature, `build.rs` builds the vendored CrispASR submodule with CMake
and links the resulting shared libraries into the Rust binary.

## Native Dependencies

Rust dependencies are handled by Cargo. Native packages are still required for
audio, keyboard hooks, synthetic typing, CMake, and backend SDKs.

| OS | Packages |
| --- | --- |
| Ubuntu 24.04 | `cmake build-essential libasound2-dev libudev-dev libxtst-dev libxdo-dev libxi-dev libx11-dev libevdev-dev libgomp1 pkg-config autoconf libtool` |
| Fedora | `cmake gcc-c++ alsa-lib-devel libudev-devel libXtst-devel libxdo-devel libXi-devel libX11-devel libevdev-devel pkgconf autoconf libtool` |
| Arch | `cmake base-devel alsa-lib libxtst xdotool libxi libx11 libevdev pkgconf autoconf libtool` |
| Windows | Visual Studio 2022 with the "Desktop development with C++" workload. |
| macOS | Xcode command line tools plus `cmake autoconf automake libtool pkg-config`. |

For CUDA, install the CUDA Toolkit and ensure `nvcc` is on `PATH`.

For Vulkan on Ubuntu/Debian, install the loader/dev package, tools, shader
compiler, SPIR-V tools, SPIR-V headers, and a driver:

```bash
sudo nala install libvulkan-dev vulkan-tools glslc spirv-tools spirv-headers mesa-vulkan-drivers
```

`spirv-tools` and `spirv-headers` are different packages. A system can have
`glslc` and `vulkan.pc` installed and still fail if `spirv-headers` is missing.

## Cargo Features

| Command | Behavior |
| --- | --- |
| `cargo build --release` | CPU-only bundled CrispASR build. |
| `cargo build --release --features cuda` | Builds ggml with CUDA support. |
| `cargo build --release --features vulkan` | Builds ggml with Vulkan support. |
| `cargo build --release --features metal` | Builds ggml with Metal support on macOS. |
| `cargo build --release --no-default-features --features daemon` | Builds the daemon against an existing `libcrispasr`; set `CRISPASR_LIB_DIR` if needed. |
| `cargo build --release --no-default-features --example transcribe-file` | Builds only the file transcription helper against an existing `libcrispasr`. |

The `daemon` feature is enabled by default and includes desktop/audio
dependencies. The file transcription example remains useful on machines where
daemon dependencies are not installed.

## Bundled CrispASR

The repository vendors CrispASR as a git submodule:

```toml
crispasr = { path = "vendor/CrispASR/crispasr", default-features = false }
```

The same submodule provides the Rust bindings and the C/C++ library source.
`build.rs` configures CMake, builds CrispASR, installs shared libraries under
`target/<profile>/build/parakit-*/out/lib`, and exposes that directory to
Cargo's linker. It also builds CrispASR's `crispasr-quantize` tool under
`target/<profile>/build/parakit-*/out/bin` so `parakit fetch` can produce the
canonical Q8_0 model from the official NVIDIA checkpoint.

Feature selection maps to CMake options:

| Cargo feature | CMake option |
| --- | --- |
| `cuda` | `-DGGML_CUDA=ON` |
| `vulkan` | `-DGGML_VULKAN=ON` |
| `metal` | `-DGGML_METAL=ON` |

## Runtime Library Paths

On Linux and BSD, parakit needs a transitive rpath because the binary loads
`libwhisper.so`, which then loads sibling `libggml*.so` files.

`build.rs` sets:

- `CMAKE_INSTALL_RPATH=$ORIGIN`
- `CMAKE_BUILD_WITH_INSTALL_RPATH=ON`
- `-Wl,--disable-new-dtags`

The last flag keeps the binary on `DT_RPATH` instead of `DT_RUNPATH`.
`DT_RUNPATH` is not transitive and can fail at runtime even when direct
dependencies resolve.

After a Linux build, verify:

```bash
ldd target/debug/parakit | grep -E "whisper|ggml"
readelf -d target/debug/parakit | grep -E "RPATH|RUNPATH"
```

The library paths should point into `target/debug/build/parakit-*/out/lib`,
and `readelf` should report `RPATH`.

## Windows DLLs

Windows does not have rpath. After building, make the generated DLLs findable:

```powershell
cargo build --release
copy target\release\build\parakit-*\out\lib\*.dll target\release\
```

Alternatively, add the generated `out\lib` directory to `PATH`.

Security software may flag the daemon because it combines a global keyboard
hook with synthetic typing. That behavior is expected for this kind of tool.

## Updating CrispASR

```bash
cd vendor/CrispASR
git fetch
git checkout <tag-or-commit>
cd ../..
git add vendor/CrispASR
cargo build
```

Keep submodule updates separate from parakit code changes.
