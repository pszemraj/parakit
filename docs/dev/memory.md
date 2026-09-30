# Model Memory And Idle Lifecycle

The daemon owns one inference session on its worker thread. Idle offload drops
that complete session; it does not unload libraries or reset a process-wide GPU
device. Configuration and user behavior are described in
[running.md](../running.md#idle-model-offload).

## Repeatable Measurements

Build `parakit` and the `profile-memory` example with the same features, native
library, optimization, model, and thread count for both runs. Keep other
inference workloads stopped. Record compiler, driver, and hardware versions.
The example uses the first two seconds and the complete supplied WAV. It follows
the daemon's warmup policy: one second on CPU, five and thirty seconds on GPU.

```bash
cargo build --release --features metal --bin parakit --example profile-memory
mkdir -p target/tmp/memory
python scripts/profile_memory.py --output target/tmp/memory/resident --vmmap -- \
  target/release/examples/profile-memory \
  --model path/to/model.gguf --audio path/to/recording.wav \
  --device gpu --threads 8 --cycles 10 --keep-loaded
python scripts/profile_memory.py --output target/tmp/memory/offload --vmmap -- \
  target/release/examples/profile-memory \
  --model path/to/model.gguf --audio path/to/recording.wav \
  --device gpu --threads 8 --cycles 10
```

Use an existing Python environment; the collector needs only the standard
library. With conda, prefix Python commands with
`conda run -n <environment> --live-stream`. Output directories must be new.
The example requires an explicit local model and never downloads one. Set
`GGML_METAL_PIPELINE_CACHE` under `target/tmp/` when isolating Metal's disk cache.
Match whether that cache is populated between comparisons.

The child pauses at each JSON checkpoint until the collector samples it:
before loading, after loading, after warmup, after short/full transcription,
after offload (or retained baseline), and after final close. Records include
operation elapsed time. Repeated full transcripts must match exactly and be
nonempty; failures return nonzero. The collector saves input hashes, command,
platform, commit/submodule identity, working-tree state, `metrics.jsonl`, and
native diagnostics. It does not persist audio or transcripts. The retained
baseline opens once; the offload run opens on every cycle.

Loading time excludes earlier process/device initialization. Add warmup time
to open time to estimate readiness after offload. OS sampling adds pauses
outside those operation timings. These are diagnostic measurements, not an
isolated latency benchmark.

Test worker admission, queueing, and configured timeouts separately with the
[PTT worker simulation](quality.md#ptt-worker-simulation). Follow the
[desktop smoke checks](quality.md#runtime-smoke-checks) for live input and insertion.

## Platform Measurements

### macOS

Build with `--features metal`, then test `--device cpu` and `--device gpu`
separately. The collector records RSS from `ps` and saves `vmmap -summary`
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

The collector reads `/proc/<pid>/smaps_rollup` and `status`, retaining RSS, PSS,
anonymous memory, file-backed PSS, private dirty pages, and swap in bytes. For
attribution, save `/proc/<pid>/smaps` at the paused checkpoints or attach a native
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
python scripts/profile_memory.py --output target/tmp/memory/windows-cuda --nvidia -- `
  target/parakit-windows-x86_64-cuda/profile-memory.exe `
  --model path/to/model.gguf --audio path/to/recording.wav `
  --device gpu --threads 8 --cycles 10
```

Adjust the helper path if `CARGO_TARGET_DIR` changes its output location.
For Vulkan, substitute its feature and bundle; for CPU, omit the accelerator
feature and use `--device cpu`. Repeat with `--keep-loaded` and a new output
directory for the resident baseline. These commands require native Windows
validation; the results below cover macOS only.

The collector calls `GetProcessMemoryInfo` to record working set, peak working
set, and private committed bytes. These measures are distinct; see Microsoft's
[PROCESS_MEMORY_COUNTERS_EX](https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters_ex).
Use Sysinternals VMMap for heap/mapped-file attribution. In Task Manager's
Details tab, add dedicated and shared GPU memory columns, or collect matching
GPU Process Memory performance counters during checkpoints. WDDM can make
`nvidia-smi` process memory unavailable; the collector records that explicitly.
Do not count shared GPU pages twice against host totals.

## Allocation Ownership And Operation Placement

At the pinned CrispASR revision, [`parakeet.cpp`](../../vendor/CrispASR/src/parakeet.cpp) owns lazy F32 CPU copies of
the predictor LSTM and joint-head weights (`pred_w`, `joint_w`), graph metadata,
and a ggml scheduler. The decoder runs manually on CPU even when the encoder
runs on GPU. GPU selection therefore does not eliminate CPU allocations.
`parakeet_free` frees scheduler, model buffer/context, both backend instances,
and the context containing those vectors.

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
A native patch needs a minimal reproducer, immutable revision, transcript
comparison, and backend validation.

## Native Results, 2026-09-30

Measured on an Apple M5 MacBook Air with 32 GiB unified memory, macOS 26.7,
Rust 1.98.1. The paired runs used the same Rust dev-profile example and existing
Metal-enabled bundled native library, eight threads, default Q8_0 model
(745,121,632 bytes), and unchanged warmup. CPU runs did not initialize Metal.
The baseline retained its session, matching the previous daemon lifetime; the
comparison dropped the same native session between dictations. Engine and
native inference code were unchanged.

Input identities:

- Model SHA256: `10f38dd9ce69ce555a413d9b4201ae5d93c2d7cadc91a285f4bfeeec6eee635a`.
- Juniper WAV: `local-scratch/Juniper_St_NE_5.wav`, 55.36 seconds;
  SHA256 `d79d7f729b192642b91aef5e1486b97c1cb7f4c34f6ce4a0d45199a9ff23ab5f`.
- CrispASR pin: `5f1bb858e803167f1b5fc1eb9a90ffdd1970f7ed`, unchanged.
- Full phase aggregates and native library identity:
  [memory-results-2026-09-30.json](memory-results-2026-09-30.json).

All numbers below are `ps` RSS in MiB. Ranges cover ten cycles.

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
addition to the manual CPU decoder. CUDA/Vulkan placement still needs the same
native check. The trace is in `target/tmp/offload-memory/metal-placement.err`.

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
was demonstrated, so this branch retains the dependency pin and warmup policy.
The Windows/Linux report of 3+ GiB host memory remains open pending native
allocation/placement measurements; it is not explained away by GPU selection.

Desktop testing did expose a separate ownership defect in parakit's stop path:
IPC called process exit while the worker still owned a loaded session. Metal's
static device teardown asserted because live residency-set buffers remained.
Stop now requests worker exit and waits for the session destructor before
native static teardown. The existing bounded stop budget remains; a timeout
uses immediate process termination instead of racing native destructors. This
fix is in parakit, with no CrispASR patch or dependency update.

Timing medians from the ten-cycle offload runs were 123 ms CPU / 93 ms Metal
for open, 187 ms / 745 ms for warmup, and 328 ms / 132 ms for the two-second
excerpt. Full-clip resident/offload medians were 7.62/8.81 seconds CPU and
1.56/2.19 seconds Metal. Native toolchain compilation overlapped some runs and
the machine was thermally active, so these timings cannot establish a throughput
regression. The cleanest Metal full-clip observations in both modes were about
1.55 seconds. Process-cold Metal initialization can add several seconds before
the open timer; reopening within the same daemon reuses that device state.

All repeated full transcripts matched exactly within each run. The production
Metal worker's raw transcript also matched the unchanged pre-implementation
binary exactly, including after reload. Both CPU and Metal workers passed two
real one-minute timeout/offload cycles with immediate queued audio on the next
press. The CPU timed run used `--quiet` and emitted zero bytes on both streams.
Local raw artifacts are under ignored `target/tmp/offload-memory/` in directories
named `resident-metal`, `offload-metal`, `resident-cpu-corrected`, `offload-cpu`,
and `offload-metal-extended`. The earlier `resident-cpu` trial initialized a GPU
probe unnecessarily and is excluded from the report.

Validation status:

| Surface | Result |
| --- | --- |
| macOS CPU and Metal allocation/session cycles | Passed ten each; thirty additional Metal cycles |
| Production worker configured timeout, automatic reload, transcript parity | Passed CPU and Metal |
| Deterministic activity, failure/recovery, status/history, configuration and cleanup tests | Passed |
| macOS permissions, microphone readiness, guarded insertion (`doctor --deep`) | Passed |
| Fresh native Metal build, operation placement, disabled offload, missing-model and required-silence failures | Passed |
| Native macOS PTT, insertion, first post-idle reload, missing-model recovery, status/history, stop/restart | Passed with an isolated daemon and native hotkey/microphone/input APIs; sounds enabled without backend errors |
| Audible cue verification and actual system sleep/wake | Pending human listening and native sleep/wake checks |
| Windows CPU/CUDA/Vulkan | Pending native measurements and desktop validation |
| Linux CPU/CUDA/Vulkan | Pending native measurements and desktop validation |

The full Rust validation loop passed. Raw all-features configuration encountered
the expected missing CUDA Toolkit on macOS; the documented `CRISPASR_LIB_DIR`
fallback passed Rust all-features checking. Known vendored CrispASR deprecation
warnings remain. Rust 1.98.1 also reports the existing `block 0.1.6` dependency's
uninhabited-static future-incompatibility warning; dependency migration is
outside this change. Project clippy checks pass with warnings denied.
