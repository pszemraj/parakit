# Model Memory and Idle Offload

See [idle model offload](../running.md#idle-model-offload) for residency, readiness, and device-selection policy.

## Allocation Ownership

The daemon worker owns the inference engine and its native session. Offloading removes the engine under the activity gate, then destroys it before processing the next queued reload. Recording can begin while destruction or reload is in progress; the coordinator retains that capture for the worker.

Session destruction releases owned weights, graph state, and backend buffers. Process allocators and GPU drivers can retain caches afterward. Residual process memory alone does not establish that a session remains loaded or identify a leak; check model residency through `parakit --verbose status` as well.

A model file's size does not bound runtime memory. Loading, readiness, and inference have different allocation patterns, and working space can grow with audio length and generation settings. GPU inference also uses host memory.

## GPU Reload Accounting

Bundled builds compare the preferred GPU's reported free memory with a backend-specific model-and-workspace requirement before reloading. The policy and allowances are documented under [idle model offload](../running.md#idle-model-offload); the implementation is in `src/daemon/engine_runtime.rs`.

This check does not reserve memory or bound later allocations. Other workloads can consume the remaining budget after the check, and a longer dictation can require more workspace. Keep startup, reload, and long-input checks separate.

### Metal Qwen Reload Accounting

Qwen's main model and shared audio tower each register a full-file Metal buffer view with the default mmap loader. The audio tower filters tensor names, but its Metal mapping covers the entire GGUF. The reload requirement counts both views in the same accounting domain as Metal's free-memory query.

Host RSS and Metal allocation accounting overlap on unified-memory systems. Record them separately rather than adding them together. Qualify artifacts, environment overrides, and hardware with the [native model procedure](model-validation.md#qwen-options-long-input-and-reload).

## Measuring Memory

Sample host RSS and per-process GPU allocation through startup, readiness, short and long inference, idle offload, and reload. Compare the allocation rise from the offloaded baseline with the reload requirement: retained driver allocations are already excluded from reported free GPU memory and should not be charged again. Sampled peaks are lower bounds, so retain the sampling interval alongside the measurements.

Record the model artifact, input audio, language and token settings, native backend, build configuration, and runtime revision with each run. Use the [Linux GPU procedure](quality.md#linux-cudavulkan) or the [native model checklist](model-validation.md), and keep generated measurements and command logs under ignored `target/tmp/`.
