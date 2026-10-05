# Model Memory And Idle Lifecycle

The daemon owns one inference session on its worker thread. Idle offload drops
that complete session; it does not unload libraries or reset a process-wide GPU
device. Configuration and user behavior are described in
[running.md](../running.md#idle-model-offload).

## Startup Comparison, 2026-10-05

Linux CPU measurements with eight threads observed 1.39-1.52 GiB reload peaks,
versus 0.73-0.86 GiB at the loaded checkpoint. Allow roughly twice the loaded
CPU residency while reopening a session. The
[GPU startup comparison](https://github.com/pszemraj/parakit/pull/13#issuecomment-5988562906)
records separate startup results and limitations. Longer recordings can still
grow the workspace, and the dated tables below retain their original policies.

## Repeatable Measurements

Build `parakit` and the `profile-memory` example with the same features, native
library, optimization, model, and thread count for both runs. Keep other
inference workloads stopped. Record compiler, driver, and hardware versions.
The example uses the first two seconds and the complete supplied WAV. It follows
the daemon's [startup and reload warmup policy](../running.md#idle-model-offload).

```bash
cargo build --release --features metal --bin parakit --example profile-memory
mkdir -p target/tmp/memory
target/release/examples/profile-memory \
  --output target/tmp/memory/resident --vmmap \
  --model path/to/model.gguf --audio path/to/recording.wav \
  --device gpu --threads 8 --cycles 10 --keep-loaded
target/release/examples/profile-memory \
  --output target/tmp/memory/offload --vmmap \
  --model path/to/model.gguf --audio path/to/recording.wav \
  --device gpu --threads 8 --cycles 10
```

Output directories must be new. The example measures its own process, so its
small, fixed profiling thread and output buffers are included in every reading.
The example requires an explicit local model and never downloads one. Set
`GGML_METAL_PIPELINE_CACHE` under `target/tmp/` when isolating Metal's disk cache.
Match whether that cache is populated between comparisons.

The example records JSON checkpoints before loading, after loading, after
warmup, after short/full transcription, after offload (or the retained
baseline), and after final close. Records include operation elapsed time.
Both transcripts must be nonempty; failures return nonzero. The example saves
input paths and sizes, command, platform, commit/submodule identity,
working-tree state, and `metrics.jsonl`. It does not persist audio or
transcripts.
The retained baseline opens once; the offload run opens on every cycle.

Each checkpoint also records `host_interval_peak`: the largest host residency
sample since the preceding checkpoint, using a 20 ms sampling interval
plus the checkpoint reading. At `after_load`, this captures sampled transient
loading and reload peaks that checkpoint-only readings can miss. Compare cycle 2 and
later `after_load` peaks with the preceding `offloaded` residency when sizing
RAM for idle offload. A reload can temporarily use more RAM than the loaded
checkpoint, even when offload releases memory between dictations.
The metric is RSS on Linux/macOS and working set on Windows; it resets for each
operation instead of reusing a process-lifetime high-water mark. Sample counts
and the interval are saved with each peak. Sampling and native-query latency can
miss shorter spikes, so these figures are observed lower bounds, not guaranteed
memory ceilings. The older tables below contain only checkpoint readings and do
not measure transient reload peaks.

The background sampler measures host residency only. With `--nvidia`, device
memory is queried at checkpoints, not throughout each interval, so it cannot
capture a transient GPU reload spike. Use a vendor profiler or device-memory
trace when sizing GPU headroom.

Loading time excludes earlier process/device initialization. Add warmup time
to open time to estimate readiness after offload. Checkpoint collection occurs
outside those operation timings; background sampling adds a small amount of
measurement overhead during operations. These are diagnostic measurements, not
an isolated latency benchmark.

For reload comparisons, `--reload-warmup startup`, `one-second`, or `none`
overrides the probe after the first session; the default `production` follows
the daemon. The first session always uses startup warmup. Exclude that first
cycle when comparing reload timings. Use `--full-first` to transcribe the full
recording before the short excerpt, so a preceding short inference cannot hide
the cost of a cold full recording. Both transcripts must be nonempty. Use the
PTT worker simulation for readable transcript-quality comparisons.

Test worker admission, queueing, and configured timeouts separately with the
[PTT worker simulation](quality.md#ptt-worker-simulation). Follow the
[desktop smoke checks](quality.md#runtime-smoke-checks) for live input and insertion.

## Platform Measurements

### macOS

Build with `--features metal`, then test `--device cpu` and `--device gpu`
separately. The example records RSS from `ps` and saves `vmmap -summary`
output with `--vmmap`. Inspect physical footprint, mapped files, MALLOC regions
(including empty retained regions), and IOKit/IOAccelerator categories. A
`vmmap` permission failure stays visible in the output; rerun in a permitted
native desktop session.

Metal uses unified memory on Apple silicon. Shared buffers and process metrics
overlap; do not add host RSS to an apparent GPU total. `vmmap`'s total resident
mappings, `ps` RSS, and physical footprint have different accounting. Keep their
names and compare each with itself.

### Linux

Use native CPU, `--features cuda`, and `--features vulkan` builds in separate
output directories. Vulkan needs the development SDK and SPIR-V headers; see
[build instructions](../build.md). Use `--device gpu` for each GPU build so
device absence cannot silently choose CPU.

The example reads `/proc/self/smaps_rollup` and `status`, retaining RSS, PSS,
anonymous memory, file-backed PSS, private dirty pages, and swap in bytes. For
attribution, save `/proc/<pid>/smaps` alongside selected checkpoints or attach a native
heap profiler. Definitions and accounting are in the
[Linux proc documentation](https://www.kernel.org/doc/html/latest/filesystems/proc.html).

Add `--nvidia` for per-process `nvidia-smi` allocations where supported. Vulkan
on other hardware needs vendor GPU tools or a Vulkan memory trace. Missing GPU
readings are unavailable, never zero. Compare CPU anonymous growth with
scheduler placement and staging ownership before calling it a leak.

### Windows

Use the [native build scripts](../../scripts/windows/README.md) with `--no-install`
for separate CPU, CUDA, and Vulkan bundles. Build the example against that
bundle's native import libraries. For CUDA, replace the library path below with
the successful build's actual `out/lib` directory; the target base can differ
when `CARGO_TARGET_DIR` is set. Copy the helper beside the matching bundle's
DLLs so Windows can load them:

```powershell
$env:CRISPASR_LIB_DIR = (Resolve-Path 'target/cuda/release/build/parakit-<hash>/out/lib').Path
cargo build --release --features cuda --example profile-memory
Remove-Item Env:\CRISPASR_LIB_DIR
Copy-Item target/release/examples/profile-memory.exe target/parakit-windows-x86_64-cuda/
target/parakit-windows-x86_64-cuda/profile-memory.exe `
  --output target/tmp/memory/windows-cuda --nvidia `
  --model path/to/model.gguf --audio path/to/recording.wav `
  --device gpu --threads 8 --cycles 10
```

Adjust the helper path if `CARGO_TARGET_DIR` changes its output location.
For Vulkan, substitute its feature and bundle; for CPU, omit the accelerator
feature and use `--device cpu`. Repeat with `--keep-loaded` and a new output
directory for the resident baseline. These commands require native Windows
validation; the measured results below cover macOS and Linux.

The example calls `GetProcessMemoryInfo` to record working set, peak working
set, and private committed bytes. These measures are distinct; see Microsoft's
[PROCESS_MEMORY_COUNTERS_EX](https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters_ex).
Use Sysinternals VMMap for heap/mapped-file attribution. In Task Manager's
Details tab, add dedicated and shared GPU memory columns, or collect matching
GPU Process Memory performance counters during checkpoints. WDDM can make
`nvidia-smi` process memory unavailable; the example records that explicitly.
Do not count shared GPU pages twice against host totals.

## Allocation Ownership And Operation Placement

At the pinned CrispASR revision, [`parakeet.cpp`](../../vendor/CrispASR/src/parakeet.cpp) owns lazy F32 CPU copies of
the predictor LSTM and joint-head weights (`pred_w`, `joint_w`), graph metadata,
and a ggml scheduler. The decoder runs manually on CPU even when the encoder
runs on GPU. GPU selection therefore does not eliminate CPU allocations.
`parakeet_free` frees scheduler, model buffer/context, both backend instances,
and the context containing those vectors.

The quantized weight-file size therefore is not a runtime RAM or VRAM budget.
Loading can temporarily keep a file mapping alongside copied weights. Inference
also needs activation workspace, decoder copies, and runtime/driver state.
Workspace grows with audio length and can retain its high-water capacity until
session destruction. Compare loading peaks and retained post-inference memory
separately, with host and device allocations reported independently.

The [GGUF loader](../../vendor/CrispASR/src/core/gguf_loader.cpp) has opt-in CPU and Metal file-mapping
paths (`CRISPASR_GGUF_MMAP=1`); the default allocates a backend buffer and copies
weights. These measurements keep the default. Distinguish a retained
mapping from a live anonymous duplicate. CUDA has device pools and host-pinned
allocation paths; Vulkan has device/host-visible buffers, pinned memory, and
synchronous staging. Ownership and cache lifetime differ by backend. Metal's
backend registry holds device/library state for the process lifetime.

Confirm operation placement in a separate diagnostic run with
`GGML_SCHED_DEBUG=2`, retaining native stderr from `profile-memory`. Inspect
encoder graph split/node assignments, including CPU fallback. The daemon needs
`--verbose` to forward debug diagnostics. Supplement with GPU traces/counters
when investigating transfers or driver allocations. Enumeration alone proves
availability, not execution. Avoid placement dumps in timed comparisons.

Fix retention at its owner after reproducing it. Do not add forced allocator
purges, GPU resets, or change warmup solely to reduce a single memory counter.
A native patch needs a minimal reproducer, immutable revision, readable
transcript comparison, and backend validation.

## Native Results, 2026-09-30

Measured on an Apple M5 MacBook Air with 32 GiB unified memory, macOS 26.7,
Rust 1.98.1. The paired runs used the same Rust dev-profile example and existing
Metal-enabled bundled native library, eight threads, default Q8_0 model
(745,121,632 bytes), and startup warmup repeated at every open. CPU runs did not
initialize Metal.
The baseline retained its session, matching the previous daemon lifetime; the
comparison dropped the same native session between dictations. Engine and
native inference code were unchanged.

The runs used the same model, 55.36-second reference WAV, and unchanged
CrispASR pin. All numbers below are `ps` RSS in MiB; ranges cover ten cycles.

| Checkpoint | CPU resident baseline | CPU offload run | Metal resident baseline | Metal offload run |
| --- | ---: | ---: | ---: | ---: |
| Before loading | 36.4 | 36.4 | 36.4 | 36.4 |
| First load | 749.6 | 748.7 | 789.1 | 776.4 |
| First warmup | 838.1 | 837.4 | 881.4 | 874.6 |
| First short dictation | 852.2 | 851.3 | 881.6 | 874.6 |
| Full dictations | 1585.8-1591.8 | 1558.8-1590.1 | 899.6-904.3 | 897.3-917.3 |
| Between dictations | 1585.8-1591.8 | 113.8-129.9 | 899.6-904.3 | 162.2-182.2 |
| Final close | 131.6 | 119.8 | 164.6 | 182.2 |

Closing released approximately **1460 MiB on CPU** and **735 MiB on Metal**
after each long dictation. This is session release, not lower active inference
memory. CPU scheduler allocations grew after the full clip and stayed resident
when the session remained open. CPU retained memory after close had no monotonic
increase across ten cycles. Metal residual RSS rose about 20 MiB in ten cycles.
An additional thirty-cycle run ranged from 162.5 to 184.7 MiB offloaded, ending
at 180.4 MiB. This bounds the observed growth; it does not prove a permanent
plateau or reproduce the reported Windows/Linux multi-gigabyte retention.

A separate scheduler trace from the freshly built Metal target confirmed
`MUL_MAT` and `IM2COL` nodes assigned to `MTL0`, with depthwise `CONV_2D_DW`
and associated `CONT` nodes assigned to CPU. Inference completed successfully.
There is real GPU execution and real CPU fallback within the encoder, in
addition to the manual CPU decoder. CUDA/Vulkan placement needs the same native
check before drawing backend-specific conclusions.

`vmmap` supplies further accounting:

- CPU baseline final close: 122.6M physical footprint; 107.7M resident in
  `MALLOC_LARGE (empty)`. Active long inference showed about 1.4G `VM_ALLOCATE`.
- Metal cycle ten offload: 155.6M physical footprint; 92.2M resident in
  `MALLOC_LARGE (empty)`, 30.1M in `MALLOC_SMALL (empty)`, 14.5M in active
  `MALLOC_LARGE`, and 13.9M in active `MALLOC_SMALL`.
- The Metal `mapped file` category stayed at 12.6M resident after close; it was
  not a retained model-sized mapping. IOAccelerator categories remained small.

These snapshots identify substantial allocator retention after live session
allocations have been freed. Device/library caches and heap fragmentation also
remain. They do not assign every residual byte to a call site. No native leak
was demonstrated, so the dependency pin is unchanged. Reload warmup is evaluated
separately below.
These macOS measurements do not explain the reported 3+ GiB Windows/Linux host
memory. The Linux follow-up below did not reproduce it; Windows remains pending.

Timing in these runs was affected by concurrent native compilation and thermal
activity, so it cannot establish a throughput regression. Process-cold Metal
initialization also occurs before the open timer; reopening within one daemon
reuses that device state.

### Reload Warmup Comparison

Sequential Metal measurements on the same machine, model, WAV, native library,
and eight threads compare a one-second reload probe with no reload warmup. Each
case ran two rounds of six sessions, with case order reversed in round two. The
first session used the then-current startup warmup and is excluded below,
leaving ten reloads per cell. Short-first and full-first runs measure the first
real inference separately. No build or other parakit daemon ran concurrently.

| Reload warmup | Open + warmup + 2 s clip, median (range), ms | Open + warmup + 55.36 s clip, median (range), ms |
| --- | ---: | ---: |
| One-second probe | 288 (275-303) | 1770 (1728-1819) |
| None | 200 (197-258) | 1693 (1653-1780) |

The one-second probe retains a real readiness check before queued dictation
while avoiding larger synthetic shapes on every reload. The measurements
support that reload choice on Metal; they do not establish CUDA/Vulkan latency
or explain Windows/Linux host-memory retention.

The totals exclude checkpoint collection and earlier process/device
initialization.
They approximate immediate-release readiness plus inference rather than
perceived latency when loading overlaps a live recording.

## Native Linux Results, 2026-10-01

Measured on Ubuntu 24.04.5, Linux 6.17.0-29-generic, a Ryzen 7 7700X,
and an RTX 5090 with NVIDIA driver 595.91.07. Rust/Cargo were 1.98.0;
CUDA was 12.9.86 and Vulkan instance support was 1.3.275. The native CPU,
CUDA, and Vulkan libraries used the unchanged CrispASR pin
`5f1bb858e803167f1b5fc1eb9a90ffdd1970f7ed`, OpenBLAS, OpenMP, CPU repacking,
and native CPU instructions. CUDA targeted SM120; Vulkan explicitly selected
the RTX 5090 with `GGML_VK_VISIBLE_DEVICES=1` rather than the AMD integrated GPU.
The measured inference code was revision `2e0c457`, before the shorter GPU
startup readiness probe in `1cd2214`. These tables retain the earlier startup
warmup policy; their post-startup-warmup allocations do not describe the current
probe. The backend-specific executables resolved their matching native libraries
through `RPATH`, without `RUNPATH`, under `target/debug/build/parakit-*/out/lib`.

These runs used Rust dev-profile executables and Release native libraries:
default features for CPU, `--features cuda` for CUDA, and `--features vulkan`
for Vulkan.
Each retained/offload pair ran ten cycles with two inference threads on CPU
cores 0 and 1, the same Q8_0 model, and the same 55.36-second reference WAV.
No concurrent build or other test workload ran during sampling. The profiler's
short checkpoint used the first two seconds of the WAV.

All host figures below are MiB from `smaps_rollup`; GPU figures are separate
per-process `nvidia-smi` readings. Ranges cover ten cycles. Do not add GPU
allocations to RSS or PSS.

| Offload run | Before load RSS | Loaded RSS | Warmed RSS | After short RSS | After full RSS | Offloaded RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| CPU | 13.8 | 727.5-793.7 | 821.8-830.1 | 849.4-852.2 | 1571.3-1604.4 | 29.7-83.2 |
| CUDA | 169.3 | 273.5-581.6 | 616.2-631.0 | 616.2-631.0 | 639.9-658.9 | 555.4-658.8 |
| Vulkan | 18.0 | 167.0-192.8 | 266.1-267.1 | 267.1-268.4 | 275.8-282.1 | 191.9-192.8 |

Before-load is one sample per process; later ranges include startup and repeated
reloads, so initialized backend state is present in later loaded checkpoints.

| Backend / run | After full RSS | Between dictations RSS | Between PSS | Between anonymous PSS | Between file PSS | Between private dirty | Full GPU | Between GPU |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| CPU retained | 1571.1-1574.4 | 1571.1-1574.4 | 1566.3-1569.7 | 1560.1-1563.4 | 6.3 | 1560.1-1563.4 | unavailable | unavailable |
| CPU offload | 1571.3-1604.4 | 29.7-83.2 | 24.8-78.4 | 18.5-72.0 | 6.4 | 18.5-72.0 | unavailable | unavailable |
| CUDA retained | 640.0 | 640.0 | 589.4-592.1 | 471.9 | 107.6-110.2 | 493.0-493.3 | 2052 | 2052 |
| CUDA offload | 639.9-658.9 | 555.4-658.8 | 507.2-610.5 | 386.2-489.5 | 110.1-111.0 | 421.9-525.2 | 2052-2054 | 566-568 |
| Vulkan retained | 335.9-337.8 | 335.9-337.8 | 275.4-277.3 | 163.3-165.2 | 112.1 | 173.6-175.6 | 1492 | 1492 |
| Vulkan offload | 275.8-282.1 | 191.9-192.8 | 131.4-132.2 | 50.0-50.8 | 81.4 | 59.7-60.7 | 1491-1492 | 22 |

Swap was zero at every checkpoint. CPU offload released about 1.5 GiB of RSS;
CUDA and Vulkan released about 1.5 GiB of device allocations. CUDA offloaded
RSS settled at 555.4 MiB for cycles 4-10; Vulkan settled at 192.8 MiB after
cycle 2. CPU offloaded RSS varied without accumulating across the
ten reloads. This host did not reproduce multi-gigabyte GPU-backend host RSS.

The final CUDA offload snapshot contained 295.6 MiB in the heap, 83.1 MiB in
anonymous mappings, 76.0 MiB in cuBLASLt, 54.0 MiB in `/dev/nvidiactl`, and
15.7 MiB in libcuda. Vulkan's largest remaining mappings included LLVM
(42.7 MiB), the heap (27.4 MiB), and NVIDIA GPU compiler code (24.2 MiB).
These identify mappings, not the owners of individual heap allocations.
Residual CUDA heap/context retention remains unattributed; no allocator purge
or device reset was added.

| Backend | Retained short / full median ms | Offload short / full median ms | Reload open + probe median ms |
| --- | ---: | ---: | ---: |
| CPU | 402 / 10428 | 411 / 10548 | 588 |
| CUDA | 99 / 2108 | 98 / 2168 | 209 |
| Vulkan | 93 / 1707 | 92 / 1699 | 251 |

Reload medians exclude cycle 1 and include the production one-second readiness
probe. Checkpoint collection is excluded; these are diagnostic timings on two
CPU cores, not latency guarantees. Scheduler diagnostics confirmed encoder work on
CUDA0 and Vulkan0 with expected CPU splits. These results establish allocation
placement for this host, not memory ceilings or native Windows behavior.
