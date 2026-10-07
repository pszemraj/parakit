# Native macOS validation, 2026-10-07

This report covers PR #13 on an Apple M5 MacBook Air with 32 GiB unified
memory, macOS 26.7 (25G229), and Rust/Cargo 1.98.1. Runtime source is
`c71b104`; the CPU memory run used its parent, `b77276f`, before the Metal-only
memory probe fix. CrispASR remains at
`5f1bb858e803167f1b5fc1eb9a90ffdd1970f7ed`.

The daemon was built with `--features metal` in Rust's dev profile. Its native
libraries are Release builds with Accelerate, OpenMP, and embedded Metal;
`otool` confirmed the bundled libraries and build-directory RPATH. Dedicated
worker and daemon runs used eight threads and isolated config, cache/runtime
directories, model copy, and Metal pipeline cache. Raw
measurements, transcripts, and diagnostics remain ignored under
`target/tmp/macos-validation/`; the audio fixtures stay local.

## Results

| Check | Result |
| --- | --- |
| Native arm64 Metal build and `doctor --deep` | Passed; Accessibility, Input Monitoring, microphone, capture, and production guarded paste available. |
| Real WAV worker, CPU and Metal | Passed two dictations and two one-minute idle offloads per backend. Transcripts matched across reload and between backends. |
| Startup `status` and `stop`, idle offload, restart | Passed on CPU and Metal. Status/history polling did not postpone offload. Stop exited zero, removed the socket, and released the singleton; restart retained the config and reset history. |
| Native speech capture and insertion | Passed one acoustic fixture capture per backend through CPAL and production paste, with exactly one history entry and insertion each. |
| Missing model reload and recovery | Passed on CPU and Metal: an offloaded missing model reported an error, retained history, and left the receiver unchanged; restoring the file cleared the error on the next PTT. |
| Stop during speech transcription | Passed on CPU and Metal with real microphone audio; exit zero, socket removed, no GGML assertion. |
| Forced Metal and reload budget guard | Forced GPU used Metal. With a sparse valid model, `auto` reloaded on CPU with a warning and identical text; `gpu` failed without a CPU reload or second transcript. |
| Clipboard restoration | Text, HTML with/without text fallback, TIFF image, HTML+image, and multiple native file URLs passed independent readback. A real screenshot passed the production deep diagnostic. A genuine Finder-copied file passed deep doctor and exact file-identity readback. |

The sparse guard fixture retains the actual GGUF tensor data but extends its
file length to 16 GiB. This exercises the production model-size-plus-workspace
comparison against real macOS memory readings without allocating 16 GiB. It
validates the fallback/rejection paths, not literal GPU exhaustion or desktop
notification display.

A bounded physical-pressure attempt held 7.25 GiB of additional Metal buffers,
but free and inactive host memory remained at least 5.31 GiB. It did not reach
the 1600 MiB target, so `auto` correctly stayed on Metal and both WAV
transcripts matched. All buffers were released within 2.422 seconds; kernel
pressure remained at its initial warning level. Forced-GPU physical stress
was skipped because the automatic fallback condition was not reached.

Acoustic tests play a 15-second excerpt of the same real recording into the
physical microphone. They validate capture and insertion, rather than
transcription quality: speaker/microphone replay changes the signal. The
full-file worker comparisons use the original 55.36-second WAV directly.

An early clipboard test harness error lost its in-memory snapshot of the
pre-test user clipboard. That original content could not be recovered.
Subsequent tests used durable private snapshots and verified restoration of
their test payloads; the clipboard results do not establish recovery of the
original user clipboard.

## Memory observations

Values are MiB. RSS is native process resident memory; Metal allocation is
`MTLDevice.currentAllocatedSize` sampled inside the worker process every
100 ms. The observer was checked against a known 256 MiB Metal allocation.
Metal shares host memory on this machine, so these columns must not be added.

| Backend | RSS after first full audio | Offloaded RSS | RSS after reload/full audio | Metal after full audio | Offloaded Metal |
| --- | ---: | ---: | ---: | ---: | ---: |
| CPU | 1580.3 | 120.1-120.5 | 1150.8-1582.5 | not measured | not measured |
| Metal | 878.9-893.2 | 123.8-147.5 | 880.1 | 1460.61 | 0.69 |

These are ranges from one process per backend, not repeated-host estimates or
memory ceilings. RSS can change while a session remains loaded because of
system memory pressure. Metal's offloaded physical footprint was about
126-127 MiB; remaining process/device state is separate from the dropped
inference session.

## Fix and validation boundary

Before the fix, ggml reported about 25 GiB free on Metal while macOS reported
about 7 GiB in free and inactive pages. A sibling process's 256 MiB Metal
allocation reduced only that sibling's reported allocation budget. Metal's
budget alone therefore did not account for other programs using unified
memory.

The fix caps Metal's reported free bytes by the free and inactive host pages.
It also treats an unsigned backend subtraction that exceeds the total budget
as zero available bytes, with a regression test. Speculative pages are already
included in free pages, and purgeable pages can overlap other queues; neither
is added again. This remains an estimate, without reserving memory. If the
native host probe is unavailable, the existing backend reading remains the
fallback.

The full Rust loop passed: package formatting, strict rustdoc (95 files,
zero issues), workspace/all-targets checking, 370 tests, strict default and
Metal Clippy, and builds. Raw all-features configuration failed because this
Mac lacks the CUDA Toolkit; the documented `CRISPASR_LIB_DIR` fallback passed
using the successful native Metal library. Known CrispASR deprecations remain.
The upstream `block` 0.1.6 dependency also reports a future incompatibility for
its uninhabited `_NSConcreteStackBlock` static; it does not fail the current
build or tests.

Literal near-exhaustion testing remains pending because the bounded physical
allocation did not reach the reload threshold. Physical keyboard input,
audible cue observation, sleep/wake, held-modifier direct typing, overlap,
and clipboard-manager behavior remain human checks. Rich text coverage here
is HTML; it does not establish restoration of RTF or arbitrary application
formats. Windows results are independent and unchanged.
