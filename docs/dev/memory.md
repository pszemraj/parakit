# Model Memory and Idle Offload

The daemon owns one inference session on its worker thread. Idle offload drops that session, releasing its owned CPU and GPU allocations. It does not unload native libraries or reset process-wide device and driver state. See [idle model offload](../running.md#idle-model-offload) for the user-facing policy.

## Qwen GPU Reload Measurements

Linux measurements on 2026-10-14 used the Qwen 0.6B and 1.7B Q8_0 presets, CrispASR v0.8.42 (`3be8bcb`), an RTX 5090 with NVIDIA driver 595.91.07, and eight inference/BLAS/OpenMP threads. Rust front ends used the dev profile; native C++ was optimized. CUDA was built with `PARAKIT_CUDA_ARCHS=120-real`. Each backend ran the 55.36-second Juniper memo, 29/30/31-second chunk-boundary inputs, silence, repeated inference, and idle offload/reload. Startup and reload used the embedded 11-second JFK speech readiness probe at automatic language/512 tokens, restoring user options before dictation.

The resident-engine sequence was memo, short speech, 270 seconds of repeated JFK speech, then the same short speech; it ran twice with an idle offload after each cycle. Separate runs used automatic language/512 tokens and forced English/4096 tokens. The latter selects the maximum supported budget; it does not mean each chunk generated 4096 tokens. Repeated JFK is an artificial memory/context stress input, not a natural long-form quality recording.

Beyond the model file and retained offloaded GPU state, these runs needed up to about 1.6 GiB for 0.6B and 1.8 GiB for 1.7B. See [idle model offload](../running.md#idle-model-offload) for the reload allowance and device policy.

The following peaks are from the explicit GPU 4096-token lifecycle runs. Host high-water RSS includes startup and reload; it is not steady inference residency.

| Backend / Qwen | Peak VRAM | Offloaded VRAM | Beyond file and offloaded state | Margin within 2 GiB | Host high-water RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| CUDA / 0.6B | 3142 MiB | 568 MiB | 1614 MiB | 434 MiB | 1741 MiB |
| CUDA / 1.7B | 4838 MiB | 568 MiB | 1879 MiB | 169 MiB | 3183 MiB |
| Vulkan / 0.6B | 2620 MiB | 68 MiB | 1592 MiB | 456 MiB | 1459 MiB |
| Vulkan / 1.7B | 4326 MiB | 70 MiB | 1865 MiB | 183 MiB | 2898 MiB |

Compare the allocation rise from an offloaded session with the free-memory requirement. Retained driver state is already excluded from free GPU memory and should not be charged again. CUDA retained about 568 MiB in these Qwen runs; Vulkan retained about 68-70 MiB. Host RSS and GPU allocation describe different accounting domains and should not be added together. Session close did not return all host RSS to the OS, and reload could peak above steady loaded RSS.

The initial matrix sampled at 50 ms; the final worker pass used nominal 5 ms active and 50 ms idle intervals. These are observed lower bounds, not allocation ceilings. The 2 GiB allowance covers the tested Q8_0 inputs at budgets 512 and 4096 on this host, with a narrow margin for 1.7B. These Q8 measurements do not qualify other artifacts or native Metal/Windows behavior. No VRAM-filling pressure test was performed. The check does not reserve memory or prevent later allocation failure. Follow the [Linux GPU procedure](quality.md#linux-cudavulkan) to record commands, per-process stage measurements, and raw transcripts for another build.

An October 18 release-worker check with the shared native resampler and windowed attention measured Qwen 1.7B Q6_K on CUDA with forced English and a 4096-token budget. The memo output and options survived idle offload/reload. Process VRAM peaked at 4316 MiB against a 568 MiB offloaded baseline: 3748 MiB incremental, leaving about 221 MiB within the model-file-plus-2-GiB allowance. Sampled host RSS peaked at 2602 MiB and VmHWM at 2668 MiB. This qualifies that Q6 case only; the complete per-phase observations are retained under ignored `target/tmp/new-models/native-preprocessing/reload/runtime/`.

## Linux Reload Measurements

These Linux measurements describe the historical CrispASR v0.6.6 runtime; they do not characterize the current v0.8.42 runtime.

Three-cycle, eight-thread runs on 2026-10-05 measured the production policy on CPU, CUDA, and Vulkan. The table shows reload cycles 2-3; cycle 1 includes process and device initialization. Each backend row comes from one run, so its range is within-run variation rather than repeatability across runs. Host peaks are interval high-water RSS from Linux `VmHWM`; other host columns are RSS at checkpoints. GPU columns are sampled per-process framebuffer allocations. Offloaded endpoints were read 250 ms after session close.

All three run metadata files recorded Parakit revision `5066d9f`, CrispASR pin `5f1bb858e803167f1b5fc1eb9a90ffdd1970f7ed` (the former v0.6.6 pin; upstream later rewrote its history as `2f8dd068`, with identical native runtime sources), Rust 1.98.0 dev-profile builds, the 745,121,632-byte Q8_0 model, and the 5,314,638-byte Juniper WAV. The host was Ubuntu 24.04.5, Linux 6.17.0-29-generic, Ryzen 7 7700X, and RTX 5090 with NVIDIA driver 595.91.07. Runs were sequential without another inference workload.

| Backend | Reload host peak | Post-load host | Post-full host | Offloaded host | GPU after reload warmup | Post-full GPU | Offloaded GPU |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| CPU | 1.43-1.46 GiB | 0.74-0.76 GiB | 1.57 GiB | 0.14 GiB | unavailable | unavailable | unavailable |
| CUDA | 1.26-1.27 GiB | 0.57-0.58 GiB | 0.68 GiB | 0.58 GiB | 1312 MiB | 2054 MiB | 566 MiB |
| Vulkan | 0.88 GiB | 0.19 GiB | 0.28 GiB | 0.19 GiB | 746 MiB | 1491 MiB | 22 MiB |

The CPU reload peak is almost twice its post-load residency. CUDA and Vulkan also peak above their post-load host checkpoints. A model file's size or a loaded checkpoint therefore does not bound RAM needed to reopen the session. GPU allocation rises during warmup and long inference, then falls after close. These are per-process observations for this host and recording, not memory ceilings; longer audio can require larger workspaces.

## Allocation Ownership

The allocation details in this section describe the historical CrispASR v0.6.6 runtime, except where the current loader is identified explicitly.

At the v0.6.6 CrispASR revision measured above, `parakeet.cpp` owns lazy F32 CPU copies of predictor and joint-head weights, graph metadata, and a ggml scheduler. The decoder runs manually on CPU even when the encoder runs on GPU, so GPU inference still uses host memory. `parakeet_free` releases the scheduler, model buffer and context, backend instances, and the context containing those vectors.

Inference also allocates activation and decoder workspace that can grow with audio length and remain at its high-water capacity while a session is loaded. The v0.6.6 default GGUF loader copied weights into a backend buffer; `CRISPASR_GGUF_MMAP=1` opted into supported CPU and Metal mapping paths. CUDA, Vulkan, Metal, and their drivers can retain process-wide state or caches after a session closes. Residual process memory alone does not prove that the inference session remains live or identify an allocator leak.

The current v0.8.42 loader uses mmap by default, and `CRISPASR_GGUF_MMAP=0` opts out. The loader and current allocation accounting are documented in the [CrispASR upgrade report](crispasr-upgrade.md#why-this-release-and-what-changes-at-runtime). These historical measurements do not establish current GPU memory-pressure behavior.

## Reload Under GPU Memory Pressure

The following pressure observations are from the historical v0.6.6 runtime, not current v0.8.42 behavior.

On the RTX 5090 with CUDA, a 55-second dictation needed about 1.5 GB free beyond the retained context: 710 MiB of weights plus a 749 MiB compute buffer and scratch pool. With less than the 710 MiB of weights free, weight allocation failed and reload reported an error. Between that and about 1.5 GB free, reload succeeded, then ggml aborted the process on the first real dictation: the VMM pool's `cuMemCreate` in `ggml-cuda.cu`, or the unchecked `ggml_gallocr_reserve_n` result leading to a `GGML_ASSERT` in `ggml-backend.cpp`. Additional CUDA runs on 2026-10-10 measured 777 MiB of workspace for 55 seconds of audio, 3,641 MiB for 265 seconds, and 3,801 MiB for 277 seconds, beyond the 566 MiB retained context and approximately 711 MiB of model weights. Reload requires the model size plus 1 GiB free before it opens on a GPU (see [idle model offload](../running.md#idle-model-offload)), which covers dictations of about a minute. Sizing the allowance for the 270-second limit would move smaller GPUs to CPU after every offload. The check cannot reserve memory, and the compute buffer grows with dictation length, so a GPU filled after reload can still abort a long dictation.

On macOS, Metal shares unified memory with the system, so the reload check uses ggml's Metal budget as reported: `recommendedMaxWorkingSetSize`, which follows the GPU wired-memory limit, minus this process's Metal allocations. After an offload this is close to the wired limit, so the check only moves reload to CPU when that limit cannot hold the model and its workspace. Memory used by other programs is left to macOS paging rather than estimated. The [native macOS validation report](macos-validation-2026-10-07.md) predates the CrispASR v0.8.42 upgrade and records CPU/Metal offload measurements and the limits of the pressure checks.
