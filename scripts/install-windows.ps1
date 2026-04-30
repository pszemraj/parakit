param(
    [string]$Features = "",
    [switch]$NoBlasAuto
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location $RepoRoot

git submodule update --init --recursive
if (-not (Test-Path "vendor\CrispASR\crispasr\Cargo.toml")) {
    throw "Missing vendor\CrispASR. Run: git submodule update --init --recursive"
}

if (-not $NoBlasAuto -and -not $env:PARAKIT_BLAS) {
    $env:PARAKIT_BLAS = "auto"
}

$CargoArgs = @("install", "--path", ".")
if ($Features.Trim()) {
    $CargoArgs += @("--features", $Features.Trim())
}
cargo @CargoArgs

$CargoBin = if ($env:CARGO_INSTALL_ROOT) {
    Join-Path $env:CARGO_INSTALL_ROOT "bin"
} elseif ($env:CARGO_HOME) {
    Join-Path $env:CARGO_HOME "bin"
} else {
    Join-Path $HOME ".cargo\bin"
}

$TargetDir = if ($env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR
} else {
    Join-Path $RepoRoot "target"
}

$ProfileDir = Join-Path $TargetDir "release"
$Dlls = Get-ChildItem -Path $ProfileDir -Filter "*.dll" -File -ErrorAction SilentlyContinue

if (-not $Dlls) {
    Write-Warning "No generated DLLs found in $ProfileDir. If parakit.exe fails to start, run `cargo build -vv` and inspect the CrispASR install output."
    exit 0
}

$Dlls | Copy-Item -Destination $CargoBin -Force
Write-Host "parakit: installed. Run: parakit doctor"
