# End-to-end self-test on a real window (it opens for about a second).
# Usage: powershell -ExecutionPolicy Bypass -File scripts\selftest.ps1 [-Exe target\debug\vibe-md-viewer.exe]
param([string]$Exe = "dist\vibe-md-viewer.exe")
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$dir = Join-Path $root '.toolcache\selftest'
New-Item -ItemType Directory -Force $dir | Out-Null
Copy-Item tests\fixtures\showcase.md (Join-Path $dir 'selftest.md') -Force
Copy-Item tests\fixtures\img (Join-Path $dir 'img') -Recurse -Force
$p = Start-Process -FilePath $Exe -ArgumentList @((Join-Path $dir 'selftest.md'), '--selftest') -PassThru
$p.WaitForExit(30000) | Out-Null
Get-Content (Join-Path $dir 'selftest.selftest.log') -Encoding UTF8
if ($p.ExitCode -eq 0) { "SELF-TEST: ALL OK" } else { "SELF-TEST: FAILURES (code $($p.ExitCode))"; exit 1 }
