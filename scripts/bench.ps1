# Startup time: from process creation to the first complete paint of the window.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\bench.ps1 [-File path.md] [-Runs 15]
param(
    [string]$File = "tests\fixtures\showcase.md",
    [int]$Runs = 15,
    [string]$Exe = "dist\vibe-md-viewer.exe"
)
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$tmpDir = Join-Path $root '.toolcache\tmp'
New-Item -ItemType Directory -Force $tmpDir | Out-Null
$out = Join-Path $tmpDir 'timing.txt'
Remove-Item $out -ErrorAction SilentlyContinue
$wall = @()
for ($i = 0; $i -lt $Runs; $i++) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $Exe -ArgumentList @($File, '--timing', $out, '--exit-after-paint') -PassThru
    $p.WaitForExit()
    $sw.Stop()
    $wall += $sw.Elapsed.TotalMilliseconds
}
$lines = Get-Content $out
$vals = $lines | ForEach-Object { [double]::Parse(($_ -split ' ')[0], [Globalization.CultureInfo]::InvariantCulture) } | Sort-Object
$wallS = $wall | Sort-Object
$median = $vals[[int]($vals.Count / 2)]
"File: $File, runs: $Runs"
"To first paint (ms): min {0:N1}, median {1:N1}, max {2:N1}" -f $vals[0], $median, $vals[-1]
"Full start+exit cycle (ms): min {0:N1}, median {1:N1}" -f $wallS[0], $wallS[[int]($wallS.Count / 2)]

"Last run by stage (ms since process start): " + ($lines[-1])
