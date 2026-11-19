# Code coverage: unit tests + the self-test on a live window, from an instrumented build
# (-C instrument-coverage) in target\cov. Needs llvm-profdata/llvm-cov of the same LLVM version
# as rustc: locally from .toolcache\llvm-tools (see docs/BUILD.md), in CI from the rustup
# component llvm-tools (pass -LlvmBin). Report: summary per file in the console,
# HTML in .toolcache\coverage\html (-Html), summary text in .toolcache\coverage\summary.txt.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\coverage.ps1 [-LlvmBin dir] [-Html] [-MinLines 0]
param([string]$LlvmBin = '', [switch]$Html, [double]$MinLines = 0)
# Not 'Stop': in PowerShell 5.1 that turns cargo's progress output on stderr into errors;
# failures are checked by exit codes below.
$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
# Locally: caches stay inside the project. In CI (CI=true) the standard cargo caches are used.
if ($env:CI -ne 'true') { . (Join-Path $PSScriptRoot 'env.ps1') }
if (-not $LlvmBin) { $LlvmBin = Join-Path $root '.toolcache\llvm-tools' }
$profdata = Join-Path $LlvmBin 'llvm-profdata.exe'
$llvmcov = Join-Path $LlvmBin 'llvm-cov.exe'
foreach ($t in $profdata, $llvmcov) { if (-not (Test-Path $t)) { throw "Not found: $t (see docs/BUILD.md, Coverage)" } }

$out = Join-Path $root '.toolcache\coverage'
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force $out | Out-Null
$env:CARGO_TARGET_DIR = Join-Path $root 'target\cov'
$env:RUSTFLAGS = '-C instrument-coverage'
$env:LLVM_PROFILE_FILE = Join-Path $out 'cov-%p-%m.profraw'

# 1) Unit tests (also the ones drawing on a real RichEdit).
$tests = cargo test --locked --no-run --message-format=json 2>$null | ForEach-Object { try { $_ | ConvertFrom-Json } catch { $null } } |
    Where-Object { $_.reason -eq 'compiler-artifact' -and $_.executable -and $_.profile.test } | ForEach-Object { $_.executable }
if (-not $tests) { throw "Test build failed" }
cargo test --locked
if ($LASTEXITCODE -ne 0) { throw "Tests failed" }

# 2) The program itself: the self-test on a live window.
cargo build --locked
if ($LASTEXITCODE -ne 0) { throw "Build failed" }
$exe = Join-Path $env:CARGO_TARGET_DIR 'debug\vibe-md-viewer.exe'
& (Join-Path $PSScriptRoot 'selftest.ps1') -Exe $exe
if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne $null) { throw "Self-test failed" }
# The service modes: chunked loading and theme/mode/zoom/scroll switches on a large document
# (--profile), a window snapshot, startup timing, the setup wizard window (snapshot only —
# nothing is installed).
$shots = Join-Path $out 'shots'
New-Item -ItemType Directory -Force $shots | Out-Null
& (Join-Path $PSScriptRoot 'profile.ps1') -Exe $exe -Size 1300x900 | Out-Null
$runs = @(
    @('tests\fixtures\showcase.md', '--size', '1100x800', '--snapshot', (Join-Path $shots 'view.png')),
    @('tests\fixtures\showcase.md', '--mode', 'code', '--size', '1100x800', '--snapshot', (Join-Path $shots 'code.png')),
    @('tests\fixtures\attack.md', '--timing', (Join-Path $shots 'timing.txt'), '--exit-after-paint'),
    @('--setup', '--snapshot', (Join-Path $shots 'setup.png'))
)
foreach ($a in $runs) {
    $p = Start-Process -FilePath $exe -ArgumentList $a -PassThru
    if (-not $p.WaitForExit(60000)) { $p.Kill(); throw "Timed out: $($a -join ' ')" }
}

# 3) Merge and report (only our sources: src\).
& $profdata merge -sparse (Get-ChildItem $out -Filter *.profraw | ForEach-Object { $_.FullName }) -o (Join-Path $out 'cov.profdata')
$objects = @($exe) + @($tests) | Select-Object -Unique
$objArgs = @($objects[0]) + ($objects | Select-Object -Skip 1 | ForEach-Object { '-object'; $_ })
$ignore = '(\\|/)(\.cargo|\.toolcache|rustc|\.rustup|registry)(\\|/)'
$report = & $llvmcov report @objArgs "-instr-profile=$(Join-Path $out 'cov.profdata')" "-ignore-filename-regex=$ignore"
$report | Set-Content -Encoding utf8 (Join-Path $out 'summary.txt')
$report
if ($Html) {
    & $llvmcov show @objArgs "-instr-profile=$(Join-Path $out 'cov.profdata')" "-ignore-filename-regex=$ignore" -format=html "-output-dir=$(Join-Path $out 'html')" -show-line-counts-or-regions
    "HTML: $(Join-Path $out 'html\index.html')"
}
$total = $report | Where-Object { $_ -match '^TOTAL' } | Select-Object -First 1
if ($total -and $MinLines -gt 0) {
    # TOTAL  regions missed cover%  functions missed exec%  lines missed cover%  branches ...
    $pct = [double](($total -split '\s+' | Where-Object { $_ -match '%$' })[2].TrimEnd('%'))
    if ($pct -lt $MinLines) { throw "Line coverage $pct% is below the minimum $MinLines%" }
}
