# Native model validation

Run this checklist on the final feature-branch source on each available Mac and Windows laptop. Record the current commit, OS, hardware, backend, command logs, and physical observations. Linux results do not establish Metal or Windows behavior. A missing device or an unperformed physical check stays pending.

The current Linux acceptance report covers all six presets at Q8_0 and Q6_K on CPU, CUDA, and Vulkan. Native Mac/Windows execution, physical microphone/PTT/insertion/cue checks, and fresh listening remain pending. The earlier [CrispASR macOS report](crispasr-macos-validation.md) and [October 7 macOS report](macos-validation-2026-10-07.md) document historical runs only; they do not qualify the current Qwen presets or shared resampler.

Transfer the Git bundle, public fixture ZIP, and source archive from the closure package. Keep downloaded models and all test state inside the checkout. The Git bundle can be loaded into a full checkout without publishing it:

~~~bash
git fetch /path/to/closure/feat-new-models.bundle feat/new-models
git switch --detach FETCH_HEAD
git submodule update --init --recursive
git rev-parse HEAD
~~~

Keep the printed commit with the returned results. The fixture archive contains public FLEURS clips and constructed controls; preserve its README attribution. The source archive includes initialized submodule sources for machines that cannot fetch them.

## macOS CPU and Metal

Use a full checkout with initialized submodules and the [native build prerequisites](../build.md). These commands build an isolated Metal-capable release binary and WAV helper. Select cpu for CPU inference and gpu for Metal.

~~~bash
export CARGO_TARGET_DIR="$PWD/target/tmp/new-models/cargo-macos"
export XDG_CACHE_HOME="$PWD/target/tmp/new-models/macos/cache"
export XDG_CONFIG_HOME="$PWD/target/tmp/new-models/macos/config"
export XDG_RUNTIME_DIR="$PWD/target/tmp/new-models/macos/runtime"
export PARAKIT_MODELS_DIR="$PWD/target/tmp/new-models/macos/models"
export PARAKIT_CONFIG_PATH="$PWD/target/tmp/new-models/macos/config.toml"
mkdir -p "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME" "$XDG_RUNTIME_DIR" "$PARAKIT_MODELS_DIR" target/tmp/new-models/macos
printf '# Isolated native validation\n' > "$PARAKIT_CONFIG_PATH"
git rev-parse HEAD > target/tmp/new-models/macos/commit.txt
sw_vers > target/tmp/new-models/macos/os.txt
system_profiler SPHardwareDataType SPDisplaysDataType > target/tmp/new-models/macos/hardware.txt
cargo build --locked --release --features metal --bin parakit --example transcribe-file
BIN="$CARGO_TARGET_DIR/release/parakit"
HELPER="$CARGO_TARGET_DIR/release/examples/transcribe-file"
FIXTURES=target/tmp/new-models/fixtures
mkdir -p "$FIXTURES"
unzip -o target/tmp/new-models/closure/laptop-fixtures.zip -d "$FIXTURES"
"$BIN" --verbose doctor > target/tmp/new-models/macos/doctor.txt 2>&1
~~~

Run both precisions for all six presets on CPU and Metal. Start with the isolated empty model directory: Q6_K fetch downloads the preset's F16 file and invokes the local quantizer, so its first run records cold quantization. Keep each command's output and exit status.

~~~bash
PRESETS=(parakeet-v2 parakeet-v3 parakeet-ultra parakeet-redux qwen3-asr-0.6b qwen3-asr-1.7b)
QUANTS=(q8_0 q6_k)
DEVICES=(cpu gpu)
record() {
  name=$1
  shift
  if "$@" > "$RUN/$name.out" 2> "$RUN/$name.err"; then printf '0\n' > "$RUN/$name.exit"; else printf '%s\n' "$?" > "$RUN/$name.exit"; fi
}
for preset in "${PRESETS[@]}"; do
  for quant in "${QUANTS[@]}"; do
    RUN="target/tmp/new-models/macos/matrix/$preset-$quant"
    mkdir -p "$RUN"
    record fetch "$BIN" fetch --model-preset "$preset" --quantize "$quant"
    for device in "${DEVICES[@]}"; do
      record "$device-file" "$HELPER" --model-preset "$preset" --quantize "$quant" --device "$device" --threads 8 --warmup --repeat 2 --audio "$FIXTURES/short.wav" --audio "$FIXTURES/resample-44100-mono.wav" --audio "$FIXTURES/resample-48000-stereo.wav" --audio "$FIXTURES/resample-full-block-48000.wav" --audio "$FIXTURES/resample-partial-block-48000.wav"
      record "$device-worker" "$BIN" --verbose start --model-preset "$preset" --quantize "$quant" --device "$device" --threads 8 --no-cleaning --no-sounds --model-idle-minutes 0 --simulate-ptt-audio "$FIXTURES/short.wav" --simulate-ptt-audio "$FIXTURES/resample-44100-mono.wav" --simulate-ptt-audio "$FIXTURES/resample-48000-stereo.wav" --simulate-ptt-audio "$FIXTURES/resample-full-block-48000.wav" --simulate-ptt-audio "$FIXTURES/resample-partial-block-48000.wav" --log-dir "$RUN/$device-jsonl"
    done
  done
done
~~~

The fixture-rate files are upsampled constructions from public `short.wav`, not independent native-rate recordings. They cover 44.1 kHz mono, 48 kHz stereo/downmix, and full/partial 1024-frame source blocks; `short.wav` is 16 kHz and bypasses resampling. The helper and simulated worker both use the WAV loader; microphone capture needs the physical checks below. Compare their transcripts and run the existing resampler tests on each host:

~~~bash
cargo test resampl
~~~

The filter covers chunk/tail drain, reset/reuse, callback parity, recording boundaries, and the 16 kHz bypass. Retain build and doctor output; device inventory alone does not prove that inference used the accelerator. macOS build prerequisites are in [build.md](../build.md).

## Windows CPU and supported GPU backends

Use the [Windows prerequisites and build scripts](../../scripts/windows/README.md) from an x64 Visual Studio developer PowerShell, with a short checkout path for Vulkan. CPU is required. Add Vulkan when its SDK/device is available, and CUDA on a supported NVIDIA/CUDA setup. Record unavailable backends and their hardware/toolkit reason as pending.

~~~powershell
$env:CARGO_TARGET_DIR = Join-Path $PWD "target\native"
$env:PARAKIT_MODELS_DIR = Join-Path $PWD "target\tmp\new-models\windows\models"
$env:PARAKIT_CONFIG_PATH = Join-Path $PWD "target\tmp\new-models\windows\config.toml"
New-Item -ItemType Directory -Force (Split-Path $env:PARAKIT_CONFIG_PATH), $env:PARAKIT_MODELS_DIR | Out-Null
[System.IO.File]::WriteAllText($env:PARAKIT_CONFIG_PATH, "# Isolated native validation" + [Environment]::NewLine)
git rev-parse HEAD | Out-File "target\tmp\new-models\windows\commit.txt"
Get-CimInstance Win32_OperatingSystem | Out-File "target\tmp\new-models\windows\os.txt"
Get-CimInstance Win32_ComputerSystem | Out-File "target\tmp\new-models\windows\hardware.txt"
Get-CimInstance Win32_VideoController | Out-File "target\tmp\new-models\windows\hardware.txt" -Append
$Backends = @("cpu", "vulkan") # Add cuda only on a supported NVIDIA/CUDA system.
foreach ($Backend in $Backends) { .\scripts\windows\build.ps1 --backend $Backend --no-install }
~~~

For each backend, extract a fresh bundle, add its DLL directory to PATH for the test process, and build its matching WAV helper. Windows Q6_K generation requires a compatible crispasr-quantize.exe on PATH; see [model presets](../running.md#model-presets). The script below records separate outputs for each preset, precision, and backend.

~~~powershell
$Fixtures = Join-Path $PWD "target\tmp\new-models\fixtures"
New-Item -ItemType Directory -Force $Fixtures | Out-Null
Expand-Archive -Path "target\tmp\new-models\closure\laptop-fixtures.zip" -DestinationPath $Fixtures -Force
$Presets = @("parakeet-v2", "parakeet-v3", "parakeet-ultra", "parakeet-redux", "qwen3-asr-0.6b", "qwen3-asr-1.7b")
$Quants = @("q8_0", "q6_k")
$OriginalPath = $env:PATH
$OriginalGenerator = $env:CMAKE_GENERATOR
foreach ($Backend in $Backends) {
    $Run = Join-Path $PWD "target\tmp\new-models\windows\$Backend"
    $Bundle = Join-Path $PWD "target\parakit-windows-x86_64-$Backend"
    $Extract = Join-Path $Run "extracted"
    New-Item -ItemType Directory -Force $Run | Out-Null
    Compress-Archive -Path (Join-Path $Bundle "*") -DestinationPath (Join-Path $Run "bundle.zip") -Force
    Expand-Archive -Path (Join-Path $Run "bundle.zip") -DestinationPath $Extract -Force
    $env:PATH = "$Extract;$OriginalPath"
    $Bin = Join-Path $Extract "parakit.exe"
    $Features = switch ($Backend) { "cpu" { @() } "cuda" { @("--features", "cuda") } "vulkan" { @("--features", "vulkan") } }
    $HelperTarget = Join-Path $env:CARGO_TARGET_DIR $Backend
    if ($Backend -ne "cpu") { $env:CMAKE_GENERATOR = "Ninja" }
    cargo build --locked --release @Features --example transcribe-file --target-dir $HelperTarget
    $env:CMAKE_GENERATOR = $OriginalGenerator
    & $Bin --verbose doctor *> (Join-Path $Run "doctor.txt")
    $Device = if ($Backend -eq "cpu") { "cpu" } else { "gpu" }
    $Helper = Join-Path $HelperTarget "release\examples\transcribe-file.exe"
    foreach ($Preset in $Presets) {
        foreach ($Quant in $Quants) {
            $Case = Join-Path $Run "$Preset-$Quant"
            New-Item -ItemType Directory -Force $Case | Out-Null
            & $Bin fetch --model-preset $Preset --quantize $Quant *> (Join-Path $Case "fetch.txt"); $LASTEXITCODE | Out-File (Join-Path $Case "fetch.exit")
            $AudioArgs = @("--audio", (Join-Path $Fixtures "short.wav"), "--audio", (Join-Path $Fixtures "resample-44100-mono.wav"), "--audio", (Join-Path $Fixtures "resample-48000-stereo.wav"), "--audio", (Join-Path $Fixtures "resample-full-block-48000.wav"), "--audio", (Join-Path $Fixtures "resample-partial-block-48000.wav"))
            $HelperArgs = @("--model-preset", $Preset, "--quantize", $Quant, "--device", $Device, "--threads", "8", "--warmup", "--repeat", "2") + $AudioArgs
            & $Helper @HelperArgs *> (Join-Path $Case "file.txt"); $LASTEXITCODE | Out-File (Join-Path $Case "file.exit")
            $WorkerArgs = @("--verbose", "start", "--model-preset", $Preset, "--quantize", $Quant, "--device", $Device, "--threads", "8", "--no-cleaning", "--no-sounds", "--model-idle-minutes", "0") + ($AudioArgs | ForEach-Object { if ($_ -eq "--audio") { "--simulate-ptt-audio" } else { $_ } }) + @("--log-dir", (Join-Path $Case "worker-jsonl"))
            & $Bin @WorkerArgs *> (Join-Path $Case "worker.txt"); $LASTEXITCODE | Out-File (Join-Path $Case "worker.exit")
        }
    }
    $env:PATH = $OriginalPath
}
~~~

Run `cargo test resampl` in the Windows checkout too. The DLL path is restored after each backend. Fixture rates and test interpretation match macOS. Keep each extracted directory fresh on reruns so stale DLLs cannot mask bundle omissions.

## Qwen options, long input, and reload

Use the language, token, and chunking controls described under [model presets](../running.md#model-presets). Keep Qwen attention overrides unset for acceptance; the default and diagnostic alternatives are described in the [Qwen comparison procedure](quality.md#qwen-and-moondream-comparisons).

For both Qwen sizes, both precisions, and each native device path, run the helper on short/270-second/short inputs at forced English and a 4096-token budget. Then run two simulated captures with a 63-second idle wait and a one-minute offload setting to verify reload and option restoration. The long fixture repeats public JFK speech for context/memory stress, not natural long-form quality.

~~~bash
for preset in qwen3-asr-0.6b qwen3-asr-1.7b; do
  for quant in q8_0 q6_k; do
    for device in cpu gpu; do
      RUN="target/tmp/new-models/macos/qwen-$preset-$quant-$device"
      mkdir -p "$RUN"
      "$HELPER" --model-preset "$preset" --quantize "$quant" --device "$device" --threads 8 --qwen-language en --qwen-max-new-tokens 4096 --warmup --audio "$FIXTURES/short.wav" --audio "$FIXTURES/repeated-speech-270s.wav" --audio "$FIXTURES/short.wav" > "$RUN/sequence.out" 2> "$RUN/sequence.err"
      "$BIN" --verbose start --model-preset "$preset" --quantize "$quant" --device "$device" --threads 8 --qwen-language en --qwen-max-new-tokens 4096 --no-cleaning --no-sounds --model-idle-minutes 1 --simulate-ptt-repeat 2 --simulate-ptt-idle-seconds 63 --simulate-ptt-audio "$FIXTURES/short.wav" --log-dir "$RUN/reload-jsonl" > "$RUN/reload.out" 2> "$RUN/reload.err"
    done
  done
done
~~~

On Windows, run the equivalent sequence after each backend's matrix, while `$Bin`, `$Helper`, and `$Device` still select that backend:

~~~powershell
foreach ($Preset in @("qwen3-asr-0.6b", "qwen3-asr-1.7b")) {
    foreach ($Quant in $Quants) {
        $Case = Join-Path $Run "context-$Preset-$Quant"
        New-Item -ItemType Directory -Force $Case | Out-Null
        $Options = @("--model-preset", $Preset, "--quantize", $Quant, "--device", $Device, "--threads", "8", "--qwen-language", "en", "--qwen-max-new-tokens", "4096")
        & $Helper @Options --warmup --audio (Join-Path $Fixtures "short.wav") --audio (Join-Path $Fixtures "repeated-speech-270s.wav") --audio (Join-Path $Fixtures "short.wav") *> (Join-Path $Case "sequence.txt")
        $LASTEXITCODE | Out-File (Join-Path $Case "sequence.exit")
        & $Bin --verbose start @Options --no-cleaning --no-sounds --model-idle-minutes 1 --simulate-ptt-repeat 2 --simulate-ptt-idle-seconds 63 --simulate-ptt-audio (Join-Path $Fixtures "short.wav") --log-dir (Join-Path $Case "reload-jsonl") *> (Join-Path $Case "reload.txt")
        $LASTEXITCODE | Out-File (Join-Path $Case "reload.exit")
    }
}
~~~

Also record default auto/512, forced German on `fleurs-de_de-0.wav`, and forced English on `fleurs-en_us-422.wav` (containing the spoken word "language"). Use a budget of 1 for a separate failure-path check: readiness should complete, while actual transcription fails without a successful JSONL row or history entry.

For every preset and precision, also check representative longer speech, silence, repeats, and idle offload/reload through the helper and worker. Use the 29/30/31-second boundary fixtures for both Qwen sizes. Save exit statuses alongside output for each additional command, as in the matrix. A worker no-speech error is expected for silence; it must not become a successful transcript or insertion. Repeat completed Q6 acquisition with network access disabled and the F16/quantizer unavailable to confirm cached startup needs neither. On Windows, separately record the first-generation result with the external quantizer available.

Sample host RSS/working set and GPU allocation separately through warmup, long inference, idle, offload, and reload. Compare incremental GPU allocation with the [reload allowance](../running.md#idle-model-offload), recording Metal's unified-memory accounting separately from discrete VRAM. Keep measured peaks and sampling intervals with the logs; Linux calibration does not qualify the native platform's memory behavior.

silence.wav and noise-only.wav are synthetic controls; inspect unwanted insertion/history changes without treating them as room-noise evidence or adding a new voice-activity/rejection policy. quiet-rms26-complete.wav, quiet-rms26-complete-lowgain.wav, human-speech-noise-tail.wav, and quiet-loud-quiet.wav are controlled diagnostics documented in the fixture README. Compare expected opening/closing phrases and tail behavior, then listen when output disagrees. Constructed noise is not real ambient sound.

## Physical desktop checks

Use isolated configuration for the tested preset, precision, device, and Qwen options; leave installed settings intact. Check both Qwen sizes, both precisions, and one Parakeet preset on each available native device path.

1. Dictate a sentence with a negation, number, and the word language. Confirm one insertion, inspect the transcript, and repeat with normal cleaning enabled.
2. After idle offload, hold PTT through reload, speak, and release. Confirm readiness before recording and one complete insertion. Release PTT immediately during reload separately; confirm the queued capture completes once and no late listening cue follows.
3. Check normal start/completion/error cues and repeat with sounds disabled. Check silence and synthetic noise without treating them as acoustic room tests.
4. Stop and restart the isolated daemon without CLI overrides; confirm saved selection and Qwen options persist. Test sleep/wake, another capture, and insertion.
5. Record real quiet speech and nonspeech in a room with fan, keyboard, breathing, or microphone handling sounds, paired with quiet and breathless speech. Record the environment and listen to disagreements. These real-world recordings remain pending until performed; do not add or assume a new VAD/rejection rule.

Check status, history, logs, and inserted text after each run. History is in memory and starts empty after restart; configuration and JSONL logs are separate. Mark each item pass, fail, or pending with evidence. Fresh listening and physical microphone results are required; dataset references and official-model output are not ground truth. Filling VRAM and physical stop-to-insert p50/p95 remain outside this closure gate.

Return logs and hardware/backend details without model files. Keep outputs under `target/tmp/new-models`. Closure package paths are `target/tmp/new-models/closure/feat-new-models.bundle`, `target/tmp/new-models/closure/laptop-fixtures.zip`, and `target/tmp/new-models/closure/parakit-model-source.tar.gz`.
