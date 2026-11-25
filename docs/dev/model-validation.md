# Native model validation

Run the matrix on each available native backend. Record the commit, OS, hardware, backend, command logs, and physical observations. Mark unavailable devices and unperformed physical checks as pending.

Initialize the submodules. Obtain `laptop-fixtures.zip` from the validation owner and place it under `target/tmp/new-models/`. The archive contains the speech, resampling, boundary, silence, and noise fixtures, including public FLEURS clips. Preserve its README attribution. Keep models and test state inside the checkout.

TODO: Provide an acquisition URL or construction recipe for `laptop-fixtures.zip`.

## macOS CPU and Metal

Install the [native build prerequisites](../build.md). These commands build an isolated Metal-capable release binary and WAV helper. Test Metal (`gpu`) first, then CPU (`cpu`).

~~~bash
export CARGO_TARGET_DIR="$PWD/target/tmp/new-models/cargo-macos"
export XDG_CACHE_HOME="$PWD/target/nm-cache"
export XDG_CONFIG_HOME="$PWD/target/tmp/new-models/macos/config"
export PARAKIT_MODELS_DIR="$PWD/target/tmp/new-models/macos/models"
export PARAKIT_CONFIG_PATH="$PWD/target/tmp/new-models/macos/config.toml"
export GGML_METAL_PIPELINE_CACHE="$PWD/target/tmp/new-models/macos/metal-cache"
mkdir -p "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME" "$PARAKIT_MODELS_DIR" "$GGML_METAL_PIPELINE_CACHE" target/tmp/new-models/macos
printf '# Isolated native validation\n' > "$PARAKIT_CONFIG_PATH"
git rev-parse HEAD > target/tmp/new-models/macos/commit.txt
sw_vers > target/tmp/new-models/macos/os.txt
system_profiler SPHardwareDataType SPDisplaysDataType > target/tmp/new-models/macos/hardware.txt
cargo build --locked --release --features metal --bin parakit --example transcribe-file
BIN="$CARGO_TARGET_DIR/release/parakit"
HELPER="$CARGO_TARGET_DIR/release/examples/transcribe-file"
FIXTURES=target/tmp/new-models/fixtures
FIXTURE_ZIP=target/tmp/new-models/laptop-fixtures.zip
mkdir -p "$FIXTURES"
unzip -o "$FIXTURE_ZIP" -d "$FIXTURES"
"$BIN" --verbose doctor > target/tmp/new-models/macos/doctor.txt 2>&1
~~~

Choose a short cache root for the [daemon control socket](../configuration.md#xdg_runtime_dir). The Metal cache override isolates the shader archive from the installed daemon's cache.

Run both precisions for all six presets on CPU and Metal. Start with an empty isolated model directory to exercise cold Q6_K preparation. The matrix saves each command's output and exit status. [Quantizer installation](README.md#source-rebuild)

~~~bash
PRESETS=(parakeet-v2 parakeet-v3 parakeet-ultra parakeet-redux qwen3-asr-0.6b qwen3-asr-1.7b)
QUANTS=(q8_0 q6_k)
DEVICES=(gpu cpu)
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
      if [[ $preset == qwen3-asr-* ]]; then
        record "$device-sequence" "$HELPER" --model-preset "$preset" --quantize "$quant" --device "$device" --threads 8 --qwen-language en --qwen-max-new-tokens 4096 --warmup --audio "$FIXTURES/short.wav" --audio "$FIXTURES/repeated-speech-270s.wav" --audio "$FIXTURES/short.wav"
        record "$device-reload" "$BIN" --verbose start --model-preset "$preset" --quantize "$quant" --device "$device" --threads 8 --qwen-language en --qwen-max-new-tokens 4096 --no-cleaning --no-sounds --model-idle-minutes 1 --simulate-ptt-repeat 2 --simulate-ptt-idle-seconds 63 --simulate-ptt-audio "$FIXTURES/short.wav" --log-dir "$RUN/$device-reload-jsonl"
      fi
    done
  done
done
~~~

The fixture-rate files are upsampled constructions from public `short.wav`, not independent native-rate recordings. They cover 44.1 kHz mono, 48 kHz stereo/downmix, and full/partial 1024-frame source blocks; `short.wav` is 16 kHz and bypasses resampling. Compare helper and worker transcripts using the [WAV helper](quality.md#wav-quality-target) and [worker simulation](quality.md#ptt-worker-simulation) procedures, and run the resampler tests on each host:

~~~bash
cargo test resampl
~~~

The filter covers chunk/tail drain, reset/reuse, callback parity, recording boundaries, and the 16 kHz bypass. Retain build and doctor output, native inference diagnostics, and [JSONL session-device metadata](../logging.md#transcription-record-fields); device inventory alone does not prove accelerator use.

## Windows CPU and supported GPU backends

Use the [Windows prerequisites and build scripts](../../scripts/windows/README.md) from an x64 Visual Studio developer PowerShell, with a short checkout path for Vulkan. CPU is required. Add Vulkan when its SDK/device is available, and CUDA on a supported NVIDIA/CUDA setup. Record unavailable backends and their hardware/toolkit reason as pending.

Verify `cargo run -- --help` and `cargo test resampl` from a fresh developer shell without a Parakit installation, bundle, native build directory, or external BLAS runtime directory on the inherited PATH. Cargo supplies staged runtime DLLs through the build script's `OUT_DIR/bin` link-search directive. Keep separately installed toolkit runtimes on PATH when the build does not bundle them. The bundle checks below add a DLL directory explicitly, so they do not validate this Cargo loader path.

A real Windows daemon uses the same user's lock and named pipe even with separate model and configuration paths, and `doctor` checks that lock. Stop an existing same-user daemon before those checks and preserve its settings; [worker simulation](quality.md#ptt-worker-simulation) bypasses them.

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
$Backends = @("cpu") # Add vulkan or cuda only when its SDK and supported device are available.
foreach ($Backend in $Backends) { .\scripts\windows\build.ps1 --backend $Backend --no-install }
~~~

For each backend, extract a fresh bundle, add its DLL directory to the process PATH, and build its matching WAV helper. The script records the matrix and [Qwen sequence](#qwen-options-long-input-and-reload) separately for each backend. [Quantizer installation](README.md#source-rebuild)

~~~powershell
$Fixtures = Join-Path $PWD "target\tmp\new-models\fixtures"
$FixtureZip = Join-Path $PWD "target\tmp\new-models\laptop-fixtures.zip"
New-Item -ItemType Directory -Force $Fixtures | Out-Null
Expand-Archive -Path $FixtureZip -DestinationPath $Fixtures -Force
$Presets = @("parakeet-v2", "parakeet-v3", "parakeet-ultra", "parakeet-redux", "qwen3-asr-0.6b", "qwen3-asr-1.7b")
$Quants = @("q8_0", "q6_k")
$OriginalPath = $env:PATH
$OriginalGenerator = $env:CMAKE_GENERATOR
foreach ($Backend in $Backends) {
    $Run = Join-Path $PWD "target\tmp\new-models\windows\$Backend"
    $Bundle = Join-Path $PWD "target\parakit-windows-x86_64-$Backend"
    $Extract = Join-Path $Run ("extracted-" + [Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force $Run | Out-Null
    Compress-Archive -Path (Join-Path $Bundle "*") -DestinationPath (Join-Path $Run "bundle.zip") -Force
    Expand-Archive -Path (Join-Path $Run "bundle.zip") -DestinationPath $Extract
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
    $env:PATH = $OriginalPath
}
~~~

Run the [Rust validation loop](quality.md#rust-validation-loop) in the Windows checkout too, including the cleaner regression matrices and resampler tests. The DLL path is restored after each backend. Fixture rates and test interpretation match macOS. Keep each extracted directory fresh on reruns so stale DLLs cannot mask bundle omissions.

### Windows installation lifecycle

Complete these checks natively for CPU and each available GPU backend, in addition to the bundle/model matrix above. Use disposable source checkouts, separate Cargo homes and targets, custom install roots, and existing isolated configuration files. Set `PARAKIT_MODELS_DIR` explicitly for every test process; XDG overrides alone do not isolate the Windows model cache. Keep the ordinary installation and User PATH out of these checks.

Keep the isolated `CARGO_HOME` short too: Cargo nests Git checkouts and submodules beneath it, which can exceed Windows path limits before compilation. For example, use `target\h` instead of nesting the Cargo home under the evidence directory.

- Install from Git and from a populated local checkout with `cargo install --locked`, using the backend's toolchain and features. Unset `CRISPASR_LIB_DIR` and `CRISPASR_SRC_DIR` for the bundled case. Record the source revision, Cargo installation metadata, commands, and exit statuses. Cargo installs executables only; retain required external BLAS/CUDA runtimes on the test process's PATH, or validate their packaging through the bundle route separately.
- Move each install prefix to a path containing spaces. From outside the disposable checkout, with test processes stopped, temporarily rename its original source checkout, Cargo home, and build directories, restoring them afterward. Remove those build/bundle directories and explicit CrispASR library overrides from the test environment while retaining approved system/toolkit runtime paths. Inspect direct and transitive PE dependencies: bundled inference must not import CrispASR/ggml DLLs or resolve dependencies from hidden build paths.
- Invoke the relocated executable by its full path. Check that `--version` agrees with the tested checkout's `Cargo.toml`, run hosted fetching, and run the [pinned WAV simulation](quality.md#ptt-worker-simulation) and [cleaned worker parity](quality.md#cleaned-worker-parity). Confirm that rejected nonspeech leaves history unchanged. Record device diagnostics and JSONL `inference.session_device`. Corroborate GPU execution with native diagnostics or per-process GPU engine activity. A CPU-only build must reject an explicit GPU request.
- Exercise cold hosted Q8_0 acquisition without Python/conversion tools on PATH, preserving required runtime DLL paths. Cold Q6_K preparation requires the separately installed quantizer. Then make the endpoint unavailable and remove conversion tools from the process PATH: completed Q6_K and `--from-source` caches must remain usable without new downloads or preparation.
- Compare the isolated model directory before and after the Rust suite and acquisition checks. The Rust suite and failed preparation must preserve existing cached artifacts; test fixtures must stay in their private directories. Successful acquisition must leave no partial-download or quantization staging files. Read back the installed executable's `cache list`; an empty or unknown mock GGUF entry is not a prepared model.
- Reinstall into the relocated Cargo root, confirm replacement by invoking the resulting executable, then uninstall and verify that Cargo's installation list and executable paths are clear. For [bundle installation](../../scripts/windows/README.md#install-an-existing-bundle), pass `-InstallDir` and `-NoUserPath` to `install.ps1` with the test prefix; check manifest files, removal of stale backend DLLs on upgrade, and preservation of the previous test installation after a failed replacement.

## Qwen options, long input, and reload

Use the language, token, and chunking controls described in the [Qwen3-ASR guide](../models/qwen3-asr.md). Keep Qwen attention overrides unset for acceptance; the default and diagnostic alternatives are described in the [Qwen comparison procedure](quality.md#qwen-and-moondream-comparisons).

The short/270-second/short sequence uses forced English and a 4096-token budget. Check option restoration and reload state independently of recognition. The long fixture repeats public JFK speech for memory and chunk-boundary stress. Compare its clipped ending with the waveform and reference. Check short-input parity separately.

Also record default auto/512, forced German on `fleurs-de_de-0.wav`, and forced English on `fleurs-en_us-422.wav` (containing the spoken word "language"). Use a budget of 1 for a separate failure-path check: readiness should complete, while actual transcription fails without a successful JSONL row or history entry.

Run the [ordered-audio checks](quality.md#ordered-audio) on native hosts. Evaluate ordinary reload parity and [prior-dictation context](../models/qwen3-asr.md#prior-dictation-context) separately.

For every preset and precision, also check representative longer speech, silence, repeats, and idle offload/reload through the helper and worker. Use the 29/30/31-second boundary fixtures for both Qwen sizes. Save exit statuses alongside output for each additional command, as in the matrix. A worker no-speech error is expected for silence; it must not become a successful transcript or insertion. Repeat completed Q6 acquisition with network access disabled and the F16/quantizer unavailable to confirm cached startup needs neither. On Windows, separately record the first-generation result with the external quantizer available.

Follow the [memory measurement procedure](memory.md#measuring-memory) on each native backend; Linux calibration does not establish Metal or Windows allocation behavior.

Use `silence.wav` and `noise-only.wav` as synthetic controls for unwanted insertion and history changes. For `quiet-rms26-complete.wav`, `quiet-rms26-complete-lowgain.wav`, `human-speech-noise-tail.wav`, and `quiet-loud-quiet.wav`, use the fixture README to check expected opening and closing phrases and tail behavior. Listen when the transcripts disagree.

## Physical desktop checks

Use isolated configuration for the tested preset, precision, device, and Qwen options; leave installed settings intact. Check both Qwen sizes, both precisions, and one Parakeet preset on each available native device path. Follow the general [desktop smoke checks](quality.md#runtime-smoke-checks) as well.

1. Dictate a sentence with a negation, number, and the word language. Confirm one insertion, inspect the transcript, and repeat with normal cleaning enabled.
2. Stop and restart the isolated daemon without CLI overrides; confirm saved selection and Qwen options persist.
3. Record quiet speech, breathless speech, and nonspeech with real fan, keyboard, breathing, or microphone handling sounds. Record the environment and listen to disagreements.

Check status, history, logs, and inserted text after each run. Mark each item pass, fail, or pending with evidence. Listen to the actual recordings to adjudicate recognition; follow the [reference comparison procedure](quality.md#recommended-ab-procedure) when outputs disagree. VRAM-pressure tests and physical stop-to-insert p50/p95 measurements are separate runs.

Keep outputs and hardware/backend details under `target/tmp/new-models`. Exclude model files when sharing results.
