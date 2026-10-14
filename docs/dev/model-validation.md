# Native model validation

This is the remaining native desktop checklist for the experimental model presets. Run it on the final feature-branch commit, record the commit and hardware, and return the command logs plus the physical observations below. Linux results do not establish Windows or Metal behavior. Missing hardware stays untested; it does not count as a pass.

Use repository-local build and runtime directories. Keep the installed command, its native libraries, and its configuration intact. Coordinate the physical hotkey checks with the existing daemon: only the intended test instance should receive the PTT chord.

## macOS CPU and Metal

Start in a full checkout with initialized submodules and the [native build prerequisites](../build.md). These commands build an isolated Metal-capable binary; `--device cpu` tests its explicit CPU path.

```bash
export CARGO_TARGET_DIR="$PWD/target/tmp/new-models/cargo-metal"
export CARGO_BUILD_JOBS=2
export CMAKE_BUILD_PARALLEL_LEVEL=2
cargo build --locked --release --features metal --bin parakit --example transcribe-file

export XDG_CACHE_HOME="$PWD/target/tmp/mv/cache"
export XDG_CONFIG_HOME="$PWD/target/tmp/mv/config"
export XDG_RUNTIME_DIR="$PWD/target/tmp/mvr"
export PARAKIT_MODELS_DIR="$PWD/target/tmp/mv/models"
export PARAKIT_CONFIG_PATH="$PWD/target/tmp/mv/config.toml"
mkdir -p "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME" "$XDG_RUNTIME_DIR" "$PARAKIT_MODELS_DIR"
printf '# Isolated native validation\n' > "$PARAKIT_CONFIG_PATH"

git rev-parse HEAD > target/tmp/mv/commit.txt
sw_vers > target/tmp/mv/os.txt
system_profiler SPHardwareDataType SPDisplaysDataType > target/tmp/mv/hardware.txt
"$CARGO_TARGET_DIR/release/parakit" --verbose doctor > target/tmp/mv/doctor.txt 2>&1

for preset in parakeet-v3 qwen3-asr-0.6b qwen3-asr-1.7b parakeet-ultra parakeet-redux; do
  "$CARGO_TARGET_DIR/release/parakit" fetch --model-preset "$preset"
  for device in cpu gpu; do
    "$CARGO_TARGET_DIR/release/examples/transcribe-file" \
      --model-preset "$preset" --device "$device" --threads 8 --warmup --repeat 2 \
      --audio vendor/CrispASR/bindings/go/samples/jfk.wav \
      > "target/tmp/mv/$preset-$device-file.out" 2> "target/tmp/mv/$preset-$device-file.err"
  done
done
```

Record every exit status. GPU means Metal in this build; retain the native diagnostics instead of inferring execution from the device inventory. Both Qwen sizes must complete readiness and repeated speech. Run the Rust suite and native source-substitution tests on this host too; the default ignored loaded-model test is exercised separately by the Linux fault driver.

## Windows CPU and available GPU backends

Use the [existing build scripts](../../scripts/windows/README.md) and their native prerequisites. In PowerShell at the repository root:

```powershell
$env:CARGO_TARGET_DIR = Join-Path $PWD "target\tmp\new-models\cargo-windows"
$env:CARGO_BUILD_JOBS = "2"
$env:CMAKE_BUILD_PARALLEL_LEVEL = "2"
.\scripts\windows\build.ps1 --backend cpu --no-install
.\scripts\windows\build.ps1 --backend vulkan --no-install
```

Build Vulkan when the laptop has a supported GPU and its SDK. On an NVIDIA laptop with the CUDA Toolkit, also build `--backend cuda --no-install`. Record unavailable backends and the hardware reason. Do not treat a missing toolkit as a successful backend check.

Validate each supported bundle after extracting it to a fresh repository-local directory. For example, after a Vulkan build:

```powershell
$Run = Join-Path $PWD "target\tmp\mv"
New-Item -ItemType Directory -Force $Run | Out-Null
Compress-Archive -Path "target\parakit-windows-x86_64-vulkan\*" -DestinationPath "$Run\vulkan.zip" -Force
Expand-Archive -Path "$Run\vulkan.zip" -DestinationPath "$Run\vulkan-extracted" -Force
$Bin = "$Run\vulkan-extracted\parakit.exe"
$env:PARAKIT_MODELS_DIR = "$Run\models"
$env:PARAKIT_CONFIG_PATH = "$Run\config.toml"
[System.IO.File]::WriteAllText($env:PARAKIT_CONFIG_PATH, "# Isolated native validation`n")
git rev-parse HEAD | Out-File "$Run\commit.txt"
Get-CimInstance Win32_OperatingSystem | Out-File "$Run\os.txt"
Get-CimInstance Win32_VideoController | Out-File "$Run\gpu.txt"
& $Bin --version
& $Bin --verbose doctor *> "$Run\doctor.txt"
```

Repeat extraction and loading for the CPU bundle and each supported accelerator. The daemon's WAV simulation runs directly from the extracted bundle:

```powershell
foreach ($Preset in @("parakeet-v3", "qwen3-asr-0.6b", "qwen3-asr-1.7b", "parakeet-ultra", "parakeet-redux")) {
    & $Bin fetch --model-preset $Preset
    & $Bin --verbose start --model-preset $Preset --device gpu --threads 8 --no-cleaning --no-sounds --model-idle-minutes 0 --simulate-ptt-audio "vendor\CrispASR\bindings\go\samples\jfk.wav" --simulate-ptt-repeat 2 --log-dir "$Run\$Preset-jsonl" *> "$Run\$Preset-worker.txt"
    $LASTEXITCODE | Out-File "$Run\$Preset-exit.txt"
}
```

Use `--device cpu` for the CPU bundle. For the matching Vulkan file helper, run the following in the native toolchain shell used for the build, then repeat the model loop with this executable:

```powershell
cargo build --locked --release --features vulkan --example transcribe-file --target-dir "$env:CARGO_TARGET_DIR\vulkan"
$env:PATH = "$Run\vulkan-extracted;" + $env:PATH
& "$env:CARGO_TARGET_DIR\vulkan\release\examples\transcribe-file.exe" --model-preset qwen3-asr-1.7b --device gpu --threads 8 --warmup --repeat 2 --audio "vendor\CrispASR\bindings\go\samples\jfk.wav"
```

For CPU, omit `--features vulkan`, select the `cpu` target subdirectory and extracted CPU bundle on `PATH`, and use `--device cpu`. For CUDA, select the matching feature, target subdirectory and bundle. The helper needs its matching DLLs; copying only its executable elsewhere is insufficient.

## Qwen options, long input and reload

Use the supplied public fixture ZIP, extracted beneath `target/tmp/`, or equivalent human recordings with known text. `short.wav` contains a negation; `repeated-speech-270s.wav` is repeated JFK speech for memory/context stress, not a natural 270-second quality sample. Keep the fixture attribution with the files.

On each native CPU and supported GPU path, run both Qwen sizes with this ordered sequence. The Unix example uses the macOS variables above:

```bash
"$CARGO_TARGET_DIR/release/parakit" --verbose start \
  --model-preset qwen3-asr-1.7b --device gpu --threads 8 \
  --qwen-language en --qwen-max-new-tokens 4096 --no-cleaning --no-sounds \
  --simulate-ptt-audio target/tmp/model-fixtures/short.wav \
  --simulate-ptt-audio target/tmp/model-fixtures/repeated-speech-270s.wav \
  --simulate-ptt-audio target/tmp/model-fixtures/short.wav \
  --simulate-ptt-repeat 2 --simulate-ptt-idle-seconds 63 --model-idle-minutes 1 \
  --log-dir target/tmp/mv/qwen-long-jsonl \
  > target/tmp/mv/qwen-long.out 2> target/tmp/mv/qwen-long.err
```

Repeat for 0.6B, CPU, and automatic language/default budget. On Windows use the extracted `$Bin`, PowerShell paths, and the same arguments. Check two idle offloads, one reload, six successful transcription rows, matching short transcripts, and unchanged effective options after reload. Record host RSS and available per-process accelerator allocations separately; Metal uses unified memory, so do not add those measurements.

Also run forced German on the public German clip with cleaning disabled and budget 128, silence, and the quiet/boundary fixtures. With budget 1, Qwen readiness must succeed and the real dictation must fail without successful JSONL/insertion/history replacement. The spoken word `language` in `fleurs-en_us-422.wav` must survive forced English. Status must show the chosen preset and user options, rather than temporary readiness settings.

## Physical checks on each desktop

Run the isolated test daemon in the foreground when the normal instance will not compete for the hotkey. Use both Qwen sizes and one Parakeet preset, then inspect `status`, `history`, and JSONL state with the same isolated configuration:

1. Dictate a short sentence containing a negation, number and the word `language`. Confirm insertion exactly once and review the actual words before cleanup. Repeat with normal cleaning separately.
2. Wait for idle offload. Hold PTT through reload, speak, and release. Confirm readiness precedes listening, the full capture is inserted once, and user options persist.
3. Release PTT immediately during reload. Confirm the queued capture completes once and no delayed listening cue follows release.
4. Check normal start/completion/error cues and `--no-sounds`. Silence and a deliberate tiny-budget failure must not paste text or replace the previous successful history entry.
5. Stop, restart with the same isolated configuration, and confirm configuration persists. Check sleep/wake, a subsequent capture, and insertion. Do not infer audible or physical behavior from headless tests.

Return the logs, current commit/hardware/backend details, and a pass/fail observation for each numbered item. Review disagreements against the recording or a human listener. Dataset reading text and official-model output are useful references, but neither resolves an unheard disagreement automatically.
