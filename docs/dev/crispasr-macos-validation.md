# CrispASR v0.8.42 macOS validation

Validated on 2026-10-11 at Parakit `351e608`, with CrispASR
`3be8bcb4390b8080dc73f807b7abb34dc0aca9c3` and its pinned nested
submodules. The machine is an Apple M5 MacBook Air with 32 GiB RAM,
macOS 26.7, Rust 1.98.1 and CMake 4.4.4.

Builds use `target/mac-vendor-validation/`; commands, logs and disposable
runners are under ignored `target/tmp/mac-vendor-validation/`. Test config
is isolated there, and `XDG_CACHE_HOME` points to `target/mv-cache/`.
The shorter cache path keeps the control socket below macOS's Unix socket
path limit. The existing main-built daemon was left running throughout.
Earlier Metal runs used the existing system shader archive. Final native
startup/restart and full-memo offload checks set `GGML_METAL_PIPELINE_CACHE`
to `target/tmp/mac-vendor-validation/metal-cache/`, confirming creation and
reuse of an independent archive.

## Build and desktop checks

- Native CPU and Metal daemon/example builds pass. These are Rust development
  binaries with CMake Release native libraries and Accelerate BLAS.
- Formatting, rustdoc (130 files, zero issues), workspace/all-target checking,
  all 415 tests and Clippy with warnings denied pass.
- Raw all-features configuration fails with `CUDA Toolkit not found`, as
  expected on this host. Rust all-features checking passes using the newly
  built Metal library through `CRISPASR_LIB_DIR`.
- Doctor reports the Apple M5 Metal device, the 48 kHz mono built-in microphone,
  and granted Accessibility, Input Monitoring and Microphone permissions.
  The deep standard-paste diagnostic passes its guarded paste/readback and
  clipboard restoration checks.
- dyld loads matching CrispASR 0.8.42 and ggml 0.26.0 libraries from each
  isolated build. The CPU build does not load Metal. The Metal binary has the
  expected `LC_RPATH`; its backend contains embedded shader source and reports
  using that embedded library during inference.
- Quiet rules, missing-model errors and isolated cache/control commands pass.
  The installed quantizer executes with the fetch helper's sibling-library
  environment; `--help` prints usage and returns its expected status 1.

The vendor's deprecated `CrispASR` wrapper warnings remain. The existing
`block` 0.1.6 dependency also emits a Rust future-incompatibility warning
about an uninhabited static; main has the same locked version. Apple Clang
does not find OpenMP on this machine, so native execution uses the available
threading support despite the requested `GGML_OPENMP=ON` option.

## Recognition and inference

The unchanged hosted Q8_0 model has SHA256
`10f38dd9ce69ce555a413d9b4201ae5d93c2d7cadc91a285f4bfeeec6eee635a`.
The 55.36-second Juniper memo has SHA256
`d79d7f729b192642b91aef5e1486b97c1cb7f4c34f6ce4a0d45199a9ff23ab5f`.
An 8.2-second leading excerpt supplies the short-input check. Both backends
use eight requested threads and that revision's startup defaults, with both
weight-copy environment variables absent at launch.

Each direct-engine example produces identical text across three inferences.
CPU and Metal words match on both inputs, and the full memo matches the
known Q8 wording apart from capitalization. Observed inference-only times:

| Backend | Audio | First inference | Next two inferences |
| --- | --- | --- | --- |
| CPU | 8.2 s | 458 ms | 405 / 410 ms |
| Metal | 8.2 s | 184 ms | 111 / 111 ms |
| CPU | 55.36 s | 3,072 ms | 3,151 / 3,081 ms |
| Metal | 55.36 s | 741 ms | 655 / 648 ms |

These are observations on a shared machine, with the main daemon resident.
No old-pin macOS timing comparison was run.

## Worker and daemon lifecycle

The real daemon worker transcribes the full memo on both CPU and Metal with
empty stderr. Quiet Metal replay writes zero bytes to stdout and stderr.
JSONL readback preserves the raw transcript and Parakit 0.4.2 version.

Four short Metal worker dictations complete with identical text, four timed
offloads and three automatic reloads. Immediate simulated release queues the
audio while reload is still underway. Reloaded loaded-residency RSS medians
are 872.6, 873.7 and 875.1 MiB. This small observed change across three reloads
does not establish behavior over an unlimited number of cycles.

A separate full-memo Metal worker stays idle for 62 seconds against a
one-minute offload timeout. Sampled RSS falls from a post-inference median
of 956.6 MiB to 215.1 MiB after offload. The largest sampled value is
957.0 MiB. Sampling every 500 ms can miss brief peaks; these RSS observations
do not measure a separate GPU allocation or macOS physical footprint.

Two native daemon launches reach idle with the microphone stream and Metal
model loaded. Each checks its own PID through status, handles a second start
as already running, reads empty history, stops through its isolated control
socket and confirms not-running status. The second launch reuses the model
and its independent shader archive. These checks send no push-to-talk chord.

## Scope

The corpus covers one real memo and its short excerpt. It does not establish
recognition accuracy on other recordings, physical push-to-talk capture
integrity, speech-driven desktop insertion, sleep/wake behavior or allocation
exhaustion. The desktop diagnostic tests a sentinel paste independently of
the real-WAV inference checks. Windows native validation remains separate.
