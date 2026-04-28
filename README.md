# parakit

A push-to-talk dictation daemon built on [CrispASR](https://github.com/CrispStrobe/CrispASR) +
NVIDIA's [Parakeet-TDT-0.6B-v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3) ASR model.

Hold **Ctrl+Space**, talk, release. The transcribed text is typed at the
cursor — no clipboard touched. Optional regex-based cleaning of speech
disfluencies ("um", "uh", repeated words, partial-word stutters, leading
filler "so", "well", "like") runs by default; every rule is named and can
be disabled individually.

```
$ parakit -m parakeet-tdt-0.6b-v3.gguf
parakit
  model:    parakeet-tdt-0.6b-v3.gguf
  mode:     Batch
  cleaning: on (56 rules)
  sounds:   on
  audio:    48000 Hz hardware (resampling), 16000 Hz target
Ready — hold Ctrl+Space to dictate. Ctrl+C in this terminal to exit.
🎙️  listening...
🤔  transcribing (3.42s of audio, 3.61s wall)...
Raw:    So, um, the the the cat sat on the mat.
Clean:  the cat sat on the mat.  (212ms)
```

---

## Status

- **Linux X11**: works.
- **Linux Wayland**: does not work. `rdev::grab` cannot intercept Ctrl+Space
  on most Wayland compositors (GNOME Mutter, KDE KWin) because Wayland blocks
  global hotkey grabs from regular clients by design. Use X11.
- **Windows**: works.
- **macOS**: works in principle. Requires Accessibility + Input Monitoring
  permissions for both the terminal and the binary. Not the recommended
  platform — there are better-integrated Mac dictation tools.

---

## Quickstart

```bash
git clone --recurse-submodules <this repo> parakit
cd parakit

# Pick your GPU backend (or omit for CPU):
cargo build --release --features cuda      # NVIDIA
cargo build --release --features metal     # macOS
cargo build --release --features vulkan    # cross-platform GPU
cargo build --release                      # CPU only

# Get a model (one-time):
huggingface-cli download CrispStrobe/parakeet-tdt-0.6b-v3-gguf \
  --include "*Q5_K_M.gguf" --local-dir ./models

# Run it:
./target/release/parakit -m models/parakeet-tdt-0.6b-v3-Q5_K_M.gguf
```

That's it. The first `cargo build` takes 5–10 minutes because it compiles
all of ggml + CrispASR; subsequent builds are cached and incremental.

---

## How the build works

`parakit` uses CrispASR's Rust crates, which are not published to crates.io.
We resolve this by vendoring CrispASR as a git submodule at `vendor/CrispASR`
and depending on the workspace member crates by path:

```toml
crispasr = { path = "vendor/CrispASR/crispasr", default-features = false }
```

This means the same submodule serves both purposes — the C library is
compiled from it (via cmake), and the Rust bindings are taken from it
directly. There's only one upstream version of truth.

The `bundled` feature (on by default) tells `build.rs` to compile CrispASR's
C library:

1. `cmake` crate handles cross-platform configuration (MSVC on Windows,
   Xcode/Make on macOS, Make/Ninja on Linux), parallelism, and incremental
   rebuilds.
2. Backend selection (`cuda` / `metal` / `vulkan`) maps to `-DGGML_CUDA=ON`
   etc. in the cmake invocation.
3. After cmake installs the library, `build.rs` creates a
   `libcrispasr → libwhisper` symlink (CrispASR's umbrella library is named
   `libwhisper`; `crispasr-sys` looks for `libcrispasr` by default).
4. `CMAKE_INSTALL_RPATH=$ORIGIN` is set so `libwhisper.so` finds its sibling
   `libggml*.so` files at runtime.
5. The parakit binary gets `-Wl,--disable-new-dtags` (Linux/BSD) so its own
   rpath becomes `DT_RPATH` (legacy, transitive) instead of `DT_RUNPATH`
   (modern, non-transitive). Without this, transitive shared-lib resolution
   fails at load time even when the binary's rpath is set correctly.

### Build flags and overrides

| Cargo invocation | What happens |
| --- | --- |
| `cargo build` | CPU build of CrispASR via cmake + bundled submodule. |
| `cargo build --features cuda` | GGML_CUDA=ON. Same as above otherwise. |
| `cargo build --features metal` | GGML_METAL=ON. macOS only. |
| `cargo build --features vulkan` | GGML_VULKAN=ON. |
| `cargo build --no-default-features` | Disables `bundled`. parakit will link against a system-installed `libcrispasr`. Set `CRISPASR_LIB_DIR=/path` if it's not in `/usr/local/lib`. |
| `CRISPASR_SRC_DIR=/elsewhere cargo build` | Use an external CrispASR checkout instead of the vendored submodule. |
| `CRISPASR_LIB_DIR=/path cargo build` | Skip the bundled build entirely; link against pre-built libs at this path. |

### Updating the bundled CrispASR

```bash
cd vendor/CrispASR
git fetch && git checkout <newer-tag>
cd ../..
git add vendor/CrispASR
cargo build  # rebuilds incrementally; cmake handles invalidation
```

### System dependencies

Cargo + Rust handle Rust deps automatically. The native deps are:

| OS | Packages |
| --- | --- |
| Ubuntu 24.04 | `cmake build-essential libasound2-dev libudev-dev libxtst-dev libxdo-dev libxi-dev libx11-dev libevdev-dev libgomp1 pkg-config autoconf libtool` |
| Fedora | `cmake gcc-c++ alsa-lib-devel libudev-devel libXtst-devel libxdo-devel libXi-devel libX11-devel libevdev-devel pkgconf autoconf libtool` |
| Arch | `cmake base-devel alsa-lib libxtst xdotool libxi libx11 libevdev pkgconf autoconf libtool` |
| Windows | Visual Studio 2022 with "Desktop development with C++" workload |
| macOS | Xcode CLT (`xcode-select --install`) and `brew install cmake autoconf automake libtool pkg-config` |

For CUDA: install the [CUDA Toolkit](https://developer.nvidia.com/cuda-downloads)
(12.x recommended) and ensure `nvcc` is on PATH before `cargo build --features cuda`.

For Vulkan: `vulkan-sdk` (Linux) or the LunarG SDK (Windows/macOS).

### Windows note

Windows doesn't have rpath. After `cargo build`, the `parakit.exe` binary
needs `whisper.dll` (and `crispasr.dll`, the alias `build.rs` creates)
findable at runtime. Easiest options:

- Copy the DLLs from `target/<profile>/build/parakit-*/out/lib/` next to
  `parakit.exe`. (`cargo build` after `bundled` writes them there.)
- Or add that directory to `PATH`.

---

## Usage

```bash
# Default — verbose, batch mode, cleaning on, sounds on
parakit -m models/parakeet-tdt-0.6b-v3.gguf

# Daemonized in background
parakit -m models/parakeet-tdt-0.6b-v3.gguf --quiet &

# Streaming mode (experimental — chunks during recording)
parakit -m models/parakeet-tdt-0.6b-v3.gguf --mode streaming
parakit -m models/parakeet-tdt-0.6b-v3.gguf --mode streaming:2.5  # 2.5s chunks

# Disable specific cleaning rules
parakit -m m.gguf --disable-rule lead-so-comma --disable-rule mid-like-noun

# All cleaning off — type the raw ASR output
parakit -m m.gguf --no-cleaning

# Show all rules
parakit --list-rules

# Test the cleaning pipeline without recording (great for iterating)
parakit --test-rules "So, um, the the the cat ran like, you know, fast"
# Raw:     So, um, the the the cat ran like, you know, fast
# Clean:   the cat ran like fast
```

The hotkey is `Ctrl+Space`. The literal space is suppressed from reaching
the focused application.

### Sounds

Three short tones are generated programmatically (no audio files):

- **Start** — A4 ding (440 Hz), played when recording begins.
- **Success** — E5 ding (659 Hz), played after successful injection.
- **Error** — Two-pulse low buzz (220 Hz + slightly detuned), played on
  failure.

Disable with `--no-sounds`.

---

## Adding your own cleaning rules

All rules live in `src/rules.rs` as a single `&[Rule]` slice called
`DEFAULT_RULES`. To add one, append an entry:

```rust
Rule {
    name: "weights-and-biases-to-wandb",
    description: "Map 'weights and biases' / '1 B' to 'wandb'",
    pattern: r"(?i)\bweights and biases\b",
    replacement: "wandb",
    default_enabled: true,
},
```

Constraints:

- Rust's `regex` crate doesn't support backreferences (`\1`). Use literal
  alternations or split into multiple rules.
- Each rule's output is the next rule's input — order matters. Specific
  rules go before generic ones.
- The final whitespace/punctuation cleanup rules must stay last in the
  slice.
- Test fast with `cargo run -- --test-rules "your input here"`.

To make a rule off by default, set `default_enabled: false`.

The user-specific rules from the original `.murmure` config (`1B → wandb`,
`weights and biases → wandb`, `clot artifact → Claude artifact`, etc.)
are intentionally **not** included as defaults. They're examples of the
class of personalization that lives well in this file but doesn't
generalize.

---

## How the daemon works

```
┌─────────────────────────────────────────────────────────────────┐
│                          Main thread                             │
│  - parses CLI                                                    │
│  - builds AudioCapture (owns cpal::Stream — !Send)               │
│  - runs rdev::grab() callback loop (blocks until shutdown)       │
└─────────────────────────────────────────────────────────────────┘
                                │
                                │ events (Start/Stop/StreamChunk)
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                       Worker thread                              │
│  - receives events on a crossbeam_channel                        │
│  - calls Engine::transcribe() (blocks main from input lag)       │
│  - applies Cleaner, calls Injector::type_text()                  │
│  - signals Sounds for start/success/error tones                  │
└─────────────────────────────────────────────────────────────────┘
                                ▲
        ┌───────────────────────┴───────────────────────┐
        │                                                │
┌───────────────────┐                          ┌────────────────────┐
│   cpal RT thread  │                          │   sounds thread    │
│  appends to       │                          │  owns rodio Output │
│  shared Vec<f32>  │                          │  Stream; plays     │
│  while recording  │                          │  cues from channel │
└───────────────────┘                          └────────────────────┘
```

`cpal::Stream` and `rodio::OutputStream` are both `!Send` on some
platforms, so each lives on its own dedicated thread. The cross-thread
contracts are:

- `AudioHandle` (from `audio.rs`) is the `Send + Sync` view: any thread
  can call `start_recording`, `stop_recording`, `snapshot_from`.
- `Sounds` (from `sounds.rs`) is a `Clone`able `Send + Sync` channel
  sender; `Cue::{Start,Success,Error}` are non-blocking sends that
  drop on full channel.

---

## Modes

### Batch (default)

Records everything from key-down to key-up. On release, the entire
buffer is sent to the model in one shot. Lowest WER (full attention
context), highest perceived latency. Recommended for most users.

### Streaming (experimental)

While the key is held, every N seconds (default 4.0) the unconsumed
tail of the buffer is shipped to the model and the result is typed
inline. Lower perceived latency, but chunk boundaries can split words.
Parakeet-TDT was trained for offline use; streaming works but isn't its
strongest mode.

---

## Future work

### Training-data logging (planned)

The current cleaning rules are pragmatic but limited — they can't catch
context-dependent disfluencies and require human-in-the-loop maintenance.
The medium-term plan is:

1. Add `--log-dir <PATH>` and `--log-format <text|json>` flags that,
   when set, append every transcription as a `(timestamp, raw,
   cleaned, audio_secs, infer_ms)` tuple to a rotating file.
2. Run that for weeks/months across normal usage.
3. Use the resulting corpus to fine-tune a small text-to-text model
   on `raw → cleaned` pairs, with an LLM in the loop generating
   improved cleanups for the harder cases.
4. Replace (or augment) the regex pipeline with the trained model.

A `TODO(transcription-logging)` comment in `src/main.rs` marks the
hook point.

### Other ideas (not planned)

- Multiple hotkeys for different "modes" — e.g. Ctrl+Alt+Space invokes
  an LLM rewrite step before injection.
- VAD-based recording (no PTT key — just talk and pause).
- Use CrispASR's HTTP server mode (`crispasr --server`) for IPC instead
  of in-process linkage.

---

## License

MIT. See [`LICENSE`](LICENSE).

`crispasr`, `cpal`, `rdev`, `enigo`, `rodio`, `rubato`, `regex`, `clap`,
and other dependencies have their own licenses (mostly MIT/Apache-2.0).
The bundled CrispASR library is also MIT-licensed.
