# Model Memory and Idle Offload

Measure model memory separately from [residency and readiness](../running.md#idle-model-offload).

## Allocation Ownership

Session destruction releases owned weights, graph state, and backend buffers. Process allocators and GPU drivers may keep their caches. Check `parakit --verbose status` and compare memory before and after destruction to distinguish model residency from retained allocations. [Session lifecycle](architecture.md#state-machine)

Loading, readiness, and inference have different allocation patterns. Working space can grow with audio length and generation settings, and GPU inference also uses host memory.

## GPU Reload Accounting

Interpret measurements alongside the [GPU reload preflight](../running.md#idle-model-offload) in [`engine_runtime.rs`](../../src/daemon/engine_runtime.rs).

### Metal Qwen Reload Accounting

With the default mmap loader, both Qwen's main model and its audio tower register a full-file Metal buffer view. The reload requirement counts both views against Metal's reported free memory.

Report host RSS and Metal allocations separately because they overlap on unified-memory systems. [Native model validation](model-validation.md#qwen-options-long-input-and-reload)

## Measuring Memory

Sample host RSS and per-process GPU allocation through startup, readiness, short and long inference, idle offload, and reload. Compare the rise from the offloaded baseline with the reload requirement because reported free GPU memory already excludes retained driver allocations. Record the sampling interval. Sampled peaks are lower bounds.

Record the model artifact, input audio, language and token settings, native backend, build configuration, and runtime revision with each run. Use the [Linux GPU procedure](quality.md#linux-cudavulkan) or the [native model checklist](model-validation.md), and keep generated measurements and command logs under ignored `target/tmp/`.
