# CrispASR v0.8.42 upgrade

The branch pins [CrispASR v0.8.42](https://github.com/CrispStrobe/CrispASR/releases/tag/v0.8.42)
at `3be8bcb4390b8080dc73f807b7abb34dc0aca9c3`. Ordinary transcription remains
the shipping path. The reviewed lower-memory controls are now startup defaults;
explicit environment overrides are preserved. Linux acceptance remains pending
on the recognition findings below; the measured memory exception and Vulkan
cold-start trade-off have been accepted.

## Why this release and what changes at runtime

The upgrade brings Parakit's native runtime, ggml and Rust bindings to one
tagged release. The reasons extend beyond memory measurements:

- **Session behavior:** Ordinary non-Japanese Parakeet transcription now uses
  the shared orchestrator, including memory-aware strategy selection and
  post-decode gap repair. This changes recognition behavior even though
  Parakit keeps calling ordinary `transcribe`. The four added words in the
  125-second comparison demonstrate the new gap-repair route; their correctness
  still needs audio review. See the
  [pinned session dispatch](https://github.com/CrispStrobe/CrispASR/blob/v0.8.42/src/crispasr_c_api.cpp#L6201-L6252).
- **Allocation handling:** The encoder checks its estimated allocation against
  available physical memory before constructing an unsafe full-attention graph.
  This is separate from strategy selection and does not establish recovery
  from every GPU allocation failure. See the
  [allocation guard](https://github.com/CrispStrobe/CrispASR/blob/v0.8.42/src/parakeet.cpp#L1008-L1030).
- **Model loading:** The newer loader uses mmap by default, changing loading
  and host residency accounting. Repeated offload/reload was therefore tested
  explicitly. This report does not attribute a mapping leak to the old pin.
  See the
  [loader default](https://github.com/CrispStrobe/CrispASR/blob/v0.8.42/src/core/gguf_loader.cpp#L499-L504).
- **Backend compatibility:** The ggml update includes the restored Metal
  matrix-multiplication kernel needed to avoid an abort on affected Apple GPUs.
  This is an upstream fix to validate on the laptop, not a locally demonstrated
  Mac result. See the
  [v0.8.42 release](https://github.com/CrispStrobe/CrispASR/releases/tag/v0.8.42).
- **Model correctness:** The release includes the corrected default for missing
  Parakeet v3 encoder-scaling metadata. The unchanged daily Q8 model already
  explicitly disables that scaling, so this fix does not establish an accuracy
  improvement for this artifact. See the
  [metadata fallback](https://github.com/CrispStrobe/CrispASR/blob/v0.8.42/src/parakeet.cpp#L373-L386).

Parakit's integration changes below also remove competing native compilation
and replace its private ggml layout assumptions. Those changes make the chosen
runtime and device-query ABI consistent across the native and Rust components.

### Long-audio behavior shipped

Ordinary transcription retains a default single-pass duration cap of 300
seconds, subject to the orchestrator's memory policy. The dependency update
therefore brings its orchestration and gap repair without forcing every
dictation into 30-second windows. See the
[strategy resolver](https://github.com/CrispStrobe/CrispASR/blob/v0.8.42/src/parakeet_orchestrate.cpp#L525-L594).

The separate bounded-default experiment requests approximately 30-second
windows through `transcribe_chunked(pcm, 0, -1)`. It was evaluated on 29/31-second
boundaries and the 55/125/265-second corpus on CPU, CUDA and Vulkan. It remains
unshipped because of the recognition changes and 31-second latency regressions
documented below. The supplied usage durations determined test priority;
long-audio evaluation was also completed.

## Integration changes

Parakit owns native compilation through concrete Cargo `links` overrides for
Linux GNU x86_64/aarch64, Apple arm64, and Windows x86_64 MSVC/GNU. Its parent
build script retains external-library discovery, backend/BLAS selection,
C++ runtime linkage, RPATHs, quantizer installation and Windows staging.
Run Cargo from the repository root so its configuration is loaded. See
[build instructions](../build.md) for the source and library overrides.

The exact nested pins are ggml `c36dab89b662838f0f5d4826c399198c0b90bbfc` and
c2pa-audio `e40329b83f16f67bb5ddc7bb13ae18de0a9376fc`. The two Rust path crates
and lockfile move with the native sources. The session adapter retains ABI
version 2, checked sample counts, explicit CPU/GPU selection and RAII cleanup.
Segment no-speech values come from the native accessor; unavailable test values
remain `-1.0`.

A small C++ adapter replaces Rust's private ggml layout mirrors. It compiles
against the selected native headers and preserves PCI matching, conservative
Windows NVIDIA free-memory readings and Metal wrapped-value handling.
Optional Opus/AMR decoders are disabled for Parakit's PCM input. Mel BLAS follows
Parakit's BLAS selection. Parakit disables the extra pointwise-Q8 and fused-QKV
weight copies by default with `CRISPASR_FC_PW_Q8=0` and
`CRISPASR_FC_FUSED_QKV=0`. Other upstream compute defaults are retained.

The daemon and `transcribe-file` initialize these defaults before starting
threads or opening a model. Library users can supply the variables when
launching their process or call the unsafe startup initializer before threads
and native initialization. `Engine::open` does not mutate these variables from
an inference worker. No CLI flag or configuration key is added.

Worker ownership, startup/reload warmup shapes, idle offload and the 1 GiB reload
allowance are unchanged. README, CI and Parakit's version are unchanged.

## Inputs and method

Ignored `target/tmp/crispasr-upgrade/` holds source snapshots, separate release
builds and matching libraries, frozen audio, runners, logs and raw measurements.
The compared states are original v0.6.6, v0.6.6 with one native builder,
ordinary v0.8.42, and an isolated bounded-default v0.8.42 experiment. The bounded
snapshot changes both entry points to `transcribe_chunked(pcm, 0, -1)` and uses
the ordinary candidate's matching native library. It adds no public setting.

The unchanged copied installed Q8 model is 745,121,632 bytes and contains
`parakeet.xscaling = false`. No fixture hashes, conversion or requantization
were introduced. The missing-metadata upstream scaling fix therefore does not
establish a quality improvement for this model. The new runtime can still
transform weights in memory, as described below.

The supplied usage EDA reports a median capture of 8.24 seconds, P75 of 16.56
seconds and P90 of 29.28 seconds. Captures of at least 30 seconds are 9.5% of
entries. Everyday 2/5/8.2/15/25-second cases take priority, with 29/31-second
boundaries, subsecond speech and silence also included. Long coverage inputs
are two real recordings of 55.36 and 125.48 seconds, plus a clearly labeled
265-second repeated-audio stress input. The latter is not an independent
natural dictation.

The supplied 55-second reference is frozen locally. No audio-checked human
reference was available for the 125-second recording; changed wording on that
input requires audio review. Raw text is compared before cleanup, and voiced
fixtures must produce expected content. Capitalization and punctuation alone
do not fail quality.

Measurements use eight inference threads with fixed OpenMP/OpenBLAS/MKL thread
settings, the same compiler, release profile and warmup policy. Each short clip
has three fresh launches and five measured warm repetitions after first
inference. Long clips have three fresh launches and one inference per launch.
Reported warm medians weight independent launches equally. Open, startup
warmup, first inference, warm inference and production reload are separate.

The Linux host has a Ryzen 7 7700X and an RTX 5090. All compared builds use
Rust 1.98, release optimization, OpenBLAS, native CPU code, OpenMP and CPU
repacking. CPU-only, CUDA-only and Vulkan-only builds resolve their own
matching libraries. CUDA uses toolkit 12.9 and native architecture selection.
The old runtime defaults `GGML_LLAMAFILE` off; v0.8.42 defaults it on. That
upstream change is retained. Candidate mel BLAS is on with the selected
OpenBLAS policy; Opus/AMR and dynamic backend plugins are off.

Initial clean original builds show both sys and parent native compilation;
their binaries resolve the parent libraries. Clean ownership/candidate builds
show one parent native compilation and no sys CMake fallback. Subsequent
original probe rebuilds run from the repository root with its new Cargo
override; the initial verbose build logs preserve the unchanged-build evidence.

Each test has isolated configuration, cache, runtime and GPU shader-cache
directories. The OS file cache is not flushed; "cold" below refers to a fresh
process and shader cache, not uncached storage. The installed daemon stays running. Read-only status monitoring
stops the test child on recording, inference, loading, count changes or lost
monitoring. Builds are paused or finished during timing windows; functional
smokes performed during compilation are excluded from performance results.

Host RSS high-water values come from fresh processes. RSS, anonymous/file
residency and PSS are recorded separately. GPU values are sampled per-process
allocations, not exact allocator peaks. A reproducible latency increase over
10% blocks acceptance. Comparable memory increases must exceed both 10% and
128 MiB to fail; unresolved accounting is not a pass.

Checkpoint analysis selects interior samples from the raw stage groups rather
than their last sample, which can cross a destruction boundary. Missing or
unstable windows remain visible. Component residency changes explain mmap
behavior; memory gates use comparable total RSS/PSS, host HWM and GPU allocation.
Sampling can miss short GPU peaks, especially the fastest CUDA inferences.
For zero-repeat long runs, the two adjacent post-inference holds contain no
intervening inference or engine teardown. Analysis validates that event order,
combines them into one retained-memory interval, requires three samples,
discards its first and last samples and takes the interior median. This resolves
the sparse constituent windows without counting them as independent checkpoints.

## Initial 8.2-second results

These are three-launch pilot results, not full-corpus acceptance. All raw
transcripts agree across the four states on this clip.

| CPU state | First inference | Warm inference | Host HWM | Post-inference RSS |
|---|---:|---:|---:|---:|
| Original v0.6.6 | 911 ms | 888 ms | 1,433 MiB | 912 MiB |
| One-builder v0.6.6 | 929 ms | 903 ms | 1,433 MiB | 912 MiB |
| v0.8.42 ordinary | 613 ms | 530 ms | 1,082 MiB | 1,082 MiB |
| v0.8.42 bounded | 634 ms | 539 ms | 1,082 MiB | 1,082 MiB |

The ordinary CPU candidate is faster and has a lower peak, but retained PSS
increases from 907 to 1,077 MiB. This crosses the agreed memory gate. File-backed
PSS increases while anonymous PSS decreases; the total increase is corroborated
by RSS and is not waived as an mmap accounting difference.

| Vulkan state | First inference | Warm inference | Open + startup warmup | Post-inference GPU allocation |
|---|---:|---:|---:|---:|
| Original v0.6.6 | 1,073 ms | 289 ms | 2,403 ms | 837 MiB |
| One-builder v0.6.6 | 1,042 ms | 285 ms | 2,342 ms | 837 MiB |
| v0.8.42 ordinary | 512 ms | 108 ms | 3,000 ms | 991 MiB |
| v0.8.42 bounded | 501 ms | 97 ms | 3,017 ms | 991 MiB |

Vulkan inference improves, but upstream defaults exceed the memory gate by
153 MiB of GPU allocations and the readiness latency gate by about 25%.
Host HWM is approximately unchanged. These results use isolated fresh shader
caches; they do not establish performance with a previously populated cache.

| CUDA state | First inference | Warm inference | Open + startup warmup | Post-inference GPU allocation |
|---|---:|---:|---:|---:|
| Original v0.6.6 | 287 ms | 284 ms | 449 ms | 1,412 MiB |
| One-builder v0.6.6 | 292 ms | 284 ms | 440 ms | 1,412 MiB |
| v0.8.42 ordinary | 19 ms | 19 ms | 655 ms | 1,604 MiB |
| v0.8.42 bounded | 18 ms | 16 ms | 644 ms | 1,604 MiB |

CUDA defaults also fail retained memory and readiness gates, despite faster
inference. Host HWM changes from approximately 981 to 989 MiB. The 192 MiB
GPU allocation increase includes allocator behavior beyond the nominal
153 MiB transformed-weight payload.

### Retained-buffer diagnosis

The candidate adds two default weight transformations in
`src/core/fastconformer.h`, called by its Parakeet loader: pointwise F16-to-Q8
copies and fused Q/K/V copies. Header-only inspection of the unchanged model
establishes 76.5 MiB for each group, or 153 MiB together. Original weight storage
remains allocated or mapped. The buffers use the selected CPU/GPU backend and
live until session destruction. This is additional retained storage, not a leak.

In a separate CPU diagnostic, setting the existing upstream environment
variables `CRISPASR_FC_PW_Q8=0` and `CRISPASR_FC_FUSED_QKV=0` reduced retained RSS
to 925 MiB, PSS to 920 MiB and HWM to 925 MiB. Warm inference was 535 ms and
open plus startup warmup was 173 ms. Thus the diagnostic recovered about
157 MiB without a material warm-latency penalty on this clip. This configuration
was approved for evaluation and then adopted after review of the everyday
latency/memory results and the Vulkan cold-start trade-off.

The same diagnostic configuration on Vulkan returns retained GPU allocation
to 838 MiB and warm inference to 101 ms. Cold readiness is 2,732 ms, still
approximately 14% above the original pilot. The broader repeats below
establish the remaining fresh-process cost.

### Lower-memory everyday clips

With both upstream controls set to zero, the paired three-launch comparison
has completed 2/5/8.2/15/25/29/31-second clips on all Linux backends. Each cell below is
old-runtime to candidate warm latency, in milliseconds.

| Clip | CPU | CUDA | Vulkan |
|---|---:|---:|---:|
| 2 seconds | 237 → 177 | 93 → 9 | 99 → 55 |
| 5 seconds | 535 → 323 | 162 → 17 | 163 → 75 |
| 8.2 seconds | 889 → 542 | 285 → 23 | 283 → 100 |
| 15 seconds | 1,611 → 923 | 484 → 24 | 465 → 141 |
| 25 seconds | 2,767 → 1,593 | 866 → 38 | 800 → 212 |
| 29 seconds | 3,378 → 1,960 | 1,090 → 50 | 1,030 → 247 |
| 31 seconds | 3,653 → 2,152 | 1,212 → 55 | 1,129 → 255 |

First inference also passes the latency gate on each of these clips. Retained
CPU RSS is approximately 11–16 MiB higher, CUDA allocation 34–36 MiB higher,
and Vulkan allocation differs by less than 1 MiB. Comparable total host/GPU
memory checkpoints and peaks are within the agreed
memory gate. CPU and CUDA readiness improve with this configuration.

Vulkan cold readiness consistently rises from approximately 2.30–2.31 seconds
to 2.74–2.82 seconds, a 19–22% regression across these independent launches.
This reproduces the earlier readiness failure rather than resolving it.
The user accepted this fresh-process Vulkan trade-off. Production reloads are
evaluated separately; populated-cache startup performance is not established.

The 2-second candidate recognizes `Um so` where the baseline returns `Um`;
the supplied full-memo reference begins with those words. Other raw text on
the 5/8.2/15/25-second clips agrees across versions and backends. This is limited corpus
evidence, not a general accuracy improvement claim.

At 29 seconds, CPU/CUDA change `going to` to `gonna`; at 31 seconds, all three
backends change `for over` to `forever`. Both candidate forms match the supplied
full-memo reference. No new omission or boundary repetition appears in these
ordinary-transcription outputs. Cut boundaries still lack independently
audio-checked short references.

### Silence and subsecond input

All three launches and five warm repetitions return empty text for one second
of silence on every backend, in both versions. First/warm inference and total
memory remain within the gates. A CPU synthetic warmup-only timing is noisy
and higher in this batch, while total readiness is still faster; this phase
is kept separate from inference and end-to-end readiness.

The 0.5-second speech slice is an inadequate quality fixture: it cuts through
speech and both versions produce unrelated mixed-language text. CPU changes
`Ham keine Autom.` to `I'm keine Autom.`; CUDA returns `I'm keine Autom.` in
both versions, and Vulkan returns truncated variants. Nonempty output does
not establish successful recognition here. Timing and memory are measured,
but subsecond recognition remains unresolved without a complete utterance
and an audio-checked reference. The frozen slice is retained rather than
silently replaced after seeing its outputs.

## Bounded-default experiment

The isolated `transcribe_chunked(pcm, 0, -1)` route does not improve the
31-second boundary case. Ordinary v0.8.42 ends the relevant phrase with
`forever`, matching the supplied full-memo reference. Bounded mode instead
returns `for a virtual` on every backend, consistently across launches and
repetitions. The cut lacks an independently checked short reference, but this
new wording is sufficient reason to withhold adoption.

| 31-second warm inference | Ordinary candidate | Bounded candidate |
|---|---:|---:|
| CPU | 2,152 ms | 2,467 ms |
| CUDA | 55 ms | 59 ms |
| Vulkan | 255 ms | 367 ms |

At 29 seconds, warm latency changes by +6% on CPU, -7% on CUDA and less than
1% on Vulkan. Retained memory is nearly unchanged at 29 seconds, with small
GPU reductions at 31 seconds. These measurements use the same lower-memory
controls as the ordinary candidate. Long-input results are evaluated separately;
no bounded route is added to the shipping code.

## Long-input comparison

These are first-inference medians from three independent launches, with the
same lower-memory controls. Long cases have no measured warm repeats.

| Backend | Input | Original | Ordinary candidate | Bounded experiment |
|---|---|---:|---:|---:|
| CPU | 55 seconds | 6,730 ms | 4,327 ms | 3,850 ms |
| CPU | 125 seconds | 16,824 ms | 12,195 ms | 8,595 ms |
| CPU | 265 seconds, repeated | 41,716 ms | 31,776 ms | 16,000 ms |
| CUDA | 55 seconds | 1,926 ms | 91 ms | 121 ms |
| CUDA | 125 seconds | 4,692 ms | 223 ms | 239 ms |
| CUDA | 265 seconds, repeated | 12,506 ms | 553 ms | 507 ms |
| Vulkan | 55 seconds | 2,447 ms | 1,237 ms | 1,402 ms |
| Vulkan | 125 seconds | 4,877 ms | 2,460 ms | 2,257 ms |
| Vulkan | 265 seconds, repeated | 9,497 ms | 4,044 ms | 3,657 ms |

Ordinary transcription passes long-input inference latency, host high-water,
sampled GPU peak and retained post-inference total-memory gates on every backend.
The corrected analysis has three launches for every metric and no unresolved
checkpoint groups. CUDA's post-offload 125-second endpoint remains a separate
failure: RSS rises from 533 to 676 MiB and PSS from 485 to 629 MiB, exceeding
both 10% and 128 MiB. The increase is predominantly anonymous host memory;
residual GPU allocation differs by only 2 MiB. A corroborating real-worker
comparison after the normal one-minute idle timeout gives offloaded RSS of
599 versus 691 MiB and PSS of 550 versus 643 MiB. That pair stays below the
128 MiB absolute gate, but does not negate the three-launch standalone failure.
The settled idle windows are flat in both worker runs. The exact owner of the
retained anonymous pages is not established; these observations do not prove
leak freedom. The user accepted this narrow standalone post-offload exception
on 2026-10-11. The measured increase remains documented; other memory gates
and recognition requirements are unchanged.

| 265-second peak memory, original → ordinary → bounded | Host HWM | Sampled GPU allocation peak |
|---|---:|---:|
| CPU | 4,466 → 4,504 → 1,296 MiB | — |
| CUDA | 995 → 1,004 → 1,004 MiB | 4,902 → 4,934 → 1,782 MiB |
| Vulkan | 889 → 897 → 897 MiB | 4,319 → 4,313 → 1,190 MiB |

Ordinary raw output matches the baseline on the 55- and 265-second inputs
across all backends. At 125 seconds, every backend adds `think that else. Uh`
between the discussion of pilot studies and the next task. The native log
identifies four words recovered by gap fill; that description does not prove
the added wording is correct. Audio review remains required.

Bounded mode substantially reduces the 265-second workspace, but its raw text
also changes on all three long inputs. The 55-second result includes
`order` → `before`, in addition to the 31-second boundary problem. These
quality findings outweigh its stress-input memory benefit for adoption here.

## Production offload and reload

The headless daemon simulation uses the real worker, immediate PTT release
during reload, unchanged one-minute idle policy and three reload transitions.
Four WAV inferences and four offloads occur in one process. GPU runs request
automatic device selection; both GPU backends select the NVIDIA GPU.
Memory analysis uses the middle half of each post-inference/offloaded interval
to avoid destruction/reload boundaries. This test checks repeat equality within
a process and does not replace the cross-version reference comparison.

The lower-memory Vulkan candidate passes matched total-memory gates with
stable post-inference residency over the three reloaded cycles. Retained GPU
allocation is 837.22 MiB for the baseline and 837.66 MiB for the candidate.
Both retain 14.09 MiB after offload. Reloaded post-inference host RSS is
338.70 versus 353.14 MiB. Candidate offloaded RSS ranges from 268 to 323 MiB,
without continuing growth; baseline reloaded offloaded RSS is 313 MiB.

| Vulkan transition | Baseline open + warmup | Candidate open + warmup |
|---|---:|---:|
| First reload | 569 ms | 495 ms |
| Second reload | 230 ms | 202 ms |
| Third reload | 257 ms | 179 ms |

These sums use rounded worker log timings and exclude preflight/queue time;
they are not end-to-end readiness measurements. The first reload introduces
the one-second probe shape. Pipeline caches persist in the Vulkan device after
session destruction. The single automatic-mode worker startup logs 2,519
versus 2,649 ms, but one launch does not supersede the reproducible fresh-process
probe readiness gate. Auto and explicit-GPU Parakeet open parameters match.

CPU also passes the matched memory gates and within-process repeat check.
Its three logged reload open-plus-warmup sums fall from 487/488/463 ms to
194/170/172 ms. Reloaded post-inference RSS spans 937–971 MiB for the baseline
and 958–965 MiB for the candidate, settling by the final cycles. Offloaded
RSS fluctuates at 60–122 versus 47–102 MiB, without continuing growth.

CUDA passes the matched total-memory gates and repeated-transcript check.
Logged reload open-plus-warmup sums fall from 195/168/169 ms to 146/116/120 ms.
Post-inference GPU allocation stays at 1,412 versus 1,444 MiB across all four
inferences; reloaded offloaded allocation stays at 568 MiB in both versions.
Reloaded post-inference RSS settles at 630 versus 668 MiB, and offloaded RSS
at 630 versus 668 MiB. Neither shows continuing growth by the final cycles.
The single worker startup logs 459 versus 527 ms; this is noisier than the
three-launch comparisons. Repeating automatic-mode startup gives median logged
open-plus-warmup sums of 463 versus 399 ms on CUDA, and 2,305 versus 2,706 ms
on Vulkan. The latter is consistent with the accepted fresh-process trade-off.

## Compatibility checks

All three Rust checkpoints passed formatting, strict rustdoc, workspace/all-target
checking, tests, strict Clippy, build and all-features checking. The final loop
passed 414 tests with three ignored. Existing vendor deprecation warnings remain.
The old-pin all-features check compiled native backends; the candidate Rust loops
used their matching prebuilt native CPU library, with independent CPU/CUDA/Vulkan
release builds providing native validation.

The unchanged lockfile failed Rust 1.87 because image 0.25.10 requires 1.88.
With approval, image is locked to 0.25.9 with only its necessary moxcms, tiff and
zune dependency changes. Rust 1.87 passes on the ordinary candidate, including
the startup-default change, with its matching native library; the declared
minimum is unchanged.

CPU worker smokes for both old and new runtimes passed on the real 55-second
memo and match the supplied reference apart from capitalization. Quiet output,
missing-model handling, isolated cache commands and ELF library resolution
passed. ELF uses RPATH rather than RUNPATH. Unix quantizer help executes using
the same sibling-library environment as the existing fetch helper; converter
source still accepts `--nemo` and `--output`. No model conversion was attempted.

Fresh CPU/CUDA/Vulkan release daemons also transcribed the 8.2-second fixture
with both weight-copy variables absent from their launch environment. They
selected CPU, CUDA0 and Vulkan1 respectively and produced the expected text.
The CPU `transcribe-file` example passed the same check. Setting both variables
explicitly to `1` preserved the transcript and increased CPU host HWM from
932 to 1,091 MiB, corroborating that explicit overrides remain effective.
These are functional checks, not additional latency samples. Main-daemon
monitoring remained clean and its dictation count stayed unchanged.

Lifecycle tests cover release during reload, queued capture, failed reload and
recovery, shutdown during loading/inference, and destruction outside the
activity lock. Low-memory guard fixtures exercise policy, not actual VRAM
exhaustion. Production repeated offload/reload measurements pass on each Linux
backend with the lower-memory controls set to zero.

## Native laptop checklist

macOS and Windows checks are best effort on Linux. The Windows shim branch
passes a Clang syntax check against the pinned headers; this does not validate
MSVC/GNU DLL imports. Source checks preserve ggml import declarations, Windows
staging/example exclusions and embedded Metal shaders. Native testing remains:

- Build CPU and the laptop's accelerator backend; verify selected devices and
  loaded libraries with `doctor` and real inference.
- Compare unchanged Q8 recognition and first/warm latency on everyday clips,
  including technical terms, then check long-input coverage separately.
- Measure loaded, post-short/post-long, idle and offloaded memory. Run three
  production reload transitions and confirm residency settles.
- Release during reload, queue audio, exercise reload recovery, and stop during
  loading/inference; verify captured audio and clean shutdown.
- Launch the extracted Windows bundle away from the source/build directories
  and verify its sibling DLLs. On Apple, verify Metal inference and embedded
  shader availability.

## Acceptance status

The implementation uses ordinary transcription and the approved lower-memory
defaults. Linux everyday first/warm inference and comparable memory gates pass,
as do three production offload/reload transitions on CPU, CUDA and Vulkan.
The user accepted the reproducible Vulkan fresh-process readiness regression
and the approximately 143 MiB CUDA standalone post-offload exception after the
125-second clip. The corroborating production worker stays below the absolute
memory gate. Bounded mode remains unshipped because of its recognition and
boundary findings.

Final acceptance remains paused on:

- Audio review of the four added words in the 125-second recording. Ordinary
  recognition agrees with the baseline on the other measured long inputs.
- A complete subsecond utterance and audio-checked reference; the frozen
  clipped-speech fixture cannot establish this quality requirement. Short cuts
  also lack independent audio-checked references.

Native macOS/Windows inference and packaging remain for laptop testing. Actual
allocation exhaustion and populated-cache startup were not evaluated. The
branch is prepared for review without pushing or merging; passing compilation
and the completed measurements do not constitute full recognition acceptance.
