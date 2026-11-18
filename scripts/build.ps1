# Release build: dist\vibe-md-viewer.exe
# Usage: powershell -ExecutionPolicy Bypass -File scripts\build.ps1 [-Test]
param([switch]$Test)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'env.ps1')
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if ($Test) {
    cargo test --release --locked
    if ($LASTEXITCODE -ne 0) { throw "Tests failed" }
}
# --locked: exactly the dependency versions from Cargo.lock (as in CI).
cargo build --release --locked
if ($LASTEXITCODE -ne 0) { throw "Build failed" }

$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force $dist | Out-Null
Copy-Item (Join-Path $env:CARGO_TARGET_DIR 'release\vibe-md-viewer.exe') (Join-Path $dist 'vibe-md-viewer.exe') -Force
$size = [Math]::Round((Get-Item (Join-Path $dist 'vibe-md-viewer.exe')).Length / 1MB, 2)
"Done: dist\vibe-md-viewer.exe ($size MB)"
