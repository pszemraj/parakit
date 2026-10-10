# Model Memory and Idle Offload

The daemon owns one inference session on its worker thread. Idle offload drops
that session, releasing its owned CPU and GPU allocations. It does not unload
native libraries or reset process-wide device and driver state. See
[idle model offload](../running.md#idle-model-offload) for the user-facing policy.

## Linux Reload Measurements

Three-cycle, eight-thread runs on 2026-10-05 measured the production policy on
CPU, CUDA, and Vulkan. The table shows reload cycles 2-3; cycle 1 includes
process and device initialization. Each backend row comes from one run, so its
range is within-run variation rather than repeatability across runs. Host peaks
are interval high-water RSS from Linux `VmHWM`; other host columns are RSS at
checkpoints. GPU columns are sampled per-process framebuffer allocations.
Offloaded endpoints were read 250 ms after session close.

All three run metadata files recorded Parakit revision `5066d9f`, CrispASR pin
`5f1bb858e803167f1b5fc1eb9a90ffdd1970f7ed`, Rust 1.98.0 dev-profile
builds, the 745,121,632-byte Q8_0 model, and the 5,314,638-byte Juniper WAV.
The host was Ubuntu 24.04.5, Linux 6.17.0-29-generic, Ryzen 7 7700X, and
RTX 5090 with NVIDIA driver 595.91.07. Runs were sequential without another
inference workload.

| Backend | Reload host peak | Post-load host | Post-full host | Offloaded host | GPU after reload warmup | Post-full GPU | Offloaded GPU |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| CPU | 1.43-1.46 GiB | 0.74-0.76 GiB | 1.57 GiB | 0.14 GiB | unavailable | unavailable | unavailable |
| CUDA | 1.26-1.27 GiB | 0.57-0.58 GiB | 0.68 GiB | 0.58 GiB | 1312 MiB | 2054 MiB | 566 MiB |
| Vulkan | 0.88 GiB | 0.19 GiB | 0.28 GiB | 0.19 GiB | 746 MiB | 1491 MiB | 22 MiB |

The CPU reload peak is almost twice its post-load residency. CUDA and Vulkan
also peak above their post-load host checkpoints. A model file's size or a
loaded checkpoint therefore does not bound RAM needed to reopen the session.
GPU allocation rises during warmup and long inference, then falls after close.
These are per-process observations for this host and recording, not memory
ceilings; longer audio can require larger workspaces. Host and GPU counters
describe different accounting domains and should not be added together.

## Allocation Ownership

At the pinned CrispASR revision, `parakeet.cpp` owns lazy F32 CPU copies of
predictor and joint-head weights, graph metadata, and a ggml scheduler. The
decoder runs manually on CPU even when the encoder runs on GPU, so GPU
inference still uses host memory. `parakeet_free` releases the scheduler,
model buffer and context, backend instances, and the context containing those
vectors.

Inference also allocates activation and decoder workspace that can grow with
audio length and remain at its high-water capacity while a session is loaded.
The default GGUF loader copies weights into a backend buffer;
`CRISPASR_GGUF_MMAP=1` opts into supported CPU and Metal mapping paths.
CUDA, Vulkan, Metal, and their drivers can retain process-wide state or caches
after a session closes. Residual process memory alone does not prove that the
inference session remains live or identify an allocator leak.

## Reload Under GPU Memory Pressure

On the RTX 5090 with CUDA, a 55-second dictation needed about 1.5 GB free beyond the retained context: 710 MiB of weights plus a 749 MiB compute buffer and scratch pool. With less than the 710 MiB of weights free, weight allocation failed and reload reported an error. Between that and about 1.5 GB free, reload succeeded, then ggml aborted the process on the first real dictation: the VMM pool's `cuMemCreate` in `ggml-cuda.cu`, or the unchecked `ggml_gallocr_reserve_n` result leading to a `GGML_ASSERT` in `ggml-backend.cpp`. Additional CUDA runs on 2026-10-10 measured 777 MiB of workspace for 55 seconds of audio, 3,641 MiB for 265 seconds, and 3,801 MiB for 277 seconds, beyond the 566 MiB retained context and approximately 711 MiB of model weights. Reload requires the model size plus 1 GiB free before it opens on a GPU (see [idle model offload](../running.md#idle-model-offload)), which covers dictations of about a minute. Sizing the allowance for the 270-second limit would move smaller GPUs to CPU after every offload. The check cannot reserve memory, and the compute buffer grows with dictation length, so a GPU filled after reload can still abort a long dictation.

On macOS, Metal shares unified memory with the system, so the reload check
uses ggml's Metal budget as reported: `recommendedMaxWorkingSetSize`, which
follows the GPU wired-memory limit, minus this process's Metal allocations.
After an offload this is close to the wired limit, so the check only moves
reload to CPU when that limit cannot hold the model and its workspace. Memory
used by other programs is left to macOS paging rather than estimated. The
[native macOS validation report](macos-validation-2026-10-07.md) records
CPU/Metal offload measurements and the limits of the pressure checks.
