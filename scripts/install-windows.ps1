param(
    [string]$Features = "",
    [switch]$NoBlasAuto
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = (Resolve-Path (Join-Path $ScriptDir "..")).Path
Set-Location $RepoRoot

function Require-Command {
    param([string]$Name)
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Required command not found on PATH: $Name"
    }
}

Require-Command "git"
Require-Command "cargo"

Write-Host "parakit: initializing git submodules"
git submodule sync --recursive
git submodule update --init --recursive

$CrispAsrManifest = Join-Path $RepoRoot "vendor\CrispASR\crispasr\Cargo.toml"
if (-not (Test-Path $CrispAsrManifest)) {
    throw "Missing $CrispAsrManifest. Reclone with --recurse-submodules or rerun git submodule update --init --recursive."
}

if (-not $NoBlasAuto -and -not $env:PARAKIT_BLAS) {
    $env:PARAKIT_BLAS = "auto"
}

$CargoArgs = @("install", "--path", ".")
if ($Features.Trim().Length -gt 0) {
    $CargoArgs += @("--features", $Features.Trim())
}

Write-Host "parakit: cargo $($CargoArgs -join ' ')"
cargo @CargoArgs

if ($env:CARGO_INSTALL_ROOT) {
    $CargoBin = Join-Path $env:CARGO_INSTALL_ROOT "bin"
} elseif ($env:CARGO_HOME) {
    $CargoBin = Join-Path $env:CARGO_HOME "bin"
} else {
    $CargoBin = Join-Path $HOME ".cargo\bin"
}

if (-not (Test-Path $CargoBin)) {
    throw "Cargo bin directory does not exist: $CargoBin"
}

$TargetDir = if ($env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR
} else {
    Join-Path $RepoRoot "target"
}
$BuildRoot = Join-Path $TargetDir "release\build"
if (-not (Test-Path $BuildRoot)) {
    Write-Warning "Build directory not found: $BuildRoot"
    exit 0
}

$Dlls = Get-ChildItem -Path $BuildRoot -Directory -Filter "parakit-*" |
    ForEach-Object {
        @(
            Join-Path $_.FullName "out\bin"
            Join-Path $_.FullName "out\lib"
        )
    } |
    Where-Object { Test-Path $_ } |
    ForEach-Object { Get-ChildItem -Path $_ -Filter "*.dll" -File -ErrorAction SilentlyContinue } |
    Sort-Object FullName -Unique

if ($Dlls.Count -eq 0) {
    Write-Warning "No CrispASR DLLs found under $BuildRoot. If parakit.exe fails to start, add the generated out\bin or out\lib directory to PATH."
    exit 0
}

Write-Host "parakit: copying $($Dlls.Count) DLL(s) to $CargoBin"
foreach ($Dll in $Dlls) {
    Copy-Item -Path $Dll.FullName -Destination $CargoBin -Force
}

Write-Host "parakit: installed. Run: parakit doctor"
